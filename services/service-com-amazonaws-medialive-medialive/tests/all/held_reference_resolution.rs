// SPDX-License-Identifier: CC0-1.0

//! What a WRITE resolves when the resource it edits already names a reference,
//! end to end over the restJson1 wire.
//!
//! Rule: an operation that writes a resource's configuration resolves only the
//! reference ids that write ADDS — an id the stored resource already names is
//! part of the configuration that resource already claims, so a request that
//! leaves a reference list alone is never refused for it, however the resource
//! that list names has fared since. A create holds nothing yet, so every id it
//! names is one it adds.
//!
//! The reference that can lose its resource under a live user is the input
//! security group a CHANNEL names: `DeleteInputSecurityGroup` is a soft delete,
//! and the group's own precondition is about the INPUTS using it ("The list of
//! inputs currently using this Input Security Group"), so a group a channel
//! names in `ChannelSecurityGroups` can still be deleted out from under it. The
//! deletes of the other resources an input or a channel references (an
//! SdiSource an input holds, an input a channel attaches, the group an input
//! names, a cluster or a channel placement group a channel is placed in) refuse
//! while that user exists, so what the channel rows take away is that group,
//! and the input rows span the other half of the same rule: an entry the input
//! already holds is not re-resolved, and only an entry the update ADDS is.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the issue's rename of an input whose group was deleted
//! is ONE row. The rows differ along what the rule generalises over — which
//! member the write names, whether it restates, replaces, extends or clears the
//! reference list that lost its resource, whether it ADDS an id this account
//! never held, where in the list that added id sits, and which `InputType`
//! family the input belongs to.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    required_for_input_type_message, unknown_reference_message, CHANNEL_CONTAINS_ERRORS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{channel_encoder_settings, input_security_group};
use crate::wire::{call, error_code, error_message, medialive};

/// An id of the shape this service mints for an input security group
/// (`state::FIRST_INPUT_SECURITY_GROUP_ID` and up) that no
/// `CreateInputSecurityGroup` ever minted in the account under test.
const UNMINTED_GROUP_ID: &str = "3999999";

/// The name every seeded resource is created under, so a row that renames it can
/// tell the rename from the stored name.
const SEEDED_NAME: &str = "ingest";

/// One input security group this account created and then deleted — a reference
/// that resolved when it was stored and does not resolve any more.
///
/// `DeleteInputSecurityGroup` is refused while INPUTS still use the group, so
/// this seeding is the CHANNEL side's: a channel's `ChannelSecurityGroups` entry
/// is not one of the group's inputs, so the group is deleted AFTER the channel
/// naming it was written and that channel's reference stops resolving.
async fn delete_group(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteInputSecurityGroup({id}): expected 200, got {status}: {body}"
    );
}

/// `UpdateInput` (PUT /prod/inputs/{InputId}), answering the status and either
/// the input it wrote or the refusal it answered.
async fn update_input(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "PUT", &format!("/prod/inputs/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["input"].clone())
    } else {
        (status, response)
    }
}

/// `DescribeInput` (GET /prod/inputs/{InputId}).
async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput({id}): expected 200, got {status}: {described}"
    );
    described
}

/// The redundant pair of source URLs a PULL-type input pulls from ("Every PULL
/// type input needs exactly two source URLs for redundancy").
fn pull_sources() -> Value {
    json!([
        { "url": "https://example.com/a.m3u8" },
        { "url": "https://example.com/b.m3u8" },
    ])
}

/// One input of `input_type` naming an input security group this account HOLDS,
/// answering its id and the group's id.
///
/// The group is not deleted here, unlike [`channel_whose_group_has_gone`] on the
/// channel side: `DeleteInputSecurityGroup` refuses a group while inputs still
/// use it ("The list of inputs currently using this Input Security Group"), so
/// an input naming a group that has gone is a state this service does not let a
/// caller reach. What the input rows below therefore span is the other half of
/// the resolution rule: an entry the input already holds is never re-resolved by
/// an update that leaves it alone, and only an entry the update ADDS is.
async fn input_holding_group(
    router: &MultiServiceRouter,
    case: &str,
    input_type: &str,
) -> (String, String) {
    let group = input_security_group(router).await;
    let mut body = json!({
        "name": SEEDED_NAME,
        "type": input_type,
        "inputSecurityGroups": [group.clone()],
    });
    if PULLED_FROM_TYPES.contains(&input_type) {
        body["sources"] = pull_sources();
    } else {
        body["destinations"] = json!([{ "streamName": "live" }]);
    }
    let (status, created) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: seeding the input: expected 201, got {status}: {created}"
    );
    let id = created["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (id, group)
}

