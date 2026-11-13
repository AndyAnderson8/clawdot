//! The audio frontend of the microWakeWord models: 16 kHz S16 PCM in, 40 log-mel features every 10 ms out
//! (spec 0007 REQ-WAK-004).
//!
//! This is the algorithm of the TensorFlow Lite Micro microfrontend (Apache-2.0, The TensorFlow Authors),
//! including its 16-bit fixed-point FFT, which is the kiss FFT (BSD-3-Clause, Mark Borgerding) built for
//! `FIXED_POINT=16`. The work done for every frame is integer arithmetic, so the output can be compared value
//! for value with the reference. The tables (twiddle factors, mel filterbank weights, the gain lookup) are built
//! once at start-up with floating point, as the reference builds them. Written from the published algorithm
//! and checked against the reference output.
//!
//! Stages per 30 ms window (every 10 ms): Hann window, scale up to use the 16-bit range, real FFT of 512
//! points, power spectrum, triangular mel filterbank (125 to 7500 Hz), square root, noise reduction,
//! per-channel gain control (PCAN), natural log scaled by 64.

/// Number of mel channels (and features per frame).
pub const NUM_CHANNELS: usize = 40;
/// Samples per feature step (10 ms).
pub const STEP: usize = 160;
const WINDOW: usize = 480;
const FFT_SIZE: usize = 512;
const NCFFT: usize = FFT_SIZE / 2;
const SPECTRUM: usize = FFT_SIZE / 2 + 1;
const SAMPLE_RATE: f32 = 16_000.0;

const WINDOW_BITS: u32 = 12;
const FILTERBANK_BITS: u32 = 12;
const NOISE_BITS: u32 = 14;
const SMOOTHING_BITS: u32 = 10;
const PCAN_SNR_BITS: u32 = 12;
const PCAN_OUTPUT_BITS: u32 = 6;
const PCAN_GAIN_BITS: u32 = 21;
/// Right shift of `signal * gain` to the SNR scale. The TFLite Micro source has `gain_bits - PCAN_SNR_BITS` (9);
/// the reference output of the library microWakeWord uses is reproduced with 6, which is what this value is
/// fixed to (found by comparison with the reference, see docs/architecture/wake-word-runtime.md).
const PCAN_SNR_SHIFT: u32 = PCAN_GAIN_BITS - PCAN_SNR_BITS - 3;
/// Bits added to the PCAN output before the logarithm, and removed from the smoothed noise estimate's scale.
const CORRECTION_BITS: u32 = 3;
const LOG_SCALE_SHIFT: u32 = 6;
const LOG_SEGMENTS_LOG2: u32 = 7;
const LOG_SCALE_LOG2: u32 = 16;
const LOG_COEFF: u64 = 45_426;

/// Number of bits needed to represent `x` (0 for 0): the `MostSignificantBit32` of the reference.
fn msb32(x: u32) -> u32 {
    32 - x.leading_zeros()
}

fn msb64(x: u64) -> u32 {
    64 - x.leading_zeros()
}

// ---------------------------------------------------------------------------------------------------------
// 16-bit fixed-point FFT (kiss FFT, FIXED_POINT=16)
// ---------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct Cpx {
    r: i16,
    i: i16,
}

fn sround(x: i32) -> i16 {
    (x.wrapping_add(1 << 14) >> 15) as i16
}

fn smul(a: i16, b: i16) -> i32 {
    i32::from(a) * i32::from(b)
}

fn fixdiv(x: i16, k: i32) -> i16 {
    sround(smul(x, (32767 / k) as i16))
}

fn cfixdiv(c: Cpx, k: i32) -> Cpx {
    Cpx {
        r: fixdiv(c.r, k),
        i: fixdiv(c.i, k),
    }
}

fn cmul(a: Cpx, b: Cpx) -> Cpx {
    Cpx {
        r: sround(smul(a.r, b.r).wrapping_sub(smul(a.i, b.i))),
        i: sround(smul(a.r, b.i).wrapping_add(smul(a.i, b.r))),
    }
}

