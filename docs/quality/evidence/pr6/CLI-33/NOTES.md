# CLI-33 - stable exit categories and bounded noninteractive behavior

Exit status is now a stable category, classified in `console::category_of` and
returned by `main`: 0 done, 2 the invocation was wrong (unknown command,
unknown flag, missing value, conflicting or missing target), 3 the reader or
simulator could not be reached or chosen, 4 the build or host cannot do it,
1 anything else. Errors carry their category as a prefix that is stripped
before printing, so messages keep their wording.

Bounded noninteractive behavior: a bare `kobo` with no terminal prints the
compact help and exits 0 immediately instead of blocking on input (first
transcript block, run with stdin at /dev/null).

Evidence: exit-categories.txt - real host runs of each category with the
recorded exit status. Tests: console::tests::exit_categories_*,
tests::exit_categories_hold_for_the_families_of_failure.
