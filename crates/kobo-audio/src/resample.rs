//! Rate conversion for interleaved stereo, from whatever a book was
//! recorded at to whatever the sink plays.
//!
//! Audiobooks are mostly 22.05 or 24 kHz mono and the Bluetooth encoder
//! wants 48 kHz stereo, so nearly every book is resampled, and upwards.
//! Four-point cubic interpolation is used rather than the two-point linear
//! kind: linear interpolation of a 22 kHz source leaves a copy of the
//! speech band folded above it at a level a listener on good headphones can
//! hear as fizz on sibilants, and cubic pushes that down by about thirty
//! decibels for a few multiplies a sample, which the reader's one core does
//! not notice.

/// Fixed-point position: the fraction between two input frames, in
/// sixty-fourths of a thousand.
const FRACTION_BITS: u32 = 16;
const FRACTION_ONE: u64 = 1 << FRACTION_BITS;

/// Converts stereo frames from one rate to another, a chunk at a time,
/// keeping the few frames of history the next chunk needs.
#[derive(Clone, Debug)]
pub struct Resampler {
    from: u32,
    to: u32,
    /// Input frames not yet consumed, interleaved, the first three of which
    /// are history for the interpolator.
    held: Vec<i16>,
    /// Where the next output frame is read from, in input frames from the
    /// start of `held`, with [`FRACTION_BITS`] of fraction.
    position: u64,
    /// How far the read position moves per output frame.
    step: u64,
}

impl Resampler {
    /// A converter from `from` to `to` frames a second. Equal rates pass
    /// sound straight through.
    #[must_use]
    pub fn new(from: u32, to: u32) -> Self {
        let from = from.max(1);
        let to = to.max(1);
        let step = (u64::from(from) << FRACTION_BITS) / u64::from(to);
        Self {
            from,
            to,
            held: Vec::new(),
            position: 0,
            step: step.max(1),
        }
    }

    #[must_use]
    pub const fn from_rate(&self) -> u32 {
        self.from
    }

    #[must_use]
    pub const fn to_rate(&self) -> u32 {
        self.to
    }

    /// Forgets the history, for a seek.
    pub fn reset(&mut self) {
        self.held.clear();
        self.position = 0;
    }

    /// Converts `input`, appending to `out` every output frame that can be
    /// made from what has arrived so far.
    pub fn push(&mut self, input: &[i16], out: &mut Vec<i16>) {
        if self.from == self.to {
            out.extend_from_slice(&input[..input.len() / 2 * 2]);
            return;
        }
        if self.held.is_empty() {
            // The first frame stands in for the history before it, so the
            // sound starts where it starts rather than fading in from zero.
            if let Some(first) = input.chunks_exact(2).next() {
                for _ in 0..3 {
                    self.held.extend_from_slice(first);
                }
                self.position = FRACTION_ONE;
            } else {
                return;
            }
        }
        self.held.extend_from_slice(&input[..input.len() / 2 * 2]);
        let frames = self.held.len() / 2;
        loop {
            let index = usize::try_from(self.position >> FRACTION_BITS).unwrap_or(usize::MAX);
            // Cubic interpolation reads one frame before and two after.
            if index == 0 || index + 2 >= frames {
                break;
            }
            let fraction = i64::try_from(self.position & (FRACTION_ONE - 1)).unwrap_or(0);
            for channel in 0..2 {
                let at = |offset: usize| i64::from(self.held[(index + offset - 1) * 2 + channel]);
                out.push(cubic(at(0), at(1), at(2), at(3), fraction));
            }
            self.position += self.step;
        }
        // Keep the frame before the read position and everything after it.
        let keep_from = usize::try_from(self.position >> FRACTION_BITS)
            .unwrap_or(0)
            .saturating_sub(1);
        if keep_from > 0 {
            self.held.drain(..keep_from * 2);
            self.position -= (keep_from as u64) << FRACTION_BITS;
        }
    }
}

