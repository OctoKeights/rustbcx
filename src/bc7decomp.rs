//! BC7 block decompression: the port of `bc7decomp.cpp` (Richard Geldreich, Jr., 3/31/2020).
//!
//! The C++ has a path written with SSE2 and a plain one, which give the same colours; this is
//! the plain one. Instead of one monster routine to unpack all the BC7 modes, the three-subset,
//! two-subset, one-subset and dual-plane modes are lumped together into simple shared routines.

/// A pixel: red, green, blue, alpha.
pub type ColorRgba = [u8; 4];

const WEIGHTS2: [u32; 4] = [0, 21, 43, 64];
const WEIGHTS3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Which of two subsets each pixel belongs to, for each of the 64 partitions.
pub(crate) const PARTITION2: [u8; 1024] = [
    0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1,
    0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1,
    0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1,
    0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1,
    0, 0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1,
    0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1,
    0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0,
    0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1,
    0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0,
    0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0, 0,
    0, 0, 0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0,
    0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1,
    0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
    0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0,
    0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0, 1,
    0, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0,
    0, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0,
    0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1,
    0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0,
    0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0,
    0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1,
    0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0,
    0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1,
    0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1,
    0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0,
    0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 1, 1, 1,
];

/// Which of three subsets each pixel belongs to, for each of the 64 partitions.
pub(crate) const PARTITION3: [u8; 1024] = [
    0, 0, 1, 1, 0, 0, 1, 1, 0, 2, 2, 1, 2, 2, 2, 2, 0, 0, 0, 1, 0, 0, 1, 1, 2, 2, 1, 1, 2, 2, 2, 1,
    0, 0, 0, 0, 2, 0, 0, 1, 2, 2, 1, 1, 2, 2, 1, 1, 0, 2, 2, 2, 0, 0, 2, 2, 0, 0, 1, 1, 0, 1, 1, 1,
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 1, 1, 2, 2, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 2, 2, 0, 0, 2, 2,
    0, 0, 2, 2, 0, 0, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 2, 2, 1, 1, 2, 2, 1, 1,
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2,
    0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 0, 0, 1, 2, 0, 0, 1, 2, 0, 0, 1, 2, 0, 0, 1, 2,
    0, 1, 1, 2, 0, 1, 1, 2, 0, 1, 1, 2, 0, 1, 1, 2, 0, 1, 2, 2, 0, 1, 2, 2, 0, 1, 2, 2, 0, 1, 2, 2,
    0, 0, 1, 1, 0, 1, 1, 2, 1, 1, 2, 2, 1, 2, 2, 2, 0, 0, 1, 1, 2, 0, 0, 1, 2, 2, 0, 0, 2, 2, 2, 0,
    0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 2, 1, 1, 2, 2, 0, 1, 1, 1, 0, 0, 1, 1, 2, 0, 0, 1, 2, 2, 0, 0,
    0, 0, 0, 0, 1, 1, 2, 2, 1, 1, 2, 2, 1, 1, 2, 2, 0, 0, 2, 2, 0, 0, 2, 2, 0, 0, 2, 2, 1, 1, 1, 1,
    0, 1, 1, 1, 0, 1, 1, 1, 0, 2, 2, 2, 0, 2, 2, 2, 0, 0, 0, 1, 0, 0, 0, 1, 2, 2, 2, 1, 2, 2, 2, 1,
    0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 2, 2, 0, 1, 2, 2, 0, 0, 0, 0, 1, 1, 0, 0, 2, 2, 1, 0, 2, 2, 1, 0,
    0, 1, 2, 2, 0, 1, 2, 2, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 0, 0, 1, 2, 1, 1, 2, 2, 2, 2, 2, 2,
    0, 1, 1, 0, 1, 2, 2, 1, 1, 2, 2, 1, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 2, 2, 1, 1, 2, 2, 1,
    0, 0, 2, 2, 1, 1, 0, 2, 1, 1, 0, 2, 0, 0, 2, 2, 0, 1, 1, 0, 0, 1, 1, 0, 2, 0, 0, 2, 2, 2, 2, 2,
    0, 0, 1, 1, 0, 1, 2, 2, 0, 1, 2, 2, 0, 0, 1, 1, 0, 0, 0, 0, 2, 0, 0, 0, 2, 2, 1, 1, 2, 2, 2, 1,
    0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 2, 1, 2, 2, 2, 0, 2, 2, 2, 0, 0, 2, 2, 0, 0, 1, 2, 0, 0, 1, 1,
    0, 0, 1, 1, 0, 0, 1, 2, 0, 0, 2, 2, 0, 2, 2, 2, 0, 1, 2, 0, 0, 1, 2, 0, 0, 1, 2, 0, 0, 1, 2, 0,
    0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0,
    0, 1, 2, 0, 2, 0, 1, 2, 1, 2, 0, 1, 0, 1, 2, 0, 0, 0, 1, 1, 2, 2, 0, 0, 1, 1, 2, 2, 0, 0, 1, 1,
    0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 2,
    0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 2, 1, 2, 1, 2, 1, 0, 0, 2, 2, 1, 1, 2, 2, 0, 0, 2, 2, 1, 1, 2, 2,
    0, 0, 2, 2, 0, 0, 1, 1, 0, 0, 2, 2, 0, 0, 1, 1, 0, 2, 2, 0, 1, 2, 2, 1, 0, 2, 2, 0, 1, 2, 2, 1,
    0, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 2, 0, 1, 0, 1, 0, 0, 0, 0, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1,
    0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 2, 2, 2, 2, 0, 2, 2, 2, 0, 1, 1, 1, 0, 2, 2, 2, 0, 1, 1, 1,
    0, 0, 0, 2, 1, 1, 1, 2, 0, 0, 0, 2, 1, 1, 1, 2, 0, 0, 0, 0, 2, 1, 1, 2, 2, 1, 1, 2, 2, 1, 1, 2,
    0, 2, 2, 2, 0, 1, 1, 1, 0, 1, 1, 1, 0, 2, 2, 2, 0, 0, 0, 2, 1, 1, 1, 2, 1, 1, 1, 2, 0, 0, 0, 2,
    0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 2, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 2, 1, 1, 2,
    0, 1, 1, 0, 0, 1, 1, 0, 2, 2, 2, 2, 2, 2, 2, 2, 0, 0, 2, 2, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 2, 2,
    0, 0, 2, 2, 1, 1, 2, 2, 1, 1, 2, 2, 0, 0, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2,
    0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 2, 2, 2, 1, 2, 2, 2, 0, 2, 2, 2, 1, 2, 2, 2,
    0, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0, 1, 1, 1, 2, 0, 1, 1, 2, 2, 0, 1, 2, 2, 2, 0,
];

