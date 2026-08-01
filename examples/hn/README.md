# Hacker News

Hacker News, on a panel with no scrollbar and no keyboard.

Four tabs along the bottom (Top, New, Ask, Show) and a comment thread behind
every story. Nothing animates, nothing scrolls, and nothing moves under a
finger that is already reaching for it.

| The stories | A thread |
| --- | --- |
| ![Numbered stories with scores down the right, and the page count in the top bar](screenshots/stories.png) | ![A comment thread, nested, with the story's facts above it and the page count at the foot](screenshots/thread.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## Why Algolia rather than the official API

Hacker News' own Firebase API returns one item per request. A story with four
hundred replies is four hundred and one requests, which on a device whose radio
is the largest single draw on the battery would flatten a charge. Algolia's
`items/:id` returns the entire thread, nested, in
one. That single fact is the reason this application is possible at all.

## What happens to a thread that does not fit

The transport carries half a megabyte and a busy thread is comfortably more. A
real one measured while writing this was 734 KB for 925 comments. Algolia
ignores `Range`, so the trick that lets Gutenbird read a novel in pieces does
not work here: asking for the second half returns the whole document again and
the ceiling rejects it.

So the request comes back `TaskError::TooLarge`. Rather than showing a dead
end, this asks a different question: `search_by_date` over that story's
comments, thirty at a time, which is bounded by construction. The nesting is
gone in that answer, so the screen says the nesting is gone.

## Running it

```sh
kobo run --sim --app hn                 # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md). The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Gutenbird](../gutenbird/README.md) ·
[RSS Reader](../rss/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)
