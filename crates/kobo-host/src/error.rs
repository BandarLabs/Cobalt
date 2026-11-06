//! What kind of failure an error is, carried in the error itself.
//!
//! Errors have been plain strings since the first command. A prefix says which
//! kind each one is, so the kind travels from the code that knows what went
//! wrong to whatever reports it. The `kobo` command turns the kind into an exit
//! status, and a desktop app turns it into what to show. Both read it with
//! [`kind_of`], and the person reads the message through [`display`], with
//! the tag removed.

/// What kind of failure an error string describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// The request itself was wrong: an unknown command or option, a missing
    /// argument, or two options that cannot both hold.
    Usage,
    /// The reader or simulator to act on could not be reached, found, or
    /// chosen without guessing.
    Target,
    /// The request is well formed, but this build, computer, or reader cannot
    /// do it.
    Unsupported,
    /// Anything no narrower kind describes.
    Failure,
}

/// Prefix an error string carries when the request itself was wrong.
pub const USAGE_PREFIX: &str = "usage: ";
/// The same, for a reader that could not be reached or chosen.
pub const TARGET_PREFIX: &str = "target: ";
/// The same, for something this build or computer cannot do.
pub const UNSUPPORTED_PREFIX: &str = "unsupported: ";

/// Tags an error as a wrong request: the fix is in how it was asked.
#[must_use]
pub fn usage(message: impl Into<String>) -> String {
    format!("{USAGE_PREFIX}{}", message.into())
}

/// Tags an error as a target problem: the reader or simulator to act on
/// could not be reached, found, or chosen without guessing.
#[must_use]
pub fn target(message: impl Into<String>) -> String {
    format!("{TARGET_PREFIX}{}", message.into())
}

/// Tags an error as beyond this build or computer: the request is well
/// formed and still cannot be done here.
#[must_use]
pub fn unsupported(message: impl Into<String>) -> String {
    format!("{UNSUPPORTED_PREFIX}{}", message.into())
}

/// Reads the kind an error was tagged with.
///
/// Usage errors predate the tags: their messages have always started with
/// "usage:" and unknown commands with "unknown command", so both read as
/// [`Kind::Usage`] without every usage line being rewritten at once.
#[must_use]
pub fn kind_of(error: &str) -> Kind {
    if error.starts_with(USAGE_PREFIX) || error.starts_with("unknown command") {
        Kind::Usage
    } else if error.starts_with(TARGET_PREFIX) {
        Kind::Target
    } else if error.starts_with(UNSUPPORTED_PREFIX) {
        Kind::Unsupported
    } else {
        Kind::Failure
    }
}

/// The error as the person should read it, with its kind's tag removed.
///
/// A usage error keeps its prefix, because "usage:" was always part of the
/// sentence a person read rather than a tag added to it.
#[must_use]
pub fn display(error: &str) -> &str {
    for prefix in [TARGET_PREFIX, UNSUPPORTED_PREFIX] {
        if let Some(rest) = error.strip_prefix(prefix) {
            return rest;
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::{display, kind_of, target, unsupported, usage, Kind};

    #[test]
    fn every_tag_reads_back_as_its_kind_and_untagged_errors_are_failures() {
        assert_eq!(kind_of(&usage("kobo wait --device <host>")), Kind::Usage);
        assert_eq!(kind_of("unknown command 'nonsense'"), Kind::Usage);
        assert_eq!(kind_of(&target("no reader answered")), Kind::Target);
        assert_eq!(kind_of(&unsupported("not compiled in")), Kind::Unsupported);
        assert_eq!(kind_of("disk full"), Kind::Failure);
    }

    #[test]
    fn a_person_reads_the_message_without_its_tag_except_for_usage() {
        assert_eq!(display(&target("no reader answered")), "no reader answered");
        assert_eq!(display(&unsupported("not compiled in")), "not compiled in");
        assert_eq!(display("usage: stays visible"), "usage: stays visible");
        assert_eq!(display("disk full"), "disk full");
    }
}
