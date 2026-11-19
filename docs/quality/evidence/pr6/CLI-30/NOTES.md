# CLI-30 - numbered plain terminal alternative

Interactive choice is a numbered list of plain lines: no cursor movement, no
escapes, no clearing, so it works on a dumb terminal, through a screen
reader, and over a serial line. The shared `console::choose_numbered` owns
the pattern; the owner menu's app-guide picker uses it, and the bare-`kobo`
owner menu remains numbered with invalid-choice retry and blank/EOF cancel.

Evidence: transcript.txt - real PTY runs via script(1): the owner menu with
Exit, an invalid choice retrying, and the numbered app-guide list where 0
retries and 1 dispatches. Tests: console::tests::numbered_choices_pick_retry_and_cancel,
owner_start menu tests.
