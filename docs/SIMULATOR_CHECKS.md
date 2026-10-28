# Repeatable catalog simulator checks

From the repository root, run:

```sh
python3 scripts/check-apps-sim.py --out target/sim-check
```

The runner builds the CLI, launches every catalog app with a fresh temporary
store, and runs each app's committed drive route when one exists. It fixes the
Inkling date and supplies local fixtures for apps that need them. A failed
launch or route returns a nonzero exit code. The runner terminates its own
simulator processes and removes temporary stores, including after interruption.

To check selected apps:

```sh
python3 scripts/check-apps-sim.py inkling pubquiz --out target/sim-check-games
```

`results.json` records the source commit, whether the checkout was dirty, and
each app's launch and route result. The output directory also contains logs
and route artifacts. CI runs the complete sweep with `--all-routes` (see
Extra routes below) and uploads the evidence.

The current catalog contains 46 apps, and every one has a committed route.
With the extra routes the complete sweep is 65 runs. A successful sweep
verifies the committed simulator scenarios; it does not certify every app
feature, external account integration, or physical Kobo behavior.

The runner requires Python 3.9 or newer, Node.js, and the repository's Rust
toolchain. `CARGO_TARGET_DIR` is supported for sharing an existing build cache.


## Running more than one simulator

Companion commands with `--sim` deliver to `$TMPDIR/cobalt-sim-data/<app>`,
and `kobo dev` reads the same folder. Two simulators running at once share it
and overwrite each other's pushes. Give each one its own folder by setting
`KOBO_SIM_DATA` for both the simulator and its companion commands:

```sh
export KOBO_SIM_DATA="$PWD/target/sim-data-frame"
kobo dev 127.0.0.1:8790 &
kobo frame push ~/Pictures/family --sim
```

`kobo dev` prints the folder it is using when it starts. The same folder
holds the library, each app's saved state (`.state/`) and the simulator's
secrets (`.secrets/`), so a new folder is a fresh simulator. Without
`KOBO_SIM_DATA` these live in `$TMPDIR` as before.

## Local time

The runtime tells every application the reader's offset from UTC in
`KOBO_UTC_OFFSET_MINUTES`, and `kobo_sdk::clock::SystemClock::reader()` reads
it. The simulator passes the computer's own offset, so "today" in an app is
the developer's today. Set the variable before `kobo dev` to pin another zone,
for example `KOBO_UTC_OFFSET_MINUTES=600` to check that an evening check-in in
Sydney lands on the right day. The value is fixed for the life of the app
process, as it is on a reader.

`KOBO_SIM_CLOCK_MILLIS` starts the simulator's clock at a chosen moment, and
an application that reads the time through `SystemClock::reader()` starts
from the same moment, so a run can be placed on any day. The status bar
follows `clock advance` steps; an application's clock does not, so restart
the simulator to move an application to another day.

## Built-in apps and the library

Books, Settings, Terminal, Launcher and Store hold what the runtime gives
them, so they can be checked like any Store app:
`python3 scripts/check-apps-sim.py books settings`. Library apps read
documents from the `library` folder beside the per-app shelves
(`$KOBO_SIM_DATA/library`, or `$TMPDIR/cobalt-sim-data/library`). The sweep
puts a sample EPUB there for Books.

## Route steps for whole flows

Beyond taps and typing, a route can do what an owner does around the app:

- `restart` stops the app and starts it again, so "comes back after a
  restart" can be checked; only what the app saved survives.
- `companion <kobo arguments> --sim` runs a computer-side command between
  steps, for example `companion frame push photo.jpg --sim`, followed by
  `lifecycle background` and `lifecycle foreground` to check the app picks the
  push up. The command must name `--sim`.
- `page next` and `page back` press the reader's page keys.
- `type` turns to the keyboard page a character is on by itself.

## Extra routes

