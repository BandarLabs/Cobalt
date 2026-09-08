//! Bounded versioned records. Decoding and migration never write or erase the
//! source; apps acknowledge the migrated replacement through `draft::Draft`.

use kobo_json::{ObjectBuilder, Value};

pub const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_MIGRATIONS: u32 = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    TooLarge,
    Corrupt,
    WrongSchema,
    NewerVersion,
    MigrationUnavailable,
    InvalidSchema,
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooLarge => {
                "This saved item is too large to read. Keep a copy before replacing it."
            }
            Self::Corrupt => "This saved item could not be read. Keep a copy before replacing it.",
            Self::WrongSchema => "This saved item belongs to a different collection.",
            Self::NewerVersion => "Update the app to open this saved item.",
            Self::MigrationUnavailable => {
                "This saved item needs an earlier app update before it can be opened."
            }
            Self::InvalidSchema => "The app requested an unsupported record format.",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq)]
pub struct Restored {
    pub payload: Value,
    /// True means a replacement still needs an acknowledged save. A decoded
    /// value is not itself evidence that migration was durably committed.
    pub migrated: bool,
}

#[derive(Clone, Debug)]
pub struct Schema {
    name: String,
    version: u32,
    limit: usize,
}
impl Schema {
    /// # Errors
    /// Rejects an empty/invalid schema, zero version or an excessive byte cap.
    pub fn new(name: &str, version: u32, limit: usize) -> Result<Self, Error> {
        if name.is_empty()
            || name.len() > 96
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
            || version == 0
            || limit == 0
            || limit > MAX_RECORD_BYTES
        {
            return Err(Error::InvalidSchema);
        }
        Ok(Self {
            name: name.into(),
            version,
            limit,
        })
    }

    /// # Errors
    /// Refuses an oversized or non-round-tripping JSON payload.
    pub fn encode(&self, payload: &Value) -> Result<Vec<u8>, Error> {
        self.encode_version(payload, self.version)
    }

    fn encode_version(&self, payload: &Value, version: u32) -> Result<Vec<u8>, Error> {
        let value = ObjectBuilder::new()
            .set("schema", self.name.as_str())
            .set("version", version)
            .set("payload", payload.clone())
            .build();
        let bytes = value.to_json().into_bytes();
        if bytes.len() > self.limit {
            return Err(Error::TooLarge);
        }
        // Reject invalid numbers/depth or ambiguous object fields even when
        // constructed by application code rather than a JSON parser.
        let roundtrip = kobo_json::parse(std::str::from_utf8(&bytes).map_err(|_| Error::Corrupt)?)
            .map_err(|_| Error::Corrupt)?;
        if roundtrip != value {
            return Err(Error::Corrupt);
        }
        reject_duplicates(&roundtrip)?;
        Ok(bytes)
    }

