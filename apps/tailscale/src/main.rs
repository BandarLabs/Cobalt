//! The switch and the sign-in screen for Cobalt's Tailscale service.
//!
//! This app owns one word, on or off, and shows what the runtime says came of
//! it. It never sees a key, a list of the tailnet's computers or a flag: the
//! supervisor in `crates/kobod/src/tailscale.rs` owns all of that, and an app
//! cannot start a daemon anyway. Saving the word is the whole request. The
//! runtime acts on it at once and writes back a status this app reads.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Heartbeat, KoboApp, Screen, ScreenBuilder,
    StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;
use std::time::Duration;

const CONFIG: &str = "tailscale-config";
const STATUS: &str = "tailscale-status";
/// Saved to ask the runtime for something the switch cannot say.
const REQUEST: &str = "tailscale-request";
const SIGN_OUT: &str = "sign-out";

/// How long the runtime may take to say anything after the switch is turned
/// on before the screen says it has not started. It answers a tap within a
/// few seconds, most of which is checking the two programs on a slow card, so
/// a minute of nothing means a runtime too old to start the service at all.
const START_WAIT: Duration = Duration::from_secs(60);
/// How long the screen follows a start. Sign-in is given ten minutes, so
/// anything still unsettled after twelve is a stale file, and following it
/// further would only cost battery.
const FOLLOW_LIMIT: Duration = Duration::from_secs(12 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Status,
    /// The approval link, as a code for a phone and as text.
    SignIn,
    About,
    /// The question asked before this reader's key is forgotten.
    SignOut,
    /// What the owner may switch on beyond being on the tailnet at all.
    Options,
}

/// Where one thing the reader offers the tailnet stands, as the owner
/// should read it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Offer {
    Off,
    /// Switched on, and the runtime has not said anything about it yet.
    Starting,
    /// Offered, and waiting for the tailnet's admin console to approve it.
    Waiting,
    Approved,
    Unavailable,
}

impl Offer {
    fn read(on: bool, connected: bool, state: &str) -> Self {
        if !on {
            return Self::Off;
        }
        match state {
            // The one answer that holds whether or not the tunnel is up.
            "unavailable" => Self::Unavailable,
            "approved" if connected => Self::Approved,
            "waiting" if connected => Self::Waiting,
            _ => Self::Starting,
        }
    }
}

/// The supervisor's status, line by line. The name came after the first four
/// lines, so an older supervisor leaves it empty rather than wrong.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ServiceStatus {
    state: String,
    address: String,
    message: String,
    sign_in: String,
    name: String,
    /// Why the reader is not staying connected outside Cobalt although it
    /// was asked to. Empty when it is, or was not asked.
    cannot_stay: String,
    /// Where the exit node and the shared network stand: nothing when off,
    /// `waiting` for the admin console, `approved`, or `unavailable`.
    exit_node: String,
    network: String,
    /// The network being shared, as the admin console writes it.
    shared: String,
    exit_node_why: String,
    network_why: String,
}

impl ServiceStatus {
    fn parse(text: &str) -> Self {
        let mut lines = text.lines().map(str::to_owned);
        let mut next = || lines.next().unwrap_or_default();
        Self {
            state: next(),
            address: next(),
            message: next(),
            sign_in: next(),
            name: next(),
            cannot_stay: next(),
            exit_node: next(),
            network: next(),
            shared: next(),
            exit_node_why: next(),
            network_why: next(),
        }
    }

    /// Whether the supervisor is still on its way somewhere, so the screen
    /// should keep looking.
    fn unsettled(&self) -> bool {
        matches!(
            self.state.as_str(),
            "" | "off" | "stopped" | "starting" | "downloading" | "needs-login" | "signed-out"
        )
    }

    /// Whether the supervisor stopped for something only the owner can fix,
    /// so its own words belong at the top of the screen.
    fn needs_the_owner(&self) -> bool {
        matches!(
            self.state.as_str(),
            "failed" | "refused" | "unsupported" | "timed-out" | "needs-approval" | "offline"
        ) && !self.message.is_empty()
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "each is a switch on the screen that the owner sets on its own"
)]
struct Tailscale {
    enabled: bool,
    /// Stay on the tailnet after Cobalt hands the screen back.
    stay: bool,
    /// Offer this reader as an exit node.
    exit_node: bool,
    /// Share the network the reader's Wi-Fi is on.
    share_network: bool,
    view: View,
    status: ServiceStatus,
    /// Ticks while a start is under way, so the screen follows it without a
    /// refresh button.
    watch: Heartbeat,
    /// The switch was turned on and the runtime has said nothing since.
    stalled: bool,
    /// The link the sign-in screen was last opened for, so it opens by itself
    /// once per link and stays closed after the owner leaves it.
    shown_link: String,
}

