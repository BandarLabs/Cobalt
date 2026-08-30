#![forbid(unsafe_code)]

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, KoboApp, PictureHandle, ScreenBuilder, TilePicture,
};
use std::process::ExitCode;

const PATTERN_WIDTH: u32 = 600;
const PATTERN_HEIGHT: u32 = 480;

struct PatternDef {
    title: &'static str,
    subtitle: &'static str,
    description: &'static str,
    generator: fn() -> Vec<u8>,
}

// -----------------------------------------------------------------------------
// Pattern 1: Philips PM5544 / SMPTE Style Broadcast TV Color Test Card
// -----------------------------------------------------------------------------
fn gen_pm5544() -> Vec<u8> {
    let w = PATTERN_WIDTH;
    let h = PATTERN_HEIGHT;
    let mut pixels = vec![0u8; (w * h) as usize];
    let cx = (w as f32) / 2.0;
    let cy = (h as f32) / 2.0;
    let r = ((w.min(h)) as f32) * 0.44;
    let border_size = 20;
    let bar_size = 20;

    for y in 0..h {
        let dy = (y as f32) - cy;
        let row_offset = (y * w) as usize;
        for x in 0..w {
            let dx = (x as f32) - cx;
            let dist_sq = dx * dx + dy * dy;
            let sub_phase = (x + y) % 3;

            // Outer border checkerboard
            if x < border_size || x >= w - border_size || y < border_size || y >= h - border_size {
                let check = ((x / border_size) + (y / border_size)) % 2 == 0;
                pixels[row_offset + x as usize] = if check { 0x00 } else { 0xFF };
                continue;
            }

            if dist_sq <= r * r {
                let norm_y = dy / r;
                let norm_x = dx / r;

                // 1. Top castellations
                if norm_y < -0.65 {
                    let bar = (x / bar_size) % 2 == 0;
                    pixels[row_offset + x as usize] = if bar { 0x00 } else { 0xFF };
                }
                // 2. 6 Color Bars: Yellow, Cyan, Green, Magenta, Red, Blue
                else if norm_y < -0.15 {
                    let bar_idx = (((norm_x + 1.0) * 3.0) as i32).clamp(0, 5);
                    let val = match bar_idx {
                        0 => if sub_phase != 2 { 0x00 } else { 0xFF }, // Yellow (R+G)
                        1 => if sub_phase != 0 { 0x00 } else { 0xFF }, // Cyan (G+B)
                        2 => if sub_phase == 1 { 0x00 } else { 0xFF }, // Green (G)
                        3 => if sub_phase != 1 { 0x00 } else { 0xFF }, // Magenta (R+B)
                        4 => if sub_phase == 0 { 0x00 } else { 0xFF }, // Red (R)
                        _ => if sub_phase == 2 { 0x00 } else { 0xFF }, // Blue (B)
                    };
                    pixels[row_offset + x as usize] = val;
                }
                // 3. Center black bar with white crosshair
                else if norm_y < 0.10 {
                    let is_cross = dx.abs() < 4.0 || dy.abs() < 4.0;
                    pixels[row_offset + x as usize] = if is_cross { 0xFF } else { 0x00 };
                }
                // 4. Color spectrum gradient
                else if norm_y < 0.35 {
                    let pos = ((norm_x + 1.0) / 2.0).clamp(0.0, 1.0);
                    let hue = pos * 300.0;
                    let val = if hue < 60.0 {
                        if sub_phase != 2 { 0x00 } else { 0xFF }
                    } else if hue < 120.0 {
                        if sub_phase == 1 || (sub_phase == 0 && (x % 2 == 0)) { 0x00 } else { 0xFF }
                    } else if hue < 180.0 {
                        if sub_phase != 0 { 0x00 } else { 0xFF }
                    } else if hue < 240.0 {
                        if sub_phase == 2 || (sub_phase == 1 && (x % 2 == 0)) { 0x00 } else { 0xFF }
                    } else {
                        if sub_phase != 1 { 0x00 } else { 0xFF }
                    };
                    pixels[row_offset + x as usize] = val;
                }
                // 5. Multiburst frequency lines
                else if norm_y < 0.55 {
                    let band = (((norm_x + 1.0) * 3.0) as u32).clamp(0, 5);
                    let pitch = (5 - band).max(1);
                    let line = ((x / pitch) % 2) == 0;
                    pixels[row_offset + x as usize] = if line { 0x00 } else { 0xFF };
                }
                // 6. Grayscale 6-step wedge
                else if norm_y < 0.80 {
                    let step = (((norm_x + 1.0) * 3.0) as u32).clamp(0, 5);
                    pixels[row_offset + x as usize] = (step * 51) as u8;
                }
                // 7. Bottom yellow baseline & red center marker
                else {
                    let val = if dx.abs() < 16.0 {
                        if sub_phase == 0 { 0x00 } else { 0xFF } // Center red
                    } else {
                        if sub_phase != 2 { 0x00 } else { 0xFF } // Yellow baseline
                    };
                    pixels[row_offset + x as usize] = val;
                }
            } else {
                let r_corner = 36.0;
                let corners = [
                    (70.0, 70.0),
                    (w as f32 - 70.0, 70.0),
                    (70.0, h as f32 - 70.0),
                    (w as f32 - 70.0, h as f32 - 70.0),
                ];
                let mut in_corner = false;
                for (c_idx, (cx_c, cy_c)) in corners.iter().enumerate() {
                    let cdx = (x as f32) - cx_c;
                    let cdy = (y as f32) - cy_c;
                    if cdx * cdx + cdy * cdy <= r_corner * r_corner {
                        in_corner = true;
                        let is_line = cdx.abs() < 2.0 || cdy.abs() < 2.0;
                        let is_dark = c_idx == 0 || c_idx == 3;
                        pixels[row_offset + x as usize] = if is_dark {
                            if is_line { 0xFF } else { 0x00 }
                        } else {
                            if is_line { 0x00 } else { 0xFF }
                        };
                        break;
                    }
                }
                if in_corner {
                    continue;
                }

                let left_x0 = 110;
                let left_x1 = 145;
                let right_x0 = w - 145;
                let right_x1 = w - 110;
                let side_y0 = 110;
                let side_y1 = h - 110;

                if (left_x0..=left_x1).contains(&x) && (side_y0..=side_y1).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase == 2 { 0x00 } else { 0xFF }; // Blue
                } else if (right_x0..=right_x1).contains(&x) && (side_y0..=side_y1).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase != 2 { 0x00 } else { 0xFF }; // Yellow
                } else if (left_x0..=170).contains(&x) && (30..=75).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase != 1 { 0x00 } else { 0xFF }; // Top-left Magenta
                } else if (w - 170..=right_x1).contains(&x) && (30..=75).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase == 1 { 0x00 } else { 0xFF }; // Top-right Green
                } else if (left_x0..=170).contains(&x) && (h - 75..=h - 30).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase != 1 { 0x00 } else { 0x88 }; // Bottom-left Purple
                } else if (w - 170..=right_x1).contains(&x) && (h - 75..=h - 30).contains(&y) {
                    pixels[row_offset + x as usize] = if sub_phase != 0 { 0x00 } else { 0xFF }; // Bottom-right Cyan
                } else {
                    let grid = (x % 25 == 0) || (y % 25 == 0);
                    pixels[row_offset + x as usize] = if grid { 0xFF } else { 0x99 };
                }
            }
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 2: Pure RGB Subpixel Filter Resonance
// -----------------------------------------------------------------------------
fn gen_rgb() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    let band_h = PATTERN_HEIGHT / 3;
    for y in 0..PATTERN_HEIGHT {
        let band = (y / band_h).min(2);
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let is_active = match band {
                0 => (x + y) % 3 == 0, // Red
                1 => (x + y) % 3 == 1, // Green
                _ => (x + y) % 3 == 2, // Blue
            };
            pixels[row_offset + x as usize] = if is_active { 0x00 } else { 0xFF };
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 3: CMYK & Primary Color Bars
// -----------------------------------------------------------------------------
fn gen_cmyk() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    let bar_count = 8;
    let bar_w = PATTERN_WIDTH / bar_count;
    for y in 0..PATTERN_HEIGHT {
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let bar = (x / bar_w).min(bar_count - 1);
            let sub_phase = (x + y) % 3;
            let val = match bar {
                0 => if sub_phase == 0 { 0x00 } else { 0xFF }, // Red
                1 => if sub_phase == 1 { 0x00 } else { 0xFF }, // Green
                2 => if sub_phase == 2 { 0x00 } else { 0xFF }, // Blue
                3 => if sub_phase != 2 { 0x00 } else { 0xFF }, // Yellow (R+G)
                4 => if sub_phase != 0 { 0x00 } else { 0xFF }, // Cyan (G+B)
                5 => if sub_phase != 1 { 0x00 } else { 0xFF }, // Magenta (R+B)
                6 => 0x00,                                      // Black
                _ => 0xFF,                                      // White
            };
            pixels[row_offset + x as usize] = val;
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 4: 4x4 Bayer Matrix Color Dither Grids
// -----------------------------------------------------------------------------
fn gen_bayer() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    const BAYER4: [u8; 16] = [
        0, 8, 2, 10,
        12, 4, 14, 6,
        3, 11, 1, 9,
        15, 7, 13, 5,
    ];
    let grid_w = PATTERN_WIDTH / 4;
    let grid_h = PATTERN_HEIGHT / 4;
    for y in 0..PATTERN_HEIGHT {
        let gy = (y / grid_h).min(3);
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let gx = (x / grid_w).min(3);
            let threshold = ((gx + gy * 4) as u8) * 16;
            let bayer_val = BAYER4[((y % 4) * 4 + (x % 4)) as usize] * 16;
            pixels[row_offset + x as usize] = if bayer_val < threshold { 0x00 } else { 0xFF };
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 5: Kaleido 3 Diagonal Frequency Alignment
// -----------------------------------------------------------------------------
fn gen_diagonal() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    let quadrant_w = PATTERN_WIDTH / 2;
    let quadrant_h = PATTERN_HEIGHT / 2;
    for y in 0..PATTERN_HEIGHT {
        let qy = if y < quadrant_h { 0 } else { 1 };
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let qx = if x < quadrant_w { 0 } else { 1 };
            let pitch = match (qx, qy) {
                (0, 0) => 1,
                (1, 0) => 2,
                (0, 1) => 3,
                _ => 4,
            };
            let stripe = ((x + y) / pitch) % 2 == 0;
            pixels[row_offset + x as usize] = if stripe { 0x00 } else { 0xFF };
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 6: Optical Color Mixing Swatches
// -----------------------------------------------------------------------------
fn gen_swatches() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    let cell_w = PATTERN_WIDTH / 3;
    let cell_h = PATTERN_HEIGHT / 2;
    for y in 0..PATTERN_HEIGHT {
        let cy = (y / cell_h).min(1);
        let in_cell_y = y % cell_h;
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let cx = (x / cell_w).min(2);
            let in_cell_x = x % cell_w;
            let border = in_cell_x < 6 || in_cell_y < 6 || in_cell_x >= cell_w - 6 || in_cell_y >= cell_h - 6;
            let index = cx + cy * 3;
            let sub_phase = (x + y) % 3;
            let val = if border {
                0x00
            } else {
                match index {
                    0 => if sub_phase == 0 { 0x11 } else { 0xEE },
                    1 => if sub_phase == 1 { 0x11 } else { 0xEE },
                    2 => if sub_phase == 2 { 0x11 } else { 0xEE },
                    3 => if sub_phase != 2 { 0x11 } else { 0xEE },
                    4 => if sub_phase != 0 { 0x11 } else { 0xEE },
                    _ => if sub_phase != 1 { 0x11 } else { 0xEE },
                }
            };
            pixels[row_offset + x as usize] = val;
        }
    }
    pixels
}

// -----------------------------------------------------------------------------
// Pattern 7: 16-Step Grayscale Calibration Wedge
// -----------------------------------------------------------------------------
fn gen_wedge() -> Vec<u8> {
    let mut pixels = vec![0u8; (PATTERN_WIDTH * PATTERN_HEIGHT) as usize];
    for y in 0..PATTERN_HEIGHT {
        let row_offset = (y * PATTERN_WIDTH) as usize;
        for x in 0..PATTERN_WIDTH {
            let step = x * 16 / PATTERN_WIDTH;
            let border = y < 6 || y >= PATTERN_HEIGHT - 6 || x % (PATTERN_WIDTH / 16) < 3;
            let val = if border {
                0x00
            } else {
                u8::try_from(step * 17).unwrap_or(u8::MAX)
            };
            pixels[row_offset + x as usize] = val;
        }
    }
    pixels
}

const PATTERNS: &[PatternDef] = &[
    PatternDef {
        title: "Broadcast TV Test Card",
        subtitle: "PM5544 Universal Color & Alignment Pattern",
        description: "Full broadcast test card with central color bars, resolution multiburst, grayscale staircase, rainbow spectrum, and checkerboard border.",
        generator: gen_pm5544,
    },
    PatternDef {
        title: "RGB Subpixel Resonance",
        subtitle: "Primary Red, Green, & Blue CFA Resonance Bands",
        description: "Selectively illuminates Red, Green, and Blue filter sites at the native 300 PPI microcapsule pitch to demonstrate primary hues on Kaleido 3.",
        generator: gen_rgb,
    },
    PatternDef {
        title: "CMYK & Primary Color Bars",
        subtitle: "Red, Green, Blue, Yellow, Cyan, Magenta, Black, White",
        description: "Standard broadcast-style color test bars using optical additive color mixing across the Color Filter Array.",
        generator: gen_cmyk,
    },
    PatternDef {
        title: "4x4 Bayer Matrix Grids",
        subtitle: "16-Density Ordered Dither Color Steps",
        description: "Simulates continuous color gradients and tonal ramps using spatial Bayer dithering across 16 density thresholds.",
        generator: gen_bayer,
    },
    PatternDef {
        title: "Diagonal Frequency Pitch",
        subtitle: "1px, 2px, 3px, 4px Frequency Sharpness Test",
        description: "Diagonal stripe patterns aligned at 45 degrees to test Color Filter Array diagonal subpixel alignment, moire, and edge sharpness.",
        generator: gen_diagonal,
    },
    PatternDef {
        title: "Color Mixing Swatches",
        subtitle: "Red, Green, Blue, Yellow, Cyan, & Magenta Palette",
        description: "Six high-contrast color swatches with framed borders to evaluate color saturation and contrast against white/black paper.",
        generator: gen_swatches,
    },
    PatternDef {
        title: "16-Step Grayscale Wedge",
        subtitle: "16 Discrete E Ink Quantization Levels (0..255)",
        description: "Shows all 16 discrete hardware grayscale levels from pure black to paper white to calibrate room lighting and waveform response.",
        generator: gen_wedge,
    },
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NavTab {
    #[default]
    TestCard,
    Primaries,
    Patterns,
    Calibrate,
    Guide,
}

impl NavTab {
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::TestCard, "nav-card", "Test Card"),
        (Self::Primaries, "nav-primaries", "RGB/CMYK"),
        (Self::Patterns, "nav-patterns", "Dither"),
        (Self::Calibrate, "nav-calibrate", "Calibrate"),
        (Self::Guide, "nav-guide", "Guide/Exit"),
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|(t, _, _)| *t == self).unwrap_or(0)
    }

    fn sub_pages(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::TestCard => &[("sub-card", "PM5544 Card")],
            Self::Primaries => &[("sub-rgb", "RGB Subpixels"), ("sub-cmyk", "CMYK Bars")],
            Self::Patterns => &[("sub-bayer", "Bayer Grids"), ("sub-diag", "Diagonal Pitch")],
            Self::Calibrate => &[("sub-swatch", "Color Swatches"), ("sub-wedge", "16 Grayscale")],
            Self::Guide => &[("sub-about", "About Kaleido 3"), ("sub-exit", "Exit App")],
        }
    }

    fn pattern_index(self, sub_page: usize) -> Option<usize> {
        match self {
            Self::TestCard => Some(0),
            Self::Primaries => Some(if sub_page == 0 { 1 } else { 2 }),
            Self::Patterns => Some(if sub_page == 0 { 3 } else { 4 }),
            Self::Calibrate => Some(if sub_page == 0 { 5 } else { 6 }),
            Self::Guide => None,
        }
    }
}

