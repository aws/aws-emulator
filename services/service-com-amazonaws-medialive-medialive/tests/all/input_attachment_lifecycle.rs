// SPDX-License-Identifier: CC0-1.0

//! The MediaLive attachment lifecycle — who holds whom, and what that makes
//! impossible — end to end over the restJson1 wire.
//!
//! Three rules, each asserted over a table that spans what it generalises over:
//!
//! * an input is attached to at most ONE channel at a time, so a channel write
//!   whose `InputAttachments` names an input another channel already attaches is
//!   refused with the operation's modelled `ConflictException` (409) carrying
//!   `inputId=<id> is already attached`, and nothing is written;
//! * an input security group is not deleted while inputs still use it
//!   (`InputSecurityGroup$Inputs` is "The list of inputs currently using this
//!   Input Security Group"), so `DeleteInputSecurityGroup` is refused with 409
//!   while that list is not empty;
//! * deleting the channel that holds an input DETACHES it, so the ordinary
//!   teardown order — channel, then input, then security group — completes: the
//!   input's delete is accepted with `DELETING` and an empty `AttachedChannels`,
//!   the clock settles it into the terminal `DELETED` with its ingest endpoints
//!   released, and the group that admitted it goes back to `IDLE` and can then
//!   be deleted.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//! A state the clock owes is waited for by POLLING the read that reports it,
//! never by sleeping for a span.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    input_already_attached_message, input_security_group_in_use_message,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{channel_encoder_settings, resolved_input_body};
use crate::wire::{call, error_code, error_message, medialive};

/// How long a poll waits for a state the clock owes before giving up. Generous
/// on purpose: the span itself is the service's, and nothing here asserts on how
/// fast the host runs.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `CreateInput`, with the resources its configuration names put in the account
/// first (`crate::support`), answering the created input.
async fn create_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// `CreateChannel` with `body`, answering the status and the response.
async fn create_channel(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/prod/channels", Some(body)).await
}

/// `DescribeInput` on `id`, which must answer 200.
async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput({id}): expected 200, got {status}: {described}"
    );
    described
}

/// `DescribeInputSecurityGroup` on `id`, which must answer 200.
async fn describe_group(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(
        router,
        "GET",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInputSecurityGroup({id}): expected 200, got {status}: {described}"
    );
    described
}

/// Every channel this account holds, as `ListChannels` reports them.
async fn listed_channels(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListChannels: expected 200, got {status}: {body}"
    );
    body["channels"].as_array().cloned().unwrap_or_default()
}

