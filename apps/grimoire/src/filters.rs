use super::{
    action_id, options, ActionId, Context, Glyph, Grimoire, Kind, Screen, ScreenBuilder, Tri, View,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Filter {
    Class,
    Level,
    School,
    Ritual,
    Concentration,
    Challenge,
    Type,
}

impl Filter {
    pub(super) const fn title(self) -> &'static str {
        match self {
            Self::Class => "Class",
            Self::Level => "Level",
            Self::School => "School",
            Self::Ritual => "Ritual",
            Self::Concentration => "Concentration",
            Self::Challenge => "Challenge rating",
            Self::Type => "Monster type",
        }
    }
    const fn action(self) -> &'static str {
        match self {
            Self::Class => "spell-class",
            Self::Level => "spell-level",
            Self::School => "spell-school",
            Self::Ritual => "ritual",
            Self::Concentration => "concentration",
            Self::Challenge => "monster-cr",
            Self::Type => "monster-type",
        }
    }
    fn values(self, app: &Grimoire) -> Vec<String> {
        match self {
            Self::Class => options(app, "class"),
            Self::School => options(app, "school"),
            Self::Type => options(app, "type"),
            Self::Level => std::iter::once("Any".to_owned())
                .chain((0..=9).map(|level| {
                    if level == 0 {
                        "Cantrip".to_owned()
                    } else {
                        format!("Level {level}")
                    }
                }))
                .collect(),
            Self::Ritual | Self::Concentration => ["Any", "Yes", "No"].map(str::to_owned).to_vec(),
            Self::Challenge => [
                "Any",
                "0",
                "Above 0 through 1",
                "Above 1 through 4",
                "Above 4 through 10",
                "Above 10",
            ]
            .map(str::to_owned)
            .to_vec(),
        }
    }
    fn selected(self, app: &Grimoire) -> usize {
        match self {
            Self::Class => app.spell_class,
            Self::Level => app.spell_level.map_or(0, |n| usize::from(n) + 1),
            Self::School => app.spell_school,
            Self::Challenge => app.cr,
            Self::Type => app.monster_type,
            Self::Ritual | Self::Concentration => match if self == Self::Ritual {
                app.ritual
            } else {
                app.concentration
            } {
                Tri::Any => 0,
                Tri::Yes => 1,
                Tri::No => 2,
            },
        }
    }
    fn set(self, app: &mut Grimoire, index: usize) {
        if index >= self.values(app).len() {
            return;
        }
        match self {
            Self::Class => app.spell_class = index,
            Self::Level => {
                app.spell_level = index.checked_sub(1).and_then(|n| u8::try_from(n).ok());
            }
            Self::School => app.spell_school = index,
            Self::Challenge => app.cr = index,
            Self::Type => app.monster_type = index,
            Self::Ritual => app.ritual = [Tri::Any, Tri::Yes, Tri::No][index],
            Self::Concentration => app.concentration = [Tri::Any, Tri::Yes, Tri::No][index],
        }
        app.page = 0;
    }
}

