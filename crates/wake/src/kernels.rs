//! int8 kernels following the TFLite reference arithmetic (spec 0007 REQ-WAK-001).
//!
//! Accumulators are `i32` with wrapping arithmetic, as in the reference; requantisation is the fixed-point
//! "rounding doubling high multiply" followed by a rounding shift. Shapes and sizes are validated when the model
//! is planned, so the loops here index within their slices; every index is still computed with checked bounds
//! by the slice accessors, and a model that slips through validation can at worst abort the process under the
//! release profile, never read outside the arena.

use crate::model::Padding;

/// A multiplier and shift pair: `real ~= multiplier * 2^(shift - 31)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requant {
    /// Q0.31 multiplier.
    pub multiplier: i32,
    /// Power-of-two exponent; positive shifts left, negative right.
    pub shift: i32,
}

/// Split a positive real multiplier into a Q0.31 multiplier and a shift (TFLite `QuantizeMultiplier`).
#[must_use]
pub fn quantize_multiplier(real: f64) -> Requant {
    if real == 0.0 || !real.is_finite() {
        return Requant {
            multiplier: 0,
            shift: 0,
        };
    }
    // frexp: real = significand * 2^exponent with significand in [0.5, 1).
    let bits = real.to_bits();
    let mut exponent = ((bits >> 52) & 0x7ff) as i32 - 1022;
    let significand = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    if (bits >> 52) & 0x7ff == 0 {
        // Subnormal: scale up and retry.
        let scaled = real * f64::from_bits(((1023 + 100) as u64) << 52);
        let r = quantize_multiplier(scaled);
        return Requant {
            multiplier: r.multiplier,
            shift: r.shift - 100,
        };
    }
    let mut q = (significand * (1u64 << 31) as f64).round() as i64;
    if q == 1i64 << 31 {
        q /= 2;
        exponent += 1;
    }
    if exponent < -31 {
        return Requant {
            multiplier: 0,
            shift: 0,
        };
    }
    Requant {
        multiplier: q as i32,
        shift: exponent,
    }
}

/// `SaturatingRoundingDoublingHighMul`.
#[must_use]
pub fn srdhm(a: i32, b: i32) -> i32 {
    if a == i32::MIN && b == i32::MIN {
        return i32::MAX;
    }
    let ab = i64::from(a) * i64::from(b);
    let nudge: i64 = if ab >= 0 { 1 << 30 } else { 1 - (1 << 30) };
    ((ab + nudge) / (1i64 << 31)) as i32
}

/// `RoundingDivideByPOT`: divide by `2^exponent`, rounding half away from zero.
#[must_use]
pub fn rounding_shift(x: i32, exponent: i32) -> i32 {
    if exponent <= 0 {
        return x;
    }
    let exponent = exponent.min(31) as u32;
    let mask = ((1i64 << exponent) - 1) as i32;
    let remainder = x & mask;
    let threshold = (mask >> 1) + i32::from(x < 0);
    (x >> exponent) + i32::from(remainder > threshold)
}

/// The single-rounding form of the same product: `(x * multiplier + 2^(total - 1)) >> total`, with `total = 31 -
/// shift`. It differs from [`multiply_by_quantized`] by one unit in rare cases (the two rounding steps of the
/// reference form can round a value just under a half upward).
#[must_use]
pub fn multiply_single_rounding(x: i32, r: Requant) -> i32 {
    let total = (31 - r.shift).clamp(1, 62);
    let round = 1i64 << (total - 1);
    ((i64::from(x) * i64::from(r.multiplier) + round) >> total) as i32
}

/// How the accumulators of convolutions and fully connected layers are rounded to the output scale.
///
/// The TFLite reference kernels (and TFLite Micro, which the models were published for) round twice: once in the
/// fixed-point multiply and once in the shift. The matrix-multiply path of the TFLite C library on x86 (ruy) rounds
/// once, so its conv and fully connected outputs differ by one unit in about one value in a thousand; its
/// depthwise kernel uses the reference form. Both are correct to within a unit; [`Rounding::GemmSingle`] reproduces
/// that library tensor for tensor and is what the validation against `pymicro-wakeword` uses. Depthwise layers
/// always use the reference form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rounding {
    /// Double rounding everywhere, as the TFLite reference kernels.
    #[default]
    Reference,
    /// Single rounding for `CONV_2D` and `FULLY_CONNECTED`, double for `DEPTHWISE_CONV_2D`.
    GemmSingle,
}

