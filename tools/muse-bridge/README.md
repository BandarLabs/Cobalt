# kobo-bridge

The computer half of Muse Panel. It keeps the link to Muse that the Kobo
cannot hold, and serves the current screen to the Kobo over HTTPS.

Muse calls seven commands on it: `kobo.show_page`, `kobo.show_text`,
`kobo.ask`, `kobo.set_status`, `kobo.draw_url`, `kobo.clear` and
`kobo.device_info`. Shell and file commands from the stock Linux service are
switched off.

## Install

    python3 -m venv .venv && . .venv/bin/activate
    pip install /path/to/muse-gadget-sdk/linux ./tools/muse-bridge

The SDK needs BlueZ, python3-dbus and python3-gi for Bluetooth pairing with
Muse, as its own README describes.

## Pair

    kobo-bridge init --host 192.168.1.20

This makes a private certificate authority, prints the address, a
six-character pairing code and the path of `ca.pem`. Install the CA on the
Kobo, then open Muse Panel and type the address and code:

    kobo trust set muse-panel --from ca.pem --device KOBO_IP

Then pair this computer with Muse as a gadget using the SDK's own flow, and
run `kobo-bridge run`. `kobo-bridge reset-pairing` issues a new code and
forgets paired readers.

## Try it without Muse

`demo/sim_demo.py` drives the bridge from the SDK's FakeVm and shows the result
in the Cobalt simulator, then taps an answer and checks that the fake VM
receives it as a chat message:

    python demo/sim_demo.py --kobo PATH/TO/kobo --app PATH/TO/apps/muse-panel \
        --out shots --profile clara-bw-391

The tests run the same path headless: `python -m pytest -q`.

## Going live

Nothing in the bridge changes. Use the SDK's normal pairing and token for your
Muse account, and run `kobo-bridge run`. The simulator demo never talks to
api.muse.ai.

## Limits

- The reader polls. A change can take up to a minute to appear and nothing
  wakes a sleeping Kobo.
- `kobo.draw_url` shows a full-screen JPEG or PNG up to 4 MB from a public
  address; private addresses are refused unless you pass `--allow-private-images`.
  Pages hold text only: an image line in a page shows its description.
- A question is at most 60 characters with at most 120 characters of context, so
  both fit above the answers at the largest text size.
- A tap is acknowledged only once Muse has accepted it. If Muse cannot be
  reached the reader gets an error and the question stays open to tap again.
- `kobo-bridge reset-pairing` takes effect on a running bridge at its next
  request: old tokens stop working and the new code is the one to use.
- A pairing code works once: after a reader pairs, the bridge issues a new code (`kobo-bridge init` prints it). The endpoint locks after repeated wrong codes.

## Tests

    pip install pytest
    MUSE_GADGET_SDK=/path/to/muse-gadget-sdk pytest

Without `MUSE_GADGET_SDK` the one test that needs the SDK's FakeVm harness is
skipped. The simulator demo needs it, plus a built `kobo` binary:

    MUSE_GADGET_SDK=/path/to/muse-gadget-sdk python demo/sim_demo.py \
        --kobo ../../target/debug/kobo --app ../../apps/muse-panel --out shots

Add `--pairing` to start unpaired and drive the address and code screens, and
`--scale 170` for the largest text size.