struct ColorApp {
    tab: NavTab,
    sub_page: [usize; 5],
    pictures: Vec<Option<TilePicture>>,
}

impl Default for ColorApp {
    fn default() -> Self {
        Self {
            tab: NavTab::TestCard,
            sub_page: [0; 5],
            pictures: vec![None; PATTERNS.len()],
        }
    }
}

impl KoboApp for ColorApp {
    fn on_start(&mut self, context: &mut Context) {
        // Pre-load all 7 prominent color patterns (total 2.0 MB, well within 8 MB cache!)
        for (i, p) in PATTERNS.iter().enumerate() {
            let handle = PictureHandle(u32::try_from(i + 1).unwrap_or(1));
            let raw_pixels = (p.generator)();
            self.pictures[i] = context.put_picture(handle, PATTERN_WIDTH, PATTERN_HEIGHT, raw_pixels);
        }
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        let sub_len = self.tab.sub_pages().len();
        let curr_sub = self.current_sub_page();
        if forward {
            if curr_sub + 1 < sub_len {
                self.sub_page[self.tab.index()] = curr_sub + 1;
            } else {
                let next_tab_idx = (self.tab.index() + 1) % NavTab::ALL.len();
                self.tab = NavTab::ALL[next_tab_idx].0;
                self.sub_page[self.tab.index()] = 0;
            }
        } else if curr_sub > 0 {
            self.sub_page[self.tab.index()] = curr_sub - 1;
        } else {
            let prev_tab_idx = if self.tab.index() > 0 {
                self.tab.index() - 1
            } else {
                NavTab::ALL.len() - 1
            };
            self.tab = NavTab::ALL[prev_tab_idx].0;
            let prev_sub_len = self.tab.sub_pages().len();
            self.sub_page[self.tab.index()] = prev_sub_len.saturating_sub(1);
        }
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // Bottom Nav Bar Destinations
        for (tab, name, _) in NavTab::ALL {
            if action == action_id(name) {
                self.tab = tab;
                self.show(context);
                return;
            }
        }

        // Top Sub-Tabs
        for (i, (name, _)) in self.tab.sub_pages().iter().enumerate() {
            if action == action_id(name) {
                self.sub_page[self.tab.index()] = i;
                self.show(context);
                return;
            }
        }

        // Top Bar Step Next / Prev
        if action == action_id("step-next") {
            self.on_page_turn(context, true);
            return;
        }
        if action == action_id("step-prev") {
            self.on_page_turn(context, false);
            return;
        }

        // Exit to Stock Kobo Home Screen
        if action == action_id("exit-to-reader") {
            context.exit();
            return;
        }

        self.show(context);
    }
}

