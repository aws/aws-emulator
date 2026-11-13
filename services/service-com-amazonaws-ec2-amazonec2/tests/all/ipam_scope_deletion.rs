// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamScope`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * Deleting an IPAM scope moves EXACTLY the scope the request names into the transient
//!   `delete-in-progress` state and answers it as the store holds it; the clock then REMOVES
//!   it, so a later `DescribeIpamScopes` for that id answers `InvalidIpamScopeId.NotFound`
//!   and the IPAM no longer counts it among its scopes. Every other scope of the IPAM is
//!   untouched.
//! * "You cannot delete the default scopes" (`DeleteIpamScope`): a scope an IPAM came up with
//!   is refused with `IncorrectState` and stays exactly as it was.
//! * A scope that still holds pools is refused with `DependencyViolation` until those pools
//!   are deleted, this operation modelling no cascade.
//! * An id outside the form this service ISSUES for an `IpamScopeId` is refused with
//!   `InvalidIpamScopeId.Malformed` before any lookup; an id of that form naming no scope of
//!   the account is refused with `InvalidIpamScopeId.NotFound`; a scope whose deletion has
//!   already been asked for is refused with `IncorrectState`. None of them writes anything.
//! * `DryRun` answers the permission question with `DryRunOperation` and writes nothing,
//!   whatever the scope it names is.
//!
//! Each table row is a distinct input along what those rules generalise over: which optional
//! members the deleted scope was created with, how many other scopes its IPAM holds, which
//! of the two default scopes is named, how many pools and of which address family are still
//! in the scope, and which part of the id is outside the issued form.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use ec2::ops::types::IpamScope;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEPENDENCY_VIOLATION, INCORRECT_STATE,
    INVALID_IPAM_SCOPE_ID_MALFORMED, INVALID_IPAM_SCOPE_ID_NOT_FOUND,
    IPAM_SCOPE_STATE_DELETE_IN_PROGRESS, IPAM_SCOPE_TYPE_PRIVATE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed `IpamScopeId` in the long (17 hexadecimal digit) form that the account holds
/// no scope for.
///
/// Spelled from the top of the hexadecimal range rather than the bottom: this store mints ids
/// from a monotonic counter, so an id this high cannot collide with one a created scope was
/// given however many ids a run has minted.
const UNKNOWN_SCOPE_ID: &str = "ipam-scope-0ffffffffffffffff";

/// How long a poll for the removal the clock owes a deleted scope keeps looking before giving
/// up: far longer than the span the service records for that removal, so a loaded host cannot
/// turn "not due yet" into a failure. The loop below leaves as soon as the condition holds, so
/// a fast host waits no longer than the span itself.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How long each look of that poll waits before looking again — a client's own retry pause,
/// well under the span it is waiting for.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The IPAM a suite creates through `CreateIpam`: what a request names it by, the ARN every
/// scope of it reports, and the two default scopes it came up with.
struct CreatedIpam {
    /// The `IpamId` a request names it by.
    id: String,
    /// "The ARN of the IPAM" (`IpamScope$IpamArn`), which every scope of it reports.
    arn: String,
    /// The IPAM's default public scope — "an IPAM contains two default scopes" (`IpamScope`).
    public_scope: String,
    /// The IPAM's default private scope.
    private_scope: String,
}

/// Create one IPAM through the operation that owns it, and answer what it is named by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| -> String {
        child_text(ipam, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
        public_scope: read("publicDefaultScopeId"),
        private_scope: read("privateDefaultScopeId"),
    }
}

/// Create one further scope in `ipam_id` through `CreateIpamScope` and answer its id.
async fn create_scope(
    case: &str,
    router: &MultiServiceRouter,
    ipam_id: &str,
    description: Option<&str>,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![param("IpamId", ipam_id)];
    if let Some(description) = description {
        params.push(param("Description", description));
    }
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "ipam-scope"));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    let body = ok(case, router, "CreateIpamScope", &params).await;
    scope_member(case, &body, "ipamScopeId")
}

