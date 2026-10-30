# Hacker News

Read Hacker News stories, discussions and articles on your Kobo.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/stories.png" alt="Ranked stories with scores"><br>Ranked stories with scores</td>
<td width="50%" valign="top"><img width="300" src="screenshots/thread.png" alt="A story's comment thread"><br>A story's comment thread</td>
</tr>
</table>

## Features

- Top, New, Ask and Show, plus **Saved** for stories you set aside.
- Tap a story to open its discussion. Its linked article opens in the shared
  reader from the story's screen.
- Stories you have opened and saved are marked in the list.
- **Save** downloads the story's article with its pictures and keeps it on the
  reader, so it opens without Wi-Fi. The Saved tab marks each story Offline
  once it is stored, and says what is ready offline, for example "Ready
  offline: 12 articles, 4.1 MB". The discussion itself still needs a
  connection.
- Articles open cut to the story, without the site's menus, share buttons and
  footers. Each article reopens where you left it, and a new one opens at the
  text size you chose last.
- Comments load as you page through them. Deep replies show their depth, and
  any comment can be folded with its replies.
- Uses Hacker News' own API, so scores, new stories and reply order match the
  site.

## Permissions

- `network`: reads stories, comments and articles.

## Development

```sh
cargo test -p kobo-hn
kobo run --sim --app hn      # in the browser simulator
python3 scripts/check-apps-sim.py hn
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).