impl ColorApp {
    fn current_sub_page(&self) -> usize {
        let max_subs = self.tab.sub_pages().len();
        self.sub_page[self.tab.index()].min(max_subs.saturating_sub(1))
    }

    fn show(&self, context: &mut Context) {
        let sub_idx = self.current_sub_page();
        let sub_pages = self.tab.sub_pages();

        // 1. Base Screen with Clean Header
        let mut builder = ScreenBuilder::new("color-app")
            .top_bar(match self.tab {
                NavTab::TestCard => "Broadcast Color Test Card",
                NavTab::Primaries => "Primary & Subpixel Colors",
                NavTab::Patterns => "Color Dithering & Matrices",
                NavTab::Calibrate => "Calibration & Swatches",
                NavTab::Guide => "Color Patterns — Guide & Exit",
            })
            .top_bar_action("step-next", "Next →")
            .top_bar_action("exit-to-reader", "Exit");

        // 2. Top Sub-Tabs strip
        if sub_pages.len() > 1 {
            builder = builder.tabs(sub_idx, sub_pages.iter().copied());
        }

        // 3. Body Content: Prominent Large Visual (110mm tall, crisp and saturated!)
        if let Some(p_idx) = self.tab.pattern_index(sub_idx) {
            let pattern = &PATTERNS[p_idx];
            if let Some(Some(pic)) = self.pictures.get(p_idx) {
                builder = builder.picture(*pic, 110);
            }
            builder = builder
                .heading(pattern.title)
                .text(pattern.subtitle)
                .text(pattern.description);
        } else {
            // Guide / Exit Tab Content
            if sub_idx == 0 {
                builder = builder
                    .banner(
                        BannerLevel::Info,
                        "Color Patterns for Kobo Libra Colour (Kaleido 3 E-Ink)",
                    )
                    .heading("About Color E-Ink & CFA")
                    .text(
                        "Your Kobo Libra Colour features a Kaleido 3 Color Filter Array (CFA) \
                         with 300 PPI black-and-white microcapsules. Calibrated test patterns \
                         illuminate Red, Green, and Blue subpixels to demonstrate gamut, \
                         optical dithering, and resolution.",
                    )
                    .heading("Navigation Controls")
                    .text("• Bottom Footer: Tap bottom tabs to switch test categories.")
                    .text("• Top Sub-Tabs: Tap sub-tabs to switch patterns inside each category.")
                    .text("• Physical Buttons: Page keys cycle smoothly through all slides.");
            } else {
                builder = builder
                    .heading("Exit Color Patterns")
                    .text("Return to your normal Kobo home screen.")
                    .primary_button("exit-to-reader", "Exit to Stock Kobo Home Screen");
            }
        }

        // 4. Fixed Bottom Footer Navigation Bar (matching gallery reference)
        builder = builder.nav_bar(
            self.tab.index(),
            NavTab::ALL.map(|(_, name, label)| (name, label)),
        );

        context.set_screen(builder.build());
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("color", ColorApp::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command};

    #[test]
    fn app_initializes_with_nav_bar_and_7_pictures() {
        let mut runner = AppRunner::new(ColorApp::default());
        let commands = runner.start();
        assert_eq!(runner.app().tab, NavTab::TestCard);

        let put_count = commands
            .iter()
            .filter(|c| matches!(c, Command::PutPicture { .. }))
            .count();
        assert_eq!(put_count, 7);
    }

    #[test]
    fn bottom_nav_bar_destination_switching() {
        let mut runner = AppRunner::new(ColorApp::default());
        runner.start();

        runner.action(action_id("nav-primaries"));
        assert_eq!(runner.app().tab, NavTab::Primaries);
        assert_eq!(runner.app().current_sub_page(), 0);

        runner.action(action_id("sub-cmyk"));
        assert_eq!(runner.app().current_sub_page(), 1);

        runner.action(action_id("nav-patterns"));
        assert_eq!(runner.app().tab, NavTab::Patterns);

        runner.action(action_id("nav-guide"));
        assert_eq!(runner.app().tab, NavTab::Guide);
    }

    #[test]
    fn page_turns_cycle_smoothly() {
        let mut runner = AppRunner::new(ColorApp::default());
        runner.start();
        assert_eq!(runner.app().tab, NavTab::TestCard);

        runner.page_turn(true);
        assert_eq!(runner.app().tab, NavTab::Primaries);
        assert_eq!(runner.app().current_sub_page(), 0);

        runner.page_turn(true);
        assert_eq!(runner.app().tab, NavTab::Primaries);
        assert_eq!(runner.app().current_sub_page(), 1);
    }

    #[test]
    fn exit_command_works() {
        let mut runner = AppRunner::new(ColorApp::default());
        runner.start();

        let exit_cmds = runner.action(action_id("exit-to-reader"));
        assert!(exit_cmds.iter().any(|c| matches!(c, Command::Exit)));
    }
}
