# CLI-28: keep prepared, sent and available-offline states distinct

The same photo is walked through its three states against the live
simulator, and the states never blur:

1. Prepared: `--preview` renders local files and nothing else - the reader
   shelf gains no file and no receipt is recorded. The completion line says
   "No photos were transferred."
2. Sent: the real transfer runs, the shelf gains the photo and the receipt
   ledger records the send (CLI-15/17).
3. Available offline: the photo then lives in the reader's own storage; the
   CLI exiting changes nothing, because availability is a property of the
   shelf, not of the connection.

The CLI keeps the vocabulary distinct too: preview says "prepared" and
"nothing is sent", send says "sent", and the receipts ledger and `kobo
report` (CLI-21) report sends as sends. No code change was needed; the
transcript is three real runs.
