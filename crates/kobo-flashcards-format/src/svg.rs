use crate::FormatError;
use std::sync::{Arc, OnceLock};

const MAX_SVG_NODES: usize = 20_000;
const MAX_SVG_BYTES: usize = 512 * 1024;
const MAX_SVG_INTRINSIC_DIMENSION: f32 = 16_384.0;
const MAX_SVG_RASTER_WIDTH: u32 = 960;
const MAX_SVG_RASTER_HEIGHT: u32 = 650;

#[allow(
    clippy::cast_precision_loss,
    reason = "raster dimensions are bounded before conversion to the resvg scale"
)]
/// Produces the deterministic greyscale PNG used for an accepted SVG source.
///
/// # Errors
///
/// Returns an error when the SVG violates parser, resolver, font, node, byte,
/// dimension, or image-memory bounds.
pub fn rasterize_svg(bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
    let text = normalized_svg(bytes)?;
    let options = safe_svg_options();
    let tree = resvg::usvg::Tree::from_data(text.as_bytes(), &options)
        .map_err(|_| invalid_svg("source cannot be rendered safely"))?;
    let source_size = tree.size();
    if source_size.width() > MAX_SVG_INTRINSIC_DIMENSION
        || source_size.height() > MAX_SVG_INTRINSIC_DIMENSION
        || f64::from(source_size.width()) * f64::from(source_size.height())
            > kobo_image::MAX_PIXELS as f64
    {
        return Err(invalid_svg(
            "intrinsic dimensions exceed the image memory bound",
        ));
    }
    let size = tree.size().to_int_size();
    let (width, height) = kobo_image::fitted_size(
        (size.width(), size.height()),
        MAX_SVG_RASTER_WIDTH,
        MAX_SVG_RASTER_HEIGHT,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| invalid_svg("dimensions are empty or too large"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width() as f32,
            height as f32 / size.height() as f32,
        ),
        &mut pixmap.as_mut(),
    );
    let grey = pixmap
        .pixels()
        .iter()
        .map(|pixel| {
            let alpha = u32::from(pixel.alpha());
            let premultiplied_luma = (77 * u32::from(pixel.red())
                + 150 * u32::from(pixel.green())
                + 29 * u32::from(pixel.blue())
                + 128)
                / 256;
            u8::try_from(premultiplied_luma + 255 - alpha).unwrap_or(255)
        })
        .collect::<Vec<_>>();
    kobo_image::encode_png_grey(width, height, &grey)
        .map_err(|_| invalid_svg("raster cannot be encoded for Kobo rendering"))
}

/// Checks an SVG against the same policy used by [`rasterize_svg`].
///
/// # Errors
///
/// Returns an error for unsafe, external, executable, malformed, or unbounded
/// SVG input.
pub fn validate_svg_source(bytes: &[u8]) -> Result<(), FormatError> {
    normalized_svg(bytes).map(|_| ())
}

fn safe_svg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        resources_dir: None,
        font_family: "Atkinson Hyperlegible".to_owned(),
        fontdb: safe_font_db(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    }
}

fn normalized_svg(bytes: &[u8]) -> Result<String, FormatError> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(invalid_svg("source exceeds the parser input bound"));
    }
    let mut text = std::str::from_utf8(bytes)
        .map_err(|_| invalid_svg("source is not UTF-8"))?
        .trim_start_matches('\u{feff}')
        .to_owned();
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!entity") {
        return Err(invalid_svg("entity declarations are not supported"));
    }
    if lower.contains("<!doctype") {
        text = strip_svg_doctype(text)?;
    }
    text = sanitize_metadata_names(&text);
    let document =
        roxmltree::Document::parse(&text).map_err(|_| invalid_svg("source is malformed"))?;
    if document.root_element().tag_name().name() != "svg"
        || document.descendants().count() > MAX_SVG_NODES
    {
        return Err(invalid_svg("root or node count is outside the bound"));
    }
    validate_svg_elements(&document)?;
    validate_svg_text_glyphs(&document)?;
    Ok(text)
}

fn sanitize_metadata_names(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find('<') {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let terminator = if remaining.starts_with("<!--") {
            "-->"
        } else if remaining.starts_with("<![CDATA[") {
            "]]>"
        } else if remaining.starts_with("<?") {
            "?>"
        } else {
            ""
        };
        if !terminator.is_empty() {
            let Some(end) = remaining.find(terminator) else {
                output.push_str(remaining);
                return output;
            };
            let end = end + terminator.len();
            output.push_str(&remaining[..end]);
            remaining = &remaining[end..];
            continue;
        }
        let Some(end) = xml_tag_end(remaining) else {
            output.push_str(remaining);
            return output;
        };
        output.push_str(&sanitize_metadata_tag(&remaining[..end]));
        remaining = &remaining[end..];
    }
    output.push_str(remaining);
    output
}

fn xml_tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in tag.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if character == '>' {
            return Some(offset + character.len_utf8());
        }
    }
    None
}

fn sanitize_metadata_tag(tag: &str) -> String {
    const PREFIXES: [(&str, &str); 3] = [
        ("kvg:", "metadata-kvg-"),
        ("inkscape:", "metadata-inkscape-"),
        ("sodipodi:", "metadata-sodipodi-"),
    ];
    let mut output = String::with_capacity(tag.len());
    let mut offset = 0;
    let mut quote = None;
    while offset < tag.len() {
        let remaining = &tag[offset..];
        let character = remaining.chars().next().expect("offset is in bounds");
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if let Some((prefix, replacement)) = PREFIXES
            .iter()
            .find(|(prefix, _)| remaining.starts_with(prefix))
        {
            output.push_str(replacement);
            offset += prefix.len();
            continue;
        }
        output.push(character);
        offset += character.len_utf8();
    }
    output
}

