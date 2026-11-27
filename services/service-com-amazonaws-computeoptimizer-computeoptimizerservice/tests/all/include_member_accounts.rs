// SPDX-License-Identifier: CC0-1.0

//! The organization-wide `includeMemberAccounts` member, and the `accountIds` that names one
//! member account of the same organization, end to end over the wire.
//!
//! Nine operations of this service take `includeMemberAccounts` — the eight
//! `Export*Recommendations` surfaces and `UpdateEnrollmentStatus` — and every one of them
//! conditions it on the CALLER's role: the
//! member asks for the resources, or the enrollment, of "all member accounts of the
//! organization", which only an organization's management account or delegated administrator
//! has. So a caller that manages no organization asking for them is refused with the modelled
//! 400 `InvalidParameterValueException` and nothing is written, while the same call that does
//! not ask for them is admitted, and a caller whose organization this service records has the
//! member honoured.
//!
//! `accountIds` is the OTHER side of the same organization: it is how a management account
//! "specif[ies] the member account for which you want to return recommendations", so the
//! accounts any surface of this service answers for are the caller's own and the member
//! accounts the store records — the very accounts `GetEnrollmentStatusesForOrganization`
//! reports a row for. A request naming one of them is answered from THAT account's analysis
//! (none, here: this emulator records an analysis for the account it presents only), and only
//! an id naming an account outside that set is refused with the cross-account refusal.
//!
//! WHICH accounts an organization holds is recorded by AWS Organizations and no operation of
//! THIS API creates one, so the fixtures record member accounts into the store the way that
//! service would and then drive every call over the same `MultiServiceRouter` dispatch a
//! client uses. What is asserted is the RULE, over a table: one row per operation that takes
//! the member, crossed with the value it carries and with the organizations a caller can
//! manage.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    MemberAccountEnrollment, EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, JOB_STATUS_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_bucket, csv_rows, emulator_with_engine, error_code, export_destination,
    get_object, make_analysis_due, opt_in, record_organization, register_analysed_functions,
    ACCOUNT,
};

/// The refusal every operation answers a caller that asks for member accounts while managing
/// no organization with: the service's constant, which names neither the member nor the
/// account — it states the two reasons the caller cannot ask for them. Spelled out here rather
/// than imported so the WORDING is what this suite pins.
const INCLUDE_MEMBER_ACCOUNTS_MESSAGE: &str = "Cannot include member accounts. This could be \
     because you are not an AWS Organization user or you are not the master account in the \
     organization.";

/// The destination bucket the export rows below export to. Compute Optimizer does not create
/// it; the request just names it.
const BUCKET: &str = "member-account-exports";

/// One operation that takes `includeMemberAccounts`, and the rest of the request it needs.
struct Surface {
    /// The operation this row drives.
    operation: &'static str,
    /// The members the request carries BESIDES `includeMemberAccounts`: the required input of
    /// this operation's own surface.
    rest: fn() -> Value,
}

/// The body an export surface carries besides the member: a destination bucket, which "you
/// must create ... before you create the export job".
fn export_rest() -> Value {
    json!({"s3DestinationConfig": {"bucket": BUCKET}})
}

/// The body `UpdateEnrollmentStatus` carries besides the member: the `status` to write, which
/// a client may set to `Active` or `Inactive`.
fn enrollment_rest() -> Value {
    json!({"status": "Active"})
}

/// Every operation whose input declares `includeMemberAccounts`
/// (`smithy_query(query="operation",operation="ExportIdleRecommendations,UpdateEnrollmentStatus")`
/// and its export siblings): the eight export surfaces and the enrollment write.
fn surfaces() -> Vec<Surface> {
    let exports = [
        "ExportAutoScalingGroupRecommendations",
        "ExportEBSVolumeRecommendations",
        "ExportEC2InstanceRecommendations",
        "ExportECSServiceRecommendations",
        "ExportIdleRecommendations",
        "ExportLambdaFunctionRecommendations",
        "ExportLicenseRecommendations",
        "ExportRDSDatabaseRecommendations",
    ];
    let mut surfaces: Vec<Surface> = exports
        .into_iter()
        .map(|operation| Surface {
            operation,
            rest: export_rest,
        })
        .collect();
    surfaces.push(Surface {
        operation: "UpdateEnrollmentStatus",
        rest: enrollment_rest,
    });
    surfaces
}

