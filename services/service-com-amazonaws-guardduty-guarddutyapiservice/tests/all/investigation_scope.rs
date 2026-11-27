// SPDX-License-Identifier: CC0-1.0

//! WHAT one investigation covered — `Investigation.Scope`, the `InvestigationSummary`
//! members that report it, and the `SummaryFormat` the summary is served in — end to end
//! over the wire (the harness lives in the `detector_lifecycle` suite).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. An ended pass reports the SUBJECT it resolved: the narrowest one the prompt names
//!    against the account's own records. A prompt naming a finding the account holds is a
//!    `FINDING` investigation of that finding; one naming none is an investigation of the
//!    account, or of the organization when the detector administers member accounts.
//!    "Only the identifier that matches `InvestigationType` is set"
//!    (`smithy_query(query="doc",shape="InvestigationScope")`), and the window the pass
//!    queried is set for exactly the two account-wide types.
//! 2. `Scope` is "returned only when the investigation status is COMPLETED", so an
//!    investigation still running reports none — and neither does `ListInvestigations`
//!    report the members it sets.
//! 3. `ListInvestigations` and `GetInvestigation` report ONE scope: the summary's
//!    `investigationType`, `findingId`, `accountId` and `adminAccountId` are the scope the
//!    read reports under `scope`.
//! 4. "When you set `SummaryFormat` to `STRUCTURED`, the summary is returned in
//!    `SummaryData` instead" and "when this field is returned, `Summary` isn't returned"
//!    (`smithy_query(query="doc",shape="Investigation",member="SummaryData")`), so the two
//!    formats are one conclusion and exactly one of them is served. The parts of the
//!    structured summary follow the investigation type, and its counts are the findings
//!    the pass analysed.

use std::time::Duration;

use serde_json::{json, Value};

use crate::detector_lifecycle::{all_finding_ids, call, investigation_detector, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    administrator_account_id, INVESTIGATION_RUNNING,
};

/// How long a poll may wait for the clock to end an analysis before the test gives up.
/// Generous on purpose: the loop ends on the CONDITION it looks for, never on a duration,
/// so a loaded host takes more attempts rather than a different outcome.
const ENDED_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll pauses between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `InvestigationType` members (`smithy_query(shape="InvestigationType")`), paired
/// with the scope identifier each one sets and the ones it must leave unset.
const TYPE_IDENTIFIERS: [(&str, &str); 3] = [
    ("FINDING", "findingId"),
    ("ACCOUNT", "accountId"),
    ("ORGANIZATION", "adminAccountId"),
];

/// Create one sample finding of `finding_type` on `detector_id`.
async fn create_sample(router: &Router, detector_id: &str, finding_type: &str, case: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": [finding_type]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-sample-findings expected 200, got {status} {body:?}"
    );
}