impl Grimoire {
    pub(super) fn filters(&self) -> &'static [Filter] {
        match self.kind {
            Kind::Spell => &[
                Filter::Class,
                Filter::Level,
                Filter::School,
                Filter::Ritual,
                Filter::Concentration,
            ],
            Kind::Monster => &[Filter::Challenge, Filter::Type],
            // A rule and a magic item are found by name, and the shelf offers
            // search for that. Neither carries an index tag worth filtering on.
            Kind::Rule | Kind::Item => &[],
        }
    }
    pub(super) fn clear_filters(&mut self) {
        self.spell_class = 0;
        self.spell_level = None;
        self.spell_school = 0;
        self.ritual = Tri::Any;
        self.concentration = Tri::Any;
        self.cr = 0;
        self.monster_type = 0;
        self.page = 0;
        self.filter_page = 0;
    }
    fn filter_rows(&self) -> Vec<(String, String, String, Glyph)> {
        if self.view == View::FilterChoice {
            self.filter
                .values(self)
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let selected = index == self.filter.selected(self);
                    (
                        format!("filter-value-{index}"),
                        value,
                        if selected {
                            "Selected".into()
                        } else {
                            String::new()
                        },
                        if selected {
                            Glyph::Check
                        } else {
                            Glyph::Circle
                        },
                    )
                })
                .collect()
        } else {
            self.filters()
                .iter()
                .map(|&filter| {
                    (
                        filter.action().into(),
                        filter.title().into(),
                        filter
                            .values(self)
                            .get(filter.selected(self))
                            .cloned()
                            .unwrap_or_else(|| "Any".into()),
                        Glyph::Filter,
                    )
                })
                .collect()
        }
    }
    fn filter_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.filter_rows();
        let measured = rows
            .iter()
            .map(|(_, title, summary, _)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        context.paginate_rows(&measured, true)
    }
    pub(super) fn filter_screen(&self, context: &Context) -> Screen {
        let rows = self.filter_rows();
        let pages = self.filter_pages(context);
        let page = self.filter_page.min(pages.len().saturating_sub(1));
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let screen = ScreenBuilder::new("grimoire-filters")
            .top_bar(self.title())
            .rows(visible.iter().map(|&index| rows[index].clone()))
            .page_turns("filters-previous", "filters-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            );
        if self.view == View::FilterChoice {
            screen
                .bottom_action("filter-cancel", "Back to filters")
                .build()
        } else {
            screen
                .action_bar([
                    ("clear", "Clear filters"),
                    ("filter-results", "View results"),
                ])
                .build()
        }
    }
    pub(super) fn filter_action(&mut self, context: &Context, action: ActionId) -> bool {
        if action == action_id("filters")
            && self.view == View::Compendium
            && self.kind != Kind::Rule
        {
            self.view = View::Filters;
            self.filter_page = 0;
            return true;
        }
        if !matches!(self.view, View::Filters | View::FilterChoice) {
            return false;
        }
        if action == ActionId::BACK
            || action == action_id("filter-cancel")
            || action == action_id("filter-results")
        {
            self.view = if self.view == View::FilterChoice {
                View::Filters
            } else {
                View::Compendium
            };
            self.filter_page = 0;
        } else if action == action_id("filters-previous") {
            self.filter_page = self.filter_page.saturating_sub(1);
        } else if action == action_id("filters-next") {
            self.filter_page =
                (self.filter_page + 1).min(self.filter_pages(context).len().saturating_sub(1));
        } else if self.view == View::Filters {
            if action == action_id("clear") {
                self.clear_filters();
            } else if let Some(&filter) = self
                .filters()
                .iter()
                .find(|filter| action_id(filter.action()) == action)
            {
                self.filter = filter;
                self.view = View::FilterChoice;
                self.filter_page = 0;
            }
        } else if let Some(index) =
            self.filter
                .values(self)
                .iter()
                .enumerate()
                .find_map(|(index, _)| {
                    (action == action_id(&format!("filter-value-{index}"))).then_some(index)
                })
        {
            self.filter.set(self, index);
            self.view = View::Filters;
            self.filter_page = 0;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, KoboApp};
    use kobo_ui::{Chrome, DisplayMetrics, TextScale};

    #[test]
    fn named_choices_apply_exact_values_and_any_stays_first() {
        let mut app = Grimoire {
            view: View::Compendium,
            ..Grimoire::default()
        };
        let mut context = Context::default();
        for key in ["class", "school"] {
            let values = options(&app, key);
            assert_eq!(values[0], "Any");
            assert!(values.iter().all(|value| !value.contains(',')));
        }
        app.on_action(&mut context, action_id("filters"));
        app.on_action(&mut context, action_id("spell-class"));
        assert_eq!(app.view, View::FilterChoice);
        let wizard = Filter::Class
            .values(&app)
            .iter()
            .position(|value| value == "Wizard")
            .expect("Wizard");
        app.on_action(&mut context, action_id(&format!("filter-value-{wizard}")));
        assert_eq!(app.view, View::Filters);
        assert_eq!(app.spell_class, wizard);
        assert!(!app.entries().is_empty());
        assert!(app
            .entries()
            .iter()
            .all(|(_, entry)| super::super::tag(entry, "class")
                .is_some_and(|value| value.split(',').any(|item| item.trim() == "Wizard"))));
        app.on_action(&mut context, action_id("spell-school"));
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.view, View::Filters);
        app.on_action(&mut context, action_id("clear"));
        assert_eq!(app.spell_class, 0);
        app.on_action(&mut context, action_id("filter-results"));
        assert_eq!(app.view, View::Compendium);
    }

    #[test]
    fn every_filter_choice_is_reachable_at_all_text_sizes() {
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
        {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let context = AppRunner::with_metrics(Grimoire::default(), metrics).context();
                for kind in [Kind::Spell, Kind::Monster] {
                    let mut app = Grimoire {
                        kind,
                        view: View::Filters,
                        ..Grimoire::default()
                    };
                    let filters = app.filters().to_vec();
                    for choice in std::iter::once(None).chain(filters.into_iter().map(Some)) {
                        app.view = if choice.is_some() {
                            View::FilterChoice
                        } else {
                            View::Filters
                        };
                        if let Some(filter) = choice {
                            app.filter = filter;
                        }
                        let rows = app.filter_rows();
                        let pages = app.filter_pages(&context);
                        assert_eq!(
                            pages.iter().flatten().copied().collect::<Vec<_>>(),
                            (0..rows.len()).collect::<Vec<_>>()
                        );
                        for (page, indices) in pages.iter().enumerate() {
                            app.filter_page = page;
                            let diagnostics = app
                                .filter_screen(&context)
                                .diagnostics(&metrics, &Chrome::measuring(true));
                            assert!(
                                diagnostics.issues.is_empty(),
                                "{metrics:?}: {:?}",
                                diagnostics.issues
                            );
                            for &index in indices {
                                assert!(diagnostics
                                    .layout
                                    .rect_of_action(action_id(&rows[index].0))
                                    .is_some());
                            }
                        }
                    }
                }
            }
        }
    }
}
