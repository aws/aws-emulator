// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AppStream image builder lifecycle over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `CreateImageBuilder` launches the builder FROM an image this account can see,
//!   selected by name or by ARN, and reports the builder from the stored record: the ARN
//!   and the platform it reports are the image's own, and a later `DescribeImageBuilders`
//!   answers identically;
//! * the builder's instance comes up on the clock — "the initial state of the builder is
//!   PENDING. When it is ready, the state is RUNNING" — so a caller reading straight after
//!   the create still sees `PENDING`, and only the state moves when it is up;
//! * a request that selects no image, or selects one both ways, or selects an image the
//!   catalog does not hold, is refused and creates nothing;
//! * `StopImageBuilder` terminates the instance: the builder moves to `STOPPING` and the
//!   clock owes it `STOPPED`;
//! * `UpdateImageBuilder` writes the members the request carries onto a STOPPED builder,
//!   leaves the members it omits alone, merges a partial agent access configuration by
//!   agent action, and clears the attributes its `AttributesToDelete` names;
//! * a builder outside the one state the operation documents admits no write at all, and
//!   is left exactly as it was;
//! * a name this account holds no builder under is refused `ResourceNotFoundException`
//!   with AWS's sentence for that refusal class, whatever form the name takes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::images::{catalog, published_image_arn};
use crate::not_found_refusal_status::REFUSAL_STATUS;
use crate::refusal_messages::{already_exists_sentence, not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The words AWS's refusal sentences use for an image builder and for an image, read from
/// the crate so the test tracks the constants rather than copies of their values.
const BUILDER_KIND: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_KIND;
const IMAGE_KIND: &str = service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_KIND;

/// The `ImageBuilderState` values this suite drives a builder through, read from the crate
/// for the same reason.
const STATE_PENDING: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_STATE_PENDING;
const STATE_RUNNING: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_STATE_RUNNING;
const STATE_STOPPING: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_STATE_STOPPING;
const STATE_STOPPED: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_STATE_STOPPED;

/// The span the clock owes a builder whose instance is coming up or going away.
const STATE_SPAN: Duration =
    service_com_amazonaws_appstream_photonadminproxyservice::state::IMAGE_BUILDER_STATE_SPAN;

/// The instance types `CreateImageBuilder.InstanceType` documents as available
/// (`smithy_query(query="doc", shape="CreateImageBuilderRequest", member="InstanceType")`:
/// "stream.standard.small stream.standard.medium stream.standard.large
/// stream.compute.large …"). The rows below span several families rather than one.
const INSTANCE_TYPES: &[&str] = &[
    "stream.standard.small",
    "stream.standard.large",
    "stream.compute.xlarge",
    "stream.memory.2xlarge",
];

/// Names spanning the class: the rows differ along length, character classes and case. The
/// last is the longest name a create admits — `Name`'s pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$` (`smithy_query(query="operation",
/// operation="CreateImageBuilder")`), so 101 characters.
fn names() -> Vec<String> {
    vec![
        "builder".to_string(),
        "Mixed.Case_builder-9".to_string(),
        "0123456789".to_string(),
        format!("i{}", "7".repeat(100)),
    ]
}

/// How a create selects the image the builder is launched from: the two members the
/// operation models for it, so every row of a table is driven both ways.
#[derive(Clone, Copy, Debug)]
enum Selector {
    ByName,
    ByArn,
}

impl Selector {
    /// The create request members that select `image` in `region`.
    fn member(self, region: &str, image: &str) -> Value {
        match self {
            Self::ByName => json!({ "ImageName": image }),
            Self::ByArn => json!({ "ImageArn": published_image_arn(region, image) }),
        }
    }
}

/// The create request for a builder named `name`, launched from `image` on
/// `instance_type`.
fn create_request(region: &str, name: &str, image: &str, instance_type: &str, by: Selector) -> Value {
    let mut request = json!({ "Name": name, "InstanceType": instance_type });
    let selector = by.member(region, image);
    for (member, value) in selector.as_object().expect("the selector is an object") {
        request[member] = value.clone();
    }
    request
}

/// The images the account can see, by name, with the platform each reports — read through
/// the front door, so the rows below are the catalog this store really holds.
async fn images(emulator: &Emulator) -> Vec<(String, String)> {
    catalog(emulator)
        .await
        .into_iter()
        .map(|image| {
            let name = image["Name"].as_str().unwrap_or_default().to_string();
            let platform = image["Platform"].as_str().unwrap_or_default().to_string();
            (name, platform)
        })
        .collect()
}

/// Create one image builder and answer the `ImageBuilder` the call reported.
async fn create(emulator: &Emulator, case: &str, request: Value) -> Value {
    let created = emulator.ok(case, "CreateImageBuilder", request).await;
    created
        .get("ImageBuilder")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ImageBuilder, got {created}"))
        .clone()
}

/// The builder one `DescribeImageBuilders` reports for `name`.
async fn described(emulator: &Emulator, case: &str, name: &str) -> Value {
    let payload = emulator
        .ok(case, "DescribeImageBuilders", json!({ "Names": [name] }))
        .await;
    payload
        .pointer("/ImageBuilders/0")
        .unwrap_or_else(|| panic!("case {case:?}: expected the builder, got {payload}"))
        .clone()
}

/// The `State` a read reports for `name`.
async fn reported_state(emulator: &Emulator, case: &str, name: &str) -> String {
    let builder = described(emulator, case, name).await;
    builder
        .get("State")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the builder's State, got {builder}"))
        .to_string()
}

/// Wait for the state the clock owes a builder. The transition is due on the clock; nothing
/// here hurries it, and the deadline is far past the span so a loaded host does not decide
/// the outcome.
async fn await_state(emulator: &Emulator, case: &str, name: &str, expected: &str) {
    let deadline = Instant::now() + STATE_SPAN * 20;
    loop {
        let state = reported_state(emulator, case, name).await;
        if state == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the builder must reach {expected} within the span the call owes \
             it, still {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Stop `name` and answer the `ImageBuilder` the call reported.
async fn stop(emulator: &Emulator, case: &str, name: &str) -> Value {
    let payload = emulator
        .ok(case, "StopImageBuilder", json!({ "Name": name }))
        .await;
    payload
        .get("ImageBuilder")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ImageBuilder, got {payload}"))
        .clone()
}

/// A builder with no instance, which is the one state a write of its fields is admitted in:
/// created, brought up by the clock, then stopped and awaited.
async fn stopped_builder(emulator: &Emulator, case: &str, request: Value) -> Value {
    let name = request["Name"].as_str().expect("the row names the builder").to_string();
    create(emulator, case, request).await;
    await_state(emulator, case, &name, STATE_RUNNING).await;
    stop(emulator, case, &name).await;
    await_state(emulator, case, &name, STATE_STOPPED).await;
    described(emulator, case, &name).await
}

/// Update `name` with `members` and answer the `ImageBuilder` the call reported.
async fn update(emulator: &Emulator, case: &str, name: &str, members: Value) -> Value {
    let mut request = json!({ "Name": name });
    for (member, value) in members.as_object().expect("the row is an object") {
        request[member] = value.clone();
    }
    let payload = emulator.ok(case, "UpdateImageBuilder", request).await;
    payload
        .get("ImageBuilder")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ImageBuilder, got {payload}"))
        .clone()
}

/// Assert that `reported` carries every member of `stored` but the ones in `moved`.
fn assert_only_moved(case: &str, stored: &Value, reported: &Value, moved: &[&str]) {
    let members = stored
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a builder object, got {stored}"));
    for (member, value) in members {
        if moved.contains(&member.as_str()) {
            continue;
        }
        let actual = reported.get(member);
        assert_eq!(
            actual,
            Some(value),
            "case {case:?}: {member} must be left as it was ({value}), got {actual:?}"
        );
    }
}

/// A complete agent access configuration, as `ImageBuilderAgentAccessConfig` requires one:
/// the settings, the screen resolution and the image format are its required members
/// (`smithy_query(query="shape", shape="ImageBuilderAgentAccessConfig")`).
fn agent_access_config() -> Value {
    json!({
        "Settings": [
            { "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" },
            { "AgentAction": "COMPUTER_INPUT", "Permission": "ENABLED" },
        ],
        "ScreenResolution": "W_1280xH_720",
        "ScreenImageFormat": "PNG",
        "ScreenshotsUploadEnabled": true,
        "S3BucketArn": "arn:aws:s3:::agent-screenshots",
    })
}

#[tokio::test]
async fn a_create_launches_the_builder_from_the_image_it_selects_and_a_read_reports_it() {
    // Rows: the Region the request's credential scope names, and — within each Region — the
    // name's form, the instance type, the image the builder is launched from and which of
    // the two members selects it.
    for region in ["us-east-1", "eu-west-2"] {
        let emulator = Emulator::in_region(region);
        let catalog = images(&emulator).await;
        assert!(
            catalog.len() >= 2,
            "case {region:?}: the account must see the published base images, got {}",
            catalog.len()
        );
        let mut created_builders: Vec<(String, String, Value)> = Vec::new();

        for (index, name) in names().into_iter().enumerate() {
            let by = if index % 2 == 0 {
                Selector::ByName
            } else {
                Selector::ByArn
            };
            let (image, platform) = catalog[index % catalog.len()].clone();
            let instance_type = INSTANCE_TYPES[index % INSTANCE_TYPES.len()];
            let case = format!("{region} {by:?} {name:?} from {image}");

            let created = create(
                &emulator,
                &case,
                create_request(region, &name, &image, instance_type, by),
            )
            .await;

            assert_eq!(
                created["Name"],
                json!(name),
                "case {case:?}: the builder must report the name it was created under, \
                 got {created}"
            );
            let expected_arn = format!(
                "arn:aws:appstream:{region}:{}:image-builder/{name}",
                engine::EMULATOR_ACCOUNT_ID
            );
            assert_eq!(
                created["Arn"],
                json!(expected_arn),
                "case {case:?}: expected Arn {expected_arn:?}, got {created}"
            );
            // The image is reported as the catalog addresses it, whichever member selected
            // it: the builder references an image this store really holds.
            assert_eq!(
                created["ImageArn"],
                json!(published_image_arn(region, &image)),
                "case {case:?}: the builder must report the ARN of the image it runs, \
                 got {created}"
            );
            assert_eq!(
                created["Platform"],
                json!(platform),
                "case {case:?}: the platform is the image's own ({platform}), got {created}"
            );
            assert_eq!(
                created["InstanceType"],
                json!(instance_type),
                "case {case:?}: expected InstanceType {instance_type:?}, got {created}"
            );
            assert_eq!(
                created["State"],
                json!(STATE_PENDING),
                "case {case:?}: a builder whose instance is coming up must be \
                 {STATE_PENDING}, got {created}"
            );

            // The create answers the stored record, so a read straight after it reports
            // the same builder.
            let read = described(&emulator, &case, &name).await;
            assert_eq!(
                read, created,
                "case {case:?}: the read must report the record the create wrote \
                 ({created}), got {read}"
            );
            created_builders.push((case, name, created));
        }

        for (case, name, created) in created_builders {
            await_state(&emulator, &case, &name, STATE_RUNNING).await;
            let running = described(&emulator, &case, &name).await;
            assert_only_moved(&case, &created, &running, &["State"]);

            // The transition is applied once: a later read reports the same running
            // builder rather than moving it on again.
            let again = described(&emulator, &case, &name).await;
            assert_eq!(
                again, running,
                "case {case:?}: a second read must answer identically, got {again}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_that_selects_no_image_it_can_launch_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();

    // Rows: the two ways a request selects no single image (neither member, both members),
    // and the two ways it names one the catalog does not hold.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "neither member",
            json!({ "Name": "no-image", "InstanceType": INSTANCE_TYPES[0] }),
            "InvalidParameterCombinationException",
            "Either ImageName or ImageArn must be specified.".to_string(),
        ),
        (
            "both members",
            json!({
                "Name": "two-images",
                "InstanceType": INSTANCE_TYPES[0],
                "ImageName": image.clone(),
                "ImageArn": published_image_arn("us-east-1", &image),
            }),
            "InvalidParameterCombinationException",
            "You must specify either the image name or the image ARN, not both.".to_string(),
        ),
        (
            "a name the catalog does not hold",
            json!({
                "Name": "absent-image",
                "InstanceType": INSTANCE_TYPES[1],
                "ImageName": "AppStream-No-Such-Image",
            }),
            "ResourceNotFoundException",
            not_found_sentence(IMAGE_KIND, "AppStream-No-Such-Image"),
        ),
        (
            "an ARN the catalog does not hold",
            json!({
                "Name": "absent-image-arn",
                "InstanceType": INSTANCE_TYPES[2],
                "ImageArn": published_image_arn("us-east-1", "AppStream-No-Such-Image"),
            }),
            "ResourceNotFoundException",
            not_found_sentence(IMAGE_KIND, "AppStream-No-Such-Image"),
        ),
    ];

    for (case, request, code, expected) in rows {
        let name = request["Name"].as_str().expect("the row names a builder").to_string();
        let status = if code == "ResourceNotFoundException" {
            REFUSAL_STATUS
        } else {
            400
        };
        let message = refusal(&emulator, case, "CreateImageBuilder", request, status, code).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // The refused create wrote nothing: the name it asked for holds no builder.
        let read = emulator
            .call("DescribeImageBuilders", json!({ "Names": [name.clone()] }))
            .await;
        assert_eq!(
            read.0, REFUSAL_STATUS,
            "case {case:?}: the refused create must have stored no builder under {name:?}, \
             got {} {}",
            read.0, read.1
        );
    }
}

#[tokio::test]
async fn a_create_of_a_name_the_account_already_holds_is_refused() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;

    for (index, name) in names().into_iter().enumerate() {
        let (image, _) = catalog[index % catalog.len()].clone();
        let case = format!("second create of {name:?}");
        let request = create_request(
            "us-east-1",
            &name,
            &image,
            INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
            Selector::ByName,
        );
        let created = create(&emulator, &case, request.clone()).await;

        let message = refusal(
            &emulator,
            &case,
            "CreateImageBuilder",
            request,
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        let expected = already_exists_sentence(BUILDER_KIND, &name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // The refused create left the builder the account holds exactly as it was.
        let read = described(&emulator, &case, &name).await;
        assert_eq!(
            read, created,
            "case {case:?}: the refused create must leave the stored builder untouched \
             ({created}), got {read}"
        );
    }
}

#[tokio::test]
async fn a_stop_moves_the_builder_to_stopping_and_the_clock_owes_it_stopped() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let mut stopping: Vec<(String, String, Value)> = Vec::new();

    // Rows: the name's form, the image and the instance type. Every builder is stopped
    // before any is awaited, so the span the clock owes them runs once.
    for (index, name) in names().into_iter().enumerate() {
        let (image, _) = catalog[index % catalog.len()].clone();
        let case = format!("stop of {name:?}");
        let request = create_request(
            "us-east-1",
            &name,
            &image,
            INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
            Selector::ByArn,
        );
        create(&emulator, &case, request).await;
        await_state(&emulator, &case, &name, STATE_RUNNING).await;
        let running = described(&emulator, &case, &name).await;

        let reported = stop(&emulator, &case, &name).await;
        assert_eq!(
            reported["State"],
            json!(STATE_STOPPING),
            "case {case:?}: a caller reading straight after the stop must still see \
             {STATE_STOPPING}, got {reported}"
        );
        assert_only_moved(&case, &running, &reported, &["State"]);

        // A second stop asks for nothing new: the builder is answered as it stands.
        let again = stop(&emulator, &case, &name).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a stop of a builder already stopping must answer the builder \
             it already is ({reported}), got {again}"
        );
        stopping.push((case, name, running));
    }

    for (case, name, running) in stopping {
        await_state(&emulator, &case, &name, STATE_STOPPED).await;
        let stopped = described(&emulator, &case, &name).await;
        assert_only_moved(&case, &running, &stopped, &["State"]);

        // A stop of a builder that already has no instance changes nothing and starts no
        // second span.
        let reported = stop(&emulator, &case, &name).await;
        assert_eq!(
            reported, stopped,
            "case {case:?}: a stop of a stopped builder must answer it unchanged \
             ({stopped}), got {reported}"
        );
    }
}

#[tokio::test]
async fn an_update_writes_the_members_it_carries_onto_a_stopped_builder() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();

    // Rows: which members the request carries — one display member, both, a whole agent
    // access configuration, a partial one merged onto it by agent action, a merge that
    // turns the screenshots off, and an `AttributesToDelete` that clears the configuration.
    // Each row is driven on its own builder, so no row depends on another's writes.
    let rows: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "the description alone",
            vec![json!({ "Description": "the first builder" })],
            json!({ "Description": "the first builder" }),
        ),
        (
            "both display members",
            vec![json!({ "Description": "described", "DisplayName": "Displayed" })],
            json!({ "Description": "described", "DisplayName": "Displayed" }),
        ),
        (
            "a whole agent access configuration",
            vec![json!({ "AgentAccessConfig": agent_access_config() })],
            json!({ "AgentAccessConfig": agent_access_config() }),
        ),
        (
            "a partial agent access configuration merged by agent action",
            vec![
                json!({ "AgentAccessConfig": agent_access_config() }),
                json!({
                    "AgentAccessConfig": {
                        "Settings": [
                            { "AgentAction": "COMPUTER_INPUT", "Permission": "DISABLED" },
                            { "AgentAction": "FORWARD_MCP_TOOLS", "Permission": "ENABLED" },
                        ],
                        "ScreenImageFormat": "JPEG",
                    },
                }),
            ],
            json!({
                "AgentAccessConfig": {
                    // The setting for the action the update named is replaced, the other
                    // stored action keeps its setting, and the action the builder held none
                    // for is appended.
                    "Settings": [
                        { "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" },
                        { "AgentAction": "COMPUTER_INPUT", "Permission": "DISABLED" },
                        { "AgentAction": "FORWARD_MCP_TOOLS", "Permission": "ENABLED" },
                    ],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "JPEG",
                    "ScreenshotsUploadEnabled": true,
                    "S3BucketArn": "arn:aws:s3:::agent-screenshots",
                },
            }),
        ),
        (
            "a merge that turns the screenshots off",
            vec![
                json!({ "AgentAccessConfig": agent_access_config() }),
                json!({ "AgentAccessConfig": { "ScreenshotsUploadEnabled": false } }),
            ],
            json!({
                "AgentAccessConfig": {
                    "Settings": [
                        { "AgentAction": "COMPUTER_VISION", "Permission": "ENABLED" },
                        { "AgentAction": "COMPUTER_INPUT", "Permission": "ENABLED" },
                    ],
                    "ScreenResolution": "W_1280xH_720",
                    "ScreenImageFormat": "PNG",
                    "ScreenshotsUploadEnabled": false,
                },
            }),
        ),
        (
            "an AttributesToDelete that clears the configuration",
            vec![
                json!({ "AgentAccessConfig": agent_access_config() }),
                json!({ "AttributesToDelete": ["AGENT_ACCESS_CONFIG"] }),
            ],
            Value::Null,
        ),
    ];

    for (index, (case, updates, expected)) in rows.into_iter().enumerate() {
        let name = format!("updated-{index}");
        let stored = stopped_builder(
            &emulator,
            case,
            create_request(
                "us-east-1",
                &name,
                &image,
                INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
                Selector::ByName,
            ),
        )
        .await;

        let mut reported = Value::Null;
        for members in &updates {
            reported = update(&emulator, case, &name, members.clone()).await;
        }

        // What the update answers is what a later read reports.
        let read = described(&emulator, case, &name).await;
        assert_eq!(
            read, reported,
            "case {case:?}: the read must report the record the update wrote ({reported}), \
             got {read}"
        );

        match expected.as_object() {
            Some(members) => {
                for (member, value) in members {
                    assert_eq!(
                        reported.get(member),
                        Some(value),
                        "case {case:?}: expected {member} {value}, got {reported}"
                    );
                }
                // Every member the updates did not name is left as the create stored it.
                let written: Vec<&str> = members.keys().map(String::as_str).collect();
                assert_only_moved(case, &stored, &reported, &written);
            }
            None => {
                assert!(
                    reported.get("AgentAccessConfig").is_none(),
                    "case {case:?}: the cleared attribute must be absent, got {reported}"
                );
                assert_only_moved(case, &stored, &reported, &["AgentAccessConfig"]);
            }
        }
    }
}

#[tokio::test]
async fn an_update_of_a_builder_outside_the_stopped_state_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();

    // Rows: each state a builder reaches that is not STOPPED — coming up, up, and going
    // down — reached the way a caller reaches it.
    for (index, state) in [STATE_PENDING, STATE_RUNNING, STATE_STOPPING]
        .into_iter()
        .enumerate()
    {
        let name = format!("not-stopped-{index}");
        let case = format!("an update of a {state} builder");
        create(
            &emulator,
            &case,
            create_request(
                "us-east-1",
                &name,
                &image,
                INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
                Selector::ByName,
            ),
        )
        .await;
        if state != STATE_PENDING {
            await_state(&emulator, &case, &name, STATE_RUNNING).await;
        }
        if state == STATE_STOPPING {
            stop(&emulator, &case, &name).await;
        }
        let stored = described(&emulator, &case, &name).await;
        assert_eq!(
            stored["State"],
            json!(state),
            "case {case:?}: the builder must be {state} before the update, got {stored}"
        );

        let message = refusal(
            &emulator,
            &case,
            "UpdateImageBuilder",
            json!({ "Name": name.clone(), "Description": "a write the state does not admit" }),
            400,
            "OperationNotPermittedException",
        )
        .await;
        let expected = format!(
            "Cannot update the attributes for ImageBuilder {name} when it is in {state} state. "
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // The refused update wrote nothing.
        let read = described(&emulator, &case, &name).await;
        assert_eq!(
            read, stored,
            "case {case:?}: the refused update must leave the builder untouched ({stored}), \
             got {read}"
        );
    }
}

#[tokio::test]
async fn an_update_that_both_sets_and_deletes_one_attribute_is_refused() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();
    let name = "both-ways";
    let case = "set and delete the agent access configuration";
    let stored = stopped_builder(
        &emulator,
        case,
        create_request("us-east-1", name, &image, INSTANCE_TYPES[0], Selector::ByName),
    )
    .await;

    let message = refusal(
        &emulator,
        case,
        "UpdateImageBuilder",
        json!({
            "Name": name,
            "AgentAccessConfig": agent_access_config(),
            "AttributesToDelete": ["AGENT_ACCESS_CONFIG"],
        }),
        400,
        "InvalidParameterCombinationException",
    )
    .await;
    let expected = "You can’t update and delete agent access config at the same time.";
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );

    let read = described(&emulator, case, name).await;
    assert_eq!(
        read, stored,
        "case {case:?}: the refused update must leave the builder untouched ({stored}), \
         got {read}"
    );
}

