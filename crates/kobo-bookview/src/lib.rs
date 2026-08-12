//! One reading surface, shared by every application that puts a document on
//! the panel.
//!
//! [`kobo_doc`] turns a file into a [`Document`], and [`kobo_read`] turns a
//! `Document` into pages, a place that survives the book being closed, a table
//! of contents, highlights and a dictionary. Between the two sits a stretch of
//! work that is neither parsing nor reading, and that every application was
//! expected to write for itself: reserving room for the illustrations before
//! any of them have been decoded, decoding them slowly enough that the panel's
//! watchdog stays asleep, handing the pixels to the runtime under the handle
//! the page reserved, and giving all of it back when the book is closed.
//!
//! That stretch is about two hundred and fifty lines, and the first
//! application to need it wrote them. The second application to need it --
//! arXiv, whose papers are HTML with figures in them -- would have written
//! them again, slightly differently, and the device would have had two
//! readers that behaved almost alike. This crate is the seam that stops that:
//! applications hold a [`BookView`], hand it bytes, and get the reader
//! everything else on the device already has.
//!
//! # What it opens
//!
//! Whatever [`kobo_doc::read`] opens, which today is EPUB, HTML, Markdown and
//! plain text, sniffed from the name and the leading bytes. A format added
//! there arrives here without this crate changing; PDF is the one people ask
//! for and the one that has no backend yet, so [`BookView::open_bytes`] on a
//! PDF fails the same way an unreadable EPUB does rather than pretending.
//!
//! # Pictures that are not in the file
//!
//! An EPUB carries its illustrations inside the container, so a book opens
//! with every plate already in hand. A web page does not: `<img src="x1.png">`
//! is a promise to fetch something, and [`kobo_doc::html`] leaves
//! [`Document::images`] empty because a parser is not allowed to touch the
//! network. Such a document opens immediately with its figures standing in as
//! their own descriptions, and the application fetches the names
//! [`BookView::missing_pictures`] gives it, hands each one back through
//! [`BookView::provide_picture`], and calls [`BookView::settle_pictures`] when
//! the batch is done. The book is measured once more at that point, and the
//! reader stays on the block they were reading rather than the page number
//! that block used to be on.

use std::collections::{BTreeMap, VecDeque};

use kobo_doc::{Block, Document};
use kobo_read::{Memory, Outcome, Reader};
use kobo_sdk::{Context, DisplayMetrics, PictureHandle, Screen, Task, TaskId, TilePicture};

/// The most pictures one document may have room reserved for.
///
/// Every one costs a handle in the runtime and a decode on a processor that
/// takes about a quarter of a second over each. An illustrated children's book
/// is a few dozen; a paper is a dozen. A container that names four hundred is
/// either a gallery or a mistake, and either way the sixty-fifth plate is not
/// the thing standing between somebody and the page they wanted.
pub const MAX_PICTURES: usize = 64;

/// The handle illustrations are numbered from.
///
/// An application draws other things through the same runtime -- a shelf of
/// covers, an icon strip -- and those hold handles of their own. Book plates
/// start high enough to leave room underneath, and an application that already
/// numbers its own pictures past this can move them with
/// [`BookView::numbered_from`].
pub const PICTURE_HANDLE_BASE: u32 = 1_000;

/// The width a plate is fitted into, in millimetres.
const PLATE_WIDTH_MM: u16 = 80;

/// The height a plate is fitted into, in millimetres.
///
/// The ceiling `kobo-read` draws pictures at. Fitted before the pixels are
/// handed over rather than after, because a plate scanned at print resolution
/// is several megabytes the panel has no way to show.
const PLATE_HEIGHT_MM: u16 = 90;

/// What a step of the picture pipeline left behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// The step was not this view's to take.
    Elsewhere,
    /// Work happened, and nothing on the page in front of anybody changed.
    ///
    /// A plate twenty pages ahead is not a reason to flash an E Ink panel, and
    /// an illustrated book has two dozen of them.
    Quiet,
    /// A picture on the page being read arrived. Draw it.
    Repaint,
}

