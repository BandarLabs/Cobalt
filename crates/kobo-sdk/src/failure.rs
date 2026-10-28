//! What to tell the owner when an account service fails, and how they fix it.
//!
//! [`crate::Failure`] reads a task error the way any reader would. An app that
//! talks to the owner's own account needs more than that, because the same
//! error means different things depending on whose server it is. A refused
//! token is the account, never the Wi-Fi or the address. A server on the home
//! network that does not answer is usually a reader that has left home. A
//! service that answers "429" may mean "slow down" or "your credit has run
//! out", and only what it said tells the two apart.
//!
//! The app describes its service once with [`Service`] and hands every failed
//! [`TaskOutcome`] to [`Service::problem`]. The [`Problem`] that comes back has
//! one sentence of copy naming the real cause and the fix. Draw it with
//! [`ScreenBuilder::problem_banner`] over whatever the app already has saved,
//! and fall back to [`ScreenBuilder::problem_state`] only when there is nothing
//! saved to show. A blank screen after a failed refresh reads as "everything
//! I kept is gone", which is almost never true.
//!
//! A banner says how bad it is with [`Problem::level`]: an account the
//! service refused is an error, a reader that is off the Wi-Fi is only
//! something to attend to. [`ScreenBuilder::problem_banner_with`] also puts
//! the one control that fixes it on the banner itself, "Join Wi-Fi", "Try
//! again" or "Sign in again", beside the words that explain it.
//!
//! A refusal that keeps the server's words arrives as a [`Refusal`] status
//! envelope in the task's bytes, and [`Service::problem`] reads it. The
//! transport can produce one today ([`kobo_net::fetch_from_with_refusals`],
//! which the computer-side commands use); a task reaches an app that way only
//! once the runtime forwards refusals for it. Until then the same call reads
//! the flattened [`TaskError`], so an app adopting this needs no change later.
//!
//! ```
//! use kobo_sdk::failure::Service;
//! use kobo_sdk::{TaskError, TaskOutcome};
//!
//! let service = Service::new("Readeck")
//!     .at("https://192.168.1.20:8000")
//!     .fixed_by("Run kobo connect readeck on your computer.");
//! let problem = service
//!     .problem(&TaskOutcome::Failed(TaskError::Unauthorized))
//!     .unwrap();
//! assert!(problem.needs_account());
//! assert!(problem.message.contains("kobo connect readeck"));
//! ```

use crate::{BannerLevel, ScreenBuilder, StandardState, TaskError, TaskOutcome, JOIN_WIFI};

pub use kobo_net::refusal::MAX_EXCERPT;
pub use kobo_net::Refusal;

/// The cause behind a failed request, in the terms the owner would act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cause {
    /// The service refused the account: a wrong, expired or revoked key.
    Rejected,
    /// The service accepted the account but will not let it do this.
    Forbidden,
    /// No account for this service is on the reader yet.
    Missing,
    /// Cobalt would not send this request with the account.
    Blocked,
    /// The service answered, but has nothing at that address.
    NotFound,
    /// The service asked the reader to wait before asking again.
    RateLimited,
    /// The service refused because the account has no credit or quota left.
    OutOfCredit,
    /// The reader has no network at all.
    Offline,
    /// A server on the home network did not answer.
    AwayFromHome,
    /// A server on the internet did not answer, or its certificate was not
    /// one the reader trusts.
    Unreachable,
    /// The server took too long.
    TimedOut,
    /// The answer was larger than the request allowed.
    TooLarge,
}

/// One failure, ready to show.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    pub cause: Cause,
    /// The real cause and the fix, in short sentences. Fits in a banner.
    pub message: String,
    /// Whether a Try again control could help without anything else changing.
    pub retryable: bool,
    /// The whole-screen state, for when there is nothing saved to show.
    pub state: StandardState,
}

impl Problem {
    /// Whether the account itself is what needs fixing.
    ///
    /// An app offers its "Sign in again" or "Connect" control when this is
    /// true, rather than a Try again that is known to fail the same way.
    #[must_use]
    pub const fn needs_account(&self) -> bool {
        matches!(
            self.cause,
            Cause::Rejected | Cause::Forbidden | Cause::Missing | Cause::Blocked
        )
    }

