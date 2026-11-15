//! Times the encoders on made-up images, so that a change's effect on speed can be seen and so
//! that the README's figures can be made again.
//!
//!   bench [size]    the side of the square image in pixels (512 unless given)
//!
//! Each line is one way of packing the image: the time on one thread, the time on all the
//! machine's threads, and thousands of blocks a second for each.

use rustbcx::bc6h::Bc6hEncSettings;
use rustbcx::image::{HdrImage, Image};
use rustbcx::rdo_bc_encoder::{BlockFormat, RdoBcParams};
use rustbcx::texture;
use std::time::Instant;

#[path = "../tests/made_up/mod.rs"]
mod made_up;

/// An image with something of everything: ramps, flat boxes, a tile, noise.
fn image(size: u32) -> Image {
    let mut random = made_up::Random { state: 7 };
    let mut image = Image::new(size, size);
    let quarter = made_up::make_image(&mut random, size, size);
    for y in 0..size {
        for x in 0..size {
            let ramp = [(x * 255 / size) as u8, (y * 255 / size) as u8, ((x + y) * 127 / size) as u8, 255];
            image.pixels[(x + y * size) as usize] = match (x * 2 / size, y * 2 / size) {
                (0, 0) => ramp,
                (1, 0) => quarter.pixels[(x + y * size) as usize],
                (0, 1) => [(x / 16 * 40) as u8, (y / 16 * 30) as u8, 90, 255],
                _ => [ramp[0] ^ (random.next() % 16) as u8, ramp[1], ramp[2] ^ (random.next() % 8) as u8, (random.next() % 256) as u8],
            };
        }
    }
    image
}

fn time(mut work: impl FnMut()) -> f64 {
    let start = Instant::now();
    work();
    start.elapsed().as_secs_f64()
}

fn main() {
    let size: u32 = std::env::args().nth(1).and_then(|size| size.parse().ok()).unwrap_or(512);
    let image = image(size);
    let hdr = HdrImage::from_rgb_f32(size, size, &image.pixels.iter().flat_map(|pixel| [f32::from(pixel[0]) / 64.0, f32::from(pixel[1]) / 64.0, f32::from(pixel[2]) / 64.0]).collect::<Vec<f32>>()).expect("an image");
    let blocks = f64::from(size.div_ceil(4) * size.div_ceil(4));
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    println!("{size} x {size} pixels, {blocks} blocks, {threads} threads");
    println!("{:<34} {:>9} {:>9} {:>10} {:>10}", "", "1 thread", "all", "kblocks/s", "kblocks/s");

    let report = |name: &str, one: f64, all: f64| println!("{name:<34} {one:>8.2}s {all:>8.2}s {:>10.1} {:>10.1}", blocks / one / 1000.0, blocks / all / 1000.0);

    let ldr = |name: &str, change: &dyn Fn(&mut RdoBcParams)| {
        let mut params = RdoBcParams::new();
        change(&mut params);
        params.threads = 1;
        let one = time(|| {
            texture::compress(&image, &params, false).expect("a texture");
        });
        params.threads = 0;
        let all = time(|| {
            texture::compress(&image, &params, false).expect("a texture");
        });
        report(name, one, all);
    };

    ldr("BC1, level 5", &|p| (p.format, p.bc1_quality_level) = (BlockFormat::Bc1, 5));
    ldr("BC1, level 18", &|p| p.format = BlockFormat::Bc1);
    ldr("BC3, level 10", &|p| (p.format, p.bc1_quality_level) = (BlockFormat::Bc3, 10));
    ldr("BC4", &|p| p.format = BlockFormat::Bc4);
    ldr("BC5", &|p| p.format = BlockFormat::Bc5);
    ldr("BC7, bc7enc, uber 0", &|p| (p.use_bc7e, p.bc7_uber_level) = (false, 0));
    ldr("BC7, bc7enc, uber 4", &|p| (p.use_bc7e, p.bc7_uber_level) = (false, 4));
    ldr("BC7, bc7e, ultrafast", &|p| p.bc7_uber_level = 0);
    ldr("BC7, bc7e, basic", &|p| p.bc7_uber_level = 3);
    ldr("BC7, bc7e, slowest", &|p| p.bc7_uber_level = 6);
    ldr("BC7, bc7e basic, RDO lambda 1", &|p| (p.bc7_uber_level, p.rdo_lambda) = (3, 1.0));
    ldr("BC1 level 5, RDO lambda 1", &|p| (p.format, p.bc1_quality_level, p.rdo_lambda) = (BlockFormat::Bc1, 5, 1.0));

    for (name, settings) in [("BC6H, veryfast", Bc6hEncSettings::veryfast()), ("BC6H, basic", Bc6hEncSettings::basic()), ("BC6H, veryslow", Bc6hEncSettings::veryslow())] {
        let one = time(|| {
            texture::compress_hdr(&hdr, &settings, false, 1).expect("a texture");
        });
        let all = time(|| {
            texture::compress_hdr(&hdr, &settings, false, 0).expect("a texture");
        });
        report(name, one, all);
    }
}
