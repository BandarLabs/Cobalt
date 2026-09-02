# Developing Cobalt

How to run the workspace, the simulator, and the loop that drives an
application and photographs what it drew. Part of [Cobalt](../README.md).

```sh
cargo test --workspace --all-features
cargo run -p kobo-cli -- dev --builtin      # browser simulator
cargo run -p kobo-cli -- run --sim          # the real runtime, host socket
cargo run -p kobo-cli -- run --sim --app rss   # ... pointed at one application
```

`run --sim` starts the real `kobod`, runs one application against it over a
host socket and saves what it drew to `target/kobo-sim-last.raw`: 1072 × 1448
bytes of eight-bit grey, one per pixel, which `P5` PGM will open directly.
`--app` takes any shipped application by the name the launcher uses or the name
cargo uses. It is the shortest way to see what a screen really looks like
without a reader in front of you.

The simulator binds only to `127.0.0.1` and currently targets the measured Kobo
Clara BW 391 profile: 1072 × 1448 at 300 PPI, including its rotated raw touch
coordinates. It uses the same renderer, layout engine, policy, typeface and
panel refresh planner as the device, so a screen that fits in the browser fits
on the panel and the reported changed rectangle, waveform and clean-refresh
cadence cannot drift from the runtime. The inspector can compare ideal pixels
with a clearly labelled approximation of E Ink residue and outline the next
refresh region.

Network requests and terminals are real. The inspector's deterministic
scenarios exercise offline, low-battery, denied-permission, missing-secret,
timeout, full-storage and image-cache-pressure paths; it can also deliver
foreground and background lifecycle events. Its layout panel reports text,
touch-target and picture diagnostics with optional outlines over the exact
failing rectangles. Run with `KOBO_TEXT_SCALE=large` or
`KOBO_TEXT_SCALE=extra-large` to verify the 120% and 140% accessibility settings
with the same metrics used for pagination.

## Driving it, and photographing the result

A layout assertion proves a button was placed. It does not prove the screen
reads as a product, and it does not prove the button is reachable. Closing that
loop, for a person or for something automating on their behalf, means
driving the application the way a finger does and then looking at the result.

```sh
cargo run -p kobo-cli -- dev 127.0.0.1:8787          # in one terminal
cargo run -p kobo-cli -- drive --script tour.kobo --shots target/shots
cargo run -p kobo-cli -- drive --step "tap Search" --step "expect Results"
```

A script is one step per line: `tap LABEL`, `tap-at X,Y`, `type TEXT`,
`expect TEXT`, `expect-missing TEXT`, `wait-for TEXT`, `clean`, `shot NAME`,
`dump`, `scenario NAME`, `lifecycle background`, `wait MS`. A failing step
reports the line and the reason and screenshots the panel first. Add `--ideal`
to take screenshots without the panel's e-ink residue, which is what you want
when a person or a model is going to read them.

`tap` resolves the label against the layout the renderer produced and then taps
the coordinate, through the panel's own touch transform and the renderer's own
hit-testing. Dispatching the action directly would have been simpler and
worthless: it passes on a screen whose only button has been laid out below the
bottom edge, which is the fault worth catching.

For the real panel:

```sh
cargo run -p kobo-cli -- shot --device <address> --out screen.png
cargo run -p kobo-cli --features device-write -- tap --device <address> 536,900
```

`shot --device` is read-only. It opens the framebuffer for reading and never
grabs, refreshes or writes, so it is safe against a device with the stock
reader in the foreground. `tap --device` writes real evdev records to the real
touch node, so the digitiser, the transform, the multitouch decoder and the
hit-testing all run as they do under a finger; it is behind `device-write` and
an unlock phrase, and it always lifts.

To record the panel rather than photograph it once:

```sh
cargo run -p kobo-cli -- record --device <address> --seconds 24 --out target/run
```

`record --device` is read-only in the same way `shot --device` is. It writes
numbered greyscale PNGs and a `timings.txt`, plus an `recording.mp4` when
ffmpeg is on the path. Every grey level is kept: the panel is greyscale and its
text is anti-aliased, so a recording that flattened the greys would look
harsher than the device and would read as a rendering bug that is not there.
What keeps it small is that e-ink barely moves, so identical frames are dropped
and only the changes are stored.

`scripts/record-apps.sh --device <address>` drives every example application
through a short tap sequence and records each one, into a dated directory under
`target/device-test/`. It is a script rather than a list of commands because
the interesting failures are the ones that appear between two runs, and
comparing runs is only possible if both were driven the same way.

Create and run a new application:

```sh
cargo build -p kobo-cli
target/debug/kobo new weather
cd weather && ../target/debug/kobo dev
```

Build every device-side program:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo run -p kobo-cli -- build --device
```

The cross-compiler the TLS stack needs is listed under
[What you need](INSTALL.md#what-you-need); `build --device` finds it under any of its
usual names and names the package to install when there is none. Rust code is
linked by `rust-lld`, which ships with the toolchain. The resulting binaries
are statically linked and need no library installed on the reader.

For the owner-attended Clara BW Wi-Fi handoff investigation, use the
[bounded passive handoff trace](WIFI_HANDOFF_TRACE.md). It is opt-in, is not a
fix, and includes the exact build, run, wait, and retrieval procedure.

## Before you commit a credential by accident

A key must never reach a commit. `tools/pre-commit` refuses one, and is
enabled per clone with:

```sh
git config core.hooksPath tools
```

It scans staged lines for published credential shapes (OpenAI, Anthropic,
GitHub, AWS, Google, Slack), for a PEM private key header, and for a shell
assignment of something named like a key. It reports the shape it matched and
never the match, because printing the key to a terminal or a CI log is the
thing being prevented.
