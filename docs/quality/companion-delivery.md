# Companion delivery · PR 4

This is the companion portion of the revised four-PR quality plan, based on
beta after #168. It retains all 133 companion and owner-acceptance tasks.
PR #181 continues the remaining catalog apps separately.

The first end-to-end journeys are Paperterm, Frame and Flashcards. They let
an owner demonstrate a live laptop terminal, a personal photo album and a
useful study collection on a reader. A successful demo requires real content,
clear preparation and transfer status, and recovery from a disconnected reader.

1. **Paperterm:** guided start, a harmless connection check, clear waiting and
   connected states, explicit Stop and an explanation that the laptop must stay
   awake. Test bidirectional input with a local fixture terminal. Keep arbitrary
   shell commands in the advanced flow.
2. **Frame:** choose photos, preview crop/pad at reader dimensions, show album
   and storage details, and distinguish prepared files from acknowledged transfer.
   Test corrupt photos, duplicate imports and an unavailable reader without
   losing the prepared album.
3. **Flashcards:** discover the supported helper and its installation status,
   preview a small original deck, verify and transfer it, then export its review
   log. Preserve the separate helper's existing license/distribution boundary.

Each flow needs CLI tests, a driven simulator journey, screenshots and updated
public instructions. Simulator success does not certify physical transfer or
panel behavior. Run the combined Clara BW acceptance after the relevant beta
builds are available. Stable promotion follows that acceptance; this PR does
not promote or merge beta into main.

Remaining companion groups stay in scope. Do not mark a task complete from
this plan alone, and do not report content as available offline until its
installation or import has been acknowledged.

## Paperterm connection check

`kobo stream demo` now runs a built-in text conversation through the real
host PTY and TLS service. It needs the existing identity and reader trust
setup. It does not interpret typed text as commands. The reader and laptop
can both submit messages; `exit` ends the child and leaves the final screen
available for one minute. The laptop's original terminal settings are restored.

Reproduce the real-PTY simulator check with:

```sh
python3 scripts/quality/check-paperterm-live.py --connection-demo \
  --output /tmp/paperterm-connection-demo
```

The check covers both input directions, Enter submission, portrait layout,
final-screen retention and terminal restoration. Its private generated identity
and pairing fixture are deleted afterward. This is simulator evidence, not
physical Clara BW acceptance. Guided first-time setup and the remaining
Paperterm companion checklist are still open.

Validation: two connection-check tests and all 20 stream tests pass on Rust
1.85.1. Strict Clippy passes for all CLI and stream targets. The final driven
simulator capture passes after shortening instructions to fit the portrait
screen with the keyboard open. Evidence is in `evidence/paperterm-connection-demo`.


`kobo stream pairing [--port PORT]` redisplays the saved computer addresses and
pairing code without regenerating credentials. Initialization saves the chosen
addresses, and demo startup repeats the connection instructions. IPv6 addresses
are bracketed, and a setup made before address storage explains how to add an
address. The expanded simulator check compares identity files before and after
reading pairing details and checks the selected port. Stream tests now total
22 passing tests; strict CLI/stream Clippy also passes.


## Flashcards helper connection

The CLI now delegates import, verify, stage and review-log export to the
existing standalone `flashcards-import` program. It finds a sibling helper,
then PATH; `KOBO_FLASHCARDS_IMPORT` can select a source-built executable.
`status` displays the helper's own notice and `--licenses` its bundled
license/source documents. No study-engine dependency was added to the CLI.
The old APKG `--out` spelling maps to merge; COLPKG replacement remains explicit.

Four routing/error tests and strict CLI Clippy pass on Rust 1.85.1. The real
helper and an original three-card fixture generator were built with Rust 1.88.
The CLI then imported, verified and staged the fixture; destination bytes
matched the prepared bundle. Corrupt verification/staging failed and preserved
the installed collection. Evidence is in `evidence/flashcards-companion`.
This validates a temporary mounted-directory fixture, not a physical reader.
Reader review, review-log round-trip, distribution and the remaining Flashcards
companion checklist are still open.

Reproduce with the built CLI, helper and fixture generator:

```sh
cargo +1.88.0 build --locked --manifest-path crates/kobo-flashcards-import/Cargo.toml \
  --example quality_fixture
python3 scripts/quality/check-flashcards-companion.py --cli /path/to/kobo \
  --helper /path/to/flashcards-import --fixture-generator /path/to/quality_fixture \
  --output /tmp/flashcards-companion
```


