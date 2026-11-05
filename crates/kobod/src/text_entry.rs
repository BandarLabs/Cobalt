//! The keyboard the runtime draws when an application asks for text.
//!
//! Shared by the device host and the simulator, so a script that types in the
//! simulator types on the same keys, laid out the same way, that the reader
//! will draw.
//!
//! ## What stays on the panel
//!
//! The field replaces the application's screen rather than sitting over it,
//! and keeps the application's title in the top bar. An application's screen
//! was laid out for the whole panel, so the keys can only sit under it by
//! cutting off whatever was at its foot, and at the largest text size on a
//! small panel turned on its side that is everything below the title. What a
//! reader needs while typing is which application is asking and what for: the
//! title says the first, and the prompt and its one line of guidance, drawn
//! here in the runtime's own type, say the second. Nothing the application
//! drew can be tapped while the field is up, so there is no control behind
//! the keys that could act on text the application has not been given yet.
//!
//! The way out is the runtime's own Back, in the top bar where it is on every
//! other screen. A Cancel under the keys as well would be a second control
//! meaning the same thing, and the row it took is the row the largest text
//! size needs.
//!
//! When even that does not fit, the guidance goes first, then the prompt moves
//! into the top bar in place of the title, and last only the end of a long
//! entry is shown. The keys never shrink below a fingertip.

use kobo_protocol::{
    DenyReason, DeviceError, DeviceRequest, DeviceResult, SecretTarget, TextOutcome,
};
use kobo_ui::keyboard::{self, Keyboard, Pressed};
use kobo_ui::{
    ActionId, Chrome, DiagnosticSeverity, DisplayMetrics, FieldKind, Node, NodeId, Screen, TopBar,
};
use std::fmt;

/// The top bar control that shows a secret, or hides it again.
pub const REVEAL: &str = "kb.reveal";

/// The identity of the screen the runtime draws for a field, so a capture or
/// a log line says whose screen it was.
const SCREEN_NAME: &str = "cobalt.keyboard";

/// How many characters of a long entry are kept on the panel when the whole
/// of it does not fit above the keys.
const TAILS: [usize; 2] = [80, 24];

/// A text field the runtime is holding open for an application.
pub struct Field {
    prompt: String,
    hint: String,
    submit: String,
    save_as: Option<SecretTarget>,
    keyboard: Keyboard,
    revealed: bool,
}

/// Written out by hand so that a field holding a password can be logged.
impl fmt::Debug for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Field")
            .field("prompt", &self.prompt)
            .field("kind", &self.keyboard.kind())
            .field("characters", &self.keyboard.count())
            .finish_non_exhaustive()
    }
}

/// What a tap did to an open field.
#[derive(Debug, Eq, PartialEq)]
pub enum Step {
    /// The field or its keys changed. Draw it again.
    Redraw,
    /// The owner is done. Answer the application, and draw its screen again.
    Finished(Finished),
}

/// How the owner left a field.
#[derive(Eq, PartialEq)]
pub enum Finished {
    /// Text to hand to the application.
    Typed(String),
    /// A secret, as the request that stores it. It is answered as
    /// [`TextOutcome::Saved`] and never handed to the application.
    Secret(DeviceRequest),
    Cancelled,
}

impl fmt::Debug for Finished {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Typed(text) => formatter.debug_tuple("Typed").field(text).finish(),
            Self::Secret(_) => formatter.write_str("Secret"),
            Self::Cancelled => formatter.write_str("Cancelled"),
        }
    }
}

impl Field {
    /// Opens a field for `app`'s request.
    ///
    /// `None` when the request is not a text request. A secret that `app`
    /// may not keep under that name is refused here, before the owner types
    /// a word of it, with the answer to send instead.
    ///
    /// # Errors
    ///
    /// The [`DeviceResult`] that refuses the request.
    #[must_use]
    pub fn open(app: &str, request: &DeviceRequest) -> Option<Result<Self, DeviceResult>> {
        let DeviceRequest::TypeText {
            prompt,
            hint,
            initial,
            submit,
            kind,
            save_as,
        } = request
        else {
            return None;
        };
        if let Some(target) = save_as {
            // Asked with a stand-in value: the question is whether this
            // application may keep a secret under this name at all, and the
            // answer must come before the owner types the real one.
            if let Some(Err(refusal)) =
                kobo_policy::credentials::validate_install(app, &target.install("x"))
            {
                return Some(Err(DeviceResult::Text(match refusal {
                    DeviceResult::Denied(reason) => TextOutcome::Denied(reason),
                    DeviceResult::Failed(error) => TextOutcome::Failed(error),
                    _ => TextOutcome::Denied(DenyReason::PolicyRejected),
                })));
            }
        }
        Some(Ok(Self {
            prompt: prompt.clone(),
            hint: hint.clone(),
            submit: submit.clone(),
            save_as: save_as.clone(),
            keyboard: Keyboard::for_kind(*kind).typed(initial.as_str()),
            revealed: false,
        }))
    }

