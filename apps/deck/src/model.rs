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
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunResult {
    pub status: String,
    pub exit: Option<i64>,
    pub tail: String,
}

impl Deck {
    pub fn fallback() -> Self {
        Self {
            version: 0,
            pages: vec![Page {
                name: "Deck".into(),
                keys: vec![],
            }],
            error: None,
        }
    }
}

pub fn decode(text: &str) -> Option<Deck> {
    let value = kobo_json::parse(text).ok()?;
    let version = value.get("version").and_then(|version| {
        version
            .as_str()
            .and_then(|version| version.parse().ok())
            .or_else(|| {
                version
                    .as_i64()
                    .and_then(|version| u64::try_from(version).ok())
            })
    })?;
    let pages = value
        .get("pages")?
        .as_array()?
        .iter()
        .filter_map(|page| {
            let name = page.get("name")?.as_str()?.to_owned();
            let keys = page
                .get("keys")?
                .as_array()?
                .iter()
                .filter_map(|key| {
                    let id = key.get("id")?.as_str()?.to_owned();
                    let label = key.get("label")?.as_str()?.to_owned();
                    (!label.is_empty()).then(|| Key {
                        id,
                        label,
                        detail: key
                            .get("detail")
                            .and_then(kobo_json::Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        confirm: key.get("confirm").and_then(kobo_json::Value::as_bool)
                            == Some(true),
                        state: key
                            .get("state")
                            .and_then(kobo_json::Value::as_str)
                            .unwrap_or("idle")
                            .to_owned(),
                    })
                })
                .collect();
            Some(Page { name, keys })
        })
        .collect::<Vec<_>>();
    (!pages.is_empty()).then(|| Deck {
        version,
        pages,
        error: value
            .get("error")
            .and_then(kobo_json::Value::as_str)
            .map(str::to_owned),
    })
}

pub fn decode_result(text: &str) -> Option<RunResult> {
    let value = kobo_json::parse(text).ok()?;
    Some(RunResult {
        status: value.get("status")?.as_str()?.to_owned(),
        exit: value.get("exit").and_then(kobo_json::Value::as_i64),
        tail: value
            .get("tail")
            .and_then(kobo_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_result};

    #[test]
    fn pages_do_not_absorb_keys_from_the_pages_after_them() {
        let deck = decode(
            r#"{"version":"3","pages":[{"name":"Build","keys":[{"id":"a","label":"Test","detail":"","confirm":false,"state":"idle"}]},{"name":"Home","keys":[{"id":"b","label":"Lights","detail":"Downstairs","confirm":true,"state":"failed"}]}]}"#,
        )
        .unwrap();
        assert_eq!(deck.pages.len(), 2);
        assert_eq!(deck.pages[0].keys.len(), 1);
        assert_eq!(deck.pages[0].keys[0].id, "a");
        assert_eq!(deck.pages[1].keys[0].id, "b");
    }

    #[test]
    fn result_tail_and_exit_are_decoded() {
        let result = decode_result(r#"{"status":"failed","exit":7,"tail":"last line"}"#).unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(result.exit, Some(7));
        assert_eq!(result.tail, "last line");
    }
}
