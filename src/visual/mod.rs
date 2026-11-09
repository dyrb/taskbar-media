//! Bounded PCM analysis. No allocations are performed while ingesting or analysing audio.

use crate::platform::ffi::{E_INVALIDARG, Guid};
use crate::platform::{Error, Result};
use std::f32::consts::PI;
use std::time::Duration;

pub const WINDOW: usize = 1024;
pub const POINTS: usize = 64;
pub const BANDS: usize = 32;
pub const CHANNELS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Unsigned8,
    Signed16,
    Signed24,
    Signed32,
    Float32,
    Float64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    pub(crate) frame_bytes: usize,
    sample_bytes: usize,
    encoding: Encoding,
}

impl Format {
    pub fn sample_rate(self) -> u32 {
        self.rate
    }

    pub fn channels(self) -> usize {
        self.channels
    }

    /// Parse the WAVEFORMATEX/WAVEFORMATEXTENSIBLE returned by GetMixFormat.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let invalid = || Error::new("unsupported audio format", E_INVALIDARG);
        if bytes.len() < 18 {
            return Err(invalid());
        }
        let u16_at = |n| u16::from_le_bytes([bytes[n], bytes[n + 1]]);
        let u32_at = |n| u32::from_le_bytes(bytes[n..n + 4].try_into().expect("checked format header"));
        let mut tag = u16_at(0);
        let channels = u16_at(2) as usize;
        let rate = u32_at(4);
        let frame_bytes = u16_at(12) as usize;
        let bits = u16_at(14);
        let extra = u16_at(16) as usize;
        if channels == 0 || channels > CHANNELS || !(8000..=384000).contains(&rate) || bytes.len() < 18 + extra {
            return Err(invalid());
        }
        if tag == 0xfffe {
            if extra < 22 || u16_at(18) == 0 || u16_at(18) > bits {
                return Err(invalid());
            }
            const PCM: Guid = Guid::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
            const FLOAT: Guid = Guid::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
            let subtype = Guid {
                data1: u32_at(24),
                data2: u16_at(28),
                data3: u16_at(30),
                data4: bytes[32..40].try_into().unwrap(),
            };
            tag = if subtype == PCM {
                1
            } else if subtype == FLOAT {
                3
            } else {
                return Err(invalid());
            };
        }
        let encoding = match (tag, bits) {
            (1, 8) => Encoding::Unsigned8,
            (1, 16) => Encoding::Signed16,
            (1, 24) => Encoding::Signed24,
            (1, 32) => Encoding::Signed32,
            (3, 32) => Encoding::Float32,
            (3, 64) => Encoding::Float64,
            _ => return Err(invalid()),
        };
        let sample_bytes = bits as usize / 8;
        if frame_bytes != channels * sample_bytes || u32_at(8) != rate * frame_bytes as u32 {
            return Err(invalid());
        }
        Ok(Self { rate, channels, frame_bytes, sample_bytes, encoding })
    }

    fn sample(self, bytes: &[u8]) -> f32 {
        let sample = match self.encoding {
            Encoding::Unsigned8 => (bytes[0] as f32 - 128.0) / 128.0,
            Encoding::Signed16 => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f32 / 32768.0,
            Encoding::Signed24 => (i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32 / 8388608.0,
            Encoding::Signed32 => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f32 / 2147483648.0,
            Encoding::Float32 => f32::from_le_bytes(bytes[..4].try_into().unwrap()),
            Encoding::Float64 => f64::from_le_bytes(bytes[..8].try_into().unwrap()) as f32,
        };
        if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub low: f32,
    pub high: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub wave: [Range; POINTS],
    /// Peak-equivalent linear amplitudes integrated over disjoint Mel-spaced bands.
    pub bands: [f32; BANDS],
    pub peak: f32,
    pub rms: f32,
    pub frames: u64,
    pub discontinuities: u64,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            wave: [Range::default(); POINTS],
            bands: [0.0; BANDS],
            peak: 0.0,
            rms: 0.0,
            frames: 0,
            discontinuities: 0,
        }
    }
}

