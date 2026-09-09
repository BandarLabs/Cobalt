//! Retained screen construction. Geometry, diagnostics and rendering remain
//! owned by kobo-ui; the SDK translates app intent into that shared contract.
use crate::{
    action_id, stable_id, ActionId, AppMetadata, BandAlign, BandSlot, BannerLevel, BarAction,
    BottomAction, Caret, Cell, Chip, ControlState, DiagnosticSeverity, DialogAction, Emphasis,
    Failure, Fold, FontHandle, Freeform, Glyph, InlineFormula, LayoutIssue, LayoutIssueKind,
    NavBar, Node, NodeId, Overlay, OverlayKind, ParagraphPresentation, Percent, QuoteRole,
    RichTextSpan, Row, RowLead, Screen, ScreenBuilder, SlotWidth, Space, StandardState, Tile,
    TilePicture, TileShape, TopBar, TransferFailure, JOIN_WIFI, MAX_BAND_SLOTS, MAX_CELLS,
    MAX_CHOICE_OPTIONS, MAX_COLUMNS, MAX_QUOTE_DEPTH, MAX_ROWS, MAX_TERMINAL_ROWS,
};

impl ScreenBuilder {
    #[must_use]
    pub fn new(name: impl AsRef<str>) -> Self {
        Self {
            id: stable_id(name.as_ref()),
            next_node: 1,
            top_bar: None,
            nodes: Vec::new(),
            nav_bar: None,
            bottom_action: None,
            page_turns: None,
            hold: None,
            owns_back: false,
            text_scale: None,
            overlay: None,
            reading: false,
            reading_font: None,
            actions: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[must_use]
    pub fn heading(self, text: impl Into<String>) -> Self {
        self.heading_at_level(1, text)
    }

    /// A heading at a given depth in a document's hierarchy, counting from
    /// one.
    ///
    /// A screen has one heading and calls [`Self::heading`]. This is for
    /// prose that carries real structure -- a book, a paper -- where setting
    /// every level as display type gives a page several titles and no
    /// hierarchy.
    #[must_use]
    pub fn heading_at_level(mut self, level: u8, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Heading {
            id,
            text: text.into(),
            level: level.max(1),
        });
        self
    }

    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Text {
            id,
            text: text.into(),
            links: Vec::new(),
        });
        self
    }

    /// Adds publisher-styled book prose without exposing arbitrary geometry.
    #[must_use]
    pub fn rich_text(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::RichText {
            id,
            text: text.into(),
            spans,
            links: Vec::new(),
            presentation,
            selection: None,
            formulae: Vec::new(),
        });
        self
    }

    /// Publisher-styled prose with tappable inline destinations.
    #[must_use]
    pub fn rich_text_linking<I, N>(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
        links: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let links = links
            .into_iter()
            .take(kobo_ui::MAX_TEXT_LINKS)
            .filter_map(|(name, start, end)| {
                (start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end))
                .then(|| kobo_ui::TextLink {
                    action: self.register(name.as_ref()),
                    start,
                    end,
                })
            })
            .collect();
        self.nodes.push(Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection: None,
            formulae: Vec::new(),
        });
        self
    }

    /// Publisher-styled reading prose whose words can be resolved on a hold.
    #[must_use]
    pub fn selectable_rich_text_linking<I, N>(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
        context: u64,
        offset: u32,
        links: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let links = links
            .into_iter()
            .take(kobo_ui::MAX_TEXT_LINKS)
            .filter_map(|(name, start, end)| {
                (start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end))
                .then(|| kobo_ui::TextLink {
                    action: self.register(name.as_ref()),
                    start,
                    end,
                })
            })
            .collect();
        self.nodes.push(Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection: Some(kobo_ui::TextSelection { context, offset }),
            formulae: Vec::new(),
        });
        self
    }

    /// Sets typeset formulas into the paragraph just added.
    ///
    /// Separate from the calls that add the paragraph because mathematics is
    /// rare and those calls already take everything a paragraph normally has.
    /// Each formula names a picture the application has handed over and the
    /// half-open range of the paragraph's own bytes it is drawn over -- the
    /// written form of the formula, which stays in the text so that a search
    /// still finds it and a reader without the picture still reads it.
    ///
    /// Does nothing if the last thing added was not a paragraph, or if a
    /// range does not land on a character boundary of it.
    #[must_use]
    pub fn with_formulae(mut self, formulae: impl IntoIterator<Item = InlineFormula>) -> Self {
        let Some(Node::RichText {
            text, formulae: on, ..
        }) = self.nodes.last_mut()
        else {
            return self;
        };
        for formula in formulae.into_iter().take(kobo_ui::MAX_INLINE_FORMULAE) {
            if formula.start < formula.end
                && formula.end <= text.len()
                && text.is_char_boundary(formula.start)
                && text.is_char_boundary(formula.end)
                && on
                    .last()
                    .is_none_or(|last: &InlineFormula| last.end <= formula.start)
            {
                on.push(formula);
            }
        }
        self
    }

    /// A paragraph with runs inside it that go somewhere.
    ///
    /// Each link is an action name and the half-open range of the paragraph's
    /// own bytes that names it. Ranges rather than the words themselves,
    /// because a paragraph often says the same words twice and only one of
    /// them is the link; a caller that has the words rather than the offsets
    /// should use `str::find` on the paragraph it is about to pass in, and
    /// leave out anything it cannot locate.
    ///
    /// A range outside the text, or landing inside a character, is dropped
    /// rather than drawn somewhere approximate: a link in the wrong place is
    /// worse than a link that is only in the list.
    #[must_use]
    pub fn text_linking<I, N>(mut self, text: impl Into<String>, links: I) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let mut runs = Vec::new();
        let mut source = links.into_iter();
        for (name, start, end) in source.by_ref().take(kobo_ui::MAX_TEXT_LINKS) {
            if start >= end
                || end > text.len()
                || !text.is_char_boundary(start)
                || !text.is_char_boundary(end)
            {
                continue;
            }
            runs.push(kobo_ui::TextLink {
                action: self.register(name.as_ref()),
                start,
                end,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "text links", kobo_ui::MAX_TEXT_LINKS);
        }
        self.nodes.push(Node::Text {
            id,
            text,
            links: runs,
        });
        self
    }

    /// Adds a line about the content rather than the content itself.
    ///
    /// A date, an author, a size, a count, a status. Set smaller and lighter
    /// than body text, which is what lets a list be read by scanning titles.
    /// Use it for anything that would otherwise be a parenthetical.
    #[must_use]
    pub fn secondary(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Secondary {
            id,
            text: text.into(),
        });
        self
    }

    /// Names the group of blocks that follows it.
    ///
    /// The organising primitive. [`Self::heading`] is display type belonging to
    /// the *screen*, so using it for a group gives a screen four titles and no
    /// hierarchy; a section is quieter than the heading on purpose and never
    /// competes with it. Every application was building this out of a spacer, a
    /// divider and a line of prose, and getting a slightly different answer.
    ///
    /// The words are used as they are given. Setting a section in capitals is a
    /// house style that breaks on scripts with no case at all, so if capitals
    /// are wanted, supply them.
    #[must_use]
    pub fn section(mut self, title: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Section {
            id,
            title: title.into(),
            value: None,
            link: None,
        });
        self
    }

    /// The same, with a count or a total against the right margin.
    ///
    /// The value is measured first and the title clamped against what is left,
    /// so a long name gives up its own hairline rather than pushing the total
    /// off the panel.
    #[must_use]
    pub fn section_with_value(
        mut self,
        title: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Section {
            id,
            title: title.into(),
            value: Some(value.into()),
            link: None,
        });
        self
    }

    /// Adds a caption-sized trailing destination to the most recent section.
    ///
    /// This deliberately searches backwards so it can follow `section_rows`:
    /// rows remain the content introduced by the section, not an obstacle to
    /// giving that section a "View all" destination.
    #[must_use]
    pub fn section_link(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Section { link, .. }) = self
            .nodes
            .iter_mut()
            .rev()
            .find(|node| matches!(node, Node::Section { .. }))
        {
            *link = Some(BarAction::new(action, label));
        }
        self
    }

    /// Sets a block of labelled facts about the thing on the screen.
    ///
    /// The answer to a detail screen with a dozen things to say and only
    /// [`Self::secondary`] to say them with, which stacks a dozen grey
    /// paragraphs and reads as a page that failed to finish loading.
    ///
    /// Labels share one column measured across every entry at once, so the
    /// values line up; the column is capped so one long label cannot squeeze
    /// every value into a gutter. Entries past `MAX_FACTS` are dropped and
    /// reported by [`Screen::validate`], rather than silently set and clipped.
    #[must_use]
    pub fn facts<I, K, V>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let id = self.next_id();
        let entries = entries
            .into_iter()
            .map(|(label, value)| (label.into(), value.into()))
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            self.nodes.push(Node::Facts { id, entries });
        }
        self
    }

    /// Places two or three columns beside each other.
    ///
    /// The one escape from the downward flow, and deliberately a small one.
    /// Each slot is built with the same builder the screen uses, so ids and
    /// action names carry straight on: a control inside a band is named and
    /// read exactly like a control anywhere else.
    ///
    /// Slots past [`MAX_BAND_SLOTS`] are dropped. When the panel cannot give
    /// every slot a readable width the band stacks itself, so this is always
    /// safe to reach for -- there is no narrow device on which it produces a
    /// column four characters wide.
    ///
    /// ```ignore
    /// screen.band(BandAlign::Top, [
    ///     (SlotWidth::Fixed(300), |slot| slot.picture(cover, 30)),
    ///     (SlotWidth::Fill, |slot| {
    ///         slot.heading(&book.title).secondary(&book.author)
    ///     }),
    /// ])
    /// ```
    #[must_use]
    pub fn band<I, F>(mut self, align: BandAlign, slots: I) -> Self
    where
        I: IntoIterator<Item = (SlotWidth, F)>,
        F: FnOnce(Self) -> Self,
    {
        let id = self.next_id();
        let outer = std::mem::take(&mut self.nodes);
        let mut built = Vec::new();
        let mut done = self;
        for (width, build) in slots.into_iter().take(MAX_BAND_SLOTS) {
            done = build(done);
            let nodes = std::mem::take(&mut done.nodes);
            built.push(BandSlot::new(width, nodes));
        }
        done.nodes = outer;
        if !built.is_empty() {
            done.nodes.push(Node::Band {
                id,
                align,
                slots: built,
            });
        }
        done
    }

    /// Runs a reusable piece of screen without breaking the builder chain.
    ///
    /// Composites are already expressible as plain `fn(ScreenBuilder) ->
    /// ScreenBuilder` functions, and several applications write them, but
    /// calling one meant stopping mid-chain and naming a temporary. This is
    /// the same thing the overlay and band closures do, exposed so anything
    /// can be factored out and reused rather than copied.
    #[must_use]
    pub fn compose(self, build: impl FnOnce(Self) -> Self) -> Self {
        build(self)
    }

    /// Puts a picture beside what it is a picture of.
    ///
    /// The masthead of a details page: a cover on the leading edge, and title,
    /// author and a few facts stacked beside it. There is deliberately no
    /// `Node::Hero` behind this. A hero is a picture next to a column, which
    /// is exactly what [`Self::band`] already is, and neither `SwiftUI` nor
    /// Compose ships a hero primitive either -- both compose one out of a
    /// stack. Adding a node would have meant a layout arm, a draw arm, a
    /// validate arm, three protocol arms and a roundtrip fixture for a screen
    /// that can already be written.
    ///
    /// The picture slot is a physical width, so the cover is the same size on
    /// a Clara as on a Sage. When the panel is too narrow to keep both slots
    /// readable the band stacks them on its own, which is why `width_mm` is
    /// the only measurement here and there is no breakpoint to get wrong.
    ///
    /// `picture` may be `None` -- a catalogue is full of books whose cover has
    /// not arrived, or has arrived and failed to decode -- in which case the
    /// metadata simply takes the whole width rather than sitting beside a
    /// grey rectangle apologising for itself.
    #[must_use]
    pub fn hero<I, K, V>(
        self,
        picture: Option<TilePicture>,
        width_mm: u16,
        title: impl Into<String>,
        subtitle: Option<String>,
        facts: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let title = title.into();
        let facts = facts
            .into_iter()
            .map(|(label, value)| (label.into(), value.into()))
            .collect::<Vec<_>>();
        let metadata = move |builder: Self| {
            let builder = builder.heading(title);
            let builder = match subtitle {
                Some(subtitle) => builder.secondary(subtitle),
                None => builder,
            };
            builder.facts(facts)
        };
        let Some(picture) = picture else {
            return self.compose(metadata);
        };
        self.band(
            BandAlign::Top,
            vec![
                (
                    SlotWidth::Fixed(width_mm.saturating_mul(10)),
                    // Twice the slot width as a height ceiling, so the fixed
                    // width is what actually decides the size of an ordinary
                    // portrait cover while a freak panorama is still stopped
                    // from taking the whole panel.
                    Box::new(move |builder: Self| {
                        builder.picture(picture, width_mm.saturating_mul(2))
                    }) as Box<dyn FnOnce(Self) -> Self>,
                ),
                (SlotWidth::Fill, Box::new(metadata)),
            ],
        )
    }

    /// Asks a question that has to be answered before anything else happens.
    ///
    /// A modal rather than a popover, deliberately: an outside tap does not
    /// close this one, because "did you mean to delete it" answered by
    /// accidentally brushing the panel is not an answer. The affirmative is
    /// the filled control and comes first, the way out is plain and second.
    ///
    /// Every application that deletes, unfollows or overwrites something was
    /// about to build this by hand out of `modal` plus two buttons, and they
    /// would have disagreed about which one was filled.
    #[must_use]
    pub fn confirm(
        self,
        title: impl Into<String>,
        question: impl Into<String>,
        confirm: (impl AsRef<str>, impl Into<String>),
        cancel: (impl AsRef<str>, impl Into<String>),
    ) -> Self {
        let question = question.into();
        let (confirm_name, confirm_label) = (confirm.0.as_ref().to_owned(), confirm.1.into());
        let (cancel_name, cancel_label) = (cancel.0.as_ref().to_owned(), cancel.1.into());
        self.modal(title, move |builder| {
            builder
                .text(question)
                .primary_button(confirm_name, confirm_label)
                .button(cancel_name, cancel_label)
        })
    }

    /// A labelled group of rows, kept together on the page.
    ///
    /// The commonest shape in the whole example set and the one nine
    /// applications each wrote out longhand: a heading that names what follows,
    /// optionally a count beside it, and then the rows. Written as one call so
    /// the header and its rows are always the same distance apart, and so the
    /// paginator is given them as one thing to place rather than two it may
    /// separate.
    #[must_use]
    pub fn section_rows<I, N, T, S, L>(
        self,
        title: impl Into<String>,
        value: Option<String>,
        rows: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
    {
        let builder = match value {
            Some(value) => self.section_with_value(title, value),
            None => self.section(title),
        };
        builder.rows(rows)
    }

    /// Adds a consistent empty, offline, denied, or error presentation.
    ///
    /// Chain [`Self::button`] when the condition has a recovery action. The
    /// state itself owns no action so an empty collection is never forced to
    /// pretend it can be fixed.
    ///
    /// Set as a splash rather than a heading and a paragraph, because a
    /// heading and a paragraph flow from the top and leave a thousand pixels
    /// of white beneath them: correct for reading, wrong for a page with six
    /// words on it. The splash centres itself in the room that is left after
    /// whatever is chained on, so a recovery button still lands under it.
    ///
    /// No banner. This used to raise one as well, which put two reports of one
    /// event on the same empty page, the banner being the vaguer of the two:
    /// "Access is not available" in a grey strip above "Permission needed" set
    /// large in the middle. A banner is for a failure that has to sit over
    /// content the reader is already looking at, and this is the case where
    /// there is none.
    #[must_use]
    pub fn standard_state(self, state: StandardState, message: impl Into<String>) -> Self {
        self.splash(Some(state.glyph()), state.title(), message)
    }

    #[must_use]
    pub fn empty_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Empty, message)
    }

    #[must_use]
    pub fn offline_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Offline, message)
    }

    #[must_use]
    pub fn permission_denied_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::PermissionDenied, message)
    }

    #[must_use]
    pub fn error_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Error, message)
    }

    /// A failed task's whole-screen presentation, with the way out of it.
    ///
    /// Chain this instead of writing [`Self::standard_state`] and a recovery
    /// button by hand, so that every application recovers from a failure the
    /// same way and gains a new route the day the SDK does.
    ///
    /// Being offline is the one failure the reader can fix on the device, and
    /// the only one that gets a second control: [`JOIN_WIFI`], which
    /// [`crate::AppRunner`] answers by opening Settings on the Wi-Fi screen. The two
    /// controls sit side by side because they are alternatives, and joining is
    /// the primary of the pair because retrying a request on a reader with no
    /// network will fail the same way it just did.
    ///
    /// A failure that is not retryable gets no control at all, rather than a
    /// Try again that is known in advance to fail.
    #[must_use]
    pub fn failure_state(self, failure: Failure, retry: impl AsRef<str>) -> Self {
        let screen = self.standard_state(failure.state, failure.advice);
        match (failure.state, failure.retryable) {
            (StandardState::Offline, _) => {
                screen.buttons([(JOIN_WIFI, "Join Wi-Fi"), (retry.as_ref(), "Try again")])
            }
            (_, true) => screen.primary_button(retry, "Try again"),
            (_, false) => screen,
        }
    }

    /// Builds a sparse, full-screen confirmation using standard controls.
    ///
    /// Kobo applications do not open floating windows: the display is a
    /// single retained page, so confirmations replace the page and use the
    /// application's typed navigator to return.
    #[must_use]
    pub fn confirmation(
        self,
        title: impl Into<String>,
        message: impl Into<String>,
        primary: DialogAction,
        secondary: DialogAction,
    ) -> Self {
        let DialogAction {
            name: primary_name,
            label: primary_label,
            state: primary_state,
        } = primary;
        let DialogAction {
            name: secondary_name,
            label: secondary_label,
            state: secondary_state,
        } = secondary;
        self.heading(title)
            .text(message)
            .divider()
            .button_with_state(primary_name, primary_label, primary_state)
            .button_with_state(secondary_name, secondary_label, secondary_state)
    }

    /// A paragraph set in from the left by `depth` levels, with a rule beside
    /// it, for a reply that answers what came before it.
    ///
    /// Depth is clamped to [`MAX_QUOTE_DEPTH`], so a thread that nests forty
    /// deep still reads: the deepest replies share an indent and say how deep
    /// they really are in their own words.
    #[must_use]
    pub fn quote(self, depth: u8, text: impl Into<String>) -> Self {
        self.quote_as(depth, QuoteRole::Body, text)
    }

    /// The line above a reply that says who wrote it and when.
    ///
    /// Set as metadata rather than as prose: a byline drawn at body size in
    /// body ink reads as the comment's opening sentence, which is what a real
    /// thread on a real panel looked like before this existed.
    #[must_use]
    pub fn byline(self, depth: u8, text: impl Into<String>) -> Self {
        self.quote_as(depth, QuoteRole::Byline, text)
    }

    /// A byline that folds away everything underneath it.
    ///
    /// `name` is the action sent when it is tapped; `hidden` is how many
    /// replies are behind it, which is drawn only while it is shut. What
    /// folding *does* is the application's business -- the renderer only sends
    /// the tap -- because only the application knows where the subtree ends.
    #[must_use]
    pub fn folding_byline(
        self,
        depth: u8,
        text: impl Into<String>,
        name: impl AsRef<str>,
        collapsed: bool,
        hidden: u16,
    ) -> Self {
        let action = action_id(name.as_ref());
        self.quote_full(
            depth,
            QuoteRole::Byline,
            text,
            Some(Fold {
                action,
                collapsed,
                hidden,
            }),
        )
    }

    /// A paragraph of a thread, saying what it is for.
    #[must_use]
    pub fn quote_as(self, depth: u8, role: QuoteRole, text: impl Into<String>) -> Self {
        self.quote_full(depth, role, text, None)
    }

    fn quote_full(
        mut self,
        depth: u8,
        role: QuoteRole,
        text: impl Into<String>,
        fold: Option<Fold>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Quote {
            id,
            depth: depth.min(MAX_QUOTE_DEPTH),
            role,
            text: text.into(),
            fold,
        });
        self
    }

    #[must_use]
    pub fn button(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        self.button_with_state(name, label, ControlState::Enabled)
    }

    /// Adds the one control the screen exists for, drawn filled.
    ///
    /// At most one per screen. A fill is the loudest mark this panel can make
    /// and the slowest to clear, so spending it on every control (which is
    /// what the platform used to do) leaves the reader with nothing to aim at
    /// and the panel with a slab to erase.
    #[must_use]
    pub fn primary_button(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        self.primary_button_with_state(name, label, ControlState::Enabled)
    }

    /// Adds the primary control with an explicit enabled state.
    ///
    /// Emphasis stays Primary so the control keeps its size while it cannot
    /// be activated, instead of collapsing to a content-width secondary.
    #[must_use]
    pub fn primary_button_with_state(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        state: ControlState,
    ) -> Self {
        let action = self.register(name.as_ref());
        let id = self.next_id();
        self.nodes.push(Node::Button {
            id,
            action,
            label: label.into(),
            state,
            emphasis: Emphasis::Primary,
        });
        self
    }

    /// Adds a button that is visible but cannot currently be activated.
    #[must_use]
    pub fn disabled_button(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        self.button_with_state(name, label, ControlState::Disabled)
    }

    /// Puts two or three secondary actions on one line.
    ///
    /// Stacked, each of them takes the full width of the panel to say one
    /// word, and a screen that ends in three of those reads as a form rather
    /// than as a page with some things you can do to it. Side by side they are
    /// as wide as they need to be and the reader can see at a glance that they
    /// belong together. Both platforms do this: a `UIStackView` of secondary
    /// buttons on iOS, a `Row` of `OutlinedButton`s on Android.
    ///
    /// This is [`ScreenBuilder::band`] with a slot per action, not a new kind
    /// of thing, so a narrow panel still stacks them by itself rather than
    /// squeezing three words into a third of a screen each.
    ///
    /// Anything past the third is dropped, which is what a band does anyway. A
    /// row of four controls is a menu, and the overflow menu is what that is
    /// for.
    #[must_use]
    pub fn buttons<I, N, L>(self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let mut actions = actions
            .into_iter()
            .take(MAX_BAND_SLOTS)
            .map(|(name, label)| (name.as_ref().to_owned(), label.into()));
        match (actions.next(), actions.next()) {
            (None, _) => self,
            // One action side by side with nothing is a button, and going
            // through a band would only cost a node and read the same.
            (Some((name, label)), None) => self.button(name, label),
            (Some(first), Some(second)) => self.band(
                BandAlign::Middle,
                [first, second].into_iter().chain(actions).map(
                    |(name, label): (String, String)| {
                        (SlotWidth::Fill, move |slot: Self| slot.button(name, label))
                    },
                ),
            ),
        }
    }

    /// Adds a button with explicit semantic enabled state.
    #[must_use]
    pub fn button_with_state(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        state: ControlState,
    ) -> Self {
        let action = self.register(name.as_ref());
        let id = self.next_id();
        self.nodes.push(Node::Button {
            id,
            action,
            label: label.into(),
            state,
            emphasis: Emphasis::Normal,
        });
        self
    }

    #[must_use]
    pub fn divider(mut self) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Divider { id });
        self
    }

    /// Adds vertical space from the design scale.
    ///
    /// There is deliberately no pixel argument. Authors choose an intent and
    /// the renderer decides what that measures on the panel in front of it.
    #[must_use]
    pub fn spacer(mut self, space: Space) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Spacer { id, space });
        self
    }

    /// Pushes everything after it to the foot of the panel.
    ///
    /// The keyboard is what this is for. It is the tallest thing a screen
    /// draws and it belongs under the thumbs, but it is placed in flow like
    /// every other node, so a compose screen with a prompt and a line of typed
    /// text put the keys across the middle of the panel with a third of a page
    /// of paper underneath them.
    ///
    /// It only ever pushes down. A screen that is already full is laid out
    /// exactly as it was.
    #[must_use]
    pub fn fill(mut self) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Flex { id });
        self
    }

    /// Adds a progress bar. Values above a hundred are clamped rather than
    /// rejected, because that is a caller mistake and not a reason to fail.
    #[must_use]
    pub fn progress(mut self, value: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Progress {
            id,
            value: Percent::new(value),
        });
        self
    }

    #[must_use]
    pub fn paged_list<I, S>(mut self, page: u16, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let id = self.next_id();
        self.nodes.push(Node::PagedList {
            id,
            page,
            items: items.into_iter().map(Into::into).collect(),
        });
        self
    }

    #[must_use]
    pub fn action(&self, name: &str) -> Option<ActionId> {
        self.actions
            .iter()
            .find_map(|(known, id)| (known == name).then_some(*id))
    }

    /// Asks for the reader's Back to arrive as an action first.
    ///
    /// The Back control belongs to the runtime and always leads out of the
    /// application in the end; this only asks for first refusal, so a screen
    /// reached from inside the application can return to where it was reached
    /// from instead of dropping the reader at the launcher. Pass
    /// [`crate::Navigator::can_go_back`] and the behaviour follows the back stack for
    /// free: deep screens pop, the root leaves.
    ///
    /// The offer expires. An application that sets this and then draws nothing
    /// in answer to [`ActionId::BACK`] is left behind and the launcher appears
    /// anyway, which is why setting it can never strand a reader.
    #[must_use]
    pub const fn owns_back(mut self, owns_back: bool) -> Self {
        self.owns_back = owns_back;
        self
    }

    /// Asks for a text size other than the reader's own.
    ///
    /// Almost no screen should. The scale is an accessibility preference and
    /// overriding it overrules someone who has already said how large they
    /// need type to be. The case it exists for is a reader, where the size of
    /// the body text is the thing being adjusted and the adjustment belongs to
    /// the book.
    ///
    /// Paginate with [`crate::Context::metrics_at`] using the same scale. Measuring at
    /// one size and drawing at another loses the end of every page.
    #[must_use]
    pub const fn text_scale(mut self, scale: kobo_ui::TextScale) -> Self {
        self.text_scale = Some(scale);
        self
    }

    /// Says this screen's text is a book rather than an interface.
    ///
    /// Sets prose in a serif drawn for continuous reading (the device's own
    /// reading face where it has one) and opens the lines to the measure books
    /// have always used. For the pages of a reader and nothing else: the
    /// interface face is chosen so a label glanced at once cannot be misread,
    /// which is a different problem with a different answer.
    ///
    /// Paginate with [`crate::Context::paginate_reading`], because a serif sets the
    /// same words wider and a page measured in the wrong face loses its last
    /// lines.
    #[must_use]
    pub const fn reading(mut self, reading: bool) -> Self {
        self.reading = reading;
        self
    }

    /// Uses a publisher font previously handed to the runtime for book prose.
    #[must_use]
    pub const fn reading_font(mut self, font: FontHandle) -> Self {
        self.reading_font = Some(font);
        self
    }

    /// Hangs a popover off the control named `anchor`.
    ///
    /// The closure builds the overlay's contents with the same builder the
    /// screen uses, so ids and action names carry on from where the screen
    /// left off: a control inside a popover is named and read exactly like a
    /// control on the screen, and nothing has to be told which is which.
    #[must_use]
    pub fn popover(self, anchor: impl AsRef<str>, build: impl FnOnce(Self) -> Self) -> Self {
        let anchor = action_id(anchor.as_ref());
        self.overlay_with(OverlayKind::Popover { anchor }, String::new(), build)
    }

    /// Puts a question over the screen that has to be answered.
    #[must_use]
    pub fn modal(self, title: impl Into<String>, build: impl FnOnce(Self) -> Self) -> Self {
        self.overlay_with(OverlayKind::Modal, title.into(), build)
    }

    fn overlay_with(
        mut self,
        kind: OverlayKind,
        title: String,
        build: impl FnOnce(Self) -> Self,
    ) -> Self {
        // The screen's nodes are set aside so the closure builds into an empty
        // list, then put back. Threading the builder through rather than
        // handing the closure a fresh one is what keeps one id counter and one
        // action table for the whole screen.
        let outer = std::mem::take(&mut self.nodes);
        let id = self.next_id();
        let mut done = build(self);
        let nodes = std::mem::replace(&mut done.nodes, outer);
        done.overlay = Some(Box::new(Overlay {
            id,
            kind,
            title,
            nodes,
        }));
        done
    }

    /// Adds the fixed top bar.
    ///
    /// Calling this twice replaces the bar rather than adding a second one. A
    /// screen has at most one, which is a property of the type rather than a
    /// rule the author has to follow.
    #[must_use]
    pub fn top_bar(mut self, title: impl Into<String>) -> Self {
        let id = self.next_id();
        self.top_bar = Some(TopBar::new(id, title));
        self
    }

    /// Adds an action to the top bar, right to left.
    ///
    /// At most two; see `kobo_ui::MAX_BAR_ACTIONS`. A no-op if there is no top
    /// bar, because an action with nowhere to live is an author mistake that
    /// should not silently become a floating button.
    #[must_use]
    pub fn top_bar_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(top_bar) = self.top_bar.take() {
            self.top_bar = Some(top_bar.with_action(BarAction::new(action, label)));
        }
        self
    }

    /// The same, drawn as one of the built-in icons.
    ///
    /// For a control whose meaning has a picture everyone already knows: the
    /// front light, a search. The label is still required, because it is what
    /// the control is called everywhere that is not the panel -- a preview, a
    /// test, a log -- and a mark with no word anywhere near it is a puzzle.
    #[must_use]
    pub fn top_bar_glyph(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        glyph: kobo_ui::Glyph,
    ) -> Self {
        let action = self.register(name.as_ref());
        if let Some(top_bar) = self.top_bar.take() {
            self.top_bar =
                Some(top_bar.with_action(BarAction::new(action, label).with_glyph(glyph)));
        }
        self
    }

    /// Puts the rest of this screen's verbs under three dots in the top bar.
    ///
    /// The answer to a top bar with more than [`kobo_ui::MAX_BAR_ACTIONS`]
    /// things to offer: the bar does not grow, the third verb goes under the
    /// dots. Nine applications were each about to rebuild this out of
    /// `top_bar_glyph` plus `popover` plus a column of buttons, and they would
    /// not have agreed on the glyph, the order or the dismissal.
    ///
    /// `open` is the application's, the way `expanded` is the application's in
    /// Compose's `DropdownMenu`: whether the menu is showing is a fact about
    /// the screen, and a screen is drawn from state here rather than mutated.
    /// The dots are drawn either way, so the bar does not jump when the menu
    /// opens.
    ///
    /// Closing it is not the application's. The popover draws a caret pointing
    /// at the control it came out of, and a tap anywhere outside it arrives as
    /// `ActionId::BACK`, because the scrim a popover puts down reports a miss.
    /// All the application does with that is set `open` back to false.
    ///
    /// A no-op with no items, rather than three dots that open onto nothing.
    #[must_use]
    pub fn top_bar_overflow<I, N, L>(self, name: impl AsRef<str>, open: bool, items: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let items = items
            .into_iter()
            .map(|(name, label)| (name.as_ref().to_owned(), label.into()))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return self;
        }
        let name = name.as_ref().to_owned();
        let screen = self.top_bar_glyph(&name, "More", kobo_ui::Glyph::More);
        if !open {
            return screen;
        }
        screen.popover(&name, move |builder| {
            items.into_iter().fold(builder, |builder, (name, label)| {
                builder.button(name, label)
            })
        })
    }

    /// The menu behind a row's overflow mark.
    ///
    /// The companion to [`Self::rows_with_menu`], and the same shape as
    /// [`Self::top_bar_overflow`]: pass the mark's name, whether it is open,
    /// and what it offers. `open` is a property of the application's state
    /// rather than something this remembers, for the reason every overlay in
    /// this SDK works that way -- the tap that closes a popover arrives as
    /// `ActionId::BACK` from the scrim, and an application that has to notice
    /// that tap itself is an application that sometimes forgets.
    ///
    /// One caution the bar's version does not need: pass `open` as false when
    /// the row is not on the current page. A popover anchored to a control
    /// that is not drawn has nothing to point at.
    /// Each item is a name, a word and a mark, and is drawn as a row rather
    /// than a button. A menu is a list of things to do to one entry, and a
    /// stack of full-width outlined buttons reads as a form; a row also gives
    /// the mark somewhere to stand, which is what lets a destructive item say
    /// "Delete" beside a bin instead of spelling the whole verb out.
    #[must_use]
    pub fn row_overflow<I, N, L>(self, anchor: impl AsRef<str>, open: bool, items: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        if !open {
            return self;
        }
        let items = items
            .into_iter()
            .map(|(name, label, glyph)| (name.as_ref().to_owned(), label.into(), glyph))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return self;
        }
        self.popover(anchor.as_ref(), move |builder| {
            builder.rows(
                items
                    .into_iter()
                    .map(|(name, label, glyph)| (name, label, String::new(), glyph)),
            )
        })
    }

    /// Adds the fixed bottom bar.
    ///
    /// Note there is no back destination to add: back belongs to the runtime's
    /// navigation stack, so it appears automatically wherever there is
    /// somewhere to go back to and cannot be omitted by an application.
    /// Turns the sides of the content area into page turns.
    ///
    /// This is how every Kobo has worked since the first one: tap the left of
    /// the page to go back, anywhere else to go on. Actions are named, like
    /// every other action, so the same two intents can later be raised by the
    /// physical page buttons some models have.
    ///
    /// Controls always win. A tap that lands on a button, a row or a keyboard
    /// key is that control's; the zones only ever collect taps that would
    /// otherwise have done nothing.
    #[must_use]
    pub fn page_turns(mut self, previous: impl AsRef<str>, next: impl AsRef<str>) -> Self {
        let previous = self.register(previous.as_ref());
        let next = self.register(next.as_ref());
        self.page_turns = Some(kobo_ui::PageTurns::new(previous, next));
        self
    }

    /// Says which page of how many the turns are moving through.
    ///
    /// `page` is one-based. Drawn centred at the foot of the content, muted,
    /// costing one caption line. Without it a paginated list gives the reader
    /// no way to tell a page turn from a list that did not move -- the
    /// catalogue cut its shelf into as many as fifty-four pages and said
    /// nothing about which one was showing.
    ///
    /// Has no effect unless [`Self::page_turns`] was asked for as well.
    #[must_use]
    pub fn page_position(mut self, page: u16, of: u16) -> Self {
        self.page_turns = self.page_turns.map(|turns| turns.with_position(page, of));
        self
    }

    /// Draws Folio's passive right-margin page rail.
    ///
    /// `page` is zero-based, matching application pagination vectors. The
    /// rail is display-only, so it is never a slider or a competing gesture.
    #[must_use]
    pub fn page_rail(mut self, page: u16, of: u16) -> Self {
        if of > 1 {
            let id = self.next_id();
            self.nodes.push(Node::PageRail { id, page, of });
        }
        self
    }

    /// Adds a middle column that asks for this screen's own controls.
    ///
    /// For a screen that carries nothing at the foot, which is every reading
    /// screen: without this there is no way to reach a setting with a finger,
    /// because the whole content area is spoken for by page turns. Left third
    /// back, middle third the controls, right third forward, which is what
    /// every other reader on this hardware does.
    ///
    /// Has no effect unless [`Self::page_turns`] was asked for as well, since
    /// the zones are one arrangement rather than three separate ones.
    #[must_use]
    pub fn reading_menu(mut self, menu: impl AsRef<str>) -> Self {
        let menu = self.register(menu.as_ref());
        self.page_turns = self.page_turns.map(|turns| turns.with_menu(menu));
        self
    }

    /// Sends an optional secondary `action` when a finger is held still on
    /// empty content.
    ///
    /// A hold is an accelerator, never the only way to reach navigation,
    /// accessibility, confirmation, destructive, or primary behavior. Keep a
    /// visible control or overflow entry for anything a reader must discover.
    /// Ordinary control taps remain immediate; holding a real control still
    /// activates that control rather than hiding it behind a gesture.
    #[must_use]
    pub fn hold(mut self, action: impl AsRef<str>) -> Self {
        self.hold = Some(self.register(action.as_ref()));
        self
    }

    /// Adds the fixed bar at the bottom of the screen.
    ///
    /// `selected` takes an index or `None`. `None` is for a bar whose entries
    /// are actions rather than places (page back, page forward, the way out)
    /// where marking any of them as current would tell the reader they are
    /// somewhere they are not.
    #[must_use]
    pub fn nav_bar<I, N, L, S>(mut self, selected: S, destinations: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
        S: Into<Option<usize>>,
    {
        let id = self.next_id();
        let destinations = destinations
            .into_iter()
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::new(id, destinations, selected.into()));
        self.bottom_action = None;
        self
    }

    /// Adds a destination bar whose labels keep their recognisable glyphs.
    #[must_use]
    pub fn nav_bar_marked<I, N, L, S>(mut self, selected: S, destinations: I) -> Self
    where
        I: IntoIterator<Item = (N, L, kobo_ui::Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
        S: Into<Option<usize>>,
    {
        let id = self.next_id();
        let destinations = destinations
            .into_iter()
            .map(|(name, label, glyph)| {
                BarAction::new(self.register(name.as_ref()), label).with_glyph(glyph)
            })
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::new(id, destinations, selected.into()));
        self.bottom_action = None;
        self
    }

    /// Pins the verbs belonging to this screen to the bottom band.
    ///
    /// The other half of [`Self::nav_bar`], and the reason that one should now
    /// always be given a selection. A nav bar names places, is the same on
    /// every screen of an application, and marks the one you are on. An action
    /// bar names things to do here, is free to change from screen to screen,
    /// and marks nothing -- because none of its entries is a place you could
    /// be standing.
    ///
    /// Android draws exactly this line between `NavigationBar` and
    /// `BottomAppBar`; iOS between a tab bar and a toolbar. Before this, three
    /// screens in the example set passed `None` to `nav_bar` to get a bar of
    /// verbs, which worked but meant nothing could tell a bar that had
    /// forgotten to say where the reader was from one that had nowhere to say.
    ///
    /// Two or three actions. A third is dropped on a panel too narrow to give
    /// all of them a finger's width, and anything past three belongs in an
    /// overflow menu.
    ///
    /// Mutually exclusive with [`Self::nav_bar`] and [`Self::bottom_action`]:
    /// they are all the same band.
    #[must_use]
    pub fn action_bar<I, N, L>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let actions = actions
            .into_iter()
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::actions(id, actions));
        self.bottom_action = None;
        self
    }

    /// The same, with a mark on each entry that has one.
    ///
    /// A bar slot is a third of a panel wide and a bar entry is a verb, so the
    /// ones with a picture everyone already knows should show it: a chevron
    /// for a page turn, a house for the way out. The mark is drawn above the
    /// word rather than instead of it, because this band is frequently the
    /// only way off a screen and is the last place to make somebody guess.
    #[must_use]
    pub fn action_bar_marked<I, N, L>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<kobo_ui::Glyph>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let actions = actions
            .into_iter()
            .map(|(name, label, glyph)| {
                let action = BarAction::new(self.register(name.as_ref()), label);
                match glyph {
                    Some(glyph) => action.with_glyph(glyph),
                    None => action,
                }
            })
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::actions(id, actions));
        self.bottom_action = None;
        self
    }

    /// Pins one control to the bottom of the panel, where a bar would go.
    ///
    /// For a screen with a single way off it. Prefer this to a button at the
    /// end of the flow whenever the control must always be reachable: layout
    /// reserves this band before it places any content, so nothing above can
    /// push the control off the panel, and a page that runs long loses its
    /// last line rather than the only way out. A trailing button reserves
    /// nothing, and the launcher shipped with its way back to the Kobo reader
    /// hanging over the bottom edge of the screen because of it.
    ///
    /// Mutually exclusive with [`Self::nav_bar`], they are the same band.
    #[must_use]
    pub fn bottom_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let id = self.next_id();
        let action = BarAction::new(self.register(name.as_ref()), label);
        self.warn_second_bottom_bar(id);
        self.bottom_action = Some(BottomAction::new(id, action));
        self.nav_bar = None;
        self
    }

    /// The same, with a mark beside the word.
    ///
    /// The mark sits next to the label rather than replacing it: one pinned
    /// control has the width for both, and this is the band a reader uses to
    /// leave.
    #[must_use]
    pub fn bottom_action_marked(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        glyph: kobo_ui::Glyph,
    ) -> Self {
        let id = self.next_id();
        let action = BarAction::new(self.register(name.as_ref()), label).with_glyph(glyph);
        self.warn_second_bottom_bar(id);
        self.bottom_action = Some(BottomAction::new(id, action));
        self.nav_bar = None;
        self
    }

    /// Adds a grid of tiles. Columns are chosen from the panel's physical
    /// width, so the author never picks a count that is wrong on some device.
    #[must_use]
    pub fn tiles<I, N, L>(mut self, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph)| Tile::new(self.register(name.as_ref()), label, glyph))
            .collect();
        self.nodes.push(Node::TileGrid {
            id,
            tiles,
            shape: TileShape::Square,
        });
        self
    }

    /// Adds a text field showing what is currently in it.
    ///
    /// Tapping yields `name`; route that to your own keyboard screen. The
    /// field does not summon a keyboard, because the runtime does not own one.
    /// What it does is show the query, which is the part a button could not do
    /// and the reason a search entry point used to be an unlabelled ellipsis
    /// in the top bar.
    #[must_use]
    pub fn field(
        mut self,
        name: impl AsRef<str>,
        value: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        let action = self.register(name.as_ref());
        self.nodes.push(Node::Field {
            id,
            action,
            value: value.into(),
            placeholder: placeholder.into(),
            clear: None,
        });
        self
    }

    /// Puts a cross in the field just added, to empty it.
    ///
    /// Does nothing if the last node is not a field, and nothing if that field
    /// is already empty: a cross beside an empty box is a control that cannot
    /// do anything, and one of those on every search screen teaches readers
    /// that controls on this platform are decorative.
    #[must_use]
    pub fn field_clear(mut self, name: impl AsRef<str>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Field { value, clear, .. }) = self.nodes.last_mut() {
            if !value.is_empty() {
                *clear = Some(action);
            }
        }
        self
    }

    /// Adds a wrapping run of short tappable labels.
    ///
    /// Subjects, facets, languages, recent searches. The renderer wraps them;
    /// you supply no geometry. Entries past [`crate::MAX_CHIPS`] are dropped and
    /// reported by `validate`.
    #[must_use]
    pub fn chips<I, N, L>(mut self, chips: I) -> Self
    where
        I: IntoIterator<Item = (N, L, bool)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let chips = chips
            .into_iter()
            .map(|(name, label, selected)| {
                Chip::new(self.register(name.as_ref()), label).selected(selected)
            })
            .collect();
        self.nodes.push(Node::Chips { id, chips });
        self
    }

    /// Adds up to [`crate::MAX_TABS`] peer views of the current screen.
    ///
    /// For filters on one destination. Destinations go in [`Self::nav_bar`],
    /// which is pinned to the bottom and says "you have gone somewhere else";
    /// a tab says "you are still here, looking at it differently".
    #[must_use]
    pub fn tabs<I, N, L>(mut self, selected: usize, tabs: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tabs = tabs
            .into_iter()
            .map(|(name, label)| Chip::new(self.register(name.as_ref()), label))
            .collect();
        self.nodes.push(Node::Tabs { id, tabs, selected });
        self
    }

    /// Adds a grid of tiles, each one configured by a closure.
    ///
    /// This is the general form, and the reason there will not be a fifth
    /// `*_tiles` method. [`Self::tiles`] and [`Self::picture_tiles`] each fixed
    /// one combination of a tile's optional parts into a tuple, so every part
    /// added afterwards would have needed a new method and a new arity. Here
    /// the tile arrives already registered and the closure says what else is
    /// true of it, exactly as a Compose slot or a `SwiftUI` modifier chain does:
    ///
    /// ```ignore
    /// screen.tile_grid(TileShape::Portrait, [
    ///     ("bleak-house", "Bleak House", Glyph::Book, |tile: Tile| {
    ///         tile.with_subtitle("Charles Dickens")
    ///             .with_state(TileState::Held)
    ///     }),
    /// ])
    /// ```
    ///
    /// A tile marked [`crate::TileState::Unavailable`] keeps its place in the grid and
    /// stops answering taps, which is the whole point: a shelf with a gap in it
    /// is a shelf that has lost its alignment.
    #[must_use]
    pub fn tile_grid<I, N, L, F>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, F)>,
        N: AsRef<str>,
        L: Into<String>,
        F: FnOnce(Tile) -> Tile,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, configure)| {
                configure(Tile::new(self.register(name.as_ref()), label, glyph))
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Adds tiles with an optional hold accelerator for a secondary menu.
    ///
    /// `menu` must also be exposed by a visible overflow, details, or section
    /// control. Holding is a convenience for experienced readers, never the
    /// only route to an action.
    #[must_use]
    pub fn contextual_tiles<I, N, L, M>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, M)>,
        N: AsRef<str>,
        L: Into<String>,
        M: AsRef<str>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, menu)| {
                let action = self.register(name.as_ref());
                let menu = self.register(menu.as_ref());
                Tile::new(action, label, glyph).with_menu(menu)
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Adds launcher tiles directly from application metadata.
    #[must_use]
    pub fn apps<I>(mut self, apps: I) -> Self
    where
        I: IntoIterator<Item = AppMetadata>,
    {
        let id = self.next_id();
        let tiles = apps
            .into_iter()
            .map(|app| app.tile(self.register(app.id)))
            .collect();
        self.nodes.push(Node::TileGrid {
            id,
            tiles,
            shape: TileShape::Square,
        });
        self
    }

    /// Adds a grid of tiles that may each carry a picture.
    ///
    /// Use [`TileShape::Portrait`] for covers and posters: a square cell
    /// letterboxes a book cover into roughly half its own area, which is what
    /// makes a shelf of covers look like a grid of stamps.
    ///
    /// A tile whose picture the runtime does not have falls back to its glyph,
    /// so a shelf is usable while the covers are still arriving.
    #[must_use]
    pub fn picture_tiles<I, N, L>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, Option<TilePicture>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, picture)| {
                let tile = Tile::new(self.register(name.as_ref()), label, glyph);
                match picture {
                    Some(picture) => tile.with_picture(picture),
                    None => tile,
                }
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Shows one picture, as large as the width and `max_height_mm` allow.
    ///
    /// The height is a physical measurement rather than a pixel count so that
    /// the same screen gives a picture the same share of the panel on a Clara
    /// and on an Elipsa.
    #[must_use]
    pub fn picture(self, picture: TilePicture, max_height_mm: u16) -> Self {
        self.drawn_picture(picture, max_height_mm, true)
    }

    /// The same, without a rule around it.
    ///
    /// For a picture that is part of the text rather than an illustration of
    /// it -- a formula set on its own line, say. An edge tells a reader where
    /// an illustration stops; drawn around a line of mathematics it only says
    /// that the line was drawn rather than written, which is not something the
    /// reader needs to know.
    #[must_use]
    pub fn unframed_picture(self, picture: TilePicture, max_height_mm: u16) -> Self {
        self.drawn_picture(picture, max_height_mm, false)
    }

    #[must_use]
    fn drawn_picture(mut self, picture: TilePicture, max_height_mm: u16, framed: bool) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Picture {
            id,
            handle: picture.handle,
            source: picture.source,
            max_height_tenths_mm: max_height_mm.saturating_mul(10),
            framed,
        });
        self
    }

    /// Lists entries that each need a sentence of explanation.
    ///
    /// Prefer this over [`Self::tiles`] whenever a one-word label would not be
    /// enough. A tile is square and spends most of its area on nothing, so a
    /// screen of tiles holds very few entries; a row holds a title, a summary
    /// and a glyph in a single finger-height band.
    #[must_use]
    pub fn rows<I, N, T, S, L>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead) in source.by_ref().take(MAX_ROWS) {
            rows.push(Row::new(self.register(name.as_ref()), title, summary, lead));
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// The same, with an overflow mark against the right edge of each row.
    ///
    /// The mark is a vertical three dot control naming an action of its own,
    /// so a tap on it is not a tap on the row. Use it for the things a reader
    /// might want to do *to* an entry rather than *with* it: stop following a
    /// feed, forget a book, remove a key. What the action opens is the
    /// application's business, and a popover is usually the right answer.
    ///
    /// An empty menu name means no mark on that row, exactly as an empty
    /// trailing value means no value.
    #[must_use]
    pub fn rows_with_menu<I, N, T, S, L, M>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L, M)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
        M: AsRef<str>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead, menu) in source.by_ref().take(MAX_ROWS) {
            let row = Row::new(self.register(name.as_ref()), title, summary, lead);
            let menu = menu.as_ref();
            rows.push(if menu.is_empty() {
                row
            } else {
                let action = self.register(menu);
                row.with_menu(action)
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// The same, with a short value against the right edge of each row.
    ///
    /// A score, a size, a date, a count. A separate method rather than a fifth
    /// element on [`Self::rows`] because most lists have no such value, and a
    /// tuple whose last member is almost always empty is a tuple every caller
    /// has to read twice.
    ///
    /// An empty value means no value, exactly as an empty summary does. The
    /// value is measured before the title is wrapped, so a long title gives up
    /// its own room rather than pushing the value off the panel.
    #[must_use]
    pub fn rows_with_trailing<I, N, T, S, L, V>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L, V)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
        V: Into<String>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead, trailing) in source.by_ref().take(MAX_ROWS) {
            let row = Row::new(self.register(name.as_ref()), title, summary, lead);
            let trailing = trailing.into();
            rows.push(if trailing.is_empty() {
                row
            } else {
                row.with_trailing(trailing)
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A list of things to be done, some of which are.
    ///
    /// The same rows, with the state carried rather than drawn: an application
    /// says whether each entry is finished and the renderer decides what
    /// finished looks like. That is why there is no way to ask for a line
    /// through a piece of text anywhere else in this SDK.
    ///
    /// Tapping a row is what completes it, and only the row that changed is
    /// repainted, so ticking something off costs one fast partial refresh
    /// rather than a whole screen.
    #[must_use]
    pub fn checklist<I, N, T, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, bool)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
    {
        let id = self.next_id();
        let mut source = items.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, done) in source.by_ref().take(MAX_ROWS) {
            let glyph = if done { Glyph::Check } else { Glyph::Circle };
            rows.push(Row::new(self.register(name.as_ref()), title, summary, glyph).done(done));
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A grid of characters, for output that was written to be read in columns.
    ///
    /// Everything else in this builder takes meaning and lets the runtime
    /// decide on appearance. This takes rows that are already positioned,
    /// because in a character grid the position *is* the meaning: a table, a
    /// diff or a shell prompt stops saying what it said the moment something
    /// re-wraps it.
    ///
    /// The grid is not negotiable from here. Ask [`kobo_ui::terminal_grid_for`]
    /// what size the rows should be before filling them, so that whatever is
    /// producing the text is told the same width the panel will show.
    #[must_use]
    pub fn terminal<I, R>(mut self, rows: I, cursor: Option<Caret>) -> Self
    where
        I: IntoIterator<Item = R>,
        R: Into<String>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let rows = source
            .by_ref()
            .take(MAX_TERMINAL_ROWS)
            .map(Into::into)
            .collect();
        if source.next().is_some() {
            self.warn_limit(id, "terminal rows", MAX_TERMINAL_ROWS);
        }
        self.nodes.push(Node::Terminal { id, rows, cursor });
        self
    }
    /// A grid of buttons.
    ///
    /// The general one: the caller picks the columns, so a board, a keypad and
    /// an on-screen keyboard are all this, rather than three primitives that
    /// each have to be added to the layout engine, the renderer, the hit test
    /// and the wire format before anybody can use them.
    ///
    /// `square` gives cells as tall as they are wide, which is what makes a
    /// board look like a board. Without it a cell is one touch target high,
    /// which is what a keyboard wants.
    #[must_use]
    pub fn grid<I, N, L>(self, columns: u8, square: bool, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        self.grid_with_selection(
            columns,
            square,
            cells.into_iter().map(|(name, label)| (name, label, false)),
        )
    }

    /// A grid with explicit selected keys. Selection uses an ink outline in
    /// addition to the ordinary key field; the app retains toggle behavior.
    /// Labels need no added brackets or check characters to communicate state.
    #[must_use]
    pub fn grid_with_selection<I, N, L>(mut self, columns: u8, square: bool, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, bool)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, selected) in source.by_ref().take(MAX_CELLS) {
            cells.push(Cell::new(self.register(name.as_ref()), label).with_selected(selected));
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square,
            cells,
        });
        self
    }

    /// A square grid whose filled cells are drawn as marks rather than words.
    ///
    /// For a board. A letter set at label size in a cell a finger and a half
    /// wide is a caption in the middle of an empty square, which is what a
    /// tic-tac-toe board looked like: the "O" was smaller than the heading
    /// above it. A mark is drawn at three fifths of the cell, so the board
    /// reads as a board from arm's length.
    ///
    /// `None` leaves the cell empty and still tappable, because an unplayed
    /// square is the one a reader is aiming at.
    #[must_use]
    pub fn board<I, N, L>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<Glyph>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph) in source.by_ref().take(MAX_CELLS) {
            let cell = Cell::new(self.register(name.as_ref()), label);
            cells.push(match glyph {
                Some(glyph) => cell.with_glyph(glyph),
                None => cell,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: true,
            cells,
        });
        self
    }

    /// Fifteen recessed square keys in three rows of five.
    ///
    /// The shape of a hardware command deck. Assigned cells carry a short
    /// label and an optional mark; unused slots stay as blank keys so the
    /// grid does not collapse into a list.
    #[must_use]
    pub fn pads<I, N, L>(mut self, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<kobo_ui::Glyph>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph) in source.by_ref().take(15) {
            let cell = Cell::new(self.register(name.as_ref()), label);
            cells.push(match glyph {
                Some(glyph) => cell.with_glyph(glyph),
                None => cell,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", 15);
        }
        while cells.len() < 15 {
            cells.push(Cell::new(
                self.register(&format!("empty-{}", cells.len())),
                "",
            ));
        }
        self.nodes.push(Node::Grid {
            id,
            columns: 5,
            square: true,
            cells,
        });
        self
    }

    /// A board whose current source cell is drawn inverted.
    #[must_use]
    pub fn board_with_selection<I, N, L>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<Glyph>, bool)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph, selected) in source.by_ref().take(MAX_CELLS) {
            let cell = Cell::new(self.register(name.as_ref()), label).with_selected(selected);
            cells.push(match glyph {
                Some(glyph) => cell.with_glyph(glyph),
                None => cell,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: true,
            cells,
        });
        self
    }

    /// Physical pencil-puzzle geometry shared by the reader and simulator.
    /// Actions use stable IDs from `action_id`; fixed clues carry no action.
    /// Requires the protocol-14 beta runtime's pencil-board node.
    #[must_use]
    pub fn pencil_board(mut self, board: kobo_ui::PencilBoard) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::PencilBoard { id, board });
        self
    }

    /// A newspaper-style grid: joined squares, corner numbers and centered letters.
    /// Requires the protocol-14 beta runtime's numbered-board node.
    #[must_use]
    pub fn crossword_board<I, N>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, char, Option<u8>, bool)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let cells = cells
            .into_iter()
            .take(MAX_CELLS)
            .map(|(name, letter, corner, selected)| {
                let mut cell = Cell::new(self.register(name.as_ref()), letter.to_string())
                    .with_selected(selected);
                cell.corner = corner.filter(|n| (1..=99).contains(n));
                cell
            })
            .collect();
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: true,
            cells,
        });
        self
    }

    /// A row of buttons that each have a picture as well as a word.
    ///
    /// For the handful of actions that have a drawing everybody already knows:
    /// the transport controls, chiefly. Reach for it only when the picture is
    /// genuinely universal. A glyph invented for a verb nobody draws is worse
    /// than the verb written out, because the reader now has to decode the
    /// icon *and* read the label to check they agree.
    ///
    /// The label always stays. The picture is the fast path for someone who
    /// already knows the control; the word is what makes it learnable, and it
    /// is the only part that can say "thirty seconds".
    #[must_use]
    pub fn controls<I, N, L>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph) in source.by_ref().take(MAX_CELLS) {
            cells.push(Cell::new(self.register(name.as_ref()), label).with_glyph(glyph));
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: false,
            cells,
        });
        self
    }

    /// Offers a value that moves one notch at a time.
    ///
    /// This is the shape a setting takes when its values form a line rather
    /// than a set: type size, brightness, playback speed. A list of named
    /// options would say the same thing in five rows of full-width boxes, and
    /// on a panel that repaints in tenths of a second the reader would rather
    /// tap the same spot twice than read five labels to find the one above the
    /// one they have.
    ///
    /// A table, drawn as columns that line up rather than as a sentence.
    ///
    /// Rows are given exactly as the document had them, headings included:
    /// the widths are worked out from all of them together, which is the only
    /// way the columns can agree, and that arithmetic belongs to the layout
    /// rather than to whoever is describing the page.
    #[must_use]
    pub fn table(mut self, rows: Vec<kobo_ui::TableRow>, weights: Vec<u16>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Table { id, rows, weights });
        self
    }

    /// The two ends carry pictures, not words, so the control needs no
    /// translating and no room for a label. Whichever end has nowhere further
    /// to go is drawn muted and stops answering taps.
    #[must_use]
    pub fn stepper(
        mut self,
        label: impl Into<String>,
        less: impl AsRef<str>,
        less_glyph: Glyph,
        more: impl AsRef<str>,
        more_glyph: Glyph,
    ) -> Self {
        let id = self.next_id();
        let less =
            BarAction::new(self.register(less.as_ref()), String::new()).with_glyph(less_glyph);
        let more =
            BarAction::new(self.register(more.as_ref()), String::new()).with_glyph(more_glyph);
        self.nodes.push(Node::Stepper {
            id,
            label: label.into(),
            less,
            more,
            less_state: ControlState::Enabled,
            more_state: ControlState::Enabled,
            fill: None,
        });
        self
    }

    /// Says which ends of the stepper just declared still have somewhere to go.
    #[must_use]
    pub fn stepper_ends(mut self, less: bool, more: bool) -> Self {
        if let Some(Node::Stepper {
            less_state,
            more_state,
            ..
        }) = self.nodes.last_mut()
        {
            *less_state = if less {
                ControlState::Enabled
            } else {
                ControlState::Disabled
            };
            *more_state = if more {
                ControlState::Enabled
            } else {
                ControlState::Disabled
            };
        }
        self
    }

    /// Draws a hairline under the stepper just declared showing where in its
    /// range the value sits, as a percentage of the way along.
    ///
    /// Worth having where the reading is a number without a natural sense of
    /// scale: "60%" says little until you can see it is past the middle.
    #[must_use]
    pub fn stepper_track(mut self, percent: u8) -> Self {
        if let Some(Node::Stepper { fill, .. }) = self.nodes.last_mut() {
            *fill = Some(percent.min(100));
        }
        self
    }

    /// Asks a question by offering answers.
    ///
    /// Prefer this over a text field. Typing on this device means summoning a
    /// keyboard onto a slow panel and hunting for keys, and it is markedly
    /// worse than tapping for anything that can be enumerated.
    #[must_use]
    pub fn choose<I, N, L>(mut self, prompt: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = options.into_iter();
        let mut options = Vec::new();
        for (name, label) in source.by_ref().take(MAX_CHOICE_OPTIONS) {
            options.push(BarAction::new(self.register(name.as_ref()), label));
        }
        if source.next().is_some() {
            self.warn_limit(id, "choice options", MAX_CHOICE_OPTIONS);
        }
        self.nodes.push(Node::Choice {
            id,
            prompt: prompt.into(),
            options,
            selected: None,
            freeform: None,
        });
        self
    }

    /// Adds the free-text escape hatch to the choice just declared.
    ///
    /// Deliberately a second call rather than a parameter, so that offering
    /// typing is a decision an author makes on purpose. The keyboard is only
    /// raised if the reader actually taps this row.
    #[must_use]
    pub fn or_type(mut self, name: impl AsRef<str>, placeholder: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Choice { freeform, .. }) = self.nodes.last_mut() {
            *freeform = Some(Freeform::new(action, placeholder));
        }
        self
    }

    /// Marks which option of the choice just declared is already the answer.
    ///
    /// State rather than decoration: the renderer draws the mark from the icon
    /// atlas, so an application never has to put a tick character in a label
    /// and never gets a missing-glyph box on a device whose face lacks it. An
    /// index naming no option leaves every row unmarked.
    #[must_use]
    pub fn chosen(mut self, index: usize) -> Self {
        if let Some(Node::Choice {
            options, selected, ..
        }) = self.nodes.last_mut()
        {
            *selected = u8::try_from(index)
                .ok()
                .filter(|index| usize::from(*index) < options.len());
        }
        self
    }

    /// Adds an attention strip.
    ///
    /// This is what to reach for instead of flashing the frontlight, which is a
    /// photosensitivity hazard and the largest power draw on the device.
    #[must_use]
    pub fn banner(mut self, level: BannerLevel, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Banner {
            id,
            level,
            text: text.into(),
        });
        self
    }

    /// Adds placeholder lines occupying the space real content will fill.
    ///
    /// Paint the real screen with these immediately and patch them as data
    /// arrives, rather than showing a splash. The panel is already displaying
    /// something at zero power, so there is no blank frame to cover.
    #[must_use]
    pub fn skeleton(mut self, lines: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Skeleton { id, lines });
        self
    }

    /// A mark, a name and a sentence, centred in the room that is left.
    ///
    /// For the moment between asking for something and it arriving: opening an
    /// application, or a screen that exists only to say what is being waited
    /// on. Everything else on this platform is set ranged left from the top,
    /// which is right for reading and wrong for four words -- they land in the
    /// corner and read as a page that failed.
    ///
    /// Takes the rest of the content area, so put it last.
    #[must_use]
    pub fn splash(
        mut self,
        glyph: Option<Glyph>,
        title: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Splash {
            id,
            glyph,
            title: title.into(),
            summary: summary.into(),
        });
        self
    }

    /// States that work is in flight, for example a network request.
    ///
    /// The replacement for a spinner. Pass `None` for progress unless a real
    /// denominator is known; a bar that invents its own position is worse than
    /// no bar. Progress is snapped to coarse steps before it is drawn.
    #[must_use]
    pub fn activity(mut self, label: impl Into<String>, progress: Option<u8>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Activity {
            id,
            label: label.into(),
            progress: progress.map(Percent::new),
            cancel: None,
            transferred: None,
            failure: None,
        });
        self
    }

    /// States that bytes are arriving.
    ///
    /// `total` is the length the server announced, and `None` when it
    /// announced none. The distinction is the entire reason this exists: with
    /// a total you get a bar and "4.2 MB of 11 MB"; without one you get the
    /// count alone and no bar, because a progress bar that has invented its
    /// own denominator lies to the reader for as long as the download lasts.
    /// Byte counts are formatted by the renderer, so every application says
    /// "4.2 MB" the same way.
    ///
    /// Both numbers are **bytes**. Counting anything else with this -- stories
    /// fetched, messages sent -- captions the bar "3 B of 6 B". Use
    /// [`Self::activity`] with a percentage and say the count in the label.
    #[must_use]
    pub fn transfer(mut self, label: impl Into<String>, received: u64, total: Option<u64>) -> Self {
        let id = self.next_id();
        let progress = total.and_then(|total| {
            (total > 0).then(|| {
                let percent = received.saturating_mul(100) / total;
                Percent::new(u8::try_from(percent.min(100)).unwrap_or(100))
            })
        });
        // "4.2 MB" with no total is a truthful report of an unknown-length
        // download. "0 B" is not the same statement: it is the state every
        // such download begins in, it is what a reader sees for the whole of
        // a transfer the runtime hands over in one piece, and it reads as a
        // download that is failing rather than one that has not answered yet.
        // With nothing received and no total there is no amount to report, so
        // the label carries the screen alone.
        let transferred = (received > 0 || total.is_some()).then_some((received, total));
        self.nodes.push(Node::Activity {
            id,
            label: label.into(),
            progress,
            cancel: None,
            transferred,
            failure: None,
        });
        self
    }

    /// Says why the transfer just declared stopped.
    ///
    /// Attaches to the activity rather than replacing the screen, so whatever
    /// the reader was looking at is still there when it fails.
    #[must_use]
    pub fn transfer_failed(mut self, reason: impl Into<String>, resumable: bool) -> Self {
        if let Some(Node::Activity { failure, .. }) = self.nodes.last_mut() {
            *failure = Some(TransferFailure {
                reason: reason.into(),
                resumable,
            });
        }
        self
    }

    /// Offers to try again, but only if trying again could work.
    ///
    /// A no-op when the failure was not resumable. Offering a retry for
    /// something that can never succeed teaches readers that the controls on
    /// this device do nothing.
    #[must_use]
    pub fn transfer_retry(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let resumable = matches!(
            self.nodes.last(),
            Some(Node::Activity {
                failure: Some(TransferFailure {
                    resumable: true,
                    ..
                }),
                ..
            })
        );
        if resumable {
            self.button(name, label)
        } else {
            self
        }
    }

    /// Lets the reader abandon the activity just declared.
    #[must_use]
    pub fn cancellable(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Activity { cancel, .. }) = self.nodes.last_mut() {
            *cancel = Some(BarAction::new(action, label));
        }
        self
    }

    #[must_use]
    pub fn build(self) -> Screen {
        Screen {
            id: self.id,
            top_bar: self.top_bar,
            nodes: self.nodes,
            nav_bar: self.nav_bar,
            bottom_action: self.bottom_action,
            page_turns: self.page_turns,
            hold: self.hold,
            owns_back: self.owns_back,
            text_scale: self.text_scale,
            overlay: self.overlay,
            reading: self.reading,
            // Applications built with this SDK emit the current protocol and
            // measure against Folio. Only a v11 decoder path marks legacy.
            legacy_typography: false,
            reading_font: self.reading_font,
        }
    }

    /// Returns warnings raised while bounded collections were added.
    ///
    /// Builders consume at most one item past each limit, so an accidental
    /// infinite iterator remains safe while the caller still learns that data
    /// was omitted.
    #[must_use]
    pub fn warnings(&self) -> &[LayoutIssue] {
        &self.warnings
    }

    /// Builds only when no rows, options, cells, or terminal lines were
    /// silently omitted.
    ///
    /// # Errors
    ///
    /// Returns every collection-limit warning raised while building. The
    /// ordinary [`Self::build`] remains available for compatibility.
    pub fn build_checked(self) -> Result<Screen, Vec<LayoutIssue>> {
        if self.warnings.is_empty() {
            Ok(self.build())
        } else {
            Err(self.warnings)
        }
    }

    pub(crate) fn register(&mut self, name: &str) -> ActionId {
        let action = action_id(name);
        if !self.actions.iter().any(|(known, _)| known == name) {
            self.actions.push((name.to_owned(), action));
        }
        action
    }

    pub(crate) fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node = self.next_node.saturating_add(1);
        id
    }

    /// Warns when a second bottom bar replaces the first.
    ///
    /// The panel has one bottom band and the last caller wins, silently. An
    /// application that called `action_bar` and then `nav_bar` -- which is
    /// what happens the moment a shared screen helper appends navigation --
    /// drew a screen with its verbs simply missing, and nothing anywhere said
    /// so.
    fn warn_second_bottom_bar(&mut self, id: NodeId) {
        if self.nav_bar.is_none() && self.bottom_action.is_none() {
            return;
        }
        self.warnings.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::CollectionTruncated {
                collection: "bottom bar",
                provided: 2,
                visible: 1,
            },
            rect: None,
        });
    }

    fn warn_limit(&mut self, id: NodeId, collection: &'static str, visible: usize) {
        self.warnings.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::CollectionTruncated {
                collection,
                provided: visible + 1,
                visible,
            },
            rect: None,
        });
    }
}
