// SPDX-License-Identifier: CC0-1.0

//! Whether the reason the tagging plane words for an ARN that names no stored
//! resource fits the member it travels on, whatever RESOURCE that ARN names.
//!
//! RULE under test: every operation that addresses a resource by `resourceARN`
//! states the miss on the modelled `ErrorMessage`, `@length(0..2048)`, so the
//! reason is rendered within those 2048 characters — reported exactly as the
//! service words it where it fits, and at the bound with its leading text and
//! trailing clause kept where it does not — for every ARN the modelled
//! `TwinMakerArn` admits, which the model bounds at those same 2048 characters.
//!
//! `tests/all/unbounded_refusal_message_bounds.rs` asserts the rule over the
//! SIZES one form of ARN — a workspace's — is driven at, and this suite spans
//! the other dimension the rule generalises over: the FORM of the ARN. Its rows
//! differ along which taggable resource the resource segment names (a workspace,
//! an entity, a component type, a scene, a sync job), whether the workspace that
//! resource nests under is one the store holds, a resource that nests under no
//! workspace at all, a resource segment naming no kind this service files, the
//! partition the ARN was minted in, and the extremes of the shape the modelled
//! pattern admits — each at the sizes the bound is decided at. The reason's
//! wording and what the rule asserts about a reported reason come from the two
//! suites that already state them, so this crate states each once.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::SYNC_SOURCE_SITEWISE;

use crate::harness::{
    component_type_arn, entity_arn, metadata_transfer_job_arn, scene_arn, sync_job_arn,
    workspace_arn, Emulator, ACCOUNT, REGION,
};
use crate::unbounded_refusal_message_bounds::{
    padded, resource_not_found, sizes, TAGGING, TWIN_MAKER_ARN_MAX,
};

/// The workspace the store DOES hold: the rows that nest an absent resource
/// under a resolvable workspace name this one, and it is also the resource whose
/// own tag set proves the refused calls wrote nothing.
const HELD_WORKSPACE: &str = "tagarnforms";

/// The id every row names for a resource the store does not hold.
const ABSENT: &str = "absent";

/// The same, as a `componentTypeId` is spelled — a dotted name, which the
/// modelled `TwinMakerArn` pattern admits in a resource segment
/// (`[\/a-zA-Z0-9_\-\.:]+`).
const ABSENT_COMPONENT_TYPE: &str = "com.example.absent";

/// The tag the held workspace carries throughout, so a refused call that wrote
/// or removed a tag anywhere would show up as a changed tag set.
const HELD_TAG_KEY: &str = "plant";
/// See [`HELD_TAG_KEY`].
const HELD_TAG_VALUE: &str = "turbine-hall";

/// Every form of ARN a row drives, as the case it is reported under and the ARN
/// as it is minted (before a row pads it to a size).
///
/// One entry per kind of resource this service files in its ARN index, each
/// naming a resource the store does not hold, plus the two forms that resolve to
/// no kind at all: a resource that nests under no workspace, and a resource
/// segment that names no kind this service files.
fn arn_forms() -> Vec<(String, String)> {
    let mut forms = vec![(
        "a workspace the store does not hold".to_string(),
        workspace_arn(ABSENT),
    )];
    for (workspace_case, workspace_id) in [
        ("the workspace the store holds", HELD_WORKSPACE),
        ("a workspace the store does not hold", ABSENT),
    ] {
        forms.push((
            format!("an entity of {workspace_case}"),
            entity_arn(workspace_id, ABSENT),
        ));
        forms.push((
            format!("a component type of {workspace_case}"),
            component_type_arn(workspace_id, ABSENT_COMPONENT_TYPE),
        ));
        forms.push((
            format!("a scene of {workspace_case}"),
            scene_arn(workspace_id, ABSENT),
        ));
        forms.push((
            format!("a sync job of {workspace_case}"),
            sync_job_arn(workspace_id, SYNC_SOURCE_SITEWISE),
        ));
    }
    forms.push((
        "a metadata transfer job, which nests under no workspace".to_string(),
        metadata_transfer_job_arn(ABSENT),
    ));
    forms.push((
        "a resource segment naming no kind this service files".to_string(),
        format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:{ABSENT}"),
    ));
    forms
}