pub(crate) const ANCHOR_INDEX_SECOND_SUBSET: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];

pub(crate) const ANCHOR_INDEX_THIRD_SUBSET_1: [u8; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5,
    15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5,
    10, 8, 13, 15, 12, 3, 3,
];

pub(crate) const ANCHOR_INDEX_THIRD_SUBSET_2: [u8; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6,
    10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];

/// The mode a block's first byte says it is (the place of its lowest set bit), or 8 for none.
pub(crate) const FIRST_BYTE_TO_MODE: [u8; 256] = [
    8, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    5, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    6, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    5, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    7, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    5, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    6, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
    5, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0, 4, 0, 1, 0, 2, 0, 1, 0, 3, 0, 1, 0, 2, 0, 1, 0,
];

#[inline]
fn insert_weight_zero(index_bits: &mut u64, bits_per_index: u32, offset: u32) {
    let low_bit_mask = (1u64 << ((bits_per_index * (offset + 1)) - 1)) - 1;
    let high_bit_mask = !low_bit_mask;
    *index_bits = ((*index_bits & high_bit_mask) << 1) | (*index_bits & low_bit_mask);
}

#[inline]
fn dequant_pbit(val: u32, pbit: u32, val_bits: u32) -> u32 {
    let total_bits = val_bits + 1;
    let mut val = (val << 1) | pbit;
    val <<= 8 - total_bits;
    val |= val >> total_bits;
    val
}

