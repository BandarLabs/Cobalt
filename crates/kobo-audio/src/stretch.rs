//! Playing speech faster or slower without changing its pitch.
//!
//! This is WSOLA, waveform-similarity overlap-add, the method every
//! audiobook player uses. The sound is cut into frames that overlap, and
//! the frames are laid back down closer together or further apart than
//! they were taken, with each one slid a few milliseconds either way to
//! where its waveform lines up with the frame before it, so the joins fall
//! on matching cycles rather than between them. Speech survives this well
//! up to about twice its speed; past that the joins are audible on
//! consonants, which is also true of every player's three times.
//!
//! The arithmetic is integer. The search is what costs, and it is done in
//! two passes, coarse then fine, so a frame of twenty milliseconds costs
//! about seventy thousand multiplies at 48 kHz, which is nothing to a
//! single core with nothing else to do.

/// How long a synthesis frame is, in milliseconds.
const FRAME_MS: u32 = 30;
/// How much of each frame overlaps the one before it, in milliseconds.
const OVERLAP_MS: u32 = 10;
/// How far either side of its natural place a frame may be slid, in
/// milliseconds. Pitch periods are under ten milliseconds for every voice,
/// so a window this wide always holds a match.
const SEARCH_MS: u32 = 8;
/// The coarse search tries every this-many positions, then the fine search
/// tries each neighbour of the best.
const COARSE_STEP: usize = 4;

/// The slowest and fastest the stretcher will go, as percentages.
pub const MIN_SPEED_PERCENT: u16 = 50;
pub const MAX_SPEED_PERCENT: u16 = 300;

/// Changes the speed of interleaved stereo without changing its pitch.
#[derive(Clone, Debug)]
pub struct Stretcher {
    speed_percent: u16,
    frame: usize,
    overlap: usize,
    search: usize,
    /// Input frames held, interleaved, starting at absolute frame `base`.
    held: Vec<i16>,
    base: u64,
    /// Where the next frame would naturally be taken from, in input frames,
    /// with sixteen bits of fraction.
    cursor: u64,
    /// The end of the last frame laid down, which the next frame is matched
    /// against and faded in over. Interleaved, `overlap` frames long.
    tail: Vec<i16>,
}

