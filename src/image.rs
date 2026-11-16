//! An image of 8-bit RGBA pixels, one of HDR pixels, what cutting them into blocks needs, and
//! the first to and from `.png`, `.bmp` and `.jpg` files and the second to and from `.exr`
//! files.

use std::path::Path;

use crate::bc6h::{float_to_half, half_to_float, RgbHalf};
use crate::bc7decomp::ColorRgba;
use crate::stb_image::{self, LoadError, LoadOptions};
use crate::stb_image_write::{self, WriteOptions};
use crate::tinyexr::{self, ChannelData, ChannelInfo, ExrError, ExrHeader, ExrImage};

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
        Image {
            width,
            height,
            pixels: vec![[0, 0, 0, 255]; width as usize * height as usize],
        }
    }

    /// An image from its pixels. None when there are not `width * height` of them.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<ColorRgba>) -> Option<Image> {
        (pixels.len() == width as usize * height as usize).then_some(Image {
            width,
            height,
            pixels,
        })
    }

    /// An image from bytes, four to a pixel (red, green, blue, alpha). None when there are not
    /// `width * height * 4` of them.
    pub fn from_rgba8(width: u32, height: u32, bytes: &[u8]) -> Option<Image> {
        if bytes.len() != width as usize * height as usize * 4 {
            return None;
        }
        Some(Image {
            width,
            height,
            pixels: bytes
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
                .collect(),
        })
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
                        new_image.pixels[x as usize + y as usize * width as usize] = new_image
                            .get_clamped(x.min(orig_w - 1) as i32, y.min(orig_h - 1) as i32);
                    }
                }
            }
            if height > orig_h {
                for y in orig_h..height {
                    for x in 0..width {
                        new_image.pixels[x as usize + y as usize * width as usize] = new_image
                            .get_clamped(x.min(orig_w - 1) as i32, y.min(orig_h - 1) as i32);
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
                let (x1, y1) = (
                    (x * 2 + 1).min(self.width - 1),
                    (y * 2 + 1).min(self.height - 1),
                );
                let (a, b, c, d) = (
                    self.pixel(x0, y0),
                    self.pixel(x1, y0),
                    self.pixel(x0, y1),
                    self.pixel(x1, y1),
                );
                let mut pixel = [0u8; 4];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    *value = ((u32::from(a[channel])
                        + u32::from(b[channel])
                        + u32::from(c[channel])
                        + u32::from(d[channel])
                        + 2)
                        / 4) as u8;
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }

    /// The image at half the size each way, each pixel the average of the two by two it covers
    /// in this image and in `behind`, an image of the same size: two slices of a volume to one
    /// of its next mip level. With itself behind it, [`Image::half`].
    ///
    /// # Panics
    ///
    /// When the two are not of one size.
    pub fn half_with(&self, behind: &Image) -> Image {
        assert_eq!(
            (self.width, self.height),
            (behind.width, behind.height),
            "two slices of one size"
        );
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut half = Image::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let (x0, y0) = ((x * 2).min(self.width - 1), (y * 2).min(self.height - 1));
                let (x1, y1) = (
                    (x * 2 + 1).min(self.width - 1),
                    (y * 2 + 1).min(self.height - 1),
                );
                let mut pixel = [0u8; 4];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    let sum: u32 = [self, behind]
                        .iter()
                        .flat_map(|slice| {
                            [
                                slice.pixel(x0, y0),
                                slice.pixel(x1, y0),
                                slice.pixel(x0, y1),
                                slice.pixel(x1, y1),
                            ]
                        })
                        .map(|pixel| u32::from(pixel[channel]))
                        .sum();
                    *value = ((sum + 4) / 8) as u8;
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }

    /// The next mip level of a volume, given as its slices from the front: half as many slices
    /// (rounded down, but never none), each the average of two ([`Image::half_with`]; a last
    /// slice with none behind it counts twice).
    pub fn half_volume(slices: &[Image]) -> Vec<Image> {
        if slices.is_empty() {
            return Vec::new();
        }
        (0..(slices.len() / 2).max(1))
            .map(|z| {
                slices[(z * 2).min(slices.len() - 1)]
                    .half_with(&slices[(z * 2 + 1).min(slices.len() - 1)])
            })
            .collect()
    }

    /// The image upside down.
    pub fn flipped_y(&self) -> Image {
        let mut flipped = Image::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                flipped.pixels[x as usize + (self.height - 1 - y) as usize * self.width as usize] =
                    self.pixel(x, y);
            }
        }
        flipped
    }

    /// The image in the bytes of a `.png`, `.bmp`, `.jpg`, `.tga`, `.psd`, `.gif`, `.pgm`,
    /// `.ppm` or `.hdr` file, read by [`crate::stb_image`]. A file of fewer channels gets the
    /// rest made up (grey in all three colours, opaque), one of 16 bits a channel is brought
    /// down to 8, and an HDR file's floats with a gamma of 2.2. Of a GIF, the first frame.
    pub fn from_file_bytes(file: &[u8]) -> Result<Image, LoadError> {
        let loaded = stb_image::load_from_memory(file, 4, &LoadOptions::default())?;
        Ok(Image {
            width: loaded.x as u32,
            height: loaded.y as u32,
            pixels: loaded
                .data
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
                .collect(),
        })
    }

    /// Reads an image file from disk, of any kind [`Image::from_file_bytes`] reads. A file that
    /// is none of them, or is damaged, comes back as an error of the kind `InvalidData`, with
    /// the [`LoadError`] inside.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Image> {
        Image::from_file_bytes(&std::fs::read(path)?)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Whether every pixel is opaque.
    pub fn is_opaque(&self) -> bool {
        self.pixels.iter().all(|pixel| pixel[3] == 255)
    }

    /// The pixels as the writers take them: three bytes each when all are opaque, else four.
    fn for_file(&self) -> Option<(i32, i32, i32, Vec<u8>)> {
        let (width, height) = (
            i32::try_from(self.width).ok()?,
            i32::try_from(self.height).ok()?,
        );
        if self.pixels.len() != self.width as usize * self.height as usize {
            return None;
        }
        if self.is_opaque() {
            Some((
                width,
                height,
                3,
                self.pixels
                    .iter()
                    .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                    .collect(),
            ))
        } else {
            Some((width, height, 4, self.as_rgba8().to_vec()))
        }
    }

    /// The image as the bytes of a `.png` file, written by [`crate::stb_image_write`]: with
    /// alpha unless every pixel is opaque. None for an image of no pixels.
    pub fn to_png(&self) -> Option<Vec<u8>> {
        let (width, height, channels, bytes) = self.for_file()?;
        stb_image_write::write_png(&bytes, 0, width, height, channels, &WriteOptions::default())
    }

    /// The image as the bytes of a `.bmp` file: 32 bits a pixel with alpha, or 24 when every
    /// pixel is opaque. None for an image of no pixels.
    pub fn to_bmp(&self) -> Option<Vec<u8>> {
        let (width, height, channels, bytes) = self.for_file()?;
        stb_image_write::write_bmp(width, height, channels, &bytes, 0, &WriteOptions::default())
    }

    /// The image as the bytes of a `.jpg` file, at a quality from 1 to 100. The format holds
    /// no alpha, and loses a little of the colour. None for an image of no pixels.
    pub fn to_jpg(&self, quality: i32) -> Option<Vec<u8>> {
        let (width, height) = (
            i32::try_from(self.width).ok()?,
            i32::try_from(self.height).ok()?,
        );
        if self.pixels.len() != self.width as usize * self.height as usize {
            return None;
        }
        stb_image_write::write_jpg(
            width,
            height,
            4,
            self.as_rgba8(),
            quality.clamp(1, 100),
            0,
            &WriteOptions::default(),
        )
    }

    /// Writes the image to disk as the file its name ends in says: `.png`, `.bmp`, or `.jpg`
    /// or `.jpeg` (at quality 90; [`Image::to_jpg`] for another). Any other ending, and an
    /// image of no pixels, is an error of the kind `InvalidInput`.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        let file = match extension.as_deref() {
            Some("png") => self.to_png(),
            Some("bmp") => self.to_bmp(),
            Some("jpg" | "jpeg") => self.to_jpg(90),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the file name has to end in .png, .bmp, .jpg or .jpeg",
                ))
            }
        };
        let file = file.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "the image has no pixels")
        })?;
        std::fs::write(path, file)
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
        HdrImage {
            width,
            height,
            pixels: vec![[0; 3]; width as usize * height as usize],
        }
    }

    /// An image from floats, three to a pixel (red, green, blue). Each is brought into what an
    /// unsigned BC6H block can hold: below zero and not-a-number become 0, above the largest
    /// half (65504) becomes it. None when there are not `width * height * 3` of them.
    pub fn from_rgb_f32(width: u32, height: u32, values: &[f32]) -> Option<HdrImage> {
        if values.len() != width as usize * height as usize * 3 {
            return None;
        }
        let half = |value: f32| {
            if value > 0.0 {
                float_to_half(value).min(0x7bff)
            } else {
                0
            }
        };
        Some(HdrImage {
            width,
            height,
            pixels: values
                .chunks_exact(3)
                .map(|pixel| [half(pixel[0]), half(pixel[1]), half(pixel[2])])
                .collect(),
        })
    }

    /// An image from floats, three to a pixel, with what is below zero kept: for signed BC6H.
    /// Each is brought into what a half holds: beyond the largest half (65504) either way
    /// becomes it, and not-a-number 0. None when there are not `width * height * 3` of them.
    pub fn from_rgb_f32_signed(width: u32, height: u32, values: &[f32]) -> Option<HdrImage> {
        if values.len() != width as usize * height as usize * 3 {
            return None;
        }
        let half = |value: f32| {
            if value.is_nan() {
                0
            } else {
                float_to_half(value.clamp(-65504.0, 65504.0))
            }
        };
        Some(HdrImage {
            width,
            height,
            pixels: values
                .chunks_exact(3)
                .map(|pixel| [half(pixel[0]), half(pixel[1]), half(pixel[2])])
                .collect(),
        })
    }

    /// The pixels as floats, three to a pixel.
    pub fn to_rgb_f32(&self) -> Vec<f32> {
        self.pixels
            .as_flattened()
            .iter()
            .map(|&half| half_to_float(half))
            .collect()
    }

    /// The red, green and blue of a `.exr` file as floats, read by [`crate::tinyexr`].
    fn exr_floats(file: &[u8]) -> Result<(u32, u32, Vec<f32>), ExrError> {
        let (rgba, width, height) = tinyexr::load_exr_from_memory(file)?;
        Ok((
            width as u32,
            height as u32,
            rgba.chunks_exact(4)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect(),
        ))
    }

    /// The image in the bytes of a `.exr` file, read by [`crate::tinyexr`]: its channels
    /// `R`, `G` and `B` (a file of one channel gives it as all three), whether the file holds
    /// halves or floats; any other channels are left. Each value is brought into what an
    /// unsigned BC6H block can hold, as [`HdrImage::from_rgb_f32`] brings it.
    pub fn from_file_bytes(file: &[u8]) -> Result<HdrImage, ExrError> {
        let (width, height, floats) = HdrImage::exr_floats(file)?;
        Ok(HdrImage::from_rgb_f32(width, height, &floats).unwrap_or_default())
    }

    /// The image in the bytes of a `.exr` file, with what is below zero kept: for signed
    /// BC6H, as [`HdrImage::from_rgb_f32_signed`] brings it.
    pub fn from_file_bytes_signed(file: &[u8]) -> Result<HdrImage, ExrError> {
        let (width, height, floats) = HdrImage::exr_floats(file)?;
        Ok(HdrImage::from_rgb_f32_signed(width, height, &floats).unwrap_or_default())
    }

    /// The image in the bytes of a Radiance `.hdr` file, read by [`crate::stb_image`]. (Its
    /// values are never below zero.)
    pub fn from_hdr_bytes(file: &[u8]) -> Result<HdrImage, LoadError> {
        let loaded = stb_image::loadf_from_memory(file, 3, &LoadOptions::default())?;
        Ok(
            HdrImage::from_rgb_f32(loaded.x as u32, loaded.y as u32, &loaded.data)
                .unwrap_or_default(),
        )
    }

    /// Reads a `.exr` or Radiance `.hdr` file from disk, whichever its bytes say it is. A file
    /// that is neither, or is damaged, comes back as an error of the kind `InvalidData`, with
    /// the [`ExrError`] or [`LoadError`] inside.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<HdrImage> {
        let file = std::fs::read(path)?;
        if stb_image::is_hdr_from_memory(&file) {
            return HdrImage::from_hdr_bytes(&file)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
        HdrImage::from_file_bytes(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Reads a `.exr` file from disk, with what is below zero kept
    /// ([`HdrImage::from_file_bytes_signed`]); or a Radiance `.hdr` file, which has nothing
    /// below zero.
    pub fn load_signed(path: impl AsRef<Path>) -> std::io::Result<HdrImage> {
        let file = std::fs::read(path)?;
        if stb_image::is_hdr_from_memory(&file) {
            return HdrImage::from_hdr_bytes(&file)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
        HdrImage::from_file_bytes_signed(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// The image as the bytes of a `.exr` file, written by [`crate::tinyexr`]: channels `B`,
    /// `G` and `R` of halves, as the pixels are, in scanlines, compressed with ZIP (which
    /// loses nothing), or not at all when the image is less than sixteen pixels each way, as
    /// tinyexr's `SaveEXR` chooses. None for an image of no pixels.
    pub fn to_exr(&self) -> Option<Vec<u8>> {
        let (width, height) = (
            i32::try_from(self.width).ok()?,
            i32::try_from(self.height).ok()?,
        );
        if self.pixels.is_empty() || self.pixels.len() != self.width as usize * self.height as usize
        {
            return None;
        }
        let channel =
            |k: usize| ChannelData::Half(self.pixels.iter().map(|pixel| pixel[k]).collect());
        let image = ExrImage {
            images: vec![channel(2), channel(1), channel(0)],
            width,
            height,
            num_channels: 3,
            ..ExrImage::default()
        };
        let header = ExrHeader {
            channels: ["B", "G", "R"]
                .iter()
                .map(|name| ChannelInfo {
                    name: name.to_string(),
                    pixel_type: tinyexr::PIXELTYPE_HALF,
                    x_sampling: 1,
                    y_sampling: 1,
                    p_linear: 0,
                })
                .collect(),
            pixel_types: vec![tinyexr::PIXELTYPE_HALF; 3],
            requested_pixel_types: vec![tinyexr::PIXELTYPE_HALF; 3],
            compression_type: if width < 16 && height < 16 {
                tinyexr::COMPRESSIONTYPE_NONE
            } else {
                tinyexr::COMPRESSIONTYPE_ZIP
            },
            ..ExrHeader::default()
        };
        tinyexr::save_exr_image_to_memory(&image, &header).ok()
    }

    /// Writes the image to disk as a `.exr` file. A name with any other ending, and an image
    /// of no pixels, is an error of the kind `InvalidInput`.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exr"))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the file name has to end in .exr",
            ));
        }
        let file = self.to_exr().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "the image has no pixels")
        })?;
        std::fs::write(path, file)
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
                block[(y * 4 + x) as usize] = self.pixel(
                    (bx * 4 + x).min(self.width - 1),
                    (by * 4 + y).min(self.height - 1),
                );
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
                let (x1, y1) = (
                    (x * 2 + 1).min(self.width - 1),
                    (y * 2 + 1).min(self.height - 1),
                );
                let (a, b, c, d) = (
                    self.pixel(x0, y0),
                    self.pixel(x1, y0),
                    self.pixel(x0, y1),
                    self.pixel(x1, y1),
                );
                let mut pixel = [0u16; 3];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    let sum = half_to_float(a[channel])
                        + half_to_float(b[channel])
                        + half_to_float(c[channel])
                        + half_to_float(d[channel]);
                    *value = float_to_half(sum * 0.25);
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }

    /// The image at half the size each way, each pixel the average of the two by two it covers
    /// in this image and in `behind`, an image of the same size: two slices of a volume to one
    /// of its next mip level. With itself behind it, [`HdrImage::half`].
    ///
    /// # Panics
    ///
    /// When the two are not of one size.
    pub fn half_with(&self, behind: &HdrImage) -> HdrImage {
        assert_eq!(
            (self.width, self.height),
            (behind.width, behind.height),
            "two slices of one size"
        );
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut half = HdrImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let (x0, y0) = ((x * 2).min(self.width - 1), (y * 2).min(self.height - 1));
                let (x1, y1) = (
                    (x * 2 + 1).min(self.width - 1),
                    (y * 2 + 1).min(self.height - 1),
                );
                let mut pixel = [0u16; 3];
                for (channel, value) in pixel.iter_mut().enumerate() {
                    let sum_of = |slice: &HdrImage| {
                        half_to_float(slice.pixel(x0, y0)[channel])
                            + half_to_float(slice.pixel(x1, y0)[channel])
                            + half_to_float(slice.pixel(x0, y1)[channel])
                            + half_to_float(slice.pixel(x1, y1)[channel])
                    };
                    *value = float_to_half((sum_of(self) + sum_of(behind)) * 0.125);
                }
                half.pixels[(x + y * width) as usize] = pixel;
            }
        }
        half
    }

    /// The next mip level of a volume, given as its slices from the front: half as many slices
    /// (rounded down, but never none), each the average of two ([`HdrImage::half_with`]; a
    /// last slice with none behind it counts twice).
    pub fn half_volume(slices: &[HdrImage]) -> Vec<HdrImage> {
        if slices.is_empty() {
            return Vec::new();
        }
        (0..(slices.len() / 2).max(1))
            .map(|z| {
                slices[(z * 2).min(slices.len() - 1)]
                    .half_with(&slices[(z * 2 + 1).min(slices.len() - 1)])
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_go_to_files_and_come_back() {
        let mut image = Image::new(13, 9);
        for (i, pixel) in image.pixels.iter_mut().enumerate() {
            *pixel = [
                (i * 2) as u8,
                100 + (i % 13) as u8,
                200 - (i / 13) as u8,
                255,
            ];
        }
        // Opaque: three channels in the file, the same pixels back.
        let png = image.to_png().unwrap();
        assert_eq!(png[25], 2);
        assert_eq!(Image::from_file_bytes(&png).unwrap(), image);
        let bmp = image.to_bmp().unwrap();
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 24);
        assert_eq!(Image::from_file_bytes(&bmp).unwrap(), image);
        let jpg = Image::from_file_bytes(&image.to_jpg(95).unwrap()).unwrap();
        assert_eq!((jpg.width, jpg.height), (13, 9));
        assert!(jpg
            .pixels
            .iter()
            .zip(&image.pixels)
            .all(|(a, b)| (0..3).all(|c| a[c].abs_diff(b[c]) < 40) && a[3] == 255));
        // With alpha: four.
        image.pixels[5][3] = 17;
        let png = image.to_png().unwrap();
        assert_eq!(png[25], 6);
        assert_eq!(Image::from_file_bytes(&png).unwrap(), image);
        let bmp = image.to_bmp().unwrap();
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 32);
        assert_eq!(Image::from_file_bytes(&bmp).unwrap(), image);
        // To disk, by the name's ending.
        let folder =
            std::env::temp_dir().join(format!("rustbcx-image-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["a.png", "a.BMP"] {
            image.save(folder.join(name)).unwrap();
            assert_eq!(Image::load(folder.join(name)).unwrap(), image);
        }
        image.save(folder.join("a.jpeg")).unwrap();
        assert_eq!(Image::load(folder.join("a.jpeg")).unwrap().width, 13);
        assert_eq!(
            image.save(folder.join("a.gif")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        std::fs::write(folder.join("b.png"), b"not a picture").unwrap();
        assert_eq!(
            Image::load(folder.join("b.png")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(&folder).unwrap();
        // No pixels, no file.
        assert!(
            Image::new(0, 4).to_png().is_none()
                && Image::new(0, 4).to_bmp().is_none()
                && Image::new(4, 0).to_jpg(90).is_none()
        );
        assert_eq!(
            Image::from_file_bytes(&[]),
            Err(LoadError("unknown image type"))
        );
    }

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
        let image = Image::from_rgba8(
            4,
            2,
            &[
                0, 0, 0, 255, 2, 0, 0, 255, 10, 0, 0, 255, 10, 0, 0, 255, 4, 0, 0, 255, 7, 0, 0,
                255, 10, 0, 0, 255, 11, 0, 0, 255,
            ],
        )
        .unwrap();
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
    fn hdr_images_are_written_and_read_as_exr() {
        // Small enough to be written as it is, and large enough to be compressed.
        for (width, height) in [(3u32, 2u32), (40, 17)] {
            let mut image = HdrImage::new(width, height);
            for (i, pixel) in image.pixels.iter_mut().enumerate() {
                // Halves from 0 up to 4 or so, a different one for each colour.
                *pixel = [
                    0x3000 + (i as u16 * 7) % 0x1000,
                    (i as u16 * 13) % 0x4400,
                    0x3c00,
                ];
            }
            let file = image.to_exr().unwrap();
            assert_eq!(&file[..4], &[0x76, 0x2f, 0x31, 0x01]);
            assert_eq!(HdrImage::from_file_bytes(&file).unwrap(), image);
            assert_eq!(HdrImage::from_file_bytes_signed(&file).unwrap(), image);
            // The file is what tinyexr's reader says: three channels of halves.
            let version = tinyexr::parse_exr_version_from_memory(&file).unwrap();
            let header = tinyexr::parse_exr_header_from_memory(&version, &file).unwrap();
            assert_eq!(
                header
                    .channels
                    .iter()
                    .map(|channel| channel.name.as_str())
                    .collect::<Vec<_>>(),
                ["B", "G", "R"]
            );
            assert_eq!(header.pixel_types, [tinyexr::PIXELTYPE_HALF; 3]);
            assert_eq!(
                header.compression_type,
                if width < 16 {
                    tinyexr::COMPRESSIONTYPE_NONE
                } else {
                    tinyexr::COMPRESSIONTYPE_ZIP
                }
            );
        }

        // What is below zero is kept only when asked for; floats in a file come in as halves.
        let file = tinyexr::save_exr_to_memory(&[-1.0, 0.5, 2.0, 1e9, 0.25, -0.5], 2, 1, 3, false)
            .unwrap();
        let image = HdrImage::from_file_bytes(&file).unwrap();
        assert_eq!(image.to_rgb_f32(), [0.0, 0.5, 2.0, 65504.0, 0.25, 0.0]);
        let signed = HdrImage::from_file_bytes_signed(&file).unwrap();
        assert_eq!(signed.to_rgb_f32(), [-1.0, 0.5, 2.0, 65504.0, 0.25, -0.5]);

        // To disk, by the name's ending.
        let folder =
            std::env::temp_dir().join(format!("rustbcx-hdr-image-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        signed.save(folder.join("a.EXR")).unwrap();
        assert_eq!(HdrImage::load(folder.join("a.EXR")).unwrap(), image);
        assert_eq!(HdrImage::load_signed(folder.join("a.EXR")).unwrap(), signed);
        assert_eq!(
            signed.save(folder.join("a.hdr")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        std::fs::write(folder.join("b.exr"), b"not a picture").unwrap();
        let error = HdrImage::load(folder.join("b.exr")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "Failed to parse EXR version. code(-1)");
        std::fs::remove_dir_all(&folder).unwrap();
        // No pixels, no file.
        assert!(HdrImage::new(0, 4).to_exr().is_none());
    }

    #[test]
    fn hdr_images_take_floats_and_halve() {
        let image = HdrImage::from_rgb_f32(
            2,
            2,
            &[
                1.0,
                0.5,
                0.0,
                3.0,
                -2.0,
                f32::NAN,
                0.0,
                1e9,
                2.0,
                4.0,
                0.25,
                f32::INFINITY,
            ],
        )
        .unwrap();
        // Below zero and not-a-number are 0; too large is the largest half.
        assert_eq!(
            image.pixels,
            [
                [0x3c00, 0x3800, 0],
                [0x4200, 0, 0],
                [0, 0x7bff, 0x4000],
                [0x4400, 0x3400, 0x7bff]
            ]
        );
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
        // With the sign kept: below zero stays, and the ends are the largest halves.
        let signed =
            HdrImage::from_rgb_f32_signed(1, 2, &[-1.0, 0.5, f32::NAN, -1e9, f32::INFINITY, -0.0])
                .unwrap();
        assert_eq!(
            signed.pixels,
            [[0xbc00, 0x3800, 0], [0xfbff, 0x7bff, 0x8000]]
        );
        assert_eq!(signed.to_rgb_f32()[..2], [-1.0, 0.5]);
        assert!(HdrImage::from_rgb_f32_signed(1, 2, &[0.0; 5]).is_none());
    }

    #[test]
    fn a_volume_halves_its_slices_too() {
        let slice = |value: u8| Image::from_pixels(2, 2, vec![[value, 0, 0, 255]; 4]).unwrap();
        // Five slices to two: the first two and the next two; the fifth is left out, as an odd
        // column is.
        let level = Image::half_volume(&[slice(10), slice(20), slice(30), slice(41), slice(200)]);
        assert_eq!(level.len(), 2);
        assert_eq!((level[0].width, level[0].height), (1, 1));
        assert_eq!((level[0].pixels[0][0], level[1].pixels[0][0]), (15, 36));
        // One slice halves as an image by itself does.
        let mut lone = Image::new(5, 3);
        for (i, pixel) in lone.pixels.iter_mut().enumerate() {
            *pixel = [(i * 17) as u8, (i * 5) as u8, 3, 255];
        }
        assert_eq!(
            Image::half_volume(std::slice::from_ref(&lone)),
            [lone.half()]
        );
        assert!(Image::half_volume(&[]).is_empty());

        let hdr = |value: f32| HdrImage::from_rgb_f32(2, 2, &[value; 12]).unwrap();
        let level = HdrImage::half_volume(&[hdr(1.0), hdr(2.0), hdr(8.0)]);
        assert_eq!(level.len(), 1);
        assert_eq!(half_to_float(level[0].pixels[0][1]), 1.5);
        let lone = HdrImage::from_rgb_f32(
            2,
            2,
            &[0.1, 0.2, 0.3, 1.5, 2.5, 3.5, 7.0, 9.0, 11.0, 0.7, 0.9, 1.1],
        )
        .unwrap();
        assert_eq!(
            HdrImage::half_volume(std::slice::from_ref(&lone)),
            [lone.half()]
        );
        assert!(HdrImage::half_volume(&[]).is_empty());
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
