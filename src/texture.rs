//! A packed texture: a format, a size, and the blocks of each mip level. What the file modules
//! ([`crate::dds`], [`crate::ktx`]) read and write, and the short way from an image to one.
//!
//! (The crate's own: bc7enc_rdo stops at the blocks of one image.)

use crate::bc6h::{self, Bc6hEncSettings};
use crate::bc7decomp;
use crate::image::{HdrImage, Image};
use crate::parallel;
use crate::{dds, ktx};
use std::path::Path;
use crate::rdo_bc_encoder::{BlockFormat, RdoBcEncoder, RdoBcParams};
use crate::rgbcx::{self, Bc1ApproxMode};

/// The block formats a texture can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// RGB, 8 bytes a block.
    Bc1,
    /// RGB with alpha, 16 bytes a block.
    Bc3,
    /// One channel, 8 bytes a block.
    Bc4,
    /// Two channels, 16 bytes a block.
    Bc5,
    /// HDR RGB, values of zero and above, 16 bytes a block.
    Bc6hUnsigned,
    /// HDR RGB, values of either sign, 16 bytes a block.
    Bc6hSigned,
    /// RGB or RGBA, 16 bytes a block.
    Bc7,
}

impl TextureFormat {
    /// The bytes of a block.
    pub fn bytes_per_block(self) -> usize {
        match self {
            TextureFormat::Bc1 | TextureFormat::Bc4 => 8,
            _ => 16,
        }
    }

    /// Whether a file can mark the format as sRGB: BC1, BC3 and BC7.
    pub fn can_be_srgb(self) -> bool {
        matches!(self, TextureFormat::Bc1 | TextureFormat::Bc3 | TextureFormat::Bc7)
    }
}

impl From<BlockFormat> for TextureFormat {
    fn from(format: BlockFormat) -> TextureFormat {
        match format {
            BlockFormat::Bc1 => TextureFormat::Bc1,
            BlockFormat::Bc3 => TextureFormat::Bc3,
            BlockFormat::Bc4 => TextureFormat::Bc4,
            BlockFormat::Bc5 => TextureFormat::Bc5,
            BlockFormat::Bc7 => TextureFormat::Bc7,
        }
    }
}

/// What is wrong with a texture file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureError {
    /// The bytes are not a file of this kind, or are damaged or cut short.
    Invalid(&'static str),
    /// The file is sound but holds something this crate does not read: another pixel format, a
    /// cube map, an array, a volume, compressed levels.
    Unsupported(&'static str),
}

impl std::fmt::Display for TextureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextureError::Invalid(what) => write!(f, "not a valid texture file: {what}"),
            TextureError::Unsupported(what) => write!(f, "a texture file of a kind not read: {what}"),
        }
    }
}

impl std::error::Error for TextureError {}

/// A packed texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub format: TextureFormat,
    /// Whether the colours are sRGB. Only a note for whoever reads the file: the blocks are the
    /// same either way.
    pub srgb: bool,
    /// The width of the first level in pixels.
    pub width: u32,
    /// The height of the first level in pixels.
    pub height: u32,
    /// The blocks of each mip level, largest first, each in rows from the top. Level `i` is
    /// `max(1, width >> i)` by `max(1, height >> i)` pixels, in whole blocks.
    pub levels: Vec<Vec<u8>>,
}