/// Create one pool in the scope `scope_id` through `CreateIpamPool` and answer its id.
async fn create_pool(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    family: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateIpamPool",
        &[param("IpamScopeId", scope_id), param("AddressFamily", family)],
    )
    .await;
    let parsed = document(case, &body);
    let pool = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPoolId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no pool (no <ipamPoolId>) in {body}"));
    child_text(pool, "ipamPoolId")
        .unwrap_or_else(|| panic!("case {case:?}: the created pool reports an id, got {body}"))
}

/// The scope element of a response, as the value of its `name` child.
fn scope_member(case: &str, body: &str, name: &str) -> String {
    scope_members(case, body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the scope in {body}"))
}

/// The same reading as [`scope_member`], answering `None` for a member the body omits.
fn scope_members(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let scope = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamScopeId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM scope (no <ipamScopeId>) in {body}"));
    child_text(scope, name)
}

/// The ids every scope in a `DescribeIpamScopes` body reports.
fn described_ids(case: &str, body: &str) -> BTreeSet<String> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "ipamScopeSet")
    else {
        return BTreeSet::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .filter_map(|scope| child_text(scope, "ipamScopeId"))
        .collect()
}

/// The `DeleteIpamScope` call, as its HTTP status and body.
async fn delete_scope(
    router: &MultiServiceRouter,
    scope_id: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![param("IpamScopeId", scope_id)];
    params.extend(extra.iter().map(|(name, value)| param(name, value)));
    invoke(router, "DeleteIpamScope", &params).await
}

/// The state `DescribeIpamScopes` reports for the one scope `scope_id`, or `None` when the
/// read is refused because the account holds no such scope any more.
async fn described_state(router: &MultiServiceRouter, scope_id: &str) -> Option<String> {
    let (status, body) =
        invoke(router, "DescribeIpamScopes", &[param("IpamScopeId.1", scope_id)]).await;
    if status != 200 {
        return None;
    }
    scope_members("the scope's state", &body, "state")
}

/// "The number of scopes in the IPAM" (`Ipam$ScopeCount`), as `DescribeIpams` reports it.
async fn reported_scope_count(case: &str, router: &MultiServiceRouter, ipam_id: &str) -> String {
    let body = ok(case, router, "DescribeIpams", &[param("IpamId.1", ipam_id)]).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM in {body}"));
    child_text(ipam, "scopeCount")
        .unwrap_or_else(|| panic!("case {case:?}: the IPAM reports a scope count, got {body}"))
}

