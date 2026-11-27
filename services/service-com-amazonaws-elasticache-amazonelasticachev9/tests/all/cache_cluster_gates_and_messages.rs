// SPDX-License-Identifier: CC0-1.0

//! The four rules this suite pins, each over a SET of inputs rather than the one
//! recorded request the issue was filed from:
//!
//! 1. MAINTENANCE WINDOW — a `PreferredMaintenanceWindow` is accepted only in
//!    the form the model documents (a `ddd:hh24:mi-ddd:hh24:mi` range on the
//!    24-hour UTC clock, spanning at least 60 minutes); anything else is refused
//!    with HTTP 400 `InvalidParameterValue` and leaves the stored window alone.
//!    The table spans the operations that accept a window, the published day
//!    abbreviations, the clock bounds, the day the range crosses and the span,
//!    with the documented minimum and one minute under it as the bound rows.
//!
//! 2. THE STATE GATE — a cluster already working through a modification still
//!    accepts the next `ModifyCacheCluster` (the documented supersede), while a
//!    cluster that is not provisioned yet does not; and the gate runs AFTER the
//!    checks that read nothing but the request, so a malformed value is reported
//!    as the bad parameter it is whatever status the cluster is in.
//!
//! 3. THE CACHE-SECURITY-GROUP CLOSURE — the construct is unavailable for this
//!    account, so every operation that names one answers the account-wide
//!    `InvalidParameterValue` closure rather than a per-name not-found.
//!
//! 4. THE CLUSTER MESSAGES — every surface that resolves a `CacheClusterId`
//!    words a missing cache cluster the way the service words it there (the
//!    shape's own spelling on the surfaces that only resolve the id, AWS's prose
//!    on the snapshot creator), and every surface that refuses a malformed
//!    `CacheClusterId` states the one identifier rule and names the one
//!    parameter.
//!
//! Everything goes over the wire through the same router the production emulator
//! mounts, via the `pub(crate)` harness the `cache_cluster_lifecycle` suite owns.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, create_cluster, describe_cluster};
use crate::harness::{hold_pending_changes, params, Emulator};

/// The identifier rule this service states when a value cannot name a cache
/// cluster, naming the member by the parameter label ElastiCache uses for it
/// (`state::parameter_identifier_refusal`).
const IDENTIFIER_RULE: &str = "The parameter CacheClusterIdentifier is not a valid identifier. \
                               Identifiers must begin with a letter; must contain only ASCII \
                               letters, digits, and hyphens; and must not end with a hyphen or \
                               contain two consecutive hyphens.";

/// One call row: the case name, the `Action`, and the request's form pairs.
type CallCase = (&'static str, &'static str, Vec<(String, String)>);

