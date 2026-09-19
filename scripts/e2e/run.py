#!/usr/bin/env python3
"""Run real companion workloads through the host CLI and their simulator apps."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FLOWS = ("vault", "nonograms", "parser", "panels", "rss")

def run(command, *, env, cwd=ROOT, capture=True):
    answer = subprocess.run([str(x) for x in command], cwd=cwd, env=env, text=True,
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.STDOUT if capture else None, check=True)
    return answer.stdout or ""

def require(workloads, name):
    path = workloads / name
    if not path.exists(): raise SystemExit(f"missing real workload: {path}")
    return path

def sha256(path): return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workloads", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--flow", action="append", choices=FLOWS)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args(); flows = args.flow or list(FLOWS)
    workloads=args.workloads.resolve(); out=args.out.resolve(); out.mkdir(parents=True, exist_ok=True)
    target=out / "target"; env=dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0")
    run(["cargo", "build", "--locked", "-p", "kobo-cli"], env=env, capture=False)
    kobo=target / "debug/kobo"; report={"source_sha":run(["git","rev-parse","HEAD"],env=env).strip(),"flows":[]}
    with tempfile.TemporaryDirectory(prefix="cobalt-real-e2e-") as temporary:
        state=Path(temporary); flow_env=dict(env, TMPDIR=str(state))
        commands={
          "vault":[[kobo,"vault","push",require(workloads,"notes"),"--sim"],[kobo,"vault","ls","--sim"]],
          "nonograms":[[kobo,"nonograms","preview",require(workloads,"moon.jpg"),"--out",out/"nonograms-preview"],[kobo,"nonograms","push",require(workloads,"moon.jpg"),"--name","Earth from the Moon","--size","9","--sim"]],
          "parser":[[kobo,"parser","inspect",require(workloads,"zork1.z3")],[kobo,"parser","push",require(workloads,"zork1.z3"),"--sim"]],
          "panels":[[kobo,"panels","inspect",require(workloads,"comic.cbz")],[kobo,"panels","push",require(workloads,"comic.cbz"),"--sim"]],
          "rss":[[kobo,"feeds","check",require(workloads,"subscriptions.opml")],[kobo,"feeds","push",require(workloads,"subscriptions.opml"),"--sim"]],
        }
        apps={"vault":"vault","nonograms":"nonograms","parser":"parser","panels":"panels","rss":"rss"}
        routes={
          "vault":"wait 500\nexpect Vault\nexpect notes\ntap Browse\nexpect Field notes\nshot real-note-list\ntap Field notes\nexpect Moon study\nshot real-note\n",
          "nonograms":"wait 500\nexpect Nonograms\ntap Photos\nwait 500\ntap Open\nwait 1500\nexpect Imported photo\nshot real-puzzle-play\n",
          "parser":"wait 500\nexpect Interactive fiction\nexpect zork1\nshot real-story-library\n",
          "panels":"wait 500\nexpect Untitled comic\nshot real-comic-library\n",
          "rss":"wait 500\nshot real-feeds\n",
        }
        for flow in flows:
            transcript=[]
            for command in commands[flow]: transcript.append("$ "+" ".join(map(str,command))+"\n"+run(command,env=flow_env))
            flow_out=out/flow; flow_out.mkdir(exist_ok=True)
            route=flow_out/"real-workload.kobo"; route.write_text(routes[flow])
            (flow_out/"host.txt").write_text("\n".join(transcript))
            run(["python3",ROOT/"scripts/check-apps-sim.py",apps[flow],"--seed-root",state,"--skip-default-seed","--route",route,"--out",flow_out/"sim","--timeout",str(args.timeout)],env=flow_env,capture=False)
            sim=json.loads((flow_out/"sim/results.json").read_text())["results"][0]
            if sim["status"] != "pass": raise SystemExit(f"{flow} simulator journey failed")
            report["flows"].append({"flow":flow,"status":"pass","simulator":sim,"inputs":sorted({str(p.relative_to(workloads)):sha256(p) for p in workloads.rglob('*') if p.is_file()}.items())})
            (out/"results.json").write_text(json.dumps(report,indent=2)+"\n")
    print(f"{len(flows)}/{len(flows)} real-workload flows passed: {out/'results.json'}")
if __name__ == "__main__": raise SystemExit(main())