/// One surface's request, carrying `include_member_accounts` — or, for `None`, not carrying
/// the member at all, which the model defaults to `false`.
fn request(surface: &Surface, include_member_accounts: Option<bool>) -> Value {
    let mut request = (surface.rest)();
    if let Some(include) = include_member_accounts {
        request["includeMemberAccounts"] = json!(include);
    }
    request
}

/// An emulator with the export destination created and the account opted in — what every
/// export call needs before the member itself is judged — and the engine it was built on, so a
/// row can record an organization the way AWS Organizations would.
async fn enrolled_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    register_analysed_functions(&router, &engine).await;
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    (router, engine)
}

/// The organizations a management account can be recorded with, spanning what the rule
/// generalises over — the organization's SIZE and the enrollment its members are in, neither of
/// which the member itself depends on: one member account, several, and one whose enrollment is
/// already settled at a status.
fn organizations() -> Vec<(&'static str, Vec<MemberAccountEnrollment>)> {
    let settled = |account_id: &str, status: &str| {
        let mut member = MemberAccountEnrollment::new(account_id);
        member.status = status.to_string();
        member
    };
    vec![
        (
            "one member account",
            vec![MemberAccountEnrollment::new("200000000001")],
        ),
        (
            "three member accounts",
            vec![
                MemberAccountEnrollment::new("200000000001"),
                MemberAccountEnrollment::new("200000000002"),
                MemberAccountEnrollment::new("200000000003"),
            ],
        ),
        (
            "a member account already opted in",
            vec![settled("200000000004", "Active")],
        ),
        (
            "a member account opted out",
            vec![settled("200000000005", "Inactive")],
        ),
    ]
}

/// The job ids `DescribeRecommendationExportJobs` reports, in the order it reported them.
async fn reported_jobs(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "DescribeRecommendationExportJobs", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the export jobs back: got {status}: {body}"
    );
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"))
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The account's own enrollment, as `GetEnrollmentStatus` reports it.
async fn enrollment(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = call(router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the enrollment back: got {status}: {body}"
    );
    body
}

/// Every operation that takes `includeMemberAccounts` refuses `true` from a caller that manages
/// no organization — with the same wording, the modelled 400, and without writing anything a
/// later caller could see.
#[tokio::test]
async fn member_accounts_are_refused_to_a_caller_that_manages_no_organization() {
    let (router, _engine) = enrolled_emulator().await;
    let before = enrollment(&router, "before any refusal").await;

    for surface in surfaces() {
        let asked = request(&surface, Some(true));
        let case = format!("{} with {asked}", surface.operation);
        let (status, body) = call(&router, surface.operation, asked).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE),
            "case {case:?}: expected the member-accounts refusal, got {body}"
        );
        assert!(
            body["jobId"].is_null(),
            "case {case:?}: a refused call answers no job id, got {body}"
        );
        assert!(
            body["status"].is_null(),
            "case {case:?}: a refused call answers no enrollment status, got {body}"
        );
    }

    // Not one of those calls wrote: no export job was queued and the account's own enrollment
    // is exactly where the opt-in left it.
    let jobs = reported_jobs(&router, "after every refusal").await;
    assert!(
        jobs.is_empty(),
        "a refused export records no job, got {jobs:?}"
    );
    let after = enrollment(&router, "after every refusal").await;
    assert_eq!(
        after, before,
        "a refused enrollment write must not move the stored record, got {after}"
    );
    assert_eq!(
        after["memberAccountsEnrolled"],
        json!(false),
        "a refused call never records member accounts as enrolled, got {after}"
    );
}

