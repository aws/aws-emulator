// SPDX-License-Identifier: CC0-1.0

//! What a client is told when it names a metric stream this account holds none of, and what
//! the operations that ACCEPT such a name leave behind.
//!
//! Two rules are under test here, both of them about a name no stream exists under:
//!
//! 1. the refusal of an error class the service answers by its CODE alone carries no text at
//!    all — the code is the whole contract — however the requested name misses, at either end
//!    of the modelled length included (the spread of ways a well-formed name can miss is
//!    tabled in [`crate::not_found_messages`], which this suite does not repeat);
//! 2. an operation that has no modelled way to report a missing stream accepts the name,
//!    changes nothing for it and materialises nothing under it — so after the call the name is
//!    still refused the same way, still appears in no listing, and a name a delete FREED is a
//!    name a fresh configuration can take, with none of the deleted stream's state or tags.
//!
//! Every table spans those rules along what they generalise over (how long the requested name
//! is and whether the store ever held it, where an unknown name sits in a batch and how often,
//! which batch operation is called, and what each configuration of a reused name carries), so
//! the recorded sequence — one unknown name beside one real one — is one row among many.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, error_message, merged, ok};

/// The error class a read of an absent metric stream is refused with
/// (`GetMetricStream`'s modelled `ResourceNotFoundException`, HTTP 404, whose `awsQueryError`
/// code is the same name).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The code a value outside its own modelled bound is refused with: the boundary validator
/// refuses it before any handler runs, and this service keeps that wording for a scalar (only
/// a collection cap, a list element outside an enumerated type and a SCALAR outside one are
/// restated — `crate::EmulatedService`'s refusal override).
const VALIDATION_CODE: &str = "ValidationException";

/// The code a parameter supplied EMPTY is refused with: a parameter carrying no characters is
/// a parameter the request gave no value for, which this service answers as the
/// missing-parameter fault whatever member it arrived on (`crate::EmulatedService`'s refusal
/// override) — still a 400 refusal of the name rather than a missing stream, which is what the
/// row below is about.
const MISSING_PARAMETER_CODE: &str = "MissingRequiredParameterException";

/// The two states a metric stream can be in: "The state of the metric stream. The possible
/// values are `running` and `stopped`." (`GetMetricStreamOutput$State`).
const RUNNING: &str = "running";
/// The stopped state of that pair.
const STOPPED: &str = "stopped";

/// The output format a row that does not vary it is configured with: "Valid values are json,
/// opentelemetry1.0, and opentelemetry0.7." (`GetMetricStreamOutput$OutputFormat`).
const DEFAULT_OUTPUT_FORMAT: &str = "json";

/// A `PutMetricStream` request for `name` with `extra` merged over it.
///
/// The required members come from the one builder every metric-stream suite in this target
/// writes them with ([`crate::metric_streams::put_request`]) and the row's own members are
/// merged over them by the harness's own merge ([`merged`]), so a row states only what it
/// varies.
fn put_request(name: &str, extra: &Value) -> Value {
    merged(
        crate::metric_streams::put_request(name, DEFAULT_OUTPUT_FORMAT),
        extra,
    )
}

/// Assert that `name` names no metric stream: HTTP 404 with the not-found code and a message
/// a client reads as empty.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "GetMetricStream", &json!({"Name": name})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, NOT_FOUND_CODE),
        "case {case:?}: expected HTTP 404 {NOT_FOUND_CODE} for {name:?}, got {status} {body}"
    );
    let message = error_message(&body);
    assert!(
        message.is_empty(),
        "case {case:?}: this refusal is answered by its code alone, so {name:?} must come back \
         with no message, got {message:?} in {body}"
    );
    // A refusal answered by its code alone states no reason, so the member that would carry one
    // is ABSENT rather than present and empty.
    assert_eq!(
        body.get("Message").and_then(Value::as_str),
        None,
        "case {case:?}: expected the refusal of {name:?} to carry no Message member, \
         got {body}"
    );
}

/// One stream as `GetMetricStream` reports it, requiring the read to succeed.
async fn stream_of(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetMetricStream", &json!({"Name": name})).await
}

/// The stream names `ListMetricStreams` reports, in the order it reports them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListMetricStreams", &json!({})).await;
    body.get("Entries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListMetricStreams must report an Entries array, got {body}"))
        .iter()
        .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

