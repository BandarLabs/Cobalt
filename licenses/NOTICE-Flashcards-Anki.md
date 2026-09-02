# Flashcards compatibility notice

Flashcards is an unofficial, offline compatibility feature for Anki package
files. It is not affiliated with, endorsed by, or sponsored by Ankitects,
Anki, AnkiDroid, or AnkiWeb. It does not include or use their logos,
trademarks, or artwork.

## Upstream sources examined

The host importer was developed against these shallow, pinned source
acquisitions, kept outside the Cobalt repository:

| Project | Revision | Licence |
| --- | --- | --- |
| Anki | `9e32ad8849068510a82273889c21b22e1acf0949` | AGPL-3.0-or-later |
| AnkiDroid | `20107044ee1934ffa7479ef969e453eb51f436f0` | GPL-3.0-or-later |

The pinned clones are retained outside the Cobalt source tree as session
artifacts. Anki's root `LICENSE` also says that its logo is copyright Alex
Fraser; no logo is used here. AnkiDroid's root `COPYING` is GPL version 3 and
its source headers identify GPL-3.0-or-later.

No upstream source file is copied or vendored in Cobalt. The importer is
independently implemented and retains this notice because its package
interoperability, validation cases, and documented boundaries were researched
against those pinned projects. Cobalt remains licensed under AGPL-3.0-only.
This engineering record is not legal advice.

## Distribution

Keep this notice with all distributions containing Flashcards source, the
host `flashcards-import` program, Flashcards documentation, or the Kobo
application package. The host command prints it with `--notice`; the device
application source and Store entry state the same non-affiliation.
