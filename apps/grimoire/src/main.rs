//! Offline SRD reference and table companion.
mod corpus;
mod filters;
use corpus::Entry;
use filters::Filter;
use kobo_bookview::BookView;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult, TaskId,
    TaskOutcome,
};
use std::fmt::Write;
use std::process::ExitCode;
const ATTRIBUTION: &str = "This work includes material taken from the System Reference Document 5.1 and System Reference Document 5.2 by Wizards of the Coast LLC, available under the Creative Commons Attribution 4.0 International License.";
const STATE: &str = "grimoire-state-v2";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Compendium,
    Filters,
    FilterChoice,
    Bookmarks,
    Search,
    Dice,
    Initiative,
    Combatant,
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
    Item,
}
impl Kind {
    const ALL: [Self; 4] = [Self::Spell, Self::Monster, Self::Rule, Self::Item];

    const fn key(self) -> &'static str {
        match self {
            Self::Spell => "spell",
            Self::Monster => "monster",
            Self::Rule => "rule",
            Self::Item => "item",
        }
    }

    /// The corpus kinds this category draws from. Conditions are looked up
    /// alongside the rules because that is where somebody at a table goes
    /// looking for "what does frightened do".
    const fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Spell => &["spell"],
            Self::Monster => &["monster"],
            Self::Rule => &["rule", "condition"],
            Self::Item => &["item"],
        }
    }

    fn holds(self, kind: &str) -> bool {
        self.keys().contains(&kind)
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Spell => "Spells",
            Self::Monster => "Monsters",
            Self::Rule => "Rules & conditions",
            Self::Item => "Magic items",
        }
    }

    /// How the category is named inside a sentence.
    const fn plural(self) -> &'static str {
        match self {
            Self::Spell => "spells",
            Self::Monster => "monsters",
            Self::Rule => "rules or conditions",
            Self::Item => "magic items",
        }
    }

    const fn action(self) -> &'static str {
        match self {
            Self::Spell => "spells",
            Self::Monster => "monsters",
            Self::Rule => "rules",
            Self::Item => "items",
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
    fn matches(self, value: bool) -> bool {
        self == Self::Any || (self == Self::Yes) == value
    }
}
#[derive(Clone, Debug)]
struct Combatant {
    name: String,
    initiative: i8,
    /// Current and maximum hit points, for a combatant whose health the table
    /// keeps. A monster added from its reference brings its own; a name typed
    /// in has none unless one is given, because a player's health lives on
    /// their party page.
    hp: Option<(i16, i16)>,
}
/// One spell level's slots: how many the character has, and how many are
/// spent. A level used to be a single tick box, so a wizard with four
/// first-level slots could record one of them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Slots {
    used: u8,
    max: u8,
}
/// The most slots any class has at one level.
const MOST_SLOTS: u8 = 4;
#[derive(Clone, Debug)]
struct Member {
    name: String,
    ac: u8,
    max_hp: i16,
    hp: i16,
    success: u8,
    failure: u8,
    slots: [Slots; 9],
}
/// What a tap on a spell level does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SlotMode {
    #[default]
    Spend,
    Regain,
    SetUp,
}
/// Hit point steps, one tap each. Damage comes in fives and tens far more
/// often than in ones, and a fireball used to be twenty-eight taps.
const HP_STEPS: [(&str, &str, i16); 6] = [
    ("hp-down-10", "\u{2212}10 HP", -10),
    ("hp-down-5", "\u{2212}5 HP", -5),
    ("hp-down", "\u{2212}1 HP", -1),
    ("hp-up", "+1 HP", 1),
    ("hp-up-5", "+5 HP", 5),
    ("hp-up-10", "+10 HP", 10),
];
fn hp_step(a: ActionId) -> Option<i16> {
    HP_STEPS
        .iter()
        .find(|(name, _, _)| a == action_id(name))
        .map(|(_, _, step)| *step)
}
fn hp_controls(s: ScreenBuilder) -> ScreenBuilder {
    s.grid(
        3,
        false,
        HP_STEPS.iter().map(|(name, label, _)| (*name, *label)),
    )
}
/// The hit points in a monster's stat block, which every SRD monster opens
/// with as "HP 135 (18d10)".
fn monster_hp(body: &str) -> Option<i16> {
    let after = body.split("HP ").nth(1)?;
    after
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
        .filter(|hp| *hp > 0)
}
fn ordinal(level: usize) -> String {
    let suffix = match level {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{level}{suffix}")
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
    /// What each step held before, offered again as the starting text so an
    /// edit changes only what is retyped.
    existing: Vec<String>,
}
struct Grimoire {
    view: View,
    kind: Kind,
    edition: u16,
    query: String,
    keyboard: Keyboard,
    page: usize,
    detail: Option<usize>,
    /// Which page of the open reference is drawn. Also pages About, which is
    /// the other screen here made of more prose than a panel holds.
    detail_page: usize,
    bookmarks: Vec<usize>,
    bookmarks_page: usize,
    roll: u16,
    modifier: i8,
    advantage: i8,
    history: Vec<u16>,
    round: u16,
    current: usize,
    initiative: Vec<Combatant>,
    /// Which combatant's own screen is open, if any.
    init_menu: Option<usize>,
    party: Vec<Member>,
    member: Option<usize>,
    edit: Option<Edit>,
    filter: Filter,
    filter_page: usize,
    spell_class: usize,
    spell_level: Option<u8>,
    spell_school: usize,
    ritual: Tri,
    concentration: Tri,
    cr: usize,
    monster_type: usize,
    corpus: Vec<Entry>,
    /// The open reference, read through the shared document reader.
    book: BookView,
    /// Which entry's actions are showing, if any.
    entry_menu: Option<usize>,
    /// Which page of a party member is showing: their health, or their slots.
    member_page: usize,
    slot_mode: SlotMode,
    /// Whether the open reference was reached from the bookmark list, which is
    /// where closing it should land.
    bookmarks_open: bool,
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
            detail_page: 0,
            bookmarks: vec![],
            bookmarks_page: 0,
            roll: 20,
            modifier: 0,
            advantage: 0,
            history: vec![],
            round: 1,
            current: 0,
            initiative: vec![],
            init_menu: None,
            party: vec![],
            member: None,
            edit: None,
            filter: Filter::Class,
            filter_page: 0,
            spell_class: 0,
            spell_level: None,
            spell_school: 0,
            ritual: Tri::Any,
            concentration: Tri::Any,
            cr: 0,
            monster_type: 0,
            corpus: corpus::load(),
            book: BookView::new(),
            entry_menu: None,
            member_page: 0,
            slot_mode: SlotMode::Spend,
            bookmarks_open: false,
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
    let mut values = std::collections::BTreeSet::new();
    for entry in &app.corpus {
        if entry.edition == app.edition && entry.kind == app.kind.key() {
            if let Some(value) = tag(entry, key) {
                for item in value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                {
                    values.insert(item.to_owned());
                }
            }
        }
    }
    std::iter::once("Any".to_owned()).chain(values).collect()
}

impl Grimoire {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context).with_own_back(self.view != View::Home));
    }
    #[cfg(test)]
    fn spell_match(&self, e: &Entry) -> bool {
        let classes = options(self, "class");
        let schools = options(self, "school");
        self.spell_match_values(
            e,
            classes.get(self.spell_class).map(String::as_str),
            schools.get(self.spell_school).map(String::as_str),
        )
    }
    fn spell_match_values(&self, e: &Entry, class: Option<&str>, school: Option<&str>) -> bool {
        (self.spell_class == 0
            || tag(e, "class")
                .is_some_and(|v| v.split(',').map(str::trim).any(|c| Some(c) == class)))
            && self
                .spell_level
                .is_none_or(|level| tag(e, "level") == Some(&level.to_string()))
            && (self.spell_school == 0 || school.is_some() && tag(e, "school") == school)
            && self.ritual.matches(tag(e, "ritual") == Some("1"))
            && self
                .concentration
                .matches(tag(e, "concentration") == Some("1"))
    }
    #[cfg(test)]
    fn monster_match(&self, e: &Entry) -> bool {
        let kinds = options(self, "type");
        self.monster_match_value(e, kinds.get(self.monster_type).map(String::as_str))
    }
    fn monster_match_value(&self, e: &Entry, kind: Option<&str>) -> bool {
        let type_ok = self.monster_type == 0 || kind.is_some() && tag(e, "type") == kind;
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
        let classes = options(self, "class");
        let schools = options(self, "school");
        let types = options(self, "type");
        let class = classes.get(self.spell_class).map(String::as_str);
        let school = schools.get(self.spell_school).map(String::as_str);
        let kind = types.get(self.monster_type).map(String::as_str);
        self.corpus
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.edition == self.edition
                    && self.kind.holds(&e.kind)
                    && (query.is_empty() || e.name.to_lowercase().starts_with(&query))
                    && (self.kind != Kind::Spell || self.spell_match_values(e, class, school))
                    && (self.kind != Kind::Monster || self.monster_match_value(e, kind))
            })
            .collect()
    }
    fn title(&self) -> &'static str {
        match self.view {
            View::Home => "Grimoire",
            View::Compendium => self.kind.title(),
            View::Filters => "Filters",
            View::FilterChoice => self.filter.title(),
            View::Bookmarks => "Bookmarks",
            View::Search => "Search",
            View::Dice => "Dice",
            View::Initiative => "Initiative",
            View::Combatant => "Combatant",
            View::Party => "Party",
            View::Member => "Party member",
            View::Edit => "Edit",
            View::Detail => "Reference",
            View::About => "About",
        }
    }
    fn screen(&self, context: &Context) -> Screen {
        let s = ScreenBuilder::new("grimoire").top_bar(self.title());
        match self.view {
            View::Home => s
                .top_bar_action("about", "About")
                .tiles([
                    ("spells", "Spells", Glyph::Book),
                    ("monsters", "Monsters", Glyph::Search),
                    ("rules", "Rules & conditions", Glyph::Book),
                    ("items", "Magic items", Glyph::Tag),
                    ("dice", "Dice", Glyph::Circle),
                    ("initiative", "Initiative", Glyph::Chart),
                    ("party", "Party", Glyph::Person),
                ])
                .build(),
            View::Compendium => self.compendium(s, context),
            View::Filters | View::FilterChoice => self.filter_screen(context),
            View::Bookmarks => self.bookmarks(s, context),
            View::Search => s
                .secondary(if self.keyboard.text().is_empty() {
                    "Type the start of a name.".to_owned()
                } else {
                    format!(
                        "Names starting with \u{201c}{}\u{201d}",
                        self.keyboard.text()
                    )
                })
                .keyboard(&self.keyboard, "Search")
                .build(),
            View::Dice => self.dice(s),
            View::Initiative => self.initiative(s, context),
            View::Combatant => self.combatant(s, context),
            View::Party => self.party(s, context),
            View::Member => self.member(s, context),
            View::Edit => self.edit(s, context),
            View::Detail => self.detail(s),
            View::About => self.about(s, context),
        }
    }

    /// What this build of the reference actually holds, counted rather than
    /// claimed. A reference that overstates its own contents sends somebody
    /// looking through filters for a spell that was never in the file.
    fn about(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let mut text = String::from(
            "Grimoire is an unofficial offline reference. It requests no capabilities.\n\n",
        );
        for edition in [2014, 2024] {
            let counted: Vec<String> = Kind::ALL
                .into_iter()
                .filter_map(|kind| {
                    let held = self.held(kind, edition);
                    (held > 0).then(|| format!("{held} {}", kind.plural()))
                })
                .collect();
            let _ = writeln!(text, "{edition}: {}", counted.join(" · "));
        }
        text.push_str(
            "\nThe System Reference Documents do not cover classes, subclasses, \
             backgrounds or feats, so this reference has none of them. No \
             third-party OGL-only material or artwork is included.\n\n",
        );
        text.push_str(ATTRIBUTION);
        let pages = context.paginate(&text, true);
        let page = self.detail_page.min(pages.len().saturating_sub(1));
        let mut s = s.text(
            pages
                .get(page)
                .map(|lines| lines.join("\n"))
                .unwrap_or_default(),
        );
        if pages.len() > 1 {
            s = s.page_turns("previous", "next").page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        s.bottom_action("back", "Back").build()
    }

    /// How many entries of one category this build holds for an edition.
    fn held(&self, kind: Kind, edition: u16) -> usize {
        self.corpus
            .iter()
            .filter(|entry| entry.edition == edition && kind.holds(&entry.kind))
            .count()
    }

    fn compendium_controls(&self, s: ScreenBuilder) -> ScreenBuilder {
        let s = s.tabs(
            usize::from(self.edition != 2014),
            [("edition-2014", "2014"), ("edition-2024", "2024")],
        );
        // A category with nothing to filter on does not offer the word: a
        // Filters screen with no filters on it is a dead end with a label.
        if self.filters().is_empty() {
            s.action_bar([("search", "Search"), ("bookmarks", "Bookmarks")])
        } else {
            s.action_bar([
                ("filters", "Filters"),
                ("search", "Search"),
                ("bookmarks", "Bookmarks"),
            ])
        }
    }
    fn compendium_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let entries = self.entries();
        let rows = entries
            .iter()
            .map(|(_, entry)| (entry.name.as_str(), entry.subtitle.as_str()))
            .collect::<Vec<_>>();
        let prefix = self
            .compendium_controls(ScreenBuilder::new("grimoire").top_bar(self.title()))
            .build();
        context.paginate_rows_with_menu_under(&rows, true, kobo_sdk::Position::AtTheFoot, &prefix)
    }
    fn compendium(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let s = self.compendium_controls(s);
        let entries = self.entries();
        if entries.is_empty() {
            let (heading, detail) = self.nothing_here();
            return s.splash(Some(Glyph::Search), heading, detail).build();
        }
        let pages = self.compendium_pages(context);
        let page = self.page.min(pages.len().saturating_sub(1));
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut s = s.rows_with_menu(visible.iter().map(|&index| {
            let (i, e) = entries[index];
            (
                format!("entry-{i}"),
                e.name.clone(),
                e.subtitle.clone(),
                if self.bookmarks.contains(&i) {
                    Glyph::Bookmark
                } else {
                    Glyph::Book
                },
                format!("entry-menu-{i}"),
            )
        }));
        // Bookmarking and sending a monster to the initiative order are things
        // done *to* a reference rather than with it, and the reading screen
        // belongs to the reader, so they live on the row.
        if let Some(open) = self
            .entry_menu
            .filter(|open| visible.iter().any(|index| entries[*index].0 == *open))
        {
            let mut items = vec![if self.bookmarks.contains(&open) {
                ("bookmark", "Remove bookmark", Glyph::Bookmark)
            } else {
                ("bookmark", "Bookmark", Glyph::Bookmark)
            }];
            if self.kind == Kind::Monster {
                items.push(("add-init", "Add to initiative", Glyph::Chart));
            }
            s = s.row_overflow(format!("entry-menu-{open}"), true, items);
        }
        s.page_turns("previous", "next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            )
            .build()
    }
    /// Why this category is empty.
    ///
    /// A category with nothing in it for this edition is a fact about what the
    /// reference holds, not something a reader can fix by changing a filter,
    /// and telling them to change one they never set is how somebody spends
    /// five minutes looking for the 2024 spells that are not in the file.
    fn nothing_here(&self) -> (String, String) {
        if self.held(self.kind, self.edition) > 0 {
            return (
                "No matching reference".to_owned(),
                "Change a filter, the search prefix, or the edition.".to_owned(),
            );
        }
        let other = if self.edition == 2014 { 2024 } else { 2014 };
        let heading = format!(
            "No {} in the {} reference",
            self.kind.plural(),
            self.edition
        );
        let detail = if self.held(self.kind, other) > 0 {
            format!(
                "The {} System Reference Document in this build does not include {}. The {other} \
                 reference does; switch edition above.",
                self.edition,
                self.kind.plural()
            )
        } else {
            format!(
                "Neither System Reference Document in this build includes {}.",
                self.kind.plural()
            )
        };
        (heading, detail)
    }

    /// One reference, read through the shared document reader.
    ///
    /// A rule section runs to several thousand words with its own headings and
    /// tables, and a stat block is a list of named abilities. Drawn as one flow
    /// of interface text they stopped at the bottom of the panel and the rest
    /// was simply not there. The reader pages them, sets their headings as
    /// headings and lines their tables up in columns, which is what it exists
    /// for; it also brings the type size control a table reaches for when the
    /// light is bad.
    fn detail(&self, s: ScreenBuilder) -> Screen {
        let Some(entry) = self.detail.and_then(|index| self.corpus.get(index)) else {
            return s
                .text("No reference selected.")
                .bottom_action("back", "Back")
                .build();
        };
        self.book.screen(&entry.name).unwrap_or_else(|| {
            ScreenBuilder::new("grimoire")
                .top_bar(entry.name.clone())
                .empty_state("This reference has no text in the bundled document.")
                .bottom_action("back", "Back")
                .build()
        })
    }

    /// Closes the open reference and hands the panel back to the list.
    fn close_reference(&mut self, c: &mut Context) {
        self.book.close(c);
        self.detail = None;
        self.view = if self.bookmarks_open {
            View::Bookmarks
        } else {
            View::Compendium
        };
    }

    /// Opens one reference, and its subtitle with it.
    fn open_reference(&mut self, c: &mut Context, index: usize) {
        let Some(entry) = self.corpus.get(index) else {
            return;
        };
        let source = if entry.subtitle.is_empty() {
            entry.body.clone()
        } else {
            format!("<p>{}</p>{}", entry.subtitle, entry.body)
        };
        self.detail = Some(index);
        self.entry_menu = None;
        self.view = View::Detail;
        self.book.close(c);
        self.book.open(
            c,
            kobo_doc::html::parse(&source),
            kobo_read::Memory::default(),
        );
    }

    fn bookmark_rows(&self, context: &Context) -> Vec<(usize, String, String)> {
        self.bookmarks
            .iter()
            .filter_map(|index| {
                self.corpus.get(*index).map(|entry| {
                    (
                        *index,
                        context.clamped_row(&entry.name, 2, true),
                        entry.subtitle.clone(),
                    )
                })
            })
            .collect()
    }

    fn bookmark_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.bookmark_rows(context);
        let labels = rows
            .iter()
            .map(|(_, name, detail)| (name.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        context.paginate_rows(&labels, true)
    }

    fn bookmarks(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let rows = self.bookmark_rows(context);
        if rows.is_empty() {
            return s
                .splash(
                    Some(Glyph::Bookmark),
                    "No bookmarks",
                    "Bookmark a reference to keep it here.",
                )
                .bottom_action("back", "Back")
                .build();
        }
        let pages = self.bookmark_pages(context);
        let page = self.bookmarks_page.min(pages.len().saturating_sub(1));
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = s.rows(visible.iter().map(|&index| {
            let (entry, name, detail) = &rows[index];
            (
                format!("entry-{entry}"),
                name.clone(),
                detail.clone(),
                Glyph::Bookmark,
            )
        }));
        if pages.len() > 1 {
            screen = screen
                .page_turns("bookmarks-previous", "bookmarks-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
        }
        screen.bottom_action("back", "Back").build()
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
    /// The order of battle.
    ///
    /// Two things a table needs that this did not have: a way back when the
    /// turn is passed by accident, which used to cost a round as well as a
    /// turn, and a way to reach a combatant who is not the one acting. Both
    /// are on the row rather than in a bar of verbs that act on "current",
    /// which nobody can point at.
    fn initiative_prefix(&self, s: ScreenBuilder) -> ScreenBuilder {
        s.top_bar_glyph("init-clear", "End combat", Glyph::Trash)
            .secondary(format!(
                "Round {} · turn {} of {}",
                self.round,
                self.current + 1,
                self.initiative.len()
            ))
    }

    fn initiative_rows(&self, context: &Context) -> Vec<(String, String)> {
        self.initiative
            .iter()
            .enumerate()
            .map(|(index, combatant)| {
                (
                    context.one_line_row(&combatant.name, true),
                    format!(
                        "Initiative {}{}{}",
                        combatant.initiative,
                        combatant
                            .hp
                            .map(|(hp, max)| format!(" · HP {hp}/{max}"))
                            .unwrap_or_default(),
                        if index == self.current {
                            " · taking this turn"
                        } else {
                            ""
                        }
                    ),
                )
            })
            .collect()
    }

    fn initiative_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.initiative_rows(context);
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(name, detail)| (name.as_str(), detail.as_str()))
            .collect();
        let prefix = self
            .initiative_prefix(ScreenBuilder::new("grimoire").top_bar(self.title()))
            .build();
        let pages =
            context.paginate_rows_under(&borrowed, true, kobo_sdk::Position::AtTheFoot, &prefix);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn initiative(&self, s: ScreenBuilder, context: &Context) -> Screen {
        if self.initiative.is_empty() {
            return s
                .splash(
                    Some(Glyph::Chart),
                    "No combatants",
                    "Add a name and an initiative, or add a monster from its reference.",
                )
                .bottom_action("init-add", "Add combatant")
                .build();
        }
        let rows = self.initiative_rows(context);
        let pages = self.initiative_pages(context);
        let page = self.page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        let mut s = self.initiative_prefix(s).rows(shown.iter().map(|&index| {
            (
                format!("init-{index}"),
                rows[index].0.clone(),
                rows[index].1.clone(),
                if index == self.current {
                    Glyph::Check
                } else {
                    Glyph::Chart
                },
            )
        }));
        if pages.len() > 1 {
            s = s.page_turns("previous", "next").page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        // Pinned rather than left at the end of the flow: with six combatants
        // and large text, buttons drawn after the list are buttons drawn off
        // the bottom of the panel. Ending combat sits in the bar above, where
        // a destructive verb is out of the way of the one tapped every turn.
        s.action_bar([
            ("turn-previous", "Previous"),
            ("turn-next", "Next turn"),
            ("init-add", "Add"),
        ])
        .build()
    }

    /// One combatant, with the things a table does to one.
    ///
    /// A screen of its own rather than a menu hanging off the row, because
    /// that is how this application already opens a party member, and because
    /// a popover over a list spends a fifth ink on a panel that has four.
    fn combatant(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let Some(index) = self
            .init_menu
            .filter(|index| *index < self.initiative.len())
        else {
            return self.initiative(s, context);
        };
        let combatant = &self.initiative[index];
        let mut facts = vec![("Initiative", combatant.initiative.to_string())];
        if let Some((hp, max)) = combatant.hp {
            facts.push(("HP", format!("{hp}/{max}")));
        }
        facts.push((
            "Turn",
            if index == self.current {
                "Taking this turn".to_owned()
            } else {
                format!("{} of {}", index + 1, self.initiative.len())
            },
        ));
        let s = s.heading(combatant.name.clone()).facts(facts);
        let s = if combatant.hp.is_some() {
            hp_controls(s)
        } else {
            s
        };
        s.primary_button("init-take", "Take the turn")
            .buttons([("init-edit", "Edit"), ("init-remove", "Remove")])
            .bottom_action("init-back", "Initiative")
            .build()
    }

    fn party_prefix(s: ScreenBuilder) -> ScreenBuilder {
        s.secondary("Select a member to change hit points, saves, or slots.")
    }

    fn party_rows(&self, context: &Context) -> Vec<(String, String)> {
        self.party
            .iter()
            .map(|member| {
                (
                    context.one_line_row(&member.name, true),
                    format!("AC {} · HP {}/{}", member.ac, member.hp, member.max_hp),
                )
            })
            .collect()
    }

    fn party_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.party_rows(context);
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(name, detail)| (name.as_str(), detail.as_str()))
            .collect();
        let prefix =
            Self::party_prefix(ScreenBuilder::new("grimoire").top_bar(self.title())).build();
        let pages =
            context.paginate_rows_under(&borrowed, true, kobo_sdk::Position::AtTheFoot, &prefix);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// The party, which is six people at a full table.
    ///
    /// Measured like any other list: drawn as one flow, the sixth member was
    /// set underneath the Add member button and clipped by the foot of the
    /// panel at the larger text sizes, which is exactly the size somebody
    /// running a game by lamplight has it set to.
    fn party(&self, s: ScreenBuilder, context: &Context) -> Screen {
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
        let rows = self.party_rows(context);
        let pages = self.party_pages(context);
        let page = self.page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        let mut s = Self::party_prefix(s).rows(shown.iter().map(|&index| {
            (
                format!("party-{index}"),
                rows[index].0.clone(),
                rows[index].1.clone(),
                Glyph::Person,
            )
        }));
        if pages.len() > 1 {
            s = s.page_turns("previous", "next").page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        s.bottom_action("party-add", "Add member").build()
    }

    /// One party member, in two pages.
    ///
    /// Hit points and death saves are what a turn asks for; the spell slots
    /// are what the end of a long rest asks for, and they are nine touch
    /// targets. Drawn together they ran off the foot of the panel at the
    /// larger text sizes: the Remove and Party controls were simply not on
    /// the screen, and the Success button had its own word clipped.
    fn member(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let Some(index) = self
            .member
            .and_then(|index| self.party.get(index).map(|_| index))
        else {
            return self.party(s, context);
        };
        let member = &self.party[index];
        // Edit and Remove ride in the bar, on both pages, so the hit point
        // steps and the slot levels keep the body at the largest text sizes.
        let s = s
            .top_bar_glyph("party-remove", "Remove", Glyph::Trash)
            .top_bar_action("party-edit", "Edit")
            .heading(member.name.clone());
        let s = if self.member_page == 0 {
            s.facts([
                ("AC", member.ac.to_string()),
                ("HP", format!("{}/{}", member.hp, member.max_hp)),
                (
                    "Death saves",
                    format!("{} success · {} failure", member.success, member.failure),
                ),
            ])
            .compose(hp_controls)
            .buttons([("save-success", "Success"), ("save-failure", "Failure")])
        } else {
            let (mode, hint) = match self.slot_mode {
                SlotMode::Spend => (0, "Tap a level to spend a slot."),
                SlotMode::Regain => (1, "Tap a level to get a slot back."),
                SlotMode::SetUp => (
                    2,
                    "Tap a level to add a slot. After 4 it goes back to none.",
                ),
            };
            s.segmented(
                mode,
                [
                    ("slots-spend", "Spend"),
                    ("slots-regain", "Regain"),
                    ("slots-setup", "Set up"),
                ],
            )
            .secondary(hint)
            .grid(
                3,
                false,
                member.slots.iter().enumerate().map(|(level, slots)| {
                    (
                        format!("slot-{level}"),
                        if slots.max == 0 {
                            format!("{} · none", ordinal(level + 1))
                        } else {
                            format!(
                                "{} · {} of {} left",
                                ordinal(level + 1),
                                slots.max - slots.used,
                                slots.max
                            )
                        },
                    )
                }),
            )
            .button("slots-rest", "Long rest")
        };
        s.page_turns("previous", "next")
            .page_position(u16::try_from(self.member_page + 1).unwrap_or(1), 2)
            .bottom_action("party-back", "Party")
            .build()
    }

    fn edit(&self, s: ScreenBuilder, context: &Context) -> Screen {
        let Some(edit) = &self.edit else {
            return self.initiative(s, context);
        };
        let prompts = match edit.target {
            Editor::Init(_) => ["Name", "Initiative", "Hit points, or leave blank", ""],
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
            .map(|x| match x.hp {
                Some((hp, max)) => format!(
                    "{}~{}~{hp}~{max}",
                    x.name.replace(['|', '~'], " "),
                    x.initiative
                ),
                None => format!("{}~{}", x.name.replace(['|', '~'], " "), x.initiative),
            })
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
                        .map(|slots| format!("{}/{}", slots.used, slots.max))
                        .collect::<Vec<_>>()
                        .join(",")
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
                .map(|c| {
                    vec![
                        c.name.clone(),
                        c.initiative.to_string(),
                        c.hp.map(|(_, max)| max.to_string()).unwrap_or_default(),
                    ]
                })
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
            values: Vec::new(),
            existing: values,
        });
        self.view = View::Edit;
    }
    fn complete_edit(&mut self, c: &mut Context, text: String) {
        let Some(mut edit) = self.edit.take() else {
            return;
        };
        edit.values.push(text);
        let count = match edit.target {
            Editor::Init(_) => 3,
            Editor::Party(_) => 4,
        };
        if edit.values.len() < count {
            // Every step after the name is a number, so an empty one opens
            // on the digits; one being edited holds what it held before.
            edit.step = edit.values.len();
            self.keyboard = match edit.existing.get(edit.step).filter(|v| !v.is_empty()) {
                Some(value) => Keyboard::with_text(value.clone()),
                None => Keyboard::numeric(),
            };
            self.edit = Some(edit);
            return;
        }
        match edit.target {
            Editor::Init(index) => {
                let value = edit.values[1].parse().unwrap_or(0);
                let before = index
                    .and_then(|index| self.initiative.get(index))
                    .and_then(|combatant| combatant.hp);
                // A new maximum starts the combatant at full health; the same
                // maximum keeps whatever damage they have taken.
                let hp = edit.values[2]
                    .trim()
                    .parse::<i16>()
                    .ok()
                    .filter(|max| *max > 0)
                    .map(|max| match before {
                        Some((hp, old)) if old == max => (hp, max),
                        _ => (max, max),
                    });
                let item = Combatant {
                    name: edit.values[0].clone(),
                    initiative: value,
                    hp,
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
                    .map_or((0, 0, [Slots::default(); 9]), |member| {
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
/// Spell slots as saved: `used/max` for each level, comma separated. A save
/// from before levels held counts is nine ticks, `0` or `1`; a ticked level
/// comes back as one slot, spent, and an unticked one as none, since the old
/// box could not say how many the character had.
fn read_slots(field: &str) -> [Slots; 9] {
    let mut slots = [Slots::default(); 9];
    if field.contains('/') {
        for (level, pair) in field.split(',').take(9).enumerate() {
            let mut numbers = pair.split('/').map(|n| n.parse::<u8>().unwrap_or(0));
            let used = numbers.next().unwrap_or(0);
            let max = numbers.next().unwrap_or(0).min(MOST_SLOTS);
            slots[level] = Slots {
                used: used.min(max),
                max,
            };
        }
    } else {
        for (level, tick) in field.chars().take(9).enumerate() {
            if tick == '1' {
                slots[level] = Slots { used: 1, max: 1 };
            }
        }
    }
    slots
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
                                let hp = p
                                    .get(2)
                                    .zip(p.get(3))
                                    .and_then(|(hp, max)| {
                                        Some((hp.parse().ok()?, max.parse().ok()?))
                                    })
                                    .filter(|(hp, max): &(i16, i16)| {
                                        *max > 0 && (0..=*max).contains(hp)
                                    });
                                Some(Combatant {
                                    name: (*p.first()?).to_string(),
                                    initiative: p.get(1)?.parse().ok()?,
                                    hp,
                                })
                            })
                            .collect()
                    });
                    self.party = f.get(4).map_or_else(Vec::new, |x| {
                        x.split(';')
                            .filter_map(|r| {
                                let p: Vec<_> = r.split('~').collect();
                                let slots = read_slots(p.get(6)?);
                                Some(Member {
                                    name: (*p.first()?).to_string(),
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
    fn on_task(&mut self, c: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.book.woke(c, task, &outcome) != kobo_bookview::Step::Elsewhere
            && self.view == View::Detail
        {
            self.show(c);
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Bookmarks {
            self.on_action(
                context,
                action_id(if forward {
                    "bookmarks-next"
                } else {
                    "bookmarks-previous"
                }),
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if self.view == View::Bookmarks
            && (a == action_id("bookmarks-next") || a == action_id("bookmarks-previous"))
        {
            let last = self.bookmark_pages(c).len().saturating_sub(1);
            self.bookmarks_page = if a == action_id("bookmarks-next") {
                self.bookmarks_page.saturating_add(1).min(last)
            } else {
                self.bookmarks_page.min(last).saturating_sub(1)
            };
            self.show(c);
            return;
        }
        if self.filter_action(c, a) {
            self.show(c);
            return;
        }
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
        // The open reference answers its own page turns and controls first.
        if self.view == View::Detail && self.book.memory().is_some() {
            match self.book.act(c, a) {
                Some(kobo_read::Outcome::Close) => self.close_reference(c),
                Some(kobo_read::Outcome::Light(level)) => c.device().set_frontlight(level),
                None if a == ActionId::BACK => self.close_reference(c),
                _ => {}
            }
            self.show(c);
            return;
        }
        if (a == ActionId::BACK || a == action_id("back")) && self.entry_menu.is_some() {
            // The scrim beside an open menu takes Back before the view does,
            // or putting the menu away leaves the compendium.
            self.entry_menu = None;
        } else if a == ActionId::BACK || a == action_id("back") {
            self.init_menu = None;
            self.view = match self.view {
                View::Member => View::Party,
                View::Combatant => View::Initiative,
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
        } else if let Some(kind) = Kind::ALL
            .into_iter()
            .find(|kind| a == action_id(kind.action()))
        {
            self.kind = kind;
            self.bookmarks_open = false;
            self.clear_filters();
            self.query.clear();
            self.view = View::Compendium;
        } else if a == action_id("dice") {
            self.view = View::Dice;
        } else if a == action_id("initiative") {
            self.page = 0;
            self.view = View::Initiative;
        } else if a == action_id("party") {
            self.page = 0;
            self.view = View::Party;
        } else if a == action_id("about") {
            self.detail_page = 0;
            self.view = View::About;
        } else if a == action_id("edition-2014") || a == action_id("edition-2024") {
            self.edition = if a == action_id("edition-2014") {
                2014
            } else {
                2024
            };
            self.clear_filters();
            self.save(c);
        } else if a == action_id("search") {
            self.keyboard = Keyboard::with_text(&self.query);
            self.view = View::Search;
        } else if a == action_id("bookmarks") {
            self.bookmarks_page = 0;
            self.bookmarks_open = true;
            self.view = View::Bookmarks;
        } else if a == action_id("previous") {
            match self.view {
                View::About => self.detail_page = self.detail_page.saturating_sub(1),
                View::Member => self.member_page = 0,
                _ => self.page = self.page.saturating_sub(1),
            }
        } else if a == action_id("next") {
            match self.view {
                View::About => self.detail_page += 1,
                View::Member => self.member_page = 1,
                View::Initiative => {
                    self.page =
                        (self.page + 1).min(self.initiative_pages(c).len().saturating_sub(1));
                }
                _ => {
                    self.page =
                        (self.page + 1).min(self.compendium_pages(c).len().saturating_sub(1));
                }
            }
        } else if a == action_id("turn-next") {
            if !self.initiative.is_empty() {
                self.current = (self.current + 1) % self.initiative.len();
                if self.current == 0 {
                    self.round += 1;
                }
                self.save(c);
            }
        } else if a == action_id("turn-previous") {
            // A turn passed by accident is the common mistake at a table, and
            // it used to cost the round as well: stepping back from the first
            // combatant takes the round back with it.
            if !self.initiative.is_empty() {
                if self.current == 0 {
                    self.current = self.initiative.len() - 1;
                    self.round = self.round.saturating_sub(1).max(1);
                } else {
                    self.current -= 1;
                }
                self.save(c);
            }
        } else if a == action_id("init-clear") {
            self.initiative.clear();
            self.init_menu = None;
            self.current = 0;
            self.round = 1;
            self.page = 0;
            self.save(c);
        } else if a == action_id("init-take") {
            if let Some(index) = self.init_menu.take().filter(|i| *i < self.initiative.len()) {
                self.current = index;
                self.view = View::Initiative;
                self.save(c);
            }
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
            self.init_menu = None;
            self.start_edit(Editor::Init(None));
        } else if a == action_id("init-edit") {
            let chosen = self.init_menu.take().unwrap_or(self.current);
            self.start_edit(Editor::Init(self.initiative.get(chosen).map(|_| chosen)));
        } else if a == action_id("init-remove") {
            let chosen = self.init_menu.take().unwrap_or(self.current);
            if chosen < self.initiative.len() {
                self.initiative.remove(chosen);
                // The turn belongs to whoever is left standing at that place.
                self.current = self.current.min(self.initiative.len().saturating_sub(1));
                self.view = View::Initiative;
                self.save(c);
            }
        } else if a == action_id("init-back") {
            self.init_menu = None;
            self.view = View::Initiative;
        } else if let Some(index) =
            (0..self.initiative.len()).find(|i| a == action_id(&format!("init-{i}")))
        {
            self.init_menu = Some(index);
            self.view = View::Combatant;
        } else if a == action_id("add-init") {
            if let Some(i) = self.entry_menu.take().or(self.detail) {
                let entry = &self.corpus[i];
                self.initiative.push(Combatant {
                    name: entry.name.clone(),
                    initiative: 0,
                    hp: (entry.kind == "monster")
                        .then(|| monster_hp(&entry.body))
                        .flatten()
                        .map(|hp| (hp, hp)),
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
        } else if let (Some(step), View::Combatant, Some(index)) =
            (hp_step(a), self.view, self.init_menu)
        {
            if let Some((hp, max)) = self.initiative.get_mut(index).and_then(|c| c.hp.as_mut()) {
                *hp = (*hp + step).clamp(0, *max);
                self.save(c);
            }
        } else if a == action_id("slots-spend") {
            self.slot_mode = SlotMode::Spend;
        } else if a == action_id("slots-regain") {
            self.slot_mode = SlotMode::Regain;
        } else if a == action_id("slots-setup") {
            self.slot_mode = SlotMode::SetUp;
        } else if let Some(i) = self.member.filter(|i| *i < self.party.len()) {
            let m = &mut self.party[i];
            if let Some(step) = hp_step(a) {
                m.hp = (m.hp + step).clamp(0, m.max_hp);
            } else if a == action_id("slots-rest") {
                for slots in &mut m.slots {
                    slots.used = 0;
                }
            } else if a == action_id("save-success") {
                m.success = (m.success + 1) % 4;
            } else if a == action_id("save-failure") {
                m.failure = (m.failure + 1) % 4;
            } else if let Some(level) =
                (0..m.slots.len()).find(|level| a == action_id(&format!("slot-{level}")))
            {
                let slots = &mut m.slots[level];
                match self.slot_mode {
                    SlotMode::Spend => slots.used = (slots.used + 1).min(slots.max),
                    SlotMode::Regain => slots.used = slots.used.saturating_sub(1),
                    SlotMode::SetUp => {
                        slots.max = if slots.max >= MOST_SLOTS {
                            0
                        } else {
                            slots.max + 1
                        };
                        slots.used = slots.used.min(slots.max);
                    }
                }
            } else {
                self.show(c);
                return;
            }
            self.save(c);
        } else if a == action_id("bookmark") {
            if let Some(i) = self.entry_menu.take().or(self.detail) {
                if let Some(at) = self.bookmarks.iter().position(|x| *x == i) {
                    self.bookmarks.remove(at);
                } else {
                    self.bookmarks.push(i);
                }
                self.save(c);
            }
        } else if let Some(i) =
            (0..self.corpus.len()).find(|i| a == action_id(&format!("entry-menu-{i}")))
        {
            self.entry_menu = Some(i);
        } else if let Some(i) =
            (0..self.corpus.len()).find(|i| a == action_id(&format!("entry-{i}")))
        {
            self.open_reference(c, i);
        } else if let Some(i) =
            (0..self.party.len()).find(|i| a == action_id(&format!("party-{i}")))
        {
            self.member = Some(i);
            self.member_page = 0;
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

    fn ticks(flags: [bool; 9]) -> [Slots; 9] {
        flags.map(|ticked| Slots {
            used: u8::from(ticked),
            max: u8::from(ticked),
        })
    }
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
                hp: None,
            },
            Combatant {
                name: "High".into(),
                initiative: 20,
                hp: None,
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
                slots: [Slots::default(); 9],
            })
            .collect();
        app.member = Some(4);
        app.party[4].hp -= 3;
        app.party[4].slots[2] = Slots { used: 1, max: 2 };
        assert_eq!(app.party[0].hp, 10);
        assert_eq!(app.party[4].hp, 7);
        assert_eq!(app.party[4].slots[2].used, 1);
        assert_eq!(app.party.len(), 6);
    }
    #[test]
    fn persisted_initiative_and_party_state_round_trips() {
        let mut saved = Grimoire::default();
        saved.round = 4;
        saved.initiative = vec![Combatant {
            name: "Owlbear".into(),
            initiative: 15,
            hp: None,
        }];
        saved.party = vec![Member {
            name: "Moss".into(),
            ac: 14,
            max_hp: 18,
            hp: 9,
            success: 2,
            failure: 1,
            slots: ticks([true, false, true, false, false, false, false, false, false]),
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
        assert_eq!(restored.party[0].slots[2], Slots { used: 1, max: 1 });
        assert_eq!(restored.bookmarks, vec![2, 8]);
    }
    fn ready() -> kobo_sdk::AppRunner<Grimoire> {
        let mut runner = kobo_sdk::AppRunner::new(Grimoire::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        runner
    }

    #[test]
    fn hit_points_move_in_steps_of_one_five_and_ten_and_stop_at_zero_and_full() {
        let mut runner = ready();
        runner.app_mut().party.push(Member {
            name: "Aria".into(),
            ac: 14,
            max_hp: 30,
            hp: 30,
            success: 0,
            failure: 0,
            slots: [Slots::default(); 9],
        });
        runner.action(action_id("party-0"));
        runner.action(action_id("hp-down-10"));
        runner.action(action_id("hp-down-5"));
        runner.action(action_id("hp-down"));
        assert_eq!(runner.app().party[0].hp, 14);
        runner.action(action_id("hp-down-10"));
        runner.action(action_id("hp-down-10"));
        assert_eq!(runner.app().party[0].hp, 0);
        for _ in 0..4 {
            runner.action(action_id("hp-up-10"));
        }
        assert_eq!(runner.app().party[0].hp, 30);
    }

    #[test]
    fn a_monster_added_from_its_reference_brings_its_hit_points_and_takes_damage() {
        let mut runner = ready();
        let aboleth = runner
            .app()
            .corpus
            .iter()
            .position(|entry| entry.edition == 2014 && entry.name == "Aboleth")
            .expect("the SRD has an aboleth");
        runner.app_mut().detail = Some(aboleth);
        runner.action(action_id("add-init"));
        assert_eq!(runner.app().initiative[0].hp, Some((135, 135)));
        runner.app_mut().view = View::Initiative;
        runner.action(action_id("init-0"));
        runner.action(action_id("hp-down-10"));
        runner.action(action_id("hp-down-5"));
        assert_eq!(runner.app().initiative[0].hp, Some((120, 135)));
        assert!(runner
            .app()
            .initiative_rows(&runner.context())
            .iter()
            .any(|(_, detail)| detail.contains("HP 120/135")));
    }

    #[test]
    fn a_spell_level_holds_several_slots_that_are_spent_regained_and_restored_by_a_long_rest() {
        let mut runner = ready();
        runner.app_mut().party.push(Member {
            name: "Wizard".into(),
            ac: 12,
            max_hp: 20,
            hp: 20,
            success: 0,
            failure: 0,
            slots: [Slots::default(); 9],
        });
        runner.action(action_id("party-0"));
        runner.action(action_id("slots-setup"));
        for _ in 0..4 {
            runner.action(action_id("slot-0"));
        }
        assert_eq!(runner.app().party[0].slots[0], Slots { used: 0, max: 4 });
        runner.action(action_id("slots-spend"));
        for _ in 0..5 {
            runner.action(action_id("slot-0"));
        }
        assert_eq!(
            runner.app().party[0].slots[0].used,
            4,
            "spent more than it had"
        );
        runner.action(action_id("slots-regain"));
        runner.action(action_id("slot-0"));
        assert_eq!(runner.app().party[0].slots[0].used, 3);
        runner.action(action_id("slots-rest"));
        assert_eq!(runner.app().party[0].slots[0], Slots { used: 0, max: 4 });
        runner.action(action_id("slots-setup"));
        runner.action(action_id("slot-0"));
        assert_eq!(runner.app().party[0].slots[0].max, 0, "four wraps to none");
    }

    #[test]
    fn slots_saved_as_ticks_by_the_old_build_come_back_as_one_spent_slot() {
        let slots = read_slots("010000000");
        assert_eq!(slots[1], Slots { used: 1, max: 1 });
        assert_eq!(slots[0], Slots::default());
        let slots = read_slots("1/4,0/3,0/0,0/0,0/0,0/0,0/0,0/0,0/0");
        assert_eq!(slots[0], Slots { used: 1, max: 4 });
        assert_eq!(slots[1], Slots { used: 0, max: 3 });
        assert_eq!(read_slots("9/2")[0], Slots { used: 2, max: 2 });
    }

    #[test]
    fn editing_a_member_keeps_what_was_not_retyped_and_takes_what_was() {
        let mut runner = ready();
        runner.app_mut().party.push(Member {
            name: "Moss".into(),
            ac: 12,
            max_hp: 10,
            hp: 8,
            success: 0,
            failure: 0,
            slots: [Slots::default(); 9],
        });
        runner.app_mut().member = Some(0);
        runner.action(action_id("party-edit"));
        assert_eq!(runner.app().keyboard.text(), "Moss");
        runner.action(action_id("kb.enter"));
        assert_eq!(
            runner.app().keyboard.text(),
            "12",
            "the armour class step lost its value"
        );
        runner.app_mut().keyboard = Keyboard::with_text("15");
        runner.action(action_id("kb.enter"));
        runner.action(action_id("kb.enter"));
        runner.action(action_id("kb.enter"));
        assert_eq!(runner.app().party[0].name, "Moss");
        assert_eq!(runner.app().party[0].ac, 15);
        assert_eq!(
            (runner.app().party[0].hp, runner.app().party[0].max_hp),
            (8, 10)
        );
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
            slots: ticks([false, true, false, false, false, false, false, false, false]),
        };
        let mut app = Grimoire {
            party: vec![member],
            edit: Some(Edit {
                target: Editor::Party(Some(0)),
                step: 3,
                values: vec!["Moss".into(), "14".into(), "18".into()],
                existing: Vec::new(),
            }),
            ..Grimoire::default()
        };
        app.complete_edit(&mut Context::default(), "12".into());
        assert_eq!(app.party[0].ac, 14);
        assert_eq!((app.party[0].success, app.party[0].failure), (2, 1));
        assert_eq!(app.party[0].slots[1].used, 1);
    }
    /// Every screen a table touches during a fight, at every size the
    /// interface offers.
    #[test]
    fn initiative_party_and_member_screens_fit_every_supported_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Grimoire::default(), metrics).context();
            let mut app = Grimoire {
                initiative: vec![Combatant {
                    name: "Goblin of the long road".into(),
                    initiative: 12,
                    hp: Some((5, 7)),
                }],
                party: vec![Member {
                    name: "Aria of the long road".into(),
                    ac: 15,
                    max_hp: 22,
                    hp: 18,
                    success: 1,
                    failure: 2,
                    slots: ticks([true, false, true, false, true, false, false, false, false]),
                }],
                member: Some(0),
                init_menu: Some(0),
                ..Grimoire::default()
            };
            for view in [
                View::Initiative,
                View::Combatant,
                View::Party,
                View::Member,
                View::Dice,
                View::About,
            ] {
                app.view = view;
                for page in 0..2 {
                    app.member_page = page;
                    let diagnostics = app
                        .screen(&context)
                        .diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{text_scale:?} {view:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                }
            }
        }
    }

    #[test]
    fn compendium_pages_fit_below_filters_without_losing_entries() {
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
        {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_ui::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let context =
                    kobo_sdk::AppRunner::with_metrics(Grimoire::default(), metrics).context();
                for kind in Kind::ALL {
                    let mut app = Grimoire {
                        view: View::Compendium,
                        kind,
                        ..Grimoire::default()
                    };
                    let pages = app.compendium_pages(&context);
                    assert_eq!(
                        pages.iter().flatten().copied().collect::<Vec<_>>(),
                        (0..app.entries().len()).collect::<Vec<_>>()
                    );
                    for page in [0, pages.len().saturating_sub(1)] {
                        app.page = page;
                        let diagnostics = app
                            .screen(&context)
                            .diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{metrics:?}, {kind:?}: {:?}",
                            diagnostics.issues
                        );
                    }
                }
            }
        }
    }

    /// The whole of a reference has to be reachable. The rule sections run to
    /// several thousand words with their own headings and tables, and drawn as
    /// one flow of interface text the panel showed the first screenful and
    /// dropped the rest without saying so.
    #[test]
    fn the_longest_reference_can_be_read_to_its_last_word_at_every_text_size() {
        let corpus = corpus::load();
        let (index, longest) = corpus
            .iter()
            .enumerate()
            .max_by_key(|(_, entry)| entry.body.len())
            .expect("a corpus with entries");
        // The last word a reader can actually see, taken from the markup the
        // same way the reader takes it: tags out, text kept.
        let mut plain = String::new();
        let mut inside = false;
        for character in longest.body.chars() {
            match character {
                '<' => inside = true,
                '>' => inside = false,
                other if !inside => plain.push(other),
                _ => {}
            }
        }
        let ending = plain
            .split_whitespace()
            .next_back()
            .expect("a body with words")
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .to_owned();
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = kobo_sdk::AppRunner::with_metrics(Grimoire::default(), metrics);
            runner.start();
            runner.store_result(StoreResult::Loaded {
                key: STATE.into(),
                value: None,
            });
            runner.action(action_id(&format!("entry-{index}")));
            assert_eq!(runner.app().view, View::Detail);
            let pages = runner
                .app()
                .book
                .memory()
                .map(|memory| memory.at)
                .map_or(0, |_| 1);
            assert_eq!(pages, 1, "the reference did not open");
            let mut seen = Vec::new();
            for _ in 0..400 {
                let screen = runner
                    .app()
                    .book
                    .screen(&longest.name)
                    .expect("an open reference draws a page");
                let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                assert!(
                    !diagnostics
                        .issues
                        .iter()
                        .any(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error),
                    "{text_scale:?}: {:?}",
                    diagnostics.issues
                );
                seen.push(page_text(&screen));
                let before = runner.app().book.memory().map(|memory| memory.at);
                runner.action(action_id(kobo_read::action::FORWARD));
                if runner.app().book.memory().map(|memory| memory.at) == before {
                    break;
                }
            }
            assert!(
                seen.len() > 1,
                "{text_scale:?}: the longest entry fitted one page"
            );
            assert!(
                seen.last().is_some_and(|page| page.contains(&ending)),
                "{text_scale:?}: the last page does not reach the end of the text"
            );
            // The reference keeps its structure rather than its markup.
            let whole = seen.join(" ");
            assert!(
                !whole.contains("##"),
                "{text_scale:?}: a heading arrived as markup"
            );
            assert!(
                !whole.contains("**"),
                "{text_scale:?}: emphasis arrived as markup"
            );
        }
    }

    fn page_text(screen: &Screen) -> String {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn an_edition_without_a_category_says_so_rather_than_blaming_a_filter() {
        let app = Grimoire {
            kind: Kind::Spell,
            edition: 2024,
            ..Grimoire::default()
        };
        let (heading, detail) = app.nothing_here();
        assert!(
            heading.contains("No spells in the 2024 reference"),
            "{heading}"
        );
        assert!(detail.contains("2014"), "{detail}");
        assert_eq!(app.held(Kind::Spell, 2024), 0);

        let filtered = Grimoire {
            kind: Kind::Spell,
            query: "zzzz".into(),
            ..Grimoire::default()
        };
        assert!(filtered.entries().is_empty());
        assert_eq!(filtered.nothing_here().0, "No matching reference");
    }

    #[test]
    fn every_category_the_reference_holds_is_reachable_from_its_tile() {
        let mut runner = kobo_sdk::AppRunner::new(Grimoire::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        for kind in Kind::ALL {
            runner.action(action_id(kind.action()));
            assert_eq!(runner.app().kind, kind);
            assert_eq!(
                runner.app().entries().len(),
                runner.app().held(kind, 2014),
                "{kind:?} is not listed by its own tile"
            );
            assert!(runner.app().held(kind, 2014) > 0, "{kind:?} holds nothing");
            runner.action(ActionId::BACK);
        }
    }

    /// A table of six, a round of six turns and a restart: the state a party
    /// has built up over an evening is the one thing this application cannot
    /// afford to lose.
    #[test]
    fn a_six_person_party_and_its_initiative_survive_a_restart() {
        let mut runner = kobo_sdk::AppRunner::new(Grimoire::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        let names = ["Aria", "Brannoc", "Cass", "Delphine", "Emeric", "Fenn"];
        for (number, name) in names.iter().enumerate() {
            runner.app_mut().party.push(Member {
                name: (*name).to_owned(),
                ac: 14 + u8::try_from(number).unwrap(),
                max_hp: 20 + i16::try_from(number).unwrap(),
                hp: 20 + i16::try_from(number).unwrap(),
                success: 0,
                failure: 0,
                slots: [Slots::default(); 9],
            });
            runner.app_mut().initiative.push(Combatant {
                name: (*name).to_owned(),
                initiative: i8::try_from(20 - number * 3).unwrap(),
                hp: None,
            });
        }
        runner.app_mut().sort_init();
        // Three turns, one of them passed by accident and taken back.
        runner.action(action_id("turn-next"));
        runner.action(action_id("turn-next"));
        runner.action(action_id("turn-previous"));
        assert_eq!((runner.app().round, runner.app().current), (1, 1));
        // A member takes damage and burns a slot.
        runner.action(action_id("party-3"));
        runner.action(action_id("hp-down"));
        runner.action(action_id("slots-setup"));
        runner.action(action_id("slot-1"));
        runner.action(action_id("slot-1"));
        runner.action(action_id("slots-spend"));
        runner.action(action_id("slot-1"));
        let commands = runner.action(action_id("save-failure"));
        let written = commands
            .iter()
            .find_map(|command| match command {
                kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key == STATE =>
                {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("the table state is written down");

        let mut reopened = kobo_sdk::AppRunner::new(Grimoire::default());
        reopened.start();
        reopened.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(written),
        });
        assert_eq!(reopened.app().party.len(), 6);
        assert_eq!(reopened.app().initiative.len(), 6);
        assert_eq!(reopened.app().current, 1);
        assert_eq!(reopened.app().round, 1);
        assert_eq!(reopened.app().party[3].hp, runner.app().party[3].hp);
        assert_eq!(reopened.app().party[3].slots[1], Slots { used: 1, max: 2 });
        assert_eq!(reopened.app().party[3].failure, 1);
        let names_back: Vec<&str> = reopened
            .app()
            .initiative
            .iter()
            .map(|combatant| combatant.name.as_str())
            .collect();
        assert_eq!(names_back, names);
    }

    #[test]
    fn passing_the_turn_back_from_the_first_combatant_takes_the_round_with_it() {
        let mut runner = kobo_sdk::AppRunner::new(Grimoire::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        for (name, initiative) in [("Owlbear", 18), ("Moss", 12)] {
            runner.app_mut().initiative.push(Combatant {
                name: name.to_owned(),
                initiative,
                hp: None,
            });
        }
        runner.action(action_id("turn-next"));
        runner.action(action_id("turn-next"));
        assert_eq!((runner.app().round, runner.app().current), (2, 0));
        runner.action(action_id("turn-previous"));
        assert_eq!((runner.app().round, runner.app().current), (1, 1));
        // The first round is the first round; there is no round zero.
        runner.action(action_id("turn-previous"));
        runner.action(action_id("turn-previous"));
        assert_eq!(runner.app().round, 1);
    }

    #[test]
    fn a_combatant_is_edited_and_removed_from_its_own_screen_rather_than_the_turn() {
        let mut runner = kobo_sdk::AppRunner::new(Grimoire::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        for (name, initiative) in [("Owlbear", 18), ("Moss", 12), ("Goblin", 8)] {
            runner.app_mut().initiative.push(Combatant {
                name: name.to_owned(),
                initiative,
                hp: None,
            });
        }
        runner.app_mut().view = View::Initiative;
        runner.action(action_id("init-2"));
        assert_eq!(runner.app().view, View::Combatant);
        runner.action(action_id("init-take"));
        assert_eq!(
            runner.app().current,
            2,
            "the combatant screen did not pass the turn"
        );
        assert_eq!(runner.app().view, View::Initiative);
        runner.action(action_id("init-0"));
        runner.action(action_id("init-remove"));
        assert_eq!(runner.app().initiative.len(), 2);
        assert_eq!(runner.app().initiative[0].name, "Moss");
        // Back from a combatant lands on the order of battle, not the home
        // screen: the list is where it was opened from.
        runner.action(action_id("init-1"));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().init_menu, None);
        assert_eq!(runner.app().view, View::Initiative);
        runner.action(action_id("init-clear"));
        assert!(runner.app().initiative.is_empty());
        assert_eq!((runner.app().round, runner.app().current), (1, 0));
    }

    /// Six at the table, which is a full party, at every size the interface
    /// offers. Drawn as one flow the sixth member fell under the Add member
    /// button and off the foot of the panel.
    #[test]
    fn a_party_of_six_is_wholly_reachable_at_every_supported_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Grimoire::default(), metrics).context();
            let mut app = Grimoire {
                view: View::Party,
                party: (0..6)
                    .map(|number| Member {
                        name: format!("Member {number} of the long road"),
                        ac: 14 + u8::try_from(number).unwrap(),
                        max_hp: 20 + i16::try_from(number).unwrap(),
                        hp: 20 + i16::try_from(number).unwrap(),
                        success: 0,
                        failure: 0,
                        slots: [Slots::default(); 9],
                    })
                    .collect(),
                ..Grimoire::default()
            };
            let pages = app.party_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..6).collect::<Vec<_>>(),
                "{text_scale:?}: a party member was left off every page"
            );
            for page in 0..pages.len() {
                app.page = page;
                let diagnostics = app
                    .screen(&context)
                    .diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?} page {page}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    #[test]
    fn six_combatants_and_one_of_their_screens_fit_every_supported_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Grimoire::default(), metrics).context();
            let mut app = Grimoire {
                view: View::Initiative,
                initiative: (0..6)
                    .map(|number| Combatant {
                        name: format!("Combatant {number} of the long road"),
                        initiative: 20 - i8::try_from(number).unwrap(),
                        hp: None,
                    })
                    .collect(),
                ..Grimoire::default()
            };
            let pages = app.initiative_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..6).collect::<Vec<_>>(),
                "{text_scale:?}: a combatant was left off every page"
            );
            for (page, listed) in pages.iter().enumerate() {
                app.page = page;
                for view in [View::Initiative, View::Combatant] {
                    app.view = view;
                    app.init_menu = listed.first().copied();
                    let diagnostics = app
                        .screen(&context)
                        .diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{text_scale:?} {view:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                }
            }
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

#[cfg(test)]
mod bookmark_tests;