impl Default for Tailscale {
    fn default() -> Self {
        Self {
            enabled: false,
            stay: false,
            exit_node: false,
            share_network: false,
            view: View::Status,
            status: ServiceStatus::default(),
            watch: Heartbeat::every(3),
            stalled: false,
            shown_link: String::new(),
        }
    }
}

impl Tailscale {
    /// Saves what the owner chose, one word a line. The switch is first,
    /// where a runtime from before the other choices looks for it.
    fn save(&self, context: &mut Context) {
        context.store().save(
            CONFIG,
            format!(
                "{}\n{}\n{}\n{}\n",
                self.enabled, self.stay, self.exit_node, self.share_network
            ),
        );
    }

    /// Whether the reader really is staying connected: asked for, and the
    /// runtime has not said it could not be arranged.
    fn staying(&self) -> bool {
        self.enabled && self.stay && self.status.cannot_stay.is_empty()
    }

    /// Follows a start that is under way, and stops when there is nothing
    /// left to follow.
    fn follow(&mut self, context: &mut Context) {
        if self.enabled && self.status.unsettled() && !self.stalled {
            self.watch.start(context);
        } else {
            self.watch.stop(context);
        }
    }

    fn connected(&self) -> bool {
        self.enabled && self.status.state == "running"
    }

    fn waiting_for_approval(&self) -> bool {
        self.enabled && self.status.state == "needs-login" && !self.status.sign_in.is_empty()
    }

