# Clippings

Read your Obsidian Web Clipper saves on the reader, mark them read, and tag
them. Your changes go back to your notes.

## Features

- A queue of the articles you clipped, unread first. Narrow it to Read or All,
  filter by tag, and sort by published or created date.
- A note has the whole panel and is set for reading. Long notes load a page
  ahead of you as you turn them.
- Mark a note read or add a tag from the note. Changes are kept on the reader
  until your computer collects them, so nothing is lost while you are offline.
- Open Clippings again after pushing and the queue refreshes, with the note you
  were reading kept where it was.

## Setup

1. Set up the reader once over USB, if you have not already: `kobo setup`.
2. Send your Web Clipper folder:

```sh
kobo clippings push ~/Vault/Clippings
```

Push again whenever you clip more. Only changed notes are sent. Commands use
the reader `kobo setup` saved. Add `--device IP` for another reader, or `--sim`
to try it on the simulator.

- A note larger than 1 MiB is refused at push time, because the reader will not
  open one that big.
- Read state and tags you add on the reader come back with
  `kobo export --app clippings --out DIR`. The Cobalt Clippings Sync Obsidian
  plugin does the push and the merge for you.

## Permissions

None. Clippings runs offline.

## Development

```sh
cargo test -p kobo-clippings -p kobo-cli
```

## Credits

Markdown is parsed with `pulldown-cmark` (MIT). Clippings is unofficial and not
affiliated with Obsidian or Dynalist Inc.
