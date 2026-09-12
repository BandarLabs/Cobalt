# Board-app follow-up sweep

On 8 September, after the shared board sizing changes, all three existing
Crossword, Logic Pack and Nonograms interaction routes passed at default text
size and failed above it: Crossword could not find a letter key after a
coordinate-based cell tap, and Logic Pack and Nonograms hit renderer text-fit
refusals. Parlor and Tic-tac-toe passed their extra-large routes. Those results
are kept here in `extra-large-results.json` as the record of what was wrong.

They were open app and route issues against CROSS-02, LOGIC-04 and NONO-01, and
each was closed by its group's own work later in this branch. Re-run on 12
September at both sizes, all six board applications pass their committed routes:
`extra-large-results-after-the-fixes.json` at 140% and
`largest-results-after-the-fixes.json` at 170%, which is the largest size a
reader can choose. This sweep does not establish when the original failures were
introduced, and the default-size sweep of the whole catalogue is not evidence of
coverage above it, which is why both sizes are recorded separately.

Results and actual failure frames with provenance are included here. Generated source was dirty during the captures, as the metadata records. No physical reader was used. The first default-size invocation accidentally used the unsupported value `normal`; the committed default result is the corrected run with `KOBO_TEXT_SCALE=default`.
