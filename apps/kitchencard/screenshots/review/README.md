# Kitchen Card navigation review

These are in-process app/renderer captures, not interactive-simulator screenshots.
The before images were captured from the original screen builders, before changing
implementation. Clara BW metrics, the installed font and synthetic runtime-equivalent
status/Back chrome are used throughout. See provenance.json for exact scenarios.

The original 12-recipe list and 18-line ingredients view clipped their remaining
rows with no navigation. The recipe screen's final Back button disappeared too.
Measured pages now expose every row and repeat a recipe category on continuation
pages. Page count and arrow targets live in the reserved page strip.

A callback regression also demonstrated that Steps -> Ingredients -> Steps reset
the cooking step. The fixed path preserves the step and checked ingredients, and
runtime Back from Ingredients returns to that step. Other supporting screens now
claim Back so the runtime can deliver their existing return handlers.

```sh
COBALT_REVIEW_OUT=/tmp/kitchen-review python3 apps/kitchencard/screenshots/review/capture.py
```

Tests cover every row at all nine text sizes, error notices, repeated page turns,
last-page selection, ingredient checks, cooking detours, Back and fresh starts.

Interactive simulation is blocked here by Unix socket bind EPERM. No network,
physical-device or full simulator validation is claimed.

The opt-in capture script copies the app source and the workspace lockfile into a
temporary Cargo package. PNG encoding adds a dependency only to that temporary
package, preserving the app’s reviewed release dependency graph. The normal app
regression suite retains every behavior and layout assertion.
