//! An image of 8-bit RGBA pixels, one of HDR pixels, and what cutting them into blocks needs.

use crate::bc6h::{float_to_half, half_to_float, RgbHalf};
use crate::bc7decomp::ColorRgba;

/// An image: `width * height` pixels in rows from the top, each red, green, blue, alpha.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<ColorRgba>,
}

impl Image {
    /// An image of this size, black and opaque (as bc7enc_rdo's `image_u8` starts).
    pub fn new(width: u32, height: u32) -> Image {
        Image { width, height, pixels: vec![[0, 0, 0, 255]; width as usize * height as usize] }
    }

    /// An image from its pixels. None when there are not `width * height` of them.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<ColorRgba>) -> Option<Image> {
        (pixels.len() == width as usize * height as usize).then_some(Image { width, height, pixels })
    }

    /// An image from bytes, four to a pixel (red, green, blue, alpha). None when there are not
    /// `width * height * 4` of them.
    pub fn from_rgba8(width: u32, height: u32, bytes: &[u8]) -> Option<Image> {
        if bytes.len() != width as usize * height as usize * 4 {
            return None;
        }
        Some(Image { width, height, pixels: bytes.chunks_exact(4).map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]]).collect() })
    }

    /// The pixels as bytes, four to a pixel.
    pub fn as_rgba8(&self) -> &[u8] {
        self.pixels.as_flattened()
    }

    /// The pixel at a place.
    pub fn pixel(&self, x: u32, y: u32) -> ColorRgba {
        self.pixels[x as usize + y as usize * self.width as usize]
    }

    /// The pixel at a place, the place brought inside the image first.
    pub(crate) fn get_clamped(&self, x: i32, y: i32) -> ColorRgba {
        let x = x.clamp(0, self.width as i32 - 1);
        let y = y.clamp(0, self.height as i32 - 1);
        self.pixels[x as usize + y as usize * self.width as usize]
    }

    /// The 4x4 block at a place counted in blocks: `image_u8::get_block`.
    pub fn get_block(&self, bx: u32, by: u32) -> [ColorRgba; 16] {
        let mut block = [[0u8; 4]; 16];
        for y in 0..4usize {
            let at = bx as usize * 4 + (by as usize * 4 + y) * self.width as usize;
            block[y * 4..y * 4 + 4].copy_from_slice(&self.pixels[at..at + 4]);
        }
        block
    }

    /// Puts a 4x4 block at a place counted in blocks: `image_u8::set_block`.
    pub fn set_block(&mut self, bx: u32, by: u32, block: &[ColorRgba; 16]) {
        for y in 0..4usize {
            let at = bx as usize * 4 + (by as usize * 4 + y) * self.width as usize;
            self.pixels[at..at + 4].copy_from_slice(&block[y * 4..y * 4 + 4]);
        }
    }

    /// Makes the image this size, keeping what fits, and fills what is new to the right and
    /// below by repeating the last column and row: `image_u8::crop_dup_borders`.
    pub fn crop_dup_borders(&mut self, width: u32, height: u32) {
        let (orig_w, orig_h) = (self.width, self.height);
        if orig_w == width && orig_h == height {
            return;
        }
        let mut new_image = Image::new(width, height);
        for y in 0..orig_h.min(height) {
            for x in 0..orig_w.min(width) {
                new_image.pixels[x as usize + y as usize * width as usize] = self.pixel(x, y);
            }
        }
        if orig_w != 0 && orig_h != 0 {
            // (From the new image, as the C++ reads it: the corner past both edges comes from
            // pixels the first pass has filled.)
            if width > orig_w {
                for x in orig_w..width {
                    for y in 0..height {
                        new_image.pixels[x as usize + y as usize * width as usize] = new_image.get_clamped(x.min(orig_w - 1) as i32, y.min(orig_h - 1) as i32);
                    }
                }
            }
            if height > orig_h {
                for y in orig_h..height {
                    for x in 0..width {
                        new_image.pixels[x as usize + y as usize * width as usize] = new_image.get_clamped(x.min(orig_w - 1) as i32, y.min(orig_h - 1) as i32);
                    }
                }
            }
        }
        *self = new_image;
    }

    /// The image at half the size each way (rounded down, but never to nothing), each pixel the
    /// average of the two by two it covers, rounded to nearest: the next mip level. The values
    /// are averaged as they are stored, alpha like the rest. (The crate's own: bc7enc_rdo
    /// makes no mip levels.)
    pub fn half(&self) -> Image {
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut half = Image::new(width, height);
        for y in 0..height {
            for x in 0..width {
                // (A side of one pixel has nothing beside it: the pixel counts twice.)
                let (x0, y0) = ((x * 2).min(self.width - 1), (y * 2).min(self.height - 1));
                let (x1, y1) = ((x * 2 + 1).min(self.width - 1), (y * 2 + 1).min(self.height - 1));
                let (a, b, c, d) = (self.pixel(x0, y0), self.pixel(x1, y0), self.pixel(x0, y1), self.pixel(x1, y1));
                let mut pixel = [0u8; 4];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    *value = ((u32::from(a[channel]) + u32::from(b[channel]) + u32::from(c[channel]) + u32::from(d[channel]) + 2) / 4) as u8;
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }

    /// The image upside down.
    pub fn flipped_y(&self) -> Image {
        let mut flipped = Image::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                flipped.pixels[x as usize + (self.height - 1 - y) as usize * self.width as usize] = self.pixel(x, y);
            }
        }
        flipped
    }
}

