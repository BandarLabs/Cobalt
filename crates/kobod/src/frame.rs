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
                            waveform: if Self::has_grey(surface, region) {
                                PanelWaveform::Gl16
                            } else {
                                PanelWaveform::Du
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
        let region = damage.intersection(whole)?;
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
        regions.sort_by_key(|region| (region.y, region.x));
        Some((regions, flipped))
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

    fn has_grey(surface: &Surface, region: Rect) -> bool {
        let Ok(left) = usize::try_from(region.x) else {
            return false;
        };
        let Ok(top) = usize::try_from(region.y) else {
            return false;
        };
        let Ok(width) = usize::try_from(region.width) else {
            return false;
        };
        let Ok(height) = usize::try_from(region.height) else {
            return false;
        };
        (top..top.saturating_add(height)).any(|y| {
            let start = y.saturating_mul(surface.width).saturating_add(left);
            let end = start.saturating_add(width);
            surface
                .pixels
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .any(|tone| *tone != tone::INK && *tone != tone::PAPER)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FramePlanner, PanelWaveform, MAX_FRAME_REGIONS};
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
        assert_eq!(planner.plan(&frame).expect("remaining").region.x, 28);
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
}
