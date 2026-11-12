//! What the runtime tells the launcher about notices, and how it travels.
//!
//! The runtime keeps the list; the launcher only shows it. Everything here is
//! on tags of its own inside protocol 18, so no existing frame changes shape.
//! See [`crate::NOTICES_VERSION`] for why that is safe in both directions.

use super::{glyph_at, glyph_tag, push_string, push_u64, Glyph, ProtocolError, Reader};

/// The most notices the runtime keeps, and so the most one list carries.
///
/// A hundred is months of clipped articles. Past that the oldest are not
/// news any more, and a list the owner pages through at the speed of e-ink
/// should stay short enough to reach its end.
pub const MAX_NOTICES: usize = 100;
/// The longest heading, in bytes. A heading is two or three words.
pub const MAX_NOTICE_HEADING_LEN: usize = 64;
/// The longest title, in bytes: the longest article headline worth keeping
/// whole, which a row clamps to two lines anyway.
pub const MAX_NOTICE_TITLE_LEN: usize = 256;
/// The longest detail, in bytes: a site name or an application's name.
pub const MAX_NOTICE_DETAIL_LEN: usize = 128;

/// What tapping a notice would open.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NoticeTarget {
    /// Nothing: the notice is the whole of the news.
    #[default]
    Nothing,
    /// Something still on the reader, opened by [`crate::DeviceRequest::OpenNotice`].
    Ready,
    /// Something that was on the reader and is not any more. Said plainly in
    /// the list rather than offered and then failed.
    Gone,
}

/// One entry in the runtime's list of notices.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Notice {
    /// Stable for as long as the notice is kept; never reused.
    pub id: u64,
    /// What happened, in the runtime's words: "Article arrived".
    pub heading: String,
    /// What it happened to: the article's title.
    pub title: String,
    /// Where it came from, such as a site's name. May be empty.
    pub detail: String,
    /// The mark of the application the notice belongs to.
    pub glyph: Glyph,
    /// When it arrived, in milliseconds since 1970.
    pub at: u64,
    /// Whether the owner has not yet opened the list since it arrived.
    pub unread: bool,
    pub target: NoticeTarget,
}

/// The whole list, newest first, as the runtime holds it right now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoticeList {
    pub entries: Vec<Notice>,
    /// Whether the owner asked to see the list, by tapping a notice that
    /// stood for several. The launcher shows it rather than its home screen.
    pub open: bool,
}

impl NoticeList {
    /// How many entries the owner has not seen in the list yet.
    #[must_use]
    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|notice| notice.unread).count()
    }
}

fn valid_text(text: &str, limit: usize, required: bool) -> bool {
    (!required || !text.trim().is_empty())
        && text.len() <= limit
        && !text.chars().any(char::is_control)
}

fn valid(notice: &Notice) -> bool {
    valid_text(&notice.heading, MAX_NOTICE_HEADING_LEN, true)
        && valid_text(&notice.title, MAX_NOTICE_TITLE_LEN, true)
        && valid_text(&notice.detail, MAX_NOTICE_DETAIL_LEN, false)
}

pub(super) fn push_list(
    output: &mut Vec<u8>,
    list: &NoticeList,
    version: u8,
) -> Result<(), ProtocolError> {
    if list.entries.len() > MAX_NOTICES {
        return Err(ProtocolError::InvalidValue("notice count"));
    }
    output.push(u8::from(list.open));
    output.push(u8::try_from(list.entries.len()).map_err(|_| ProtocolError::TooManyNodes)?);
    for notice in &list.entries {
        if !valid(notice) {
            return Err(ProtocolError::InvalidValue("notice"));
        }
        push_u64(output, notice.id);
        push_string(output, &notice.heading)?;
        push_string(output, &notice.title)?;
        push_string(output, &notice.detail)?;
        output.push(glyph_tag(notice.glyph, version));
        push_u64(output, notice.at);
        output.push(u8::from(notice.unread));
        output.push(match notice.target {
            NoticeTarget::Nothing => 0,
            NoticeTarget::Ready => 1,
            NoticeTarget::Gone => 2,
        });
    }
    Ok(())
}

fn flag(reader: &mut Reader<'_>, what: &'static str) -> Result<bool, ProtocolError> {
    match reader.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidValue(what)),
    }
}

pub(super) fn read_list(reader: &mut Reader<'_>, version: u8) -> Result<NoticeList, ProtocolError> {
    let open = flag(reader, "notice list")?;
    let count = usize::from(reader.u8()?);
    if count > MAX_NOTICES {
        return Err(ProtocolError::InvalidValue("notice count"));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let notice = Notice {
            id: reader.u64()?,
            heading: reader.string()?,
            title: reader.string()?,
            detail: reader.string()?,
            glyph: glyph_at(reader.u8()?, version).ok_or(ProtocolError::InvalidValue("glyph"))?,
            at: reader.u64()?,
            unread: flag(reader, "notice")?,
            target: match reader.u8()? {
                0 => NoticeTarget::Nothing,
                1 => NoticeTarget::Ready,
                2 => NoticeTarget::Gone,
                _ => return Err(ProtocolError::InvalidValue("notice target")),
            },
        };
        if !valid(&notice) {
            return Err(ProtocolError::InvalidValue("notice"));
        }
        entries.push(notice);
    }
    Ok(NoticeList { entries, open })
}
