//! The decompressor: the port of `miniz_tinfl.c`.
//!
//! The original is one function that can stop wherever its input or its room for output runs
//! out and go on from there when called again. Ported here is what it does when it is given
//! all of its input and all the room at once, which is `tinfl_decompress_mem_to_mem`: the
//! same steps in the same order, so that it takes and refuses the streams the original does
//! and writes the same bytes; where the original would stop to ask for more, this fails, as
//! `tinfl_decompress_mem_to_mem` does.
//!
//! It follows the original as built for a 64-bit processor (a 64-bit bit buffer), on every
//! machine.

// (The arithmetic is written as the original has it.)
#![allow(clippy::int_plus_one)]

use super::mz_adler32;

/// If set, the input has a valid zlib header and ends with an adler32 checksum (it's a valid
/// zlib stream). Otherwise, the input is a raw deflate stream.
pub const TINFL_FLAG_PARSE_ZLIB_HEADER: u32 = 1;
/// Force adler-32 checksum computation of the decompressed bytes.
pub const TINFL_FLAG_COMPUTE_ADLER32: u32 = 8;

const TINFL_MAX_HUFF_SYMBOLS_0: usize = 288;
const TINFL_MAX_HUFF_SYMBOLS_1: usize = 32;
const TINFL_MAX_HUFF_SYMBOLS_2: usize = 19;
const TINFL_FAST_LOOKUP_BITS: u32 = 10;
const TINFL_FAST_LOOKUP_SIZE: usize = 1 << TINFL_FAST_LOOKUP_BITS;

/// Why a stream was not decompressed: the statuses of `tinfl_status` that say so, and the two
/// that, with all the input and all the room given at once, say so too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TinflError {
    /// The stream ends before it is finished (`TINFL_STATUS_FAILED_CANNOT_MAKE_PROGRESS`).
    CannotMakeProgress,
    /// The stream is whole, but its checksum is not that of the bytes it holds
    /// (`TINFL_STATUS_ADLER32_MISMATCH`).
    Adler32Mismatch,
    /// The stream is not deflate, or is damaged (`TINFL_STATUS_FAILED`).
    Failed,
    /// The stream holds more bytes than there is room for (`TINFL_STATUS_HAS_MORE_OUTPUT`).
    HasMoreOutput,
}

static S_LENGTH_BASE: [u16; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 0, 0,
];
static S_LENGTH_EXTRA: [u8; 31] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0, 0, 0,
];
static S_DIST_BASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
];
static S_DIST_EXTRA: [u8; 32] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 0, 0,
];
static S_LENGTH_DEZIGZAG: [u8; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
static S_MIN_TABLE_SIZES: [u32; 3] = [257, 1, 4];

/// What the original keeps in `tinfl_decompressor` and in the locals of `tinfl_decompress`.
struct Decompressor<'a> {
    input: &'a [u8],
    /// `pIn_buf_cur`
    in_cur: usize,
    bit_buf: u64,
    num_bits: u32,
    out: Vec<u8>,
    /// `pOut_buf_end`, counted from the start.
    out_end: usize,
    m_look_up: [[i16; TINFL_FAST_LOOKUP_SIZE]; 3],
    m_tree_0: [i16; TINFL_MAX_HUFF_SYMBOLS_0 * 2],
    m_tree_1: [i16; TINFL_MAX_HUFF_SYMBOLS_1 * 2],
    m_tree_2: [i16; TINFL_MAX_HUFF_SYMBOLS_2 * 2],
    m_code_size_0: [u8; TINFL_MAX_HUFF_SYMBOLS_0],
    m_code_size_1: [u8; TINFL_MAX_HUFF_SYMBOLS_1],
    m_code_size_2: [u8; TINFL_MAX_HUFF_SYMBOLS_2],
    m_table_sizes: [u32; 3],
    m_len_codes: [u8; TINFL_MAX_HUFF_SYMBOLS_0 + TINFL_MAX_HUFF_SYMBOLS_1 + 137],
}

