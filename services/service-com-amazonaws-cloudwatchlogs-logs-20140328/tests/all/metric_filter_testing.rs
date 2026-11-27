// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestMetricFilter`: running a `filterPattern` over a sample of log
//! event messages and reporting the events it matched and the values it extracted.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.TestMetricFilter` through the router that mounts it.
//!
//! The tables below span the classes the filter-pattern syntax generalises over, so any one
//! pattern an issue happens to mention is a single row:
//!
//! * the pattern FORM — unstructured terms, a space-delimited field list (`[ … ]`) and a
//!   JSON condition (`{ … }`), plus the empty pattern;
//! * within the term form — a bare term, a quoted phrase, a `%…%` regular expression, a
//!   conjunction, a `-` exclusion and `?` alternatives;
//! * within the space-delimited form — the field count, the `...` wildcard, a condition on a
//!   named field, an unnamed slot, and the grouping that makes a `"…"` or `[…]` run one
//!   field;
//! * within the JSON form — string, numeric, wildcard, boolean, array-index and
//!   presence conditions, and an event that is not JSON at all;
//! * the refusals — every defect the syntax has, each with the message that states it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::filter_pattern::{
    MAX_EXTRACTED_BYTES, MAX_EXTRACTED_VALUES,
};

use crate::harness::Logs;

/// The regular expression patterns one filter pattern may carry. Source: `PutMetricFilter`
/// — "there is a quota of two regular expression patterns within a single filter pattern".
const MAX_REGEX_PER_PATTERN: usize = 2;

/// A space-delimited log event of the form the filter-pattern syntax documents for the
/// form: seven fields, two of which are GROUPS — a `[…]` run and a `"…"` run — each of
/// which is one field whose value excludes the delimiters.
const APACHE_EVENT: &str = "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700] \
                            \"GET /apache_pb.gif HTTP/1.0\" 200 2326";

/// The same shape of event, with a different status code and byte count, so a condition on
/// one field is the only thing that tells the two apart.
const APACHE_EVENT_NOT_FOUND: &str = "10.0.0.7 - anna [10/Oct/2000:13:30:01 -0700] \
                                      \"GET /missing.gif HTTP/1.0\" 404 128";

/// Call `TestMetricFilter` and hand back the `matches` it reports.
async fn matches(logs: &Logs, pattern: &str, messages: &[&str]) -> Vec<Value> {
    let reported = logs
        .ok(
            "TestMetricFilter",
            &json!({"filterPattern": pattern, "logEventMessages": messages}),
        )
        .await;
    reported
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {pattern:?}: TestMetricFilter reports matches, got {reported}")
        })
}

/// The 1-based event numbers a set of matches names, in the order they were reported.
fn event_numbers(reported: &[Value]) -> Vec<i64> {
    reported
        .iter()
        .map(|entry| {
            entry
                .get("eventNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("a match carries eventNumber, got {entry}"))
        })
        .collect()
}

/// The values one match reports as extracted, empty when it reports none.
fn extracted(entry: &Value) -> BTreeMap<String, String> {
    entry
        .get("extractedValues")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("an extracted value is a string, got {value}")
                            })
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the matching table: a pattern, the sample it is run over, and the 1-based
/// positions in that sample it is expected to match.
struct MatchCase {
    /// The `filterPattern` under test.
    pattern: &'static str,
    /// The sample of log event messages.
    sample: &'static [&'static str],
    /// The event numbers the pattern matches.
    expected: &'static [i64],
}

