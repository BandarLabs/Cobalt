//! Atomic frame capture envelope: little-endian u32 metadata length, UTF-8 JSON,
//! then exact grayscale pixels. Metadata and pixels describe one committed frame.

use kobo_json::{ObjectBuilder as Object, Value};
use kobo_ui::Screen;

pub const MAX_METADATA: usize = 128 * 1024;

#[derive(Clone, Debug, Default)]
pub struct CaptureSource {
    pub revision: Option<String>,
    pub dirty: Option<bool>,
    pub binary_sha256: Option<String>,
    pub fixture: Option<String>,
    /// A requested app fixture seed, not a claim that arbitrary apps use it.
    pub seed: Option<u64>,
}
impl CaptureSource {
    pub(super) fn valid(&self) -> bool {
        let digest = |value: &Option<String>, lengths: &[usize]| {
            value.as_ref().is_none_or(|v| {
                lengths.contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit())
            })
        };
        digest(&self.revision, &[40, 64])
            && digest(&self.binary_sha256, &[64])
            && self
                .fixture
                .as_ref()
                .is_none_or(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
    }
    fn value(&self) -> Value {
        Object::new()
            .set("revision", optional(self.revision.as_deref()))
            .set("dirty", self.dirty.map_or(Value::Null, Value::Bool))
            .set("binarySha256", optional(self.binary_sha256.as_deref()))
            .set("fixture", optional(self.fixture.as_deref()))
            .set(
                "requestedSeed",
                self.seed
                    .map_or(Value::Null, |seed| Value::from(seed.to_string())),
            )
            .build()
    }
}
fn optional(text: Option<&str>) -> Value {
    text.map_or(Value::Null, Value::from)
}

pub(super) struct View<'a> {
    pub app: &'a str,
    pub mode: &'a str,
    pub screen: &'a Screen,
    pub source: &'a CaptureSource,
    pub paints: u64,
    pub orientation: kobo_ui::Orientation,
    pub simulation: String,
    pub frame: &'a [u8],
    pub ideal: bool,
}
impl View<'_> {
    pub fn pack(self) -> std::io::Result<Vec<u8>> {
        let fonts = kobo_text::installed_sources().map_or(Value::Null, |fonts| {
            Value::Array(
                fonts
                    .iter()
                    .map(|(role, source)| {
                        Object::new()
                            .set("role", role.as_str())
                            .set("source", source.as_str())
                            .build()
                    })
                    .collect(),
            )
        });
        let metadata = Object::new()
            .set("schema", "cobalt.simulator-capture")
            .set("version", 1_u32)
            .set("app", self.app)
            .set("mode", self.mode)
            .set("runtimeVersion", env!("CARGO_PKG_VERSION"))
            .set("source", self.source.value())
            .set("fonts", fonts)
            .set(
                "publisherFontHandle",
                self.screen
                    .reading_font
                    .map_or(Value::Null, |handle| Value::from(handle.0)),
            )
            .set(
                "interfaceScalePercent",
                super::profile_metrics().text_scale.percent(),
            )
            .set(
                "readingScalePercent",
                self.screen.text_scale.unwrap_or_default().percent(),
            )
            .set(
                "orientation",
                match self.orientation {
                    kobo_ui::Orientation::Portrait => "portrait",
                    kobo_ui::Orientation::Landscape => "landscape",
                },
            )
            .set("paints", self.paints.to_string())
            .set(
                "simulation",
                kobo_json::parse(&self.simulation)
                    .map_err(|error| std::io::Error::other(error.to_string()))?,
            )
            .set(
                "frame",
                Object::new()
                    .set("format", "grey8")
                    .set(
                        "view",
                        if self.ideal {
                            "ideal"
                        } else {
                            "approximate-panel"
                        },
                    )
                    .set("sha256", kobo_net::sha256::hex_digest(self.frame))
                    .build(),
            )
            .build()
            .to_json()
            .into_bytes();
        if metadata.len() > MAX_METADATA {
            return Err(std::io::Error::other("capture metadata exceeds limit"));
        }
        let mut bytes = Vec::with_capacity(4 + metadata.len() + self.frame.len());
        bytes.extend_from_slice(
            &u32::try_from(metadata.len())
                .map_err(|_| std::io::Error::other("capture metadata length"))?
                .to_le_bytes(),
        );
        bytes.extend(metadata);
        bytes.extend_from_slice(self.frame);
        Ok(bytes)
    }
}
