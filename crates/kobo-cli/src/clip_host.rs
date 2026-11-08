//! `kobo clip native-host`: the Send to Kobo extension's half on this
//! computer.
//!
//! A browser starts this when the extension asks, and talks to it over
//! stdin and stdout in the browser's native messaging framing: each message
//! is a four byte length in the machine's own byte order, then that many
//! bytes of JSON. Nothing else may reach stdout, or the browser drops the
//! connection, so everything here says what happened in a message.
//!
//! The extension sends the page as the owner sees it, rendered and with the
//! hidden parts gone, so a page behind a sign-in clips as it reads. This
//! makes the book, sends it or keeps it for later as `kobo clip` does, and
//! answers with progress and then the outcome.

use super::clip_delivery as delivery;
use std::io::{Read, Write};

/// The most a browser may send in one message. A rendered page is a few
/// hundred kilobytes and the article reader refuses pages over 8 MB, so this
/// leaves room for the JSON around one and refuses anything stranger.
pub const MOST_IN: usize = 16 * 1024 * 1024;

/// The most a host may send back, which Chrome refuses beyond.
pub const MOST_OUT: usize = 1024 * 1024;

/// Reads one message. `None` when the browser has closed the connection.
///
/// # Errors
///
/// A length over [`MOST_IN`], or a message cut short.
pub fn read_message(input: &mut impl Read) -> Result<Option<Vec<u8>>, String> {
    let mut length = [0_u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(format!("read from the browser: {error}")),
    }
    let length = usize::try_from(u32::from_ne_bytes(length)).unwrap_or(usize::MAX);
    if length > MOST_IN {
        return Err(format!(
            "the browser sent {length} bytes, more than a page can be"
        ));
    }
    let mut message = vec![0; length];
    input
        .read_exact(&mut message)
        .map_err(|error| format!("read from the browser: {error}"))?;
    Ok(Some(message))
}

/// Writes one message.
///
/// # Errors
///
/// A message over [`MOST_OUT`], or a browser that went away.
pub fn write_message(output: &mut impl Write, json: &str) -> Result<(), String> {
    if json.len() > MOST_OUT {
        return Err("a reply too large for the browser".to_owned());
    }
    let length = u32::try_from(json.len()).map_err(|_| "a reply too large".to_owned())?;
    output
        .write_all(&length.to_ne_bytes())
        .and_then(|()| output.write_all(json.as_bytes()))
        .and_then(|()| output.flush())
        .map_err(|error| format!("write to the browser: {error}"))
}

/// What the extension asks for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Asked {
    pub url: String,
    pub title: Option<String>,
    /// The page as rendered. Absent for a link, which is fetched here.
    pub html: Option<String>,
    /// The owner's selection, as HTML, for Send selection to Kobo.
    pub selection: Option<String>,
}

/// Reads an ask.
///
/// # Errors
///
/// Anything that is not one, in words the extension shows.
pub fn asked(bytes: &[u8]) -> Result<Asked, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "The page did not arrive as text.")?;
    let value = kobo_json::parse(text).map_err(|_| "The page did not arrive whole.")?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(kobo_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    let url = text("url").ok_or("The page arrived without its address.")?;
    if !kobo_article::address::is_web_address(&url) {
        return Err("Only web pages can be sent to your Kobo.".to_owned());
    }
    Ok(Asked {
        url,
        title: text("title"),
        html: text("html"),
        selection: text("selection"),
    })
}

