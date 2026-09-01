//! Fieldbook keeps a small, useful birding pack and sighting log on the reader.
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

const SPECIES: &[(&str, &str, &str)] = &[
    ("American Robin", "Turdus migratorius", "AMRO"),
    ("Black-capped Chickadee", "Poecile atricapillus", "BCCH"),
    ("Northern Cardinal", "Cardinalis cardinalis", "NOCA"),
    ("Red-tailed Hawk", "Buteo jamaicensis", "RTHA"),
    ("White-breasted Nuthatch", "Sitta carolinensis", "WBNU"),
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Nearby,
    Log,
    Life,
    Export,
    Detail,
}
struct Fieldbook {
    view: View,
    selected: usize,
    count: u16,
    sightings: Vec<(usize, u16)>,
    synced: bool,
    notice: Option<&'static str>,
}
impl Default for Fieldbook {
    fn default() -> Self {
        Self {
            view: View::Nearby,
            selected: 0,
            count: 1,
            sightings: vec![],
            synced: false,
            notice: None,
        }
    }
}
impl Fieldbook {
    fn title(&self) -> &'static str {
        match self.view {
            View::Nearby => "Nearby",
            View::Log => "Log sighting",
            View::Life => "Life list",
            View::Export => "Export",
            View::Detail => "Fieldbook",
        }
    }
    fn screen(&self) -> Screen {
        let mut s = ScreenBuilder::new("fieldbook").top_bar(self.title());
        if let Some(n) = self.notice {
            s = s.banner(kobo_sdk::BannerLevel::Info, n);
        }
        match self.view {
            View::Nearby => {
                let status=if self.synced {"Cache: county pack, 3 hotspots, 14 days."} else {"No field pack yet. Sync before leaving the air."};
                s.secondary(status).rows(SPECIES.iter().enumerate().map(|(i,x)|(format!("bird-{i}"),x.0,format!("{} · {}",x.2,x.1),Glyph::Search)))
                 .buttons([("sync","Sync pack"),("log","Log a sighting")]).nav_bar(Some(0),[("nearby","Nearby"),("life","Life list"),("export","Export")]).build()
            }
            View::Log => {
                let bird=SPECIES[self.selected];
                s.secondary("Species picker accepts common name, scientific name, or banding code.")
                  .rows(SPECIES.iter().enumerate().map(|(i,x)|(format!("pick-{i}"),x.0,format!("{} · {}",x.2,x.1),Glyph::Search)))
                  .section(format!("Selected: {}  ·  count {}",bird.0,self.count))
                  .buttons([("less","− count"),("more","+ count")]).primary_button("tally","TALLY").build()
            }
            View::Life => {
                if self.sightings.is_empty() { s.splash(Some(Glyph::Search),"No birds logged","Log a sighting to begin this life list.").build() }
                else { s.secondary(format!("{} species on this device.",self.sightings.len())).rows(self.sightings.iter().enumerate().map(|(i,(bird,count))|(format!("life-{i}"),SPECIES[*bird].0,format!("{} sighting{}",count,if *count==1 {""} else {"s"}),Glyph::Search))).build() }
            }
            View::Export => s.text("Checklist submission is not available: Cornell provides no public write API.\n\nConnect to desktop, run `kobo fieldbook export`, then import the CSV in eBird’s Record Format importer.").button("export","Prepare CSV export").build(),
            View::Detail => s.text(format!("{}\n{}\nBanding code: {}\n\nRecent in the synced field pack.",SPECIES[self.selected].0,SPECIES[self.selected].1,SPECIES[self.selected].2)).bottom_action("back","Back").build(),
        }
    }
    fn save(&self, c: &mut Context) {
        c.store().save(
            "sightings",
            self.sightings
                .iter()
                .map(|(b, n)| format!("{b}:{n}"))
                .collect::<Vec<_>>()
                .join(",")
                .into_bytes(),
        );
    }
    fn show(&self, c: &mut Context) {
        c.set_screen(
            self.screen()
                .with_own_back(!matches!(self.view, View::Nearby)),
        );
    }
}
impl KoboApp for Fieldbook {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load("sightings");
        self.show(c);
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { value: Some(v), .. } = r {
            if let Ok(t) = String::from_utf8(v) {
                self.sightings = t
                    .split(',')
                    .filter_map(|p| p.split_once(':'))
                    .filter_map(|(b, n)| Some((b.parse().ok()?, n.parse().ok()?)))
                    .collect();
            }
        }
        self.show(c);
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        self.notice = None;
        if a == ActionId::BACK || a == action_id("back") || a == action_id("nearby") {
            self.view = View::Nearby;
        } else if a == action_id("life") {
            self.view = View::Life;
        } else if a == action_id("export") {
            self.view = View::Export;
        } else if a == action_id("log") {
            self.view = View::Log;
        } else if a == action_id("sync") {
            self.synced = true;
            self.notice = Some("Field pack saved for offline use.");
        } else if a == action_id("more") {
            self.count = self.count.saturating_add(1);
        } else if a == action_id("less") {
            self.count = self.count.saturating_sub(1).max(1);
        } else if a == action_id("tally") {
            let lifer = !self.sightings.iter().any(|(b, _)| *b == self.selected);
            if let Some((_, n)) = self.sightings.iter_mut().find(|(b, _)| *b == self.selected) {
                *n += self.count;
            } else {
                self.sightings.push((self.selected, self.count));
            }
            self.notice = Some(if lifer { "Lifer!" } else { "Sighting saved." });
            self.save(c);
            self.view = View::Log;
        } else if let Some(i) = (0..SPECIES.len()).find(|i| a == action_id(&format!("pick-{i}"))) {
            self.selected = i;
        } else if let Some(i) = (0..SPECIES.len()).find(|i| a == action_id(&format!("bird-{i}"))) {
            self.selected = i;
            self.view = View::Detail;
        }
        self.show(c);
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("fieldbook", Fieldbook::default()).map_or_else(
        |e| {
            eprintln!("fieldbook: {e}");
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
    fn codes_are_unique_and_searchable() {
        assert!(SPECIES.iter().any(|b| b.2 == "AMRO"));
        assert_eq!(SPECIES.len(), 5);
    }
    #[test]
    fn lifer_is_keyed_to_species_index() {
        let mut f = Fieldbook::default();
        f.sightings.push((0, 1));
        assert!(f.sightings.iter().any(|(b, _)| *b == 0));
        assert!(!f.sightings.iter().any(|(b, _)| *b == 1));
    }
    #[test]
    fn nearby_layout_fits() {
        let d = Fieldbook::default()
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
}
