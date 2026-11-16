//! The Huffman coder of OpenEXR's PIZ, as OpenEXRCore has it, for the AC coefficients of
//! DWAA and DWAB: a port of `internal_huf.c` of OpenEXR v3.5.4, with both of its decoders
//! (the table one for streams of 128 bits or less, the fast one, on x86, for longer ones).

use super::{Code, ARGUMENT_OUT_OF_RANGE, CORRUPT_CHUNK, INVALID_ARGUMENT, OUT_OF_MEMORY};

const HUF_ENCBITS: u32 = 16;
const HUF_DECBITS: u32 = 14;
const HUF_ENCSIZE: usize = (1 << HUF_ENCBITS) + 1;
const HUF_DECSIZE: usize = 1 << HUF_DECBITS;
const HUF_DECMASK: u64 = HUF_DECSIZE as u64 - 1;

const SHORT_ZEROCODE_RUN: u64 = 59;
const LONG_ZEROCODE_RUN: u64 = 63;
const SHORTEST_LONG_RUN: u64 = 2 + LONG_ZEROCODE_RUN - SHORT_ZEROCODE_RUN;
const LONGEST_LONG_RUN: u64 = 255 + SHORTEST_LONG_RUN;

fn huf_length(code: u64) -> u32 {
    (code & 63) as u32
}

fn huf_code(code: u64) -> u64 {
    code >> 6
}

/// `hufCanonicalCodeTable`: the lengths in `hcode` given their canonical codes.
fn canonical_code_table(hcode: &mut [u64]) {
    let mut n = [0u64; 59];
    for &length in hcode.iter() {
        n[length as usize] += 1;
    }
    let mut c = 0u64;
    for i in (1..=58).rev() {
        let nc = (c + n[i]) >> 1;
        n[i] = c;
        c = nc;
    }
    for code in hcode.iter_mut() {
        let l = *code;
        if l > 0 {
            *code = l | (n[l as usize] << 6);
            n[l as usize] += 1;
        }
    }
}

// The heap of `hufBuildEncTable`: indices into the frequencies, the least frequent (and of
// those, the highest index) on top.

fn heap_compare(frq: &[u64], a: usize, b: usize) -> bool {
    frq[a] > frq[b] || (frq[a] == frq[b] && a > b)
}

fn intern_push_heap(frq: &[u64], first: &mut [usize], mut hole_index: usize, top_index: usize, value: usize) {
    let mut parent = hole_index.wrapping_sub(1) / 2;
    while hole_index > top_index && heap_compare(frq, first[parent], value) {
        first[hole_index] = first[parent];
        hole_index = parent;
        parent = hole_index.wrapping_sub(1) / 2;
    }
    first[hole_index] = value;
}

fn adjust_heap(frq: &[u64], first: &mut [usize], mut hole_index: usize, len: usize, value: usize) {
    let top_index = hole_index;
    let mut second_child = hole_index;
    while second_child < (len - 1) / 2 {
        second_child = 2 * (second_child + 1);
        if heap_compare(frq, first[second_child], first[second_child - 1]) {
            second_child -= 1;
        }
        first[hole_index] = first[second_child];
        hole_index = second_child;
    }
    if len & 1 == 0 && second_child == (len - 2) / 2 {
        second_child = 2 * (second_child + 1);
        first[hole_index] = first[second_child - 1];
        hole_index = second_child - 1;
    }
    intern_push_heap(frq, first, hole_index, top_index, value);
}

fn push_heap(frq: &[u64], first: &mut [usize], last: usize) {
    let value = first[last - 1];
    intern_push_heap(frq, first, last - 1, 0, value);
}

fn pop_heap(frq: &[u64], first: &mut [usize], last: usize) {
    if last > 1 {
        let last = last - 1;
        let value = first[last];
        first[last] = first[0];
        adjust_heap(frq, first, 0, last, value);
    }
}

fn make_heap(frq: &[u64], first: &mut [usize], len: usize) {
    if len < 2 {
        return;
    }
    let mut parent = (len - 2) / 2;
    loop {
        let value = first[parent];
        adjust_heap(frq, first, parent, len, value);
        if parent == 0 {
            return;
        }
        parent -= 1;
    }
}

