//! The lossy DCT coding of DWAA and DWAB: `LossyDctEncoder` (`internal_dwa_encoder.h`) and
//! `LossyDctDecoder` (`internal_dwa_decoder.h`) of OpenEXRCore.

use super::simd::{
    convert_float_to_half64, csc709_forward64, csc709_inverse, csc709_inverse64, dct_forward8x8, dct_inverse8x8, dct_inverse8x8_dc_only, float_to_half, from_half_zig_zag, half_to_float, INV_REMAP,
};
use super::{to_linear, to_nonlinear, Code, CORRUPT_CHUNK, PIXEL_FLOAT};

// ---------------------------------------------------------------------------------------
// Quantization.

#[rustfmt::skip]
const JPEG_QUANT_TABLE_Y: [i32; 64] = [
    16, 11, 10, 16, 24,  40,  51,  61,  12, 12, 14, 19, 26,  58,  60,  55,
    14, 13, 16, 24, 40,  57,  69,  56,  14, 17, 22, 29, 51,  87,  80,  62,
    18, 22, 37, 56, 68,  109, 103, 77,  24, 35, 55, 64, 81,  104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99];
const JPEG_QUANT_TABLE_Y_MIN: i32 = 10;

#[rustfmt::skip]
const JPEG_QUANT_TABLE_CBCR: [i32; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99];
const JPEG_QUANT_TABLE_CBCR_MIN: i32 = 17;

/// `countSetBits`, which takes a `uint16_t`.
fn count_set_bits(x: u32) -> i32 {
    (x as u16).count_ones() as i32
}

/// `countLeadingZeros`, which takes a `uint16_t` (and is never given 0).
fn count_leading_zeros(x: u32) -> u32 {
    u32::from(x as u16).leading_zeros()
}

/// The search of `TEST_QUANT_ALTERNATE_SMALL` and `_LARGE`.
struct Search {
    smallest: u32,
    smallbits: i32,
    smalldelta: f32,
    err_tol: f32,
    src_float: f32,
}

impl Search {
    fn new(abssrc: u32, err_tol: f32, src_float: f32) -> Search {
        Search { smallest: abssrc, smallbits: count_set_bits(abssrc), smalldelta: err_tol, err_tol, src_float }
    }

    fn test(&mut self, alt: u32, large: bool) {
        let bits = count_set_bits(alt);
        let delta = || if large { half_to_float(alt as u16) - self.src_float } else { self.src_float - half_to_float(alt as u16) };
        if bits < self.smallbits {
            let delta = delta();
            if delta < self.err_tol {
                self.smallbits = bits;
                self.smalldelta = delta;
                self.smallest = alt;
            }
        } else if bits == self.smallbits {
            let delta = delta();
            if delta < self.smalldelta {
                self.smallest = alt;
                self.smalldelta = delta;
                self.smallbits = bits;
            }
        }
    }

    fn small(&mut self, alt: u32) {
        self.test(alt, false);
    }

    fn large(&mut self, alt: u32) {
        self.test(alt, true);
    }
}

fn handle_quantize_denorm_tol(abssrc: u32, tol_sig: u32, err_tol: f32, src_float: f32) -> u32 {
    let tsigshift = 32 - count_leading_zeros(tol_sig);
    let npow2 = 1u32 << tsigshift;
    let lowermask = npow2 - 1;
    let mask = !lowermask;
    let mask2 = mask ^ npow2;

    let mut s = Search::new(abssrc, err_tol, src_float);
    s.small(abssrc & mask2);
    s.small(abssrc & mask);
    s.large((abssrc + npow2) & mask);
    s.large((abssrc + (npow2 << 1)) & mask);
    s.smallest
}

fn handle_quantize_generic(abssrc: u32, tol_sig: u32, err_tol: f32, src_float: f32) -> u32 {
    let tsigshift = 32 - count_leading_zeros(tol_sig);
    let npow2 = 1u32 << tsigshift;
    let lowermask = npow2 - 1;
    let mask = !lowermask;
    let mask2 = mask ^ npow2;
    let src_masked_val = abssrc & lowermask;
    let extrabit = u32::from(tol_sig > src_masked_val);
    let mask3 = mask2 ^ (((npow2 << 1) * extrabit) | ((npow2 >> 1) * u32::from(extrabit == 0)));

    let mut s = Search::new(abssrc, err_tol, src_float);
    if extrabit != 0 {
        s.small(abssrc & mask3);
        s.small(abssrc & mask2);
        s.small(abssrc & mask);
    } else if (abssrc & npow2) != 0 {
        s.small(abssrc & mask2);
        s.small(abssrc & mask3);
        s.small(abssrc & mask);
    } else {
        s.small(abssrc & mask2);
        s.small(abssrc & mask);
        s.small(abssrc & mask3);
    }
    s.large((abssrc + npow2) & mask);
    s.smallest
}

