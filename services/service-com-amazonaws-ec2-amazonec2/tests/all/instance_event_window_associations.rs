// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the targets of an instance event window —
//! `AssociateInstanceEventWindow` and `DisassociateInstanceEventWindow` — driven over the
//! wire the service actually speaks, `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness. The window itself (its schedule, its own tags, the filters
//! the describe narrows by) is covered by [`super::instance_event_windows`].
//!
//! The rules under test are:
//!
//! 1. A CALL THAT CHANGES A WINDOW'S TARGETS WRITES THE STORE AND REPORTS IT: the response
//!    is the window as `DescribeInstanceEventWindows` reports it afterwards — the association
//!    target carrying each of its three sets, the ids or the tags the request named in the
//!    one that holds that kind of target and nothing in the other two;
//! 2. THE WINDOW IS HANDED BACK IN A NON-TERMINAL STATE AND THE CLOCK, NOT A CALLER, MOVES
//!    IT ON: `associating` from the association and `disassociating` from the
//!    disassociation, then `active` once the span the service owes the move has passed;
//! 3. A TARGET IS A SET OF RESOURCES: associating one the window already carries leaves one
//!    target, not two, disassociating one it does not carry takes nothing off it, and a
//!    disassociation that names some of the window's targets leaves the rest;
//! 4. A TARGET NAMES RESOURCES OF ONE TYPE AND OF THIS ACCOUNT: "only one *type* of target
//!    (instance ID, instance tag, or Dedicated Host ID) can be associated with an event
//!    window", each named instance and Dedicated Host is one this account holds, and every
//!    refusal carries EC2's own code with NOTHING written.
//!
//! Rule 1 reaches `DescribeInstanceEventWindows`'s own target filters too: what an
//! association writes is what that describe narrows by.
//!
//! Rules 2 and 3 are asked ACROSS a SEQUENCE of calls as well as within one: what successive
//! associations leave the window carrying is the union of the resources they named, the same
//! set however they grouped, ordered or repeated them, and the span before `active` is owed by
//! every one of them — not only by the first.
//!
//! Each table row is a distinct input along what the rules generalise over — which type of
//! target, how many of them, which tag shape, which of the two calls, whether the target is
//! already held, each published bound and one past it — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::{
    availability_zones_in_region, DEFAULT_INSTANCE_TYPE, INSTANCE_EVENT_WINDOW_STATE_ACTIVE,
    INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING, INSTANCE_EVENT_WINDOW_STATE_DISASSOCIATING,
    INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED, INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND,
    INVALID_HOST_ID_NOT_FOUND, MAX_EVENT_WINDOW_TARGET_TAGS, MAX_TAG_KEY_CHARS, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for: an event window carries no ARN and no zone, so
/// nothing asserted here is derived from it — it is the scope the ec2Query envelope and the
/// Availability Zone of the seeded hosts are read from.
const REGION: &str = "us-west-2";

/// The AMI the seeded launches name. The launch path stores the id it is given, so an id of
/// the long form is all a launch needs.
const AMI: &str = "ami-0123456789abcdef0";

/// A well-formed event window id the account holds no window for.
const UNKNOWN_WINDOW: &str = "iew-0fedcba987654321f";

/// A string that is not an event window id at all.
const MALFORMED_WINDOW: &str = "iew-not-an-id";

/// A well-formed instance id the account holds no instance for.
const UNKNOWN_INSTANCE: &str = "i-0fedcba987654321f";

