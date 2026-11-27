// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpam` and the describes a deleted IPAM is read back through,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=DeleteIpam`, answered
//! with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a delete of an IPAM WRITES THE STORED RECORD into the transient
//! `delete-in-progress` member of its own state catalog with the clock owing its REMOVAL, and
//! answers that record with every other member unchanged — so what the delete answered is what
//! a read straight back reports, and once the removal falls due neither the IPAM, nor the
//! scopes it contained, nor the pools created in them resolve any more. It refuses rather than
//! reporting a deletion it did not perform: a value that is not an IPAM id at all with that
//! resource's `Malformed` code, a well-formed id the account holds no IPAM for with its
//! `NotFound` code, an IPAM a delete has already started on with `IncorrectState`, and an IPAM
//! that still holds a pool with `DependencyViolation` — `Cascade` deleting the pools in the
//! private scopes with it, and refusing while a pool is in the public scope. A dry run reports
//! the permitted outcome and writes nothing.
//!
//! Every table row is a distinct input along what that rule generalises over: what the IPAM
//! still holds when the delete arrives (nothing, one private-scope pool, two of them, a pool
//! with a CIDR provisioned to it and a pool inside that pool, a public-scope pool), whether the
//! request carries `Cascade`, which form an unresolvable id takes, and how much of the delete's
//! own span the clock has run.
//!
//! Nothing here waits a fixed time for a state the clock owes: each look is taken again until
//! the describe reports what the clock owed, or a deadline far past the service's own
//! transition span passes.

use std::time::{Duration, Instant};

use ec2::ops::types::{IPAM_POOL_STATE_VALUES, IPAM_STATE_VALUES};
use ec2::state::{
    ADDRESS_FAMILY_IPV4, DEPENDENCY_VIOLATION, DRY_RUN_OPERATION, DRY_RUN_OPERATION_MESSAGE,
    DRY_RUN_OPERATION_STATUS, INCORRECT_STATE, INVALID_IPAM_ID_MALFORMED,
    INVALID_IPAM_ID_NOT_FOUND, INVALID_IPAM_POOL_ID_NOT_FOUND, INVALID_IPAM_SCOPE_ID_NOT_FOUND,
    IPAM_POOL_STATE_DELETE_COMPLETE, IPAM_POOL_STATE_DELETE_IN_PROGRESS,
    IPAM_STATE_CREATE_COMPLETE, IPAM_STATE_CREATE_IN_PROGRESS, IPAM_STATE_DELETE_IN_PROGRESS,
    TRANSITION_DELAY, invalid_id_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The ec2Query `Action` under test.
const ACTION: &str = "DeleteIpam";

/// The describe a deleted IPAM is read back through.
const DESCRIBE: &str = "DescribeIpams";

/// The describe the scopes an IPAM contains are read back through.
const DESCRIBE_SCOPES: &str = "DescribeIpamScopes";

/// The describe the pools created in those scopes are read back through.
const DESCRIBE_POOLS: &str = "DescribeIpamPools";

/// The HTTP status EC2 answers a refused request with.
const REFUSAL_STATUS: u16 = 400;

/// The deadline the polls below give up at: a large multiple of the service's own transition
/// span, so an assertion is about the state the clock owes and never about how fast the host
/// runs.
fn removal_deadline() -> Duration {
    TRANSITION_DELAY * 60
}

/// How long one look waits before looking again — a fraction of that span, so a poll inside the
/// deadline above costs many looks rather than one.
fn poll_interval() -> Duration {
    TRANSITION_DELAY / 10
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
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

/// Assert that `action` was refused `status` with `code` and exactly `message`, and answer the
/// body it was refused with.
async fn refused(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    status: u16,
    code: &str,
    message: &str,
) -> String {
    let (actual_status, body) = invoke(router, action, params).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected HTTP {status} from {action}, got {actual_status} with body {body}"
    );
    let actual_code = error_code(&body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected the code {code:?} from {action}, got {actual_code:?} in {body}"
    );
    let actual_message = error_message(&body);
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?} from {action}, got {actual_message:?}"
    );
    body
}