/// Which events a pattern selects, across every form of the syntax.
///
/// The rule: an unstructured pattern matches a message that carries every unprefixed term,
/// none of its `-` terms and at least one of its `?` terms; a `[ … ]` pattern matches an
/// event whose fields its field list describes and whose fields satisfy its conditions; a
/// `{ … }` pattern matches an event that is a JSON document satisfying its conditions; the
/// empty pattern matches every event.
#[tokio::test]
async fn a_pattern_selects_the_events_its_form_describes() {
    let logs = Logs::new();

    let cases = [
        // Unstructured: one bare term, matched anywhere in the message, case-sensitively.
        MatchCase {
            pattern: "ERROR",
            sample: &["[ERROR] disk full", "[INFO] all good", "ERROR 500"],
            expected: &[1, 3],
        },
        MatchCase {
            pattern: "Error",
            sample: &["Error here", "error here"],
            expected: &[1],
        },
        // Unstructured: several terms are a conjunction.
        MatchCase {
            pattern: "ERROR Exception",
            sample: &[
                "ERROR NullException raised",
                "ERROR only",
                "Exception only",
            ],
            expected: &[1],
        },
        // Unstructured: a `-` term must not appear.
        MatchCase {
            pattern: "ERROR -Exception",
            sample: &[
                "ERROR NullException raised",
                "ERROR only",
                "Exception only",
            ],
            expected: &[2],
        },
        // Unstructured: `?` terms are alternatives.
        MatchCase {
            pattern: "?ERROR ?WARN",
            sample: &["ERROR x", "WARN y", "INFO z"],
            expected: &[1, 2],
        },
        // Unstructured: a quoted term is one phrase, spaces included.
        MatchCase {
            pattern: "\"Internal Server Error\"",
            sample: &[
                "500 Internal Server Error",
                "Internal Error",
                "Internal  Server Error",
            ],
            expected: &[1],
        },
        // Unstructured: a `%…%` term is a regular expression, whose own escape sequences
        // are part of the expression.
        MatchCase {
            pattern: "%ERR[0-9]+%",
            sample: &["ERR42 happened", "ERR happened"],
            expected: &[1],
        },
        MatchCase {
            pattern: "%ERR\\d{3}%",
            sample: &["ERR429 happened", "ERR42 happened", "ERRd{3}"],
            expected: &[1],
        },
        MatchCase {
            pattern: "%^v1\\.2$%",
            sample: &["v1.2", "v1x2"],
            expected: &[1],
        },
        // The empty pattern describes every event.
        MatchCase {
            pattern: "",
            sample: &["anything at all", "{\"a\":1}"],
            expected: &[1, 2],
        },
        // Space-delimited: a list of N fields describes an event of AT LEAST N fields, naming
        // its leading fields, where a `"…"` or `[…]` run counts as one field.
        MatchCase {
            pattern: "[ip, user, username, timestamp, request, status_code, bytes]",
            sample: &[APACHE_EVENT, "one two three"],
            expected: &[1],
        },
        MatchCase {
            pattern: "[w1, w2, w3]",
            sample: &["a b c", "a b", "a b c d"],
            expected: &[1, 3],
        },
        // Space-delimited: `...` stands for any number of fields.
        MatchCase {
            pattern: "[w1, w2, ...]",
            sample: &["a b", "a b c d", "a"],
            expected: &[1, 2],
        },
        MatchCase {
            pattern: "[..., status_code, bytes]",
            sample: &[APACHE_EVENT, "200 2326", "only-one"],
            expected: &[1, 2],
        },
        // Space-delimited: a condition on one named field.
        MatchCase {
            pattern: "[ip, user, username, timestamp, request, status_code=404, bytes]",
            sample: &[APACHE_EVENT, APACHE_EVENT_NOT_FOUND],
            expected: &[2],
        },
        MatchCase {
            pattern: "[ip, user, username, timestamp, request, status_code, bytes>1000]",
            sample: &[APACHE_EVENT, APACHE_EVENT_NOT_FOUND],
            expected: &[1],
        },
        // Space-delimited: an unnamed slot still counts as a field, and a condition can
        // name the slot it is written on.
        MatchCase {
            pattern: "[, , size>1000]",
            sample: &["a b 2000", "a b 500", "a b"],
            expected: &[1],
        },
        // Space-delimited: a `*` literal stands for any run of characters.
        MatchCase {
            pattern: "[ip, user, username, timestamp, request=\"*missing*\", status_code, bytes]",
            sample: &[APACHE_EVENT, APACHE_EVENT_NOT_FOUND],
            expected: &[2],
        },
        // JSON: a string condition, and an event that is no JSON document at all.
        MatchCase {
            pattern: "{$.eventType = \"UpdateTrail\"}",
            sample: &[
                "{\"eventType\":\"UpdateTrail\"}",
                "{\"eventType\":\"CreateTrail\"}",
                "UpdateTrail",
            ],
            expected: &[1],
        },
        // JSON: a numeric comparison.
        MatchCase {
            pattern: "{$.bandwidth > 75}",
            sample: &["{\"bandwidth\":100}", "{\"bandwidth\":50}"],
            expected: &[1],
        },
        // JSON: a conjunction over a nested path and a wildcard literal.
        MatchCase {
            pattern: "{$.user.type = \"IAMUser\" && $.errorCode = \"*Denied*\"}",
            sample: &[
                "{\"user\":{\"type\":\"IAMUser\"},\"errorCode\":\"AccessDenied\"}",
                "{\"user\":{\"type\":\"Root\"},\"errorCode\":\"AccessDenied\"}",
                "{\"user\":{\"type\":\"IAMUser\"},\"errorCode\":\"Throttled\"}",
            ],
            expected: &[1],
        },
        // JSON: an alternation.
        MatchCase {
            pattern: "{$.code = 404 || $.code = 500}",
            sample: &["{\"code\":404}", "{\"code\":500}", "{\"code\":200}"],
            expected: &[1, 2],
        },
        // JSON: an array index.
        MatchCase {
            pattern: "{$.items[0].id = 7}",
            sample: &[
                "{\"items\":[{\"id\":7},{\"id\":8}]}",
                "{\"items\":[{\"id\":8},{\"id\":7}]}",
                "{\"items\":[]}",
            ],
            expected: &[1],
        },
        // JSON: a boolean state.
        MatchCase {
            pattern: "{$.flag IS TRUE}",
            sample: &["{\"flag\":true}", "{\"flag\":false}", "{\"other\":1}"],
            expected: &[1],
        },
        // JSON: the absence of a field.
        MatchCase {
            pattern: "{$.optional NOT EXISTS}",
            sample: &["{\"other\":1}", "{\"optional\":1}"],
            expected: &[1],
        },
    ];

    for case in cases {
        let reported = matches(&logs, case.pattern, case.sample).await;
        let numbers = event_numbers(&reported);
        assert_eq!(
            numbers, case.expected,
            "case {:?} over {:?}: expected the events {:?} to match, got {:?}",
            case.pattern, case.sample, case.expected, numbers
        );
        for (entry, number) in reported.iter().zip(&numbers) {
            let message = entry.get("eventMessage").and_then(Value::as_str);
            let sent = case
                .sample
                .get(usize::try_from(*number - 1).expect("a 1-based event number"))
                .copied();
            assert_eq!(
                message, sent,
                "case {:?}: a match echoes the message at its event number, got {entry}",
                case.pattern
            );
        }
    }
}