/// A well-formed Dedicated Host id the account holds no host for.
const UNKNOWN_HOST: &str = "h-0fedcba987654321f";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// Create one event window and answer the id the service minted for it. The schedule is
/// whatever satisfies the window rules — [`super::instance_event_windows`] is where the
/// schedule itself is covered.
pub(super) async fn create_window(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = api(
        router,
        "CreateInstanceEventWindow",
        &[
            p("Name", name),
            p("TimeRange.1.StartWeekDay", "monday"),
            p("TimeRange.1.StartHour", "2"),
            p("TimeRange.1.EndWeekDay", "monday"),
            p("TimeRange.1.EndHour", "8"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateInstanceEventWindow must succeed, got {status} with body {body}"
    );
    element(&body, "instanceEventWindowId")
        .unwrap_or_else(|| panic!("the create must report the id it minted: {body}"))
}

/// Launch `count` instances and answer their ids, in the order the launch reported them.
pub(super) async fn launch_instances(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
            p("InstanceType", DEFAULT_INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let launched = elements(&body, "instanceId");
    assert_eq!(
        launched.len(),
        count,
        "RunInstances must report {count} instances, got {launched:?} in {body}"
    );
    launched
}

/// Allocate `count` Dedicated Hosts in the Region's first zone and answer their ids.
pub(super) async fn allocate_hosts(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let zone = availability_zones_in_region(REGION)[0].zone_name.clone();
    let (status, body) = api(
        router,
        "AllocateHosts",
        &[
            p("InstanceType", DEFAULT_INSTANCE_TYPE),
            p("AvailabilityZone", zone),
            p("Quantity", count.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "AllocateHosts must succeed, got {status} with body {body}"
    );
    let allocated = element(&body, "hostIdSet")
        .map(|set| elements(&set, "item"))
        .unwrap_or_default();
    assert_eq!(
        allocated.len(),
        count,
        "AllocateHosts must report {count} hosts, got {allocated:?} in {body}"
    );
    allocated
}

/// The association target one window reports, read off the wire.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) instance_ids: Vec<String>,
    pub(super) tags: Vec<(String, String)>,
    pub(super) dedicated_host_ids: Vec<String>,
}

impl Target {
    pub(super) fn of_instances(ids: &[String]) -> Self {
        Self {
            instance_ids: ids.to_vec(),
            ..Self::default()
        }
    }

    pub(super) fn of_tags(tags: &[(&str, &str)]) -> Self {
        Self {
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            ..Self::default()
        }
    }

    pub(super) fn of_hosts(ids: &[String]) -> Self {
        Self {
            dedicated_host_ids: ids.to_vec(),
            ..Self::default()
        }
    }
}

/// The association target the one window of `body` reports, or `None` when it reports no
/// target element at all.
///
/// Read through the XML tree rather than by substring: the window has a `tagSet` of its OWN
/// tags, and the tags of a target are the `tagSet` INSIDE `associationTarget`. Shared with
/// [`super::instance_event_windows`], which asserts what a freshly created window is
/// associated with, so one reader serves every suite that reads a window's targets.
pub(super) fn reported_target(case: &str, body: &str) -> Option<Target> {
    let parsed = document(case, body);
    let window = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "instanceEventWindowId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no event window in {body}"));
    let target = child(window, "associationTarget")?;
    let ids = |name: &str| -> Vec<String> {
        set_items(target, name)
            .into_iter()
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect()
    };
    Some(Target {
        instance_ids: ids("instanceIdSet"),
        tags: set_items(target, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
        dedicated_host_ids: ids("dedicatedHostIdSet"),
    })
}

/// The `InstanceEventWindowState` the one window of `body` reports.
fn reported_state(case: &str, body: &str) -> String {
    let parsed = document(case, body);
    let window = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "instanceEventWindowId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no event window in {body}"));
    child_text(window, "state")
        .unwrap_or_else(|| panic!("case {case:?}: no <state> on the window in {body}"))
}

/// The target the describe reports for `window_id`, and the state it reports with it.
pub(super) async fn described(case: &str, router: &MultiServiceRouter, window_id: &str) -> (Option<Target>, String) {
    let (status, body) = api(
        router,
        "DescribeInstanceEventWindows",
        &[p("InstanceEventWindowId.1", window_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    (reported_target(case, &body), reported_state(case, &body))
}

/// `AssociationTarget.InstanceId.N` / `.DedicatedHostId.N` / `.InstanceTag.N.Key` form
/// fields for one target, as ec2Query flattens the three lists of the nested
/// `AssociationTarget` structure (each list's member is named in the singular on the wire).
fn target_params(target: &Target) -> Params {
    let mut params = Params::new();
    for (index, instance_id) in target.instance_ids.iter().enumerate() {
        params.push(p(
            &format!("AssociationTarget.InstanceId.{}", index + 1),
            instance_id.clone(),
        ));
    }
    for (index, (key, value)) in target.tags.iter().enumerate() {
        params.push(p(
            &format!("AssociationTarget.InstanceTag.{}.Key", index + 1),
            key.clone(),
        ));
        params.push(p(
            &format!("AssociationTarget.InstanceTag.{}.Value", index + 1),
            value.clone(),
        ));
    }
    for (index, host_id) in target.dedicated_host_ids.iter().enumerate() {
        params.push(p(
            &format!("AssociationTarget.DedicatedHostId.{}", index + 1),
            host_id.clone(),
        ));
    }
    params
}

/// One call that changes a window's targets: the window, the targets it names and nothing
/// else.
fn change_params(window_id: &str, target: &Target) -> Params {
    let mut params = vec![p("InstanceEventWindowId", window_id)];
    params.extend(target_params(target));
    params
}

pub(super) async fn associate(router: &MultiServiceRouter, window_id: &str, target: &Target) -> (u16, String) {
    api(
        router,
        "AssociateInstanceEventWindow",
        &change_params(window_id, target),
    )
    .await
}

async fn disassociate(
    router: &MultiServiceRouter,
    window_id: &str,
    target: &Target,
) -> (u16, String) {
    api(
        router,
        "DisassociateInstanceEventWindow",
        &change_params(window_id, target),
    )
    .await
}

/// One row of the association table: the targets to associate and the target the window must
/// report afterwards. Built after the resources are seeded, because an instance id and a host
/// id are minted by the service and not written by the test.
struct Row {
    case: String,
    associate: Target,
}

/// The table: one row per type of target, per count, and per tag shape — the things the
/// rules generalise over.
fn rows(instances: &[String], hosts: &[String]) -> Vec<Row> {
    vec![
        Row {
            case: "one instance id".to_string(),
            associate: Target::of_instances(&instances[..1]),
        },
        Row {
            case: "two instance ids".to_string(),
            associate: Target::of_instances(&instances[..2]),
        },
        Row {
            case: "one instance tag".to_string(),
            associate: Target::of_tags(&[("Name", "web")]),
        },
        Row {
            case: "two instance tags".to_string(),
            associate: Target::of_tags(&[("Name", "web"), ("Team", "payments")]),
        },
        Row {
            // A tag key with no value selects by key alone, so the empty value is a shape of
            // its own and not a missing member.
            case: "an instance tag with an empty value".to_string(),
            associate: Target::of_tags(&[("Stage", "")]),
        },
        Row {
            case: "one Dedicated Host id".to_string(),
            associate: Target::of_hosts(&hosts[..1]),
        },
        Row {
            case: "two Dedicated Host ids".to_string(),
            associate: Target::of_hosts(&hosts[..2]),
        },
    ]
}

/// Rule 1 and rule 2 for the association: the window the response reports carries the targets
/// the request named and nothing else, the describe reports the same window, and the state is
/// the non-terminal `associating`.
#[tokio::test]
async fn an_association_writes_the_targets_the_request_named() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let hosts = allocate_hosts(&router, 2).await;
    for row in rows(&instances, &hosts) {
        let case = &row.case;
        let window_id = create_window(&router, "probe").await;
        let (status, body) = associate(&router, &window_id, &row.associate).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            reported_target(case, &body),
            Some(row.associate.clone()),
            "case {case:?}: the association must report the targets the request named, \
             got body {body}"
        );
        assert_eq!(
            reported_state(case, &body),
            INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING,
            "case {case:?}: an associating window is not yet active, got body {body}"
        );

        let (described_target, described_state) = described(case, &router, &window_id).await;
        assert_eq!(
            described_target,
            Some(row.associate.clone()),
            "case {case:?}: the describe must report the same target the association did"
        );
        assert_eq!(
            described_state, INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING,
            "case {case:?}: the describe must report the same state the association did"
        );
    }
}

/// Rule 2: the clock, not a caller, moves an associating or a disassociating window to
/// `active`, and the targets it holds do not change when it does.
#[tokio::test]
async fn the_clock_moves_a_changed_window_to_active() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let hosts = allocate_hosts(&router, 2).await;
    for row in rows(&instances, &hosts) {
        let case = &row.case;
        let window_id = create_window(&router, "probe").await;
        let (status, body) = associate(&router, &window_id, &row.associate).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        past_the_transition().await;
        let (target, state) = described(case, &router, &window_id).await;
        assert_eq!(
            state, INSTANCE_EVENT_WINDOW_STATE_ACTIVE,
            "case {case:?}: the clock must move an associated window to active"
        );
        assert_eq!(
            target,
            Some(row.associate.clone()),
            "case {case:?}: the move to active must not change the window's targets"
        );

        let (status, body) = disassociate(&router, &window_id, &row.associate).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the disassociation, got {status} \
             with body {body}"
        );
        assert_eq!(
            reported_state(case, &body),
            INSTANCE_EVENT_WINDOW_STATE_DISASSOCIATING,
            "case {case:?}: a disassociating window is not yet active, got body {body}"
        );
        // Rule 1 for the disassociation: every set is reported, and the one the request
        // emptied is empty rather than absent.
        assert_eq!(
            reported_target(case, &body),
            Some(Target::default()),
            "case {case:?}: the disassociation must report the window with no targets left, \
             got body {body}"
        );
        past_the_transition().await;
        let (target, state) = described(case, &router, &window_id).await;
        assert_eq!(
            state, INSTANCE_EVENT_WINDOW_STATE_ACTIVE,
            "case {case:?}: the clock must move a disassociated window to active"
        );
        assert_eq!(
            target,
            Some(Target::default()),
            "case {case:?}: a disassociated window holds no targets"
        );
    }
}

