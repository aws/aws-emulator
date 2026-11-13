// SPDX-License-Identifier: CC0-1.0

//! How `GetRevocationStatus` reads the `signatureTimestamp` it takes on the query string.
//!
//! The rule under test: a timestamp bound to the query string carries an INSTANT, and the
//! renderings a client puts that instant on the wire in belong to the protocol rather than
//! to one member — the RFC 3339 `date-time`, with the `Z` zone or with the numeric UTC
//! offset RFC 3339 equally admits, and the number of seconds since the epoch, whole or
//! fractional. Every one of them names the same instant, so every one of them binds it and
//! the request is then answered by this operation's own rules and by the modelled
//! constraints of its other members. A value that names NO instant never becomes a
//! modelled input at all, so it is refused by the protocol in the framework's own error
//! envelope rather than as a violation of a constraint the model declares.
//!
//! Every assertion is driven over a TABLE whose rows differ along what the rule
//! generalises over: the FORM of the value (the `date-time` form the framework's own
//! reader takes, the epoch-seconds number, the four zone suffixes RFC 3339 admits, with
//! both signs), the percent-escaping the value reaches the server under, and — for each
//! form — whether the instant it names falls on the revocation's effective time or one
//! second before it, which is what proves the instant was READ rather than merely
//! accepted.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;

use service_com_amazonaws_signerdata_signerdataplane::state::{
    RevocationRecord, RevokedEntityKind,
};

use crate::revocation_status::{
    base_instant, certificate_hash, job_arn, params, profile_version_arn, revoked_entities,
    timestamp_param, Emulator, PLATFORM,
};

/// The epoch seconds of `at`.
fn epoch_secs(at: SystemTime) -> i64 {
    let since = at
        .duration_since(UNIX_EPOCH)
        .expect("the test's instants are after the epoch");
    i64::try_from(since.as_secs()).expect("seconds fit in i64")
}

/// `instant` rendered as RFC 3339 in the zone `offset_seconds` names, with `suffix` as its
/// zone: the SAME instant, spelled the way a client in that zone spells it.
///
/// The civil fields are rendered by the maintained reader this crate reads timestamps
/// with; only the constant shift into the zone is applied here.
fn zoned(instant: SystemTime, offset_seconds: i64, suffix: &str) -> String {
    let civil = DateTime::from_secs(epoch_secs(instant) + offset_seconds)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time");
    let civil = civil
        .strip_suffix('Z')
        .expect("the date-time rendering ends in Z");
    format!("{civil}{suffix}")
}

/// One wire rendering of a `signatureTimestamp`, and the instant it names.
struct Form {
    case: &'static str,
    /// The query value for the instant the argument names.
    render: fn(SystemTime) -> String,
}

/// Every form the wire names an instant in. The `Z`-zoned `date-time` is the one the
/// framework's own query-value reader takes; every other row is a form it does not, and
/// the rule is that all of them name the same instant.
fn forms() -> Vec<Form> {
    vec![
        Form {
            case: "date-time with the Z zone",
            render: |at| timestamp_param(at),
        },
        Form {
            case: "date-time with the Z zone, colons percent-encoded",
            render: |at| timestamp_param(at).replace(':', "%3A"),
        },
        Form {
            case: "date-time with a lowercase z zone",
            render: |at| zoned(at, 0, "z"),
        },
        Form {
            case: "date-time with a zero numeric offset",
            // The `+` is percent-escaped, as a query component must escape it: a raw `+`
            // is a SPACE in a query string.
            render: |at| zoned(at, 0, "%2B00:00"),
        },
        Form {
            case: "date-time with a positive offset in the hh:mm form",
            render: |at| zoned(at, 5 * 3600 + 1800, "%2B05:30"),
        },
        Form {
            case: "date-time with a negative offset in the hh:mm form",
            render: |at| zoned(at, -8 * 3600, "-08:00"),
        },
        Form {
            case: "date-time with a positive offset in the hhmm form",
            render: |at| zoned(at, 5 * 3600 + 1800, "%2B0530"),
        },
        Form {
            case: "date-time with a negative offset in the hh form",
            render: |at| zoned(at, -8 * 3600, "-08"),
        },
        Form {
            case: "epoch seconds",
            render: |at| epoch_secs(at).to_string(),
        },
        Form {
            case: "epoch seconds with a fractional part",
            render: |at| format!("{}.000", epoch_secs(at)),
        },
    ]
}

