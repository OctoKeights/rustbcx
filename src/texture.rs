//! A packed texture: a format, a size, and the blocks of each mip level. What the file modules
//! ([`crate::dds`], [`crate::ktx`]) read and write, and the short way from an image to one.
//!
//! A texture is one image with its mip levels, or several that go together: the layers of a
//! texture array, the six faces of a cube map, the slices of a volume texture.
//!
//! (The crate's own: `bc7enc_rdo` stops at the blocks of one image.)

use crate::bc2;
use crate::bc6h::{self, Bc6hEncSettings};
use crate::bc6h_enc;
use crate::bc7decomp;
use crate::image::{HdrImage, Image};
use crate::parallel;
use crate::rdo_bc_encoder::{BlockFormat, RdoBcEncoder, RdoBcParams};
use crate::rgbcx::{self, Bc1ApproxMode};
use crate::{dds, ktx};
use std::path::Path;

/// The block formats a texture can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// RGB, 8 bytes a block.
    Bc1,
    /// RGB with alpha of four bits a pixel, 16 bytes a block.
    Bc2,
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
    #[must_use]
    pub fn bytes_per_block(self) -> usize {
        match self {
            TextureFormat::Bc1 | TextureFormat::Bc4 => 8,
            _ => 16,
        }
    }

    /// Whether a file can mark the format as sRGB: BC1, BC2, BC3 and BC7.
    #[must_use]
    pub fn can_be_srgb(self) -> bool {
        matches!(
            self,
            TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 | TextureFormat::Bc7
        )
    }
}

impl From<BlockFormat> for TextureFormat {
    fn from(format: BlockFormat) -> TextureFormat {
        match format {
            BlockFormat::Bc1 => TextureFormat::Bc1,
            BlockFormat::Bc2 => TextureFormat::Bc2,
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
    /// cube map with faces missing, compressed levels.
    Unsupported(&'static str),
}

impl std::fmt::Display for TextureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextureError::Invalid(what) => write!(f, "not a valid texture file: {what}"),
            TextureError::Unsupported(what) => {
                write!(f, "a texture file of a kind not read: {what}")
            }
        }
    }
}

impl std::error::Error for TextureError {}

/// A packed texture.
///
/// Most textures are one image with its mip levels: [`Texture::new`] makes one of those from
/// its levels. `depth`, `layers` and `cube` are for the rest, and hold what the headers of
/// `.ktx` files hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    /// The format of the blocks.
    pub format: TextureFormat,
    /// Whether the colours are sRGB. Only a note for whoever reads the file: the blocks are the
    /// same either way.
    pub srgb: bool,
    /// The width of the first level in pixels.
    pub width: u32,
    /// The height of the first level in pixels.
    pub height: u32,
    /// The depth of the first level of a volume texture, in pixels; 0 for a texture that is not
    /// a volume. Each level has half the slices of the one before (never fewer than one), and
    /// each slice is packed as an image by itself. A volume texture has no layers and is not a
    /// cube map.
    pub depth: u32,
    /// The number of layers of a texture array; 0 for a texture that is not an array. (An array
    /// of one layer is an array still, and `.ktx` and `.ktx2` files keep it so; a `.dds` file
    /// cannot tell it from a texture that is not an array.)
    pub layers: u32,
    /// Whether the texture is a cube map: six square faces, in the order +X, -X, +Y, -Y, +Z,
    /// -Z. With `layers`, an array of cube maps, six faces to each layer.
    pub cube: bool,
    /// The blocks of each mip level, largest first. Level `i` is `max(1, width >> i)` by
    /// `max(1, height >> i)` pixels, in whole blocks, in rows from the top.
    ///
    /// A level of a texture with several images holds them one after another: layer by layer,
    /// in each layer face by face, or, for a volume texture, slice by slice from the front
    /// ([`Texture::image`] gives one of them).
    pub levels: Vec<Vec<u8>>,
}

