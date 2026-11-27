// SPDX-License-Identifier: CC0-1.0

//! What an AWS SDK CLIENT does to a TIMESTAMP a caller handed it before it sends the call: it
//! puts the value in the form the called protocol carries one in.
//!
//! An SDK takes for an input member the model types as a `timestamp` whatever its caller has:
//! a native instant, the number of seconds since the epoch, or the TEXT of an instant — the
//! `configurationItemCaptureTime` an AWS Config rule's own evaluator reads off the
//! configuration item it was handed and delivers its verdict with is exactly that text — and
//! the client converts each of them to the form the service's protocol states, which for the
//! AWS JSON protocols is the seconds since the epoch. So a service never sees the text a
//! caller wrote, and a handler that passes one is served exactly as it is on a managed
//! runtime, where the SDK on its import path converts it.
//!
//! The conversion is the CLIENT's half of a call, which is why it lives on the host side of
//! the one call seam ([`super::aws_api`]) rather than inside any one of the SDKs this runtime
//! supplies — every language's client does it, so it is done once for all of them, before the
//! request is built and signed — and beside the other thing a client fills in there
//! ([`super::idempotency_token`]).
//!
//! WHICH values those are is read from the called service's own Smithy model, through the one
//! reading of it this runtime holds (`super::sdk_model`), which answers where a
//! request's instants sit for one input document ([`super::blob_members::AnswerPlan`]): no
//! member name is stated here and none can drift from the surface the service serves. A member
//! bound outside the JSON body — a URI label, a query parameter, a header — is no part of that
//! plan, because such a binding carries its own form.
//!
//! A value this cannot read as an instant is left EXACTLY as the caller wrote it: the called
//! service is what judges a value it cannot place, and answers its own refusal for it, rather
//! than this seam guessing one.

use ::serde_json::Value;

/// The forms an instant's TEXT is written in, each read with `chrono`'s own reader.
///
/// Source: the ISO 8601 forms AWS's protocols carry a timestamp in — the `date-time` of
/// RFC 3339 with an offset or `Z`, and the same instant with no zone at all, which a client
/// reads in UTC — with and without a fraction of a second, and the space-separated spelling a
/// caller's own tooling writes.
const INSTANT_FORMS: [&str; 6] = [
    "%Y-%m-%dT%H:%M:%S%.f%#z",
    "%Y-%m-%dT%H:%M:%S%#z",
    "%Y-%m-%d %H:%M:%S%.f%#z",
    "%Y-%m-%d %H:%M:%S%#z",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S%.f",
];

/// `body` — one call's input document — with every timestamp member of `operation` in the form
/// the protocol carries one in, or `None` when there is nothing to convert: this emulator
/// holds no model for the service, the operation has no such member, every one of them is
/// already a number, or the body is not an input document at all.
///
/// `None` means the document travels exactly as the guest wrote it.
#[must_use]
pub fn in_protocol_form(service: &str, operation: &str, body: &str) -> Option<String> {
    let held = super::sdk_model::client_exports(service);
    let Ok(mut document) = ::serde_json::from_str::<Value>(body) else {
        return None;
    };
    if !document.is_object() {
        return None;
    }
    let paths = held.answer_instants.input_paths(operation, &document);
    if paths.is_empty() {
        return None;
    }
    let mut converted = false;
    for path in &paths {
        let Some(steps) = path.as_array() else {
            continue;
        };
        let Some(carried) = reached(&mut document, steps) else {
            continue;
        };
        let Some(seconds) = epoch_seconds(carried) else {
            continue;
        };
        *carried = Value::from(seconds);
        converted = true;
    }
    converted.then(|| document.to_string())
}

/// The value the path `steps` reaches inside `document`, or `None` where it reaches none.
fn reached<'a>(document: &'a mut Value, steps: &[Value]) -> Option<&'a mut Value> {
    let mut held = document;
    for step in steps {
        held = match step {
            Value::String(key) => held.as_object_mut()?.get_mut(key.as_str())?,
            Value::Number(index) => held.as_array_mut()?.get_mut(index.as_u64()? as usize)?,
            _ => return None,
        };
    }
    Some(held)
}