fn cadd(a: Cpx, b: Cpx) -> Cpx {
    Cpx {
        r: a.r.wrapping_add(b.r),
        i: a.i.wrapping_add(b.i),
    }
}

fn csub(a: Cpx, b: Cpx) -> Cpx {
    Cpx {
        r: a.r.wrapping_sub(b.r),
        i: a.i.wrapping_sub(b.i),
    }
}

fn cexp(phase: f64) -> Cpx {
    Cpx {
        r: (0.5 + 32767.0 * phase.cos()).floor() as i16,
        i: (0.5 + 32767.0 * phase.sin()).floor() as i16,
    }
}

/// Real FFT of 512 points as a complex FFT of 256 points (four radix-4 stages) plus the real-signal split.
#[derive(Debug, Clone)]
struct Fft {
    twiddles: Vec<Cpx>,
    super_twiddles: Vec<Cpx>,
    tmp: Vec<Cpx>,
}

impl Fft {
    fn new() -> Self {
        let pi = std::f64::consts::PI;
        let twiddles = (0..NCFFT)
            .map(|i| cexp(-2.0 * pi * i as f64 / NCFFT as f64))
            .collect();
        let super_twiddles = (0..NCFFT / 2)
            .map(|i| cexp(-pi * ((i + 1) as f64 / NCFFT as f64 + 0.5)))
            .collect();
        Self {
            twiddles,
            super_twiddles,
            tmp: vec![Cpx::default(); NCFFT],
        }
    }

    /// Radix-4 butterfly over `m` columns (the kiss FFT `kf_bfly4`, forward).
    fn bfly4(fout: &mut [Cpx], fstride: usize, tw: &[Cpx], m: usize) {
        for k in 0..m {
            let t1 = tw.get(k * fstride).copied().unwrap_or_default();
            let t2 = tw.get(2 * k * fstride).copied().unwrap_or_default();
            let t3 = tw.get(3 * k * fstride).copied().unwrap_or_default();
            let a = cfixdiv(fout[k], 4);
            let b = cfixdiv(fout[k + m], 4);
            let c = cfixdiv(fout[k + 2 * m], 4);
            let d = cfixdiv(fout[k + 3 * m], 4);
            let s0 = cmul(b, t1);
            let s1 = cmul(c, t2);
            let s2 = cmul(d, t3);
            let s5 = csub(a, s1);
            let a = cadd(a, s1);
            let s3 = cadd(s0, s2);
            let s4 = csub(s0, s2);
            fout[k + 2 * m] = csub(a, s3);
            fout[k] = cadd(a, s3);
            fout[k + m] = Cpx {
                r: s5.r.wrapping_add(s4.i),
                i: s5.i.wrapping_sub(s4.r),
            };
            fout[k + 3 * m] = Cpx {
                r: s5.r.wrapping_sub(s4.i),
                i: s5.i.wrapping_add(s4.r),
            };
        }
    }

    /// Decimation-in-time recursion over the factor list `(radix, remaining length)` (kiss `kf_work`).
    fn work(fout: &mut [Cpx], f: &[Cpx], fstride: usize, factors: &[(usize, usize)], tw: &[Cpx]) {
        let Some(&(p, m)) = factors.first() else {
            return;
        };
        if m == 1 {
            for (k, o) in fout.iter_mut().take(p).enumerate() {
                *o = f.get(k * fstride).copied().unwrap_or_default();
            }
        } else {
            for k in 0..p {
                let part = &mut fout[k * m..(k + 1) * m];
                let src = f.get(k * fstride..).unwrap_or_default();
                Self::work(part, src, fstride * p, &factors[1..], tw);
            }
        }
        Self::bfly4(fout, fstride, tw, m);
    }

