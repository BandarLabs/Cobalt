# Offline dictionaries

Cobalt's reader looks up selected words without starting Wi-Fi or sending the
word anywhere. Dictionaries are loaded by the runtime, not by the reading app.

On a Kobo, place UTF-8 TSV files in:

```text
/mnt/onboard/.adds/cobalt/dictionaries
```

The host simulator uses:

```text
~/.config/kobo/dictionaries
```

Each non-comment line contains a headword, one tab, and a definition. Optional
metadata comments precede the entries:

```text
# name=My English dictionary
# language=en
# priority=10
reader	A person who reads.
reading	The act or practice of interpreting written text.
```

Higher priority dictionaries are shown first; ties are ordered by dictionary
name. Language tags use short BCP 47-style values such as `en`, `fr`, or
`pt-BR`. Files, entries, definitions, results, and the total index are bounded.
A malformed line or file is skipped without disabling other dictionaries.

Lookup performs Unicode compatibility normalization, case folding, surrounding
punctuation removal, and small inflection hooks. It tries the exact normalized
word before an inferred stem. An empty result is shown explicitly.

The format intentionally contains no HTML or executable content. Dictionary
licensing remains the owner's responsibility; Cobalt ships the service rather
than redistributing a third-party dictionary corpus.
