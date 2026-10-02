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
COBALT_REVIEW_OUT=/tmp/homepanel-review python3 apps/homepanel/screenshots/review/capture.py
```

The regression suite covers all nine supported text scales, 100 discovered
entities, both tile layouts, notices, every page's actual hit targets, repeated
page turns, search reset, selecting the final device, and Back.

Interactive simulator attempts were blocked by this executor rejecting Unix-domain
socket bind with EPERM, including the approved elevated retry. No interactive,
network, physical-device, or ARM-build success is claimed by these captures.

The opt-in capture script copies the app source and the workspace lockfile into a
temporary Cargo package. PNG encoding adds a dependency only to that temporary
package, preserving the app’s reviewed release dependency graph. The normal app
regression suite retains every behavior and layout assertion.

## Overlapping request callbacks

`callback-before.png` and `callback-after.png` show the same mocked sequence: start
a status poll, tap the desk light, complete the older poll, then fail the newer
service request. The original handler removed the current task before checking the
reply's ID, so it discarded the service result and stayed at “Updating desk…”. The
handler now checks identity first and displays the current service error. Exact
source and scenario details are in `callback-provenance.json`.

A later request owns the displayed result. Earlier replies leave its task, pending
action and banner intact. Each accepted repeated tap still emits the same service
request as before; the fix adds no request cancellation, deduplication or queue.
Regression tests exercise both completion orders, successful confirmation, failed
services, stale cancellation, device discovery, connection tests and temperature
errors. Every callback is mocked; no Home Assistant server receives a request.