/// One maintenance-window row: the case name, the value, and the message the
/// refusal must carry — `None` when the documented form accepts the value.
type WindowCase = (&'static str, &'static str, Option<&'static str>);

/// What a value that is not a range of two `ddd:hh24:mi` points is refused with.
const FORMAT_RULE: &str = state::INVALID_MAINTENANCE_WINDOW_MESSAGE;

/// What a well-formed range shorter than the documented minimum is refused with;
/// the bound is checked against the one the state module publishes below.
const SPAN_RULE: &str = "The maintenance window must be at least 60 minutes.";

/// The window table: every row differs from its neighbours along what the rule
/// generalises over — the day named, the clock fields, whether the range crosses
/// a day or the week's end, and the span it covers.
///
/// Sourced from `PreferredMaintenanceWindow`
/// (`smithy_query(query="doc",shape="ModifyCacheClusterMessage",member="PreferredMaintenanceWindow")`):
/// "a range in the format ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC). The minimum
/// maintenance window is a 60 minute period", with `ddd` one of sun, mon, tue,
/// wed, thu, fri, sat.
const WINDOW_CASES: &[WindowCase] = &[
    // The member's own example, crossing into the next day.
    ("the documented example", "sun:23:00-mon:01:30", None),
    // A day the issue never cited, and exactly the documented 60-minute minimum.
    ("exactly the minimum span", "wed:09:00-wed:10:00", None),
    // One minute under it.
    (
        "a minute under the minimum",
        "wed:09:00-wed:09:59",
        Some(SPAN_RULE),
    ),
    // The two ends at the same point span nothing at all.
    ("a window of no length", "thu:09:00-thu:09:00", Some(SPAN_RULE)),
    // Wrapping the week's end.
    ("wrapping sat into sun", "sat:23:30-sun:00:30", None),
    // The clock's published bounds: midnight, and the last minute of the hour.
    ("the first minute of a day", "tue:00:00-tue:01:00", None),
    ("the last minute of a day", "fri:22:00-fri:23:59", None),
    // One past each bound.
    (
        "an hour past the 24-hour clock",
        "sun:24:00-mon:01:00",
        Some(FORMAT_RULE),
    ),
    (
        "a minute past the clock",
        "sun:23:60-mon:01:00",
        Some(FORMAT_RULE),
    ),
    // `ddd` is matched case-insensitively.
    ("an upper-case day", "SUN:05:00-SUN:06:00", None),
    // The recorded instances the issue reported, and the other ways a value can
    // fail to be a range of two clock points.
    (
        "a day that is not a day",
        "notaday:99:99-notaday:100:00",
        Some(FORMAT_RULE),
    ),
    ("not a range at all", "not-a-valid-window", Some(FORMAT_RULE)),
    ("only one end", "sun:23:00", Some(FORMAT_RULE)),
    (
        "one-digit clock fields",
        "sun:2:00-mon:1:30",
        Some(FORMAT_RULE),
    ),
    (
        "a fourth clock field",
        "sun:23:00:00-mon:01:30",
        Some(FORMAT_RULE),
    ),
    ("an empty day", ":23:00-mon:01:30", Some(FORMAT_RULE)),
];

/// The window a cluster under test starts from, so a refused call can be shown
/// to have changed nothing. Not the service default, so "unchanged" is
/// distinguishable from "reset".
const STARTING_WINDOW: &str = "mon:03:00-mon:05:00";

#[tokio::test]
async fn a_maintenance_window_is_accepted_only_in_the_documented_form() {
    let emulator = Emulator::new();

    // The span rule's wording states the bound the state module publishes, from
    // "The minimum maintenance window is a 60 minute period."
    assert_eq!(
        SPAN_RULE,
        format!(
            "The maintenance window must be at least {} minutes.",
            state::MAINTENANCE_WINDOW_MIN_MINUTES
        ),
        "the span refusal must state the published minimum ({})",
        state::MAINTENANCE_WINDOW_MIN_MINUTES
    );

    // A cluster to modify, carrying a window of its own.
    create_available_cluster(
        &emulator,
        "window-target",
        &[("PreferredMaintenanceWindow", STARTING_WINDOW)],
    )
    .await;

    for (index, (case, window, refusal)) in WINDOW_CASES.iter().enumerate() {
        let accepted = &refusal.is_none();
        // CREATE: the window reaches the stored cluster, or no cluster is
        // created at all.
        let created_id = format!("window-create-{index}");
        let created = create_cluster(
            &emulator,
            &created_id,
            &[("PreferredMaintenanceWindow", window)],
        )
        .await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case {case:?} ({window:?}): CreateCacheCluster must accept the window, got {} {}",
                created.status, created.body
            );
            let described = describe_cluster(&emulator, &created_id).await;
            assert_eq!(
                described.text("CacheCluster/PreferredMaintenanceWindow"),
                Some(*window),
                "case {case:?}: the created cluster must carry the window, got {:?}",
                described.text("CacheCluster/PreferredMaintenanceWindow")
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?} ({window:?}): CreateCacheCluster must refuse the window, got {} {}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                created.body
            );
            let described = describe_cluster(&emulator, &created_id).await;
            assert_eq!(
                described.error_code(),
                "CacheClusterNotFound",
                "case {case:?}: a refused create must store no cluster, got {} {}",
                described.status, described.body
            );
        }

        // MODIFY: the same rule, and a refusal leaves the running window alone.
        let modified = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "window-target"),
                    ("PreferredMaintenanceWindow", window),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        let described = describe_cluster(&emulator, "window-target").await;
        if *accepted {
            assert_eq!(
                modified.status, 200,
                "case {case:?} ({window:?}): ModifyCacheCluster must accept the window, got {} {}",
                modified.status, modified.body
            );
            assert_eq!(
                described.text("CacheCluster/PreferredMaintenanceWindow"),
                Some(*window),
                "case {case:?}: the modified cluster must carry the window, got {:?}",
                described.text("CacheCluster/PreferredMaintenanceWindow")
            );
            // Put the starting window back, so the next row's refusal is
            // measured against a known value.
            let restored = emulator
                .call(
                    "ModifyCacheCluster",
                    &params(&[
                        ("CacheClusterId", "window-target"),
                        ("PreferredMaintenanceWindow", STARTING_WINDOW),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: restoring the starting window must succeed, got {}",
                restored.body
            );
        } else {
            assert_eq!(
                modified.status, 400,
                "case {case:?} ({window:?}): ModifyCacheCluster must refuse the window, got {} {}",
                modified.status, modified.body
            );
            assert_eq!(
                modified.error_code(),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                modified.body
            );
            assert_eq!(
                Some(modified.error_message()),
                *refusal,
                "case {case:?}: the refusal must state the rule the value breaks, got {:?}",
                modified.error_message()
            );
            assert_eq!(
                described.text("CacheCluster/PreferredMaintenanceWindow"),
                Some(STARTING_WINDOW),
                "case {case:?}: a refused modify must leave the window alone, got {:?}",
                described.text("CacheCluster/PreferredMaintenanceWindow")
            );
        }

        // CREATE REPLICATION GROUP takes the same member, so the same rule.
        let group_id = format!("window-group-{index}");
        let group = emulator
            .call(
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", &group_id),
                    ("ReplicationGroupDescription", "a window-rule group"),
                    ("PreferredMaintenanceWindow", window),
                ]),
            )
            .await;
        let expected_status = if *accepted { 200 } else { 400 };
        assert_eq!(
            group.status, expected_status,
            "case {case:?} ({window:?}): CreateReplicationGroup must answer HTTP \
             {expected_status}, got {} {}",
            group.status, group.body
        );
        if *accepted {
            // Round-trip: the group's member cluster carries the window.
            let member = state::member_cluster_id(&group_id, "0001", 1, false);
            let described = describe_cluster(&emulator, &member).await;
            assert_eq!(
                described.text("CacheCluster/PreferredMaintenanceWindow"),
                Some(*window),
                "case {case:?}: the member cluster must carry the group's window, got {:?}",
                described.text("CacheCluster/PreferredMaintenanceWindow")
            );
        } else {
            assert_eq!(
                group.error_code(),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                group.body
            );
        }
    }
}