The Flashcards journey now also opens the staged collection in the actual SDK
simulator, reveals and grades one card, then exports the saved review through
the CLI. Normal and 170% text sizes pass with clean layout diagnostics; the
export is byte-for-byte identical to the one-record reader log. Screenshots
were inspected at both scales. FLASHCLI-01 and FLASHCLI-08 are complete on this
evidence; the other Flashcards tasks and physical acceptance remain open.
Add `--reader-sim --scale 170` to the reproduction command above to include
this journey. The script copies between private simulated shelves explicitly;
it does not claim a physical USB or network transfer.


Helper discovery now includes the actual helper version/source and notice.
`formats` prints the installed helper's package subset, modern-package refusal
and separate-review-log boundary before an import. Four CLI tests, three helper
command tests and strict CLI Clippy pass. The rebuilt real-helper acceptance
also verifies status and formats. FLASHCLI-02 and FLASHCLI-05 are complete;
verified binary distribution (FLASHCLI-03) remains open.


The real-helper acceptance now also imports a second original three-card
package into a new merged bundle: verification reports six due cards and the
source bundle stays byte-identical. Explicit COLPKG replacement of the merged
bundle returns to three due cards. Importing a malformed package into an
existing output fails without changing it; corrupt staging likewise preserves
the installed collection. The command transcript records each operation and
its outcome. These checks exercise the public CLI and standalone helper,
without touching an owner collection.

All 22 importer library tests pass on Rust 1.88, including metadata conflicts,
malformed databases, duplicate media and archive-bomb refusal. FLASHCLI-07 and
FLASHCLI-09 are complete on these regressions and the public-CLI acceptance.


`kobo flashcards preview COLLECTION.cobfc --out PREVIEW.html [--card NUMBER]`
now creates an offline, front/back content preview with card/due/media counts.
PNG/JPEG bytes are embedded; other media have names and byte counts. Card text
is escaped and scripts are disabled. A preview requires a new output filename.
Real-helper acceptance verifies text, original image bytes, file preservation
and invalid selection. Five helper-command tests and strict CLI Clippy pass.
The generated HTML was inspected in the browser and captured in `preview.png`.
FLASHCLI-06 is complete; verified distribution and the final license-boundary
audit remain open.


The first clean artifact build completed its ARM and host compilation, then
failed in packaging because the scripts read the retired central app catalog.
Both builder and auditor now collect the canonical registry, including app
contributions and derived minimum runtime versions. Shell syntax checks and
manifest generation against the current registry pass. The full artifact
audit must be rerun before FLASHCLI-04 can close.


## Frame comparison preview

`kobo frame preview INPUT --out DIRECTORY [--profile PROFILE]` compares crop
and pad for multiple photos before transfer. It reuses the bounded Frame image
preparation engine and shows reader resolution, album names and image storage.
Images open at full resolution; previews stay compact enough to compare both
fits. Existing output directories are refused and failed preparation leaves
no preview directory. A regression checks real downsize, differing fits,
output preservation and corrupt-image rejection. Strict CLI Clippy passes.
A two-image Clara BW comparison was generated and inspected in the browser;
evidence and source credits are under `evidence/frame-companion-preview`.
FRAMECLI-01 and FRAMECLI-02 are complete. Album control, transfer acknowledgement
and recovery remain separate open tasks.

Frame transfer planning: `frame plan` reports named additions/removals, reused
photos and image bytes without publishing anything. `--album` persists through
push/list. Seven Frame tests and strict CLI Clippy pass; the automated private
simulator-shelf journey verifies unchanged bytes after planning and repeated
push. Evidence: `evidence/frame-companion-plan`. FRAMECLI-03/04/05 are complete;
recoverable deletion and physical reader acceptance remain open.

The Flashcards artifact audit at a8ac3863 stopped at the stale generated device
dependency notice. FLASHCLI-04 stays open until the notices are reconciled and
the complete audit passes.

Frame recovery: before a changed nonempty shelf is published, preserve a complete
copy in one of two rotating slots. `frame restore` restores the prior photo
files and manifest; repeated unchanged pushes leave recovery untouched. The
CLI acceptance script passes replacement/removal restoration, bounded slots,
failed-copy refusal and incomplete-backup refusal. Generated device shell
scripts also pass local execution tests, including failed backup preservation.
Fifteen filtered CLI tests and strict all-target CLI Clippy pass. FRAMECLI-06 is
complete; physical Clara BW transfer/restore acceptance is still outstanding.

