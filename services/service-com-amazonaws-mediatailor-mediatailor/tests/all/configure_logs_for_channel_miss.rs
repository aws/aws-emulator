// SPDX-License-Identifier: CC0-1.0

//! What `ConfigureLogsForChannel` answers for a channel the account does not
//! hold, end to end over HTTP.
//!
//! The harness is the one `channel_lifecycle.rs` owns, so every assertion here
//! goes through the router: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! The rule under test is the one every route that addresses a channel by name
//! implements: a `ChannelName` the account holds no channel under addresses
//! nothing, which is not a malformed request, so the call is refused as absent —
//! HTTP 404 under this service's not-found error type — with the channel-miss
//! sentence, which is entirely FIXED text that names neither the requested
//! channel nor anything else of the request, carried under the one wire key
//! `message` beside the `requestId` naming that response; and nothing is
//! written.
//!
//! The model declares no error on these operations
//! (`smithy_query(query="operation",operation="ConfigureLogsForChannel")` →
//! `errors: []`), so the status and the error type are the service's recorded
//! behaviour, and the one client error the model DOES declare
//! (`BadRequestException`, HTTP 400) is a different claim: "your request
//! contains unexpected data", which a create-if-absent flow must not read as
//! "absent".
//!
//! The tables span the classes the rule generalises over: the forms a name that
//! addresses nothing takes (never created, deleted since it was configured, a
//! stored name in another case, a stored name with a suffix, a prefix of a
//! stored name, a name at the longest form the lifecycle table spans, and one
//! carrying the punctuation a channel name allows), crossed with what the
//! request asks to be logged (no type at all, one type, every type the model
//! declares, and the same type named twice), and — for the generalisation
//! beyond this one operation — every channel-name route this service serves.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{
    encode, Emulator, Reply, CHANNEL_NOT_FOUND_MESSAGE, NOT_FOUND_CODE, NOT_FOUND_STATUS,
};
use crate::channel_response_projection::configure_logs;
use crate::error_envelope::error_body_members;

/// `LogType`'s modelled values, as the generated `ops::types::LOG_TYPE_VALUES`
/// enumerates them (`smithy_query(query="shape",shape="LogType")`), so the row
/// that names "every type" spans the whole enum rather than a hand-written
/// sample of it.
const LOG_TYPES: &[&str] =
    service_com_amazonaws_mediatailor_mediatailor::ops::types::LOG_TYPE_VALUES;

/// The channel the misses below are compared against: one that IS stored, so a
/// refusal is about the name the request carried and not about an empty store.
const STORED: &str = "stored";

/// A channel request that is accepted, so the comparison channel exists.
fn a_channel() -> Value {
    json!({
        "PlaybackMode": "LOOP",
        "Outputs": [{
            "ManifestName": "index",
            "SourceGroup": "default",
            "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
        }],
    })
}

/// What a request asks to be logged. Each row is a well-formed `LogTypes` list,
/// so none of them is what the refusal is about: the answer has to be the same
/// whichever is sent.
fn log_type_requests() -> Vec<(String, Value)> {
    let mut rows = vec![
        ("no log type at all".to_string(), json!([])),
        ("every modelled log type".to_string(), json!(LOG_TYPES)),
        (
            format!("the type {} named twice", LOG_TYPES[0]),
            json!([LOG_TYPES[0], LOG_TYPES[0]]),
        ),
    ];
    // One row per modelled type, each labelled with the type it sends, so a failing row
    // says which one it was.
    for log_type in LOG_TYPES {
        rows.push((format!("the one type {log_type}"), json!([log_type])));
    }
    rows
}

/// Builds an emulator holding [`STORED`] with every modelled log type already
/// configured on it, plus a channel that is created, configured and then
/// deleted — so one row below addresses a name whose channel is gone rather than
/// one that never existed. Returns the deleted name.
async fn emulator_with_a_stored_and_a_deleted_channel() -> (Emulator, String) {
    let emulator = Emulator::new();

    assert_eq!(
        emulator.create(STORED, &a_channel()).await.status,
        200,
        "the channel the misses are compared against must be created"
    );
    assert_eq!(
        configure_logs(&emulator, STORED, &json!(LOG_TYPES)).await.status,
        200,
        "the channel the misses are compared against must accept log types"
    );

    let deleted = "deleted-since-it-was-configured".to_string();
    assert_eq!(
        emulator.create(&deleted, &a_channel()).await.status,
        200,
        "the channel whose name is freed must be created"
    );
    assert_eq!(
        configure_logs(&emulator, &deleted, &json!(LOG_TYPES))
            .await
            .status,
        200,
        "the channel whose name is freed must accept log types while it is stored"
    );
    assert_eq!(
        emulator.delete(&deleted).await.status,
        200,
        "the channel whose name is freed must be deleted"
    );

    (emulator, deleted)
}

/// The names that address no channel: several forms, including one that differs
/// from the stored name only in case and one whose channel has been deleted.
///
/// None of them is a substring of [`CHANNEL_NOT_FOUND_MESSAGE`], so a row can
/// assert that the sentence does not carry the requested name.
fn missing_names(deleted: &str) -> Vec<(&'static str, String)> {
    vec![
        ("never created", "never-created".to_string()),
        ("deleted since it was configured", deleted.to_string()),
        ("a stored name in another case", STORED.to_uppercase()),
        ("a stored name with a suffix", format!("{STORED}-2")),
        ("a prefix of a stored name", STORED[..4].to_string()),
        (
            "a name at the longest form the lifecycle table spans",
            "d".repeat(64),
        ),
        (
            "a name carrying the punctuation a channel name allows",
            "miss.ing_name-9".to_string(),
        ),
    ]
}