#[tokio::test]
async fn every_published_day_abbreviation_names_a_window() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "day-target", &[]).await;

    // The catalog, from `PreferredMaintenanceWindow`'s "Valid values for ddd
    // are: sun, mon, tue, wed, thu, fri, sat" — the list the state module
    // publishes, so a day the service accepts and a day this test accepts cannot
    // drift apart.
    assert_eq!(
        state::MAINTENANCE_WINDOW_DAYS.len(),
        7,
        "a week has seven days, got {:?}",
        state::MAINTENANCE_WINDOW_DAYS
    );
    for day in state::MAINTENANCE_WINDOW_DAYS {
        let window = format!("{day}:03:00-{day}:04:00");
        let modified = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "day-target"),
                    ("PreferredMaintenanceWindow", &window),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {day:?}: a published day must be accepted, got {} {}",
            modified.status, modified.body
        );
        let described = describe_cluster(&emulator, "day-target").await;
        assert_eq!(
            described.text("CacheCluster/PreferredMaintenanceWindow"),
            Some(window.as_str()),
            "case {day:?}: the window must round-trip, got {:?}",
            described.text("CacheCluster/PreferredMaintenanceWindow")
        );
    }

    // And a three-letter abbreviation that is not one of them is not a day.
    for bogus in ["sun2", "sund", "son", "0un"] {
        let window = format!("{bogus}:03:00-{bogus}:04:00");
        let refused = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "day-target"),
                    ("PreferredMaintenanceWindow", &window),
                ]),
            )
            .await;
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {bogus:?}: a value that is not a published day must be refused, got {} {}",
            refused.status, refused.body
        );
    }
}

#[tokio::test]
async fn a_cluster_working_through_a_modification_still_accepts_the_next_one() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "converge",
        &[
            ("Engine", "memcached"),
            ("NumCacheNodes", "1"),
            ("PreferredMaintenanceWindow", STARTING_WINDOW),
        ],
    )
    .await;

    // A node-count change is work `PendingModifiedValues` carries, so an
    // immediate one leaves the cluster transient.
    let first = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "converge"),
                ("NumCacheNodes", "2"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first modify must succeed, got {} {}",
        first.status, first.body
    );
    assert_eq!(
        first.text("CacheClusterStatus"),
        Some(state::STATUS_MODIFYING),
        "an immediate node change leaves the cluster transient, got {}",
        first.body
    );

    // The documented supersede: a second change arriving while the first is
    // still being worked through is accepted, not refused with a state fault.
    let second = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "converge"),
                ("PreferredMaintenanceWindow", "fri:01:00-fri:02:00"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        second.status, 200,
        "a modify of a modifying cluster must be accepted, got {} {}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "",
        "a modify of a modifying cluster reports no error, got {}",
        second.body
    );
    assert_eq!(
        second.text("CacheCluster/PreferredMaintenanceWindow"),
        Some("fri:01:00-fri:02:00"),
        "the second change applies, got {}",
        second.body
    );

    // And a malformed value is refused as the bad parameter it is, not masked by
    // the cluster's status.
    let malformed = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "converge"),
                ("PreferredMaintenanceWindow", "not-a-valid-window"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        malformed.error_code(),
        "InvalidParameterValue",
        "a malformed value must not be masked by the cluster's status, got {} {}",
        malformed.status, malformed.body
    );
}