/// RULE: every operation that addresses a resource by `resourceARN` states the
/// miss within the 2048 characters the member the reason travels on admits,
/// whatever resource the ARN names and however long the ARN is.
#[tokio::test]
async fn a_tagging_refusal_fits_its_member_for_every_form_of_arn() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(HELD_WORKSPACE).await.status,
        200,
        "the workspace the rows nest an absent resource under must exist"
    );
    let held = workspace_arn(HELD_WORKSPACE);
    let tagged = emulator
        .tag_resource(&held, json!({HELD_TAG_KEY: HELD_TAG_VALUE}))
        .await;
    assert_eq!(
        tagged.status, 200,
        "the tag the refusals are checked against must be written, got {tagged:?}"
    );

    let fixed = resource_not_found("").chars().count();
    for (form_case, base) in arn_forms() {
        // The sizes are the ones the bound is decided at, derived from the
        // member's maximum and the ARN's own, in this crate's one construction
        // of them: the ARN as minted, the one whose reason lands exactly on the
        // member's maximum, one character past it, and the widest the model
        // admits.
        for (size_case, length) in sizes(fixed, base.chars().count(), TWIN_MAKER_ARN_MAX) {
            let resource_arn = padded(&base, length);
            for operation in TAGGING {
                let case = format!("{operation:?} against {form_case} with {size_case}");
                operation
                    .assert_refusal_fits(&emulator, &case, &resource_arn)
                    .await;
            }
        }
    }

    // Nothing the refused calls asked for happened, read back through the
    // operation that reports it: the one resource the store holds carries
    // exactly the tag it was given, neither the tag a refused write asked for
    // nor one less than a refused removal asked to drop.
    let listed = emulator.list_tags_for_resource(&held, json!({})).await;
    assert_eq!(
        (
            listed.status,
            listed.member("tags")[HELD_TAG_KEY].as_str(),
            listed.member("tags").as_object().map(serde_json::Map::len)
        ),
        (200, Some(HELD_TAG_VALUE), Some(1)),
        "a refused tagging call changes no stored tag set, got {listed:?}"
    );
}

/// The ARNs at the extremes of the shape the modelled `TwinMakerArn` pattern
/// admits (`^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$`,
/// from `smithy_query(query="operation",operation="ListTagsForResource")`), each
/// naming a resource no store of this service holds.
fn arn_shapes() -> Vec<(String, String)> {
    vec![
        (
            "the shortest ARN the pattern's own segments admit".to_string(),
            format!("arn:aws:iottwinmaker:a:{ACCOUNT}:a"),
        ),
        (
            "an ARN of the aws-cn partition".to_string(),
            format!("arn:aws-cn:iottwinmaker:cn-north-1:{ACCOUNT}:workspace/{ABSENT}"),
        ),
        (
            "an ARN of the aws-us-gov partition".to_string(),
            format!("arn:aws-us-gov:iottwinmaker:us-gov-west-1:{ACCOUNT}:workspace/{ABSENT}"),
        ),
        (
            "an ARN whose resource segment is colon-separated".to_string(),
            format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace:{ABSENT}"),
        ),
    ]
}

/// RULE: the same reason is reported, within the same member, for an ARN at
/// either extreme of the shape the model admits — the reason is built from what
/// the request named, so no form of ARN is reported some other way.
#[tokio::test]
async fn a_tagging_refusal_fits_its_member_for_every_shape_of_arn() {
    let emulator = Emulator::new();

    for (shape_case, resource_arn) in arn_shapes() {
        for operation in TAGGING {
            let case = format!("{operation:?} against {shape_case}");
            operation
                .assert_refusal_fits(&emulator, &case, &resource_arn)
                .await;
        }
    }
}
