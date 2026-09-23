use kobo_sdk::{Credential, Task};

pub const SECRET: &str = "homeassistant";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub state: String,
}

/// What a climate entity reports: the room temperature and the temperature
/// it is holding, when Home Assistant publishes them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Climate {
    pub current: Option<f64>,
    pub target: Option<f64>,
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
\"a\":{{{{{{'brightness':state_attr(e,'brightness'),'unit':state_attr(e,'unit_of_measurement'),\
'ct':state_attr(e,'current_temperature'),'t':state_attr(e,'temperature')}}|tojson}}}}\
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

/// Discovery goes through the template endpoint rather than /api/states:
/// the credential policy allows the panel's bearer token on exactly the
/// connection test, the template, and service calls, and a template can
/// answer the same question.
pub fn entities(base: &str) -> Task {
    let template = concat!(
        "[{% for e in states %}",
        "{\"id\":{{ e.entity_id|tojson }},",
        "\"s\":{{ e.state|tojson }},",
        "\"n\":{{ e.attributes.friendly_name|default(e.entity_id, true)|tojson }}}",
        "{% if not loop.last %},{% endif %}{% endfor %}]"
    );
    Task::Post {
        url: endpoint(base, "/api/template"),
        body: template.to_owned(),
        content_type: "text/plain".to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
        max_bytes: 1024 * 1024,
    }
}

/// Sensors and the other domains that only ever report are readings.
/// Everything else keeps its control.
///
/// Named the other way round on purpose. Home Assistant gains domains, and a
/// list of the ones that may be triggered would quietly take the controls off
/// a cover or a media player the day somebody adds one.
#[must_use]
pub fn can_trigger(entity: &str) -> bool {
    !matches!(
        entity.split('.').next(),
        Some(
            "sensor"
                | "binary_sensor"
                | "person"
                | "device_tracker"
                | "sun"
                | "weather"
                | "zone"
                | "update"
                | "calendar"
                | "image"
                | "camera"
        )
    )
}

pub fn service(base: &str, entity: &str) -> Option<Task> {
    if !can_trigger(entity) {
        return None;
    }
    let domain = entity.split('.').next().unwrap_or("homeassistant");
    let action = match domain {
        "scene" | "script" | "automation" => "turn_on",
        "button" | "input_button" => "press",
        _ => "toggle",
    };
    Some(Task::Post {
        url: endpoint(base, &format!("/api/services/{domain}/{action}")),
        body: format!(r#"{{"entity_id":"{entity}"}}"#),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
        max_bytes: 4096,
    })
}

pub fn set_temperature(base: &str, entity: &str, value: f64) -> Task {
    Task::Post {
        url: endpoint(base, "/api/services/climate/set_temperature"),
        body: format!(r#"{{"entity_id":"{entity}","temperature":{value}}}"#),
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

pub fn climate_rows(bytes: &[u8]) -> Vec<(String, Climate)> {
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
                let attrs = item.get("a")?;
                let current = attrs.get("ct").and_then(kobo_json::Value::as_f64);
                let target = attrs.get("t").and_then(kobo_json::Value::as_f64);
                Some((id, Climate { current, target }))
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
                let id = item.get("id")?.as_str()?.to_owned();
                let state = item.get("s")?.as_str()?.to_owned();
                let name = item
                    .get("n")
                    .and_then(kobo_json::Value::as_str)
                    .filter(|name| !name.is_empty())
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
    fn climate_answer_reads_room_and_target_temperatures() {
        let rows = climate_rows(
            br#"[
                {"id":"climate.bedroom","s":"heat","a":{"ct":19.5,"t":21}},
                {"id":"light.desk","s":"on","a":{"ct":null,"t":null}}
            ]"#,
        );
        assert_eq!(
            rows,
            vec![
                (
                    "climate.bedroom".to_owned(),
                    Climate {
                        current: Some(19.5),
                        target: Some(21.0),
                    },
                ),
                (
                    "light.desk".to_owned(),
                    Climate {
                        current: None,
                        target: None,
                    },
                ),
            ]
        );
    }

    #[test]
    fn set_temperature_posts_the_named_target() {
        let Task::Post { url, body, .. } =
            set_temperature("https://ha.example", "climate.bedroom", 21.5)
        else {
            panic!("a post")
        };
        assert_eq!(
            url,
            "https://ha.example/api/services/climate/set_temperature"
        );
        assert_eq!(
            body,
            r#"{"entity_id":"climate.bedroom","temperature":21.5}"#
        );
    }

    #[test]
    fn entity_picker_uses_friendly_names_and_sorts_them() {
        let rows = entity_rows(
            br#"[
                {"id":"switch.z_desk","s":"off","n":""},
                {"id":"light.kitchen","s":"on","n":"Kitchen"}
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

    #[test]
    fn discovery_is_a_template_post_the_credential_policy_allows() {
        let Task::Post {
            url,
            body,
            credential,
            ..
        } = entities("https://ha.example/")
        else {
            panic!("a post")
        };
        assert_eq!(url, "https://ha.example/api/template");
        assert!(body.contains("for e in states"), "{body}");
        assert_eq!(credential.expect("secret").secret, SECRET);
    }

    #[test]
    fn sensors_are_readings_and_are_not_toggled() {
        assert!(can_trigger("light.kitchen"));
        assert!(can_trigger("climate.bedroom"));
        assert!(!can_trigger("sensor.office"));
        assert!(!can_trigger("binary_sensor.door"));
        assert!(service("https://ha.example", "sensor.office").is_none());
        assert!(service("https://ha.example", "light.kitchen").is_some());
    }

    /// A tile that could be switched before must not quietly stop switching.
    ///
    /// These all posted a service call until the readings were separated out.
    /// A list of what may be triggered turns every domain nobody thought of
    /// into a tile that answers a tap by explaining it does nothing.
    #[test]
    fn a_domain_that_is_not_a_reading_keeps_its_control() {
        for entity in [
            "cover.garage_door",
            "media_player.living_room",
            "humidifier.nursery",
            "valve.irrigation",
            "siren.alarm",
            "remote.tv",
            "group.downstairs",
        ] {
            assert!(can_trigger(entity), "{entity}");
            assert!(service("https://ha.example", entity).is_some(), "{entity}");
        }
    }
}
