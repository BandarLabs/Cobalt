# Daily Brief

A news brief that keeps loading while you use other apps.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/brief.png" alt="The brief, with its source and fetch time"><br>The brief, with its source and fetch time</td>
<td width="50%" valign="top"><img width="300" src="screenshots/sources.png" alt="Choosing the source list"><br>Choosing the source list</td>
</tr>
</table>

## Features

- Draws from one of four Hacker News lists: the front page, best of the week,
  Ask HN or Show HN. **Source** switches between them.
- Tap **Refresh** and switch to another app. The brief keeps loading and is
  ready when you come back.
- Shows which list it came from and when it was fetched.
- Tap a story to read it in the document reader, with figures and type
  controls. Read stories are saved and open offline.
- If a refresh fails, the last brief stays and you can try again.
- Saved whenever the app goes to the background, so it survives the reader
  sleeping.

## Permissions

- `network`: fetches stories and articles.

## Development

```sh
cargo test -p kobo-brief
kobo run --sim --app brief      # in the browser simulator
python3 scripts/check-apps-sim.py brief
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).