/// Record one member account on `detector_id`, so the account administers an
/// organization.
async fn create_member(router: &Router, detector_id: &str, account_id: &str, case: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member"),
        Some(json!({"accountDetails": [{"accountId": account_id, "email": "member@example.com"}]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-members expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["unprocessedAccounts"],
        json!([]),
        "case {case:?}: expected the member to be recorded, got {body:?}"
    );
}

/// Initiate one investigation with `prompt` and return its id.
async fn initiate(router: &Router, detector_id: &str, prompt: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({"triggerPrompt": prompt})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-investigation expected 200, got {status} {body:?}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {body:?}"))
        .to_string()
}

/// One `GetInvestigation` read, with `query` appended to the path.
async fn read(router: &Router, detector_id: &str, id: &str, query: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/investigation/{id}{query}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-investigation expected 200, got {status} {body:?}"
    );
    body["investigation"].clone()
}

/// Read the investigation until its pass has ended, or until [`ENDED_DEADLINE`] passes.
async fn ended(router: &Router, detector_id: &str, id: &str, query: &str, case: &str) -> Value {
    let deadline = std::time::Instant::now() + ENDED_DEADLINE;
    loop {
        let investigation = read(router, detector_id, id, query, case).await;
        if investigation["status"] != json!(INVESTIGATION_RUNNING) {
            return investigation;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the analysis never ended within {ENDED_DEADLINE:?}, last read \
             {investigation}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `InvestigationSummary` `ListInvestigations` reports for `id`.
async fn summary_of(router: &Router, detector_id: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation/list"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-investigations expected 200, got {status} {body:?}"
    );
    body["investigations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["investigationId"].as_str() == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {id} is not listed at all, got {body:?}"))
}

/// Assert the scope sets the identifier its type names and NO other, and that the window
/// is set for exactly the two account-wide types.
fn assert_one_identifier(scope: &Value, expected_type: &str, case: &str) {
    assert_eq!(
        scope["investigationType"],
        json!(expected_type),
        "case {case:?}: expected the {expected_type} scope, got {scope:?}"
    );
    for (investigation_type, identifier) in TYPE_IDENTIFIERS {
        let set = !scope[identifier].is_null();
        assert_eq!(
            set,
            investigation_type == expected_type,
            "case {case:?}: only the identifier of the {expected_type} scope is set, so \
             {identifier} must be {}, got {scope:?}",
            if investigation_type == expected_type {
                "set"
            } else {
                "absent"
            }
        );
    }
    let window = scope["queryTimeWindow"].clone();
    let account_wide = expected_type != "FINDING";
    assert_eq!(
        !window.is_null(),
        account_wide,
        "case {case:?}: the queried window is set for an account-wide scope only, got {scope:?}"
    );
    if account_wide {
        let start = window["startTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a window startTime, got {window:?}")
        });
        let end = window["endTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a window endTime, got {window:?}"));
        assert!(
            start > 0.0 && end > start,
            "case {case:?}: expected a window that starts before it ends, got {window:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 1 and 3.
// ---------------------------------------------------------------------------

/// The subject an ended pass resolved, over the table of subjects a request can ask
/// about: the rows differ along what the prompt names (nothing of the account's, a
/// finding it holds) and along what the detector holds (no findings, findings, member
/// accounts).
#[tokio::test]
async fn an_ended_pass_reports_the_subject_it_resolved() {
    // (case, the sample finding types the detector holds, whether it administers a
    // member account, whether the prompt names a stored finding, the expected type)
    let rows: [(&str, &[&str], bool, bool, &str); 5] = [
        ("an account holding nothing", &[], false, false, "ACCOUNT"),
        (
            "an account holding findings",
            &["Recon:EC2/Portscan", "Impact:EC2/PortSweep"],
            false,
            false,
            "ACCOUNT",
        ),
        (
            "a prompt naming one of the account's findings",
            &["Recon:EC2/Portscan", "Impact:EC2/PortSweep"],
            false,
            true,
            "FINDING",
        ),
        (
            "an account administering a member account",
            &["Recon:EC2/Portscan"],
            true,
            false,
            "ORGANIZATION",
        ),
        (
            "a prompt naming a finding of an account administering a member",
            &["Recon:EC2/Portscan"],
            true,
            true,
            "FINDING",
        ),
    ];

    for (case, finding_types, with_member, name_a_finding, expected_type) in rows {
        // A store per row, so one row's findings and membership cannot explain
        // another's scope.
        let router = new_router();
        let detector_id = investigation_detector(&router).await;
        for finding_type in finding_types {
            create_sample(&router, &detector_id, finding_type, case).await;
        }
        if with_member {
            create_member(&router, &detector_id, "700000000001", case).await;
        }
        let findings = all_finding_ids(&router, &detector_id, 50).await;
        let named = match name_a_finding {
            true => findings
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: the row needs a finding to name")),
            false => String::new(),
        };
        let prompt = match name_a_finding {
            true => format!("Investigate the finding {named}"),
            false => "Investigate the threat posture here".to_string(),
        };

        let id = initiate(&router, &detector_id, &prompt, case).await;
        let investigation = ended(&router, &detector_id, &id, "", case).await;
        let scope = investigation["scope"].clone();
        assert!(
            !scope.is_null(),
            "case {case:?}: an ended pass reports the scope it resolved, got {investigation:?}"
        );
        assert_one_identifier(&scope, expected_type, case);

        let administrator = administrator_account_id();
        match expected_type {
            "FINDING" => assert_eq!(
                scope["findingId"],
                json!(named),
                "case {case:?}: expected the scope to name the finding the prompt did, got \
                 {scope:?}"
            ),
            "ACCOUNT" => assert_eq!(
                scope["accountId"],
                json!(administrator),
                "case {case:?}: expected the scope to name the investigated account, got {scope:?}"
            ),
            _ => assert_eq!(
                scope["adminAccountId"],
                json!(administrator),
                "case {case:?}: expected the scope to name the organization's administrator, got \
                 {scope:?}"
            ),
        }

        // Rule 3: the listing reports the SAME scope, member for member.
        let summary = summary_of(&router, &detector_id, &id, case).await;
        assert_eq!(
            summary["investigationType"], scope["investigationType"],
            "case {case:?}: the listing and the read report one investigationType, got {summary:?} \
             and {scope:?}"
        );
        assert_eq!(
            summary["findingId"], scope["findingId"],
            "case {case:?}: the listing and the read report one findingId, got {summary:?} and \
             {scope:?}"
        );
        assert_eq!(
            summary["adminAccountId"], scope["adminAccountId"],
            "case {case:?}: the listing and the read report one adminAccountId, got {summary:?} \
             and {scope:?}"
        );
        assert_eq!(
            summary["accountId"],
            json!(administrator),
            "case {case:?}: the listing reports the account the investigation belongs to, got \
             {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2.
// ---------------------------------------------------------------------------

/// An investigation whose pass has not ended has resolved no subject, so neither view
/// reports one.
#[tokio::test]
async fn a_running_investigation_reports_no_scope() {
    let case = "an investigation read straight after it was initiated";
    let router = new_router();
    let detector_id = investigation_detector(&router).await;
    create_sample(&router, &detector_id, "Recon:EC2/Portscan", case).await;

    let id = initiate(&router, &detector_id, "Investigate this account", case).await;
    let investigation = read(&router, &detector_id, &id, "", case).await;
    assert_eq!(
        investigation["status"],
        json!(INVESTIGATION_RUNNING),
        "case {case:?}: expected the transient status, got {investigation:?}"
    );
    assert!(
        investigation["scope"].is_null(),
        "case {case:?}: a pass that has not ended has resolved no scope, got {investigation:?}"
    );
    assert!(
        investigation["summaryData"].is_null() && investigation["summary"].is_null(),
        "case {case:?}: a pass that has not ended has reached no summary, got {investigation:?}"
    );

    let summary = summary_of(&router, &detector_id, &id, case).await;
    for member in ["investigationType", "findingId", "adminAccountId"] {
        assert!(
            summary[member].is_null(),
            "case {case:?}: GuardDuty sets {member} when the investigation completes, so a \
             running one reports none, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4.
// ---------------------------------------------------------------------------

/// The summary is served in exactly ONE format, the one the request asks for, and the
/// structured form carries the parts its investigation type names.
#[tokio::test]
async fn the_summary_is_served_in_the_format_the_request_asks_for() {
    // (case, the summaryFormat query, whether the structured form is expected)
    let formats: [(&str, &str, bool); 3] = [
        ("no summaryFormat at all", "", false),
        ("summaryFormat=STRING", "?summaryFormat=STRING", false),
        (
            "summaryFormat=STRUCTURED",
            "?summaryFormat=STRUCTURED",
            true,
        ),
    ];
    // (case, the findings the detector holds, whether it administers a member, whether
    // the prompt names a finding, the structured part the type carries)
    let subjects: [(&str, &[&str], bool, bool, &str); 3] = [
        (
            "a finding-scoped investigation",
            &["Impact:EC2/PortSweep"],
            false,
            true,
            "countermeasures",
        ),
        (
            "an account-wide investigation",
            &["Recon:EC2/Portscan", "Impact:EC2/PortSweep"],
            false,
            false,
            "accountAnalysis",
        ),
        (
            "an organization-wide investigation",
            &["Recon:EC2/Portscan"],
            true,
            false,
            "organizationAnalysis",
        ),
    ];

    for (subject_case, finding_types, with_member, name_a_finding, part) in subjects {
        let router = new_router();
        let detector_id = investigation_detector(&router).await;
        for finding_type in finding_types {
            create_sample(&router, &detector_id, finding_type, subject_case).await;
        }
        if with_member {
            create_member(&router, &detector_id, "700000000002", subject_case).await;
        }
        let findings = all_finding_ids(&router, &detector_id, 50).await;
        let prompt = match name_a_finding {
            true => format!("Investigate the finding {}", findings[0]),
            false => "Investigate the threat posture here".to_string(),
        };
        let id = initiate(&router, &detector_id, &prompt, subject_case).await;

        for (format_case, query, structured) in formats {
            let case = format!("{subject_case}, {format_case}");
            let investigation = ended(&router, &detector_id, &id, query, &case).await;
            assert_eq!(
                !investigation["summaryData"].is_null(),
                structured,
                "case {case:?}: the structured summary is served for STRUCTURED alone, got \
                 {investigation:?}"
            );
            assert_eq!(
                !investigation["summary"].is_null(),
                !structured,
                "case {case:?}: exactly one of summary and summaryData is served, got \
                 {investigation:?}"
            );
            if !structured {
                continue;
            }

            let data = investigation["summaryData"].clone();
            assert!(
                !data[part].is_null(),
                "case {case:?}: the structured summary of this type carries {part}, got {data:?}"
            );
            // The parts of the OTHER types are not reported for this one.
            for other in ["countermeasures", "accountAnalysis", "organizationAnalysis"] {
                if other == part {
                    continue;
                }
                assert!(
                    data[other].is_null(),
                    "case {case:?}: the structured summary of this type carries no {other}, got \
                     {data:?}"
                );
            }
            // Every structured summary carries the pass's key observations, and its
            // counts are the findings the pass analysed.
            assert!(
                !data["keyObservations"]["title"].is_null(),
                "case {case:?}: expected the pass's key observations, got {data:?}"
            );
            let expected_findings = match name_a_finding {
                true => 1,
                false => findings.len(),
            };
            if let Some(summary) = data[part]["findingsSummary"].as_object() {
                assert_eq!(
                    summary["totalFindings"],
                    json!(expected_findings),
                    "case {case:?}: expected the analysed findings to be counted, got {data:?}"
                );
            }
            if name_a_finding {
                assert!(
                    !data["findingType"].is_null(),
                    "case {case:?}: a finding-scoped summary names the analysed finding's type, \
                     got {data:?}"
                );
            }
        }
    }
}