/// The tag pairs `ListTagsForResource` reports for `arn`, requiring the read to succeed.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> Vec<Value> {
    let body = ok(router, "ListTagsForResource", &json!({"ResourceARN": arn})).await;
    body.get("Tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_name_at_either_modelled_length_bound_is_refused_by_its_code_alone() {
    let router = build_router();
    ok(
        &router,
        "PutMetricStream",
        &put_request(
            "held-stream",
            &json!({"Tags": [{"Key": "Env", "Value": "prod"}]}),
        ),
    )
    .await;
    ok(
        &router,
        "PutMetricStream",
        &put_request("removed-stream", &json!({})),
    )
    .await;
    ok(
        &router,
        "DeleteMetricStream",
        &json!({"Name": "removed-stream"}),
    )
    .await;

    // (case, the name the read asks for, whether it is a name the model lets through to the
    // store at all). `MetricStreamName` is length 1..255 (the model's own constraint), so each
    // published bound is a well-formed name this store simply holds no stream for, while one
    // character past either bound is refused before any lookup happens — a length fault, which
    // is not a not-found. The spread of ways a WELL-FORMED name can miss (never created,
    // created then deleted, a stored name in another case or with a suffix) is tabled in
    // `crate::not_found_messages`; the rows here are the lengths that table does not reach,
    // plus the one name this suite's own deletes leave behind.
    let cases: Vec<(String, String, bool)> = vec![        (
            "a name created and then deleted".to_string(),
            "removed-stream".to_string(),
            true,
        ),
        ("a name of 1 character".to_string(), "a".to_string(), true),
        (
            "a name of 255 characters".to_string(),
            "n".repeat(255),
            true,
        ),
        (
            "a name of 256 characters".to_string(),
            "n".repeat(256),
            false,
        ),
        ("the empty name".to_string(), String::new(), false),
    ];

    for (case, name, well_formed) in cases {
        if well_formed {
            assert_absent(&router, &case, &name).await;
            continue;
        }
        let (status, body) = call(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name outside the modelled length must be refused with HTTP 400, \
             got {status} {body}"
        );
        // Both are a fault of the VALUE rather than a missing resource, and which of the two
        // depends on the value alone: a name of no characters is a parameter the request gave
        // no value for, so it is the missing-parameter fault; any other name outside the
        // modelled length is the request-validation class.
        let expected = if name.is_empty() {
            MISSING_PARAMETER_CODE
        } else {
            VALIDATION_CODE
        };
        assert_eq!(
            error_code(&body).as_str(),
            expected,
            "case {case:?}: a name outside the modelled length is a value fault, not a missing \
             resource, got {body}"
        );
    }

    // The refused reads changed nothing: the one stream the store holds still reads back, and
    // it is still the only one listed.
    let held = stream_of(&router, "held-stream").await;
    assert_eq!(
        held.get("State").and_then(Value::as_str),
        Some(RUNNING),
        "case \"after the refused reads\": the stored stream must still be reported running, \
         got {held}"
    );
    let listed = listed_names(&router).await;
    assert_eq!(
        listed,
        vec!["held-stream".to_string()],
        "case \"after the refused reads\": no refused read may be materialised into a listable \
         stream, got {listed:?}"
    );
}