fn handle_quantize_equal_exp(abssrc: u32, tol_sig: u32, err_tol: f32, src_float: f32) -> u32 {
    let npow2 = 0x0800u32;
    let lowermask = npow2 - 1;
    let mask = !lowermask;
    let mask2 = mask ^ npow2;
    let src_masked_val = abssrc & lowermask;
    let extrabit = u32::from(tol_sig > src_masked_val);
    let mask3 = mask2 ^ (((npow2 << 1) * extrabit) | ((npow2 >> 1) * u32::from(extrabit == 0)));

    let mut s = Search::new(abssrc, err_tol, src_float);
    if src_masked_val == abssrc {
        s.small(abssrc & mask3);
    } else {
        let mut alt0 = abssrc & mask2;
        let alt1 = abssrc & mask;
        if alt0 == alt1 {
            alt0 = abssrc & mask3;
        }
        s.small(alt0);
        s.small(alt1);
    }
    s.large((abssrc + npow2) & mask);
    s.smallest
}

fn handle_quantize_close_exp(abssrc: u32, tol_sig: u32, err_tol: f32, src_float: f32) -> u32 {
    let npow2 = 0x0400u32;
    let lowermask = npow2 - 1;
    let mask = !lowermask;
    let mask2 = mask ^ npow2;
    let src_masked_val = abssrc & lowermask;
    let extrabit = u32::from(tol_sig > src_masked_val);
    let mask3 = mask2 ^ (((npow2 << 1) * extrabit) | ((npow2 >> 1) * u32::from(extrabit == 0)));

    let mut alternates = [0u32; 3];
    if (abssrc & npow2) == 0 {
        // by definition, src&mask2 == src&mask
        if extrabit != 0 {
            alternates[0] = abssrc & mask3;
            alternates[1] = abssrc & mask;
        } else {
            alternates[0] = abssrc & mask;
            alternates[1] = abssrc & mask3;
        }
    } else if extrabit != 0 {
        alternates[0] = abssrc & mask3;
        alternates[1] = abssrc & mask2;
        let alt1delta = src_float - half_to_float(alternates[1] as u16);
        if alt1delta >= err_tol {
            alternates[1] = abssrc & mask;
        }
    } else {
        alternates[0] = abssrc & mask2;
        alternates[1] = abssrc & mask3;
        let alt0delta = src_float - half_to_float(alternates[0] as u16);
        if alt0delta >= err_tol {
            alternates[0] = abssrc & mask;
        }
    }
    alternates[2] = (abssrc + npow2) & mask;

    let mut s = Search::new(abssrc, err_tol, src_float);
    s.small(alternates[0]);
    s.small(alternates[1]);
    s.large(alternates[2]);
    s.smallest
}

/// `handleQuantizeLargerSig` and `handleQuantizeSmallerSig`, which differ in the first
/// alternate's mask.
fn handle_quantize_larger_or_smaller_sig(abssrc: u32, npow2: u32, mask0: u32, mask: u32, err_tol: f32, src_float: f32) -> u32 {
    let alt0 = abssrc & mask0;
    let alt1 = (abssrc + npow2) & mask;
    let bits0 = count_set_bits(alt0);
    let bits1 = count_set_bits(alt1);

    if bits1 < bits0 {
        let delta = half_to_float(alt1 as u16) - src_float; // alt1 >= srcFloat
        if delta < err_tol {
            return alt1;
        }
        let delta = src_float - half_to_float(alt0 as u16); // alt0 <= srcFloat
        if delta < err_tol {
            return alt0;
        }
    } else if bits1 == bits0 {
        let delta = src_float - half_to_float(alt0 as u16);
        let delta1 = half_to_float(alt1 as u16) - src_float;
        if delta < err_tol {
            return if delta1 < delta { alt1 } else { alt0 };
        }
        if delta1 < err_tol {
            return alt1;
        }
    } else {
        let delta = src_float - half_to_float(alt0 as u16);
        if delta < err_tol {
            return alt0;
        }
        let srcbits = count_set_bits(abssrc);
        if bits1 < srcbits {
            let delta = half_to_float(alt1 as u16) - src_float;
            if delta < err_tol {
                return alt1;
            }
        }
    }
    abssrc
}

