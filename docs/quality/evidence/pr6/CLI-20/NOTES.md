# CLI-20 - useful owner errors with optional technical details

Errors say what to do in owner language by default; KOBO_DEBUG=1 (or
KOBO_DETAILS=1) appends the technical half where one exists
(console::Console::with_details): the saved addresses tried and the sweep
count for a reader that will not answer, the forwarded companion argv for a
failed send. The owner text never changes between the two.

Evidence: transcript.txt - the same unreachable-reader error with and
without KOBO_DEBUG, and a failed Frame send showing the offline checklist
plus the details line. Tests: console::tests cover the envelope and prefix
machinery this rides on.
