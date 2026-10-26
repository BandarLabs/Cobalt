#!/usr/bin/env python3
"""Render original Read Later screens without sockets; not a simulator run.

Run after the checkout's normal Cargo build has fetched its locked dependencies.
The harness includes the app source unchanged and exercises native callbacks.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

RUST = r'''
#![allow(dead_code)]
    include!("SOURCE_PATH");
    #[test]
    fn native_review_snapshots() {
        use kobo_sdk::{AppRunner, Command};
        let output = std::path::PathBuf::from(std::env::var("CAPTURE_DIR").unwrap());
        std::fs::create_dir_all(&output).unwrap();
        for (name, scale) in [("default", kobo_ui::TextScale::Default), ("largest", kobo_ui::TextScale::Largest)] {
            let metrics = kobo_sdk::DisplayMetrics { text_scale: scale, ..kobo_sdk::CLARA_BW_METRICS };
            kobo_text::install(metrics).unwrap();
            let mut context = AppRunner::with_metrics(ReadLater::default(), metrics).context();
            let mut app = ReadLater { server: "https://wallabag.example".into(), depth: 50, notice: Some("Reading list synced.".into()), ..ReadLater::default() };
            app.entries = (0..50).map(|n| Entry { id:n+1, title:format!("Article {}: A morning beside the river",n+1), site:"example.org".into(), reading_time:8, position:0, content:"The first light reaches the bridge before the town wakes.".into(), starred:false, archived:false }).collect();
            app.show(&mut context);
            let capture = |label: &str, context: &Context| {
                let screen = context.commands().iter().rev().find_map(|c| if let Command::SetScreen(s)=c {Some(s)} else {None}).unwrap();
                let chrome = kobo_ui::Chrome::for_screen(screen, false, kobo_ui::Chrome::measuring(true).status);
                let safe_screen = kobo_ui::ensure_way_back(screen.clone(), &chrome, "Read Later");
                let screen = &safe_screen;
                let layout = screen.layout_with(&metrics, &chrome);
                let mut surface = kobo_ui::Surface::new(metrics.width as usize,metrics.height as usize);
                kobo_ui::render_with(screen,&metrics,&chrome,&mut surface,None);
                let png = kobo_image::encode_png_grey(metrics.width as u32,metrics.height as u32,&surface.pixels).unwrap();
                std::fs::write(output.join(format!("{name}-{label}.png")),png).unwrap();
                std::fs::write(output.join(format!("{name}-{label}.txt")),format!("Native renderer snapshot; not interactive simulator.\nOwns runtime Back: {}\nDiagnostics: {:#?}\nLayout: {:#?}\n",screen.owns_back,screen.diagnostics(&metrics,&chrome),layout)).unwrap();
            };
            capture("queue-50",&context);
            app.on_action(&mut context,action_id("settings"));
            capture("settings",&context);
            app.on_action(&mut context,action_id("server"));
            capture("address",&context);
            app.on_action(&mut context,ActionId::BACK);
            app.on_action(&mut context,ActionId::BACK);
            app.on_action(&mut context,action_id("entry-0"));
            capture("article",&context);
        }
    }
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkout", type=Path, default=next(parent for parent in Path(__file__).resolve().parents if (parent / "crates/kobo-sdk").is_dir()))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.checkout.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = root / "apps/readlater/src/main.rs"
    dependencies = ("html", "json", "sdk", "image", "text", "ui")
    with tempfile.TemporaryDirectory(prefix="readlater-native-") as temporary:
        harness = Path(temporary)
        (harness / "src").mkdir()
        manifest = '[package]\nname = "cobalt-readlater-native-audit"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n[workspace]\n[dependencies]\n'
        for name in dependencies:
            manifest += f'kobo-{name} = {{ path = "{root}/crates/kobo-{name}" }}\n'
        (harness / "Cargo.toml").write_text(manifest)
        shutil.copyfile(root / "Cargo.lock", harness / "Cargo.lock")
        (harness / "src/lib.rs").write_text(RUST.replace("SOURCE_PATH", str(source)))
        subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(harness / "Cargo.toml"), "native_review_snapshots", "--", "--nocapture"], check=True, env=dict(os.environ, CAPTURE_DIR=str(output)))
    provenance = {
        "kind": "native-renderer-snapshot-not-interactive-simulator",
        "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "app": "readlater", "profile": "CLARA_BW_METRICS", "scales": ["Default", "Largest"],
        "renderer": "kobo_ui::render_with", "fonts": "kobo_text::install(metrics)",
        "chrome": "Chrome::for_screen with representative Chrome::measuring status, then ensure_way_back",
        "fixture": "50 original synthetic articles at example.org; no live service, account, or network",
        "interactions": "native app callbacks for settings, address editing, Back twice, and opening an article; no simulator socket or touch-coordinate path",
        "files": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(output.glob("*.png"))},
    }
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")


if __name__ == "__main__":
    main()
