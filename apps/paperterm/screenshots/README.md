# Paperterm screenshots

Actual SDK simulator captures on a Clara BW profile, not mockups or
physical-device captures.

- `welcome.png`, `preview.png`, the three setup steps (`setup.png`,
  `trust.png`, `start.png`), `pairing.png`, `address-error.png` and
  `code-error.png` come from the offline route in `../drive.kobo`.
- `terminal.png` is a Clara BW in portrait at the default text size, sharing a
  host terminal. The terminal is 75 columns wide; opening the keyboard leaves
  25 rows.
- `terminal.png`, `connection-check.png`, `input-paused.png` and
  `reconnecting.png` come from live sessions.
  `scripts/quality/check-paperterm-live.py` runs one against a local host with
  private test certificates. `input-paused.png` is the recovery screen after
  an injected key-request timeout.
- `read-failed.png` and `pairing-not-saved.png` are the storage recovery
  screens, from the same script's `--load-failure` and `--save-failure`
  options. They check that read and save can be retried without interrupting
  the shared terminal; `--temporary-pairing` checks that continuing without
  saving leaves the original pairing untouched.