    #[must_use]
    pub fn kind(&self) -> FieldKind {
        self.keyboard.kind()
    }

    /// Applies one tap on the field's screen.
    ///
    /// `None` for a tap that meant nothing here, which is answered by doing
    /// nothing: the application's controls are not on this screen, so there
    /// is nobody else it could have been for.
    pub fn press(&mut self, action: ActionId) -> Option<Step> {
        if action == ActionId::BACK {
            return Some(Step::Finished(Finished::Cancelled));
        }
        if action == ActionId::from_name(REVEAL) && self.kind() == FieldKind::Secret {
            self.revealed = !self.revealed;
            return Some(Step::Redraw);
        }
        Some(match self.keyboard.press(action)? {
            Pressed::Edited | Pressed::Shifted => Step::Redraw,
            Pressed::Submitted => Step::Finished(self.finish()),
        })
    }

    fn finish(&mut self) -> Finished {
        let text = self.keyboard.take();
        match &self.save_as {
            // Exactly as typed. Spaces can be part of a password, and an
            // empty one is nothing to save, which is the same as backing out.
            Some(_) if text.is_empty() => Finished::Cancelled,
            Some(target) => Finished::Secret(target.install(&text)),
            None => Finished::Typed(text),
        }
    }

    /// The screen for this field, as the application whose screen is
    /// `behind` asked for it, fitted to `metrics`.
    #[must_use]
    pub fn screen(&self, behind: Option<&Screen>, metrics: &DisplayMetrics) -> Screen {
        let title = behind
            .and_then(|screen| screen.top_bar.as_ref())
            .map(|bar| bar.title.trim())
            .filter(|title| !title.is_empty());
        let shapes = [
            Shape {
                title,
                hint: true,
                tail: None,
            },
            Shape {
                title,
                hint: false,
                tail: None,
            },
            Shape {
                title: None,
                hint: false,
                tail: None,
            },
            Shape {
                title: None,
                hint: false,
                tail: Some(TAILS[0]),
            },
            Shape {
                title: None,
                hint: false,
                tail: Some(TAILS[1]),
            },
        ];
        let chrome = Chrome::measuring(true);
        let mut drawn = Screen::new(0, Vec::new());
        for shape in &shapes {
            drawn = self.draw(shape);
            let fits = !drawn
                .diagnostics(metrics, &chrome)
                .issues
                .iter()
                .any(|issue| issue.severity == DiagnosticSeverity::Error);
            if fits {
                break;
            }
        }
        drawn
    }

    fn draw(&self, shape: &Shape<'_>) -> Screen {
        let mut nodes = Vec::new();
        let mut next = 2;
        let mut id = || {
            let id = NodeId(next);
            next += 1;
            id
        };
        let bar_title = match shape.title {
            // The prompt names the field, not the page: the bar already says
            // whose screen this is. Set at the first level it was a second
            // title, larger than the first, over a single line of input.
            Some(title) => {
                nodes.push(Node::Heading {
                    id: id(),
                    text: self.prompt.clone(),
                    level: 2,
                });
                title.to_owned()
            }
            None => self.prompt.clone(),
        };
        if shape.hint && !self.hint.trim().is_empty() {
            nodes.push(Node::Secondary {
                id: id(),
                text: self.hint.clone(),
            });
        }
        for line in self.field_lines(shape.tail) {
            nodes.push(match line {
                Line::Plain(text) => Node::Text {
                    id: id(),
                    text,
                    links: Vec::new(),
                },
                Line::Quiet(text) => Node::Secondary { id: id(), text },
            });
        }
        nodes.push(Node::Divider { id: id() });
        // The keys go to the foot of the panel, which is where thumbs are and
        // where every other platform puts them.
        nodes.push(Node::Flex { id: id() });
        for row in self.keyboard.keys(&self.submit) {
            nodes.push(Node::Grid {
                id: id(),
                columns: u8::try_from(row.len()).unwrap_or(kobo_ui::MAX_COLUMNS),
                square: false,
                cells: row.iter().map(keyboard::Key::cell).collect(),
            });
        }
        let mut bar = TopBar::new(NodeId(1), bar_title);
        if self.kind() == FieldKind::Secret {
            bar = bar.action(
                ActionId::from_name(REVEAL),
                if self.revealed { "Hide" } else { "Show" },
            );
        }
        let mut screen = Screen::new(ActionId::from_name(SCREEN_NAME).0, nodes);
        screen.top_bar = Some(bar);
        screen.owns_back = true;
        screen
    }

