# Board-app follow-up sweep

After the shared board sizing changes, all three existing Crossword, Logic Pack and Nonograms interaction routes pass at default text size. At extra-large size, Crossword cannot find a letter key after a coordinate-based cell tap; Logic Pack and Nonograms hit renderer text-fit refusals. Parlor and Tic-tac-toe pass their extra-large routes.

These are open app/route issues, recorded against CROSS-02, LOGIC-04 and NONO-01. This sweep does not establish when the failures were introduced. The earlier 43-app default-size sweep is not evidence of extra-large coverage.

Results and actual failure frames with provenance are included here. Generated source was dirty during the captures, as the metadata records. No physical reader was used. The first default-size invocation accidentally used the unsupported value `normal`; the committed default result is the corrected run with `KOBO_TEXT_SCALE=default`.
