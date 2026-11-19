# CLI-27: support offline preparation and bundled local help

Preparation never needs a network: `kobo send --preview` renders the photos
(crop and pad variants plus a comparison index.html) entirely on this
computer and says so - "No photos were transferred." This sandbox has no
outbound network at all (link-local only), so the transcript's run is not a
simulation of offline; it is offline.

Help is bundled, not fetched: `kobo help` and every `kobo <command> --help`
print text compiled into the binary, so the manual is present on an
air-gapped computer. Both are shown by real invocation in the transcript.
