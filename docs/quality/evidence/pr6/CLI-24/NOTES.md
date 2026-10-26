# CLI-24: explain wake/network effects of continuous sync

Continuous sync now states its costs wherever the owner enables or runs it:

- `kobo sync` help closes with: while the peer runs it uses this computer's
  network; a sleeping reader is not kept awake and syncs during the windows
  its owner opens on the Kobo.
- The `kobo sync setup` completion message carries the same two facts at the
  moment the mapping is created.
- `kobo sync run` already said both at startup (background and foreground
  variants); left as is.

The transcript shows the real help output of the shipped binary and points
at the source lines for the setup and run messages. Sandbox limit: setup
cannot run end-to-end here (no Syncthing binary, no paired reader), so the
setup message is evidenced from source rather than a live setup run; the
help output is a real invocation.