An app's `drive.kobo` runs from the seeded state above. Any
`drive-<flow>.kobo` beside it runs with `--all-routes`, in a fresh simulator,
as its first line asks:

```text
# sweep: --bare
# sweep: --bare --seed-root examples/chat/fixtures/cut-short
# sweep: after-main
# sweep: skip
```

`--bare` skips the default seed, `--seed-root` copies a prepared state in
first, `after-main` runs in the state the main route left behind, and `skip`
keeps a route for manual use. A route with no such line runs like the main
one.

## Live Paperterm session

After building `kobo-cli`, run:

```sh
python3 scripts/quality/check-paperterm-live.py --scale default --output /tmp/paperterm-live
```

This launches the actual SDK app and a real host PTY with an original synthetic
command. It types from the laptop and reader into the same session, checks
wide output, keyboard resizing, Ctrl-C and laptop terminal restoration, and
captures the portrait UI. It seeds pairing in a temporary store; this does not
validate manual onboarding. Use `--profile` and `--scale` for other displays.

The fixture sets `KOBO_STREAM_CONFIG_DIR` and `KOBO_SIM_TRUST_DIR` to private
temporary directories. The latter replaces the simulator's default owner-root
directory (`~/.config/kobo/trust`); ordinary certificate verification remains
active. No owner identity or trust roots are changed. Live long polls remain
outstanding by design, so this route waits for visible content instead of
waiting for all tasks to become idle. See the [portrait session](../apps/paperterm/screenshots/terminal.png).


Add `--pair-on-reader` to enter the private address and code through the on-screen
keyboard instead of seeding the pairing record. This verifies the reader-side
form and its successful save; the fixture still installs trust locally, so it
does not claim hardware trust transfer. The live route also checks uncertain
input, explicit resume, offline reconnect with the keyboard open and closed,
and restored two-way input. The committed `apps/paperterm/drive.kobo` route
separately covers the welcome, offline preview, setup pages and corrected form
errors, asserting zero fetch/post effects throughout.

For pairing storage recovery, add `--pair-on-reader --load-failure --save-failure`.
The fixture makes the private pairing path unreadable, retries the read, injects
a full store, and verifies that reader input still reaches the laptop before
explicitly retrying the save. Network recovery must not silently retry saving.
Use `--pair-on-reader --load-failure --temporary-pairing` instead to verify a
connection that leaves the original pairing path untouched throughout the run.
Each storage route includes the full two-way session and captures recovery UI.

## Logic Pack progress and recovery

After building `kobo-cli`, run:

```sh
python3 scripts/quality/check-logicpack-sim.py --output /tmp/logicpack-check
```

The actual SDK app completes and forcibly reopens all 20 original
puzzles, switches between independent progress records, undoes moves after
reopening, retries a full-store failure, and confirms and undoes a restart.
It also checks first-mine relocation and loss undo, the committed drive route,
both single-game and four-game legacy migrations, direct Kakuro digit entry,
paged collections, difficulty guides and preservation of future-version records. The default is
Clara BW at Extra-large; `--profile` and `--scale` select another supported
profile or text size. Captures include layout and source/font provenance and
assert zero fetch/post effects. The route also checks shared pencil-board nodes
for connected loops, circular islands and attached diagonal sum clues. The original generator separately verifies unique loop, bridge and cross-sum
solutions and deterministic reproduction with `make-logicpack-collection.py --check`.
Mines difficulty describes field size and density; guess-free play is not promised.


## calibre-web catalogs, downloads and offline reading

After building `kobo-cli`, run:

```sh
python3 scripts/quality/check-calibre-sim.py --output /tmp/calibre-check
```

This starts an original OPDS/EPUB fixture on a private, locally trusted HTTPS
server and drives the actual app with isolated storage. It follows author and
shelf links, downloads an EPUB, saves a reading position, kills the app and
reopens offline. It checks failed position/setup saves, explicit retry, damaged
file repair, HTTP account refusals, malformed catalogs and unreadable settings.
The final stage enters a Basic account on the reader and verifies authenticated
catalog, author-section and EPUB requests through the runtime's server binding.
Captures include actual layouts and source/binary/font provenance. No owner
server, credentials or device storage is used.