impl Stretcher {
    /// A stretcher for sound at `rate`, starting at normal speed.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let per_ms = |ms: u32| {
            usize::try_from(u64::from(rate) * u64::from(ms) / 1_000)
                .unwrap_or(1)
                .max(1)
        };
        Self {
            speed_percent: 100,
            frame: per_ms(FRAME_MS),
            overlap: per_ms(OVERLAP_MS),
            search: per_ms(SEARCH_MS),
            held: Vec::new(),
            base: 0,
            cursor: 0,
            tail: Vec::new(),
        }
    }

    /// Sets the speed as a percentage, clamped to the range the stretcher
    /// can do well enough to be worth offering.
    pub fn set_speed(&mut self, percent: u16) {
        self.speed_percent = percent.clamp(MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
    }

    #[must_use]
    pub const fn speed_percent(&self) -> u16 {
        self.speed_percent
    }

    /// Forgets everything held, for a seek.
    pub fn reset(&mut self) {
        self.held.clear();
        self.base = 0;
        self.cursor = 0;
        self.tail.clear();
    }

    /// Feeds `input` and appends to `out` as many output frames as can be
    /// made. At normal speed the sound passes straight through.
    pub fn push(&mut self, input: &[i16], out: &mut Vec<i16>) {
        if self.speed_percent == 100 && self.held.is_empty() {
            out.extend_from_slice(&input[..input.len() / 2 * 2]);
            return;
        }
        self.held.extend_from_slice(&input[..input.len() / 2 * 2]);
        let hop = self.frame - self.overlap;
        loop {
            let natural = self.cursor >> 16;
            let Some(start) = natural.checked_sub(self.base) else {
                self.reset();
                return;
            };
            let Ok(start) = usize::try_from(start) else {
                self.reset();
                return;
            };
            let search = if self.tail.is_empty() { 0 } else { self.search };
            // The frame, and room to slide it, must all have arrived.
            if start + self.frame + search > self.held.len() / 2 {
                break;
            }
            let from = if self.tail.is_empty() {
                start
            } else {
                self.best_offset(start, search)
            };
            self.lay_down(from, out);
            self.tail.clear();
            self.tail
                .extend_from_slice(&self.held[(from + hop) * 2..(from + self.frame) * 2]);
            let step =
                u64::try_from(hop).unwrap_or(0) * u64::from(self.speed_percent) * 65_536 / 100;
            self.cursor += step;
        }
        // Drop input before the search window of the next frame.
        let next = usize::try_from((self.cursor >> 16).saturating_sub(self.base)).unwrap_or(0);
        let drop = next.saturating_sub(self.search);
        if drop > 0 {
            let drop = drop.min(self.held.len() / 2);
            self.held.drain(..drop * 2);
            self.base += drop as u64;
        }
    }

    /// The frame start in `start - search ..= start + search` whose first
    /// `overlap` frames best continue the tail.
    fn best_offset(&self, start: usize, search: usize) -> usize {
        let low = start.saturating_sub(search);
        let high = start + search;
        let score = |at: usize| -> i64 { self.similarity(at) };
        let mut best = start;
        let mut best_score = i64::MIN;
        let mut at = low;
        while at <= high {
            let value = score(at);
            if value > best_score {
                best_score = value;
                best = at;
            }
            at += COARSE_STEP;
        }
        let fine_low = best.saturating_sub(COARSE_STEP - 1).max(low);
        let fine_high = (best + COARSE_STEP - 1).min(high);
        for at in fine_low..=fine_high {
            let value = score(at);
            if value > best_score {
                best_score = value;
                best = at;
            }
        }
        best
    }

    /// Cross-correlation of the tail with the input at `at`, on the mono
    /// mix, every other sample. Enough to find the matching cycle, at a
    /// quarter of the cost of the full thing.
    fn similarity(&self, at: usize) -> i64 {
        let mut total = 0_i64;
        let mut index = 0;
        while index < self.overlap {
            let tail = i64::from(self.tail[index * 2]) + i64::from(self.tail[index * 2 + 1]);
            let held =
                i64::from(self.held[(at + index) * 2]) + i64::from(self.held[(at + index) * 2 + 1]);
            total += tail * held;
            index += 2;
        }
        total
    }

    /// Fades the frame at `from` in over the tail, then copies the rest of
    /// its hop.
    fn lay_down(&self, from: usize, out: &mut Vec<i16>) {
        let hop = self.frame - self.overlap;
        let overlap = i64::try_from(self.overlap).unwrap_or(1);
        for index in 0..hop {
            for channel in 0..2 {
                let fresh = i64::from(self.held[(from + index) * 2 + channel]);
                let sample = if index < self.overlap && !self.tail.is_empty() {
                    let old = i64::from(self.tail[index * 2 + channel]);
                    let weight = i64::try_from(index).unwrap_or(0);
                    (old * (overlap - weight) + fresh * weight) / overlap
                } else {
                    fresh
                };
                out.push(
                    i16::try_from(sample.clamp(i64::from(i16::MIN), i64::from(i16::MAX)))
                        .expect("the sample was clamped to i16"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Stretcher;

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
                let sample = (phase.sin() * 16_000.0) as i16;
                [sample, sample]
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss, reason = "counts, not samples")]
    fn crossings_per_second(samples: &[i16], rate: u32) -> f64 {
        let left = samples.iter().step_by(2).collect::<Vec<_>>();
        let crossings = left
            .windows(2)
            .filter(|pair| (*pair[0] < 0) != (*pair[1] < 0))
            .count();
        crossings as f64 * f64::from(rate) / left.len() as f64
    }

    fn run(speed: u16, input: &[i16]) -> Vec<i16> {
        let mut stretcher = Stretcher::new(48_000);
        stretcher.set_speed(speed);
        let mut out = Vec::new();
        for chunk in input.chunks(4_800) {
            stretcher.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn normal_speed_is_the_sound_itself() {
        let input = sine(48_000, 200, 9_600);
        assert_eq!(run(100, &input), input);
    }

    #[test]
    fn faster_is_shorter_and_slower_is_longer_at_the_same_pitch() {
        let input = sine(48_000, 220, 48_000 * 2);
        for (speed, expected) in [(150, 64_000), (200, 48_000), (75, 128_000), (50, 192_000)] {
            let out = run(speed, &input);
            let frames = out.len() / 2;
            assert!(
                (expected - 3_000..=expected + 1_000).contains(&frames),
                "{speed}%: {frames} frames, expected about {expected}"
            );
            let pitch = crossings_per_second(&out, 48_000) / 2.0;
            assert!((205.0..235.0).contains(&pitch), "{speed}%: {pitch} Hz");
        }
    }

    #[test]
    fn joins_fall_on_matching_cycles() {
        // At the join of two frames the waveform must not jump: the largest
        // step between neighbouring samples should stay close to the tone's
        // own largest step.
        let input = sine(48_000, 180, 48_000);
        let out = run(140, &input);
        let left = out.iter().step_by(2).copied().collect::<Vec<_>>();
        let largest = left
            .windows(2)
            .map(|pair| (i32::from(pair[1]) - i32::from(pair[0])).abs())
            .max()
            .unwrap_or(0);
        // The tone's own largest step is amplitude times angular frequency
        // per sample: 16000 * 2 * pi * 180 / 48000, a little under 377.
        let natural = 377;
        assert!(
            largest <= natural * 2,
            "largest step {largest}, natural {natural}"
        );
    }

    #[test]
    fn speed_is_clamped_to_what_sounds_acceptable() {
        let mut stretcher = Stretcher::new(48_000);
        stretcher.set_speed(10);
        assert_eq!(stretcher.speed_percent(), 50);
        stretcher.set_speed(900);
        assert_eq!(stretcher.speed_percent(), 300);
    }
}
