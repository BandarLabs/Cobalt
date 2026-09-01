mod protocol;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const GATEWAY: &str = "gateway";
const CACHE: &str = "letters";
const REFRESH: &str = "refresh";
const REPLY: &str = "reply";
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
    letters: Vec<(String, String, String)>,
    open: usize,
    keyboard: Keyboard,
    task: Option<TaskId>,
    posting: bool,
    notice: Option<String>,
}
impl Post {
    fn show(&self, c: &mut Context) {
        c.set_screen(
            match self.view {
                View::Opening => ScreenBuilder::new("post-opening")
                    .top_bar("Post")
                    .activity("Opening", None)
                    .build(),
                View::Setup => self.setup(),
                View::Inbox => self.inbox(),
                View::Letter => self.letter(),
                View::Compose => self.compose(),
            }
            .with_own_back(matches!(self.view, View::Letter | View::Compose)),
        );
    }
    fn setup(&self) -> Screen {
        let mut screen = ScreenBuilder::new("post-setup")
            .top_bar("Post")
            .heading("Connect a Hermes gateway")
            .text(
                "Use your HTTPS gateway URL. Install its bearer token with kobo secret set hermes-post --device <ip>.",
            )
            .field(
                "gateway-url",
                self.keyboard.text(),
                "https://hermes.example.net",
            );
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen.keyboard(&self.keyboard, "Save gateway").build()
    }
    fn inbox(&self) -> Screen {
        let mut s = ScreenBuilder::new("post-inbox").top_bar("Post");
        s = if self.letters.is_empty() {
            s.splash(
                Some(Glyph::Chat),
                "No letters yet",
                "Letters appear here when your Hermes gateway delivers one.",
            )
        } else {
            s.rows(
                self.letters
                    .iter()
                    .enumerate()
                    .map(|(n, (_, title, body))| {
                        (
                            format!("letter.{n}"),
                            title.clone(),
                            excerpt(body),
                            Glyph::Chat,
                        )
                    }),
            )
        };
        if let Some(notice) = &self.notice {
            s = s.banner(BannerLevel::Attention, notice);
        }
        s.spacer(Space::Small)
            .button(REFRESH, "Check for letters")
            .build()
    }
    fn letter(&self) -> Screen {
        let Some((_, title, body)) = self.letters.get(self.open) else {
            return self.inbox();
        };
        let mut s = ScreenBuilder::new("post-letter")
            .top_bar("Post")
            .heading(title.clone())
            .text(body.clone());
        if let Some(notice) = &self.notice {
            s = s.banner(BannerLevel::Attention, notice);
        }
        s.spacer(Space::Small)
            .button(REPLY, "Write a reply")
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
    fn check(&mut self, c: &mut Context) {
        if self.task.is_none() {
            self.task = c.spawn(protocol::inbox(&self.gateway));
            self.notice = Some("Checking for letters…".into());
            self.show(c);
        }
    }
    fn save_cache(&self, c: &mut Context) {
        let text = self
            .letters
            .iter()
            .map(|(id, t, b)| format!("{id}\t{t}\t{}", b.replace('\n', "\\n")))
            .collect::<Vec<_>>()
            .join("\n");
        c.store().save(CACHE, text.into_bytes());
    }
}
fn excerpt(text: &str) -> String {
    text.chars().take(72).collect()
}
impl KoboApp for Post {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(GATEWAY);
        c.store().load(CACHE);
        self.show(c);
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        let StoreResult::Loaded { key, value } = r else {
            return;
        };
        let text = value
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        if key == GATEWAY {
            self.gateway = text;
        } else if key == CACHE {
            self.letters = text
                .lines()
                .filter_map(|line| {
                    let mut p = line.splitn(3, '\t');
                    Some((
                        p.next()?.into(),
                        p.next()?.into(),
                        p.next()?.replace("\\n", "\n"),
                    ))
                })
                .collect();
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
        if a == ActionId::BACK {
            self.view = View::Inbox;
            self.show(c);
            return;
        }
        if matches!(self.view, View::Setup | View::Compose) {
            if let Some(key) = self.keyboard.press(a) {
                if matches!(key, Pressed::Edited | Pressed::Shifted) {
                    self.show(c);
                }
                if matches!(key, Pressed::Submitted) {
                    let text = self.keyboard.text().trim().to_owned();
                    if self.view == View::Setup {
                        if text.starts_with("https://") {
                            self.gateway = text;
                            c.store().save(GATEWAY, self.gateway.clone().into_bytes());
                            self.view = View::Inbox;
                            self.keyboard.clear();
                            self.show(c);
                            self.check(c);
                        } else {
                            self.notice = Some("Use an https:// gateway URL.".into());
                            self.show(c);
                        }
                    } else if let Some((id, _, _)) = self.letters.get(self.open) {
                        if let Some(task) = c.spawn(protocol::reply(&self.gateway, id, &text)) {
                            self.task = Some(task);
                            self.posting = true;
                            self.notice = Some("Sending letter…".into());
                            self.show(c);
                        }
                    }
                }
                return;
            }
        }
        if a == action_id(REFRESH) {
            self.check(c);
        } else if a == action_id(REPLY) && self.view == View::Letter {
            self.keyboard.clear();
            self.view = View::Compose;
            self.show(c);
        } else if self.view == View::Inbox {
            if let Some(n) =
                (0..self.letters.len()).find(|n| a == action_id(&format!("letter.{n}")))
            {
                self.open = n;
                self.view = View::Letter;
                self.notice = None;
                self.show(c);
            }
        }
    }
    fn on_task(&mut self, c: &mut Context, id: TaskId, out: TaskOutcome) {
        if self.task != Some(id) {
            return;
        }
        self.task = None;
        match out {
            TaskOutcome::Completed(bytes) if !self.posting => {
                let found = protocol::letters(&bytes);
                if !found.is_empty() {
                    self.letters = found;
                    self.save_cache(c);
                }
                self.notice = None;
                self.show(c);
            }
            TaskOutcome::Completed(_) => {
                self.posting = false;
                self.keyboard.clear();
                self.notice = Some("Sent to Hermes.".into());
                self.view = View::Letter;
                self.show(c);
            }
            TaskOutcome::Failed(e) => {
                self.posting = false;
                self.notice = Some(format!(
                    "Off the air — {}. Check Wi-Fi or kobo secret set hermes-post.",
                    Failure::of(e).advice
                ));
                self.show(c);
            }
            TaskOutcome::Cancelled => {}
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
                (
                    "1".into(),
                    "Morning letter".into(),
                    "A completed note from Hermes.".into()
                );
                8
            ],
            ..Default::default()
        };
        assert!(!app.inbox().layout().nodes.is_empty());
    }
    #[test]
    fn excerpt_is_short() {
        assert_eq!(excerpt("one"), "one");
    }
}
