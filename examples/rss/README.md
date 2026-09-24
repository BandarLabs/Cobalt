# Feeds

Follow websites and read their articles offline.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/articles.png" alt="Unread articles from a feed"><br>Unread articles from a feed</td>
<td width="50%" valign="top"><img width="300" src="screenshots/reading.png" alt="Reading an article"><br>Reading an article</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/browse-feeds.png" alt="Browsing suggested feeds"><br>Browsing suggested feeds</td>
<td width="50%" valign="top"><img width="300" src="screenshots/search-articles.png" alt="Searching saved articles"><br>Searching saved articles</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/import-opml.png" alt="Importing subscriptions from OPML"><br>Importing subscriptions from OPML</td>
<td width="50%" valign="top"><img width="300" src="screenshots/save-articles.png" alt="Retrying a save that failed"><br>Retrying a save that failed</td>
</tr>
</table>

## Features

- Find a feed by website name, paste its HTTPS address, or browse the starter
  feeds. Names are looked up with Feedsearch. Pasted addresses are fetched
  directly and never sent to Feedsearch.
- RSS, Atom and JSON Feed. Each feed is previewed with its name and article
  count before you add it.
- Import subscriptions from an OPML file.
- Search saved articles across every subscription.
- Articles open in the shared reader, with text size, saved position and
  inline images with captions.
- Articles and images are saved for offline reading. Images are downloaded
  once and reused across feeds. Idle images are removed once the store passes
  128 copies or 8 MB.
- Refreshing keeps the current articles until new ones arrive. If saving fails,
  **Retry saving** saves the refresh without downloading it again.

Feeds shows what a feed provides. If a feed carries only summaries, that is
what you get. For a Miniflux account, use [Digest](../../apps/rss-miniflux/).

## Permissions

- `network`: fetches feeds, articles and images.

## Development

```sh
cargo test -p kobo-rss
kobo run --sim --app rss      # in the browser simulator
python3 scripts/check-apps-sim.py rss
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).