/// Catmull-Rom interpolation between `p1` and `p2`, `fraction` of the way,
/// with `p0` and `p3` shaping the curve.
fn cubic(p0: i64, p1: i64, p2: i64, p3: i64, fraction: i64) -> i16 {
    let t = fraction;
    let one = i64::try_from(FRACTION_ONE).unwrap_or(i64::MAX);
    let a = -p0 + 3 * p1 - 3 * p2 + p3;
    let b = 2 * p0 - 5 * p1 + 4 * p2 - p3;
    let c = -p0 + p2;
    // Horner's form in fixed point: each product is shifted back as it is
    // made so the running value never leaves 48 bits.
    let mut value = a * t / one;
    value = (value + b) * t / one;
    value = (value + c) * t / one;
    value = (value + 2 * p1) / 2;
    i16::try_from(value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)))
        .expect("the sample was clamped to i16")
}

#[cfg(test)]
mod tests {
    use super::{cubic, Resampler, FRACTION_ONE};

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "a test tone, where the rounding is the point"
    )]
    fn sine(rate: u32, hertz: u32, frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|frame| {
                let phase =
                    frame as f64 * f64::from(hertz) / f64::from(rate) * std::f64::consts::TAU;
                let sample = (phase.sin() * 20_000.0) as i16;
                [sample, sample]
            })
            .collect()
    }

    /// Zero crossings a second, which for a clean tone is twice its pitch.
    #[allow(clippy::cast_precision_loss, reason = "counts, not samples")]
    fn crossings_per_second(samples: &[i16], rate: u32) -> f64 {
        let left = samples.iter().step_by(2).collect::<Vec<_>>();
        let crossings = left
            .windows(2)
            .filter(|pair| (*pair[0] < 0) != (*pair[1] < 0))
            .count();
        crossings as f64 * f64::from(rate) / left.len() as f64
    }

    #[test]
    fn equal_rates_pass_straight_through() {
        let mut resampler = Resampler::new(48_000, 48_000);
        let input = sine(48_000, 440, 480);
        let mut out = Vec::new();
        resampler.push(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn a_tone_keeps_its_pitch_and_its_length_going_up() {
        let mut resampler = Resampler::new(22_050, 48_000);
        let input = sine(22_050, 440, 22_050);
        let mut out = Vec::new();
        for chunk in input.chunks(2_000) {
            resampler.push(chunk, &mut out);
        }
        let frames = out.len() / 2;
        assert!((47_900..=48_010).contains(&frames), "{frames} frames");
        let pitch = crossings_per_second(&out, 48_000) / 2.0;
        assert!((435.0..445.0).contains(&pitch), "{pitch} Hz");
    }

    #[test]
    fn a_tone_keeps_its_pitch_going_down() {
        let mut resampler = Resampler::new(96_000, 48_000);
        let input = sine(96_000, 1_000, 96_000);
        let mut out = Vec::new();
        resampler.push(&input, &mut out);
        let frames = out.len() / 2;
        assert!((47_900..=48_000).contains(&frames), "{frames} frames");
        let pitch = crossings_per_second(&out, 48_000) / 2.0;
        assert!((990.0..1_010.0).contains(&pitch), "{pitch} Hz");
    }

    #[test]
    fn the_curve_passes_through_its_points_and_stays_between_them() {
        assert_eq!(cubic(0, 1_000, 2_000, 3_000, 0), 1_000);
        let half = i64::try_from(FRACTION_ONE / 2).unwrap();
        assert_eq!(cubic(0, 1_000, 2_000, 3_000, half), 1_500);
        assert_eq!(
            cubic(
                i64::from(i16::MAX),
                i64::from(i16::MAX),
                i64::from(i16::MAX),
                i64::from(i16::MAX),
                half
            ),
            i16::MAX
        );
        assert_eq!(
            cubic(
                i64::from(i16::MIN),
                i64::from(i16::MIN),
                i64::from(i16::MIN),
                i64::from(i16::MIN),
                half
            ),
            i16::MIN
        );
    }

    #[test]
    fn chunk_boundaries_leave_no_seam() {
        let mut whole = Resampler::new(24_000, 48_000);
        let mut pieces = Resampler::new(24_000, 48_000);
        let input = sine(24_000, 300, 2_400);
        let mut one = Vec::new();
        whole.push(&input, &mut one);
        let mut many = Vec::new();
        for chunk in input.chunks(94) {
            pieces.push(chunk, &mut many);
        }
        assert_eq!(one, many);
    }
}