/// Asserts `reply` is the whole envelope the rule requires of a channel miss,
/// and returns the `requestId` it named.
fn assert_channel_miss(case: &str, call: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, NOT_FOUND_STATUS,
        "case {case:?}: expected {call} to answer HTTP {NOT_FOUND_STATUS}, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code, NOT_FOUND_CODE,
        "case {case:?}: expected {call} to answer the error type {NOT_FOUND_CODE:?}, got {:?}",
        reply.error_code
    );
    assert_eq!(
        reply.message(),
        CHANNEL_NOT_FOUND_MESSAGE,
        "case {case:?}: expected {call} to answer {CHANNEL_NOT_FOUND_MESSAGE:?}, got {:?}",
        reply.body
    );

    // The body carries the envelope's members and no other key — in particular
    // the text travels under the member the not-found shape this service
    // synthesises declares, and under no second spelling.
    let members: BTreeSet<String> = reply
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        members,
        error_body_members(crate::channel_lifecycle::SYNTHETIC_ERROR_MESSAGE_MEMBER),
        "case {case:?}: expected {call} to answer the error body's members, got {:?}",
        reply.body
    );

    let request_id = reply.request_id();
    assert!(
        service_common::query_error_envelope::is_request_id(&request_id),
        "case {case:?}: expected {call} to name its request, got {request_id:?} in {:?}",
        reply.body
    );
    request_id
}

#[tokio::test]
async fn a_configure_logs_call_on_a_channel_the_account_does_not_hold_is_refused_as_absent() {
    let (emulator, deleted) = emulator_with_a_stored_and_a_deleted_channel().await;

    let mut request_ids: Vec<String> = Vec::new();
    for (name_case, name) in missing_names(&deleted) {
        for (types_case, log_types) in log_type_requests() {
            let case = format!("{name_case} / {types_case} ({name:?})");
            let refused = configure_logs(&emulator, &name, &log_types).await;
            request_ids.push(assert_channel_miss(
                &case,
                "ConfigureLogsForChannel",
                &refused,
            ));

            // The sentence is fixed text: the name the request addressed is not
            // interpolated into it, so two callers asking about two different
            // absent channels read the same words.
            assert!(
                !refused.message().contains(name.as_str()),
                "case {case:?}: expected the sentence not to carry the requested name, \
                 got {:?}",
                refused.message()
            );
        }
    }

    // One id per response: every refusal named its own request.
    let distinct: BTreeSet<&String> = request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "expected every refusal to name its own request, got {request_ids:?}"
    );
}

#[tokio::test]
async fn a_refused_configure_logs_call_writes_nothing() {
    let (emulator, deleted) = emulator_with_a_stored_and_a_deleted_channel().await;

    for (name_case, name) in missing_names(&deleted) {
        for (types_case, log_types) in log_type_requests() {
            let case = format!("{name_case} / {types_case} ({name:?})");
            assert_eq!(
                configure_logs(&emulator, &name, &log_types).await.status,
                NOT_FOUND_STATUS,
                "case {case:?}: expected the call to be refused as absent"
            );

            // The refused call created no channel under the name it addressed...
            let described = emulator.describe(&name).await;
            assert_eq!(
                described.status, NOT_FOUND_STATUS,
                "case {case:?}: expected the refused call to store no channel, got {} {:?}",
                described.status, described.body
            );
            // ...and left the channel that DOES exist carrying the types it was
            // given, rather than the ones the refused request named.
            let stored = emulator.describe(STORED).await;
            assert_eq!(
                stored.get("/LogConfiguration/LogTypes"),
                &json!(LOG_TYPES),
                "case {case:?}: expected the stored channel to keep its own log types, got {:?}",
                stored.get("/LogConfiguration")
            );
        }
    }
}

#[tokio::test]
async fn every_channel_name_route_answers_a_miss_in_the_same_envelope() {
    let (emulator, deleted) = emulator_with_a_stored_and_a_deleted_channel().await;

    // The rule is not this one operation's: every route that resolves a channel
    // by name answers an absent one the same way. The routes are the model's
    // own (`smithy_query(query="operation",operation="DescribeChannel,
    // StartChannel,StopChannel,DeleteChannel,GetChannelSchedule,
    // ConfigureLogsForChannel")`).
    for (name_case, name) in missing_names(&deleted) {
        let calls: Vec<(&str, Reply)> = vec![
            ("DescribeChannel", emulator.describe(&name).await),
            ("DeleteChannel", emulator.delete(&name).await),
            (
                "StartChannel",
                emulator
                    .request("PUT", &format!("/channel/{}/start", encode(&name)), None)
                    .await,
            ),
            (
                "StopChannel",
                emulator
                    .request("PUT", &format!("/channel/{}/stop", encode(&name)), None)
                    .await,
            ),
            (
                "GetChannelSchedule",
                emulator
                    .request("GET", &format!("/channel/{}/schedule", encode(&name)), None)
                    .await,
            ),
            (
                "ConfigureLogsForChannel",
                configure_logs(&emulator, &name, &json!(LOG_TYPES)).await,
            ),
        ];

        for (call, reply) in &calls {
            // A delete is the one route that answers an absent channel with its
            // success status: deleting a channel the account does not hold has
            // nothing left to remove, so it is idempotent rather than refused
            // (pinned by `channel_delete.rs`).
            if *call == "DeleteChannel" {
                assert_eq!(
                    reply.status, 200,
                    "case {name_case:?}: expected a repeat delete to stay idempotent, got {} {:?}",
                    reply.status, reply.body
                );
                continue;
            }
            assert_channel_miss(name_case, call, reply);
        }
    }
}
