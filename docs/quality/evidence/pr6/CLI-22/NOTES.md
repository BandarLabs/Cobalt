# CLI-22: understandable background status, pause and quit

`kobo sync` carries the full lifecycle surface: `status` (running/stopped,
per-folder direction, state, last change, error count), `pause`/`resume`
(transfers suspended, peer keeps running, the confirmation names its own
undo), and `stop` (clean shutdown, already-stopped is a statement not an
error, a daemon that ignores shutdown gets a plain 15-second report).

The transcript runs every lifecycle verb for real in an isolated HOME. Each
refusal is one line that names the missing precondition and the command that
fixes it (`sync setup ...` spelled out in full). Help output is included.

Sandbox limit, stated honestly: this computer has no Syncthing binary and no
paired reader, so a configured `status` table and a real pause against a live
peer cannot be exercised here. The unconfigured paths above are real
invocations of the shipped binary; the configured-path copy quoted in the
help and in `sync.rs` was reviewed line by line against this item's wording.
Existing unit tests (17 in sync.rs) cover state parsing, plan/status JSON,
pause/stop preconditions and timestamp handling.
