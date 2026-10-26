# CLI-05 - name readers by stable identity and owner nickname

A reader's stable identity is its serial, read from the identity script; the
owner nickname lives beside it in the saved-reader store
(~/.config/kobo/readers, KOBO_CONFIG_DIR overrides it). `kobo stream init
--reader NAME` records serial, address and pairing state under NAME when it
pairs. `--reader NAME` on any target-taking command resolves through the
store only. An unknown name is a target error that names the saved readers;
it never resolves to whichever reader answered first.

Evidence: transcript.txt - real runs against a store holding clara and
beckett: unknown name (exit 3, saved readers listed), saved but unreachable
(exit 3, serial prefix shown, no substitute), name + address together
(usage, exit 2). Tests: readers::tests::a_store_round_trips,
resolution_*; targets::tests.