/// The same operations admit the call that does NOT ask for member accounts — `false`, and the
/// member left out altogether, which the model defaults to `false` — so the refusal is about
/// what the request asks for and not about the member being present.
#[tokio::test]
async fn a_call_that_asks_for_no_member_accounts_is_admitted() {
    for include_member_accounts in [Some(false), None] {
        for surface in surfaces() {
            // One emulator per admitted export: only one export job of a resource type may be
            // in progress per Region, so two rows of one surface cannot share a store.
            let (router, _engine) = enrolled_emulator().await;
            let asked = request(&surface, include_member_accounts);
            let case = format!("{} with {asked}", surface.operation);

            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be admitted, got {status}: {body}"
            );

            let after = enrollment(&router, &case).await;
            assert_eq!(
                after["memberAccountsEnrolled"],
                json!(false),
                "case {case:?}: a call that asks for no member accounts records none as \
                 enrolled, got {after}"
            );
        }
    }
}

/// A caller whose organization this service records has `includeMemberAccounts=true` honoured
/// by every one of those operations, whatever that organization holds: the export is queued and
/// reported, and the enrollment write records the member accounts as enrolled and reaches every
/// one of them.
#[tokio::test]
async fn member_accounts_are_honoured_for_a_caller_that_manages_an_organization() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);

        let mut admitted: Vec<String> = Vec::new();
        for surface in surfaces() {
            let asked = request(&surface, Some(true));
            let case = format!("{organization}: {} with {asked}", surface.operation);
            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be admitted, got {status}: {body}"
            );
            if let Some(job_id) = body["jobId"].as_str() {
                admitted.push(job_id.to_string());
            }
        }

        // The admitted exports are in the store — one per export surface, each the job its
        // call answered.
        let jobs = reported_jobs(&router, organization).await;
        assert_eq!(
            jobs, admitted,
            "case {organization:?}: the store holds exactly the admitted export jobs, got \
             {jobs:?}"
        );

        // The admitted enrollment write recorded the member accounts as enrolled, and the
        // status it asked for reached every member account the organization holds.
        let after = enrollment(&router, organization).await;
        assert_eq!(
            after["memberAccountsEnrolled"],
            json!(true),
            "case {organization:?}: an honoured call records the member accounts as enrolled, \
             got {after}"
        );
        let (status, body) = call(
            &router,
            "GetEnrollmentStatusesForOrganization",
            json!({"maxResults": 100}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {organization:?}: reading the organization back: got {status}: {body}"
        );
        let rows = body["accountEnrollmentStatuses"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {organization:?}: the reader reports a row list, got {body}")
            });
        assert_eq!(
            rows.len(),
            members.len(),
            "case {organization:?}: one row per member account, got {body}"
        );
        for row in rows {
            let reported = row["status"].as_str().unwrap_or_default();
            assert!(
                reported == "Active" || reported == "Pending",
                "case {organization:?}: the enrollment asked for reaches every member account \
                 (through Pending), got {row}"
            );
        }
    }
}

/// The member-accounts refusal precedes the enrollment gate every export surface applies: it is
/// about what the request asks for, not about what the store holds for the caller, so an
/// account that never opted in gets the same 400 rather than the 403 its own export gets.
#[tokio::test]
async fn the_member_accounts_refusal_precedes_the_opt_in_gate() {
    let (router, _engine) = emulator_with_engine();
    create_bucket(&router, BUCKET).await;

    for surface in surfaces() {
        if surface.operation == "UpdateEnrollmentStatus" {
            // The enrollment write is what opts an account IN, so it has no opt-in gate to
            // precede; its own refusal is asserted by the rows above.
            continue;
        }
        let asked = request(&surface, Some(true));
        let case = format!("not opted in: {} with {asked}", surface.operation);
        let (status, body) = call(&router, surface.operation, asked).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the member-accounts refusal, got {status}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE),
            "case {case:?}: expected the member-accounts refusal, got {body}"
        );
    }

    // Enrolling is what admits an export — and what admits the reader that reports them: the
    // store the refused calls left alone is empty.
    opt_in(&router).await;
    let jobs = reported_jobs(&router, "after every refusal").await;
    assert!(
        jobs.is_empty(),
        "a refused export records no job, got {jobs:?}"
    );
}

