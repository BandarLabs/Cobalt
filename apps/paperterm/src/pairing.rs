//! Parse user-entered pairing details before constructing any network URL.
use std::net::{Ipv4Addr, Ipv6Addr};

pub(super) fn address(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > 320 {
        return None;
    }
    let text = text
        .strip_prefix("https://")
        .unwrap_or(text)
        .trim_end_matches('/');
    if let Some(rest) = text.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']')?;
        let host = host.parse::<Ipv6Addr>().ok()?;
        let port = if suffix.is_empty() {
            9332
        } else {
            port(suffix.strip_prefix(':')?)?
        };
        return Some(format!("[{host}]:{port}"));
    }
    let (host, number) = match text.split_once(':') {
        Some((host, number)) => (host, port(number)?),
        None => (text, 9332),
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host.len() > 253
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') && host.parse::<Ipv4Addr>().is_err() {
        return None;
    }
    Some(format!("{}:{number}", host.to_ascii_lowercase()))
}
fn port(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port != 0)
}
pub(super) fn code(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 6 && text.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| text.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses_normalize_without_accepting_url_credentials_or_queries() {
        for (input, expected) in [
            ("laptop", "laptop:9332"),
            (" https://Laptop.local:9443/ ", "laptop.local:9443"),
            ("192.168.1.20:9332", "192.168.1.20:9332"),
            ("[::1]", "[::1]:9332"),
        ] {
            assert_eq!(address(input).as_deref(), Some(expected));
        }
        for input in [
            "",
            "http://laptop",
            "user@laptop",
            "laptop/path",
            "laptop?token=x",
            "laptop#x",
            "laptop:0",
            "laptop:65536",
            "a b",
            "999.1.1.1",
            "[::1]:1/path",
            "a..b",
            "-host",
        ] {
            assert_eq!(address(input), None, "{input}");
        }
    }
    #[test]
    fn codes_accept_the_printed_alphabet_and_reject_url_delimiters() {
        assert_eq!(code(" ABC234 ").as_deref(), Some("abc234"));
        for input in ["abc", "abcdefg", "ab&123", "ab\n123", "éabc1"] {
            assert_eq!(code(input), None);
        }
    }
}
