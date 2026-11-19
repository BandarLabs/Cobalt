# CLI-32 - progress on stderr, versioned JSON on stdout

Progress and explanations print on stderr; results print on stdout. `--json`
prints one JSON object on stdout with a `version` field (currently 1), the
answering command, and its data. Implemented in `console::Console`
(progress / json_envelope / print_json) and adopted by `kobo devices --json`.

Evidence: transcript.txt, stdout.json, stderr.txt - a real sweep of this
machine's local subnet (169.254.0.0/24). stdout held only the JSON object,
stderr only the progress line; no reader answered, so the exit was 3
(target). Tests: console::tests::json_envelope_carries_version_command_and_data,
a_pipe_gets_the_text_unchanged.
