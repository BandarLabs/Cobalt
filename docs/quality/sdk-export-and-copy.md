# Shared exports and UI copy

## Prepare an export in an app

Use `kobo_sdk::exports::Export` for text, Markdown, PNG or JPEG copies up to
32 MiB. The app validates image content with its encoder or decoder before
constructing an export; the shared flow checks the container signature and
verifies the transferred bytes. Construction does not write or send anything.

1. Construct the export from the owner's selected content and show `screen()`.
2. On the owner's `export-confirm` action, call `begin(context)`.
3. Forward `on_shelf` and `on_save` callbacks, including failures. The export
   uses the shared verified import copy and publishes its offer only after
   the file, readback, receipt and offer have been acknowledged.
4. On `export-retry`, call `begin` again. A failed offer save retries only the
   offer; it does not rewrite a complete file.
5. Keep one export object until its outstanding callbacks have drained.
   Replacing a pending export can misattribute a later store answer. A screen
   may close after its operation finishes; the original content remains owned
   by the app. Apps must include pending exports in their save/exit policy.

The reserved app-store key is `cobalt-export`. One offer is current per app;
a later explicitly prepared export replaces this offer, while older verified
copies remain on the app shelf. The offer contains bounded versioned metadata
and a SHA-256 shelf name. It contains no arbitrary path or computer address.

## Receive it on the computer

For a reader with the existing SSH connection enabled during USB setup:

```sh
kobo export --app fieldbook --device reader.local --out "$HOME/Downloads"
```

For a running simulator, use its same `TMPDIR` and replace `--device` with
`--sim`. The command requires an explicit output folder. It does not enrol a
new computer, bypass SSH host checks, expose a new network listener, or send
content to a cloud service. It uses the installed SSH identity and existing
reader host-key verification. Physical transfer validation is part of the
combined Clara BW acceptance run.

The receiver reads only the offer and its exact digest-named file, bounds its
reads, verifies size and digest, flushes the receiving file, and publishes it
without replacing an existing name. A conflicting name gets a numeric suffix;
a complete identical copy is reused. The reader's original and offer remain
available after a transfer fails or succeeds. A computer save failure cannot
cause deletion on the reader. The receiver confirms saving only after its file
and directory flushes succeed.

`Ready for your computer` means the local offer is acknowledged. It does not
mean the computer received it. Only the receiver prints `Saved` after its own
checks. App-specific companion commands can call this receiver rather than
inventing another transfer format. Their guided onboarding remains in PR 3.

## Shared UI copy contract

Name the item or operation when the app knows it. Use the shared failure advice
for generic failures, then put the relevant recovery action beside the affected
content. Successful empty results and missing/failed requests are different:
a missing requested item must not imply that a library is successfully empty.

Account help must not assume that setup requires a computer. Prefer separate
username and password fields where the service uses them. Never display a
secret, a generated Basic-auth string, a schema name or a wire error code.
Describe an unanswered request without guessing whether the service is down or
blaming the reader's network speed. Do not report `Saved`, `Connected` or
`Up to date` before the corresponding acknowledgement and content validation.

Primary UI should explain what happened and what the owner can do. Technical
paths belong in an explicit transfer/setup step when needed to complete it;
protocol, storage and implementation diagnostics belong in logs or developer
surfaces. Keep detailed provider errors separate from account values. Existing
app-specific wording and companion adoption remain catalog/companion tasks.

## Verification

The export fixture hosts a real SDK app beside the local launcher. Run:

```sh
cargo build -p kobo-cli -p kobo-launcher
cargo build -p kobo-sim --example export
python3 scripts/quality/check-export-sim.py --output /tmp/new-export-evidence
```

It exercises original text and generated geometric image content at Clara BW's
largest interface scale. It checks that no copy is offered before confirmation,
full storage remains a failure, retry succeeds, the actual CLI receives the
same bytes, repeated receiving reuses the copy, and damaged reader bytes cannot
replace a completed computer file. It performs no physical reader commands.
