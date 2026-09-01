use kobo_sdk::{Credential, Task};

pub const SECRET: &str = "homeassistant";

pub fn endpoint(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub fn test_connection(base: &str) -> Task {
    Task::Fetch {
        url: endpoint(base, "/api/"),
        offset: 0,
        max_bytes: 4096,
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
    }
}

pub fn poll(base: &str, ids: &[String]) -> Task {
    let names = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "")))
        .collect::<Vec<_>>()
        .join(",");
    let template = format!(
        "[{{% for e in [{names}] %}}\
{{\"id\":\"{{{{e}}}}\",\"s\":\"{{{{states(e)}}}}\",\
\"a\":{{{{{{'brightness':state_attr(e,'brightness'),'unit':state_attr(e,'unit_of_measurement')}}|tojson}}}}\
}}{{{{',' if not loop.last}}}}{{% endfor %}}]"
    );
    Task::Post {
        url: endpoint(base, "/api/template"),
        body: template,
        content_type: "text/plain".to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
        max_bytes: 32 * 1024,
    }
}

pub fn service(base: &str, entity: &str) -> Task {
    let domain = entity.split('.').next().unwrap_or("homeassistant");
    let action = match domain {
        "scene" | "script" | "automation" => "turn_on",
        "button" => "press",
        _ => "toggle",
    };
    Task::Post {
        url: endpoint(base, &format!("/api/services/{domain}/{action}")),
        body: format!(r#"{{"entity_id":"{entity}"}}"#),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
        max_bytes: 4096,
    }
}

pub fn state_rows(bytes: &[u8]) -> Vec<(String, String)> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(items) = kobo_json::parse(text) else {
        return Vec::new();
    };
    items.as_array().map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?.to_owned();
                let state = item.get("s")?.as_str()?.to_owned();
                Some((id, state))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_poll_is_one_credentialed_post_for_all_tiles() {
        let Task::Post {
            url,
            body,
            credential,
            ..
        } = poll(
            "https://ha.example/",
            &["light.desk".into(), "lock.front".into()],
        )
        else {
            panic!("a post")
        };
        assert_eq!(url, "https://ha.example/api/template");
        assert!(body.contains("light.desk") && body.contains("lock.front"));
        assert_eq!(credential.expect("secret").secret, SECRET);
    }

    #[test]
    fn compact_template_answer_keeps_only_id_and_state() {
        assert_eq!(
            state_rows(br#"[{"id":"light.desk","s":"on","a":{}}]"#),
            vec![("light.desk".into(), "on".into())]
        );
    }
}