#[inline]
fn dequant(val: u32, val_bits: u32) -> u32 {
    let mut val = val << (8 - val_bits);
    val |= val >> val_bits;
    val
}

#[inline]
fn interp(l: u32, h: u32, w: usize, bits: u32) -> u32 {
    let weight = match bits {
        2 => WEIGHTS2[w],
        3 => WEIGHTS3[w],
        4 => WEIGHTS4[w],
        _ => return 0,
    };
    (l * (64 - weight) + h * weight + 32) >> 6
}

/// The colours between a pair of endpoints, all four components of them, for indices of
/// `weight_bits` bits.
fn interpolate(endpoints: &[ColorRgba], weight_bits: u32, out: &mut [ColorRgba; 8]) {
    for (i, color) in out.iter_mut().enumerate().take(1 << weight_bits) {
        for c in 0..4 {
            color[c] = interp(
                u32::from(endpoints[0][c]),
                u32::from(endpoints[1][c]),
                i,
                weight_bits,
            ) as u8;
        }
    }
}

fn unpack_bc7_mode0_2(mode: u32, data_chunks: &[u64; 2], pixels: &mut [ColorRgba; 16]) -> bool {
    const ENDPOINTS: usize = 6;
    const COMPS: usize = 3;
    let weight_bits: u32 = if mode == 0 { 3 } else { 2 };
    let weight_mask = (1u64 << weight_bits) - 1;
    let endpoint_bits: u32 = if mode == 0 { 4 } else { 5 };
    let endpoint_mask = (1u64 << endpoint_bits) - 1;
    let pbit_count = if mode == 0 { 6 } else { 0 };
    let part_bits = if mode == 0 { 4 } else { 6 };
    let part_mask = (1u64 << part_bits) - 1;

    let low_chunk = data_chunks[0];
    let high_chunk = data_chunks[1];

    let part = ((low_chunk >> (mode + 1)) & part_mask) as usize;

    let channel_read_chunks = if mode == 0 {
        [
            low_chunk >> 5,
            low_chunk >> 29,
            (low_chunk >> 53) | (high_chunk << 11),
        ]
    } else {
        [
            low_chunk >> 9,
            (low_chunk >> 39) | (high_chunk << 25),
            high_chunk >> 5,
        ]
    };

    let mut endpoints = [[0u8; 4]; ENDPOINTS];
    for c in 0..COMPS {
        let mut channel_read_chunk = channel_read_chunks[c];
        for endpoint in &mut endpoints {
            endpoint[c] = (channel_read_chunk & endpoint_mask) as u8;
            channel_read_chunk >>= endpoint_bits;
        }
    }

    let mut pbits = [0u32; 6];
    if mode == 0 {
        let p_bits_chunk = ((high_chunk >> 13) & 0xff) as u32;
        for (p, pbit) in pbits.iter_mut().enumerate().take(pbit_count) {
            *pbit = (p_bits_chunk >> p) & 1;
        }
    }

    let mut weights_read_chunk = high_chunk >> (67 - 16 * weight_bits);
    let (anchor1, anchor2) = (
        u32::from(ANCHOR_INDEX_THIRD_SUBSET_1[part]),
        u32::from(ANCHOR_INDEX_THIRD_SUBSET_2[part]),
    );
    insert_weight_zero(&mut weights_read_chunk, weight_bits, 0);
    insert_weight_zero(&mut weights_read_chunk, weight_bits, anchor1.min(anchor2));
    insert_weight_zero(&mut weights_read_chunk, weight_bits, anchor1.max(anchor2));

    let mut weights = [0usize; 16];
    for weight in &mut weights {
        *weight = (weights_read_chunk & weight_mask) as usize;
        weights_read_chunk >>= weight_bits;
    }

    for (e, endpoint) in endpoints.iter_mut().enumerate() {
        for c in 0..4 {
            endpoint[c] = if c == 3 {
                255
            } else if pbit_count != 0 {
                dequant_pbit(u32::from(endpoint[c]), pbits[e], endpoint_bits) as u8
            } else {
                dequant(u32::from(endpoint[c]), endpoint_bits) as u8
            };
        }
    }

    let mut block_colors = [[[0u8; 4]; 8]; 3];
    for (s, colors) in block_colors.iter_mut().enumerate() {
        interpolate(&endpoints[s * 2..s * 2 + 2], weight_bits, colors);
    }

    for (i, pixel) in pixels.iter_mut().enumerate() {
        *pixel = block_colors[usize::from(PARTITION3[part * 16 + i])][weights[i]];
    }
    true
}

