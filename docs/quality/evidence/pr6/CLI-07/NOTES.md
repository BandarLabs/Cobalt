# CLI-07 - remember pairing and reconnect after address changes

Pairing is remembered by serial: `kobo stream init` identifies the reader
after installing the trust root and records serial, address, pairing state
and (with --reader NAME) the nickname. Later, `--reader NAME` tries the
saved addresses but accepts one only when the serial behind it matches -
an address is a lease, not an identity. When none answers as this reader,
the network is swept for the serial and the fresh address replaces the
stale one in the store, so a DHCP change is a re-identification, not a new
pairing and not a lost reader.

Evidence: transcript.txt - real run with clara saved at an address that
answers to nobody: the serial check fails, the sweep finds no N365..., and
the result is an honest target error (exit 3), not a guess. The
serial-match and address-replacement paths are proven in
readers::tests::resolution_finds_the_serial_after_its_address_changed and
remembering_moves_the_fresh_address_first_and_keeps_the_record (no second
physical reader exists in this sandbox; that is stated, not simulated).
