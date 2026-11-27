// SPDX-License-Identifier: CC0-1.0

//! What `UpdatePlan` does when several callers write one plan at the same time.
//!
//! The rule under test: a plan is written in whole revisions. Every `UpdatePlan` against
//! one plan is either APPLIED whole — every mutable member of that one request, at one new
//! revision — or refused with the operation's own modelled conflict,
//! `RetryableConflictException` (HTTP 409, "the request conflicted with a concurrent
//! modification of the same resource and did not take effect",
//! `smithy_query(query="operation", operation="UpdatePlan", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
//! Two writers can never leave the plan carrying one caller's workflows beside another
//! caller's execution role, the plan's `version` advances by exactly one per accepted
//! write, and a plan that has been written concurrently is still writable afterwards.
//!
//! The rows differ along how many callers write at once; each writer states every mutable
//! member it can, keyed to itself, so a mixed record is visible in the response.
//!
//! The request/response helpers are shared with [`crate::plan_lifecycle`]: every case
//! drives the emulator through the same `MultiServiceRouter::dispatch` the router serves
//! on a socket (`awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::plan_lifecycle::{
    call, create_plan, create_request, default_workflows, error_code, router, workflow, ACCOUNT,
};

/// How many callers race in each row.
const WRITER_COUNTS: [usize; 3] = [2, 3, 8];

/// The members of one writer's update, each keyed to the writer, so the stored plan says
/// which writer's request it holds — and says so for every member, not just one.
fn writer_update(arn: &str, writer: usize) -> Value {
    let step = format!("step-of-writer-{writer}");
    json!({
        "arn": arn,
        "description": format!("writer-{writer}"),
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/Writer{writer}"),
        "workflows": [workflow("activate", &[step.as_str()])],
        // Inside `recoveryTimeObjectiveMinutes`'s modelled @range(1, 10080).
        "recoveryTimeObjectiveMinutes": writer + 1,
        "serviceQuotaChecksEnabled": writer.is_multiple_of(2),
    })
}

/// The mutable members every writer states, so a record mixing two writers' requests fails
/// the comparison whichever member it mixed.
const WRITTEN_MEMBERS: &[&str] = &[
    "description",
    "executionRole",
    "workflows",
    "recoveryTimeObjectiveMinutes",
    "serviceQuotaChecksEnabled",
];

/// The plan's server-owned `version`, as the revision number the model reports as a string.
fn version_of(case: &str, plan: &Value) -> u64 {
    let raw = plan["version"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a version string, got {plan}"));
    raw.parse::<u64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric version, got {raw:?}"))
}

/// Which writer's request a stored plan holds, read from the one member that names it.
fn writer_of(case: &str, plan: &Value) -> usize {
    let description = plan["description"].as_str().unwrap_or_default();
    description
        .strip_prefix("writer-")
        .and_then(|index| index.parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the stored plan must carry one writer's description, got {plan}"
            )
        })
}