/// An `accountIds` naming the caller's own account is still mutually exclusive with the member:
/// "This parameter can't be specified together with the include member accounts parameter. The
/// parameters are mutually exclusive" — refused as a conflict rather than as the member's own
/// organization rule, whichever organization the caller manages.
#[tokio::test]
async fn the_mutual_exclusion_is_judged_before_the_organization() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);

        for surface in surfaces() {
            let mut asked = request(&surface, Some(true));
            if surface.operation == "UpdateEnrollmentStatus" {
                // `accountIds` is not a member of this surface at all.
                continue;
            }
            asked["accountIds"] = json!([ACCOUNT]);
            let case = format!("{organization}: {} with {asked}", surface.operation);

            let (status, body) = call(&router, surface.operation, asked).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the conflict to be refused, got {status}: {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(
                    "The value supplied for parameter accountIds is not valid: accountIds and \
                     includeMemberAccounts are mutually exclusive."
                ),
                "case {case:?}: expected the mutual-exclusion refusal, got {body}"
            );
        }
    }
}

// ===========================================================================
// `accountIds` naming a member account of the SAME organization
//
// `accountIds` is how a management account "specif[ies] the member account for which you want
// to return recommendations", so the accounts every surface of this service answers for are the
// caller's own and the member accounts the store records — the accounts
// `GetEnrollmentStatusesForOrganization` reports a row for and `includeMemberAccounts` covers.
// A request naming one of them is answered from THAT account's analysis, and only an id naming
// an account outside the set is refused.
// ===========================================================================

/// The refusal every surface answers a request about an account it answers nothing about with:
/// the service's constant, which names neither the member nor the account. Spelled out here
/// rather than imported so the WORDING is what this suite pins.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// How many times [`completed_job`] looks at an export job before giving up, one of the job's
/// own lifecycle steps apart: a deadline far past the steps a job takes, because what it waits
/// for is the job reaching its terminal status rather than a span of the clock.
const JOB_POLLS: usize = 200;

/// Every reader of this service that takes `accountIds`: the operation, the member of its
/// answer that carries what it reports, and whether the store this emulator starts with holds
/// an analysis of that resource type for the caller's own account (an Amazon ECS service
/// recommendation is computed over the ECS services the ECS emulator holds, and an Amazon
/// Aurora or RDS database recommendation over the DB instances the Amazon RDS emulator holds —
/// this suite creates neither).
const READERS: [(&str, &str, bool); 9] = [
    (
        "GetAutoScalingGroupRecommendations",
        "autoScalingGroupRecommendations",
        true,
    ),
    ("GetEBSVolumeRecommendations", "volumeRecommendations", true),
    (
        "GetEC2InstanceRecommendations",
        "instanceRecommendations",
        true,
    ),
    (
        "GetECSServiceRecommendations",
        "ecsServiceRecommendations",
        false,
    ),
    ("GetIdleRecommendations", "idleRecommendations", true),
    (
        "GetLambdaFunctionRecommendations",
        "lambdaFunctionRecommendations",
        true,
    ),
    ("GetLicenseRecommendations", "licenseRecommendations", true),
    (
        "GetRDSDatabaseRecommendations",
        "rdsDBRecommendations",
        false,
    ),
    ("GetRecommendationSummaries", "recommendationSummaries", true),
];

/// The eight `Export*Recommendations` surfaces: the rows of [`surfaces`] that export, without
/// the enrollment write, which takes no `accountIds`.
fn export_surfaces() -> Vec<Surface> {
    surfaces()
        .into_iter()
        .filter(|surface| surface.operation != "UpdateEnrollmentStatus")
        .collect()
}

/// The account ids of `members`, in the order the organization holds them.
fn member_ids(members: &[MemberAccountEnrollment]) -> Vec<String> {
    members
        .iter()
        .map(|member| member.account_id.clone())
        .collect()
}

/// Well-formed 12-digit ids that are NEITHER the caller's own account NOR a member account of
/// the recorded organization: the class "an account this service answers nothing about",
/// spanned rather than named once — the extremes of the 12-digit range, the documentation's own
/// example account, and the ids one digit away from the caller's own and from a member account,
/// so membership is pinned to the recorded id and not to a prefix it shares.
fn accounts_outside(members: &[MemberAccountEnrollment]) -> Vec<String> {
    let neighbour = |account_id: &str| {
        let mut neighbour = account_id.to_string();
        let last = neighbour.pop().unwrap_or('0');
        neighbour.push(if last == '9' { '8' } else { '9' });
        neighbour
    };
    let recorded = member_ids(members);
    let mut candidates = vec![
        "000000000001".to_string(),
        "999999999999".to_string(),
        "111122223333".to_string(),
        neighbour(ACCOUNT),
    ];
    if let Some(member) = recorded.first() {
        candidates.push(neighbour(member));
    }
    candidates.retain(|id| id != ACCOUNT && !recorded.contains(id));
    candidates
}