/// The most levels a size can have: down to one pixel.
pub fn max_levels(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

impl Texture {
    /// The size in pixels of a level.
    pub fn level_size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// The bytes a level of this texture has to hold.
    pub fn level_bytes(&self, level: usize) -> usize {
        level_bytes(self.format, self.width, self.height, level)
    }

    /// Whether there is at least one level, no more than the size allows, and each holds the
    /// bytes it should.
    pub fn is_valid(&self) -> bool {
        self.width != 0 && self.height != 0 && !self.levels.is_empty() && self.levels.len() <= max_levels(self.width, self.height) as usize && self.levels.iter().enumerate().all(|(i, level)| level.len() == self.level_bytes(i))
    }

    /// Reads a texture from the bytes of a `.dds`, `.ktx` or `.ktx2` file, told apart by how
    /// they begin.
    pub fn from_file_bytes(file: &[u8]) -> Result<Texture, TextureError> {
        if file.starts_with(b"DDS ") {
            dds::read(file)
        } else if file.starts_with(b"\xABKTX 11") {
            ktx::read_ktx1(file)
        } else if file.starts_with(b"\xABKTX 20") {
            ktx::read_ktx2(file)
        } else {
            Err(TextureError::Invalid("neither a .dds, a .ktx nor a .ktx2 file"))
        }
    }

    /// Reads a `.dds`, `.ktx` or `.ktx2` file from disk. A file that is not a texture this
    /// crate reads comes back as an error of the kind `InvalidData`, with the
    /// [`TextureError`] inside.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Texture> {
        Texture::from_file_bytes(&std::fs::read(path)?).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Writes the texture to disk as the file its name ends in says: `.dds` (with the old
    /// header where the format allows), `.ktx` or `.ktx2`. Any other ending, and a texture
    /// that is not whole, is an error of the kind `InvalidInput`.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let extension = path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase);
        let file = match extension.as_deref() {
            Some("dds") => dds::write(self, false),
            Some("ktx") => ktx::write_ktx1(self),
            Some("ktx2") => ktx::write_ktx2(self),
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the file name has to end in .dds, .ktx or .ktx2")),
        };
        let file = file.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "the texture's levels are not the sizes its format and size need"))?;
        std::fs::write(path, file)
    }

    /// Unpacks a level of a BC6H texture, of either kind, to an HDR image of its size. None for
    /// the other formats and for a level that is not there or not whole.
    pub fn unpack_level_hdr(&self, level: usize) -> Option<HdrImage> {
        let is_signed = match self.format {
            TextureFormat::Bc6hUnsigned => false,
            TextureFormat::Bc6hSigned => true,
            _ => return None,
        };
        let blocks = self.levels.get(level)?;
        if blocks.len() != self.level_bytes(level) {
            return None;
        }
        let (width, height) = self.level_size(level);
        let blocks_x = width.div_ceil(4);
        let mut image = HdrImage::new(width, height);
        for (i, block) in blocks.chunks_exact(16).enumerate() {
            let pixels = bc6h::unpack_bc6h_half(block.try_into().ok()?, is_signed);
            let (bx, by) = (i as u32 % blocks_x, i as u32 / blocks_x);
            for y in 0..4 {
                for x in 0..4 {
                    let (px, py) = (bx * 4 + x, by * 4 + y);
                    if px < width && py < height {
                        image.pixels[(px + py * width) as usize] = pixels[(y * 4 + x) as usize];
                    }
                }
            }
        }
        Some(image)
    }

    /// Unpacks a level to an image of its size. None for the HDR formats (see
    /// [`Texture::unpack_level_hdr`]), for a level that is not there or not whole, and for a BC7
    /// block of no mode. BC4 unpacks to red and BC5 to red and green, on black.
    pub fn unpack_level(&self, level: usize, bc1_mode: Bc1ApproxMode) -> Option<Image> {
        let blocks = self.levels.get(level)?;
        if blocks.len() != self.level_bytes(level) {
            return None;
        }
        let (width, height) = self.level_size(level);
        let (blocks_x, blocks_y) = (width.div_ceil(4), height.div_ceil(4));
        let mut whole = Image::new(blocks_x * 4, blocks_y * 4);
        for (i, block) in blocks.chunks_exact(self.format.bytes_per_block()).enumerate() {
            let mut pixels = [[0u8, 0, 0, 255]; 16];
            match self.format {
                TextureFormat::Bc1 => {
                    rgbcx::unpack_bc1(block.try_into().ok()?, &mut pixels, true, bc1_mode);
                }
                TextureFormat::Bc3 => {
                    rgbcx::unpack_bc3(block.try_into().ok()?, &mut pixels, bc1_mode);
                }
                TextureFormat::Bc4 => rgbcx::unpack_bc4(block.try_into().ok()?, pixels.as_flattened_mut(), 4),
                TextureFormat::Bc5 => rgbcx::unpack_bc5(block.try_into().ok()?, pixels.as_flattened_mut(), 0, 1, 4),
                TextureFormat::Bc7 => {
                    if !bc7decomp::unpack_bc7(block.try_into().ok()?, &mut pixels) {
                        return None;
                    }
                }
                TextureFormat::Bc6hUnsigned | TextureFormat::Bc6hSigned => return None,
            }
            whole.set_block(i as u32 % blocks_x, i as u32 / blocks_x, &pixels);
        }
        whole.crop_dup_borders(width, height);
        Some(whole)
    }
}

