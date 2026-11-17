//! `.ktx` and `.ktx2` files, the Khronos Texture containers that OpenGL and Vulkan tools read
//! and write. (The crate's own: bc7enc_rdo writes `.dds` only.)
//!
//! A `.ktx2` file can hold its levels as they are or compressed again, which the format calls
//! supercompression: [`Supercompression`] says how, with zlib or with Zstandard, and
//! [`write_ktx2_with`] takes one. [`read_ktx2`] reads all three.
//! Its data format descriptor is the one the Khronos tools write for each format.

use crate::miniz;
use crate::texture::{Reader, Texture, TextureError, TextureFormat, texture_from_header};
use crate::zstd;

/// How the levels of a `.ktx2` file are compressed again, each by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Supercompression {
    /// Not at all: the blocks as they are, ready for the GPU.
    None,
    /// As zlib streams ([`crate::miniz`]), at one of miniz's levels: from
    /// [`miniz::Level::FASTEST`] to [`miniz::Level::BEST`], or [`miniz::Level::DEFAULT`].
    Zlib(miniz::Level),
    /// As Zstandard frames ([`crate::zstd`]), at one of zstd's levels: from 1, the quickest,
    /// to 22, the smallest, or a negative one, which is quicker still and compresses less.
    Zstd(zstd::Level),
}

impl Supercompression {
    /// The scheme's number in a file.
    fn scheme(self) -> u32 {
        match self {
            Supercompression::None => 0,
            Supercompression::Zstd(_) => 2,
            Supercompression::Zlib(_) => 3,
        }
    }
}

const KTX1_IDENTIFIER: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'1', b'1', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];
const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];
const KTX1_ENDIANNESS: u32 = 0x0403_0201;

const GL_RED: u32 = 0x1903;
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_RG: u32 = 0x8227;
const GL_COMPRESSED_RGB_S3TC_DXT1_EXT: u32 = 0x83F0;
const GL_COMPRESSED_RGBA_S3TC_DXT1_EXT: u32 = 0x83F1;
const GL_COMPRESSED_RGBA_S3TC_DXT3_EXT: u32 = 0x83F2;
const GL_COMPRESSED_RGBA_S3TC_DXT5_EXT: u32 = 0x83F3;
const GL_COMPRESSED_SRGB_S3TC_DXT1_EXT: u32 = 0x8C4C;
const GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT: u32 = 0x8C4D;
const GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT: u32 = 0x8C4E;
const GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT: u32 = 0x8C4F;
const GL_COMPRESSED_RED_RGTC1: u32 = 0x8DBB;
const GL_COMPRESSED_RG_RGTC2: u32 = 0x8DBD;
const GL_COMPRESSED_RGBA_BPTC_UNORM: u32 = 0x8E8C;
const GL_COMPRESSED_SRGB_ALPHA_BPTC_UNORM: u32 = 0x8E8D;
const GL_COMPRESSED_RGB_BPTC_SIGNED_FLOAT: u32 = 0x8E8E;
const GL_COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT: u32 = 0x8E8F;

/// OpenGL's internal format and base internal format for a texture.
fn gl_formats(format: TextureFormat, srgb: bool) -> (u32, u32) {
    match (format, srgb) {
        (TextureFormat::Bc1, false) => (GL_COMPRESSED_RGB_S3TC_DXT1_EXT, GL_RGB),
        (TextureFormat::Bc1, true) => (GL_COMPRESSED_SRGB_S3TC_DXT1_EXT, GL_RGB),
        (TextureFormat::Bc2, false) => (GL_COMPRESSED_RGBA_S3TC_DXT3_EXT, GL_RGBA),
        (TextureFormat::Bc2, true) => (GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT, GL_RGBA),
        (TextureFormat::Bc3, false) => (GL_COMPRESSED_RGBA_S3TC_DXT5_EXT, GL_RGBA),
        (TextureFormat::Bc3, true) => (GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT, GL_RGBA),
        (TextureFormat::Bc4, _) => (GL_COMPRESSED_RED_RGTC1, GL_RED),
        (TextureFormat::Bc5, _) => (GL_COMPRESSED_RG_RGTC2, GL_RG),
        (TextureFormat::Bc6hUnsigned, _) => (GL_COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT, GL_RGB),
        (TextureFormat::Bc6hSigned, _) => (GL_COMPRESSED_RGB_BPTC_SIGNED_FLOAT, GL_RGB),
        (TextureFormat::Bc7, false) => (GL_COMPRESSED_RGBA_BPTC_UNORM, GL_RGBA),
        (TextureFormat::Bc7, true) => (GL_COMPRESSED_SRGB_ALPHA_BPTC_UNORM, GL_RGBA),
    }
}