    /// What the field says, under the prompt.
    ///
    /// A secret is not on the panel unless the owner asks for it, and its
    /// length always is. Typing blind with no count meant a 24-character
    /// token copied by eye from a phone failed on one mis-tap with nothing on
    /// screen to say which; the count catches a dropped or doubled key
    /// against the length the service printed, and Show lets the owner read
    /// it back once, somewhere nobody is looking over their shoulder.
    fn field_lines(&self, tail: Option<usize>) -> Vec<Line> {
        let text = self.keyboard.text();
        if text.is_empty() {
            return vec![Line::Plain("Type here".to_owned())];
        }
        let shown = match tail {
            Some(keep) if self.keyboard.count() > keep => {
                let skip = self.keyboard.count() - keep;
                format!("\u{2026}{}", text.chars().skip(skip).collect::<String>())
            }
            _ => text.to_owned(),
        };
        if self.kind() != FieldKind::Secret {
            return vec![Line::Plain(shown)];
        }
        let count = match self.keyboard.count() {
            1 => "1 character entered".to_owned(),
            count => format!("{count} characters entered"),
        };
        if self.revealed {
            vec![Line::Plain(shown), Line::Quiet(count)]
        } else {
            vec![Line::Plain(count)]
        }
    }
}

/// One way of laying the field out, from the most it can say to the least.
struct Shape<'a> {
    /// The application's title for the top bar, with the prompt as a heading
    /// under it. `None` puts the prompt in the bar instead.
    title: Option<&'a str>,
    hint: bool,
    /// How many characters of the entry to show, from its end.
    tail: Option<usize>,
}

enum Line {
    Plain(String),
    Quiet(String),
}

/// The answer an application gets for a finished field.
///
/// `store` writes a secret and answers as [`DeviceRequest::SetSecret`] would.
/// It is passed in rather than called here because the device and the
/// simulator keep secrets in different places, and the simulator can be told
/// to fail a write the way a full card would.
pub fn answer(
    finished: Finished,
    store: impl FnOnce(&DeviceRequest) -> DeviceResult,
) -> DeviceResult {
    DeviceResult::Text(match finished {
        Finished::Typed(text) => TextOutcome::Typed(text),
        Finished::Cancelled => TextOutcome::Cancelled,
        Finished::Secret(install) => match store(&install) {
            DeviceResult::Done => TextOutcome::Saved,
            DeviceResult::Denied(reason) => TextOutcome::Denied(reason),
            DeviceResult::Failed(error) => TextOutcome::Failed(error),
            _ => TextOutcome::Failed(DeviceError::Backend),
        },
    })
}

