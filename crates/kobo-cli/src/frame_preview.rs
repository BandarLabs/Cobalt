//! Offline comparison of the same bounded preparation used by Frame transfers.
use kobo_frame_host::{prepare_for_panel, Fit, Manifest, Panel};
use std::{fmt::Write as _, fs, path::Path};

const USAGE: &str = "usage: kobo frame preview INPUT --out DIRECTORY [--profile PROFILE]\nCompares crop and pad locally. No photos are transferred.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    let (input, output, profile) = match arguments {
        [input, flag, output] if flag == "--out" => (input, output, "clara-bw-391"),
        [input, flag, output, selector, profile] if flag == "--out" && selector == "--profile" => {
            (input, output, profile.as_str())
        }
        _ => return Err(USAGE.into()),
    };
    let profile = kobo_profile::SUPPORTED_PROFILES
        .iter()
        .find(|candidate| candidate.id == profile)
        .ok_or_else(|| {
            "Unknown reader profile. Use a supported Cobalt profile identifier.".to_owned()
        })?;
    generate(
        Path::new(input),
        Path::new(output),
        Panel {
            width: profile.width,
            height: profile.height,
        },
    )
}

fn generate(input: &Path, output: &Path, panel: Panel) -> Result<(), String> {
    // Prepare before creating the destination, so rejected sources leave no partial preview.
    let crop = prepare_for_panel(input, Fit::Crop, &Manifest::default(), false, panel)?;
    let pad = prepare_for_panel(input, Fit::Pad, &Manifest::default(), false, panel)?;
    if crop.photos.len() != pad.photos.len()
        || crop
            .photos
            .iter()
            .zip(&pad.photos)
            .any(|(left, right)| left.photo.digest != right.photo.digest)
    {
        return Err("Photos changed while preparing the comparison. Run preview again.".into());
    }
    fs::create_dir(output)
        .map_err(|error| format!("Create preview directory: {error}. Choose a new directory."))?;
    let result = write_preview(output, panel, &crop, &pad);
    if result.is_err() {
        // This directory was exclusively created above; never remove an existing owner directory.
        let _ = fs::remove_dir_all(output);
    }
    result?;
    println!("Prepared {} photos at {} × {}. Open {} to compare crop and pad. No photos were transferred.",
             crop.photos.len(), panel.width, panel.height, output.join("index.html").display());
    Ok(())
}

fn write_preview(
    output: &Path,
    panel: Panel,
    crop: &kobo_frame_host::Push,
    pad: &kobo_frame_host::Push,
) -> Result<(), String> {
    let bytes = |push: &kobo_frame_host::Push| {
        push.photos
            .iter()
            .filter_map(|photo| photo.png.as_ref())
            .map(Vec::len)
            .sum::<usize>()
    };
    let mut html = String::from(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self'; style-src 'unsafe-inline'"><title>Frame photo preview</title><style>body{margin:0;background:#f4f3ef;color:#222;font:17px/1.5 system-ui,sans-serif}main{max-width:960px;margin:auto;padding:32px 24px}h1{font-size:28px}h2{font-size:21px;margin-top:36px;overflow-wrap:anywhere}.pair{display:grid;grid-template-columns:1fr 1fr;gap:24px}figure{margin:0;min-width:0}img{width:auto;max-width:100%;max-height:44vh;height:auto;display:block;border:1px solid #999;box-sizing:border-box}figcaption{margin:8px 0}p{overflow-wrap:anywhere}@media(max-width:640px){.pair{gap:12px}main{padding:24px 12px}}</style><main><h1>Frame photo preview</h1>"#,
    );
    write!(html, "<p>{} photos · {} × {} pixels</p><p>Prepared image storage: crop {} bytes · pad {} bytes. Manifest storage is additional.</p><p>Crop fills the screen and trims the edges. Pad keeps the whole photo with white borders. These files are local previews.</p>", crop.photos.len(), panel.width, panel.height, bytes(crop), bytes(pad)).unwrap();
    for (index, photo) in crop.photos.iter().enumerate() {
        write!(
            html,
            "<h2>{}</h2><p>Album: {}</p><div class=\"pair\">",
            escape(&photo.photo.name),
            escape(&photo.photo.album)
        )
        .unwrap();
        for (label, prepared) in [("Crop", &crop.photos[index]), ("Pad", &pad.photos[index])] {
            let filename = format!("{index}-{}.png", label.to_lowercase());
            let png = prepared
                .png
                .as_ref()
                .ok_or("Prepared preview image is missing")?;
            fs::write(output.join(&filename), png)
                .map_err(|error| format!("Write preview image: {error}"))?;
            write!(html, "<figure><figcaption>{label}</figcaption><a href=\"{filename}\"><img src=\"{filename}\" alt=\"{}: {}\"></a></figure>", label, escape(&prepared.photo.name)).unwrap();
        }
        html.push_str("</div>");
    }
    html.push_str("</main></html>");
    fs::write(output.join("index.html"), html)
        .map_err(|error| format!("Write preview page: {error}"))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_uses_distinct_fits_and_preserves_an_existing_destination() {
        let root = std::env::temp_dir().join(format!("frame-preview-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("sample.png");
        fs::write(
            &input,
            kobo_image::encode_png_grey(640, 320, &vec![0; 640 * 320]).unwrap(),
        )
        .unwrap();
        let output = root.join("preview");
        generate(
            &input,
            &output,
            Panel {
                width: 64,
                height: 96,
            },
        )
        .unwrap();
        let crop = fs::read(output.join("0-crop.png")).unwrap();
        let pad = fs::read(output.join("0-pad.png")).unwrap();
        assert_ne!(crop, pad);
        for png in [&crop, &pad] {
            let decoded = kobo_image::decode(png).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (64, 96));
        }
        let original = fs::read(output.join("index.html")).unwrap();
        assert!(generate(
            &input,
            &output,
            Panel {
                width: 64,
                height: 96
            }
        )
        .is_err());
        assert_eq!(fs::read(output.join("index.html")).unwrap(), original);
        let bad = root.join("bad.png");
        fs::write(&bad, b"broken image").unwrap();
        let rejected = root.join("rejected");
        assert!(generate(
            &bad,
            &rejected,
            Panel {
                width: 64,
                height: 96
            }
        )
        .is_err());
        assert!(!rejected.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
