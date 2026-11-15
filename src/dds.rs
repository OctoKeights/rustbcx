//! `.dds` files, DirectDraw Surface: the container Direct3D tools read and write.
//!
//! [`write()`] for one level, without sRGB, gives the bytes bc7enc_rdo's `save_dds`
//! (`utils.cpp`) writes. Mip levels, sRGB, BC2, BC6H and [`read()`] are the crate's own.

use crate::texture::{check_size, level_bytes, Reader, Texture, TextureError, TextureFormat};

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;
const DDPF_FOURCC: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;
const D3D10_RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;
const DDS_RESOURCE_MISC_TEXTURECUBE: u32 = 0x4;

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

/// The format's number in DXGI.
fn dxgi_format(format: TextureFormat, srgb: bool) -> u32 {
    match (format, srgb) {
        (TextureFormat::Bc1, false) => 71,
        (TextureFormat::Bc1, true) => 72,
        (TextureFormat::Bc2, false) => 74,
        (TextureFormat::Bc2, true) => 75,
        (TextureFormat::Bc3, false) => 77,
        (TextureFormat::Bc3, true) => 78,
        (TextureFormat::Bc4, _) => 80,
        (TextureFormat::Bc5, _) => 83,
        (TextureFormat::Bc6hUnsigned, _) => 95,
        (TextureFormat::Bc6hSigned, _) => 96,
        (TextureFormat::Bc7, false) => 98,
        (TextureFormat::Bc7, true) => 99,
    }
}

/// The bytes of a `.dds` file for a texture.
///
/// BC1, BC2, BC3, BC4 and BC5 get the old header with a FourCC (`DXT1`, `DXT3`, `DXT5`, `ATI1`,
/// `ATI2`),
/// which every tool reads, unless `force_dx10_header` is set; BC7, BC6H and anything marked
/// sRGB get the DX10 header, which is the only one that can say so.
///
/// None when the texture is not whole ([`Texture::is_valid`]).
pub fn write(texture: &Texture, force_dx10_header: bool) -> Option<Vec<u8>> {
    if !texture.is_valid() {
        return None;
    }
    let srgb = texture.srgb && texture.format.can_be_srgb();
    let legacy_fourcc = match texture.format {
        _ if force_dx10_header || srgb => None,
        TextureFormat::Bc1 => Some(fourcc(b"DXT1")),
        TextureFormat::Bc2 => Some(fourcc(b"DXT3")),
        TextureFormat::Bc3 => Some(fourcc(b"DXT5")),
        TextureFormat::Bc4 => Some(fourcc(b"ATI1")),
        TextureFormat::Bc5 => Some(fourcc(b"ATI2")),
        _ => None,
    };
    let levels = texture.levels.len() as u32;

    // DDSURFACEDESC2, 31 numbers after the signature.
    let mut desc = [0u32; 31];
    desc[0] = 124;
    desc[1] = DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT | DDSD_CAPS | DDSD_LINEARSIZE;
    desc[2] = texture.height;
    desc[3] = texture.width;
    desc[4] = texture.levels[0].len() as u32;
    if levels > 1 {
        desc[1] |= DDSD_MIPMAPCOUNT;
        desc[6] = levels;
    }
    desc[18] = 32;
    desc[19] = DDPF_FOURCC;
    desc[20] = legacy_fourcc.unwrap_or(fourcc(b"DX10"));
    desc[26] = DDSCAPS_TEXTURE | if levels > 1 { DDSCAPS_COMPLEX | DDSCAPS_MIPMAP } else { 0 };

    let mut file = Vec::with_capacity(148 + texture.levels.iter().map(Vec::len).sum::<usize>());
    file.extend_from_slice(b"DDS ");
    for value in desc {
        file.extend_from_slice(&value.to_le_bytes());
    }
    if legacy_fourcc.is_none() {
        // DDS_HEADER_DXT10
        for value in [dxgi_format(texture.format, srgb), D3D10_RESOURCE_DIMENSION_TEXTURE2D, 0, 1, 0] {
            file.extend_from_slice(&value.to_le_bytes());
        }
    }
    for level in &texture.levels {
        file.extend_from_slice(level);
    }
    Some(file)
}

