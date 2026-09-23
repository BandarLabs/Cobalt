# CLI-15 - preview content for the selected reader before sending

`kobo send --preview FILE` renders or checks content locally before anything
moves: Frame renders crop/pad at the reader's panel size (1072 x 1448 here,
--profile selects another), Panels renders pages to --out, Feeds checks the
subscription list, Parser inspects the story file. A preview is local by
definition, so target flags with --preview are a usage error, and nothing is
transferred - the run says so.

Evidence: transcript.txt - real renders: a PNG became crop/pad panels plus
index.html, an OPML list was checked and summarised, and --preview with
--sim refused (exit 2). Tests: detect routing plus the companions' own
preview tests.
