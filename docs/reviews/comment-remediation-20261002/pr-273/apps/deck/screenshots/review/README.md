# Deck command-output review

These are in-process app/renderer captures, not interactive simulator screenshots.
The before image uses the unchanged app screen builder. All images use Clara BW
metrics, the installed interface face, and synthetic runtime status/Back chrome.

A command result within the daemon's 2 KB output limit overflowed a single screen.
The fixed result is paginated using the SDK's measured prose path. The command
heading and exit status occupy page one; continuation pages carry the command name
in the top bar. Output line boundaries are kept, and page arrows/position remain
reachable. Runtime Back now returns from a result to the deck, preserving its
last-result acknowledgement.

```sh
COBALT_REVIEW_OUT=/tmp/deck-review cargo test -p kobo-deck capture_review_pages_when_requested
```

Regression tests verify every output word and page at all nine text sizes,
reachable page targets, repeated turns, no command dispatch while paging, empty
and missing output, and the Back transition. The existing command confirmation,
preview safety and polling tests remain intact.

The executor blocks the simulator's Unix-domain socket bind with EPERM. No
interactive simulator, actual computer command, or physical Kobo success is claimed.
