use kobo_sdk::{Credential, Task};

pub const SECRET: &str = "hermes-post";
pub fn endpoint(gateway: &str, route: &str) -> String {
    format!("{}/{}", gateway.trim_end_matches('/'), route.trim_start_matches('/'))
}
pub fn inbox(gateway: &str) -> Task {
    Task::Fetch {
        url: endpoint(gateway, "/letters"),
        offset: 0, max_bytes: 64 * 1024,
        credential: Some(Credential::bearer(SECRET)), headers: Vec::new(),
    }
}
pub fn reply(gateway: &str, letter: &str, body: &str) -> Task {
    Task::Post {
        url: endpoint(gateway, "/replies"),
        body: format!(r#"{{"letter_id":"{}","body":"{}"}}"#, escape(letter), escape(body)),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer(SECRET)), headers: Vec::new(), max_bytes: 4096,
    }
}
fn escape(value: &str) -> String { value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n") }
pub fn letters(bytes: &[u8]) -> Vec<(String, String, String)> {
    let Ok(text) = std::str::from_utf8(bytes) else { return Vec::new() };
    let Ok(root) = kobo_json::parse(text) else { return Vec::new() };
    root.as_array().map_or_else(Vec::new, |items| items.iter().filter_map(|letter| Some((
        letter.get("id")?.as_str()?.to_owned(), letter.get("title")?.as_str()?.to_owned(),
        letter.get("body")?.as_str()?.to_owned()))).collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bearer_secret_is_named_not_embedded() {
        let Task::Fetch { credential: Some(secret), url, .. } = inbox("https://gateway.example") else { panic!("fetch") };
        assert_eq!(secret.secret, SECRET); assert!(!url.contains(SECRET));
    }
    #[test] fn reads_completed_letters() {
        assert_eq!(letters(br#"[{"id":"dawn","title":"Morning note","body":"Tea first."}]"#)[0].1, "Morning note");
    }
}
