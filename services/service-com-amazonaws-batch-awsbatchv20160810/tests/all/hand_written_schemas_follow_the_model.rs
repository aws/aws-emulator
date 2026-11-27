// SPDX-License-Identifier: CC0-1.0

//! The shapes this crate declares BY HAND, pinned against the model they are declared from.
//!
//! Four of this service's shapes cannot be declared through
//! `#[smithy_macros::smithy(shape = ...)]`, because the derive admits exactly the model's
//! member list and writes a member only when it holds a value:
//!
//! * `JobDetail` and `DescribeServiceJobResponse` each carry a `startedAt` that is modelled
//!   `@required` AND `@clientOptional` — Smithy's statement that a server legally omits it —
//!   so the member is held as an `Option` and written only for a job whose run began;
//! * `ContainerProperties` reports the two deprecated resource reservations whatever the
//!   registration supplied, as an explicit null where it supplied neither;
//! * `NetworkConfiguration` reports an interface-configuration list the published model
//!   snapshot does not declare at all.
//!
//! A hand-written declaration can drift from the model, and a model refresh is exactly when
//! it does: a refreshed snapshot that adds a member, drops one or renames one leaves the
//! hand-written schema speaking the old member set, and no other suite would notice, because
//! a member a schema does not carry reads on the wire exactly like a member the record does
//! not hold.
//!
//! So the rule under test, over the SET of hand-declared shapes rather than any one of them:
//! a hand-written schema carries exactly the members the model declares for its shape — plus,
//! for the one member AWS reports that the snapshot does not declare, that member alone, with
//! its shape id outside the service's own namespace so it cannot claim to be modelled. The
//! second suite drives the same shapes over the wire, where every member a read reports must
//! be one the model declares.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::ops::types::{
    CONTAINER_PROPERTIES_SCHEMA, DESCRIBE_SERVICE_JOB_OUTPUT_SCHEMA, JOB_DETAIL_SCHEMA,
    NETWORK_CONFIGURATION_SCHEMA,
};

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue, Batch,
};

/// The published model snapshot this crate is generated from, embedded at compile time so the
/// hand-written declarations are checked against the model itself rather than a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-batch-awsbatchv20160810.json"
));

/// This service's own namespace: where a member the model DOES declare has its shape id.
const MODELLED_NAMESPACE: &str = "com.amazonaws.batch";

/// The namespace the one unmodelled member's shape id sits in.
const RECORDED_NAMESPACE: &str = "com.amazonaws.batch.recorded";

/// The member AWS reports on `NetworkConfiguration` that the snapshot does not declare, as an
/// empty list where nothing holds an entry.
const RECORDED_MEMBER: &str = "interfaceConfigurations";

/// The two `ContainerProperties` members a read reports whatever the registration supplied.
/// Source: the model's own shape — its two `@deprecated` integer members.
const DEPRECATED_RESERVATIONS: &[&str] = &["vcpus", "memory"];

/// An execution role, which a job on Fargate resources requires.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/execution";

/// Every shape this crate declares by hand: its shape id, the schema written out for it, and
/// the members it carries that the snapshot does not declare.
///
/// The schemas are reached through the `&'static Schema` statics each declaration publishes,
/// which is what the framework itself serializes against.
#[allow(clippy::type_complexity)]
fn hand_written_shapes() -> Vec<(
    &'static str,
    &'static [&'static aws_smithy_schema::Schema],
    &'static [&'static str],
)> {
    vec![
        (
            "com.amazonaws.batch#JobDetail",
            JOB_DETAIL_SCHEMA.members(),
            &[],
        ),
        (
            "com.amazonaws.batch#DescribeServiceJobResponse",
            DESCRIBE_SERVICE_JOB_OUTPUT_SCHEMA.members(),
            &[],
        ),
        (
            "com.amazonaws.batch#ContainerProperties",
            CONTAINER_PROPERTIES_SCHEMA.members(),
            &[],
        ),
        (
            "com.amazonaws.batch#NetworkConfiguration",
            NETWORK_CONFIGURATION_SCHEMA.members(),
            &[RECORDED_MEMBER],
        ),
    ]
}

