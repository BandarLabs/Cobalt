# Music Stand companion acceptance - real public-domain score

The actual `kobo musicstand` CLI prepared and atomically published a six-page
PDF, then the running Music Stand SDK app read the shelf and displayed the
real notation in the Clara BW simulator. The source is J. S. Bach's Cello
Suite No. 1, BWV 1007, typeset by Mutopia from a 1916 Schirmer source and
listed as Public Domain.

Source page: https://www.mutopiaproject.org/cgibin/piece-info.cgi?id=517
Source archive: https://www.mutopiaproject.org/ftp/BachJS/BWV1007/bwv1007/bwv1007-a4-pdfs.zip
PDF SHA-256: `c82a7977944dfed870a1c14736ae502c83e82758b4350a984df253aed94cddc3`

Acceptance run:

- `init --sim` created the protected simulator shelf.
- `plan ... --sim` converted six PDF pages to fitted grayscale pages without publishing.
- `push ... --sim` published one score and its manifest atomically.
- `ls --sim` read the resulting six-page shelf entry.
- the running app showed `bwv1007-a4`, opened page 1, and rendered the notation.
- its reading menu reported page 1 of 6, whole-page zoom and the available mark action.

The screenshots were pixel-inspected at 1072x1448. The library is readable;
the score page clearly shows real staff notation and title information; the
reading menu remains legible over the page. This validates simulator behavior,
not a physical reader or USB/SSH transfer.