    /// How the banner for this problem is drawn.
    ///
    /// An error is something the service or Cobalt refused, which waiting
    /// will not mend: a refused account, a page that is not there, a file
    /// too large to open. Everything that may come right by itself, or by
    /// walking back into range of the Wi-Fi, is drawn as something to attend
    /// to, so an owner away from home is not told something is broken.
    #[must_use]
    pub const fn level(&self) -> BannerLevel {
        match self.cause {
            Cause::Rejected
            | Cause::Forbidden
            | Cause::Blocked
            | Cause::NotFound
            | Cause::OutOfCredit
            | Cause::TooLarge => BannerLevel::Error,
            Cause::Missing
            | Cause::RateLimited
            | Cause::Offline
            | Cause::AwayFromHome
            | Cause::Unreachable
            | Cause::TimedOut => BannerLevel::Attention,
        }
    }
}

/// Whether the server's words say the account has run out of credit.
///
/// Read from the body because the status does not settle it. `OpenAI` answers
/// 429 both for "slow down" and for `insufficient_quota`, and Anthropic
/// answers 400 for a credit balance that is too low.
fn out_of_credit(refusal: &Refusal) -> bool {
    let said = refusal.excerpt.to_ascii_lowercase();
    refusal.status == 402
        || [
            "insufficient_quota",
            "credit balance",
            "out of credit",
            "exceeded your current quota",
            "billing",
            "payment required",
        ]
        .iter()
        .any(|phrase| said.contains(phrase))
}

/// The account service an app talks to, named once so every failure can be
/// told in its terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Service<'a> {
    name: &'a str,
    server: Option<&'a str>,
    fix: Option<&'a str>,
}

impl<'a> Service<'a> {
    /// `name` is what the owner calls the service, such as "Miniflux".
    #[must_use]
    pub const fn new(name: &'a str) -> Self {
        Self {
            name,
            server: None,
            fix: None,
        }
    }

    /// The owner's server address. A server on the home network gets the
    /// home-network hint when it does not answer.
    #[must_use]
    pub const fn at(mut self, server: &'a str) -> Self {
        self.server = Some(server);
        self
    }

    /// One sentence saying how this app's account is fixed, such as "Run
    /// kobo connect readeck on your computer." Appended to every failure that
    /// the account is the cause of.
    #[must_use]
    pub const fn fixed_by(mut self, fix: &'a str) -> Self {
        self.fix = Some(fix);
        self
    }

    /// The problem behind a task's outcome, or `None` when it succeeded or
    /// was cancelled. A completed task whose bytes are a status envelope is a
    /// refusal, and is read as one.
    #[must_use]
    pub fn problem(&self, outcome: &TaskOutcome) -> Option<Problem> {
        match outcome {
            TaskOutcome::Failed(error) => Some(self.explain(*error)),
            TaskOutcome::Completed(bytes) => {
                Refusal::parse_envelope(bytes).map(|refusal| self.explain_refusal(&refusal))
            }
            TaskOutcome::Cancelled => None,
        }
    }

    /// The problem behind one task error.
    #[must_use]
    pub fn explain(&self, error: TaskError) -> Problem {
        match error {
            TaskError::Unauthorized => self.problem_for(Cause::Rejected, None),
            TaskError::NoCredential => self.problem_for(Cause::Missing, None),
            TaskError::Denied => self.problem_for(Cause::Blocked, None),
            TaskError::NotFound => self.problem_for(Cause::NotFound, None),
            TaskError::RateLimited(seconds) => self.problem_for(Cause::RateLimited, Some(seconds)),
            TaskError::Offline => self.problem_for(Cause::Offline, None),
            TaskError::Unreachable if self.on_home_network() => {
                self.problem_for(Cause::AwayFromHome, None)
            }
            TaskError::Unreachable => self.problem_for(Cause::Unreachable, None),
            TaskError::TimedOut => self.problem_for(Cause::TimedOut, None),
            TaskError::TooLarge => self.problem_for(Cause::TooLarge, None),
        }
    }

