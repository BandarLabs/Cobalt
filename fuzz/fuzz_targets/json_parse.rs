#![no_main]
//! Any string parses to a value or an error, never a panic, and a value
//! written back out reads back as the same value.
//!
//! Non-finite numbers are the one case the second half does not hold for:
//! JSON has no spelling for them, so a value holding one is skipped rather
//! than counted against the writer.

use kobo_json::Value;
use libfuzzer_sys::fuzz_target;

fn finite(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.is_finite(),
        Value::Array(items) => items.iter().all(finite),
        Value::Object(fields) => fields.iter().all(|(_, value)| finite(value)),
        Value::Null | Value::Bool(_) | Value::String(_) => true,
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(value) = kobo_json::parse(text) else {
        return;
    };
    if !finite(&value) {
        return;
    }
    let written = value.to_json();
    let again = kobo_json::parse(&written).expect("what the writer wrote must parse");
    assert_eq!(again, value, "a value must read back as itself");
    assert_eq!(again.to_json(), written, "writing is stable");
    let pretty = value.to_json_pretty();
    assert_eq!(kobo_json::parse(&pretty).ok(), Some(value), "the pretty form is the same value");
});
