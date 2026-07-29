//! Pure request construction and response parsing for the three providers.

use kobo_json::{parse, ObjectBuilder, Value};
use kobo_sdk::{Credential, Header, Task};

pub const EXA_ENDPOINT: &str = "https://api.exa.ai/search";
pub const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/responses";
pub const ELEVENLABS_ENDPOINT: &str = concat!(
    "https://api.elevenlabs.io/v1/text-to-speech/",
    "JBFqnCBsd6RMkjVDRZzb?output_format=mp3_22050_32"
);

const MAX_RESEARCH_BYTES: usize = 180 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Book {
    pub title: String,
    pub summary: String,
    pub chapters: Vec<(String, String)>,
}

pub fn research(topic: &str) -> Task {
    let schema = ObjectBuilder::new()
        .set("type", "object")
        .set(
            "properties",
            ObjectBuilder::new()
                .set(
                    "research",
                    ObjectBuilder::new().set("type", "string").set(
                        "description",
                        "A detailed, source-grounded research brief with inline source URLs.",
                    ),
                )
                .set(
                    "sources",
                    ObjectBuilder::new().set("type", "array").set(
                        "items",
                        ObjectBuilder::new()
                            .set("type", "object")
                            .set(
                                "properties",
                                ObjectBuilder::new()
                                    .set("title", ObjectBuilder::new().set("type", "string"))
                                    .set("url", ObjectBuilder::new().set("type", "string")),
                            )
                            .set("required", vec!["title", "url"])
                            .set("additionalProperties", false),
                    ),
                )
                .build(),
        )
        .set("required", vec!["research", "sources"])
        .set("additionalProperties", false)
        .build();
    let body = ObjectBuilder::new()
        .set("query", topic)
        .set("type", "deep")
        .set("numResults", 12_u32)
        .set("moderation", true)
        .set(
            "systemPrompt",
            "Research this topic for a factual, engaging general-audience audiobook. Prefer primary and authoritative sources, represent uncertainty, include dates where relevant, and do not copy long passages.",
        )
        .set("outputSchema", schema)
        .build()
        .to_json();
    Task::Post {
        url: EXA_ENDPOINT.to_owned(),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(Credential::in_header("exa", "x-api-key")),
        headers: Vec::new(),
        max_bytes: 256 * 1024,
    }
}

pub fn write_book(topic: &str, exa_response: &[u8]) -> Result<Task, &'static str> {
    let research = research_context(exa_response)?;
    let schema = ObjectBuilder::new()
        .set("type", "object")
        .set(
            "properties",
            ObjectBuilder::new()
                .set("title", ObjectBuilder::new().set("type", "string"))
                .set("summary", ObjectBuilder::new().set("type", "string"))
                .set(
                    "chapters",
                    ObjectBuilder::new()
                        .set("type", "array")
                        .set("minItems", 3_u32)
                        .set("maxItems", 5_u32)
                        .set(
                            "items",
                            ObjectBuilder::new()
                                .set("type", "object")
                                .set(
                                    "properties",
                                    ObjectBuilder::new()
                                        .set("title", ObjectBuilder::new().set("type", "string"))
                                        .set(
                                            "narration",
                                            ObjectBuilder::new().set("type", "string"),
                                        ),
                                )
                                .set("required", vec!["title", "narration"])
                                .set("additionalProperties", false),
                        ),
                )
                .build(),
        )
        .set("required", vec!["title", "summary", "chapters"])
        .set("additionalProperties", false)
        .build();
    let instructions = "You are an audiobook editor. Turn the supplied Exa research into an original, accurate 8–12 minute audiobook for a curious general audience. Use 3–5 chapters, a strong spoken opening, smooth transitions, and a brief conclusion. Write for the ear: no markdown, URLs, footnote markers, lists, or visual references. Paraphrase sources; do not reproduce passages. Clearly qualify disputed or uncertain claims.";
    let user = format!("Topic requested by the listener: {topic}\n\nExa research:\n{research}");
    let body = ObjectBuilder::new()
        .set("model", "gpt-5.6-sol")
        .set("instructions", instructions)
        .set("input", user)
        .set("reasoning", ObjectBuilder::new().set("effort", "medium"))
        .set("max_output_tokens", 8_000_u32)
        .set(
            "text",
            ObjectBuilder::new().set(
                "format",
                ObjectBuilder::new()
                    .set("type", "json_schema")
                    .set("name", "audiobook_script")
                    .set("strict", true)
                    .set("schema", schema),
            ),
        )
        .build()
        .to_json();
    Ok(Task::Post {
        url: OPENAI_ENDPOINT.to_owned(),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer("openai")),
        headers: Vec::new(),
        max_bytes: 128 * 1024,
    })
}

