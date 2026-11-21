//! Putting a book together from the ranges the runtime hands over.
//!
//! A book up to four megabytes arrives in one answer. A larger one cannot,
//! because the answer has to fit a frame, so it is asked for a range at a
//! time and assembled here. Each answer is a callback of its own that copies
//! one piece and asks for the next, so however large the book is, no callback
//! does more than a quarter of a megabyte of work and the panel stays alive
//! between them.
//!
//! The runtime holds nothing between two ranges, so nothing stops the file
//! being replaced half-way through: a sync delivers a newer copy, or the same
//! name is given to another book. Every answer says how long the file is and
//! carries a stamp that changes when the file is written, and an answer that
//! disagrees with the first one ends the assembly rather than splicing two
//! books into something that might even parse.

use kobo_sdk::{Context, DeviceRequest, DeviceResult, MAX_LIBRARY_RANGE_BYTES};

/// One book on its way in.
#[derive(Debug)]
pub struct Assembly {
    id: String,
    bytes: Vec<u8>,
    /// The most this assembly will hold, whatever the file turns out to be.
    limit: u32,
    /// The length and stamp the first range gave, which every later one has
    /// to repeat.
    file: Option<(u32, u64)>,
}

/// Where an assembly stands after one answer.
#[derive(Debug, Eq, PartialEq)]
pub enum Arrival {
    /// The answer was not this assembly's: for another book, or for a range
    /// this one is no longer waiting on.
    Elsewhere,
    /// A piece landed and the next has been asked for.
    Moving {
        done: u32,
        total: u32,
    },
    /// Every byte is in hand. Take them with [`Assembly::take`].
    Done,
    Failed(Fault),
}

/// Why a book could not be put together.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// The file is this long, which is more than the assembly may hold. The
    /// listing can be stale, so the length the runtime gives with the first
    /// range is the one that counts.
    TooLarge(u32),
    /// The file was written again between two ranges.
    Changed,
    /// The runtime would not or could not read it.
    Unreadable,
}

