# CLI-10 - describe installation changes before approval

Two gates. `kobo setup --dry-run` prints every change the install would
make - install target, SSH state, settings keys, trust roots, menu entry and
exactly what a root extraction would contain, the welcome note, the eject
and the wait - and writes nothing. Without --dry-run the same summary is
shown as "Ready to install Cobalt:" and the install proceeds only after a
yes on the terminal (/dev/tty, not piped stdin), or --yes, which the
noninteractive refusal names explicitly. An unconfirmed noninteractive run
is a usage error, exit 2.

Evidence: transcript.txt - real runs against a mounted volume: the full
dry-run plan (exit 0, nothing written), the noninteractive refusal (exit 2),
and the --no-sample plan line. Tests: tests::preparing::a_dry_run_names_every_change_it_would_make,
a_dry_run_names_the_welcome_note_and_its_opt_out,
noninteractive_change_without_yes_is_a_usage_error.