fn unpack_bc7_mode1_3_7(mode: u32, data_chunks: &[u64; 2], pixels: &mut [ColorRgba; 16]) -> bool {
    const ENDPOINTS: usize = 4;
    let comps: usize = if mode == 7 { 4 } else { 3 };
    let weight_bits: u32 = if mode == 1 { 3 } else { 2 };
    let weight_mask = (1u64 << weight_bits) - 1;
    let endpoint_bits: u32 = if mode == 7 {
        5
    } else if mode == 1 {
        6
    } else {
        7
    };
    let endpoint_mask = (1u64 << endpoint_bits) - 1;
    let pbit_count = if mode == 1 { 2 } else { 4 };
    let shared_pbits = mode == 1;

    let low_chunk = data_chunks[0];
    let high_chunk = data_chunks[1];

    let part = ((low_chunk >> (mode + 1)) & 0x3f) as usize;

    let mut channel_read_chunks = [0u64; 4];
    channel_read_chunks[0] = low_chunk >> (mode + 7);
    let p_read_chunk;
    let mut weight_read_chunk;
    match mode {
        1 => {
            channel_read_chunks[1] = low_chunk >> 32;
            channel_read_chunks[2] = (low_chunk >> 56) | (high_chunk << 8);
            p_read_chunk = high_chunk >> 16;
            weight_read_chunk = high_chunk >> 18;
        }
        3 => {
            channel_read_chunks[1] = (low_chunk >> 38) | (high_chunk << 26);
            channel_read_chunks[2] = high_chunk >> 2;
            p_read_chunk = high_chunk >> 30;
            weight_read_chunk = high_chunk >> 34;
        }
        7 => {
            channel_read_chunks[1] = low_chunk >> 34;
            channel_read_chunks[2] = (low_chunk >> 54) | (high_chunk << 10);
            channel_read_chunks[3] = high_chunk >> 10;
            p_read_chunk = high_chunk >> 30;
            weight_read_chunk = high_chunk >> 34;
        }
        _ => return false,
    }

    let mut endpoints = [[0u8; 4]; ENDPOINTS];
    for c in 0..comps {
        let mut channel_read_chunk = channel_read_chunks[c];
        for endpoint in &mut endpoints {
            endpoint[c] = (channel_read_chunk & endpoint_mask) as u8;
            channel_read_chunk >>= endpoint_bits;
        }
    }

    let mut pbits = [0u32; 4];
    for (p, pbit) in pbits.iter_mut().enumerate().take(pbit_count) {
        *pbit = ((p_read_chunk >> p) & 1) as u32;
    }

    insert_weight_zero(&mut weight_read_chunk, weight_bits, 0);
    insert_weight_zero(
        &mut weight_read_chunk,
        weight_bits,
        u32::from(ANCHOR_INDEX_SECOND_SUBSET[part]),
    );

    let mut weights = [0usize; 16];
    for weight in &mut weights {
        *weight = (weight_read_chunk & weight_mask) as usize;
        weight_read_chunk >>= weight_bits;
    }

    for (e, endpoint) in endpoints.iter_mut().enumerate() {
        for c in 0..4 {
            endpoint[c] = if mode != 7 && c == 3 {
                255
            } else {
                dequant_pbit(
                    u32::from(endpoint[c]),
                    pbits[if shared_pbits { e >> 1 } else { e }],
                    endpoint_bits,
                ) as u8
            };
        }
    }

    let mut block_colors = [[[0u8; 4]; 8]; 2];
    for (s, colors) in block_colors.iter_mut().enumerate() {
        interpolate(&endpoints[s * 2..s * 2 + 2], weight_bits, colors);
    }

    for (i, pixel) in pixels.iter_mut().enumerate() {
        *pixel = block_colors[usize::from(PARTITION2[part * 16 + i])][weights[i]];
    }
    true
}

