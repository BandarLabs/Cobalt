//! Pure request construction and response parsing for the three providers.

use kobo_json::{parse, ObjectBuilder, Value};
use kobo_sdk::{Credential, Header, Task};

pub const EXA_ENDPOINT: &str = "https://api.exa.ai/agent/runs";
pub const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/responses";

/// The languages a book can be narrated in, each with a voice whose accent is
/// native to it.
///
/// A single multilingual voice can read all six, but reads five of them with
/// an English accent. These six are the most-used narration voices in the
/// `ElevenLabs` voice library for their language, chosen by ear. The runtime
/// holds the same list in its credential policy, so adding a language here
/// means adding its voice there.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Language {
    #[default]
    English,
    Hindi,
    Spanish,
    French,
    German,
    Chinese,
}

pub const LANGUAGES: [Language; 6] = [
    Language::English,
    Language::Hindi,
    Language::Spanish,
    Language::French,
    Language::German,
    Language::Chinese,
];

impl Language {
    /// The name a reader picks. Latin script throughout: the device's fonts
    /// carry no Devanagari or Han glyphs, and a chip of boxes says nothing.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Hindi => "Hindi",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::German => "Deutsch",
            Self::Chinese => "Chinese",
        }
    }

    /// The name the writing model is instructed with.
    #[must_use]
    pub const fn english_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Hindi => "Hindi",
            Self::Spanish => "Spanish",
            Self::French => "French",
            Self::German => "German",
            Self::Chinese => "Simplified Chinese",
        }
    }

    /// A stable word for building action names.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Hindi => "hindi",
            Self::Spanish => "spanish",
            Self::French => "french",
            Self::German => "german",
            Self::Chinese => "chinese",
        }
    }

    /// The narrator: George, Monika Sogam, Alberto Rodríguez, Nicolas, Lea
    /// and James Gao.
    #[must_use]
    pub const fn voice_id(self) -> &'static str {
        match self {
            Self::English => "JBFqnCBsd6RMkjVDRZzb",
            Self::Hindi => "1qEiC6qsybMkmnNdVMbK",
            Self::Spanish => "l1zE9xgNpUTaQCZzpNJa",
            Self::French => "aQROLel5sQbj1vuIVi6B",
            Self::German => "7eVMgwCnXydb3CikjV7a",
            Self::Chinese => "4VZIsMPtgggwNg7OXbPY",
        }
    }

    fn endpoint(self) -> String {
        format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}?output_format=mp3_44100_128",
            self.voice_id()
        )
    }
}

const MAX_RESEARCH_BYTES: usize = 180 * 1024;

/// How long the finished book is asked to run, in minutes.
///
/// One pair of numbers for the instructions the writer is given and for the
/// estimate the owner reads before paying, so the two cannot drift apart.
pub const BOOK_MINUTES: (u32, u32) = (8, 12);

/// The most the writer may produce, which is most of what writing costs.
pub const SCRIPT_TOKENS: u32 = 8_000;

/// An unhurried narrator's pace, and the characters a spoken word takes once
/// its space is counted. Narration is billed by the character, so this is
/// how a running time becomes a bill.
const WORDS_PER_MINUTE: u32 = 150;
const CHARACTERS_PER_WORD: u32 = 6;

/// What one thousand narrated characters costs, in US cents, from the
/// largest `ElevenLabs` plan to pay-as-you-go overage. Narration is most of
/// the bill, so it is the range worth stating.
const NARRATION_CENTS_PER_THOUSAND: (u32, u32) = (12, 30);

/// What the research run and the script cost together, in US cents. Both
/// are billed by the work the service chose to do, so this is a range taken
/// from typical runs rather than anything the reader can compute.
const RESEARCH_AND_WRITING_CENTS: (u32, u32) = (25, 115);

/// How long a book takes to make, start to saved, in minutes. Research runs
/// for one to three, the script for one or two, and narration for one to
/// three more.
const MAKING_MINUTES: (u32, u32) = (3, 8);

/// What a book will cost and how long it will take, before any of it is
/// spent. Every figure is a range, because the script's length is the
/// writer's choice and every account is on its own plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Estimate {
    pub minutes: (u32, u32),
    pub narration_characters: (u32, u32),
    pub dollars: (u32, u32),
}