/// The `accountIds` lists an export may name over a recorded organization, spanning what the
/// rule generalises over: how many ids the list holds, and whether the caller's own account is
/// among them ("You can specify multiple account IDs per request").
fn member_account_lists(members: &[MemberAccountEnrollment]) -> Vec<(String, Vec<String>)> {
    let recorded = member_ids(members);
    let mut rows = vec![("every member account".to_string(), recorded.clone())];
    if let Some(member) = recorded.first() {
        rows.push((
            "the caller's own account and a member account".to_string(),
            vec![ACCOUNT.to_string(), member.clone()],
        ));
        rows.push(("one member account".to_string(), vec![member.clone()]));
    }
    rows
}

/// Call `operation` and assert it was served, answering the body.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> Value {
    let (status, body) = call(router, operation, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be served, got {status}: {body}"
    );
    body
}

/// Assert `request` is refused with the modelled 400 and the cross-account refusal.
async fn refused_as_cross_account(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterValueException"),
        "case {case:?}: expected InvalidParameterValueException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(CROSS_ACCOUNT_MESSAGE),
        "case {case:?}: expected the cross-account refusal, got {body}"
    );
}

/// Whether `body` reports no resource at all in `member`.
///
/// A findings reader reports one row per resource, so an account that holds no analysis reads
/// an empty list. `GetRecommendationSummaries` instead reports one summary per resource type
/// this service TRACKS whatever the account holds — "the number of" resources at each finding —
/// so for such an account every count it carries is zero.
fn reports_no_resource(member: &str, body: &Value) -> bool {
    let reported = body[member].as_array().cloned().unwrap_or_default();
    if member != "recommendationSummaries" {
        return reported.is_empty();
    }
    reported.iter().all(|summary| {
        summary["summaries"]
            .as_array()
            .is_none_or(|counts| counts.iter().all(|count| count["value"] == json!(0.0)))
    })
}

/// Every `accountId` the answer carries, wherever it appears in it: what a reader reports
/// belongs to the account the request named and to no other.
fn account_ids_in(body: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_account_ids(body, &mut found);
    found
}

/// See [`account_ids_in`].
fn collect_account_ids(body: &Value, found: &mut Vec<String>) {
    match body {
        Value::Object(members) => {
            for (name, value) in members {
                if name == "accountId" {
                    if let Some(id) = value.as_str() {
                        found.push(id.to_string());
                    }
                }
                collect_account_ids(value, found);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_account_ids(value, found);
            }
        }
        _ => {}
    }
}

/// Poll `job_id` to its terminal status, asserting it completed.
///
/// Waits for the JOB's own status rather than for a span of the clock, so nothing built on it
/// depends on how fast the host runs.
async fn completed_job(router: &MultiServiceRouter, case: &str, job_id: &str) {
    for _ in 0..JOB_POLLS {
        let (status, body) = call(
            router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": [job_id]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading job {job_id}: got {status}: {body}"
        );
        let reported = body["recommendationExportJobs"][0]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if reported == JOB_STATUS_COMPLETE || reported == JOB_STATUS_FAILED {
            assert_eq!(
                reported, JOB_STATUS_COMPLETE,
                "case {case:?}: the export job must complete, got {body}"
            );
            return;
        }
        tokio::time::sleep(EXPORT_JOB_STEP).await;
    }
    panic!("case {case:?}: export job {job_id} never reached a terminal status");
}

