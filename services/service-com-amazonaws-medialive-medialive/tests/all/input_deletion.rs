// SPDX-License-Identifier: CC0-1.0

//! `DeleteInput` — the deletion half of the MediaLive input lifecycle, end to
//! end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `URL_PULL`-attached-to-one-channel case
//! is ONE row of each. The wire harness (`medialive`, `call`, `error_code`) is
//! the crate's shared one in `wire`, and the clock wait is the channel suite's —
//! reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::resolved_input_body;
use crate::wire::{call, error_code, error_message, medialive};

/// The ids an input configuration names are resolved first, so every create in
/// this suite is one the service can ingest through (`crate::support`).
async fn create_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputs/{id}"), None).await
}

async fn delete_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/prod/inputs/{id}"), None).await
}

/// The `State` an input reports right now, read back through `DescribeInput`.
async fn state_of(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, described) = describe_input(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput({id}): expected 200, got {status}: {described}"
    );
    described["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// One row of the deletion table: an input of some shape, attached by some
/// number of channels through some number of attachments each.
struct Row {
    case: &'static str,
    /// The `CreateInput` body for the input under test.
    input: fn() -> Value,
    /// How many channels attach it.
    channels: usize,
    /// How many `InputAttachments` entries each of those channels gives it.
    attachments_per_channel: usize,
}

/// Rule: `DeleteInput` refuses with the modelled `ConflictException` (409) and
/// changes nothing while any channel's `InputAttachments` names the input;
/// otherwise it deletes it — the input enters `DELETING`, the clock settles it
/// into the terminal `DELETED`, and `DescribeInput` reports that state. A
/// repeated delete is idempotent and no other input is touched.
///
/// The rows differ along the SHAPE of the input (which per-pipeline list its
/// `InputType` family ingests through) and along whether a channel attaches it
/// and through how many attachments — which is what the rule generalises over —
/// because the rule is about whether ANY channel attaches the input, not about
/// any one input type or any one attachment. No row attaches one input to two
/// channels: an input is attached to at most one channel at a time, so the
/// second `CreateChannel` would be refused with its own conflict.
#[tokio::test]
async fn deleting_an_input_is_refused_while_a_channel_attaches_it() {
    let table: [Row; 4] = [
        Row {
            case: "url pull attached to one channel",
            input: || {
                json!({
                    "name": "pull-attached",
                    "type": "URL_PULL",
                    "sources": [source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
                })
            },
            channels: 1,
            attachments_per_channel: 1,
        },
        Row {
            case: "rtmp push attached to one channel",
            input: || {
                json!({
                    "name": "push-attached",
                    "type": "RTMP_PUSH",
                    "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
                })
            },
            channels: 1,
            attachments_per_channel: 1,
        },
        Row {
            case: "mediaconnect attached twice by one channel",
            input: || {
                json!({
                    "name": "flow-attached",
                    "type": "MEDIACONNECT",
                    "mediaConnectFlows": [
                        { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                    ],
                    "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
                })
            },
            channels: 1,
            attachments_per_channel: 2,
        },
        Row {
            // No channel attaches it, so there is nothing to refuse: the delete
            // is accepted on the first call.
            case: "sdi input attached to no channel",
            input: || {
                json!({
                    "name": "sdi-detached",
                    "type": "SDI",
                    "sdiSources": ["sdi-a", "sdi-b"],
                    "inputNetworkLocation": "ON_PREMISES",
                })
            },
            channels: 0,
            attachments_per_channel: 0,
        },
    ];

    for Row {
        case,
        input,
        channels,
        attachments_per_channel,
    } in table
    {
        let router = medialive();

        // A second input nothing deletes, so the suite can tell a deletion from
        // a store-wide change.
        let bystander = create_input(
            &router,
            case,
            json!({
                "name": "bystander",
                "type": "URL_PULL",
                "sources": [source("https://example.com/bystander.m3u8")],
            }),
        )
        .await;
        let bystander_id = bystander["id"].as_str().unwrap_or_default().to_string();

        let created = create_input(&router, case, input()).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let mut channel_ids: Vec<String> = Vec::new();
        for index in 0..channels {
            let attachments: Vec<Value> = (0..attachments_per_channel)
                .map(|slot| {
                    json!({ "inputId": id, "inputAttachmentName": format!("slot-{slot}") })
                })
                .collect();
            let (status, channel) = call(
                &router,
                "POST",
                "/prod/channels",
                Some(json!({
                    "name": format!("attaching-{index}"),
                    "inputAttachments": attachments,
                    "encoderSettings": crate::support::channel_encoder_settings(),
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateChannel: expected 201, got {status}: {channel}"
            );
            channel_ids.push(
                channel["channel"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        channel_ids.sort();

        if channels > 0 {
            let (_, before) = describe_input(&router, &id).await;
            assert_eq!(
                before["state"],
                json!("ATTACHED"),
                "case {case:?}: expected the attached input to be ATTACHED, got {:?}",
                before["state"]
            );

            let (status, refusal) = delete_input(&router, &id).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected deleting a busy input to be refused with 409, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "ConflictException",
                "case {case:?}: expected the operation's modelled ConflictException, got {refusal}"
            );
            assert!(
                error_message(&refusal).contains(&id),
                "case {case:?}: expected the refusal to name the input it refused, got {:?}",
                error_message(&refusal)
            );

            // The refusal had no effect: the input reads back exactly as it did.
            let (status, after) = describe_input(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the refused input to stay readable, got {status}: {after}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected a refused delete to change nothing, got {after}"
            );
            assert_eq!(
                after["attachedChannels"],
                json!(channel_ids),
                "case {case:?}: expected the attaching channels, got {:?}",
                after["attachedChannels"]
            );
        }

        // Detach by deleting the channels: a channel is deletable once it has
        // settled into IDLE, and its removal is due a span after that.
        if channels > 0 {
            wait_for_the_clock();
            for channel_id in &channel_ids {
                let (status, deleting) =
                    call(&router, "DELETE", &format!("/prod/channels/{channel_id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteChannel({channel_id}): expected 200, got {status}: {deleting}"
                );
            }
            wait_for_the_clock();
            assert_eq!(
                state_of(&router, case, &id).await,
                "DETACHED",
                "case {case:?}: expected the input to be DETACHED once its channels are gone"
            );
        }

        let (status, deleted) = delete_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected deleting a detached input to succeed with 200, got {status}: {deleted}"
        );

        // The deletion is asynchronous, so a caller acting straight after the
        // call still sees the transient state.
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETING",
            "case {case:?}: expected the input to be DELETING before the clock makes the deletion due"
        );

        wait_for_the_clock();
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETED",
            "case {case:?}: expected the deletion to have settled into the terminal DELETED"
        );

        // Idempotent: deleting a deleted input neither fails nor revives it.
        let (status, again) = delete_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated delete to succeed, got {status}: {again}"
        );
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETED",
            "case {case:?}: expected a repeated delete to leave the input DELETED"
        );

        assert_eq!(
            state_of(&router, case, &bystander_id).await,
            "DETACHED",
            "case {case:?}: expected the input nothing deleted to be untouched"
        );
    }
}

/// Rule: `DeleteInput` on an id no input has answers the modelled
/// `NotFoundException` (404), with the message this service composes from the
/// request — the resource type, the identifier, then `not found`.
///
/// The identifiers differ in length and character class, because the rule is
/// about resolution rather than about any one id shape.
#[tokio::test]
async fn deleting_an_unknown_input_is_not_found() {
    let router = medialive();
    // One input exists, so the refusals are about the id and not about an empty
    // store.
    let created = create_input(
        &router,
        "existing",
        json!({
            "name": "existing",
            "type": "URL_PULL",
            "sources": [source("https://example.com/a.m3u8")],
        }),
    )
    .await;
    let existing = created["id"].as_str().unwrap_or_default().to_string();

    for id in ["0000000", "999999999", "1000001", "no-such-input"] {
        let (status, body) = delete_input(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 for an unknown input, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {id:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Input {id} not found"),
            "case {id:?}: expected the message to name the resource type then the identifier, got {:?}",
            error_message(&body)
        );
    }

    assert_eq!(
        state_of(&router, "existing", &existing).await,
        "DETACHED",
        "expected the refused deletes to leave the existing input alone"
    );
}

/// Rule: the resources an input references report what is CURRENTLY using them
/// (`InputSecurityGroup$Inputs` is "The list of inputs currently using this
/// Input Security Group"; `SdiSource$Inputs` is "The list of inputs that are
/// currently using this SDI source"), so an input stops being one of their
/// users as soon as its delete is ACCEPTED — the transient `DELETING` step
/// included, because the delete is what releases what the input held — and a
/// resource left with no user at all goes back to `IDLE`.
///
/// The rows span the two resource kinds an input references by id, because the
/// rule is about deletion ending a reference, not about either one of them, and
/// each is read both inside the `DELETING` window and after the clock has
/// settled the input into `DELETED`.
#[tokio::test]
async fn a_deleted_input_stops_using_the_resources_it_referenced() {
    let router = medialive();

    let (status, group) = call(
        &router,
        "POST",
        "/prod/inputSecurityGroups",
        Some(json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInputSecurityGroup: got {status}: {group}"
    );
    let group_id = group["securityGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, sdi) = call(
        &router,
        "POST",
        "/prod/sdiSources",
        Some(json!({ "name": "wired", "type": "SINGLE" })),
    )
    .await;
    // `201 Created` is the recorded status of a MediaLive create (see
    // `CreateSdiSource`'s `SUCCESS_STATUS`).
    assert_eq!(status, 201, "CreateSdiSource: got {status}: {sdi}");
    let sdi_id = sdi["sdiSource"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let created = create_input(
        &router,
        "referencing input",
        json!({
            "name": "referencing",
            "type": "SDI",
            "sdiSources": [sdi_id.clone()],
            "inputSecurityGroups": [group_id.clone()],
        }),
    )
    .await;
    let input_id = created["id"].as_str().unwrap_or_default().to_string();

    // (case, the read's URI, the output member the resource is wrapped in —
    // `DescribeInputSecurityGroup`'s output is flat, `DescribeSdiSource`'s wraps
    // its resource, per `smithy_query(query="operation", …)`)
    let referenced: [(&str, String, Option<&str>); 2] = [
        (
            "input security group",
            format!("/prod/inputSecurityGroups/{group_id}"),
            None,
        ),
        (
            "sdi source",
            format!("/prod/sdiSources/{sdi_id}"),
            Some("sdiSource"),
        ),
    ];

    for (case, uri, member) in &referenced {
        let (status, read) = call(&router, "GET", uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {read}");
        let body = member.map_or_else(|| read.clone(), |name| read[name].clone());
        assert_eq!(
            body["state"],
            json!("IN_USE"),
            "case {case:?}: expected IN_USE while an input references it, got {:?}",
            body["state"]
        );
        assert_eq!(
            body["inputs"],
            json!([input_id.clone()]),
            "case {case:?}: expected the referencing input to be listed, got {:?}",
            body["inputs"]
        );
    }

    let (status, deleted) = delete_input(&router, &input_id).await;
    assert_eq!(status, 200, "DeleteInput: got {status}: {deleted}");

    // The reference ends when the delete is ACCEPTED, not one transient span
    // later: the input is already being torn down, so it is no longer a current
    // user of either resource and both are back to IDLE.
    assert_eq!(
        state_of(&router, "referencing input", &input_id).await,
        "DELETING",
        "expected the input to be in the transient state the delete starts"
    );
    for (case, uri, member) in &referenced {
        let (status, read) = call(&router, "GET", uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {read}");
        let body = member.map_or_else(|| read.clone(), |name| read[name].clone());
        assert_eq!(
            body["state"],
            json!("IDLE"),
            "case {case:?}: expected IDLE once the input's delete was accepted, got {:?}",
            body["state"]
        );
        assert_eq!(
            body["inputs"],
            json!([]),
            "case {case:?}: expected an input whose delete was accepted to stop being a current \
             user, got {:?}",
            body["inputs"]
        );
    }

    wait_for_the_clock();
    assert_eq!(
        state_of(&router, "referencing input", &input_id).await,
        "DELETED",
        "expected the input's deletion to have settled"
    );

    for (case, uri, member) in &referenced {
        let (status, read) = call(&router, "GET", uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {read}");
        let body = member.map_or_else(|| read.clone(), |name| read[name].clone());
        assert_eq!(
            body["state"],
            json!("IDLE"),
            "case {case:?}: expected IDLE once nothing uses it any more, got {:?}",
            body["state"]
        );
        assert_eq!(
            body["inputs"],
            json!([]),
            "case {case:?}: expected a deleted input to stay out of the current users, got {:?}",
            body["inputs"]
        );
    }
}

/// Rule: `DeleteInput` answers with the deleted input's own representation — the
/// flat projection `DescribeInputResponse` models, carrying the `State` the
/// delete just put the input in — not an empty body, so a delete-then-wait flow
/// reads its starting state straight off the delete.
///
/// The rows differ along what the rule generalises over: the SHAPE of the input
/// being deleted (which per-pipeline list its `InputType` family ingests
/// through, how many entries it has, which optional members it carries, whether
/// it has tags or a security group), because the rule is about projecting
/// whatever input was deleted rather than about any one input type. Each row is
/// checked member by member against what `DescribeInput` reported for the same
/// input immediately before the delete — identical but for `State`.
#[tokio::test]
async fn deleting_an_input_answers_with_the_deleted_input() {
    // Every member `DescribeInputResponse` declares
    // (`smithy_query(query="operation", operation="DescribeInput")`), as the
    // wire names them; the ones a row does not set are reported empty or absent,
    // which is itself part of the projection.
    const ALWAYS_REPORTED: [&str; 15] = [
        "arn",
        "id",
        "name",
        "type",
        "state",
        "securityGroups",
        "destinations",
        "inputClass",
        "inputNetworkLocation",
        "inputSourceType",
        "sources",
        "mediaConnectFlows",
        "inputDevices",
        "attachedChannels",
        "inputPartnerIds",
    ];

    // (case, the CreateInput body)
    let table: [(&str, Value); 5] = [
        (
            "rtmp push with two destinations, a security group and tags",
            json!({
                "name": "push-deleted",
                "type": "RTMP_PUSH",
                "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
                "tags": { "env": "test" },
            }),
        ),
        (
            "url pull with one source",
            json!({
                "name": "pull-deleted",
                "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8")],
            }),
        ),
        (
            "mediaconnect with a flow and a role",
            json!({
                "name": "flow-deleted",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
        ),
        (
            "on-premises sdi input",
            json!({
                "name": "sdi-deleted",
                "type": "SDI",
                "sdiSources": ["sdi-a", "sdi-b"],
                "inputNetworkLocation": "ON_PREMISES",
            }),
        ),
        (
            "an input carrying nothing but a name and a type",
            json!({ "name": "bare", "type": "URL_PULL" }),
        ),
    ];

    for (case, body) in table {
        let router = medialive();
        let created = create_input(&router, case, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        // What the reader says about the input one instant before the delete.
        let (status, before) = describe_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput: expected 200, got {status}: {before}"
        );
        assert_eq!(
            before["state"],
            json!("DETACHED"),
            "case {case:?}: expected a detached input before the delete, got {:?}",
            before["state"]
        );

        let (status, deleted) = delete_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteInput: expected 200, got {status}: {deleted}"
        );

        // The body is the input, not an empty object.
        assert!(
            deleted.as_object().is_some_and(|members| !members.is_empty()),
            "case {case:?}: expected the deleted input's representation, got {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETING"),
            "case {case:?}: expected the delete to report the state it just entered, got {:?}",
            deleted["state"]
        );
        for member in ALWAYS_REPORTED {
            assert!(
                !deleted[member].is_null(),
                "case {case:?}: expected the delete to report {member}, got {deleted}"
            );
            if member != "state" {
                assert_eq!(
                    deleted[member], before[member],
                    "case {case:?}: expected the delete's {member} to be the input's own {:?}, got {:?}",
                    before[member], deleted[member]
                );
            }
        }
        // Every member the reader reports, the delete reports identically —
        // `State` excepted, which the delete moved.
        for (member, value) in before.as_object().expect("the describe body is an object") {
            if member == "state" {
                continue;
            }
            assert_eq!(
                &deleted[member], value,
                "case {case:?}: expected the delete to report the same {member} as the reader, got {:?}",
                deleted[member]
            );
        }

        // The body was not assembled out of thin air: the reader agrees the input
        // is DELETING, and once the clock has run both the reader and a repeated
        // delete report the terminal DELETED.
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETING",
            "case {case:?}: expected the reader to agree the input is DELETING"
        );
        wait_for_the_clock();
        let (status, again) = delete_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated delete to succeed, got {status}: {again}"
        );
        assert_eq!(
            again["state"],
            json!("DELETED"),
            "case {case:?}: expected a repeated delete to report the terminal DELETED, got {:?}",
            again["state"]
        );
        assert_eq!(
            again["id"], deleted["id"],
            "case {case:?}: expected a repeated delete to report the same input, got {:?}",
            again["id"]
        );
    }
}

/// Rule: a refused `DeleteInput` answers with its modelled error and no input
/// representation at all — the body of a success is the deleted input, so a
/// refusal must not carry one.
#[tokio::test]
async fn a_refused_delete_carries_no_input() {
    let router = medialive();
    let created = create_input(
        &router,
        "attached",
        json!({
            "name": "attached",
            "type": "URL_PULL",
            "sources": [source("https://example.com/a.m3u8")],
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "attaching",
            "inputAttachments": [{ "inputId": id, "inputAttachmentName": "slot-0" }],
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");

    // (case, the id to delete, the expected status and error code)
    let table: [(&str, &str, u16, &str); 2] = [
        ("a channel attaches it", id.as_str(), 409, "ConflictException"),
        ("no input has that id", "9999999", 404, "NotFoundException"),
    ];

    for (case, target, expected_status, expected_code) in table {
        let (status, body) = delete_input(&router, target).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        for member in ["id", "arn", "state"] {
            assert!(
                body[member].is_null(),
                "case {case:?}: expected a refusal to carry no {member}, got {body}"
            );
        }
    }
}
