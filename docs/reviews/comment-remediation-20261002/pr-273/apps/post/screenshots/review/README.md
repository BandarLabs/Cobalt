# Post letter layout review

These are in-process app/renderer captures, not interactive-simulator screenshots.
The original app screen builder produced the before image before implementation
changed. All images use the installed reading face, Clara BW metrics, runtime Back,
and the runtime rule that hides the status strip on reading screens.

The original paginator allocated a full reading page before adding a reply button
and notices. Its long-letter screen reported TextOverflow and could hide words
before the next page. The fixed view reserves the reply action's bottom band, the
page strip, and the actual measured height of reply status and error notices.
Page turns use the same measurement as rendering and preserve saved word offsets.

The reply action stays in place. The page strip shows position and arrow targets,
and runtime Back returns to the inbox without a second competing Inbox button.

```sh
COBALT_REVIEW_OUT=/tmp/post-review python3 apps/post/screenshots/review/capture.py
```

Regressions verify every original word, every page's layout and reply target at
all nine text sizes, queued/offline notices, repeated turns, cached position
restoration, opening a reply and returning through the inbox.

Interactive simulation is blocked by Unix socket bind EPERM in this executor.
No interactive, live gateway or physical-device success is claimed.

The opt-in capture script copies the app source and the workspace lockfile into a
temporary Cargo package. PNG encoding adds a dependency only to that temporary
package, preserving the app’s reviewed release dependency graph. The normal app
regression suite retains every behavior and layout assertion.
