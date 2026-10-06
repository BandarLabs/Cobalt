from __future__ import annotations

import argparse
import asyncio
import logging
import sys
from pathlib import Path

from musegadget import config

from kobo_bridge.auth import Auth
from kobo_bridge.service import lan_addresses, run
from kobo_bridge.tls import ensure_certificates

DEFAULT_DIR = Path.home() / ".local" / "share" / "kobo-bridge"
DEFAULT_PORT = 8473


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="kobo-bridge")
    parser.add_argument("--state-dir", type=Path, default=DEFAULT_DIR)
    parser.add_argument("-v", "--verbose", action="store_true")
    sub = parser.add_subparsers(dest="command", required=True)
    init = sub.add_parser("init", help="create certificates and show the pairing code")
    init.add_argument("--host", action="append", default=[], help="extra address for the certificate")
    runner = sub.add_parser("run", help="serve the reader and stay connected to Muse")
    runner.add_argument("--listen", default="0.0.0.0")
    runner.add_argument("--port", type=int, default=DEFAULT_PORT)
    runner.add_argument("--host", action="append", default=[])
    runner.add_argument("--allow-private-images", action="store_true")
    sub.add_parser("reset-pairing", help="new pairing code, forget paired readers")
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO,
                        format="%(asctime)s %(levelname)s %(message)s")

    if args.command == "reset-pairing":
        auth = Auth(args.state_dir)
        auth.reset()
        print("pairing code:", auth.code)
        return 0
    hosts = lan_addresses() + args.host
    if args.command == "init":
        ca, _, _ = ensure_certificates(args.state_dir / "tls", hosts)
        print("addresses:", ", ".join(hosts))
        print("trust file:", ca)
        print("pairing code:", Auth(args.state_dir).code)
        return 0
    try:
        token = config.sdk_token()
    except ValueError as err:
        print(err, file=sys.stderr)
        return 2
    try:
        asyncio.run(run(args.state_dir, args.listen, args.port, hosts,
                        args.allow_private_images, token))
    except KeyboardInterrupt:
        pass
    return 0