/// The input `id`, read back once its `State` is `expected`.
///
/// A poll, not a sleep: the state is one the clock owes, so the test looks again
/// until the condition holds or [`POLL_DEADLINE`] passes.
async fn input_in_state(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: &str,
) -> Value {
    let started = Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < POLL_DEADLINE {
        last = describe_input(router, case, id).await;
        if last["state"] == json!(expected) {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: input {id} never reached {expected:?}, last read {last}");
}

/// The channel `id`, read back once its `State` is `expected`, for the same
/// reason as [`input_in_state`].
async fn channel_in_state(router: &MultiServiceRouter, case: &str, id: &str, expected: &str) {
    let started = Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < POLL_DEADLINE {
        let (status, described) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel({id}): expected 200, got {status}: {described}"
        );
        if described["state"] == json!(expected) {
            return;
        }
        last = described;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: channel {id} never reached {expected:?}, last read {last}");
}

/// One `InputAttachments` entry naming `input_id`.
fn attachment(name: &str, input_id: &str) -> Value {
    json!({ "inputAttachmentName": name, "inputId": input_id })
}

/// The `CreateInput` body of an input of `input_type`, with the per-type ingest
/// list that type needs. The types span the ingest families
/// (`smithy_query(query="shape", shape="InputType")`): pulled-from source URLs,
/// pushed-to destinations, and a MediaConnect flow.
fn input_body(name: &str, input_type: &str) -> Value {
    let mut body = json!({ "name": name, "type": input_type });
    match input_type {
        "RTMP_PUSH" => {
            body["destinations"] = json!([{ "streamName": "live/a" }, { "streamName": "live/b" }]);
        }
        "MEDIACONNECT" => {
            body["mediaConnectFlows"] = json!([
                { "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1:a" },
            ]);
            body["roleArn"] = json!("arn:aws:iam::000000000000:role/MediaLiveAccessRole");
        }
        // Every other type in this suite is a pulled-from one, whose redundant
        // pair of source URLs `crate::support` supplies.
        _ => {}
    }
    body
}

/// One row of the double-booking table: the input's type, how many attachments
/// the holding channel gives it, and where in the second channel's attachment
/// list the held input sits.
struct DoubleBooking {
    case: &'static str,
    input_type: &'static str,
    /// How many `InputAttachments` entries the FIRST channel names the input in.
    held_by: usize,
    /// The position the SECOND channel's list names the held input at — the
    /// entries before it name a free input of the same account.
    position: usize,
    /// Whether the holding channel is left in the transient state its create
    /// started it in, or polled into `IDLE` first.
    settle_holder: bool,
}

/// Rule: an input is attached to at most one channel at a time, so
/// `CreateChannel` refuses a request whose `InputAttachments` names an input
/// another channel already attaches — 409 `ConflictException` carrying
/// `inputId=<id> is already attached` — and creates no channel.
///
/// The rows differ along what the rule generalises over: the input's ingest
/// family, how many attachments the holder gives it, where in the refused
/// request's list the held input sits, and whether the holder has settled out of
/// the state its create started it in.
#[tokio::test]
async fn attaching_an_input_another_channel_holds_is_refused() {
    let table: [DoubleBooking; 5] = [
        DoubleBooking {
            case: "url pull held by one attachment",
            input_type: "URL_PULL",
            held_by: 1,
            position: 0,
            settle_holder: true,
        },
        DoubleBooking {
            case: "rtmp push held by one attachment",
            input_type: "RTMP_PUSH",
            held_by: 1,
            position: 0,
            settle_holder: true,
        },
        DoubleBooking {
            case: "mediaconnect held twice by its channel",
            input_type: "MEDIACONNECT",
            held_by: 2,
            position: 0,
            settle_holder: true,
        },
        DoubleBooking {
            case: "the held input behind a free one",
            input_type: "URL_PULL",
            held_by: 1,
            position: 1,
            settle_holder: true,
        },
        DoubleBooking {
            case: "the holder still in its transient state",
            input_type: "TS_FILE",
            held_by: 1,
            position: 0,
            settle_holder: false,
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();
        let held = create_input(&router, case, input_body("held", row.input_type)).await;
        let held_id = held["id"].as_str().unwrap_or_default().to_string();

        let holder_attachments: Vec<Value> = (0..row.held_by)
            .map(|slot| attachment(&format!("slot-{slot}"), &held_id))
            .collect();
        let (status, holder) = create_channel(
            &router,
            json!({
                "name": "holder",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": holder_attachments,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannel \"holder\": expected 201, got {status}: {holder}"
        );
        let holder_id = holder["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if row.settle_holder {
            channel_in_state(&router, case, &holder_id, "IDLE").await;
        }

        // The second request names free inputs of this account up to the
        // position the held one sits at, so the refusal is about the held entry
        // and not about the list's shape.
        let mut attachments: Vec<Value> = Vec::new();
        for slot in 0..row.position {
            let free = create_input(
                &router,
                case,
                input_body(&format!("free-{slot}"), "URL_PULL"),
            )
            .await;
            attachments.push(attachment(
                &format!("free-{slot}"),
                free["id"].as_str().unwrap_or_default(),
            ));
        }
        attachments.push(attachment("double-booked", &held_id));

        let before = listed_channels(&router, case).await.len();
        let (status, refusal) = create_channel(
            &router,
            json!({
                "name": "second",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": attachments,
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the double booking to be refused with 409, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            input_already_attached_message(&held_id),
            "case {case:?}: expected the message {:?}, got {:?}",
            input_already_attached_message(&held_id),
            error_message(&refusal)
        );

        // Nothing was written for the refusal: no channel was created and the
        // input is still held by exactly the one channel that held it.
        let after = listed_channels(&router, case).await;
        assert_eq!(
            after.len(),
            before,
            "case {case:?}: expected the refused create to store nothing, got {after:?}"
        );
        let read = describe_input(&router, case, &held_id).await;
        assert_eq!(
            read["attachedChannels"],
            json!([holder_id]),
            "case {case:?}: expected the input to stay attached to its one channel, got {:?}",
            read["attachedChannels"]
        );
        assert_eq!(
            read["state"],
            json!("ATTACHED"),
            "case {case:?}: expected the input to stay ATTACHED, got {:?}",
            read["state"]
        );
    }
}

/// Rule (the same rule, through the other write): `UpdateChannel` refuses an
/// attachment list that would attach an input another channel holds, while a
/// list that re-declares the channel's OWN attachment is accepted — the
/// conflict is about a SECOND holder, not about the input being attached.
///
/// The rows span both sides of that distinction and both ingest families.
#[tokio::test]
async fn updating_a_channel_to_attach_a_held_input_is_refused() {
    // (case, the input type, whether the update names the OTHER channel's input)
    let table: [(&str, &str, bool); 4] = [
        ("url pull held by another channel", "URL_PULL", true),
        ("rtmp push held by another channel", "RTMP_PUSH", true),
        ("its own attachment restated", "URL_PULL", false),
        ("its own attachment restated, pushed-to", "RTMP_PUSH", false),
    ];

    for (case, input_type, names_other) in table {
        let router = medialive();
        let held = create_input(&router, case, input_body("held", input_type)).await;
        let held_id = held["id"].as_str().unwrap_or_default().to_string();
        let own = create_input(&router, case, input_body("own", "URL_PULL")).await;
        let own_id = own["id"].as_str().unwrap_or_default().to_string();

        let (status, holder) = create_channel(
            &router,
            json!({
                "name": "holder",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [attachment("held", &held_id)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannel \"holder\": expected 201, got {status}: {holder}"
        );
        let (status, edited) = create_channel(
            &router,
            json!({
                "name": "edited",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [attachment("own", &own_id)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannel \"edited\": expected 201, got {status}: {edited}"
        );
        let edited_id = edited["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // An update is accepted from a channel that has stopped.
        channel_in_state(&router, case, &edited_id, "IDLE").await;

        let named = if names_other { &held_id } else { &own_id };
        let (status, response) = call(
            &router,
            "PUT",
            &format!("/prod/channels/{edited_id}"),
            Some(json!({ "inputAttachments": [attachment("named", named)] })),
        )
        .await;

        if names_other {
            assert_eq!(
                status, 409,
                "case {case:?}: expected the update to be refused with 409, got {status}: {response}"
            );
            assert_eq!(
                error_code(status, &response),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                input_already_attached_message(&held_id),
                "case {case:?}: expected the message {:?}, got {:?}",
                input_already_attached_message(&held_id),
                error_message(&response)
            );
            let read = describe_input(&router, case, &own_id).await;
            assert_eq!(
                read["attachedChannels"],
                json!([edited_id]),
                "case {case:?}: expected the refused update to leave the attachments alone, got {:?}",
                read["attachedChannels"]
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {response}"
            );
            let read = describe_input(&router, case, &own_id).await;
            assert_eq!(
                read["attachedChannels"],
                json!([edited_id]),
                "case {case:?}: expected the input to stay attached to its channel, got {:?}",
                read["attachedChannels"]
            );
        }
    }
}

/// Rule: an input freed by the delete of the channel that held it can be
/// attached again — the attachment conflict is about the channels CURRENTLY
/// attaching the input, so a channel whose delete this service has accepted
/// holds nothing.
///
/// The rows span the ingest families and the point in the former holder's
/// deletion the second create is sent at: inside the transient `DELETING`
/// window, and after the clock has settled it into the terminal `DELETED`.
#[tokio::test]
async fn an_input_freed_by_its_channel_delete_can_be_attached_again() {
    // (case, the input type, whether to wait for the holder's DELETED)
    let table: [(&str, &str, bool); 4] = [
        ("url pull, holder still DELETING", "URL_PULL", false),
        ("url pull, holder DELETED", "URL_PULL", true),
        ("rtmp push, holder still DELETING", "RTMP_PUSH", false),
        ("mediaconnect, holder DELETED", "MEDIACONNECT", true),
    ];

    for (case, input_type, settle) in table {
        let router = medialive();
        let input = create_input(&router, case, input_body("feed", input_type)).await;
        let input_id = input["id"].as_str().unwrap_or_default().to_string();

        let (status, holder) = create_channel(
            &router,
            json!({
                "name": "holder",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [attachment("feed", &input_id)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannel \"holder\": expected 201, got {status}: {holder}"
        );
        let holder_id = holder["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        channel_in_state(&router, case, &holder_id, "IDLE").await;

        let (status, deleting) =
            call(&router, "DELETE", &format!("/prod/channels/{holder_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: expected 200, got {status}: {deleting}"
        );
        if settle {
            channel_in_state(&router, case, &holder_id, "DELETED").await;
        }

        // The input is free the instant the delete is accepted, so it reports no
        // attached channel and is DETACHED again.
        let read = describe_input(&router, case, &input_id).await;
        assert_eq!(
            read["attachedChannels"],
            json!([]),
            "case {case:?}: expected the deleted channel to hold the input no more, got {:?}",
            read["attachedChannels"]
        );
        assert_eq!(
            read["state"],
            json!("DETACHED"),
            "case {case:?}: expected the freed input to be DETACHED, got {:?}",
            read["state"]
        );

        let (status, second) = create_channel(
            &router,
            json!({
                "name": "second",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [attachment("feed", &input_id)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the freed input to be attachable again, got {status}: {second}"
        );
        let second_id = second["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = describe_input(&router, case, &input_id).await;
        assert_eq!(
            read["attachedChannels"],
            json!([second_id]),
            "case {case:?}: expected the input to be held by the new channel alone, got {:?}",
            read["attachedChannels"]
        );
    }
}

/// Rule: a repeated `CreateChannel` carrying the same `RequestId` (the
/// operation's `@idempotencyToken`) returns the channel the first call created
/// rather than minting another, so the input it attaches reports exactly one
/// `AttachedChannels` entry however often the create is retried.
///
/// The rows span the retry counts and the attachment shapes the rule
/// generalises over.
#[tokio::test]
async fn a_retried_create_attaches_its_input_once() {
    // (case, how many times the create is sent in all, attachments per channel)
    let table: [(&str, usize, usize); 3] = [
        ("sent twice", 2, 1),
        ("sent four times", 4, 1),
        ("sent twice, two attachments on one input", 2, 2),
    ];

    for (case, sends, attachments_per_channel) in table {
        let router = medialive();
        let input = create_input(&router, case, input_body("feed", "URL_PULL")).await;
        let input_id = input["id"].as_str().unwrap_or_default().to_string();
        let attachments: Vec<Value> = (0..attachments_per_channel)
            .map(|slot| attachment(&format!("slot-{slot}"), &input_id))
            .collect();

        let mut minted: Vec<String> = Vec::new();
        for send in 0..sends {
            let (status, created) = create_channel(
                &router,
                json!({
                    "name": "retried",
                    "requestId": "a-token-the-client-chose",
                    "encoderSettings": channel_encoder_settings(),
                    "inputAttachments": attachments.clone(),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: send {send}: expected 201, got {status}: {created}"
            );
            minted.push(
                created["channel"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        let first = minted.first().cloned().unwrap_or_default();
        assert!(
            minted.iter().all(|id| *id == first),
            "case {case:?}: expected every retry to answer the channel {first:?}, got {minted:?}"
        );
        assert_eq!(
            listed_channels(&router, case).await.len(),
            1,
            "case {case:?}: expected the retries to have created one channel in all"
        );
        let read = describe_input(&router, case, &input_id).await;
        assert_eq!(
            read["attachedChannels"],
            json!([first]),
            "case {case:?}: expected one attached channel, got {:?}",
            read["attachedChannels"]
        );
    }
}

/// Rule: an input security group is not deleted while inputs still use it, so
/// `DeleteInputSecurityGroup` is refused with 409 `ConflictException` carrying
/// "Input Security Group <id> cannot be deleted because it is still being used
/// by inputs." and the group is left exactly as it was; once those inputs'
/// deletes are accepted the group has no current user and the same delete is
/// accepted.
///
/// The rows differ along what the rule generalises over: how many inputs use the
/// group and which ingest family they belong to (a pushed-to input is REACHED
/// through the whitelist, a pulled-from one merely references it).
#[tokio::test]
async fn deleting_a_group_inputs_still_use_is_refused() {
    // (case, the types of the inputs that use the group)
    let table: [(&str, &[&str]); 4] = [
        ("one pushed-to input", &["RTMP_PUSH"]),
        ("one pulled-from input", &["URL_PULL"]),
        ("two inputs of one family", &["URL_PULL", "TS_FILE"]),
        ("two inputs of both families", &["RTMP_PUSH", "URL_PULL"]),
    ];

    for (case, types) in table {
        let router = medialive();
        let group = crate::support::input_security_group(&router).await;

        let mut input_ids: Vec<String> = Vec::new();
        for (index, input_type) in types.iter().enumerate() {
            let mut body = input_body(&format!("user-{index}"), input_type);
            body["inputSecurityGroups"] = json!([group.clone()]);
            let created = create_input(&router, case, body).await;
            input_ids.push(created["id"].as_str().unwrap_or_default().to_string());
        }
        input_ids.sort();

        let before = describe_group(&router, case, &group).await;
        assert_eq!(
            before["inputs"],
            json!(input_ids),
            "case {case:?}: expected the group to report its users, got {:?}",
            before["inputs"]
        );

        let (status, refusal) = call(
            &router,
            "DELETE",
            &format!("/prod/inputSecurityGroups/{group}"),
            None,
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete to be refused with 409, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            input_security_group_in_use_message(&group),
            "case {case:?}: expected the message {:?}, got {:?}",
            input_security_group_in_use_message(&group),
            error_message(&refusal)
        );

        // The refusal changed nothing: the group reads back exactly as it did,
        // IN_USE rather than DELETED.
        let after = describe_group(&router, case, &group).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to change nothing, got {after}"
        );
        assert_eq!(
            after["state"],
            json!("IN_USE"),
            "case {case:?}: expected the group to stay IN_USE, got {:?}",
            after["state"]
        );

        // Deleting the inputs first releases the group, and the same delete is
        // then accepted.
        for id in &input_ids {
            let (status, deleted) = call(&router, "DELETE", &format!("/prod/inputs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteInput({id}): expected 200, got {status}: {deleted}"
            );
        }
        let released = describe_group(&router, case, &group).await;
        assert_eq!(
            released["inputs"],
            json!([]),
            "case {case:?}: expected the released group to report no user, got {:?}",
            released["inputs"]
        );
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/prod/inputSecurityGroups/{group}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the released group's delete to be accepted, got {status}: {deleted}"
        );
        let gone = describe_group(&router, case, &group).await;
        assert_eq!(
            gone["state"],
            json!("DELETED"),
            "case {case:?}: expected the deleted group to report DELETED, got {:?}",
            gone["state"]
        );
    }
}

/// Rule: the ordinary teardown order completes — deleting the channel detaches
/// the input it held, the input's own delete is then accepted with 200,
/// answering the input it deleted in `DELETING` with an empty
/// `AttachedChannels`, the clock settles it into the terminal `DELETED` with its
/// ingest endpoints released (`Destinations` empty), and the security group that
/// admitted it is deletable once no input uses it.
///
/// The rows span the ingest families, so what is torn down is the relationship
/// and not one input type: a pushed-to input reports the endpoints the delete
/// releases, a pulled-from one reports none to begin with.
#[tokio::test]
async fn the_ordinary_teardown_order_completes() {
    // (case, the input's type, how many ingest endpoints it reports while live)
    let table: [(&str, &str, usize); 3] = [
        ("pushed-to input with two endpoints", "RTMP_PUSH", 2),
        ("pulled-from input", "URL_PULL", 0),
        ("file input", "TS_FILE", 0),
    ];

    for (case, input_type, endpoints) in table {
        let router = medialive();
        let group = crate::support::input_security_group(&router).await;
        let mut body = input_body("feed", input_type);
        body["inputSecurityGroups"] = json!([group.clone()]);
        let input = create_input(&router, case, body).await;
        let input_id = input["id"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            input["destinations"].as_array().map(Vec::len),
            Some(endpoints),
            "case {case:?}: expected {endpoints} ingest endpoint(s), got {:?}",
            input["destinations"]
        );

        let (status, channel) = create_channel(
            &router,
            json!({
                "name": "live",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [attachment("feed", &input_id)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateChannel: expected 201, got {status}: {channel}"
        );
        let channel_id = channel["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        channel_in_state(&router, case, &channel_id, "IDLE").await;

        // While the channel holds it, the input is ATTACHED and its own delete
        // is refused.
        let (status, refusal) =
            call(&router, "DELETE", &format!("/prod/inputs/{input_id}"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the attached input's delete to be refused, got {status}: {refusal}"
        );

        let (status, deleting) =
            call(&router, "DELETE", &format!("/prod/channels/{channel_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: expected 200, got {status}: {deleting}"
        );

        let detached = describe_input(&router, case, &input_id).await;
        assert_eq!(
            detached["state"],
            json!("DETACHED"),
            "case {case:?}: expected the input to be DETACHED once its channel is deleted, got {:?}",
            detached["state"]
        );
        assert_eq!(
            detached["attachedChannels"],
            json!([]),
            "case {case:?}: expected no attached channel, got {:?}",
            detached["attachedChannels"]
        );

        let (status, deleted) =
            call(&router, "DELETE", &format!("/prod/inputs/{input_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the detached input's delete to succeed, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETING"),
            "case {case:?}: expected the delete to answer DELETING, got {:?}",
            deleted["state"]
        );
        assert_eq!(
            deleted["attachedChannels"],
            json!([]),
            "case {case:?}: expected the delete to answer an empty attachedChannels, got {:?}",
            deleted["attachedChannels"]
        );
        assert_eq!(
            deleted["id"],
            json!(input_id),
            "case {case:?}: expected the delete to answer the input it deleted, got {deleted}"
        );
        // Every member of the input's projection is answered, not an empty body.
        for member in ["arn", "id", "type", "state", "attachedChannels", "securityGroups"] {
            assert!(
                !deleted[member].is_null(),
                "case {case:?}: expected the delete to report {member}, got {deleted}"
            );
        }

        // The clock owes the terminal state, which a client polls for.
        let settled = input_in_state(&router, case, &input_id, "DELETED").await;
        assert_eq!(
            settled["destinations"],
            json!([]),
            "case {case:?}: expected the deleted input to report no ingest endpoint, got {:?}",
            settled["destinations"]
        );
        assert_eq!(
            settled["attachedChannels"],
            json!([]),
            "case {case:?}: expected the deleted input to report no attached channel, got {:?}",
            settled["attachedChannels"]
        );

        // And the group it was admitted through is now deletable.
        let released = describe_group(&router, case, &group).await;
        assert_eq!(
            released["state"],
            json!("IDLE"),
            "case {case:?}: expected the released group to be IDLE, got {:?}",
            released["state"]
        );
        let (status, group_deleted) = call(
            &router,
            "DELETE",
            &format!("/prod/inputSecurityGroups/{group}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group's delete to be accepted, got {status}: {group_deleted}"
        );
    }
}