/// The member names the snapshot declares for `shape_id`.
fn modelled_members(shape_id: &str) -> BTreeSet<String> {
    let model: Value = serde_json::from_str(SNAPSHOT).expect("the model snapshot must be JSON");
    let shape = &model["shapes"][shape_id];
    assert!(
        !shape.is_null(),
        "the snapshot no longer declares {shape_id}, so nothing can be declared from it"
    );
    shape["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the snapshot must declare {shape_id} with members"))
        .keys()
        .cloned()
        .collect()
}

/// The local name of a shape id: what each of its members' shape ids is prefixed with.
fn local_name(shape_id: &str) -> &str {
    shape_id
        .split_once('#')
        .map(|(_, local)| local)
        .unwrap_or(shape_id)
}

/// The keys of a reported JSON object.
fn reported_members(reported: &Value) -> BTreeSet<String> {
    reported
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// RULE: a hand-written schema carries exactly the members the model declares for its shape,
/// plus the members it documents as unmodelled and nothing else; each member sits at its own
/// index, and its shape id names the namespace it belongs to — the service's own for a
/// modelled member, a namespace of its own for one the model does not declare.
///
/// The rows are the hand-declared shapes themselves: they differ along how many members each
/// carries, along whether the shape appends an unmodelled member at all, and along what kind
/// of declaration it is — an operation output, a shape nested in one, and a shape that is both
/// nested in an output and reached by an operation INPUT.
#[test]
fn every_hand_written_schema_carries_the_models_member_list() {
    for (case, members, unmodelled) in hand_written_shapes() {
        let declared = modelled_members(case);
        for added in unmodelled {
            assert!(
                !declared.contains(*added),
                "case {case}: the snapshot now declares {added}, so the hand-written \
                 declaration should give way to the modelled member; declared {declared:?}"
            );
        }
        let expected: BTreeSet<String> = declared
            .iter()
            .cloned()
            .chain(unmodelled.iter().map(|name| (*name).to_string()))
            .collect();
        let carried: BTreeSet<String> = members
            .iter()
            .map(|member| member.member_name().unwrap_or("<unnamed>").to_string())
            .collect();
        assert_eq!(
            carried, expected,
            "case {case}: expected the schema to carry {expected:?}, got {carried:?}"
        );
        assert_eq!(
            members.len(),
            expected.len(),
            "case {case}: expected {} members, got {} — a member is declared twice",
            expected.len(),
            members.len()
        );

        for (position, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {case}.{name}: expected member_index {position}, got {:?}",
                member.member_index()
            );
            let namespace = if unmodelled.contains(&name) {
                RECORDED_NAMESPACE
            } else {
                MODELLED_NAMESPACE
            };
            let expected_id = format!("{namespace}#{}${name}", local_name(case));
            assert_eq!(
                member.shape_id().to_string(),
                expected_id,
                "case {case}.{name}: expected the shape id {expected_id}, got {}",
                member.shape_id()
            );
            assert_eq!(
                member.shape_id().namespace(),
                namespace,
                "case {case}.{name}: expected the namespace {namespace}, got {}",
                member.shape_id().namespace()
            );
        }
    }
}

