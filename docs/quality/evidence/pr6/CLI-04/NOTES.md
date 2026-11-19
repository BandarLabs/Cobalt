# CLI-04 - a desktop/local companion surface using the same operations

The local companion surface is the guided terminal menu (bare `kobo`) plus
the compact owner help. It is not a parallel implementation: every choice
produces the argv of the same typed command an owner could have run, and the
binary dispatches it through the same registry (`owner_start::choose` returns
command vectors; `run` executes them). A regression test
(owner_start::tests::every_choice_dispatches_to_a_real_command) keeps every
menu choice mapped to a command the binary actually has.

Evidence: transcript.txt - real PTY runs via script(1): choice 5 dispatches
to the same --help, choice 6 to the same apps setup guide, and a
noninteractive run prints the compact help naming the same owner operations
and exits 0.
