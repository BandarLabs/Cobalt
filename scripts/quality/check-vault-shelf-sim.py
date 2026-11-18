#!/usr/bin/env python3
"""Drive Vault against shelves prepared by the real `kobo vault` CLI."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]


def write_vault(root):
    """A small vault shaped like a real one: folders, tags, links, a long note
    with headings, and a draft to exclude."""
    (root / "Projects").mkdir(parents=True)
    (root / "Reading").mkdir()
    (root / "Welcome.md").write_text(
        "# Welcome\n\nHome note of the fixture vault. See [[Projects/Alpha|Alpha]].\n")
    (root / "Projects" / "Alpha.md").write_text(
        "# Alpha\n\nA project note linked back to [[Welcome]].\n\n#project\n")
    (root / "Reading" / "Reading List.md").write_text(
        "# Reading List\n\nBooks for winter. Start at [[Welcome]].\n\n#reading\n")
    parts = []
    for part in range(1, 9):
        parts.append(f"## {['First', 'Second', 'Third', 'Fourth', 'Fifth', 'Sixth', 'Seventh', 'Eighth'][part - 1]} part\n")
        parts.append("The quick brown fox crossed the field at dawn, and the long note "
                     "goes on about the weather and the light for a good while. " * 6 + "\n")
    parts.append("Final sentence of the long note.\n")
    (root / "Reading" / "Long Note.md").write_text("# Long Note\n\n" + "\n".join(parts))
    (root / "Draft-secret.md").write_text("# Draft\n\nNot for the reader.\n")
    return root


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--vault", type=Path, default=None,
                        help="Real vault folder to push; a small one is written when omitted.")
    parser.add_argument("--drop", type=Path, default=None,
                        help="Delivered folder to ingest; one is written when omitted.")
    parser.add_argument("--push-cli", type=Path, default=None,
                        help="CLI binary with `kobo vault`; defaults to the built one.")
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    push_cli = (args.push_cli or cli).resolve() if args.push_cli else cli
    if args.push_cli is None:
        verify_cli(push_cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-vault-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   # A frozen clock keeps the chrome's minute tick from
                   # overpainting mid-capture; 11:11 also keeps every digit
                   # off the interface font's slashed zero.
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)
        if args.vault is None:
            args.vault = write_vault(private / "vault")
        if args.drop is None:
            args.drop = private / "drop"
            args.drop.mkdir()
            (args.drop / "Synced Note.md").write_text(
                "# Synced Note\n\nDelivered by another tool. Back to [[Welcome]].\n")

        # The shelves are prepared by the real companion CLI, exactly as an
        # owner would on their computer; only the sim root is redirected.
        subprocess.run([str(push_cli), "vault", "init", "--sim"], cwd=ROOT,
                       env=env, check=True, capture_output=True)
        pushed = subprocess.run([str(push_cli), "vault", "push", str(args.vault), "--sim",
                                 "--exclude", "Draft-secret"],
                                cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        ingested = subprocess.run([str(push_cli), "vault", "ingest", str(args.drop), "--sim"],
                                  cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        shelf = private / "cobalt-sim-data" / "vault"
        manifest = json.loads((shelf / "manifest.v1").read_text())
        synced = json.loads((shelf / "synced.v1").read_text())
        assert manifest["notes"], f"no notes on the shelf: {pushed.stdout}"
        assert synced["notes"], f"no synced notes: {ingested.stdout}"
        assert "Draft-secret" not in {n["path"] for n in manifest["notes"]}, "exclusion failed"
        assert "Now on the reader:" in pushed.stdout and "kobo sync" not in pushed.stdout, \
            pushed.stdout

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      shelf=dict(notes=len(manifest["notes"]), synced=len(synced["notes"])),
                      checks=[])
        with (out / "simulator.log").open("w") as log:
            def stop():
                nonlocal process
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=15)
                process = None

            def start():
                nonlocal process, address
                offset = (out / "simulator.log").stat().st_size
                process = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"],
                                           cwd=ROOT / "apps/vault", env=env,
                                           stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError("Simulator exited; see simulator.log")
                    match = re.search(r"Kobo app simulator: http://(127\.0\.0\.1:\d+)",
                                      (out / "simulator.log").read_text()[offset:])
                    if match:
                        address = match.group(1)
                        return
                    time.sleep(.1)
                raise TimeoutError("Simulator startup timed out")

            def drive(*steps):
                command = [str(cli), "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=180)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                drive("wait-for notes", "wait-for synced")
                capture("vault-home")
                drive("tap Browse", "wait-for Projects", "wait-for Reading")
                capture("vault-browse")
                drive("tap Reading", "wait-for Long Note")
                capture("vault-browse-folder")
                drive("tap Long Note", "wait 2000", "wait-for quick brown fox")
                capture("vault-note")
                for _ in range(60):
                    try:
                        drive("expect Final sentence of the long note")
                        break
                    except subprocess.CalledProcessError:
                        drive("tap Next", "wait 300")
                else:
                    raise RuntimeError("long note never reached its final sentence")
                capture("vault-note-final")
                drive("tap back", "wait-for Reading List")
                drive("tap back", "wait-for Projects")
                drive("tap back", "wait-for notes")
                drive("tap Tags", "wait-for reading")
                capture("vault-tags")
                drive("tap reading", "wait-for Reading List")
                capture("vault-tag-notes")
                drive("tap back", "wait-for Tags", "tap back", "wait-for notes")
                drive("tap Search", "wait-for Find text", "type final sentence",
                      "tap Search", "wait-for Final sentence of the long note")
                capture("vault-search")
                drive("tap back", "wait-for notes")
                drive("tap Browse", "tap Welcome", "wait 1500", "wait-for Home note",
                      "wait-for Links (3)")
                capture("vault-note-welcome")
                drive("tap Links (3)", "wait-for Alpha", "wait-for Synced Note")
                capture("vault-backlinks")
                # Headings are headings, and a wiki link opens its note; Back
                # returns to the note it was followed from.
                drive("tap back", "wait-for Home note")
                # The link's words, underlined inside the paragraph; a tap
                # by label would land on the middle of the paragraph, and a
                # fixed point is only on the link at one text size. The link
                # carries its note's place in the list, pushed notes first.
                alpha = next(index for index, note in enumerate(manifest["notes"])
                             if note["path"].endswith("Alpha.md"))
                drive(f"tap-id wiki-{alpha}", "wait 1500", "wait-for A project note")
                capture("vault-link-followed")
                drive("tap back", "wait-for Home note")
                drive("tap back", "wait-for Projects")
                # The long note reopens where it was left, at its last page,
                # so turn back to its first heading.
                drive("tap Reading", "tap Long Note", "wait 1500", "wait-for Final sentence")
                for _ in range(40):
                    try:
                        drive("expect First part")
                        break
                    except subprocess.CalledProcessError:
                        drive("tap Previous", "wait 300")
                else:
                    raise RuntimeError("the long note never showed its first heading")
                capture("vault-headings")
                drive("tap back", "tap back", "wait-for Projects", "tap back", "wait-for notes")
                # A note pushed while Vault is open is there when it comes
                # back, and nothing pushed before is lost.
                extra = private / "extra"
                extra.mkdir()
                (extra / "Field Notes.md").write_text(
                    "# Field Notes\n\nA wren at the feeder. See [[Welcome]].\n")
                added = subprocess.run([str(push_cli), "vault", "push", str(extra), "--sim"],
                                       cwd=ROOT, env=env, check=True, capture_output=True,
                                       text=True)
                count = len(manifest["notes"]) + 1
                assert f"Now on the reader: {count} notes." in added.stdout, added.stdout
                drive("lifecycle background", "wait 300", "lifecycle foreground",
                      f"wait-for {count} notes")
                drive("tap Browse", "wait-for Field Notes", "expect Welcome")
                capture("vault-after-push")
                drive("tap back", "wait-for notes")
                result["checks"].append(dict(
                    name="headings, links and a push on return",
                    detail="headings drew as headings, a wiki link opened its note and Back "
                           "returned to it, and a note pushed while Vault was open appeared "
                           "on return beside every earlier note",
                    status="passed"))
                result["checks"].append(dict(
                    name="shelf-driven journey",
                    detail="CLI pushed a real 5-note vault (one excluded) and ingested a "
                           "synced drop; the app browsed folders, read a long note to its "
                           "final sentence, filtered by tag, searched note bodies and saw "
                           "the synced source",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
