//! The Settings surface for Cobalt's runtime-owned Tailscale service.
//!
//! EXPERIMENTAL (P0/P1): not yet validated on hardware. The app persists the
//! on/off state, reads runtime-written status, and shows the approval code
//! when the daemon asks for sign-in. It never sees a key, a peer list, or a
//! flag: the supervisor in crates/kobod/src/tailscale.rs owns all of that.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, ScreenBuilder, StoreResult,
};
use std::process::ExitCode;
use std::time::Duration;

const CONFIG: &str = "tailscale-config";
const STATUS: &str = "tailscale-status";

/// How soon after a change the platform wake launcher gets a chance to run
/// `kobod --tailscale ensure`. The app cannot start a daemon itself; it asks
/// for a wake and the runtime side does the work.
const ENSURE_WAKE_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Status,
    Login,
    About,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TailscaleStatus {
    state: String,
    ip: String,
    message: String,
    login_url: String,
}

struct Tailscale {
    enabled: bool,
    view: View,
    status: TailscaleStatus,
}

impl Default for Tailscale {
    fn default() -> Self {
        Self {
            enabled: false,
            view: View::Status,
            status: TailscaleStatus {
                state: "disabled".into(),
                message: "Tailscale has not run.".into(),
                ..TailscaleStatus::default()
            },
        }
    }
}

impl Tailscale {
    fn save(&self, context: &mut Context) {
        context
            .store()
            .save(CONFIG, if self.enabled { "true" } else { "false" });
    }

    fn request_ensure(context: &mut Context) {
        context
            .device()
            .schedule_wake(Duration::from_secs(ENSURE_WAKE_SECONDS));
    }

    fn login_code(&self) -> String {
        self.status
            .login_url
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => {
                let state = match self.status.state.as_str() {
                    "running" => "Connected",
                    "needs-login" => "Waiting for approval",
                    "failed" => "Failed",
                    _ if self.enabled => "Starting",
                    _ => "Off",
                };
                let mut screen = ScreenBuilder::new("tailscale")
                    .top_bar("Tailscale")
                    .section_with_value("Service", state)
                    .rows([
                        (
                            "toggle",
                            if self.enabled {
                                "Turn Tailscale off".to_owned()
                            } else {
                                "Turn Tailscale on".to_owned()
                            },
                            "Applies within a minute.".to_owned(),
                            Glyph::Settings,
                        ),
                        (
                            "login",
                            "Sign-in".to_owned(),
                            "Shows the approval code when this reader needs one.".to_owned(),
                            Glyph::Settings,
                        ),
                        (
                            "refresh",
                            "Refresh status".to_owned(),
                            "Read the runtime-owned Tailscale status.".to_owned(),
                            Glyph::Refresh,
                        ),
                        (
                            "about",
                            "About Tailscale".to_owned(),
                            "How the tailnet works here and software information.".to_owned(),
                            Glyph::Settings,
                        ),
                    ])
                    .facts([
                        ("Tailnet IP", if self.status.ip.is_empty() {
                            "none yet".to_owned()
                        } else {
                            self.status.ip.clone()
                        }),
                        ("Status", self.status.message.clone()),
                    ]);
                if self.status.state == "needs-login" {
                    screen = screen.banner(
                        BannerLevel::Info,
                        "This reader is waiting for approval. Open Sign-in for the code.",
                    );
                } else if !self.enabled {
                    screen = screen.banner(BannerLevel::Info, "Tailscale is off.");
                }
                screen.build()
            }
            View::Login => {
                let code = self.login_code();
                if code.is_empty() {
                    ScreenBuilder::new("tailscale")
                        .top_bar("Sign-in")
                        .empty_state("No approval is pending. Turn Tailscale on and refresh.")
                        .button("back", "Back")
                        .build()
                } else {
                    ScreenBuilder::new("tailscale")
                        .top_bar("Sign-in")
                        .heading("Approve this reader")
                        .text(format!(
                            "On any phone or computer, open login.tailscale.com and enter code {code}. The full link is {}.",
                            self.status.login_url
                        ))
                        .secondary(
                            "In the Tailscale admin console, disable key expiry for this device; node keys expire after 180 days by default.",
                        )
                        .button("back", "Back")
                        .build()
                }
            }
            View::About => ScreenBuilder::new("tailscale")
                .top_bar("About Tailscale")
                .heading("Tailscale")
                .text(
                    "Tailscale puts this reader on your tailnet so tailnet services - a Calibre-Web library, a paperterm host, anything with a 100.x address - work from any network. Tailscale is available under the BSD 3-Clause license.",
                )
                .secondary(
                    "Experimental: the runtime runs the pinned upstream build with kernel TUN and no firewall changes, keeps name resolution in /etc/hosts, and leaves DNS alone. Hardware validation is pending.",
                )
                .button("back", "Back")
                .build(),
        };
        context.set_screen(screen);
    }
}

impl KoboApp for Tailscale {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
        context.store().load(STATUS);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
                self.enabled = value
                    .as_deref()
                    .is_some_and(|raw| String::from_utf8_lossy(raw).lines().next() == Some("true"));
            } else if key == STATUS {
                let text = value.as_deref().map_or_else(
                    || "disabled\n\nTailscale has not run.\n".into(),
                    String::from_utf8_lossy,
                );
                let mut lines = text.lines();
                lines
                    .next()
                    .unwrap_or("disabled")
                    .clone_into(&mut self.status.state);
                lines
                    .next()
                    .unwrap_or_default()
                    .clone_into(&mut self.status.ip);
                lines
                    .next()
                    .unwrap_or_default()
                    .clone_into(&mut self.status.message);
                lines
                    .next()
                    .unwrap_or_default()
                    .clone_into(&mut self.status.login_url);
            }
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("toggle") {
            self.enabled = !self.enabled;
            self.status.state = if self.enabled { "starting" } else { "disabled" }.into();
            self.status.message = if self.enabled {
                "Starting within a minute.".into()
            } else {
                "Stopping within a minute.".into()
            };
            self.save(context);
            Self::request_ensure(context);
        } else if action == action_id("login") {
            self.view = View::Login;
        } else if action == action_id("refresh") {
            context.store().load(STATUS);
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Status;
        }
        self.show(context);
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // The platform wake launcher invokes `kobod --tailscale ensure`;
        // this app only repaints the result when the reader next sees it.
        context.store().load(STATUS);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("tailscale", Tailscale::default()).map_or_else(
        |e| {
            eprintln!("tailscale: {e}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, DeviceRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn primary_control_fits_clara_bw() {
        let layout = ScreenBuilder::new("tailscale")
            .top_bar("Tailscale")
            .empty_state("Tailscale is off.")
            .bottom_action("toggle", "Turn Tailscale on")
            .build()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let toggle = layout
            .rect_of_action(action_id("toggle"))
            .expect("tailscale control");
        assert!(toggle.height >= CLARA_BW_METRICS.touch_target_minimum());
    }

    #[test]
    fn turning_on_requests_a_prompt_wake() {
        let mut app = Tailscale::default();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("toggle"));
        assert!(context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Device(DeviceRequest::ScheduleWake { seconds: 30 })
            )
        }));
    }

    #[test]
    fn login_code_is_the_last_path_component() {
        let app = Tailscale {
            status: TailscaleStatus {
                login_url: "https://login.tailscale.com/a/1a2b3c4d".into(),
                ..TailscaleStatus::default()
            },
            ..Tailscale::default()
        };
        assert_eq!(app.login_code(), "1a2b3c4d");
    }
}