/// Vulkan's format number for a texture.
fn vk_format(format: TextureFormat, srgb: bool) -> u32 {
    match (format, srgb) {
        (TextureFormat::Bc1, false) => 131,
        (TextureFormat::Bc1, true) => 132,
        (TextureFormat::Bc2, false) => 135,
        (TextureFormat::Bc2, true) => 136,
        (TextureFormat::Bc3, false) => 137,
        (TextureFormat::Bc3, true) => 138,
        (TextureFormat::Bc4, _) => 139,
        (TextureFormat::Bc5, _) => 141,
        (TextureFormat::Bc6hUnsigned, _) => 143,
        (TextureFormat::Bc6hSigned, _) => 144,
        (TextureFormat::Bc7, false) => 145,
        (TextureFormat::Bc7, true) => 146,
    }
}

fn put_u32(file: &mut Vec<u8>, value: u32) {
    file.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(file: &mut Vec<u8>, value: u64) {
    file.extend_from_slice(&value.to_le_bytes());
}

/// The bytes of a `.ktx` file (KTX version 1) for a texture. None when the texture is not
/// whole ([`Texture::is_valid`]).
pub fn write_ktx1(texture: &Texture) -> Option<Vec<u8>> {
    if !texture.is_valid() {
        return None;
    }
    let (internal_format, base_internal_format) =
        gl_formats(texture.format, texture.srgb && texture.format.can_be_srgb());
    let mut file = Vec::with_capacity(
        64 + texture
            .levels
            .iter()
            .map(|level| level.len() + 4)
            .sum::<usize>(),
    );
    file.extend_from_slice(&KTX1_IDENTIFIER);
    // Endianness; no type, a type size of 1 and no format, as for all compressed textures; the
    // two internal formats; the size, with the depth of a volume; the layers of an array; the
    // faces; the levels; no key and value data.
    for value in [
        KTX1_ENDIANNESS,
        0,
        1,
        0,
        internal_format,
        base_internal_format,
        texture.width,
        texture.height,
        texture.depth,
        texture.layers,
        if texture.cube { 6 } else { 1 },
        texture.levels.len() as u32,
        0,
    ] {
        put_u32(&mut file, value);
    }
    for (i, level) in texture.levels.iter().enumerate() {
        // (For a cube map that is not an array, the size is that of one face.)
        put_u32(&mut file, ktx1_image_size(texture, i) as u32);
        file.extend_from_slice(level);
        // (Blocks are 8 or 16 bytes, so neither a face nor a level needs padding to four.)
    }
    Some(file)
}

/// The size a `.ktx` file gives before a level: of the whole level, except for a cube map that
/// is not an array, where it is of one face.
fn ktx1_image_size(texture: &Texture, level: usize) -> usize {
    if texture.cube && texture.layers == 0 {
        texture.image_bytes(level)
    } else {
        texture.level_bytes(level)
    }
}

/// Reads a `.ktx` file (KTX version 1): a 2D texture in BC1, BC2, BC3, BC4, BC5, BC6H or BC7,
/// with its mip levels; a cube map, a texture array, an array of cube maps or a volume texture
/// as well as a plain one. A file written on a big-endian machine and other formats are refused
/// as unsupported.
pub fn read_ktx1(file: &[u8]) -> Result<Texture, TextureError> {
    if file.len() < 12 || file[..12] != KTX1_IDENTIFIER {
        return Err(TextureError::Invalid("no KTX identifier"));
    }
    let mut reader = Reader {
        bytes: file,
        at: 12,
    };
    let mut header = [0u32; 13];
    for value in &mut header {
        *value = reader.u32()?;
    }
    let [
        endianness,
        gl_type,
        _type_size,
        gl_format,
        internal_format,
        _base,
        width,
        height,
        depth,
        array_elements,
        faces,
        level_count,
        key_value_bytes,
    ] = header;
    if endianness != KTX1_ENDIANNESS {
        return Err(if endianness == KTX1_ENDIANNESS.swap_bytes() {
            TextureError::Unsupported("a big-endian file")
        } else {
            TextureError::Invalid("the endianness mark is wrong")
        });
    }
    if gl_type != 0 || gl_format != 0 {
        return Err(TextureError::Unsupported("pixels that are not in blocks"));
    }
    let (format, srgb) = match internal_format {
        GL_COMPRESSED_RGB_S3TC_DXT1_EXT | GL_COMPRESSED_RGBA_S3TC_DXT1_EXT => {
            (TextureFormat::Bc1, false)
        }
        GL_COMPRESSED_SRGB_S3TC_DXT1_EXT | GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT => {
            (TextureFormat::Bc1, true)
        }
        GL_COMPRESSED_RGBA_S3TC_DXT3_EXT => (TextureFormat::Bc2, false),
        GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT => (TextureFormat::Bc2, true),
        GL_COMPRESSED_RGBA_S3TC_DXT5_EXT => (TextureFormat::Bc3, false),
        GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT => (TextureFormat::Bc3, true),
        GL_COMPRESSED_RED_RGTC1 => (TextureFormat::Bc4, false),
        GL_COMPRESSED_RG_RGTC2 => (TextureFormat::Bc5, false),
        GL_COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT => (TextureFormat::Bc6hUnsigned, false),
        GL_COMPRESSED_RGB_BPTC_SIGNED_FLOAT => (TextureFormat::Bc6hSigned, false),
        GL_COMPRESSED_RGBA_BPTC_UNORM => (TextureFormat::Bc7, false),
        GL_COMPRESSED_SRGB_ALPHA_BPTC_UNORM => (TextureFormat::Bc7, true),
        _ => {
            return Err(TextureError::Unsupported(
                "an OpenGL format other than BC1 to BC7",
            ));
        }
    };
    if faces != 1 && faces != 6 {
        return Err(TextureError::Invalid("faces that are neither one nor six"));
    }
    let mut texture = texture_from_header(
        format,
        srgb,
        width,
        height,
        depth,
        array_elements,
        faces == 6,
        level_count,
    )?;

    let mut at = reader
        .at
        .checked_add(key_value_bytes as usize)
        .ok_or(TextureError::Invalid("the file is cut short"))?;
    for level in 0..texture.levels.len() {
        reader.at = at;
        if reader.u32()? as usize != ktx1_image_size(&texture, level) {
            return Err(TextureError::Invalid(
                "a level is not the size its pixels need",
            ));
        }
        let size = texture.level_bytes(level);
        let blocks = reader
            .at
            .checked_add(size)
            .and_then(|end| file.get(reader.at..end))
            .ok_or(TextureError::Invalid("the file is cut short"))?;
        texture.levels[level] = blocks.to_vec();
        at = reader.at + size.next_multiple_of(4);
    }
    Ok(texture)
}

/// The data format descriptor of a format, as the Khronos tools write it (`createDFDCompressed`
/// of KTX-Software): its total size, then one basic descriptor block with a sample for each
/// part of a block.
fn data_format_descriptor(format: TextureFormat, srgb: bool) -> Vec<u8> {
    const FLOAT: u32 = 1 << 7;
    const SIGNED: u32 = 1 << 6;
    const LINEAR: u32 = 1 << 4;
    const ALPHA: u32 = 15;
    // The colour model, and for each sample its channel and where its bits start.
    let (model, samples): (u32, &[(u32, u32)]) = match format {
        TextureFormat::Bc1 => (128, &[(0, 0)]),
        TextureFormat::Bc2 => (129, &[(15, 0), (0, 64)]),
        TextureFormat::Bc3 => (130, &[(15, 0), (0, 64)]),
        TextureFormat::Bc4 => (131, &[(0, 0)]),
        TextureFormat::Bc5 => (132, &[(0, 0), (1, 64)]),
        TextureFormat::Bc6hUnsigned | TextureFormat::Bc6hSigned => (133, &[(0, 0)]),
        TextureFormat::Bc7 => (134, &[(0, 0)]),
    };
    let sample_bits: u32 = if samples.len() == 2 || format.bytes_per_block() == 8 {
        64
    } else {
        128
    };
    let (flags, lower, upper) = match format {
        TextureFormat::Bc6hUnsigned => (FLOAT, 0.0f32.to_bits(), 1.0f32.to_bits()),
        TextureFormat::Bc6hSigned => (FLOAT | SIGNED, (-1.0f32).to_bits(), 1.0f32.to_bits()),
        _ => (0, 0, u32::MAX),
    };
    let block_size = 24 + 16 * samples.len() as u32;
    let mut words = vec![
        4 + block_size,
        // The vendor (Khronos) and the kind of descriptor (basic).
        0,
        // The version of the descriptor (2, for the data format specification 1.3), and the
        // size of this block.
        2 | (block_size << 16),
        // The model; BT.709 primaries; linear or sRGB; straight alpha.
        model | (1 << 8) | (if srgb { 2 } else { 1 } << 16),
        // A block is four by four texels (each held as one less).
        3 | (3 << 8),
        // The bytes of a block, in the first of eight planes.
        format.bytes_per_block() as u32,
        0,
    ];
    for &(channel, bit_offset) in samples {
        // (The alpha of an sRGB texture is not sRGB, and its sample has to say so.)
        let linear = if srgb && channel == ALPHA { LINEAR } else { 0 };
        words.extend_from_slice(&[
            bit_offset | ((sample_bits - 1) << 16) | ((channel | flags | linear) << 24),
            0,
            lower,
            upper,
        ]);
    }
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The bytes of a `.ktx2` file (KTX version 2) for a texture, its levels not supercompressed.
/// None when the texture is not whole ([`Texture::is_valid`]).
pub fn write_ktx2(texture: &Texture) -> Option<Vec<u8>> {
    write_ktx2_with(texture, Supercompression::None)
}

/// The bytes of a `.ktx2` file (KTX version 2) for a texture, its levels supercompressed as
/// asked. None when the texture is not whole ([`Texture::is_valid`]).
pub fn write_ktx2_with(texture: &Texture, supercompression: Supercompression) -> Option<Vec<u8>> {
    if !texture.is_valid() {
        return None;
    }
    let srgb = texture.srgb && texture.format.can_be_srgb();
    let descriptor = data_format_descriptor(texture.format, srgb);
    let level_count = texture.levels.len();
    // The bytes of each level as the file holds them.
    let packed: Vec<std::borrow::Cow<[u8]>> = texture
        .levels
        .iter()
        .map(|level| match supercompression {
            Supercompression::None => Some(std::borrow::Cow::Borrowed(&level[..])),
            Supercompression::Zlib(zlib_level) => {
                Some(std::borrow::Cow::Owned(miniz::compress(level, zlib_level)))
            }
            Supercompression::Zstd(zstd_level) => zstd::compress(level, zstd_level)
                .ok()
                .map(std::borrow::Cow::Owned),
        })
        .collect::<Option<_>>()?;
    // A level starts at a multiple of the block's bytes (and of four); a supercompressed one
    // anywhere.
    let alignment = if supercompression == Supercompression::None {
        texture.format.bytes_per_block()
    } else {
        1
    };

    // Where each level goes: the smallest first, the largest last.
    let descriptor_at = 80 + 24 * level_count;
    let mut at = descriptor_at + descriptor.len();
    let mut offsets = vec![0usize; level_count];
    for level in (0..level_count).rev() {
        at = at.next_multiple_of(alignment);
        offsets[level] = at;
        at += packed[level].len();
    }

    let mut file = Vec::with_capacity(at);
    file.extend_from_slice(&KTX2_IDENTIFIER);
    // The format; a type size of 1, as for all block formats; the size, with the depth of a
    // volume; the layers of an array; the faces; the levels; the supercompression.
    for value in [
        vk_format(texture.format, srgb),
        1,
        texture.width,
        texture.height,
        texture.depth,
        texture.layers,
        if texture.cube { 6 } else { 1 },
        level_count as u32,
        supercompression.scheme(),
    ] {
        put_u32(&mut file, value);
    }
    // The descriptor's place and size; no key and value data; no supercompression data.
    for value in [descriptor_at as u32, descriptor.len() as u32, 0, 0] {
        put_u32(&mut file, value);
    }
    put_u64(&mut file, 0);
    put_u64(&mut file, 0);
    // For each level: where it is, its bytes in the file, and its bytes as blocks.
    for level in 0..level_count {
        put_u64(&mut file, offsets[level] as u64);
        put_u64(&mut file, packed[level].len() as u64);
        put_u64(&mut file, texture.levels[level].len() as u64);
    }
    file.extend_from_slice(&descriptor);
    for level in (0..level_count).rev() {
        file.resize(offsets[level], 0);
        file.extend_from_slice(&packed[level]);
    }
    Some(file)
}

/// Reads a `.ktx2` file (KTX version 2): a 2D texture in BC1, BC2, BC3, BC4, BC5, BC6H or BC7,
/// with its mip levels; a cube map, a texture array, an array of cube maps or a volume texture
/// as well as a plain one, its levels as they are or supercompressed with zlib or with
/// Zstandard. Other formats and the supercompression that is left (BasisLZ, which is only for
/// Basis's own formats) are refused as unsupported.
pub fn read_ktx2(file: &[u8]) -> Result<Texture, TextureError> {
    if file.len() < 12 || file[..12] != KTX2_IDENTIFIER {
        return Err(TextureError::Invalid("no KTX 2 identifier"));
    }
    let mut reader = Reader {
        bytes: file,
        at: 12,
    };
    let mut header = [0u32; 9];
    for value in &mut header {
        *value = reader.u32()?;
    }
    let [
        format_number,
        _type_size,
        width,
        height,
        depth,
        layers,
        faces,
        level_count,
        supercompression,
    ] = header;
    let (format, srgb) = match format_number {
        131 | 133 => (TextureFormat::Bc1, false),
        132 | 134 => (TextureFormat::Bc1, true),
        135 => (TextureFormat::Bc2, false),
        136 => (TextureFormat::Bc2, true),
        137 => (TextureFormat::Bc3, false),
        138 => (TextureFormat::Bc3, true),
        139 => (TextureFormat::Bc4, false),
        141 => (TextureFormat::Bc5, false),
        143 => (TextureFormat::Bc6hUnsigned, false),
        144 => (TextureFormat::Bc6hSigned, false),
        145 => (TextureFormat::Bc7, false),
        146 => (TextureFormat::Bc7, true),
        _ => {
            return Err(TextureError::Unsupported(
                "a Vulkan format other than BC1 to BC7",
            ));
        }
    };
    if faces != 1 && faces != 6 {
        return Err(TextureError::Invalid("faces that are neither one nor six"));
    }
    match supercompression {
        0 | 2 | 3 => {}
        1 => {
            return Err(TextureError::Unsupported(
                "BasisLZ supercompression, which is not for block formats",
            ));
        }
        _ => {
            return Err(TextureError::Unsupported(
                "a supercompression that is not known",
            ));
        }
    }
    let mut texture = texture_from_header(
        format,
        srgb,
        width,
        height,
        depth,
        layers,
        faces == 6,
        level_count,
    )?;

    // (Past the places of the descriptor, the key and value data and the supercompression
    // data, none of which is needed.)
    reader.at = 80;
    for level in 0..texture.levels.len() {
        let (offset, length, uncompressed) = (reader.u64()?, reader.u64()?, reader.u64()?);
        let size = texture.level_bytes(level);
        if (if supercompression == 0 {
            length
        } else {
            uncompressed
        }) != size as u64
        {
            return Err(TextureError::Invalid(
                "a level is not the size its pixels need",
            ));
        }
        let bytes = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(offset, length)| file.get(offset..offset.checked_add(length)?))
            .ok_or(TextureError::Invalid("the file is cut short"))?;
        texture.levels[level] = match supercompression {
            0 => bytes.to_vec(),
            2 => zstd::decompress(bytes, size)
                .ok()
                .filter(|blocks| blocks.len() == size)
                .ok_or(TextureError::Invalid(
                    "a level's compressed bytes are damaged",
                ))?,
            _ => miniz::decompress(bytes, size)
                .ok()
                .filter(|blocks| blocks.len() == size)
                .ok_or(TextureError::Invalid(
                    "a level's compressed bytes are damaged",
                ))?,
        };
    }
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMATS: [TextureFormat; 8] = [
        TextureFormat::Bc1,
        TextureFormat::Bc2,
        TextureFormat::Bc3,
        TextureFormat::Bc4,
        TextureFormat::Bc5,
        TextureFormat::Bc6hUnsigned,
        TextureFormat::Bc6hSigned,
        TextureFormat::Bc7,
    ];

    fn texture(format: TextureFormat, levels: usize) -> Texture {
        shaped(format, levels, 9, 6, 0, 0, false)
    }

    /// A texture of a shape, every byte of it saying where it is.
    fn shaped(
        format: TextureFormat,
        levels: usize,
        width: u32,
        height: u32,
        depth: u32,
        layers: u32,
        cube: bool,
    ) -> Texture {
        let mut texture = Texture {
            format,
            srgb: false,
            width,
            height,
            depth,
            layers,
            cube,
            levels: Vec::new(),
        };
        texture.levels = (0..levels)
            .map(|level| {
                (0..texture.level_bytes(level))
                    .map(|i| (i * 7 + i / texture.image_bytes(level) * 31 + level * 3) as u8)
                    .collect()
            })
            .collect();
        assert!(texture.is_valid());
        texture
    }

    /// Plain, an array of three and of one, a cube map, an array of two cube maps, a volume.
    const SHAPES: [(u32, u32, u32, u32, bool); 6] = [
        (9, 6, 0, 0, false),
        (9, 6, 0, 3, false),
        (9, 6, 0, 1, false),
        (8, 8, 0, 0, true),
        (8, 8, 0, 2, true),
        (9, 6, 5, 0, false),
    ];

    #[test]
    fn every_format_goes_out_and_comes_back() {
        for format in FORMATS {
            for levels in [1, 2, 4] {
                for srgb in [false, true] {
                    let mut out = texture(format, levels);
                    out.srgb = srgb;
                    let (ktx1, ktx2) = (write_ktx1(&out).unwrap(), write_ktx2(&out).unwrap());
                    out.srgb = srgb && format.can_be_srgb();
                    assert_eq!(read_ktx1(&ktx1).unwrap(), out, "{format:?} {levels} {srgb}");
                    assert_eq!(read_ktx2(&ktx2).unwrap(), out, "{format:?} {levels} {srgb}");
                }
            }
        }
    }

    #[test]
    fn every_shape_goes_out_and_comes_back() {
        for format in [
            TextureFormat::Bc1,
            TextureFormat::Bc3,
            TextureFormat::Bc6hUnsigned,
        ] {
            for (width, height, depth, layers, cube) in SHAPES {
                for levels in [1, 3] {
                    let out = shaped(format, levels, width, height, depth, layers, cube);
                    assert_eq!(
                        read_ktx1(&write_ktx1(&out).unwrap()).unwrap(),
                        out,
                        "{format:?} {width} {height} {depth} {layers} {cube} {levels}"
                    );
                    assert_eq!(
                        read_ktx2(&write_ktx2(&out).unwrap()).unwrap(),
                        out,
                        "{format:?} {width} {height} {depth} {layers} {cube} {levels}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_shapes_are_laid_out_as_the_formats_say() {
        let word =
            |file: &[u8], at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());

        // A cube map: six faces in the header, and before each level the size of one face.
        let cube = shaped(TextureFormat::Bc1, 2, 8, 8, 0, 0, true);
        let file = write_ktx1(&cube).unwrap();
        assert_eq!(
            [word(&file, 44), word(&file, 48), word(&file, 52)],
            [0, 0, 6]
        );
        assert_eq!(word(&file, 64), 32);
        assert_eq!(&file[68..68 + 192], &cube.levels[0][..]);
        assert_eq!(word(&file, 68 + 192), 8);
        assert_eq!(file.len(), 64 + 4 + 192 + 4 + 48);
        let file = write_ktx2(&cube).unwrap();
        assert_eq!(
            [word(&file, 28), word(&file, 32), word(&file, 36)],
            [0, 0, 6]
        );
        assert_eq!(u64::from_le_bytes(file[88..96].try_into().unwrap()), 192);

        // An array of two cube maps: the size before a level is of the whole level.
        let cubes = shaped(TextureFormat::Bc1, 1, 8, 8, 0, 2, true);
        let file = write_ktx1(&cubes).unwrap();
        assert_eq!(
            [word(&file, 48), word(&file, 52), word(&file, 64)],
            [2, 6, 384]
        );

        // A volume: its depth in the header, and each level with its slices.
        let volume = shaped(TextureFormat::Bc7, 2, 9, 6, 5, 0, false);
        let file = write_ktx1(&volume).unwrap();
        assert_eq!(
            [
                word(&file, 44),
                word(&file, 48),
                word(&file, 52),
                word(&file, 64)
            ],
            [5, 0, 1, 5 * 96]
        );
        let file = write_ktx2(&volume).unwrap();
        assert_eq!(
            [word(&file, 28), word(&file, 32), word(&file, 36)],
            [5, 0, 1]
        );
        assert_eq!(u64::from_le_bytes(file[88..96].try_into().unwrap()), 5 * 96);
        assert_eq!(
            u64::from_le_bytes(file[112..120].try_into().unwrap()),
            2 * 16
        );

        // Faces that are neither one nor six, a cube map that is not square and an array of
        // volumes are not read.
        let mut faces = write_ktx1(&cube).unwrap();
        faces[52] = 5;
        assert!(matches!(read_ktx1(&faces), Err(TextureError::Invalid(_))));
        let mut oblong = write_ktx2(&cube).unwrap();
        oblong[24] = 4;
        assert_eq!(
            read_ktx2(&oblong),
            Err(TextureError::Invalid("a cube map that is not square"))
        );
        let mut stacked = write_ktx2(&volume).unwrap();
        stacked[32] = 2;
        assert!(matches!(
            read_ktx2(&stacked),
            Err(TextureError::Unsupported(_))
        ));
    }

    #[test]
    fn levels_go_out_supercompressed_and_come_back() {
        // Blocks that compress: the same few over and over.
        let mut out = shaped(TextureFormat::Bc7, 4, 64, 40, 0, 2, false);
        for level in &mut out.levels {
            for (i, byte) in level.iter_mut().enumerate() {
                *byte = (i % 16) as u8 * 3 + (i / 16 % 5) as u8;
            }
        }
        let plain = write_ktx2(&out).unwrap();
        for zlib_level in [1, 6, 9] {
            let level = miniz::Level::new(zlib_level).unwrap();
            let file = write_ktx2_with(&out, Supercompression::Zlib(level)).unwrap();
            assert!(
                file.len() < plain.len() / 4,
                "{zlib_level}: {} {}",
                file.len(),
                plain.len()
            );
            assert_eq!(read_ktx2(&file).unwrap(), out, "{zlib_level}");
            // The scheme is ZLIB, and each level says its bytes in the file and as blocks.
            assert_eq!(u32::from_le_bytes(file[44..48].try_into().unwrap()), 3);
            let entry = |level: usize, field: usize| {
                u64::from_le_bytes(
                    file[80 + level * 24 + field * 8..88 + level * 24 + field * 8]
                        .try_into()
                        .unwrap(),
                ) as usize
            };
            for level in 0..4 {
                assert_eq!(entry(level, 2), out.levels[level].len());
                assert_eq!(
                    miniz::decompress(
                        &file[entry(level, 0)..entry(level, 0) + entry(level, 1)],
                        out.levels[level].len()
                    )
                    .unwrap(),
                    out.levels[level]
                );
            }
            // The levels follow one another with nothing between, the smallest first.
            assert_eq!(entry(3, 0) + entry(3, 1), entry(2, 0));
            assert_eq!(entry(0, 0) + entry(0, 1), file.len());
            // A damaged level is not read, nor one that says it holds another number of bytes.
            let mut damaged = file.clone();
            let middle = entry(0, 0) + entry(0, 1) / 2;
            damaged[middle] ^= 0x10;
            assert!(matches!(read_ktx2(&damaged), Err(TextureError::Invalid(_))));
            let mut wrong = file.clone();
            wrong[80 + 16] ^= 16;
            assert_eq!(
                read_ktx2(&wrong),
                Err(TextureError::Invalid(
                    "a level is not the size its pixels need"
                ))
            );
        }
        assert_eq!(
            write_ktx2_with(&out, Supercompression::None).unwrap(),
            plain
        );

        // The same with Zstandard: the scheme is 2, and each level is one frame.
        for zstd_level in [-5, 1, 3, 9, 19] {
            let level = zstd::Level::new(zstd_level).unwrap();
            let file = write_ktx2_with(&out, Supercompression::Zstd(level)).unwrap();
            assert!(
                file.len() < plain.len() / 4,
                "{zstd_level}: {} {}",
                file.len(),
                plain.len()
            );
            assert_eq!(read_ktx2(&file).unwrap(), out, "{zstd_level}");
            assert_eq!(u32::from_le_bytes(file[44..48].try_into().unwrap()), 2);
            let entry = |level: usize, field: usize| {
                u64::from_le_bytes(
                    file[80 + level * 24 + field * 8..88 + level * 24 + field * 8]
                        .try_into()
                        .unwrap(),
                ) as usize
            };
            for level in 0..4 {
                assert_eq!(entry(level, 2), out.levels[level].len());
                assert_eq!(
                    zstd::decompress(
                        &file[entry(level, 0)..entry(level, 0) + entry(level, 1)],
                        out.levels[level].len()
                    )
                    .unwrap(),
                    out.levels[level]
                );
            }
            assert_eq!(entry(0, 0) + entry(0, 1), file.len());
        }
    }

    #[test]
    fn a_ktx1_file_is_laid_out_as_the_format_says() {
        let file = write_ktx1(&texture(TextureFormat::Bc7, 2)).unwrap();
        assert_eq!(&file[..12], b"\xABKTX 11\xBB\r\n\x1A\n");
        // 13 numbers, then each level behind its size.
        assert_eq!(file.len(), 12 + 52 + (4 + 6 * 16) + (4 + 16));
        assert_eq!(u32::from_le_bytes(file[28..32].try_into().unwrap()), 0x8E8C);
        assert_eq!(u32::from_le_bytes(file[64..68].try_into().unwrap()), 96);
    }

    #[test]
    fn a_ktx2_file_is_laid_out_as_the_format_says() {
        let out = texture(TextureFormat::Bc3, 3);
        let file = write_ktx2(&out).unwrap();
        assert_eq!(&file[..12], b"\xABKTX 20\xBB\r\n\x1A\n");
        assert_eq!(u32::from_le_bytes(file[12..16].try_into().unwrap()), 137);
        // The descriptor follows the three level entries: its size first, two samples.
        let descriptor_at = u32::from_le_bytes(file[48..52].try_into().unwrap()) as usize;
        assert_eq!(descriptor_at, 80 + 3 * 24);
        assert_eq!(
            u32::from_le_bytes(file[52..56].try_into().unwrap()),
            4 + 24 + 32
        );
        assert_eq!(
            u32::from_le_bytes(file[descriptor_at..descriptor_at + 4].try_into().unwrap()),
            60
        );
        // The smallest level comes first, and every level starts at a multiple of 16.
        let offset = |level: usize| {
            u64::from_le_bytes(file[80 + level * 24..88 + level * 24].try_into().unwrap()) as usize
        };
        assert!(offset(2) < offset(1) && offset(1) < offset(0));
        assert!((0..3).all(|level| offset(level) % 16 == 0));
        assert_eq!(&file[offset(0)..], &out.levels[0][..]);
        assert_eq!(file.len(), offset(0) + 6 * 16);
    }

    #[test]
    fn the_descriptors_are_the_khronos_tools_own() {
        // BC7, sRGB: one sample of 128 bits.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc7, true)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        assert_eq!(
            words,
            [
                44,
                0,
                2 | (40 << 16),
                134 | (1 << 8) | (2 << 16),
                0x0303,
                16,
                0,
                127 << 16,
                0,
                0,
                u32::MAX
            ]
        );
        // BC5: red at bit 0 and green at bit 64, 64 bits each.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc5, false)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        assert_eq!(
            words[..7],
            [
                60,
                0,
                2 | (56 << 16),
                132 | (1 << 8) | (1 << 16),
                0x0303,
                16,
                0
            ]
        );
        assert_eq!(
            words[7..],
            [
                63 << 16,
                0,
                0,
                u32::MAX,
                64 | (63 << 16) | (1 << 24),
                0,
                0,
                u32::MAX
            ]
        );
        // BC3, sRGB: alpha at bit 0, marked linear, and colour at bit 64.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc3, true)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        assert_eq!(words[3], 130 | (1 << 8) | (2 << 16));
        assert_eq!(
            words[7..],
            [
                (63 << 16) | (0x1F << 24),
                0,
                0,
                u32::MAX,
                64 | (63 << 16),
                0,
                0,
                u32::MAX
            ]
        );
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc2, false)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        assert_eq!(words[3], 129 | (1 << 8) | (1 << 16));
        assert_eq!(words[7], (63 << 16) | (0x0F << 24));
        // BC6H, signed: a float sample from -1 to 1.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc6hSigned, false)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        assert_eq!(
            words[7..],
            [(127 << 16) | (0xC0 << 24), 0, 0xBF80_0000, 0x3F80_0000]
        );
    }

    #[test]
    fn damaged_files_are_refused() {
        let out = texture(TextureFormat::Bc1, 2);
        let (ktx1, ktx2) = (write_ktx1(&out).unwrap(), write_ktx2(&out).unwrap());
        assert_eq!(
            read_ktx1(&ktx1[..ktx1.len() - 1]),
            Err(TextureError::Invalid("the file is cut short"))
        );
        assert_eq!(
            read_ktx2(&ktx2[..ktx2.len() - 1]),
            Err(TextureError::Invalid("the file is cut short"))
        );
        assert_eq!(
            read_ktx1(&ktx2),
            Err(TextureError::Invalid("no KTX identifier"))
        );
        assert_eq!(
            read_ktx2(&ktx1),
            Err(TextureError::Invalid("no KTX 2 identifier"))
        );
        let mut big_endian = ktx1.clone();
        big_endian[12..16].copy_from_slice(&[4, 3, 2, 1]);
        assert_eq!(
            read_ktx1(&big_endian),
            Err(TextureError::Unsupported("a big-endian file"))
        );
        let mut compressed = ktx2.clone();
        // Said to be compressed with Zstandard, which the blocks are not.
        compressed[44] = 2;
        assert_eq!(
            read_ktx2(&compressed),
            Err(TextureError::Invalid(
                "a level's compressed bytes are damaged"
            ))
        );
        compressed[44] = 1;
        assert!(matches!(
            read_ktx2(&compressed),
            Err(TextureError::Unsupported(_))
        ));
        let mut wrong_size = ktx2;
        wrong_size[88] ^= 8;
        assert!(matches!(
            read_ktx2(&wrong_size),
            Err(TextureError::Invalid(_))
        ));
        let mut broken = out;
        broken.levels.push(Vec::new());
        assert!(write_ktx1(&broken).is_none() && write_ktx2(&broken).is_none());
    }
}
