//! Portable Cobalt probe evidence. Observations never authorize device writes.
//!
//! Measured geometry is kept apart from the profile inferred from it. A
//! synthetic fixture is always labeled synthetic, and untested timings,
//! waveforms and touch calibration remain unverified.

use crate::{Bitfield, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, TouchSnapshot};
use kobo_json::{ObjectBuilder as Object, Value};

pub const MAX_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    DeviceProbe,
    SyntheticFixture,
}

impl Origin {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::DeviceProbe => "cobalt-device-probe",
            Self::SyntheticFixture => "synthetic-fixture",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub origin: Origin,
    /// Unix seconds, encoded as a decimal string to retain integer precision.
    pub captured_at: u64,
    pub snapshot: DeviceSnapshot,
    /// Explicitly observed runtime backends. Empty means none verified; it
    /// never means every backend is available. Names use manifest spelling.
    pub available_backends: Vec<String>,
}

impl Observation {
    /// Captures only the read-only probe's facts. Service availability must be
    /// observed independently by the runtime that owns those services.
    #[must_use]
    pub fn probe(snapshot: DeviceSnapshot, captured_at: u64) -> Self {
        Self {
            origin: Origin::DeviceProbe,
            captured_at,
            snapshot,
            available_backends: Vec::new(),
        }
    }

    /// Serializes bounded facts with explicit evidence provenance.
    ///
    /// # Errors
    /// Refuses data that cannot round-trip through the bounded schema.
    pub fn to_json(&self) -> Result<String, String> {
        let profile = crate::identify_profile(&self.snapshot);
        let inferred = profile.map_or(Value::Null, |p| {
            Object::new()
                .set("profile", p.id)
                .set("touch_transform", format!("{:?}", p.touch_transform))
                .set(
                    "basis",
                    "Cobalt profile match; not a new physical calibration",
                )
                .build()
        });
        let result = Object::new()
            .set("schema", "cobalt.hardware-observation")
            .set("version", 1_u32)
            .set("origin", self.origin.name())
            .set("captured_at", self.captured_at.to_string())
            .set("observed", snapshot_value(&self.snapshot))
            .set("inferred", inferred)
            .set(
                "available_backends",
                self.available_backends
                    .iter()
                    .map(|s| Value::from(s.as_str()))
                    .collect::<Vec<_>>(),
            )
            .set(
                "unverified",
                vec![
                    Value::from("refresh timing and ghosting"),
                    Value::from("physical corner calibration"),
                    Value::from("sleep and power consumption"),
                    Value::from("color and inversion calibration"),
                ],
            )
            .build()
            .to_json();
        Self::parse(&result)?;
        Ok(result)
    }

    /// Reads a versioned observation without trusting its inferred claims.
    ///
    /// # Errors
    /// Refuses future schemas, invalid ranges, excessive records and strings.
    pub fn parse(source: &str) -> Result<Self, String> {
        if source.len() > MAX_BYTES {
            return Err("hardware observation exceeds 32 KiB".into());
        }
        let value = kobo_json::parse(source).map_err(|e| e.to_string())?;
        if string(&value, "schema")? != "cobalt.hardware-observation"
            || number(&value, "version")? != 1
        {
            return Err("unsupported hardware observation schema".into());
        }
        let origin = match string(&value, "origin")?.as_str() {
            "cobalt-device-probe" => Origin::DeviceProbe,
            "synthetic-fixture" => Origin::SyntheticFixture,
            _ => return Err("unknown hardware observation origin".into()),
        };
        let captured_at = string(&value, "captured_at")?
            .parse()
            .map_err(|_| "invalid capture time")?;
        let measured = field(&value, "observed")?;
        let identity = field(measured, "identity")?;
        let prefix = optional_string(identity, "serial_prefix")?;
        if prefix
            .as_ref()
            .is_some_and(|s| s.len() != 4 || !s.is_ascii())
        {
            return Err("only a four-character model prefix may be recorded".into());
        }
        let snapshot = DeviceSnapshot {
            compatible: strings(measured, "compatible", 32)?,
            model: optional_string(measured, "model")?,
            framebuffer: optional(measured, "framebuffer", parse_framebuffer)?,
            touch: optional(measured, "touch", parse_touch)?,
            identity: IdentitySnapshot {
                serial_prefix: prefix,
                firmware_version: optional_string(identity, "firmware_version")?,
                kernel_release: optional_string(identity, "kernel_release")?,
                device_code: optional(identity, "device_code", |v| {
                    u16::try_from(integer(v)?).map_err(|_| "invalid device code".into())
                })?,
            },
        };
        let available_backends = strings(&value, "available_backends", 32)?;
        let mut unique = std::collections::BTreeSet::new();
        for name in &available_backends {
            if name.is_empty()
                || name.len() > 32
                || !name.bytes().all(|c| c.is_ascii_lowercase() || c == b'-')
                || !unique.insert(name)
            {
                return Err("invalid or duplicate backend name".into());
            }
        }
        Ok(Self {
            origin,
            captured_at,
            snapshot,
            available_backends,
        })
    }
}