impl Decompressor<'_> {
    /// `TINFL_GET_BYTE`
    #[inline]
    fn get_byte(&mut self) -> Result<u32, TinflError> {
        let byte = *self
            .input
            .get(self.in_cur)
            .ok_or(TinflError::CannotMakeProgress)?;
        self.in_cur += 1;
        Ok(u32::from(byte))
    }

    /// `TINFL_NEED_BITS`
    #[inline]
    fn need_bits(&mut self, n: u32) -> Result<(), TinflError> {
        loop {
            let c = self.get_byte()?;
            self.bit_buf |= u64::from(c) << self.num_bits;
            self.num_bits += 8;
            if self.num_bits >= n {
                return Ok(());
            }
        }
    }

    /// `TINFL_SKIP_BITS`
    #[inline]
    fn skip_bits(&mut self, n: u32) -> Result<(), TinflError> {
        if self.num_bits < n {
            self.need_bits(n)?;
        }
        self.bit_buf >>= n;
        self.num_bits -= n;
        Ok(())
    }

    /// `TINFL_GET_BITS`
    #[inline]
    fn get_bits(&mut self, n: u32) -> Result<u32, TinflError> {
        if self.num_bits < n {
            self.need_bits(n)?;
        }
        let b = (self.bit_buf & ((1u64 << n) - 1)) as u32;
        self.bit_buf >>= n;
        self.num_bits -= n;
        Ok(b)
    }

    fn tree(&self, table: usize) -> &[i16] {
        match table {
            0 => &self.m_tree_0,
            1 => &self.m_tree_1,
            _ => &self.m_tree_2,
        }
    }

    /// `TINFL_HUFF_DECODE`: decodes the next Huffman coded symbol. It's more complex than you
    /// would initially expect because the zlib API expects the decompressor to never read
    /// beyond the final byte of the deflate stream. (In other words, when this wants to read
    /// another byte from the input, it REALLY needs another byte in order to fully decode the
    /// next Huffman code.) The slow path is only executed at the very end of the input buffer.
    #[inline]
    fn huff_decode(&mut self, table: usize) -> Result<u32, TinflError> {
        let mut temp: i32;
        let mut code_len: u32;
        if self.num_bits < 15 {
            if self.input.len() - self.in_cur < 2 {
                // TINFL_HUFF_BITBUF_FILL() is only used rarely, when the number of bytes
                // remaining in the input buffer falls below 2. It reads just enough bytes from
                // the input stream that are needed to decode the next Huffman code (and
                // absolutely no more). It works by trying to fully decode a Huffman code by
                // using whatever bits are currently present in the bit buffer. If this fails,
                // it reads another byte, and tries again until it succeeds or until the bit
                // buffer contains >=15 bits (deflate's max. Huffman code size).
                loop {
                    temp = i32::from(
                        self.m_look_up[table]
                            [(self.bit_buf & (TINFL_FAST_LOOKUP_SIZE as u64 - 1)) as usize],
                    );
                    if temp >= 0 {
                        code_len = (temp >> 9) as u32;
                        if code_len != 0 && self.num_bits >= code_len {
                            break;
                        }
                    } else if self.num_bits > TINFL_FAST_LOOKUP_BITS {
                        code_len = TINFL_FAST_LOOKUP_BITS;
                        loop {
                            temp = i32::from(
                                self.tree(table)
                                    [(!temp + ((self.bit_buf >> code_len) & 1) as i32) as usize],
                            );
                            code_len += 1;
                            if !(temp < 0 && self.num_bits >= code_len + 1) {
                                break;
                            }
                        }
                        if temp >= 0 {
                            break;
                        }
                    }
                    let c = self.get_byte()?;
                    self.bit_buf |= u64::from(c) << self.num_bits;
                    self.num_bits += 8;
                    if self.num_bits >= 15 {
                        break;
                    }
                }
            } else {
                self.bit_buf |= (u64::from(self.input[self.in_cur]) << self.num_bits)
                    | (u64::from(self.input[self.in_cur + 1]) << (self.num_bits + 8));
                self.in_cur += 2;
                self.num_bits += 16;
            }
        }
        temp = i32::from(
            self.m_look_up[table][(self.bit_buf & (TINFL_FAST_LOOKUP_SIZE as u64 - 1)) as usize],
        );
        if temp >= 0 {
            code_len = (temp >> 9) as u32;
            temp &= 511;
        } else {
            code_len = TINFL_FAST_LOOKUP_BITS;
            loop {
                temp = i32::from(
                    self.tree(table)[(!temp + ((self.bit_buf >> code_len) & 1) as i32) as usize],
                );
                code_len += 1;
                if temp >= 0 {
                    break;
                }
            }
        }
        self.bit_buf >>= code_len;
        self.num_bits = self.num_bits.wrapping_sub(code_len);
        Ok(temp as u32)
    }

    /// Puts a byte out, when there is room.
    #[inline]
    fn put(&mut self, byte: u8) -> Result<(), TinflError> {
        if self.out.len() >= self.out_end {
            return Err(TinflError::HasMoreOutput);
        }
        self.out.push(byte);
        Ok(())
    }

    /// Builds the look-up table and the tree of one of the three sets of codes from its code
    /// sizes.
    fn build_table(&mut self, table: usize) -> Result<(), TinflError> {
        let table_size = self.m_table_sizes[table] as usize;
        let mut next_code = [0u32; 17];
        let mut total_syms = [0u32; 16];
        self.m_look_up[table] = [0; TINFL_FAST_LOOKUP_SIZE];
        // tinfl_clear_tree
        match table {
            0 => self.m_tree_0 = [0; TINFL_MAX_HUFF_SYMBOLS_0 * 2],
            1 => self.m_tree_1 = [0; TINFL_MAX_HUFF_SYMBOLS_1 * 2],
            _ => self.m_tree_2 = [0; TINFL_MAX_HUFF_SYMBOLS_2 * 2],
        }
        let code_sizes: [u8; TINFL_MAX_HUFF_SYMBOLS_0] = {
            let mut sizes = [0u8; TINFL_MAX_HUFF_SYMBOLS_0];
            match table {
                0 => sizes.copy_from_slice(&self.m_code_size_0),
                1 => sizes[..TINFL_MAX_HUFF_SYMBOLS_1].copy_from_slice(&self.m_code_size_1),
                _ => sizes[..TINFL_MAX_HUFF_SYMBOLS_2].copy_from_slice(&self.m_code_size_2),
            }
            sizes
        };
        for &size in &code_sizes[..table_size] {
            total_syms[usize::from(size)] += 1;
        }
        let (mut used_syms, mut total) = (0u32, 0u32);
        for i in 1..=15 {
            used_syms += total_syms[i];
            total = (total + total_syms[i]) << 1;
            next_code[i + 1] = total;
        }
        if 65536 != total && used_syms > 1 {
            return Err(TinflError::Failed);
        }
        let mut tree_next: i32 = -1;
        let look_up = &mut self.m_look_up[table];
        let tree: &mut [i16] = match table {
            0 => &mut self.m_tree_0,
            1 => &mut self.m_tree_1,
            _ => &mut self.m_tree_2,
        };
        for sym_index in 0..table_size {
            let mut rev_code = 0u32;
            let code_size = u32::from(code_sizes[sym_index]);
            if code_size == 0 {
                continue;
            }
            let mut cur_code = next_code[code_size as usize];
            next_code[code_size as usize] += 1;
            for _ in 0..code_size {
                rev_code = (rev_code << 1) | (cur_code & 1);
                cur_code >>= 1;
            }
            if code_size <= TINFL_FAST_LOOKUP_BITS {
                let k = ((code_size << 9) | sym_index as u32) as i16;
                while (rev_code as usize) < TINFL_FAST_LOOKUP_SIZE {
                    look_up[rev_code as usize] = k;
                    rev_code += 1 << code_size;
                }
                continue;
            }
            let mut tree_cur = i32::from(look_up[rev_code as usize & (TINFL_FAST_LOOKUP_SIZE - 1)]);
            if tree_cur == 0 {
                look_up[rev_code as usize & (TINFL_FAST_LOOKUP_SIZE - 1)] = tree_next as i16;
                tree_cur = tree_next;
                tree_next -= 2;
            }
            rev_code >>= TINFL_FAST_LOOKUP_BITS - 1;
            let mut j = code_size;
            while j > TINFL_FAST_LOOKUP_BITS + 1 {
                rev_code >>= 1;
                tree_cur -= (rev_code & 1) as i32;
                let at = (-tree_cur - 1) as usize;
                if tree[at] == 0 {
                    tree[at] = tree_next as i16;
                    tree_cur = tree_next;
                    tree_next -= 2;
                } else {
                    tree_cur = i32::from(tree[at]);
                }
                j -= 1;
            }
            rev_code >>= 1;
            tree_cur -= (rev_code & 1) as i32;
            tree[(-tree_cur - 1) as usize] = sym_index as i16;
        }
        Ok(())
    }

    /// The body of `tinfl_decompress`. Gives back the checksum the stream ends with, when it
    /// is a zlib stream.
    fn decompress(&mut self, decomp_flags: u32) -> Result<u32, TinflError> {
        let mut m_z_adler32 = 1u32;
        if decomp_flags & TINFL_FLAG_PARSE_ZLIB_HEADER != 0 {
            let m_zhdr0 = self.get_byte()?;
            let m_zhdr1 = self.get_byte()?;
            if (m_zhdr0 * 256 + m_zhdr1) % 31 != 0 || (m_zhdr1 & 32) != 0 || (m_zhdr0 & 15) != 8 {
                return Err(TinflError::Failed);
            }
        }

        loop {
            let m_final = self.get_bits(3)?;
            let m_type = m_final >> 1;
            if m_type == 0 {
                self.skip_bits(self.num_bits & 7)?;
                let mut m_raw_header = [0u32; 4];
                for header in &mut m_raw_header {
                    *header = if self.num_bits != 0 {
                        self.get_bits(8)?
                    } else {
                        self.get_byte()?
                    };
                }
                let mut counter = m_raw_header[0] | (m_raw_header[1] << 8);
                if counter != (0xFFFF ^ (m_raw_header[2] | (m_raw_header[3] << 8))) {
                    return Err(TinflError::Failed);
                }
                while counter != 0 && self.num_bits != 0 {
                    let dist = self.get_bits(8)?;
                    self.put(dist as u8)?;
                    counter -= 1;
                }
                while counter != 0 {
                    if self.out.len() >= self.out_end {
                        return Err(TinflError::HasMoreOutput);
                    }
                    if self.in_cur >= self.input.len() {
                        return Err(TinflError::CannotMakeProgress);
                    }
                    let n = (self.out_end - self.out.len())
                        .min(self.input.len() - self.in_cur)
                        .min(counter as usize);
                    self.out
                        .extend_from_slice(&self.input[self.in_cur..self.in_cur + n]);
                    self.in_cur += n;
                    counter -= n as u32;
                }
            } else if m_type == 3 {
                return Err(TinflError::Failed);
            } else {
                if m_type == 1 {
                    self.m_table_sizes[0] = 288;
                    self.m_table_sizes[1] = 32;
                    self.m_code_size_1 = [5; 32];
                    let p = &mut self.m_code_size_0;
                    p[..=143].fill(8);
                    p[144..=255].fill(9);
                    p[256..=279].fill(7);
                    p[280..=287].fill(8);
                } else {
                    for counter in 0..3 {
                        self.m_table_sizes[counter] = self.get_bits([5, 5, 4][counter])?;
                        self.m_table_sizes[counter] += S_MIN_TABLE_SIZES[counter];
                    }
                    self.m_code_size_2 = [0; TINFL_MAX_HUFF_SYMBOLS_2];
                    for counter in 0..self.m_table_sizes[2] as usize {
                        let s = self.get_bits(3)?;
                        self.m_code_size_2[usize::from(S_LENGTH_DEZIGZAG[counter])] = s as u8;
                    }
                    self.m_table_sizes[2] = 19;
                }
                let mut table = m_type as i32;
                while table >= 0 {
                    self.build_table(table as usize)?;
                    if table == 2 {
                        let total = (self.m_table_sizes[0] + self.m_table_sizes[1]) as usize;
                        let mut counter = 0usize;
                        while counter < total {
                            let dist = self.huff_decode(2)?;
                            if dist < 16 {
                                self.m_len_codes[counter] = dist as u8;
                                counter += 1;
                                continue;
                            }
                            if dist == 16 && counter == 0 {
                                return Err(TinflError::Failed);
                            }
                            let num_extra = [2, 3, 7][dist as usize - 16];
                            let mut s = self.get_bits(num_extra)? as usize;
                            s += [3, 3, 11][dist as usize - 16];
                            let value = if dist == 16 {
                                self.m_len_codes[counter - 1]
                            } else {
                                0
                            };
                            self.m_len_codes[counter..counter + s].fill(value);
                            counter += s;
                        }
                        if total != counter {
                            return Err(TinflError::Failed);
                        }
                        let sizes0 = self.m_table_sizes[0] as usize;
                        let sizes1 = self.m_table_sizes[1] as usize;
                        self.m_code_size_0[..sizes0].copy_from_slice(&self.m_len_codes[..sizes0]);
                        self.m_code_size_1[..sizes1]
                            .copy_from_slice(&self.m_len_codes[sizes0..sizes0 + sizes1]);
                    }
                    table -= 1;
                }
                loop {
                    let mut counter: u32;
                    loop {
                        if self.input.len() - self.in_cur < 4 || self.out_end - self.out.len() < 2 {
                            counter = self.huff_decode(0)?;
                            if counter >= 256 {
                                break;
                            }
                            self.put(counter as u8)?;
                        } else {
                            let mut sym2: i32;
                            let mut code_len: u32;
                            if self.num_bits < 30 {
                                let word = u32::from_le_bytes([
                                    self.input[self.in_cur],
                                    self.input[self.in_cur + 1],
                                    self.input[self.in_cur + 2],
                                    self.input[self.in_cur + 3],
                                ]);
                                self.bit_buf |= u64::from(word) << self.num_bits;
                                self.in_cur += 4;
                                self.num_bits += 32;
                            }
                            sym2 = i32::from(
                                self.m_look_up[0]
                                    [(self.bit_buf & (TINFL_FAST_LOOKUP_SIZE as u64 - 1)) as usize],
                            );
                            if sym2 >= 0 {
                                code_len = (sym2 >> 9) as u32;
                            } else {
                                code_len = TINFL_FAST_LOOKUP_BITS;
                                loop {
                                    sym2 = i32::from(
                                        self.m_tree_0[(!sym2
                                            + ((self.bit_buf >> code_len) & 1) as i32)
                                            as usize],
                                    );
                                    code_len += 1;
                                    if sym2 >= 0 {
                                        break;
                                    }
                                }
                            }
                            counter = sym2 as u32;
                            self.bit_buf >>= code_len;
                            self.num_bits = self.num_bits.wrapping_sub(code_len);
                            if code_len == 0 {
                                return Err(TinflError::Failed);
                            }
                            if counter & 256 != 0 {
                                break;
                            }

                            sym2 = i32::from(
                                self.m_look_up[0]
                                    [(self.bit_buf & (TINFL_FAST_LOOKUP_SIZE as u64 - 1)) as usize],
                            );
                            if sym2 >= 0 {
                                code_len = (sym2 >> 9) as u32;
                            } else {
                                code_len = TINFL_FAST_LOOKUP_BITS;
                                loop {
                                    sym2 = i32::from(
                                        self.m_tree_0[(!sym2
                                            + ((self.bit_buf >> code_len) & 1) as i32)
                                            as usize],
                                    );
                                    code_len += 1;
                                    if sym2 >= 0 {
                                        break;
                                    }
                                }
                            }
                            self.bit_buf >>= code_len;
                            self.num_bits = self.num_bits.wrapping_sub(code_len);
                            if code_len == 0 {
                                return Err(TinflError::Failed);
                            }

                            self.out.push(counter as u8);
                            if sym2 & 256 != 0 {
                                counter = sym2 as u32;
                                break;
                            }
                            self.out.push(sym2 as u8);
                        }
                    }
                    counter &= 511;
                    if counter == 256 {
                        break;
                    }

                    let mut num_extra = u32::from(S_LENGTH_EXTRA[counter as usize - 257]);
                    counter = u32::from(S_LENGTH_BASE[counter as usize - 257]);
                    if num_extra != 0 {
                        let extra_bits = self.get_bits(num_extra)?;
                        counter += extra_bits;
                    }

                    let mut dist = self.huff_decode(1)?;
                    num_extra = u32::from(S_DIST_EXTRA[dist as usize]);
                    dist = u32::from(S_DIST_BASE[dist as usize]);
                    if num_extra != 0 {
                        let extra_bits = self.get_bits(num_extra)?;
                        dist += extra_bits;
                    }

                    let dist_from_out_buf_start = self.out.len();
                    if dist == 0
                        || dist as usize > dist_from_out_buf_start
                        || dist_from_out_buf_start == 0
                    {
                        return Err(TinflError::Failed);
                    }

                    let src = dist_from_out_buf_start - dist as usize;

                    if self.out.len() + counter as usize > self.out_end {
                        // (The original puts out what there is room for before it stops.)
                        return Err(TinflError::HasMoreOutput);
                    }
                    for i in 0..counter as usize {
                        let byte = self.out[src + i];
                        self.out.push(byte);
                    }
                }
            }
            if m_final & 1 != 0 {
                break;
            }
        }

        // Ensure byte alignment and put back any bytes from the bitbuf if we've looked
        // ahead too far on gzip, or other Deflate streams followed by arbitrary data.
        // I'm being super conservative here. A number of simplifications
        // can be made to the byte alignment part, and the Adler32 check
        // shouldn't ever need to worry about reading from the bitbuf now.
        self.skip_bits(self.num_bits & 7)?;
        while self.in_cur > 0 && self.num_bits >= 8 {
            self.in_cur -= 1;
            self.num_bits -= 8;
        }
        self.bit_buf &= !(!0u64).checked_shl(self.num_bits).unwrap_or(0);

        if decomp_flags & TINFL_FLAG_PARSE_ZLIB_HEADER != 0 {
            for _ in 0..4 {
                let s = if self.num_bits != 0 {
                    self.get_bits(8)?
                } else {
                    self.get_byte()?
                };
                m_z_adler32 = (m_z_adler32 << 8) | s;
            }
        }
        Ok(m_z_adler32)
    }
}