Frame publication verification: re-read the target manifest and all file sizes
before the success message. Refuse mismatched manifests, missing/empty files
and wrong-size new files. Eight Frame tests, strict CLI Clippy and the complete
private-shelf acceptance journey pass. FRAMECLI-07 is complete. This verifies
storage and target reachability, not physical display; that acceptance remains
open. Overall: 219 done, 276 open, one deferred.

Paperterm Stop: startup prints the laptop Ctrl+] shortcut and computer-awake
explanation. Stop closes the PTY command/session and sharing service, restores
the laptop terminal and prints a stopped message. Real CLI PTY tests cover an
active connection check and the final-screen service, including closed port
and restored mode settings. macOS PENDIN is ignored as a transient retype flag;
all other terminal settings are compared. 22 stream tests and strict Clippy
pass. STREAMCLI-04 complete; evidence in `evidence/paperterm-stop`.

Flashcards audit at ea937dc1 passed the generated license notice check, then
stopped because `app-verify` and `app-catalog-verify` are absent from the CLI.
The complete boundary task stays open pending real verification commands and
a full passing audit. The audit process has exited; no audit is running.

Artifact verification commands (in progress): `kobo app-verify --package PATH
--public-key PATH --manifest PATH --binary PATH` verifies the existing Store
signature format and exact manifest/binary identity, including ARM ELF checks.
`kobo app-catalog-verify --catalog PATH --signature PATH --public-key PATH
--package PATH` verifies both signatures and the matching catalog entry's
manifest, package digest and length. The public-key and signature paths contain
hexadecimal text. These commands do not install or publish anything. The
complete Flashcards audit remains open until rerun successfully.

Verification command validation: the signed-fixture regression passes altered
package bytes, wrong key, altered catalog, mismatched binary and a correctly
signed wrong-length catalog entry. Strict CLI Clippy passes. Both commands
also pass against the existing ea937dc1 ARM validation package and signed
catalog; evidence is `evidence/flashcards-verification-commands/result.json`.
This is not a substitute for the complete fresh-source artifact audit.

Paperterm presets: no-argument `kobo stream` shows connection check, login
shell and system monitor. `terminal` uses the default shell as a literal
executable; `monitor` runs top. Both retain pairing, custom-port and Stop
behavior. Real PTY tests observe connection-check, shell and monitor output,
then verify Stop within five seconds, restored terminal modes and closed
ports. The monitor harness consumes redraw output throughout stopping, as a
terminal emulator does. Preset regression and strict CLI Clippy pass.
STREAMCLI-02/05 complete; named-reader setup and state reporting remain open.

Paperterm states: accepted hello/screen/input requests update connection
activity; unauthorized and stale requests do not. The CLI reports waiting,
connected, waiting to reconnect after 45 seconds of silence, and stopped.
Live TLS acceptance checks wrong-token refusal, lease negotiation, the actual
idle interval, reconnect and final-screen state. 23 stream tests and strict
Clippy pass. STREAMCLI-03 complete; named-reader pairing remains open.

Flashcards full artifact audit: exit 0 at source
6aa7f8ff6aa2f59e5534b20d4cc97fbc5eb99d2f. Report and artifact hashes retained in
`evidence/flashcards-artifact-audit`. The current follow-on changes concern
Paperterm; Flashcards sources are unchanged since that audited commit.
FLASHCLI-04 complete. FLASHCLI-03 remains open for verified distribution.
Overall: 224 done, 271 open, one deferred.

Main CLI entry: bare kobo offers numbered owner tasks in a terminal, with
developer/release help as a separate choice. Redirected input/output uses
compact help without a prompt. Literal paths, cancellation and invalid-choice
retry pass unit tests. Real PTY acceptance opens the menu and validates an
original OPML file with a space-containing path through the existing handler.
Strict CLI Clippy passes. CLI-01/02/03 complete; broader reader selection,
desktop surface and transfer-operation work remain open. Evidence:
`evidence/owner-start`. Overall: 227 done, 268 open, one deferred.

Feeds companion hardening: bound file reads to the shared 256 KB OPML limit
before parsing; publish simulator imports through a synced temporary file.
An occupied staging path preserves both the current list and the other staged
file. Six Feeds regression tests and strict CLI Clippy pass. This advances the
shared transfer reliability work but does not close the broader CLI-18 task.
Checklist totals remain 227 done, 268 open, one deferred.