pub struct Smoothing {
    bands: [f32; BANDS],
    attack: f32,
    release: f32,
}

impl Default for Smoothing {
    fn default() -> Self {
        Self { bands: [0.0; BANDS], attack: 0.035, release: 0.180 }
    }
}

impl Smoothing {
    pub fn new(attack: Duration, release: Duration) -> Result<Self> {
        if attack.is_zero()
            || release.is_zero()
            || attack > Duration::from_secs(10)
            || release > Duration::from_secs(10)
        {
            return Err(Error::new(
                "smoothing duration must be greater than zero and at most ten seconds",
                E_INVALIDARG,
            ));
        }
        Ok(Self { bands: [0.0; BANDS], attack: attack.as_secs_f32(), release: release.as_secs_f32() })
    }

    pub fn apply(&mut self, bands: &mut [f32; BANDS], elapsed: Duration) {
        let attack = 1.0 - (-elapsed.as_secs_f32() / self.attack).exp();
        let release = 1.0 - (-elapsed.as_secs_f32() / self.release).exp();
        for (previous, value) in self.bands.iter_mut().zip(bands) {
            let target = if value.is_finite() { value.max(0.0) } else { 0.0 };
            let coefficient = if target > *previous { attack } else { release };
            *previous += (target - *previous) * coefficient;
            *value = *previous;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f32,
    im: f32,
}

struct Fft<const N: usize> {
    roots: [Complex; N],
}

impl<const N: usize> Fft<N> {
    fn new() -> Self {
        const { assert!(N >= 2 && N.is_power_of_two()) };
        Self {
            roots: std::array::from_fn(|i| {
                let angle = -2.0 * PI * i as f32 / N as f32;
                Complex { re: angle.cos(), im: angle.sin() }
            }),
        }
    }

    fn transform(&self, data: &mut [Complex; N]) {
        let shift = usize::BITS - N.trailing_zeros();
        for i in 0..N {
            let j = i.reverse_bits() >> shift;
            if j > i {
                data.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= N {
            let half = len / 2;
            for start in (0..N).step_by(len) {
                for j in 0..half {
                    let a = data[start + j];
                    let b = data[start + j + half];
                    let root = self.roots[j * (N / len)];
                    let re = b.re * root.re - b.im * root.im;
                    let im = b.re * root.im + b.im * root.re;
                    data[start + j] = Complex { re: a.re + re, im: a.im + im };
                    data[start + j + half] = Complex { re: a.re - re, im: a.im - im };
                }
            }
            len *= 2;
        }
    }
}

pub struct Analyzer {
    format: Format,
    ring: [[f32; WINDOW]; CHANNELS],
    next: usize,
    valid: usize,
    frames: u64,
    discontinuities: u64,
    window: [f32; WINDOW],
    spectrum_scale: f32,
    fft: Fft<WINDOW>,
    scratch: [Complex; WINDOW],
    power: [f32; WINDOW / 2 + 1],
    edges: [usize; BANDS + 1],
}

impl Analyzer {
    pub fn new(format: Format) -> Self {
        let bin_hz = format.rate as f32 / WINDOW as f32;
        let high = 16000.0f32.min(format.rate as f32 / 2.0);
        let end = ((high / bin_hz).floor() as usize).min(WINDOW / 2) + 1;
        let low_mel = (1.0 + bin_hz.max(40.0) / 700.0).ln();
        let high_mel = (1.0 + high / 700.0).ln();
        let mut edges = [0; BANDS + 1];
        edges[0] = ((40.0 / bin_hz).round() as usize).clamp(1, end - BANDS);
        for i in 1..BANDS {
            let mel = low_mel + (high_mel - low_mel) * i as f32 / BANDS as f32;
            let bin = (700.0 * mel.exp_m1() / bin_hz).round() as usize;
            edges[i] = bin.clamp(edges[i - 1] + 1, end - (BANDS - i));
        }
        edges[BANDS] = end;
        let window = std::array::from_fn(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (WINDOW - 1) as f32).cos());
        let spectrum_scale = (4.0 / (WINDOW as f32 * window.iter().map(|v| v * v).sum::<f32>())).sqrt();
        Self {
            format,
            ring: [[0.0; WINDOW]; CHANNELS],
            next: 0,
            valid: 0,
            frames: 0,
            discontinuities: 0,
            window,
            spectrum_scale,
            fft: Fft::new(),
            scratch: [Complex::default(); WINDOW],
            power: [0.0; WINDOW / 2 + 1],
            edges,
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn push(&mut self, data: Option<&[u8]>, frames: usize, discontinuity: bool) -> Result<()> {
        let needed =
            frames.checked_mul(self.format.frame_bytes).ok_or(Error::new("audio packet size", E_INVALIDARG))?;
        if data.is_some_and(|d| d.len() < needed) {
            return Err(Error::new("truncated audio packet", E_INVALIDARG));
        }
        if discontinuity {
            self.ring.fill([0.0; WINDOW]);
            self.valid = 0;
            self.discontinuities = self.discontinuities.saturating_add(1);
        }
        // Only the final window is needed for drawing; still account for the entire packet.
        let begin = frames.saturating_sub(WINDOW);
        self.next = (self.next + begin) & (WINDOW - 1);
        for i in begin..frames {
            for channel in 0..self.format.channels {
                self.ring[channel][self.next] = data.map_or(0.0, |d| {
                    self.format.sample(&d[i * self.format.frame_bytes + channel * self.format.sample_bytes..])
                });
            }
            self.next = (self.next + 1) & (WINDOW - 1);
        }
        self.valid = self.valid.saturating_add(frames).min(WINDOW);
        self.frames = self.frames.saturating_add(frames as u64);
        Ok(())
    }

    pub fn frame(&mut self, spectrum: bool) -> Frame {
        let mut result = Frame { frames: self.frames, discontinuities: self.discontinuities, ..Frame::default() };
        let mut sum = 0.0f64;
        for i in 0..WINDOW {
            for channel in 0..self.format.channels {
                let value = self.ring[channel][(self.next + i) & (WINDOW - 1)];
                let range = &mut result.wave[i * POINTS / WINDOW];
                range.low = range.low.min(value);
                range.high = range.high.max(value);
                result.peak = result.peak.max(value.abs());
                sum += (value as f64).powi(2);
            }
        }
        if self.valid > 0 {
            result.rms = (sum / (self.valid * self.format.channels) as f64).sqrt() as f32;
        }
        if spectrum && result.peak > 0.0 {
            self.power.fill(0.0);
            for channel in 0..self.format.channels {
                let mean = self.ring[channel].iter().sum::<f32>() / WINDOW as f32;
                for i in 0..WINDOW {
                    self.scratch[i] = Complex {
                        re: (self.ring[channel][(self.next + i) & (WINDOW - 1)] - mean) * self.window[i],
                        im: 0.0,
                    };
                }
                self.fft.transform(&mut self.scratch);
                for (power, value) in self.power.iter_mut().zip(&self.scratch) {
                    *power += value.re * value.re + value.im * value.im;
                }
            }
            for (i, band) in result.bands.iter_mut().enumerate() {
                let first = self.edges[i];
                let end = self.edges[i + 1];
                let power = self.power[first..end].iter().sum::<f32>();
                *band = (power / self.format.channels as f32).sqrt() * self.spectrum_scale;
            }
        }
        result
    }
}

/// Fixed -60..0 dB display range; silence stays silent, with no per-frame auto gain.
pub fn spectrum_height(amplitude: f32, gain: f32) -> f32 {
    let value = amplitude * gain;
    if !value.is_finite() || value <= 0.001 {
        0.0
    } else if value >= 1.0 {
        1.0
    } else {
        1.0 + value.log2() * (20.0 * std::f32::consts::LOG10_2 / 60.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float_stereo() -> Format {
        Format { rate: 48000, channels: 2, frame_bytes: 8, sample_bytes: 4, encoding: Encoding::Float32 }
    }

    #[test]
    fn fft_matches_direct_dft_for_complex_input() {
        const N: usize = 16;
        let data: [Complex; N] =
            std::array::from_fn(|i| Complex { re: (i as f32 * 0.7).sin(), im: (i as f32 * 0.3).cos() });
        let mut transformed = data;
        Fft::new().transform(&mut transformed);
        for (k, actual) in transformed.iter().enumerate() {
            let mut expected = Complex::default();
            for (n, x) in data.iter().enumerate() {
                let angle = -2.0 * PI * (n * k) as f32 / N as f32;
                expected.re += x.re * angle.cos() - x.im * angle.sin();
                expected.im += x.re * angle.sin() + x.im * angle.cos();
            }
            assert!((actual.re - expected.re).abs() < 0.0001);
            assert!((actual.im - expected.im).abs() < 0.0001);
        }
    }

    #[test]
    fn antiphase_stereo_has_energy_and_a_frequency_peak() {
        let mut analyzer = Analyzer::new(float_stereo());
        let mut bytes = Vec::new();
        for n in 0..WINDOW {
            let value = (2.0 * PI * 32.0 * n as f32 / WINDOW as f32).sin() * 0.5;
            bytes.extend_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&(-value).to_le_bytes());
        }
        analyzer.push(Some(&bytes), WINDOW, false).unwrap();
        let frame = analyzer.frame(true);
        assert!((frame.peak - 0.5).abs() < 0.001);
        assert!((frame.rms - 0.5 / 2.0f32.sqrt()).abs() < 0.001);
        assert!((frame.bands.iter().map(|v| v * v).sum::<f32>().sqrt() - 0.5).abs() < 0.001);
        assert!(frame.wave.iter().any(|p| p.high > 0.49 && p.low < -0.49));
        analyzer.push(None, WINDOW, false).unwrap();
        let silent = analyzer.frame(true);
        assert_eq!(silent.peak, 0.0);
        assert!(silent.bands.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn malformed_formats_and_packets_are_rejected_without_mutation() {
        assert!(Format::parse(&[0; 18]).is_err());
        let mut analyzer = Analyzer::new(float_stereo());
        assert!(analyzer.push(Some(&[0; 7]), 1, false).is_err());
        assert_eq!(analyzer.frame(true).frames, 0);
        let mut data = Vec::new();
        data.extend_from_slice(&f32::NAN.to_le_bytes());
        data.extend_from_slice(&f32::INFINITY.to_le_bytes());
        analyzer.push(Some(&data), 1, false).unwrap();
        assert_eq!(analyzer.frame(true).peak, 0.0);
        assert!(size_of::<Analyzer>() < 256 * 1024);
    }

    #[test]
    fn spectrum_bands_do_not_duplicate_bins_at_any_supported_rate() {
        for rate in [8000, 44100, 48000, 96000, 192000, 384000] {
            let analyzer = Analyzer::new(Format { rate, ..float_stereo() });
            assert!(analyzer.edges.windows(2).all(|e| e[0] < e[1]));
            assert!(analyzer.edges[0] >= 1 && analyzer.edges[BANDS] <= WINDOW / 2 + 1);
            if rate == 48000 {
                assert!(
                    analyzer.edges[1..].iter().filter(|&&bin| bin as f32 * 48000.0 / WINDOW as f32 <= 250.0).count()
                        <= 3
                );
            }
        }
    }

    #[test]
    fn spectrum_preserves_tone_energy_across_frequencies_and_removes_dc() {
        for bin in [4, 32, 192] {
            let mut analyzer = Analyzer::new(float_stereo());
            let bytes: Vec<u8> = (0..WINDOW)
                .flat_map(|n| {
                    let value = 0.25 + 0.125 * (2.0 * PI * bin as f32 * n as f32 / WINDOW as f32).sin();
                    [value.to_le_bytes(), value.to_le_bytes()].concat()
                })
                .collect();
            analyzer.push(Some(&bytes), WINDOW, false).unwrap();
            let frame = analyzer.frame(true);
            let energy = frame.bands.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((energy - 0.125).abs() < 0.001, "bin {bin}: {energy}");
            let peak = frame.bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
            assert!(analyzer.edges[peak] <= bin && bin < analyzer.edges[peak + 1]);
        }
        let mut dc = Analyzer::new(float_stereo());
        let bytes = 0.25f32.to_le_bytes().repeat(WINDOW * 2);
        dc.push(Some(&bytes), WINDOW, false).unwrap();
        assert!(dc.frame(true).bands.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn spectrum_display_shows_quiet_bands_without_lifting_silence() {
        assert_eq!(spectrum_height(0.0, 16.0), 0.0);
        assert_eq!(spectrum_height(0.0001, 1.0), 0.0);
        assert_eq!(spectrum_height(1.0, 1.0), 1.0);
        assert_eq!(spectrum_height(f32::NAN, 1.0), 0.0);
        assert!((spectrum_height(0.01, 1.0) - 1.0 / 3.0).abs() < 0.00001);
        assert!((spectrum_height(0.1, 1.0) - 2.0 / 3.0).abs() < 0.00001);
    }

    #[test]
    fn signed_24_bit_endpoints_are_decoded_correctly() {
        let f = Format { encoding: Encoding::Signed24, ..float_stereo() };
        assert_eq!(f.sample(&[0, 0, 0x80]), -1.0);
        assert!(f.sample(&[0xff, 0xff, 0x7f]) > 0.999);
    }

    #[test]
    fn packet_boundaries_do_not_change_the_latest_window() {
        let bytes: Vec<u8> = (0..WINDOW * 5 + 137)
            .flat_map(|i| {
                let value = (i as f32 * 0.13).sin() * 0.7;
                [value.to_le_bytes(), (-value * 0.5).to_le_bytes()].concat()
            })
            .collect();
        let mut whole = Analyzer::new(float_stereo());
        whole.push(Some(&bytes), bytes.len() / 8, false).unwrap();
        let expected = whole.frame(true);
        for chunk_frames in [1, 13, 137, 512, 1700] {
            let mut chunked = Analyzer::new(float_stereo());
            for chunk in bytes.chunks(chunk_frames * 8) {
                chunked.push(Some(chunk), chunk.len() / 8, false).unwrap();
            }
            assert_eq!(chunked.frame(true), expected);
        }
    }

    #[test]
    fn wave_format_extensions_and_all_truncations_are_checked() {
        let mut header = [0u8; 40];
        header[0..2].copy_from_slice(&0xfffeu16.to_le_bytes());
        header[2..4].copy_from_slice(&2u16.to_le_bytes());
        header[4..8].copy_from_slice(&48000u32.to_le_bytes());
        header[8..12].copy_from_slice(&384000u32.to_le_bytes());
        header[12..14].copy_from_slice(&8u16.to_le_bytes());
        header[14..16].copy_from_slice(&32u16.to_le_bytes());
        header[16..18].copy_from_slice(&22u16.to_le_bytes());
        header[18..20].copy_from_slice(&32u16.to_le_bytes());
        header[24..40].copy_from_slice(&[3, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
        assert_eq!(Format::parse(&header).unwrap(), float_stereo());
        for end in 0..header.len() {
            assert!(Format::parse(&header[..end]).is_err());
        }
        header[18] = 33;
        assert!(Format::parse(&header).is_err());
        header[18] = 32;
        header[2] = 9;
        assert!(Format::parse(&header).is_err());
    }

    #[test]
    fn smoothing_uses_elapsed_time_instead_of_frame_count() {
        let mut single = Smoothing::default();
        let mut many = Smoothing::default();
        let mut expected = [0.75; BANDS];
        single.apply(&mut expected, Duration::from_millis(100));
        let mut actual = [0.0; BANDS];
        for _ in 0..10 {
            actual.fill(0.75);
            many.apply(&mut actual, Duration::from_millis(10));
        }
        assert!((actual[0] - expected[0]).abs() < 0.00001);
        actual.fill(0.0);
        many.apply(&mut actual, Duration::from_millis(100));
        assert!(actual[0] > 0.0 && actual[0] < expected[0]);
        assert!(Smoothing::new(Duration::ZERO, Duration::from_millis(100)).is_err());
    }
}
