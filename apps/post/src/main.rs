mod protocol;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, TaskError, TaskId, TaskOutcome,
};
use protocol::{Letter, Reply, ReplyState};
use std::process::ExitCode;

const GATEWAY: &str = "gateway";
const CACHE: &str = "letters";
const DRAFTS: &str = "drafts";
const OUTBOX: &str = "outbox";
const REFRESH: &str = "refresh";
const OLDER: &str = "older";
const NEWER: &str = "newer";
const REPLY: &str = "reply";
const RETRY: &str = "retry";

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum View {
    #[default]
    Opening,
    Setup,
    Inbox,
    Letter,
    Compose,
}

#[derive(Default)]
struct Post {
    view: View,
    gateway: String,
    letters: Vec<Letter>,
    places: Vec<(String, usize)>,
    total: usize,
    drafts: Vec<(String, String)>,
    outbox: Vec<Reply>,
    open: usize,
    keyboard: Keyboard,
    sending: Option<TaskId>,
    fetching: Option<TaskId>,
    fetching_page: usize,
    page_index: usize,
    advance_on_fetch: bool,
    loaded: bool,
    notice: Option<String>,
}

impl Post {
    fn show(&self, c: &mut Context) {
        let built = match self.view {
            View::Opening => ScreenBuilder::new("post-opening")
                .top_bar("Post")
                .activity("Opening", None)
                .build(),
            View::Setup => self.setup(),
            View::Inbox => self.inbox(),
            View::Letter => self.letter(c),
            View::Compose => self.compose(),
        };
        c.set_screen(built.with_own_back(matches!(self.view, View::Letter | View::Compose)));
    }

    fn setup(&self) -> Screen {
        let mut screen = ScreenBuilder::new("post-setup")
            .top_bar("Post")
            .heading("Connect a Hermes gateway")
            .text("Post reads letters from a Hermes gateway you run. On your computer, finish the gateway setup, then install its token with:")
            .text("kobo post login --gateway <address> --token-file <path> --device <reader>")
            .text("The token is pinned to your gateway and never enters this app. Then enter the same gateway address here.");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .field(
                "gateway-url",
                self.keyboard.text(),
                "https://hermes.example.net",
            )
            .keyboard(&self.keyboard, "Save gateway")
            .build()
    }

    fn inbox(&self) -> Screen {
        let mut s = ScreenBuilder::new("post-inbox")
            .top_bar("Post")
            .top_bar_action(REFRESH, "Check");
        s = if self.letters.is_empty() {
            s.splash(
                Some(Glyph::Chat),
                "No letters yet",
                "Letters appear here when your Hermes gateway delivers one.",
            )
        } else {
            let start = self.page_index * protocol::PER_PAGE;
            s.rows(
                self.letters
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(protocol::PER_PAGE)
                    .map(|(n, letter)| {
                        (
                            format!("letter.{n}"),
                            letter.title.clone(),
                            excerpt(&letter.body),
                            Glyph::Chat,
                        )
                    }),
            )
        };
        if let Some(notice) = &self.notice {
            s = s.banner(BannerLevel::Attention, notice);
        }
        let queued = self
            .outbox
            .iter()
            .filter(|reply| matches!(reply.state, ReplyState::Queued | ReplyState::Sending))
            .count();
        if queued > 0 {
            s = s.text(if queued == 1 {
                "1 reply waiting to be sent.".into()
            } else {
                format!("{queued} replies waiting to be sent.")
            });
        }
        if self.total > protocol::PER_PAGE {
            s = s.text(format!(
                "Page {} of {}",
                self.page_index + 1,
                self.total.div_ceil(protocol::PER_PAGE)
            ));
        }
        s.spacer(Space::Small)
            .action_bar([("newer", "Newer"), ("older", "Older")])
            .build()
    }

