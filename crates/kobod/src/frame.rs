#![forbid(unsafe_code)]
// This module is compiled by both the device runtime and simulator. Each uses
// a different half of the diagnostic API, so neither crate uses every method.
#![allow(dead_code)]

//! Panel update planning shared by the device runtime and simulator.

use kobo_ui::{tone, Rect, Surface};

pub const PANEL_CLEAN_INTERVAL: u32 = 8;

/// The physical update strategy selected from a frame's changed pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PanelWaveform {
    /// Fast, two-level feedback for changes containing only black and white.
    Du,
    /// Sixteen-level partial refresh for text and images containing grey.
    Gl16,
    /// Full sixteen-level refresh that clears accumulated residue.
    Gc16,
}

impl PanelWaveform {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Du => "DU",
            Self::Gl16 => "GL16",
            Self::Gc16 => "GC16",
        }
    }
}

/// One region the runtime will ask the panel controller to update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRegion {
    pub region: Rect,
    pub waveform: PanelWaveform,
}

/// One logical frame, which may use several panel regions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameTransition {
    /// The box containing every region, retained for diagnostics.
    pub region: Rect,
    /// The slowest waveform used by any region, retained for diagnostics.
    pub waveform: PanelWaveform,
    pub regions: Vec<FrameRegion>,
    pub full: bool,
    /// One-based number of the refresh in this session.
    pub refresh: u64,
    /// Pixels that will have been repainted since the last cleaning refresh,
    /// once this transition has been applied. Zero directly after a clean.
    pub dirty: u64,
    exact_damage: bool,
}

/// Nearby changes are cheaper as one panel update than as many tiny updates.
const DAMAGE_JOIN_GAP: i32 = 8;
/// Prevents one frame from exhausting either the runtime or controller queue.
const MAX_FRAME_REGIONS: usize = 8;
/// Shortest side of one partial panel update.
///
/// Update regions a single pixel wide or tall have been observed to upset
/// e-ink controller drivers, up to soft-locking some of them. Every planned
/// region keeps both sides at least this long, growing over neighbouring
/// unchanged pixels when the damage itself is thinner.
const MIN_REGION_SIDE: i32 = 2;
/// Every planned region starts and ends on this pixel grid.
///
/// Panel controllers work on pixel groups rather than single pixels, and a
/// region whose edge falls inside a group has been seen to come up one pixel
/// short of the requested edge, leaving a stale seam. Snapping both axes to
/// the grid removes the seam whichever way the panel is rotated, and makes
/// neighbouring regions overlap so they merge into one update instead of two
/// updates the controller has to serialise.
const REGION_ALIGNMENT: i32 = 8;

/// Shared state machine for choosing Kobo panel transitions.
///
/// The device runtime and simulator both use this exact planner. Physics such
/// as visible residue remains a simulator approximation, but the changed
/// rectangle, waveform and cleaning cadence cannot drift from the device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FramePlanner {
    width: usize,
    height: usize,
    previous: Vec<u8>,
    dirty: u64,
    refreshes: u64,
    started: bool,
}

