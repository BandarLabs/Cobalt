# CLI-31 - narrow widths, resize, NO_COLOR, non-TTY and reduced motion

`console::Console` decides capabilities per render from the environment and
the real streams: COLUMNS (clamped to 40-100, default 80), NO_COLOR and
TERM=dumb switch off emphasis, KOBO_REDUCED_MOTION / REDUCED_MOTION switch
off animation, and a pipe gets full unwrapped lines. Help and owner prose
wrap over-long lines to the detected width; lines that fit pass through byte
for byte, so aligned columns keep their alignment.

Evidence: transcript.txt - real PTY runs via script(1): COLUMNS=48 wraps
--help to 48 columns (narrow-help-clean.txt; longest content line 48), and
TERM=dumb + NO_COLOR + KOBO_REDUCED_MOTION runs emit zero escape sequences.
Tests: console::tests::no_color_*, reduced_motion_*, width_*, wrap_*,
a_pipe_gets_the_text_unchanged.
