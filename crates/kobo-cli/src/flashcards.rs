//! Host-side guard for Flashcards imports.
//!
//! Anki packages are not a two-field text format. A correct importer must use
//! Anki's renderer and media importer so that templates, clozes, media and
//! scheduling are not silently changed. Until that host-only integration is
//! available, refuse packages instead of producing a plausible but wrong
//! device bundle.

use std::path::Path;

const UNSUPPORTED: &str = "Flashcards APKG/COLPKG import is unavailable: Cobalt does not yet \
use Anki's rslib renderer and media importer. No bundle was written, so templates, clozes, \
media, deck options, and review history were not discarded. Keep using Anki for this collection.";

pub fn import(_input: &Path, _output: &Path) -> Result<(), String> {
    Err(UNSUPPORTED.to_owned())
}

const USAGE: &str = "usage: kobo flashcards import DECK.apkg --out DECK.flashcards\n\
                     Host-only. Anki APKG/COLPKG import is refused until Cobalt uses Anki's\n\
                     rslib renderer. No bundle is written, so templates and media are not lost.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let [verb, input, flag, output] = arguments else {
        return Err(USAGE.to_owned());
    };
    if verb != "import" || flag != "--out" {
        return Err(USAGE.to_owned());
    }
    import(Path::new(input), Path::new(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_doc::zip::stored;
    use rusqlite::Connection;

    fn fixture() -> Vec<u8> {
        let database = Connection::open_in_memory().expect("fixture collection");
        database
            .execute_batch(
                r#"CREATE TABLE col (
                    id integer primary key, crt integer, mod integer, scm integer, ver integer,
                    dty integer, usn integer, ls integer, conf text, models text, decks text,
                    dconf text, tags text
                );
                CREATE TABLE notes (
                    id integer primary key, guid text, mid integer, mod integer, usn integer,
                    tags text, flds text, sfld integer, csum integer, flags integer, data text
                );
                CREATE TABLE cards (
                    id integer primary key, nid integer, did integer, ord integer, mod integer,
                    usn integer, type integer, queue integer, due integer, ivl integer,
                    factor integer, reps integer, lapses integer, left integer, odue integer,
                    odid integer, flags integer, data text
                );
                CREATE TABLE revlog (
                    id integer primary key, cid integer, usn integer, ease integer, ivl integer,
                    lastIvl integer, factor integer, time integer, type integer
                );
                INSERT INTO col VALUES (
                    1, 0, 0, 0, 11, 0, 0, 0, '{}',
                    '{"1":{"name":"Basic","flds":[{"name":"Front"},{"name":"Back"}],
                      "tmpls":[{"name":"Card 1","qfmt":"{{Front}}","afmt":"{{FrontSide}}<hr id=answer>{{Back}}"}],
                      "css":"body { color: red; }"},
                     "2":{"name":"Cloze","type":1,"flds":[{"name":"Text"},{"name":"Extra"}],
                      "tmpls":[{"name":"Cloze","qfmt":"{{cloze:Text}}","afmt":"{{cloze:Text}}<br>{{Extra}}"}],
                      "css":".cloze { font-weight: bold; }"}}',
                    '{"1":{"name":"Language::日本語"}}', '{}', '{}'
                );
                INSERT INTO notes VALUES
                    (10, 'basic', 1, 0, 0, '', '<b>hello</b>\u001fworld <img src="photo.png"> [sound:voice.mp3]', 0, 0, 0, ''),
                    (11, 'cloze', 2, 0, 0, '', '{{c1::京都}} is in Japan\u001fUnicode', 0, 0, 0, '');
                INSERT INTO cards VALUES
                    (20, 10, 1, 0, 0, 0, 2, 2, 42, 10, 2500, 4, 1, 0, 0, 0, 0, ''),
                    (21, 11, 1, 0, 0, 0, 2, 2, 42, 10, 2500, 4, 1, 0, 0, 0, 0, '');
                INSERT INTO revlog VALUES (30, 20, 0, 3, 10, 5, 2500, 100, 1);"#,
            )
            .expect("fixture schema");
        stored(&[
            (
                "collection.anki2".to_owned(),
                database
                    .serialize("main")
                    .expect("fixture database")
                    .to_vec(),
            ),
            (
                "media".to_owned(),
                br#"{"0":"photo.png","1":"voice.mp3","2":"../escape.png"}"#.to_vec(),
            ),
            ("0".to_owned(), b"\x89PNG\r\nfixture".to_vec()),
            ("1".to_owned(), b"audio fixture".to_vec()),
            ("2".to_owned(), b"must not escape".to_vec()),
        ])
        .expect("fixture archive")
    }

    #[test]
    fn apkg_fixture_is_rejected_without_a_lossy_or_partial_bundle() {
        let base = std::env::current_dir()
            .expect("workspace")
            .join(format!("flashcards-compat-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("fixture directory");
        let input = base.join("compatibility.apkg");
        let output = base.join("compatibility.flashcards");
        std::fs::write(&input, fixture()).expect("fixture");

        let error = import(&input, &output).expect_err("must fail closed");
        assert_eq!(error, UNSUPPORTED);
        assert!(!output.exists(), "a failed import must be atomic");

        std::fs::remove_dir_all(base).expect("cleanup fixture");
    }

    #[test]
    fn help_succeeds() {
        command(&["--help".into()]).expect("help");
        command(&[]).expect("empty is help");
    }
}