/// `hufBuildEncTable`: the codes of the frequencies `frq`, in place, with the symbol after
/// the last used one added for runs. The answer is the least and the most symbol used.
fn build_enc_table(frq: &mut [u64]) -> (u32, u32) {
    let mut hlink = vec![0u32; HUF_ENCSIZE];
    let mut fheap = vec![0usize; HUF_ENCSIZE];
    let mut nf = 0usize;
    let mut im = 0usize;
    while frq[im] == 0 {
        im += 1;
    }
    let mut i_max = 0usize;
    for i in im..HUF_ENCSIZE {
        hlink[i] = i as u32;
        if frq[i] != 0 {
            fheap[nf] = i;
            nf += 1;
            i_max = i;
        }
    }
    // A pseudo-symbol, with a frequency count of 1, for runs.
    i_max += 1;
    frq[i_max] = 1;
    fheap[nf] = i_max;
    nf += 1;

    make_heap(frq, &mut fheap, nf);
    let mut scode = vec![0u64; HUF_ENCSIZE];
    while nf > 1 {
        let mm = fheap[0];
        pop_heap(frq, &mut fheap, nf);
        nf -= 1;
        let m = fheap[0];
        pop_heap(frq, &mut fheap, nf);
        frq[m] += frq[mm];
        push_heap(frq, &mut fheap, nf);

        let mut j = m;
        loop {
            scode[j] += 1;
            if hlink[j] as usize == j {
                hlink[j] = mm as u32;
                break;
            }
            j = hlink[j] as usize;
        }
        let mut j = mm;
        loop {
            scode[j] += 1;
            if hlink[j] as usize == j {
                break;
            }
            j = hlink[j] as usize;
        }
    }
    canonical_code_table(&mut scode);
    frq.copy_from_slice(&scode);
    (im as u32, i_max as u32)
}

/// Bits written most significant first (`outputBits`), into `out` of no more than `max`.
struct BitWriter {
    out: Vec<u8>,
    max: usize,
    c: u64,
    lc: u32,
}

impl BitWriter {
    fn output_bits(&mut self, n_bits: u32, bits: u64) -> Result<(), Code> {
        self.c = (self.c << n_bits) | bits;
        self.lc += n_bits;
        while self.lc >= 8 {
            if self.out.len() >= self.max {
                return Err(ARGUMENT_OUT_OF_RANGE);
            }
            self.lc -= 8;
            self.out.push((self.c >> self.lc) as u8);
        }
        Ok(())
    }

    fn output_code(&mut self, code: u64) -> Result<(), Code> {
        self.output_bits(huf_length(code), huf_code(code))
    }

    /// `sendCode`.
    fn send_code(&mut self, s_code: u64, run_count: i32, run_code: u64) -> Result<(), Code> {
        if ((huf_length(s_code) + huf_length(run_code) + 8) as i32) < huf_length(s_code) as i32 * run_count {
            self.output_code(s_code)?;
            self.output_code(run_code)?;
            self.output_bits(8, run_count as u64)?;
        } else {
            let mut run_count = run_count;
            while run_count >= 0 {
                self.output_code(s_code)?;
                run_count -= 1;
            }
        }
        Ok(())
    }

    /// The last bits, as a byte filled out with zeros.
    fn finish(&mut self) -> Result<(), Code> {
        if self.lc > 0 {
            if self.out.len() >= self.max {
                return Err(ARGUMENT_OUT_OF_RANGE);
            }
            self.out.push((self.c << (8 - self.lc)) as u8);
        }
        Ok(())
    }
}

/// `hufPackEncTable`: the lengths of the codes from `im` to `i_max`.
fn pack_enc_table(hcode: &[u64], mut im: u32, i_max: u32, w: &mut BitWriter) -> Result<(), Code> {
    while im <= i_max {
        let l = huf_length(hcode[im as usize]);
        if l == 0 {
            let mut zerun = 1u64;
            while im < i_max && zerun < LONGEST_LONG_RUN {
                if huf_length(hcode[im as usize + 1]) > 0 {
                    break;
                }
                im += 1;
                zerun += 1;
            }
            if zerun >= 2 {
                if zerun >= SHORTEST_LONG_RUN {
                    w.output_bits(6, LONG_ZEROCODE_RUN)?;
                    w.output_bits(8, zerun - SHORTEST_LONG_RUN)?;
                } else {
                    w.output_bits(6, SHORT_ZEROCODE_RUN + zerun - 2)?;
                }
                im += 1;
                continue;
            }
        }
        w.output_bits(6, u64::from(l))?;
        im += 1;
    }
    w.finish()
}

