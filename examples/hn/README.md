# Hacker News

Hacker News, on a panel with no scrollbar and no keyboard.

Five destinations along the bottom. Four are the site's own lists (Top, New,
Ask, Show) and the fifth is what you put aside. Behind every story is its
discussion, and behind a story with a link is the article itself, read on the
device. Nothing animates, nothing scrolls, and nothing moves under a finger
that is already reaching for it.

| The stories | A thread |
| --- | --- |
| ![Numbered stories with scores down the right, and the page count in the top bar](screenshots/stories.png) | ![A comment thread, nested, with the story's facts above it and the page count at the foot](screenshots/thread.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## The story and the discussion are two different places

A row opens the discussion, because that is what this application is for. The
article behind the story is offered on the story's own screen, next to Save,
and opens in the same reader the rest of the system uses: the same type sizes,
the same front light control, the same page turns.

## What the device remembers

Which stories have been opened, and which ones were put aside. Both are shown
at the front of the row's second line, where an eye running down the left edge
of a list finds them, and only on the rows that have them.

A saved story is written down whole, with its article beside it, so the Saved
list is the one that works on a train: it needs no radio to draw, and the
article opens from the copy on the device. The site keeps both of these for a
logged-in reader and will keep neither for an application, and the alternative
to keeping them here is asking somebody for their Hacker News password so that
a list can be grey where they have already been.

## One item per request, on purpose

Hacker News' own API answers one item at a time, which is more round trips than
a search index needs. It is worth every one of them: it is the site's own
record, so a story submitted a minute ago is in the list, every score is the
score on the page, and `kids` is the order the site draws replies in. A client
cannot recompute that ordering, and a ranked search index answering thirty at
once got Ask HN wrong by thirteen years.

Comments are fetched as the reader pages into them, so the radio a thread costs
tracks how far it was actually read rather than how popular it is. A reply
deeper than the gutter can show says how deep it is in its byline, which costs
no width at all, and any comment can be folded away with its replies.

## Running it

```sh
kobo run --sim --app hn                 # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md), which
[installs on a Kobo](../../README.md#install-it-on-your-kobo) with one
command over USB. The other apps:
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