/// The bytes of a level of a texture of this format and size.
pub(crate) fn level_bytes(format: TextureFormat, width: u32, height: u32, level: usize) -> usize {
    let (w, h) = ((width >> level).max(1) as usize, (height >> level).max(1) as usize);
    w.div_ceil(4) * h.div_ceil(4) * format.bytes_per_block()
}

/// Packs an image into a texture with the image encoder ([`RdoBcEncoder`]): the format, the
/// quality, the rate-distortion pass and the threads are all in the settings. With `mipmaps`,
/// every level down to one pixel is made from the one before by averaging and packed the same
/// way. None for an image with no pixels.
pub fn compress(image: &Image, params: &RdoBcParams, mipmaps: bool) -> Option<Texture> {
    if image.width == 0 || image.height == 0 || image.pixels.len() != image.width as usize * image.height as usize {
        return None;
    }
    let mut encoder = RdoBcEncoder::new();
    let mut levels = Vec::new();
    let mut level_image;
    let mut current = image;
    loop {
        if !encoder.init(current, params) || !encoder.encode() {
            return None;
        }
        levels.push(encoder.blocks().to_vec());
        if !mipmaps || (current.width == 1 && current.height == 1) {
            break;
        }
        level_image = current.half();
        current = &level_image;
    }
    Some(Texture { format: params.format.into(), srgb: false, width: image.width, height: image.height, levels })
}

/// Packs an HDR image into an unsigned BC6H texture ([`bc6h::compress_block_bc6h`]), on
/// `threads` threads (0 for all the machine has; the blocks do not depend on it). The image is
/// padded to whole blocks by repeating its last column and row. With `mipmaps`, every level
/// down to one pixel is made from the one before by averaging and packed the same way.
///
/// A pixel value the format cannot hold is brought into range first: a negative half becomes
/// 0, and infinity and not-a-number the largest half. None for an image with no pixels.
pub fn compress_hdr(image: &HdrImage, settings: &Bc6hEncSettings, mipmaps: bool, threads: u32) -> Option<Texture> {
    if image.width == 0 || image.height == 0 || image.pixels.len() != image.width as usize * image.height as usize {
        return None;
    }
    let mut first = image.clone();
    for value in first.pixels.as_flattened_mut() {
        *value = if *value & 0x8000 != 0 { 0 } else { (*value).min(0x7bff) };
    }
    let mut levels = Vec::new();
    let mut current = first;
    loop {
        let blocks_x = current.width.div_ceil(4) as usize;
        let mut blocks = vec![0u8; blocks_x * current.height.div_ceil(4) as usize * 16];
        let rows: Vec<(u32, &mut [u8])> = blocks.chunks_exact_mut(blocks_x * 16).enumerate().map(|(by, row)| (by as u32, row)).collect();
        let level = &current;
        parallel::for_each(rows, parallel::worker_count(threads), |(by, row)| {
            for (bx, block) in row.chunks_exact_mut(16).enumerate() {
                bc6h::compress_block_bc6h(block.try_into().expect("16 bytes"), &level.get_block(bx as u32, by), settings);
            }
        });
        levels.push(blocks);
        if !mipmaps || (current.width == 1 && current.height == 1) {
            break;
        }
        current = current.half();
    }
    Some(Texture { format: TextureFormat::Bc6hUnsigned, srgb: false, width: image.width, height: image.height, levels })
}

// What the file modules share.