fn handle_quantize_equal_sig(abssrc: u32, npow2: u32, mask: u32, err_tol: f32, src_float: f32) -> u32 {
    let mut alt0 = abssrc & mask;
    let alt1 = (abssrc + npow2) & mask;

    let mut delta0 = src_float - half_to_float(alt0 as u16);
    if delta0 >= err_tol {
        let mask2 = mask ^ (npow2 | (npow2 >> 1));
        alt0 = abssrc & mask2;
        delta0 = src_float - half_to_float(alt0 as u16);

        if delta0 >= err_tol {
            let delta1 = half_to_float(alt1 as u16) - src_float;
            if delta1 < err_tol {
                let bits1 = count_set_bits(alt1);
                let srcbits = count_set_bits(abssrc);
                if bits1 < srcbits {
                    return alt1;
                }
            }
            return abssrc;
        }
    }

    let bits0 = count_set_bits(alt0);
    let bits1 = count_set_bits(alt1);
    if bits1 < bits0 {
        let delta1 = half_to_float(alt1 as u16) - src_float;
        if delta1 < err_tol {
            return alt1;
        }
    } else if bits1 == bits0 {
        let delta1 = half_to_float(alt1 as u16) - src_float;
        if delta1 < delta0 {
            return alt1;
        }
    }
    alt0
}

fn handle_quantize_default(abssrc: u32, tol_sig: u32, err_tol: f32, src_float: f32) -> u32 {
    let tsigshift = 32 - count_leading_zeros(tol_sig);
    let npow2 = 1u32 << tsigshift;
    let lowermask = npow2 - 1;
    let mask = !lowermask;
    let src_masked_val = abssrc & lowermask;

    if src_masked_val > tol_sig {
        let mask2 = mask ^ (npow2 | (npow2 >> 1));
        return handle_quantize_larger_or_smaller_sig(abssrc, npow2, mask2, mask, err_tol, src_float);
    } else if src_masked_val < tol_sig {
        return handle_quantize_larger_or_smaller_sig(abssrc, npow2, mask, mask, err_tol, src_float);
    }
    handle_quantize_equal_sig(abssrc, npow2, mask, err_tol, src_float)
}

/// `algoQuantize`: the half `src` (a DCT coefficient) as a half within `err_tol` of it with
/// as few bits set as can be found.
fn algo_quantize(src: u32, herr_tol: u32, err_tol: f32, src_float: f32) -> u16 {
    let sign = src & 0x8000;
    let abssrc = src & 0x7fff;
    let src_float = src_float.abs();
    let src_exp_biased = src & 0x7c00;
    let tol_exp_biased = herr_tol & 0x7c00;

    // if nan / inf, just bail and return src
    if src_exp_biased == 0x7c00 {
        return src as u16;
    }
    // can't possibly beat 0 bits
    if src_float < err_tol {
        return 0;
    }

    let exp_diff = src_exp_biased.wrapping_sub(tol_exp_biased) >> 10;
    // (A shift of 32 or more, by the instruction, is of the count's low five bits.)
    let mut tol_sig = ((herr_tol & 0x3ff) | (1 << 10)).wrapping_shr(exp_diff);

    if tol_exp_biased == 0 {
        if exp_diff == 0 || exp_diff == 1 {
            tol_sig = herr_tol & 0x3ff;
            if tol_sig == 0 {
                return src as u16;
            }
            return (sign | handle_quantize_generic(abssrc, tol_sig, err_tol, src_float)) as u16;
        }
        tol_sig = herr_tol & 0x3ff;
        if tol_sig == 0 {
            return src as u16;
        }
        tol_sig >>= exp_diff;
        if tol_sig == 0 {
            tol_sig = 1;
        }
        return (sign | handle_quantize_denorm_tol(abssrc, tol_sig, err_tol, src_float)) as u16;
    }

    if tol_sig == 0 {
        return src as u16;
    }

    if exp_diff > 1 || src_exp_biased == 0 {
        return (sign | handle_quantize_default(abssrc, tol_sig, err_tol, src_float)) as u16;
    }
    if exp_diff == 0 {
        return (sign | handle_quantize_equal_exp(abssrc, tol_sig, err_tol, src_float)) as u16;
    }
    (sign | handle_quantize_close_exp(abssrc, tol_sig, err_tol, src_float)) as u16
}

