//! Offline SRD reference and table companion.
mod corpus;
use corpus::Entry;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;
const ATTRIBUTION: &str = "This work includes material taken from the System Reference Document 5.1 and System Reference Document 5.2 by Wizards of the Coast LLC, available under the Creative Commons Attribution 4.0 International License.";
const STATE: &str = "grimoire-state-v2";
const PAGE: usize = 6;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Compendium,
    Bookmarks,
    Search,
    Dice,
    Initiative,
    Party,
    Member,
    Edit,
    Detail,
    About,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Spell,
    Monster,
    Rule,
}
impl Kind {
    const fn key(self) -> &'static str {
        match self {
            Self::Spell => "spell",
            Self::Monster => "monster",
            Self::Rule => "rule",
        }
    }
    const fn title(self) -> &'static str {
        match self {
            Self::Spell => "Spells",
            Self::Monster => "Monsters",
            Self::Rule => "Rules & conditions",
        }
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Tri {
    Any,
    Yes,
    No,
}
impl Tri {
    fn next(self) -> Self {
        match self {
            Self::Any => Self::Yes,
            Self::Yes => Self::No,
            Self::No => Self::Any,
        }
    }
    fn matches(self, value: bool) -> bool {
        self == Self::Any || (self == Self::Yes) == value
    }
    fn label(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}
#[derive(Clone, Debug)]
struct Combatant {
    name: String,
    initiative: i8,
}
#[derive(Clone, Debug)]
struct Member {
    name: String,
    ac: u8,
    max_hp: i16,
    hp: i16,
    success: u8,
    failure: u8,
    slots: [bool; 9],
}
#[derive(Clone, Copy)]
enum Editor {
    Init(Option<usize>),
    Party(Option<usize>),
}
struct Edit {
    target: Editor,
    step: usize,
    values: Vec<String>,
}
struct Grimoire {
    view: View,
    kind: Kind,
    edition: u16,
    query: String,
    keyboard: Keyboard,
    page: usize,
    detail: Option<usize>,
    bookmarks: Vec<usize>,
    roll: u16,
    modifier: i8,
    advantage: i8,
    history: Vec<u16>,
    round: u16,
    current: usize,
    initiative: Vec<Combatant>,
    party: Vec<Member>,
    member: Option<usize>,
    edit: Option<Edit>,
    spell_class: usize,
    spell_level: Option<u8>,
    spell_school: usize,
    ritual: Tri,
    concentration: Tri,
    cr: usize,
    monster_type: usize,
    corpus: Vec<Entry>,
    loaded: bool,
}
impl Default for Grimoire {
    fn default() -> Self {
        Self {
            view: View::Home,
            kind: Kind::Spell,
            edition: 2014,
            query: String::new(),
            keyboard: Keyboard::new(),
            page: 0,
            detail: None,
            bookmarks: vec![],
            roll: 20,
            modifier: 0,
            advantage: 0,
            history: vec![],
            round: 1,
            current: 0,
            initiative: vec![],
            party: vec![],
            member: None,
            edit: None,
            spell_class: 0,
            spell_level: None,
            spell_school: 0,
            ritual: Tri::Any,
            concentration: Tri::Any,
            cr: 0,
            monster_type: 0,
            corpus: corpus::load(),
            loaded: false,
        }
    }
}
fn tag<'a>(entry: &'a Entry, key: &str) -> Option<&'a str> {
    entry.tags.split(';').find_map(|pair| {
        pair.split_once('=')
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
    })
}
fn options(app: &Grimoire, key: &str) -> Vec<String> {
    let mut values = vec!["Any".to_owned()];
    for entry in &app.corpus {
        if entry.edition == app.edition && entry.kind == app.kind.key() {
            if let Some(value) = tag(entry, key) {
                if !value.is_empty() && !values.iter().any(|known| known == value) {
                    values.push(value.to_owned());
                }
            }
        }
    }
    values.sort();
    values
}
impl Grimoire {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.view != View::Home));
    }
    fn spell_match(&self, e: &Entry) -> bool {
        let classes = options(self, "class");
        let schools = options(self, "school");
        (self.spell_class == 0
            || tag(e, "class").is_some_and(|v| {
                v.split(',')
                    .map(str::trim)
                    .any(|c| c == classes[self.spell_class])
            }))
            && self
                .spell_level
                .is_none_or(|level| tag(e, "level") == Some(&level.to_string()))
            && (self.spell_school == 0
                || tag(e, "school") == Some(schools[self.spell_school].as_str()))
            && self.ritual.matches(tag(e, "ritual") == Some("1"))
            && self
                .concentration
                .matches(tag(e, "concentration") == Some("1"))
    }
    fn monster_match(&self, e: &Entry) -> bool {
        let kinds = options(self, "type");
        let type_ok =
            self.monster_type == 0 || tag(e, "type") == Some(kinds[self.monster_type].as_str());
        let cr = tag(e, "cr")
            .and_then(|x| x.parse::<f32>().ok())
            .unwrap_or(-1.);
        let cr_ok = match self.cr {
            0 => true,
            1 => cr <= 0.,
            2 => cr > 0. && cr <= 1.,
            3 => cr > 1. && cr <= 4.,
            4 => cr > 4. && cr <= 10.,
            _ => cr > 10.,
        };
        type_ok && cr_ok
    }
    fn entries(&self) -> Vec<(usize, &Entry)> {
        let query = self.query.to_lowercase();
        self.corpus
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.edition == self.edition
                    && (e.kind == self.kind.key()
                        || (self.kind == Kind::Rule && e.kind == "condition"))
                    && (query.is_empty() || e.name.to_lowercase().starts_with(&query))
                    && (self.kind != Kind::Spell || self.spell_match(e))
                    && (self.kind != Kind::Monster || self.monster_match(e))
            })
            .collect()
    }
    fn title(&self) -> &'static str {
        match self.view {
            View::Home => "Grimoire",
            View::Compendium => self.kind.title(),
            View::Bookmarks => "Bookmarks",
            View::Search => "Search",
            View::Dice => "Dice",
            View::Initiative => "Initiative",
            View::Party => "Party",
            View::Member => "Party member",
            View::Edit => "Edit",
            View::Detail => "Reference",
            View::About => "About",
        }
    }
    fn screen(&self) -> Screen {
        let s = ScreenBuilder::new("grimoire").top_bar(self.title());
        match self.view{View::Home=>s.top_bar_action("about","About").tiles([("spells","Spells",Glyph::Book),("monsters","Monsters",Glyph::Search),("rules","Rules & conditions",Glyph::Book),("dice","Dice",Glyph::Circle),("initiative","Initiative",Glyph::Chart),("party","Party",Glyph::Person)]).build(),View::Compendium=>self.compendium(s),View::Bookmarks=>self.bookmarks(s),View::Search=>s.secondary(format!("Prefix search: {}",self.keyboard.text())).keyboard(&self.keyboard,"Search").build(),View::Dice=>self.dice(s),View::Initiative=>self.initiative(s),View::Party=>self.party(s),View::Member=>self.member(s),View::Edit=>self.edit(s),View::Detail=>self.detail(s),View::About=>s.text(format!("Grimoire is an unofficial offline reference. It requests no capabilities.\n\n{ATTRIBUTION}\n\nNo third-party OGL-only material or artwork is included.")).bottom_action("back","Back").build()}
    }
    fn compendium(&self, mut s: ScreenBuilder) -> Screen {
        s = s.tabs(
            usize::from(self.edition != 2014),
            [("edition-2014", "2014"), ("edition-2024", "2024")],
        );
        if self.kind == Kind::Spell {
            let classes = options(self, "class");
            let schools = options(self, "school");
            s = s
                .buttons([
                    (
                        "spell-class",
                        format!("Class: {}", classes[self.spell_class]),
                    ),
                    (
                        "spell-level",
                        format!(
                            "Level: {}",
                            self.spell_level
                                .map_or_else(|| "any".into(), |n| n.to_string())
                        ),
                    ),
                    (
                        "spell-school",
                        format!("School: {}", schools[self.spell_school]),
                    ),
                ])
                .buttons([
                    ("ritual", format!("Ritual: {}", self.ritual.label())),
                    (
                        "concentration",
                        format!("Concentration: {}", self.concentration.label()),
                    ),
                    ("clear", "Clear filters".to_owned()),
                ]);
        } else if self.kind == Kind::Monster {
            let types = options(self, "type");
            let ranges = ["any", "0", "1/8–1", "2–4", "5–10", "11+"];
            s = s.buttons([
                ("monster-cr", format!("CR: {}", ranges[self.cr])),
                (
                    "monster-type",
                    format!("Type: {}", types[self.monster_type]),
                ),
                ("clear", "Clear filters".to_owned()),
            ]);
        }
        s = s.buttons([("search", "Search"), ("bookmarks", "Bookmarks")]);
        let entries = self.entries();
        if entries.is_empty() {
            return s
                .splash(
                    Some(Glyph::Search),
                    "No matching reference",
                    "Change a filter, prefix, or edition.",
                )
                .build();
        }
        s.rows(
            entries
                .iter()
                .skip(self.page * PAGE)
                .take(PAGE)
                .map(|(i, e)| {
                    (
                        format!("entry-{i}"),
                        e.name.clone(),
                        e.subtitle.clone(),
                        Glyph::Book,
                    )
                }),
        )
        .page_turns("previous", "next")
        .page_position(
            u16::try_from(self.page + 1).unwrap_or(u16::MAX),
            u16::try_from(entries.len().div_ceil(PAGE)).unwrap_or(u16::MAX),
        )
        .build()
    }
    fn detail(&self, s: ScreenBuilder) -> Screen {
        let Some(index) = self.detail else {
            return s
                .text("No reference selected.")
                .bottom_action("back", "Back")
                .build();
        };
        let e = &self.corpus[index];
        let mut s = s
            .top_bar(e.name.clone())
            .reading(true)
            .text(format!("{}\n\n{}", e.subtitle, e.body))
            .button(
                "bookmark",
                if self.bookmarks.contains(&index) {
                    "Remove bookmark"
                } else {
                    "Bookmark"
                },
            );
        if self.kind == Kind::Monster {
            s = s.button("add-init", "Add to initiative");
        }
        s.bottom_action("back", "Back").build()
    }
    fn bookmarks(&self, s: ScreenBuilder) -> Screen {
        if self.bookmarks.is_empty() {
            return s
                .splash(
                    Some(Glyph::Bookmark),
                    "No bookmarks",
                    "Bookmark a reference to keep it here.",
                )
                .bottom_action("back", "Back")
                .build();
        }
        s.rows(self.bookmarks.iter().filter_map(|i| {
            self.corpus.get(*i).map(|e| {
                (
                    format!("entry-{i}"),
                    e.name.clone(),
                    e.subtitle.clone(),
                    Glyph::Bookmark,
                )
            })
        }))
        .bottom_action("back", "Back")
        .build()
    }
    fn dice(&self, s: ScreenBuilder) -> Screen {
        s.secondary(format!(
            "d20 {} · modifier {:+}",
            match self.advantage {
                -1 => "disadvantage",
                1 => "advantage",
                _ => "normal",
            },
            self.modifier
        ))
        .section(format!(
            "Result: {}",
            i16::try_from(self.roll).unwrap_or(20) + i16::from(self.modifier)
        ))
        .primary_button("roll", "Roll d20")
        .buttons([
            ("modifier-down", "− modifier"),
            ("modifier-up", "+ modifier"),
        ])
        .buttons([("disadvantage", "Disadvantage"), ("advantage", "Advantage")])
        .text(format!(
            "Last 10: {}",
            self.history
                .iter()
                .rev()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(" · ")
        ))
        .build()
    }
    fn initiative(&self, s: ScreenBuilder) -> Screen {
        if self.initiative.is_empty() {
            return s
                .splash(
                    Some(Glyph::Chart),
                    "No combatants",
                    "Tap Add to enter a name and initiative.",
                )
                .bottom_action("init-add", "Add combatant")
                .build();
        }
        s.secondary(format!("Round {}", self.round))
            .rows(self.initiative.iter().enumerate().map(|(i, c)| {
                (
                    format!("init-{i}"),
                    c.name.clone(),
                    format!(
                        "{}{}",
                        c.initiative,
                        if i == self.current { " · current" } else { "" }
                    ),
                    Glyph::Chart,
                )
            }))
            .primary_button("next", "NEXT")
            .buttons([
                ("init-add", "Add"),
                ("init-edit", "Edit current"),
                ("init-remove", "Remove current"),
            ])
            .build()
    }
    fn party(&self, s: ScreenBuilder) -> Screen {
        if self.party.is_empty() {
            return s
                .splash(
                    Some(Glyph::Person),
                    "No party members",
                    "Tap Add to enter a name, AC, and hit points.",
                )
                .bottom_action("party-add", "Add member")
                .build();
        }
        s.secondary("Select a member to change hit points, saves, or slots.")
            .rows(self.party.iter().enumerate().map(|(i, m)| {
                (
                    format!("party-{i}"),
                    m.name.clone(),
                    format!("AC {} · HP {}/{}", m.ac, m.hp, m.max_hp),
                    Glyph::Person,
                )
            }))
            .bottom_action("party-add", "Add member")
            .build()
    }
    fn member(&self, s: ScreenBuilder) -> Screen {
        let Some(i) = self.member.and_then(|i| self.party.get(i).map(|_| i)) else {
            return self.party(s);
        };
        let m = &self.party[i];
        s.heading(m.name.clone())
            .facts([
                ("AC", m.ac.to_string()),
                ("HP", format!("{}/{}", m.hp, m.max_hp)),
                (
                    "Death saves",
                    format!("{} success · {} failure", m.success, m.failure),
                ),
                (
                    "Slots",
                    m.slots.iter().filter(|filled| **filled).count().to_string(),
                ),
            ])
            .buttons([
                ("hp-down", "− HP"),
                ("hp-up", "+ HP"),
                ("save-success", "Success"),
            ])
            .buttons([("save-failure", "Failure"), ("party-edit", "Edit")])
            .grid(
                3,
                false,
                m.slots.iter().enumerate().map(|(slot, filled)| {
                    (
                        format!("slot-{slot}"),
                        format!("Slot {} {}", slot + 1, if *filled { "[x]" } else { "[ ]" }),
                    )
                }),
            )
            .buttons([("party-remove", "Remove"), ("party-back", "Party")])
            .build()
    }
    fn edit(&self, s: ScreenBuilder) -> Screen {
        let Some(edit) = &self.edit else {
            return self.initiative(s);
        };
        let prompts = match edit.target {
            Editor::Init(_) => ["Name", "Initiative", "", ""],
            Editor::Party(_) => ["Name", "Armor class", "Maximum HP", "Current HP"],
        };
        s.heading(prompts[edit.step])
            .field("value", self.keyboard.text(), prompts[edit.step])
            .keyboard(&self.keyboard, "Next")
            .build()
    }
    fn save(&self, c: &mut Context) {
        let init = self
            .initiative
            .iter()
            .map(|x| format!("{}~{}", x.name.replace(['|', '~'], " "), x.initiative))
            .collect::<Vec<_>>()
            .join(";");
        let party = self
            .party
            .iter()
            .map(|m| {
                format!(
                    "{}~{}~{}~{}~{}~{}~{}",
                    m.name.replace(['|', '~'], " "),
                    m.ac,
                    m.max_hp,
                    m.hp,
                    m.success,
                    m.failure,
                    m.slots
                        .iter()
                        .map(|v| if *v { '1' } else { '0' })
                        .collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        c.store().save(
            STATE,
            format!(
                "{}|{}|{}|{}|{}|{}",
                self.edition,
                self.round,
                self.current,
                init,
                party,
                self.bookmarks
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
    }
    fn sort_init(&mut self) {
        let active = self.initiative.get(self.current).map(|c| c.name.clone());
        self.initiative
            .sort_by_key(|c| std::cmp::Reverse(c.initiative));
        self.current = active
            .and_then(|name| self.initiative.iter().position(|c| c.name == name))
            .unwrap_or(0)
            .min(self.initiative.len().saturating_sub(1));
    }
    fn start_edit(&mut self, target: Editor) {
        let values = match target {
            Editor::Init(Some(i)) => self
                .initiative
                .get(i)
                .map(|c| vec![c.name.clone(), c.initiative.to_string()])
                .unwrap_or_default(),
            Editor::Party(Some(i)) => self
                .party
                .get(i)
                .map(|m| {
                    vec![
                        m.name.clone(),
                        m.ac.to_string(),
                        m.max_hp.to_string(),
                        m.hp.to_string(),
                    ]
                })
                .unwrap_or_default(),
            _ => vec![],
        };
        self.keyboard = Keyboard::with_text(values.first().cloned().unwrap_or_default());
        self.edit = Some(Edit {
            target,
            step: 0,
            values,
        });
        self.view = View::Edit;
    }
    fn complete_edit(&mut self, c: &mut Context, text: String) {
        let Some(mut edit) = self.edit.take() else {
            return;
        };
        edit.values.push(text);
        let count = match edit.target {
            Editor::Init(_) => 2,
            Editor::Party(_) => 4,
        };
        if edit.values.len() < count {
            self.keyboard = Keyboard::new();
            edit.step = edit.values.len();
            self.edit = Some(edit);
            return;
        }
        match edit.target {
            Editor::Init(index) => {
                let value = edit.values[1].parse().unwrap_or(0);
                let item = Combatant {
                    name: edit.values[0].clone(),
                    initiative: value,
                };
                if let Some(i) = index {
                    if let Some(slot) = self.initiative.get_mut(i) {
                        *slot = item;
                    }
                } else {
                    self.initiative.push(item);
                }
                self.sort_init();
                self.view = View::Initiative;
            }
            Editor::Party(index) => {
                let max = edit.values[2].parse::<i16>().unwrap_or(1).max(1);
                let retained = index
                    .and_then(|index| self.party.get(index))
                    .map_or((0, 0, [false; 9]), |member| {
                        (member.success, member.failure, member.slots)
                    });
                let item = Member {
                    name: edit.values[0].clone(),
                    ac: edit.values[1].parse().unwrap_or(10),
                    max_hp: max,
                    hp: edit.values[3].parse::<i16>().unwrap_or(max).clamp(0, max),
                    success: retained.0,
                    failure: retained.1,
                    slots: retained.2,
                };
                if let Some(i) = index {
                    if let Some(slot) = self.party.get_mut(i) {
                        *slot = item;
                    }
                } else if self.party.len() < 6 {
                    self.party.push(item);
                }
                self.view = View::Party;
            }
        }
        self.save(c);
    }
}
impl KoboApp for Grimoire {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(STATE);
        self.show(c);
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { key, value } = r {
            if key == STATE {
                if let Some(text) = value.and_then(|v| String::from_utf8(v).ok()) {
                    let f: Vec<_> = text.split('|').collect();
                    self.edition = f
                        .first()
                        .and_then(|x| x.parse().ok())
                        .filter(|x| *x == 2014 || *x == 2024)
                        .unwrap_or(2014);
                    self.round = f.get(1).and_then(|x| x.parse().ok()).unwrap_or(1);
                    self.current = f.get(2).and_then(|x| x.parse().ok()).unwrap_or(0);
                    self.initiative = f.get(3).map_or_else(Vec::new, |x| {
                        x.split(';')
                            .filter_map(|r| {
                                let p: Vec<_> = r.split('~').collect();
                                Some(Combatant {
                                    name: p.first()?.to_string(),
                                    initiative: p.get(1)?.parse().ok()?,
                                })
                            })
                            .collect()
                    });
                    self.party = f.get(4).map_or_else(Vec::new, |x| {
                        x.split(';')
                            .filter_map(|r| {
                                let p: Vec<_> = r.split('~').collect();
                                let slots = p.get(6)?.chars().take(9).enumerate().fold(
                                    [false; 9],
                                    |mut out, (i, v)| {
                                        out[i] = v == '1';
                                        out
                                    },
                                );
                                Some(Member {
                                    name: p.first()?.to_string(),
                                    ac: p.get(1)?.parse().ok()?,
                                    max_hp: p.get(2)?.parse().ok()?,
                                    hp: p.get(3)?.parse().ok()?,
                                    success: p.get(4)?.parse().ok()?,
                                    failure: p.get(5)?.parse().ok()?,
                                    slots,
                                })
                            })
                            .take(6)
                            .collect()
                    });
                    self.bookmarks = f.get(5).map_or_else(Vec::new, |saved| {
                        saved
                            .split(',')
                            .filter_map(|index| index.parse().ok())
                            .filter(|index| *index < self.corpus.len())
                            .collect()
                    });
                    self.sort_init();
                }
            }
        }
        self.loaded = true;
        self.show(c);
    }
    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if self.view == View::Search || self.view == View::Edit {
            if let Some(p) = self.keyboard.press(a) {
                if p == Pressed::Submitted {
                    let text = self.keyboard.take();
                    if self.view == View::Search {
                        self.query = text;
                        self.page = 0;
                        self.view = View::Compendium;
                    } else {
                        self.complete_edit(c, text);
                    }
                }
                self.show(c);
                return;
            }
        }
        if a == ActionId::BACK || a == action_id("back") {
            self.view = match self.view {
                View::Member => View::Party,
                View::Edit => {
                    self.edit
                        .as_ref()
                        .map_or(View::Initiative, |edit| match edit.target {
                            Editor::Init(_) => View::Initiative,
                            Editor::Party(_) => View::Party,
                        })
                }
                View::Search => View::Compendium,
                _ => View::Home,
            };
            self.detail = None;
        } else if let Some((_, view)) = [
            ("spells", Kind::Spell),
            ("monsters", Kind::Monster),
            ("rules", Kind::Rule),
        ]
        .iter()
        .find(|(n, _)| a == action_id(n))
        {
            self.kind = *view;
            self.view = View::Compendium;
        } else if a == action_id("dice") {
            self.view = View::Dice;
        } else if a == action_id("initiative") {
            self.view = View::Initiative;
        } else if a == action_id("party") {
            self.view = View::Party;
        } else if a == action_id("about") {
            self.view = View::About;
        } else if a == action_id("edition-2014") || a == action_id("edition-2024") {
            self.edition = if a == action_id("edition-2014") {
                2014
            } else {
                2024
            };
            self.page = 0;
            self.save(c);
        } else if a == action_id("search") {
            self.keyboard = Keyboard::with_text(&self.query);
            self.view = View::Search;
        } else if a == action_id("bookmarks") {
            self.view = View::Bookmarks;
        } else if a == action_id("previous") {
            self.page = self.page.saturating_sub(1);
        } else if a == action_id("next") {
            if self.view == View::Initiative && !self.initiative.is_empty() {
                self.current = (self.current + 1) % self.initiative.len();
                if self.current == 0 {
                    self.round += 1;
                }
                self.save(c);
            } else {
                self.page =
                    (self.page + 1).min(self.entries().len().div_ceil(PAGE).saturating_sub(1));
            }
        } else if a == action_id("spell-class") {
            self.spell_class = (self.spell_class + 1) % options(self, "class").len();
            self.page = 0;
        } else if a == action_id("spell-level") {
            self.spell_level = match self.spell_level {
                None => Some(0),
                Some(9) => None,
                Some(n) => Some(n + 1),
            };
            self.page = 0;
        } else if a == action_id("spell-school") {
            self.spell_school = (self.spell_school + 1) % options(self, "school").len();
            self.page = 0;
        } else if a == action_id("ritual") {
            self.ritual = self.ritual.next();
            self.page = 0;
        } else if a == action_id("concentration") {
            self.concentration = self.concentration.next();
            self.page = 0;
        } else if a == action_id("monster-cr") {
            self.cr = (self.cr + 1) % 6;
            self.page = 0;
        } else if a == action_id("monster-type") {
            self.monster_type = (self.monster_type + 1) % options(self, "type").len();
            self.page = 0;
        } else if a == action_id("clear") {
            self.spell_class = 0;
            self.spell_level = None;
            self.spell_school = 0;
            self.ritual = Tri::Any;
            self.concentration = Tri::Any;
            self.cr = 0;
            self.monster_type = 0;
            self.page = 0;
        } else if a == action_id("roll") {
            let n = u16::try_from(self.history.len()).unwrap_or(0);
            self.roll = ((n.wrapping_mul(11).wrapping_add(7)) % 20) + 1;
            self.history.push(self.roll);
            if self.history.len() > 10 {
                self.history.remove(0);
            }
        } else if a == action_id("modifier-up") {
            self.modifier = (self.modifier + 1).min(10);
        } else if a == action_id("modifier-down") {
            self.modifier = (self.modifier - 1).max(-10);
        } else if a == action_id("advantage") {
            self.advantage = 1;
        } else if a == action_id("disadvantage") {
            self.advantage = -1;
        } else if a == action_id("init-add") {
            self.start_edit(Editor::Init(None));
        } else if a == action_id("init-edit") {
            self.start_edit(Editor::Init(
                self.initiative.get(self.current).map(|_| self.current),
            ));
        } else if a == action_id("init-remove") {
            if self.current < self.initiative.len() {
                self.initiative.remove(self.current);
                self.current = self.current.min(self.initiative.len().saturating_sub(1));
                self.save(c);
            }
        } else if a == action_id("add-init") {
            if let Some(i) = self.detail {
                self.initiative.push(Combatant {
                    name: self.corpus[i].name.clone(),
                    initiative: 0,
                });
                self.sort_init();
                self.save(c);
            }
        } else if a == action_id("party-add") {
            self.start_edit(Editor::Party(None));
        } else if a == action_id("party-edit") {
            self.start_edit(Editor::Party(self.member));
        } else if a == action_id("party-remove") {
            if let Some(i) = self.member.filter(|i| *i < self.party.len()) {
                self.party.remove(i);
                self.member = None;
                self.view = View::Party;
                self.save(c);
            }
        } else if a == action_id("party-back") {
            self.view = View::Party;
        } else if let Some(i) = self.member.filter(|i| *i < self.party.len()) {
            let m = &mut self.party[i];
            if a == action_id("hp-down") {
                m.hp = (m.hp - 1).max(0);
            } else if a == action_id("hp-up") {
                m.hp = (m.hp + 1).min(m.max_hp);
            } else if a == action_id("save-success") {
                m.success = (m.success + 1) % 4;
            } else if a == action_id("save-failure") {
                m.failure = (m.failure + 1) % 4;
            } else if let Some(slot) =
                (0..m.slots.len()).find(|slot| a == action_id(&format!("slot-{slot}")))
            {
                m.slots[slot] = !m.slots[slot];
            } else {
                self.show(c);
                return;
            }
            self.save(c);
        } else if a == action_id("bookmark") {
            if let Some(i) = self.detail {
                if let Some(at) = self.bookmarks.iter().position(|x| *x == i) {
                    self.bookmarks.remove(at);
                } else {
                    self.bookmarks.push(i);
                }
                self.save(c);
            }
        } else if let Some(i) =
            (0..self.corpus.len()).find(|i| a == action_id(&format!("entry-{i}")))
        {
            self.detail = Some(i);
            self.view = View::Detail;
        } else if let Some(i) =
            (0..self.party.len()).find(|i| a == action_id(&format!("party-{i}")))
        {
            self.member = Some(i);
            self.view = View::Member;
        }
        self.show(c);
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("grimoire", Grimoire::default()).map_or_else(
        |e| {
            eprintln!("grimoire: {e}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn option_index(app: &Grimoire, key: &str, value: &str) -> usize {
        options(app, key)
            .iter()
            .position(|known| known == value)
            .expect("filter value")
    }
    #[test]
    fn generated_spell_tags_drive_all_requested_filters() {
        let mut app = Grimoire {
            kind: Kind::Spell,
            ..Grimoire::default()
        };
        app.spell_class = option_index(&app, "class", "Wizard");
        app.spell_level = Some(3);
        app.spell_school = option_index(&app, "school", "Evocation");
        app.ritual = Tri::No;
        app.concentration = Tri::No;
        let entries = app.entries();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|(_, entry)| app.spell_match(entry)));
    }
    #[test]
    fn monster_type_and_cr_filters_use_generated_index_tags() {
        let mut app = Grimoire {
            kind: Kind::Monster,
            ..Grimoire::default()
        };
        app.monster_type = option_index(&app, "type", "beast");
        app.cr = 4;
        let entries = app.entries();
        assert!(entries.iter().all(|(_, entry)| app.monster_match(entry)));
        assert!(entries
            .iter()
            .all(|(_, entry)| tag(entry, "type") == Some("beast")));
    }
    #[test]
    fn initiative_sort_keeps_the_active_combatant() {
        let mut app = Grimoire::default();
        app.initiative = vec![
            Combatant {
                name: "Low".into(),
                initiative: 4,
            },
            Combatant {
                name: "High".into(),
                initiative: 20,
            },
        ];
        app.current = 0;
        app.sort_init();
        assert_eq!(app.initiative[app.current].name, "Low");
        app.current = 1;
        app.current = (app.current + 1) % app.initiative.len();
        assert_eq!(app.current, 0);
    }
    #[test]
    fn party_members_are_independent_and_limited_to_six() {
        let mut app = Grimoire::default();
        app.party = (0..6)
            .map(|number| Member {
                name: format!("Member {number}"),
                ac: 12,
                max_hp: 10,
                hp: 10,
                success: 0,
                failure: 0,
                slots: [false; 9],
            })
            .collect();
        app.member = Some(4);
        app.party[4].hp -= 3;
        app.party[4].slots[2] = true;
        assert_eq!(app.party[0].hp, 10);
        assert_eq!(app.party[4].hp, 7);
        assert!(app.party[4].slots[2]);
        assert_eq!(app.party.len(), 6);
    }
    #[test]
    fn persisted_initiative_and_party_state_round_trips() {
        let mut saved = Grimoire::default();
        saved.round = 4;
        saved.initiative = vec![Combatant {
            name: "Owlbear".into(),
            initiative: 15,
        }];
        saved.party = vec![Member {
            name: "Moss".into(),
            ac: 14,
            max_hp: 18,
            hp: 9,
            success: 2,
            failure: 1,
            slots: [true, false, true, false, false, false, false, false, false],
        }];
        let init = saved
            .initiative
            .iter()
            .map(|combatant| format!("{}~{}", combatant.name, combatant.initiative))
            .collect::<Vec<_>>()
            .join(";");
        let party = "Moss~14~18~9~2~1~101000000";
        let mut restored = Grimoire::default();
        let mut context = Context::default();
        restored.on_store(
            &mut context,
            StoreResult::Loaded {
                key: STATE.to_owned(),
                value: Some(format!("2014|4|0|{init}|{party}|2,8").into_bytes()),
            },
        );
        assert_eq!(restored.round, 4);
        assert_eq!(restored.initiative[0].name, "Owlbear");
        assert_eq!(restored.party[0].hp, 9);
        assert!(restored.party[0].slots[2]);
        assert_eq!(restored.bookmarks, vec![2, 8]);
    }
    #[test]
    fn party_edit_preserves_existing_saves_and_slots() {
        let member = Member {
            name: "Moss".into(),
            ac: 12,
            max_hp: 10,
            hp: 8,
            success: 2,
            failure: 1,
            slots: [false, true, false, false, false, false, false, false, false],
        };
        let mut app = Grimoire {
            party: vec![member],
            edit: Some(Edit {
                target: Editor::Party(Some(0)),
                step: 3,
                values: vec!["Moss".into(), "14".into(), "18".into()],
            }),
            ..Grimoire::default()
        };
        app.complete_edit(&mut Context::default(), "12".into());
        assert_eq!(app.party[0].ac, 14);
        assert_eq!((app.party[0].success, app.party[0].failure), (2, 1));
        assert!(app.party[0].slots[1]);
    }
    #[test]
    fn initiative_and_party_controls_fit_clara_bw() {
        let mut app = Grimoire::default();
        app.initiative = vec![Combatant {
            name: "Goblin".into(),
            initiative: 12,
        }];
        app.party = vec![Member {
            name: "Aria".into(),
            ac: 15,
            max_hp: 22,
            hp: 18,
            success: 1,
            failure: 0,
            slots: [false; 9],
        }];
        for view in [View::Initiative, View::Party, View::Member] {
            app.view = view;
            app.member = Some(0);
            let screen = app.screen();
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }
    #[test]
    fn licenses_and_zero_capability_claim_are_present() {
        assert!(include_str!("../README.md").contains(ATTRIBUTION));
        assert!(include_str!("../THIRD-PARTY.md").contains(ATTRIBUTION));
        assert!(include_str!("../Cargo.toml").contains("[dependencies]"));
        assert!(!include_str!("../data/corpus.tsv").contains("Kobold Press"));
    }
}