/// The `InputType`s that ingest through the source URLs they pull from, so a
/// seeded input of one of them is given that redundant pair rather than a push
/// destination. Source: the `Sources` member's own documentation ("The source
/// URLs for a PULL-type input") and the `InputType` enum
/// (`smithy_query(query="shape", shape="InputType")`).
const PULLED_FROM_TYPES: [&str; 4] = ["URL_PULL", "RTMP_PULL", "MP4_FILE", "TS_FILE"];

/// One row of the input table: the patch an `UpdateInput` sends for an input
/// that already names an `InputSecurityGroups` entry, and what the service must
/// answer.
struct InputRow {
    case: &'static str,
    /// The `InputType` the row's input is created with.
    input_type: &'static str,
    /// The patch the update sends, given the group that has gone and a group
    /// this account still holds.
    patch: fn(&str, &str) -> Value,
    /// The status the update must answer.
    status: u16,
    /// The `SecurityGroups` the input must report afterwards.
    groups: fn(&str, &str) -> Value,
    /// The `Name` the input must report afterwards.
    name: &'static str,
    /// The message a refused row's `BadRequestException` must carry, given the
    /// group the input was seeded with and a second one this account holds;
    /// `None` on an accepted row.
    message: Option<fn(&str, &str) -> String>,
}

