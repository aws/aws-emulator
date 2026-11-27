// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a compute node group's `scaleDownIdleTimeInSeconds` OVERRIDE,
//! driven over the awsJson1_0 wire through the same router the emulator serves from, on the
//! harness [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: where a request member's own documentation defines a value that REMOVES
//! the per-resource override the member carries, a request naming that value leaves the
//! resource with no override — every later read reports the member absent — while any other
//! value it names is stored as the override and a request that omits the member leaves the
//! stored override alone. Read against both paths that write this member,
//! `CreateComputeNodeGroup` and `UpdateComputeNodeGroup`, and against the cluster-level
//! member of the same name, whose documentation defines no such value.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    DEFAULT_SCALE_DOWN_IDLE_TIME_IN_SECONDS, MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER,
    REMOVE_SCALE_DOWN_IDLE_TIME_OVERRIDE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, create_cluster, create_compute_node_group, emulator};

/// The member this suite is about, as the wire spells it.
const MEMBER: &str = "scaleDownIdleTimeInSeconds";

/// Values a compute node group's override can HOLD: the lowest the member's
/// `@range(min: -1, max: 10000000)` admits that is not the removal value, the published
/// maximum, and values between them. Source: the `@range` and the documentation of
/// `UpdateComputeNodeGroupSlurmConfigurationRequest$scaleDownIdleTimeInSeconds`
/// (`smithy_query(query="shape", shape="UpdateComputeNodeGroupSlurmConfigurationRequest")`),
/// which gives the range's minimum a meaning of its own, so the values an override holds
/// start one above it.
const OVERRIDES: &[i64] = &[0, 1, 600, 900, 9_999_999, 10_000_000];

/// One past each published bound of the same `@range`, which the modelled constraint
/// refuses.
const PAST_THE_BOUNDS: &[i64] = &[-2, 10_000_001];

/// The removal value as the wire carries it.
fn removal() -> i64 {
    i64::from(REMOVE_SCALE_DOWN_IDLE_TIME_OVERRIDE)
}

/// One case: the Slurm configuration the create carries, the Slurm configurations the
/// updates that follow carry in order, and the override every read must report once they
/// have all been served.
struct Row {
    case: String,
    created: Option<Value>,
    updates: Vec<Value>,
    expected: Option<i64>,
}

/// A Slurm configuration carrying only this suite's member.
fn idle_time(seconds: i64) -> Value {
    json!({MEMBER: seconds})
}

/// A `CreateCluster` request whose cluster CARRIES a Slurm configuration, so the
/// cluster-level setting a group without an override falls back to is stored and readable,
/// and whose scheduler runs the release the override's own documentation asks for
/// ("Requires Slurm version 25.11 or later").
fn create_cluster_with_slurm(name: &str) -> Value {
    let mut request = create_cluster(name, "SMALL");
    if let Some(target) = request.as_object_mut() {
        target.insert(
            "scheduler".to_string(),
            json!({"type": "SLURM", "version": "25.11"}),
        );
        target.insert("slurmConfiguration".to_string(), json!({}));
    }
    request
}

/// Create a compute node group in `cluster`, carrying `slurm` when there is one.
async fn create_group(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    slurm: Option<&Value>,
) -> (u16, Value) {
    let mut request = create_compute_node_group(cluster, name);
    if let (Some(target), Some(slurm)) = (request.as_object_mut(), slurm) {
        target.insert("slurmConfiguration".to_string(), slurm.clone());
    }
    call(router, "CreateComputeNodeGroup", &request).await
}

/// Update `group` in `cluster` with `slurm` as its whole requested Slurm configuration.
async fn update_group(
    router: &MultiServiceRouter,
    cluster: &str,
    group: &str,
    slurm: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdateComputeNodeGroup",
        &json!({
            "clusterIdentifier": cluster,
            "computeNodeGroupIdentifier": group,
            "slurmConfiguration": slurm,
        }),
    )
    .await
}