/// Reads a `.dds` file: a 2D texture in BC1, BC2, BC3, BC4, BC5, BC6H or BC7, with its mip
/// levels. (The old FourCCs `DXT2` and `DXT4` are BC2 and BC3 with the colours multiplied by
/// alpha; they are read as BC2 and BC3, and that the colours are multiplied is not kept.)
/// Cube maps, arrays, volumes and other pixel formats are refused as unsupported.
pub fn read(file: &[u8]) -> Result<Texture, TextureError> {
    if file.len() < 4 || &file[..4] != b"DDS " {
        return Err(TextureError::Invalid("no DDS signature"));
    }
    let mut reader = Reader { bytes: file, at: 4 };
    let mut desc = [0u32; 31];
    for value in &mut desc {
        *value = reader.u32()?;
    }
    if desc[0] != 124 || desc[18] != 32 {
        return Err(TextureError::Invalid("the header's sizes are wrong"));
    }
    let (height, width) = (desc[2], desc[3]);
    if desc[27] & DDSCAPS2_CUBEMAP != 0 {
        return Err(TextureError::Unsupported("a cube map"));
    }
    if desc[27] & DDSCAPS2_VOLUME != 0 {
        return Err(TextureError::Unsupported("a volume texture"));
    }
    if desc[19] & DDPF_FOURCC == 0 {
        return Err(TextureError::Unsupported("pixels that are not in blocks"));
    }

    let code = desc[20];
    let (format, srgb) = if code == fourcc(b"DX10") {
        let (dxgi, dimension, misc, array_size) = (reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?);
        reader.u32()?;
        if dimension != D3D10_RESOURCE_DIMENSION_TEXTURE2D {
            return Err(TextureError::Unsupported("a texture that is not 2D"));
        }
        if misc & DDS_RESOURCE_MISC_TEXTURECUBE != 0 {
            return Err(TextureError::Unsupported("a cube map"));
        }
        if array_size > 1 {
            return Err(TextureError::Unsupported("a texture array"));
        }
        match dxgi {
            70 | 71 => (TextureFormat::Bc1, false),
            72 => (TextureFormat::Bc1, true),
            73 | 74 => (TextureFormat::Bc2, false),
            75 => (TextureFormat::Bc2, true),
            76 | 77 => (TextureFormat::Bc3, false),
            78 => (TextureFormat::Bc3, true),
            79 | 80 => (TextureFormat::Bc4, false),
            82 | 83 => (TextureFormat::Bc5, false),
            94 | 95 => (TextureFormat::Bc6hUnsigned, false),
            96 => (TextureFormat::Bc6hSigned, false),
            97 | 98 => (TextureFormat::Bc7, false),
            99 => (TextureFormat::Bc7, true),
            _ => return Err(TextureError::Unsupported("a DXGI format other than BC1 to BC7")),
        }
    } else if code == fourcc(b"DXT1") {
        (TextureFormat::Bc1, false)
    } else if code == fourcc(b"DXT2") || code == fourcc(b"DXT3") {
        (TextureFormat::Bc2, false)
    } else if code == fourcc(b"DXT4") || code == fourcc(b"DXT5") {
        (TextureFormat::Bc3, false)
    } else if code == fourcc(b"ATI1") || code == fourcc(b"BC4U") {
        (TextureFormat::Bc4, false)
    } else if code == fourcc(b"ATI2") || code == fourcc(b"BC5U") {
        (TextureFormat::Bc5, false)
    } else {
        return Err(TextureError::Unsupported("a FourCC other than DXT1 to DXT5, ATI1 and ATI2"));
    };

    let level_count = check_size(width, height, if desc[1] & DDSD_MIPMAPCOUNT != 0 { desc[6] } else { 1 })?;
    let mut levels = Vec::with_capacity(level_count as usize);
    let mut at = reader.at;
    for level in 0..level_count as usize {
        let size = level_bytes(format, width, height, level);
        let blocks = file.get(at..at + size).ok_or(TextureError::Invalid("the file is cut short"))?;
        levels.push(blocks.to_vec());
        at += size;
    }
    Ok(Texture { format, srgb, width, height, levels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(format: TextureFormat, levels: usize) -> Texture {
        let (width, height) = (9, 6);
        let levels = (0..levels).map(|level| (0..level_bytes(format, width, height, level)).map(|i| (i * 7 + level) as u8).collect()).collect();
        Texture { format, srgb: false, width, height, levels }
    }

    #[test]
    fn every_format_goes_out_and_comes_back() {
        for format in [TextureFormat::Bc1, TextureFormat::Bc2, TextureFormat::Bc3, TextureFormat::Bc4, TextureFormat::Bc5, TextureFormat::Bc6hUnsigned, TextureFormat::Bc6hSigned, TextureFormat::Bc7] {
            for levels in [1, 2, 4] {
                for srgb in [false, true] {
                    for dx10 in [false, true] {
                        let mut out = texture(format, levels);
                        out.srgb = srgb;
                        let file = write(&out, dx10).unwrap();
                        let back = read(&file).unwrap();
                        // sRGB is kept where the format can be.
                        out.srgb = srgb && format.can_be_srgb();
                        assert_eq!(back, out, "{format:?} {levels} {srgb} {dx10}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_old_header_is_used_where_it_can_be() {
        let file = write(&texture(TextureFormat::Bc1, 1), false).unwrap();
        assert_eq!(file.len(), 128 + 3 * 2 * 8);
        assert_eq!(&file[84..88], b"DXT1");
        // Width, height and the size of the first level.
        assert_eq!(u32::from_le_bytes(file[16..20].try_into().unwrap()), 9);
        assert_eq!(u32::from_le_bytes(file[12..16].try_into().unwrap()), 6);
        assert_eq!(u32::from_le_bytes(file[20..24].try_into().unwrap()), 48);
        let file = write(&texture(TextureFormat::Bc7, 3), false).unwrap();
        assert_eq!(&file[84..88], b"DX10");
        assert_eq!(u32::from_le_bytes(file[128..132].try_into().unwrap()), 98);
        assert_eq!(u32::from_le_bytes(file[28..32].try_into().unwrap()), 3);
        // Nine by six pixels is six blocks; the two levels after it are one block each.
        assert_eq!(file.len(), 148 + (6 + 1 + 1) * 16);
    }

    #[test]
    fn damaged_files_are_refused() {
        let file = write(&texture(TextureFormat::Bc3, 2), false).unwrap();
        assert_eq!(read(&file[..file.len() - 1]), Err(TextureError::Invalid("the file is cut short")));
        assert_eq!(read(&file[..100]), Err(TextureError::Invalid("the file is cut short")));
        assert_eq!(read(b"DDX "), Err(TextureError::Invalid("no DDS signature")));
        let mut other = file.clone();
        other[84..88].copy_from_slice(b"UYVY");
        assert!(matches!(read(&other), Err(TextureError::Unsupported(_))));
        let mut cube = file.clone();
        cube[112..116].copy_from_slice(&0x200u32.to_le_bytes());
        assert!(matches!(read(&cube), Err(TextureError::Unsupported("a cube map"))));
        let mut levels = file;
        levels[28] = 9;
        assert!(matches!(read(&levels), Err(TextureError::Invalid(_))));
        // A texture that is not whole is not written.
        let mut broken = texture(TextureFormat::Bc1, 1);
        broken.levels[0].pop();
        assert!(write(&broken, false).is_none());
    }
}
