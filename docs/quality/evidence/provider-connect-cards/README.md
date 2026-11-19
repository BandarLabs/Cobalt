# Provider connect cards

`kobo apps connect APP` renders the safest connection path from the exact
bundled Store setup metadata. For Lichess it labels the official browser URL,
then the command whose token comes from a private file. It prints no token. For
Panels, whose Komga account form belongs on the reader, it says no computer-side
sign-in is declared and points to the full setup guide instead of inventing a
host flow.

This gives every app one Connect entry point while preserving each provider's
actual authentication shape: browser-assisted where the manifest has an
official link, file-backed tokens where it declares a secret command, and
on-reader setup otherwise. Three app-guide tests and strict all-target CLI
Clippy pass.