/// Poll `DescribeIpamScopes` for `scope_id` the way a client waiting for a deletion does,
/// until the read is refused or the deadline passes, and answer that last read.
///
/// The wait is on the CONDITION, not on a span: the loop leaves the moment the scope stops
/// resolving, and [`POLL_DEADLINE`] is only the point at which it gives up and lets the
/// assertion report what it last saw.
async fn poll_until_gone(router: &MultiServiceRouter, scope_id: &str) -> (u16, String) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) =
            invoke(router, "DescribeIpamScopes", &[param("IpamScopeId.1", scope_id)]).await;
        if status != 200 || Instant::now() >= deadline {
            return (status, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// What a delete moves, and what the clock then removes
// ---------------------------------------------------------------------------

/// One row of the delete table: the case, the optional members the deleted scope was created
/// with, and how many other scopes its IPAM holds beside it.
struct DeleteCase {
    case: &'static str,
    description: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    other_scopes: usize,
}

/// The rule: `DeleteIpamScope` leaves EXACTLY the scope the request names in the transient
/// `delete-in-progress` state, answers it as the store holds it, and the clock then removes
/// it — so a later `DescribeIpamScopes` for that id answers `InvalidIpamScopeId.NotFound`,
/// the IPAM stops counting it, and every other scope of the IPAM is untouched.
#[tokio::test]
async fn deleting_a_scope_moves_it_and_the_clock_then_removes_it() {
    let cases = [
        DeleteCase {
            case: "a scope created with no optional member",
            description: None,
            tags: Vec::new(),
            other_scopes: 0,
        },
        DeleteCase {
            case: "a described and tagged scope",
            description: Some("the scope this delete takes away"),
            tags: vec![("Owner", "TeamA"), ("Tier", "test")],
            other_scopes: 0,
        },
        DeleteCase {
            case: "one of three created scopes of the same IPAM",
            description: Some("one of several"),
            tags: Vec::new(),
            other_scopes: 2,
        },
    ];

    for case in cases {
        let DeleteCase {
            case,
            description,
            tags,
            other_scopes,
        } = case;
        let (router, _engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let mut kept: BTreeSet<String> =
            BTreeSet::from([ipam.public_scope.clone(), ipam.private_scope.clone()]);
        for index in 0..other_scopes {
            let other = create_scope(
                case,
                &router,
                &ipam.id,
                Some(&format!("another scope {index}")),
                &[],
            )
            .await;
            kept.insert(other);
        }
        let scope_id = create_scope(case, &router, &ipam.id, description, &tags).await;

        let (status, body) = delete_scope(&router, &scope_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {scope_id}, got {status} with body {body}"
        );
        assert_eq!(
            scope_member(case, &body, "ipamScopeId"),
            scope_id,
            "case {case:?}: the delete answers the scope it was asked about, got {body}"
        );
        assert_eq!(
            scope_member(case, &body, "state"),
            IPAM_SCOPE_STATE_DELETE_IN_PROGRESS,
            "case {case:?}: a deleted scope is answered in the transient state, got {body}"
        );
        assert_eq!(
            scope_members(case, &body, "ipamArn"),
            Some(ipam.arn.clone()),
            "case {case:?}: the deleted scope reports its IPAM's ARN, got {body}"
        );
        assert_eq!(
            scope_members(case, &body, "isDefault").as_deref(),
            Some("false"),
            "case {case:?}: a caller-created scope is not a default one, got {body}"
        );
        assert_eq!(
            scope_members(case, &body, "description").as_deref(),
            description,
            "case {case:?}: the deleted scope reports the description it was created with, \
             got {body}"
        );

        // The IPAM no longer HOLDS the scope, so its own count of them drops by the one this
        // delete took: the two defaults plus however many others the row created.
        assert_eq!(
            reported_scope_count(case, &router, &ipam.id).await,
            kept.len().to_string(),
            "case {case:?}: the IPAM counts only the scopes it still holds"
        );

        let (status, body) = poll_until_gone(&router, &scope_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a removed scope stops resolving, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_IPAM_SCOPE_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_IPAM_SCOPE_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The IPAM scope ({scope_id}) does not exist."),
            "case {case:?}: the refusal names the id the request asked about, got {body}"
        );

        // Nothing else moved: every other scope of the IPAM is still reported.
        let all = ok(case, &router, "DescribeIpamScopes", &[]).await;
        assert_eq!(
            described_ids(case, &all),
            kept,
            "case {case:?}: the delete took exactly one scope, got {all}"
        );
    }
}

// ---------------------------------------------------------------------------
// The scopes an IPAM comes up with
// ---------------------------------------------------------------------------

/// The rule: "you cannot delete the default scopes" (`DeleteIpamScope`) — a scope the IPAM
/// came up with is refused with `IncorrectState`, and nothing about it or its IPAM changes.
#[tokio::test]
async fn the_default_scopes_of_an_ipam_cannot_be_deleted() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM whose defaults are named", &router).await;
    let created = create_scope(
        "a scope beside the defaults",
        &router,
        &ipam.id,
        Some("a scope a caller asked for"),
        &[],
    )
    .await;

    for (case, scope_id) in [
        ("the default public scope", ipam.public_scope.clone()),
        ("the default private scope", ipam.private_scope.clone()),
    ] {
        let before = described_state(&router, &scope_id).await;
        let (status, body) = delete_scope(&router, &scope_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 deleting {scope_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The IPAM scope ({scope_id}) is a default scope and cannot be deleted."),
            "case {case:?}: the refusal names the scope the request asked about, got {body}"
        );
        // Nothing was written: the scope is still there and is not on its way out. Its state
        // is whatever the clock has made of the IPAM's creation by now, so what this asserts
        // is that the refusal did not START a deletion.
        let after = described_state(&router, &scope_id).await;
        assert!(
            after.is_some(),
            "case {case:?}: the refused scope {scope_id} is still reported, it read as {after:?}"
        );
        assert_ne!(
            after.as_deref(),
            Some(IPAM_SCOPE_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a refused delete writes nothing, yet {scope_id} moved from \
             {before:?} to {after:?}"
        );
    }

    // The IPAM still holds its two defaults and the created scope: three in all.
    assert_eq!(
        reported_scope_count("the refused defaults", &router, &ipam.id).await,
        "3",
        "the refused deletes left the IPAM's scope count where it was"
    );
    let all = ok("the refused defaults", &router, "DescribeIpamScopes", &[]).await;
    assert_eq!(
        described_ids("the refused defaults", &all),
        BTreeSet::from([
            ipam.public_scope.clone(),
            ipam.private_scope.clone(),
            created,
        ]),
        "every scope is still reported, got {all}"
    );
}

// ---------------------------------------------------------------------------
// What is still inside the scope
// ---------------------------------------------------------------------------

/// The rule: a pool is a resource created WITHIN a scope and this operation models no
/// cascade, so a scope that still holds one is refused with `DependencyViolation` — and the
/// same delete is accepted once the pools are deleted.
#[tokio::test]
async fn a_scope_that_still_holds_pools_is_refused_until_they_are_deleted() {
    for (case, families) in [
        ("one IPv4 pool", vec!["ipv4"]),
        ("two pools of either family", vec!["ipv4", "ipv6"]),
    ] {
        let (router, _engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let scope_id = create_scope(case, &router, &ipam.id, Some("a scope with pools"), &[]).await;
        let mut pools: Vec<String> = Vec::new();
        for family in &families {
            pools.push(create_pool(case, &router, &scope_id, family).await);
        }

        let (status, body) = delete_scope(&router, &scope_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 deleting a scope holding {} pools, got {status} \
             with body {body}",
            pools.len()
        );
        assert_eq!(
            error_code(&body),
            DEPENDENCY_VIOLATION,
            "case {case:?}: expected {DEPENDENCY_VIOLATION}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The specified action is not valid for {scope_id}. There are one or more pools \
                 in the scope."
            ),
            "case {case:?}: the refusal names the scope and what is still in it, got {body}"
        );
        assert_ne!(
            described_state(&router, &scope_id).await.as_deref(),
            Some(IPAM_SCOPE_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a refused delete writes nothing"
        );

        // Take the pools away through the operation that owns them, and the same delete is
        // admissible.
        for pool_id in &pools {
            let body = ok(
                case,
                &router,
                "DeleteIpamPool",
                &[param("IpamPoolId", pool_id)],
            )
            .await;
            assert!(
                body.contains(pool_id),
                "case {case:?}: the pool delete answers {pool_id}, got {body}"
            );
        }
        let (status, body) = delete_scope(&router, &scope_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an emptied scope is deletable, got {status} with body {body}"
        );
        assert_eq!(
            scope_member(case, &body, "state"),
            IPAM_SCOPE_STATE_DELETE_IN_PROGRESS,
            "case {case:?}: the accepted delete answers the transient state, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ids a delete refuses
// ---------------------------------------------------------------------------

/// The rule: an id outside the form this service ISSUES is refused as malformed before any
/// lookup, an id of that form naming no scope of the account is refused as not found, and
/// neither writes anything.
#[tokio::test]
async fn an_id_this_service_could_not_have_issued_is_refused_before_any_lookup() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the surviving scope belongs to", &router).await;
    let scope_id = create_scope(
        "the scope no refusal touches",
        &router,
        &ipam.id,
        None,
        &[],
    )
    .await;

    // Each row is a different part of the issued form being wrong: nothing at all, no
    // prefix, the prefix alone, an empty suffix, a suffix that is not hexadecimal, another
    // resource's prefix, and the issued prefix in another case.
    for (case, id) in [
        ("an empty id", ""),
        ("no prefix at all", "not-an-ipam-scope-id"),
        ("the prefix alone", "ipam-scope"),
        ("an empty suffix", "ipam-scope-"),
        ("a suffix that is not hexadecimal", "ipam-scope-0123456789abcdefz"),
        ("another resource's prefix", "ipam-pool-0123456789abcdef0"),
        ("the prefix in another case", "IPAM-SCOPE-0123456789abcdef0"),
    ] {
        let (status, body) = delete_scope(&router, id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {id:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_IPAM_SCOPE_ID_MALFORMED,
            "case {case:?}: expected {INVALID_IPAM_SCOPE_ID_MALFORMED} for {id:?}, got {body}"
        );
    }

    let (status, body) = delete_scope(&router, UNKNOWN_SCOPE_ID, &[]).await;
    assert_eq!(
        status, 400,
        "a well-formed id the account holds no scope for is refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        "expected {INVALID_IPAM_SCOPE_ID_NOT_FOUND} for {UNKNOWN_SCOPE_ID}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The IPAM scope ({UNKNOWN_SCOPE_ID}) does not exist."),
        "the refusal names the id the request asked about, got {body}"
    );

    // Nothing was written by any of it: the account's scopes are exactly what it held.
    let all = ok("after every refusal", &router, "DescribeIpamScopes", &[]).await;
    assert_eq!(
        described_ids("after every refusal", &all),
        BTreeSet::from([
            ipam.public_scope.clone(),
            ipam.private_scope.clone(),
            scope_id,
        ]),
        "every refusal left the store alone, got {all}"
    );
}

/// The rule: a scope whose deletion has already been asked for has nothing left to delete, so
/// a second delete is refused with `IncorrectState` naming the state it is in rather than
/// restarting the clock on it.
///
/// The scope is seeded in that state through the store's own writer rather than by deleting a
/// created scope twice: the second call would then have to land inside the span the clock owes
/// the removal, which is a race no assertion should depend on.
#[tokio::test]
async fn a_scope_whose_deletion_was_already_asked_for_is_refused() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM the deleting scope belongs to", &router).await;
    let scope_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_ipam_scope(IpamScope {
            ipam_arn: Some(ipam.arn.clone()),
            ipam_region: Some(REGION.to_string()),
            ipam_scope_type: Some(IPAM_SCOPE_TYPE_PRIVATE.to_string()),
            is_default: Some(false),
            state: Some(IPAM_SCOPE_STATE_DELETE_IN_PROGRESS.to_string()),
            ..Default::default()
        })
        .expect("the store holds the seeded IPAM scope");

    let (status, body) = delete_scope(&router, &scope_id, &[]).await;
    assert_eq!(
        status, 400,
        "a scope already being deleted is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INCORRECT_STATE,
        "expected {INCORRECT_STATE}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("IPAM scope '{scope_id}' is in the '{IPAM_SCOPE_STATE_DELETE_IN_PROGRESS}' state."),
        "the refusal names the scope and the state it is in, got {body}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// The rule: `DryRun` asks about the CALLER's permission, so it is answered with
/// `DryRunOperation` once the request's own form has been checked and before anything is
/// resolved or written — whatever scope the request names — while an id outside the issued
/// form is still malformed.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM a dry run names scopes of", &router).await;
    let created = create_scope("the dry run's target", &router, &ipam.id, None, &[]).await;

    for (case, id) in [
        ("a created scope", created.clone()),
        ("a default scope", ipam.private_scope.clone()),
        ("an id naming no scope", UNKNOWN_SCOPE_ID.to_string()),
    ] {
        let (status, body) = delete_scope(&router, &id, &[("DryRun", "true")]).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} for {id}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation for {id}, got {body}"
        );
    }

    // The form of the id is checked BEFORE the permission question, so a malformed id is
    // still malformed under DryRun.
    let (status, body) = delete_scope(&router, "ipam-scope-nope", &[("DryRun", "true")]).await;
    assert_eq!(
        status, 400,
        "a malformed id is refused for its form even under DryRun, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_IPAM_SCOPE_ID_MALFORMED,
        "expected {INVALID_IPAM_SCOPE_ID_MALFORMED}, got {body}"
    );

    // Nothing moved: a dry run resolves nothing and writes nothing, so every scope the
    // account held is still reported.
    let all = ok("after the dry runs", &router, "DescribeIpamScopes", &[]).await;
    assert_eq!(
        described_ids("after the dry runs", &all),
        BTreeSet::from([
            ipam.public_scope.clone(),
            ipam.private_scope.clone(),
            created,
        ]),
        "every dry run left the store alone, got {all}"
    );
}
