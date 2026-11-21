# Offline dictionaries

Cobalt's reader looks up selected words without starting Wi-Fi or sending the
word anywhere. Dictionaries are loaded by the runtime, not by the reading app.

## Installing a dictionary

`kobo dict install FILE` puts a dictionary on the reader `kobo setup` saved,
over Wi-Fi. FILE is either kind:

- A Cobalt dictionary: a `.tsv` in the format described below.
- A StarDict dictionary: the `.ifo` file, with its `.idx` and its `.dict` or
  `.dict.dz` beside it under the same name, or a `.zip` holding those.

A StarDict dictionary is converted on the computer. Definitions written in
HTML or other markup become plain text, a pronunciation is kept in square
brackets, and sounds and pictures are left out. A definition longer than the
reader keeps is shortened and the command says how many were. The result is
read back with the reader's own parser before anything is sent, and a
dictionary too large for one file is split into several, never between two
senses of one headword. One the reader could not load, alone or beside what
is already installed, is refused with the numbers.

```text
kobo dict check FILE             convert and read back; nothing is sent
kobo dict install FILE           install on the saved reader
kobo dict list                   what is installed, and how much room is left
kobo dict remove NAME            remove one, by the name list shows
```

`--name NAME` sets the name a dictionary is listed, removed and replaced by;
installing a name again replaces it. `--language CODE` says which books it
answers for, such as `en` or `pt-BR`; without one it answers for every book.
`check` prints the number of entries and a few headwords, so a conversion can
be judged before it is installed.

Each of `install`, `list` and `remove` goes to the saved reader, or to
`--reader NAME`, `--device IP`, `--sim`, or `--volume PATH` for a reader
mounted over USB. Cobalt loads dictionaries at the first word looked up after
it starts, so leave Cobalt and start it again to use a new one.

Freely licensed StarDict dictionaries are published by several open
dictionary projects and are packaged by most Linux distributions. Cobalt
downloads none of them and does not ship one.

## Where dictionaries live

The command writes to the folder the runtime reads. On a Kobo that is:

```text
/mnt/onboard/.adds/cobalt/dictionaries
```

The host simulator uses:

```text
~/.config/kobo/dictionaries
```

A TSV file copied into either by hand is loaded the same way, and `kobo dict
list` shows it.

## The TSV format

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
`pt-BR`. Up to 32 files are read, each at most 8 MiB and 64 MiB in all; a
headword is at most 128 bytes, a definition at most 4 KiB, and a lookup shows
at most eight entries.
A malformed line or file is skipped without disabling other dictionaries.

Lookup performs Unicode compatibility normalization, case folding, surrounding
punctuation removal, and small inflection hooks. It tries the exact normalized
word before an inferred stem. An empty result is shown explicitly.

The format intentionally contains no HTML or executable content. Dictionary
licensing remains the owner's responsibility; Cobalt ships the service rather
than redistributing a third-party dictionary corpus.