/// The seconds since the epoch `value` states, for a value that is not already the number a
/// JSON protocol carries a timestamp as; `None` for one that is, and for text that states no
/// instant this can read.
///
/// The text forms are [`INSTANT_FORMS`], plus the digits of the seconds themselves, which is
/// what a caller that already has them as text writes. A value carrying no zone is read in
/// UTC, the zone a function runs in, so the two spellings of one instant travel as the same
/// number.
fn epoch_seconds(value: &Value) -> Option<i64> {
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(seconds) = text.parse::<i64>() {
        return Some(seconds);
    }
    if let Ok(seconds) = text.parse::<f64>() {
        return Some(seconds.trunc() as i64);
    }
    for form in INSTANT_FORMS {
        if let Ok(zoned) = ::chrono::DateTime::parse_from_str(text, form) {
            return Some(zoned.timestamp());
        }
        if let Ok(naive) = ::chrono::NaiveDateTime::parse_from_str(text, form) {
            return Some(naive.and_utc().timestamp());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a timestamp member of a call's input travels as the seconds since the epoch,
    /// whatever form the caller wrote it in, and nothing else of the document changes.
    ///
    /// The rows span the forms a caller's value comes in: the text of an instant with a zone,
    /// with `Z`, with a fraction of a second, with no zone at all, the digits of the seconds
    /// themselves, and the number a client already converted — against the instant each one
    /// states.
    #[test]
    fn a_timestamp_the_caller_wrote_as_text_travels_as_the_seconds() {
        // (case, the value as the caller wrote it, the seconds it states)
        let rows: &[(&str, Value, i64)] = &[
            (
                "the text of an instant with a zone",
                Value::from("2026-01-02T03:04:05+00:00"),
                1_767_323_045,
            ),
            (
                "the same instant written with Z",
                Value::from("2026-01-02T03:04:05Z"),
                1_767_323_045,
            ),
            (
                "an instant carrying a fraction of a second",
                Value::from("2026-01-02T03:04:05.123Z"),
                1_767_323_045,
            ),
            (
                "an instant stated in another zone",
                Value::from("2026-01-02T04:04:05+01:00"),
                1_767_323_045,
            ),
            (
                "an instant carrying no zone at all",
                Value::from("2026-01-02T03:04:05"),
                1_767_323_045,
            ),
            (
                "the digits of the seconds themselves",
                Value::from("1767323045"),
                1_767_323_045,
            ),
            (
                "the number a client already converted",
                Value::from(1_767_323_045_i64),
                1_767_323_045,
            ),
        ];

        for (case, written, seconds) in rows {
            let body = ::serde_json::json!({
                "Evaluations": [{
                    "ComplianceResourceType": "AWS::SQS::Queue",
                    "ComplianceResourceId": "the-queue",
                    "ComplianceType": "COMPLIANT",
                    "OrderingTimestamp": written,
                }],
                "ResultToken": "the-token",
            })
            .to_string();
            let sent = in_protocol_form("config", "PutEvaluations", &body).unwrap_or(body);
            let document: Value = ::serde_json::from_str(&sent)
                .unwrap_or_else(|_| panic!("case {case:?}: expected a document, got {sent}"));
            assert_eq!(
                document["Evaluations"][0]["OrderingTimestamp"],
                Value::from(*seconds),
                "case {case:?}: expected the seconds since the epoch, got {sent}"
            );
            assert_eq!(
                document["ResultToken"],
                Value::from("the-token"),
                "case {case:?}: expected the caller's own members untouched, got {sent}"
            );
            assert_eq!(
                document["Evaluations"][0]["ComplianceType"],
                Value::from("COMPLIANT"),
                "case {case:?}: expected a text member to stay as written, got {sent}"
            );
        }
    }

    /// A document with nothing to convert travels exactly as the guest wrote it: a value no
    /// instant can be read from, a service this emulator holds no model for, a name that is no
    /// operation of it, an operation whose input types no member as a timestamp, and a body
    /// that is no input document at all.
    #[test]
    fn a_document_with_nothing_to_convert_travels_as_written() {
        // (case, service token, operation, the body)
        let rows: &[(&str, &str, &str, &str)] = &[
            (
                "a value no instant can be read from",
                "config",
                "PutEvaluations",
                r#"{"Evaluations":[{"OrderingTimestamp":"not an instant"}]}"#,
            ),
            (
                "a service this emulator holds no model for",
                "opensearch",
                "CreateDomain",
                r#"{"StartTime":"2026-01-02T03:04:05Z"}"#,
            ),
            (
                "no operation of the service",
                "config",
                "Nonesuch",
                r#"{"OrderingTimestamp":"2026-01-02T03:04:05Z"}"#,
            ),
            (
                "an operation whose input types no member as a timestamp",
                "config",
                "DescribeConfigRules",
                r#"{"ConfigRuleNames":["the-rule"]}"#,
            ),
            ("a body that is no document", "config", "PutEvaluations", "{"),
            (
                "a body that is no input document",
                "config",
                "PutEvaluations",
                "[]",
            ),
        ];

        for (case, service, operation, body) in rows {
            let answered = in_protocol_form(service, operation, body);
            assert!(
                answered.is_none(),
                "case {case:?}: expected the document to travel as written, got {answered:?}"
            );
        }
    }
}