fn optional<T>(
    value: &Value,
    key: &str,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Result<Option<T>, String> {
    match field(value, key)? {
        Value::Null => Ok(None),
        value => parse(value).map(Some),
    }
}
fn optional_string(value: &Value, key: &str) -> Result<Option<String>, String> {
    optional(value, key, text)
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("missing observation field {key}"))
}
fn text(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|s| s.len() <= 512 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| "invalid observation text".into())
}
fn string(value: &Value, key: &str) -> Result<String, String> {
    text(field(value, key)?)
}
fn integer(value: &Value) -> Result<i64, String> {
    let number = value.as_f64().ok_or("invalid observation number")?;
    if !number.is_finite() || number.fract() != 0.0 {
        return Err("invalid observation integer".into());
    }
    value
        .as_i64()
        .ok_or_else(|| "invalid observation integer".into())
}
fn number(value: &Value, key: &str) -> Result<u32, String> {
    u32::try_from(integer(field(value, key)?)?).map_err(|_| format!("invalid {key}"))
}
fn signed(value: &Value, key: &str) -> Result<i32, String> {
    i32::try_from(integer(field(value, key)?)?).map_err(|_| format!("invalid {key}"))
}
fn strings(value: &Value, key: &str, limit: usize) -> Result<Vec<String>, String> {
    let values = field(value, key)?
        .as_array()
        .ok_or("invalid observation list")?;
    if values.len() > limit {
        return Err("observation list exceeds limit".into());
    }
    values.iter().map(text).collect()
}
fn maybe_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn snapshot_value(s: &DeviceSnapshot) -> Value {
    Object::new()
        .set(
            "compatible",
            s.compatible
                .iter()
                .map(|v| Value::from(v.as_str()))
                .collect::<Vec<_>>(),
        )
        .set("model", maybe_text(s.model.as_deref()))
        .set(
            "framebuffer",
            s.framebuffer
                .as_ref()
                .map_or(Value::Null, framebuffer_value),
        )
        .set(
            "touch",
            s.touch.as_ref().map_or(Value::Null, |t| {
                Object::new()
                    .set("name", t.name.as_str())
                    .set("path", t.path.as_str())
                    .set("x_min", t.x_min)
                    .set("x_max", t.x_max)
                    .set("y_min", t.y_min)
                    .set("y_max", t.y_max)
                    .build()
            }),
        )
        .set(
            "identity",
            Object::new()
                .set(
                    "serial_prefix",
                    maybe_text(s.identity.serial_prefix.as_deref()),
                )
                .set(
                    "firmware_version",
                    maybe_text(s.identity.firmware_version.as_deref()),
                )
                .set(
                    "kernel_release",
                    maybe_text(s.identity.kernel_release.as_deref()),
                )
                .set(
                    "device_code",
                    s.identity
                        .device_code
                        .map_or(Value::Null, |v| Value::from(u32::from(v))),
                )
                .build(),
        )
        .build()
}