/// Racing updates of one plan each land whole or are refused with the modelled conflict:
/// the plan ends as exactly one writer's request, at one revision per accepted write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_plan_updates_land_whole_revisions_or_the_modelled_conflict() {
    for writers in WRITER_COUNTS {
        let case = format!("{writers} writers at once");
        let router = Arc::new(router());
        let arn = create_plan(&router, "raced", ["us-east-1", "us-west-2"]).await;

        let mut racers = Vec::with_capacity(writers);
        for writer in 0..writers {
            let router = Arc::clone(&router);
            let request = writer_update(&arn, writer);
            racers.push(tokio::spawn(async move {
                (writer, call(&router, "UpdatePlan", &request).await)
            }));
        }

        let mut accepted: Vec<usize> = Vec::with_capacity(writers);
        for racer in racers {
            let (writer, (status, body)) = racer.await.expect("the racing update task completes");
            if status == 200 {
                // An accepted write answers with the plan it stored — all of its own
                // members, none of another writer's.
                let answered = body["plan"].clone();
                let expected = writer_update(&arn, writer);
                for member in WRITTEN_MEMBERS {
                    assert_eq!(
                        answered[member], expected[member],
                        "case {case:?}: writer {writer} was accepted, so member {member} must \
                         be {}, got {}",
                        expected[member], answered[member]
                    );
                }
                accepted.push(writer);
            } else {
                // The only other answer the model gives this operation for a write that
                // did not take effect.
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (409, "RetryableConflictException"),
                    "case {case:?}: writer {writer} was not accepted, so it must be the \
                     modelled 409 RetryableConflictException, got {status} with {body}"
                );
            }
        }
        assert!(
            !accepted.is_empty(),
            "case {case:?}: at least one racing UpdatePlan must be accepted, none was"
        );

        // The plan a later caller reads is ONE accepted writer's whole request.
        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        let stored = fetched["plan"].clone();
        let landed = writer_of(&case, &stored);
        assert!(
            accepted.contains(&landed),
            "case {case:?}: the stored plan holds writer {landed}, which was not among the \
             accepted writers {accepted:?}: {stored}"
        );
        let expected = writer_update(&arn, landed);
        for member in WRITTEN_MEMBERS {
            assert_eq!(
                stored[member], expected[member],
                "case {case:?}: the stored plan holds writer {landed}, so member {member} \
                 must be {}, got {}",
                expected[member], stored[member]
            );
        }

        // One revision per accepted write, counted from the revision the create made.
        let version = version_of(&case, &stored);
        let expected_version = 1 + accepted.len() as u64;
        assert_eq!(
            version, expected_version,
            "case {case:?}: {} accepted writes after the create expected version \
             {expected_version}, got {version}",
            accepted.len()
        );

        // Nothing is left holding the plan: the next writer is accepted and lands.
        let after = writers + 1;
        let (status, body) = call(&router, "UpdatePlan", &writer_update(&arn, after)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update after the race expected 200, got {status} with {body}"
        );
        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan after the race expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            writer_of(&case, &fetched["plan"]),
            after,
            "case {case:?}: the update after the race must be what the plan holds, got {}",
            fetched["plan"]
        );
        assert_eq!(
            version_of(&case, &fetched["plan"]),
            expected_version + 1,
            "case {case:?}: the update after the race must be one more revision, got {}",
            fetched["plan"]
        );
    }
}

/// The identity of the plan is untouched by a race: whichever writer lands, the members
/// `UpdatePlanRequest` does not carry are still the created plan's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_plan_updates_leave_the_plans_identity_alone() {
    /// The `Plan` members no `UpdatePlan` request carries, from the operation's input
    /// (`smithy_query(query="operation", operation="UpdatePlan", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
    const IMMUTABLE_MEMBERS: &[&str] = &[
        "arn",
        "name",
        "regions",
        "recoveryApproach",
        "primaryRegion",
        "owner",
    ];

    for writers in WRITER_COUNTS {
        let case = format!("{writers} writers at once");
        let router = Arc::new(router());
        // Created WITH a primary Region, so every member under test carries a value of
        // its own rather than being absent on both sides of the comparison.
        let mut request = create_request(
            "identity",
            ["eu-west-1", "eu-central-1"],
            "activePassive",
            default_workflows(),
        );
        request["primaryRegion"] = json!("eu-west-1");
        let (status, created) = call(&router, "CreatePlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePlan expected 200, got {status} with {created}"
        );
        let created = created["plan"].clone();
        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        for member in IMMUTABLE_MEMBERS {
            assert!(
                !created[member].is_null(),
                "case {case:?}: the created plan must carry {member}, got {created}"
            );
        }

        let mut racers = Vec::with_capacity(writers);
        for writer in 0..writers {
            let router = Arc::clone(&router);
            let request = writer_update(&arn, writer);
            racers.push(tokio::spawn(async move {
                call(&router, "UpdatePlan", &request).await
            }));
        }
        for racer in racers {
            let (status, body) = racer.await.expect("the racing update task completes");
            assert!(
                status == 200
                    || (status, error_code(&body).as_str()) == (409, "RetryableConflictException"),
                "case {case:?}: a racing UpdatePlan is accepted or the modelled 409 \
                 RetryableConflictException, got {status} with {body}"
            );
        }

        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        for member in IMMUTABLE_MEMBERS {
            assert_eq!(
                fetched["plan"][member], created[member],
                "case {case:?}: member {member} expected the created plan's {}, got {}",
                created[member], fetched["plan"][member]
            );
        }
    }
}