pub fn speech(text: &str) -> Task {
    let body = ObjectBuilder::new()
        .set("text", text)
        .set("model_id", "eleven_multilingual_v2")
        .set(
            "voice_settings",
            ObjectBuilder::new()
                .set("stability", 0.58)
                .set("similarity_boost", 0.78)
                .set("style", 0.12)
                .set("use_speaker_boost", true),
        )
        .build()
        .to_json();
    Task::Post {
        url: ELEVENLABS_ENDPOINT.to_owned(),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(Credential::in_header("elevenlabs", "xi-api-key")),
        headers: vec![Header::new("accept", "audio/mpeg")],
        max_bytes: 512 * 1024,
    }
}

pub fn parse_book(response: &[u8]) -> Result<Book, &'static str> {
    let response = std::str::from_utf8(response)
        .map_err(|_| "The script came back in a form this reader cannot read.")?;
    let envelope = parse(response).map_err(|_| "The script came back malformed.")?;
    let text = response_text(&envelope).ok_or("No script came back for that topic.")?;
    let script = parse(text).map_err(|_| "The script came back malformed.")?;
    let title = string(&script, "title")?;
    let summary = string(&script, "summary")?;
    let chapters = script
        .get("chapters")
        .and_then(Value::as_array)
        .ok_or("the script had no chapters")?
        .iter()
        .filter_map(|chapter| {
            Some((
                chapter.get("title")?.as_str()?.trim().to_owned(),
                chapter.get("narration")?.as_str()?.trim().to_owned(),
            ))
        })
        .filter(|(title, narration)| !title.is_empty() && !narration.is_empty())
        .take(5)
        .collect::<Vec<_>>();
    if chapters.len() < 3 {
        return Err("That topic did not yield enough material for an audiobook.");
    }
    Ok(Book {
        title,
        summary,
        chapters,
    })
}

pub fn narration_parts(book: &Book) -> Vec<String> {
    let mut parts = Vec::new();
    for (title, narration) in &book.chapters {
        let spoken = format!("{title}.\n\n{narration}");
        parts.extend(split_spoken(&spoken, 1_000));
    }
    parts
}

fn research_context(response: &[u8]) -> Result<String, &'static str> {
    let text = std::str::from_utf8(response)
        .map_err(|_| "The research came back in a form this reader cannot read.")?;
    let parsed = parse(text).map_err(|_| "The research came back malformed.")?;
    let output = parsed
        .get("output")
        .ok_or("No sources were found for that topic.")?;
    let mut context = output.to_json();
    if let Some(grounding) = parsed.get("results") {
        context.push_str("\nSearch results: ");
        grounding.write_json(&mut context);
    }
    if context.len() > MAX_RESEARCH_BYTES {
        let mut end = MAX_RESEARCH_BYTES;
        while !context.is_char_boundary(end) {
            end -= 1;
        }
        context.truncate(end);
    }
    Ok(context)
}

