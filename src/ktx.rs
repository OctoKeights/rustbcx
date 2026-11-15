//! `.ktx` and `.ktx2` files, the Khronos Texture containers that OpenGL and Vulkan tools read
//! and write. (The crate's own: bc7enc_rdo writes `.dds` only.)
//!
//! A `.ktx2` file here holds its levels as they are, without the supercompression (zstd, zlib,
//! Basis) the format allows: that would take a compressor, and this crate has no dependencies.
//! Its data format descriptor is the one the Khronos tools write for each format.

use crate::texture::{check_size, level_bytes, Reader, Texture, TextureError, TextureFormat};

const KTX1_IDENTIFIER: [u8; 12] = [0xAB, b'K', b'T', b'X', b' ', b'1', b'1', 0xBB, b'\r', b'\n', 0x1A, b'\n'];
const KTX2_IDENTIFIER: [u8; 12] = [0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n'];
const KTX1_ENDIANNESS: u32 = 0x0403_0201;

const GL_RED: u32 = 0x1903;
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_RG: u32 = 0x8227;
const GL_COMPRESSED_RGB_S3TC_DXT1_EXT: u32 = 0x83F0;
const GL_COMPRESSED_RGBA_S3TC_DXT1_EXT: u32 = 0x83F1;
const GL_COMPRESSED_RGBA_S3TC_DXT5_EXT: u32 = 0x83F3;
const GL_COMPRESSED_SRGB_S3TC_DXT1_EXT: u32 = 0x8C4C;
const GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT: u32 = 0x8C4D;
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
    let (internal_format, base_internal_format) = gl_formats(texture.format, texture.srgb && texture.format.can_be_srgb());
    let mut file = Vec::with_capacity(64 + texture.levels.iter().map(|level| level.len() + 4).sum::<usize>());
    file.extend_from_slice(&KTX1_IDENTIFIER);
    // Endianness; no type, a type size of 1 and no format, as for all compressed textures; the
    // two internal formats; the size; no depth, no array, one face; the levels; no key and
    // value data.
    for value in [KTX1_ENDIANNESS, 0, 1, 0, internal_format, base_internal_format, texture.width, texture.height, 0, 0, 1, texture.levels.len() as u32, 0] {
        put_u32(&mut file, value);
    }
    for level in &texture.levels {
        put_u32(&mut file, level.len() as u32);
        file.extend_from_slice(level);
        // (Blocks are 8 or 16 bytes, so a level never needs padding to four.)
    }
    Some(file)
}