#[tokio::test]
async fn an_unprovisioned_cluster_is_gated_but_only_after_its_input_is_read() {
    let emulator = Emulator::new();
    // Created and held in `creating`: the clock owes a just-created cluster its
    // settled status from the call that created it, so the phase a gate acts in
    // is held here rather than raced for.
    let created = create_cluster(&emulator, "unsettled", &[]).await;
    hold_pending_changes(&emulator);
    assert_eq!(
        created.status, 200,
        "the cluster must be created, got {}",
        created.body
    );
    assert_eq!(
        created.text("CacheClusterStatus"),
        Some(state::STATUS_CREATING),
        "a fresh cluster is not provisioned yet, got {}",
        created.body
    );

    // `(case, the window the request carries, the awsQuery code)`: a cluster that
    // is not provisioned yet is gated, but the gate runs after the checks that
    // read nothing but the request.
    let cases: [(&str, &str, &str); 2] = [
        (
            "a well-formed window on an unprovisioned cluster",
            "tue:05:00-tue:06:00",
            "InvalidCacheClusterState",
        ),
        (
            "a malformed window on an unprovisioned cluster",
            "not-a-valid-window",
            "InvalidParameterValue",
        ),
    ];
    for (case, window, code) in cases {
        let reply = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "unsettled"),
                    ("PreferredMaintenanceWindow", window),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code:?}, got {}",
            reply.body
        );
    }

    // Neither refusal changed the cluster: it is still running the window it was
    // created with.
    let described = describe_cluster(&emulator, "unsettled").await;
    assert_eq!(
        described.text("CacheCluster/PreferredMaintenanceWindow"),
        Some(state::DEFAULT_MAINTENANCE_WINDOW),
        "a refused modify stores nothing, got {:?}",
        described.text("CacheCluster/PreferredMaintenanceWindow")
    );
}

#[tokio::test]
async fn naming_a_cache_security_group_is_the_account_wide_closure() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "sg-target", &[]).await;

    // `(case, Action, the form pairs)`: every operation that can name a cache
    // security group, whether it is the surface's own or a cluster call that
    // references one.
    let cases: [CallCase; 5] = [
        (
            "CreateCacheCluster",
            "CreateCacheCluster",
            crate::harness::cluster_create_params(
                "sg-create",
                &[("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a")],
            ),
        ),
        (
            "ModifyCacheCluster",
            "ModifyCacheCluster",
            params(&[
                ("CacheClusterId", "sg-target"),
                ("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a"),
            ]),
        ),
        (
            "CreateReplicationGroup",
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", "sg-group"),
                ("ReplicationGroupDescription", "a closure group"),
                ("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a"),
            ]),
        ),
        (
            "the surface's own create",
            "CreateCacheSecurityGroup",
            params(&[
                ("CacheSecurityGroupName", "sg-a"),
                ("Description", "a closure group"),
            ]),
        ),
        (
            "the surface's own describe",
            "DescribeCacheSecurityGroups",
            params(&[("CacheSecurityGroupName", "sg-a")]),
        ),
    ];

    for (case, action, request) in cases {
        let reply = emulator.call(action, &request).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected the account-wide closure, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            state::CACHE_SECURITY_GROUPS_UNSUPPORTED,
            "case {case}: expected the closure's wording, got {:?}",
            reply.error_message()
        );
    }

    // The refused cluster call changed nothing, and the refused create made no
    // cluster.
    let described = describe_cluster(&emulator, "sg-target").await;
    assert_eq!(
        described.text("CacheCluster/CacheClusterStatus"),
        Some(state::STATUS_AVAILABLE),
        "a refused modify leaves the cluster settled, got {:?}",
        described.text("CacheCluster/CacheClusterStatus")
    );
    let missing = describe_cluster(&emulator, "sg-create").await;
    assert_eq!(
        missing.error_code(),
        "CacheClusterNotFound",
        "a refused create stores no cluster, got {} {}",
        missing.status, missing.body
    );
}