/// `MultiplyByQuantizedMultiplier`.
#[must_use]
pub fn multiply_by_quantized(x: i32, r: Requant) -> i32 {
    let left = r.shift.max(0);
    let right = (-r.shift).max(0);
    let shifted = if left > 0 {
        x.wrapping_shl(left as u32)
    } else {
        x
    };
    rounding_shift(srdhm(shifted, r.multiplier), right)
}

/// Geometry of a 2-D convolution over NHWC data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvGeometry {
    /// Batch.
    pub batch: usize,
    /// Input height, width, channels.
    pub in_h: usize,
    /// Input width.
    pub in_w: usize,
    /// Input channels.
    pub in_c: usize,
    /// Output height.
    pub out_h: usize,
    /// Output width.
    pub out_w: usize,
    /// Output channels.
    pub out_c: usize,
    /// Kernel height.
    pub k_h: usize,
    /// Kernel width.
    pub k_w: usize,
    /// Stride along height.
    pub stride_h: usize,
    /// Stride along width.
    pub stride_w: usize,
    /// Dilation along height.
    pub dil_h: usize,
    /// Dilation along width.
    pub dil_w: usize,
    /// Zero rows added before the first input row.
    pub pad_h: usize,
    /// Zero columns added before the first input column.
    pub pad_w: usize,
    /// Depth multiplier (depthwise; 1 otherwise).
    pub depth_mult: usize,
}

/// Output size and leading padding for one axis.
#[must_use]
pub fn out_size(
    input: usize,
    kernel: usize,
    stride: usize,
    dilation: usize,
    padding: Padding,
) -> (usize, usize) {
    let eff = (kernel - 1) * dilation + 1;
    match padding {
        Padding::Same => {
            let out = input.div_ceil(stride);
            let total = ((out.saturating_sub(1)) * stride + eff).saturating_sub(input);
            (out, total / 2)
        }
        Padding::Valid => {
            let out = if input >= eff {
                (input - eff) / stride + 1
            } else {
                0
            };
            (out, 0)
        }
    }
}

/// Quantisation constants of a convolution-like operator.
#[derive(Debug, Clone)]
pub struct QuantParams {
    /// Input zero point.
    pub input_zp: i32,
    /// Output zero point.
    pub output_zp: i32,
    /// Output clamp.
    pub act_min: i32,
    /// Output clamp.
    pub act_max: i32,
    /// One entry per output channel (or a single entry for per-tensor quantisation).
    pub requant: Vec<Requant>,
    /// Rounding of the accumulator (convolution and fully connected layers).
    pub rounding: Rounding,
}

impl QuantParams {
    fn requant_for(&self, c: usize) -> Requant {
        self.requant
            .get(c)
            .or_else(|| self.requant.first())
            .copied()
            .unwrap_or(Requant {
                multiplier: 0,
                shift: 0,
            })
    }

    /// Scale an accumulator of a convolution or fully connected layer to the output scale (before the zero point).
    fn scale_gemm(&self, acc: i32, channel: usize) -> i32 {
        let r = self.requant_for(channel);
        match self.rounding {
            Rounding::Reference => multiply_by_quantized(acc, r),
            Rounding::GemmSingle => multiply_single_rounding(acc, r),
        }
    }
}

