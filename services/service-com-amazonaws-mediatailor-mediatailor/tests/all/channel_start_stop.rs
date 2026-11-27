// SPDX-License-Identifier: CC0-1.0

//! Starting and stopping a channel, end to end over HTTP.
//!
//! The harness is the one `channel_lifecycle.rs` owns, so every assertion here
//! goes through the router: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! The rule under test is the one both calls implement: each names the state
//! the ONE channel its `ChannelName` addresses is to be IN — the playing state
//! for a start, the not-playing state for a stop — so the state every later
//! read of that channel reports is the one the most recent of those calls
//! named, whatever order they came in and however often each was repeated;
//! being idempotent, a call naming the state the channel is already in leaves
//! the stored record byte for byte as it was (its last-modified instant
//! included, which says when the channel itself last changed); no other member
//! of that channel and no other channel is touched; and a name the account
//! holds no channel under addresses nothing to start or stop, so it is refused
//! and stores nothing.
//!
//! That refusal is the service's not-found envelope — HTTP 404 under the
//! not-found error type, the fixed channel-miss sentence under the one wire key
//! `message`, and a `requestId` naming the response — and never the 400
//! `BadRequestException` the model declares for a request that carries
//! unexpected data, which is a different claim: a name addressing no channel is
//! a well-formed request for a resource that is simply absent. The last test
//! below pins that envelope over the class of absent names.
//!
//! The tables span the class the rule generalises over: every channel the
//! lifecycle table spans (both playback modes, every tier, one and several
//! outputs in each playlist form, audiences present and absent, time-shift at
//! both published bounds, tag counts from none to several, several forms of
//! name), crossed with sequences that differ in which call comes first, how the
//! two alternate and how many times each is repeated.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{
    encode, table, Emulator, Reply, CHANNEL_NOT_FOUND_MESSAGE, NOT_FOUND_CODE, NOT_FOUND_STATUS,
};
use crate::error_envelope::error_body_members;

/// The two calls under test. `ChannelState`'s modelled values are `RUNNING |
/// STOPPED` (`shared_types::CHANNEL_STATE_VALUES`), and the operations' own
/// routes are `PUT /channel/{ChannelName}/start` and `.../stop`
/// (`smithy_query(query="operation",operation="StartChannel,StopChannel")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Start,
    Stop,
}

impl Call {
    /// The last segment of this call's route.
    fn segment(self) -> &'static str {
        match self {
            Call::Start => "start",
            Call::Stop => "stop",
        }
    }

    /// The `ChannelState` a channel this call succeeded on is in.
    fn state(self) -> &'static str {
        match self {
            Call::Start => "RUNNING",
            Call::Stop => "STOPPED",
        }
    }
}

/// `StartChannel` / `StopChannel`: `ChannelName` is the `@httpLabel` and
/// neither operation carries a request body.
async fn call(emulator: &Emulator, name: &str, which: Call) -> Reply {
    emulator
        .request(
            "PUT",
            &format!("/channel/{}/{}", encode(name), which.segment()),
            None,
        )
        .await
}

/// The members a start or a stop is allowed to move: the state it names, and
/// the instant that records when the channel last changed. Every other member
/// of the stored channel is left as it stood.
const MUTABLE_MEMBERS: &[&str] = &["ChannelState", "LastModifiedTime"];

/// Creates one channel from a lifecycle-table row and reports its read.
async fn create(emulator: &Emulator, case: &str, name: &str, request: &Value) -> Reply {
    let created = emulator.create(name, request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the channel under test to be created, got {} {:?}",
        created.status, created.body
    );
    let described = emulator.describe(name).await;
    assert_eq!(
        described.string("/ChannelState"),
        Call::Stop.state(),
        "case {case:?}: expected a created channel to be in the not-playing state, got {:?}",
        described.get("/ChannelState")
    );
    described
}

