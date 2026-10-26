# CLI-14 - detect suitable target apps and resolve ambiguity

detect.rs names the companion a file belongs to: extension first (photos ->
Frame, .opml -> Feeds, .cbz -> Panels, story files -> Parser, .apkg/.colpkg
-> Flashcards), magic bytes when the name says nothing (PNG/JPEG/GIF/BMP ->
Frame, SQLite -> Flashcards, Glulx -> Parser). Resolution rules, all
category-tagged: nothing known is unsupported (exit 4), an --app that does
not fit the file is usage (exit 2), and a container more than one companion
reads asks - numbered picker on a terminal, --app required on a pipe -
rather than picking the first. Decks are a two-step import, not a push, so
send prints their real commands instead of pretending.

Evidence: transcript.txt - real sends into the simulator shelves: an OPML
list staged for Feeds, a real 1x1 PNG transferred and verified by Frame, a
real CBZ packaged by Panels; then the refusal matrix: extensionless zip
(usage on a pipe, numbered picker on a PTY, --app settles it), photo --app
feeds (usage), plain text (unsupported), missing file (usage), no target
(usage). Tests: detect::tests::* (extension routing, magic fallback,
unsupported-not-a-guess, --app must fit, several-ask).
