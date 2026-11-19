# CLI-16 - report real preparing/sending/checking/ready stages

Stage lines name real work, on stderr: "preparing" is the content hash for
the receipt comparison; the companion's own lines are the sending and
checking (Frame: "11520 image bytes to send" then "Frame transfer verified";
Feeds: the staged summary; Panels: the packaged page count); "sent to
<target>" closes only after the companion acknowledged. Nothing prints a
stage that did not run.

Evidence: transcript.txt + stdout.txt + stderr.txt of a real send - stage
lines on stderr, results on stdout. Tests: console progress/stream
separation from CLI-32 hold for the new call sites.
