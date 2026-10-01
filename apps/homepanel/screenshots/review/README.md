# Home Panel reachability review

These are in-process renderer captures, not interactive-simulator screenshots.
The original app screen builders rendered the before images before implementation
changes. The shared Kobo renderer and installed interface face drew all images at
Clara BW metrics with a synthetic status strip and runtime-equivalent Back chrome.

- `picker-before.png`: 20 discovered devices overflowed the page with no way to
  reach the remaining rows. Diagnostics reported Clipped and InteractiveOffscreen.
- `picker-after.png`: measured pages keep every device reachable. The page strip
  provides the page count and Previous/Next arrows without duplicate controls.
- `wall-before.png`: 12 one-column tiles ran below the panel.
- `wall-last-after.png`: the final tiles are reachable on the second page.

Recreate the after images from the repository root:

```sh
COBALT_REVIEW_OUT=/tmp/homepanel-review cargo test -p kobo-homepanel capture_review_pages_when_requested
```

The regression suite covers all nine supported text scales, 100 discovered
entities, both tile layouts, notices, every page's actual hit targets, repeated
page turns, search reset, selecting the final device, and Back.

Interactive simulator attempts were blocked by this executor rejecting Unix-domain
socket bind with EPERM, including the approved elevated retry. No interactive,
network, physical-device, or ARM-build success is claimed by these captures.