/// Rule 3: a target is a SET of resources — associating one twice leaves one target,
/// disassociating one the window does not carry takes nothing off it, and a disassociation
/// that names some of the targets leaves the others.
#[tokio::test]
async fn a_targets_set_is_a_set() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let hosts = allocate_hosts(&router, 2).await;
    for row in rows(&instances, &hosts) {
        let case = &row.case;
        let window_id = create_window(&router, "probe").await;
        for attempt in 1..=2 {
            let (status, body) = associate(&router, &window_id, &row.associate).await;
            assert_eq!(
                status, 200,
                "case {case:?}: association {attempt} must succeed, got {status} \
                 with body {body}"
            );
            assert_eq!(
                reported_target(case, &body),
                Some(row.associate.clone()),
                "case {case:?}: association {attempt} must leave one target per resource, \
                 got body {body}"
            );
        }

        // A target the window does not carry takes nothing off it: the call succeeds and the
        // window reports exactly what it held.
        let untouched = Target::of_tags(&[("NotAssociated", "never")]);
        let (status, body) = disassociate(&router, &window_id, &untouched).await;
        assert_eq!(
            status, 200,
            "case {case:?}: disassociating a target the window does not carry must succeed, \
             got {status} with body {body}"
        );
        assert_eq!(
            reported_target(case, &body),
            Some(row.associate.clone()),
            "case {case:?}: disassociating a target the window does not carry must leave its \
             targets alone, got body {body}"
        );
    }

    // A disassociation that names SOME of the window's targets leaves the rest: two ids in,
    // one out, one left.
    let window_id = create_window(&router, "partial").await;
    let both = Target::of_instances(&instances[..2]);
    let (status, body) = associate(&router, &window_id, &both).await;
    assert_eq!(
        status, 200,
        "the association of both instances must succeed, got {status} with body {body}"
    );
    let first = Target::of_instances(&instances[..1]);
    let (status, body) = disassociate(&router, &window_id, &first).await;
    assert_eq!(
        status, 200,
        "the partial disassociation must succeed, got {status} with body {body}"
    );
    assert_eq!(
        reported_target("partial", &body),
        Some(Target::of_instances(&instances[1..2])),
        "the partial disassociation must leave the target it did not name, got body {body}"
    );
}

