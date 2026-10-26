# CLI-35: show host/reader/helper compatibility and signed update status

`kobo version` stays the one script-readable line. New: `kobo version
--full` prints the compatibility report - host version and platform, the
status of each helper (kobo-doctor, flashcards-import) probed by a real
`--version` call ("not installed", "present but ...", or a lookup that
itself failed, each named honestly), and the update guarantees.

Signed update status is not a claim, it is enforced code, and the
transcript exercises it for real:

- `kobo host-release-sign` with a well-formed manifest but a wrong seed is
  refused ("seed does not match the public release key") and writes no
  signature files.
- `kobo host-release-verify` with a forged (all-zero) signature fails
  verification, exit 1.
- setup refuses a release package built for a newer kobo than the running
  one with an update prompt (main.rs), and each companion declares its
  minimum kobo version in the signed manifest (registry
  minimum_cobalt_version).

A reader's own model/firmware compatibility is read from the device itself
(`kobo doctor --device`, setup dry run); this sandbox has no reader, so that
half is evidenced by the doctor/detect code paths and the dry-run
identification recorded in CLI-06's evidence.