    /// The problem behind a refusal the runtime kept the body of.
    #[must_use]
    pub fn explain_refusal(&self, refusal: &Refusal) -> Problem {
        if out_of_credit(refusal) {
            return self.problem_for(Cause::OutOfCredit, None);
        }
        match refusal.status {
            401 => self.problem_for(Cause::Rejected, None),
            403 => self.problem_for(Cause::Forbidden, None),
            429 => self.problem_for(Cause::RateLimited, refusal.retry_after),
            _ => self.problem_for(Cause::NotFound, None),
        }
    }

    /// Whether the server lives on the owner's own network.
    ///
    /// Read from the address alone: a private IPv4 range, a link-local or
    /// unique-local IPv6 address, a single-label host, or one of the suffixes
    /// home routers hand out. The radio cannot say which network the reader
    /// is on, but it does not need to: a home server that does not answer is
    /// far more often a reader that has left home than a server that is down.
    #[must_use]
    pub fn on_home_network(&self) -> bool {
        self.server
            .and_then(host_of)
            .is_some_and(|host| home_host(&host))
    }

    fn account_fix(&self) -> String {
        self.fix.map_or_else(String::new, |fix| format!(" {fix}"))
    }

    fn problem_for(&self, cause: Cause, wait: Option<u32>) -> Problem {
        let name = self.name;
        let fix = self.account_fix();
        let (message, retryable, state) = match cause {
            Cause::Rejected => (
                format!("{name} did not accept your account. The key or password may be wrong, expired or revoked.{fix}"),
                false,
                StandardState::PermissionDenied,
            ),
            Cause::Forbidden => (
                format!("{name} accepted your account but will not let it do this. Give it more access on {name}, then try again."),
                false,
                StandardState::PermissionDenied,
            ),
            Cause::Missing => (
                format!("No {name} account is on this reader yet.{fix}"),
                false,
                StandardState::PermissionDenied,
            ),
            Cause::Blocked => (
                format!("Cobalt did not let this request use your {name} account.{fix}"),
                false,
                StandardState::PermissionDenied,
            ),
            Cause::NotFound => (
                format!("{name} answered but has nothing at that address. Check the server address, then try again."),
                false,
                StandardState::Error,
            ),
            Cause::RateLimited => (
                match wait.filter(|seconds| *seconds > 0) {
                    Some(seconds) if seconds >= 90 => format!(
                        "{name} asked this reader to slow down. Try again in {} minutes.",
                        seconds.div_ceil(60)
                    ),
                    Some(_) => format!("{name} asked this reader to slow down. Try again in a minute."),
                    None => format!("{name} asked this reader to slow down. Try again later."),
                },
                true,
                StandardState::Error,
            ),
            Cause::OutOfCredit => (
                format!("{name} says this account has no credit left. Add credit on the {name} website, then try again."),
                false,
                StandardState::PermissionDenied,
            ),
            Cause::Offline => (
                "This reader is not on Wi-Fi. Join Wi-Fi and try again.".to_owned(),
                true,
                StandardState::Offline,
            ),
            Cause::AwayFromHome => (
                format!("{name} did not answer. It is on your home network, so the reader must be at home on the same Wi-Fi, and the server must be running."),
                true,
                StandardState::Error,
            ),
            Cause::Unreachable => (
                format!("{name} did not answer, or its certificate is not one this reader trusts. Try again later."),
                true,
                StandardState::Error,
            ),
            Cause::TimedOut => (
                format!("{name} took too long to answer. Try again."),
                true,
                StandardState::Error,
            ),
            Cause::TooLarge => (
                "This is too large to open on the reader.".to_owned(),
                false,
                StandardState::Error,
            ),
        };
        Problem {
            cause,
            message,
            retryable,
            state,
        }
    }
}

