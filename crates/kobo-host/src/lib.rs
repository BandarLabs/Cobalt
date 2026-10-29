//! What a computer does for a Kobo owner, without saying it.
//!
//! The `kobo` command finds a reader, installs Cobalt on it, connects accounts
//! and sends files, and it has done all of that in the same functions that
//! print to a terminal and read the answers back. A desktop app cannot use
//! code like that: it has no terminal, and reading another program's output
//! is how two front ends come to disagree. So the operations move here, a
//! piece at a time. This crate returns what happened and never prints or
//! prompts. The `kobo` command keeps every sentence it prints, so its output
//! stays the same while the code under it moves.

pub mod connect;
pub mod error;
pub mod transport;