    fn letter(&self, c: &mut Context) -> Screen {
        let Some(letter) = self.letters.get(self.open) else {
            return self.inbox();
        };
        let pages = c.paginate_reading(&letter.body, true);
        let index = letter_page(&pages, self.place(&letter.id));
        let mut page = ScreenBuilder::new("post-letter")
            .top_bar(&letter.title)
            .reading(true)
            .page_position(
                u16::try_from(index + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        if let Some(reply) = self
            .outbox
            .iter()
            .find(|reply| reply.letter_id == letter.id)
        {
            page = page.text(format!("Your reply: {}.", reply.state.label()));
        }
        if let Some(notice) = &self.notice {
            page = page.banner(BannerLevel::Attention, notice);
        }
        if let Some(paragraphs) = pages.get(index) {
            for paragraph in paragraphs {
                page = page.text(paragraph);
            }
        }
        let write = if self
            .outbox
            .iter()
            .any(|reply| reply.letter_id == letter.id && reply.state == ReplyState::Rejected)
        {
            "Edit your reply"
        } else if self.drafts.iter().any(|(id, _)| *id == letter.id) {
            "Continue your reply"
        } else {
            "Write a reply"
        };
        page.spacer(Space::Small)
            .button(REPLY, write)
            .action_bar([
                ("previous-page", "Previous"),
                ("back", "Inbox"),
                ("next-page", "Next"),
            ])
            .build()
    }

    fn compose(&self) -> Screen {
        let mut s = ScreenBuilder::new("post-compose")
            .top_bar("Reply")
            .heading("Write a letter")
            .field("reply-body", self.keyboard.text(), "Your reply");
        if let Some(notice) = &self.notice {
            s = s.banner(BannerLevel::Attention, notice);
        }
        s.keyboard(&self.keyboard, "Send letter").build()
    }

    fn place(&self, id: &str) -> usize {
        self.places
            .iter()
            .find(|(letter, _)| letter == id)
            .map_or(0, |(_, place)| *place)
    }

    fn set_place(&mut self, id: &str, place: usize) {
        if let Some(entry) = self.places.iter_mut().find(|(letter, _)| letter == id) {
            entry.1 = place;
        } else {
            self.places.push((id.to_owned(), place));
        }
    }

    fn draft(&self, id: &str) -> Option<&str> {
        self.drafts
            .iter()
            .find(|(letter, _)| letter == id)
            .map(|(_, body)| body.as_str())
    }

    fn save_draft(&mut self, c: &mut Context, id: &str, body: &str) {
        if body.trim().is_empty() {
            self.drafts.retain(|(letter, _)| letter != id);
        } else if let Some(entry) = self.drafts.iter_mut().find(|(letter, _)| letter == id) {
            body.clone_into(&mut entry.1);
        } else {
            self.drafts.push((id.to_owned(), body.to_owned()));
        }
        c.store()
            .save(DRAFTS, protocol::encode_drafts(&self.drafts));
    }

    fn save_cache(&self, c: &mut Context) {
        c.store()
            .save(CACHE, protocol::encode_cache(&self.letters, &self.places));
    }

    fn save_outbox(&self, c: &mut Context) {
        c.store()
            .save(OUTBOX, protocol::encode_outbox(&self.outbox));
    }

    fn check(&mut self, c: &mut Context) {
        if self.fetching.is_none() {
            self.fetching = c.spawn(protocol::letters_page(&self.gateway, 1));
            self.fetching_page = 1;
            // A progress banner only fits while the list is empty.
            self.notice = if self.letters.is_empty() {
                Some("Checking for letters…".into())
            } else {
                None
            };
            self.show(c);
        }
        self.flush_replies(c);
    }

    fn flush_replies(&mut self, c: &mut Context) {
        if self.sending.is_some() || self.gateway.is_empty() {
            return;
        }
        let Some(next) = self
            .outbox
            .iter_mut()
            .find(|reply| reply.state == ReplyState::Queued)
        else {
            return;
        };
        next.state = ReplyState::Sending;
        let reply = next.clone();
        if let Some(task) = c.spawn(protocol::send_reply(&self.gateway, &reply)) {
            self.sending = Some(task);
            self.save_outbox(c);
            self.show(c);
        }
    }

    fn fetch_outcome(&mut self, c: &mut Context, out: TaskOutcome) {
        match out {
            TaskOutcome::Completed(bytes) => {
                if let Some((total, found)) = protocol::page(&bytes) {
                    self.total = total;
                    if self.fetching_page == 1 {
                        self.letters = found;
                        self.page_index = 0;
                    } else {
                        let known: Vec<String> =
                            self.letters.iter().map(|l| l.id.clone()).collect();
                        self.letters
                            .extend(found.into_iter().filter(|l| !known.contains(&l.id)));
                    }
                    if self.advance_on_fetch {
                        self.advance_on_fetch = false;
                        let last = self.letters.len().saturating_sub(1) / protocol::PER_PAGE;
                        self.page_index = (self.fetching_page - 1).min(last);
                    }
                    self.loaded = true;
                    self.notice = None;
                    self.save_cache(c);
                } else {
                    self.notice = Some(
                        "The gateway answered with something other than letters. Your cached list is unchanged."
                            .into(),
                    );
                }
            }
            TaskOutcome::Failed(TaskError::NoCredential) => {
                self.notice =
                    Some("Finish Post setup on your computer with `kobo post login`.".into());
            }
            TaskOutcome::Failed(e) => {
                self.notice = Some(format!(
                    "Couldn't connect — {}. Your cached letters are still here.",
                    Failure::of(e).advice
                ));
            }
            TaskOutcome::Cancelled => {}
        }
        self.show(c);
    }

    fn send_outcome(&mut self, c: &mut Context, out: TaskOutcome) {
        let completed = matches!(out, TaskOutcome::Completed(_));
        let Some(sent) = self
            .outbox
            .iter_mut()
            .find(|r| r.state == ReplyState::Sending)
        else {
            return;
        };
        match out {
            TaskOutcome::Completed(bytes) => {
                if protocol::reply_outcome(&bytes).is_some() {
                    sent.state = ReplyState::Delivered;
                    self.notice = Some("Sent to Hermes.".into());
                } else {
                    sent.state = ReplyState::Queued;
                    self.notice =
                        Some("The gateway's answer made no sense; the reply stays queued.".into());
                }
            }
            TaskOutcome::Failed(TaskError::NotFound) => {
                sent.state = ReplyState::Rejected;
                self.notice = Some(
                    "The gateway rejected the reply: it no longer knows the letter. Edit it and send again."
                        .into(),
                );
            }
            TaskOutcome::Failed(TaskError::TooLarge) => {
                sent.state = ReplyState::Rejected;
                self.notice = Some(
                    "The reply is too long for the gateway. Shorten it and send again.".into(),
                );
            }
            TaskOutcome::Failed(TaskError::NoCredential | TaskError::Unauthorized) => {
                sent.state = ReplyState::Queued;
                self.notice =
                    Some("Finish Post setup on your computer with `kobo post login`.".into());
            }
            TaskOutcome::Failed(TaskError::RateLimited(_)) => {
                sent.state = ReplyState::Queued;
                self.notice = Some(
                    "The gateway asked for a pause; the reply will send on the next check.".into(),
                );
            }
            TaskOutcome::Failed(_) | TaskOutcome::Cancelled => {
                sent.state = ReplyState::Queued;
                self.notice =
                    Some("The reply is still queued; it will send on the next connection.".into());
            }
        }
        let delivered = completed && sent.state == ReplyState::Delivered;
        self.save_outbox(c);
        self.show(c);
        // Only a delivered reply opens the gate for the next one; a failure
        // waits for the next check rather than retrying in a hot loop.
        if delivered {
            self.flush_replies(c);
        }
    }

    fn submit_reply(&mut self, c: &mut Context, letter_id: &str, text: &str) {
        if let Some(reply) = Reply::new(&self.gateway, letter_id, text) {
            self.outbox
                .retain(|old| old.letter_id != letter_id || old.state != ReplyState::Rejected);
            self.save_draft(c, letter_id, "");
            self.outbox.push(reply);
            self.save_outbox(c);
            self.keyboard.clear();
            self.view = View::Letter;
            self.notice = Some("Reply queued; sending on this connection.".into());
            self.show(c);
            self.flush_replies(c);
        } else {
            self.notice = Some("Write a reply of at most 32,000 characters.".into());
            self.show(c);
        }
    }

    fn submit_gateway(&mut self, c: &mut Context, text: &str) {
        if text.starts_with("https://") {
            text.clone_into(&mut self.gateway);
            c.store().save(GATEWAY, self.gateway.clone().into_bytes());
            self.view = View::Inbox;
            self.keyboard.clear();
            self.show(c);
            self.check(c);
        } else {
            self.notice = Some("Enter the secure https address of your Hermes gateway.".into());
            self.show(c);
        }
    }

    fn open_compose(&mut self, c: &mut Context) {
        if let Some(letter) = self.letters.get(self.open) {
            let rejected = self
                .outbox
                .iter()
                .find(|reply| reply.letter_id == letter.id && reply.state == ReplyState::Rejected)
                .map(|reply| reply.body.clone());
            let draft = self.draft(&letter.id).map(str::to_owned);
            self.keyboard = Keyboard::with_text(rejected.or(draft).unwrap_or_default());
            self.view = View::Compose;
            self.notice = None;
            self.show(c);
        }
    }
}

fn excerpt(text: &str) -> String {
    text.chars().take(72).collect()
}

fn page_words(page: &[String]) -> usize {
    page.iter()
        .map(|paragraph| paragraph.split_whitespace().count())
        .sum()
}

// A word offset survives display-size changes.
fn letter_page(pages: &[Vec<String>], position: usize) -> usize {
    let mut end = 0;
    for (index, page) in pages.iter().enumerate() {
        end += page_words(page);
        if position < end {
            return index;
        }
    }
    pages.len().saturating_sub(1)
}

impl KoboApp for Post {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(GATEWAY);
        c.store().load(CACHE);
        c.store().load(DRAFTS);
        c.store().load(OUTBOX);
        self.show(c);
    }

    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        let StoreResult::Loaded { key, value } = r else {
            return;
        };
        if key == GATEWAY {
            self.gateway = value
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
        } else if key == CACHE {
            if let Some(bytes) = value {
                if let Some((letters, places)) = protocol::decode_cache(&bytes) {
                    self.letters = letters;
                    self.places = places;
                }
            }
        } else if key == DRAFTS {
            if let Some(bytes) = value {
                if let Some(drafts) = protocol::decode_drafts(&bytes) {
                    self.drafts = drafts;
                }
            }
        } else if key == OUTBOX {
            if let Some(bytes) = value {
                if let Some(outbox) = protocol::decode_outbox(&bytes) {
                    self.outbox = outbox;
                }
            }
        }
        if self.view == View::Opening {
            self.view = if self.gateway.is_empty() {
                self.keyboard = Keyboard::with_text("https://");
                View::Setup
            } else {
                View::Inbox
            };
            self.show(c);
            if self.view == View::Inbox {
                self.check(c);
            }
        }
    }

    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if a == ActionId::BACK || a == action_id("back") {
            self.view = View::Inbox;
            self.show(c);
            return;
        }
        if matches!(self.view, View::Setup | View::Compose) {
            if let Some(key) = self.keyboard.press(a) {
                if matches!(key, Pressed::Edited | Pressed::Shifted) {
                    if self.view == View::Compose {
                        if let Some(letter) = self.letters.get(self.open) {
                            let id = letter.id.clone();
                            let body = self.keyboard.text().to_owned();
                            self.save_draft(c, &id, &body);
                        }
                    }
                    self.show(c);
                }
                if matches!(key, Pressed::Submitted) {
                    let text = self.keyboard.text().trim().to_owned();
                    if self.view == View::Setup {
                        self.submit_gateway(c, &text);
                    } else if let Some(letter) = self.letters.get(self.open) {
                        let id = letter.id.clone();
                        self.submit_reply(c, &id, &text);
                    }
                }
                return;
            }
        }
        if a == action_id(REFRESH) {
            self.check(c);
        } else if a == action_id(OLDER) && self.view == View::Inbox {
            let target = self.page_index + 1;
            if target * protocol::PER_PAGE < self.letters.len() {
                self.page_index = target;
                self.show(c);
            } else if self.letters.len() < self.total && self.fetching.is_none() {
                self.fetching = c.spawn(protocol::letters_page(&self.gateway, target + 1));
                self.fetching_page = target + 1;
                self.advance_on_fetch = true;
            }
        } else if a == action_id(NEWER) && self.view == View::Inbox {
            self.page_index = self.page_index.saturating_sub(1);
            self.show(c);
        } else if a == action_id(REPLY) && self.view == View::Letter {
            self.open_compose(c);
        } else if a == action_id(RETRY) {
            for reply in &mut self.outbox {
                if reply.state == ReplyState::Rejected {
                    reply.state = ReplyState::Queued;
                }
            }
            self.save_outbox(c);
            self.flush_replies(c);
            self.show(c);
        }
        if self.view == View::Inbox {
            if let Some(n) =
                (0..self.letters.len()).find(|n| a == action_id(&format!("letter.{n}")))
            {
                self.open = n;
                self.view = View::Letter;
                self.notice = None;
                self.show(c);
            }
        } else if self.view == View::Letter
            && (a == action_id("previous-page") || a == action_id("next-page"))
        {
            if let Some(letter) = self.letters.get(self.open) {
                let pages = c.paginate_reading(&letter.body, true);
                let current = letter_page(&pages, self.place(&letter.id));
                let next = if a == action_id("next-page") {
                    (current + 1).min(pages.len().saturating_sub(1))
                } else {
                    current.saturating_sub(1)
                };
                if next != current {
                    let place: usize = pages.iter().take(next).map(|p| page_words(p)).sum();
                    let id = letter.id.clone();
                    self.set_place(&id, place);
                    self.save_cache(c);
                }
            }
            self.show(c);
        }
    }

    fn on_task(&mut self, c: &mut Context, id: TaskId, out: TaskOutcome) {
        if self.fetching == Some(id) {
            self.fetching = None;
            self.fetch_outcome(c, out);
            return;
        }
        if self.sending == Some(id) {
            self.sending = None;
            self.send_outcome(c, out);
        }
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("post", Post::default())
        .map_or_else(|_| ExitCode::FAILURE, |()| ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_rows_fit_panel() {
        let app = Post {
            view: View::Inbox,
            letters: vec![
                Letter {
                    id: "1".into(),
                    title: "Morning letter".into(),
                    body: "A completed note from Hermes.".into()
                };
                8
            ],
            ..Default::default()
        };
        assert!(!app.inbox().layout().nodes.is_empty());
    }

    #[test]
    fn a_long_letter_pages_and_resumes() {
        let mut c = Context::default();
        let body = "One sentence per line.\n\n".repeat(120);
        let mut app = Post {
            view: View::Letter,
            letters: vec![Letter {
                id: "long".into(),
                title: "A long letter".into(),
                body: body.clone(),
            }],
            ..Default::default()
        };
        let pages = c.paginate_reading(&body, true);
        assert!(pages.len() > 1);
        app.on_action(&mut c, action_id("next-page"));
        let place = app.place("long");
        assert!(place > 0);
        // A reflow at a different size still lands on the same words.
        let changed = Context::default().paginate_reading(&body, true);
        let offset = page_words(&pages[0]);
        let target = letter_page(&changed, offset);
        let start: usize = changed.iter().take(target).map(|p| page_words(p)).sum();
        assert!(start <= offset && start + page_words(&changed[target]) > offset);
        assert_eq!(letter_page(&pages, place), 1);
    }

    #[test]
    fn a_restart_restores_inbox_drafts_and_outbox() {
        let mut c = Context::default();
        let mut app = Post {
            gateway: "https://gateway.example".into(),
            letters: vec![Letter {
                id: "dawn".into(),
                title: "Morning note".into(),
                body: "Tea first.".into(),
            }],
            ..Default::default()
        };
        app.open = 0;
        app.save_draft(&mut c, "dawn", "Hello from the reader");
        let reply = Reply::new("https://gateway.example", "dawn", "Tea first.").unwrap();
        app.outbox.push(reply.clone());
        app.save_outbox(&mut c);
        app.save_cache(&mut c);

        let mut fresh = Post::default();
        fresh.on_store(
            &mut c,
            StoreResult::Loaded {
                key: CACHE.into(),
                value: Some(protocol::encode_cache(&app.letters, &app.places)),
            },
        );
        fresh.on_store(
            &mut c,
            StoreResult::Loaded {
                key: DRAFTS.into(),
                value: Some(protocol::encode_drafts(&app.drafts)),
            },
        );
        fresh.on_store(
            &mut c,
            StoreResult::Loaded {
                key: OUTBOX.into(),
                value: Some(protocol::encode_outbox(&app.outbox)),
            },
        );
        assert_eq!(fresh.letters, app.letters);
        assert_eq!(
            fresh.drafts,
            vec![("dawn".to_string(), "Hello from the reader".to_string())]
        );
        assert_eq!(fresh.outbox, vec![reply]);
    }

    #[test]
    fn a_rejected_reply_retries_with_the_same_key() {
        let mut reply = Reply::new("https://gateway.example", "dawn", "Tea first.").unwrap();
        reply.state = ReplyState::Rejected;
        let mut app = Post {
            gateway: "https://gateway.example".into(),
            outbox: vec![reply.clone()],
            ..Default::default()
        };
        let mut c = Context::default();
        app.on_action(&mut c, action_id(RETRY));
        assert_eq!(app.outbox[0].state, ReplyState::Sending);
        assert_eq!(app.outbox[0].id, reply.id);
    }
}