/// The most levels a size can have: down to one pixel.
#[must_use]
pub fn max_levels(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

impl Texture {
    /// A texture of one image with its mip levels, largest first: not sRGB, not a volume, not
    /// an array, not a cube map.
    #[must_use]
    pub fn new(format: TextureFormat, width: u32, height: u32, levels: Vec<Vec<u8>>) -> Texture {
        Texture {
            format,
            srgb: false,
            width,
            height,
            depth: 0,
            layers: 0,
            cube: false,
            levels,
        }
    }

    /// The size in pixels of a level.
    #[must_use]
    pub fn level_size(&self, level: usize) -> (u32, u32) {
        (shifted(self.width, level), shifted(self.height, level))
    }

    /// The slices a level has: the depth of a volume texture halved for each level, and 1 for
    /// any other texture.
    #[must_use]
    pub fn level_depth(&self, level: usize) -> u32 {
        shifted(self.depth, level)
    }

    /// The images a level holds: a layer's faces for each layer, or a volume's slices.
    #[must_use]
    pub fn images_per_level(&self, level: usize) -> usize {
        self.layers.max(1) as usize
            * if self.cube { 6 } else { 1 }
            * self.level_depth(level) as usize
    }

    /// The bytes one image of a level takes.
    #[must_use]
    pub fn image_bytes(&self, level: usize) -> usize {
        level_bytes(self.format, self.width, self.height, level)
    }

    /// The bytes a level of this texture has to hold: all its images.
    #[must_use]
    pub fn level_bytes(&self, level: usize) -> usize {
        self.image_bytes(level) * self.images_per_level(level)
    }

    /// The most levels the texture can have: down to one pixel each way.
    #[must_use]
    pub fn max_levels(&self) -> u32 {
        max_levels(self.width.max(self.depth), self.height)
    }

    /// The blocks of one image of a level. `index` counts the level's images in the order they
    /// are held: for an array, `layer` (and for an array of cube maps `layer * 6 + face`); for
    /// a cube map, the face; for a volume texture, the slice. None when it is not there.
    #[must_use]
    pub fn image(&self, level: usize, index: usize) -> Option<&[u8]> {
        if index >= self.images_per_level(level) {
            return None;
        }
        let size = self.image_bytes(level);
        self.levels
            .get(level)?
            .get(index * size..(index + 1) * size)
    }

    /// Whether the texture is whole: a size, at least one level and no more than the size
    /// allows, each holding the bytes it should; a cube map square; a volume texture neither
    /// an array nor a cube map.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.width != 0
            && self.height != 0
            && !self.levels.is_empty()
            && self.levels.len() <= self.max_levels() as usize
            && !(self.cube && self.width != self.height)
            && !(self.depth != 0 && (self.cube || self.layers != 0))
            && self
                .levels
                .iter()
                .enumerate()
                .all(|(i, level)| level.len() == self.level_bytes(i))
    }

    /// Reads a texture from the bytes of a `.dds`, `.ktx` or `.ktx2` file, told apart by how
    /// they begin.
    ///
    /// # Errors
    ///
    /// [`TextureError::Invalid`] for a file that is none of them, and what [`dds::read`],
    /// [`ktx::read_ktx1`] and [`ktx::read_ktx2`] refuse.
    pub fn from_file_bytes(file: &[u8]) -> Result<Texture, TextureError> {
        if file.starts_with(b"DDS ") {
            dds::read(file)
        } else if file.starts_with(b"\xABKTX 11") {
            ktx::read_ktx1(file)
        } else if file.starts_with(b"\xABKTX 20") {
            ktx::read_ktx2(file)
        } else {
            Err(TextureError::Invalid(
                "neither a .dds, a .ktx nor a .ktx2 file",
            ))
        }
    }

    /// Reads a `.dds`, `.ktx` or `.ktx2` file from disk.
    ///
    /// # Errors
    ///
    /// The error reading the file; or, for a file that is not a texture this crate reads, an
    /// error of the kind `InvalidData` with the [`TextureError`] inside.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Texture> {
        Texture::from_file_bytes(&std::fs::read(path)?)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Writes the texture to disk as the file its name ends in says: `.dds` (with the old
    /// header where the format allows), `.ktx` or `.ktx2`.
    ///
    /// # Errors
    ///
    /// Any other ending, and a texture that is not whole, as an error of the kind
    /// `InvalidInput`; and the error writing the file.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        let file = match extension.as_deref() {
            Some("dds") => dds::write(self, false),
            Some("ktx") => ktx::write_ktx1(self),
            Some("ktx2") => ktx::write_ktx2(self),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the file name has to end in .dds, .ktx or .ktx2",
                ));
            }
        };
        let file = file.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the texture's levels are not the sizes its format and size need",
            )
        })?;
        std::fs::write(path, file)
    }

    /// Writes the texture to disk as a `.ktx2` file with its levels supercompressed as asked,
    /// whatever the name ends in.
    ///
    /// # Errors
    ///
    /// A texture that is not whole, and a Zstandard level the compressor does not take, as an
    /// error of the kind `InvalidInput`; and the error writing the file.
    pub fn save_ktx2(
        &self,
        path: impl AsRef<Path>,
        supercompression: ktx::Supercompression,
    ) -> std::io::Result<()> {
        let file = ktx::write_ktx2_with(self, supercompression).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                if self.is_valid() {
                    "the Zstandard level is not one the compressor takes"
                } else {
                    "the texture's levels are not the sizes its format and size need"
                },
            )
        })?;
        std::fs::write(path, file)
    }

    /// Unpacks a level of a BC6H texture, of either kind, to an HDR image of its size: the
    /// first image of the level, for a texture with several ([`Texture::unpack_image_hdr`]
    /// for the others). None for the other formats and for a level that is not there or not
    /// whole.
    #[must_use]
    pub fn unpack_level_hdr(&self, level: usize) -> Option<HdrImage> {
        self.unpack_image_hdr(level, 0)
    }

    /// Unpacks one image of a level of a BC6H texture ([`Texture::image`] says how they are
    /// counted) to an HDR image of its size. None for the other formats and for an image that
    /// is not there.
    #[must_use]
    pub fn unpack_image_hdr(&self, level: usize, index: usize) -> Option<HdrImage> {
        let is_signed = match self.format {
            TextureFormat::Bc6hUnsigned => false,
            TextureFormat::Bc6hSigned => true,
            _ => return None,
        };
        let blocks = self.image(level, index)?;
        let (width, height) = self.level_size(level);
        let blocks_x = width.div_ceil(4);
        let mut image = HdrImage::new(width, height);
        for (i, block) in blocks.as_chunks::<16>().0.iter().enumerate() {
            let pixels = bc6h::unpack_bc6h_half(block, is_signed);
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

    /// Unpacks a level to an image of its size: the first image of the level, for a texture
    /// with several ([`Texture::unpack_image`] for the others). None for the HDR formats (see
    /// [`Texture::unpack_level_hdr`]), for a level that is not there or not whole, and for a BC7
    /// block of no mode. BC4 unpacks to red and BC5 to red and green, on black.
    #[must_use]
    pub fn unpack_level(&self, level: usize, bc1_mode: Bc1ApproxMode) -> Option<Image> {
        self.unpack_image(level, 0, bc1_mode)
    }

    /// Unpacks one image of a level ([`Texture::image`] says how they are counted) to an image
    /// of its size. None for the HDR formats, for an image that is not there, and for a BC7
    /// block of no mode.
    #[must_use]
    pub fn unpack_image(
        &self,
        level: usize,
        index: usize,
        bc1_mode: Bc1ApproxMode,
    ) -> Option<Image> {
        let blocks = self.image(level, index)?;
        let (width, height) = self.level_size(level);
        let (blocks_x, blocks_y) = (width.div_ceil(4), height.div_ceil(4));
        let mut whole = Image::new(blocks_x * 4, blocks_y * 4);
        for (i, block) in blocks
            .chunks_exact(self.format.bytes_per_block())
            .enumerate()
        {
            let mut pixels = [[0u8, 0, 0, 255]; 16];
            match self.format {
                TextureFormat::Bc1 => {
                    rgbcx::unpack_bc1(block.try_into().ok()?, &mut pixels, true, bc1_mode);
                }
                TextureFormat::Bc2 => {
                    bc2::unpack_bc2(block.try_into().ok()?, &mut pixels, bc1_mode);
                }
                TextureFormat::Bc3 => {
                    rgbcx::unpack_bc3(block.try_into().ok()?, &mut pixels, bc1_mode);
                }
                TextureFormat::Bc4 => {
                    rgbcx::unpack_bc4(block.try_into().ok()?, pixels.as_flattened_mut(), 4);
                }
                TextureFormat::Bc5 => {
                    rgbcx::unpack_bc5(block.try_into().ok()?, pixels.as_flattened_mut(), 0, 1, 4);
                }
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

/// A side of a level: halved for each level, never to nothing. (No side has more than 32
/// levels.)
fn shifted(side: u32, level: usize) -> u32 {
    side.checked_shr(level as u32).unwrap_or(0).max(1)
}

/// The bytes of one image of a level of a texture of this format and size.
pub(crate) fn level_bytes(format: TextureFormat, width: u32, height: u32, level: usize) -> usize {
    let (w, h) = (
        shifted(width, level) as usize,
        shifted(height, level) as usize,
    );
    w.div_ceil(4) * h.div_ceil(4) * format.bytes_per_block()
}

/// How several images make one texture ([`compress_many`], [`compress_hdr_many`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    /// A texture array: each image is a layer.
    Array,
    /// A cube map: six square images, its faces in the order +X, -X, +Y, -Y, +Z, -Z. Twelve
    /// images or more (six for each) make an array of cube maps.
    Cube,
    /// A volume texture: each image is a slice, from the front. Its mip levels halve the number
    /// of slices as well as their size.
    Volume,
}

/// What a set of images of one size says about the texture they make: its depth, its layers and
/// whether it is a cube map. None when they cannot make one.
fn shape_of(layout: Layout, count: usize, width: u32, height: u32) -> Option<(u32, u32, bool)> {
    let count = u32::try_from(count).ok().filter(|&count| count != 0)?;
    match layout {
        Layout::Array => Some((0, count, false)),
        Layout::Cube if count % 6 == 0 && width == height => {
            Some((0, if count == 6 { 0 } else { count / 6 }, true))
        }
        Layout::Cube => None,
        Layout::Volume => Some((count, 0, false)),
    }
}

/// Packs an image into a texture with the image encoder ([`RdoBcEncoder`]): the format, the
/// quality, the rate-distortion pass and the threads are all in the settings. With `mipmaps`,
/// every level down to one pixel is made from the one before by averaging and packed the same
/// way. None for an image with no pixels.
#[must_use]
pub fn compress(image: &Image, params: &RdoBcParams, mipmaps: bool) -> Option<Texture> {
    if image.width == 0
        || image.height == 0
        || image.pixels.len() != image.width as usize * image.height as usize
    {
        return None;
    }
    let mut encoder = RdoBcEncoder::new();
    let mut levels = Vec::new();
    let mut level_image;
    let mut current = image;
    loop {
        encoder.init(current, params);
        encoder.encode().ok()?;
        levels.push(encoder.blocks().to_vec());
        if !mipmaps || (current.width == 1 && current.height == 1) {
            break;
        }
        level_image = current.half();
        current = &level_image;
    }
    Some(Texture::new(
        params.format.into(),
        image.width,
        image.height,
        levels,
    ))
}

/// Packs several images of one size into one texture: the layers of an array, the faces of a
/// cube map or the slices of a volume texture, as `layout` says. Each image is packed as
/// [`compress`] packs one, and with `mipmaps` every level down to one pixel is made the same
/// way (for a volume texture, from two slices of the level before at a time).
///
/// None for no images, images that are not all one size, and for a cube map faces that are not
/// square or not six to a cube.
pub fn compress_many(
    images: &[Image],
    layout: Layout,
    params: &RdoBcParams,
    mipmaps: bool,
) -> Option<Texture> {
    let first = images.first()?;
    let (width, height) = (first.width, first.height);
    if width == 0
        || height == 0
        || images.iter().any(|image| {
            image.width != width
                || image.height != height
                || image.pixels.len() != width as usize * height as usize
        })
    {
        return None;
    }
    let (depth, layers, cube) = shape_of(layout, images.len(), width, height)?;
    let mut encoder = RdoBcEncoder::new();
    let mut levels = Vec::new();
    let mut current = images.to_vec();
    loop {
        let mut level = Vec::new();
        for image in &current {
            encoder.init(image, params);
            encoder.encode().ok()?;
            level.extend_from_slice(encoder.blocks());
        }
        levels.push(level);
        if !mipmaps
            || (current[0].width == 1
                && current[0].height == 1
                && (layout != Layout::Volume || current.len() == 1))
        {
            break;
        }
        current = if layout == Layout::Volume {
            Image::half_volume(&current).ok()?
        } else {
            current.iter().map(Image::half).collect()
        };
    }
    Some(Texture {
        format: params.format.into(),
        srgb: false,
        width,
        height,
        depth,
        layers,
        cube,
        levels,
    })
}

/// Which packer of BC6H, and so which kind of block.
#[derive(Clone, Copy)]
enum HdrPacker<'a> {
    /// Unsigned, by the port of Intel's encoder.
    Unsigned(&'a Bc6hEncSettings),
    /// Signed, by the port of `bc6h_enc`.
    Signed,
}

/// Packs an HDR image to BC6H blocks, in rows.
fn pack_hdr_image(image: &HdrImage, packer: HdrPacker, threads: u32) -> Vec<u8> {
    let blocks_x = image.width.div_ceil(4) as usize;
    let mut blocks = vec![0u8; blocks_x * image.height.div_ceil(4) as usize * 16];
    let rows: Vec<(u32, &mut [u8])> = blocks
        .chunks_exact_mut(blocks_x * 16)
        .enumerate()
        .map(|(by, row)| (by as u32, row))
        .collect();
    parallel::for_each(rows, parallel::worker_count(threads), |(by, row)| {
        for (bx, block) in row.as_chunks_mut::<16>().0.iter_mut().enumerate() {
            let pixels = image.get_block(bx as u32, by);
            match packer {
                HdrPacker::Unsigned(settings) => {
                    bc6h::compress_block_bc6h(block, &pixels, settings);
                }
                HdrPacker::Signed => {
                    let floats = pixels.map(|pixel| {
                        [
                            bc6h::half_to_float(pixel[0]),
                            bc6h::half_to_float(pixel[1]),
                            bc6h::half_to_float(pixel[2]),
                            1.0,
                        ]
                    });
                    bc6h_enc::encode_bc6hs(block, &floats);
                }
            }
        }
    });
    blocks
}

/// An HDR image with what the kind of BC6H cannot hold brought into range: infinity and
/// not-a-number become the largest half (of their sign), and for the unsigned kind a negative
/// half becomes 0.
fn in_range(image: &HdrImage, packer: HdrPacker) -> HdrImage {
    let mut image = image.clone();
    for value in image.pixels.as_flattened_mut() {
        *value = match packer {
            HdrPacker::Unsigned(_) if *value & 0x8000 != 0 => 0,
            _ => (*value & 0x8000) | (*value & 0x7fff).min(0x7bff),
        };
    }
    image
}

/// Packs HDR images of one size into a BC6H texture: one image by itself (no `layout`), or
/// several as an array, a cube map or a volume.
fn compress_hdr_with(
    images: &[HdrImage],
    layout: Option<Layout>,
    packer: HdrPacker,
    mipmaps: bool,
    threads: u32,
) -> Option<Texture> {
    let first = images.first()?;
    let (width, height) = (first.width, first.height);
    if width == 0
        || height == 0
        || images.iter().any(|image| {
            image.width != width
                || image.height != height
                || image.pixels.len() != width as usize * height as usize
        })
    {
        return None;
    }
    let (depth, layers, cube) = match layout {
        Some(layout) => shape_of(layout, images.len(), width, height)?,
        None => (0, 0, false),
    };
    let volume = layout == Some(Layout::Volume);
    let mut levels = Vec::new();
    let mut current: Vec<HdrImage> = images.iter().map(|image| in_range(image, packer)).collect();
    loop {
        levels.push(
            current
                .iter()
                .flat_map(|image| pack_hdr_image(image, packer, threads))
                .collect(),
        );
        if !mipmaps
            || (current[0].width == 1 && current[0].height == 1 && (!volume || current.len() == 1))
        {
            break;
        }
        current = if volume {
            HdrImage::half_volume(&current).ok()?
        } else {
            current.iter().map(HdrImage::half).collect()
        };
    }
    let format = match packer {
        HdrPacker::Unsigned(_) => TextureFormat::Bc6hUnsigned,
        HdrPacker::Signed => TextureFormat::Bc6hSigned,
    };
    Some(Texture {
        format,
        srgb: false,
        width,
        height,
        depth,
        layers,
        cube,
        levels,
    })
}

/// Packs an HDR image into an unsigned BC6H texture ([`bc6h::compress_block_bc6h`]), on
/// `threads` threads (0 for all the machine has; the blocks do not depend on it). The image is
/// padded to whole blocks by repeating its last column and row. With `mipmaps`, every level
/// down to one pixel is made from the one before by averaging and packed the same way.
///
/// A pixel value the format cannot hold is brought into range first: a negative half becomes
/// 0, and infinity and not-a-number the largest half. None for an image with no pixels.
#[must_use]
pub fn compress_hdr(
    image: &HdrImage,
    settings: &Bc6hEncSettings,
    mipmaps: bool,
    threads: u32,
) -> Option<Texture> {
    compress_hdr_with(
        std::slice::from_ref(image),
        None,
        HdrPacker::Unsigned(settings),
        mipmaps,
        threads,
    )
}

/// Packs several HDR images of one size into one unsigned BC6H texture: the layers of an array,
/// the faces of a cube map or the slices of a volume texture, as `layout` says. Each image is
/// packed as [`compress_hdr`] packs one. None as for [`compress_many`].
#[must_use]
pub fn compress_hdr_many(
    images: &[HdrImage],
    layout: Layout,
    settings: &Bc6hEncSettings,
    mipmaps: bool,
    threads: u32,
) -> Option<Texture> {
    compress_hdr_with(
        images,
        Some(layout),
        HdrPacker::Unsigned(settings),
        mipmaps,
        threads,
    )
}

/// Packs an HDR image into a signed BC6H texture, which holds values below zero as well
/// ([`bc6h_enc::encode_bc6hs`]): for the rest as [`compress_hdr`], but with the thorough
/// encoder, which has no settings and takes far longer.
///
/// Infinity and not-a-number are packed as the largest half of their sign. None for an image
/// with no pixels.
#[must_use]
pub fn compress_hdr_signed(image: &HdrImage, mipmaps: bool, threads: u32) -> Option<Texture> {
    compress_hdr_with(
        std::slice::from_ref(image),
        None,
        HdrPacker::Signed,
        mipmaps,
        threads,
    )
}

/// Packs several HDR images of one size into one signed BC6H texture, as `layout` says. Each
/// image is packed as [`compress_hdr_signed`] packs one. None as for [`compress_many`].
#[must_use]
pub fn compress_hdr_many_signed(
    images: &[HdrImage],
    layout: Layout,
    mipmaps: bool,
    threads: u32,
) -> Option<Texture> {
    compress_hdr_with(images, Some(layout), HdrPacker::Signed, mipmaps, threads)
}

// What the file modules share.

/// Reads little-endian numbers out of a file's bytes.
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl Reader<'_> {
    pub fn u32(&mut self) -> Result<u32, TextureError> {
        let bytes = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or(TextureError::Invalid("the file is cut short"))?;
        self.at += 4;
        Ok(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, TextureError> {
        Ok(u64::from(self.u32()?) | (u64::from(self.u32()?) << 32))
    }
}

/// Checks what a file's header says of its texture, and makes the texture with room for its
/// levels and nothing in them yet. `levels` is how many the file says it has (none counts as
/// one), and `depth`, `layers` and `cube` are as [`Texture`] holds them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn texture_from_header(
    format: TextureFormat,
    srgb: bool,
    width: u32,
    height: u32,
    depth: u32,
    layers: u32,
    cube: bool,
    levels: u32,
) -> Result<Texture, TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::Invalid("a size of nothing"));
    }
    // (GPUs stop at 16,384 pixels and 2,048 layers; this stops sizes that could not be held
    // at all.)
    if width > 65536 || height > 65536 || depth > 65536 {
        return Err(TextureError::Unsupported(
            "a side of more than 65,536 pixels",
        ));
    }
    if layers > 65536 {
        return Err(TextureError::Unsupported("more than 65,536 layers"));
    }
    if cube && width != height {
        return Err(TextureError::Invalid("a cube map that is not square"));
    }
    if depth != 0 && (cube || layers != 0) {
        return Err(TextureError::Unsupported(
            "an array or a cube map of volume textures",
        ));
    }
    let mut texture = Texture {
        format,
        srgb,
        width,
        height,
        depth,
        layers,
        cube,
        levels: Vec::new(),
    };
    let levels = levels.max(1);
    if levels > texture.max_levels() {
        return Err(TextureError::Invalid("more mip levels than the size has"));
    }
    texture.levels = vec![Vec::new(); levels as usize];
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_hdr_image_packs_to_every_level_and_back() {
        // A smooth field, 22 by 9.
        let values: Vec<f32> = (0..22 * 9)
            .flat_map(|i| {
                [
                    1.0 + (i % 22) as f32 * 0.02,
                    2.0 + (i / 22) as f32 * 0.03,
                    0.25,
                ]
            })
            .collect();
        let image = HdrImage::from_rgb_f32(22, 9, &values).unwrap();
        let texture = compress_hdr(&image, &Bc6hEncSettings::basic(), true, 0).unwrap();
        assert!(texture.is_valid());
        assert_eq!(
            (texture.format, texture.levels.len()),
            (TextureFormat::Bc6hUnsigned, 5)
        );
        let unpacked = texture.unpack_level_hdr(0).unwrap();
        assert_eq!((unpacked.width, unpacked.height), (22, 9));
        for (a, b) in image.to_rgb_f32().iter().zip(unpacked.to_rgb_f32()) {
            assert!((a - b).abs() <= 0.01 + a * 0.03, "{a} {b}");
        }
        // The number of threads changes nothing.
        assert_eq!(
            compress_hdr(&image, &Bc6hEncSettings::basic(), true, 1).unwrap(),
            texture
        );
        assert_eq!(
            compress_hdr(&image, &Bc6hEncSettings::basic(), true, 3).unwrap(),
            texture
        );
        // What the format cannot hold is brought into range and does not upset the rest.
        let mut odd = image.clone();
        odd.pixels[0] = [0x8001, 0xfc00, 0x7e00];
        assert!(
            compress_hdr(&odd, &Bc6hEncSettings::veryfast(), false, 0)
                .unwrap()
                .is_valid()
        );
        // The LDR unpacker says no to it, and the HDR one to an LDR texture.
        assert!(texture.unpack_level(0, Bc1ApproxMode::Ideal).is_none());
        assert!(compress_hdr(&HdrImage::new(0, 0), &Bc6hEncSettings::fast(), true, 0).is_none());
    }

    /// An image of one colour, lighter to the right.
    fn tinted(width: u32, height: u32, colour: [u8; 3]) -> Image {
        let mut image = Image::new(width, height);
        for (i, pixel) in image.pixels.iter_mut().enumerate() {
            let x = (i as u32 % width) as u8;
            *pixel = [colour[0] + x, colour[1] + x, colour[2] + x, 255];
        }
        image
    }

    fn near(a: [u8; 4], b: [u8; 4]) -> bool {
        (0..3).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= 8)
    }

    // (One test of every way of packing several images, each checked in turn.)
    #[allow(clippy::too_many_lines)]
    #[test]
    fn several_images_pack_to_an_array_a_cube_map_or_a_volume() {
        let colours: Vec<[u8; 3]> = (0..12u8)
            .map(|i| [i * 20, 200 - i * 15, 40 + i * 9])
            .collect();
        let images: Vec<Image> = colours.iter().map(|&colour| tinted(8, 8, colour)).collect();
        let mut params = RdoBcParams::new();
        params.format = BlockFormat::Bc1;

        // An array of three, each layer with its four levels.
        let array = compress_many(&images[..3], Layout::Array, &params, true).unwrap();
        assert!(array.is_valid());
        assert_eq!(
            (array.depth, array.layers, array.cube, array.levels.len()),
            (0, 3, false, 4)
        );
        assert_eq!(
            (array.images_per_level(0), array.images_per_level(3)),
            (3, 3)
        );
        for (i, image) in images[..3].iter().enumerate() {
            let back = array.unpack_image(0, i, Bc1ApproxMode::Ideal).unwrap();
            assert!(
                image
                    .pixels
                    .iter()
                    .zip(&back.pixels)
                    .all(|(&a, &b)| near(a, b)),
                "layer {i}"
            );
            // Each layer is what the image packs to by itself.
            let alone = compress(image, &params, true).unwrap();
            assert!((0..4).all(|level| array.image(level, i) == Some(&alone.levels[level][..])));
        }
        assert!(array.unpack_image(0, 3, Bc1ApproxMode::Ideal).is_none());
        assert_eq!(
            array.unpack_level(1, Bc1ApproxMode::Ideal),
            array.unpack_image(1, 0, Bc1ApproxMode::Ideal)
        );

        // A cube map of six faces, and an array of two.
        let cube = compress_many(&images[..6], Layout::Cube, &params, false).unwrap();
        assert!(cube.is_valid());
        assert_eq!(
            (cube.depth, cube.layers, cube.cube, cube.images_per_level(0)),
            (0, 0, true, 6)
        );
        assert!(near(
            cube.unpack_image(0, 4, Bc1ApproxMode::Ideal)
                .unwrap()
                .pixel(0, 0),
            images[4].pixel(0, 0)
        ));
        let cubes = compress_many(&images, Layout::Cube, &params, true).unwrap();
        assert!(cubes.is_valid());
        assert_eq!(
            (cubes.layers, cubes.cube, cubes.images_per_level(2)),
            (2, true, 12)
        );
        assert!(near(
            cubes
                .unpack_image(0, 11, Bc1ApproxMode::Ideal)
                .unwrap()
                .pixel(0, 0),
            images[11].pixel(0, 0)
        ));
        // Five faces, or faces that are not square, make no cube map.
        assert!(compress_many(&images[..5], Layout::Cube, &params, true).is_none());
        assert!(
            compress_many(
                &vec![tinted(8, 4, [1, 2, 3]); 6],
                Layout::Cube,
                &params,
                true
            )
            .is_none()
        );

        // A volume of five slices: 8x8x5, 4x4x2, 2x2x1, 1x1x1.
        let volume = compress_many(&images[..5], Layout::Volume, &params, true).unwrap();
        assert!(volume.is_valid());
        assert_eq!(
            (
                volume.depth,
                volume.layers,
                volume.cube,
                volume.levels.len()
            ),
            (5, 0, false, 4)
        );
        assert_eq!(
            [0, 1, 2, 3].map(|level| volume.level_depth(level)),
            [5, 2, 1, 1]
        );
        assert!(near(
            volume
                .unpack_image(0, 4, Bc1ApproxMode::Ideal)
                .unwrap()
                .pixel(3, 3),
            images[4].pixel(3, 3)
        ));
        // The second slice of the second level is the third and fourth slices together.
        let between = Image::half_volume(&images[..5]).unwrap()[1].pixel(1, 1);
        assert!(near(
            volume
                .unpack_image(1, 1, Bc1ApproxMode::Ideal)
                .unwrap()
                .pixel(1, 1),
            between
        ));
        // A deep volume of small slices goes on halving its slices: 2x2x9 has four levels.
        let deep = compress_many(
            &vec![tinted(2, 2, [9, 9, 9]); 9],
            Layout::Volume,
            &params,
            true,
        )
        .unwrap();
        assert!(deep.is_valid());
        assert_eq!(
            (deep.levels.len(), deep.level_depth(3), deep.max_levels()),
            (4, 1, 4)
        );

        // No images, and images of different sizes, make nothing.
        assert!(compress_many(&[], Layout::Array, &params, true).is_none());
        assert!(
            compress_many(
                &[tinted(8, 8, [0; 3]), tinted(4, 8, [0; 3])],
                Layout::Array,
                &params,
                true
            )
            .is_none()
        );

        // To disk and back, each as each kind of file.
        let folder =
            std::env::temp_dir().join(format!("rustbcx-test-shapes-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for (texture, what) in [
            (&array, "array"),
            (&cube, "cube"),
            (&cubes, "cubes"),
            (&volume, "volume"),
        ] {
            for name in ["a.dds", "a.ktx", "a.ktx2"] {
                let path = folder.join(name);
                texture.save(&path).unwrap();
                assert_eq!(&Texture::load(&path).unwrap(), texture, "{what} {name}");
            }
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn hdr_images_pack_to_a_cube_map_and_a_volume() {
        let faces: Vec<HdrImage> = (0..6)
            .map(|i| HdrImage::from_rgb_f32(4, 4, &[0.5 + i as f32; 48]).unwrap())
            .collect();
        let cube =
            compress_hdr_many(&faces, Layout::Cube, &Bc6hEncSettings::veryfast(), true, 0).unwrap();
        assert!(cube.is_valid());
        assert_eq!(
            (
                cube.format,
                cube.cube,
                cube.levels.len(),
                cube.images_per_level(1)
            ),
            (TextureFormat::Bc6hUnsigned, true, 3, 6)
        );
        for (i, face) in faces.iter().enumerate() {
            let back = cube.unpack_image_hdr(2, i).unwrap().to_rgb_f32()[0];
            assert!(
                (back - face.to_rgb_f32()[0]).abs() < 0.05,
                "face {i}: {back}"
            );
        }
        assert!(cube.unpack_image_hdr(0, 6).is_none());
        let volume = compress_hdr_many(
            &faces[..4],
            Layout::Volume,
            &Bc6hEncSettings::veryfast(),
            true,
            1,
        )
        .unwrap();
        assert!(volume.is_valid());
        assert_eq!(
            (volume.depth, volume.levels.len(), volume.level_depth(1)),
            (4, 3, 2)
        );
        // Slices 2 and 3 (2.5 and 3.5) make the second slice of the next level.
        assert!((volume.unpack_image_hdr(1, 1).unwrap().to_rgb_f32()[0] - 3.0).abs() < 0.05);
        assert!(
            compress_hdr_many(
                &faces[..5],
                Layout::Cube,
                &Bc6hEncSettings::veryfast(),
                true,
                0
            )
            .is_none()
        );
    }

    #[test]
    fn an_hdr_image_of_both_signs_packs_to_signed_blocks() {
        // A smooth field that is below zero in red, 13 by 6.
        let values: Vec<f32> = (0..13 * 6)
            .flat_map(|i| {
                [
                    -1.0 - (i % 13) as f32 * 0.02,
                    2.0 + (i / 13) as f32 * 0.03,
                    0.25,
                ]
            })
            .collect();
        let mut image = HdrImage::from_rgb_f32_signed(13, 6, &values).unwrap();
        let texture = compress_hdr_signed(&image, true, 0).unwrap();
        assert!(texture.is_valid());
        assert_eq!(
            (texture.format, texture.levels.len()),
            (TextureFormat::Bc6hSigned, 4)
        );
        let unpacked = texture.unpack_level_hdr(0).unwrap();
        for (a, b) in image.to_rgb_f32().iter().zip(unpacked.to_rgb_f32()) {
            assert!((a - b).abs() <= 0.01 + a.abs() * 0.03, "{a} {b}");
        }
        // The number of threads changes nothing; the unsigned packer has the red as nothing.
        assert_eq!(compress_hdr_signed(&image, true, 1).unwrap(), texture);
        let unsigned = compress_hdr(&image, &Bc6hEncSettings::veryfast(), false, 0).unwrap();
        assert_eq!(unsigned.unpack_level_hdr(0).unwrap().to_rgb_f32()[0], 0.0);
        // Infinity of either sign is the largest half of that sign.
        image.pixels.fill([0xfc00, 0x7c00, 0x7e00]);
        let texture = compress_hdr_signed(&image, false, 0).unwrap();
        let back = texture.unpack_level_hdr(0).unwrap().to_rgb_f32();
        assert!(
            back[0] < -60000.0 && back[1] > 60000.0 && back[2] > 60000.0,
            "{:?}",
            &back[..3]
        );
        // Several images, as a cube map.
        let faces: Vec<HdrImage> = (0..6)
            .map(|i| HdrImage::from_rgb_f32_signed(4, 4, &[i as f32 - 2.5; 48]).unwrap())
            .collect();
        let cube = compress_hdr_many_signed(&faces, Layout::Cube, true, 0).unwrap();
        assert!(cube.is_valid() && cube.cube && cube.format == TextureFormat::Bc6hSigned);
        for i in 0..6 {
            assert!(
                (cube.unpack_image_hdr(0, i).unwrap().to_rgb_f32()[0] - (i as f32 - 2.5)).abs()
                    < 0.05
            );
        }
    }

    #[test]
    fn a_texture_that_is_not_whole_says_so() {
        let whole = Texture::new(TextureFormat::Bc1, 8, 8, vec![vec![0; 32], vec![0; 8]]);
        assert!(whole.is_valid());
        assert_eq!(
            (
                whole.images_per_level(0),
                whole.level_depth(5),
                whole.max_levels()
            ),
            (1, 1, 4)
        );
        // A cube map has to be square and to hold six faces; a volume is neither an array nor
        // a cube map.
        let mut cube = Texture {
            cube: true,
            ..whole.clone()
        };
        assert!(!cube.is_valid());
        cube.levels = vec![vec![0; 192], vec![0; 48]];
        assert!(cube.is_valid());
        assert!(
            !Texture {
                height: 4,
                ..cube.clone()
            }
            .is_valid()
        );
        assert!(
            !Texture {
                depth: 1,
                ..cube.clone()
            }
            .is_valid()
        );
        let volume = Texture {
            depth: 3,
            levels: vec![vec![0; 96], vec![0; 8]],
            ..whole.clone()
        };
        assert!(volume.is_valid());
        assert!(
            !Texture {
                layers: 1,
                ..volume.clone()
            }
            .is_valid()
        );
        // A level far past the last is still one pixel, and holds one block.
        assert_eq!((whole.level_size(40), whole.image_bytes(40)), ((1, 1), 8));
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
                assert!(
                    (i32::from(a[c]) - i32::from(b[c])).abs() <= 8,
                    "{a:?} {b:?}"
                );
            }
        }
        // The last level is the average of the image.
        let last = texture.unpack_level(5, Bc1ApproxMode::Ideal).unwrap();
        assert!((i32::from(last.pixels[0][2]) - 90).abs() <= 2);
        // BC2 the same way, with its alpha on sixteen levels.
        let mut with_alpha = image.clone();
        for (i, pixel) in with_alpha.pixels.iter_mut().enumerate() {
            pixel[3] = (i % 16 * 17) as u8;
        }
        let mut bc2_params = RdoBcParams::new();
        bc2_params.format = BlockFormat::Bc2;
        let bc2_texture = compress(&with_alpha, &bc2_params, true).unwrap();
        assert!(bc2_texture.is_valid() && bc2_texture.format == TextureFormat::Bc2);
        let unpacked = bc2_texture.unpack_level(0, Bc1ApproxMode::Ideal).unwrap();
        assert!(
            with_alpha
                .pixels
                .iter()
                .zip(&unpacked.pixels)
                .all(|(a, b)| a[3] == b[3])
        );
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
        assert_eq!(
            texture.save(folder.join("a.png")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        std::fs::write(folder.join("b.dds"), b"not a texture").unwrap();
        assert_eq!(
            Texture::load(folder.join("b.dds")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            Texture::load(folder.join("none.dds")).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