    /// Decode an existing record. Only `None` is expected first launch. An
    /// empty byte slice is a damaged record, never permission to reset it.
    /// Each migration converts precisely version N to N+1, at most 32 steps.
    ///
    /// # Errors
    /// Refuses wrong/future schemas, corrupt data, missing migrations and
    /// excessive output without modifying the caller's bytes.
    pub fn restore(
        &self,
        bytes: Option<&[u8]>,
        mut migrate: impl FnMut(u32, Value) -> Result<Value, Error>,
    ) -> Result<Option<Restored>, Error> {
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if bytes.len() > self.limit {
            return Err(Error::TooLarge);
        }
        let value = kobo_json::parse(std::str::from_utf8(bytes).map_err(|_| Error::Corrupt)?)
            .map_err(|_| Error::Corrupt)?;
        reject_duplicates(&value)?;
        if value.get("schema").and_then(Value::as_str) != Some(&self.name) {
            return Err(Error::WrongSchema);
        }
        let number = value
            .get("version")
            .and_then(Value::as_f64)
            .ok_or(Error::Corrupt)?;
        if !number.is_finite()
            || number.fract() != 0.0
            || number < 1.0
            || number > f64::from(u32::MAX)
        {
            return Err(Error::Corrupt);
        }
        let version = value
            .get("version")
            .and_then(Value::as_i64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(Error::Corrupt)?;
        if version > self.version {
            return Err(Error::NewerVersion);
        }
        if self.version - version > MAX_MIGRATIONS {
            return Err(Error::MigrationUnavailable);
        }
        let mut payload = value.get("payload").ok_or(Error::Corrupt)?.clone();
        for step in version..self.version {
            payload = migrate(step, payload)?;
            self.encode_version(&payload, step + 1)?;
        }
        Ok(Some(Restored {
            payload,
            migrated: version != self.version,
        }))
    }
}

fn reject_duplicates(value: &Value) -> Result<(), Error> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(fields) => {
                let mut keys = std::collections::BTreeSet::new();
                for (key, child) in fields {
                    if !keys.insert(key) {
                        return Err(Error::Corrupt);
                    }
                    pending.push(child);
                }
            }
            Value::Array(values) => pending.extend(values),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_retains_source_until_a_separate_save_is_acknowledged() {
        let old = Schema::new("article", 1, 1024).unwrap();
        let source = old.encode(&Value::from("Original article")).unwrap();
        let current = Schema::new("article", 3, 1024).unwrap();
        let mut steps = Vec::new();
        let restored = current
            .restore(Some(&source), |version, payload| {
                steps.push(version);
                Ok(ObjectBuilder::new().set("previous", payload).build())
            })
            .unwrap()
            .unwrap();
        assert_eq!(steps, vec![1, 2]);
        assert!(restored.migrated);
        assert_eq!(
            old.restore(Some(&source), |_, _| unreachable!())
                .unwrap()
                .unwrap()
                .payload,
            Value::from("Original article")
        );
        let replacement = current.encode(&restored.payload).unwrap();
        assert!(
            !current
                .restore(Some(&replacement), |_, _| unreachable!())
                .unwrap()
                .unwrap()
                .migrated
        );
        assert_eq!(
            old.restore(Some(&replacement), |_, _| unreachable!()),
            Err(Error::NewerVersion)
        );
    }
    #[test]
    fn absent_empty_ambiguous_future_and_oversized_are_not_conflated() {
        let schema = Schema::new("article", 1, 1024).unwrap();
        assert_eq!(schema.restore(None, |_, _| unreachable!()), Ok(None));
        assert_eq!(
            schema.restore(Some(b""), |_, _| unreachable!()),
            Err(Error::Corrupt)
        );
        for source in [
            r#"{"schema":"article","version":1.5,"payload":null}"#,
            r#"{"schema":"article","version":1,"version":2,"payload":null}"#,
            r#"{"schema":"article","version":1,"payload":{"x":1,"x":2}}"#,
        ] {
            assert_eq!(
                schema.restore(Some(source.as_bytes()), |_, _| unreachable!()),
                Err(Error::Corrupt)
            );
        }
        assert_eq!(
            schema.restore(Some(&vec![0; 1025]), |_, _| unreachable!()),
            Err(Error::TooLarge)
        );
        assert_eq!(
            schema.encode(&Value::from("x".repeat(1024))),
            Err(Error::TooLarge)
        );
    }
    #[test]
    fn migration_failure_and_excess_output_leave_input_available() {
        let source = Schema::new("article", 1, 1024)
            .unwrap()
            .encode(&Value::from("keep"))
            .unwrap();
        let schema = Schema::new("article", 2, 1024).unwrap();
        assert_eq!(
            schema.restore(Some(&source), |_, _| Err(Error::MigrationUnavailable)),
            Err(Error::MigrationUnavailable)
        );
        assert_eq!(
            schema.restore(Some(&source), |_, _| Ok(Value::from("x".repeat(2048)))),
            Err(Error::TooLarge)
        );
        assert!(String::from_utf8(source).unwrap().contains("keep"));
    }
}