/// One row of the refusal table: what the call is, the parameters, the code EC2 answers,
/// and the window whose targets must be exactly what they were once the call is refused.
struct RefusalRow {
    case: String,
    action: &'static str,
    params: Params,
    code: &'static str,
    window: String,
    held: Option<Target>,
}

/// Rule 4: every refusal carries EC2's own code, and the window the request named is left
/// exactly as it was.
#[tokio::test]
async fn a_refused_change_carries_ec2s_code_and_writes_nothing() {
    let router = emulator();
    let instances = launch_instances(&router, 1).await;
    let hosts = allocate_hosts(&router, 1).await;
    // A window already associated with instance tags: the rows that need a window with
    // targets of one type read this one.
    let tagged = create_window(&router, "tagged").await;
    let held = Target::of_tags(&[("Name", "web")]);
    let (status, body) = associate(&router, &tagged, &held).await;
    assert_eq!(
        status, 200,
        "the seeding association must succeed, got {status} with body {body}"
    );
    // A window nothing is associated with: the rows about resolving the resources a target
    // names read this one, so what is refused is the resource and not the type of target.
    let bare = create_window(&router, "bare").await;

    let mixed = Target {
        instance_ids: instances[..1].to_vec(),
        tags: vec![("Name".to_string(), "web".to_string())],
        dedicated_host_ids: Vec::new(),
    };
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let too_many_tags: Vec<(String, String)> = (0..=MAX_EVENT_WINDOW_TARGET_TAGS)
        .map(|index| (format!("Key{index}"), "value".to_string()))
        .collect();

    let rows = vec![
        RefusalRow {
            case: "an event window id the account holds no window for".to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(UNKNOWN_WINDOW, &held),
            code: INVALID_INSTANCE_EVENT_WINDOW_ID_NOT_FOUND,
            window: tagged.clone(),
            held: Some(held.clone()),
        },
        RefusalRow {
            case: "an event window id that is not one at all".to_string(),
            action: "DisassociateInstanceEventWindow",
            params: change_params(MALFORMED_WINDOW, &held),
            code: INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED,
            window: tagged.clone(),
            held: Some(held.clone()),
        },
        RefusalRow {
            case: "an association naming two types of target".to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&tagged, &mixed),
            code: "InvalidParameterValue",
            window: tagged.clone(),
            held: Some(held.clone()),
        },
        RefusalRow {
            case: "an association of instances onto a window associated with tags".to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&tagged, &Target::of_instances(&instances[..1])),
            code: "InvalidParameterValue",
            window: tagged.clone(),
            held: Some(held.clone()),
        },
        RefusalRow {
            case: "an association of Dedicated Hosts onto a window associated with tags"
                .to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&tagged, &Target::of_hosts(&hosts[..1])),
            code: "InvalidParameterValue",
            window: tagged.clone(),
            held: Some(held.clone()),
        },
        RefusalRow {
            case: "an association naming an instance the account did not launch".to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&bare, &Target::of_instances(&[UNKNOWN_INSTANCE.to_string()])),
            code: "InvalidInstanceID.NotFound",
            window: bare.clone(),
            held: Some(Target::default()),
        },
        RefusalRow {
            case: "an association naming something that is not an instance id".to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&bare, &Target::of_instances(&["i-not-an-id".to_string()])),
            code: "InvalidInstanceID.Malformed",
            window: bare.clone(),
            held: Some(Target::default()),
        },
        RefusalRow {
            case: "an association naming a Dedicated Host the account did not allocate"
                .to_string(),
            action: "AssociateInstanceEventWindow",
            params: change_params(&bare, &Target::of_hosts(&[UNKNOWN_HOST.to_string()])),
            code: INVALID_HOST_ID_NOT_FOUND,
            window: bare.clone(),
            held: Some(Target::default()),
        },
        RefusalRow {
            // One past the published tag key length ([`MAX_TAG_KEY_CHARS`]); the bound itself
            // is served below.
            case: format!("a tag key of {} characters", long_key.chars().count()),
            action: "AssociateInstanceEventWindow",
            params: change_params(
                &bare,
                &Target {
                    tags: vec![(long_key.clone(), "value".to_string())],
                    ..Target::default()
                },
            ),
            code: "InvalidParameterValue",
            window: bare.clone(),
            held: Some(Target::default()),
        },
        RefusalRow {
            // One past the number of target tags this service stores; the bound itself is
            // served below.
            case: format!("{} target tags", too_many_tags.len()),
            action: "AssociateInstanceEventWindow",
            params: change_params(
                &bare,
                &Target {
                    tags: too_many_tags.clone(),
                    ..Target::default()
                },
            ),
            code: "InvalidParameterValue",
            window: bare.clone(),
            held: Some(Target::default()),
        },
    ];

    for row in rows {
        let case = &row.case;
        let (status, body) = api(&router, row.action, &row.params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, row.code),
            "case {case:?}: expected HTTP 400 {code}, got {status} with body {body}",
            code = row.code
        );
        let (target, _) = described(case, &router, &row.window).await;
        assert_eq!(
            target, row.held,
            "case {case:?}: a refused call must leave the window's targets alone"
        );
    }

    // The bounds themselves are served: a tag key of exactly the published length, and as
    // many target tags as this service stores.
    let at_the_bound = Target {
        tags: vec![("k".repeat(MAX_TAG_KEY_CHARS), "value".to_string())],
        ..Target::default()
    };
    let (status, body) = associate(&router, &create_window(&router, "bound").await, &at_the_bound)
        .await;
    assert_eq!(
        status, 200,
        "a tag key of {MAX_TAG_KEY_CHARS} characters is served, got {status} with body {body}"
    );
    let full = Target {
        tags: too_many_tags[..MAX_EVENT_WINDOW_TARGET_TAGS].to_vec(),
        ..Target::default()
    };
    let filled = create_window(&router, "full").await;
    let (status, body) = associate(&router, &filled, &full).await;
    assert_eq!(
        status, 200,
        "{MAX_EVENT_WINDOW_TARGET_TAGS} target tags are served, got {status} with body {body}"
    );
    assert_eq!(
        reported_target("full", &body),
        Some(full.clone()),
        "the window must report every target tag the request named, got body {body}"
    );

    // The ceiling is on what the WINDOW holds, not on one call: associating the same tags
    // again adds none and is served, while one FURTHER tag would take the window past it.
    let (status, body) = associate(&router, &filled, &full).await;
    assert_eq!(
        status, 200,
        "re-associating the tags the window already holds is served, got {status} \
         with body {body}"
    );
    let one_more = Target::of_tags(&[("OneMore", "value")]);
    let (status, body) = associate(&router, &filled, &one_more).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "a tag past the {MAX_EVENT_WINDOW_TARGET_TAGS} a window holds is refused, got \
         {status} with body {body}"
    );
    let (target, _) = described("one past the window's tags", &router, &filled).await;
    assert_eq!(
        target,
        Some(full),
        "the refused association must leave the window's targets alone"
    );
}

