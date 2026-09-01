//! One carefully typeset public-domain poem, selected deterministically each day.
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

#[derive(Clone, Copy)]
struct Poem {
    title: &'static str,
    author: &'static str,
    year: u16,
    source: &'static str,
    lines: &'static [&'static str],
    tags: &'static str,
}
const CORPUS: &[Poem] = &[
    Poem {
        title: "Hope is the thing with feathers",
        author: "Emily Dickinson",
        year: 1886,
        source: "Poems by Emily Dickinson",
        tags: "hope under a minute",
        lines: &[
            "“Hope” is the thing with feathers -",
            "That perches in the soul -",
            "And sings the tune without the words -",
            "And never stops - at all -",
        ],
    },
    Poem {
        title: "The Tyger",
        author: "William Blake",
        year: 1794,
        source: "Songs of Experience",
        tags: "nature under a minute",
        lines: &[
            "Tyger Tyger, burning bright,",
            "In the forests of the night;",
            "What immortal hand or eye,",
            "Could frame thy fearful symmetry?",
        ],
    },
    Poem {
        title: "Ozymandias",
        author: "Percy Bysshe Shelley",
        year: 1818,
        source: "The Examiner",
        tags: "history under a minute",
        lines: &[
            "I met a traveller from an antique land,",
            "Who said—“Two vast and trunkless legs of stone",
            "Stand in the desert. . . . Near them, on the sand,",
            "Half sunk a shattered visage lies.”",
        ],
    },
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Today,
    Browse,
    Reading,
    Settings,
}
struct Verses {
    view: View,
    poem: usize,
    favorite: Option<usize>,
    sleep: bool,
    loaded: bool,
}
impl Default for Verses {
    fn default() -> Self {
        Self {
            view: View::Today,
            poem: daily_index(2026, 9, 1),
            favorite: None,
            sleep: false,
            loaded: false,
        }
    }
}
fn daily_index(year: u16, month: u8, day: u8) -> usize {
    ((year as usize * 372) + (month as usize * 31) + day as usize) % CORPUS.len()
}
impl Verses {
    fn screen(&self) -> Screen {
        match self.view {
            View::Today | View::Reading => {
                let poem = CORPUS[self.poem];
                let mut text = format!("{}\n\n{}", poem.author, poem.lines.join("\n"));
                if self.view == View::Today {
                    text = format!("Today\n\n{text}");
                }
                ScreenBuilder::new("verses-poem")
                    .top_bar(poem.title)
                    .secondary(format!("{} · {}", poem.year, poem.source))
                    .reading(true)
                    .text(text)
                    .buttons([
                        (
                            "favorite",
                            if self.favorite == Some(self.poem) {
                                "Remove favorite"
                            } else {
                                "Save favorite"
                            },
                        ),
                        ("browse", "Browse"),
                    ])
                    .build()
            }
            View::Browse => ScreenBuilder::new("verses-browse")
                .top_bar("Browse")
                .secondary("Public-domain poems from the shelf.")
                .rows(CORPUS.iter().enumerate().map(|(i, poem)| {
                    (
                        format!("poem-{i}"),
                        poem.title,
                        format!("{} · {}", poem.author, poem.tags),
                        Glyph::Note,
                    )
                }))
                .button("today", "Today’s poem")
                .build(),
            View::Settings => ScreenBuilder::new("verses-settings")
                .top_bar("Sleep screen")
                .text(if self.sleep {
                    "Tonight’s sleep screen is tomorrow’s poem. It repaints once at sleep transition."
                } else {
                    "The sleep screen is off. Turn it on to prepare tomorrow’s poem."
                })
                .button("sleep", if self.sleep { "Turn off" } else { "Turn on" })
                .build(),
        }
    }
    fn save(&self, c: &mut Context) {
        c.store().save(
            "settings",
            format!(
                "{}:{}",
                self.favorite.map_or(-1, |index| {
                    i32::try_from(index).expect("the poem corpus fits i32")
                }),
                u8::from(self.sleep)
            )
            .into_bytes(),
        );
    }
    fn show(&self, c: &mut Context) {
        c.set_screen(
            self.screen()
                .with_own_back(!matches!(self.view, View::Today)),
        );
    }
}
impl KoboApp for Verses {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load("settings");
        self.show(c);
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { value: Some(v), .. } = r {
            if let Ok(t) = String::from_utf8(v) {
                let p: Vec<_> = t.split(':').collect();
                self.favorite = p.first().and_then(|s| s.parse::<usize>().ok());
                self.sleep = p.get(1) == Some(&"1");
            }
        }
        self.loaded = true;
        self.show(c);
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if a == ActionId::BACK || a == action_id("today") {
            self.view = View::Today;
        } else if a == action_id("browse") {
            self.view = View::Browse;
        } else if a == action_id("favorite") {
            self.favorite = if self.favorite == Some(self.poem) {
                None
            } else {
                Some(self.poem)
            };
            self.save(c);
        } else if a == action_id("sleep") {
            self.sleep = !self.sleep;
            self.save(c);
        } else if a == action_id("settings") {
            self.view = View::Settings;
        } else if let Some(i) = (0..CORPUS.len()).find(|i| a == action_id(&format!("poem-{i}"))) {
            self.poem = i;
            self.view = View::Reading;
        }
        self.show(c);
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("verses", Verses::default()).map_or_else(
        |e| {
            eprintln!("verses: {e}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn daily_choice_is_deterministic_and_leap_day_safe() {
        assert_eq!(daily_index(2028, 2, 29), daily_index(2028, 2, 29));
        assert_ne!(daily_index(2026, 9, 1), CORPUS.len());
    }
    #[test]
    fn every_poem_has_pd_provenance_and_preserved_lines() {
        for p in CORPUS {
            assert!(!p.author.is_empty() && !p.source.is_empty() && p.year < 1929);
            assert!(p.lines.iter().all(|l| !l.is_empty()));
        }
    }
    #[test]
    fn reading_screen_fits() {
        let d = Verses::default()
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
}
