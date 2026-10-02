#!/usr/bin/env python3
"""Capture real app screens with synthetic state; no service/device claims."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

PROFILES = ['clara-bw-391', 'clara-bw-395', 'clara-hd-376', 'clara-colour-393',
            'elipsa-2e-389', 'libra-2-388', 'libra-colour-390',
            'libra-colour-390-4.46.23836', 'libra-h2o-384']
SCENES = {
'chat': r'''
let mut app = Chat::default();
app.conversation.push(Role::Assistant, "A long answer remains readable across complete pages. Every sentence belongs to the conversation. ".repeat(24));
let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
let count = runner.app().measured_pages(&runner.context()).len();
let mut app = runner.into_app_for_capture();
for back in 0..count { app.pages_back = back; capture(&format!("transcript-{:02}", count-1-back), &app.screen_for(&context), metrics); }
''',
'kitchencard': r'''
let mut app = Kitchen::default();
app.server = "https://mealie.example".into(); app.credential = "mealie".into();
for (name, view) in [("settings", View::Settings), ("finished", View::Finished)] {
 app.view = Some(view); app.show(&mut context); capture_context(name, &context, metrics);
}
''',
'readlater': r'''
let app = ReadLater { server: "https://bag.example".into(), entries_origin: Some("https://bag.example".into()), entries: (1..=3).map(|id| Entry { id, title: format!("Article {id}"), site: "example.org".into(), reading_time: 1, position: 0, content: format!("Body of article {id}. This is the article selected before the refresh."), starred: false, archived: false }).collect(), ..ReadLater::default() };
let mut runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
let commands = runner.action(action_id("sync"));
let task = commands.iter().find_map(|c| if let kobo_sdk::Command::Spawn {task, ..} = c {Some(*task)} else {None}).unwrap();
runner.action(action_id("entry-1")); runner.app().show(&mut context); capture_context("selected-before-refresh", &context, metrics);
runner.task_outcome(task, TaskOutcome::Completed(br#"{"items":[{"id":2,"title":"Article 2"},{"id":3,"title":"Article 3"}]}"#.to_vec()));
runner.app().show(&mut context); capture_context("selected-after-refresh", &context, metrics);
runner.action(action_id("archive")); runner.app().show(&mut context); capture_context("archived", &context, metrics);
''',
'parser': r'''
let mut app = Parser::default(); app.show(&mut context); capture_context("library", &context, metrics);
''',
'gutenbird': r'''
let mut app = Gutenbird::default(); app.view = View::Catalogs; app.show(&mut context); capture_context("catalogs", &context, metrics);
''',
}
COMMON = r'''
#[cfg(test)]
mod approved_fix_capture {
use super::*;
fn capture_context(name: &str, context: &kobo_sdk::Context, metrics: kobo_ui::DisplayMetrics) {
 let screen = context.commands().iter().rev().find_map(|c| if let kobo_sdk::Command::SetScreen(s) = c {Some(s)} else {None}).unwrap();
 capture(name, screen, metrics);
}
fn capture(name: &str, screen: &kobo_ui::Screen, metrics: kobo_ui::DisplayMetrics) {
 let out = std::path::PathBuf::from(std::env::var("CAPTURE_OUT").unwrap()).join(format!("{}-{name}", metrics.text_scale.percent()));
 let chrome = kobo_ui::Chrome::for_screen(screen, false, kobo_ui::Chrome::measuring(true).status);
 let screen = kobo_ui::ensure_way_back(screen.clone(), &chrome, "APP");
 let diagnostics = screen.diagnostics(&metrics, &chrome);
 std::fs::write(out.with_extension("txt"), format!("{diagnostics:#?}")).unwrap();
 let mut surface = kobo_ui::Surface::new(metrics.width as usize, metrics.height as usize);
 kobo_ui::render_all(&screen, &metrics, &chrome, &(), &mut surface, None);
 std::fs::write(out.with_extension("png"), kobo_image::encode_png_grey(metrics.width as u32, metrics.height as u32, &surface.pixels).unwrap()).unwrap();
 if std::env::var("CAPTURE_PHASE").unwrap() == "after" { assert!(!diagnostics.has_errors(), "{name}: {:?}", diagnostics.issues); }
}
#[test]
fn approved_fix_capture() {
 let profile = std::env::var("CAPTURE_PROFILE").unwrap();
 let (width, height, pixels_per_inch) = if profile.starts_with("elipsa") {(1404,1872,227)} else if profile.starts_with("libra") {(1264,1680,300)} else {(1072,1448,300)};
 for text_scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
 let metrics = kobo_ui::DisplayMetrics {width,height,pixels_per_inch,text_scale};
 kobo_text::install(metrics).unwrap();
 let _runner = kobo_sdk::AppRunner::with_metrics(APP_TYPE::default(), metrics);
 let mut context = _runner.context();
 SCENE
 }
}
}
'''
TYPES = dict(chat='Chat', kitchencard='Kitchen', readlater='ReadLater', parser='Parser', gutenbird='Gutenbird')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phase', choices=['before', 'after'], required=True)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {'source': subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),
                'kind': 'native app renderer and callbacks, not runtime navigation or hardware',
                'profiles': PROFILES, 'scales': [100,170], 'phase': args.phase, 'apps': {}}
    for app, scene in SCENES.items():
        source = next(root/group/app for group in ['apps','examples'] if (root/group/app).exists())
        with tempfile.TemporaryDirectory(prefix=f'five-{app}-') as tmp:
            package = Path(tmp)
            for path in source.rglob('*.rs'):
                dest = package/path.relative_to(source); dest.parent.mkdir(parents=True,exist_ok=True)
                text = re.sub(r'(include_(?:str|bytes)!\(\s*)"([^"]+)"', lambda m:m[1]+json.dumps(str((path.parent/m[2]).resolve())), path.read_text())
                dest.write_text(text)
            cfg = tomllib.loads((source/'Cargo.toml').read_text())
            deps = cfg.get('dependencies',{}) | cfg.get('dev-dependencies',{})
            for name in ['ui','image','text']:
                deps[f'kobo-{name}'] = {'path':str(root/'crates'/f'kobo-{name}')}
            lines = ['[workspace]', '[package]', f'name="five-capture-{app}"', 'version="0.1.0"', 'edition="2021"', '[dependencies]']
            for name, spec in deps.items():
                if isinstance(spec,str): lines.append(f'{name}={json.dumps(spec)}'); continue
                spec = dict(spec)
                if 'path' in spec: spec['path'] = str((source/spec['path']).resolve())
                lines.append(name+' = { '+', '.join(key+'='+json.dumps(value) for key,value in spec.items())+' }')
            (package/'Cargo.toml').write_text('\n'.join(lines)+'\n')
            shutil.copyfile(root/'Cargo.lock',package/'Cargo.lock')
            # Mutate through the runner to retain its installed metrics/context.
            scene = scene.replace('let mut app = runner.into_app_for_capture();', 'let mut runner = runner;')
            scene = scene.replace('app.pages_back = back;', 'runner.app_mut().pages_back = back;').replace('&app.screen_for(&context)', '&runner.app().screen_for(&context)')
            with (package/'src/main.rs').open('a') as stream:
                stream.write(COMMON.replace('APP_TYPE',TYPES[app]).replace('"APP"',json.dumps(app)).replace('SCENE',scene))
            for profile in PROFILES:
                dest = output/app/profile; dest.mkdir(parents=True,exist_ok=True)
                env = dict(os.environ,CAPTURE_PROFILE=profile,CAPTURE_OUT=str(dest),CAPTURE_PHASE=args.phase)
                with (dest/'capture.log').open('w') as log:
                    subprocess.run(['cargo','test','--offline','--manifest-path',str(package/'Cargo.toml'),'approved_fix_capture','--','--nocapture'],env=env,stdout=log,stderr=log,check=True)
            manifest['apps'][app] = subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD:'+str(source.relative_to(root))+'/src'],text=True).strip()
            (output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    manifest['sha256'] = {str(path.relative_to(output)): hashlib.sha256(path.read_bytes()).hexdigest() for path in output.rglob('*') if path.suffix in ['png','txt']}
    (output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')

if __name__ == '__main__':
    main()
