#!/usr/bin/env python3
"""External original-app audit: copied source, real native renderer, no simulator claim."""
import argparse, hashlib, json, os, re, shutil, subprocess, tempfile, tomllib
from pathlib import Path

COMMON = r'''
fn audit_capture(screen: &kobo_sdk::Screen, context: &kobo_sdk::Context, name: &str) {
    let metrics = context.metrics();
    kobo_text::install(metrics).unwrap();
    let chrome = kobo_ui::Chrome::for_screen(screen, false, kobo_ui::Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(screen.clone(), &chrome, "Audit");
    let mut surface = kobo_ui::Surface::new(metrics.width as usize, metrics.height as usize);
    kobo_ui::render_with(&screen, &metrics, &chrome, &mut surface, None);
    let output = std::path::PathBuf::from(std::env::var("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join(format!("{name}.png")), kobo_image::encode_png_grey(metrics.width as u32, metrics.height as u32, &surface.pixels).unwrap()).unwrap();
    std::fs::write(output.join(format!("{name}.txt")), format!("Native renderer snapshot, not interactive simulator.\nOwns runtime Back: {}\n{:#?}",screen.owns_back, screen.diagnostics(&metrics, &chrome))).unwrap();
}
fn audit_context(context: &kobo_sdk::Context, name: &str) {
    let screen = context.commands().iter().rev().find_map(|command| if let kobo_sdk::Command::SetScreen(screen)=command {Some(screen)} else {None}).unwrap();
    audit_capture(screen, context, name);
}
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,default=next(parent for parent in Path(__file__).resolve().parents if (parent / "crates/kobo-sdk").is_dir()))
    parser.add_argument('--app',default='readlater')
    parser.add_argument('--scenario',type=Path,default=Path(__file__).with_name('interrupted-scenario.rs'))
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--scope-tests',action='store_true')
    args=parser.parse_args()
    root=args.root.resolve(); app=next(root/g/args.app for g in ['apps','examples'] if (root/g/args.app).is_dir()); source=app/'src/main.rs'; output=args.output.resolve();output.mkdir(parents=True,exist_ok=True)
    cargo=tomllib.loads((app/'Cargo.toml').read_text());dependencies=cargo.get('dependencies',{})|cargo.get('dev-dependencies',{})
    dependencies.update({f'kobo-{name}':{'path':str(root/f'crates/kobo-{name}')} for name in ['image','text','ui']})
    with tempfile.TemporaryDirectory(prefix=f'cobalt-{args.app}-native-') as tmp:
        harness=Path(tmp);(harness/'src').mkdir()
        manifest=f'[package]\nname = "cobalt-native-audit-{args.app}"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n[workspace]\n[dependencies]\n'
        for name,spec in dependencies.items():
            path=(app/spec['path']).resolve(); options=f'path = {json.dumps(str(path))}'
            if 'features' in spec:options+=', features = '+json.dumps(spec['features'])
            manifest+=f'{name} = {{ {options} }}\n'
        (harness/'Cargo.toml').write_text(manifest);shutil.copy(root/'Cargo.lock',harness/'Cargo.lock')
        text=source.read_text()
        text=re.sub(r'(?m)^mod (\w+);',lambda m:f'#[path = {json.dumps(str(source.parent/(m[1]+".rs")))}]\nmod {m[1]};',text)
        text=re.sub(r'(include_(?:str|bytes)!\(\s*)"([^"]+)"',lambda m:m[1]+json.dumps(str((source.parent/m[2]).resolve())),text)
        scenario = args.scenario.read_text().replace('APP_FIXTURES',str(app/'fixtures'))
        if args.scope_tests:
            end = text.rfind('}')
            text = text[:end]+'\n'+scenario+'\n'+text[end:]
            scenario = ''
        (harness/'src/lib.rs').write_text(text+'\n'+COMMON+'\n'+scenario)
        subprocess.run(['cargo','test','--offline','--manifest-path',str(harness/'Cargo.toml'),'native_review_snapshots','--','--nocapture'],env=dict(os.environ,CAPTURE_DIR=str(output)),check=True)
    provenance={'kind':'native-renderer-snapshots-not-interactive-simulator','app':args.app,'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'adaptation':'Original main.rs copied unchanged apart from absolute module and include fixture paths for temporary audit crate; scenario appended.','renderer':'kobo_ui::render_with and kobo_text::install, CLARA_BW_METRICS','chrome':'Chrome::for_screen with representative measuring status; ensure_way_back','scenario_sha256':hashlib.sha256(args.scenario.read_bytes()).hexdigest(),'images':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.glob('*.png')}}
    (output/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
if __name__=='__main__':main()