/// A document, open, with everything it costs the device accounted for.
///
/// Created with [`BookView::new`], filled with [`BookView::open`] or
/// [`BookView::open_bytes`], drawn with [`BookView::screen`], driven with
/// [`BookView::act`] and [`BookView::woke`], and emptied with
/// [`BookView::close`]. A view with no document in it is not an error state:
/// it is what an application holds while somebody is browsing, and every
/// method says so rather than panicking.
#[derive(Debug, Default)]
pub struct BookView {
    reader: Option<Reader>,
    /// The room claimed for each picture, by name.
    ///
    /// Kept rather than handed straight to the reader because a picture that
    /// arrives late has to be merged into what was reserved at open, and the
    /// reader takes the whole map or none of it.
    reserved: BTreeMap<String, TilePicture>,
    /// Source bytes for pictures with room reserved and no pixels yet, in the
    /// order the text refers to them.
    plates: VecDeque<(String, Vec<u8>)>,
    /// A plate read and fitted, waiting for its greys.
    dithering: Option<(String, kobo_image::Picture)>,
    /// The task carrying the pipeline forward, if one is in flight.
    plating: Option<TaskId>,
    /// Bytes handed over for pictures the document could not supply itself,
    /// waiting for [`BookView::settle_pictures`] to measure them.
    offered: BTreeMap<String, Vec<u8>>,
    /// Every handle the runtime is holding pixels against for this document.
    handles: Vec<PictureHandle>,
    /// The next handle to reserve against.
    next_handle: u32,
    /// The handle plates are numbered from.
    base: u32,
}

impl BookView {
    /// An empty view, numbering its plates from [`PICTURE_HANDLE_BASE`].
    #[must_use]
    pub fn new() -> Self {
        Self::numbered_from(PICTURE_HANDLE_BASE)
    }

    /// An empty view, numbering its plates from `base`.
    ///
    /// For an application whose own pictures would otherwise collide with the
    /// book's.
    #[must_use]
    pub fn numbered_from(base: u32) -> Self {
        Self {
            base,
            next_handle: base,
            ..Self::default()
        }
    }

    /// A view holding a document somebody else opened.
    ///
    /// For an application that already has a [`Reader`] in hand. Nothing is
    /// reserved and nothing is queued: a document arriving this way draws its
    /// figures as their own descriptions until pictures are offered through
    /// [`BookView::provide_picture`].
    #[must_use]
    pub fn holding(reader: Reader) -> Self {
        Self {
            reader: Some(reader),
            ..Self::new()
        }
    }

    /// Opens whatever `bytes` turn out to be.
    ///
    /// The name is what decides the format when the bytes are ambiguous, so it
    /// is worth passing the real one; `kobo_doc` sniffs the leading bytes
    /// either way. An error here is a file this device cannot read, and the
    /// view is left holding nothing.
    ///
    /// # Errors
    ///
    /// Returns the parse fault when the bytes are not a document this device
    /// knows how to open.
    pub fn open_bytes(
        &mut self,
        context: &mut Context,
        name: &str,
        bytes: &[u8],
        memory: Memory,
    ) -> Result<(), kobo_doc::epub::Fault> {
        let document = kobo_doc::read(name, bytes)?;
        self.open(context, document, memory);
        Ok(())
    }

    /// Opens a document that has already been parsed.
    ///
    /// The pictures the container carried are taken off the document rather
    /// than left on it: the reader owns the document from here, and holding
    /// the scanned bytes as well as the decoded greyscale meant every plate
    /// was paid for twice.
    pub fn open(&mut self, context: &mut Context, mut document: Document, memory: Memory) {
        // Whatever the last document handed over is about to be replaced, so
        // it goes back first. Without this, a book reopened from the shelf
        // left its first set decoded in the runtime with nothing left that
        // could name it.
        self.release(context);
        let images = std::mem::take(&mut document.images);
        let metrics = context.metrics();
        let mut reader = Reader::open(document, memory, &metrics);
        // Before the page count is read, because the sizes are what an
        // illustrated document is measured at.
        for name in reader
            .pictures_wanted()
            .into_iter()
            .take(MAX_PICTURES)
            .map(str::to_owned)
            .collect::<Vec<_>>()
        {
            if let Some(bytes) = images.get(&name) {
                self.reserve(&name, bytes, &metrics);
            }
        }
        reader.set_pictures(self.reserved.clone(), &metrics);
        self.reader = Some(reader);
        // Handed to the runtime rather than started here. Opening has already
        // spent its parse and its pagination, and a plate on top of that is
        // what puts a lifecycle callback past the watchdog's deadline.
        self.kick(context);
    }

