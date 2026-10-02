"""Run the same 31-app scope as the original 279-case profile sweep."""
import importlib.util,json,os,subprocess,sys
from pathlib import Path
root=Path.cwd();out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
apps=('launcher sidekick readlater chat pubquiz parser backgammon settings parlor '
      'homepanel kitchencard deck post habits vault syncthing zotero-reader '
      'rss-miniflux gutenbird rss hn inkling arxiv needles verses fanshelf '
      'musicstand grimoire birds audiobook fieldbook').split()
spec=importlib.util.spec_from_file_location('sim',root/'scripts/check-apps-sim.py')
sim=importlib.util.module_from_spec(spec);spec.loader.exec_module(sim)
subprocess.run(['cargo','build','--locked','-p','kobo-cli'],check=True)
cli=Path(os.environ['CARGO_TARGET_DIR'])/'debug/kobo'
report=dict(source_sha=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            profile=os.environ['KOBO_SIM_PROFILE'],scale=os.environ['KOBO_TEXT_SCALE'],results=[])
for app in apps:
    folder=out/app;folder.mkdir(exist_ok=True)
    override=Path(__file__).with_name('launcher-review-route.kobo') if app=='launcher' else None
    row=sim.run_app(app,cli,folder,os.environ,300,route_override=override)
    report['results'].append(row);print(json.dumps(row),flush=True)
    if row['status']!='pass':print((folder/(app+'.log')).read_text()[-4000:])
    (out/'results.json').write_text(json.dumps(report,indent=2))
failed=sum(row['status']!='pass' for row in report['results'])
print(f'{len(apps)-failed}/{len(apps)} owned routes passed')
raise SystemExit(bool(failed))