/// The page to make the book from.
///
/// A selection becomes a page of its own under the page's title, marked as
/// the article, so the reader keeps exactly what was chosen rather than
/// looking for an article around it.
pub(crate) fn source_of(asked: &Asked) -> super::clip::Source {
    if let Some(selection) = &asked.selection {
        let title = escape(asked.title.as_deref().unwrap_or("Selection"));
        return super::clip::Source::Html {
            html: format!(
                "<!DOCTYPE html><html><head><title>{title}</title></head>\
                 <body><article><h1>{title}</h1>{selection}</article></body></html>"
            ),
            url: asked.url.clone(),
        };
    }
    match &asked.html {
        Some(html) => super::clip::Source::Html {
            html: html.clone(),
            url: asked.url.clone(),
        },
        None => super::clip::Source::Address(asked.url.clone()),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A step under way, for the extension's notification.
#[must_use]
pub fn progress(text: &str) -> String {
    kobo_json::ObjectBuilder::new()
        .set("type", "progress")
        .set("text", text)
        .build()
        .to_json()
}

/// How it ended: `landed`, `waiting` or `failed`, with the article's title
/// and a sentence for the owner.
#[must_use]
pub fn done(outcome: &str, title: &str, message: &str) -> String {
    kobo_json::ObjectBuilder::new()
        .set("type", "done")
        .set("outcome", outcome)
        .set("title", title)
        .set("message", message)
        .build()
        .to_json()
}

/// Serves the browser until it closes the connection.
///
/// # Errors
///
/// Only when the browser's side of the pipe fails.
pub fn run() -> Result<(), String> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    while let Some(bytes) = read_message(&mut input)? {
        let reply = handle(&bytes, &mut |text| {
            let _ = write_message(&mut output, &progress(text));
        });
        write_message(&mut output, &reply)?;
    }
    Ok(())
}

fn handle(bytes: &[u8], say: &mut dyn FnMut(&str)) -> String {
    handle_with(
        bytes,
        say,
        |asked| {
            let clipping = super::clip::make(&source_of(asked), &super::clip::reader_options())?;
            Ok(super::clip::article_of(&clipping))
        },
        |article| {
            delivery::send_now(
                &delivery::queue_folder(),
                article,
                super::clip::reader_for(&super::targets::TargetArgs::default()),
            )
        },
    )
}

pub(crate) fn handle_with(
    bytes: &[u8],
    say: &mut dyn FnMut(&str),
    make: impl FnOnce(&Asked) -> Result<delivery::Article, String>,
    send: impl FnOnce(&delivery::Article) -> Result<delivery::Landing, String>,
) -> String {
    let asked = match asked(bytes) {
        Ok(asked) => asked,
        Err(reason) => return done("failed", "", &reason),
    };
    say("Making the article");
    let article = match make(&asked) {
        Ok(article) => article,
        Err(reason) => {
            let title = asked.title.as_deref().unwrap_or_default();
            return done("failed", title, &reason);
        }
    };
    say("Sending to your Kobo");
    match send(&article) {
        Ok(delivery::Landing::Landed { .. }) => done(
            "landed",
            &article.title,
            &format!("Landed on your Kobo: {}", article.title),
        ),
        Ok(delivery::Landing::Waiting { why }) => done(
            "waiting",
            &article.title,
            &format!(
                "{why} {} waits on this computer and goes over the next time you sync.",
                article.title
            ),
        ),
        Err(reason) => done("failed", &article.title, &reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(json: &str) -> Vec<u8> {
        let mut bytes = u32::try_from(json.len())
            .expect("small")
            .to_ne_bytes()
            .to_vec();
        bytes.extend_from_slice(json.as_bytes());
        bytes
    }

    #[test]
    fn a_message_is_read_by_its_length_and_the_next_one_follows_it() {
        let mut stream = framed(r#"{"url":"https://a.example/"}"#);
        stream.extend(framed("{}"));
        let mut input = std::io::Cursor::new(stream);
        assert_eq!(
            read_message(&mut input).expect("read").as_deref(),
            Some(br#"{"url":"https://a.example/"}"#.as_slice())
        );
        assert_eq!(
            read_message(&mut input).expect("read").as_deref(),
            Some(b"{}".as_slice())
        );
        assert_eq!(read_message(&mut input).expect("closed"), None);
    }

    #[test]
    fn a_length_past_any_page_is_refused_before_anything_is_read() {
        let mut stream = u32::try_from(MOST_IN + 1)
            .expect("fits")
            .to_ne_bytes()
            .to_vec();
        stream.extend_from_slice(b"{}");
        assert!(read_message(&mut std::io::Cursor::new(stream)).is_err());
    }

    #[test]
    fn a_message_cut_short_is_an_error_not_a_page() {
        let mut stream = framed(r#"{"url":"https://a.example/"}"#);
        stream.truncate(10);
        assert!(read_message(&mut std::io::Cursor::new(stream)).is_err());
    }

    #[test]
    fn a_reply_is_written_with_its_length_in_front() {
        let mut out = Vec::new();
        write_message(&mut out, "{\"type\":\"done\"}").expect("written");
        assert_eq!(out, framed("{\"type\":\"done\"}"));
        assert!(write_message(&mut Vec::new(), &"x".repeat(MOST_OUT + 1)).is_err());
    }

    #[test]
    fn an_ask_carries_the_page_or_only_a_link_and_never_a_browser_page() {
        let page = asked(br#"{"url":"https://a.example/x","title":"X","html":"<p>hi</p>"}"#)
            .expect("a page");
        assert_eq!(page.html.as_deref(), Some("<p>hi</p>"));
        let link = asked(br#"{"url":"https://a.example/y"}"#).expect("a link");
        assert_eq!(link.html, None);
        assert!(matches!(
            source_of(&link),
            super::super::clip::Source::Address(_)
        ));
        assert!(asked(br#"{"url":"chrome://settings"}"#).is_err());
        assert!(asked(br#"{"title":"No address"}"#).is_err());
        assert!(asked(b"not json").is_err());
    }

    #[test]
    fn a_selection_becomes_the_article_under_the_pages_title() {
        let selection = asked(
            br#"{"url":"https://a.example/x","title":"Tides <and> moons","html":"<p>all</p>","selection":"<p>chosen</p>"}"#,
        )
        .expect("a selection");
        let super::super::clip::Source::Html { html, url } = source_of(&selection) else {
            panic!("a selection is a page of its own");
        };
        assert_eq!(url, "https://a.example/x");
        assert!(html.contains("<article><h1>Tides &lt;and&gt; moons</h1><p>chosen</p></article>"));
        assert!(!html.contains("<p>all</p>"));
    }

    fn article() -> delivery::Article {
        delivery::Article {
            title: "Tides".to_owned(),
            site: None,
            file_name: "Tides.epub".to_owned(),
            epub: Vec::new(),
        }
    }

    fn reply(json: &str) -> (String, String) {
        let value = kobo_json::parse(json).expect("json");
        let text = |key: &str| {
            value
                .get(key)
                .and_then(kobo_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        (text("outcome"), text("message"))
    }

    #[test]
    fn the_extension_hears_progress_then_that_the_article_landed() {
        let mut said = Vec::new();
        let answer = handle_with(
            br#"{"url":"https://a.example/x","html":"<p>hi</p>"}"#,
            &mut |text| said.push(text.to_owned()),
            |_| Ok(article()),
            |_| {
                Ok(delivery::Landing::Landed {
                    reader: "the reader".to_owned(),
                    also: 0,
                })
            },
        );
        assert_eq!(said, ["Making the article", "Sending to your Kobo"]);
        assert_eq!(
            reply(&answer),
            ("landed".to_owned(), "Landed on your Kobo: Tides".to_owned())
        );
    }

    #[test]
    fn an_article_for_a_sleeping_reader_is_waiting_not_failed() {
        let answer = handle_with(
            br#"{"url":"https://a.example/x","html":"<p>hi</p>"}"#,
            &mut |_| {},
            |_| Ok(article()),
            |_| {
                Ok(delivery::Landing::Waiting {
                    why: delivery::AWAY.to_owned(),
                })
            },
        );
        let (outcome, message) = reply(&answer);
        assert_eq!(outcome, "waiting");
        assert!(message.starts_with(delivery::AWAY), "{message}");
        assert!(
            message.contains("Tides waits on this computer"),
            "{message}"
        );
    }

    #[test]
    fn a_page_that_is_not_an_article_fails_with_the_reason() {
        let answer = handle_with(
            br#"{"url":"https://a.example/x","title":"Home","html":"<p>hi</p>"}"#,
            &mut |_| {},
            |_| Err("This page has no article on it.".to_owned()),
            |_| panic!("nothing is sent when nothing was made"),
        );
        assert_eq!(
            reply(&answer),
            (
                "failed".to_owned(),
                "This page has no article on it.".to_owned()
            )
        );
    }
}
