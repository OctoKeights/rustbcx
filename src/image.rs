//! An image of 8-bit RGBA pixels, and what cutting it into blocks needs.

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
    fn bytes_and_pixels_agree() {
        let image = Image::from_rgba8(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(image.pixel(1, 0), [5, 6, 7, 8]);
        assert_eq!(image.as_rgba8(), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(Image::from_rgba8(2, 2, &[0; 8]).is_none());
        assert_eq!(image.flipped_y(), image);
    }
}