    /// Gives back everything the open document was costing.
    ///
    /// Leaving a book used to release nothing, so an owner who read one
    /// illustrated book and went back to browsing carried the whole of it --
    /// parsed document, source bytes and every decoded plate -- for the rest
    /// of the session.
    pub fn close(&mut self, context: &mut Context) {
        self.release(context);
        self.reader = None;
    }

    /// Whether there is a document open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.reader.is_some()
    }

    /// The reader, for the parts of it this view does not wrap.
    #[must_use]
    pub const fn reader(&self) -> Option<&Reader> {
        self.reader.as_ref()
    }

    /// The reader, mutably, for the parts of it this view does not wrap.
    pub fn reader_mut(&mut self) -> Option<&mut Reader> {
        self.reader.as_mut()
    }

    /// Where the reader had got to, for saving.
    #[must_use]
    pub fn memory(&self) -> Option<&Memory> {
        self.reader.as_ref().map(Reader::memory)
    }

    /// How many plates are still waiting to become pixels.
    ///
    /// For an application that logs what opening a document cost.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.plates.len() + usize::from(self.dithering.is_some())
    }

    /// The page to draw, titled.
    #[must_use]
    pub fn screen(&self, title: &str) -> Option<Screen> {
        self.reader.as_ref().map(|reader| reader.screen(title))
    }

    /// Offers an action to the open document.
    ///
    /// `None` when nothing is open or the reader does not answer that action,
    /// which is the application's cue to handle it itself. Carrying the
    /// picture pipeline forward is part of this: a page turn is also the
    /// moment a plate on the page turned to is wanted, and an application that
    /// only ever decoded on a timer would stop decoding the moment the runtime
    /// had no room for one.
    pub fn act(&mut self, context: &mut Context, action: kobo_ui::ActionId) -> Option<Outcome> {
        let metrics = context.metrics();
        let outcome = match self.reader.as_mut()?.act_on(action, &metrics) {
            Outcome::Elsewhere => return None,
            handled => handled,
        };
        if self.plating.is_none() && (!self.plates.is_empty() || self.dithering.is_some()) {
            self.decode_more(context);
        }
        Some(outcome)
    }

    /// Carries the picture pipeline forward when its task lands.
    ///
    /// An application passes every finished task here; a task that was not
    /// this view's comes back as [`Step::Elsewhere`] and costs a comparison.
    pub fn woke(&mut self, context: &mut Context, task: TaskId) -> Step {
        if self.plating != Some(task) {
            return Step::Elsewhere;
        }
        self.plating = None;
        self.decode_more(context)
    }

    /// Every picture the document refers to and could not supply.
    ///
    /// Empty for a container that carries its own illustrations. For a web
    /// page these are the `src` attributes, exactly as they were written, so
    /// an application resolves them against wherever it fetched the page from.
    #[must_use]
    pub fn missing_pictures(&self) -> Vec<String> {
        let Some(reader) = &self.reader else {
            return Vec::new();
        };
        reader
            .pictures_wanted()
            .into_iter()
            .take(MAX_PICTURES)
            .filter(|name| !self.reserved.contains_key(*name))
            .map(str::to_owned)
            .collect()
    }

    /// Hands over the bytes for a picture the document could not supply.
    ///
    /// Held, not measured: measuring is what changes where the page breaks
    /// fall, and doing that once per figure as a dozen of them arrive would
    /// move the text under somebody who is reading it. The whole batch is
    /// taken in by [`BookView::settle_pictures`].
    ///
    /// Returns whether the picture was wanted; bytes for a name the document
    /// never mentioned are dropped rather than held.
    pub fn provide_picture(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() || self.reserved.contains_key(name) {
            return false;
        }
        if self.reserved.len() + self.offered.len() >= MAX_PICTURES {
            return false;
        }
        let wanted = self
            .reader
            .as_ref()
            .is_some_and(|reader| reader.pictures_wanted().contains(&name));
        if !wanted {
            return false;
        }
        self.offered.insert(name.to_owned(), bytes);
        true
    }

    /// Measures everything handed over since the document opened.
    ///
    /// One repagination for the batch. The reader keeps its place across it:
    /// where somebody is is remembered as a block, not a page number, so the
    /// words they were looking at are still the words in front of them even
    /// though there are now figures above them.
    ///
    /// Returns whether anything changed, which is whether to repaint.
    pub fn settle_pictures(&mut self, context: &mut Context) -> bool {
        if self.offered.is_empty() {
            return false;
        }
        let metrics = context.metrics();
        let offered = std::mem::take(&mut self.offered);
        // In the order the text refers to them rather than the order they
        // happened to arrive, so the queue decodes down the document.
        let order: Vec<String> = self.reader.as_ref().map_or_else(Vec::new, |reader| {
            reader
                .pictures_wanted()
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
        let mut settled = false;
        for name in order {
            if let Some(bytes) = offered.get(&name) {
                settled |= self.reserve(&name, bytes, &metrics);
            }
        }
        if !settled {
            return false;
        }
        let reserved = self.reserved.clone();
        if let Some(reader) = &mut self.reader {
            reader.set_pictures(reserved, &metrics);
        }
        self.kick(context);
        true
    }

    /// Claims the room one picture will take, decoding none of it.
    ///
    /// A picture's header states its size in its first few dozen bytes, and
    /// that is all pagination needs. Opening a book used to decode, fit and
    /// dither every illustration before it returned, inside a callback with a
    /// two hundred and fifty millisecond deadline: twenty-eight plates came to
    /// two thousand seven hundred milliseconds, during which nothing drew, no
    /// control answered, and three watchdogs counted.
    ///
    /// A header that will not parse is a picture that will not decode, so it
    /// is left out here and the page reads its description instead -- which is
    /// what a document with a broken figure should look like.
    fn reserve(&mut self, name: &str, bytes: &[u8], metrics: &DisplayMetrics) -> bool {
        if self.reserved.len() >= MAX_PICTURES {
            return false;
        }
        let Some((width, height)) = plate_box(metrics) else {
            return false;
        };
        let Ok(source) = kobo_image::size(bytes) else {
            return false;
        };
        let (drawn_width, drawn_height) = kobo_image::fitted_size(source, width, height);
        if drawn_width == 0 || drawn_height == 0 {
            return false;
        }
        let handle = PictureHandle(self.next_handle);
        self.next_handle = self.next_handle.saturating_add(1);
        self.reserved.insert(
            name.to_owned(),
            TilePicture::new(handle, drawn_width, drawn_height),
        );
        self.handles.push(handle);
        self.plates.push_back((name.to_owned(), bytes.to_vec()));
        true
    }

    /// Asks the runtime to carry the pipeline forward, if there is anything
    /// left to carry and nothing already carrying it.
    fn kick(&mut self, context: &mut Context) {
        if self.plating.is_some() || (self.plates.is_empty() && self.dithering.is_none()) {
            return;
        }
        self.plating = context.spawn(Task::Sleep { seconds: 0 });
    }

    /// Carries one plate one step further towards the panel, and asks to be
    /// called again while any step is left.
    ///
    /// One step, not as many as fit in a time budget. Nothing here can be
    /// interrupted half way, so a budget can only be checked before starting
    /// something, and the pass then runs for the budget plus however long that
    /// something takes: on the panel a hundred and twenty millisecond budget
    /// produced callbacks of 272, 293 and 311 milliseconds against a deadline
    /// of 250. One whole plate per pass was no better -- 250 to 311 -- because
    /// one whole plate is itself over the deadline on this processor.
    ///
    /// So a plate is taken in its two natural halves. Reading the file and
    /// fitting it is one; reducing a million pixels to the sixteen greys the
    /// panel can hold is the other, and on a development machine the two are
    /// two milliseconds and three, which on the reader is about a hundred and
    /// about a hundred and seventy. Either alone is comfortably inside the
    /// deadline.
    ///
    /// The page being looked at is decoded first. Everything else can arrive
    /// while it is being read.
    fn decode_more(&mut self, context: &mut Context) -> Step {
        let metrics = context.metrics();
        let Some((width, height)) = plate_box(&metrics) else {
            self.plates.clear();
            self.dithering = None;
            return Step::Quiet;
        };
        let showing: Vec<String> = self.reader.as_ref().map_or_else(Vec::new, |reader| {
            reader
                .pictures_on_page()
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
        let mut on_this_page = false;
        if let Some((name, mut picture)) = self.dithering.take() {
            // The second half: the greys.
            picture.dither(kobo_image::PANEL_GREYS);
            let (drawn_width, drawn_height) = (picture.width(), picture.height());
            if let Some(reserved) = self.handed(&name) {
                if context
                    .put_picture(reserved, drawn_width, drawn_height, picture.into_grey())
                    .is_some()
                    && showing.contains(&name)
                {
                    on_this_page = true;
                }
            }
        } else {
            self.wanted_first(&showing);
            // The first half: the file. A plate that will not read costs
            // nothing, so this goes past it rather than spending a whole round
            // trip discovering there was nothing to draw, and stops on the
            // first one that works.
            while let Some((name, bytes)) = self.plates.pop_front() {
                if self.handed(&name).is_none() {
                    continue;
                }
                let Ok(picture) = kobo_image::decode(&bytes) else {
                    continue;
                };
                let Ok(picture) = picture.fit(width, height) else {
                    continue;
                };
                self.dithering = Some((name, picture));
                break;
            }
        }
        self.kick(context);
        if on_this_page {
            Step::Repaint
        } else {
            Step::Quiet
        }
    }

    /// Moves the named plates to the front of the queue, keeping their order.
    fn wanted_first(&mut self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let (wanted, rest): (VecDeque<_>, VecDeque<_>) = self
            .plates
            .drain(..)
            .partition(|(name, _)| names.contains(name));
        self.plates = wanted.into_iter().chain(rest).collect();
    }

    /// The handle a plate was reserved against, if it was.
    fn handed(&self, name: &str) -> Option<PictureHandle> {
        self.reserved.get(name).map(|picture| picture.handle)
    }

    /// Hands back every handle and forgets every plate, leaving the reader.
    ///
    /// Whatever is still queued is source bytes for a document nobody is
    /// reading any more. A sleep already in flight is left to land and be
    /// ignored: cancelling it would cost a message to save nothing.
    fn release(&mut self, context: &mut Context) {
        for handle in self.handles.drain(..) {
            context.drop_picture(handle);
        }
        self.plates.clear();
        self.offered.clear();
        self.reserved.clear();
        self.dithering = None;
        self.plating = None;
        self.next_handle = self.base;
    }
}

/// The box an illustration is fitted into, in pixels.
///
/// `None` on a panel so small the box has no area, which is not a device this
/// runs on but is a shape the arithmetic can produce.
fn plate_box(metrics: &DisplayMetrics) -> Option<(u32, u32)> {
    let width = metrics.tenth_mm(i32::from(PLATE_WIDTH_MM) * 10);
    let height = metrics.tenth_mm(i32::from(PLATE_HEIGHT_MM) * 10);
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(width), Ok(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => None,
    }
}

/// Every picture a document refers to, before it is opened.
///
/// What an application asks in order to know what to fetch when the document
/// is a web page whose figures live somewhere else. The same order
/// [`BookView::missing_pictures`] uses, and available without a reader.
#[must_use]
pub fn pictures_in(document: &Document) -> Vec<&str> {
    let mut wanted = Vec::new();
    for block in &document.blocks {
        if let Block::Picture { name, .. } = block {
            if !wanted.contains(&name.as_str()) && wanted.len() < MAX_PICTURES {
                wanted.push(name.as_str());
            }
        }
    }
    wanted
}

#[cfg(test)]
mod tests {
    use super::{pictures_in, BookView, Step};
    use kobo_doc::{Block, Document};
    use kobo_read::{Memory, Reader};
    use kobo_sdk::{Command, Context, PictureHandle, TaskId};

    /// A page of prose, so that the room a plate takes is the difference
    /// between one page and two.
    fn prose(blocks: usize) -> Vec<Block> {
        (0..blocks)
            .map(|_| {
                Block::Paragraph(
                    "Once upon a time there were four little rabbits, and their names were \
                     Flopsy, Mopsy, Cotton-tail and Peter."
                        .to_owned(),
                )
            })
            .collect()
    }

    fn plate_bytes() -> Vec<u8> {
        kobo_image::encode_png_grey(1200, 2000, &vec![128; 1200 * 2000]).expect("a png of a plate")
    }

    fn illustrated(plate: &[u8]) -> Document {
        let mut blocks = prose(12);
        blocks.push(Block::Picture {
            name: "plate.png".to_owned(),
            alt: "Four rabbits".to_owned(),
        });
        Document {
            blocks,
            images: [("plate.png".to_owned(), plate.to_vec())]
                .into_iter()
                .collect(),
            ..Document::default()
        }
    }

    fn handed(commands: &[Command]) -> Vec<(PictureHandle, u32, u32)> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::PutPicture {
                    handle,
                    width,
                    height,
                    ..
                } => Some((*handle, *width, *height)),
                _ => None,
            })
            .collect()
    }

    fn dropped(commands: &[Command]) -> Vec<PictureHandle> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::DropPicture(handle) => Some(*handle),
                _ => None,
            })
            .collect()
    }

    /// A plate is measured from its header and decoded later.
    ///
    /// Opening The Tale of Peter Rabbit on the panel decoded twenty-eight
    /// images inside a lifecycle callback and took 2,784 milliseconds doing
    /// it, against a deadline of 250. Reading the headers costs microseconds
    /// and answers the only question pagination has, so the book opens at the
    /// page count it will be read at and the pixels arrive afterwards.
    #[test]
    fn a_book_of_plates_is_measured_from_its_headers_and_decoded_afterwards() {
        let plate = plate_bytes();
        let mut context = Context::default();
        let metrics = context.metrics();

        let bare = Document {
            blocks: prose(12),
            ..Document::default()
        };
        let without = Reader::open(bare, Memory::default(), &metrics).page_count();

        let mut view = BookView::new();
        view.open(&mut context, illustrated(&plate), Memory::default());

        assert_eq!(
            view.queued(),
            1,
            "the plate should be queued rather than decoded"
        );
        assert!(
            handed(&context.take_commands()).is_empty(),
            "a plate was decoded while the book was opening"
        );
        assert!(
            view.reader().expect("a book").page_count() > without,
            "the book was not measured around the room the plate takes"
        );
    }

    /// The room claimed from the header is the room the decoder asks for.
    #[test]
    fn the_room_claimed_from_a_header_is_the_room_the_decoder_wants() {
        let plate = plate_bytes();
        let mut context = Context::default();
        let metrics = context.metrics();
        let (width, height) = super::plate_box(&metrics).expect("a panel with room on it");

        let mut view = BookView::new();
        view.open(&mut context, illustrated(&plate), Memory::default());

        let decoded = kobo_image::decode(&plate)
            .expect("decode")
            .fit(width, height)
            .expect("fit");
        assert_eq!(
            view.reader()
                .and_then(|reader| reader.picture_named("plate.png"))
                .map(|picture| picture.source),
            Some((decoded.width(), decoded.height())),
            "the size claimed from the header is not the size the decoder produces"
        );
    }

    /// And the pixels, when they come, go into the room already reserved.
    #[test]
    fn a_queued_plate_is_handed_over_against_the_handle_it_was_measured_at() {
        let plate = plate_bytes();
        let mut context = Context::default();
        let mut view = BookView::new();
        let document = Document {
            blocks: vec![Block::Picture {
                name: "plate.png".to_owned(),
                alt: "Four rabbits".to_owned(),
            }],
            images: [("plate.png".to_owned(), plate)].into_iter().collect(),
            ..Document::default()
        };
        view.open(&mut context, document, Memory::default());
        let claimed = view
            .reader()
            .and_then(|reader| reader.picture_named("plate.png"))
            .expect("the plate was reserved");
        let first_task = view.plating.expect("the queue was not started");
        let _ = context.take_commands();

        // The first pass reads the plate and asks for another; the second
        // reduces it to the panel's greys and hands it over. Neither half is
        // allowed to take as long as both would.
        let step = view.woke(&mut context, first_task);
        assert!(
            handed(&context.take_commands()).is_empty(),
            "a plate was decoded and dithered in one callback"
        );
        assert_eq!(step, Step::Quiet, "a plate off screen asked for a repaint");
        assert!(
            view.plates.is_empty() && view.dithering.is_some(),
            "the plate was not read out of the book"
        );
        let next = view.plating.expect("another pass was not asked for");

        let step = view.woke(&mut context, next);
        assert_eq!(
            handed(&context.take_commands()),
            vec![(claimed.handle, claimed.source.0, claimed.source.1)],
            "the plate did not fill the frame that was standing in for it"
        );
        assert_eq!(
            step,
            Step::Repaint,
            "the plate on the page arrived without asking to be drawn"
        );
        assert!(
            view.dithering.is_none(),
            "the decoded plate was kept after it was drawn"
        );
    }

    /// A sleep belonging to somebody else is not this view's to answer.
    #[test]
    fn a_task_that_was_not_the_pipelines_is_left_alone() {
        let mut context = Context::default();
        let mut view = BookView::new();
        assert_eq!(
            view.woke(&mut context, TaskId(7)),
            Step::Elsewhere,
            "an unrelated task was taken for a plate"
        );
    }

    /// Leaving a document gives back every handle the runtime was holding.
    ///
    /// The runtime frees pictures when an application exits, which is no help
    /// at all to one that stays open: a book of plates was costing the device
    /// several megabytes of decoded greyscale while its owner was back on the
    /// shelf looking at covers.
    #[test]
    fn leaving_a_document_gives_back_every_handle_the_runtime_was_holding() {
        let plate = plate_bytes();
        let mut context = Context::default();
        let mut view = BookView::new();
        view.open(&mut context, illustrated(&plate), Memory::default());
        let handle = view
            .reader()
            .and_then(|reader| reader.picture_named("plate.png"))
            .expect("the plate was reserved")
            .handle;
        let _ = context.take_commands();

        view.close(&mut context);

        assert_eq!(
            dropped(&context.take_commands()),
            vec![handle],
            "the runtime was left holding the plates"
        );
        assert!(!view.is_open(), "the parsed document was kept");
        assert_eq!(view.queued(), 0, "the queue outlived the document");
    }

    /// A web page names its figures and carries none of them.
    #[test]
    fn a_page_whose_figures_live_elsewhere_says_which_ones_it_wants() {
        let mut context = Context::default();
        let mut view = BookView::new();
        let document = kobo_doc::html::parse(
            "<article><p>A paper.</p><img src=\"x1.png\" alt=\"A figure\"></article>",
        );
        assert_eq!(pictures_in(&document), vec!["x1.png"]);

        view.open(&mut context, document, Memory::default());

        assert_eq!(
            view.missing_pictures(),
            vec!["x1.png".to_owned()],
            "a figure with no bytes was not asked for"
        );
        assert_eq!(view.queued(), 0, "a figure with no bytes was queued anyway");
    }

    /// And when they arrive, the page is measured around them.
    #[test]
    fn a_figure_fetched_afterwards_is_measured_in_and_keeps_the_readers_place() {
        let plate = plate_bytes();
        let mut context = Context::default();
        let mut view = BookView::new();
        let mut html = String::from("<article>");
        for _ in 0..12 {
            html.push_str("<p>Once upon a time there were four little rabbits.</p>");
        }
        html.push_str("<img src=\"x1.png\" alt=\"A figure\"></article>");
        view.open(
            &mut context,
            kobo_doc::html::parse(&html),
            Memory::default(),
        );
        let without = view.reader().expect("a paper").page_count();

        assert!(
            view.provide_picture("x1.png", plate),
            "the figure the page asked for was refused"
        );
        assert_eq!(
            view.queued(),
            0,
            "an offered figure was queued before it was measured"
        );
        assert!(
            view.settle_pictures(&mut context),
            "measuring the offered figure changed nothing"
        );

        assert_eq!(view.queued(), 1, "the settled figure was not queued");
        assert!(
            view.missing_pictures().is_empty(),
            "the figure is still wanted"
        );
        assert!(
            view.reader().expect("a paper").page_count() > without,
            "the paper was not measured around the room the figure takes"
        );
    }

    /// Bytes for something the page never mentioned are not held.
    #[test]
    fn a_picture_the_page_never_asked_for_is_refused() {
        let mut context = Context::default();
        let mut view = BookView::new();
        view.open(
            &mut context,
            kobo_doc::html::parse("<p>A paper with no figures.</p>"),
            Memory::default(),
        );
        assert!(!view.provide_picture("x1.png", plate_bytes()));
        assert!(!view.settle_pictures(&mut context));
    }
}
