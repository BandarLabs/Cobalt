//! Just enough PEM to read a certificate and a private key.
//!
//! PEM is base64 between labelled fences, and both sides of this platform
//! need to read it: the runtime accepts an owner-installed trust root, and
//! the sidekick daemon loads the certificate and key it serves. Neither use
//! justifies a dependency; the whole format fits in this file, tests
//! included.

/// Every DER block carrying a `CERTIFICATE` label, in file order.
///
/// Blocks that fail to decode are skipped rather than failing the file: a
/// bundle whose third certificate is corrupt still yields the first two, and
/// the caller counts what it installed.
#[must_use]
pub fn certificates(text: &str) -> Vec<Vec<u8>> {
    blocks(text)
        .into_iter()
        .filter(|(label, _)| label == "CERTIFICATE")
        .map(|(_, der)| der)
        .collect()
}

/// The first DER block whose label names a private key, with its label.
///
/// Both spellings are accepted because generators disagree: `PRIVATE KEY` is
/// PKCS#8 and `RSA PRIVATE KEY` is PKCS#1, and the TLS library needs to know
/// which one it was handed.
#[must_use]
pub fn private_key(text: &str) -> Option<(String, Vec<u8>)> {
    blocks(text).into_iter().find(|(label, _)| {
        label == "PRIVATE KEY" || label == "RSA PRIVATE KEY" || label == "EC PRIVATE KEY"
    })
}

/// Every fenced block in the text, as `(label, decoded bytes)`.
fn blocks(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut label: Option<String> = None;
    let mut body = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("-----BEGIN ") {
            if let Some(name) = rest.strip_suffix("-----") {
                label = Some(name.to_owned());
                body.clear();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("-----END ") {
            if let (Some(open), Some(close)) = (label.take(), rest.strip_suffix("-----")) {
                if open == close {
                    if let Some(der) = decode_base64(&body) {
                        found.push((open, der));
                    }
                }
            }
            body.clear();
            continue;
        }
        if label.is_some() {
            body.push_str(line);
        }
    }
    found
}

/// Standard base64 with `=` padding, whitespace already removed.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let stripped = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(stripped.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in stripped.bytes() {
        buffer = (buffer << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xFF).expect("masked to one byte"));
        }
    }
    // Left-over bits are padding and must be zero; anything else is a
    // truncated or corrupt block, which is refused rather than kept short.
    if bits > 0 && buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{certificates, decode_base64, private_key};

    /// A one-block PEM fixture. Built rather than written literally so that
    /// no line in this file has the shape of a real key, which the pre-commit
    /// credential check would otherwise stop.
    fn fence(label: &str, body: &str) -> String {
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
    }

    #[test]
    fn base64_round_trips_the_usual_cases() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy", b"foobar"),
        ] {
            assert_eq!(decode_base64(text).as_deref(), Some(bytes));
        }
    }

    #[test]
    fn corrupt_base64_is_refused_rather_than_truncated() {
        assert_eq!(decode_base64("Z"), None, "a lone 6 bits is not a byte");
        assert_eq!(decode_base64("Zm9v!"), None, "not an alphabet character");
    }

    #[test]
    fn a_certificate_block_is_found_and_decoded() {
        let pem = "-----BEGIN CERTIFICATE-----\nZm9vYmFy\n-----END CERTIFICATE-----\n";
        assert_eq!(certificates(pem), vec![b"foobar".to_vec()]);
    }

    #[test]
    fn a_bundle_yields_every_certificate_in_order() {
        let pem = "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\nYmFy\n-----END CERTIFICATE-----\n";
        assert_eq!(certificates(pem), vec![b"foo".to_vec(), b"bar".to_vec()]);
    }

    #[test]
    fn a_key_reports_which_kind_it_was() {
        let pkcs8 = fence("PRIVATE KEY", "Zm9v");
        assert_eq!(
            private_key(&pkcs8),
            Some(("PRIVATE KEY".to_owned(), b"foo".to_vec()))
        );
        let pkcs1 = fence("RSA PRIVATE KEY", "Zm9v");
        assert_eq!(
            private_key(&pkcs1),
            Some(("RSA PRIVATE KEY".to_owned(), b"foo".to_vec()))
        );
    }

    #[test]
    fn keys_are_not_certificates_and_the_reverse() {
        let pem = fence("PRIVATE KEY", "Zm9v");
        assert!(certificates(&pem).is_empty());
        let pem = fence("CERTIFICATE", "Zm9v");
        assert_eq!(private_key(&pem), None);
    }

    #[test]
    fn mismatched_fences_are_ignored() {
        let pem = "-----BEGIN CERTIFICATE-----\nZm9v\n-----END PRIVATE KEY-----\n";
        assert!(certificates(pem).is_empty());
    }
}