/// Rule 1 reaching the DESCRIBE's own filters: the targets an association writes are what
/// `DescribeInstanceEventWindows` narrows by, so a window is selected by each filter its
/// kind of target answers and by none of the others.
///
/// One row per documented target filter — `dedicated-host-id`, `instance-id`,
/// `instance-tag`, `instance-tag-key`, `instance-tag-value`
/// (`DescribeInstanceEventWindowsRequest$Filters`) — against a window of each kind, so the
/// filter that selects a window of ONE kind is told apart from the ones that select none.
#[tokio::test]
async fn the_describe_narrows_by_the_targets_an_association_wrote() {
    let router = emulator();
    let instances = launch_instances(&router, 1).await;
    let hosts = allocate_hosts(&router, 1).await;

    let by_instance = create_window(&router, "by-instance").await;
    let by_tag = create_window(&router, "by-tag").await;
    let by_host = create_window(&router, "by-host").await;
    for (window_id, target) in [
        (&by_instance, Target::of_instances(&instances[..1])),
        (&by_tag, Target::of_tags(&[("Name", "web")])),
        (&by_host, Target::of_hosts(&hosts[..1])),
    ] {
        let (status, body) = associate(&router, window_id, &target).await;
        assert_eq!(
            status, 200,
            "seeding {window_id} must succeed, got {status} with body {body}"
        );
    }

    // `(case, the filter, its values, the window the filter must select)`.
    let rows: Vec<(&str, &str, Vec<&str>, &String)> = vec![
        (
            "instance-id",
            "instance-id",
            vec![instances[0].as_str()],
            &by_instance,
        ),
        ("instance-tag", "instance-tag", vec!["Name=web"], &by_tag),
        ("instance-tag-key", "instance-tag-key", vec!["Name"], &by_tag),
        (
            "instance-tag-value",
            "instance-tag-value",
            vec!["web"],
            &by_tag,
        ),
        (
            "dedicated-host-id",
            "dedicated-host-id",
            vec![hosts[0].as_str()],
            &by_host,
        ),
    ];
    for (case, name, values, expected) in rows {
        let params = filters(&[(name, values.as_slice())]);
        let (status, body) = api(&router, "DescribeInstanceEventWindows", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "instanceEventWindowId"),
            vec![expected.clone()],
            "case {case:?}: the filter must select the window the association targeted, \
             got body {body}"
        );
    }

    // A value no target carries selects no window at all, so the filters above are narrowing
    // rather than matching everything.
    let params = filters(&[("instance-tag-key", &["NotAssociated"])]);
    let (status, body) = api(&router, "DescribeInstanceEventWindows", &params).await;
    assert_eq!(
        status, 200,
        "the describe must succeed, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "instanceEventWindowId"),
        Vec::<String>::new(),
        "a tag key nothing is associated with selects no window, got body {body}"
    );
}