/// Rule: `UpdateInput` resolves only the `InputSecurityGroups` entries it ADDS —
/// the entries the input already holds are the configuration it already claims,
/// so an update that leaves them alone is never refused for them — while an
/// entry naming an id this account never held is refused with the modelled
/// `BadRequestException` (400) and nothing is written.
#[tokio::test]
async fn an_input_update_resolves_only_the_security_groups_it_adds() {
    let table: [InputRow; 11] = [
        InputRow {
            case: "a rename names no reference at all",
            input_type: "RTMP_PUSH",
            patch: |_, _| json!({ "name": "renamed" }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: "renamed",
            message: None,
        },
        InputRow {
            case: "a destination rewrite names no reference at all",
            input_type: "RTMP_PUSH",
            patch: |_, _| json!({ "destinations": [{ "streamName": "other" }] }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            message: None,
        },
        InputRow {
            case: "the seeded group restated adds nothing",
            input_type: "RTMP_PUSH",
            patch: |gone, _| json!({ "inputSecurityGroups": [gone] }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            message: None,
        },
        InputRow {
            case: "a second group added beside the seeded one",
            input_type: "RTMP_PUSH",
            patch: |gone, held| json!({ "inputSecurityGroups": [gone, held] }),
            status: 200,
            groups: |gone, held| json!([gone, held]),
            name: SEEDED_NAME,
            message: None,
        },
        InputRow {
            case: "the seeded group replaced by another one",
            input_type: "RTMP_PUSH",
            patch: |_, held| json!({ "inputSecurityGroups": [held] }),
            status: 200,
            groups: |_, held| json!([held]),
            name: SEEDED_NAME,
            message: None,
        },
        InputRow {
            case: "a rename of an input of another pushed-to type",
            input_type: "UDP_PUSH",
            patch: |_, _| json!({ "name": "renamed" }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: "renamed",
            message: None,
        },
        InputRow {
            case: "a rename of a pulled-from input holding the seeded group",
            input_type: "URL_PULL",
            patch: |_, _| json!({ "name": "renamed" }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: "renamed",
            message: None,
        },
        InputRow {
            case: "a pulled-from input clears the seeded group",
            input_type: "URL_PULL",
            patch: |_, _| json!({ "inputSecurityGroups": [] }),
            status: 200,
            groups: |_, _| json!([]),
            name: SEEDED_NAME,
            message: None,
        },
        InputRow {
            case: "an unminted id added beside the seeded one",
            input_type: "RTMP_PUSH",
            patch: |gone, _| json!({ "inputSecurityGroups": [gone, UNMINTED_GROUP_ID] }),
            status: 400,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            message: Some(|_, _| {
                unknown_reference_message(
                    "inputSecurityGroups",
                    "input security group",
                    UNMINTED_GROUP_ID,
                )
            }),
        },
        InputRow {
            case: "an unminted id replacing the seeded one, beside a rename",
            input_type: "RTMP_PUSH",
            patch: |_, _| {
                json!({ "name": "renamed", "inputSecurityGroups": [UNMINTED_GROUP_ID] })
            },
            status: 400,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            message: Some(|_, _| {
                unknown_reference_message(
                    "inputSecurityGroups",
                    "input security group",
                    UNMINTED_GROUP_ID,
                )
            }),
        },
        // The ingest-list rule is untouched by the resolution: a pushed-to input
        // is reached through the whitelist that admits the system pushing to it,
        // so an update that takes the whitelist away still describes an input
        // this service could not ingest through.
        InputRow {
            case: "the whitelist a pushed-to input ingests through taken away",
            input_type: "RTMP_PUSH",
            patch: |_, _| json!({ "inputSecurityGroups": [] }),
            status: 400,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            message: Some(|_, _| {
                required_for_input_type_message("inputSecurityGroups", "RTMP_PUSH")
            }),
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();
        let (id, gone) = input_holding_group(&router, case, row.input_type).await;
        let held = input_security_group(&router).await;
        let before = describe_input(&router, case, &id).await;

        let (status, response) = update_input(&router, &id, (row.patch)(&gone, &held)).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {}, got {status}: {response}",
            row.status
        );

        let after = describe_input(&router, case, &id).await;
        assert_eq!(
            after["securityGroups"],
            (row.groups)(&gone, &held),
            "case {case:?}: expected the groups {:?}, got {:?}",
            (row.groups)(&gone, &held),
            after["securityGroups"]
        );
        assert_eq!(
            after["name"],
            json!(row.name),
            "case {case:?}: expected the name {:?}, got {:?}",
            row.name,
            after["name"]
        );

        match row.message {
            Some(message) => {
                assert_eq!(
                    error_code(status, &response),
                    "BadRequestException",
                    "case {case:?}: expected the operation's own BadRequestException, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    message(&gone, &held),
                    "case {case:?}: expected the message {:?}, got {:?}",
                    message(&gone, &held),
                    error_message(&response)
                );
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the refused update to have written nothing, got {after}"
                );
            }
            // An accepted update answers the input it wrote, so the response and
            // the read-back agree.
            None => assert_eq!(
                response["securityGroups"], after["securityGroups"],
                "case {case:?}: expected the response to report what was stored, got {response}"
            ),
        }
    }
}

/// `CreateChannel` with `body`.
async fn create_channel(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: seeding the channel: expected 201, got {status}: {created}"
    );
    created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DescribeChannel` (GET /prod/channels/{ChannelId}).
async fn describe_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel({id}): expected 200, got {status}: {described}"
    );
    described
}

/// One channel naming an input security group this account then deletes,
/// answering its id and the id of the group that has gone. The channel has
/// stopped by the time this returns, which is the state an update is accepted
/// from.
async fn channel_whose_group_has_gone(
    router: &MultiServiceRouter,
    case: &str,
) -> (String, String) {
    let group = input_security_group(router).await;
    let id = create_channel(
        router,
        case,
        json!({
            "name": SEEDED_NAME,
            "encoderSettings": channel_encoder_settings(),
            "channelSecurityGroups": [group.clone()],
        }),
    )
    .await;
    delete_group(router, case, &group).await;
    wait_for_the_clock();
    (id, group)
}

/// One row of the channel table: the patch an `UpdateChannel` sends for a
/// channel whose stored `ChannelSecurityGroups` entry no longer resolves.
struct ChannelRow {
    case: &'static str,
    /// The patch the update sends, given the group that has gone and a group
    /// this account still holds.
    patch: fn(&str, &str) -> Value,
    /// The status the update must answer.
    status: u16,
    /// The `ChannelSecurityGroups` the channel must report afterwards.
    groups: fn(&str, &str) -> Value,
    /// The `Name` the channel must report afterwards.
    name: &'static str,
    /// The `(elementPath, errorMessage)` problems a refused row must report.
    problems: fn(&str, &str) -> Vec<(String, String)>,
}

/// Rule: the same resolution on the channel side — `UpdateChannel` resolves only
/// the `ChannelSecurityGroups` entries it ADDS, so a channel that already names a
/// group whose delete this service accepted can still be updated, while an entry
/// naming an id this account never held is refused with the modelled
/// `UnprocessableEntityException` (422), one `ValidationErrors` entry per
/// problem, and nothing is written.
#[tokio::test]
async fn a_channel_update_resolves_only_the_security_groups_it_adds() {
    let table: [ChannelRow; 6] = [
        ChannelRow {
            case: "a rename names no reference at all",
            patch: |_, _| json!({ "name": "renamed" }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: "renamed",
            problems: |_, _| Vec::new(),
        },
        ChannelRow {
            case: "the gone group restated adds nothing",
            patch: |gone, _| json!({ "channelSecurityGroups": [gone] }),
            status: 200,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            problems: |_, _| Vec::new(),
        },
        ChannelRow {
            case: "a held group added beside the gone one",
            patch: |gone, held| json!({ "channelSecurityGroups": [gone, held] }),
            status: 200,
            groups: |gone, held| json!([gone, held]),
            name: SEEDED_NAME,
            problems: |_, _| Vec::new(),
        },
        ChannelRow {
            case: "the gone group replaced by a held one",
            patch: |_, held| json!({ "channelSecurityGroups": [held] }),
            status: 200,
            groups: |_, held| json!([held]),
            name: SEEDED_NAME,
            problems: |_, _| Vec::new(),
        },
        ChannelRow {
            case: "an empty list detaches the gone group",
            patch: |_, _| json!({ "channelSecurityGroups": [] }),
            status: 200,
            groups: |_, _| json!([]),
            name: SEEDED_NAME,
            problems: |_, _| Vec::new(),
        },
        ChannelRow {
            case: "an unminted id added beside the gone one",
            patch: |gone, _| json!({ "channelSecurityGroups": [gone, UNMINTED_GROUP_ID] }),
            status: 422,
            groups: |gone, _| json!([gone]),
            name: SEEDED_NAME,
            problems: |_, _| {
                vec![(
                    "channelSecurityGroups/1".to_string(),
                    unknown_reference_message(
                        "channelSecurityGroups",
                        "input security group",
                        UNMINTED_GROUP_ID,
                    ),
                )]
            },
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();
        let (id, gone) = channel_whose_group_has_gone(&router, case).await;
        let held = input_security_group(&router).await;
        let before = describe_channel(&router, case, &id).await;

        let (status, response) = call(
            &router,
            "PUT",
            &format!("/prod/channels/{id}"),
            Some((row.patch)(&gone, &held)),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {}, got {status}: {response}",
            row.status
        );

        let after = describe_channel(&router, case, &id).await;
        assert_eq!(
            after["channelSecurityGroups"],
            (row.groups)(&gone, &held),
            "case {case:?}: expected the groups {:?}, got {:?}",
            (row.groups)(&gone, &held),
            after["channelSecurityGroups"]
        );
        assert_eq!(
            after["name"],
            json!(row.name),
            "case {case:?}: expected the name {:?}, got {:?}",
            row.name,
            after["name"]
        );

        let expected = (row.problems)(&gone, &held);
        if expected.is_empty() {
            assert_eq!(
                response["channel"]["channelSecurityGroups"], after["channelSecurityGroups"],
                "case {case:?}: expected the response to report what was stored, got {response}"
            );
        } else {
            assert_eq!(
                error_code(status, &response),
                "UnprocessableEntityException",
                "case {case:?}: expected UnprocessableEntityException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                CHANNEL_CONTAINS_ERRORS_MESSAGE,
                "case {case:?}: expected the message {CHANNEL_CONTAINS_ERRORS_MESSAGE:?}, got {:?}",
                error_message(&response)
            );
            assert_eq!(
                reported_problems(&response),
                expected,
                "case {case:?}: expected the problems {expected:?}, got {:?}",
                response["validationErrors"]
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to have written nothing, got {after}"
            );
        }
    }
}

/// The `(elementPath, errorMessage)` pairs a channel-configuration refusal
/// reported, in the order it reported them.
fn reported_problems(body: &Value) -> Vec<(String, String)> {
    body["validationErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|problem| {
            (
                problem["elementPath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                problem["errorMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}