impl Estimate {
    #[must_use]
    pub const fn for_a_book() -> Self {
        let characters = (
            BOOK_MINUTES.0 * WORDS_PER_MINUTE * CHARACTERS_PER_WORD,
            BOOK_MINUTES.1 * WORDS_PER_MINUTE * CHARACTERS_PER_WORD,
        );
        let cents = (
            characters.0 * NARRATION_CENTS_PER_THOUSAND.0 / 1_000 + RESEARCH_AND_WRITING_CENTS.0,
            characters.1 * NARRATION_CENTS_PER_THOUSAND.1 / 1_000 + RESEARCH_AND_WRITING_CENTS.1,
        );
        Self {
            minutes: MAKING_MINUTES,
            narration_characters: characters,
            // Rounded outwards, so the range never claims less than it is.
            dollars: (cents.0 / 100, cents.1.div_ceil(100)),
        }
    }

    /// The narration range in thousands of characters, as a person says it.
    /// Written as words rather than as "7,000", whose zeros the panel's
    /// figure style draws slashed.
    #[must_use]
    pub const fn narration_thousands(&self) -> (u32, u32) {
        (
            (self.narration_characters.0 + 500) / 1_000,
            (self.narration_characters.1 + 500) / 1_000,
        )
    }
}

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
    let query = format!(
        "Research this topic for a factual, engaging general-audience audiobook: {topic}. Prefer primary and authoritative sources, represent uncertainty, include dates where relevant, and do not copy long passages."
    );
    let body = ObjectBuilder::new()
        .set("query", query.as_str())
        .set("effort", "high")
        .set("outputSchema", schema)
        .build()
        .to_json();
    // An agent run takes minutes, so the answer is asked for as a stream of
    // events. The transport reads the stream to its end and hands the whole
    // transcript over; the terminal event carries the completed run. Without
    // the stream the endpoint answers immediately with a run id to poll, and
    // polling is a second URL, a credential rule per poll, and a clock, all
    // to reproduce what the socket already does by staying open. The events
    // arrive seconds apart with keep-alives between, so the read timeout
    // never comes near.
    Task::Post {
        url: EXA_ENDPOINT.to_owned(),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(Credential::in_header("exa", "x-api-key")),
        headers: vec![Header::new("accept", "text/event-stream")],
        max_bytes: 512 * 1024,
    }
}

pub fn write_book(
    topic: &str,
    language: Language,
    exa_response: &[u8],
) -> Result<Task, &'static str> {
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
    let instructions = format!("You are an audiobook editor. Turn the supplied Exa research into an original, accurate {} to {} minute audiobook for a curious general audience. Use 3–5 chapters, a strong spoken opening, smooth transitions, and a brief conclusion. Write for the ear: no markdown, URLs, footnote markers, lists, or visual references. Paraphrase sources; do not reproduce passages. Clearly qualify disputed or uncertain claims. Write the chapter titles and the narration in {}, whatever language the research is in. Write the book title and the summary in that language too, but strictly in Latin script (romanized if the language is not written in it): they are shown on a screen whose fonts have no other glyphs.", BOOK_MINUTES.0, BOOK_MINUTES.1, language.english_name());
    let user = format!("Topic requested by the listener: {topic}\n\nExa research:\n{research}");
    let body = ObjectBuilder::new()
        .set("model", "gpt-5.6-sol")
        .set("instructions", instructions)
        .set("input", user)
        .set("reasoning", ObjectBuilder::new().set("effort", "medium"))
        .set("max_output_tokens", SCRIPT_TOKENS)
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
        max_bytes: 512 * 1024,
    })
}

pub fn speech(text: &str, language: Language) -> Task {
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
        url: language.endpoint(),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(Credential::in_header("elevenlabs", "xi-api-key")),
        headers: vec![Header::new("accept", "audio/mpeg")],
        // A minute of 128 kbps MP3 is a megabyte; a slow, expressive
        // narrator can take a thousand-byte part well past that.
        max_bytes: 4 * 1024 * 1024,
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
    let run = completed_run(text)?;
    let structured = run
        .get("output")
        .and_then(|output| output.get("structured"))
        .ok_or("No sources were found for that topic.")?;
    let mut context = structured.to_json();
    if context.len() > MAX_RESEARCH_BYTES {
        let mut end = MAX_RESEARCH_BYTES;
        while !context.is_char_boundary(end) {
            end -= 1;
        }
        context.truncate(end);
    }
    Ok(context)
}