/// `quantizeCoeffAndZigXDR`.
fn quantize_coeff_and_zig(half_zig: &mut [u16; 64], dct: &[f32; 64], tolerances: &[f32; 64], halftols: &[u16; 64]) {
    for i in 0..64 {
        let src = float_to_half(dct[i]);
        let quantized = algo_quantize(u32::from(src), u32::from(halftols[i]), tolerances[i], half_to_float(src));
        half_zig[INV_REMAP[i]] = quantized;
    }
}

/// `LossyDctEncoder_rleAc`: the AC coefficients of a block, with runs of zeros as
/// `0xff00 | length` (`0xff00` for one to the end of the block).
fn rle_ac(block: &[u16; 64], out: &mut Vec<u16>) {
    let mut dct_comp = 1usize;
    while dct_comp < 64 {
        let mut run_len = 1usize;
        // If we don't have a 0, output verbatim
        if block[dct_comp] != 0 {
            out.push(block[dct_comp]);
            dct_comp += run_len;
            continue;
        }
        while dct_comp + run_len < 64 && block[dct_comp + run_len] == 0 {
            run_len += 1;
        }
        if run_len == 1 {
            out.push(block[dct_comp]);
        } else if run_len + dct_comp == 64 {
            out.push(0xff00);
        } else {
            out.push(0xff00 | run_len as u16);
        }
        dct_comp += run_len;
    }
}

// ---------------------------------------------------------------------------------------

/// A channel's lines in the block, for the encoder: where each starts, and their type.
pub(super) struct Source {
    pub rows: Vec<usize>,
    pub pixel_type: i32,
}

/// `LossyDctEncoder_execute`: one channel, or three (red, green and blue, converted to
/// Y'CbCr), of `raw` coded, their AC coefficients added to `packed_ac` and their DC ones to
/// `packed_dc`. `nonlinear` is whether the values go through `toNonlinear` first.
pub(super) fn encode(raw: &[u8], sources: &[Source], quant_base_error: f32, nonlinear: bool, width: usize, height: usize, packed_ac: &mut Vec<u16>, packed_dc: &mut Vec<u16>) {
    let quant_base_error = if quant_base_error < 0.0 { 0.0 } else { quant_base_error };
    let mut quant_table_y = [0f32; 64];
    let mut hquant_table_y = [0u16; 64];
    let mut quant_table_cbcr = [0f32; 64];
    let mut hquant_table_cbcr = [0u16; 64];
    for idx in 0..64 {
        quant_table_y[idx] = quant_base_error * JPEG_QUANT_TABLE_Y[idx] as f32 / JPEG_QUANT_TABLE_Y_MIN as f32;
        hquant_table_y[idx] = float_to_half(quant_table_y[idx]);
        quant_table_cbcr[idx] = quant_base_error * JPEG_QUANT_TABLE_CBCR[idx] as f32 / JPEG_QUANT_TABLE_CBCR_MIN as f32;
        hquant_table_cbcr[idx] = float_to_half(quant_table_cbcr[idx]);
    }

    let num_comp = sources.len();
    let num_blocks_x = ((width as f32) / 8.0).ceil() as usize;
    let num_blocks_y = ((height as f32) / 8.0).ceil() as usize;

    // The values as halves; floats clamped to the halves' range first.
    let halves: Vec<Vec<u16>> = sources
        .iter()
        .map(|source| {
            let mut halves = Vec::with_capacity(width * height);
            for &row in &source.rows {
                for x in 0..width {
                    if source.pixel_type == PIXEL_FLOAT {
                        let at = row + 4 * x;
                        // Clamp to half ranges, instead of just casting. (A NaN stays one.)
                        let src = f32::from_bits(u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])).clamp(-65504.0, 65504.0);
                        halves.push(float_to_half(src));
                    } else {
                        let at = row + 2 * x;
                        halves.push(u16::from_le_bytes([raw[at], raw[at + 1]]));
                    }
                }
            }
            halves
        })
        .collect();

    let mut dc: Vec<Vec<u16>> = vec![Vec::with_capacity(num_blocks_x * num_blocks_y); num_comp];
    let mut dct_data = vec![[0f32; 64]; num_comp];
    let mut half_zig = [0u16; 64];
    let (w, h) = (width as i32, height as i32);
    for blocky in 0..num_blocks_y {
        for blockx in 0..num_blocks_x {
            for chan in 0..num_comp {
                // Break the source into 8x8 blocks. If we don't fit at the edges, mirror.
                for y in 0..8 {
                    for x in 0..8 {
                        let mut vx = 8 * blockx as i32 + x;
                        let mut vy = 8 * blocky as i32 + y;
                        if vx >= w {
                            vx = w - (vx - (w - 1));
                        }
                        if vx < 0 {
                            vx = w - 1;
                        }
                        if vy >= h {
                            vy = h - (vy - (h - 1));
                        }
                        if vy < 0 {
                            vy = h - 1;
                        }
                        let mut value = halves[chan][vy as usize * width + vx as usize];
                        if nonlinear {
                            value = to_nonlinear(value);
                        }
                        dct_data[chan][(y * 8 + x) as usize] = half_to_float(value);
                    }
                }
            }

            if num_comp == 3 {
                csc709_forward64(&mut dct_data, [0, 1, 2]);
            }

            for chan in 0..num_comp {
                dct_forward8x8(&mut dct_data[chan]);
                if chan == 0 {
                    quantize_coeff_and_zig(&mut half_zig, &dct_data[chan], &quant_table_y, &hquant_table_y);
                } else {
                    quantize_coeff_and_zig(&mut half_zig, &dct_data[chan], &quant_table_cbcr, &hquant_table_cbcr);
                }
                dc[chan].push(half_zig[0]);
                rle_ac(&half_zig, packed_ac);
            }
        }
    }
    for plane in dc {
        packed_dc.extend_from_slice(&plane);
    }
}

