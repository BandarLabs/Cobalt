# CLI-34: keep typed operations shared across UI, CLI and agent callers

There is one send engine, and every caller drives it the same way:

- The guided menu (bare `kobo` in a terminal) does not reimplement anything:
  choice 7 collects a file and a destination, builds the literal argv
  `send FILE --sim|--device IP|--reader NAME`, and hands it to the same
  `run()` dispatch the typed CLI uses (owner_start.rs send_choice ->
  main.rs run). The transcript shows a real menu-driven send transferring a
  fresh photo to the live simulator - then the typed `kobo send` of the
  same file deduping against the menu's receipt, because both paths share
  the receipts ledger too.
- Agent callers get the machine contract documented at the end of
  `kobo help`: results on stdout, explanations on stderr, `--json` printing
  one versioned object, exit statuses as stable categories (CLI-32/33), and
  verbs that keep the same meaning across commands.

No parallel implementations exist to drift apart; this item is evidenced by
call sites plus two real runs.