/// What a matching event yields: the values the pattern extracted from it, under the keys
/// that select them.
///
/// The rule: a space-delimited field lands under `$name`, or under `$position` when the
/// list left it unnamed; a JSON leaf lands under the `$.path` that selects it; an
/// unstructured pattern names no field of the event, so it extracts nothing at all.
#[tokio::test]
async fn a_match_carries_the_values_the_pattern_extracted() {
    let logs = Logs::new();

    /// One row: a pattern, one event, and the values it extracts from it.
    struct ExtractCase {
        pattern: &'static str,
        event: &'static str,
        expected: &'static [(&'static str, &'static str)],
    }

    let cases = [
        // Named space-delimited fields, including the two grouped runs — the extracted
        // value excludes the `[`/`]` and `"` that grouped it.
        ExtractCase {
            pattern: "[ip, user, username, timestamp, request, status_code, bytes]",
            event: APACHE_EVENT,
            expected: &[
                ("$ip", "127.0.0.1"),
                ("$user", "-"),
                ("$username", "frank"),
                ("$timestamp", "10/Oct/2000:13:25:15 -0700"),
                ("$request", "GET /apache_pb.gif HTTP/1.0"),
                ("$status_code", "200"),
                ("$bytes", "2326"),
            ],
        },
        // An unnamed slot is extracted under its 1-based position.
        ExtractCase {
            pattern: "[, , bytes]",
            event: "alpha beta 99",
            expected: &[("$1", "alpha"), ("$2", "beta"), ("$bytes", "99")],
        },
        // `...` consumes the fields it stands for, so they are not extracted.
        ExtractCase {
            pattern: "[..., status_code, bytes]",
            event: APACHE_EVENT,
            expected: &[("$status_code", "200"), ("$bytes", "2326")],
        },
        // Every leaf of a JSON event, under the path that selects it.
        ExtractCase {
            pattern: "{$.a = 1}",
            event: "{\"a\":1,\"b\":[\"x\",\"y\"],\"c\":{\"d\":true}}",
            expected: &[
                ("$.a", "1"),
                ("$.b[0]", "x"),
                ("$.b[1]", "y"),
                ("$.c.d", "true"),
            ],
        },
        // An unstructured pattern selects no field, so it extracts nothing.
        ExtractCase {
            pattern: "ERROR",
            event: "ERROR disk full",
            expected: &[],
        },
    ];

    for case in cases {
        let reported = matches(&logs, case.pattern, &[case.event]).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the event matches, got {reported:?}",
            case.pattern
        );
        let found = extracted(&reported[0]);
        let expected: BTreeMap<String, String> = case
            .expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            found, expected,
            "case {:?} over {:?}: expected the values {expected:?}, got {found:?}",
            case.pattern, case.event
        );
    }
}