/// One IPAM as a response reports it: the members a delete answers and a describe reports, with
/// the operating Regions flattened into comparable values.
///
/// Comparing WHOLE records is what makes "the delete answered the record the store holds, with
/// nothing else changed" an assertion rather than a list of members somebody remembered to
/// check.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedIpam {
    ipam_id: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    owner_id: Option<String>,
    public_default_scope_id: Option<String>,
    private_default_scope_id: Option<String>,
    scope_count: Option<String>,
    default_resource_discovery_id: Option<String>,
    default_resource_discovery_association_id: Option<String>,
    resource_discovery_association_count: Option<String>,
    description: Option<String>,
    tier: Option<String>,
    metered_account: Option<String>,
    state: Option<String>,
    state_message: Option<String>,
    /// The `regionName`s of the IPAM, sorted: operating Regions are a SET
    /// (`Ipam$OperatingRegions`), so the order they are reported in is not part of the rule.
    operating_regions: Vec<String>,
    tags: Vec<(String, String)>,
}

/// The same record with the two STATE members cleared, which is how two reports of one IPAM are
/// compared across a call that is expected to move its state and nothing else.
fn beside_the_state(ipam: &ReportedIpam) -> ReportedIpam {
    ReportedIpam {
        state: None,
        state_message: None,
        ..ipam.clone()
    }
}

