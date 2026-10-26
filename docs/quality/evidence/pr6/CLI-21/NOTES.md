# CLI-21: export diagnostic reports without secrets or content by default

`kobo report` builds a snapshot a helper can read: the kobo version and OS,
saved readers with four-character serial prefixes, completed setup steps,
send counts and destinations, and any send waiting for a retry.

Redaction is the default and is spelled out in the report itself: no file
contents, no book or photo names, no trust material, no keys, no full
serials, no file paths, no network addresses. `--include-paths` adds the
named files for the cases where the path is the question; it must be asked
for explicitly. `--out FILE` writes the report where the owner chooses.

The transcript is a real run on this computer against a config with a saved
reader, a real send receipt, and a pending retry: the default report names
the reader and counts but not the file, the full serial, or any address; the
grep checks print 0 for each; `--include-paths` is the only way the file
name appears. The unit test builds a fixture config with a serial, an
address and a private-looking path and asserts none of them leak by default.