/// Export through `operation` with `request`, assert the call was admitted, and answer the job
/// id and the two object keys its destination names.
async fn admitted_export(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> (String, String, String, String) {
    let body = read(router, case, operation, request).await;
    let job_id = body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an admitted export answers a job id, got {body}"))
        .to_string();
    let (bucket, key, metadata_key) = export_destination(case, &body);
    (job_id, bucket, key, metadata_key)
}

/// Download the object at `key`, asserting Amazon S3 served it.
async fn downloaded(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> Vec<u8> {
    let (status, object) = get_object(router, bucket, key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: downloading {key}: got {status}: {}",
        String::from_utf8_lossy(&object)
    );
    object
}

/// Every reader that takes `accountIds` answers for a member account of the recorded
/// organization — the accounts `includeMemberAccounts` covers — instead of refusing it: the
/// call is served, and it reports that account's own analysis, which this emulator holds none
/// of, rather than the caller's rows under another account's id.
#[tokio::test]
async fn a_member_account_is_answered_for_by_every_reader_that_takes_account_ids() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);
        make_analysis_due(&engine);

        for (operation, member, holds_analysis) in READERS {
            // The caller's own account reads its own analysis, whichever organization it
            // manages: the row every member-account row below is compared against.
            let case = format!("{organization}: {operation} for the caller's own account");
            let own = read(&router, &case, operation, json!({})).await;
            assert_eq!(
                !reports_no_resource(member, &own),
                holds_analysis,
                "case {case:?}: expected the caller's own analysis to be reported \
                 (rows={holds_analysis}), got {own}"
            );
            assert!(
                account_ids_in(&own).iter().all(|id| id == ACCOUNT),
                "case {case:?}: the caller's own answer names only its own account, got {own}"
            );

            for account_id in member_ids(&members) {
                let case = format!("{organization}: {operation} for member account {account_id}");
                let body = read(
                    &router,
                    &case,
                    operation,
                    json!({"accountIds": [account_id.clone()]}),
                )
                .await;
                assert!(
                    reports_no_resource(member, &body),
                    "case {case:?}: this emulator records no analysis for a member account, so \
                     the reader reports no resource for it, got {body}"
                );
                let named = account_ids_in(&body);
                assert!(
                    named.iter().all(|id| id == &account_id),
                    "case {case:?}: the answer carries only the account the request named, got \
                     {named:?} in {body}"
                );
            }
        }
    }
}

/// Every export surface admits an `accountIds` naming member accounts of the recorded
/// organization: the job is queued and reported, it completes, and the metadata of the file it
/// writes names exactly the accounts the request asked for.
#[tokio::test]
async fn an_export_may_name_a_member_account_of_the_recorded_organization() {
    for (organization, members) in organizations() {
        for (form, account_ids) in member_account_lists(&members) {
            // One emulator per `accountIds` form: only one export job of a resource type may
            // be in progress per Region, so two forms of one surface cannot share a store.
            let (router, engine) = enrolled_emulator().await;
            record_organization(&engine, &members);
            make_analysis_due(&engine);

            let mut queued: Vec<(String, String, String, String)> = Vec::new();
            for surface in export_surfaces() {
                let case = format!("{organization}/{form}: {}", surface.operation);
                let mut asked = (surface.rest)();
                asked["accountIds"] = json!(account_ids);
                let (job_id, bucket, _, metadata_key) =
                    admitted_export(&router, &case, surface.operation, asked).await;
                queued.push((case, job_id, bucket, metadata_key));
            }

            // The store holds exactly the jobs those calls answered, in creation order.
            let jobs = reported_jobs(&router, &format!("{organization}/{form}")).await;
            let admitted: Vec<String> = queued
                .iter()
                .map(|(_, job_id, _, _)| job_id.clone())
                .collect();
            assert_eq!(
                jobs, admitted,
                "case {organization:?}/{form:?}: the store holds exactly the admitted export \
                 jobs, got {jobs:?}"
            );

            for (case, job_id, bucket, metadata_key) in &queued {
                completed_job(&router, case, job_id).await;
                let object = downloaded(&router, case, bucket, metadata_key).await;
                let metadata: Value = serde_json::from_slice(&object).unwrap_or_else(|error| {
                    panic!("case {case:?}: the metadata file is JSON: {error}")
                });
                assert_eq!(
                    metadata["accountIds"],
                    json!(account_ids),
                    "case {case:?}: the metadata names the accounts the export asked for, got \
                     {metadata}"
                );
            }
        }
    }
}

