//! The compressor: a port of `deflate_compress.c` (with `hc_matchfinder.h` and
//! `matchfinder_common.h`) and `zlib_compress.c` of libdeflate v1.26, for its levels 0 and 2
//! to 9: no compression, and its greedy, lazy and lazy2 parsers over hash chains, each with
//! its splitting of blocks and its choice of the cheapest kind of block.
//!
//! (Level 1, with its own hash table, and the near-optimal levels 10 to 12 are not ported;
//! OpenEXR asks for 4 and 9.)

// (The arithmetic is written as the original has it.)
#![allow(clippy::int_plus_one, clippy::eq_op, clippy::manual_abs_diff)]

use super::adler32;

const MIN_MATCH_LEN: u32 = 3;
const MAX_MATCH_LEN: u32 = 258;
const NUM_LITLEN_SYMS: usize = 288;
const NUM_OFFSET_SYMS: usize = 32;
const NUM_PRECODE_SYMS: usize = 19;
const NUM_LITERALS: usize = 256;
const END_OF_BLOCK: usize = 256;
const FIRST_LEN_SYM: usize = 257;

const MIN_BLOCK_LENGTH: usize = 5000;
const SOFT_MAX_BLOCK_LENGTH: usize = 300000;
const SEQ_STORE_LENGTH: usize = 50000;

const MAX_LITLEN_CODEWORD_LEN: u32 = 14;
const MAX_OFFSET_CODEWORD_LEN: u32 = 15;
const MAX_PRE_CODEWORD_LEN: u32 = 7;

/// The highest level ported.
pub const MAX_LEVEL: i32 = 9;