## Explicit HTTP updates

The simulator handles `Task::Update` through the same policy and transport as
Kobo. Offline, host-down, permission-denied, missing-secret and timeout scenarios
apply to PUT/PATCH as they do to other network tasks. Activity JSON reports
separate `effects.put` and `effects.patch` counters; `effects.post` remains POST
only. All update tasks participate in active-work and callback/idle accounting.
Counters never contain URLs, account names or bodies. Include all four network
counters (`fetch`, `post`, `put`, `patch`) when asserting that a journey is offline.

Rebuild `kobo-cli` after SDK, protocol, policy or simulator changes before running
app fixtures. Protocol 14 task tag 4 requires a matching beta runtime. A server
receiving a request is not proof that the app received its acknowledgement; test
lost responses and reconciliation before calling a sync feature complete.

## Offline HTTP stream fixtures

Debug simulator builds accept `KOBO_SIM_HTTP_FIXTURE=hostname=127.0.0.1:port`
(or a numeric IPv6 loopback socket). This routes that hostname's HTTPS port 443
to a local fixture server. Every other host and port is refused, including
redirect targets; there is no fallback to the public network. Requests keep
their original URL, Host header and TLS server name. Certificate verification,
credential policy, response limits, cancellation and retained stream parsing
use the normal runtime paths.

The fixture server must present a certificate for the original hostname.
Put its test CA in a private `KOBO_SIM_TRUST_DIR`, and use a private `TMPDIR`
with synthetic credentials and stores. Do not set `KOBO_SIM_OFFLINE`: that
separate switch starts the simulator with no network, so every request fails
the way it does on a reader without Wi-Fi. Invalid fixture configuration
also disables requests and prints a diagnostic. Release builds do not contain
the routing hook and refuse fixture-mode networking. Restart the simulator to
change the endpoint; configuration is fixed before the first TLS request.

The transport acceptance test can be run with:

```sh
cargo +1.85.1 test -p kobo-net --test fixture_endpoint
```

It runs real TLS GET, authenticated POST and retained NDJSON requests, checks
the original Host header and TLS server name, and verifies that unlisted
hosts and ports are denied. This is infrastructure for complete app fixtures;
it does not by itself prove a Lichess match or saved-session restart.

The complete Lichess fixture now uses this transport:

```sh
python3 scripts/quality/check-lichess-session-sim.py --output /tmp/lichess-session
python3 scripts/quality/check-lichess-session-sim.py --scale 170 --output /tmp/lichess-session-large
```

It creates a temporary CA, server and synthetic token, follows the visible
pairing controls, verifies the distinction between POST success and board
acknowledgement, kills and restarts the simulator with the same private store,
checks restored piece positions and completes by an agreed draw. It verifies
saved-session cleanup and resumed account polling. Output includes screenshots,
layout, request methods/paths and build fingerprints. Private keys, certificates
and token stores are removed with the temporary directory. No live account or
service is used. Python 3.9+, OpenSSL and the normal simulator dependencies are
required.

Use `--drop-start-event` with the Lichess session fixture to omit every match
notification. It verifies the app opens the matched board from its current-games
check without account polling or a second seek. The measured host recovery is
bounded to 15 seconds for the ten-second check; this is not device latency
calibration. The remainder of the match and restart checks still run.

Use `--disconnect-board` to close the retained TLS board stream mid-game and
hold its replacement open until the driver has checked the disconnected UI.
The test verifies unchanged piece positions, `--:--` clock placeholders,
blocked move submissions, restored active-side clocks, and removal of stale
reconnect guidance. It continues through process restart and game completion.
Both normal and 170% text-size routes validate rendering and touch diagnostics.