/// The call sequences the table crosses every channel with: which call comes
/// first, how the two alternate, and how often each is repeated.
fn sequences() -> Vec<(&'static str, Vec<Call>)> {
    use Call::{Start, Stop};
    vec![
        ("one start", vec![Start]),
        ("a stop on a channel that was never started", vec![Stop]),
        ("start then stop", vec![Start, Stop]),
        (
            "three starts, then two stops",
            vec![Start, Start, Start, Stop, Stop],
        ),
        (
            "the two alternating, beginning with a stop",
            vec![Stop, Start, Stop, Start],
        ),
    ]
}

#[tokio::test]
async fn the_state_a_channel_reports_is_the_one_the_last_call_named() {
    for (channel_case, name, request) in table() {
        for (sequence_case, sequence) in sequences() {
            let case = format!("{channel_case} / {sequence_case}");
            let emulator = Emulator::new();
            let created = create(&emulator, &case, name, &request).await;
            let creation = created.get("/CreationTime").as_f64().expect("a creation instant");

            let mut previous = created.body.clone();
            let mut current_state = Call::Stop.state().to_string();

            for (index, which) in sequence.iter().copied().enumerate() {
                let idempotent = which.state() == current_state;
                let reply = call(&emulator, name, which).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected call {index} ({which:?}) to answer 200, got {} {:?}",
                    reply.status, reply.body
                );
                assert!(
                    reply.body == Value::Null || reply.body == json!({}),
                    "case {case:?}: expected call {index} ({which:?}) to answer an empty \
                     response, got {:?}",
                    reply.body
                );

                let described = emulator.describe(name).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: expected the channel to still be readable after call \
                     {index} ({which:?}), got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    described.string("/ChannelState"),
                    which.state(),
                    "case {case:?}: expected call {index} ({which:?}) to leave the channel in \
                     {:?}, got {:?}",
                    which.state(),
                    described.get("/ChannelState")
                );

                // Only the state and the last-modified instant may move.
                for (member, value) in previous.as_object().expect("a channel read object") {
                    if MUTABLE_MEMBERS.contains(&member.as_str()) {
                        continue;
                    }
                    assert_eq!(
                        described.body.get(member),
                        Some(value),
                        "case {case:?}: expected call {index} ({which:?}) to leave {member} as \
                         it was ({value:?}), got {:?}",
                        described.body.get(member)
                    );
                }

                let modified = described
                    .get("/LastModifiedTime")
                    .as_f64()
                    .expect("a last-modified instant");
                let previously_modified = previous
                    .pointer("/LastModifiedTime")
                    .and_then(Value::as_f64)
                    .expect("the previous read's last-modified instant");
                if idempotent {
                    // The call named the state the channel was already in, so
                    // the channel did not change — nothing about the stored
                    // record did either.
                    assert_eq!(
                        described.body, previous,
                        "case {case:?}: expected call {index} ({which:?}) on a channel already \
                         in {:?} to change nothing, got {:?}",
                        which.state(),
                        described.body
                    );
                } else {
                    // The channel DID change, so the instant that records when
                    // it last changed is this call's, never an earlier one: it
                    // is at or after both the creation instant and the one the
                    // previous read reported.
                    assert!(
                        modified >= creation && modified >= previously_modified,
                        "case {case:?}: expected call {index} ({which:?}) to record a \
                         last-modified instant at or after the creation instant {creation:?} \
                         and the previous one {previously_modified:?}, got {modified:?}"
                    );
                }

                previous = described.body.clone();
                current_state = which.state().to_string();
            }
        }
    }
}