    fn state_words(&self) -> &'static str {
        if !self.enabled {
            return if self.status.state == "signed-out" {
                "Signed out"
            } else {
                "Off"
            };
        }
        if self.stalled {
            return "Not started";
        }
        match self.status.state.as_str() {
            "running" => "Connected",
            "downloading" => "Downloading",
            "needs-login" => "Waiting for approval",
            "needs-approval" => "Waiting for an admin",
            "offline" => "Waiting for Wi-Fi",
            "timed-out" => "Sign-in timed out",
            "failed" => "Could not start",
            "refused" => "Not started",
            "unsupported" => "Not supported",
            _ => "Starting",
        }
    }

    fn banner(&self) -> Option<(BannerLevel, String)> {
        if !self.enabled {
            // Off is said once, by the switch and the line under it. A banner
            // and a status line saying it again made the first screen anyone
            // sees repeat itself three times.
            return (self.status.state == "signed-out" && !self.status.message.is_empty())
                .then(|| (BannerLevel::Info, self.status.message.clone()));
        }
        if self.stalled {
            return Some((
                BannerLevel::Error,
                "Tailscale has not started. Check Settings for a Cobalt update, then switch it off and on.".to_owned(),
            ));
        }
        if self.waiting_for_approval() {
            return Some((
                BannerLevel::Attention,
                "This reader is waiting to be approved from your phone.".to_owned(),
            ));
        }
        if self.status.needs_the_owner() {
            // A start that failed is tried again by itself. The others wait
            // for the owner, and say what they are waiting for.
            let level = if self.status.state == "failed" {
                BannerLevel::Error
            } else {
                BannerLevel::Attention
            };
            return Some((level, self.status.message.clone()));
        }
        if self.status.state == "downloading" {
            return Some((BannerLevel::Info, self.status.message.clone()));
        }
        None
    }

    fn status_screen(&self) -> Screen {
        // The banner leads, so it is the last thing a larger text size drops.
        let mut screen = ScreenBuilder::new("tailscale").top_bar("Tailscale");
        if let Some((level, words)) = self.banner() {
            screen = screen.banner(level, words);
        }
        // The line under the switch says what is true now, as the switch
        // does, not what a tap would do. It is the only title on this
        // screen with a line under it. A row's name says what the row is
        // for, and a second line that says it again in more words is noise
        // on a panel that redraws slowly and holds little.
        let summary = if self.staying() {
            "Stays connected after you leave Cobalt."
        } else if self.enabled {
            "Runs while Cobalt is open."
        } else {
            "Off. Nothing reaches this reader over a tailnet."
        };
        let mut rows = Vec::with_capacity(3);
        if self.waiting_for_approval() {
            rows.push(("sign-in", "Approve this reader", "", Glyph::Key));
        } else if self.enabled
            && (self.stalled
                || matches!(
                    self.status.state.as_str(),
                    "timed-out" | "needs-approval" | "failed" | "refused"
                ))
        {
            rows.push(("retry", "Try again", "", Glyph::Refresh));
        }
        rows.push(("options", "Options", "", Glyph::Settings));
        rows.push(("about", "About Tailscale", "", Glyph::Globe));
        let mut facts = Vec::with_capacity(3);
        if self.enabled {
            facts.push(("Status", self.state_words().to_owned()));
        }
        if self.connected() {
            if !self.status.name.is_empty() {
                facts.push(("Name", self.status.name.clone()));
            }
            if !self.status.address.is_empty() {
                facts.push(("Address", self.status.address.clone()));
            }
        }
        screen
            .toggle("toggle", "Tailscale", summary, self.enabled)
            .rows(rows)
            .facts(facts.iter().map(|(label, value)| (*label, value.as_str())))
            .build()
    }

    /// The choices beyond being on the tailnet, each off until the owner
    /// turns it on, with what it costs said under it once it is on.
    ///
    /// A page of their own, so the first screen stays one switch. An owner
    /// who never opens this has a reader that is a plain member of the
    /// tailnet while Cobalt is open, which is the safe thing to be.
    fn options_screen(&self) -> Screen {
        let cannot_stay = self.stay && !self.status.cannot_stay.is_empty();
        let mut screen = ScreenBuilder::new("tailscale")
            .top_bar("Tailscale options")
            .toggle(
                "stay",
                "Stay connected outside Cobalt",
                // Under a switch goes what the switch cannot say: that it is
                // on and not working. Off, and on and working, it has said.
                if cannot_stay {
                    "Not available on this reader right now."
                } else {
                    ""
                },
                self.stay,
            );
        if cannot_stay {
            screen = screen.secondary(self.status.cannot_stay.clone());
        } else if self.stay {
            screen = screen.secondary("Uses more battery. The Kobo software keeps Wi-Fi on only while the reader is awake. After a restart, open Cobalt once to connect again.");
        }
        // What each says under it comes from the runtime, which is the one
        // that knows whether the tailnet has approved it and whether this
        // reader can do it at all. A switch that is on and cannot work says
        // so in the runtime's words; it never just sits there on.
        let connected = self.connected();
        let exit_node = Offer::read(self.exit_node, connected, &self.status.exit_node);
        screen = screen.toggle(
            "exit-node",
            "Offer as an exit node",
            match exit_node {
                Offer::Off | Offer::Starting => "",
                Offer::Waiting => "Waiting for approval.",
                Offer::Approved => "Approved.",
                Offer::Unavailable => "Not available on this reader.",
            },
            self.exit_node,
        );
        if exit_node == Offer::Unavailable && !self.status.exit_node_why.is_empty() {
            screen = screen.secondary(self.status.exit_node_why.clone());
        }
        let network = Offer::read(self.share_network, connected, &self.status.network);
        screen = screen.toggle(
            "share-network",
            "Share this network",
            match network {
                Offer::Off | Offer::Starting => String::new(),
                Offer::Waiting => format!("Waiting for approval: {}", self.status.shared),
                Offer::Approved => format!("Shared: {}", self.status.shared),
                Offer::Unavailable => "Not available here.".to_owned(),
            },
            self.share_network,
        );
        // Said once when both are refused for the same reason.
        if network == Offer::Unavailable
            && !self.status.network_why.is_empty()
            && !(exit_node == Offer::Unavailable
                && self.status.network_why == self.status.exit_node_why)
        {
            screen = screen.secondary(self.status.network_why.clone());
        }
        // Where to approve is said only while something is waiting to be,
        // and what it costs for as long as anything is offered.
        let offers = [exit_node, network];
        let unapproved = offers
            .iter()
            .any(|offer| matches!(offer, Offer::Starting | Offer::Waiting));
        let offered = unapproved || offers.contains(&Offer::Approved);
        if unapproved {
            screen = screen.secondary("Approve it once in the Tailscale admin console, under this reader's route settings.");
        }
        if offered {
            screen = screen.secondary("A reader is slow at this, and it stops carrying traffic when it sleeps or Tailscale stops.");
        }
        screen.build()
    }

    /// The approval link as a code for the owner's phone, with the address
    /// spelled out under it for a device that cannot scan.
    ///
    /// The code comes first because the person holding the reader approves it
    /// on the phone in their other hand, and nobody types forty characters
    /// off an e-ink panel if they can point a camera at it instead.
    fn sign_in_screen(&self) -> Screen {
        ScreenBuilder::new("tailscale")
            .top_bar("Approve this reader")
            .code(
                &self.status.sign_in,
                "Scan with your phone and sign in to Tailscale.",
            )
            .secondary("Or open this address on any phone or computer:")
            .command(&self.status.sign_in)
            .secondary("The code works for ten minutes. This screen closes by itself once the reader is approved.")
            .build()
    }

    fn sign_out_screen() -> Screen {
        about_builder()
            .confirm(
                "Sign out of your tailnet",
                "This reader forgets its key and Tailscale is switched off. To use it again you approve the reader again.",
                ("sign-out-confirmed", "Sign out"),
                ("back", "Cancel"),
            )
            .build()
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => self.status_screen(),
            View::SignIn => self.sign_in_screen(),
            View::About => about_builder().build(),
            View::SignOut => Self::sign_out_screen(),
            View::Options => self.options_screen(),
        };
        // Supporting pages receive the runtime's Back before it can leave the
        // app; the status screen is the way out.
        context.set_screen(screen.with_own_back(self.view != View::Status));
    }

    fn load_status(&mut self, context: &mut Context, value: Option<&[u8]>) {
        let fresh = value
            .map(|bytes| ServiceStatus::parse(&String::from_utf8_lossy(bytes)))
            .unwrap_or_default();
        if fresh != self.status {
            // The runtime has spoken, so the start is not stalled.
            self.stalled = false;
        }
        self.status = fresh;
        if self.waiting_for_approval() {
            // Opened once for each link: the owner switched Tailscale on and
            // is waiting for exactly this, but one who went back to the first
            // screen meant to.
            if self.view == View::Status && self.shown_link != self.status.sign_in {
                self.view = View::SignIn;
            }
            self.shown_link.clone_from(&self.status.sign_in);
        } else if self.view == View::SignIn {
            // Approved, timed out or switched off: the code is no use now.
            self.view = View::Status;
        }
        self.follow(context);
    }

    /// Asks the runtime to start again, which is done by saving the switch a
    /// second time: any save of it is looked at immediately.
    fn try_again(&mut self, context: &mut Context) {
        self.stalled = false;
        self.status = ServiceStatus::default();
        self.save(context);
        self.follow(context);
    }
}