/// `internal_huf_compress`: `raw` Huffman-coded into no more than `outsz` bytes.
pub(super) fn compress(raw: &[u16], outsz: usize) -> Result<Vec<u8>, Code> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    if outsz < 20 {
        return Err(INVALID_ARGUMENT);
    }
    let mut freq = vec![0u64; HUF_ENCSIZE];
    for &value in raw {
        freq[value as usize] += 1;
    }
    let (im, i_max) = build_enc_table(&mut freq);

    let mut w = BitWriter { out: vec![0u8; 20], max: outsz, c: 0, lc: 0 };
    pack_enc_table(&freq, im, i_max, &mut w)?;
    let table_length = w.out.len() - 20;

    // hufEncode
    let data_start = w.out.len();
    w.c = 0;
    w.lc = 0;
    let run_code = freq[i_max as usize];
    let mut s = u32::from(raw[0]);
    let mut cs = 0i32;
    for &ns in &raw[1..] {
        let ns = u32::from(ns);
        if cs == 255 || s != ns {
            w.send_code(freq[s as usize], cs, run_code)?;
            cs = 0;
            s = ns;
        } else {
            cs += 1;
        }
    }
    w.send_code(freq[s as usize], cs, run_code)?;
    let lc = w.lc;
    if lc != 0 {
        if w.out.len() >= w.max {
            return Err(ARGUMENT_OUT_OF_RANGE);
        }
        w.out.push((w.c << (8 - lc)) as u8);
    }
    let n_bits = ((w.out.len() - data_start - usize::from(lc != 0)) * 8) as u64 + u64::from(lc);
    if n_bits > u64::from(u32::MAX) {
        return Err(ARGUMENT_OUT_OF_RANGE);
    }
    let mut out = w.out;
    out[0..4].copy_from_slice(&im.to_le_bytes());
    out[4..8].copy_from_slice(&i_max.to_le_bytes());
    out[8..12].copy_from_slice(&(table_length as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(n_bits as u32).to_le_bytes());
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// The table decoder

/// `getBits`: bits most significant first, from `input` at `*at`.
fn get_bits(n_bits: u32, c: &mut u64, lc: &mut u32, input: &[u8], at: &mut usize) -> u64 {
    while *lc < n_bits {
        *c = (*c << 8) | u64::from(input[*at]);
        *at += 1;
        *lc += 8;
    }
    *lc -= n_bits;
    (*c >> *lc) & ((1u64 << n_bits) - 1)
}

/// `hufUnpackEncTable`: the lengths of the codes from `im` to `i_max`, from `input` at
/// `*at`, into `hcode`, as codes. (Where it stops short, as the original's caller does not
/// look, the lengths so far are left as they are, without their codes, and `*at` too.)
fn unpack_enc_table(input: &[u8], at: &mut usize, n_left: &mut u64, mut im: u32, i_max: u32, hcode: &mut [u64]) -> Result<(), Code> {
    let start = *at;
    let mut p = *at;
    let mut c = 0u64;
    let ni = *n_left;
    let mut lc = 0u32;
    hcode.iter_mut().for_each(|code| *code = 0);
    while im <= i_max {
        let nr = (p - start) as u64;
        if lc < 6 && nr >= ni {
            return Err(OUT_OF_MEMORY);
        }
        let l = get_bits(6, &mut c, &mut lc, input, &mut p);
        hcode[im as usize] = l;
        if l == LONG_ZEROCODE_RUN {
            let nr = (p - start) as u64;
            if lc < 8 && nr >= ni {
                return Err(OUT_OF_MEMORY);
            }
            let zerun = get_bits(8, &mut c, &mut lc, input, &mut p) + SHORTEST_LONG_RUN;
            if u64::from(im) + zerun > u64::from(i_max) + 1 {
                return Err(CORRUPT_CHUNK);
            }
            for _ in 0..zerun {
                hcode[im as usize] = 0;
                im += 1;
            }
            im -= 1;
        } else if l >= SHORT_ZEROCODE_RUN {
            let zerun = l - SHORT_ZEROCODE_RUN + 2;
            if u64::from(im) + zerun > u64::from(i_max) + 1 {
                return Err(CORRUPT_CHUNK);
            }
            for _ in 0..zerun {
                hcode[im as usize] = 0;
                im += 1;
            }
            im -= 1;
        }
        im += 1;
    }
    *n_left -= (p - start) as u64;
    *at = p;
    canonical_code_table(hcode);
    Ok(())
}

/// `HufDec`: a short code's length and symbol, or the symbols of the long codes that start
/// with these bits.
#[derive(Clone, Default)]
struct HufDec {
    len: u32,
    lit: u32,
    p: Option<Vec<u32>>,
}

/// `hufBuildDecTable`.
fn build_dec_table(hcode: &[u64], mut im: u32, i_max: u32, hdecod: &mut [HufDec]) -> Result<(), Code> {
    while im <= i_max {
        let c = huf_code(hcode[im as usize]);
        let l = huf_length(hcode[im as usize]);
        if l < 64 && (c >> l) != 0 {
            return Err(CORRUPT_CHUNK);
        }
        if l > HUF_DECBITS {
            let pl = &mut hdecod[(c >> (l - HUF_DECBITS)) as usize];
            if pl.len != 0 {
                return Err(CORRUPT_CHUNK);
            }
            pl.lit += 1;
            pl.p.get_or_insert_with(Vec::new).push(im);
        } else if l != 0 {
            let start = (c << (HUF_DECBITS - l)) as usize;
            for pl in &mut hdecod[start..start + (1usize << (HUF_DECBITS - l))] {
                if pl.len != 0 || pl.p.is_some() {
                    return Err(CORRUPT_CHUNK);
                }
                pl.len = l;
                pl.lit = im;
            }
        }
        im += 1;
    }
    Ok(())
}

/// `getCode`: a symbol out, or a run of the one before.
#[allow(clippy::too_many_arguments)]
fn get_code(po: u32, rlc: u32, c: &mut u64, lc: &mut i32, input: &[u8], at: &mut usize, ie: usize, out: &mut [u16], o: &mut usize) -> Result<(), Code> {
    if po == rlc {
        if *lc < 8 {
            if *at >= ie {
                return Err(OUT_OF_MEMORY);
            }
            *c = (*c << 8) | u64::from(input[*at]);
            *at += 1;
            *lc += 8;
        }
        *lc -= 8;
        let cs = (*c >> *lc) as u8 as usize;
        if *o + cs > out.len() {
            return Err(CORRUPT_CHUNK);
        } else if *o == 0 {
            return Err(OUT_OF_MEMORY);
        }
        let s = out[*o - 1];
        out[*o..*o + cs].fill(s);
        *o += cs;
    } else if *o < out.len() {
        out[*o] = po as u16;
        *o += 1;
    } else {
        return Err(CORRUPT_CHUNK);
    }
    Ok(())
}

/// `hufDecode`.
fn decode(hcode: &[u64], hdecod: &[HufDec], input: &[u8], start: usize, ni: u64, rlc: u32, out: &mut [u16]) -> Result<(), Code> {
    let mut c = 0u64;
    let mut lc = 0i32;
    let mut o = 0usize;
    let mut at = start;
    let ie = start + ni.div_ceil(8) as usize;

    while at < ie {
        c = (c << 8) | u64::from(input[at]);
        at += 1;
        lc += 8;
        while lc >= HUF_DECBITS as i32 {
            let decoffset = (c >> (lc - HUF_DECBITS as i32)) & HUF_DECMASK;
            let pl = &hdecod[decoffset as usize];
            if pl.len != 0 {
                if pl.len as i32 > lc {
                    return Err(CORRUPT_CHUNK);
                }
                lc -= pl.len as i32;
                get_code(pl.lit, rlc, &mut c, &mut lc, input, &mut at, ie, out, &mut o)?;
            } else {
                let Some(decbuf) = &pl.p else { return Err(CORRUPT_CHUNK) };
                let mut found = false;
                for &sym in decbuf.iter().take(pl.lit as usize) {
                    let l = huf_length(hcode[sym as usize]) as i32;
                    while lc < l && at < ie {
                        c = (c << 8) | u64::from(input[at]);
                        at += 1;
                        lc += 8;
                    }
                    if lc >= l && huf_code(hcode[sym as usize]) == ((c >> (lc - l)) & ((1u64 << l) - 1)) {
                        lc -= l;
                        get_code(sym, rlc, &mut c, &mut lc, input, &mut at, ie, out, &mut o)?;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(CORRUPT_CHUNK);
                }
            }
        }
    }

    // Get remaining (short) codes
    let i = (8u64.wrapping_sub(ni)) & 7;
    c >>= i;
    lc -= i as i32;
    while lc > 0 {
        let decoffset = (c << (HUF_DECBITS as i32 - lc)) & HUF_DECMASK;
        let pl = &hdecod[decoffset as usize];
        if pl.len != 0 {
            if pl.len as i32 > lc {
                return Err(CORRUPT_CHUNK);
            }
            lc -= pl.len as i32;
            get_code(pl.lit, rlc, &mut c, &mut lc, input, &mut at, ie, out, &mut o)?;
        } else {
            return Err(CORRUPT_CHUNK);
        }
    }
    if o != out.len() {
        return Err(OUT_OF_MEMORY);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The fast decoder

const MAX_CODE_LEN: usize = 58;
const TABLE_LOOKUP_BITS: u32 = 12;
const INDEX_BIT_SHIFT: u32 = 64 - TABLE_LOOKUP_BITS;

/// `FastHufDecoder`.
struct FastHufDecoder {
    rle_symbol: i32,
    num_symbols: u32,
    min_code_length: u8,
    max_code_length: u8,
    id_to_symbol: Vec<u32>,
    lj_base: [u64; MAX_CODE_LEN + 2],
    lj_offset: [u64; MAX_CODE_LEN + 1],
    table_symbol: Vec<i32>,
    table_code_len: Vec<u8>,
    table_min: u64,
}

fn read64(input: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(input[at..at + 8].try_into().expect("eight bytes"))
}

/// `fasthuf_read_bits`.
fn fast_read_bits(num_bits: u32, buffer: &mut u64, buffer_num_bits: &mut u32, input: &[u8], at: &mut usize) -> u64 {
    while *buffer_num_bits < num_bits {
        *buffer = (*buffer << 8) | u64::from(input[*at]);
        *at += 1;
        *buffer_num_bits += 8;
    }
    *buffer_num_bits -= num_bits;
    (*buffer >> *buffer_num_bits) & ((1u64 << num_bits) - 1)
}

impl FastHufDecoder {
    /// `fasthuf_initialize`: the code lengths from `input` at `*at`, of `num_bytes` at most.
    fn initialize(input: &[u8], at: &mut usize, num_bytes: u64, min_symbol: u32, max_symbol: u32, rle_symbol: i32) -> Result<FastHufDecoder, Code> {
        let mut base = [u64::MAX; MAX_CODE_LEN + 1];
        let mut offset = [0u64; MAX_CODE_LEN + 1];
        let mut code_count = [0usize; MAX_CODE_LEN + 1];
        let start = *at;
        let top_byte = start as u64 + num_bytes;
        let mut fhd = FastHufDecoder {
            rle_symbol,
            num_symbols: 0,
            min_code_length: 255,
            max_code_length: 0,
            id_to_symbol: vec![0; 65537],
            lj_base: [0; MAX_CODE_LEN + 2],
            lj_offset: [0; MAX_CODE_LEN + 1],
            table_symbol: vec![0; 1 << TABLE_LOOKUP_BITS],
            table_code_len: vec![0; 1 << TABLE_LOOKUP_BITS],
            table_min: 0,
        };

        let (mut curr, mut curr_bits, mut curr_bit_count) = (start, 0u64, 0u32);
        let mut symbol = u64::from(min_symbol);
        while symbol <= u64::from(max_symbol) {
            if curr as u64 >= top_byte {
                return Err(CORRUPT_CHUNK);
            }
            let code_len = fast_read_bits(6, &mut curr_bits, &mut curr_bit_count, input, &mut curr);
            if code_len < SHORT_ZEROCODE_RUN {
                if code_len == 0 {
                    symbol += 1;
                    continue;
                }
                fhd.min_code_length = fhd.min_code_length.min(code_len as u8);
                fhd.max_code_length = fhd.max_code_length.max(code_len as u8);
                code_count[code_len as usize] += 1;
            } else if code_len == LONG_ZEROCODE_RUN {
                if curr as u64 >= top_byte {
                    return Err(CORRUPT_CHUNK);
                }
                symbol += fast_read_bits(8, &mut curr_bits, &mut curr_bit_count, input, &mut curr) + SHORTEST_LONG_RUN - 1;
            } else {
                symbol += code_len - SHORT_ZEROCODE_RUN + 1;
            }
            if symbol > u64::from(max_symbol) {
                return Err(CORRUPT_CHUNK);
            }
            symbol += 1;
        }

        for &count in &code_count[..MAX_CODE_LEN] {
            fhd.num_symbols = fhd.num_symbols.wrapping_add(count as u32);
        }
        if fhd.num_symbols as usize > fhd.id_to_symbol.len() {
            return Err(CORRUPT_CHUNK);
        }

        // Compute base - once we have the code length counts, there is a closed form
        // solution for this (in doubles, kept in `offset`).
        let (min_len, max_len) = (fhd.min_code_length as usize, fhd.max_code_length as usize);
        let mut count_tmp = [0f64; MAX_CODE_LEN + 1];
        for l in min_len..=max_len {
            count_tmp[l] = code_count[l] as f64 * (2i64 << (max_len - l)) as f64;
        }
        for l in min_len..=max_len {
            let mut tmp = 0f64;
            for &count in &count_tmp[l + 1..=max_len] {
                tmp += count;
            }
            tmp /= (2i64 << (max_len - l)) as f64;
            base[l] = tmp.ceil() as u64;
        }

        // Compute offset - these are the positions of the first id (not symbol) that has
        // length [i]
        offset[max_len] = 0;
        let mut i = max_len as isize - 1;
        while i >= min_len as isize {
            offset[i as usize] = offset[i as usize + 1] + code_count[i as usize + 1] as u64;
            i -= 1;
        }

        let mut mapping = [u64::MAX; MAX_CODE_LEN + 1];
        // (With no code at all, the lengths go from 255 down to 0, and nothing is copied.)
        #[allow(clippy::manual_memcpy)]
        for l in min_len..=max_len {
            mapping[l] = offset[l];
        }
        let (mut curr, mut curr_bits, mut curr_bit_count) = (start, 0u64, 0u32);
        let mut symbol = u64::from(min_symbol);
        while symbol <= u64::from(max_symbol) {
            let code_len = fast_read_bits(6, &mut curr_bits, &mut curr_bit_count, input, &mut curr);
            if code_len < SHORT_ZEROCODE_RUN {
                if code_len == 0 {
                    symbol += 1;
                    continue;
                }
                if mapping[code_len as usize] >= u64::from(fhd.num_symbols) {
                    return Err(CORRUPT_CHUNK);
                }
                fhd.id_to_symbol[mapping[code_len as usize] as usize] = symbol as u32;
                mapping[code_len as usize] += 1;
            } else if code_len == LONG_ZEROCODE_RUN {
                symbol += fast_read_bits(8, &mut curr_bits, &mut curr_bit_count, input, &mut curr) + SHORTEST_LONG_RUN - 1;
            } else {
                symbol += code_len - SHORT_ZEROCODE_RUN + 1;
            }
            symbol += 1;
        }
        *at = curr;
        fhd.build_tables(&base, &offset)?;
        Ok(fhd)
    }

    /// `FastHufDecoder_buildTables`.
    fn build_tables(&mut self, base: &[u64; MAX_CODE_LEN + 1], offset: &[u64; MAX_CODE_LEN + 1]) -> Result<(), Code> {
        for i in 0..=MAX_CODE_LEN {
            self.lj_base[i] = if base[i] != u64::MAX { base[i].wrapping_shl(64 - i as u32) } else { u64::MAX };
        }
        self.lj_base[MAX_CODE_LEN + 1] = 0;

        self.lj_offset[0] = offset[0].wrapping_sub(self.lj_base[0]);
        for i in 1..=MAX_CODE_LEN {
            self.lj_offset[i] = offset[i].wrapping_sub(self.lj_base[i] >> (64 - i));
        }

        for i in 0..1u64 << TABLE_LOOKUP_BITS {
            let value = i << INDEX_BIT_SHIFT;
            self.table_symbol[i as usize] = 0xffff;
            self.table_code_len[i as usize] = 0;
            for code_len in self.min_code_length as usize..=self.max_code_length as usize {
                if self.lj_base[code_len] <= value {
                    self.table_code_len[i as usize] = code_len as u8;
                    let id = self.lj_offset[code_len].wrapping_add(value >> (64 - code_len));
                    if id < u64::from(self.num_symbols) {
                        self.table_symbol[i as usize] = self.id_to_symbol[id as usize] as i32;
                    } else {
                        return Err(CORRUPT_CHUNK);
                    }
                    break;
                }
            }
        }

        let mut min_idx = TABLE_LOOKUP_BITS as usize;
        while min_idx > 0 && self.lj_base[min_idx] == u64::MAX {
            min_idx -= 1;
        }
        self.table_min = self.lj_base[min_idx];
        Ok(())
    }
}

/// `FastHufDecoder_refill`.
fn refill(buffer: &mut u64, mut num_bits: i32, buffer_back: &mut u64, buffer_back_num_bits: &mut i32, input: &[u8], curr: &mut usize, curr_bits_left: &mut u64) {
    loop {
        if num_bits > 0 {
            let need_bits = 64 - num_bits;
            *buffer |= *buffer_back >> num_bits;
            if *buffer_back_num_bits >= need_bits {
                *buffer_back = buffer_back.wrapping_shl(need_bits as u32);
                *buffer_back_num_bits -= need_bits;
                break;
            }
            num_bits += *buffer_back_num_bits;
            *buffer_back_num_bits = 0;
        } else {
            *buffer = *buffer_back;
            num_bits = *buffer_back_num_bits;
            *buffer_back_num_bits = 0;
        }
        if *buffer_back_num_bits <= 0 {
            if *curr_bits_left >= 64 {
                *buffer_back = read64(input, *curr);
                *buffer_back_num_bits = 64;
                *curr += 8;
                *curr_bits_left -= 64;
            } else {
                let mut shift = 56u32;
                *buffer_back = 0;
                *buffer_back_num_bits = 64;
                while *curr_bits_left >= 8 {
                    *buffer_back |= u64::from(input[*curr]) << shift;
                    *curr += 1;
                    shift = shift.wrapping_sub(8);
                    *curr_bits_left -= 8;
                }
                if *curr_bits_left > 0 {
                    *buffer_back |= u64::from(input[*curr]).wrapping_shl(shift);
                    *curr += 1;
                    *curr_bits_left = 0;
                }
            }
        }
        if num_bits >= 64 {
            break;
        }
    }
}

impl FastHufDecoder {
    /// `fasthuf_decode`.
    fn decode(&self, input: &[u8], src: usize, num_src_bits: u64, dst: &mut [u16]) -> Result<(), Code> {
        let mut curr = src + 16;
        let mut num_src_bits = num_src_bits - 128;
        let mut buffer = read64(input, src);
        let mut buffer_num_bits = 64i32;
        let mut buffer_back = read64(input, src + 8);
        let mut buffer_back_num_bits = 64i32;
        let mut dst_idx = 0usize;
        let t_min = self.table_min;
        let rle_sym = self.rle_symbol as u32;

        while dst_idx < dst.len() {
            let (symbol, code_len);
            if t_min <= buffer {
                let table_idx = (buffer >> INDEX_BIT_SHIFT) as usize;
                code_len = u32::from(self.table_code_len[table_idx]);
                symbol = self.table_symbol[table_idx];
            } else {
                if buffer_num_bits < 64 {
                    refill(&mut buffer, buffer_num_bits, &mut buffer_back, &mut buffer_back_num_bits, input, &mut curr, &mut num_src_bits);
                    buffer_num_bits = 64;
                }
                let mut len = TABLE_LOOKUP_BITS as usize + 1;
                while self.lj_base[len] > buffer {
                    len += 1;
                }
                if len > self.max_code_length as usize {
                    return Err(CORRUPT_CHUNK);
                }
                let id = self.lj_offset[len].wrapping_add(buffer >> (64 - len));
                if id < u64::from(self.num_symbols) {
                    symbol = self.id_to_symbol[id as usize] as i32;
                } else {
                    return Err(CORRUPT_CHUNK);
                }
                code_len = len as u32;
            }

            buffer = buffer.wrapping_shl(code_len);
            buffer_num_bits -= code_len as i32;

            if symbol == rle_sym as i32 {
                if buffer_num_bits < 8 {
                    refill(&mut buffer, buffer_num_bits, &mut buffer_back, &mut buffer_back_num_bits, input, &mut curr, &mut num_src_bits);
                    buffer_num_bits = 64;
                }
                let rle_count = (buffer >> 56) as u32;
                if dst_idx < 1 {
                    return Err(CORRUPT_CHUNK);
                }
                if dst_idx as u64 + u64::from(rle_count) > dst.len() as u64 {
                    return Err(CORRUPT_CHUNK);
                }
                if rle_count == 0 || rle_count >= i32::MAX as u32 {
                    return Err(CORRUPT_CHUNK);
                }
                let previous = dst[dst_idx - 1];
                dst[dst_idx..dst_idx + rle_count as usize].fill(previous);
                dst_idx += rle_count as usize;
                buffer <<= 8;
                buffer_num_bits -= 8;
            } else {
                dst[dst_idx] = symbol as u16;
                dst_idx += 1;
            }

            if buffer_num_bits < TABLE_LOOKUP_BITS as i32 {
                refill(&mut buffer, buffer_num_bits, &mut buffer_back, &mut buffer_back_num_bits, input, &mut curr, &mut num_src_bits);
                buffer_num_bits = 64;
            }
        }
        if num_src_bits != 0 {
            return Err(CORRUPT_CHUNK);
        }
        Ok(())
    }
}

/// `internal_huf_decompress`: `compressed` into `raw`.
pub(super) fn decompress(compressed: &[u8], raw: &mut [u16]) -> Result<(), Code> {
    let n_compressed = compressed.len() as u64;
    let huf_info_block_size = 20u64;
    if n_compressed < 20 {
        if !raw.is_empty() {
            return Err(INVALID_ARGUMENT);
        }
        return Ok(());
    }
    let read_uint = |at: usize| u32::from_le_bytes(compressed[at..at + 4].try_into().expect("four bytes"));
    let im = read_uint(0);
    let i_max = read_uint(4);
    let n_bits = read_uint(12);
    if im as usize >= HUF_ENCSIZE || i_max as usize >= HUF_ENCSIZE {
        return Err(CORRUPT_CHUNK);
    }
    let mut ptr = 20usize;
    let n_bytes = u64::from(n_bits).div_ceil(8);
    if huf_info_block_size + n_bytes > n_compressed {
        return Err(OUT_OF_MEMORY);
    }

    if n_bits > 128 {
        let fhd = FastHufDecoder::initialize(compressed, &mut ptr, n_compressed - huf_info_block_size, im, i_max, i_max as i32)?;
        if ptr as u64 + n_bytes > n_compressed {
            return Err(OUT_OF_MEMORY);
        }
        fhd.decode(compressed, ptr, u64::from(n_bits), raw)
    } else {
        let mut freq = vec![0u64; HUF_ENCSIZE];
        let mut hdec = vec![HufDec::default(); HUF_DECSIZE];
        let mut n_left = n_compressed - 20;
        // (Its answer is not looked at.)
        let _ = unpack_enc_table(compressed, &mut ptr, &mut n_left, im, i_max, &mut freq);
        if u64::from(n_bits) > 8 * n_left {
            return Err(CORRUPT_CHUNK);
        }
        build_dec_table(&freq, im, i_max, &mut hdec)?;
        decode(&freq, &hdec, compressed, ptr, u64::from(n_bits), i_max, raw)
    }
}
