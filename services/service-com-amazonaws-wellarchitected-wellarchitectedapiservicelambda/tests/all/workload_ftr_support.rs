// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `Workload$FTRSupported`, the flag saying whether a workload
//! is supported for a Foundational Technical Review.
//!
//! The rule pinned here, over a TABLE of workloads and of the reads that project them
//! rather than one recorded request: every projection of a stored workload REPORTS the
//! flag — `false` for a workload that is not FTR-supported, rather than leaving the member
//! out — so a caller reading it to decide whether to offer the workflow can tell "not
//! supported" from "unknown". No operation this service exposes enrols a workload in that
//! review, so no workload this emulator holds is FTR-supported.
//!
//! The member is absent from the upstream model snapshot and is declared from the overlay
//! model beside it (`smithy-model/LOCAL-ADDITIONS.md`); see `Workload::ftr_supported` in
//! `src/ops/types/workload_projection_types.rs`. `super::workload_projection_members` pins
//! that declaration against the snapshot's own member list.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// The projected `Workload` of an answer, which every row must carry.
fn workload<'a>(case: &str, answer: &'a Answer, member: &str) -> &'a Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {answer:?}"
    );
    answer
        .body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry {member}, got {answer:?}"))
}

/// `FTRSupported` is PRESENT and `false` — asserted on the raw JSON, so a member the
/// serializer omitted fails here rather than reading as `false`.
fn assert_not_ftr_supported(case: &str, projected: &Value) {
    assert_eq!(
        projected.get("FTRSupported"),
        Some(&Value::Bool(false)),
        "case {case:?}: FTRSupported must be reported as false, got {projected:?}"
    );
}

/// One row of the table: how the workload under test is created.
struct Case {
    case: &'static str,
    /// Members merged over [`minimal_create`].
    overrides: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "minimal",
            overrides: json!({}),
        },
        Case {
            case: "two-lenses",
            overrides: json!({ "Lenses": ["wellarchitected", "serverless"] }),
        },
        Case {
            case: "no-lenses",
            overrides: json!({ "Lenses": [] }),
        },
        Case {
            case: "tagged-and-annotated",
            overrides: json!({
                "Tags": { "Env": "prod" },
                "Notes": "notes the workload carries",
                "Industry": "Financial Services",
                "IndustryType": "Financial Services",
            }),
        },
        Case {
            case: "pre-production-with-non-aws-regions",
            overrides: json!({
                "Environment": "PREPRODUCTION",
                "NonAwsRegions": ["on-premises-dc-1"],
            }),
        },
    ]
}

fn create_request(case: &Case, index: usize) -> Value {
    let mut request = minimal_create(&format!("ftr-workload-{index}"), &format!("tok-ftr-{index}"));
    if let Some(overrides) = case.overrides.as_object() {
        for (member, value) in overrides {
            request[member.as_str()] = value.clone();
        }
    }
    request
}

/// Every read that projects a stored workload reports the flag: the `GetWorkload` that
/// reads it back, the `UpdateWorkload` that revises it, and the `GetMilestone` that reports
/// the workload a milestone froze.
#[tokio::test]
async fn every_workload_projection_reports_whether_it_is_ftr_supported() {
    let client = Client::new();

    for (index, case) in cases().iter().enumerate() {
        let created = client.create(create_request(case, index)).await;
        let workload_id = created_id(&created, case.case);

        let fetched = client.get(&workload_id).await;
        assert_not_ftr_supported(
            &format!("{}/GetWorkload", case.case),
            workload(case.case, &fetched, "Workload"),
        );

        let revised = client
            .update(
                &workload_id,
                json!({ "Description": "a description this update writes" }),
            )
            .await;
        assert_not_ftr_supported(
            &format!("{}/UpdateWorkload", case.case),
            workload(case.case, &revised, "Workload"),
        );

        let recorded = client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/milestones"),
                Some(json!({
                    "MilestoneName": format!("ftr-milestone-{index}"),
                    "ClientRequestToken": format!("tok-ftr-ms-{index}"),
                })),
            )
            .await;
        assert_eq!(
            recorded.status, 200,
            "case {:?}: CreateMilestone must succeed, got {recorded:?}",
            case.case
        );
        let milestone_number = recorded
            .body
            .get("MilestoneNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let milestone = client
            .send(
                "GET",
                &format!("/workloads/{workload_id}/milestones/{milestone_number}"),
                None,
            )
            .await;
        let frozen = workload(case.case, &milestone, "Milestone");
        assert_not_ftr_supported(
            &format!("{}/GetMilestone", case.case),
            frozen
                .get("Workload")
                .unwrap_or_else(|| panic!("case {:?}: the milestone must carry the workload it froze, got {milestone:?}", case.case)),
        );
    }
}