/// Rule 4's dry run: "the error response is DryRunOperation" and the store is untouched,
/// for both of the calls that change a window's targets.
#[tokio::test]
async fn a_dry_run_changes_no_target() {
    let router = emulator();
    let window_id = create_window(&router, "dry-run").await;
    let held = Target::of_tags(&[("Name", "web")]);

    let mut params = change_params(&window_id, &held);
    params.push(p("DryRun", "true"));
    let (status, body) = api(&router, "AssociateInstanceEventWindow", &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "a dry-run association is refused as a dry run, got {status} with body {body}"
    );
    let (target, _) = described("dry-run association", &router, &window_id).await;
    assert_eq!(
        target,
        Some(Target::default()),
        "a dry-run association must associate nothing at all"
    );

    let (status, body) = associate(&router, &window_id, &held).await;
    assert_eq!(
        status, 200,
        "the association must succeed, got {status} with body {body}"
    );
    let mut params = change_params(&window_id, &held);
    params.push(p("DryRun", "true"));
    let (status, body) = api(&router, "DisassociateInstanceEventWindow", &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "a dry-run disassociation is refused as a dry run, got {status} with body {body}"
    );
    let (target, _) = described("dry-run disassociation", &router, &window_id).await;
    assert_eq!(
        target,
        Some(held.clone()),
        "a dry-run disassociation must leave every target on the window"
    );
}