    /// `kiss_fftr`: `input` has 512 real samples, `out` gets 257 bins.
    fn real(&mut self, input: &[i16; FFT_SIZE], out: &mut [Cpx; SPECTRUM]) {
        let mut cin = [Cpx::default(); NCFFT];
        for (k, c) in cin.iter_mut().enumerate() {
            *c = Cpx {
                r: input[2 * k],
                i: input[2 * k + 1],
            };
        }
        const FACTORS: [(usize, usize); 4] = [(4, 64), (4, 16), (4, 4), (4, 1)];
        Self::work(&mut self.tmp, &cin, 1, &FACTORS, &self.twiddles);

        let tdc = cfixdiv(self.tmp[0], 2);
        out[0] = Cpx {
            r: tdc.r.wrapping_add(tdc.i),
            i: 0,
        };
        out[NCFFT] = Cpx {
            r: tdc.r.wrapping_sub(tdc.i),
            i: 0,
        };
        for k in 1..=NCFFT / 2 {
            let fpk = cfixdiv(self.tmp[k], 2);
            let nk = self.tmp[NCFFT - k];
            let fpnk = cfixdiv(
                Cpx {
                    r: nk.r,
                    i: nk.i.wrapping_neg(),
                },
                2,
            );
            let f1k = cadd(fpk, fpnk);
            let f2k = csub(fpk, fpnk);
            let tw = cmul(f2k, self.super_twiddles[k - 1]);
            out[k] = Cpx {
                r: ((i32::from(f1k.r) + i32::from(tw.r)) >> 1) as i16,
                i: ((i32::from(f1k.i) + i32::from(tw.i)) >> 1) as i16,
            };
            out[NCFFT - k] = Cpx {
                r: ((i32::from(f1k.r) - i32::from(tw.r)) >> 1) as i16,
                i: ((i32::from(tw.i) - i32::from(f1k.i)) >> 1) as i16,
            };
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Mel filterbank
// ---------------------------------------------------------------------------------------------------------

fn freq_to_mel(freq: f32) -> f32 {
    (1127.0_f64 * (f64::from(freq) / 700.0).ln_1p()) as f32
}

/// Integer square root with the reference's rounding: the floor, plus one when the remainder exceeds it.
fn sqrt_round(num: u64) -> u32 {
    if num == 0 {
        return 0;
    }
    let mut res: u64 = 0;
    let mut rem = num;
    let mut bit: u64 = 1 << ((msb64(num) - 1) & !1);
    while bit != 0 {
        if rem >= res + bit {
            rem -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }
    let small = num >> 32 == 0;
    if rem > res && !(small && res == 0xFFFF) {
        res += 1;
    }
    res as u32
}

#[derive(Debug, Clone)]
struct Filterbank {
    start_index: usize,
    end_index: usize,
    freq_starts: [usize; NUM_CHANNELS + 1],
    weight_starts: [usize; NUM_CHANNELS + 1],
    widths: [usize; NUM_CHANNELS + 1],
    weights: Vec<i16>,
    unweights: Vec<i16>,
}

impl Filterbank {
    fn new(lower: f32, upper: f32) -> Self {
        let n = NUM_CHANNELS;
        let mel_low = freq_to_mel(lower);
        let mel_hi = freq_to_mel(upper);
        let mel_spacing = (mel_hi - mel_low) / (n as f32 + 1.0);
        let center: Vec<f32> = (0..=n)
            .map(|i| mel_low + mel_spacing * (i as f32 + 1.0))
            .collect();
        let hz_per_sbin = (0.5 * f64::from(SAMPLE_RATE) / (SPECTRUM as f64 - 1.0)) as f32;
        let start_index = (1.5 + f64::from(lower / hz_per_sbin)) as usize;
        let end_index = (0.5 + f64::from(upper / hz_per_sbin)) as usize;

        // Which channel's right slope each bin belongs to (-2: unused).
        let mut mapper = vec![-2i32; SPECTRUM];
        let mut channel = 0usize;
        for (i, m) in mapper.iter_mut().enumerate() {
            let melf = freq_to_mel(i as f32 * hz_per_sbin);
            if i < start_index || i > end_index {
                *m = -2;
            } else {
                while channel < n && center[channel] < melf {
                    channel += 1;
                }
                *m = channel as i32 - 1;
            }
        }
        let mut fb = Self {
            start_index,
            end_index,
            freq_starts: [0; NUM_CHANNELS + 1],
            weight_starts: [0; NUM_CHANNELS + 1],
            widths: [0; NUM_CHANNELS + 1],
            weights: Vec::new(),
            unweights: Vec::new(),
        };
        for chan in 0..=n {
            let mut first = None;
            let mut width = 0usize;
            #[allow(clippy::needless_range_loop)]
            for i in start_index..=end_index {
                if mapper[i] == chan as i32 - 1 {
                    first.get_or_insert(i);
                    width += 1;
                }
            }
            fb.freq_starts[chan] = first.unwrap_or(0);
            fb.widths[chan] = width;
            fb.weight_starts[chan] = fb.weights.len();
            for j in 0..width {
                let i = fb.freq_starts[chan] + j;
                let melf = freq_to_mel(i as f32 * hz_per_sbin);
                let c = mapper[i];
                let w = if c >= 0 {
                    let c = c as usize;
                    (center[c + 1] - melf) / (center[c + 1] - center[c])
                } else {
                    (center[0] - melf) / (center[0] - mel_low)
                };
                let scale = (1u32 << FILTERBANK_BITS) as f32;
                fb.weights.push((f64::from(w * scale) + 0.5).floor() as i16);
                fb.unweights
                    .push((f64::from((1.0 - w) * scale) + 0.5).floor() as i16);
            }
        }
        fb
    }
}

// ---------------------------------------------------------------------------------------------------------
// PCAN gain lookup
// ---------------------------------------------------------------------------------------------------------

fn gain_lookup(input_bits: u32, x: u32) -> i16 {
    let x_as_float = x as f32 / (1u32 << input_bits) as f32;
    let gain = (1u32 << PCAN_GAIN_BITS) as f32 * (x_as_float + 80.0).powf(-0.95);
    if gain > 32767.0 {
        32767
    } else {
        (gain + 0.5) as i16
    }
}

/// The piecewise-quadratic table of `PcanGainControlPopulateState`, indexed as the reference does.
fn gain_lut(input_bits: u32) -> Vec<i16> {
    let mut lut = vec![0i16; 4 * 32 - 6 + 3];
    lut[0] = gain_lookup(input_bits, 0);
    lut[1] = gain_lookup(input_bits, 1);
    for interval in 2..=32usize {
        let x0 = 1u32 << (interval - 1);
        let x1 = x0 + (x0 >> 1);
        let x2 = if interval == 32 { u32::MAX } else { x0 + x0 };
        let y0 = gain_lookup(input_bits, x0);
        let y1 = gain_lookup(input_bits, x1);
        let y2 = gain_lookup(input_bits, x2);
        let diff1 = i32::from(y1) - i32::from(y0);
        let diff2 = i32::from(y2) - i32::from(y0);
        let a1 = 4 * diff1 - diff2;
        let a2 = diff2 - a1;
        let base = 4 * interval - 6;
        lut[base] = y0;
        lut[base + 1] = a1 as i16;
        lut[base + 2] = a2 as i16;
    }
    lut
}

fn wide_dynamic(x: u32, lut: &[i16]) -> i32 {
    if x <= 2 {
        return i32::from(lut[x as usize]);
    }
    let interval = msb32(x);
    let base = (4 * interval - 6) as usize;
    let frac = (if interval < 11 {
        x << (11 - interval)
    } else {
        x >> (interval - 11)
    } & 0x3FF) as i32;
    let mut result = (i32::from(lut[base + 2]) * frac) >> 5;
    result += i32::from(lut[base + 1]) << 5;
    result = result.wrapping_mul(frac);
    result = (result + (1 << 14)) >> 15;
    result += i32::from(lut[base]);
    i32::from(result as i16)
}

fn pcan_shrink(x: u32) -> u32 {
    if x < (2 << PCAN_SNR_BITS) {
        (x.wrapping_mul(x)) >> (2 + 2 * PCAN_SNR_BITS - PCAN_OUTPUT_BITS)
    } else {
        (x >> (PCAN_SNR_BITS - PCAN_OUTPUT_BITS)) - (1 << PCAN_OUTPUT_BITS)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Log scale
// ---------------------------------------------------------------------------------------------------------

fn log_lut() -> Vec<i32> {
    (0..=(1usize << LOG_SEGMENTS_LOG2))
        .map(|k| {
            let x = k as f64 / f64::from(1u32 << LOG_SEGMENTS_LOG2);
            (65536.0 * ((1.0 + x).log2() - x)).round() as i32
        })
        .collect()
}

fn log2_fraction(lut: &[i32], x: u32, log2x: u32) -> u32 {
    let mut frac = (i64::from(x) - (1i64 << log2x)) as i32;
    if log2x < LOG_SCALE_LOG2 {
        frac <<= LOG_SCALE_LOG2 - log2x;
    } else {
        frac >>= log2x - LOG_SCALE_LOG2;
    }
    let base_seg = (frac >> (LOG_SCALE_LOG2 - LOG_SEGMENTS_LOG2)) as usize;
    let seg_unit = (1i32 << LOG_SCALE_LOG2) >> LOG_SEGMENTS_LOG2;
    let c0 = lut.get(base_seg).copied().unwrap_or(0);
    let c1 = lut.get(base_seg + 1).copied().unwrap_or(c0);
    let seg_base = seg_unit * base_seg as i32;
    let rel_pos = ((c1 - c0) * (frac - seg_base)) >> LOG_SCALE_LOG2;
    (frac + c0 + rel_pos) as u32
}

fn log_scaled(lut: &[i32], x: u32) -> u32 {
    let integer = msb32(x) - 1;
    let fraction = log2_fraction(lut, x, integer);
    let log2 = (integer << LOG_SCALE_LOG2) + fraction;
    let round = 1u64 << (LOG_SCALE_LOG2 - 1);
    let loge = ((LOG_COEFF * u64::from(log2) + round) >> LOG_SCALE_LOG2) as u32;
    (((u64::from(loge) << LOG_SCALE_SHIFT) + round) >> LOG_SCALE_LOG2) as u32
}

// ---------------------------------------------------------------------------------------------------------
// The frontend
// ---------------------------------------------------------------------------------------------------------

/// Streaming frontend. Feed samples with [`Frontend::process`]; a feature frame is produced for every 160
/// samples once the first 480 have arrived.
#[derive(Debug, Clone)]
pub struct Frontend {
    coefficients: Vec<i16>,
    input: [i16; WINDOW],
    input_used: usize,
    fft: Fft,
    filterbank: Filterbank,
    lut_gain: Vec<i16>,
    lut_log: Vec<i32>,
    noise_estimate: [u32; NUM_CHANNELS],
    even_smoothing: u32,
    odd_smoothing: u32,
    min_signal_remaining: u32,
    samples: u64,
    // Scratch, kept to avoid allocation while processing.
    windowed: [i16; WINDOW],
    fft_in: [i16; FFT_SIZE],
    spectrum: [Cpx; SPECTRUM],
    energy: [i32; SPECTRUM],
}

impl Default for Frontend {
    fn default() -> Self {
        Self::new()
    }
}

impl Frontend {
    /// A frontend with the microWakeWord parameters.
    #[must_use]
    pub fn new() -> Self {
        let arg = std::f64::consts::PI * 2.0 / WINDOW as f64;
        let coefficients = (0..WINDOW)
            .map(|i| {
                ((0.5 - 0.5 * (arg * (i as f64 + 0.5)).cos()) * f64::from(1u32 << WINDOW_BITS)
                    + 0.5)
                    .floor() as i16
            })
            .collect();
        Self {
            coefficients,
            input: [0; WINDOW],
            input_used: 0,
            fft: Fft::new(),
            filterbank: Filterbank::new(125.0, 7500.0),
            lut_gain: gain_lut(SMOOTHING_BITS - CORRECTION_BITS),
            lut_log: log_lut(),
            noise_estimate: [0; NUM_CHANNELS],
            even_smoothing: (0.025_f32 * (1u32 << NOISE_BITS) as f32) as u32,
            odd_smoothing: (0.06_f32 * (1u32 << NOISE_BITS) as f32) as u32,
            min_signal_remaining: (0.05_f32 * (1u32 << NOISE_BITS) as f32) as u32,
            samples: 0,
            windowed: [0; WINDOW],
            fft_in: [0; FFT_SIZE],
            spectrum: [Cpx::default(); SPECTRUM],
            energy: [0; SPECTRUM],
        }
    }

    /// Forget all audio and the noise estimate.
    pub fn reset(&mut self) {
        self.input = [0; WINDOW];
        self.input_used = 0;
        self.noise_estimate = [0; NUM_CHANNELS];
        self.samples = 0;
    }

    /// Samples consumed since construction or the last [`Frontend::reset`].
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Feed audio of any length. `on_frame` is called with each completed feature frame and the stream offset
    /// (in samples) at which the frame's window ends.
    pub fn process(&mut self, pcm: &[i16], mut on_frame: impl FnMut(&[u16; NUM_CHANNELS], u64)) {
        for &s in pcm {
            self.input[self.input_used] = s;
            self.input_used += 1;
            self.samples += 1;
            if self.input_used == WINDOW {
                let frame = self.frame();
                on_frame(&frame, self.samples);
                self.input.copy_within(STEP.., 0);
                self.input_used = WINDOW - STEP;
            }
        }
    }

    fn frame(&mut self) -> [u16; NUM_CHANNELS] {
        // Window.
        // The reference keeps the running maximum in an int16: the absolute value of -32768 wraps and is ignored.
        let mut max_abs: i16 = 0;
        for i in 0..WINDOW {
            let v = ((i32::from(self.coefficients[i]) * i32::from(self.input[i])) >> WINDOW_BITS)
                as i16;
            self.windowed[i] = v;
            max_abs = max_abs.max(v.wrapping_abs());
        }
        // Scale up so the FFT has as much resolution as possible.
        let input_shift = 15 - msb32(max_abs as u32);
        for i in 0..WINDOW {
            self.fft_in[i] = ((self.windowed[i] as u16 as u32) << input_shift) as u16 as i16;
        }
        for v in &mut self.fft_in[WINDOW..] {
            *v = 0;
        }
        self.fft.real(&self.fft_in, &mut self.spectrum);

        // Power spectrum of the used bins.
        let fb = &self.filterbank;
        for k in fb.start_index..=fb.end_index {
            let c = self.spectrum[k];
            let re = i32::from(c.r);
            let im = i32::from(c.i);
            self.energy[k] = re.wrapping_mul(re).wrapping_add(im.wrapping_mul(im));
        }
        // Triangular filters: work[i] is the channel centred between segments i-1 and i.
        let mut work = [0u64; NUM_CHANNELS + 1];
        let mut weight_acc: u64 = 0;
        let mut unweight_acc: u64 = 0;
        #[allow(clippy::needless_range_loop)]
        for i in 0..=NUM_CHANNELS {
            let fs = fb.freq_starts[i];
            let ws = fb.weight_starts[i];
            for j in 0..fb.widths[i] {
                let e = self.energy[fs + j] as i64 as u64;
                weight_acc =
                    weight_acc.wrapping_add(u64::from(fb.weights[ws + j] as u16).wrapping_mul(e));
                unweight_acc = unweight_acc
                    .wrapping_add(u64::from(fb.unweights[ws + j] as u16).wrapping_mul(e));
            }
            work[i] = weight_acc;
            weight_acc = unweight_acc;
            unweight_acc = 0;
        }
        let mut signal = [0u32; NUM_CHANNELS];
        for (i, s) in signal.iter_mut().enumerate() {
            *s = sqrt_round(work[i + 1]) >> input_shift;
        }

        // Noise reduction.
        for (i, s) in signal.iter_mut().enumerate() {
            let smoothing = if i & 1 == 0 {
                self.even_smoothing
            } else {
                self.odd_smoothing
            };
            let one_minus = (1u32 << NOISE_BITS) - smoothing;
            let scaled_up = *s << SMOOTHING_BITS;
            let mut estimate = ((u64::from(scaled_up) * u64::from(smoothing)
                + u64::from(self.noise_estimate[i]) * u64::from(one_minus))
                >> NOISE_BITS) as u32;
            self.noise_estimate[i] = estimate;
            if estimate > scaled_up {
                estimate = scaled_up;
            }
            let floor =
                ((u64::from(*s) * u64::from(self.min_signal_remaining)) >> NOISE_BITS) as u32;
            let subtracted = (scaled_up - estimate) >> SMOOTHING_BITS;
            *s = subtracted.max(floor);
        }

        // Per-channel gain control.
        for (i, s) in signal.iter_mut().enumerate() {
            let gain = wide_dynamic(self.noise_estimate[i], &self.lut_gain) as u32;
            let snr = ((u64::from(*s) * u64::from(gain)) >> PCAN_SNR_SHIFT) as u32;
            *s = pcan_shrink(snr);
        }

        // Log scale.
        let mut out = [0u16; NUM_CHANNELS];
        for (o, &s) in out.iter_mut().zip(signal.iter()) {
            let value = s << CORRECTION_BITS;
            *o = if value > 1 {
                log_scaled(&self.lut_log, value).min(u32::from(u16::MAX)) as u16
            } else {
                0
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_rounds_like_the_reference() {
        for n in 0u64..5000 {
            let r = u64::from(sqrt_round(n));
            let f = (n as f64).sqrt();
            assert!((r as f64 - f).abs() <= 0.5 + 1e-9, "n={n} r={r}");
        }
        assert_eq!(sqrt_round(u64::MAX >> 1), 3_037_000_500);
    }

    #[test]
    fn log_table_matches_the_published_start() {
        let l = log_lut();
        assert_eq!(l.len(), 129);
        assert_eq!(&l[..6], &[0, 224, 442, 654, 861, 1063]);
    }

    #[test]
    fn the_fixed_point_fft_matches_a_float_dft_scaled_by_the_length() {
        let mut fft = Fft::new();
        let mut input = [0i16; FFT_SIZE];
        for (n, v) in input.iter_mut().enumerate() {
            let t = n as f64;
            *v = (8000.0 * (2.0 * std::f64::consts::PI * 32.0 * t / 512.0).sin()
                + 3000.0 * (2.0 * std::f64::consts::PI * 100.5 * t / 512.0).cos())
                as i16;
        }
        let mut out = [Cpx::default(); SPECTRUM];
        fft.real(&input, &mut out);
        let mut worst: f64 = 0.0;
        for (k, c) in out.iter().enumerate() {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &x) in input.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / 512.0;
                re += f64::from(x) * a.cos();
                im += f64::from(x) * a.sin();
            }
            worst = worst
                .max((re / 512.0 - f64::from(c.r)).abs())
                .max((im / 512.0 - f64::from(c.i)).abs());
        }
        assert!(worst < 4.0, "worst error {worst}");
    }

    #[test]
    fn silence_gives_a_frame_every_ten_milliseconds_after_the_first_window() {
        let mut fe = Frontend::new();
        let mut ends = Vec::new();
        fe.process(&vec![0i16; 1600], |_, end| ends.push(end));
        assert_eq!(ends.first(), Some(&480));
        assert_eq!(ends.len(), 8);
        assert!(ends.windows(2).all(|w| w[1] - w[0] == 160));
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_audio_is_cut() {
        let audio: Vec<i16> = (0..8000)
            .map(|i| {
                ((i as f64 * 0.37).sin() * 6000.0 + ((i * 7919) % 2001) as f64 - 1000.0) as i16
            })
            .collect();
        let collect = |chunk: usize| {
            let mut fe = Frontend::new();
            let mut v = Vec::new();
            for c in audio.chunks(chunk) {
                fe.process(c, |f, e| v.push((*f, e)));
            }
            v
        };
        assert_eq!(collect(160), collect(1));
        assert_eq!(collect(160), collect(777));
    }
}