/// An image of HDR colour: `width * height` pixels in rows from the top, each red, green and
/// blue as half floats. (The crate's own.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HdrImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<RgbHalf>,
}

impl HdrImage {
    /// An image of this size, black.
    pub fn new(width: u32, height: u32) -> HdrImage {
        HdrImage { width, height, pixels: vec![[0; 3]; width as usize * height as usize] }
    }

    /// An image from floats, three to a pixel (red, green, blue). Each is brought into what an
    /// unsigned BC6H block can hold: below zero and not-a-number become 0, above the largest
    /// half (65504) becomes it. None when there are not `width * height * 3` of them.
    pub fn from_rgb_f32(width: u32, height: u32, values: &[f32]) -> Option<HdrImage> {
        if values.len() != width as usize * height as usize * 3 {
            return None;
        }
        let half = |value: f32| if value > 0.0 { float_to_half(value).min(0x7bff) } else { 0 };
        Some(HdrImage { width, height, pixels: values.chunks_exact(3).map(|pixel| [half(pixel[0]), half(pixel[1]), half(pixel[2])]).collect() })
    }

    /// The pixels as floats, three to a pixel.
    pub fn to_rgb_f32(&self) -> Vec<f32> {
        self.pixels.as_flattened().iter().map(|&half| half_to_float(half)).collect()
    }

    /// The pixel at a place.
    pub fn pixel(&self, x: u32, y: u32) -> RgbHalf {
        self.pixels[x as usize + y as usize * self.width as usize]
    }

    /// The 4x4 block at a place counted in blocks; past the right and bottom edges the last
    /// column and row are repeated.
    pub fn get_block(&self, bx: u32, by: u32) -> [RgbHalf; 16] {
        let mut block = [[0u16; 3]; 16];
        for y in 0..4 {
            for x in 0..4 {
                block[(y * 4 + x) as usize] = self.pixel((bx * 4 + x).min(self.width - 1), (by * 4 + y).min(self.height - 1));
            }
        }
        block
    }