/// The exported FILE holds the rows of the accounts the request named and of no others: an
/// export that names a member account — whose analysis this emulator holds none of — writes the
/// header row alone, while one that names the caller's own account writes exactly the rows a
/// default export writes.
#[tokio::test]
async fn an_export_file_holds_only_the_rows_of_the_accounts_it_named() {
    let member = "200000000001".to_string();
    let members = vec![MemberAccountEnrollment::new(&member)];

    // Two surfaces whose analysis this emulator records rows of, so "only the named account's
    // rows" is a difference the file can show.
    for operation in [
        "ExportAutoScalingGroupRecommendations",
        "ExportEC2InstanceRecommendations",
    ] {
        let mut own_rows: Option<usize> = None;
        for (form, account_ids) in [
            ("absent", None),
            ("the caller's own account", Some(vec![ACCOUNT.to_string()])),
            (
                "the caller's own account and a member account",
                Some(vec![ACCOUNT.to_string(), member.clone()]),
            ),
            ("a member account", Some(vec![member.clone()])),
        ] {
            let case = format!("{operation} with accountIds {form}");
            let (router, engine) = enrolled_emulator().await;
            record_organization(&engine, &members);
            make_analysis_due(&engine);

            let mut request = export_rest();
            if let Some(account_ids) = &account_ids {
                request["accountIds"] = json!(account_ids);
            }
            let (job_id, bucket, key, _) =
                admitted_export(&router, &case, operation, request).await;
            completed_job(&router, &case, &job_id).await;
            let object = downloaded(&router, &case, &bucket, &key).await;
            let document = String::from_utf8(object)
                .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
            let rows = csv_rows(&document);
            let (header, data) = rows
                .split_first()
                .unwrap_or_else(|| panic!("case {case:?}: the export file carries a header row"));

            // Every row the file holds belongs to an account the request named.
            let names_own = account_ids
                .as_ref()
                .is_none_or(|named| named.iter().any(|id| id == ACCOUNT));
            for row in data {
                let reported = crate::harness::cell(header, row, "AccountId");
                assert!(
                    account_ids
                        .as_ref()
                        .is_none_or(|named| named.iter().any(|id| id == reported)),
                    "case {case:?}: the file holds a row of {reported:?}, which the export did \
                     not name: {row:?}"
                );
            }

            if names_own {
                // The caller's own rows: as many as a default export writes, whichever form
                // named the account.
                let counted = *own_rows.get_or_insert(data.len());
                assert!(
                    data.len() == counted && counted > 0,
                    "case {case:?}: expected the caller's own {counted} rows, got {}",
                    data.len()
                );
            } else {
                assert!(
                    data.is_empty(),
                    "case {case:?}: this emulator records no analysis for a member account, so \
                     the file holds the header row alone, got {data:?}"
                );
            }
        }
    }
}

/// An account that is NEITHER the caller's own nor a member account of the recorded
/// organization is still refused with the cross-account refusal — by every reader that takes
/// `accountIds` and by every export surface — so admitting a member account did not admit any
/// account at all.
#[tokio::test]
async fn an_account_outside_the_organization_is_refused_by_every_surface() {
    for (organization, members) in organizations() {
        let (router, engine) = enrolled_emulator().await;
        record_organization(&engine, &members);
        make_analysis_due(&engine);

        for account_id in accounts_outside(&members) {
            for (operation, _, _) in READERS {
                let case = format!("{organization}: {operation} for {account_id}");
                refused_as_cross_account(
                    &router,
                    &case,
                    operation,
                    json!({"accountIds": [account_id.clone()]}),
                )
                .await;
            }
            for surface in export_surfaces() {
                let case = format!("{organization}: {} for {account_id}", surface.operation);
                let mut asked = (surface.rest)();
                asked["accountIds"] = json!([account_id.clone()]);
                refused_as_cross_account(&router, &case, surface.operation, asked).await;
            }
        }

        // Not one of those calls wrote: no export job was queued.
        let jobs = reported_jobs(&router, organization).await;
        assert!(
            jobs.is_empty(),
            "case {organization:?}: a refused export records no job, got {jobs:?}"
        );
    }
}