/// Reads a `.ktx` file (KTX version 1): a 2D texture in BC1, BC3, BC4, BC5, BC6H or BC7, with
/// its mip levels. A file written on a big-endian machine, cube maps, arrays, volumes and
/// other formats are refused as unsupported.
pub fn read_ktx1(file: &[u8]) -> Result<Texture, TextureError> {
    if file.len() < 12 || file[..12] != KTX1_IDENTIFIER {
        return Err(TextureError::Invalid("no KTX identifier"));
    }
    let mut reader = Reader { bytes: file, at: 12 };
    let mut header = [0u32; 13];
    for value in &mut header {
        *value = reader.u32()?;
    }
    let [endianness, gl_type, _type_size, gl_format, internal_format, _base, width, height, depth, array_elements, faces, level_count, key_value_bytes] = header;
    if endianness != KTX1_ENDIANNESS {
        return Err(if endianness == KTX1_ENDIANNESS.swap_bytes() { TextureError::Unsupported("a big-endian file") } else { TextureError::Invalid("the endianness mark is wrong") });
    }
    if gl_type != 0 || gl_format != 0 {
        return Err(TextureError::Unsupported("pixels that are not in blocks"));
    }
    let (format, srgb) = match internal_format {
        GL_COMPRESSED_RGB_S3TC_DXT1_EXT | GL_COMPRESSED_RGBA_S3TC_DXT1_EXT => (TextureFormat::Bc1, false),
        GL_COMPRESSED_SRGB_S3TC_DXT1_EXT | GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT => (TextureFormat::Bc1, true),
        GL_COMPRESSED_RGBA_S3TC_DXT5_EXT => (TextureFormat::Bc3, false),
        GL_COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT => (TextureFormat::Bc3, true),
        GL_COMPRESSED_RED_RGTC1 => (TextureFormat::Bc4, false),
        GL_COMPRESSED_RG_RGTC2 => (TextureFormat::Bc5, false),
        GL_COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT => (TextureFormat::Bc6hUnsigned, false),
        GL_COMPRESSED_RGB_BPTC_SIGNED_FLOAT => (TextureFormat::Bc6hSigned, false),
        GL_COMPRESSED_RGBA_BPTC_UNORM => (TextureFormat::Bc7, false),
        GL_COMPRESSED_SRGB_ALPHA_BPTC_UNORM => (TextureFormat::Bc7, true),
        _ => return Err(TextureError::Unsupported("an OpenGL format other than BC1, BC3, BC4, BC5, BC6H and BC7")),
    };
    if depth > 1 {
        return Err(TextureError::Unsupported("a volume texture"));
    }
    if array_elements > 0 {
        return Err(TextureError::Unsupported("a texture array"));
    }
    if faces != 1 {
        return Err(TextureError::Unsupported("a cube map"));
    }
    let level_count = check_size(width, height, level_count)?;

    let mut at = reader.at.checked_add(key_value_bytes as usize).ok_or(TextureError::Invalid("the file is cut short"))?;
    let mut levels = Vec::with_capacity(level_count as usize);
    for level in 0..level_count as usize {
        reader.at = at;
        let size = reader.u32()? as usize;
        if size != level_bytes(format, width, height, level) {
            return Err(TextureError::Invalid("a level is not the size its pixels need"));
        }
        let blocks = file.get(reader.at..reader.at + size).ok_or(TextureError::Invalid("the file is cut short"))?;
        levels.push(blocks.to_vec());
        at = reader.at + size.next_multiple_of(4);
    }
    Ok(Texture { format, srgb, width, height, levels })
}

/// The data format descriptor of a format, as the Khronos tools write it (`createDFDCompressed`
/// of KTX-Software): its total size, then one basic descriptor block with a sample for each
/// part of a block.
fn data_format_descriptor(format: TextureFormat, srgb: bool) -> Vec<u8> {
    const FLOAT: u32 = 1 << 7;
    const SIGNED: u32 = 1 << 6;
    // The colour model, and for each sample its channel and where its bits start.
    let (model, samples): (u32, &[(u32, u32)]) = match format {
        TextureFormat::Bc1 => (128, &[(0, 0)]),
        TextureFormat::Bc3 => (130, &[(15, 0), (0, 64)]),
        TextureFormat::Bc4 => (131, &[(0, 0)]),
        TextureFormat::Bc5 => (132, &[(0, 0), (1, 64)]),
        TextureFormat::Bc6hUnsigned | TextureFormat::Bc6hSigned => (133, &[(0, 0)]),
        TextureFormat::Bc7 => (134, &[(0, 0)]),
    };
    let sample_bits: u32 = if samples.len() == 2 || format.bytes_per_block() == 8 { 64 } else { 128 };
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
        words.extend_from_slice(&[bit_offset | ((sample_bits - 1) << 16) | ((channel | flags) << 24), 0, lower, upper]);
    }
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The bytes of a `.ktx2` file (KTX version 2) for a texture, its levels not supercompressed.
/// None when the texture is not whole ([`Texture::is_valid`]).
pub fn write_ktx2(texture: &Texture) -> Option<Vec<u8>> {
    if !texture.is_valid() {
        return None;
    }
    let srgb = texture.srgb && texture.format.can_be_srgb();
    let descriptor = data_format_descriptor(texture.format, srgb);
    let level_count = texture.levels.len();
    // A level starts at a multiple of the block's bytes (and of four).
    let alignment = texture.format.bytes_per_block();

    // Where each level goes: the smallest first, the largest last.
    let descriptor_at = 80 + 24 * level_count;
    let mut at = descriptor_at + descriptor.len();
    let mut offsets = vec![0usize; level_count];
    for level in (0..level_count).rev() {
        at = at.next_multiple_of(alignment);
        offsets[level] = at;
        at += texture.levels[level].len();
    }

    let mut file = Vec::with_capacity(at);
    file.extend_from_slice(&KTX2_IDENTIFIER);
    // The format; a type size of 1, as for all block formats; the size; no depth, no layers,
    // one face; the levels; no supercompression.
    for value in [vk_format(texture.format, srgb), 1, texture.width, texture.height, 0, 0, 1, level_count as u32, 0] {
        put_u32(&mut file, value);
    }
    // The descriptor's place and size; no key and value data; no supercompression data.
    for value in [descriptor_at as u32, descriptor.len() as u32, 0, 0] {
        put_u32(&mut file, value);
    }
    put_u64(&mut file, 0);
    put_u64(&mut file, 0);
    for level in 0..level_count {
        put_u64(&mut file, offsets[level] as u64);
        put_u64(&mut file, texture.levels[level].len() as u64);
        put_u64(&mut file, texture.levels[level].len() as u64);
    }
    file.extend_from_slice(&descriptor);
    for level in (0..level_count).rev() {
        file.resize(offsets[level], 0);
        file.extend_from_slice(&texture.levels[level]);
    }
    Some(file)
}