/// An event number is the message's position in the sample the REQUEST sent, not its
/// position among the matches — so a caller can tell which of its messages matched.
#[tokio::test]
async fn an_event_number_is_the_position_in_the_sample_sent() {
    let logs = Logs::new();

    // The sample grows one message at a time, and the one matching message moves with it.
    for ahead in 0..5usize {
        let mut sample: Vec<&str> = vec!["nothing here"; ahead];
        sample.push("ERROR disk full");
        sample.push("nothing here either");
        let reported = matches(&logs, "ERROR", &sample).await;
        let numbers = event_numbers(&reported);
        let expected = vec![i64::try_from(ahead + 1).expect("a small position")];
        assert_eq!(
            numbers, expected,
            "case {ahead} leading messages: expected the event numbers {expected:?}, got {numbers:?}"
        );
    }

    // A sample nothing matches reports an empty list rather than no list.
    let reported = matches(&logs, "ERROR", &["all good", "still good"]).await;
    assert!(
        reported.is_empty(),
        "case no match: expected no matched events, got {reported:?}"
    );
}

/// Every defect the filter-pattern syntax has is refused as a bad parameter, with the one
/// fixed text the service answers for a METRIC filter's pattern it cannot read — and nothing
/// is written while refusing. The two defects that carry a message of their own are the
/// bracketed form that closes with the wrong delimiter and the regular-expression quota.
#[tokio::test]
async fn a_pattern_that_does_not_parse_is_refused() {
    let logs = Logs::new();

    let three_regexes = "%a% %b% %c%";
    let quota = format!(
        "a single filter pattern can contain at most {MAX_REGEX_PER_PATTERN} regular \
         expression patterns"
    );
    // Source: the recorded refusal for a metric filter's `filterPattern` the service cannot
    // read, which carries this text and no detail about what it could not read.
    let unreadable = "Invalid metric filter pattern".to_string();

    let cases: [(&str, String); 18] = [
        (
            "[ip, user",
            "If a filter pattern starts with '[' it must end with ']'".to_string(),
        ),
        (
            "{$.a = 1",
            "If a filter pattern starts with '{' it must end with '}'".to_string(),
        ),
        // A quoted string, a regular-expression literal or a bracket the pattern never
        // closes.
        ("[a, \"b]", unreadable.clone()),
        ("[%a]", unreadable.clone()),
        ("[a, b]]", unreadable.clone()),
        // A space-delimited pattern that names no field, or uses the `...` wildcard twice,
        // or whose condition names a field it never declared.
        ("[]", unreadable.clone()),
        ("[a, ..., b, ..., c]", unreadable.clone()),
        ("[a, b, c = 1 && $z9 = 2]", unreadable.clone()),
        // A JSON pattern that states no condition on a `$.` selector, or selects a field
        // some other way.
        ("{}", unreadable.clone()),
        ("{$a = 1}", unreadable.clone()),
        // A condition states its structure with `&&`, `||` and `( )`; a comma is no part of
        // it, and neither is a single `&` or `|`.
        ("{$.a = 1, $.b = 2}", unreadable.clone()),
        ("[a, (b,c)]", unreadable.clone()),
        ("{$.a = 1 & $.b = 2}", unreadable.clone()),
        ("{$.a = 1 | $.b = 2}", unreadable.clone()),
        // A `?` or `-` prefixes a term; one that prefixes nothing names no term, whether the
        // pattern ends there or whitespace follows it.
        ("ERROR - x", unreadable.clone()),
        ("ERROR ?", unreadable.clone()),
        // A `$.path` segment is a member name and the element selectors applied to it; a
        // bracket group that is neither an index nor `*` selects nothing.
        ("{$.a[x] = 1}", unreadable.clone()),
        (three_regexes, quota),
    ];

    for (pattern, expected) in &cases {
        let (code, message) = logs
            .refusal(
                "TestMetricFilter",
                &json!({"filterPattern": pattern, "logEventMessages": ["anything"]}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {pattern:?}: expected InvalidParameterException, got {code} {message}"
        );
        assert_eq!(
            &message, expected,
            "case {pattern:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}

/// Malformed, truncated and oversized input is answered, never run: the pattern reader
/// carries its own bounds, so no request drives it past them.
///
/// The rule: a pattern is at most the modelled 1024 characters, a condition nests at most a
/// bounded number of parenthesised groups, and anything the syntax does not describe — a
/// pattern cut off at any character — is a refusal rather than a server fault or a verdict
/// the pattern does not support.
#[tokio::test]
async fn malformed_truncated_and_oversized_input_is_refused_rather_than_run() {
    let logs = Logs::new();

    /// The longest pattern the model admits. Source: the `FilterPattern` shape,
    /// `length 0..1024`.
    const MAX_PATTERN_CHARS: usize = 1024;

    // Each published edge of the length bound, and one past it.
    for (characters, admitted) in [
        (MAX_PATTERN_CHARS - 1, true),
        (MAX_PATTERN_CHARS, true),
        (MAX_PATTERN_CHARS + 1, false),
    ] {
        let pattern = "a".repeat(characters);
        let (status, body) = logs
            .call(
                "TestMetricFilter",
                &json!({"filterPattern": pattern, "logEventMessages": ["aaa"]}),
            )
            .await;
        assert_eq!(
            status,
            if admitted { 200 } else { 400 },
            "case {characters} characters: expected admitted={admitted}, got {status} {body}"
        );
    }

    // Nesting far past any bound the reader keeps is a refusal, not a fault.
    let deep = format!("{{{}$.a = 1{}}}", "(".repeat(256), ")".repeat(256));
    let (status, body) = logs
        .call(
            "TestMetricFilter",
            &json!({"filterPattern": deep, "logEventMessages": ["{\"a\":1}"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case 256 nested groups: expected a refusal, got {status} {body}"
    );

    // A deeply nested EVENT is data rather than a pattern: it is answered, matched or not,
    // without a fault.
    let nested_event = format!("{}1{}", "[".repeat(64), "]".repeat(64));
    let (status, body) = logs
        .call(
            "TestMetricFilter",
            &json!({"filterPattern": "{$.a = 1}", "logEventMessages": [nested_event]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case a deeply nested event: expected it to be answered, got {status} {body}"
    );

    // Every prefix of a well-formed pattern of each form — a truncated pattern — is either
    // read or refused, and never answered with a server fault or a 501.
    for whole in [
        "[ip, user, username, timestamp, request, status_code=404, bytes]",
        "{$.user.type = \"IAMUser\" && $.items[0].id > 7}",
        "ERROR -\"Internal Server Error\" ?WARN %ERR[0-9]+%",
    ] {
        for cut in 0..=whole.chars().count() {
            let pattern: String = whole.chars().take(cut).collect();
            let (status, body) = logs
                .call(
                    "TestMetricFilter",
                    &json!({"filterPattern": pattern, "logEventMessages": ["a b c"]}),
                )
                .await;
            assert!(
                status == 200 || status == 400,
                "case {pattern:?} (a {cut}-character prefix of {whole:?}): expected it to be \
                 read or refused, got {status} {body}"
            );
            if status == 400 {
                assert_eq!(
                    crate::harness::error_code(&body),
                    "InvalidParameterException",
                    "case {pattern:?} (a {cut}-character prefix of {whole:?}): a pattern the \
                     syntax does not describe is a bad parameter, got {body}"
                );
            }
        }
    }
}

/// What a caller can make the EXTRACTION do is bounded too: a message larger than the
/// largest log event the service holds is refused, and an event with more leaves or more
/// bytes than the extraction reports is answered with at most what the bounds allow.
///
/// The rule: a sample log event message is at most the documented maximum log event size,
/// and the values one match reports are capped in count and in bytes, so a small request
/// cannot be amplified into an arbitrarily larger response.
#[tokio::test]
async fn the_extraction_a_caller_can_drive_is_bounded() {
    let logs = Logs::new();

    /// The largest single log event CloudWatch Logs holds. Source: `PutLogEvents` — "Each
    /// log event can be no larger than 1 MB."
    const MAX_LOG_EVENT_BYTES: usize = 1_048_576;

    // Each edge of the per-message bound, and one past it.
    for (bytes, admitted) in [
        (MAX_LOG_EVENT_BYTES - 1, true),
        (MAX_LOG_EVENT_BYTES, true),
        (MAX_LOG_EVENT_BYTES + 1, false),
    ] {
        let message = "m".repeat(bytes);
        let (status, body) = logs
            .call(
                "TestMetricFilter",
                &json!({"filterPattern": "ERROR", "logEventMessages": [message]}),
            )
            .await;
        assert_eq!(
            status,
            if admitted { 200 } else { 400 },
            "case a {bytes}-byte message: expected admitted={admitted}, got {status}"
        );
        if !admitted {
            assert_eq!(
                crate::harness::error_code(&body),
                "InvalidParameterException",
                "case a {bytes}-byte message: expected InvalidParameterException, got {body}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some("Each log event can be no larger than 1 MB."),
                "case a {bytes}-byte message: expected the documented sentence, got {body}"
            );
        }
    }

    // An event with far more leaves than the extraction reports is answered, with at most
    // the bounded number of values.
    let leaves: Vec<String> = (0..20_000).map(|index| format!("\"k{index}\":1")).collect();
    let wide_event = format!("{{\"a\":1,{}}}", leaves.join(","));
    let reported = matches(&logs, "{$.a = 1}", &[wide_event.as_str()]).await;
    assert_eq!(
        reported.len(),
        1,
        "case a wide event: the event matches, got {reported:?}"
    );
    let found = extracted(&reported[0]);
    assert!(
        !found.is_empty() && found.len() <= MAX_EXTRACTED_VALUES,
        "case a wide event: expected between 1 and {MAX_EXTRACTED_VALUES} extracted values, \
         got {}",
        found.len()
    );
    let weight: usize = found
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum();
    assert!(
        weight <= MAX_EXTRACTED_BYTES,
        "case a wide event: expected at most {MAX_EXTRACTED_BYTES} bytes of extracted keys \
         and values, got {weight}"
    );

    // One enormous leaf is bounded by the byte cap rather than by the entry cap.
    let heavy_event = format!("{{\"a\":1,\"big\":\"{}\"}}", "x".repeat(400_000));
    let reported = matches(&logs, "{$.a = 1}", &[heavy_event.as_str()]).await;
    let found = extracted(&reported[0]);
    let weight: usize = found
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum();
    assert!(
        weight <= MAX_EXTRACTED_BYTES,
        "case one enormous leaf: expected at most {MAX_EXTRACTED_BYTES} bytes of extracted \
         keys and values, got {weight}"
    );
}

/// `TestMetricFilter` "tests the filter pattern of a metric filter against a sample of log
/// event messages": it reports a verdict on the request and writes nothing, so the metric
/// filters a log group holds are the same after the call as before it.
#[tokio::test]
async fn testing_a_pattern_writes_nothing() {
    let logs = Logs::new();
    let group = "test-metric-filter-group";

    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    logs.ok(
        "PutMetricFilter",
        &json!({
            "logGroupName": group,
            "filterName": "errors",
            "filterPattern": "ERROR",
            "metricTransformations": [{
                "metricName": "Errors",
                "metricNamespace": "Test",
                "metricValue": "1",
            }],
        }),
    )
    .await;

    let before = logs
        .ok("DescribeMetricFilters", &json!({"logGroupName": group}))
        .await;

    // A pattern naming the same group's filter, and one that is refused, are both answered
    // without touching the store.
    matches(&logs, "[ip, status_code, bytes]", &["1.2.3.4 200 10"]).await;
    logs.refusal(
        "TestMetricFilter",
        &json!({"filterPattern": "[ip", "logEventMessages": ["1.2.3.4 200 10"]}),
    )
    .await;

    let after = logs
        .ok("DescribeMetricFilters", &json!({"logGroupName": group}))
        .await;
    assert_eq!(
        before, after,
        "case {group}: testing a pattern leaves the group's metric filters unchanged, \
         got {after} after {before}"
    );

    let group_after = logs
        .describe_group(group)
        .await
        .unwrap_or_else(|| panic!("case {group}: the log group still exists"));
    assert_eq!(
        group_after.get("metricFilterCount").and_then(Value::as_i64),
        Some(1),
        "case {group}: testing a pattern creates no metric filter, got {group_after}"
    );
}
