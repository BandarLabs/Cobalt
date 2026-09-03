use kobo_sdk::{Credential, Task};

pub const SECRET: &str = "homeassistant";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub state: String,
}

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

pub fn entities(base: &str) -> Task {
    Task::Fetch {
        url: endpoint(base, "/api/states"),
        offset: 0,
        max_bytes: 1024 * 1024,
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
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

pub fn entity_rows(bytes: &[u8]) -> Vec<Entity> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(items) = kobo_json::parse(text) else {
        return Vec::new();
    };
    let mut entities = items.as_array().map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|item| {
                let id = item.get("entity_id")?.as_str()?.to_owned();
                let state = item.get("state")?.as_str()?.to_owned();
                let name = item
                    .get("attributes")
                    .and_then(|attributes| attributes.get("friendly_name"))
                    .and_then(kobo_json::Value::as_str)
                    .map_or_else(
                        || id.rsplit('.').next().unwrap_or(&id).replace('_', " "),
                        str::to_owned,
                    );
                Some(Entity { id, name, state })
            })
            .collect()
    });
    entities.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    entities
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

    #[test]
    fn entity_picker_uses_friendly_names_and_sorts_them() {
        let rows = entity_rows(
            br#"[
                {"entity_id":"switch.z_desk","state":"off","attributes":{}},
                {"entity_id":"light.kitchen","state":"on","attributes":{"friendly_name":"Kitchen"}}
            ]"#,
        );
        assert_eq!(
            rows,
            vec![
                Entity {
                    id: "light.kitchen".into(),
                    name: "Kitchen".into(),
                    state: "on".into(),
                },
                Entity {
                    id: "switch.z_desk".into(),
                    name: "z desk".into(),
                    state: "off".into(),
                },
            ]
        );
    }
}