macro_rules! framebuffer_fields {
    ($f:ident, $($name:ident),*) => {
        fn framebuffer_value($f: &FramebufferSnapshot) -> Value {
            Object::new().set("id", $f.id.as_str())$(.set(stringify!($name), $f.$name))*
                .set("red", bitfield_value($f.red)).set("green", bitfield_value($f.green))
                .set("blue", bitfield_value($f.blue)).set("alpha", bitfield_value($f.alpha)).build()
        }
        fn parse_framebuffer($f: &Value) -> Result<FramebufferSnapshot, String> {
            Ok(FramebufferSnapshot { id: string($f, "id")?, $($name: number($f, stringify!($name))?,)*
                red: parse_bitfield(field($f, "red")?)?, green: parse_bitfield(field($f, "green")?)?,
                blue: parse_bitfield(field($f, "blue")?)?, alpha: parse_bitfield(field($f, "alpha")?)? })
        }
    };
}
framebuffer_fields!(
    f,
    width,
    height,
    virtual_width,
    virtual_height,
    x_offset,
    y_offset,
    bits_per_pixel,
    grayscale,
    stride,
    memory_length,
    kind,
    visual,
    rotation
);
fn bitfield_value(v: Bitfield) -> Value {
    Object::new()
        .set("offset", v.offset)
        .set("length", v.length)
        .set("msb_right", v.msb_right)
        .build()
}
fn parse_bitfield(v: &Value) -> Result<Bitfield, String> {
    Ok(Bitfield {
        offset: number(v, "offset")?,
        length: number(v, "length")?,
        msb_right: number(v, "msb_right")?,
    })
}
fn parse_touch(v: &Value) -> Result<TouchSnapshot, String> {
    Ok(TouchSnapshot {
        name: string(v, "name")?,
        path: string(v, "path")?,
        x_min: signed(v, "x_min")?,
        x_max: signed(v, "x_max")?,
        y_min: signed(v, "y_min")?,
        y_max: signed(v, "y_max")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_synthetic_fixture_round_trips_and_keeps_its_origin() {
        let source = include_str!("../../../docs/quality/fixtures/clara-bw-synthetic.json");
        let observation = Observation::parse(source).unwrap();
        assert_eq!(observation.origin, Origin::SyntheticFixture);
        assert_eq!(
            Observation::parse(&observation.to_json().unwrap()).unwrap(),
            observation
        );
        assert_eq!(
            crate::identify_profile(&observation.snapshot).unwrap().id,
            "clara-bw-391"
        );
        assert!(observation
            .to_json()
            .unwrap()
            .contains("\"inferred\":{\"profile\":\"clara-bw-391\""));
    }
    #[test]
    fn probe_round_trip_keeps_missing_facts_unknown_and_never_invents_backends() {
        let observation = Observation::probe(DeviceSnapshot::default(), u64::MAX);
        let json = observation.to_json().unwrap();
        assert_eq!(Observation::parse(&json).unwrap(), observation);
        assert!(json.contains("\"inferred\":null"));
        assert!(json.contains("\"available_backends\":[]"));
    }
    #[test]
    fn schema_and_private_identity_are_bounded() {
        let mut observation = Observation::probe(DeviceSnapshot::default(), 0);
        let json = observation.to_json().unwrap();
        assert!(Observation::parse(&json.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(Observation::parse(&" ".repeat(MAX_BYTES + 1)).is_err());
        observation.snapshot.identity.serial_prefix = Some("serial-too-long".into());
        assert!(observation.to_json().is_err());
        observation.snapshot.identity.serial_prefix = None;
        observation.available_backends = vec!["audio".into(), "audio".into()];
        assert!(observation.to_json().is_err());
    }
}