/// The one IPAM a response carries, or `None` when it carries none.
///
/// Only DIRECT children of the IPAM element are read, so a `regionName` of a nested
/// `operatingRegionSet` item or a `key` of a `tagSet` item is never mistaken for a member of the
/// IPAM itself.
fn reported_ipam(case: &str, body: &str) -> Option<ReportedIpam> {
    let parsed = document(case, body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())?;
    let read = |name: &str| child_text(ipam, name);
    let items = |set: &str| {
        child(ipam, set)
            .map(|node| {
                node.children()
                    .filter(|item| item.is_element() && item.tag_name().name() == "item")
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut operating_regions: Vec<String> = items("operatingRegionSet")
        .into_iter()
        .filter_map(|item| child_text(item, "regionName"))
        .collect();
    operating_regions.sort();
    Some(ReportedIpam {
        ipam_id: read("ipamId"),
        ipam_arn: read("ipamArn"),
        ipam_region: read("ipamRegion"),
        owner_id: read("ownerId"),
        public_default_scope_id: read("publicDefaultScopeId"),
        private_default_scope_id: read("privateDefaultScopeId"),
        scope_count: read("scopeCount"),
        default_resource_discovery_id: read("defaultResourceDiscoveryId"),
        default_resource_discovery_association_id: read("defaultResourceDiscoveryAssociationId"),
        resource_discovery_association_count: read("resourceDiscoveryAssociationCount"),
        description: read("description"),
        tier: read("tier"),
        metered_account: read("meteredAccount"),
        state: read("state"),
        state_message: read("stateMessage"),
        operating_regions,
        tags: items("tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
    })
}

/// The one IPAM a response carries, or a panic naming the case.
fn required_ipam(case: &str, body: &str) -> ReportedIpam {
    reported_ipam(case, body)
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"))
}

/// One IPAM created over the wire, with the two default scopes it came up with.
#[derive(Debug, Clone)]
struct Fixture {
    id: String,
    private_scope: String,
    public_scope: String,
}

/// Create one IPAM carrying every member a report can show, so a comparison across the delete
/// covers a complete record rather than a mostly-empty one.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> Fixture {
    let body = ok(
        case,
        router,
        "CreateIpam",
        &[
            param("Description", "the IPAM under delete"),
            param("Tier", "advanced"),
            param("MeteredAccount", "resource-owner"),
            param("OperatingRegion.1.RegionName", REGION),
            param("OperatingRegion.2.RegionName", "eu-west-1"),
            param("TagSpecification.1.ResourceType", "ipam"),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "under-delete"),
        ],
    )
    .await;
    let ipam = required_ipam(case, &body);
    let member = |name: &str, value: Option<String>| {
        value.unwrap_or_else(|| panic!("case {case:?}: the created IPAM reports no {name}: {body}"))
    };
    Fixture {
        id: member("ipamId", ipam.ipam_id.clone()),
        private_scope: member(
            "privateDefaultScopeId",
            ipam.private_default_scope_id.clone(),
        ),
        public_scope: member("publicDefaultScopeId", ipam.public_default_scope_id.clone()),
    }
}

/// Create one IPv4 pool in `scope_id`, with `extra` beside it, and answer the `IpamPoolId` the
/// service minted.
async fn create_pool(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    extra: &[Param],
) -> String {
    let mut params = vec![
        param("IpamScopeId", scope_id),
        param("AddressFamily", ADDRESS_FAMILY_IPV4),
    ];
    params.extend(extra.iter().cloned());
    let body = ok(case, router, "CreateIpamPool", &params).await;
    let parsed = document(case, &body);
    let pool = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPoolId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no pool (no <ipamPoolId>) in {body}"));
    child_text(pool, "ipamPoolId")
        .unwrap_or_else(|| panic!("case {case:?}: the created pool reports no id: {body}"))
}

/// `DescribeIpams` for one id: the status and the body, whichever way it answered.
async fn describe_ipam(router: &MultiServiceRouter, ipam_id: &str) -> (u16, String) {
    invoke(router, DESCRIBE, &[param("IpamId.1", ipam_id)]).await
}

/// `DescribeIpamPools` for one id: the status and the body, whichever way it answered.
async fn describe_pool(router: &MultiServiceRouter, pool_id: &str) -> (u16, String) {
    invoke(router, DESCRIBE_POOLS, &[param("IpamPoolId.1", pool_id)]).await
}

/// The `state` of one pool, read back through its own describe.
async fn pool_state(case: &str, router: &MultiServiceRouter, pool_id: &str) -> String {
    let body = ok(
        case,
        router,
        DESCRIBE_POOLS,
        &[param("IpamPoolId.1", pool_id)],
    )
    .await;
    let parsed = document(case, &body);
    let pool = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPoolId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no pool (no <ipamPoolId>) in {body}"));
    child_text(pool, "state")
        .unwrap_or_else(|| panic!("case {case:?}: the pool {pool_id} reports no state: {body}"))
}

/// Look at the IPAM AGAIN until `DescribeIpams` for its id refuses `InvalidIpamId.NotFound` —
/// the removal the clock owed the delete — or a deadline far past the service's own transition
/// span passes. The condition is waited for, never a fixed time.
async fn poll_until_gone(case: &str, router: &MultiServiceRouter, ipam_id: &str) {
    let deadline = Instant::now() + removal_deadline();
    loop {
        let (status, body) = describe_ipam(router, ipam_id).await;
        if status == REFUSAL_STATUS {
            assert_eq!(
                error_code(&body),
                INVALID_IPAM_ID_NOT_FOUND,
                "case {case:?}: expected the removed IPAM {ipam_id} to answer \
                 {INVALID_IPAM_ID_NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("The IPAM ({ipam_id}) does not exist."),
                "case {case:?}: expected this resource's own not-found sentence for {ipam_id}, \
                 got {body}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {ipam_id} to stop resolving, got HTTP {status} with {body}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

// ---------------------------------------------------------------------------
// The words this suite asserts
// ---------------------------------------------------------------------------

/// Every state word written into an assertion below is a member of the modelled catalog it
/// belongs to, so no row can pin a state the API does not have.
#[test]
fn the_asserted_state_words_are_the_models_own() {
    for (case, word) in [
        ("an IPAM being created", IPAM_STATE_CREATE_IN_PROGRESS),
        ("a created IPAM", IPAM_STATE_CREATE_COMPLETE),
        ("an IPAM being deleted", IPAM_STATE_DELETE_IN_PROGRESS),
    ] {
        assert!(
            IPAM_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled IpamState catalog, \
             which carries {IPAM_STATE_VALUES:?}"
        );
    }
    for (case, word) in [
        ("a pool being deleted", IPAM_POOL_STATE_DELETE_IN_PROGRESS),
        ("a deleted pool", IPAM_POOL_STATE_DELETE_COMPLETE),
    ] {
        assert!(
            IPAM_POOL_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled IpamPoolState \
             catalog, which carries {IPAM_POOL_STATE_VALUES:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the delete records, and what the clock then removes
// ---------------------------------------------------------------------------

/// What the IPAM of one row still holds in its PRIVATE scope when the delete arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holdings {
    /// Nothing but the two default scopes an IPAM comes up with.
    Nothing,
    /// That many top-level pools in the IPAM's private scope.
    Pools(usize),
    /// One pool with a CIDR provisioned to it and a pool created INSIDE it, which holds a
    /// block of that CIDR — so the cascade has a tree to take, not a flat list.
    PoolTree,
}

/// Build what one row's IPAM holds, and answer the `IpamPoolId`s of the pools it created.
async fn fill(
    case: &str,
    router: &MultiServiceRouter,
    fixture: &Fixture,
    holdings: Holdings,
) -> Vec<String> {
    match holdings {
        Holdings::Nothing => Vec::new(),
        Holdings::Pools(count) => {
            let mut pools = Vec::new();
            for _ in 0..count {
                pools.push(create_pool(case, router, &fixture.private_scope, &[]).await);
            }
            pools
        }
        Holdings::PoolTree => {
            let source = create_pool(case, router, &fixture.private_scope, &[]).await;
            ok(
                case,
                router,
                "ProvisionIpamPoolCidr",
                &[param("IpamPoolId", &source), param("Cidr", "10.0.0.0/16")],
            )
            .await;
            let child_pool = create_pool(
                case,
                router,
                &fixture.private_scope,
                &[param("SourceIpamPoolId", &source)],
            )
            .await;
            ok(
                case,
                router,
                "ProvisionIpamPoolCidr",
                &[
                    param("IpamPoolId", &child_pool),
                    param("NetmaskLength", "24"),
                ],
            )
            .await;
            vec![source, child_pool]
        }
    }
}

/// The rule: a delete writes the stored IPAM into the transient `delete-in-progress` state with
/// the clock owing its removal, answers that record with every other member unchanged, and once
/// the removal falls due neither the IPAM, nor the scopes it contained, nor the pools created in
/// them resolve any more — `Cascade` taking the pools in the private scopes with it.
///
/// The rows differ along what the rule generalises over: what the IPAM still holds (nothing, one
/// pool, two pools, a pool with a CIDR provisioned to it holding a pool of its own) and whether
/// the request carries `Cascade` — which an IPAM holding nothing needs no more than one holding
/// pools needs it to be absent.
#[tokio::test]
async fn a_delete_records_the_transient_state_and_the_clock_removes_the_ipam() {
    // (case, what the IPAM holds, whether the request carries Cascade)
    let rows: [(&str, Holdings, bool); 5] = [
        ("an IPAM holding nothing", Holdings::Nothing, false),
        ("an IPAM holding nothing, with Cascade", Holdings::Nothing, true),
        ("one pool in the private scope", Holdings::Pools(1), true),
        ("two pools in the private scope", Holdings::Pools(2), true),
        ("a pool tree with provisioned CIDRs", Holdings::PoolTree, true),
    ];

    for (case, holdings, cascade) in rows {
        let router = emulator();
        let fixture = create_ipam(case, &router).await;
        let pools = fill(case, &router, &fixture, holdings).await;

        // The record the store holds the moment before the delete, which is what the delete has
        // to answer back.
        let before = required_ipam(
            case,
            &ok(case, &router, DESCRIBE, &[param("IpamId.1", &fixture.id)]).await,
        );

        let mut params = vec![param("IpamId", &fixture.id)];
        if cascade {
            params.push(param("Cascade", "true"));
        }
        let answered = required_ipam(case, &ok(case, &router, ACTION, &params).await);

        assert_eq!(
            answered.ipam_id.as_deref(),
            Some(fixture.id.as_str()),
            "case {case:?}: expected the delete to answer the IPAM it was given, got {answered:?}"
        );
        assert_eq!(
            answered.state.as_deref(),
            Some(IPAM_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: expected the delete to answer the transient delete state, got \
             {answered:?}"
        );
        assert_eq!(
            beside_the_state(&answered),
            beside_the_state(&before),
            "case {case:?}: expected the delete to answer the stored record with only its state \
             moved, got {answered:?} against {before:?}"
        );

        // The delete is PERSISTED, not just answered: a read straight back reports the same
        // record in the same state — or, where the removal the clock owes it has already fallen
        // due, the id has stopped resolving, which is the later state of the same write. Both
        // are reads of what the delete wrote, so neither look depends on how fast the host runs.
        let (status, body) = describe_ipam(&router, &fixture.id).await;
        if status == 200 {
            let read_back = required_ipam(case, &body);
            assert_eq!(
                read_back.state.as_deref(),
                Some(IPAM_STATE_DELETE_IN_PROGRESS),
                "case {case:?}: expected the describe to report the state the delete wrote, got \
                 {read_back:?}"
            );
            assert_eq!(
                beside_the_state(&read_back),
                beside_the_state(&answered),
                "case {case:?}: expected the describe to report the record the delete answered, \
                 got {read_back:?} against {answered:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                INVALID_IPAM_ID_NOT_FOUND,
                "case {case:?}: expected the read straight back to report the delete or the \
                 removal it owed, got HTTP {status} with {body}"
            );
        }

        // Every pool the cascade took is on its way out as well, in its own catalog's words —
        // or already removed with the IPAM, where the clock has run that far. Asserted
        // unconditionally, so a cascade row cannot pass without the pools having moved.
        for pool_id in &pools {
            let (pool_status, pool_body) = describe_pool(&router, pool_id).await;
            if pool_status == 200 {
                let state = pool_state(case, &router, pool_id).await;
                assert!(
                    state == IPAM_POOL_STATE_DELETE_IN_PROGRESS
                        || state == IPAM_POOL_STATE_DELETE_COMPLETE,
                    "case {case:?}: expected the cascade to delete the pool {pool_id}, which \
                     reports {state:?}"
                );
            } else {
                assert_eq!(
                    error_code(&pool_body),
                    INVALID_IPAM_POOL_ID_NOT_FOUND,
                    "case {case:?}: expected the pool {pool_id} to be deleting or gone with the \
                     IPAM, got HTTP {pool_status} with {pool_body}"
                );
            }
        }

        poll_until_gone(case, &router, &fixture.id).await;

        // And the whole-store report no longer carries it.
        let all = ok(case, &router, DESCRIBE, &[]).await;
        assert!(
            !all.contains(fixture.id.as_str()),
            "case {case:?}: expected the unfiltered report to have dropped {ipam}, got {all}",
            ipam = fixture.id
        );

        // "Deleting an IPAM removes all monitored data associated with the IPAM"
        // (`DeleteIpam`): the scopes it contained and the pools created in them are gone with
        // it, each answering its OWN resource's not-found refusal.
        for (scope_kind, scope_id) in [
            ("the private default scope", &fixture.private_scope),
            ("the public default scope", &fixture.public_scope),
        ] {
            let scope_case = format!("{case}: {scope_kind}");
            refused(
                &scope_case,
                &router,
                DESCRIBE_SCOPES,
                &[param("IpamScopeId.1", scope_id)],
                REFUSAL_STATUS,
                INVALID_IPAM_SCOPE_ID_NOT_FOUND,
                &format!("The IPAM scope ({scope_id}) does not exist."),
            )
            .await;
        }
        for pool_id in &pools {
            let pool_case = format!("{case}: the pool {pool_id}");
            refused(
                &pool_case,
                &router,
                DESCRIBE_POOLS,
                &[param("IpamPoolId.1", pool_id)],
                REFUSAL_STATUS,
                INVALID_IPAM_POOL_ID_NOT_FOUND,
                &format!("The IPAM pool ({pool_id}) does not exist."),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// The ids the delete cannot resolve
// ---------------------------------------------------------------------------

/// The rule: an `IpamId` whose FORM is not an EC2 id of this kind is refused with this
/// resource's `Malformed` code before anything is looked up, a well-formed id the account holds
/// no IPAM for with its `NotFound` code — and neither leaves a trace, so the IPAM the store does
/// hold is still readable and still not deleting.
///
/// The rows differ along the form of the unresolvable value: a value with no id prefix at all,
/// an id of another IPAM resource, too few digits, a digit outside hexadecimal, the upper-cased
/// form of an id this store really minted, and both published digit counts of an id nothing
/// minted.
#[tokio::test]
async fn a_delete_of_an_id_the_service_cannot_resolve_is_refused_and_writes_nothing() {
    let router = emulator();
    let held = create_ipam("the IPAM the store holds", &router).await;

    // (case, the IpamId the request names, the code, the message)
    let rows: [(&str, String, &str, String); 7] = [
        (
            "a value with no id prefix",
            "not-an-ipam".to_string(),
            INVALID_IPAM_ID_MALFORMED,
            invalid_id_message("not-an-ipam"),
        ),
        (
            "an id of another IPAM resource",
            "ipam-pool-0123456789abcdef0".to_string(),
            INVALID_IPAM_ID_MALFORMED,
            invalid_id_message("ipam-pool-0123456789abcdef0"),
        ),
        (
            "too few digits for either id form",
            "ipam-0123456789abcde".to_string(),
            INVALID_IPAM_ID_MALFORMED,
            invalid_id_message("ipam-0123456789abcde"),
        ),
        (
            "a digit outside hexadecimal",
            "ipam-0123456789abcdefg".to_string(),
            INVALID_IPAM_ID_MALFORMED,
            invalid_id_message("ipam-0123456789abcdefg"),
        ),
        (
            "the upper-cased form of an id the store minted",
            held.id.to_uppercase(),
            INVALID_IPAM_ID_MALFORMED,
            invalid_id_message(&held.id.to_uppercase()),
        ),
        (
            "a well-formed long id nothing minted",
            "ipam-00000000000000000".to_string(),
            INVALID_IPAM_ID_NOT_FOUND,
            "The IPAM (ipam-00000000000000000) does not exist.".to_string(),
        ),
        (
            "a well-formed short id nothing minted",
            "ipam-0a0a0a0a".to_string(),
            INVALID_IPAM_ID_NOT_FOUND,
            "The IPAM (ipam-0a0a0a0a) does not exist.".to_string(),
        ),
    ];

    for (case, ipam_id, code, message) in rows {
        refused(
            case,
            &router,
            ACTION,
            &[param("IpamId", &ipam_id)],
            REFUSAL_STATUS,
            code,
            &message,
        )
        .await;
        // Nothing was written: the IPAM the store holds is still readable, and still not on its
        // way out.
        let read_back = required_ipam(
            case,
            &ok(case, &router, DESCRIBE, &[param("IpamId.1", &held.id)]).await,
        );
        assert_ne!(
            read_back.state.as_deref(),
            Some(IPAM_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a refused delete must leave the stored IPAM alone, got {read_back:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The pools an IPAM still holds
// ---------------------------------------------------------------------------

/// Which default scope one row's pool is created in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Private,
    Public,
}

/// The rule: an IPAM that still holds a pool cannot be deleted — `Cascade` is the one way to
/// take the pools in its PRIVATE scopes with it, and "you cannot delete the IPAM with this
/// option if there is a pool in your public scope" (`DeleteIpamRequest$Cascade`) — and a refused
/// delete writes nothing: the IPAM and its pools stay exactly as they were.
///
/// What a refusal NAMES is the resource that blocks the delete: without `Cascade` that is the
/// SCOPE the pools are attached to, which is the container a caller has to empty, so the
/// sentence carries the scope's id; the `Cascade` arm is refused for the IPAM itself, whose
/// option it is.
///
/// The rows differ along which scope holds the pool and whether the request carries `Cascade`,
/// which is what the dependency generalises over.
#[tokio::test]
async fn an_ipam_that_still_holds_a_pool_cannot_be_deleted() {
    // (case, the scopes the pools are created in, whether the request carries Cascade, whether
    //  the refusal names the blocking SCOPE rather than the IPAM, the sentence the refusal
    //  carries after the resource it is not valid for)
    let rows: [(&str, &[Scope], bool, bool, &str); 4] = [
        (
            "a private-scope pool, no cascade",
            &[Scope::Private],
            false,
            true,
            "The scope is either default scope or there are one or more pools attached to the \
             scope.",
        ),
        (
            "a public-scope pool, no cascade",
            &[Scope::Public],
            false,
            true,
            "The scope is either default scope or there are one or more pools attached to the \
             scope.",
        ),
        (
            "a public-scope pool, with Cascade",
            &[Scope::Public],
            true,
            false,
            "There are one or more pools in the public scope of the IPAM.",
        ),
        (
            "a pool in each scope, with Cascade",
            &[Scope::Private, Scope::Public],
            true,
            false,
            "There are one or more pools in the public scope of the IPAM.",
        ),
    ];

    for (case, scopes, cascade, names_scope, sentence) in rows {
        let router = emulator();
        let fixture = create_ipam(case, &router).await;
        let mut pools: Vec<String> = Vec::new();
        let mut scope_ids: Vec<String> = Vec::new();
        for scope in scopes {
            let scope_id = match scope {
                Scope::Private => &fixture.private_scope,
                Scope::Public => &fixture.public_scope,
            };
            scope_ids.push(scope_id.clone());
            pools.push(create_pool(case, &router, scope_id, &[]).await);
        }

        let mut params = vec![param("IpamId", &fixture.id)];
        if cascade {
            params.push(param("Cascade", "true"));
        }
        // The blocking scope is the one this row put a pool in; the rows that name the IPAM
        // instead are the `Cascade` ones, refused for the option rather than for a container.
        let mut blocking = scope_ids.clone();
        blocking.sort();
        let named = if names_scope {
            blocking.first().cloned().unwrap_or_default()
        } else {
            fixture.id.clone()
        };
        refused(
            case,
            &router,
            ACTION,
            &params,
            REFUSAL_STATUS,
            DEPENDENCY_VIOLATION,
            &format!("The specified action is not valid for {named}. {sentence}"),
        )
        .await;

        // Refused, so nothing moved: the IPAM is still readable and not deleting, and every
        // pool it holds is still readable and not deleting either.
        let read_back = required_ipam(
            case,
            &ok(case, &router, DESCRIBE, &[param("IpamId.1", &fixture.id)]).await,
        );
        assert_ne!(
            read_back.state.as_deref(),
            Some(IPAM_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a refused delete must leave the IPAM alone, got {read_back:?}"
        );
        for pool_id in &pools {
            let state = pool_state(case, &router, pool_id).await;
            assert!(
                state != IPAM_POOL_STATE_DELETE_IN_PROGRESS
                    && state != IPAM_POOL_STATE_DELETE_COMPLETE,
                "case {case:?}: a refused delete must leave the pool {pool_id} alone, which \
                 reports {state:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A delete of an IPAM a delete has already started on
// ---------------------------------------------------------------------------

/// The rule: a delete is asked for ONCE — an IPAM a delete has already started on is refused
/// with EC2's own wording for a resource refused solely for the state it is in, rather than
/// having the clock on its removal restarted, and once that removal is due the id is refused as
/// one the account holds no IPAM for.
#[tokio::test]
async fn a_second_delete_of_the_same_ipam_is_refused() {
    let case = "an IPAM already being deleted";
    let router = emulator();
    let fixture = create_ipam(case, &router).await;
    let answered = required_ipam(
        case,
        &ok(case, &router, ACTION, &[param("IpamId", &fixture.id)]).await,
    );
    assert_eq!(
        answered.state.as_deref(),
        Some(IPAM_STATE_DELETE_IN_PROGRESS),
        "case {case:?}: expected the first delete to answer the transient state, got {answered:?}"
    );

    // While the delete is under way the second one is refused for the state the IPAM is in; once
    // the removal has fallen due, for the id naming no IPAM at all. Which of the two a given
    // look sees depends only on how far the clock has run, so both are accepted here and the
    // state-refusal is the one asserted in full while the IPAM is still there.
    let (status, body) = invoke(&router, ACTION, &[param("IpamId", &fixture.id)]).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected the second delete to be refused, got HTTP {status} with {body}"
    );
    let code = error_code(&body);
    if code == INCORRECT_STATE {
        assert_eq!(
            error_message(&body),
            format!(
                "IPAM '{ipam}' is in the '{IPAM_STATE_DELETE_IN_PROGRESS}' state.",
                ipam = fixture.id
            ),
            "case {case:?}: expected EC2's own sentence for a resource refused for its state, \
             got {body}"
        );
    } else {
        assert_eq!(
            code, INVALID_IPAM_ID_NOT_FOUND,
            "case {case:?}: expected the second delete to be refused for the state of the IPAM \
             or for the removal that had already fallen due, got {body}"
        );
    }

    // Once the removal is due, the id names no IPAM at all — to the delete as much as to the
    // describe.
    poll_until_gone(case, &router, &fixture.id).await;
    refused(
        "an IPAM the clock has removed",
        &router,
        ACTION,
        &[param("IpamId", &fixture.id)],
        REFUSAL_STATUS,
        INVALID_IPAM_ID_NOT_FOUND,
        &format!("The IPAM ({ipam}) does not exist.", ipam = fixture.id),
    )
    .await;
}

// ---------------------------------------------------------------------------
// The dry run
// ---------------------------------------------------------------------------

/// The rule: `DryRun` "checks whether you have the required permissions for the action, without
/// actually making the request" (`DeleteIpamRequest$DryRun`), so it is answered once the
/// request's own members have been read and before any state is resolved — 412
/// `DryRunOperation` for a caller this emulator admits, with nothing written — while a value
/// whose form the operation cannot read is still refused as malformed, there being nothing to
/// look up whatever the caller may do.
///
/// The rows differ along what the dry run names: a deletable IPAM, one that still holds a pool
/// (a request a check that reads state would have refused), an id the account holds no IPAM for,
/// and a value that is no IPAM id at all.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_and_deletes_nothing() {
    let router = emulator();
    let deletable = create_ipam("a deletable IPAM", &router).await;
    let holding = create_ipam("an IPAM holding a pool", &router).await;
    let pool_id = create_pool(
        "an IPAM holding a pool",
        &router,
        &holding.private_scope,
        &[],
    )
    .await;

    // (case, the IpamId the dry run names, whether it carries Cascade)
    let probes: [(&str, &str, bool); 4] = [
        ("a deletable IPAM", &deletable.id, false),
        ("a deletable IPAM, with Cascade", &deletable.id, true),
        ("an IPAM holding a pool", &holding.id, false),
        (
            "an id the account holds no IPAM for",
            "ipam-00000000000000000",
            false,
        ),
    ];
    for (case, ipam_id, cascade) in probes {
        let mut params = vec![param("IpamId", ipam_id), param("DryRun", "true")];
        if cascade {
            params.push(param("Cascade", "true"));
        }
        refused(
            case,
            &router,
            ACTION,
            &params,
            DRY_RUN_OPERATION_STATUS,
            DRY_RUN_OPERATION,
            DRY_RUN_OPERATION_MESSAGE,
        )
        .await;
    }

    // A value the operation cannot read as an id of this kind is refused on its form, under the
    // flag as without it.
    refused(
        "a dry run naming no IPAM id at all",
        &router,
        ACTION,
        &[param("IpamId", "not-an-ipam"), param("DryRun", "true")],
        REFUSAL_STATUS,
        INVALID_IPAM_ID_MALFORMED,
        &invalid_id_message("not-an-ipam"),
    )
    .await;

    // Nothing the dry runs named was written: both IPAMs are still readable and not deleting,
    // and the pool is still in its scope.
    for (case, ipam_id) in [
        ("a deletable IPAM", &deletable.id),
        ("an IPAM holding a pool", &holding.id),
    ] {
        let read_back = required_ipam(
            case,
            &ok(case, &router, DESCRIBE, &[param("IpamId.1", ipam_id)]).await,
        );
        assert_ne!(
            read_back.state.as_deref(),
            Some(IPAM_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a dry run must delete nothing, got {read_back:?}"
        );
    }
    let state = pool_state("an IPAM holding a pool", &router, &pool_id).await;
    assert!(
        state != IPAM_POOL_STATE_DELETE_IN_PROGRESS && state != IPAM_POOL_STATE_DELETE_COMPLETE,
        "a dry run must leave the pool {pool_id} alone, which reports {state:?}"
    );
    let (status, body) = describe_pool(&router, &pool_id).await;
    assert_eq!(
        status, 200,
        "a dry run must leave the pool {pool_id} readable, got HTTP {status} with {body}"
    );
}
