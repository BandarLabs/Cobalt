# Gutenbird download choices

`download-choices.png` renders an original two-format fixture using the app's
actual screen and runtime fonts at Clara BW default metrics. It is a local
app render, not a physical-reader or live-catalog capture.

Validation: 92 Gutenbird tests and strict all-target Clippy pass. The interaction
test selects plain text, checks the fetched URL and saved-position key, refuses
to switch an active download, then selects an EPUB already stored locally and
verifies a shelf read without a network task. Another test covers unavailable,
paid, unsupported and oversized links, sample labeling, paging and Back.

This capture covers the format-picker change in 1.0.15. The subsequent 1.0.16
edition-choice evidence completes GUTEN-02; see the validation log.
