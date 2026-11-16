//! `.dds` files, DirectDraw Surface: the container Direct3D tools read and write.
//!
//! [`write()`] for one level, without sRGB, gives the bytes bc7enc_rdo's `save_dds`
//! (`utils.cpp`) writes. Mip levels, sRGB, BC2, BC6H, cube maps, arrays, volume textures and
//! [`read()`] are the crate's own.
//!
//! A `.dds` file holds a texture of several images image by image, each with all its mip
//! levels (a volume texture level by level, each with all its slices); [`Texture`] holds it
//! level by level, and the two here put it one way and the other.

use crate::texture::{Reader, Texture, TextureError, TextureFormat, texture_from_header};

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;
const DDSD_DEPTH: u32 = 0x80_0000;
const DDPF_FOURCC: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
/// The six faces of a cube map, each with its own flag.
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0xFC00;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;
const D3D10_RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;
const D3D10_RESOURCE_DIMENSION_TEXTURE3D: u32 = 4;
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
/// which every tool reads, unless `force_dx10_header` is set; BC7, BC6H, anything marked sRGB
/// and every texture array get the DX10 header, which is the only one that can say so. A cube
/// map and a volume texture fit either header.
///
/// None when the texture is not whole ([`Texture::is_valid`]).
pub fn write(texture: &Texture, force_dx10_header: bool) -> Option<Vec<u8>> {
    if !texture.is_valid() {
        return None;
    }
    let srgb = texture.srgb && texture.format.can_be_srgb();
    let legacy_fourcc = match texture.format {
        _ if force_dx10_header || srgb || texture.layers != 0 => None,
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
    // (The size of the first image, as for a texture of one.)
    desc[4] = texture.image_bytes(0) as u32;
    if texture.depth != 0 {
        desc[1] |= DDSD_DEPTH;
        desc[5] = texture.depth;
    }
    if levels > 1 {
        desc[1] |= DDSD_MIPMAPCOUNT;
        desc[6] = levels;
    }
    desc[18] = 32;
    desc[19] = DDPF_FOURCC;
    desc[20] = legacy_fourcc.unwrap_or(fourcc(b"DX10"));
    // (Complex: more than one surface in the file.)
    desc[26] = DDSCAPS_TEXTURE
        | if levels > 1 { DDSCAPS_MIPMAP } else { 0 }
        | if levels > 1 || texture.cube || texture.depth != 0 || texture.layers > 1 {
            DDSCAPS_COMPLEX
        } else {
            0
        };
    desc[27] = if texture.cube {
        DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_ALLFACES
    } else {
        0
    } | if texture.depth != 0 {
        DDSCAPS2_VOLUME
    } else {
        0
    };

    let mut file = Vec::with_capacity(148 + texture.levels.iter().map(Vec::len).sum::<usize>());
    file.extend_from_slice(b"DDS ");
    for value in desc {
        file.extend_from_slice(&value.to_le_bytes());
    }
    if legacy_fourcc.is_none() {
        // DDS_HEADER_DXT10: the format, the dimension, whether a cube map, the number of
        // textures in the array (of cube maps, for an array of them), no word on alpha.
        let dimension = if texture.depth != 0 {
            D3D10_RESOURCE_DIMENSION_TEXTURE3D
        } else {
            D3D10_RESOURCE_DIMENSION_TEXTURE2D
        };
        for value in [
            dxgi_format(texture.format, srgb),
            dimension,
            if texture.cube {
                DDS_RESOURCE_MISC_TEXTURECUBE
            } else {
                0
            },
            texture.layers.max(1),
            0,
        ] {
            file.extend_from_slice(&value.to_le_bytes());
        }
    }
    if texture.depth != 0 {
        // A volume: level by level, as the texture holds it.
        for level in &texture.levels {
            file.extend_from_slice(level);
        }
    } else {
        // Anything else: image by image, each with its levels.
        for image in 0..texture.images_per_level(0) {
            for level in 0..texture.levels.len() {
                file.extend_from_slice(texture.image(level, image)?);
            }
        }
    }
    Some(file)
}

/// Reads a `.dds` file: a texture in BC1, BC2, BC3, BC4, BC5, BC6H or BC7, with its mip
/// levels; a cube map, a texture array, an array of cube maps or a volume texture as well as a
/// plain one. (The old FourCCs `DXT2` and `DXT4` are BC2 and BC3 with the colours multiplied by
/// alpha; they are read as BC2 and BC3, and that the colours are multiplied is not kept.)
/// Other pixel formats, 1D textures and cube maps with faces missing are refused as
/// unsupported.
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
    if desc[19] & DDPF_FOURCC == 0 {
        return Err(TextureError::Unsupported("pixels that are not in blocks"));
    }
    // What the old header says of the shape; the DX10 header, when there is one, says it again
    // and is the one believed.
    let mut cube = desc[27] & DDSCAPS2_CUBEMAP != 0;
    let mut depth = if desc[27] & DDSCAPS2_VOLUME != 0 {
        desc[5].max(1)
    } else {
        0
    };
    let mut layers = 0;

    let code = desc[20];
    let (format, srgb) = if code == fourcc(b"DX10") {
        let (dxgi, dimension, misc, array_size) =
            (reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?);
        reader.u32()?;
        match dimension {
            D3D10_RESOURCE_DIMENSION_TEXTURE2D => {
                cube = misc & DDS_RESOURCE_MISC_TEXTURECUBE != 0;
                depth = 0;
                layers = if array_size > 1 { array_size } else { 0 };
            }
            D3D10_RESOURCE_DIMENSION_TEXTURE3D => {
                if array_size > 1 {
                    return Err(TextureError::Unsupported(
                        "an array or a cube map of volume textures",
                    ));
                }
                cube = false;
                depth = desc[5].max(1);
            }
            _ => {
                return Err(TextureError::Unsupported(
                    "a texture that is neither 2D nor 3D",
                ));
            }
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
            _ => {
                return Err(TextureError::Unsupported(
                    "a DXGI format other than BC1 to BC7",
                ));
            }
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
        return Err(TextureError::Unsupported(
            "a FourCC other than DXT1 to DXT5, ATI1 and ATI2",
        ));
    };

    // (Only the old header can leave faces of a cube map out.)
    if code != fourcc(b"DX10")
        && cube
        && desc[27] & DDSCAPS2_CUBEMAP_ALLFACES != DDSCAPS2_CUBEMAP_ALLFACES
    {
        return Err(TextureError::Unsupported(
            "a cube map without all six faces",
        ));
    }

    let mut texture = texture_from_header(
        format,
        srgb,
        width,
        height,
        depth,
        layers,
        cube,
        if desc[1] & DDSD_MIPMAPCOUNT != 0 {
            desc[6]
        } else {
            1
        },
    )?;
    let mut at = reader.at;
    let mut take = |size: usize| -> Result<&[u8], TextureError> {
        let blocks = at
            .checked_add(size)
            .and_then(|end| file.get(at..end))
            .ok_or(TextureError::Invalid("the file is cut short"))?;
        at += size;
        Ok(blocks)
    };
    if depth != 0 {
        for level in 0..texture.levels.len() {
            texture.levels[level] = take(texture.level_bytes(level))?.to_vec();
        }
    } else {
        for _ in 0..texture.images_per_level(0) {
            for level in 0..texture.levels.len() {
                let blocks = take(texture.image_bytes(level))?;
                texture.levels[level].extend_from_slice(blocks);
            }
        }
    }
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for format in [
            TextureFormat::Bc1,
            TextureFormat::Bc2,
            TextureFormat::Bc3,
            TextureFormat::Bc4,
            TextureFormat::Bc5,
            TextureFormat::Bc6hUnsigned,
            TextureFormat::Bc6hSigned,
            TextureFormat::Bc7,
        ] {
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
    fn every_shape_goes_out_and_comes_back() {
        for format in [
            TextureFormat::Bc1,
            TextureFormat::Bc3,
            TextureFormat::Bc6hUnsigned,
        ] {
            for (width, height, depth, layers, cube) in SHAPES {
                for levels in [1, 3] {
                    for dx10 in [false, true] {
                        let mut out = shaped(format, levels, width, height, depth, layers, cube);
                        let back = read(&write(&out, dx10).unwrap()).unwrap();
                        // (A .dds file cannot say that one texture is an array.)
                        if layers == 1 {
                            out.layers = 0;
                        }
                        assert_eq!(
                            back, out,
                            "{format:?} {width} {height} {depth} {layers} {cube} {levels} {dx10}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_shapes_are_laid_out_as_the_format_says() {
        let word =
            |file: &[u8], at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());

        // A cube map with the old header: all six faces flagged, and face by face, each with
        // its levels.
        let cube = shaped(TextureFormat::Bc1, 2, 8, 8, 0, 0, true);
        let file = write(&cube, false).unwrap();
        assert_eq!(&file[84..88], b"DXT1");
        assert_eq!(word(&file, 108) & DDSCAPS_COMPLEX, DDSCAPS_COMPLEX);
        assert_eq!(word(&file, 112), 0xFE00);
        assert_eq!(word(&file, 20), 32);
        assert_eq!(file.len(), 128 + 6 * (32 + 8));
        assert_eq!(&file[128..160], cube.image(0, 0).unwrap());
        assert_eq!(&file[160..168], cube.image(1, 0).unwrap());
        assert_eq!(&file[168..200], cube.image(0, 1).unwrap());
        assert_eq!(&file[file.len() - 8..], cube.image(1, 5).unwrap());

        // An array of two cube maps: the DX10 header, a cube map, two of them, twelve faces.
        let cubes = shaped(TextureFormat::Bc1, 1, 8, 8, 0, 2, true);
        let file = write(&cubes, false).unwrap();
        assert_eq!(&file[84..88], b"DX10");
        assert_eq!(
            [word(&file, 132), word(&file, 136), word(&file, 140)],
            [3, 4, 2]
        );
        assert_eq!(file.len(), 148 + 12 * 32);

        // A volume: its depth in the header, and level by level, each with its slices.
        let volume = shaped(TextureFormat::Bc1, 2, 9, 6, 5, 0, false);
        let file = write(&volume, false).unwrap();
        assert_eq!(word(&file, 8) & DDSD_DEPTH, DDSD_DEPTH);
        assert_eq!(word(&file, 24), 5);
        assert_eq!(word(&file, 112), DDSCAPS2_VOLUME);
        assert_eq!(file.len(), 128 + 5 * 48 + 2 * 8);
        assert_eq!(&file[128..128 + 240], &volume.levels[0][..]);
        assert_eq!(&file[128 + 240..], &volume.levels[1][..]);
        let file = write(&volume, true).unwrap();
        assert_eq!(
            [word(&file, 132), word(&file, 136), word(&file, 140)],
            [4, 0, 1]
        );

        // A cube map with a face left out is refused, as is a texture of one dimension.
        let mut partial = write(&cube, false).unwrap();
        partial[113] &= !0x04;
        assert_eq!(
            read(&partial),
            Err(TextureError::Unsupported(
                "a cube map without all six faces"
            ))
        );
        let mut line = write(&cube, true).unwrap();
        line[132] = 2;
        assert!(matches!(read(&line), Err(TextureError::Unsupported(_))));
        // A cube map has to be square, and a volume cannot be an array.
        let mut oblong = write(&cube, false).unwrap();
        oblong[12] = 4;
        assert_eq!(
            read(&oblong),
            Err(TextureError::Invalid("a cube map that is not square"))
        );
        let mut stacked = write(&volume, true).unwrap();
        stacked[140] = 2;
        assert!(matches!(read(&stacked), Err(TextureError::Unsupported(_))));
        // Cut short anywhere among the faces, it is not read.
        let file = write(&cube, false).unwrap();
        assert_eq!(
            read(&file[..file.len() - 41]),
            Err(TextureError::Invalid("the file is cut short"))
        );
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
        assert_eq!(
            read(&file[..file.len() - 1]),
            Err(TextureError::Invalid("the file is cut short"))
        );
        assert_eq!(
            read(&file[..100]),
            Err(TextureError::Invalid("the file is cut short"))
        );
        assert_eq!(
            read(b"DDX "),
            Err(TextureError::Invalid("no DDS signature"))
        );
        let mut other = file.clone();
        other[84..88].copy_from_slice(b"UYVY");
        assert!(matches!(read(&other), Err(TextureError::Unsupported(_))));
        let mut levels = file;
        levels[28] = 9;
        assert!(matches!(read(&levels), Err(TextureError::Invalid(_))));
        // A texture that is not whole is not written.
        let mut broken = texture(TextureFormat::Bc1, 1);
        broken.levels[0].pop();
        assert!(write(&broken, false).is_none());
    }
}