/// Which kind of target a row of the accumulation table names, as three distinct targets of
/// that kind: an index picks one of the three, so ONE table of call shapes drives every kind
/// of target a window takes.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Instances,
    Tags,
    Hosts,
}

/// The three tag targets the [`Kind::Tags`] rows select by. Each carries a key of its own, so
/// each is a target of its own under the key/value pair a tag target is compared by.
const TAG_TARGETS: [(&str, &str); 3] =
    [("Name", "web"), ("Team", "payments"), ("Stage", "production")];

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Instances => "instance ids",
            Kind::Tags => "instance tags",
            Kind::Hosts => "Dedicated Host ids",
        }
    }

    /// The target naming the resources `indices` picks out of the seeded ones.
    fn target(self, indices: &[usize], instances: &[String], hosts: &[String]) -> Target {
        let pick = |ids: &[String]| -> Vec<String> {
            indices.iter().map(|index| ids[*index].clone()).collect()
        };
        match self {
            Kind::Instances => Target::of_instances(&pick(instances)),
            Kind::Tags => Target::of_tags(
                &indices
                    .iter()
                    .map(|index| TAG_TARGETS[*index])
                    .collect::<Vec<_>>(),
            ),
            Kind::Hosts => Target::of_hosts(&pick(hosts)),
        }
    }
}

/// `target` with each of its three sets in a canonical order.
///
/// A target is a SET of resources, so which call named which resource is not part of what the
/// window carries afterwards; comparing two targets as sets is what lets one expectation
/// serve every grouping and ordering of the calls that wrote them.
fn as_sets(target: &Target) -> Target {
    let mut sorted = target.clone();
    sorted.instance_ids.sort();
    sorted.tags.sort();
    sorted.dedicated_host_ids.sort();
    sorted
}