const LENGTH_SLOT_BASE: [u32; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const EXTRA_LENGTH_BITS: [u32; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const OFFSET_SLOT_BASE: [u32; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const EXTRA_OFFSET_BITS: [u32; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const PRECODE_LENS_PERMUTATION: [usize; NUM_PRECODE_SYMS] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
const EXTRA_PRECODE_BITS: [u32; NUM_PRECODE_SYMS] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];

/// `deflate_length_slot`: the slot of each match length.
fn length_slot(length: u32) -> usize {
    LENGTH_SLOT_BASE.iter().rposition(|&base| base <= length).expect("a length of 3 or more")
}

/// `deflate_get_offset_slot`.
fn offset_slot(offset: u32) -> usize {
    OFFSET_SLOT_BASE.iter().rposition(|&base| base <= offset).expect("an offset of 1 or more")
}

// ---------------------------------------------------------------------------------------
// Huffman codes

const NUM_SYMBOL_BITS: u32 = 10;
const SYMBOL_MASK: u32 = (1 << NUM_SYMBOL_BITS) - 1;
const FREQ_MASK: u32 = !SYMBOL_MASK;

/// `heapify_subtree`, with 1-based indices into `a` (whose element 0 is unused).
fn heapify_subtree(a: &mut [u32], length: usize, subtree_idx: usize) {
    let v = a[subtree_idx];
    let mut parent_idx = subtree_idx;
    loop {
        let mut child_idx = parent_idx * 2;
        if child_idx > length {
            break;
        }
        if child_idx < length && a[child_idx + 1] > a[child_idx] {
            child_idx += 1;
        }
        if v >= a[child_idx] {
            break;
        }
        a[parent_idx] = a[child_idx];
        parent_idx = child_idx;
    }
    a[parent_idx] = v;
}

/// `heap_sort` of `a`.
fn heap_sort(a: &mut [u32]) {
    let mut length = a.len();
    // (1-based: a copy with a slot in front.)
    let mut one = Vec::with_capacity(length + 1);
    one.push(0);
    one.extend_from_slice(a);
    for subtree_idx in (1..=length / 2).rev() {
        heapify_subtree(&mut one, length, subtree_idx);
    }
    while length >= 2 {
        one.swap(1, length);
        length -= 1;
        heapify_subtree(&mut one, length, 1);
    }
    a.copy_from_slice(&one[1..]);
}

/// `sort_symbols`: the symbols of nonzero frequency, by frequency and then by symbol, each
/// with its frequency above it, into `symout`; zero lengths for the others. The answer is how
/// many there are.
fn sort_symbols(freqs: &[u32], lens: &mut [u8], symout: &mut [u32]) -> usize {
    let num_syms = freqs.len();
    let num_counters = num_syms;
    let mut counters = vec![0u32; num_counters];
    for &freq in freqs {
        counters[(freq as usize).min(num_counters - 1)] += 1;
    }
    let mut num_used_syms = 0u32;
    for counter in counters.iter_mut().skip(1) {
        let count = *counter;
        *counter = num_used_syms;
        num_used_syms += count;
    }
    for (sym, &freq) in freqs.iter().enumerate() {
        if freq != 0 {
            let at = &mut counters[(freq as usize).min(num_counters - 1)];
            symout[*at as usize] = sym as u32 | (freq << NUM_SYMBOL_BITS);
            *at += 1;
        } else {
            lens[sym] = 0;
        }
    }
    let (start, end) = (counters[num_counters - 2] as usize, counters[num_counters - 1] as usize);
    heap_sort(&mut symout[start..end]);
    num_used_syms as usize
}

/// `build_tree`: the non-leaf nodes of the Huffman tree of the `sym_count` sorted
/// frequencies in `a`, each with its parent's index.
fn build_tree(a: &mut [u32], sym_count: usize) {
    let last_idx = sym_count - 1;
    let (mut i, mut b, mut e) = (0usize, 0usize, 0usize);
    loop {
        let new_freq;
        if i + 1 <= last_idx && (b == e || (a[i + 1] & FREQ_MASK) <= (a[b] & FREQ_MASK)) {
            // Two leaves
            new_freq = (a[i] & FREQ_MASK).wrapping_add(a[i + 1] & FREQ_MASK);
            i += 2;
        } else if b + 2 <= e && (i > last_idx || (a[b + 1] & FREQ_MASK) < (a[i] & FREQ_MASK)) {
            // Two non-leaves
            new_freq = (a[b] & FREQ_MASK).wrapping_add(a[b + 1] & FREQ_MASK);
            a[b] = ((e as u32) << NUM_SYMBOL_BITS) | (a[b] & SYMBOL_MASK);
            a[b + 1] = ((e as u32) << NUM_SYMBOL_BITS) | (a[b + 1] & SYMBOL_MASK);
            b += 2;
        } else {
            // One leaf and one non-leaf
            new_freq = (a[i] & FREQ_MASK).wrapping_add(a[b] & FREQ_MASK);
            a[b] = ((e as u32) << NUM_SYMBOL_BITS) | (a[b] & SYMBOL_MASK);
            i += 1;
            b += 1;
        }
        a[e] = new_freq | (a[e] & SYMBOL_MASK);
        e += 1;
        if e >= last_idx {
            break;
        }
    }
}

/// `compute_length_counts`: how many codewords of each length, kept to `max_codeword_len`.
fn compute_length_counts(a: &mut [u32], root_idx: usize, len_counts: &mut [u32; 16], max_codeword_len: u32) {
    *len_counts = [0; 16];
    len_counts[1] = 2;
    a[root_idx] &= SYMBOL_MASK;
    for node in (0..root_idx).rev() {
        let parent = (a[node] >> NUM_SYMBOL_BITS) as usize;
        let parent_depth = a[parent] >> NUM_SYMBOL_BITS;
        let mut depth = parent_depth + 1;
        a[node] = (a[node] & SYMBOL_MASK) | (depth << NUM_SYMBOL_BITS);
        if depth >= max_codeword_len {
            depth = max_codeword_len;
            loop {
                depth -= 1;
                if len_counts[depth as usize] != 0 {
                    break;
                }
            }
        }
        len_counts[depth as usize] -= 1;
        len_counts[depth as usize + 1] += 2;
    }
}

/// `gen_codewords`: lengths to the symbols, by frequency, and their canonical codewords,
/// bit-reversed, into `a`.
fn gen_codewords(a: &mut [u32], lens: &mut [u8], len_counts: &[u32; 16], max_codeword_len: u32) {
    let mut i = 0;
    for len in (1..=max_codeword_len).rev() {
        for _ in 0..len_counts[len as usize] {
            lens[(a[i] & SYMBOL_MASK) as usize] = len as u8;
            i += 1;
        }
    }
    let mut next_codewords = [0u32; 16];
    for len in 2..=max_codeword_len as usize {
        next_codewords[len] = (next_codewords[len - 1] + len_counts[len - 1]) << 1;
    }
    for (sym, slot) in a.iter_mut().enumerate().take(lens.len()) {
        let len = u32::from(lens[sym]);
        let codeword = next_codewords[len as usize];
        next_codewords[len as usize] += 1;
        *slot = if len == 0 { 0 } else { codeword.reverse_bits() >> (32 - len) };
    }
}

/// `deflate_make_huffman_code`: a canonical code of `max_codeword_len` bits at most for
/// `freqs`, into `lens` and `codewords`.
fn make_huffman_code(max_codeword_len: u32, freqs: &[u32], lens: &mut [u8], codewords: &mut [u32]) {
    let num_used_syms = sort_symbols(freqs, lens, codewords);
    if num_used_syms < 2 {
        let sym = if num_used_syms != 0 { (codewords[0] & SYMBOL_MASK) as usize } else { 0 };
        let nonzero_idx = if sym != 0 { sym } else { 1 };
        codewords[0] = 0;
        lens[0] = 1;
        codewords[nonzero_idx] = 1;
        lens[nonzero_idx] = 1;
        return;
    }
    build_tree(codewords, num_used_syms);
    let mut len_counts = [0u32; 16];
    compute_length_counts(codewords, num_used_syms - 2, &mut len_counts, max_codeword_len);
    gen_codewords(codewords, lens, &len_counts, max_codeword_len);
}

/// `struct deflate_codes`: the lengths of both codes in one array, litlen then offset, as the
/// original lays them out (and moves them about).
#[derive(Clone)]
struct Codes {
    litlen_codewords: [u32; NUM_LITLEN_SYMS],
    offset_codewords: [u32; NUM_OFFSET_SYMS],
    lens: [u8; NUM_LITLEN_SYMS + NUM_OFFSET_SYMS],
}

#[derive(Clone)]
struct Freqs {
    litlen: [u32; NUM_LITLEN_SYMS],
    offset: [u32; NUM_OFFSET_SYMS],
}

/// `deflate_make_huffman_codes`.
fn make_huffman_codes(freqs: &Freqs, codes: &mut Codes) {
    let (litlen_lens, offset_lens) = codes.lens.split_at_mut(NUM_LITLEN_SYMS);
    make_huffman_code(MAX_LITLEN_CODEWORD_LEN, &freqs.litlen, litlen_lens, &mut codes.litlen_codewords);
    make_huffman_code(MAX_OFFSET_CODEWORD_LEN, &freqs.offset, offset_lens, &mut codes.offset_codewords);
}

// ---------------------------------------------------------------------------------------
// Output

/// `struct deflate_output_bitstream`, writing to a vector that has room enough.
struct Output {
    out: Vec<u8>,
    bitbuf: u64,
    bitcount: u32,
}

impl Output {
    fn add_bits(&mut self, bits: u32, n: u32) {
        self.bitbuf |= u64::from(bits) << self.bitcount;
        self.bitcount += n;
        while self.bitcount >= 8 {
            self.out.push(self.bitbuf as u8);
            self.bitbuf >>= 8;
            self.bitcount -= 8;
        }
    }
}

// ---------------------------------------------------------------------------------------
// The hash chains (`hc_matchfinder.h`)

const WINDOW_SIZE: usize = 1 << 15;
const HASH3_ORDER: u32 = 15;
const HASH4_ORDER: u32 = 16;
const INITVAL: i16 = i16::MIN;

struct HcMatchfinder {
    hash3_tab: Vec<i16>,
    hash4_tab: Vec<i16>,
    next_tab: Vec<i16>,
}

/// `lz_hash`.
fn lz_hash(seq: u32, num_bits: u32) -> u32 {
    seq.wrapping_mul(0x1E35_A7BD) >> (32 - num_bits)
}

fn load_u32(input: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([input[at], input[at + 1], input[at + 2], input[at + 3]])
}

/// `lz_extend`: how far `str_at` and `match_at` agree, from `start_len` to `max_len`.
fn lz_extend(input: &[u8], str_at: usize, match_at: usize, start_len: u32, max_len: u32) -> u32 {
    let mut len = start_len;
    while len < max_len && input[match_at + len as usize] == input[str_at + len as usize] {
        len += 1;
    }
    len
}

/// `matchfinder_rebase` for a window of 32768.
fn rebase(table: &mut [i16]) {
    for value in table {
        *value = (0x8000u16 as i16) | (*value & !(*value >> 15));
    }
}

impl HcMatchfinder {
    fn new() -> Self {
        HcMatchfinder { hash3_tab: vec![INITVAL; 1 << HASH3_ORDER], hash4_tab: vec![INITVAL; 1 << HASH4_ORDER], next_tab: vec![INITVAL; WINDOW_SIZE] }
    }

    fn slide_window(&mut self) {
        rebase(&mut self.hash3_tab);
        rebase(&mut self.hash4_tab);
        rebase(&mut self.next_tab);
    }

    /// `hc_matchfinder_longest_match`: the longest match at `in_next` longer than
    /// `best_len`, as its length and offset (`best_len` and no offset where there is none).
    #[allow(clippy::too_many_arguments)]
    fn longest_match(&mut self, input: &[u8], in_base: &mut usize, in_next: usize, mut best_len: u32, max_len: u32, nice_len: u32, max_search_depth: u32, next_hashes: &mut [u32; 2]) -> (u32, u32) {
        let mut depth_remaining = max_search_depth;
        let mut best_match = in_next;
        let mut cur_pos = (in_next - *in_base) as u32;

        if cur_pos as usize == WINDOW_SIZE {
            self.slide_window();
            *in_base += WINDOW_SIZE;
            cur_pos = 0;
        }
        let base = *in_base as isize;
        let cutoff = cur_pos.wrapping_sub(WINDOW_SIZE as u32) as i16;
        let at = |node: i16| (base + node as isize) as usize;

        'out: {
            if max_len < 5 {
                break 'out;
            }

            let hash3 = next_hashes[0] as usize;
            let hash4 = next_hashes[1] as usize;
            let cur_node3 = self.hash3_tab[hash3];
            let mut cur_node4 = self.hash4_tab[hash4];
            self.hash3_tab[hash3] = cur_pos as i16;
            self.hash4_tab[hash4] = cur_pos as i16;
            self.next_tab[cur_pos as usize] = cur_node4;

            let next_hashseq = load_u32(input, in_next + 1);
            next_hashes[0] = lz_hash(next_hashseq & 0xFF_FFFF, HASH3_ORDER);
            next_hashes[1] = lz_hash(next_hashseq, HASH4_ORDER);

            let next = |table: &[i16], node: i16| table[(node as u16 as usize) & (WINDOW_SIZE - 1)];

            if best_len < 4 {
                // Check for a length 3 match if needed.
                if cur_node3 <= cutoff {
                    break 'out;
                }
                let seq4 = load_u32(input, in_next);
                if best_len < 3 {
                    let match_at = at(cur_node3);
                    if load_u32(input, match_at) & 0xFF_FFFF == seq4 & 0xFF_FFFF {
                        best_len = 3;
                        best_match = match_at;
                    }
                }
                // Check for a length 4 match.
                if cur_node4 <= cutoff {
                    break 'out;
                }
                let mut match_at;
                loop {
                    match_at = at(cur_node4);
                    if load_u32(input, match_at) == seq4 {
                        break;
                    }
                    cur_node4 = next(&self.next_tab, cur_node4);
                    depth_remaining = depth_remaining.wrapping_sub(1);
                    if cur_node4 <= cutoff || depth_remaining == 0 {
                        break 'out;
                    }
                }
                // Found a match of length >= 4.  Extend it to its full length.
                best_match = match_at;
                best_len = lz_extend(input, in_next, best_match, 4, max_len);
                if best_len >= nice_len {
                    break 'out;
                }
                cur_node4 = next(&self.next_tab, cur_node4);
                depth_remaining = depth_remaining.wrapping_sub(1);
                if cur_node4 <= cutoff || depth_remaining == 0 {
                    break 'out;
                }
            } else if cur_node4 <= cutoff || best_len >= nice_len {
                break 'out;
            }

            // Check for matches of length >= 5.
            loop {
                let mut match_at;
                loop {
                    match_at = at(cur_node4);
                    let b = best_len as usize;
                    if load_u32(input, match_at + b - 3) == load_u32(input, in_next + b - 3) && load_u32(input, match_at) == load_u32(input, in_next) {
                        break;
                    }
                    cur_node4 = next(&self.next_tab, cur_node4);
                    depth_remaining = depth_remaining.wrapping_sub(1);
                    if cur_node4 <= cutoff || depth_remaining == 0 {
                        break 'out;
                    }
                }
                let len = lz_extend(input, in_next, match_at, 4, max_len);
                if len > best_len {
                    best_len = len;
                    best_match = match_at;
                    if best_len >= nice_len {
                        break 'out;
                    }
                }
                cur_node4 = next(&self.next_tab, cur_node4);
                depth_remaining = depth_remaining.wrapping_sub(1);
                if cur_node4 <= cutoff || depth_remaining == 0 {
                    break 'out;
                }
            }
        }
        (best_len, (in_next - best_match) as u32)
    }

    /// `hc_matchfinder_skip_bytes`: `count` positions from `in_next` put in the chains.
    fn skip_bytes(&mut self, input: &[u8], in_base: &mut usize, mut in_next: usize, count: u32, next_hashes: &mut [u32; 2]) {
        if count as usize + 5 > input.len() - in_next {
            return;
        }
        let mut cur_pos = in_next - *in_base;
        let mut hash3 = next_hashes[0];
        let mut hash4 = next_hashes[1];
        for _ in 0..count {
            if cur_pos == WINDOW_SIZE {
                self.slide_window();
                *in_base += WINDOW_SIZE;
                cur_pos = 0;
            }
            self.hash3_tab[hash3 as usize] = cur_pos as i16;
            self.next_tab[cur_pos] = self.hash4_tab[hash4 as usize];
            self.hash4_tab[hash4 as usize] = cur_pos as i16;
            in_next += 1;
            let next_hashseq = load_u32(input, in_next);
            hash3 = lz_hash(next_hashseq & 0xFF_FFFF, HASH3_ORDER);
            hash4 = lz_hash(next_hashseq, HASH4_ORDER);
            cur_pos += 1;
        }
        next_hashes[0] = hash3;
        next_hashes[1] = hash4;
    }
}

// ---------------------------------------------------------------------------------------
// Block splitting

const NUM_LITERAL_OBSERVATION_TYPES: usize = 8;
const NUM_OBSERVATION_TYPES: usize = NUM_LITERAL_OBSERVATION_TYPES + 2;
const NUM_OBSERVATIONS_PER_BLOCK_CHECK: u32 = 512;

#[derive(Default)]
struct SplitStats {
    new_observations: [u32; NUM_OBSERVATION_TYPES],
    observations: [u32; NUM_OBSERVATION_TYPES],
    num_new_observations: u32,
    num_observations: u32,
}

impl SplitStats {
    fn observe_literal(&mut self, lit: u8) {
        self.new_observations[(((lit >> 5) & 0x6) | (lit & 1)) as usize] += 1;
        self.num_new_observations += 1;
    }

    fn observe_match(&mut self, length: u32) {
        self.new_observations[NUM_LITERAL_OBSERVATION_TYPES + usize::from(length >= 9)] += 1;
        self.num_new_observations += 1;
    }

    fn merge_new_observations(&mut self) {
        for i in 0..NUM_OBSERVATION_TYPES {
            self.observations[i] += self.new_observations[i];
            self.new_observations[i] = 0;
        }
        self.num_observations += self.num_new_observations;
        self.num_new_observations = 0;
    }

    /// `do_end_block_check`.
    fn do_end_block_check(&mut self, block_length: u32) -> bool {
        if self.num_observations > 0 {
            let mut total_delta = 0u32;
            for i in 0..NUM_OBSERVATION_TYPES {
                let expected = self.observations[i].wrapping_mul(self.num_new_observations);
                let actual = self.new_observations[i].wrapping_mul(self.num_observations);
                let delta = if actual > expected { actual - expected } else { expected - actual };
                total_delta = total_delta.wrapping_add(delta);
            }
            let num_items = self.num_observations + self.num_new_observations;
            let mut cutoff = (self.num_new_observations.wrapping_mul(200) / 512).wrapping_mul(self.num_observations);
            if block_length < 10000 && num_items < 8192 {
                cutoff = cutoff.wrapping_add((u64::from(cutoff) * u64::from(8192 - num_items) / 8192) as u32);
            }
            if total_delta.wrapping_add((block_length / 4096).wrapping_mul(self.num_observations)) >= cutoff {
                return true;
            }
        }
        self.merge_new_observations();
        false
    }

    /// `should_end_block`.
    fn should_end_block(&mut self, in_block_begin: usize, in_next: usize, in_end: usize) -> bool {
        if !(self.num_new_observations >= NUM_OBSERVATIONS_PER_BLOCK_CHECK && in_next - in_block_begin >= MIN_BLOCK_LENGTH && in_end - in_next >= MIN_BLOCK_LENGTH) {
            return false;
        }
        self.do_end_block_check((in_next - in_block_begin) as u32)
    }
}

/// `choose_min_match_len`.
fn choose_min_match_len(num_used_literals: u32, max_search_depth: u32) -> u32 {
    const MIN_LENS: [u8; 80] = [
        9, 9, 9, 9, 9, 9, 8, 8, 7, 7, 6, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, 4, 4, 4, 4, 4, 4,
    ];
    if num_used_literals as usize >= MIN_LENS.len() {
        return 3;
    }
    let mut min_len = u32::from(MIN_LENS[num_used_literals as usize]);
    if max_search_depth < 16 {
        if max_search_depth < 5 {
            min_len = min_len.min(4);
        } else if max_search_depth < 10 {
            min_len = min_len.min(5);
        } else {
            min_len = min_len.min(7);
        }
    }
    min_len
}

/// `calculate_min_match_len`.
fn calculate_min_match_len(data: &[u8], max_search_depth: u32) -> u32 {
    if data.len() < 512 {
        return MIN_MATCH_LEN;
    }
    let mut used = [false; 256];
    for &byte in &data[..data.len().min(4096)] {
        used[byte as usize] = true;
    }
    choose_min_match_len(used.iter().filter(|&&u| u).count() as u32, max_search_depth)
}

/// `recalculate_min_match_len`.
fn recalculate_min_match_len(freqs: &Freqs, max_search_depth: u32) -> u32 {
    let literal_freq: u32 = freqs.litlen[..NUM_LITERALS].iter().fold(0u32, |sum, &f| sum.wrapping_add(f));
    let cutoff = literal_freq >> 10;
    let num_used_literals = freqs.litlen[..NUM_LITERALS].iter().filter(|&&f| f > cutoff).count() as u32;
    choose_min_match_len(num_used_literals, max_search_depth)
}

/// `choose_max_block_end`.
fn choose_max_block_end(in_block_begin: usize, in_end: usize, soft_max_len: usize) -> usize {
    if in_end - in_block_begin < soft_max_len + MIN_BLOCK_LENGTH {
        in_end
    } else {
        in_block_begin + soft_max_len
    }
}

// ---------------------------------------------------------------------------------------
// The compressor

/// `struct deflate_sequence`: a run of literals and the match after it (none for the last).
#[derive(Clone, Copy, Default)]
struct Sequence {
    litrunlen: u32,
    length: u32,
    offset: u32,
    offset_slot: usize,
}

/// `struct libdeflate_compressor`, for the levels ported.
struct Compressor {
    max_search_depth: u32,
    nice_match_length: u32,
    freqs: Freqs,
    split_stats: SplitStats,
    codes: Codes,
    static_codes: Codes,
    // The precode (`c->o.precode`).
    precode_freqs: [u32; NUM_PRECODE_SYMS],
    precode_codewords: [u32; NUM_PRECODE_SYMS],
    precode_lens: [u8; NUM_PRECODE_SYMS],
    precode_items: Vec<u32>,
    num_litlen_syms: usize,
    num_offset_syms: usize,
    num_explicit_lens: usize,
    // The full codewords of each match length (`c->o.length`).
    length_codewords: [u32; MAX_MATCH_LEN as usize + 1],
    length_lens: [u32; MAX_MATCH_LEN as usize + 1],
    hc_mf: HcMatchfinder,
    sequences: Vec<Sequence>,
}

impl Compressor {
    fn new(max_search_depth: u32, nice_match_length: u32) -> Self {
        let empty = Codes { litlen_codewords: [0; NUM_LITLEN_SYMS], offset_codewords: [0; NUM_OFFSET_SYMS], lens: [0; NUM_LITLEN_SYMS + NUM_OFFSET_SYMS] };
        let mut c = Compressor {
            max_search_depth,
            nice_match_length,
            freqs: Freqs { litlen: [0; NUM_LITLEN_SYMS], offset: [0; NUM_OFFSET_SYMS] },
            split_stats: SplitStats::default(),
            codes: empty.clone(),
            static_codes: empty,
            precode_freqs: [0; NUM_PRECODE_SYMS],
            precode_codewords: [0; NUM_PRECODE_SYMS],
            precode_lens: [0; NUM_PRECODE_SYMS],
            precode_items: Vec::new(),
            num_litlen_syms: 0,
            num_offset_syms: 0,
            num_explicit_lens: 0,
            length_codewords: [0; MAX_MATCH_LEN as usize + 1],
            length_lens: [0; MAX_MATCH_LEN as usize + 1],
            hc_mf: HcMatchfinder::new(),
            sequences: vec![Sequence::default(); SEQ_STORE_LENGTH + 1],
        };
        // deflate_init_static_codes
        for (i, freq) in c.freqs.litlen.iter_mut().enumerate() {
            *freq = match i {
                0..=143 => 1 << (9 - 8),
                144..=255 => 1 << (9 - 9),
                256..=279 => 1 << (9 - 7),
                _ => 1 << (9 - 8),
            };
        }
        c.freqs.offset = [1 << (5 - 5); NUM_OFFSET_SYMS];
        make_huffman_codes(&c.freqs, &mut c.static_codes);
        c
    }

    fn reset_symbol_frequencies(&mut self) {
        self.freqs.litlen = [0; NUM_LITLEN_SYMS];
        self.freqs.offset = [0; NUM_OFFSET_SYMS];
    }

    /// `deflate_compute_precode_items`, over the first `num_lens` lengths of `codes.lens`.
    fn compute_precode_items(&mut self, num_lens: usize) {
        let lens = &self.codes.lens;
        self.precode_freqs = [0; NUM_PRECODE_SYMS];
        self.precode_items.clear();
        let mut run_start = 0usize;
        loop {
            let len = lens[run_start];
            let mut run_end = run_start;
            loop {
                run_end += 1;
                if run_end == num_lens || len != lens[run_end] {
                    break;
                }
            }
            if len == 0 {
                while run_end - run_start >= 11 {
                    let extra_bits = ((run_end - run_start) - 11).min(0x7F);
                    self.precode_freqs[18] += 1;
                    self.precode_items.push(18 | ((extra_bits as u32) << 5));
                    run_start += 11 + extra_bits;
                }
                if run_end - run_start >= 3 {
                    let extra_bits = ((run_end - run_start) - 3).min(0x7);
                    self.precode_freqs[17] += 1;
                    self.precode_items.push(17 | ((extra_bits as u32) << 5));
                    run_start += 3 + extra_bits;
                }
            } else if run_end - run_start >= 4 {
                self.precode_freqs[len as usize] += 1;
                self.precode_items.push(u32::from(len));
                run_start += 1;
                loop {
                    let extra_bits = ((run_end - run_start) - 3).min(0x3);
                    self.precode_freqs[16] += 1;
                    self.precode_items.push(16 | ((extra_bits as u32) << 5));
                    run_start += 3 + extra_bits;
                    if run_end - run_start < 3 {
                        break;
                    }
                }
            }
            while run_start != run_end {
                self.precode_freqs[len as usize] += 1;
                self.precode_items.push(u32::from(len));
                run_start += 1;
            }
            if run_start == num_lens {
                break;
            }
        }
    }

    /// `deflate_precompute_huffman_header`.
    fn precompute_huffman_header(&mut self) {
        self.num_litlen_syms = NUM_LITLEN_SYMS;
        while self.num_litlen_syms > 257 && self.codes.lens[self.num_litlen_syms - 1] == 0 {
            self.num_litlen_syms -= 1;
        }
        self.num_offset_syms = NUM_OFFSET_SYMS;
        while self.num_offset_syms > 1 && self.codes.lens[NUM_LITLEN_SYMS + self.num_offset_syms - 1] == 0 {
            self.num_offset_syms -= 1;
        }
        // (The offset lengths moved down to follow the litlen ones, and back after.)
        if self.num_litlen_syms != NUM_LITLEN_SYMS {
            self.codes.lens.copy_within(NUM_LITLEN_SYMS..NUM_LITLEN_SYMS + self.num_offset_syms, self.num_litlen_syms);
        }
        self.compute_precode_items(self.num_litlen_syms + self.num_offset_syms);
        make_huffman_code(MAX_PRE_CODEWORD_LEN, &self.precode_freqs.clone(), &mut self.precode_lens, &mut self.precode_codewords);
        self.num_explicit_lens = NUM_PRECODE_SYMS;
        while self.num_explicit_lens > 4 && self.precode_lens[PRECODE_LENS_PERMUTATION[self.num_explicit_lens - 1]] == 0 {
            self.num_explicit_lens -= 1;
        }
        if self.num_litlen_syms != NUM_LITLEN_SYMS {
            self.codes.lens.copy_within(self.num_litlen_syms..self.num_litlen_syms + self.num_offset_syms, NUM_LITLEN_SYMS);
        }
    }

    /// `deflate_compute_full_len_codewords`.
    fn compute_full_len_codewords(&mut self, use_static: bool) {
        let codes = if use_static { &self.static_codes } else { &self.codes };
        for len in MIN_MATCH_LEN..=MAX_MATCH_LEN {
            let slot = length_slot(len);
            let litlen_sym = FIRST_LEN_SYM + slot;
            let extra_bits = len - LENGTH_SLOT_BASE[slot];
            let sym_len = u32::from(codes.lens[litlen_sym]);
            self.length_codewords[len as usize] = codes.litlen_codewords[litlen_sym] | (extra_bits.wrapping_shl(sym_len));
            self.length_lens[len as usize] = sym_len + EXTRA_LENGTH_BITS[slot];
        }
    }

    /// `deflate_flush_block`: the block of `input[block_begin..block_begin + block_length]`,
    /// of the given sequences, as the cheapest kind of block.
    fn flush_block(&mut self, os: &mut Output, input: &[u8], block_begin: usize, block_length: usize, sequences_end: usize, is_final_block: bool) {
        let mut dynamic_cost = 3u32;
        let mut static_cost = 3u32;
        let mut uncompressed_cost = 3u32;

        self.precompute_huffman_header();

        dynamic_cost += 5 + 5 + 4 + (3 * self.num_explicit_lens as u32);
        for sym in 0..NUM_PRECODE_SYMS {
            let extra = EXTRA_PRECODE_BITS[sym];
            dynamic_cost = dynamic_cost.wrapping_add(self.precode_freqs[sym].wrapping_mul(extra + u32::from(self.precode_lens[sym])));
        }
        for sym in 0..256 {
            dynamic_cost = dynamic_cost.wrapping_add(self.freqs.litlen[sym].wrapping_mul(u32::from(self.codes.lens[sym])));
            static_cost = static_cost.wrapping_add(self.freqs.litlen[sym].wrapping_mul(if sym < 144 { 8 } else { 9 }));
        }
        dynamic_cost = dynamic_cost.wrapping_add(u32::from(self.codes.lens[END_OF_BLOCK]));
        static_cost += 7;
        for sym in FIRST_LEN_SYM..FIRST_LEN_SYM + EXTRA_LENGTH_BITS.len() {
            let extra = EXTRA_LENGTH_BITS[sym - FIRST_LEN_SYM];
            dynamic_cost = dynamic_cost.wrapping_add(self.freqs.litlen[sym].wrapping_mul(extra + u32::from(self.codes.lens[sym])));
            static_cost = static_cost.wrapping_add(self.freqs.litlen[sym].wrapping_mul(extra + u32::from(self.static_codes.lens[sym])));
        }
        for sym in 0..EXTRA_OFFSET_BITS.len() {
            let extra = EXTRA_OFFSET_BITS[sym];
            dynamic_cost = dynamic_cost.wrapping_add(self.freqs.offset[sym].wrapping_mul(extra + u32::from(self.codes.lens[NUM_LITLEN_SYMS + sym])));
            static_cost = static_cost.wrapping_add(self.freqs.offset[sym].wrapping_mul(extra + 5));
        }
        uncompressed_cost += ((os.bitcount + 3).wrapping_neg() & 7) + 32 + (40 * (block_length.div_ceil(u16::MAX as usize) as u32 - 1)) + (8 * block_length as u32);

        let best_cost = dynamic_cost.min(static_cost.min(uncompressed_cost));

        if best_cost == uncompressed_cost {
            // Uncompressed block(s), of no more than 65535 bytes each.
            let mut in_next = block_begin;
            let in_end = block_begin + block_length;
            loop {
                let mut bfinal = 0;
                let mut len = u16::MAX as usize;
                if in_end - in_next <= u16::MAX as usize {
                    bfinal = u32::from(is_final_block);
                    len = in_end - in_next;
                }
                os.out.push(((bfinal << os.bitcount) | os.bitbuf as u32) as u8);
                if os.bitcount > 5 {
                    os.out.push(0);
                }
                os.bitbuf = 0;
                os.bitcount = 0;
                os.out.extend_from_slice(&(len as u16).to_le_bytes());
                os.out.extend_from_slice(&(!(len as u16)).to_le_bytes());
                os.out.extend_from_slice(&input[in_next..in_next + len]);
                in_next += len;
                if in_next == in_end {
                    break;
                }
            }
            return;
        }

        let use_static = best_cost == static_cost;
        if use_static {
            os.add_bits(u32::from(is_final_block), 1);
            os.add_bits(1, 2);
        } else {
            os.add_bits(u32::from(is_final_block), 1);
            os.add_bits(2, 2);
            os.add_bits(self.num_litlen_syms as u32 - 257, 5);
            os.add_bits(self.num_offset_syms as u32 - 1, 5);
            os.add_bits(self.num_explicit_lens as u32 - 4, 4);
            for &sym in &PRECODE_LENS_PERMUTATION[..self.num_explicit_lens] {
                os.add_bits(u32::from(self.precode_lens[sym]), 3);
            }
            for &item in &self.precode_items {
                let sym = (item & 0x1F) as usize;
                os.add_bits(self.precode_codewords[sym], u32::from(self.precode_lens[sym]));
                os.add_bits(item >> 5, EXTRA_PRECODE_BITS[sym]);
            }
        }

        self.compute_full_len_codewords(use_static);
        let codes = if use_static { &self.static_codes } else { &self.codes };
        let mut in_next = block_begin;
        for seq in &self.sequences[..=sequences_end] {
            for _ in 0..seq.litrunlen {
                let lit = input[in_next] as usize;
                in_next += 1;
                os.add_bits(codes.litlen_codewords[lit], u32::from(codes.lens[lit]));
            }
            if seq.length == 0 {
                break;
            }
            // WRITE_MATCH
            os.add_bits(self.length_codewords[seq.length as usize], self.length_lens[seq.length as usize]);
            os.add_bits(codes.offset_codewords[seq.offset_slot], u32::from(codes.lens[NUM_LITLEN_SYMS + seq.offset_slot]));
            os.add_bits(seq.offset - OFFSET_SLOT_BASE[seq.offset_slot], EXTRA_OFFSET_BITS[seq.offset_slot]);
            in_next += seq.length as usize;
        }
        os.add_bits(codes.litlen_codewords[END_OF_BLOCK], u32::from(codes.lens[END_OF_BLOCK]));
    }

    /// `deflate_finish_block`.
    fn finish_block(&mut self, os: &mut Output, input: &[u8], block_begin: usize, block_length: usize, sequences_end: usize, is_final_block: bool) {
        self.freqs.litlen[END_OF_BLOCK] += 1;
        let freqs = self.freqs.clone();
        make_huffman_codes(&freqs, &mut self.codes);
        self.flush_block(os, input, block_begin, block_length, sequences_end, is_final_block);
    }

    /// `deflate_choose_literal`.
    fn choose_literal(&mut self, literal: u8, seq: usize) {
        self.freqs.litlen[literal as usize] += 1;
        self.split_stats.observe_literal(literal);
        self.sequences[seq].litrunlen += 1;
    }

    /// `deflate_choose_match`.
    fn choose_match(&mut self, length: u32, offset: u32, seq: &mut usize) {
        let slot = offset_slot(offset);
        self.freqs.litlen[FIRST_LEN_SYM + length_slot(length)] += 1;
        self.freqs.offset[slot] += 1;
        self.split_stats.observe_match(length);
        let s = &mut self.sequences[*seq];
        s.length = length;
        s.offset = offset;
        s.offset_slot = slot;
        *seq += 1;
        self.sequences[*seq] = Sequence::default();
    }

    /// `deflate_compress_greedy`.
    fn compress_greedy(&mut self, input: &[u8], os: &mut Output) {
        let in_end = input.len();
        let mut in_next = 0usize;
        let mut in_cur_base = 0usize;
        let mut max_len = MAX_MATCH_LEN;
        let mut nice_len = self.nice_match_length.min(max_len);
        let mut next_hashes = [0u32; 2];
        loop {
            let in_block_begin = in_next;
            let in_max_block_end = choose_max_block_end(in_next, in_end, SOFT_MAX_BLOCK_LENGTH);
            let mut seq = 0usize;
            self.split_stats = SplitStats::default();
            self.reset_symbol_frequencies();
            self.sequences[0] = Sequence::default();
            let min_len = calculate_min_match_len(&input[in_next..in_max_block_end], self.max_search_depth);
            loop {
                adjust_max_and_nice_len(&mut max_len, &mut nice_len, in_end - in_next);
                let (length, offset) = self.hc_mf.longest_match(input, &mut in_cur_base, in_next, min_len - 1, max_len, nice_len, self.max_search_depth, &mut next_hashes);
                if length >= min_len && (length > MIN_MATCH_LEN || offset <= 4096) {
                    // Match found
                    self.choose_match(length, offset, &mut seq);
                    self.hc_mf.skip_bytes(input, &mut in_cur_base, in_next + 1, length - 1, &mut next_hashes);
                    in_next += length as usize;
                } else {
                    // No match found
                    self.choose_literal(input[in_next], seq);
                    in_next += 1;
                }
                if !(in_next < in_max_block_end && seq < SEQ_STORE_LENGTH && !self.split_stats.should_end_block(in_block_begin, in_next, in_end)) {
                    break;
                }
            }
            self.finish_block(os, input, in_block_begin, in_next - in_block_begin, seq, in_next == in_end);
            if in_next == in_end {
                break;
            }
        }
    }

    /// `deflate_compress_lazy_generic`.
    fn compress_lazy(&mut self, input: &[u8], os: &mut Output, lazy2: bool) {
        let in_end = input.len();
        let mut in_next = 0usize;
        let mut in_cur_base = 0usize;
        let mut max_len = MAX_MATCH_LEN;
        let mut nice_len = self.nice_match_length.min(max_len);
        let mut next_hashes = [0u32; 2];
        let bsr = |x: u32| 31 - x.leading_zeros() as i32;
        loop {
            let in_block_begin = in_next;
            let in_max_block_end = choose_max_block_end(in_next, in_end, SOFT_MAX_BLOCK_LENGTH);
            let mut next_recalc_min_len = in_next + (in_end - in_next).min(10000);
            let mut seq = 0usize;
            self.split_stats = SplitStats::default();
            self.reset_symbol_frequencies();
            self.sequences[0] = Sequence::default();
            let mut min_len = calculate_min_match_len(&input[in_next..in_max_block_end], self.max_search_depth);
            loop {
                if in_next >= next_recalc_min_len {
                    min_len = recalculate_min_match_len(&self.freqs, self.max_search_depth);
                    next_recalc_min_len += (in_end - next_recalc_min_len).min(in_next - in_block_begin);
                }
                adjust_max_and_nice_len(&mut max_len, &mut nice_len, in_end - in_next);
                let (mut cur_len, mut cur_offset) = self.hc_mf.longest_match(input, &mut in_cur_base, in_next, min_len - 1, max_len, nice_len, self.max_search_depth, &mut next_hashes);
                if cur_len < min_len || (cur_len == MIN_MATCH_LEN && cur_offset > 8192) {
                    // No match found.  Choose a literal.
                    self.choose_literal(input[in_next], seq);
                    in_next += 1;
                } else {
                    in_next += 1;
                    // have_cur_match:
                    loop {
                        if cur_len >= nice_len {
                            self.choose_match(cur_len, cur_offset, &mut seq);
                            self.hc_mf.skip_bytes(input, &mut in_cur_base, in_next, cur_len - 1, &mut next_hashes);
                            in_next += cur_len as usize - 1;
                            break;
                        }
                        adjust_max_and_nice_len(&mut max_len, &mut nice_len, in_end - in_next);
                        let (next_len, next_offset) = self.hc_mf.longest_match(input, &mut in_cur_base, in_next, cur_len - 1, max_len, nice_len, self.max_search_depth >> 1, &mut next_hashes);
                        in_next += 1;
                        if next_len >= cur_len && 4 * (next_len - cur_len) as i32 + (bsr(cur_offset) - bsr(next_offset)) > 2 {
                            self.choose_literal(input[in_next - 2], seq);
                            cur_len = next_len;
                            cur_offset = next_offset;
                            continue;
                        }
                        if lazy2 {
                            adjust_max_and_nice_len(&mut max_len, &mut nice_len, in_end - in_next);
                            let (next_len, next_offset) = self.hc_mf.longest_match(input, &mut in_cur_base, in_next, cur_len - 1, max_len, nice_len, self.max_search_depth >> 2, &mut next_hashes);
                            in_next += 1;
                            if next_len >= cur_len && 4 * (next_len - cur_len) as i32 + (bsr(cur_offset) - bsr(next_offset)) > 6 {
                                self.choose_literal(input[in_next - 3], seq);
                                self.choose_literal(input[in_next - 2], seq);
                                cur_len = next_len;
                                cur_offset = next_offset;
                                continue;
                            }
                            self.choose_match(cur_len, cur_offset, &mut seq);
                            if cur_len > 3 {
                                self.hc_mf.skip_bytes(input, &mut in_cur_base, in_next, cur_len - 3, &mut next_hashes);
                                in_next += cur_len as usize - 3;
                            }
                        } else {
                            self.choose_match(cur_len, cur_offset, &mut seq);
                            self.hc_mf.skip_bytes(input, &mut in_cur_base, in_next, cur_len - 2, &mut next_hashes);
                            in_next += cur_len as usize - 2;
                        }
                        break;
                    }
                }
                if !(in_next < in_max_block_end && seq < SEQ_STORE_LENGTH && !self.split_stats.should_end_block(in_block_begin, in_next, in_end)) {
                    break;
                }
            }
            self.finish_block(os, input, in_block_begin, in_next - in_block_begin, seq, in_next == in_end);
            if in_next == in_end {
                break;
            }
        }
    }
}

/// `adjust_max_and_nice_len`.
fn adjust_max_and_nice_len(max_len: &mut u32, nice_len: &mut u32, remaining: usize) {
    if remaining < MAX_MATCH_LEN as usize {
        *max_len = remaining as u32;
        *nice_len = (*nice_len).min(*max_len);
    }
}

/// `deflate_compress_none`.
fn compress_none(input: &[u8], out: &mut Vec<u8>) {
    if input.is_empty() {
        out.push(1);
        out.extend_from_slice(&0xFFFF_0000u32.to_le_bytes());
        return;
    }
    for (k, chunk) in input.chunks(u16::MAX as usize).enumerate() {
        let bfinal = (k + 1) * u16::MAX as usize >= input.len();
        out.push(u8::from(bfinal));
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        out.extend_from_slice(chunk);
    }
}

/// `libdeflate_deflate_compress` at `level`.
fn deflate_compress(level: i32, input: &[u8], out: &mut Vec<u8>) {
    let max_passthrough_size = if level == 0 { usize::MAX } else { (55 - level * 4) as usize };
    if input.len() <= max_passthrough_size {
        compress_none(input, out);
        return;
    }
    let (depth, nice) = match level {
        2 => (6, 10),
        3 => (12, 14),
        4 => (16, 30),
        5 => (16, 30),
        6 => (35, 65),
        7 => (100, 130),
        8 => (300, MAX_MATCH_LEN),
        _ => (600, MAX_MATCH_LEN),
    };
    let mut c = Compressor::new(depth, nice);
    let mut os = Output { out: std::mem::take(out), bitbuf: 0, bitcount: 0 };
    match level {
        2..=4 => c.compress_greedy(input, &mut os),
        5..=7 => c.compress_lazy(input, &mut os, false),
        _ => c.compress_lazy(input, &mut os, true),
    }
    if os.bitcount != 0 {
        os.out.push(os.bitbuf as u8);
    }
    *out = os.out;
}

/// `exr_compress_max_buffer_size` of OpenEXR 3.5: the room `exr_compress_buffer` is given,
/// which is also enough for its Huffman coder's spare memory.
pub fn compress_bound(in_bytes: usize) -> usize {
    // libdeflate_zlib_compress_bound
    let max_blocks = in_bytes.div_ceil(MIN_BLOCK_LENGTH).max(1);
    let Some(mut r) = (5 * max_blocks).checked_add(in_bytes).and_then(|r| r.checked_add(6 + 9)) else { return usize::MAX };
    let Some(extra) = r.checked_mul(130) else { return usize::MAX };
    let extra = extra / 128;
    if extra > usize::MAX - 100 {
        return usize::MAX;
    }
    if extra > r {
        r = extra;
    }
    // in case huf is larger than zlib (`internal_exr_huf_compress_spare_bytes` and
    // `internal_exr_huf_decompress_spare_bytes`)
    r = r.max(in_bytes.saturating_add(1_835_036)).max(in_bytes.saturating_add(786_440));
    r.max(8192)
}

/// `exr_compress_buffer` of OpenEXR 3.5 (`compression.c`) with libdeflate's
/// `libdeflate_zlib_compress`: `input` as a zlib stream at `level` (a negative level is
/// OpenEXR's default, 4; libdeflate's levels 0 and 2 to 9).
///
/// # Panics
///
/// For level 1 or a level above 9, which are not ported.
pub fn compress_buffer(level: i32, input: &[u8]) -> Vec<u8> {
    let level = if level < 0 { 4 } else { level };
    assert!(level == 0 || (2..=MAX_LEVEL).contains(&level), "libdeflate level {level} is not ported");
    let level_hint: u16 = if level < 2 {
        0
    } else if level < 6 {
        1
    } else if level < 8 {
        2
    } else {
        3
    };
    let mut hdr: u16 = (8 << 8) | (7 << 12);
    hdr |= level_hint << 6;
    hdr |= 31 - (hdr % 31);
    let mut out = Vec::with_capacity(compress_bound(input.len()));
    out.extend_from_slice(&hdr.to_be_bytes());
    let mut deflated = Vec::new();
    deflate_compress(level, input, &mut deflated);
    out.extend_from_slice(&deflated);
    out.extend_from_slice(&adler32(1, input).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_come_back() {
        let mut data = Vec::new();
        let mut state = 7u64;
        for i in 0..200_000u32 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            data.push(if i % 1000 < 600 { (i / 7 % 13) as u8 } else { (state >> 56) as u8 });
        }
        for level in [0, 2, 3, 4, 5, 6, 7, 8, 9] {
            for len in [0usize, 1, 30, 100, 5000, 70000, 200_000] {
                let stream = compress_buffer(level, &data[..len]);
                let mut back = vec![0u8; len];
                assert_eq!(super::super::uncompress_buffer(&stream, &mut back), Ok(len), "level {level}, {len} bytes");
                assert_eq!(back, &data[..len]);
            }
        }
    }
}