fn response_text(response: &Value) -> Option<&str> {
    for item in response.get("output")?.as_array()? {
        for content in item.get("content")?.as_array()? {
            if content.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn string(value: &Value, key: &str) -> Result<String, &'static str> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("the script omitted required text")?;
    Ok(value.to_owned())
}

fn split_spoken(text: &str, max_bytes: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let hard_end = (start + max_bytes).min(text.len());
        let mut end = hard_end;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len() {
            let window = &text[start..end];
            if let Some(boundary) = window
                .char_indices()
                .rev()
                .find(|(_, character)| matches!(character, '.' | '!' | '?' | '\n'))
                .map(|(index, character)| index + character.len_utf8())
                .filter(|boundary| *boundary >= max_bytes / 2)
            {
                end = start + boundary;
            }
        }
        let part = text[start..end].trim();
        if !part.is_empty() {
            parts.push(part.to_owned());
        }
        start = end;
        while text[start..].starts_with(char::is_whitespace) {
            start += text[start..].chars().next().map_or(0, char::len_utf8);
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::{
        narration_parts, parse_book, research, speech, split_spoken, write_book, Book,
        ELEVENLABS_ENDPOINT, EXA_ENDPOINT, OPENAI_ENDPOINT,
    };
    use kobo_sdk::{SecretHeader, Task};

    fn post(task: Task) -> (String, String, kobo_sdk::Credential) {
        let Task::Post {
            url,
            body,
            credential: Some(credential),
            ..
        } = task
        else {
            panic!("provider work must be an authenticated POST");
        };
        (url, body, credential)
    }

    #[test]
    fn provider_requests_use_the_expected_endpoints_shapes_and_secret_names() {
        let (url, body, credential) = post(research("volcanoes"));
        assert_eq!(url, EXA_ENDPOINT);
        assert!(body.contains("\"type\":\"deep\""));
        assert_eq!(&*credential.secret, "exa");
        assert!(matches!(credential.header, SecretHeader::Named(ref name) if name == "x-api-key"));

        let exa = br#"{"output":{"research":"brief","sources":[]},"results":[]}"#;
        let (url, body, credential) = post(write_book("volcanoes", exa).expect("research"));
        assert_eq!(url, OPENAI_ENDPOINT);
        assert!(body.contains("\"model\":\"gpt-5.6-sol\""));
        assert!(body.contains("\"type\":\"json_schema\""));
        assert_eq!(&*credential.secret, "openai");
        assert_eq!(credential.header, SecretHeader::Bearer);

        let (url, body, credential) = post(speech("A spoken sentence."));
        assert_eq!(url, ELEVENLABS_ENDPOINT);
        assert!(body.contains("\"model_id\":\"eleven_multilingual_v2\""));
        assert_eq!(&*credential.secret, "elevenlabs");
        assert!(matches!(credential.header, SecretHeader::Named(ref name) if name == "xi-api-key"));
    }

    #[test]
    fn a_responses_envelope_yields_the_structured_script() {
        let script = r#"{"title":"The Moon","summary":"A tour","chapters":[{"title":"One","narration":"First."},{"title":"Two","narration":"Second."},{"title":"Three","narration":"Third."}]}"#;
        let envelope = kobo_json::ObjectBuilder::new()
            .set(
                "output",
                vec![kobo_json::ObjectBuilder::new().set(
                    "content",
                    vec![kobo_json::ObjectBuilder::new()
                        .set("type", "output_text")
                        .set("text", script)],
                )],
            )
            .build()
            .to_json();
        let book = parse_book(envelope.as_bytes()).expect("a book");
        assert_eq!(book.title, "The Moon");
        assert_eq!(book.chapters.len(), 3);
    }

    #[test]
    fn narration_is_cut_at_spoken_boundaries_under_the_transport_budget() {
        let text = "A sentence. ".repeat(300);
        let parts = split_spoken(&text, 1_000);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|part| part.len() <= 1_000));
        assert!(parts.iter().all(|part| part.ends_with('.')));
    }

    #[test]
    fn every_chapter_title_is_spoken() {
        let book = Book {
            title: "Book".into(),
            summary: "Summary".into(),
            chapters: vec![("Opening".into(), "Words".into())],
        };
        assert!(narration_parts(&book)[0].starts_with("Opening."));
    }
}