/// The host of an `https://` or `http://` address, lowercased, without its
/// port or brackets.
fn host_of(server: &str) -> Option<String> {
    let rest = server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn home_host(host: &str) -> bool {
    if let Ok(address) = host.parse::<std::net::Ipv4Addr>() {
        return address.is_private() || address.is_link_local() || address.is_loopback();
    }
    if let Ok(address) = host.parse::<std::net::Ipv6Addr>() {
        let first = address.segments()[0];
        return (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || address.is_loopback();
    }
    !host.contains('.')
        || [
            ".local",
            ".lan",
            ".home",
            ".internal",
            ".home.arpa",
            ".localdomain",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

impl ScreenBuilder {
    /// Puts a problem in a banner above what the app already shows.
    ///
    /// The banner is for a failure that has to sit over content the reader is
    /// already looking at: the saved inbox, the last copy of the list. Use it
    /// whenever there is anything saved, and keep drawing that content below.
    #[must_use]
    pub fn problem_banner(self, problem: &Problem) -> Self {
        self.banner(problem.level(), &problem.message)
    }

    /// The same banner, carrying the one control that fixes the problem.
    ///
    /// A reader off the Wi-Fi gets Join Wi-Fi. A refused or missing account
    /// gets `sign_in`, labelled "Sign in again" or "Sign in", when the app
    /// has a sign-in to go to; pass `None` when the account is set up only
    /// from a computer, because a control that leads nowhere new teaches the
    /// owner to stop trusting controls. Anything worth asking again gets
    /// `retry`, labelled "Try again". Whatever the banner carries, the app
    /// does not need a second control for it elsewhere on the screen.
    #[must_use]
    pub fn problem_banner_with(
        self,
        problem: &Problem,
        retry: impl AsRef<str>,
        sign_in: Option<&str>,
    ) -> Self {
        let screen = self.problem_banner(problem);
        match (problem.cause, sign_in) {
            (Cause::Offline, _) => screen.banner_action(JOIN_WIFI, "Join Wi-Fi"),
            (Cause::Rejected | Cause::Blocked, Some(sign_in)) => {
                screen.banner_action(sign_in, "Sign in again")
            }
            (Cause::Missing, Some(sign_in)) => screen.banner_action(sign_in, "Sign in"),
            _ if problem.retryable => screen.banner_action(retry, "Try again"),
            _ => screen,
        }
    }

    /// A problem's whole-screen presentation, for when nothing is saved.
    ///
    /// Offline gets Join Wi-Fi beside Try again, as [`Self::failure_state`]
    /// does. A retryable problem gets Try again. A problem with the account
    /// gets no control here: the app adds its own sign-in control, because
    /// only it knows where that leads.
    #[must_use]
    pub fn problem_state(self, problem: &Problem, retry: impl AsRef<str>) -> Self {
        let screen = self.standard_state(problem.state, &problem.message);
        match (problem.state, problem.retryable) {
            (StandardState::Offline, _) => {
                screen.buttons([(JOIN_WIFI, "Join Wi-Fi"), (retry.as_ref(), "Try again")])
            }
            (_, true) => screen.primary_button(retry, "Try again"),
            (_, false) => screen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cause, Refusal, Service};
    use crate::{ScreenBuilder, TaskError, TaskOutcome};

    const FIX: &str = "Run kobo connect readeck on your computer.";

    fn readeck(server: &str) -> Service<'_> {
        Service::new("Readeck").at(server).fixed_by(FIX)
    }

    #[test]
    fn a_refused_key_blames_the_account_and_never_the_wifi_or_the_address() {
        let problem = readeck("https://read.example").explain(TaskError::Unauthorized);
        assert_eq!(problem.cause, Cause::Rejected);
        assert!(problem.needs_account());
        assert!(!problem.retryable);
        assert!(problem.message.contains("expired"), "{}", problem.message);
        assert!(problem.message.ends_with(FIX), "{}", problem.message);
        for blamed in ["Wi-Fi", "address", "network", "Off the air"] {
            assert!(!problem.message.contains(blamed), "{}", problem.message);
        }
    }

    #[test]
    fn a_home_server_that_does_not_answer_says_the_reader_must_be_at_home() {
        for server in [
            "https://192.168.1.20:8000",
            "https://10.0.0.4",
            "https://172.20.1.1/miniflux",
            "https://nas.local",
            "https://homeassistant:8123",
            "https://[fd12:3456::1]:8443",
            "https://books.home.arpa",
        ] {
            let problem = readeck(server).explain(TaskError::Unreachable);
            assert_eq!(problem.cause, Cause::AwayFromHome, "{server}");
            assert!(problem.message.contains("home network"), "{server}");
            assert!(!problem.needs_account());
        }
    }

    #[test]
    fn a_public_server_that_does_not_answer_mentions_its_certificate_not_home() {
        for server in [
            "https://read.example",
            "https://172.32.0.1",
            "https://8.8.8.8",
        ] {
            let problem = readeck(server).explain(TaskError::Unreachable);
            assert_eq!(problem.cause, Cause::Unreachable, "{server}");
            assert!(problem.message.contains("certificate"), "{server}");
            assert!(!problem.message.contains("home"), "{server}");
        }
        assert_eq!(
            Service::new("Lichess")
                .explain(TaskError::Unreachable)
                .cause,
            Cause::Unreachable
        );
    }

    #[test]
    fn out_of_credit_is_told_apart_from_slow_down_by_what_the_server_said() {
        let service = Service::new("OpenAI");
        let quota = Refusal::parse_envelope(
            b"COBALT-HTTP/1 429\nRetry-After: 20\n\n{\"error\":{\"code\":\"insufficient_quota\"}}",
        )
        .expect("an envelope");
        assert_eq!(quota.status, 429);
        assert_eq!(quota.retry_after, Some(20));
        assert_eq!(service.explain_refusal(&quota).cause, Cause::OutOfCredit);

        let slow =
            Refusal::parse_envelope(b"COBALT-HTTP/1 429\nRetry-After: 300\n\n{\"error\":\"rate\"}")
                .expect("an envelope");
        let problem = service.explain_refusal(&slow);
        assert_eq!(problem.cause, Cause::RateLimited);
        assert!(problem.message.contains("5 minutes"), "{}", problem.message);

        let credit = Refusal::parse_envelope(
            b"COBALT-HTTP/1 400\nRetry-After: -\n\nYour credit balance is too low",
        )
        .expect("an envelope");
        assert_eq!(
            Service::new("Anthropic").explain_refusal(&credit).cause,
            Cause::OutOfCredit
        );
        let forbidden =
            Refusal::parse_envelope(b"COBALT-HTTP/1 403\nRetry-After: -\n\n").expect("an envelope");
        assert_eq!(service.explain_refusal(&forbidden).cause, Cause::Forbidden);
    }

    #[test]
    fn only_a_well_formed_envelope_is_read_as_a_refusal() {
        assert!(Refusal::parse_envelope(b"{\"items\":[]}").is_none());
        assert!(Refusal::parse_envelope(b"COBALT-HTTP/1 200\nRetry-After: -\n\n").is_none());
        assert!(Refusal::parse_envelope(b"COBALT-HTTP/1 429\nRetry-After: soon\n\n").is_none());
        assert!(Refusal::parse_envelope(b"COBALT-HTTP/1 429\nRetry-After: 5\n").is_none());
        let long = format!("COBALT-HTTP/1 400\nRetry-After: -\n\n{}", "x".repeat(600));
        assert!(Refusal::parse_envelope(long.as_bytes()).is_none());
        let service = Service::new("Readeck");
        assert_eq!(
            service.problem(&TaskOutcome::Completed(b"[]".to_vec())),
            None
        );
        assert_eq!(service.problem(&TaskOutcome::Cancelled), None);
        assert_eq!(
            service
                .problem(&TaskOutcome::Completed(
                    b"COBALT-HTTP/1 401\nRetry-After: -\n\n".to_vec()
                ))
                .map(|problem| problem.cause),
            Some(Cause::Rejected)
        );
    }

    #[test]
    fn a_request_cobalt_would_not_send_names_the_apps_own_fix() {
        let digest = Service::new("Miniflux").fixed_by(
            "Run kobo connect rss-miniflux on your computer to tie the key to your server.",
        );
        let problem = digest.explain(TaskError::Denied);
        assert_eq!(problem.cause, Cause::Blocked);
        assert!(problem.message.contains("kobo connect rss-miniflux"));
        assert!(!problem.message.contains("Wi-Fi"));
        assert_eq!(digest.explain(TaskError::Offline).cause, Cause::Offline);
        assert!(digest
            .explain(TaskError::Offline)
            .message
            .contains("Join Wi-Fi"));
    }

    #[test]
    fn a_refused_account_is_an_error_and_a_reader_away_from_home_is_not() {
        use crate::BannerLevel;
        let service = readeck("https://192.168.1.20");
        for (error, level) in [
            (TaskError::Unauthorized, BannerLevel::Error),
            (TaskError::Denied, BannerLevel::Error),
            (TaskError::NotFound, BannerLevel::Error),
            (TaskError::TooLarge, BannerLevel::Error),
            (TaskError::NoCredential, BannerLevel::Attention),
            (TaskError::Offline, BannerLevel::Attention),
            (TaskError::Unreachable, BannerLevel::Attention),
            (TaskError::TimedOut, BannerLevel::Attention),
            (TaskError::RateLimited(30), BannerLevel::Attention),
        ] {
            let problem = service.explain(error);
            assert_eq!(problem.level(), level, "{error:?}");
            let screen = ScreenBuilder::new("inbox").problem_banner(&problem).build();
            assert!(
                matches!(&screen.nodes[0], kobo_ui::Node::Banner { level: drawn, action: None, .. } if *drawn == level),
                "{screen:?}"
            );
        }
    }

    #[test]
    fn the_banner_carries_the_one_control_that_fixes_the_problem() {
        let service = readeck("https://read.example");
        let carried = |error: TaskError, sign_in: Option<&str>| {
            let screen = ScreenBuilder::new("inbox")
                .problem_banner_with(&service.explain(error), "refresh", sign_in)
                .build();
            match &screen.nodes[0] {
                kobo_ui::Node::Banner { action, .. } => action
                    .as_ref()
                    .map(|action| (action.action, action.label.clone())),
                other => panic!("a banner: {other:?}"),
            }
        };
        let id = crate::action_id;
        assert_eq!(
            carried(TaskError::Offline, Some("sign-in")),
            Some((id(crate::JOIN_WIFI), "Join Wi-Fi".to_owned()))
        );
        assert_eq!(
            carried(TaskError::Unauthorized, Some("sign-in")),
            Some((id("sign-in"), "Sign in again".to_owned()))
        );
        assert_eq!(
            carried(TaskError::NoCredential, Some("sign-in")),
            Some((id("sign-in"), "Sign in".to_owned()))
        );
        assert_eq!(
            carried(TaskError::Unauthorized, None),
            None,
            "an account fixed only from a computer has nothing to tap here"
        );
        assert_eq!(
            carried(TaskError::TimedOut, Some("sign-in")),
            Some((id("refresh"), "Try again".to_owned()))
        );
        assert_eq!(carried(TaskError::NotFound, Some("sign-in")), None);
    }

    #[test]
    fn a_banner_keeps_the_saved_list_on_screen_and_fits_the_clara() {
        let metrics = kobo_ui::CLARA_BW_METRICS;
        #[cfg(feature = "text")]
        kobo_text::install(metrics).unwrap();
        for error in [
            TaskError::Unauthorized,
            TaskError::Unreachable,
            TaskError::Offline,
            TaskError::Denied,
        ] {
            let problem = readeck("https://192.168.1.20").explain(error);
            let screen = ScreenBuilder::new("inbox")
                .top_bar("Readeck")
                .problem_banner_with(&problem, "refresh", Some("sign-in"))
                .text("Saved article")
                .build();
            let drawn = format!("{screen:?}");
            assert!(drawn.contains("Saved article"), "{drawn}");
            assert!(drawn.contains(&problem.message), "{drawn}");
            let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
            let errors: Vec<_> = screen
                .diagnostics(&metrics, &chrome)
                .issues
                .into_iter()
                .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "{error:?}: {errors:?}");

            let empty = ScreenBuilder::new("inbox")
                .top_bar("Readeck")
                .problem_state(&problem, "retry")
                .build();
            let drawn = format!("{empty:?}");
            assert_eq!(
                drawn.contains("Try again"),
                problem.retryable,
                "{error:?}: {drawn}"
            );
            assert_eq!(drawn.contains("Join Wi-Fi"), error == TaskError::Offline);
        }
    }
}