#[tokio::test]
async fn a_deleted_metric_stream_frees_its_name_for_a_fresh_configuration() {
    // (case, what the first configuration adds to the required members, what the second one
    // adds). The rows differ along what each configuration carries — tags or none, which of
    // the two mutually exclusive namespace filters, which output format — because the freed
    // name takes the SECOND configuration whatever either of them holds.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a tagged json stream replaced by an untagged OpenTelemetry one",
            json!({"Tags": [{"Key": "Env", "Value": "prod"}]}),
            json!({"OutputFormat": "opentelemetry1.0"}),
        ),
        (
            "a filtered stream replaced by one filtered the other way, and tagged",
            json!({
                "OutputFormat": "opentelemetry0.7",
                "IncludeFilters": [{"Namespace": "AWS/EC2"}],
            }),
            json!({
                "ExcludeFilters": [{"Namespace": "AWS/Lambda", "MetricNames": ["Errors"]}],
                "Tags": [{"Key": "Team", "Value": "search"}],
            }),
        ),
        (
            "two tags replaced by the same configuration with none",
            json!({
                "Tags": [{"Key": "Env", "Value": "dev"}, {"Key": "Team", "Value": "search"}],
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                    "AdditionalStatistics": ["p99"],
                }],
            }),
            json!({}),
        ),
    ];

    for (case, first, second) in cases {
        let router = build_router();
        let name = "reusable-stream";

        let created = ok(&router, "PutMetricStream", &put_request(name, &first)).await;
        let arn = created
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: PutMetricStream must report an Arn, got {created}"))
            .to_string();

        // The stream is stopped before it is deleted, the way a client winds one down, so the
        // state the delete removes is NOT the state a fresh configuration starts in.
        ok(&router, "StopMetricStreams", &json!({"Names": [name]})).await;
        let stopped = stream_of(&router, name).await;
        assert_eq!(
            stopped.get("State").and_then(Value::as_str),
            Some(STOPPED),
            "case {case:?}: the configuration must still be retrievable while stopped, got \
             {stopped}"
        );

        ok(&router, "DeleteMetricStream", &json!({"Name": name})).await;
        assert_absent(&router, case, name).await;
        // The ARN names nothing to tag either: the tags went with the stream.
        let (tag_status, tag_body) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            (tag_status, error_code(&tag_body).as_str()),
            (404, NOT_FOUND_CODE),
            "case {case:?}: the deleted stream's ARN must name no taggable resource, got \
             {tag_status} {tag_body}"
        );

        // The freed name is taken by a fresh stream, not by the deleted one coming back.
        let recreated = ok(&router, "PutMetricStream", &put_request(name, &second)).await;
        assert_eq!(
            recreated.get("Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: a stream's ARN is its Region and its name, so the reused name must \
             report {arn}, got {recreated}"
        );
        let fresh = stream_of(&router, name).await;
        assert_eq!(
            fresh.get("State").and_then(Value::as_str),
            Some(RUNNING),
            "case {case:?}: a newly created stream starts running, so the deleted stream's \
             stopped state must not survive it, got {fresh}"
        );

        // Every member the second request named is reported back, and a member only the FIRST
        // request carried is gone: the stream is the second configuration and nothing else.
        let second_request = put_request(name, &second);
        for (member, value) in second_request.as_object().expect("a request object") {
            if member == "Tags" {
                // "To change the tags of an existing metric stream, use TagResource or
                // UntagResource." (`PutMetricStreamInput$Tags`) — `GetMetricStream` reports no
                // tags, so they are read below through the tag reader instead.
                continue;
            }
            assert_eq!(
                fresh.get(member.as_str()),
                Some(value),
                "case {case:?}: expected the reused name to report {member} as {value}, got \
                 {fresh}"
            );
        }
        for member in ["IncludeFilters", "ExcludeFilters", "StatisticsConfigurations"] {
            if second_request.get(member).is_some() {
                continue;
            }
            assert!(
                fresh.get(member).is_none(),
                "case {case:?}: the fresh configuration names no {member}, so the deleted \
                 stream's must not survive, got {fresh}"
            );
        }

        let tags = listed_tags(&router, &arn).await;
        let expected = second
            .get("Tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tags, expected,
            "case {case:?}: the reused name must carry only the tags its own creation named, \
             got {tags:?}"
        );
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec![name.to_string()],
            "case {case:?}: the reused name must be listed exactly once, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_name_in_a_batch_is_ignored_and_never_materialised() {
    // The name every row's real stream is created under.
    const REAL: &str = "real-stream";
    // A name nothing is ever created under, and one created and then deleted: the two ways a
    // batch's name can miss.
    const NEVER_MADE: &str = "never-made";
    const REMOVED: &str = "removed-stream";

    // (case, the operation called, the state the real stream is wound to first, the names the
    // batch carries, the state the real stream must be in afterwards). The rows differ along
    // which batch operation is called, where the unknown names sit in the list, how many of
    // them there are and whether one repeats.
    let cases: Vec<(&str, &str, &str, Vec<&str>, &str)> = vec![
        (
            "a Stop batch whose unknown name comes last",
            "StopMetricStreams",
            RUNNING,
            vec![REAL, NEVER_MADE],
            STOPPED,
        ),
        (
            "a Stop batch whose unknown name comes first",
            "StopMetricStreams",
            RUNNING,
            vec![NEVER_MADE, REAL],
            STOPPED,
        ),
        (
            "a Stop batch repeating one unknown name",
            "StopMetricStreams",
            RUNNING,
            vec![NEVER_MADE, NEVER_MADE, REAL],
            STOPPED,
        ),
        (
            "a Start batch whose unknown name comes last",
            "StartMetricStreams",
            STOPPED,
            vec![REAL, NEVER_MADE],
            RUNNING,
        ),
        (
            "a Start batch with an unknown name on either side of the real one",
            "StartMetricStreams",
            STOPPED,
            vec![NEVER_MADE, REAL, REMOVED],
            RUNNING,
        ),
        (
            "a Start batch naming unknown names only",
            "StartMetricStreams",
            STOPPED,
            vec![NEVER_MADE, REMOVED],
            STOPPED,
        ),
        (
            "a Stop batch naming unknown names only",
            "StopMetricStreams",
            RUNNING,
            vec![REMOVED, NEVER_MADE],
            RUNNING,
        ),
    ];

    for (case, operation, baseline, names, after_state) in cases {
        let router = build_router();
        ok(&router, "PutMetricStream", &put_request(REAL, &json!({}))).await;
        // The deleted name is a name this store HELD and no longer does, which is the other
        // way a batch's name misses.
        ok(&router, "PutMetricStream", &put_request(REMOVED, &json!({}))).await;
        ok(&router, "DeleteMetricStream", &json!({"Name": REMOVED})).await;
        if baseline == STOPPED {
            ok(&router, "StopMetricStreams", &json!({"Names": [REAL]})).await;
        }
        let before = stream_of(&router, REAL).await;
        assert_eq!(
            before.get("State").and_then(Value::as_str),
            Some(baseline),
            "case {case:?}: the row winds the real stream to {baseline:?} first, got {before}"
        );

        let (status, body) = call(&router, operation, &json!({"Names": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unknown name in the batch must not refuse the call, got {status} \
             {body}"
        );

        // Only `State` may differ, and only for the stream the batch actually named — beside
        // the `LastUpdateDate` a transition stamps ("The date that the configuration of this
        // metric stream was most recently updated.", `GetMetricStreamOutput$LastUpdateDate`),
        // which a batch that moved the stream nowhere leaves alone.
        let after = stream_of(&router, REAL).await;
        let mut expected = before.clone();
        expected["State"] = json!(after_state);
        if after_state != baseline {
            // Two calls inside one millisecond stamp the same instant, which the timestamp
            // type cannot tell apart, so the date never moves BACKWARDS.
            assert!(
                after.get("LastUpdateDate").and_then(Value::as_f64)
                    >= before.get("LastUpdateDate").and_then(Value::as_f64),
                "case {case:?}: a transition must not move LastUpdateDate back, got {after}"
            );
            expected["LastUpdateDate"] = after["LastUpdateDate"].clone();
        }
        assert_eq!(
            after, expected,
            "case {case:?}: expected the real stream to be reported as {expected}, got {after}"
        );

        // The unknown names are still unknown, and the listing still holds the one real
        // stream: the call materialised nothing.
        for unknown in [NEVER_MADE, REMOVED] {
            assert_absent(&router, case, unknown).await;
        }
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec![REAL.to_string()],
            "case {case:?}: no name the batch could not resolve may become a listable stream, \
             got {listed:?}"
        );

        // The same call naming the real stream alone asks for the state its own operation
        // owns: it answers 200 and reports exactly the stream the batch above left, except
        // where the batch did not move it there — a transition stamps `LastUpdateDate`, and a
        // call that moves the stream nowhere stamps nothing.
        let (again_status, again_body) = call(&router, operation, &json!({"Names": [REAL]})).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected HTTP 200 from the solo {operation}, got {again_status} \
             {again_body}"
        );
        let repeated = stream_of(&router, REAL).await;
        let solo_state = if operation == "StartMetricStreams" {
            RUNNING
        } else {
            STOPPED
        };
        let mut expected = after.clone();
        expected["State"] = json!(solo_state);
        if after.get("State").and_then(Value::as_str) != Some(solo_state) {
            expected["LastUpdateDate"] = repeated["LastUpdateDate"].clone();
        }
        assert_eq!(
            repeated, expected,
            "case {case:?}: the solo {operation} must leave {expected}, got {repeated}"
        );
    }
}