/// Finds the completed run in an agent event stream.
///
/// The transcript is server-sent events: frames of `event:` and `data:`
/// lines separated by blank lines, with `:` comment lines as keep-alives.
/// Progress events narrate the run as it goes; only the terminal frame
/// matters here, and its `data` payload is the finished run itself. A frame's
/// data may span several `data:` lines, which the format defines as one
/// payload joined by newlines.
fn completed_run(transcript: &str) -> Result<Value, &'static str> {
    let mut event = "";
    let mut data = String::new();
    let mut completed = None;
    for line in transcript.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            match event {
                "agent_run.completed" => completed = Some(std::mem::take(&mut data)),
                "agent_run.failed" => return Err("The research service gave up on that topic."),
                _ => {}
            }
            event = "";
            data.clear();
        } else if let Some(name) = line.strip_prefix("event:") {
            event = name.trim();
        } else if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
        }
    }
    let completed = completed.ok_or("The research never finished.")?;
    parse(&completed).map_err(|_| "The research came back malformed.")
}

/// Finds the written script in a Responses envelope.
///
/// Every item without content is skipped rather than ending the search. A
/// reasoning model puts its reasoning first and that item carries no `content`
/// at all, so a `?` here would return empty for every successful response: the
/// script is in the second item and the search stopped at the first. It looked
/// exactly like the model having said nothing, which is the worst kind of bug
/// to read a log for.
fn response_text(response: &Value) -> Option<&str> {
    let items = response.get("output")?.as_array()?;
    items
        .iter()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .find(|content| content.get("type").and_then(Value::as_str) == Some("output_text"))
        .and_then(|content| content.get("text").and_then(Value::as_str))
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
        narration_parts, parse_book, research, speech, split_spoken, write_book, Book, Language,
        EXA_ENDPOINT, LANGUAGES, OPENAI_ENDPOINT,
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

    /// The stream the agent endpoint answers with: progress frames, a
    /// keep-alive comment, and a terminal frame whose data is the whole
    /// completed run, split across two `data:` lines the way the format
    /// allows.
    fn sse_transcript() -> String {
        [
            "id: 1",
            "event: agent_run.created",
            r#"data: {"id":"agent_run_1","status":"queued"}"#,
            "",
            "id: 2",
            "event: agent_run.started",
            r#"data: {"id":"agent_run_1","status":"running"}"#,
            "",
            ": keep-alive",
            "",
            "id: 3",
            "event: agent_run.completed",
            r#"data: {"id":"agent_run_1","status":"completed","output":{"text":"an answer","structured":"#,
            r#"data: {"research":"brief","sources":[{"title":"A source","url":"https://example.org"}]}}}"#,
            "",
        ]
        .join("\n")
    }

    #[test]
    fn provider_requests_use_the_expected_endpoints_shapes_and_secret_names() {
        let (url, body, credential) = post(research("volcanoes"));
        assert_eq!(url, EXA_ENDPOINT);
        assert!(body.contains("\"effort\":\"high\""));
        assert!(body.contains("volcanoes"));
        assert_eq!(&*credential.secret, "exa");
        assert!(matches!(credential.header, SecretHeader::Named(ref name) if name == "x-api-key"));

        let exa = sse_transcript();
        let (url, body, credential) =
            post(write_book("volcanoes", Language::Hindi, exa.as_bytes()).expect("research"));
        assert_eq!(url, OPENAI_ENDPOINT);
        assert!(body.contains("\"model\":\"gpt-5.6-sol\""));
        assert!(body.contains("\"type\":\"json_schema\""));
        assert!(body.contains("in Hindi"));
        assert_eq!(&*credential.secret, "openai");
        assert_eq!(credential.header, SecretHeader::Bearer);

        let (url, body, credential) = post(speech("A spoken sentence.", Language::default()));
        assert_eq!(
            url,
            "https://api.elevenlabs.io/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb?output_format=mp3_44100_128"
        );
        assert!(body.contains("\"model_id\":\"eleven_multilingual_v2\""));
        assert_eq!(&*credential.secret, "elevenlabs");
        assert!(matches!(credential.header, SecretHeader::Named(ref name) if name == "xi-api-key"));
    }

    /// The agent endpoint without a stream answers instantly with an id to
    /// poll; the header is what asks it to stay on the line instead.
    #[test]
    fn research_asks_for_the_event_stream() {
        let Task::Post { headers, .. } = research("volcanoes") else {
            panic!("research must be a POST");
        };
        assert!(headers
            .iter()
            .any(|header| header.name == "accept" && header.value == "text/event-stream"));
    }

    #[test]
    fn the_completed_run_in_a_transcript_becomes_the_writing_context() {
        let task = write_book("volcanoes", Language::English, sse_transcript().as_bytes())
            .expect("a completed run carries research");
        let (_, body, _) = post(task);
        assert!(body.contains("brief"));
        assert!(
            !body.contains("keep-alive"),
            "progress frames are not research"
        );
    }

    #[test]
    fn a_failed_run_is_reported_not_written_from() {
        let transcript = "event: agent_run.failed\ndata: {\"id\":\"agent_run_1\",\"status\":\"failed\",\"error\":{\"code\":\"x\"}}\n\n";
        assert_eq!(
            write_book("volcanoes", Language::English, transcript.as_bytes()).unwrap_err(),
            "The research service gave up on that topic."
        );
    }

    #[test]
    fn a_stream_that_ends_early_is_not_mistaken_for_research() {
        let transcript =
            "event: agent_run.started\ndata: {\"id\":\"agent_run_1\",\"status\":\"running\"}\n\n";
        assert_eq!(
            write_book("volcanoes", Language::English, transcript.as_bytes()).unwrap_err(),
            "The research never finished."
        );
    }

    /// Every offered language narrates with its own native voice, so two
    /// languages sharing a voice id means one of them got the other's
    /// accent by a copy-paste.
    #[test]
    fn every_language_has_its_own_voice() {
        for language in LANGUAGES {
            let (url, _, _) = post(speech("hello", language));
            assert!(url.contains(language.voice_id()), "{language:?}");
            assert!(
                url.ends_with("?output_format=mp3_44100_128"),
                "{language:?}"
            );
            for other in LANGUAGES {
                assert!(
                    language == other || language.voice_id() != other.voice_id(),
                    "{language:?} and {other:?} share a voice"
                );
            }
        }
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

    /// The shape a reasoning model actually returns. The first item is its
    /// reasoning and carries no `content` key, so anything that treats a
    /// missing `content` as the end of the search never reaches the script in
    /// the second item and reports a perfectly good answer as no answer.
    #[test]
    fn a_reasoning_item_before_the_script_does_not_hide_the_script() {
        let script = r#"{"title":"Deep Time","summary":"A tour","chapters":[{"title":"One","narration":"First."},{"title":"Two","narration":"Second."},{"title":"Three","narration":"Third."}]}"#;
        let reasoning = kobo_json::ObjectBuilder::new()
            .set("type", "reasoning")
            .set("id", "rs_1")
            .set(
                "summary",
                vec![kobo_json::ObjectBuilder::new()
                    .set("type", "summary_text")
                    .set("text", "Planning the chapters.")],
            );
        let message = kobo_json::ObjectBuilder::new()
            .set("type", "message")
            .set("role", "assistant")
            .set(
                "content",
                vec![kobo_json::ObjectBuilder::new()
                    .set("type", "output_text")
                    .set("text", script)],
            );
        let envelope = kobo_json::ObjectBuilder::new()
            .set("output", vec![reasoning, message])
            .build()
            .to_json();
        let book = parse_book(envelope.as_bytes()).expect("a book behind the reasoning");
        assert_eq!(book.title, "Deep Time");
        assert_eq!(book.chapters.len(), 3);
    }

    /// A response that genuinely says nothing must still say nothing, rather
    /// than the fix above turning every empty answer into a parse error
    /// further down.
    #[test]
    fn reasoning_alone_is_still_reported_as_no_script() {
        let envelope = kobo_json::ObjectBuilder::new()
            .set(
                "output",
                vec![kobo_json::ObjectBuilder::new()
                    .set("type", "reasoning")
                    .set("id", "rs_1")],
            )
            .build()
            .to_json();
        assert_eq!(
            parse_book(envelope.as_bytes()).unwrap_err(),
            "No script came back for that topic."
        );
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
    fn the_estimate_follows_the_length_the_writer_is_asked_for() {
        let estimate = super::Estimate::for_a_book();
        let (short, long) = estimate.narration_characters;
        assert!(short < long);
        // The writer is told the same running time the estimate is built
        // from, so changing one changes the other.
        let (_, body, _) = post(
            write_book("volcanoes", Language::English, sse_transcript().as_bytes())
                .expect("research"),
        );
        let asked = format!(
            "{} to {} minute audiobook",
            super::BOOK_MINUTES.0,
            super::BOOK_MINUTES.1
        );
        assert!(body.contains(&asked), "{body}");
        assert!(
            estimate.dollars.0 >= 1 && estimate.dollars.1 <= 6,
            "{estimate:?}"
        );
        assert!(estimate.dollars.0 < estimate.dollars.1);
        assert!(estimate.minutes.0 < estimate.minutes.1);
    }

    #[test]
    fn the_narration_range_is_said_in_whole_thousands() {
        assert_eq!(super::Estimate::for_a_book().narration_thousands(), (7, 11));
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
