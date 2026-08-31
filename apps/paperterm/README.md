# Paperterm

Paperterm is the reader half of `kobo stream`: a terminal session rendered on
e-ink while its pty, shell, command, and credentials remain on the computer.
The app has only the `network` capability; it cannot run a shell and does not
store terminal content.

<img width="300" src="screenshots/pairing.png" alt="Paperterm pairing address field and keyboard in the Clara BW simulator">

This Store MVP supplies the pairing and terminal-mirror UI, including an honest
`off the air` state. The companion `kobo stream` host/TLS service specified for
the production protocol is not included in this app-only delivery, so a paired
screen remains offline until that host is installed. No input controls are
shown in the read-only MVP.

The mirror uses the platform terminal node and its measured grid. Its intended
policy is row-only repainting for received deltas; an empty poll repaints
nothing.
