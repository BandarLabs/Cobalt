//! Fetching a page and its pictures over the runtime's own transport.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use kobo_protocol::TaskError;

use crate::{address, Answer, Fetch, Kind, Request};

/// Fetches over HTTPS with the transport the reader itself uses, so a clip
/// made on the owner's computer meets the same TLS roots and ceilings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Web;

fn said(error: TaskError) -> String {
    match error {
        TaskError::Offline => "this computer is offline".to_owned(),
        TaskError::Unreachable => "the site could not be reached".to_owned(),
        TaskError::TooLarge => "the answer was larger than allowed".to_owned(),
        TaskError::TimedOut => "the site took too long to answer".to_owned(),
        TaskError::NotFound => {
            "the site answered with an error, or the address is not one that can be fetched"
                .to_owned()
        }
        TaskError::Unauthorized => "the site asked for a login".to_owned(),
        other => format!("the fetch failed ({other:?})"),
    }
}

/// The address as the transport takes it: escaped as a browser would escape
/// it, since an owner pastes `https://example.org/café.html` and the
/// transport parses strictly, and over TLS, the only way it fetches.
fn request_target(requested: &str) -> String {
    let escaped = address::resolve(requested, requested).unwrap_or_else(|| requested.to_owned());
    address::fetchable(&escaped)
}

impl Fetch for Web {
    fn fetch(&self, request: &Request<'_>) -> Result<Answer, String> {
        let target = request_target(request.address);
        let accept = match request.kind {
            Kind::Page => "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5",
            // No AVIF: a CDN that negotiates on this header would otherwise
            // send a format nothing here decodes.
            Kind::Picture => "image/webp,image/jpeg,image/png,image/gif;q=0.8",
        };
        let mut headers = vec![("Accept", accept)];
        if let Some(referer) = request.referer {
            // Some image hosts refuse a picture asked for without the page
            // it appears on.
            headers.push(("Referer", referer));
        }
        let limit = u32::try_from(request.max_bytes).unwrap_or(u32::MAX);

        // The transport checks this flag while it waits, so the deadline ends
        // the request itself rather than leaving it running behind us.
        let cancel = Arc::new(AtomicBool::new(false));
        let (done, finished) = mpsc::channel::<()>();
        let timer = {
            let cancel = Arc::clone(&cancel);
            let timeout = request.timeout;
            thread::spawn(move || {
                if let Err(mpsc::RecvTimeoutError::Timeout) = finished.recv_timeout(timeout) {
                    cancel.store(true, Ordering::SeqCst);
                }
            })
        };
        let result = kobo_net::fetch_from_controlled_at(
            &target,
            0,
            limit,
            None,
            &headers,
            kobo_net::RequestOptions::default(),
            &cancel,
        );
        let timed_out = cancel.load(Ordering::SeqCst);
        drop(done);
        let _ = timer.join();
        match result {
            Ok(fetched) => Ok(Answer {
                body: fetched.body,
                address: fetched.address,
            }),
            Err(_) if timed_out => Err(said(TaskError::TimedOut)),
            Err(error) => Err(said(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::request_target;

    #[test]
    fn a_page_address_with_letters_outside_ascii_is_escaped_before_it_is_fetched() {
        assert_eq!(
            request_target("https://example.org/caf\u{e9}.html"),
            "https://example.org/caf%C3%A9.html"
        );
        assert_eq!(
            request_target("http://example.org/a b?q=x#part"),
            "https://example.org/a%20b?q=x"
        );
        assert_eq!(
            request_target("https://example.org/caf%C3%A9.html"),
            "https://example.org/caf%C3%A9.html"
        );
    }
}
