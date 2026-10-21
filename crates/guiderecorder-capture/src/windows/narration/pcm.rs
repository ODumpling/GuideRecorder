//! The microphone's samples as narration uses them: mono, at the rate the
//! recogniser or the audio file wants, whatever the device gives.
//!
//! WASAPI's shared mode hands over the device's mix format, usually 32-bit
//! float at 48 kHz with two channels, sometimes integer PCM. [`to_mono`]
//! averages the channels to one float; [`Resampler`] changes the rate by
//! averaging the input over each output sample (a box filter: enough
//! against aliasing for speech, and exact over any ratio, so no drift);
//! [`to_i16`] makes 16-bit PCM of the result.

/// How the device's samples are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixFormat {
    pub rate: u32,
    pub channels: u16,
    /// Bytes per sample of one channel (2, 3 or 4).
    pub bytes: u16,
    /// 32-bit IEEE float, rather than signed integer PCM.
    pub float: bool,
}

impl MixFormat {
    /// Bytes per frame: one sample of every channel.
    pub fn frame_bytes(&self) -> usize {
        usize::from(self.channels) * usize::from(self.bytes)
    }

    /// Whether [`to_mono`] can read it.
    pub fn is_readable(&self) -> bool {
        self.rate > 0
            && self.channels > 0
            && if self.float {
                self.bytes == 4
            } else {
                matches!(self.bytes, 2..=4)
            }
    }
}

/// `data` (whole frames in `format`) as one channel of floats in -1..1,
/// the channels averaged. A partial frame at the end is left out.
pub fn to_mono(data: &[u8], format: &MixFormat) -> Vec<f32> {
    let frame = format.frame_bytes();
    if frame == 0 || !format.is_readable() {
        return Vec::new();
    }
    let size = usize::from(format.bytes);
    let channels = f32::from(format.channels);
    data.chunks_exact(frame)
        .map(|frame| {
            frame
                .chunks_exact(size)
                .map(|s| sample(s, format.float))
                .sum::<f32>()
                / channels
        })
        .collect()
}

/// One little-endian sample as a float in -1..1.
fn sample(bytes: &[u8], float: bool) -> f32 {
    match (bytes, float) {
        (&[a, b, c, d], true) => f32::from_le_bytes([a, b, c, d]),
        (&[a, b], false) => f32::from(i16::from_le_bytes([a, b])) / 32_768.0,
        // 24-bit: shifted into the top of an i32 to keep the sign.
        (&[a, b, c], false) => i32::from_le_bytes([0, a, b, c]) as f32 / 2_147_483_648.0,
        (&[a, b, c, d], false) => i32::from_le_bytes([a, b, c, d]) as f32 / 2_147_483_648.0,
        _ => 0.0,
    }
}

/// Floats in -1..1 as 16-bit PCM, clipped.
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
        .collect()
}

