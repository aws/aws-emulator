// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the SDK's percent-encoding for a
//! `@httpLabel` path segment (every operation of this service but the two creates binds
//! an ARN there), and the three accessors over an answered body — a string member, a
//! modelled `date-time` member, and the error code of a refusal.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;

/// Percent-encode a `@httpLabel` value the way an AWS SDK does: everything outside
/// RFC 3986's unreserved set, so an ARN's `:` and `/` stay inside one path segment.
pub fn encode_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One string member of an answered body, or a panic naming the case and the member.
pub fn string_at(value: &Value, member: &str, case: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: member {member:?} missing from {value:?}"))
        .to_string()
}

/// Parse a modelled `date-time` member into comparable (seconds, nanos).
///
/// Every timestamp member of this service's responses targets `DateTimestamp`, which carries
/// `@timestampFormat("date-time")`, so the only form accepted here is the RFC 3339 STRING
/// that trait asks for: a JSON number would be the protocol default for a member schema
/// reporting no format, which no generated SDK client can read back into the modelled
/// `date-time`.
pub fn timestamp_at(value: &Value, member: &str, case: &str) -> (i64, u32) {
    let raw = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: timestamp {member:?} missing from {value:?}"));
    let text = raw.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member:?} must be an RFC 3339 date-time string, got {raw} in \
             {value:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: {member:?} = {text:?} is not a date-time: {e}")
    });
    (parsed.secs(), parsed.subsec_nanos())
}

/// The error code carried in a restJson1 error body, with any namespace stripped
/// (`com.amazonaws.bedrockdataautomation#Foo` -> `Foo`).
pub fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert that every member and element of `expected` appears in `actual` with an
/// equal value, naming the case and the JSON path of the first difference.
///
/// This is the relation a project read has to the configuration its create or update
/// carried: the read reports what the request specified AND the service's defaults
/// wherever the request was silent, so it is a SUPERSET of the request's own
/// configuration rather than equal to it. A list is compared element for element,
/// including its length — a read neither drops nor invents an element.
pub fn assert_json_contains(actual: &Value, expected: &Value, case: &str, path: &str) {
    match expected {
        Value::Object(members) => {
            let found = actual.as_object().unwrap_or_else(|| {
                panic!("case {case:?}: {path} must be a JSON object, got {actual:?}")
            });
            for (name, value) in members {
                let child = found.get(name).unwrap_or_else(|| {
                    panic!("case {case:?}: {path}.{name} is absent from {actual:?}")
                });
                assert_json_contains(child, value, case, &format!("{path}.{name}"));
            }
        }
        Value::Array(elements) => {
            let found = actual.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: {path} must be a JSON array, got {actual:?}")
            });
            assert_eq!(
                found.len(),
                elements.len(),
                "case {case:?}: {path} must carry {} element(s), got {found:?}",
                elements.len()
            );
            for (index, value) in elements.iter().enumerate() {
                assert_json_contains(&found[index], value, case, &format!("{path}[{index}]"));
            }
        }
        leaf => assert_eq!(
            actual, leaf,
            "case {case:?}: {path}: expected {leaf:?}, got {actual:?}"
        ),
    }
}