impl FramePlanner {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            previous: vec![tone::INK; width.saturating_mul(height)],
            dirty: 0,
            refreshes: 0,
            started: false,
        }
    }

    /// Plans the next update without changing planner state.
    ///
    /// Returning `None` means the surface is the wrong size or no pixel has
    /// changed. Call [`Self::commit`] only after the update succeeds.
    #[must_use]
    pub fn plan(&self, surface: &Surface) -> Option<FrameTransition> {
        if !self.matches(surface) {
            return None;
        }
        let whole = self.whole()?;
        let (regions, dirty) = if self.started {
            let (changed, flipped) = self.changed_regions(surface)?;
            // The budget is checked before this update is added to it, so that
            // a full panel's worth of repainting still buys exactly
            // PANEL_CLEAN_INTERVAL updates before anything flashes, as it did
            // when updates rather than pixels were being counted.
            if self.dirty >= self.clean_after() {
                (
                    vec![FrameRegion {
                        region: whole,
                        waveform: PanelWaveform::Gc16,
                    }],
                    0,
                )
            } else {
                (
                    changed
                        .into_iter()
                        .map(|region| FrameRegion {
                            region,
                            waveform: if self.two_level_transition(surface, region) {
                                PanelWaveform::Du
                            } else {
                                PanelWaveform::Gl16
                            },
                        })
                        .collect(),
                    self.dirty.saturating_add(flipped),
                )
            }
        } else {
            (
                vec![FrameRegion {
                    region: whole,
                    waveform: PanelWaveform::Gc16,
                }],
                0,
            )
        };
        self.transition(regions, dirty, false)
    }

    /// Plans an update for a region whose pixels and waveform are already
    /// known by the caller.
    ///
    /// This path does not compare the full surface. It is intended for direct
    /// interaction feedback where the renderer has just changed one known
    /// rectangle. The ordinary planner remains responsible for application
    /// frames whose damage is not known in advance.
    #[must_use]
    pub fn plan_damage(
        &self,
        surface: &Surface,
        damage: Rect,
        waveform: PanelWaveform,
    ) -> Option<FrameTransition> {
        if !self.matches(surface) {
            return None;
        }
        let whole = self.whole()?;
        if !self.started || self.dirty >= self.clean_after() {
            return self.transition(
                vec![FrameRegion {
                    region: whole,
                    waveform: PanelWaveform::Gc16,
                }],
                0,
                false,
            );
        }
        let region = Self::align_to_panel(damage.intersection(whole)?, whole);
        // The waveform still follows the pixels themselves. A two-level
        // waveform over antialiased grey would crush edges that no later
        // frame comparison could repair, because committing this update
        // marks the rectangle current.
        let waveform =
            if waveform == PanelWaveform::Du && !self.two_level_transition(surface, region) {
                PanelWaveform::Gl16
            } else {
                waveform
            };
        let area = u64::try_from(region.width)
            .ok()?
            .saturating_mul(u64::try_from(region.height).ok()?);
        self.transition(
            vec![FrameRegion { region, waveform }],
            self.dirty.saturating_add(area),
            true,
        )
    }

    /// Records a successfully applied transition.
    pub fn commit(&mut self, surface: &Surface, transition: &FrameTransition) -> bool {
        if !self.matches(surface) || transition.refresh != self.refreshes.saturating_add(1) {
            return false;
        }
        if transition.exact_damage {
            for update in &transition.regions {
                if !Self::copy_region(
                    self.width,
                    self.height,
                    &mut self.previous,
                    &surface.pixels,
                    update.region,
                ) {
                    return false;
                }
            }
        } else {
            self.previous.copy_from_slice(&surface.pixels);
        }
        self.dirty = transition.dirty;
        self.refreshes = transition.refresh;
        self.started = true;
        true
    }

    #[must_use]
    pub const fn refreshes(&self) -> u64 {
        self.refreshes
    }

    #[must_use]
    /// Pixels repainted since the last cleaning refresh.
    pub const fn dirty(&self) -> u64 {
        self.dirty
    }

    fn matches(&self, surface: &Surface) -> bool {
        surface.width == self.width
            && surface.height == self.height
            && surface.pixels.len() == self.previous.len()
    }

    fn whole(&self) -> Option<Rect> {
        Some(Rect {
            x: 0,
            y: 0,
            width: i32::try_from(self.width).ok()?,
            height: i32::try_from(self.height).ok()?,
        })
    }

    fn transition(
        &self,
        regions: Vec<FrameRegion>,
        dirty: u64,
        exact_damage: bool,
    ) -> Option<FrameTransition> {
        let region = Self::bounding_region(&regions)?;
        let waveform = regions
            .iter()
            .map(|update| update.waveform)
            .max_by_key(|waveform| match waveform {
                PanelWaveform::Du => 0,
                PanelWaveform::Gl16 => 1,
                PanelWaveform::Gc16 => 2,
            })?;
        Some(FrameTransition {
            region,
            waveform,
            full: waveform == PanelWaveform::Gc16,
            regions,
            refresh: self.refreshes.saturating_add(1),
            dirty,
            exact_damage,
        })
    }

    /// Changed regions and the exact number of pixels that changed.
    ///
    /// Whole-row slice comparisons find the vertical bounds using the library
    /// comparison path. Pixel scanning then runs only on rows that differ.
    fn changed_regions(&self, surface: &Surface) -> Option<(Vec<Rect>, u64)> {
        let current_rows = surface.pixels.chunks_exact(self.width);
        let previous_rows = self.previous.chunks_exact(self.width);
        let top = current_rows
            .clone()
            .zip(previous_rows.clone())
            .position(|(current, previous)| current != previous)?;
        let bottom = current_rows
            .clone()
            .zip(previous_rows.clone())
            .rposition(|(current, previous)| current != previous)?;
        let mut regions = Vec::new();
        let mut flipped = 0_u64;
        for y in top..=bottom {
            let current = surface.pixels.get(y * self.width..(y + 1) * self.width)?;
            let previous = self.previous.get(y * self.width..(y + 1) * self.width)?;
            if current == previous {
                continue;
            }
            flipped = flipped.saturating_add(
                current
                    .iter()
                    .zip(previous)
                    .filter(|(now, before)| now != before)
                    .count() as u64,
            );
            let mut x = 0;
            while x < self.width {
                let Some(relative) = current[x..]
                    .iter()
                    .zip(&previous[x..])
                    .position(|(now, before)| now != before)
                else {
                    break;
                };
                let start = x + relative;
                let mut last_changed = start;
                let mut cursor = start + 1;
                while cursor < self.width {
                    if current[cursor] != previous[cursor] {
                        last_changed = cursor;
                    } else if cursor.saturating_sub(last_changed) > DAMAGE_JOIN_GAP as usize {
                        break;
                    }
                    cursor += 1;
                }
                Self::merge_region(
                    &mut regions,
                    Rect {
                        x: i32::try_from(start).ok()?,
                        y: i32::try_from(y).ok()?,
                        width: i32::try_from(last_changed - start + 1).ok()?,
                        height: 1,
                    },
                );
                x = last_changed + 1;
            }
        }
        Self::limit_regions(&mut regions);
        let whole = self.whole()?;
        // Snapping to the grid can make regions touch that were apart before,
        // so they are merged once more rather than submitted overlapping.
        let mut aligned = Vec::with_capacity(regions.len());
        for region in regions {
            Self::merge_region(&mut aligned, Self::align_to_panel(region, whole));
        }
        aligned.sort_by_key(|region| (region.y, region.x));
        Some((aligned, flipped))
    }

    /// Snaps a region's edges outward to the [`REGION_ALIGNMENT`] grid and
    /// grows it to [`MIN_REGION_SIDE`], staying inside `whole`. The extra
    /// pixels are unchanged and merely repainted.
    fn align_to_panel(region: Rect, whole: Rect) -> Rect {
        let far_x = whole.x.saturating_add(whole.width);
        let far_y = whole.y.saturating_add(whole.height);
        let left = Self::snap_down(region.x).max(whole.x);
        let top = Self::snap_down(region.y).max(whole.y);
        let right = Self::snap_up(region.x.saturating_add(region.width)).min(far_x);
        let bottom = Self::snap_up(region.y.saturating_add(region.height)).min(far_y);
        Self::widen_to_minimum(
            Rect {
                x: left,
                y: top,
                width: right.saturating_sub(left),
                height: bottom.saturating_sub(top),
            },
            whole,
        )
    }

    fn snap_down(edge: i32) -> i32 {
        edge.saturating_sub(edge.rem_euclid(REGION_ALIGNMENT))
    }

    fn snap_up(edge: i32) -> i32 {
        let over = edge.rem_euclid(REGION_ALIGNMENT);
        if over == 0 {
            edge
        } else {
            edge.saturating_add(REGION_ALIGNMENT - over)
        }
    }

    /// Grows a thin region to [`MIN_REGION_SIDE`] on each side, staying
    /// inside `whole`. The extra pixels are unchanged and merely repainted.
    fn widen_to_minimum(region: Rect, whole: Rect) -> Rect {
        let mut region = region;
        if region.width < MIN_REGION_SIDE && whole.width >= MIN_REGION_SIDE {
            region.width = MIN_REGION_SIDE;
            region.x = region
                .x
                .min(whole.x.saturating_add(whole.width) - MIN_REGION_SIDE);
        }
        if region.height < MIN_REGION_SIDE && whole.height >= MIN_REGION_SIDE {
            region.height = MIN_REGION_SIDE;
            region.y = region
                .y
                .min(whole.y.saturating_add(whole.height) - MIN_REGION_SIDE);
        }
        region
    }

    fn merge_region(regions: &mut Vec<Rect>, mut incoming: Rect) {
        let mut index = 0;
        while index < regions.len() {
            if Self::regions_are_near(regions[index], incoming) {
                incoming = Self::union(regions.swap_remove(index), incoming);
                index = 0;
            } else {
                index += 1;
            }
        }
        regions.push(incoming);
    }

    fn regions_are_near(left: Rect, right: Rect) -> bool {
        let horizontal_gap = if left.x.saturating_add(left.width) < right.x {
            right.x.saturating_sub(left.x.saturating_add(left.width))
        } else if right.x.saturating_add(right.width) < left.x {
            left.x.saturating_sub(right.x.saturating_add(right.width))
        } else {
            0
        };
        let vertical_gap = if left.y.saturating_add(left.height) < right.y {
            right.y.saturating_sub(left.y.saturating_add(left.height))
        } else if right.y.saturating_add(right.height) < left.y {
            left.y.saturating_sub(right.y.saturating_add(right.height))
        } else {
            0
        };
        horizontal_gap <= DAMAGE_JOIN_GAP && vertical_gap <= DAMAGE_JOIN_GAP
    }

    fn union(left: Rect, right: Rect) -> Rect {
        let x = left.x.min(right.x);
        let y = left.y.min(right.y);
        let far_x = left
            .x
            .saturating_add(left.width)
            .max(right.x.saturating_add(right.width));
        let far_y = left
            .y
            .saturating_add(left.height)
            .max(right.y.saturating_add(right.height));
        Rect {
            x,
            y,
            width: far_x.saturating_sub(x),
            height: far_y.saturating_sub(y),
        }
    }

    fn bounding_region(regions: &[FrameRegion]) -> Option<Rect> {
        regions
            .iter()
            .map(|update| update.region)
            .reduce(Self::union)
    }

    fn limit_regions(regions: &mut Vec<Rect>) {
        while regions.len() > MAX_FRAME_REGIONS {
            let mut best = None;
            for left in 0..regions.len() {
                for right in left + 1..regions.len() {
                    let union = Self::union(regions[left], regions[right]);
                    let cost = Self::area(union)
                        .saturating_sub(Self::area(regions[left]))
                        .saturating_sub(Self::area(regions[right]));
                    if best.is_none_or(|(_, _, best_cost)| cost < best_cost) {
                        best = Some((left, right, cost));
                    }
                }
            }
            let Some((left, right, _)) = best else {
                break;
            };
            let combined = Self::union(regions[left], regions[right]);
            regions.swap_remove(right);
            regions.swap_remove(left);
            regions.push(combined);
        }
    }

    fn area(region: Rect) -> u64 {
        u64::try_from(region.width)
            .unwrap_or(0)
            .saturating_mul(u64::try_from(region.height).unwrap_or(0))
    }

    fn copy_region(
        width: usize,
        height: usize,
        destination: &mut [u8],
        source: &[u8],
        region: Rect,
    ) -> bool {
        let Some(region) = region.intersection(Rect {
            x: 0,
            y: 0,
            width: i32::try_from(width).unwrap_or(i32::MAX),
            height: i32::try_from(height).unwrap_or(i32::MAX),
        }) else {
            return false;
        };
        let (Ok(x), Ok(y), Ok(region_width), Ok(region_height)) = (
            usize::try_from(region.x),
            usize::try_from(region.y),
            usize::try_from(region.width),
            usize::try_from(region.height),
        ) else {
            return false;
        };
        for row in y..y.saturating_add(region_height) {
            let start = row.saturating_mul(width).saturating_add(x);
            let end = start.saturating_add(region_width);
            let (Some(to), Some(from)) = (destination.get_mut(start..end), source.get(start..end))
            else {
                return false;
            };
            to.copy_from_slice(from);
        }
        true
    }

    /// Changed pixels that may accumulate before the panel is cleaned.
    fn clean_after(&self) -> u64 {
        (self.width as u64)
            .saturating_mul(self.height as u64)
            .saturating_mul(u64::from(PANEL_CLEAN_INTERVAL))
    }

    /// Whether every pixel in `region` is black or white both before and
    /// after this frame.
    ///
    /// A two-level waveform is only clean for a two-level transition. Its
    /// drive is tuned for black-to-white and white-to-black; started from a
    /// grey it stops short and leaves the grey's outline behind, which is how
    /// erased antialiased text reads as a faint copy of the old screen.
    /// Checking only the new pixels missed exactly that case.
    fn two_level_transition(&self, surface: &Surface, region: Rect) -> bool {
        !Self::has_grey_in(&self.previous, self.width, region)
            && !Self::has_grey_in(&surface.pixels, surface.width, region)
    }

    fn has_grey_in(pixels: &[u8], width: usize, region: Rect) -> bool {
        let Ok(left) = usize::try_from(region.x) else {
            return false;
        };
        let Ok(top) = usize::try_from(region.y) else {
            return false;
        };
        let Ok(region_width) = usize::try_from(region.width) else {
            return false;
        };
        let Ok(height) = usize::try_from(region.height) else {
            return false;
        };
        (top..top.saturating_add(height)).any(|y| {
            let start = y.saturating_mul(width).saturating_add(left);
            let end = start.saturating_add(region_width);
            pixels
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .any(|tone| *tone != tone::INK && *tone != tone::PAPER)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FramePlanner, PanelWaveform, MAX_FRAME_REGIONS, MIN_REGION_SIDE};
    use kobo_ui::{tone, Rect, Surface};

    fn started(width: usize, height: usize) -> (FramePlanner, Surface) {
        let mut planner = FramePlanner::new(width, height);
        let frame = Surface::new(width, height);
        let first = planner.plan(&frame).expect("first frame");
        assert!(first.full);
        assert!(planner.commit(&frame, &first));
        (planner, frame)
    }

    #[test]
    fn erasing_grey_is_not_driven_two_level() {
        // Antialiased text on the panel, then the same area cleared to paper.
        // The new pixels are pure paper, but the transition starts from grey,
        // and a two-level waveform from grey leaves the old text behind.
        let (mut planner, mut frame) = started(32, 32);
        for x in 4..12 {
            frame.pixels[8 * 32 + x] = 128;
        }
        let text = planner.plan(&frame).expect("text appears");
        assert_eq!(text.regions[0].waveform, PanelWaveform::Gl16);
        assert!(planner.commit(&frame, &text));

        for x in 4..12 {
            frame.pixels[8 * 32 + x] = tone::PAPER;
        }
        let erased = planner.plan(&frame).expect("text erased");
        assert_eq!(erased.regions.len(), 1);
        assert_eq!(
            erased.regions[0].waveform,
            PanelWaveform::Gl16,
            "clearing grey must use a sixteen-level waveform"
        );

        // Known damage takes the same rule: an inverted control whose old
        // pixels held grey is not driven two-level on release.
        let feedback = planner
            .plan_damage(
                &frame,
                Rect {
                    x: 4,
                    y: 8,
                    width: 8,
                    height: 1,
                },
                PanelWaveform::Du,
            )
            .expect("feedback over previously grey pixels");
        assert_eq!(feedback.regions[0].waveform, PanelWaveform::Gl16);
    }

    #[test]
    fn a_black_and_white_transition_stays_two_level() {
        let (mut planner, mut frame) = started(32, 32);
        frame.pixels[8 * 32 + 8] = tone::INK;
        let drawn = planner.plan(&frame).expect("ink appears");
        assert_eq!(drawn.regions[0].waveform, PanelWaveform::Du);
        assert!(planner.commit(&frame, &drawn));
        frame.pixels[8 * 32 + 8] = tone::PAPER;
        let cleared = planner.plan(&frame).expect("ink erased");
        assert_eq!(cleared.regions[0].waveform, PanelWaveform::Du);
    }

    #[test]
    fn distant_changes_remain_separate_regions() {
        let (planner, mut frame) = started(32, 32);
        frame.pixels[0] = tone::INK;
        frame.pixels[32 * 32 - 1] = tone::INK;
        let update = planner.plan(&frame).expect("two corners");
        assert_eq!(update.regions.len(), 2);
        assert_eq!(update.region.width, 32);
        assert_eq!(update.region.height, 32);
        assert_eq!(update.dirty, 2);
    }

    #[test]
    fn known_damage_does_not_hide_changes_elsewhere() {
        let (mut planner, mut frame) = started(32, 32);
        frame.pixels[4 * 32 + 4] = tone::INK;
        frame.pixels[28 * 32 + 28] = tone::INK;
        let feedback = planner
            .plan_damage(
                &frame,
                Rect {
                    x: 2,
                    y: 2,
                    width: 6,
                    height: 6,
                },
                PanelWaveform::Du,
            )
            .expect("feedback");
        assert!(planner.commit(&frame, &feedback));
        assert_eq!(planner.plan(&frame).expect("remaining").region.x, 24);
    }

    #[test]
    fn known_damage_containing_grey_is_not_driven_two_level() {
        let (planner, mut frame) = started(32, 32);
        frame.pixels[4 * 32 + 4] = tone::INK;
        frame.pixels[4 * 32 + 5] = 128;
        let damage = Rect {
            x: 2,
            y: 2,
            width: 6,
            height: 6,
        };
        let feedback = planner
            .plan_damage(&frame, damage, PanelWaveform::Du)
            .expect("feedback");
        assert_eq!(feedback.regions[0].waveform, PanelWaveform::Gl16);
        assert_eq!(feedback.waveform, PanelWaveform::Gl16);
        assert!(!feedback.full);

        frame.pixels[4 * 32 + 5] = tone::PAPER;
        let pure = planner
            .plan_damage(&frame, damage, PanelWaveform::Du)
            .expect("feedback");
        assert_eq!(pure.regions[0].waveform, PanelWaveform::Du);
    }

    #[test]
    fn one_frame_has_a_bounded_number_of_regions() {
        let (planner, mut frame) = started(256, 32);
        for index in 0..16 {
            frame.pixels[index * 16] = tone::INK;
        }
        let update = planner.plan(&frame).expect("separated changes");
        assert!(update.regions.len() <= MAX_FRAME_REGIONS);
        assert_eq!(update.dirty, 16);
    }

    #[test]
    fn no_region_is_a_single_pixel_wide_or_tall() {
        let (planner, mut frame) = started(32, 32);
        frame.pixels[16 * 32 + 16] = tone::INK;
        let update = planner.plan(&frame).expect("one pixel");
        let region = update.regions[0].region;
        assert!(region.width >= MIN_REGION_SIDE);
        assert!(region.height >= MIN_REGION_SIDE);
        assert!(region.x <= 16 && 16 < region.x + region.width);
        assert!(region.y <= 16 && 16 < region.y + region.height);

        let feedback = planner
            .plan_damage(
                &frame,
                Rect {
                    x: 8,
                    y: 8,
                    width: 1,
                    height: 1,
                },
                PanelWaveform::Du,
            )
            .expect("thin damage");
        let region = feedback.regions[0].region;
        assert!(region.width >= MIN_REGION_SIDE);
        assert!(region.height >= MIN_REGION_SIDE);
    }

    #[test]
    fn an_aligned_region_stays_inside_the_screen() {
        let (planner, mut frame) = started(32, 32);
        frame.pixels[32 * 32 - 1] = tone::INK;
        let update = planner.plan(&frame).expect("corner pixel");
        let region = update.regions[0].region;
        assert_eq!(
            region,
            Rect {
                x: 24,
                y: 24,
                width: 8,
                height: 8,
            }
        );

        // A panel whose size is not a grid multiple clamps the last region
        // to the edge instead of running past it.
        let (planner, mut frame) = started(30, 30);
        frame.pixels[30 * 30 - 1] = tone::INK;
        let update = planner.plan(&frame).expect("corner pixel");
        let region = update.regions[0].region;
        assert_eq!(
            region,
            Rect {
                x: 24,
                y: 24,
                width: 6,
                height: 6,
            }
        );
    }

    #[test]
    fn regions_start_and_end_on_the_panel_grid() {
        let (planner, mut frame) = started(64, 64);
        frame.pixels[21 * 64 + 13] = tone::INK;
        let update = planner.plan(&frame).expect("one pixel");
        assert_eq!(
            update.regions[0].region,
            Rect {
                x: 8,
                y: 16,
                width: 8,
                height: 8,
            }
        );

        let feedback = planner
            .plan_damage(
                &frame,
                Rect {
                    x: 3,
                    y: 5,
                    width: 10,
                    height: 20,
                },
                PanelWaveform::Du,
            )
            .expect("known damage");
        assert_eq!(
            feedback.regions[0].region,
            Rect {
                x: 0,
                y: 0,
                width: 16,
                height: 32,
            }
        );
    }

    #[test]
    fn regions_that_touch_after_alignment_become_one_update() {
        // Two changes a little over the join gap apart stay separate as raw
        // damage, but snapping each to the grid makes them meet.
        let (planner, mut frame) = started(64, 16);
        frame.pixels[4 * 64 + 7] = tone::INK;
        frame.pixels[4 * 64 + 17] = tone::INK;
        let update = planner.plan(&frame).expect("two pixels");
        assert_eq!(update.regions.len(), 1);
        assert_eq!(update.regions[0].region.x, 0);
        assert_eq!(update.regions[0].region.width, 24);
        assert_eq!(update.dirty, 2);
    }
}