/// Rule 3 ACROSS CALLS: "associates one or more targets with an event window" ADDS to what
/// the window already carries, so after any number of associations the window carries the
/// union of the resources they named — the same set however those calls grouped, ordered or
/// repeated them — and `DescribeInstanceEventWindows` reports that union too.
///
/// One row per shape of the call sequence that builds one three-resource target (how many
/// calls, how the resources are grouped, which order they are named in, whether two calls
/// overlap), against each of the three kinds of target, so no single sequence is the
/// specification.
#[tokio::test]
async fn associations_leave_the_window_the_union_of_what_they_named() {
    let router = emulator();
    let instances = launch_instances(&router, 3).await;
    let hosts = allocate_hosts(&router, 3).await;

    let sequences: Vec<(&str, Vec<Vec<usize>>)> = vec![
        ("all three in one call", vec![vec![0, 1, 2]]),
        ("one, then the other two", vec![vec![0], vec![1, 2]]),
        ("two, then the third", vec![vec![0, 1], vec![2]]),
        ("one at a time, in order", vec![vec![0], vec![1], vec![2]]),
        ("one at a time, in reverse", vec![vec![2], vec![1], vec![0]]),
        ("two calls that overlap", vec![vec![0, 1], vec![1, 2]]),
    ];

    for kind in [Kind::Instances, Kind::Tags, Kind::Hosts] {
        for (shape, calls) in &sequences {
            let case = format!("{} associated {shape}", kind.label());
            let window_id = create_window(&router, "union").await;
            // The resources named so far, in the order the calls named them: what the window
            // must carry after each one.
            let mut named: Vec<usize> = Vec::new();
            for group in calls {
                let (status, body) =
                    associate(&router, &window_id, &kind.target(group, &instances, &hosts)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: association of {group:?} must succeed, got {status} \
                     with body {body}"
                );
                for index in group {
                    if !named.contains(index) {
                        named.push(*index);
                    }
                }
                let union = as_sets(&kind.target(&named, &instances, &hosts));
                let reported = reported_target(&case, &body).map(|target| as_sets(&target));
                assert_eq!(
                    reported,
                    Some(union),
                    "case {case:?}: after associating {group:?} the window must carry \
                     {named:?} and nothing else, got {reported:?} in body {body}"
                );
            }

            let all = as_sets(&kind.target(&[0, 1, 2], &instances, &hosts));
            let (described_target, _) = described(&case, &router, &window_id).await;
            let described_sets = described_target.as_ref().map(as_sets);
            assert_eq!(
                described_sets,
                Some(all),
                "case {case:?}: the describe must report every resource the calls named, \
                 got {described_sets:?}"
            );
        }
    }
}

/// Rule 2 ACROSS CALLS: the span before `active` is owed by EVERY association and not only by
/// the first, so a window that has already reached `active` reports `associating` again once a
/// further association is made, carries the grown target while it does, and is moved back to
/// `active` by the clock rather than by a caller.
#[tokio::test]
async fn a_further_association_owes_the_move_to_active_again() {
    let router = emulator();
    let instances = launch_instances(&router, 2).await;
    let window_id = create_window(&router, "again").await;

    let (status, body) = associate(
        &router,
        &window_id,
        &Target::of_instances(&instances[..1]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first association must succeed, got {status} with body {body}"
    );
    past_the_transition().await;
    let (_, state) = described("the first association", &router, &window_id).await;
    assert_eq!(
        state, INSTANCE_EVENT_WINDOW_STATE_ACTIVE,
        "case \"the first association\": the clock must move the window to active before the \
         further association, got {state:?}"
    );

    let (status, body) = associate(
        &router,
        &window_id,
        &Target::of_instances(&instances[1..2]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the further association must succeed, got {status} with body {body}"
    );
    let both = Target::of_instances(&instances[..2]);
    assert_eq!(
        reported_state("the further association", &body),
        INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING,
        "case \"the further association\": a further association must hand an active window \
         back associating, got body {body}"
    );
    let (target, state) = described("the further association", &router, &window_id).await;
    let reported = target.as_ref().map(as_sets);
    assert_eq!(
        state, INSTANCE_EVENT_WINDOW_STATE_ASSOCIATING,
        "case \"the further association\": the describe must report the window associating \
         until the span has passed, got {state:?}"
    );
    assert_eq!(
        reported,
        Some(as_sets(&both)),
        "case \"the further association\": an associating window already carries both \
         instances, got {reported:?}"
    );

    past_the_transition().await;
    let (target, state) = described("after the further span", &router, &window_id).await;
    let reported = target.as_ref().map(as_sets);
    assert_eq!(
        state, INSTANCE_EVENT_WINDOW_STATE_ACTIVE,
        "case \"after the further span\": the clock must move the window to active again, \
         got {state:?}"
    );
    assert_eq!(
        reported,
        Some(as_sets(&both)),
        "case \"after the further span\": the move to active must not change the window's \
         targets, got {reported:?}"
    );
}