/// Every form binds the instant it names: a revocation effective at an instant applies to
/// a signature made AT it and not to one made a second earlier, whichever form the
/// timestamp was sent in.
///
/// The recorded identifier is deliberately not the registry key, so an answer echoing the
/// request's own values cannot pass; and the request is otherwise valid, so a form that
/// did not bind would be visible as a refusal rather than as the wrong list.
#[tokio::test]
async fn the_signature_timestamp_binds_in_every_wire_form_of_an_instant() {
    let emulator = Emulator::new();
    let effective = base_instant();
    let one_second_earlier = effective
        .checked_sub(Duration::from_secs(1))
        .expect("an instant one second before the base is representable");

    let profile = profile_version_arn("timestamp-forms");
    let job = job_arn("timestamp-forms");
    let hash = certificate_hash("timestamp-forms");
    let entity = "revoked-profile-version-timestamp-forms";
    emulator.revoke(
        RevokedEntityKind::ProfileVersion,
        &profile,
        RevocationRecord::new(entity, effective),
    );

    for form in forms() {
        for (at, when, expected) in [
            (effective, "on the effective time", vec![entity.to_string()]),
            (
                one_second_earlier,
                "one second before the effective time",
                Vec::new(),
            ),
        ] {
            let case = format!("{} {when}", form.case);
            let request: Vec<(&str, String)> = params(
                at,
                PLATFORM,
                &profile,
                &job,
                std::slice::from_ref(&hash),
            )
            .into_iter()
            .map(|(name, value)| {
                if name == "signatureTimestamp" {
                    (name, (form.render)(at))
                } else {
                    (name, value)
                }
            })
            .collect();

            let (status, body) = emulator.get(&request).await;
            assert_eq!(
                revoked_entities(status, &body, &case),
                expected,
                "case {case}: expected {expected:?} for the instant this form names, got \
                 {body}"
            );
        }
    }
}

/// A `signatureTimestamp` that names no instant is refused by the PROTOCOL, not as a
/// modelled constraint violation: HTTP 400, the framework's `x-amzn-errortype`, and the
/// sentence under the capitalized `Message` member of the framework envelope, naming the
/// value it could not convert — with no modelled `message`/`code` pair, since the value
/// never became a modelled input of this service.
///
/// The rows differ along the kind of unreadable value (a word, nothing at all, a number in
/// an encoding no timestamp format describes, a `date-time` with no zone, one past the
/// calendar, one whose zone is out of range) and along the percent-escaping it arrives
/// under, so the echoed value is the DECODED one the reader was handed.
#[tokio::test]
async fn a_signature_timestamp_that_names_no_instant_is_refused_by_the_protocol() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("no-instant");
    let job = job_arn("no-instant");
    let hash = certificate_hash("no-instant");

    // (case, the value as the wire carries it, the value the refusal echoes)
    let rows: [(&str, &str, &str); 9] = [
        ("a word", "yesterday", "yesterday"),
        (
            "a phrase whose spaces are escaped",
            "not%20a%20timestamp",
            "not a timestamp",
        ),
        ("nothing at all", "", ""),
        ("scientific notation", "1.7e9", "1.7e9"),
        (
            "an explicit plus sign on a number",
            "%2B1700000000",
            "+1700000000",
        ),
        ("a trailing decimal point", "1700000000.", "1700000000."),
        (
            "a date-time with no zone at all",
            "2023-11-14T22%3A13%3A20",
            "2023-11-14T22:13:20",
        ),
        (
            "a date-time past the calendar",
            "2023-13-14T22%3A13%3A20Z",
            "2023-13-14T22:13:20Z",
        ),
        (
            "a date-time whose zone offset is out of range",
            "2023-11-14T22%3A13%3A20%2B24%3A00",
            "2023-11-14T22:13:20+24:00",
        ),
    ];

    for (case, sent, echoed) in rows {
        let request: Vec<(&str, String)> = params(
            base_instant(),
            PLATFORM,
            &profile,
            &job,
            std::slice::from_ref(&hash),
        )
        .into_iter()
        .map(|(name, value)| {
            if name == "signatureTimestamp" {
                (name, sent.to_string())
            } else {
                (name, value)
            }
        })
        .collect();

        let (status, error_type, body) = emulator.get_with_error_type(&request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("SerializationException"),
            "case {case}: expected the protocol's own errortype, got {error_type:?}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(format!("'{echoed}' can not be converted to Date").as_str()),
            "case {case}: expected the framework envelope's Message naming the value it \
             could not convert, got {body}"
        );
        for modelled in ["message", "code", "__type"] {
            assert!(
                body.get(modelled).is_none(),
                "case {case}: a protocol-level refusal carries none of the modelled error's \
                 members, got {modelled} in {body}"
            );
        }
    }
}