/// Decompresses a block in memory to no more than `out_buf_len` bytes, and says why when it
/// cannot: `tinfl_decompress` given all the input and all the room at once. With
/// [`TINFL_FLAG_PARSE_ZLIB_HEADER`] in `flags` the block is a zlib stream and its checksum
/// is checked; without, raw deflate. Whatever follows the stream is left alone.
///
/// Gives back the bytes and how much of the block the stream took.
pub fn tinfl_decompress(
    src_buf: &[u8],
    out_buf_len: usize,
    flags: u32,
) -> Result<(Vec<u8>, usize), TinflError> {
    let mut r = Decompressor {
        input: src_buf,
        in_cur: 0,
        bit_buf: 0,
        num_bits: 0,
        // (Room is taken as it is needed, so that a small stream that claims to be large
        // costs nothing.)
        out: Vec::new(),
        out_end: out_buf_len,
        m_look_up: [[0; TINFL_FAST_LOOKUP_SIZE]; 3],
        m_tree_0: [0; TINFL_MAX_HUFF_SYMBOLS_0 * 2],
        m_tree_1: [0; TINFL_MAX_HUFF_SYMBOLS_1 * 2],
        m_tree_2: [0; TINFL_MAX_HUFF_SYMBOLS_2 * 2],
        m_code_size_0: [0; TINFL_MAX_HUFF_SYMBOLS_0],
        m_code_size_1: [0; TINFL_MAX_HUFF_SYMBOLS_1],
        m_code_size_2: [0; TINFL_MAX_HUFF_SYMBOLS_2],
        m_table_sizes: [0; 3],
        m_len_codes: [0; TINFL_MAX_HUFF_SYMBOLS_0 + TINFL_MAX_HUFF_SYMBOLS_1 + 137],
    };
    let m_z_adler32 = r.decompress(flags)?;
    if flags & TINFL_FLAG_PARSE_ZLIB_HEADER != 0 && mz_adler32(1, &r.out) != m_z_adler32 {
        return Err(TinflError::Adler32Mismatch);
    }
    Ok((r.out, r.in_cur))
}

/// Decompresses a block in memory to no more than `out_buf_len` bytes:
/// `tinfl_decompress_mem_to_mem`. None on failure.
pub fn tinfl_decompress_mem_to_mem(
    src_buf: &[u8],
    out_buf_len: usize,
    flags: u32,
) -> Option<Vec<u8>> {
    tinfl_decompress(src_buf, out_buf_len, flags)
        .ok()
        .map(|(out, _)| out)
}