/// Reads a `.ktx2` file (KTX version 2): a 2D texture in BC1, BC3, BC4, BC5, BC6H or BC7, with
/// its mip levels. Supercompressed levels (zstd, zlib, Basis), cube maps, arrays, volumes and
/// other formats are refused as unsupported.
pub fn read_ktx2(file: &[u8]) -> Result<Texture, TextureError> {
    if file.len() < 12 || file[..12] != KTX2_IDENTIFIER {
        return Err(TextureError::Invalid("no KTX 2 identifier"));
    }
    let mut reader = Reader { bytes: file, at: 12 };
    let mut header = [0u32; 9];
    for value in &mut header {
        *value = reader.u32()?;
    }
    let [format_number, _type_size, width, height, depth, layers, faces, level_count, supercompression] = header;
    let (format, srgb) = match format_number {
        131 | 133 => (TextureFormat::Bc1, false),
        132 | 134 => (TextureFormat::Bc1, true),
        137 => (TextureFormat::Bc3, false),
        138 => (TextureFormat::Bc3, true),
        139 => (TextureFormat::Bc4, false),
        141 => (TextureFormat::Bc5, false),
        143 => (TextureFormat::Bc6hUnsigned, false),
        144 => (TextureFormat::Bc6hSigned, false),
        145 => (TextureFormat::Bc7, false),
        146 => (TextureFormat::Bc7, true),
        _ => return Err(TextureError::Unsupported("a Vulkan format other than BC1, BC3, BC4, BC5, BC6H and BC7")),
    };
    if depth > 1 {
        return Err(TextureError::Unsupported("a volume texture"));
    }
    if layers > 0 {
        return Err(TextureError::Unsupported("a texture array"));
    }
    if faces != 1 {
        return Err(TextureError::Unsupported("a cube map"));
    }
    if supercompression != 0 {
        return Err(TextureError::Unsupported("supercompressed levels"));
    }
    let level_count = check_size(width, height, level_count)?;

    // (Past the places of the descriptor, the key and value data and the supercompression
    // data, none of which is needed.)
    reader.at = 80;
    let mut levels = Vec::with_capacity(level_count as usize);
    for level in 0..level_count as usize {
        let (offset, length, _uncompressed) = (reader.u64()?, reader.u64()?, reader.u64()?);
        if length != level_bytes(format, width, height, level) as u64 {
            return Err(TextureError::Invalid("a level is not the size its pixels need"));
        }
        let blocks = usize::try_from(offset).ok().and_then(|offset| file.get(offset..offset.checked_add(length as usize)?)).ok_or(TextureError::Invalid("the file is cut short"))?;
        levels.push(blocks.to_vec());
    }
    Ok(Texture { format, srgb, width, height, levels })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMATS: [TextureFormat; 7] = [TextureFormat::Bc1, TextureFormat::Bc3, TextureFormat::Bc4, TextureFormat::Bc5, TextureFormat::Bc6hUnsigned, TextureFormat::Bc6hSigned, TextureFormat::Bc7];

    fn texture(format: TextureFormat, levels: usize) -> Texture {
        let (width, height) = (9, 6);
        let levels = (0..levels).map(|level| (0..level_bytes(format, width, height, level)).map(|i| (i * 7 + level) as u8).collect()).collect();
        Texture { format, srgb: false, width, height, levels }
    }

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
        assert_eq!(u32::from_le_bytes(file[52..56].try_into().unwrap()), 4 + 24 + 32);
        assert_eq!(u32::from_le_bytes(file[descriptor_at..descriptor_at + 4].try_into().unwrap()), 60);
        // The smallest level comes first, and every level starts at a multiple of 16.
        let offset = |level: usize| u64::from_le_bytes(file[80 + level * 24..88 + level * 24].try_into().unwrap()) as usize;
        assert!(offset(2) < offset(1) && offset(1) < offset(0));
        assert!((0..3).all(|level| offset(level) % 16 == 0));
        assert_eq!(&file[offset(0)..], &out.levels[0][..]);
        assert_eq!(file.len(), offset(0) + 6 * 16);
    }

    #[test]
    fn the_descriptors_are_the_khronos_tools_own() {
        // BC7, sRGB: one sample of 128 bits.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc7, true).chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
        assert_eq!(words, [44, 0, 2 | (40 << 16), 134 | (1 << 8) | (2 << 16), 0x0303, 16, 0, 127 << 16, 0, 0, u32::MAX]);
        // BC5: red at bit 0 and green at bit 64, 64 bits each.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc5, false).chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
        assert_eq!(words[..7], [60, 0, 2 | (56 << 16), 132 | (1 << 8) | (1 << 16), 0x0303, 16, 0]);
        assert_eq!(words[7..], [63 << 16, 0, 0, u32::MAX, 64 | (63 << 16) | (1 << 24), 0, 0, u32::MAX]);
        // BC6H, signed: a float sample from -1 to 1.
        let words: Vec<u32> = data_format_descriptor(TextureFormat::Bc6hSigned, false).chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
        assert_eq!(words[7..], [(127 << 16) | (0xC0 << 24), 0, 0xBF80_0000, 0x3F80_0000]);
    }

    #[test]
    fn damaged_files_are_refused() {
        let out = texture(TextureFormat::Bc1, 2);
        let (ktx1, ktx2) = (write_ktx1(&out).unwrap(), write_ktx2(&out).unwrap());
        assert_eq!(read_ktx1(&ktx1[..ktx1.len() - 1]), Err(TextureError::Invalid("the file is cut short")));
        assert_eq!(read_ktx2(&ktx2[..ktx2.len() - 1]), Err(TextureError::Invalid("the file is cut short")));
        assert_eq!(read_ktx1(&ktx2), Err(TextureError::Invalid("no KTX identifier")));
        assert_eq!(read_ktx2(&ktx1), Err(TextureError::Invalid("no KTX 2 identifier")));
        let mut big_endian = ktx1.clone();
        big_endian[12..16].copy_from_slice(&[4, 3, 2, 1]);
        assert_eq!(read_ktx1(&big_endian), Err(TextureError::Unsupported("a big-endian file")));
        let mut cube = ktx1;
        cube[52] = 6;
        assert_eq!(read_ktx1(&cube), Err(TextureError::Unsupported("a cube map")));
        let mut compressed = ktx2.clone();
        compressed[44] = 2;
        assert_eq!(read_ktx2(&compressed), Err(TextureError::Unsupported("supercompressed levels")));
        let mut wrong_size = ktx2;
        wrong_size[88] ^= 8;
        assert!(matches!(read_ktx2(&wrong_size), Err(TextureError::Invalid(_))));
        let mut broken = out;
        broken.levels.push(Vec::new());
        assert!(write_ktx1(&broken).is_none() && write_ktx2(&broken).is_none());
    }
}