    /// The image at half the size each way (rounded down, but never to nothing), each pixel the
    /// average of the two by two it covers, worked out in floats: the next mip level.
    pub fn half(&self) -> HdrImage {
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut half = HdrImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let (x0, y0) = ((x * 2).min(self.width - 1), (y * 2).min(self.height - 1));
                let (x1, y1) = ((x * 2 + 1).min(self.width - 1), (y * 2 + 1).min(self.height - 1));
                let (a, b, c, d) = (self.pixel(x0, y0), self.pixel(x1, y0), self.pixel(x0, y1), self.pixel(x1, y1));
                let mut pixel = [0u16; 3];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    let sum = half_to_float(a[channel]) + half_to_float(b[channel]) + half_to_float(c[channel]) + half_to_float(d[channel]);
                    *value = float_to_half(sum * 0.25);
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_column_and_row_are_repeated() {
        let mut image = Image::new(3, 2);
        for (i, pixel) in image.pixels.iter_mut().enumerate() {
            *pixel = [i as u8, 0, 0, 255];
        }
        image.crop_dup_borders(4, 4);
        let red: Vec<u8> = image.pixels.iter().map(|pixel| pixel[0]).collect();
        assert_eq!(red, [0, 1, 2, 2, 3, 4, 5, 5, 3, 4, 5, 5, 3, 4, 5, 5]);
        assert_eq!(image.get_block(0, 0)[5], [4, 0, 0, 255]);
        // Smaller cuts off.
        image.crop_dup_borders(2, 1);
        assert_eq!(image.pixels, [[0, 0, 0, 255], [1, 0, 0, 255]]);
    }

    #[test]
    fn half_averages_two_by_two() {
        let image = Image::from_rgba8(4, 2, &[0, 0, 0, 255, 2, 0, 0, 255, 10, 0, 0, 255, 10, 0, 0, 255, 4, 0, 0, 255, 7, 0, 0, 255, 10, 0, 0, 255, 11, 0, 0, 255]).unwrap();
        let half = image.half();
        assert_eq!((half.width, half.height), (2, 1));
        // (0 + 2 + 4 + 7 + 2) / 4 and (10 + 10 + 10 + 11 + 2) / 4.
        assert_eq!(half.pixels, [[3, 0, 0, 255], [10, 0, 0, 255]]);
        let last = half.half();
        assert_eq!((last.width, last.height), (1, 1));
        assert_eq!(last.pixels, [[7, 0, 0, 255]]);
        assert_eq!(last.half(), last);
        // An odd side leaves its last column out.
        assert_eq!(Image::new(5, 3).half().width, 2);
    }

    #[test]
    fn hdr_images_take_floats_and_halve() {
        let image = HdrImage::from_rgb_f32(2, 2, &[1.0, 0.5, 0.0, 3.0, -2.0, f32::NAN, 0.0, 1e9, 2.0, 4.0, 0.25, f32::INFINITY]).unwrap();
        // Below zero and not-a-number are 0; too large is the largest half.
        assert_eq!(image.pixels, [[0x3c00, 0x3800, 0], [0x4200, 0, 0], [0, 0x7bff, 0x4000], [0x4400, 0x3400, 0x7bff]]);
        assert_eq!(image.to_rgb_f32()[..4], [1.0, 0.5, 0.0, 3.0]);
        let half = image.half();
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half_to_float(half.pixels[0][0]), 2.0);
        // Past the edge a block repeats the last column and row.
        let block = image.get_block(0, 0);
        assert_eq!(block[0], image.pixels[0]);
        assert_eq!(block[3], image.pixels[1]);
        assert_eq!(block[15], image.pixels[3]);
        assert!(HdrImage::from_rgb_f32(2, 2, &[0.0; 11]).is_none());
    }

    #[test]
    fn bytes_and_pixels_agree() {
        let image = Image::from_rgba8(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(image.pixel(1, 0), [5, 6, 7, 8]);
        assert_eq!(image.as_rgba8(), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(Image::from_rgba8(2, 2, &[0; 8]).is_none());
        assert_eq!(image.flipped_y(), image);
    }
}