fn unpack_bc7_mode4_5(mode: u32, data_chunks: &[u64; 2], pixels: &mut [ColorRgba; 16]) -> bool {
    const WEIGHT_BITS: u32 = 2;
    const WEIGHT_MASK: u64 = (1 << WEIGHT_BITS) - 1;
    let a_weight_bits: u32 = if mode == 4 { 3 } else { 2 };
    let a_weight_mask = (1u64 << a_weight_bits) - 1;
    let endpoint_bits: u32 = if mode == 4 { 5 } else { 7 };
    let endpoint_mask = (1u64 << endpoint_bits) - 1;
    let a_endpoint_bits: u32 = if mode == 4 { 6 } else { 8 };
    let a_endpoint_mask = (1u64 << a_endpoint_bits) - 1;

    let low_chunk = data_chunks[0];
    let high_chunk = data_chunks[1];

    let comp_rot = ((low_chunk >> (mode + 1)) & 0x3) as usize;
    let index_mode = mode == 4 && (low_chunk >> 7) & 1 != 0;

    let mut color_read_bits = low_chunk >> 8;

    let mut endpoints = [[0u8; 4]; 2];
    for c in 0..3 {
        for endpoint in &mut endpoints {
            endpoint[c] = (color_read_bits & endpoint_mask) as u8;
            color_read_bits >>= endpoint_bits;
        }
    }

    let mut rgb_weights_chunk;
    let mut a_weights_chunk;
    if mode == 4 {
        endpoints[0][3] = (color_read_bits & a_endpoint_mask) as u8;
        endpoints[1][3] = ((color_read_bits >> a_endpoint_bits) & a_endpoint_mask) as u8;
        rgb_weights_chunk = (low_chunk >> 50) | (high_chunk << 14);
        a_weights_chunk = high_chunk >> 17;
    } else if mode == 5 {
        endpoints[0][3] = (color_read_bits & a_endpoint_mask) as u8;
        endpoints[1][3] = (((low_chunk >> 58) | (high_chunk << 6)) & a_endpoint_mask) as u8;
        rgb_weights_chunk = high_chunk >> 2;
        a_weights_chunk = high_chunk >> 33;
    } else {
        return false;
    }

    insert_weight_zero(&mut rgb_weights_chunk, WEIGHT_BITS, 0);
    insert_weight_zero(&mut a_weights_chunk, a_weight_bits, 0);

    let weight_bits = if index_mode {
        [a_weight_bits, WEIGHT_BITS]
    } else {
        [WEIGHT_BITS, a_weight_bits]
    };
    let weight_mask = if index_mode {
        [a_weight_mask, WEIGHT_MASK]
    } else {
        [WEIGHT_MASK, a_weight_mask]
    };

    if index_mode {
        std::mem::swap(&mut rgb_weights_chunk, &mut a_weights_chunk);
    }

    let mut weights = [0usize; 16];
    let mut a_weights = [0usize; 16];
    for weight in &mut weights {
        *weight = (rgb_weights_chunk & weight_mask[0]) as usize;
        rgb_weights_chunk >>= weight_bits[0];
    }
    for weight in &mut a_weights {
        *weight = (a_weights_chunk & weight_mask[1]) as usize;
        a_weights_chunk >>= weight_bits[1];
    }

    for endpoint in &mut endpoints {
        for c in 0..4 {
            endpoint[c] = dequant(
                u32::from(endpoint[c]),
                if c == 3 {
                    a_endpoint_bits
                } else {
                    endpoint_bits
                },
            ) as u8;
        }
    }

    let mut block_colors = [[0u8; 4]; 8];
    interpolate(&endpoints, weight_bits[0], &mut block_colors);
    for (i, color) in block_colors
        .iter_mut()
        .enumerate()
        .take(1 << weight_bits[1])
    {
        color[3] = interp(
            u32::from(endpoints[0][3]),
            u32::from(endpoints[1][3]),
            i,
            weight_bits[1],
        ) as u8;
    }

    for (i, pixel) in pixels.iter_mut().enumerate() {
        *pixel = block_colors[weights[i]];
        pixel[3] = block_colors[a_weights[i]][3];
        if comp_rot >= 1 {
            pixel.swap(3, comp_rot - 1);
        }
    }
    true
}