#[tokio::test]
async fn a_name_the_account_holds_no_builder_under_is_refused_by_every_reader_and_writer() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();
    // One builder the account does hold, so the refusals below are about the name asked for
    // rather than about an empty store. A fleet of one of the names too: a builder is a
    // resource family of its own, so a name another family holds still names no builder.
    let stored = create(
        &emulator,
        "setup",
        create_request(
            "us-east-1",
            "stored-builder",
            &image,
            INSTANCE_TYPES[0],
            Selector::ByName,
        ),
    )
    .await;
    emulator
        .ok(
            "setup",
            "CreateFleet",
            json!({ "Name": "builder", "InstanceType": INSTANCE_TYPES[0] }),
        )
        .await;

    // Rows: each operation that addresses a builder by name, over each form of name.
    for operation in [
        "DescribeImageBuilders",
        "StopImageBuilder",
        "UpdateImageBuilder",
    ] {
        for name in names() {
            let case = format!("{operation} of absent {name:?}");
            let request = if operation == "DescribeImageBuilders" {
                json!({ "Names": [name.clone()] })
            } else {
                json!({ "Name": name.clone() })
            };
            let message = refusal(
                &emulator,
                &case,
                operation,
                request,
                REFUSAL_STATUS,
                "ResourceNotFoundException",
            )
            .await;
            let expected = not_found_sentence(BUILDER_KIND, &name);
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }

    // The refused calls wrote nothing: the one builder the account holds is as it was.
    let read = described(&emulator, "stored-builder", "stored-builder").await;
    assert_eq!(
        read, stored,
        "case \"stored-builder\": the refused calls must leave the stored builder \
         untouched ({stored}), got {read}"
    );
}