/// RULE: every member a read reports for one of these shapes is a member the model declares —
/// plus the one member AWS reports that the snapshot does not — and the members the
/// hand-written serializers write unconditionally are on the wire, while the instant of a
/// transition that has not happened is not.
///
/// The rows differ along the registration the read is of: an Amazon EC2 container that named
/// no network configuration and no reservation, a Fargate container that named a network
/// configuration, and an Amazon EC2 container that named both deprecated reservations.
#[tokio::test]
async fn a_read_reports_only_members_the_model_declares() {
    let batch = Batch::new();
    // (case, the registration body, minus the name this suite fills in)
    let rows: Vec<(&str, Value)> = vec![
        (
            "no network configuration and no reservation",
            json!({"type": "container", "containerProperties": container_properties()}),
        ),
        (
            "a network configuration",
            json!({
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": {
                    "image": "public.ecr.aws/amazonlinux/amazonlinux",
                    "executionRoleArn": EXECUTION_ROLE,
                    "resourceRequirements": [
                        {"type": "VCPU", "value": "0.25"},
                        {"type": "MEMORY", "value": "512"},
                    ],
                    "networkConfiguration": {"assignPublicIp": "DISABLED"},
                },
            }),
        ),
        (
            "both deprecated reservations",
            json!({
                "type": "container",
                "containerProperties": {"image": "busybox", "vcpus": 2, "memory": 4096},
            }),
        ),
    ];

    let container_declared = modelled_members("com.amazonaws.batch#ContainerProperties");
    let network_declared = modelled_members("com.amazonaws.batch#NetworkConfiguration");

    for (index, (case, registration)) in rows.iter().enumerate() {
        let name = format!("jd-model-members-{index}");
        let mut request = registration.clone();
        request["jobDefinitionName"] = json!(&name);
        batch.ok("/v1/registerjobdefinition", request).await;
        let described = batch
            .ok(
                "/v1/describejobdefinitions",
                json!({"jobDefinitionName": &name}),
            )
            .await;
        let reported = described["jobDefinitions"][0]["containerProperties"].clone();
        for member in reported_members(&reported) {
            assert!(
                container_declared.contains(&member),
                "case {case:?}: {member:?} is not a member the model declares for \
                 ContainerProperties, got {reported}"
            );
        }
        for member in DEPRECATED_RESERVATIONS {
            assert!(
                reported.get(*member).is_some(),
                "case {case:?}: the deprecated reservation {member} is reported whatever the \
                 registration supplied, got {reported}"
            );
        }

        let network = &reported["networkConfiguration"];
        if network.is_null() {
            continue;
        }
        for member in reported_members(network) {
            assert!(
                network_declared.contains(&member) || member == RECORDED_MEMBER,
                "case {case:?}: {member:?} is not a member the model declares for \
                 NetworkConfiguration, got {network}"
            );
        }
        assert_eq!(
            network.get(RECORDED_MEMBER),
            Some(&json!([])),
            "case {case:?}: expected {RECORDED_MEMBER} [], got {network}"
        );
    }

    // A described JOB, over the same rule: the members its record holds are reported under the
    // names the model gives them, and the instant of a transition it never made is not.
    let environment = compute_environment(&batch, "ce-model-members").await;
    job_queue(&batch, "q-model-members", &environment).await;
    job_definition(&batch, "jd-model-members-job", container_properties()).await;
    let submitted = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "j-model-members",
                "jobQueue": "q-model-members",
                "jobDefinition": "jd-model-members-job",
            }),
        )
        .await;
    let job_id = submitted["jobId"].as_str().unwrap_or_default().to_string();
    let detail = describe_job(&batch, &job_id).await;
    let job_declared = modelled_members("com.amazonaws.batch#JobDetail");
    for member in reported_members(&detail) {
        assert!(
            job_declared.contains(&member),
            "case a described job: {member:?} is not a member the model declares for \
             JobDetail, got {detail}"
        );
    }
    // The members the hand-written serializer writes unconditionally: a described job carries
    // each of them, whatever has happened to it.
    for member in ["jobName", "jobId", "jobQueue", "status", "jobDefinition"] {
        assert!(
            detail.get(member).is_some(),
            "case a described job: {member} is written for every described job, got {detail}"
        );
    }
    // Nothing placed this job — the compute environment is UNMANAGED — so the `STARTING` ->
    // `RUNNING` transition never happened and its instant is absent rather than zero.
    assert!(
        detail.get("startedAt").is_none(),
        "case a described job: a job nothing started reports no startedAt, got {detail}"
    );
}