/// Reads little-endian numbers out of a file's bytes.
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl Reader<'_> {
    pub fn u32(&mut self) -> Result<u32, TextureError> {
        let bytes = self.bytes.get(self.at..self.at + 4).ok_or(TextureError::Invalid("the file is cut short"))?;
        self.at += 4;
        Ok(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, TextureError> {
        Ok(u64::from(self.u32()?) | (u64::from(self.u32()?) << 32))
    }
}

/// Checks a size and a number of levels read from a file, and gives the levels to read.
pub(crate) fn check_size(width: u32, height: u32, levels: u32) -> Result<u32, TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::Invalid("a size of nothing"));
    }
    // (GPUs stop at 16,384; this stops sizes that could not be held at all.)
    if width > 65536 || height > 65536 {
        return Err(TextureError::Unsupported("a side of more than 65,536 pixels"));
    }
    let levels = levels.max(1);
    if levels > max_levels(width, height) {
        return Err(TextureError::Invalid("more mip levels than the size has"));
    }
    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_hdr_image_packs_to_every_level_and_back() {
        // A smooth field, 22 by 9.
        let values: Vec<f32> = (0..22 * 9).flat_map(|i| [1.0 + (i % 22) as f32 * 0.02, 2.0 + (i / 22) as f32 * 0.03, 0.25]).collect();
        let image = HdrImage::from_rgb_f32(22, 9, &values).unwrap();
        let texture = compress_hdr(&image, &Bc6hEncSettings::basic(), true, 0).unwrap();
        assert!(texture.is_valid());
        assert_eq!((texture.format, texture.levels.len()), (TextureFormat::Bc6hUnsigned, 5));
        let unpacked = texture.unpack_level_hdr(0).unwrap();
        assert_eq!((unpacked.width, unpacked.height), (22, 9));
        for (a, b) in image.to_rgb_f32().iter().zip(unpacked.to_rgb_f32()) {
            assert!((a - b).abs() <= 0.01 + a * 0.03, "{a} {b}");
        }
        // The number of threads changes nothing.
        assert_eq!(compress_hdr(&image, &Bc6hEncSettings::basic(), true, 1).unwrap(), texture);
        assert_eq!(compress_hdr(&image, &Bc6hEncSettings::basic(), true, 3).unwrap(), texture);
        // What the format cannot hold is brought into range and does not upset the rest.
        let mut odd = image.clone();
        odd.pixels[0] = [0x8001, 0xfc00, 0x7e00];
        assert!(compress_hdr(&odd, &Bc6hEncSettings::veryfast(), false, 0).unwrap().is_valid());
        // The LDR unpacker says no to it, and the HDR one to an LDR texture.
        assert!(texture.unpack_level(0, Bc1ApproxMode::Ideal).is_none());
        assert!(compress_hdr(&HdrImage::new(0, 0), &Bc6hEncSettings::fast(), true, 0).is_none());
    }

    #[test]
    fn levels_halve_down_to_one_pixel() {
        assert_eq!(max_levels(1, 1), 1);
        assert_eq!(max_levels(70, 50), 7);
        assert_eq!(max_levels(256, 4), 9);
        assert_eq!(level_bytes(TextureFormat::Bc1, 70, 50, 0), 18 * 13 * 8);
        assert_eq!(level_bytes(TextureFormat::Bc7, 70, 50, 3), 2 * 2 * 16);
        assert_eq!(level_bytes(TextureFormat::Bc7, 70, 50, 6), 16);
    }

    #[test]
    fn an_image_packs_to_every_level_and_back() {
        let mut image = Image::new(37, 21);
        for (i, pixel) in image.pixels.iter_mut().enumerate() {
            *pixel = [(i % 37 * 3) as u8, (i / 37 * 5) as u8, 90, 255];
        }
        let mut params = RdoBcParams::new();
        params.bc7_uber_level = 0;
        let texture = compress(&image, &params, true).unwrap();
        assert!(texture.is_valid());
        assert_eq!(texture.levels.len(), 6);
        assert_eq!(texture.level_size(5), (1, 1));
        let unpacked = texture.unpack_level(0, Bc1ApproxMode::Ideal).unwrap();
        assert_eq!((unpacked.width, unpacked.height), (37, 21));
        for (a, b) in image.pixels.iter().zip(&unpacked.pixels) {
            for c in 0..3 {
                assert!((i32::from(a[c]) - i32::from(b[c])).abs() <= 8, "{a:?} {b:?}");
            }
        }
        // The last level is the average of the image.
        let last = texture.unpack_level(5, Bc1ApproxMode::Ideal).unwrap();
        assert!((i32::from(last.pixels[0][2]) - 90).abs() <= 2);
        // One level without mipmaps; nothing from no image.
        assert_eq!(compress(&image, &params, false).unwrap().levels.len(), 1);
        assert!(compress(&Image::new(0, 5), &params, true).is_none());
        assert!(texture.unpack_level(6, Bc1ApproxMode::Ideal).is_none());

        // To disk and back, as each kind of file.
        let folder = std::env::temp_dir().join(format!("rustbcx-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["a.dds", "a.ktx", "a.KTX2"] {
            let path = folder.join(name);
            texture.save(&path).unwrap();
            assert_eq!(Texture::load(&path).unwrap(), texture, "{name}");
        }
        assert_eq!(texture.save(folder.join("a.png")).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        std::fs::write(folder.join("b.dds"), b"not a texture").unwrap();
        assert_eq!(Texture::load(folder.join("b.dds")).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(Texture::load(folder.join("none.dds")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