/// Changes the sample rate of a stream given in pieces. Each output
/// sample is the average of the input over its span, counted in whole
/// units (an input sample is `to` units long, an output one `from`), so
/// any ratio is exact and the output never drifts from the input.
#[derive(Debug, Clone)]
pub struct Resampler {
    from: u64,
    to: u64,
    /// Units of the output sample being built that are filled.
    filled: u64,
    /// Their sum, weighted by units.
    sum: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            from: u64::from(from.max(1)),
            to: u64::from(to.max(1)),
            filled: 0,
            sum: 0.0,
        }
    }

    /// Whether it changes anything: false when the rates are the same.
    pub fn is_needed(&self) -> bool {
        self.from != self.to
    }

    /// Takes the next input samples; returns the output samples they
    /// complete. The rest waits for the next call.
    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if !self.is_needed() {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity(input.len() * self.to as usize / self.from as usize + 1);
        for &x in input {
            let mut left = self.to;
            while left > 0 {
                let take = left.min(self.from - self.filled);
                self.sum += f64::from(x) * take as f64;
                self.filled += take;
                left -= take;
                if self.filled == self.from {
                    out.push((self.sum / self.from as f64) as f32);
                    self.filled = 0;
                    self.sum = 0.0;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOAT_STEREO: MixFormat = MixFormat {
        rate: 48_000,
        channels: 2,
        bytes: 4,
        float: true,
    };

    #[test]
    fn channels_are_averaged_to_mono() {
        let data: Vec<u8> = [0.5f32, -0.5, 1.0, 0.0, 0.25]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        // Two whole frames; the half frame at the end is left out.
        assert_eq!(to_mono(&data, &FLOAT_STEREO), vec![0.0, 0.5]);
    }

    #[test]
    fn integer_samples_of_every_size_read_as_floats() {
        let format = |bytes| MixFormat {
            rate: 16_000,
            channels: 1,
            bytes,
            float: false,
        };
        let i16s: Vec<u8> = [i16::MIN, 0, 16_384]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(to_mono(&i16s, &format(2)), vec![-1.0, 0.0, 0.5]);
        // 24-bit: -1, then half scale.
        let i24s = [0x00, 0x00, 0x80, 0x00, 0x00, 0x40];
        assert_eq!(to_mono(&i24s, &format(3)), vec![-1.0, 0.5]);
        let i32s: Vec<u8> = [i32::MIN, 1 << 30]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(to_mono(&i32s, &format(4)), vec![-1.0, 0.5]);
    }

    #[test]
    fn unreadable_formats_give_nothing() {
        let odd = MixFormat {
            bytes: 8,
            ..FLOAT_STEREO
        };
        assert!(!odd.is_readable());
        assert!(to_mono(&[0; 32], &odd).is_empty());
        let silent = MixFormat {
            channels: 0,
            ..FLOAT_STEREO
        };
        assert!(to_mono(&[0; 32], &silent).is_empty());
    }

    #[test]
    fn floats_become_clipped_16_bit() {
        assert_eq!(
            to_i16(&[0.0, 1.0, -1.0, 2.0, -3.0, 0.5]),
            vec![0, 32_767, -32_767, 32_767, -32_767, 16_384]
        );
    }

    #[test]
    fn downsampling_averages_each_span_and_keeps_the_count_exact() {
        let mut r = Resampler::new(48_000, 16_000);
        assert!(r.is_needed());
        // Three in, one out: their average.
        assert_eq!(r.push(&[0.3, 0.6, 0.9, 1.0]), vec![0.6]);
        // The fourth waits for two more.
        assert_eq!(r.push(&[1.0, 1.0]), vec![1.0]);
        // Over any split of a second of input: exactly a second out.
        let mut r = Resampler::new(44_100, 16_000);
        let total: usize = (0..441).map(|_| r.push(&[0.5; 100]).len()).sum();
        assert_eq!(total, 16_000);
    }

    #[test]
    fn upsampling_repeats_and_blends_samples() {
        let mut r = Resampler::new(8_000, 16_000);
        assert_eq!(r.push(&[0.2, 0.4]), vec![0.2, 0.2, 0.4, 0.4]);
        // 2:3 splits the middle output sample across two inputs.
        let mut r = Resampler::new(2, 3);
        let out = r.push(&[0.0, 0.9]);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.45).abs() < 1e-6, "{out:?}");
        assert!((out[2] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn the_same_rate_passes_through() {
        let mut r = Resampler::new(16_000, 16_000);
        assert!(!r.is_needed());
        assert_eq!(r.push(&[0.1, 0.2]), vec![0.1, 0.2]);
    }

    #[test]
    fn a_tone_keeps_its_level_through_resampling() {
        // 440 Hz at 48 kHz down to 16 kHz: well under the new Nyquist, so
        // it comes out at much the same level.
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
            .collect();
        let out = Resampler::new(48_000, 16_000).push(&tone);
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let (a, b) = (rms(&tone), rms(&out));
        assert!((b / a - 1.0).abs() < 0.05, "{a} {b}");
    }
}
