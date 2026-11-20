//! The page itself, carried inside the program.
//!
//! Four files, compiled in, so the window works on a computer with no
//! network and cannot be changed by anything on disk. None of them names
//! another origin: no font, script, stylesheet or image is fetched from
//! anywhere but this server, and the response headers forbid the browser
//! from doing so even if one of them tried.

use std::sync::Arc;

use super::http::Response;
use super::server::{Request, Shared};

const INDEX: &str = include_str!("page/index.html");
const STYLE: &str = include_str!("page/app.css");
const SCRIPT: &str = include_str!("page/app.js");
const LOGIC: &str = include_str!("page/logic.mjs");

/// Where the page's markup carries the request token.
const TOKEN_PLACE: &str = "{{request-token}}";

/// `GET /`: the page, with this session's request token written into it.
///
/// The token is put in the markup rather than fetched by script because the
/// page is the one response only the cookie's holder is ever sent, and no
/// other origin can read a response from here. A script that had to ask for
/// it would need a route that hands it out.
pub fn index(shared: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    let page = INDEX.replace(TOKEN_PLACE, shared.session.request_token());
    Response::file("text/html; charset=utf-8", page.as_bytes())
}

/// `GET /app.css`.
pub fn style(_: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::file("text/css; charset=utf-8", STYLE.as_bytes())
}

/// `GET /app.js`.
pub fn script(_: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::file("text/javascript; charset=utf-8", SCRIPT.as_bytes())
}

/// `GET /logic.mjs`: the part of the page's script that decides things,
/// kept apart so the project's own tests can import it.
pub fn logic(_: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::file("text/javascript; charset=utf-8", LOGIC.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_has_exactly_one_place_for_the_request_token() {
        assert_eq!(INDEX.matches(TOKEN_PLACE).count(), 1);
    }

    #[test]
    fn nothing_the_page_loads_or_asks_for_is_on_another_origin() {
        // Links an owner follows by choice are allowed and open outside the
        // window. Anything the page would fetch for itself is not: a source,
        // a stylesheet, an import, a font, or a request made by script.
        for (name, text) in [
            ("index.html", INDEX),
            ("app.css", STYLE),
            ("app.js", SCRIPT),
            ("logic.mjs", LOGIC),
        ] {
            let lowered = text.to_ascii_lowercase();
            for loader in [
                "src=\"http",
                "src='http",
                "src=\"//",
                "url(http",
                "url(\"http",
                "url('http",
                "url(//",
                "@import",
                "fetch(\"http",
                "fetch('http",
                "fetch(`http",
                "from \"http",
                "from 'http",
                "import(\"http",
                "sendbeacon(",
                "websocket(",
                "eventsource(",
                "<link rel=\"stylesheet\" href=\"http",
                "<link rel=\"preconnect\"",
                "<link rel=\"dns-prefetch\"",
            ] {
                assert!(!lowered.contains(loader), "{name} contains {loader}");
            }
        }
    }

    #[test]
    fn the_markup_carries_no_inline_script_or_style_for_the_policy_to_refuse() {
        let lowered = INDEX.to_ascii_lowercase();
        assert!(!lowered.contains("<style"));
        assert!(!lowered.contains(" style=\""));
        assert!(!lowered.contains(" onclick="));
        assert!(!lowered.contains("javascript:"));
        for script in lowered.split("<script").skip(1) {
            assert!(
                script.trim_start().starts_with("type=\"module\" src=\"/"),
                "a script that is not one of the page's own files"
            );
        }
    }
}
