#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub confirm: bool,
    pub state: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page {
    pub name: String,
    pub keys: Vec<Key>,
}
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Deck {
    pub version: u64,
    pub pages: Vec<Page>,
}
impl Deck {
    pub fn fallback() -> Self {
        Self {
            version: 0,
            pages: vec![Page {
                name: "Deck".into(),
                keys: vec![],
            }],
        }
    }
}
/// Decodes the daemon's deliberately small response shape. Unknown fields are ignored.
pub fn decode(text: &str) -> Option<Deck> {
    let version = value(text, "version")?.parse().ok()?;
    let mut pages = Vec::new();
    for segment in text.split("\"name\":").skip(1) {
        let name = quoted(segment)?;
        let mut keys = Vec::new();
        for key_part in segment.split("\"id\":").skip(1) {
            let id = quoted(key_part)?;
            let label = value(key_part, "label").unwrap_or("").to_owned();
            if label.is_empty() {
                continue;
            }
            let detail = value(key_part, "detail").unwrap_or("").to_owned();
            let confirm = value(key_part, "confirm").unwrap_or("false") == "true";
            let state = value(key_part, "state").unwrap_or("idle").to_owned();
            keys.push(Key {
                id: id.to_owned(),
                label,
                detail,
                confirm,
                state,
            });
        }
        pages.push(Page {
            name: name.to_owned(),
            keys,
        });
    }
    (!pages.is_empty()).then_some(Deck { version, pages })
}
fn quoted(text: &str) -> Option<&str> {
    let text = text.trim_start_matches([' ', ':']);
    let text = text.strip_prefix('\"')?;
    Some(text.split('\"').next()?)
}
fn value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let rest = text.split_once(&format!("\"{name}\":"))?.1;
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('\"') {
        Some(stripped.split('\"').next()?)
    } else {
        Some(rest.split([',', '}']).next()?.trim())
    }
}