/// The stored compute node group, as `GetComputeNodeGroup` reports it.
async fn stored_group(router: &MultiServiceRouter, cluster: &str, group: &str) -> Value {
    let (status, read) = call(
        router,
        "GetComputeNodeGroup",
        &json!({
            "clusterIdentifier": cluster,
            "computeNodeGroupIdentifier": group,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the group back: expected the read to be served, got {status} with {read}"
    );
    read["computeNodeGroup"].clone()
}

/// Assert the `reported` group holds exactly `expected` as its override: the value when
/// there is one, and no `scaleDownIdleTimeInSeconds` member at all when there is none.
fn assert_override(case: &str, source: &str, reported: &Value, expected: Option<i64>) {
    let carried = reported["slurmConfiguration"]
        .as_object()
        .is_some_and(|slurm| slurm.contains_key(MEMBER));
    let actual = reported["slurmConfiguration"][MEMBER].clone();
    match expected {
        Some(seconds) => assert!(
            carried && actual == json!(seconds),
            "case {case:?} ({source}): expected the override {seconds}, got {actual:?} in \
             {reported}"
        ),
        None => assert!(
            !carried,
            "case {case:?} ({source}): expected no override at all, got {actual:?} in \
             {reported}"
        ),
    }
}

/// Every case the rule is read over: which call carries the removal value, what the group's
/// override was before it, which other values arrive after it, and what the member must
/// report at the end.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for seconds in OVERRIDES {
        // A value that is not the removal value is stored as the override, whichever call
        // carries it.
        rows.push(Row {
            case: format!("a create carrying {seconds}"),
            created: Some(idle_time(*seconds)),
            updates: Vec::new(),
            expected: Some(*seconds),
        });
        rows.push(Row {
            case: format!("an update carrying {seconds}"),
            created: None,
            updates: vec![idle_time(*seconds)],
            expected: Some(*seconds),
        });
        // ... and the removal value drops it again, from either starting point.
        rows.push(Row {
            case: format!("an update removing an override of {seconds} a create stored"),
            created: Some(idle_time(*seconds)),
            updates: vec![idle_time(removal())],
            expected: None,
        });
        rows.push(Row {
            case: format!("an update removing an override of {seconds} an update stored"),
            created: None,
            updates: vec![idle_time(*seconds), idle_time(removal())],
            expected: None,
        });
    }

    rows.push(Row {
        case: "a create carrying the removal value".to_string(),
        created: Some(idle_time(removal())),
        updates: Vec::new(),
        expected: None,
    });
    rows.push(Row {
        case: "an update removing an override the group never had".to_string(),
        created: None,
        updates: vec![idle_time(removal())],
        expected: None,
    });
    rows.push(Row {
        case: "two removals in a row".to_string(),
        created: Some(idle_time(900)),
        updates: vec![idle_time(removal()), idle_time(removal())],
        expected: None,
    });
    rows.push(Row {
        case: "an override set again after it was removed".to_string(),
        created: Some(idle_time(900)),
        updates: vec![idle_time(removal()), idle_time(1_800)],
        expected: Some(1_800),
    });
    rows.push(Row {
        case: "a removal followed by an update that omits the member".to_string(),
        created: Some(idle_time(900)),
        updates: vec![
            idle_time(removal()),
            json!({"slurmCustomSettings": [{"parameterName": "Weight", "parameterValue": "10"}]}),
        ],
        expected: None,
    });
    rows.push(Row {
        case: "a create carrying the removal value beside the other Slurm members"
            .to_string(),
        created: Some(json!({
            MEMBER: removal(),
            "slurmCustomSettings": [{"parameterName": "CpuBind", "parameterValue": "cores"}],
            "gresCustomSettings": [{"Name": "gpu", "Type": "a100", "File": "/dev/nvidia0"}],
        })),
        updates: Vec::new(),
        expected: None,
    });
    rows.push(Row {
        case: "an update carrying the removal value beside the other Slurm members"
            .to_string(),
        created: Some(idle_time(900)),
        updates: vec![json!({
            MEMBER: removal(),
            "slurmCustomSettings": [{"parameterName": "CpuBind", "parameterValue": "cores"}],
        })],
        expected: None,
    });
    rows
}