fn validate_svg_elements(document: &roxmltree::Document<'_>) -> Result<(), FormatError> {
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let element = node.tag_name().name().to_ascii_lowercase();
        if matches!(
            element.as_str(),
            "script"
                | "foreignobject"
                | "iframe"
                | "object"
                | "embed"
                | "audio"
                | "video"
                | "image"
                | "animate"
                | "animatemotion"
                | "animatetransform"
                | "set"
        ) {
            return Err(invalid_svg(
                "active or externally resolved elements are not supported",
            ));
        }
        for attribute in node.attributes() {
            let name = attribute.name().to_ascii_lowercase();
            let value = attribute.value().trim();
            if name.starts_with("on") {
                return Err(invalid_svg("event handlers are not supported"));
            }
            if matches!(name.as_str(), "href" | "src")
                && !value.is_empty()
                && !value.starts_with('#')
            {
                return Err(invalid_svg(
                    "external, relative, absolute, and data references are not supported",
                ));
            }
            validate_svg_css(value)?;
        }
        if element == "style" {
            validate_svg_css(node.text().unwrap_or_default())?;
        }
    }
    Ok(())
}

fn validate_svg_text_glyphs(document: &roxmltree::Document<'_>) -> Result<(), FormatError> {
    let fonts = safe_font_db();
    for node in document.descendants().filter(roxmltree::Node::is_text) {
        let in_text = node
            .ancestors()
            .filter(roxmltree::Node::is_element)
            .any(|ancestor| ancestor.tag_name().name() == "text");
        if !in_text {
            continue;
        }
        for character in node.text().unwrap_or_default().chars() {
            let available = character.is_whitespace()
                || fonts.faces().any(|face| {
                    fonts
                        .with_face_data(face.id, |data, index| {
                            ttf_parser::Face::parse(data, index)
                                .ok()
                                .and_then(|face| face.glyph_index(character))
                                .is_some()
                        })
                        .unwrap_or(false)
                });
            if !available {
                return Err(invalid_svg(
                    "text needs a glyph outside the bundled deterministic font set",
                ));
            }
        }
    }
    Ok(())
}

fn strip_svg_doctype(mut text: String) -> Result<String, FormatError> {
    let lower = text.to_ascii_lowercase();
    let start = lower
        .find("<!doctype")
        .ok_or_else(|| invalid_svg("document type could not be located"))?;
    if lower[start + 2..].contains("<!doctype") {
        return Err(invalid_svg("source contains more than one document type"));
    }
    let mut quote = None;
    let mut brackets = 0_u32;
    let mut end = None;
    for (offset, character) in text[start..].char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '[' => brackets = brackets.saturating_add(1),
            ']' => brackets = brackets.saturating_sub(1),
            '>' if brackets == 0 => {
                end = Some(start + offset + character.len_utf8());
                break;
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| invalid_svg("document type is unterminated"))?;
    text.replace_range(start..end, "");
    Ok(text)
}

fn validate_svg_css(value: &str) -> Result<(), FormatError> {
    let lower = value.to_ascii_lowercase();
    if lower.contains("@import")
        || lower.contains("@font-face")
        || lower.contains("expression(")
        || lower.contains("javascript:")
        || lower.contains("file:")
    {
        return Err(invalid_svg(
            "stylesheet contains an external or executable construct",
        ));
    }
    let mut remaining = lower.as_str();
    while let Some(start) = remaining.find("url(") {
        let after = &remaining[start + 4..];
        let Some(end) = after.find(')') else {
            return Err(invalid_svg("stylesheet has an unterminated URL"));
        };
        let target = after[..end].trim().trim_matches(['"', '\'']);
        if !target.starts_with('#') {
            return Err(invalid_svg("stylesheet URL is not an internal fragment"));
        }
        remaining = &after[end + 1..];
    }
    Ok(())
}

fn safe_font_db() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = resvg::usvg::fontdb::Database::new();
            fonts.load_font_data(kobo_text::TEXT_FONT.to_vec());
            fonts.load_font_data(kobo_text::DISPLAY_FONT.to_vec());
            fonts.load_font_data(kobo_text::MONO_FONT.to_vec());
            fonts.set_serif_family("Atkinson Hyperlegible");
            fonts.set_sans_serif_family("Atkinson Hyperlegible");
            fonts.set_monospace_family("DejaVu Sans Mono");
            Arc::new(fonts)
        })
        .clone()
}

fn invalid_svg(message: &str) -> FormatError {
    FormatError::InvalidSvg(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::rasterize_svg;

    #[test]
    fn namespace_words_in_visible_text_are_not_rewritten() {
        let original = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="180" height="20"><g kvg:element="demo"><text x="1" y="14">inkscape:demo</text></g></svg>"#,
        )
        .expect("original text");
        let rewritten = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="180" height="20"><text x="1" y="14">metadata-inkscape-demo</text></svg>"#,
        )
        .expect("different text");
        assert_ne!(original, rewritten);
    }
}