fn unpack_bc7_mode6(data_chunks: &[u64; 2], pixels: &mut [ColorRgba; 16]) -> bool {
    let (lo, hi) = (data_chunks[0], data_chunks[1]);
    if lo & 0x7f != 1 << 6 {
        return false;
    }
    let field = |n: u32| ((lo >> (7 + 7 * n)) & 0x7f) as u32;
    let p0 = (lo >> 63) as u32;
    let p1 = (hi & 1) as u32;
    // (In the block: r0, r1, g0, g1, b0, b1, a0, a1.)
    let low = [
        (field(0) << 1) | p0,
        (field(2) << 1) | p0,
        (field(4) << 1) | p0,
        (field(6) << 1) | p0,
    ];
    let high = [
        (field(1) << 1) | p1,
        (field(3) << 1) | p1,
        (field(5) << 1) | p1,
        (field(7) << 1) | p1,
    ];

    let mut vals = [[0u8; 4]; 16];
    for (i, val) in vals.iter_mut().enumerate() {
        let w = WEIGHTS4[i];
        let iw = 64 - w;
        for c in 0..4 {
            val[c] = ((low[c] * iw + high[c] * w + 32) >> 6) as u8;
        }
    }

    // The first index has three bits (its top bit is known to be zero), the rest four.
    pixels[0] = vals[((hi >> 1) & 7) as usize];
    for (i, pixel) in pixels.iter_mut().enumerate().skip(1) {
        *pixel = vals[((hi >> (i * 4)) & 15) as usize];
    }
    true
}

/// Unpacks a BC7 block of any mode to 16 pixels, in rows from the top. False (and the pixels
/// all zero) for a block that is of no mode.
pub fn unpack_bc7(block: &[u8; 16], pixels: &mut [ColorRgba; 16]) -> bool {
    let mode = FIRST_BYTE_TO_MODE[usize::from(block[0])];
    let data_chunks = [
        u64::from_le_bytes(block[..8].try_into().unwrap()),
        u64::from_le_bytes(block[8..].try_into().unwrap()),
    ];
    match mode {
        0 | 2 => unpack_bc7_mode0_2(u32::from(mode), &data_chunks, pixels),
        1 | 3 | 7 => unpack_bc7_mode1_3_7(u32::from(mode), &data_chunks, pixels),
        4 | 5 => unpack_bc7_mode4_5(u32::from(mode), &data_chunks, pixels),
        6 => unpack_bc7_mode6(&data_chunks, pixels),
        _ => {
            *pixels = [[0; 4]; 16];
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_no_mode_is_refused() {
        let mut pixels = [[9u8; 4]; 16];
        assert!(!unpack_bc7(&[0; 16], &mut pixels));
        assert_eq!(pixels, [[0; 4]; 16]);
    }

    #[test]
    fn mode_6_of_one_colour() {
        // Mode 6, every endpoint value 127 with its p-bit set (255), every index 0.
        let mut block = [0u8; 16];
        block[..8].copy_from_slice(&0xFFFF_FFFF_FFFF_FFC0u64.to_le_bytes());
        block[8] = 1;
        let mut pixels = [[0u8; 4]; 16];
        assert!(unpack_bc7(&block, &mut pixels));
        assert_eq!(pixels, [[255; 4]; 16]);

        // The same with the low endpoint 0 and the last index 15: the last pixel is the high
        // endpoint, the rest the low one.
        block[..8].copy_from_slice(&0x7F01_FC07_F01F_C040u64.to_le_bytes());
        block[15] = 0xF0;
        assert!(unpack_bc7(&block, &mut pixels));
        assert_eq!(pixels[0], [0, 0, 0, 0]);
        assert_eq!(pixels[15], [255, 255, 255, 255]);
    }
}