// ---------------------------------------------------------------------------------------

/// A channel's lines in the block, for the decoder.
pub(super) struct Target {
    /// The channel's index (three targets may be the same channel).
    pub channel: usize,
    pub rows: Vec<usize>,
    pub pixel_type: i32,
}

/// `LossyDctDecoder`, with the per-channel blocks (`_dctData`, `_halfZigData`) of every
/// channel of the block.
pub(super) struct Decoder {
    dct: Vec<[f32; 64]>,
    half_zig: Vec<[u16; 64]>,
    /// How many AC and DC values the last decode took (`_packedAcCount`, `_packedDcCount`).
    pub ac_count: usize,
    pub dc_count: usize,
}

impl Decoder {
    pub(super) fn new(channels: usize) -> Decoder {
        Decoder { dct: vec![[0f32; 64]; channels], half_zig: vec![[0u16; 64]; channels], ac_count: 0, dc_count: 0 }
    }

    /// `LossyDctDecoder_unRleAc`: the AC values of a block from `packed_ac[*at..end]`, and
    /// the index of the last that is not zero.
    fn un_rle_ac(&mut self, channel: usize, packed_ac: &[u16], at: &mut usize, end: u64) -> Result<usize, Code> {
        let mut dct_comp = 1usize;
        let mut ac = *at;
        let mut lnz = 0usize;
        let mut ac_count = 0usize;
        while dct_comp < 64 {
            if ac as u64 >= end {
                return Err(CORRUPT_CHUNK);
            }
            // (Past the coefficients there are, which takes damaged counters and rules
            // together, the original reads on into whatever follows them.)
            let Some(&val) = packed_ac.get(ac) else { return Err(CORRUPT_CHUNK) };
            if (val & 0xff00) == 0xff00 {
                let count = usize::from(val & 0xff);
                dct_comp += if count == 0 { 64 } else { count };
            } else {
                lnz = dct_comp;
                self.half_zig[channel][dct_comp] = val;
                dct_comp += 1;
            }
            ac_count += 1;
            ac += 1;
        }
        self.ac_count += ac_count;
        *at = ac;
        Ok(lnz)
    }