#[tokio::test]
async fn a_missing_cache_cluster_is_worded_the_way_each_surface_words_it() {
    let emulator = Emulator::new();
    let ghost = "ghost-cluster";
    // Every surface that RESOLVES a `CacheClusterId` names the resource the way
    // the shape spells it; the snapshot creator is the one that words the same
    // fault as prose, which is what AWS answers `CreateSnapshot` with.
    let shape_wording = format!("CacheCluster not found: {ghost}");
    let prose_wording = format!("Cache cluster not found: {ghost}");

    // `(case, Action, the form pairs)`: every surface that resolves a
    // `CacheClusterId` against the store. `DescribeSnapshots` is not one of
    // them: there the member FILTERS the snapshot list rather than naming a
    // resource the call acts on, so an id nothing was created under is a
    // successful empty page (the `filtered_reads` suite).
    let cases: [CallCase; 7] = [
        (
            "modify",
            "ModifyCacheCluster",
            params(&[("CacheClusterId", ghost)]),
        ),
        (
            "delete",
            "DeleteCacheCluster",
            params(&[("CacheClusterId", ghost)]),
        ),
        (
            "describe",
            "DescribeCacheClusters",
            params(&[("CacheClusterId", ghost)]),
        ),
        (
            "reboot",
            "RebootCacheCluster",
            params(&[
                ("CacheClusterId", ghost),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
        ),
        (
            "snapshot",
            "CreateSnapshot",
            params(&[("SnapshotName", "ghost-snap"), ("CacheClusterId", ghost)]),
        ),
        (
            "allowed node types",
            "ListAllowedNodeTypeModifications",
            params(&[("CacheClusterId", ghost)]),
        ),
        (
            "a replication group's primary",
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", "ghost-group"),
                ("ReplicationGroupDescription", "a group with no primary"),
                ("PrimaryClusterId", ghost),
            ]),
        ),
    ];

    for (case, action, request) in cases {
        let expected = if action == "CreateSnapshot" {
            prose_wording.clone()
        } else {
            shape_wording.clone()
        };
        let reply = emulator.call(action, &request).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: expected HTTP 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "CacheClusterNotFound",
            "case {case}: expected CacheClusterNotFound, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            expected,
            "case {case}: expected message {expected:?}, got {:?}",
            reply.error_message()
        );
    }
}

#[tokio::test]
async fn a_malformed_cluster_id_is_refused_with_the_services_identifier_rule() {
    let emulator = Emulator::new();

    // The axis: the way the value breaks the rule. Each row must be refused the
    // same way by the create that would mint the id and by the describe that
    // resolves it.
    let cases: [(&str, &str); 6] = [
        ("underscores", "Sim2Real_Behav_Bad_Id"),
        ("a leading digit", "9starts-with-a-digit"),
        ("a trailing hyphen", "ends-with-a-hyphen-"),
        ("two consecutive hyphens", "two--hyphens"),
        ("a character outside the set", "dots.not.allowed"),
        ("an empty id", ""),
    ];
    for (case, id) in cases {
        for action in ["CreateCacheCluster", "DescribeCacheClusters"] {
            let reply = emulator
                .call(action, &params(&[("CacheClusterId", id)]))
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({action}): expected HTTP 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case {case:?} ({action}): expected InvalidParameterValue, got {}",
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                IDENTIFIER_RULE,
                "case {case:?} ({action}): expected the service's identifier rule, got {:?}",
                reply.error_message()
            );
        }
    }

    // The published length bound, and one character past it. The bound is the
    // one the state module publishes from `CacheClusterId`'s own documentation
    // ("A name must contain from 1 to 50 alphanumeric characters or hyphens").
    let longest = format!("c{}", "a".repeat(state::CLUSTER_ID_MAX_LEN - 1));
    let too_long = format!("c{}", "a".repeat(state::CLUSTER_ID_MAX_LEN));
    let accepted = create_cluster(&emulator, &longest, &[]).await;
    assert_eq!(
        accepted.status, 200,
        "an id of exactly {} characters must be accepted, got {} {}",
        state::CLUSTER_ID_MAX_LEN,
        accepted.status,
        accepted.body
    );
    let refused = create_cluster(&emulator, &too_long, &[]).await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "an id one character past the bound must be refused, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        format!(
            "The parameter CacheClusterIdentifier is not a valid identifier because it is \
             longer than {} characters.",
            state::CLUSTER_ID_MAX_LEN
        ),
        "the length refusal must state the published bound, got {:?}",
        refused.error_message()
    );

    // A well-formed id the account does not hold is still a not-found, not a bad
    // parameter value: the rule is about the FORM.
    let unknown = describe_cluster(&emulator, "well-formed-unknown").await;
    assert_eq!(
        unknown.error_code(),
        "CacheClusterNotFound",
        "a well-formed unknown id is a not-found, got {} {}",
        unknown.status,
        unknown.body
    );
}
