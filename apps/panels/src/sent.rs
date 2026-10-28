//! Comics sent from the computer with `kobo panels push`, listed under the
//! titles they were sent with until they are added to the shelf.

use super::{transfer, Context, Kept, Panels, Route, ShelfDownload, ShelfProgress, StoreResult};
use kobo_comic::inbox::{self, Sent};
use kobo_sdk::{Glyph, Position, Screen, ScreenBuilder};

impl Panels {
    /// Reads the list of sent comics again.
    ///
    /// When Panels opens, when the reader wakes and whenever somebody comes
    /// back to it: a comic pushed while Panels sat in the background is then
    /// waiting by the time anybody looks for it.
    pub(super) fn look_for_sent(&mut self, context: &mut Context) {
        if self.sent_load.is_some() {
            return;
        }
        let mut load = ShelfDownload::new(inbox::MANIFEST).at_most(inbox::MAX_MANIFEST);
        load.start(context);
        self.sent_load = Some(load);
    }

    pub(super) fn advance_sent(&mut self, context: &mut Context, result: &StoreResult) {
        let Some(load) = &mut self.sent_load else {
            return;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.sent_load.take().expect("active sent list").take();
                // A damaged list hides nothing that is already on the shelf,
                // so it is set aside rather than reported as a fault: the
                // next push writes a whole one.
                self.sent = inbox::decode(&bytes).unwrap_or_default();
                self.add_waiting(context);
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.sent_load = None;
                self.sent.clear();
            }
            ShelfProgress::Failed(_) => self.sent_load = None,
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
    }

    fn on_shelf_already(&self, sent: &Sent) -> Option<Kept> {
        self.library_entries()
            .iter()
            .find(|kept| kept.key == sent.sha256)
            .cloned()
    }

    /// Sent comics that are not on the shelf yet.
    pub(super) fn waiting(&self) -> usize {
        self.sent
            .iter()
            .filter(|sent| self.on_shelf_already(sent).is_none())
            .count()
    }

    /// The line the shelf shows while sent comics are waiting to be added.
    pub(super) fn waiting_notice(&self) -> Option<String> {
        match self.waiting() {
            0 => None,
            1 => Some("A comic from your computer is waiting. Choose Add comic.".to_owned()),
            count => Some(format!(
                "{count} comics from your computer are waiting. Choose Add comic."
            )),
        }
    }

    fn sent_rows(&self, context: &Context) -> Vec<(String, String, String)> {
        self.sent
            .iter()
            .enumerate()
            .map(|(index, sent)| {
                let state = if self.on_shelf_already(sent).is_some() {
                    "On your shelf"
                } else {
                    "Ready to add"
                };
                (
                    format!("sent-{index}"),
                    context.clamped_row(&sent.title, 2, true),
                    format!(
                        "{} page{} · {state}",
                        sent.pages,
                        if sent.pages == 1 { "" } else { "s" }
                    ),
                )
            })
            .collect()
    }

    pub(super) fn sent_screen(&self, context: &Context) -> Screen {
        let screen = ScreenBuilder::new("panels-sent")
            .top_bar("Add comic")
            .owns_back(true)
            .heading("Sent from your computer")
            .secondary("Choose a comic to add it to your shelf.");
        let rows = self.sent_rows(context);
        let labels = rows
            .iter()
            .map(|(_, title, detail)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_under(
            &labels,
            true,
            Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = self.sent_page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen.rows(visible.iter().map(|&row| {
            let (action, title, detail) = &rows[row];
            (action.clone(), title.clone(), detail.clone(), Glyph::Reader)
        }));
        if count > 1 {
            screen = screen
                .page_turns("sent-previous", "sent-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        screen
            .action_bar([("sample-comic", "Sample"), ("usb-help", "Over USB")])
            .build()
    }

    /// Opens a sent comic: the shelf copy when it is already there, otherwise
    /// the preview that adds it, under the title it was sent with.
    pub(super) fn choose_sent(&mut self, context: &mut Context, index: usize) {
        let Some(sent) = self.sent.get(index).cloned() else {
            return;
        };
        if let Some(kept) = self.on_shelf_already(&sent) {
            self.route = Route::Library;
            self.open_kept(context, &kept);
            return;
        }
        if self.library.is_none()
            || self.local_load.is_some()
            || self.import.is_some()
            || self.pending_open.is_some()
            || self.shelf_load.is_some()
        {
            return;
        }
        self.route = Route::Import;
        self.notice = None;
        let mut load = ShelfDownload::new(&sent.file).at_most(transfer::MAX_COMIC);
        load.start(context);
        self.local_load = Some(load);
        self.sent_from = Some(sent);
    }

    /// Once Panels holds its own verified copy, the sent file has done its
    /// job, and leaving it would keep every comic on the reader twice.
    pub(super) fn tidy_sent(&mut self, context: &mut Context) {
        if let Some(sent) = self.sent_from.take() {
            context.shelf().remove(&sent.file);
            self.sent_cleanup.insert(sent.file);
        }
    }

    /// Adds the next comic waiting from the computer, when nothing else is in
    /// hand and the shelf is on screen.
    ///
    /// A comic pushed from the computer is one the owner already chose, so it
    /// goes on the shelf by itself on return rather than waiting behind three
    /// more taps: an owner who pushed a comic and found an empty shelf took
    /// the push for a failure. The add is the same verified import the Add
    /// comic screen runs, with its progress on screen while it works.
    pub(super) fn add_waiting(&mut self, context: &mut Context) {
        if self.route != Route::Library
            || !self.loaded
            || self.local_load.is_some()
            || self.import.is_some()
            || self.pending_open.is_some()
            || self.shelf_load.is_some()
            || self.upload.is_some()
            || !self
                .library
                .as_ref()
                .is_some_and(|library| library.status() == kobo_state::draft::Status::Saved)
        {
            return;
        }
        let next = self.sent.iter().position(|sent| {
            self.on_shelf_already(sent).is_none() && !self.add_failed.contains(&sent.sha256)
        });
        match next {
            Some(index) => {
                self.adding = true;
                self.choose_sent(context, index);
            }
            None => self.adding = false,
        }
    }

    /// The import that was adding a sent comic is safely on the shelf: back
    /// to the shelf, and on to the next one.
    pub(super) fn added_one(&mut self, context: &mut Context) {
        if self.adding
            && self.route == Route::Import
            && self
                .import
                .as_ref()
                .is_some_and(kobo_sdk::imports::Import::is_available)
        {
            self.import = None;
            self.import_entry = None;
            self.route = Route::Library;
            self.add_waiting(context);
        }
    }

    /// A sent comic that could not be read stops the run, and is left for
    /// the owner to see on the Add comic list rather than tried again.
    pub(super) fn stop_adding(&mut self) {
        self.adding = false;
        if let Some(sent) = self.sent_from.take() {
            self.add_failed.insert(sent.sha256);
        }
        self.route = Route::Library;
    }

    /// Takes the answer to a removal [`Self::tidy_sent`] asked for. A file
    /// that could not be removed is only space: the comic is listed as on the
    /// shelf, and the next push with `--replace` clears it.
    pub(super) fn removed_sent(&mut self, name: &str) -> bool {
        self.sent_cleanup.remove(name)
    }
}