/// The page that says what the service is for and what it opens, which the
/// sign-out question is also asked over, so the owner answers it looking at
/// the page they asked from.
fn about_builder() -> ScreenBuilder {
    ScreenBuilder::new("tailscale")
        .top_bar("About Tailscale")
        .text("Tailscale puts this reader on your tailnet, the private network between your own devices. Your computer can then reach the reader from any network, and apps here can reach your own servers by name. It runs while Cobalt is open, or stays connected if you ask in Options.")
        .secondary("Once connected, save the reader's tailnet address on your computer. After that kobo send and kobo clip reach it from anywhere:")
        .command("kobo pair kobo --device ADDRESS")
        .secondary("Computers on your tailnet can reach this reader's SSH server, which takes your key only. Unless you switch it on in Options, the reader forwards no traffic and offers no exit node and no routes. It never runs Tailscale SSH.")
        .secondary("The reader downloads Tailscale the first time it is switched on. Tailscale is available under the BSD 3-Clause license.")
        .rows([(
            "sign-out",
            "Sign out",
            "",
            Glyph::Trash,
        )])
}

impl KoboApp for Tailscale {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
        context.store().load(STATUS);
        self.show(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        context.store().load(STATUS);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        match key.as_str() {
            CONFIG => {
                let saved = value
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                let mut lines = saved.lines();
                let mut next = || lines.next() == Some("true");
                self.enabled = next();
                self.stay = next();
                self.exit_node = next();
                self.share_network = next();
                self.follow(context);
            }
            STATUS => self.load_status(context, value.as_deref()),
            _ => return,
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if !self.watch.on_task(context, task, &outcome) {
            return;
        }
        let quiet = matches!(
            self.status.state.as_str(),
            "" | "off" | "stopped" | "signed-out"
        );
        if self.enabled && quiet && self.watch.waited() >= START_WAIT {
            self.stalled = true;
            self.watch.stop(context);
            self.show(context);
            return;
        }
        if self.watch.waited() >= FOLLOW_LIMIT {
            self.watch.stop(context);
        }
        context.store().load(STATUS);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("toggle") {
            self.enabled = !self.enabled;
            self.stalled = false;
            // What the last run said is not about this one.
            self.status = ServiceStatus::default();
            self.save(context);
            self.follow(context);
        } else if action == action_id("retry") {
            self.try_again(context);
        } else if action == action_id("sign-in") {
            self.view = View::SignIn;
        } else if action == action_id("options") {
            self.view = View::Options;
        } else if action == action_id("stay") {
            self.stay = !self.stay;
            self.save(context);
            // The runtime ends one supervisor and starts the other kind, so
            // the screen follows that the way it follows any start.
            self.watch.start(context);
        } else if action == action_id("exit-node") || action == action_id("share-network") {
            if action == action_id("exit-node") {
                self.exit_node = !self.exit_node;
            } else {
                self.share_network = !self.share_network;
            }
            // What the runtime said about the old choice is not about this
            // one, and it starts the daemon again to apply it.
            self.status.exit_node.clear();
            self.status.network.clear();
            self.save(context);
            self.watch.start(context);
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("sign-out") {
            self.view = View::SignOut;
        } else if action == action_id("sign-out-confirmed") {
            // The request first and the switch second: the runtime reads them
            // in that order, and a sign-out it took for a plain off would
            // stop the daemon before it could tell the tailnet.
            context.store().save(REQUEST, SIGN_OUT);
            self.enabled = false;
            self.stalled = false;
            self.status = ServiceStatus::default();
            self.save(context);
            self.view = View::Status;
            // Followed for a moment although it is off, so the runtime's
            // answer that the key is gone reaches the screen.
            self.watch.start(context);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = match self.view {
                View::SignOut => View::About,
                _ => View::Status,
            };
        }
        self.show(context);
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
    use kobo_sdk::{AppRunner, Command, StoreRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn screen(commands: &[Command]) -> &Screen {
        commands
            .iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("screen update")
    }

    fn words(screen: &Screen) -> String {
        screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .layout
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn saves(commands: &[Command]) -> Vec<(String, String)> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Store(StoreRequest::Save { key, value }) => {
                    Some((key.clone(), String::from_utf8_lossy(value).into_owned()))
                }
                _ => None,
            })
            .collect()
    }

    fn with_status(enabled: bool, status: &str) -> Tailscale {
        Tailscale {
            enabled,
            status: ServiceStatus::parse(status),
            ..Tailscale::default()
        }
    }

    const WAITING: &str =
        "needs-login\n\nApprove this reader to finish.\nhttps://login.tailscale.com/a/1a2b3c4d5e6f\n\n";
    const CONNECTED: &str = "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\n";

    #[test]
    fn turning_tailscale_on_saves_the_word_the_runtime_acts_on_and_nothing_else() {
        let mut runner = AppRunner::new(Tailscale::default());
        let turned_on = runner.action(action_id("toggle"));
        assert_eq!(
            saves(&turned_on),
            [(CONFIG.to_owned(), "true\nfalse\nfalse\nfalse\n".to_owned())]
        );
        assert!(words(screen(&turned_on)).contains("Starting"));
        // A scheduled wake is never delivered on a reader, so asking for one
        // here would be asking nobody.
        assert!(!turned_on
            .iter()
            .any(|command| matches!(command, Command::Device(_))));
        let turned_off = runner.action(action_id("toggle"));
        assert_eq!(
            saves(&turned_off),
            [(CONFIG.to_owned(), "false\nfalse\nfalse\nfalse\n".to_owned())]
        );
        assert!(words(screen(&turned_off)).contains("Off. Nothing reaches this reader"));
    }

    #[test]
    fn the_sign_in_screen_opens_by_itself_with_a_code_for_a_phone_and_the_address() {
        let mut app = with_status(true, "starting\n\nStarting Tailscale.\n\n\n");
        let mut context = Context::default();
        app.load_status(&mut context, Some(WAITING.as_bytes()));
        assert_eq!(app.view, View::SignIn);
        let screen = app.sign_in_screen();
        let code = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_ui::Node::Code { payload, .. } => Some(payload.as_str()),
                _ => None,
            })
            .expect("a code for a phone");
        assert_eq!(code, "https://login.tailscale.com/a/1a2b3c4d5e6f");
        let said = words(&screen);
        assert!(
            said.contains("1a2b3c4d5e6f"),
            "the address is not written out"
        );
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }

