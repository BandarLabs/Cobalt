//! A local photo-frame surface. Incoming pictures belong to the host transfer service.
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, PictureHandle, Screen, ScreenBuilder, TilePicture,
};
use std::process::ExitCode;
use std::time::Duration;

const PHOTO: PictureHandle = PictureHandle(1);
const SHOW: &str = "show";
const MODE: &str = "mode";
const INTERVAL: &str = "interval";
/// The currently available per-picture budget holds this portrait image.
///
/// The runtime refuses an oversized picture rather than silently blanking the
/// frame.  The transfer host therefore must pre-fit photographs to this bound
/// until the platform gains tiled full-panel pictures.
const PHOTO_WIDTH: u32 = 536;
const PHOTO_HEIGHT: u32 = 724;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Photo,
}

struct Frame {
    view: View,
    slow: bool,
    interval: u8,
}
impl Default for Frame {
    fn default() -> Self {
        Self {
            view: View::Home,
            slow: false,
            interval: 15,
        }
    }
}

fn sample_photo() -> Vec<u8> {
    const W: u32 = PHOTO_WIDTH;
    const H: u32 = PHOTO_HEIGHT;
    let mut pixels = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            let horizon = H * 47 / 100;
            let grey = if y < horizon {
                // Light sky, a sun, and a little intentional grain.
                let sun = (x as i32 - 390).pow(2) + (y as i32 - 135).pow(2) < 42_i32.pow(2);
                if sun {
                    248
                } else {
                    205_u32.saturating_sub(y / 5) as u8
                }
            } else if y < horizon + 20 {
                80
            } else {
                let wave = ((x / 19 + y / 13) % 5) as u8;
                102 + wave * 13
            };
            pixels.push(grey);
        }
    }
    pixels
}

impl Frame {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Home => {
                let timing = if self.slow {
                    format!("Slow slideshow · every {} hours", self.interval)
                } else {
                    format!("Frame mode · every {} minutes", self.interval)
                };
                ScreenBuilder::new("frame-home")
                    .top_bar("Frame")
                    .picture(TilePicture::new(PHOTO, PHOTO_WIDTH, PHOTO_HEIGHT), 72)
                    .secondary("Sample photograph. Connect a host to add your own.")
                    .rows([
                        (
                            MODE,
                            timing,
                            "Switches between awake and scheduled photo changes.",
                            Glyph::Clock,
                        ),
                        (
                            INTERVAL,
                            "Change interval".to_owned(),
                            "5, 15, 60 minutes; 1, 6, 24 hours.",
                            Glyph::Clock,
                        ),
                    ])
                    .button(SHOW, "Show photograph")
                    .build()
            }
            View::Photo => ScreenBuilder::new("frame-show")
                .unframed_picture(TilePicture::new(PHOTO, PHOTO_WIDTH, PHOTO_HEIGHT), 130)
                .build(),
        }
    }
    fn apply_power_policy(&self, context: &mut Context) {
        if self.slow {
            context.device().allow_sleep();
            context
                .device()
                .schedule_wake(Duration::from_secs(u64::from(self.interval) * 3600));
        } else {
            context
                .device()
                .keep_awake(Duration::from_secs(u64::from(self.interval) * 60));
            context.device().cancel_wake();
        }
    }
}

impl KoboApp for Frame {
    fn on_start(&mut self, context: &mut Context) {
        let _ = context.put_picture(PHOTO, 536, 724, sample_photo());
        self.apply_power_policy(context);
        self.show(context);
    }
    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // A production transfer manifest selects the next decoded photo here.
        self.apply_power_policy(context);
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(SHOW) {
            self.view = View::Photo;
        } else if action == action_id(MODE) {
            self.slow = !self.slow;
            self.interval = if self.slow { 6 } else { 15 };
            self.apply_power_policy(context);
        } else if action == action_id(INTERVAL) {
            self.interval = if self.slow {
                match self.interval {
                    1 => 6,
                    6 => 24,
                    _ => 1,
                }
            } else {
                match self.interval {
                    5 => 15,
                    15 => 60,
                    _ => 5,
                }
            };
            self.apply_power_policy(context);
        } else {
            return;
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("frame", Frame::default()).map_or_else(
        |error| {
            eprintln!("frame: {error}");
            ExitCode::FAILURE
        },
        |_| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, DeviceRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn sample_is_a_panel_sized_greyscale_picture() {
        assert_eq!(sample_photo().len(), (PHOTO_WIDTH * PHOTO_HEIGHT) as usize);
        assert!(sample_photo().iter().any(|&pixel| pixel < 100));
        assert!(sample_photo().iter().any(|&pixel| pixel > 220));
    }
    #[test]
    fn home_layout_fits_and_show_is_reachable() {
        let screen = Frame::default().screen();
        assert!(screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(SHOW))
            .is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn slow_mode_requests_a_scheduled_wake() {
        let frame = Frame {
            slow: true,
            interval: 6,
            ..Frame::default()
        };
        let mut context = Context::default();
        frame.apply_power_policy(&mut context);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ScheduleWake { .. }))));
    }
}
