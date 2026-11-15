//! The PXR24 compression of OpenEXR: a port of `DecompressPxr24` of tinyexr v3.4.0
//! (`tinyexr.h`).
//!
//! Floats are kept as 24 bits, which loses a little; halves and integers are kept whole. The
//! bytes of a line's values are stored most significant first, as differences from the value
//! to the left, and the whole block is a zlib stream.

use crate::miniz;

use super::{PIXELTYPE_HALF, PIXELTYPE_UINT};

/// `DecompressPxr24`: a block of `num_lines` lines of `data_width` pixels into `out`, four
/// bytes a value for integers and floats and two for halves. `pixel_types` are the channels'.
pub(crate) fn decompress_pxr24(out: &mut [u8], input: &[u8], data_width: i32, num_lines: i32, pixel_types: &[i32]) -> bool {
    // PXR24 stores data per-scanline, per-channel with byte planes: UINT in 4 bytes, HALF in
    // 2 bytes, FLOAT in 3 bytes (24-bit).

    // Calculate the expected uncompressed size in PXR24 format.
    let mut pxr24_size = 0usize;
    for &pixel_type in pixel_types {
        let ch_pixels = data_width.wrapping_mul(num_lines);
        if pixel_type == PIXELTYPE_UINT {
            pxr24_size += (ch_pixels as usize).wrapping_mul(4);
        } else if pixel_type == PIXELTYPE_HALF {
            pxr24_size += (ch_pixels as usize).wrapping_mul(2);
        } else {
            // FLOAT: 24 bits
            pxr24_size += (ch_pixels as usize).wrapping_mul(3);
        }
    }

    // First decompress with zlib.
    let pxr24_buf;
    if pxr24_size == input.len() {
        // Data is not compressed.
        pxr24_buf = input.to_vec();
    } else {
        match miniz::uncompress(input, pxr24_size) {
            Some(buf) => pxr24_buf = buf,
            None => return false,
        }
    }

    if pxr24_buf.len() != pxr24_size {
        return false;
    }

    // Convert from PXR24 format to standard EXR format.
    let mut in_p = 0usize;
    let mut out_p = 0usize;
    let w = data_width as usize;

    for _line in 0..num_lines {
        for &pixel_type in pixel_types {
            if pixel_type == PIXELTYPE_UINT {
                // UINT: 4 byte planes with delta encoding.
                let (ptr0, ptr1, ptr2, ptr3) = (in_p, in_p + w, in_p + w * 2, in_p + w * 3);
                in_p += w * 4;

                let mut pixel = 0u32;
                for x in 0..w {
                    let diff = ((pxr24_buf[ptr0 + x] as u32) << 24) | ((pxr24_buf[ptr1 + x] as u32) << 16) | ((pxr24_buf[ptr2 + x] as u32) << 8) | pxr24_buf[ptr3 + x] as u32;
                    pixel = pixel.wrapping_add(diff);
                    out[out_p..out_p + 4].copy_from_slice(&pixel.to_le_bytes());
                    out_p += 4;
                }
            } else if pixel_type == PIXELTYPE_HALF {
                // HALF: 2 byte planes with delta encoding.
                let (ptr0, ptr1) = (in_p, in_p + w);
                in_p += w * 2;

                let mut pixel = 0u32;
                for x in 0..w {
                    let diff = ((pxr24_buf[ptr0 + x] as u32) << 8) | pxr24_buf[ptr1 + x] as u32;
                    pixel = pixel.wrapping_add(diff);
                    out[out_p..out_p + 2].copy_from_slice(&(pixel as u16).to_le_bytes());
                    out_p += 2;
                }
            } else {
                // FLOAT: 3 byte planes (24-bit) with delta encoding.
                let (ptr0, ptr1, ptr2) = (in_p, in_p + w, in_p + w * 2);
                in_p += w * 3;

                let mut pixel = 0u32;
                for x in 0..w {
                    let diff = ((pxr24_buf[ptr0 + x] as u32) << 24) | ((pxr24_buf[ptr1 + x] as u32) << 16) | ((pxr24_buf[ptr2 + x] as u32) << 8);
                    pixel = pixel.wrapping_add(diff);
                    // Float24 to float32: the 24-bit value is in the upper 24 bits.
                    out[out_p..out_p + 4].copy_from_slice(&pixel.to_le_bytes());
                    out_p += 4;
                }
            }
        }
    }

    true
}