/// RULE: the value the member documents as removing the override leaves the group with no
/// override at all, whichever call carries it and whatever the group carried before; any
/// other value in range is stored as the override, and a later request that omits the member
/// leaves what is stored — an override or none — alone.
#[tokio::test]
async fn the_documented_removal_value_leaves_a_group_with_no_override() {
    let router = emulator();
    // Every row needs a compute node group of its own to start from a state of its own, and a
    // cluster holds at most [`MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER`] of them, so the rows are
    // spread over as many clusters as that ceiling needs.
    let rows = rows();
    let clusters: Vec<String> = (0..rows.len().div_ceil(MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER))
        .map(|index| format!("idle-override-cluster-{index}"))
        .collect();
    for cluster in &clusters {
        let (status, created) =
            call(&router, "CreateCluster", &create_cluster_with_slurm(cluster)).await;
        assert_eq!(
            status, 200,
            "setup: expected the cluster {cluster} to be created, got {status} with {created}"
        );
    }

    for (index, row) in rows.into_iter().enumerate() {
        let cluster = clusters[index / MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER].as_str();
        let name = format!("idle-group-{index}");
        let (status, answered) =
            create_group(&router, cluster, &name, row.created.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to be served, got {status} with {answered}",
            row.case
        );
        let group_id = answered["computeNodeGroup"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut last = answered["computeNodeGroup"].clone();

        for (step, slurm) in row.updates.iter().enumerate() {
            let (status, answered) = update_group(&router, cluster, &group_id, slurm).await;
            assert_eq!(
                status, 200,
                "case {:?} (update {step}): expected the update to be served, got {status} \
                 with {answered}",
                row.case
            );
            last = answered["computeNodeGroup"].clone();
        }

        // The call that applied the change and a later read cannot differ: both are read off
        // the one stored record.
        assert_override(&row.case, "the call itself", &last, row.expected);
        let read = stored_group(&router, cluster, &group_id).await;
        assert_override(&row.case, "GetComputeNodeGroup", &read, row.expected);
    }
}

/// RULE: removing the override touches nothing else — the other members of the same Slurm
/// configuration keep what the calls before stored, and the cluster-level setting the group
/// falls back to is left where it is.
#[tokio::test]
async fn removing_the_override_leaves_every_other_stored_member_alone() {
    let router = emulator();
    let cluster = "idle-override-neighbours";
    let (status, created) =
        call(&router, "CreateCluster", &create_cluster_with_slurm(cluster)).await;
    assert_eq!(
        status, 200,
        "setup: expected the cluster to be created, got {status} with {created}"
    );

    let slurm_settings = json!([
        {"parameterName": "Weight", "parameterValue": "10"},
        {"parameterName": "CpuBind", "parameterValue": "cores"},
    ]);
    let gres_settings = json!([{"Name": "gpu", "Type": "a100", "File": "/dev/nvidia0"}]);
    let (status, answered) = create_group(
        &router,
        cluster,
        "idle-neighbours",
        Some(&json!({
            MEMBER: 900,
            "slurmCustomSettings": slurm_settings,
            "gresCustomSettings": gres_settings,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the create to be served, got {status} with {answered}"
    );
    let group_id = answered["computeNodeGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, answered) =
        update_group(&router, cluster, &group_id, &idle_time(removal())).await;
    assert_eq!(
        status, 200,
        "case removing the override: expected the update to be served, got {status} with \
         {answered}"
    );

    let read = stored_group(&router, cluster, &group_id).await;
    assert_override(
        "removing the override",
        "GetComputeNodeGroup",
        &read,
        None,
    );
    assert_eq!(
        read["slurmConfiguration"]["slurmCustomSettings"], slurm_settings,
        "case removing the override: expected the custom settings left alone, got {read}"
    );
    assert_eq!(
        read["slurmConfiguration"]["gresCustomSettings"], gres_settings,
        "case removing the override: expected the gres settings left alone, got {read}"
    );

    // The cluster-level setting the group now falls back to is the cluster's own, untouched
    // by a call about one of its compute node groups.
    let (status, cluster_read) = call(
        &router,
        "GetCluster",
        &json!({"clusterIdentifier": cluster}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case removing the override: expected the cluster read to be served, got {status} \
         with {cluster_read}"
    );
    assert_eq!(
        cluster_read["cluster"]["slurmConfiguration"][MEMBER],
        json!(DEFAULT_SCALE_DOWN_IDLE_TIME_IN_SECONDS),
        "case removing the override: expected the cluster-level setting left where it is, \
         got {cluster_read}"
    );
}

/// RULE: the removal value is a value the member's own `@range` admits, so it is the
/// member's documentation — not the value — that gives it its meaning: one past either
/// published bound is refused on both paths, and the CLUSTER's member of the same name,
/// whose `@range` starts above it, refuses the removal value outright.
#[tokio::test]
async fn a_value_the_range_does_not_admit_is_refused_on_every_path() {
    let router = emulator();
    let cluster = "idle-override-bounds";
    let (status, created) =
        call(&router, "CreateCluster", &create_cluster_with_slurm(cluster)).await;
    assert_eq!(
        status, 200,
        "setup: expected the cluster to be created, got {status} with {created}"
    );
    let (status, answered) = create_group(&router, cluster, "idle-bounds", None).await;
    assert_eq!(
        status, 200,
        "setup: expected the create to be served, got {status} with {answered}"
    );
    let group_id = answered["computeNodeGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (index, seconds) in PAST_THE_BOUNDS.iter().enumerate() {
        let (status, refused) = create_group(
            &router,
            cluster,
            &format!("idle-bounds-{index}"),
            Some(&idle_time(*seconds)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case a create carrying {seconds}: expected a refusal past the bound, got \
             {status} with {refused}"
        );
        let (status, refused) =
            update_group(&router, cluster, &group_id, &idle_time(*seconds)).await;
        assert_eq!(
            status, 400,
            "case an update carrying {seconds}: expected a refusal past the bound, got \
             {status} with {refused}"
        );
    }

    // `ClusterSlurmConfigurationRequest$scaleDownIdleTimeInSeconds` is `@range(min: 1)` and
    // documents no removal value: the cluster-level setting is the one a group falls back
    // TO, so there is nothing for it to drop.
    let (status, refused) = call(
        &router,
        "UpdateCluster",
        &json!({
            "clusterIdentifier": cluster,
            "slurmConfiguration": idle_time(removal()),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the cluster's own member: expected the removal value refused past its bound, \
         got {status} with {refused}"
    );

    // Nothing a refusal carried was stored: the group still reports no override.
    let read = stored_group(&router, cluster, &group_id).await;
    assert_override("every refusal above", "GetComputeNodeGroup", &read, None);
}
