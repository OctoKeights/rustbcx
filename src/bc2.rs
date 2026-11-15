//! BC2 (DXT3, and DXT2 with the colours multiplied by alpha): a BC1 block of colour behind
//! sixteen alphas of four bits each. Little used since BC3, which holds smooth alpha far
//! better; BC2 keeps hard-edged alpha exactly on sixteen levels, and old files are full of it.
//!
//! (The crate's own: bc7enc_rdo has no BC2. The colours are packed and unpacked by
//! [`crate::rgbcx`] as BC3's are; the unpacker is compared with bcdec's `bcdec_bc2`.)

use crate::rgbcx::{self, Bc1ApproxMode, Color32, Rgbcx};

/// Packs sixteen alphas, `stride` bytes apart in `pixels`, to the alpha half of a BC2 block:
/// each to the nearest of the sixteen levels (0, 17, 34 ... 255), four bits a pixel in rows
/// from the top, the low bits first.
pub fn encode_bc2_alpha(dst: &mut [u8; 8], pixels: &[u8], stride: usize) {
    for (row, byte_pair) in dst.chunks_exact_mut(2).enumerate() {
        let mut bits = 0u16;
        for x in 0..4 {
            let alpha = u16::from(pixels[(row * 4 + x) * stride]);
            bits |= ((alpha + 8) / 17) << (x * 4);
        }
        byte_pair.copy_from_slice(&bits.to_le_bytes());
    }
}

/// Unpacks the alpha half of a BC2 block to sixteen values, `stride` bytes apart in `pixels`.
pub fn unpack_bc2_alpha(block: &[u8; 8], pixels: &mut [u8], stride: usize) {
    for i in 0..16 {
        pixels[i * stride] = ((block[i / 2] >> ((i % 2) * 4)) & 15) * 17;
    }
}

/// Encodes a 4x4 block of RGBA pixels to BC2 format: the colours as [`Rgbcx::encode_bc1`]
/// packs them at `level` without three-colour blocks, the alphas by [`encode_bc2_alpha`].
pub fn encode_bc2(encoder: &Rgbcx, level: u32, dst: &mut [u8; 16], pixels: &[Color32; 16]) {
    let (alpha, color) = dst.split_at_mut(8);
    encode_bc2_alpha(alpha.try_into().expect("8 bytes"), &pixels.as_flattened()[3..], 4);
    encoder.encode_bc1(level, color.try_into().expect("8 bytes"), pixels, false, false, None);
}

/// The low-level BC2 encoder: the BC1 flags and total orderings to try given directly, as for
/// [`Rgbcx::encode_bc3_with_flags`].
pub fn encode_bc2_with_flags(encoder: &Rgbcx, dst: &mut [u8; 16], pixels: &[Color32; 16], flags: u32, total_orderings_to_try: u32) {
    // 3-color blocks are not allowed with BC2, as with BC3.
    let flags = flags & !(rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS_FOR_BLACK_PIXELS | rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS);

    let (alpha, color) = dst.split_at_mut(8);
    encode_bc2_alpha(alpha.try_into().expect("8 bytes"), &pixels.as_flattened()[3..], 4);
    encoder.encode_bc1_with_flags(color.try_into().expect("8 bytes"), pixels, flags, total_orderings_to_try, rgbcx::DEFAULT_TOTAL_ORDERINGS_TO_TRY3, None);
}

/// The four colours of the colour half of a BC2 block, all opaque. A BC2 block always has
/// four colours: its two endpoints and the two between them, whichever endpoint is the larger
/// number (in BC1 that order chooses three colours and transparent black instead).
pub fn unpack_bc2_block_colors(color_block: &[u8; 8], mode: Bc1ApproxMode) -> [Color32; 4] {
    let low = u16::from_le_bytes([color_block[0], color_block[1]]);
    let high = u16::from_le_bytes([color_block[2], color_block[3]]);
    let mut colors = [[0u8; 4]; 4];
    if low > high {
        rgbcx::unpack_bc1_block_colors(color_block, &mut colors, mode);
    } else if low == high {
        // Between a colour and itself is that colour, every way of interpolating.
        rgbcx::unpack_bc1_block_colors(color_block, &mut colors, mode);
        colors = [colors[0]; 4];
    } else {
        // The other way round BC1 would give three colours: unpack with the endpoints
        // exchanged and put the four back in their places.
        let exchanged = [color_block[2], color_block[3], color_block[0], color_block[1], 0, 0, 0, 0];
        rgbcx::unpack_bc1_block_colors(&exchanged, &mut colors, mode);
        colors = [colors[1], colors[0], colors[3], colors[2]];
    }
    colors
}

