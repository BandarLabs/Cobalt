#!/usr/bin/env python3
"""Run existing committed routes against exact, independently checked-out app heads."""
import argparse, importlib.util, json, os, pathlib, subprocess, sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
a=argparse.ArgumentParser(); a.add_argument('--profile',required=True); a.add_argument('--phase',choices=['before','after'],required=True); args=a.parse_args()
m=json.loads((ROOT/'scripts/profile-review-heads.json').read_text());out=ROOT/'target/profile-review'/args.phase/args.profile;out.mkdir(parents=True,exist_ok=True)
report={'kind':'full simulator with committed drive routes','profile':args.profile,'phase':args.phase,'baseline':m['baseline'],'results':[]}
env=dict(os.environ,KOBO_SIM_PROFILE=args.profile,CARGO_TARGET_DIR=str(ROOT/'target'))
def run(*cmd,**kw):return subprocess.run(cmd,cwd=ROOT,env=env,check=True,**kw)
if args.phase=='before':
    work=ROOT/'target/profile-source'
    run('git','worktree','add','--detach',str(work),m['baseline'])
    proc=subprocess.run([sys.executable,str(work/'scripts/check-apps-sim.py'),*[p['app'] for p in m['apps']],'--out',str(out)],cwd=work,env=env)
    report['results']=json.loads((out/'results.json').read_text())['results'] if (out/'results.json').exists() else [{'status':'fail','error':'runner produced no report'}]
    for r in report['results']:r['source_sha']=m['baseline']
else:
    for p in m['apps']:
        work=ROOT/'target'/('source-'+p['app'])
        run('git','worktree','add','--detach',str(work),p['head'])
        dest=out/p['app'];dest.mkdir(exist_ok=True)
        proc=subprocess.run([sys.executable,str(work/'scripts/check-apps-sim.py'),p['app'],'--out',str(dest)],cwd=work,env=env)
        result=json.loads((dest/'results.json').read_text())['results'][0] if (dest/'results.json').exists() else {'app':p['app'],'status':'fail','error':'runner produced no report'}
        result.update(source_sha=p['head'],pr=p['number'],profile=args.profile,phase=args.phase,app_source_tree=subprocess.check_output(['git','-C',str(work),'rev-parse','HEAD:'+next(group+'/'+p['app'] for group in ['apps','examples'] if (work/group/p['app']).is_dir())],text=True).strip())
        report['results'].append(result)
        (out/'profile-results.json').write_text(json.dumps(report,indent=2)+'\n')
        run('git','worktree','remove',str(work))
(out/'profile-results.json').write_text(json.dumps(report,indent=2)+'\n')
sys.exit(any(r['status']!='pass' for r in report['results']))