/// The refusal of a value that names no instant is not sized by the request: past the one
/// ceiling this endpoint quotes a value to, the sentence states a prefix and how many bytes
/// were sent.
///
/// The rows walk the length of the refused value across that ceiling — under it, on it, and
/// past it — so what is pinned is the bound rather than one value.
#[tokio::test]
async fn a_refused_timestamp_is_not_echoed_without_bound() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("echo-bound");
    let job = job_arn("echo-bound");
    let hash = certificate_hash("echo-bound");

    // The ceiling `service_common::validation`'s `quoted_echo` holds ONE quoted value to,
    // written out here so the bytes a client reads are pinned independently of that
    // module's own constant.
    const ECHO_CEILING: usize = 16 * 1024;

    for length in [ECHO_CEILING - 1, ECHO_CEILING, ECHO_CEILING + 1, ECHO_CEILING * 2] {
        // Letters, so the value names no instant in any form and carries nothing a query
        // string reserves.
        let value = "n".repeat(length);
        let request: Vec<(&str, String)> = params(
            base_instant(),
            PLATFORM,
            &profile,
            &job,
            std::slice::from_ref(&hash),
        )
        .into_iter()
        .map(|(name, carried)| {
            if name == "signatureTimestamp" {
                (name, value.clone())
            } else {
                (name, carried)
            }
        })
        .collect();

        let (status, error_type, body) = emulator.get_with_error_type(&request).await;
        let case = format!("a value of {length} characters");
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with a body of {} bytes",
            body.to_string().len()
        );
        assert_eq!(
            error_type.as_deref(),
            Some("SerializationException"),
            "case {case}: expected the protocol's own errortype, got {error_type:?}"
        );
        let stated = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: the refusal must carry a Message, got {body}"));
        if length <= ECHO_CEILING {
            assert_eq!(
                stated,
                format!("'{value}' can not be converted to Date"),
                "case {case}: expected the value echoed whole, got a message of {} characters",
                stated.len()
            );
        } else {
            assert!(
                !stated.contains(&value),
                "case {case}: expected the echo to be bounded, got a message of {} characters",
                stated.len()
            );
            // Bounded by the CEILING rather than by the value: the sentence's fixed text
            // and the marker naming the byte count are all that sit beside the prefix, so
            // the message does not grow with the request.
            assert!(
                stated.len() < ECHO_CEILING + 128,
                "case {case}: expected a message bounded by the ceiling, got {} characters",
                stated.len()
            );
            assert!(
                stated.contains(&format!("[{length} bytes]"))
                    && stated.ends_with("' can not be converted to Date"),
                "case {case}: expected the sentence to state how many bytes were sent, got a \
                 message of {} characters",
                stated.len()
            );
        }
    }
}

/// With the timestamp in a form the framework's own reader does not take, the request is
/// still refused for the member whose MODELLED constraint it actually violates — the
/// timestamp no longer preempts the boundary walk.
///
/// The rows differ along the member at fault and the constraint it violates (each `Arn`'s
/// `@length` minimum of 20, the `platformId` `@pattern`), and along the form the valid
/// timestamp is sent in.
#[tokio::test]
async fn a_bound_timestamp_lets_another_member_s_constraint_decide_the_refusal() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("preempt");
    let job = job_arn("preempt");
    let hash = certificate_hash("preempt");

    // The modelled bounds: `Arn` is `@length(20, 2048)` and `PlatformId` is
    // `@pattern("^[a-zA-Z0-9_-]+$")`
    // (`smithy_query(query="operation", operation="GetRevocationStatus")`).
    let rows: [(&str, &str, &str); 3] = [
        (
            "jobArn below the modelled minimum length",
            "jobArn",
            "1 validation error detected: Value 'short' at 'jobArn' failed to satisfy \
             constraint: Member must have length greater than or equal to 20",
        ),
        (
            "profileVersionArn below the modelled minimum length",
            "profileVersionArn",
            "1 validation error detected: Value 'short' at 'profileVersionArn' failed to \
             satisfy constraint: Member must have length greater than or equal to 20",
        ),
        (
            "platformId outside the modelled pattern",
            "platformId",
            "1 validation error detected: Value 'short' at 'platformId' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$",
        ),
    ];

    for form in forms() {
        for (what, member, expected) in rows {
            // `short` violates the length bound of either ARN, and — once a `.` is on it —
            // the platform pattern too, so one value drives every row.
            let bad = if member == "platformId" {
                "short."
            } else {
                "short"
            };
            let expected = expected.replace("Value 'short'", &format!("Value '{bad}'"));
            let case = format!("{what}, timestamp as {}", form.case);
            let request: Vec<(&str, String)> = params(
                base_instant(),
                PLATFORM,
                &profile,
                &job,
                std::slice::from_ref(&hash),
            )
            .into_iter()
            .map(|(name, value)| {
                if name == "signatureTimestamp" {
                    (name, (form.render)(base_instant()))
                } else if name == member {
                    (name, bad.to_string())
                } else {
                    (name, value)
                }
            })
            .collect();

            let (status, error_type, body) = emulator.get_with_error_type(&request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected the modelled ValidationException, got {error_type:?} \
                 with {body}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the refusal to name the member whose modelled bound \
                 the request violated, got {body}"
            );
        }
    }
}