#[tokio::test]
async fn a_call_moves_only_the_channel_it_names() {
    let emulator = Emulator::new();

    // Every channel the lifecycle table spans, all stored side by side.
    for (case, name, request) in table() {
        create(&emulator, case, name, &request).await;
    }

    // Each row is addressed in turn; every other channel's read has to stand
    // exactly as it did before that call.
    for (case, name, _) in table() {
        for which in [Call::Start, Call::Stop] {
            let before: Vec<(String, Value)> = {
                let mut snapshots = Vec::new();
                for (_, other, _) in table() {
                    snapshots.push((
                        other.to_string(),
                        emulator.describe(other).await.body.clone(),
                    ));
                }
                snapshots
            };

            let reply = call(&emulator, name, which).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected {which:?} to answer 200, got {} {:?}",
                reply.status, reply.body
            );

            for (other, snapshot) in before {
                let described = emulator.describe(&other).await;
                if other == name {
                    assert_eq!(
                        described.string("/ChannelState"),
                        which.state(),
                        "case {case:?}: expected {which:?} to leave {other:?} in {:?}, got {:?}",
                        which.state(),
                        described.get("/ChannelState")
                    );
                } else {
                    assert_eq!(
                        described.body, snapshot,
                        "case {case:?}: expected {which:?} on {name:?} to leave {other:?} \
                         untouched, got {:?}",
                        described.body
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_name_the_account_holds_no_channel_under_is_refused() {
    let emulator = Emulator::new();

    // Stored, so the refusals below are about the name and not about an empty
    // store.
    let (_, stored, request) = table().remove(0);
    create(&emulator, "the stored channel", stored, &request).await;

    for name in ["absent", "stored", "loop-MINIMAL", "loop-minimal-2", "a.b_c"] {
        if name == stored {
            continue;
        }
        for which in [Call::Start, Call::Stop] {
            let refused = call(&emulator, name, which).await;
            assert_eq!(
                refused.status, NOT_FOUND_STATUS,
                "case {name:?}/{which:?}: expected a channel the account does not hold to be \
                 refused as not found, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code, NOT_FOUND_CODE,
                "case {name:?}/{which:?}: expected {NOT_FOUND_CODE}, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                CHANNEL_NOT_FOUND_MESSAGE,
                "case {name:?}/{which:?}: expected the channel-miss sentence, got {:?}",
                refused.message()
            );

            // The refused call stored nothing: the name is still free.
            let missing = emulator.describe(name).await;
            assert_eq!(
                missing.status, NOT_FOUND_STATUS,
                "case {name:?}/{which:?}: expected the refused call to store no channel, got \
                 {} {:?}",
                missing.status, missing.body
            );
        }
    }
}

/// The channel the envelope table holds, so every refusal it makes is about the
/// name the request addressed and not about an empty store.
const HELD: &str = "held-channel";

/// The channel the envelope table creates and then deletes, so one row
/// addresses a name the account held until it was released.
const RELEASED: &str = "released-channel";

/// The names the envelope table spans: each addresses no channel, and they
/// differ in HOW they are absent (never used, released by a delete, one
/// character or one case away from the held name) and in the form of the name
/// itself (digits only, punctuation, a character needing percent-encoding,
/// non-ASCII, many times the held name's length) — so what the refusal reports
/// is the absence, and nothing about how the name is spelled.
///
/// None of them occurs in [`CHANNEL_NOT_FOUND_MESSAGE`], so a row can also show
/// that the sentence is fixed text with the requested name nowhere in it.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("a name never used", "never-created".to_string()),
        ("the held name in another case", HELD.to_uppercase()),
        ("the held name with one more character", format!("{HELD}x")),
        ("a prefix of the held name", HELD[.."held".len()].to_string()),
        ("a name of digits only", "12345".to_string()),
        ("a name carrying dots and underscores", "a.b_c-1".to_string()),
        ("a name needing percent-encoding", "two words".to_string()),
        ("a non-ASCII name", "canal-ünïcode".to_string()),
        ("a name many times the held name's length", "n".repeat(120)),
        (
            "a name the account held until it was deleted",
            RELEASED.to_string(),
        ),
    ]
}

/// Assert `reply` is the not-found envelope a channel miss is answered with,
/// and return the `requestId` it named.
fn assert_channel_miss(case: &str, which: Call, name: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, NOT_FOUND_STATUS,
        "case {case:?}: expected {which:?} on {name:?} to answer HTTP {NOT_FOUND_STATUS}, got \
         {} {:?}",
        reply.status, reply.body
    );
    // The error type a client branches on — the service's not-found code, never
    // the model's `BadRequestException`.
    assert_eq!(
        reply.error_code, NOT_FOUND_CODE,
        "case {case:?}: expected {which:?} on {name:?} to answer the error type \
         {NOT_FOUND_CODE:?}, got {:?}",
        reply.error_code
    );

    // The body carries exactly the envelope's members, so the text travels
    // under the member the not-found shape declares and under no second key.
    let members: BTreeSet<String> = reply
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        members,
        error_body_members(crate::channel_lifecycle::SYNTHETIC_ERROR_MESSAGE_MEMBER),
        "case {case:?}: expected {which:?} on {name:?} to carry the error body's members, got \
         {:?}",
        reply.body
    );
    assert!(
        reply.string("/__type").contains(NOT_FOUND_CODE),
        "case {case:?}: expected the body to discriminate on {NOT_FOUND_CODE:?}, got {:?}",
        reply.body
    );

    assert_eq!(
        reply.message(),
        CHANNEL_NOT_FOUND_MESSAGE,
        "case {case:?}: expected {which:?} on {name:?} to read {CHANNEL_NOT_FOUND_MESSAGE:?}, \
         got {:?}",
        reply.body
    );
    // The sentence is fixed text: the requested name is not interpolated.
    assert!(
        !reply.message().contains(name),
        "case {case:?}: expected the sentence to carry no part of {name:?}, got {:?}",
        reply.message()
    );

    let request_id = reply.request_id();
    assert!(
        service_common::query_error_envelope::is_request_id(&request_id),
        "case {case:?}: expected {which:?} on {name:?} to name its request, got {request_id:?} \
         in {:?}",
        reply.body
    );
    request_id
}

#[tokio::test]
async fn a_channel_miss_is_refused_with_the_not_found_envelope() {
    let emulator = Emulator::new();
    let (_, _, request) = table().remove(0);

    // One channel the account holds, and one whose name it holds until the
    // delete releases it.
    create(&emulator, "the held channel", HELD, &request).await;
    let held_before = emulator.describe(HELD).await.body.clone();
    create(&emulator, "the released channel", RELEASED, &request).await;
    assert_eq!(
        emulator.delete(RELEASED).await.status,
        200,
        "the channel whose name is released must be deleted"
    );

    let mut request_ids: Vec<String> = Vec::new();
    for (row, name) in absent_names() {
        for which in [Call::Start, Call::Stop] {
            // Retried: the refusal's text is a property of the refusal, so it
            // repeats byte for byte, while the id names ONE response.
            for attempt in 1..=2 {
                let case = format!("{row} / {which:?} / attempt {attempt}");
                let reply = call(&emulator, &name, which).await;
                request_ids.push(assert_channel_miss(&case, which, &name, &reply));
            }

            let case = format!("{row} / {which:?}");
            // The refused call stored nothing: the name is still free...
            let missing = emulator.describe(&name).await;
            assert_eq!(
                missing.status, NOT_FOUND_STATUS,
                "case {case:?}: expected the refused call to store no channel, got {} {:?}",
                missing.status, missing.body
            );
            // ...and the channel the account does hold did not move.
            let held_now = emulator.describe(HELD).await;
            assert_eq!(
                held_now.body, held_before,
                "case {case:?}: expected the held channel to stand as it was, got {:?}",
                held_now.body
            );
        }
    }

    // One id per response: no two of these refusals named the same request.
    let distinct: BTreeSet<&String> = request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "expected every refusal to name its own request, got {request_ids:?}"
    );

    // The 404 answers the ABSENCE, not the route: both calls are served for the
    // name the account does hold.
    for which in [Call::Start, Call::Stop] {
        let served = call(&emulator, HELD, which).await;
        assert_eq!(
            served.status, 200,
            "case {which:?}: expected the held channel to be served, got {} {:?}",
            served.status, served.body
        );
        assert_eq!(
            emulator.describe(HELD).await.string("/ChannelState"),
            which.state(),
            "case {which:?}: expected the held channel to reach {:?}",
            which.state()
        );
    }
}