fn bias_at(bias: &[u8], i: usize) -> i32 {
    match bias.get(i * 4..i * 4 + 4) {
        Some(b) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}

fn clamp_i8(v: i32, lo: i32, hi: i32) -> i8 {
    v.clamp(lo, hi) as i8
}

/// `CONV_2D`: filter is `[out_c, k_h, k_w, in_c]`, int8 with zero point 0.
pub fn conv2d(
    g: &ConvGeometry,
    q: &QuantParams,
    input: &[i8],
    filter: &[u8],
    bias: &[u8],
    out: &mut [i8],
) {
    for b in 0..g.batch {
        for oy in 0..g.out_h {
            for ox in 0..g.out_w {
                for oc in 0..g.out_c {
                    let mut acc: i32 = bias_at(bias, oc);
                    for ky in 0..g.k_h {
                        let iy = (oy * g.stride_h + ky * g.dil_h) as isize - g.pad_h as isize;
                        if iy < 0 || iy >= g.in_h as isize {
                            continue;
                        }
                        for kx in 0..g.k_w {
                            let ix = (ox * g.stride_w + kx * g.dil_w) as isize - g.pad_w as isize;
                            if ix < 0 || ix >= g.in_w as isize {
                                continue;
                            }
                            let in_base =
                                ((b * g.in_h + iy as usize) * g.in_w + ix as usize) * g.in_c;
                            let f_base = ((oc * g.k_h + ky) * g.k_w + kx) * g.in_c;
                            let (Some(inp), Some(fil)) = (
                                input.get(in_base..in_base + g.in_c),
                                filter.get(f_base..f_base + g.in_c),
                            ) else {
                                continue;
                            };
                            for (&i, &f) in inp.iter().zip(fil.iter()) {
                                acc = acc.wrapping_add(
                                    (i32::from(i) - q.input_zp).wrapping_mul(i32::from(f as i8)),
                                );
                            }
                        }
                    }
                    let v = q.scale_gemm(acc, oc).wrapping_add(q.output_zp);
                    let idx = ((b * g.out_h + oy) * g.out_w + ox) * g.out_c + oc;
                    if let Some(o) = out.get_mut(idx) {
                        *o = clamp_i8(v, q.act_min, q.act_max);
                    }
                }
            }
        }
    }
}

/// `DEPTHWISE_CONV_2D`: filter is `[1, k_h, k_w, in_c * depth_mult]`.
pub fn depthwise_conv2d(
    g: &ConvGeometry,
    q: &QuantParams,
    input: &[i8],
    filter: &[u8],
    bias: &[u8],
    out: &mut [i8],
) {
    for b in 0..g.batch {
        for oy in 0..g.out_h {
            for ox in 0..g.out_w {
                for ic in 0..g.in_c {
                    for m in 0..g.depth_mult {
                        let oc = ic * g.depth_mult + m;
                        let mut acc: i32 = bias_at(bias, oc);
                        for ky in 0..g.k_h {
                            let iy = (oy * g.stride_h + ky * g.dil_h) as isize - g.pad_h as isize;
                            if iy < 0 || iy >= g.in_h as isize {
                                continue;
                            }
                            for kx in 0..g.k_w {
                                let ix =
                                    (ox * g.stride_w + kx * g.dil_w) as isize - g.pad_w as isize;
                                if ix < 0 || ix >= g.in_w as isize {
                                    continue;
                                }
                                let i = input
                                    .get(
                                        ((b * g.in_h + iy as usize) * g.in_w + ix as usize)
                                            * g.in_c
                                            + ic,
                                    )
                                    .copied()
                                    .unwrap_or(0);
                                let f = filter
                                    .get((ky * g.k_w + kx) * g.out_c + oc)
                                    .copied()
                                    .unwrap_or(0);
                                acc = acc.wrapping_add(
                                    (i32::from(i) - q.input_zp).wrapping_mul(i32::from(f as i8)),
                                );
                            }
                        }
                        let v =
                            multiply_by_quantized(acc, q.requant_for(oc)).wrapping_add(q.output_zp);
                        let idx = ((b * g.out_h + oy) * g.out_w + ox) * g.out_c + oc;
                        if let Some(o) = out.get_mut(idx) {
                            *o = clamp_i8(v, q.act_min, q.act_max);
                        }
                    }
                }
            }
        }
    }
}

/// `FULLY_CONNECTED`: weights `[out, depth]`, input `[batches * depth]`.
#[allow(clippy::too_many_arguments)]
pub fn fully_connected(
    batches: usize,
    depth: usize,
    out_c: usize,
    q: &QuantParams,
    input: &[i8],
    weights: &[u8],
    bias: &[u8],
    out: &mut [i8],
) {
    for b in 0..batches {
        let Some(inp) = input.get(b * depth..(b + 1) * depth) else {
            continue;
        };
        for oc in 0..out_c {
            let Some(w) = weights.get(oc * depth..(oc + 1) * depth) else {
                continue;
            };
            let mut acc: i32 = bias_at(bias, oc);
            for (&i, &f) in inp.iter().zip(w.iter()) {
                acc =
                    acc.wrapping_add((i32::from(i) - q.input_zp).wrapping_mul(i32::from(f as i8)));
            }
            let v = q.scale_gemm(acc, oc).wrapping_add(q.output_zp);
            if let Some(o) = out.get_mut(b * out_c + oc) {
                *o = clamp_i8(v, q.act_min, q.act_max);
            }
        }
    }
}

/// A 256-entry table for the int8 logistic: index is `q + 128`.
///
/// Built with the TFLite rules: inputs at or beyond the saturation radius give the extremes, the rest are
/// `round(256 * sigmoid(x)) - 128`. The reference computes the sigmoid in 32-bit fixed point; its error
/// (about 2^-28) can differ from this table only for an input within that distance of a rounding boundary of
/// the 1/256 output grid.
#[must_use]
pub fn logistic_table(input_scale: f32, input_zp: i32) -> [i8; 256] {
    let mut t = [0i8; 256];
    let multiplier = f64::from(input_scale) * f64::from(1u32 << 27);
    let r = quantize_multiplier(multiplier);
    let radius = ((15i64 * (1i64 << 27)) >> r.shift.clamp(0, 62)) as i32;
    for (i, e) in t.iter_mut().enumerate() {
        let q = i as i32 - 128;
        let x = q - input_zp;
        *e = if x <= -radius {
            -128
        } else if x >= radius {
            127
        } else {
            let real = f64::from(x) * f64::from(input_scale);
            let s = 1.0 / (1.0 + (-real).exp());
            ((s * 256.0 + 0.5).floor() as i32 - 128).clamp(-128, 127) as i8
        };
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_multiplier_matches_known_values() {
        assert_eq!(
            quantize_multiplier(1.0),
            Requant {
                multiplier: 1 << 30,
                shift: 1
            }
        );
        assert_eq!(
            quantize_multiplier(0.5),
            Requant {
                multiplier: 1 << 30,
                shift: 0
            }
        );
        assert_eq!(
            quantize_multiplier(0.25),
            Requant {
                multiplier: 1 << 30,
                shift: -1
            }
        );
        let r = quantize_multiplier(0.0123);
        let back = f64::from(r.multiplier) / f64::from(1u32 << 31) * 2f64.powi(r.shift);
        assert!((back - 0.0123).abs() < 1e-9);
        assert_eq!(
            quantize_multiplier(0.0),
            Requant {
                multiplier: 0,
                shift: 0
            }
        );
    }

    #[test]
    fn rounding_matches_gemmlowp_on_the_edges() {
        assert_eq!(rounding_shift(5, 1), 3); // 2.5 -> 3
        assert_eq!(rounding_shift(-5, 1), -3); // -2.5 -> -3 (away from zero)
        assert_eq!(rounding_shift(4, 1), 2);
        assert_eq!(srdhm(i32::MIN, i32::MIN), i32::MAX);
        assert_eq!(srdhm(1 << 30, 1 << 30), 1 << 29);
        assert_eq!(multiply_by_quantized(100, quantize_multiplier(1.0)), 100);
        assert_eq!(multiply_by_quantized(-100, quantize_multiplier(0.5)), -50);
    }

    #[test]
    fn same_and_valid_padding_sizes() {
        assert_eq!(out_size(5, 5, 3, 1, Padding::Valid), (1, 0));
        assert_eq!(out_size(1, 1, 1, 1, Padding::Same), (1, 0));
        assert_eq!(out_size(10, 3, 1, 1, Padding::Same), (10, 1));
        assert_eq!(out_size(2, 5, 1, 1, Padding::Valid), (0, 0));
    }

    #[test]
    fn a_1x1_convolution_is_a_matrix_product() {
        let g = ConvGeometry {
            batch: 1,
            in_h: 1,
            in_w: 1,
            in_c: 2,
            out_h: 1,
            out_w: 1,
            out_c: 2,
            k_h: 1,
            k_w: 1,
            stride_h: 1,
            stride_w: 1,
            dil_h: 1,
            dil_w: 1,
            pad_h: 0,
            pad_w: 0,
            depth_mult: 1,
        };
        let q = QuantParams {
            input_zp: 0,
            output_zp: 0,
            act_min: -128,
            act_max: 127,
            requant: vec![quantize_multiplier(1.0)],
            rounding: Rounding::Reference,
        };
        let mut out = [0i8; 2];
        conv2d(
            &g,
            &q,
            &[3, -4],
            &[1u8, 2, 0xffu8, 1],
            &[10, 0, 0, 0, 0, 0, 0, 0],
            &mut out,
        );
        assert_eq!(out, [3 - 8 + 10, -3 - 4]);
    }

    #[test]
    fn the_logistic_table_is_monotonic_and_saturates() {
        let t = logistic_table(0.0947, 36);
        assert!(t.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(t[0], -128);
        assert_eq!(t[255], 127);
        // sigma(0) = 0.5 -> 128 - 128 = 0 at the zero point.
        assert_eq!(t[(36 + 128) as usize], 0);
    }
}