/// A second request arriving while a field is open replaces it.
///
/// The application has moved on, and the newest field is the one it is
/// drawing. The first is answered as cancelled, so every request still gets
/// exactly one answer and in the order they were asked.
#[must_use]
pub const fn superseded() -> DeviceResult {
    DeviceResult::Text(TextOutcome::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::{answer, Field, Finished, Step, REVEAL};
    use kobo_protocol::{
        DenyReason, DeviceError, DeviceRequest, DeviceResult, SecretTarget, TextOutcome,
    };
    use kobo_ui::{
        ActionId, Chrome, DiagnosticSeverity, DisplayMetrics, FieldKind, LayoutKind, Node, Screen,
        TextScale, TopBar,
    };

    fn request(kind: FieldKind) -> DeviceRequest {
        DeviceRequest::TypeText {
            prompt: "Account key".into(),
            hint: "24 characters, from lichess.org/account/oauth/token".into(),
            initial: String::new(),
            submit: "Save".into(),
            kind,
            save_as: (kind == FieldKind::Secret).then(|| SecretTarget::named("lichess")),
        }
    }

    fn open(kind: FieldKind) -> Field {
        Field::open("lichess", &request(kind))
            .expect("a text request")
            .expect("lichess may keep a lichess token")
    }

    fn tap(field: &mut Field, name: &str) -> Option<Step> {
        field.press(ActionId::from_name(name))
    }

    fn app_screen() -> Screen {
        let mut screen = Screen::new(1, Vec::new());
        screen.top_bar = Some(TopBar::new(kobo_ui::NodeId(1), "Lichess"));
        screen
    }

    fn panels() -> Vec<(String, DisplayMetrics)> {
        kobo_profile::SUPPORTED_PROFILES
            .iter()
            .flat_map(|profile| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width fits layout"),
                    height: i32::try_from(profile.height).expect("profile height fits layout"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: TextScale::Default,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (format!("{} portrait", profile.id), portrait),
                    (format!("{} landscape", profile.id), landscape),
                ]
            })
            .collect()
    }

    fn drawn(screen: &Screen) -> String {
        format!("{screen:?}")
    }

    #[test]
    fn each_kind_opens_on_the_keys_its_answer_starts_with() {
        let first_key = |kind| {
            let screen = open(kind).screen(Some(&app_screen()), &kobo_ui::CLARA_BW_METRICS);
            screen
                .nodes
                .iter()
                .find_map(|node| match node {
                    Node::Grid { cells, .. } => cells
                        .iter()
                        .find(|cell| cell.action == ActionId::from_name("kb.r0c0"))
                        .map(|cell| cell.label.clone()),
                    _ => None,
                })
                .expect("the first key is drawn")
        };
        assert_eq!(first_key(FieldKind::Text), "q");
        assert_eq!(first_key(FieldKind::Secret), "q");
        assert_eq!(first_key(FieldKind::Number), "1");
        assert_eq!(first_key(FieldKind::Address), "q");
        let address = drawn(&open(FieldKind::Address).screen(None, &kobo_ui::CLARA_BW_METRICS));
        assert!(address.contains("label: \"@\""), "{address}");
        assert!(!address.contains("label: \"space\""), "{address}");
    }

    #[test]
    fn typed_text_comes_back_when_submitted_and_nothing_does_when_backed_out_of() {
        let mut field = open(FieldKind::Text);
        for key in ["kb.shift", "kb.r0c0", "kb.r0c1", "kb.space", "kb.r0c2"] {
            assert_eq!(tap(&mut field, key), Some(Step::Redraw));
        }
        assert_eq!(
            tap(&mut field, "kb.enter"),
            Some(Step::Finished(Finished::Typed("Qw e".into())))
        );
        let mut field = open(FieldKind::Text);
        tap(&mut field, "kb.r0c0");
        assert_eq!(
            field.press(ActionId::BACK),
            Some(Step::Finished(Finished::Cancelled))
        );
        assert_eq!(tap(&mut field, "something.of.the.apps"), None);
        assert_eq!(
            answer(Finished::Cancelled, |_| unreachable!("nothing is stored")),
            DeviceResult::Text(TextOutcome::Cancelled)
        );
    }

    #[test]
    fn a_secret_is_stored_and_the_application_is_told_only_that_it_was() {
        let mut field = open(FieldKind::Secret);
        for key in ["kb.r0c0", "kb.r0c1", "kb.layer", "kb.r1c9"] {
            tap(&mut field, key);
        }
        let screen = drawn(&field.screen(Some(&app_screen()), &kobo_ui::CLARA_BW_METRICS));
        assert!(!screen.contains("qw_"), "{screen}");
        assert!(screen.contains("3 characters entered"), "{screen}");
        assert!(!format!("{field:?}").contains("qw_"));

        tap(&mut field, REVEAL);
        let shown = drawn(&field.screen(Some(&app_screen()), &kobo_ui::CLARA_BW_METRICS));
        assert!(shown.contains("qw_"), "{shown}");
        assert!(shown.contains("Hide"), "{shown}");

        let Some(Step::Finished(finished)) = tap(&mut field, "kb.enter") else {
            panic!("enter finishes the field")
        };
        assert!(!format!("{finished:?}").contains("qw_"));
        let mut stored = None;
        let answered = answer(finished, |install| {
            stored = Some(install.clone());
            DeviceResult::Done
        });
        assert_eq!(answered, DeviceResult::Text(TextOutcome::Saved));
        assert!(matches!(
            stored,
            Some(DeviceRequest::SetSecret { name, value }) if name == "lichess" && value.as_str() == "qw_"
        ));
        assert!(!format!("{answered:?}").contains("qw_"));
    }

    #[test]
    fn a_secret_that_cannot_be_written_says_so_without_the_value() {
        let mut field = open(FieldKind::Secret);
        tap(&mut field, "kb.r0c0");
        let Some(Step::Finished(finished)) = tap(&mut field, "kb.enter") else {
            panic!("enter finishes the field")
        };
        assert_eq!(
            answer(finished, |_| DeviceResult::Failed(DeviceError::Backend)),
            DeviceResult::Text(TextOutcome::Failed(DeviceError::Backend))
        );
        let mut empty = open(FieldKind::Secret);
        assert_eq!(
            tap(&mut empty, "kb.enter"),
            Some(Step::Finished(Finished::Cancelled)),
            "an empty secret is nothing to save"
        );
    }

    #[test]
    fn a_secret_the_application_may_not_keep_is_refused_before_anything_is_typed() {
        let refused = Field::open("todo", &request(FieldKind::Secret))
            .expect("a text request")
            .expect_err("todo may not keep a lichess token");
        assert_eq!(
            refused,
            DeviceResult::Text(TextOutcome::Denied(DenyReason::NotDeclared))
        );
        assert!(Field::open("todo", &DeviceRequest::ReadBattery).is_none());
    }

    #[test]
    fn the_field_keeps_the_applications_title_and_says_what_is_asked() {
        let screen = open(FieldKind::Text).screen(Some(&app_screen()), &kobo_ui::CLARA_BW_METRICS);
        assert_eq!(
            screen.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Lichess")
        );
        assert!(screen.owns_back, "Back is the way out of a field");
        let text = drawn(&screen);
        assert!(text.contains("Account key"), "{text}");
        assert!(text.contains("lichess.org/account/oauth/token"), "{text}");
        let untitled = open(FieldKind::Text).screen(None, &kobo_ui::CLARA_BW_METRICS);
        assert_eq!(
            untitled.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Account key")
        );
    }

    /// Every key on every page, at every text size, on every reader held
    /// either way: drawn on the panel, a fingertip wide, and reaching the
    /// key it is drawn as.
    #[test]
    fn every_field_fits_and_every_key_is_reachable_at_every_size_held_either_way() {
        // Measured in the runtime's own type, as the panel will draw it.
        kobo_text::install(kobo_ui::CLARA_BW_METRICS).expect("the runtime's fonts");
        let pages: [&[&str]; 4] = [&[], &["kb.shift"], &["kb.layer"], &["kb.layer", "kb.shift"]];
        for (name, mut metrics) in panels() {
            for scale in TextScale::STEPS {
                metrics.text_scale = scale;
                for kind in [
                    FieldKind::Text,
                    FieldKind::Secret,
                    FieldKind::Number,
                    FieldKind::Address,
                ] {
                    for page in pages {
                        let mut field = open(kind);
                        for _ in 0..30 {
                            tap(&mut field, "kb.r0c1");
                        }
                        tap(&mut field, REVEAL);
                        for key in page {
                            tap(&mut field, key);
                        }
                        let screen = field.screen(Some(&app_screen()), &metrics);
                        let chrome = Chrome::for_screen(&screen, false, None);
                        let errors: Vec<_> = screen
                            .diagnostics(&metrics, &Chrome::measuring(true))
                            .issues
                            .into_iter()
                            .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                            .collect();
                        assert!(
                            errors.is_empty(),
                            "{name} at {scale:?}, {kind:?} {page:?}: {errors:?}"
                        );
                        let layout = screen.layout_with(&metrics, &chrome);
                        for node in &layout.nodes {
                            let LayoutKind::Cell(action, ..) = node.kind else {
                                continue;
                            };
                            let rect = node.rect;
                            assert!(
                                rect.width >= metrics.touch_target_minimum()
                                    && rect.height >= metrics.touch_target_minimum(),
                                "{name} at {scale:?}: a key is smaller than a fingertip"
                            );
                            assert_eq!(
                                layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                                Some(action),
                                "{name} at {scale:?}: a key is covered"
                            );
                        }
                    }
                }
            }
        }
    }
}