    #[test]
    fn the_sign_in_screen_stays_closed_once_the_owner_has_left_it() {
        let mut app = with_status(true, "starting\n\nStarting Tailscale.\n\n\n");
        let mut context = Context::default();
        app.load_status(&mut context, Some(WAITING.as_bytes()));
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.view, View::Status);
        // The same link read again a few seconds later.
        app.load_status(&mut context, Some(WAITING.as_bytes()));
        assert_eq!(app.view, View::Status);
        let said = words(&app.status_screen());
        assert!(said.contains("Approve this reader"));
        assert!(said.contains("Waiting for approval"));
        app.on_action(&mut context, action_id("sign-in"));
        assert_eq!(app.view, View::SignIn);
    }

    #[test]
    fn approval_closes_the_sign_in_screen_and_shows_the_name_and_address() {
        let mut app = with_status(true, WAITING);
        app.view = View::SignIn;
        let mut context = Context::default();
        app.load_status(&mut context, Some(CONNECTED.as_bytes()));
        assert_eq!(app.view, View::Status);
        let said = words(&app.status_screen());
        for expected in [
            "Connected",
            "kobo-cobalt.example.ts.net",
            "100.64.0.7",
            "Runs while Cobalt is open.",
        ] {
            assert!(said.contains(expected), "missing {expected:?} in {said}");
        }
        assert!(!said.contains("Approve this reader"));
    }

    #[test]
    fn every_state_the_runtime_reports_is_said_in_plain_words_that_fit() {
        for (status, expected) in [
            ("", "Starting"),
            ("starting\n\nStarting Tailscale.\n\n\n", "Starting"),
            (
                "downloading\n\nDownloading Tailscale. This takes a few minutes the first time.\n\n\n",
                "Downloading Tailscale.",
            ),
            (WAITING, "waiting to be approved"),
            (CONNECTED, "Connected"),
            (
                "offline\n\nWaiting for Wi-Fi. Tailscale starts by itself when the reader is online.\n\n\n",
                "Waiting for Wi-Fi.",
            ),
            (
                "failed\n\nTailscale could not be downloaded. Check Wi-Fi and try again.\n\n\n",
                "could not be downloaded",
            ),
            (
                "refused\n\nRoot can still log in to this reader with a password. Run kobo setup on your computer to close that, then turn Tailscale on again.\n\n\n",
                "Run kobo setup",
            ),
            (
                "unsupported\n\nThis reader's software has no tunnel support, so Tailscale cannot run on it.\n\n\n",
                "Not supported",
            ),
            (
                "timed-out\n\nSign-in was not finished in time. Tap Try again for a new code.\n\n\n",
                "Sign-in timed out",
            ),
            (
                "needs-approval\n\nSigned in. Somebody who manages your tailnet has to approve this reader. Tap Try again once they have.\n\n\n",
                "Waiting for an admin",
            ),
            (
                "stopped\n\nTailscale stopped when you left Cobalt. It starts again the next time you open Cobalt.\n\n\n",
                "Starting",
            ),
        ] {
            let screen = with_status(true, status).status_screen();
            let report = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
            assert!(
                !report.has_errors(),
                "{status:?} does not fit: {:?}",
                report.issues
            );
            let said = words(&screen);
            assert!(said.contains(expected), "missing {expected:?} in {said}");
        }
        let off = words(&with_status(false, CONNECTED).status_screen());
        assert!(off.contains("Off"));
        assert!(!off.contains("100.64.0.7"), "an address shown while off");
    }

    #[test]
    fn a_start_that_did_not_finish_offers_another_try_which_saves_the_switch_again() {
        let mut runner = AppRunner::new(with_status(
            true,
            "timed-out\n\nSign-in was not finished in time. Tap Try again for a new code.\n\n\n",
        ));
        let again = runner.action(action_id("retry"));
        assert_eq!(
            saves(&again),
            [(CONFIG.to_owned(), "true\nfalse\nfalse\nfalse\n".to_owned())]
        );
        assert!(words(screen(&again)).contains("Starting"));
    }

    #[test]
    fn signing_out_asks_first_then_saves_the_request_before_the_switch() {
        let mut runner = AppRunner::new(with_status(true, CONNECTED));
        runner.action(action_id("about"));
        let asked = runner.action(action_id("sign-out"));
        assert!(saves(&asked).is_empty(), "signed out without being asked");
        assert!(words(screen(&asked)).contains("forgets its key"));
        // Cancel goes back to the page the question was asked from.
        runner.action(action_id("back"));
        assert_eq!(runner.app().view, View::About);
        assert!(runner.app().enabled);
        runner.action(action_id("sign-out"));
        let confirmed = runner.action(action_id("sign-out-confirmed"));
        assert_eq!(
            saves(&confirmed),
            [
                (REQUEST.to_owned(), SIGN_OUT.to_owned()),
                (CONFIG.to_owned(), "false\nfalse\nfalse\nfalse\n".to_owned()),
            ]
        );
        assert!(!runner.app().enabled);
        assert_eq!(runner.app().view, View::Status);
    }

    #[test]
    fn runtime_back_returns_from_every_supporting_page() {
        let mut runner = AppRunner::new(with_status(true, WAITING));
        for action in ["sign-in", "about", "options"] {
            let opened = runner.action(action_id(action));
            assert!(screen(&opened).owns_back, "{action} must receive Back");
            assert!(
                screen(&opened)
                    .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
                    .issues
                    .is_empty(),
                "{action} has layout issues"
            );
            let returned = runner.action(ActionId::BACK);
            assert_eq!(runner.app().view, View::Status);
            assert!(!screen(&returned).owns_back, "root Back leaves the app");
        }
    }

    #[test]
    fn staying_connected_is_off_until_asked_for_and_says_what_it_costs_once_on() {
        let mut runner = AppRunner::new(with_status(true, CONNECTED));
        let options = runner.action(action_id("options"));
        let said = words(screen(&options));
        assert!(said.contains("Stay connected outside Cobalt"));
        assert!(
            !said.contains("battery"),
            "a cost shown for a switch that is off"
        );

        let asked = runner.action(action_id("stay"));
        assert_eq!(
            saves(&asked),
            [(CONFIG.to_owned(), "true\ntrue\nfalse\nfalse\n".to_owned())]
        );
        let said = words(screen(&asked));
        for expected in [
            "Uses more battery.",
            "only while the reader is awake",
            "After a restart, open Cobalt once to connect again.",
        ] {
            assert!(said.contains(expected), "missing {expected:?} in {said}");
        }
        assert!(screen(&asked)
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
        // And the first screen says which of the two it is doing.
        let first = runner.action(ActionId::BACK);
        assert!(words(screen(&first)).contains("Stays connected after you leave Cobalt."));
    }

    #[test]
    fn a_reader_that_cannot_stay_connected_says_why_and_does_not_claim_to() {
        let mut app = with_status(
            true,
            "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\nThe reader's system storage does not have the 110 MB free that staying connected outside Cobalt needs. Tailscale runs while Cobalt is open instead.\n",
        );
        app.stay = true;
        let options = app.options_screen();
        assert!(words(&options).contains("does not have the 110 MB free"));
        assert!(!options
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .has_errors());
        let first = words(&app.status_screen());
        assert!(first.contains("Runs while Cobalt is open."));
        assert!(!first.contains("Stays connected"));
    }

    const ROUTING: &str = "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\n\napproved\nwaiting\n192.168.1.0/24\n\n\n";

    fn routing_app(status: &str) -> Tailscale {
        Tailscale {
            exit_node: true,
            share_network: true,
            ..with_status(true, status)
        }
    }

    #[test]
    fn the_exit_node_and_the_shared_network_are_off_until_each_is_switched_on() {
        let mut runner = AppRunner::new(with_status(true, CONNECTED));
        let options = runner.action(action_id("options"));
        let said = words(screen(&options));
        assert!(said.contains("Offer as an exit node"));
        assert!(said.contains("Share this network"));
        assert!(
            !said.contains("admin console"),
            "approval asked for nothing"
        );

        let exit = runner.action(action_id("exit-node"));
        assert_eq!(
            saves(&exit),
            [(CONFIG.to_owned(), "true\nfalse\ntrue\nfalse\n".to_owned())]
        );
        let share = runner.action(action_id("share-network"));
        assert_eq!(
            saves(&share),
            [(CONFIG.to_owned(), "true\nfalse\ntrue\ntrue\n".to_owned())]
        );
        let said = words(screen(&share));
        assert!(said.contains("Approve it once in the Tailscale admin console"));
        assert!(said.contains("stops carrying traffic when it sleeps"));
    }

    #[test]
    fn each_offer_says_whether_the_tailnet_has_approved_it() {
        let options = routing_app(ROUTING).options_screen();
        let said = words(&options);
        for expected in [
            "Approved.",
            "Waiting for approval: 192.168.1.0/24",
            "Approve it once in the Tailscale admin console",
        ] {
            assert!(said.contains(expected), "missing {expected:?} in {said}");
        }
        assert!(options
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
        let both = routing_app(
            "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\n\napproved\napproved\n192.168.1.0/24\n\n\n",
        );
        let said = words(&both.options_screen());
        assert!(said.contains("Shared: 192.168.1.0/24"));
        // Nothing is waiting, so where to approve is no longer said. What
        // it costs still is.
        assert!(!said.contains("admin console"));
        assert!(said.contains("stops carrying traffic when it sleeps"));
    }

    #[test]
    fn an_offer_this_reader_cannot_make_says_why_and_never_just_sits_there_on() {
        let reason = "Tailscale on this reader could not carry traffic for other devices, so it is not an exit node and does not share its network.";
        let refused = routing_app(&format!(
            "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\n\nunavailable\nunavailable\n\n{reason}\n{reason}\n"
        ));
        let options = refused.options_screen();
        let said = words(&options);
        assert!(said.contains("Not available on this reader."));
        assert!(said.contains("Not available here."));
        assert_eq!(said.matches("could not carry traffic").count(), 1);
        assert!(
            !said.contains("admin console"),
            "approval asked for nothing"
        );
        assert!(!options
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .has_errors());

        // The network alone can be the problem, for its own reason.
        let hotel = routing_app(
            "running\n100.64.0.7\nConnected.\n\nkobo-cobalt.example.ts.net\n\nwaiting\nunavailable\n\n\nThe Wi-Fi this reader is on is not a private home network, so it is not shared.\n",
        );
        let said = words(&hotel.options_screen());
        assert!(said.contains("Waiting for approval."));
        assert!(said.contains("not a private home network"));
    }

    #[test]
    fn no_title_has_a_line_under_it_that_only_says_the_title_again() {
        // Every screen, in the states that show the most rows and switches.
        let mut waiting = with_status(true, WAITING);
        waiting.shown_link.clone_from(&waiting.status.sign_in);
        let screens = [
            with_status(false, "").status_screen(),
            waiting.status_screen(),
            with_status(
                true,
                "timed-out\n\nSign-in was not finished in time. Tap Try again for a new code.\n\n\n",
            )
            .status_screen(),
            with_status(true, CONNECTED).options_screen(),
            routing_app(ROUTING).options_screen(),
            about_builder().build(),
        ];
        let said = screens.iter().map(words).collect::<Vec<_>>().join(" ");
        for explains in [
            "Show the code",
            "Start Tailscale again",
            "Stay connected, exit node",
            "What it does",
            "Forget this reader's key before",
            "Stops when you leave Cobalt",
            "Starts with Tailscale",
            "Your devices can choose it",
        ] {
            assert!(
                !said.contains(explains),
                "{explains:?} is back under a title"
            );
        }
        // What does stand under a title is a state, never a description.
        let off = words(&with_status(false, "").status_screen());
        assert!(off.contains("Off. Nothing reaches this reader over a tailnet."));
        // And a switch that is off says only its name.
        // The band's clock is the only other thing the page draws.
        assert_eq!(
            words(&with_status(true, CONNECTED).options_screen()),
            "00:00 Tailscale options Stay connected outside Cobalt Offer as an exit node Share this network"
        );
    }

    #[test]
    fn choices_saved_by_an_app_from_before_the_options_are_read_as_off() {
        let mut runner = AppRunner::new(Tailscale::default());
        runner.store_result(StoreResult::Loaded {
            key: CONFIG.to_owned(),
            value: Some(b"true".to_vec()),
        });
        assert!(runner.app().enabled);
        assert!(!runner.app().stay);
        assert!(!runner.app().exit_node);
        assert!(!runner.app().share_network);
    }

    #[test]
    fn the_status_parses_what_an_older_supervisor_wrote() {
        let status = ServiceStatus::parse("running\n100.64.0.7\nConnected. 2 peer(s).\n");
        assert_eq!(status.state, "running");
        assert_eq!(status.address, "100.64.0.7");
        assert!(status.sign_in.is_empty());
        assert!(status.name.is_empty());
    }
}