/// Unpacks a BC2 block to 16 pixels, in rows from the top. `mode` is how the two colours
/// between the endpoints are worked out, as for BC1.
pub fn unpack_bc2(block: &[u8; 16], pixels: &mut [Color32; 16], mode: Bc1ApproxMode) {
    let (alpha, color) = block.split_at(8);
    let colors = unpack_bc2_block_colors(color.try_into().expect("8 bytes"), mode);
    for (i, pixel) in pixels.iter_mut().enumerate() {
        *pixel = colors[usize::from((color[4 + i / 4] >> ((i % 4) * 2)) & 3)];
    }
    unpack_bc2_alpha(alpha.try_into().expect("8 bytes"), &mut pixels.as_flattened_mut()[3..], 4);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_alpha_goes_to_its_nearest_level() {
        for alpha in 0..=255u8 {
            let mut block = [0u8; 8];
            encode_bc2_alpha(&mut block, &[alpha; 16], 1);
            let mut back = [0u8; 16];
            unpack_bc2_alpha(&block, &mut back, 1);
            let nearest = (0..16).map(|level| level * 17).min_by_key(|&level: &i32| (level - i32::from(alpha)).abs()).unwrap();
            assert_eq!(back, [nearest as u8; 16], "{alpha}");
        }
    }

    #[test]
    fn the_alphas_are_in_rows_low_bits_first() {
        let mut alphas = [0u8; 16];
        for (i, alpha) in alphas.iter_mut().enumerate() {
            *alpha = i as u8 * 17;
        }
        let mut block = [0u8; 8];
        encode_bc2_alpha(&mut block, &alphas, 1);
        assert_eq!(block, [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe]);
    }

    #[test]
    fn a_block_comes_back_near_its_pixels() {
        let encoder = Rgbcx::default();
        let mut pixels = [[0u8; 4]; 16];
        for (i, pixel) in pixels.iter_mut().enumerate() {
            *pixel = [40 + i as u8 * 2, 200 - i as u8, 90, (i as u8 % 4) * 85];
        }
        for level in [0, 5, 18] {
            let mut block = [0u8; 16];
            encode_bc2(&encoder, level, &mut block, &pixels);
            let mut with_flags = [0u8; 16];
            encode_bc2_with_flags(&encoder, &mut with_flags, &pixels, rgbcx::ENCODE_BC1_USE_3COLOR_BLOCKS, 4);
            for packed in [block, with_flags] {
                let mut back = [[0u8; 4]; 16];
                unpack_bc2(&packed, &mut back, Bc1ApproxMode::Ideal);
                for (a, b) in pixels.iter().zip(&back) {
                    assert_eq!(a[3], b[3]);
                    for c in 0..3 {
                        assert!((i32::from(a[c]) - i32::from(b[c])).abs() <= 12, "{a:?} {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_block_has_four_colours_whatever_the_order_of_its_endpoints() {
        // White and black; the selectors 0, 1, 2, 3 in each row.
        let forward = [0u8, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0xe4, 0xe4, 0xe4, 0xe4];
        let backward = [0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xe4, 0xe4, 0xe4, 0xe4];
        for mode in [Bc1ApproxMode::Ideal, Bc1ApproxMode::IdealRound4, Bc1ApproxMode::NVidia, Bc1ApproxMode::Amd] {
            let (mut a, mut b) = ([[0u8; 4]; 16], [[0u8; 4]; 16]);
            unpack_bc2(&forward, &mut a, mode);
            unpack_bc2(&backward, &mut b, mode);
            // The same four colours from the other end, none of them the black of BC1's
            // three-colour blocks where it should be a third of the way.
            assert_eq!([a[0], a[1], a[2], a[3]], [b[1], b[0], b[3], b[2]], "{mode:?}");
            assert!(b[2][0] > 60 && b[3][0] > 150, "{mode:?} {b:?}");
            assert!(a.iter().chain(&b).all(|pixel| pixel[3] == 0));
        }
        // Two endpoints the same: that colour for every selector.
        let same = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x34, 0x12, 0x34, 0x12, 0xe4, 0xe4, 0xe4, 0xe4];
        let mut pixels = [[0u8; 4]; 16];
        unpack_bc2(&same, &mut pixels, Bc1ApproxMode::NVidia);
        assert!(pixels.iter().all(|pixel| *pixel == pixels[0] && pixel[3] == 255));
    }
}