impl Assembly {
    /// Starts asking for `id`, which is refused once it is known to be
    /// longer than `limit`.
    pub fn begin(context: &mut Context, id: impl Into<String>, limit: u32) -> Self {
        let assembly = Self {
            id: id.into(),
            bytes: Vec::new(),
            limit,
            file: None,
        };
        assembly.ask(context);
        assembly
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The bytes in hand so far, and the whole length once it is known.
    pub fn progress(&self) -> (u32, Option<u32>) {
        (self.held(), self.file.map(|(size, _)| size))
    }

    /// The book, once [`Arrival::Done`] has been returned.
    pub fn take(self) -> Vec<u8> {
        self.bytes
    }

    /// Feeds the assembly one answer, with the request it answers.
    ///
    /// The request is what says whether the answer is wanted. An assembly
    /// that was abandoned and begun again still has answers on their way to
    /// its first attempt, and one of those is for a range this attempt has
    /// already passed or not reached yet. Such an answer is let go without
    /// asking for anything, so the attempt it belonged to ends there.
    pub fn advance(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> Arrival {
        let DeviceRequest::ReadLibraryRange { id, offset, .. } = request else {
            return Arrival::Elsewhere;
        };
        if *id != self.id || *offset != self.held() {
            return Arrival::Elsewhere;
        }
        let DeviceResult::LibraryRange {
            id,
            offset,
            size,
            stamp,
            bytes,
        } = result
        else {
            return Arrival::Failed(Fault::Unreadable);
        };
        // An answer about another book, or another part of this one, than
        // the request it is paired with. Copying it in would make a file
        // that parses and is not the file.
        if *id != self.id || *offset != self.held() || bytes.len() > MAX_LIBRARY_RANGE_BYTES {
            return Arrival::Failed(Fault::Unreadable);
        }
        match self.file {
            Some(first) if first != (*size, *stamp) => return Arrival::Failed(Fault::Changed),
            Some(_) => {}
            None => {
                if *size > self.limit {
                    return Arrival::Failed(Fault::TooLarge(*size));
                }
                // Asked for once and exactly, so the buffer never doubles
                // its way past the book while the book is arriving. A reader
                // without that much to give says so here rather than being
                // stopped by the kernel thirty megabytes in.
                let whole = usize::try_from(*size).unwrap_or(usize::MAX);
                if self.bytes.try_reserve_exact(whole).is_err() {
                    return Arrival::Failed(Fault::TooLarge(*size));
                }
                self.file = Some((*size, *stamp));
            }
        }
        let left = usize::try_from(size.saturating_sub(self.held())).unwrap_or(usize::MAX);
        if bytes.len() > left {
            return Arrival::Failed(Fault::Unreadable);
        }
        self.bytes.extend_from_slice(bytes);
        let done = self.held();
        if done == *size {
            return Arrival::Done;
        }
        // Nothing came and the file says there is more. Asking again would
        // ask for the same nothing for as long as the owner cared to wait.
        if bytes.is_empty() {
            return Arrival::Failed(Fault::Unreadable);
        }
        self.ask(context);
        Arrival::Moving { done, total: *size }
    }

    fn held(&self) -> u32 {
        u32::try_from(self.bytes.len()).unwrap_or(u32::MAX)
    }

    fn ask(&self, context: &mut Context) {
        let piece = u32::try_from(MAX_LIBRARY_RANGE_BYTES).unwrap_or(u32::MAX);
        context
            .device()
            .read_library_range(self.id.clone(), self.held(), piece);
    }
}

#[cfg(test)]
pub mod tests {
    use super::{Arrival, Assembly, Fault};
    use kobo_sdk::{Context, DeviceError, DeviceRequest, DeviceResult, MAX_LIBRARY_RANGE_BYTES};

    const ID: &str = "0/Atlas of the Coast.epub";
    const PIECE: usize = MAX_LIBRARY_RANGE_BYTES;

    fn a_file_of(length: usize) -> Vec<u8> {
        (0..length)
            .map(|at| u8::try_from((at * 7 + at / 253) % 253).expect("under 253"))
            .collect()
    }

    /// The ranges an application asked for and has not been answered, in
    /// order, as the runtime would see them.
    fn asked(context: &mut Context) -> Vec<DeviceRequest> {
        context
            .take_commands()
            .into_iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::Device(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// What the runtime answers a range request with, for a file that is
    /// `file` and was last written at `stamp`.
    pub fn answer(request: &DeviceRequest, file: &[u8], stamp: u64) -> DeviceResult {
        let DeviceRequest::ReadLibraryRange { id, offset, length } = request else {
            panic!("not a range request: {request:?}");
        };
        let from = usize::try_from(*offset).expect("an offset").min(file.len());
        let to = from
            .saturating_add(usize::try_from(*length).expect("a length"))
            .min(file.len());
        DeviceResult::LibraryRange {
            id: id.clone(),
            offset: *offset,
            size: u32::try_from(file.len()).expect("a small file"),
            stamp,
            bytes: file[from..to].to_vec(),
        }
    }

    #[test]
    fn a_book_arrives_a_piece_at_a_time_and_comes_out_whole() {
        let book = a_file_of(2 * PIECE + 4_321);
        let mut context = Context::default();
        let mut assembly = Assembly::begin(&mut context, ID, 32 * 1024 * 1024);
        let mut steps = Vec::new();
        let mut offsets = Vec::new();
        loop {
            let requests = asked(&mut context);
            assert_eq!(requests.len(), 1, "one range is asked for at a time");
            let DeviceRequest::ReadLibraryRange { offset, length, .. } = &requests[0] else {
                panic!("not a range request");
            };
            offsets.push(*offset);
            assert_eq!(usize::try_from(*length).ok(), Some(PIECE));
            let step =
                assembly.advance(&mut context, &requests[0], &answer(&requests[0], &book, 9));
            let done = step == Arrival::Done;
            steps.push(step);
            if done {
                break;
            }
        }
        let total = u32::try_from(book.len()).expect("a small file");
        let piece = u32::try_from(PIECE).expect("a piece");
        assert_eq!(offsets, [0, piece, 2 * piece]);
        assert_eq!(
            steps,
            [
                Arrival::Moving { done: piece, total },
                Arrival::Moving {
                    done: 2 * piece,
                    total
                },
                Arrival::Done
            ]
        );
        assert_eq!(assembly.progress(), (total, Some(total)));
        assert!(asked(&mut context).is_empty(), "asked on past the end");
        assert_eq!(assembly.take(), book);
    }

    #[test]
    fn a_book_that_fits_one_piece_is_done_at_the_first_answer() {
        let book = a_file_of(900);
        let mut context = Context::default();
        let mut assembly = Assembly::begin(&mut context, ID, 4_096);
        let request = asked(&mut context).remove(0);
        assert_eq!(
            assembly.advance(&mut context, &request, &answer(&request, &book, 1)),
            Arrival::Done
        );
        assert_eq!(assembly.take(), book);
    }

    #[test]
    fn a_file_longer_than_the_limit_is_refused_at_its_first_range_with_its_length() {
        let book = a_file_of(PIECE + 1);
        let mut context = Context::default();
        let limit = u32::try_from(PIECE).expect("a piece");
        let mut assembly = Assembly::begin(&mut context, ID, limit);
        let request = asked(&mut context).remove(0);
        assert_eq!(
            assembly.advance(&mut context, &request, &answer(&request, &book, 1)),
            Arrival::Failed(Fault::TooLarge(limit + 1))
        );
        assert!(
            asked(&mut context).is_empty(),
            "a book that will not be opened was asked for again"
        );
    }

    #[test]
    fn a_file_replaced_between_two_ranges_is_not_spliced_together() {
        let first = a_file_of(3 * PIECE);
        for (replacement, stamp, what) in [
            (a_file_of(PIECE + 10), 2, "a shorter copy"),
            (a_file_of(5 * PIECE), 2, "a longer copy"),
            // The same length to the byte, told apart only by its stamp.
            (first.iter().rev().copied().collect(), 2, "another book"),
        ] {
            let mut context = Context::default();
            let mut assembly = Assembly::begin(&mut context, ID, 32 * 1024 * 1024);
            let request = asked(&mut context).remove(0);
            assert!(matches!(
                assembly.advance(&mut context, &request, &answer(&request, &first, 1)),
                Arrival::Moving { .. }
            ));
            let request = asked(&mut context).remove(0);
            assert_eq!(
                assembly.advance(
                    &mut context,
                    &request,
                    &answer(&request, &replacement, stamp)
                ),
                Arrival::Failed(Fault::Changed),
                "{what} was taken for more of the first"
            );
            assert!(asked(&mut context).is_empty(), "{what}: asked on");
        }
    }

    #[test]
    fn a_refusal_or_an_answer_that_is_not_the_range_asked_for_ends_the_assembly() {
        type Reply<'a> = &'a dyn Fn(&DeviceRequest) -> DeviceResult;
        let book = a_file_of(2 * PIECE);
        let wrong_offset = |request: &DeviceRequest| {
            let mut shifted = request.clone();
            if let DeviceRequest::ReadLibraryRange { offset, .. } = &mut shifted {
                *offset += 1;
            }
            answer(&shifted, &book, 1)
        };
        let other_book = |request: &DeviceRequest| {
            let mut other = request.clone();
            if let DeviceRequest::ReadLibraryRange { id, .. } = &mut other {
                *id = "0/Another.epub".to_owned();
            }
            answer(&other, &book, 1)
        };
        // The file says there is more and the piece is empty.
        let nothing = |request: &DeviceRequest| {
            let DeviceResult::LibraryRange {
                id,
                offset,
                size,
                stamp,
                ..
            } = answer(request, &book, 1)
            else {
                unreachable!("a range answer");
            };
            DeviceResult::LibraryRange {
                id,
                offset,
                size,
                stamp,
                bytes: Vec::new(),
            }
        };
        let answers: [(&str, Reply<'_>); 6] = [
            ("not found", &|_| {
                DeviceResult::Failed(DeviceError::NotFound)
            }),
            ("backend", &|_| DeviceResult::Failed(DeviceError::Backend)),
            ("a whole document", &|_| DeviceResult::LibraryDocument {
                id: ID.to_owned(),
                bytes: vec![1],
            }),
            ("another offset", &wrong_offset),
            ("another book", &other_book),
            ("an empty piece", &nothing),
        ];
        for (what, reply) in answers {
            let mut context = Context::default();
            let mut assembly = Assembly::begin(&mut context, ID, 32 * 1024 * 1024);
            let request = asked(&mut context).remove(0);
            assert_eq!(
                assembly.advance(&mut context, &request, &reply(&request)),
                Arrival::Failed(Fault::Unreadable),
                "{what}"
            );
            assert!(asked(&mut context).is_empty(), "{what}: asked on");
        }
    }

    #[test]
    fn an_answer_to_an_attempt_that_was_given_up_is_let_go_without_asking_again() {
        let book = a_file_of(3 * PIECE);
        let mut context = Context::default();
        // The first attempt got one piece in and was abandoned with its
        // second range still on the way.
        let mut first = Assembly::begin(&mut context, ID, 32 * 1024 * 1024);
        let request = asked(&mut context).remove(0);
        first.advance(&mut context, &request, &answer(&request, &book, 1));
        let stale = asked(&mut context).remove(0);
        drop(first);

        let mut second = Assembly::begin(&mut context, ID, 32 * 1024 * 1024);
        let fresh = asked(&mut context).remove(0);
        assert_eq!(
            second.advance(&mut context, &stale, &answer(&stale, &book, 1)),
            Arrival::Elsewhere
        );
        assert!(asked(&mut context).is_empty(), "the stale answer asked on");
        assert_eq!(second.progress(), (0, None));
        // Its own answer still counts, and so does one for another book.
        let elsewhere = DeviceRequest::ReadLibraryRange {
            id: "0/Another.epub".to_owned(),
            offset: 0,
            length: 1,
        };
        assert_eq!(
            second.advance(&mut context, &elsewhere, &answer(&elsewhere, &book, 1)),
            Arrival::Elsewhere
        );
        assert!(matches!(
            second.advance(&mut context, &fresh, &answer(&fresh, &book, 1)),
            Arrival::Moving { .. }
        ));
    }
}
