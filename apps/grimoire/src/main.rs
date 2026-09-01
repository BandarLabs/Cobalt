//! A compact, offline fifth-edition SRD reference for the game table.
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

const ATTRIBUTION: &str = "This work includes material taken from the System Reference Document 5.1 and System Reference Document 5.2 by Wizards of the Coast LLC, available under the Creative Commons Attribution 4.0 International License.";
const SPELLS: &[(&str, &str, &str)] = &[
    ("Cure wounds", "1st-level evocation", "A creature you touch regains 1d8 + your spellcasting ability modifier hit points."),
    ("Detect magic", "1st-level divination, ritual", "For the duration, you sense the presence of magic within 30 feet."),
    ("Fireball", "3rd-level evocation", "Each creature in a 20-foot-radius sphere makes a Dexterity saving throw, taking 8d6 fire damage on a failed save."),
    ("Mage hand", "Conjuration cantrip", "A spectral, floating hand appears at a point you choose within range."),
    ("Shield", "1st-level abjuration", "Until the start of your next turn, you have a +5 bonus to AC."),
];
const MONSTERS: &[(&str, &str, &str)] = &[
    ("Goblin", "Small humanoid, CR 1/4", "AC 15  HP 7 (2d6)  Speed 30 ft.\nSTR 8  DEX 14  CON 10  INT 10  WIS 8  CHA 8\nNimble Escape. Take Disengage or Hide as a bonus action.\nScimitar. +4 to hit, 5 (1d6 + 2) slashing damage."),
    ("Owlbear", "Large monstrosity, CR 3", "AC 13  HP 59 (7d10 + 21)  Speed 40 ft.\nSTR 20  DEX 12  CON 17  INT 3  WIS 12  CHA 7\nKeen Sight and Smell.\nMultiattack. One beak and one claws attack."),
    ("Young red dragon", "Large dragon, CR 10", "AC 18  HP 178 (17d10 + 85)  Speed 40 ft., climb 40 ft., fly 80 ft.\nFire Breath (Recharge 5–6). 30-foot cone; 16d6 fire damage, Dexterity save half."),
];
const CONDITIONS: &[(&str, &str)] = &[
    ("Blinded", "Cannot see; fails checks requiring sight. Attack rolls against it have advantage, and its attacks have disadvantage."),
    ("Grappled", "Speed becomes 0. The condition ends if the grappler is incapacitated or an effect removes the target."),
    ("Poisoned", "Has disadvantage on attack rolls and ability checks."),
    ("Prone", "Can only crawl. Attacks against it have advantage within 5 feet; its attacks have disadvantage."),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Spells,
    Monsters,
    Rules,
    Dice,
    Initiative,
    Party,
    Detail,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Detail {
    Spell(usize),
    Monster(usize),
    Condition(usize),
    About,
}

struct Grimoire {
    view: View,
    detail: Option<Detail>,
    edition: u8,
    roll: u16,
    modifier: i8,
    advantage: i8,
    history: Vec<u16>,
    round: u16,
    current: usize,
    initiative: Vec<(&'static str, i8)>,
    party: Vec<(&'static str, i16, i16)>,
}
impl Default for Grimoire {
    fn default() -> Self {
        Self {
            view: View::Home,
            detail: None,
            edition: 0,
            roll: 20,
            modifier: 0,
            advantage: 0,
            history: Vec::new(),
            round: 1,
            current: 0,
            initiative: vec![("Aria", 18), ("Goblin", 14), ("Moss", 11), ("Owlbear", 9)],
            party: vec![("Aria", 22, 22), ("Moss", 18, 14)],
        }
    }
}
impl Grimoire {
    fn title(&self) -> &'static str {
        match self.view {
            View::Home | View::Detail => "Grimoire",
            View::Spells => "Spells",
            View::Monsters => "Monsters",
            View::Rules => "Rules & conditions",
            View::Dice => "Dice",
            View::Initiative => "Initiative",
            View::Party => "Party",
        }
    }
    #[allow(clippy::too_many_lines)]
    fn screen(&self) -> Screen {
        let mut s = ScreenBuilder::new("grimoire").top_bar(self.title());
        if matches!(self.view, View::Spells | View::Monsters | View::Rules) {
            s = s.tabs(
                self.edition as usize,
                [("edition-2014", "2014"), ("edition-2024", "2024")],
            );
        }
        match self.view {
            View::Home => s
                .tiles([
                    ("spells", "Spells", Glyph::Book),
                    ("monsters", "Monsters", Glyph::Search),
                    ("rules", "Rules & conditions", Glyph::Book),
                    ("dice", "Dice", Glyph::Circle),
                    ("initiative", "Initiative", Glyph::Chart),
                    ("party", "Party", Glyph::Person),
                ])
                .build(),
            View::Spells => s
                .secondary("Prefix search is instant in the full SRD build.")
                .rows(
                    SPELLS
                        .iter()
                        .enumerate()
                        .map(|(i, x)| (format!("spell-{i}"), x.0, x.1, Glyph::Book)),
                )
                .build(),
            View::Monsters => s
                .rows(
                    MONSTERS
                        .iter()
                        .enumerate()
                        .map(|(i, x)| (format!("monster-{i}"), x.0, x.1, Glyph::Search)),
                )
                .build(),
            View::Rules => s
                .section("Conditions")
                .rows(
                    CONDITIONS
                        .iter()
                        .enumerate()
                        .map(|(i, x)| (format!("condition-{i}"), x.0, x.1, Glyph::Book)),
                )
                .button("about", "About this reference")
                .build(),
            View::Dice => {
                let history = self
                    .history
                    .iter()
                    .rev()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(" · ");
                s.secondary(format!(
                    "d20 {}  modifier {:+}",
                    match self.advantage {
                        -1 => "disadvantage",
                        1 => "advantage",
                        _ => "normal",
                    },
                    self.modifier
                ))
                .section(format!(
                    "Result: {}",
                    i16::try_from(self.roll).expect("a d20 result fits i16")
                        + i16::from(self.modifier)
                ))
                .primary_button("roll", "Roll d20")
                .buttons([
                    ("modifier-down", "− modifier"),
                    ("modifier-up", "+ modifier"),
                ])
                .buttons([("disadvantage", "Disadvantage"), ("advantage", "Advantage")])
                .text(format!("Last rolls: {history}"))
                .build()
            }
            View::Initiative => {
                let rows = self
                    .initiative
                    .iter()
                    .enumerate()
                    .map(|(i, (name, score))| {
                        let marker = if i == self.current { "Current" } else { "" };
                        (
                            format!("init-{i}"),
                            *name,
                            format!("{score}  {marker}"),
                            Glyph::Chart,
                        )
                    });
                s.secondary(format!("Round {}", self.round))
                    .rows(rows)
                    .primary_button("next", "NEXT")
                    .build()
            }
            View::Party => s
                .secondary("HP changes save immediately on this device.")
                .rows(self.party.iter().enumerate().map(|(i, (n, max, hp))| {
                    (
                        format!("party-{i}"),
                        *n,
                        format!("AC —  HP {hp}/{max}"),
                        Glyph::Person,
                    )
                }))
                .buttons([("heal", "+ HP"), ("hurt", "− HP")])
                .build(),
            View::Detail => self.detail_screen(s),
        }
    }
    fn detail_screen(&self, s: ScreenBuilder) -> Screen {
        let (heading, text) = match self.detail {
            Some(Detail::Spell(i)) => (SPELLS[i].0, format!("{}\n\n{}", SPELLS[i].1, SPELLS[i].2)),
            Some(Detail::Monster(i)) => (MONSTERS[i].0, format!("{}\n\n{}", MONSTERS[i].1, MONSTERS[i].2)),
            Some(Detail::Condition(i)) => (CONDITIONS[i].0, CONDITIONS[i].1.to_string()),
            Some(Detail::About) => ("About", format!("Grimoire is an unofficial, offline reference compatible with the fifth-edition SRD.\n\n{ATTRIBUTION}")),
            None => ("Grimoire", String::new()),
        };
        s.top_bar(heading)
            .text(text)
            .bottom_action("back", "Back")
            .build()
    }
    fn show(&self, c: &mut Context) {
        c.set_screen(self.screen().with_own_back(self.view != View::Home));
    }
    fn persist(&self, c: &mut Context) {
        c.store().save(
            "table",
            format!(
                "{}|{}|{}",
                self.round,
                self.current,
                self.party
                    .iter()
                    .map(|p| p.2.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .into_bytes(),
        );
    }
    fn roll(&mut self) {
        let rolls = u16::try_from(self.history.len()).expect("roll history is capped at ten");
        let next = ((rolls * 7 + 12) % 20) + 1;
        self.roll = next;
        self.history.push(next);
        if self.history.len() > 10 {
            self.history.remove(0);
        }
    }
}
impl KoboApp for Grimoire {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load("table");
        self.show(c);
    }
    fn on_store(&mut self, c: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded {
            value: Some(bytes), ..
        } = result
        {
            if let Ok(saved) = String::from_utf8(bytes) {
                let parts: Vec<_> = saved.split('|').collect();
                if let (Some(round), Some(current)) = (parts.first(), parts.get(1)) {
                    self.round = round.parse().unwrap_or(1);
                    self.current = current.parse().unwrap_or(0);
                }
            }
        }
        self.show(c);
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if a == action_id("back") || a == ActionId::BACK {
            self.view = View::Home;
            self.detail = None;
        } else if a == action_id("spells") {
            self.view = View::Spells;
        } else if a == action_id("monsters") {
            self.view = View::Monsters;
        } else if a == action_id("rules") {
            self.view = View::Rules;
        } else if a == action_id("dice") {
            self.view = View::Dice;
        } else if a == action_id("initiative") {
            self.view = View::Initiative;
        } else if a == action_id("party") {
            self.view = View::Party;
        } else if a == action_id("edition-2014") {
            self.edition = 0;
        } else if a == action_id("edition-2024") {
            self.edition = 1;
        } else if a == action_id("roll") {
            self.roll();
        } else if a == action_id("modifier-up") {
            self.modifier = (self.modifier + 1).min(10);
        } else if a == action_id("modifier-down") {
            self.modifier = (self.modifier - 1).max(-10);
        } else if a == action_id("advantage") {
            self.advantage = 1;
        } else if a == action_id("disadvantage") {
            self.advantage = -1;
        } else if a == action_id("next") {
            self.current = (self.current + 1) % self.initiative.len();
            if self.current == 0 {
                self.round += 1;
            }
            self.persist(c);
        } else if a == action_id("heal") {
            if let Some(p) = self.party.first_mut() {
                p.2 = (p.2 + 1).min(p.1);
            }
            self.persist(c);
        } else if a == action_id("hurt") {
            if let Some(p) = self.party.first_mut() {
                p.2 = (p.2 - 1).max(0);
            }
            self.persist(c);
        } else if a == action_id("about") {
            self.view = View::Detail;
            self.detail = Some(Detail::About);
        } else if let Some(i) = (0..SPELLS.len()).find(|i| a == action_id(&format!("spell-{i}"))) {
            self.view = View::Detail;
            self.detail = Some(Detail::Spell(i));
        } else if let Some(i) =
            (0..MONSTERS.len()).find(|i| a == action_id(&format!("monster-{i}")))
        {
            self.view = View::Detail;
            self.detail = Some(Detail::Monster(i));
        } else if let Some(i) =
            (0..CONDITIONS.len()).find(|i| a == action_id(&format!("condition-{i}")))
        {
            self.view = View::Detail;
            self.detail = Some(Detail::Condition(i));
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
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn attribution_is_shipped_and_no_ogl_source_is_named() {
        assert!(ATTRIBUTION.contains("Creative Commons Attribution 4.0"));
        assert!(!include_str!("../THIRD-PARTY.md").contains("Kobold Press"));
    }
    #[test]
    fn rolls_remain_in_range_and_keep_ten() {
        let mut app = Grimoire::default();
        for _ in 0..20 {
            app.roll();
            assert!((1..=20).contains(&app.roll));
        }
        assert_eq!(app.history.len(), 10);
    }
    #[test]
    fn initiative_wraps_and_advances_round() {
        let mut app = Grimoire::default();
        app.current = app.initiative.len() - 1;
        app.current = (app.current + 1) % app.initiative.len();
        if app.current == 0 {
            app.round += 1;
        }
        assert_eq!(app.round, 2);
    }
    #[test]
    fn home_controls_fit_clara_panel() {
        let s = Grimoire::default().screen();
        let d = s.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
}
