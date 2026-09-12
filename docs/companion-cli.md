# Use Cobalt from your computer

Run `kobo` in a terminal to choose an owner task by number:

1. Set up a reader over USB. Connect the reader and choose **Connect** on its
   screen. Setup identifies the device and keeps its existing installation
   review and approval step.
2. Preview photos for Frame. Enter a photo or folder path, then a new output
   folder. Open the generated `index.html` to compare crop and pad. Previewing
   does not transfer photos.
3. Check a feed subscription file. Enter an OPML export path to see its feed
   count and subscription addresses. Checking does not contact those sites or
   transfer anything.
4. Check a Paperterm connection. This starts the built-in typing check using
   your saved pairing identity. For first-time configuration, run `kobo stream`
   to see the pairing instructions.
5. Open the developer and release command reference.

Choose **0** or press Enter at a blank prompt to cancel. Paths can contain
spaces; enter the path itself without shell quotes. The menu passes it
literally to the same command used by scripts.

If input or output is redirected, bare `kobo` prints compact help and exits
without prompting. Explicit commands are unchanged. `kobo --help` opens the
full command reference; `kobo frame --help`, `kobo flashcards --help` and
`kobo feeds --help` show companion-specific commands.

Named-reader selection, a desktop companion window and resumable operation
receipts remain under development.
