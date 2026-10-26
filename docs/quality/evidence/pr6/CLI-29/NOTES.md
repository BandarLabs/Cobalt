# CLI-29: keyboard and screen-reader-friendly forms

The CLI's forms are its prompts and its output, and both are plain text:
every interaction is a typed command or a typed answer, and every line is a
complete line. Screen readers and Braille displays get the same bytes a
sighted terminal user gets, because there is nothing else to get.

Real evidence in the transcript:

1. The same send to the live simulator, once on a terminal and once piped:
   the output is byte-identical (modulo the PTY's carriage returns). No
   progress bar, spinner or in-place redraw exists to lose meaning when
   read aloud or captured.
2. Zero ESC bytes in the terminal output. Colour and emphasis are decided
   in one place (console.rs Console::from_env, honouring NO_COLOR and
   TERM=dumb) and nothing emits any yet - so a terminal that asks not to
   see escapes cannot receive one. TERM=dumb + NO_COLOR=1 output is
   byte-identical again.
3. Errors are full sentences on their own lines (see CLI-10/19/22
   transcripts), so a screen reader announces the whole problem and the
   fix in one utterance.

No code change was needed; this item audits the shipped behavior with real
runs on both terminal and pipe and records the invariant.
