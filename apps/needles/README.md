# Needles

Needles is an unofficial companion for your Ravelry library. It reads only
your account through Ravelry's official API; install the HTTP Basic credential
with `kobo secret set ravelry`. The secret is named in runtime tasks and is
never available to the application.

The row counter autosaves every tap and works offline after a pattern sync.
Text pages are intended for the `kobo needles sync` host pipeline on port
9341: its PDF inspection produces reflowable text where possible, while
charts and scans remain image pages. PDF conversion and project-note postback
are host-pipeline work not yet available in this device-only MVP.

“Ravelry” is used nominatively. This app is not affiliated with Ravelry.

```sh
cargo test -p kobo-needles
kobo run --sim --app needles
```
