# Fuzz targets

The runtime runs as root and reads what strangers' servers send: JSON, XML,
HTML, gzip, zip and EPUB, URLs, OPDS and OPML, and the wire protocol every
application speaks to it. All of those readers are written in this workspace
with `unsafe` forbidden, so a malformed input cannot corrupt memory. It can
still panic, and release builds abort on panic, so a panic in any of them ends
the runtime and the owner waits for the watchdog.

Each target here feeds one reader arbitrary bytes and asserts the property its
documentation claims: it answers or refuses, it never panics, and where it
writes something back out, what it wrote reads back as itself.

```sh
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run json_parse -- -max_total_time=60
```

`cargo fuzz` needs a nightly compiler for the sanitizer. The root
`rust-toolchain.toml` pins stable for the device workspace, so name nightly
on the command line as above. A crashing input is written to `artifacts/`, and
`cargo +nightly fuzz fmt <target> <file>` shows it as the target saw it.

The Fuzz workflow runs every target for a bounded time on a schedule and keeps
any crashing input as a run artifact.