#[tokio::test]
async fn a_read_pages_every_builder_exactly_once() {
    let emulator = Emulator::new();
    let catalog = images(&emulator).await;
    let (image, _) = catalog[0].clone();
    let mut expected: Vec<String> = Vec::new();
    for (index, name) in names().into_iter().enumerate() {
        create(
            &emulator,
            "paging",
            create_request(
                "us-east-1",
                &name,
                &image,
                INSTANCE_TYPES[index % INSTANCE_TYPES.len()],
                Selector::ByName,
            ),
        )
        .await;
        expected.push(name);
    }
    expected.sort();

    // Rows: the page size the request asks for, from one record per page up to more than
    // the store holds.
    for page_size in [1, 2, 3, 100] {
        let case = format!("MaxResults {page_size}");
        let mut reported: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = next_token.clone() {
                request["NextToken"] = token;
            }
            let payload = emulator.ok(&case, "DescribeImageBuilders", request).await;
            let page = payload["ImageBuilders"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a page, got {payload}"));
            assert!(
                page.len() <= page_size.max(1),
                "case {case:?}: a page must hold at most {page_size} records, got {payload}"
            );
            reported.extend(
                page.iter()
                    .map(|builder| builder["Name"].as_str().unwrap_or_default().to_string()),
            );
            next_token = payload.get("NextToken").filter(|token| !token.is_null()).cloned();
            if next_token.is_none() {
                break;
            }
        }
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the walk must report every stored builder exactly once, \
             expected {expected:?}, got {reported:?}"
        );
    }
}
