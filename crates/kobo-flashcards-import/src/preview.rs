//! Offline preview of validated neutral card content; no template HTML executes.
use kobo_flashcards_format::{decode, ParsedBundle};
use std::{fmt::Write as _, fs, io::Write as _, path::Path};

pub fn write(input: &Path, output: &Path, number: usize) -> Result<(), String> {
    super::verify_bundle(input).map_err(|error| error.to_string())?;
    let bytes = fs::read(input).map_err(|error| error.to_string())?;
    let bundle = decode(&bytes).map_err(|error| error.to_string())?;
    let manifest = bundle.manifest();
    let card = number
        .checked_sub(1)
        .and_then(|index| manifest.cards.get(index))
        .ok_or_else(|| format!("Choose a card from 1 to {}", manifest.cards.len()))?;
    let mut html = String::from(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'"><title>Flashcards preview</title><style>
body{margin:0;background:#f4f3ef;color:#222;font:18px/1.5 system-ui,sans-serif}main{max-width:960px;margin:auto;padding:36px 24px}h1{font-size:28px;margin:0 0 8px}p{margin:8px 0 24px}.sides{display:grid;grid-template-columns:1fr 1fr;gap:24px}section{background:white;border:1px solid #aaa;padding:24px;min-width:0}h2{font-size:14px;text-transform:uppercase;letter-spacing:.08em;color:#555;margin:0 0 24px}.text{font-size:24px;white-space:pre-wrap;overflow-wrap:anywhere}img{max-width:100%;max-height:440px;object-fit:contain;margin-top:24px}.media{font-size:14px;color:#555;overflow-wrap:anywhere}footer{margin-top:24px;color:#555;font-size:15px}@media(max-width:640px){.sides{grid-template-columns:1fr}main{padding:24px 16px}}</style><main><h1>Flashcards preview</h1>"#,
    );
    write!(
        html,
        "<p>Card {number} of {} · {} due · {} media files</p><div class=\"sides\">",
        manifest.cards.len(),
        manifest.review_queue.card_ids.len(),
        manifest.media.len()
    )
    .unwrap();
    side(
        &mut html,
        "Front",
        &card.front,
        &card.question_media_names,
        &bundle,
    );
    side(
        &mut html,
        "Back",
        &card.back,
        &card.answer_media_names,
        &bundle,
    );
    html.push_str("</div><footer>Preview of imported content. Reader layout may differ. This does not install a collection or record a review.</footer></main></html>");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| {
            format!(
                "Create preview {}: {error}. Choose a new output filename.",
                output.display()
            )
        })?;
    if let Err(error) = file
        .write_all(html.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(output);
        return Err(format!("Write preview: {error}"));
    }
    println!("Previewed card {number} of {} at {}. Open this file in your browser. Use --card NUMBER to inspect another card.", manifest.cards.len(), output.display());
    Ok(())
}

fn side(html: &mut String, title: &str, text: &str, names: &[String], bundle: &ParsedBundle) {
    write!(
        html,
        "<section><h2>{title}</h2><div class=\"text\">{}</div>",
        escape(text)
    )
    .unwrap();
    for name in names {
        if let Some(media) = bundle
            .manifest()
            .media
            .iter()
            .find(|media| &media.name == name)
        {
            if matches!(media.mime.as_str(), "image/png" | "image/jpeg") {
                if let Some(bytes) = bundle.media(name) {
                    write!(
                        html,
                        "<img alt=\"{}\" src=\"data:{};base64,{}\">",
                        escape(name),
                        media.mime,
                        base64(bytes)
                    )
                    .unwrap();
                }
            }
            write!(
                html,
                "<p class=\"media\">{} · {} bytes</p>",
                escape(name),
                media.length
            )
            .unwrap();
        }
    }
    html.push_str("</section>");
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn card_content_cannot_become_markup() {
        assert_eq!(
            super::escape("<script>\"&'</script>"),
            "&lt;script&gt;&quot;&amp;&#39;&lt;/script&gt;"
        );
    }
    #[test]
    fn image_encoding_handles_partial_groups() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
    }
}
