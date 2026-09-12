# Explicit HTTP update tasks

[Miniflux uses PUT for entry updates](https://miniflux.app/docs/api.html#update-entries);
the shared task API also supports PATCH. The SDK now exposes `Task::Update` with a closed `UpdateMethod` rather than
changing the meaning of POST or forwarding a method-override header.

The beta protocol uses task tag 4 under version 14, preserving the previous task
encodings. Older protocol versions refuse update tasks. Apps, runtime and
simulator must be rebuilt together. The native and simulated hosts dispatch
updates through the same policy runner and HTTPS transport.

Credential policy receives the exact method, destination, body and content type.
Existing provider grants do not inherit PUT/PATCH permission. Server-bound
read-only accounts remain read-only. The subsequent [Miniflux policy](miniflux-account-policy.md)
adds narrowly scoped entry updates and feed creation to its bound token. Miniflux
app integration and Read Later provider rules remain in progress; these platform
changes do not complete their sync tasks.

The transport sends an update once without redirects or stale-connection replay.
`spawn_retrying` also sends update tasks once. A lost reply can follow an applied
change, so apps must retain their pending action and reconcile server state before
trying again. Cancellation ends waiting and produces one cancelled callback; it
cannot undo a request the server accepted.

The simulator includes updates in network fault injection, capability checks,
active-work accounting and callback barriers. Activity reports separate `put` and
`patch` counters without retaining destinations, credentials or bodies. Offline
journey scripts now check all four network methods.

## Validation · 9 September 2026

- Protocol: 99 tests pass, including both methods, truncated frames, unknown
  method bytes, older wire versions and body/header bounds.
- Policy: 134 tests pass, plus the subsequently added focused cancellation test.
  Coverage includes exact method authorization, resolved credentials, missing
  network capability, missing secrets, POST-only grants, forbidden headers and
  refusal of retained/line-stream controls. Existing provider grants are tested
  against both update methods.
- SDK: 153 tests pass, including delivery of an uncertain update failure to the
  app without a retry or delay task. Two existing documentation examples remain
  ignored.
- Simulator: 75 tests pass, including method counters and all network fault
  scenarios for PUT/PATCH.
- Native runtime with `device-write`: 25 library and 151 binary tests pass.
- Strict all-target Clippy passes for protocol, policy, SDK, simulator and native
  runtime. Rust 1.85.1 ARMv7 musl checking passes for the SDK and runtime with
  `device-write`. This is compilation, not hardware execution.
- Rebuilt `kobo-cli`, then ran `check-crossword-sim.py` on Clara BW at Extra-large.
  All [17 actual SDK/IPC checks](evidence/http-updates/result.json) pass, including
  storage-full recovery, restart, undo, completion and damaged-state preservation.
  Each capture checks zero fetch/post/put/patch effects. The selected
  [portrait capture](evidence/http-updates/02-numbered-grid.png) has a verified
  raw-frame digest and its original source/font/profile provenance sidecar.
  Captures truthfully identify the pre-commit revision and dirty working tree.
- Workspace all-target compilation passes with Zotero Reader excluded.
- Six modified Python fixture scripts parse. Only Crossword's updated full route
  was rerun for this platform change; the other five retain their earlier app
  evidence and now assert the extra method counters on their next run.

This proves the shared transport/task integration and existing-app regression
scope above. A real Miniflux PUT or Wallabag PATCH through their app UI has not yet
been demonstrated. Their durable outboxes, provider policies, reconciliation,
offline reading and screenshots remain in progress. Checklist totals are unchanged.
