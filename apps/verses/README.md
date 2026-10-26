# Verses

A public-domain poem each day, set as a reading page.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/today.png" alt="Today's poem"><br>Today's poem</td>
<td width="50%" valign="top"><img width="300" src="screenshots/reading.png" alt="The reading page"><br>The reading page</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/browse-offline.png" alt="The offline shelf"><br>The offline shelf</td>
<td width="50%" valign="top"><img width="300" src="screenshots/attribution.png" alt="A poem with its author, year and source"><br>A poem with its author, year and source</td>
</tr>
</table>

## Features

- A new public-domain poem every day.
- An offline shelf that keeps each poem's author, first publication year and
  source.
- Search [PoetryDB](https://poetrydb.org/) by title, poet or line, and keep
  poems you find as offline favourites.
- Translations appear only when the translation itself is in the public
  domain.

## Permissions

- `network`: fetches the daily poem and PoetryDB search results.

## Development

```sh
cargo test -p kobo-verses
python3 scripts/check-apps-sim.py verses
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`.