    /// `LossyDctDecoder_execute`: one or three channels (red, green, blue, from Y'CbCr) into
    /// `out`, from the AC values `packed_ac[ac_at..]` (`rem_ac` of them left) and the DC
    /// ones `packed_dc[dc_at..]` (`rem_dc`). `linear` is whether the values go through
    /// `toLinear`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode(
        &mut self,
        out: &mut [u8],
        targets: &[Target],
        packed_ac: &[u16],
        ac_at: usize,
        rem_ac: u64,
        packed_dc: &[u16],
        dc_at: usize,
        rem_dc: u64,
        linear: bool,
        width: usize,
        height: usize,
    ) -> Result<(), Code> {
        self.ac_count = 0;
        self.dc_count = 0;
        let num_comp = targets.len();
        let num_blocks_x = width.div_ceil(8);
        let num_blocks_y = height.div_ceil(8);
        let leftover_x = width + 8 - num_blocks_x * 8;
        let leftover_y = height + 8 - num_blocks_y * 8;
        let num_full_blocks_x = width / 8;

        if rem_dc < (num_comp * num_blocks_x * num_blocks_y) as u64 {
            return Err(CORRUPT_CHUNK);
        }

        let block_row = num_blocks_x * 64;
        let mut row_block = vec![0u16; num_comp * block_row];
        let mut curr_dc: Vec<usize> = (0..num_comp).map(|comp| dc_at + comp * num_blocks_x * num_blocks_y).collect();
        let mut curr_ac = ac_at;
        let ac_end = ac_at as u64 + rem_ac;

        for blocky in 0..num_blocks_y {
            let mut max_x = 8;
            let max_y = if blocky == num_blocks_y - 1 { leftover_y } else { 8 };

            for blockx in 0..num_blocks_x {
                let mut block_is_constant = true;
                if blockx == num_blocks_x - 1 {
                    max_x = leftover_x;
                }

                for (comp, target) in targets.iter().enumerate() {
                    let ch = target.channel;
                    self.half_zig[ch] = [0; 64];
                    self.half_zig[ch][0] = packed_dc[curr_dc[comp]];
                    curr_dc[comp] += 1;
                    self.dc_count += 1;

                    let last_non_zero = self.un_rle_ac(ch, packed_ac, &mut curr_ac, ac_end)?;

                    if last_non_zero == 0 {
                        // DC only case - AC components are all 0
                        self.dct[ch][0] = half_to_float(self.half_zig[ch][0]);
                        dct_inverse8x8_dc_only(&mut self.dct[ch]);
                    } else {
                        block_is_constant = false;
                        let half_zig = self.half_zig[ch];
                        from_half_zig_zag(&half_zig, &mut self.dct[ch]);
                        // If lastNonZero is less than the first item on each row, the
                        // whole row is zero.
                        let zeroed_rows = match last_non_zero {
                            0..=1 => 7,
                            2 => 6,
                            3..=8 => 5,
                            9 => 4,
                            10..=19 => 3,
                            20 => 2,
                            21..=34 => 1,
                            _ => 0,
                        };
                        dct_inverse8x8(&mut self.dct[ch], zeroed_rows);
                    }
                }

                if num_comp == 3 {
                    let idx = [targets[0].channel, targets[1].channel, targets[2].channel];
                    if !block_is_constant {
                        csc709_inverse64(&mut self.dct, idx);
                    } else {
                        csc709_inverse(&mut self.dct, idx);
                    }
                }

                for (comp, target) in targets.iter().enumerate() {
                    let dst = &mut row_block[comp * block_row + blockx * 64..comp * block_row + blockx * 64 + 64];
                    if !block_is_constant {
                        convert_float_to_half64(dst, &self.dct[target.channel]);
                    } else {
                        dst.fill(float_to_half(self.dct[target.channel][0]));
                    }
                }
            }

            // The halves blocked in rowBlock, back to linear, into the lines.
            for (comp, target) in targets.iter().enumerate() {
                let convert = |v: u16| if linear { to_linear(v) } else { v };
                for y in 8 * blocky..8 * blocky + max_y {
                    let row = target.rows[y];
                    for blockx in 0..num_full_blocks_x {
                        let src = comp * block_row + blockx * 64 + (y & 7) * 8;
                        for x in 0..8 {
                            let at = row + 2 * (8 * blockx + x);
                            out[at..at + 2].copy_from_slice(&convert(row_block[src + x]).to_le_bytes());
                        }
                    }
                    if num_full_blocks_x != num_blocks_x {
                        let src = comp * block_row + num_full_blocks_x * 64 + (y & 7) * 8;
                        for x in 0..max_x {
                            let at = row + 2 * (8 * num_full_blocks_x + x);
                            out[at..at + 2].copy_from_slice(&convert(row_block[src + x]).to_le_bytes());
                        }
                    }
                }
            }
        }

        // The channels of floats, from halves, in place (from the end of each line).
        for target in targets {
            if target.pixel_type != PIXEL_FLOAT {
                continue;
            }
            for &row in &target.rows[..height] {
                for x in (0..width).rev() {
                    let h = u16::from_le_bytes([out[row + 2 * x], out[row + 2 * x + 1]]);
                    let f = half_to_float(h);
                    out[row + 4 * x..row + 4 * x + 4].copy_from_slice(&f.to_bits().to_le_bytes());
                }
            }
        }
        Ok(())
    }
}
