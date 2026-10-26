#![forbid(unsafe_code)]
//! First-party metadata shared by simulator and owner tooling.

use kobo_json::Value;

include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

#[derive(Clone, Debug)]
pub struct App {
    pub id: String,
    pub title: String,
    pub label: String,
    pub summary: String,
    pub version: String,
    pub glyph: String,
    pub capabilities: Vec<String>,
    pub setup: Option<Value>,
}

impl App {
    /// Parses the same manifest used to publish a first-party catalog entry.
    ///
    /// # Errors
    /// Returns an explanation for malformed identity or capability metadata.
    pub fn parse(source: &str) -> Result<Self, String> {
        let value = kobo_json::parse(source).map_err(|e| e.to_string())?;
        let field = |name| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("app metadata needs {name}"))
        };
        let id = field("id")?;
        if id.len() > 32
            || !id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err("invalid app identity".into());
        }
        let capabilities = value
            .get("capabilities")
            .and_then(Value::as_array)
            .ok_or("app metadata needs capabilities")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or("invalid capability".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        kobo_policy::Declared::parse(capabilities.iter().map(String::as_str))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            id,
            title: field("display_name")?,
            label: field("short_label")?,
            summary: field("summary")?,
            version: field("version")?,
            glyph: field("glyph")?,
            capabilities,
            setup: value.get("setup").cloned(),
        })
    }
}

/// Catalog contents at build time, sorted by application identity.
///
/// # Errors
/// Refuses malformed or duplicate entries instead of silently omitting them.
pub fn bundled() -> Result<Vec<App>, String> {
    let mut apps = SOURCES
        .iter()
        .map(|s| App::parse(s))
        .collect::<Result<Vec<_>, _>>()?;
    apps.sort_by(|a, b| a.id.cmp(&b.id));
    if apps.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("duplicate app identity".into());
    }
    Ok(apps)
}

#[cfg(test)]
mod tests {
    #[test]
    fn published_manifests_are_valid_and_unique() {
        let apps = super::bundled().expect("valid catalog");
        assert!(apps.len() >= 44);
        assert!(apps.iter().any(|a| a.id == "panels"));
        assert!(apps
            .iter()
            .find(|a| a.id == "sudoku")
            .unwrap()
            .capabilities
            .is_empty());
    }
}
