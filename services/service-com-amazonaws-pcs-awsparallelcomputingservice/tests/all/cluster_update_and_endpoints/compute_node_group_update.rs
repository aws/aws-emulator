// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateComputeNodeGroup`, driven over the awsJson1_0 wire through
//! the same router the emulator serves from, on the harness
//! [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: an update applies every configuration member the request carries to the
//! stored compute node group of the cluster it names, leaves every member the request omits
//! at its stored value, reports the group `UPDATING` and owes it `ACTIVE` again — and
//! refuses, without storing anything, a request whose cluster or group does not resolve,
//! whose group is not settled, or whose values would leave the stored group outside a
//! constraint the model declares for it.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    service_state, MAX_STORED_LIST_ENTRIES, REMOVE_SCALE_DOWN_IDLE_TIME_OVERRIDE, STATUS_ACTIVE,
    STATUS_CREATING, STATUS_DELETED, STATUS_DELETING, STATUS_UPDATING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{
    call, create_cluster, create_compute_node_group, emulator, emulator_with_engine,
};

/// The `PurchaseOption` members the model declares
/// (`smithy_query(query="shape",shape="PurchaseOption")`), every one of which an update
/// stores.
const PURCHASE_OPTIONS: &[&str] = &[
    "ONDEMAND",
    "SPOT",
    "CAPACITY_BLOCK",
    "INTERRUPTIBLE_CAPACITY_RESERVATION",
];

/// The `SpotAllocationStrategy` members the model declares
/// (`smithy_query(query="shape",shape="SpotAllocationStrategy")`).
const SPOT_ALLOCATION_STRATEGIES: &[&str] =
    &["lowest-price", "capacity-optimized", "price-capacity-optimized"];

/// The published bounds of `scaleDownIdleTimeInSeconds`
/// (`@range(min: -1, max: 10000000)` on
/// `UpdateComputeNodeGroupSlurmConfigurationRequest$scaleDownIdleTimeInSeconds`), and a
/// value between them. The minimum is the value that member documents as REMOVING the
/// per-group override, so it is driven after a row that stored one, and its row expects the
/// member GONE rather than set; the rule itself is read over a set of inputs by
/// `crate::cluster_update_and_endpoints::compute_node_group_idle_time_override`.
const SCALE_DOWN_SECONDS: &[i64] = &[900, REMOVE_SCALE_DOWN_IDLE_TIME_OVERRIDE as i64, 10_000_000];

/// A subnet id of the form `SubnetId`'s `@pattern` admits (`^subnet-\w{8,17}$`).
fn subnet(index: usize) -> String {
    format!("subnet-0123456789abcde{index}")
}

/// An instance-profile ARN of the form `InstanceProfileArn`'s `@pattern` admits, holding a
/// role name on the half of the naming convention the member documents.
fn instance_profile(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:instance-profile/AWSPCS-{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `resource` without `members`, so what one call answered can be compared WHOLE against
/// what the next read answers, minus only the members the call was expected to move.
fn without_members(resource: &Value, members: &[&str]) -> Value {
    let mut stripped = resource.clone();
    if let Some(object) = stripped.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    stripped
}

/// `patch` as a whole `UpdateComputeNodeGroup` request about `group` in `cluster`.
fn update_request(cluster: &str, group: &str, patch: &Value) -> Value {
    let mut request = json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupIdentifier": group,
    });
    if let (Some(target), Some(members)) = (request.as_object_mut(), patch.as_object()) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    request
}

/// Create a cluster and a compute node group inside it, and answer the generated ids. The
/// request that follows a create observes the resource settled, so there is nothing to wait
/// for.
async fn active_group(
    router: &MultiServiceRouter,
    cluster_name: &str,
    group_name: &str,
) -> (String, String) {
    let (status, created) = call(
        router,
        "CreateCluster",
        &create_cluster(cluster_name, "SMALL"),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateCluster got {status} with {created}");
    let cluster_id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, created) = call(
        router,
        "CreateComputeNodeGroup",
        &create_compute_node_group(&cluster_id, group_name),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateComputeNodeGroup got {status} with {created}"
    );
    let group_id = created["computeNodeGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (cluster_id, group_id)
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
        "case read back {group}: got {status} with {read}"
    );
    read["computeNodeGroup"].clone()
}

/// Every row of [`updatable_members`]: a case name, the members one request carries, and
/// what the stored group must report for each of those members afterwards.
type MemberRow = (String, Value, Value);

/// One row per member (and per value class within a member) an update can carry: the
/// request's patch, and the members the stored group must report afterwards. The expected
/// value differs from the patch only where the service MERGES rather than replaces — the
/// Slurm configuration, where a member the request omits keeps the value an earlier call
/// stored.
fn updatable_members() -> Vec<MemberRow> {
    let mut rows: Vec<MemberRow> = vec![
        (
            "amiId".to_string(),
            json!({"amiId": "ami-00f1e2d3c4b5a6978"}),
            json!({"amiId": "ami-00f1e2d3c4b5a6978"}),
        ),
        (
            "subnetIds-three".to_string(),
            json!({"subnetIds": [subnet(1), subnet(2), subnet(3)]}),
            json!({"subnetIds": [subnet(1), subnet(2), subnet(3)]}),
        ),
        (
            "subnetIds-reordered".to_string(),
            json!({"subnetIds": [subnet(3), subnet(1), subnet(2)]}),
            json!({"subnetIds": [subnet(3), subnet(1), subnet(2)]}),
        ),
        (
            "subnetIds-one".to_string(),
            json!({"subnetIds": [subnet(2)]}),
            json!({"subnetIds": [subnet(2)]}),
        ),
        (
            "customLaunchTemplate".to_string(),
            json!({"customLaunchTemplate": {"id": "lt-0abcdef1234567890", "version": "7"}}),
            json!({"customLaunchTemplate": {"id": "lt-0abcdef1234567890", "version": "7"}}),
        ),
        (
            "iamInstanceProfileArn".to_string(),
            json!({"iamInstanceProfileArn": instance_profile("updated")}),
            json!({"iamInstanceProfileArn": instance_profile("updated")}),
        ),
        (
            "scalingConfiguration-empty-fleet".to_string(),
            json!({"scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 0}}),
            json!({"scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 0}}),
        ),
        (
            "scalingConfiguration-wide".to_string(),
            json!({"scalingConfiguration": {"minInstanceCount": 3, "maxInstanceCount": 12}}),
            json!({"scalingConfiguration": {"minInstanceCount": 3, "maxInstanceCount": 12}}),
        ),
        (
            "scalingConfiguration-fixed".to_string(),
            json!({"scalingConfiguration": {"minInstanceCount": 6, "maxInstanceCount": 6}}),
            json!({"scalingConfiguration": {"minInstanceCount": 6, "maxInstanceCount": 6}}),
        ),
    ];

    // Every member of each enum the model declares for these two members, so the rule is
    // checked on values the issue never cited as well.
    for option in PURCHASE_OPTIONS {
        rows.push((
            format!("purchaseOption-{option}"),
            json!({"purchaseOption": option}),
            json!({"purchaseOption": option}),
        ));
    }
    for strategy in SPOT_ALLOCATION_STRATEGIES {
        rows.push((
            format!("spotOptions-{strategy}"),
            json!({"spotOptions": {"allocationStrategy": strategy}}),
            json!({"spotOptions": {"allocationStrategy": strategy}}),
        ));
    }

    // The Slurm configuration is MERGED: each row below carries one of its members and the
    // expectation keeps what the rows before it stored.
    let mut slurm = json!({});
    for seconds in SCALE_DOWN_SECONDS {
        if *seconds == i64::from(REMOVE_SCALE_DOWN_IDLE_TIME_OVERRIDE) {
            // The value the member documents as removing the override: the stored group
            // reports no `scaleDownIdleTimeInSeconds` at all once this row is applied.
            if let Some(members) = slurm.as_object_mut() {
                members.remove("scaleDownIdleTimeInSeconds");
            }
        } else {
            slurm["scaleDownIdleTimeInSeconds"] = json!(seconds);
        }
        rows.push((
            format!("slurmConfiguration-scaleDownIdleTimeInSeconds-{seconds}"),
            json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": seconds}}),
            json!({"slurmConfiguration": slurm.clone()}),
        ));
    }
    let custom_settings = json!([
        {"parameterName": "Weight", "parameterValue": "10"},
        {"parameterName": "CpuBind", "parameterValue": "cores"},
    ]);
    slurm["slurmCustomSettings"] = custom_settings.clone();
    rows.push((
        "slurmConfiguration-slurmCustomSettings".to_string(),
        json!({"slurmConfiguration": {"slurmCustomSettings": custom_settings}}),
        json!({"slurmConfiguration": slurm.clone()}),
    ));
    let gres_settings = json!([{"Name": "gpu", "Type": "a100", "File": "/dev/nvidia0"}]);
    slurm["gresCustomSettings"] = gres_settings.clone();
    rows.push((
        "slurmConfiguration-gresCustomSettings".to_string(),
        json!({"slurmConfiguration": {"gresCustomSettings": gres_settings}}),
        json!({"slurmConfiguration": slurm.clone()}),
    ));

    // The lifecycle actions are a WHOLE set: the second row below states only `nodeReady`,
    // so the stage the first row stored is gone rather than merged.
    let bootstrapped = json!({
        "stages": {
            "nodeBootstrapped": [{
                "name": "mount-shared-storage",
                "scriptSource": {"scriptLocation": "s3://pcs-emulator-scripts/mount.sh"},
                "onError": "CONTINUE",
                "executionPolicy": "EVERY_BOOT",
            }],
        },
        "scriptCachingPolicy": "REFRESH_ON_REBOOT",
    });
    rows.push((
        "nodeLifecycleActions-nodeBootstrapped".to_string(),
        json!({"nodeLifecycleActions": bootstrapped.clone()}),
        json!({"nodeLifecycleActions": bootstrapped}),
    ));
    let ready = json!({
        "stages": {
            "nodeReady": [{
                "name": "register-with-controller",
                "scriptSource": {"scriptLocation": "https://scripts.example.com/ready.sh"},
                "arguments": ["--verbose", "--wait"],
                "onError": "STOP_SEQUENCE",
                "executionPolicy": "EVERY_BOOT",
            }],
        },
        "scriptCachingPolicy": "REFRESH_ON_REBOOT",
    });
    rows.push((
        "nodeLifecycleActions-nodeReady".to_string(),
        json!({"nodeLifecycleActions": ready.clone()}),
        json!({"nodeLifecycleActions": ready}),
    ));
    rows
}

/// RULE: every member an update carries is applied to the stored group and every member it
/// omits keeps its stored value; the update itself reports the transient status and the
/// group is `ACTIVE` again on the next request.
#[tokio::test]
async fn every_member_an_update_carries_is_applied_and_the_rest_are_left_alone() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "update-nodes-cluster", "update-nodes").await;
    let mut settled = stored_group(&router, &cluster_id, &group_id).await;

    for (case, patch, expected) in updatable_members() {
        let (status, answered) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_request(&cluster_id, &group_id, &patch),
        )
        .await;
        assert_eq!(status, 200, "case {case}: got {status} with {answered}");
        let updated = answered["computeNodeGroup"].clone();
        assert_eq!(
            updated["status"],
            json!(STATUS_UPDATING),
            "case {case}: expected the update to report the transient status, got {updated}"
        );

        let members = expected
            .as_object()
            .map(|members| members.keys().map(String::as_str).collect::<Vec<&str>>())
            .unwrap_or_default();
        for member in &members {
            assert_eq!(
                updated[*member], expected[*member],
                "case {case}: expected {member} to be {}, got {updated}",
                expected[*member]
            );
        }

        // Nothing else moved: the whole resource, minus the members this row changed and
        // the two a transition moves, is what the previous read answered.
        let untouched: Vec<&str> = members
            .iter()
            .copied()
            .chain(["status", "modifiedAt"])
            .collect();
        assert_eq!(
            without_members(&updated, &untouched),
            without_members(&settled, &untouched),
            "case {case}: expected every member the request omitted to keep its stored \
             value, got {updated} against {settled}"
        );

        // The change persisted, and the group settled back on this very request.
        let read = stored_group(&router, &cluster_id, &group_id).await;
        assert_eq!(
            read["status"],
            json!(STATUS_ACTIVE),
            "case {case}: expected the group ACTIVE again on the request that follows the \
             update, got {read}"
        );
        assert_eq!(
            without_members(&read, &["status", "modifiedAt"]),
            without_members(&updated, &["status", "modifiedAt"]),
            "case {case}: expected the read to answer what the update stored, got {read} \
             against {updated}"
        );
        settled = read;
    }
}

/// RULE: an update is addressed by either form `ComputeNodeGroupIdentifier` allows — the
/// generated id or the caller-chosen name — and both reach one record.
#[tokio::test]
async fn an_update_resolves_the_group_by_id_or_by_name() {
    for by_name in [false, true] {
        let case = if by_name { "by name" } else { "by id" };
        let router = emulator();
        let (cluster_id, group_id) = active_group(&router, "ref-cluster", "ref-nodes").await;
        let group_ref = if by_name {
            "ref-nodes".to_string()
        } else {
            group_id.clone()
        };
        let cluster_ref = if by_name {
            "ref-cluster".to_string()
        } else {
            cluster_id.clone()
        };

        let (status, answered) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_request(
                &cluster_ref,
                &group_ref,
                &json!({"scalingConfiguration": {"minInstanceCount": 1, "maxInstanceCount": 8}}),
            ),
        )
        .await;
        assert_eq!(status, 200, "case {case}: got {status} with {answered}");
        assert_eq!(
            answered["computeNodeGroup"]["id"],
            json!(group_id),
            "case {case}: expected the update to answer the one stored group, got {answered}"
        );

        let read = stored_group(&router, &cluster_id, &group_id).await;
        assert_eq!(
            read["scalingConfiguration"],
            json!({"minInstanceCount": 1, "maxInstanceCount": 8}),
            "case {case}: expected the update to persist, got {read}"
        );
    }
}

/// RULE: an update that carries no configuration member asks for no change, so the group
/// keeps its status and every stored value.
#[tokio::test]
async fn an_update_that_carries_no_member_changes_nothing() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "noop-cluster", "noop-nodes").await;
    let settled = stored_group(&router, &cluster_id, &group_id).await;

    let (status, answered) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_request(&cluster_id, &group_id, &json!({})),
    )
    .await;
    assert_eq!(status, 200, "case empty update: got {status} with {answered}");
    assert_eq!(
        answered["computeNodeGroup"], settled,
        "case empty update: expected the stored group answered unchanged, got {answered}"
    );

    let read = stored_group(&router, &cluster_id, &group_id).await;
    assert_eq!(
        read, settled,
        "case empty update: expected the stored group left alone, got {read}"
    );
}

/// RULE: an update of a group that does not resolve — or inside a cluster that does not —
/// is refused, and stores nothing. WHICH refusal an unresolvable `clusterIdentifier` earns
/// is decided by its form: a generated id names a cluster that is not there, a cluster NAME
/// is a permissions failure.
#[tokio::test]
async fn an_update_is_refused_when_its_cluster_or_group_does_not_resolve() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "present-cluster", "present-nodes").await;
    let settled = stored_group(&router, &cluster_id, &group_id).await;
    let patch = json!({"amiId": "ami-0fedcba9876543210"});

    // (case, clusterIdentifier, computeNodeGroupIdentifier, the error the refusal names)
    let rows: [(&str, String, String, &str); 5] = [
        (
            "a cluster id nothing holds",
            "pcs_0123456789abcdef".to_string(),
            group_id.clone(),
            "ResourceNotFoundException",
        ),
        (
            "a cluster name nothing holds",
            "no-such-cluster".to_string(),
            group_id.clone(),
            "AccessDeniedException",
        ),
        (
            "a group name nothing holds",
            cluster_id.clone(),
            "no-such-group".to_string(),
            "ResourceNotFoundException",
        ),
        (
            "a group id nothing holds",
            cluster_id.clone(),
            "pcs_fedcba9876543210".to_string(),
            "ResourceNotFoundException",
        ),
        (
            "a group of another cluster's name",
            cluster_id.clone(),
            "present-cluster".to_string(),
            "ResourceNotFoundException",
        ),
    ];

    for (case, cluster_ref, group_ref, expected) in rows {
        let (status, refused) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_request(&cluster_ref, &group_ref, &patch),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case}: expected the client-error status, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!(expected),
            "case {case}: expected {expected}, got {refused}"
        );
        if expected == "ResourceNotFoundException" {
            let kind = if cluster_ref == cluster_id {
                "compute node group"
            } else {
                "cluster"
            };
            assert_eq!(
                refused["message"],
                json!(format!("AWS PCS couldn't find the {kind} you specified.")),
                "case {case}: expected the service's own sentence, got {refused}"
            );
            let named = if kind == "cluster" {
                &cluster_ref
            } else {
                &group_ref
            };
            assert_eq!(
                refused["resourceId"],
                json!(named),
                "case {case}: expected the identifier as supplied in resourceId, got {refused}"
            );
        }

        let read = stored_group(&router, &cluster_id, &group_id).await;
        assert_eq!(
            read, settled,
            "case {case}: expected the refusal to store nothing, got {read}"
        );
    }
}

/// Hold the compute node group `name` of the cluster `cluster` in `status` with nothing
/// owed — a group the real service has not finished provisioning, is updating, or is taking
/// out. No sequence of requests leaves one there, so this is written through the service's
/// own store.
fn hold_group(engine: &engine::InMemoryEngine, cluster: &str, name: &str, status: &str) {
    let store = service_state(engine).expect("this service's own store");
    let mut state = store.lock();
    let held = state
        .cluster_mut(cluster)
        .expect("the cluster the create persisted")
        .compute_node_groups
        .values_mut()
        .find(|group| group.name == name)
        .expect("the group the create persisted");
    held.status = status.to_string();
    held.pending = None;
}

/// RULE: only a settled group takes an update — one still provisioning, already applying an
/// update, or on its way out is refused with the modelled `ConflictException` naming the
/// group.
#[tokio::test]
async fn an_update_is_refused_while_the_group_is_not_settled() {
    // Rows: the unsettled `ComputeNodeGroupStatus` members a group can be held in
    // (`smithy_query(query="shape",shape="ComputeNodeGroupStatus")`).
    for held in [
        STATUS_CREATING,
        STATUS_UPDATING,
        STATUS_DELETING,
        STATUS_DELETED,
    ] {
        let (router, engine) = emulator_with_engine();
        let (cluster_id, group_id) = active_group(&router, "gated-cluster", "gated-nodes").await;
        hold_group(&engine, &cluster_id, "gated-nodes", held);

        let (status, refused) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_request(
                &cluster_id,
                &group_id,
                &json!({"amiId": "ami-0123456789abcdef0"}),
            ),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {held}: expected the client-error status, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ConflictException"),
            "case {held}: expected a conflict, got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!("A compute node group isn't in ACTIVE status."),
            "case {held}: expected the service's own sentence, got {refused}"
        );
        assert_eq!(
            (
                refused["resourceId"].as_str(),
                refused["resourceType"].as_str()
            ),
            (Some(group_id.as_str()), Some("computenodegroup")),
            "case {held}: expected the refusal to name the group, got {refused}"
        );

        let read = stored_group(&router, &cluster_id, &group_id).await;
        assert_eq!(
            read["amiId"],
            Value::Null,
            "case {held}: expected the refusal to store nothing, got {read}"
        );
    }
}

/// RULE: a value that would leave the stored group outside a constraint the model declares
/// for it, or outside the ceiling this emulator holds a stored list to, is refused with the
/// modelled `ValidationException` and stores nothing.
#[tokio::test]
async fn an_update_is_refused_when_a_value_would_not_hold_on_the_stored_group() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "refused-cluster", "refused-nodes").await;
    let settled = stored_group(&router, &cluster_id, &group_id).await;

    let over_the_ceiling: Vec<String> = (0..=MAX_STORED_LIST_ENTRIES).map(subnet).collect();
    let over_the_ceiling_settings: Vec<Value> = (0..=MAX_STORED_LIST_ENTRIES)
        .map(|index| json!({"parameterName": format!("Setting{index}"), "parameterValue": "1"}))
        .collect();

    // (case, patch, the member the refusal names)
    let rows: [(&str, Value, &str); 6] = [
        // `ComputeNodeGroup$subnetIds` is `@length(min: 1)`, so a group cannot be left
        // with none.
        ("no subnet at all", json!({"subnetIds": []}), "subnetIds"),
        (
            "one subnet past the stored-list ceiling",
            json!({"subnetIds": over_the_ceiling}),
            "subnetIds",
        ),
        (
            "one Slurm setting past the stored-list ceiling",
            json!({"slurmConfiguration": {"slurmCustomSettings": over_the_ceiling_settings}}),
            "slurmConfiguration.slurmCustomSettings",
        ),
        (
            "a minimum one past the maximum",
            json!({"scalingConfiguration": {"minInstanceCount": 1, "maxInstanceCount": 0}}),
            "scalingConfiguration",
        ),
        (
            "a minimum well past the maximum",
            json!({"scalingConfiguration": {"minInstanceCount": 40, "maxInstanceCount": 4}}),
            "scalingConfiguration",
        ),
        (
            "a fleet that cannot shrink below its own maximum",
            json!({"scalingConfiguration": {"minInstanceCount": 7, "maxInstanceCount": 6}}),
            "scalingConfiguration",
        ),
    ];

    for (case, patch, member) in rows {
        let (status, refused) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_request(&cluster_id, &group_id, &patch),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case}: expected the client-error status, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {case}: expected a validation failure, got {refused}"
        );
        assert_eq!(
            refused["fieldList"][0]["name"],
            json!(member),
            "case {case}: expected the refusal to name {member}, got {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .unwrap_or_default()
                .contains(member),
            "case {case}: expected the sentence to name {member}, got {refused}"
        );

        let read = stored_group(&router, &cluster_id, &group_id).await;
        assert_eq!(
            read, settled,
            "case {case}: expected the refusal to store nothing, got {read}"
        );
    }
}

/// RULE, on both paths that WRITE a group's subnet list: a stored group's `subnetIds` is
/// `SubnetIdList`, `@length(min: 1)`, while the create's and the update's request members
/// both target the unbounded `StringList` — so each refuses an empty list rather than
/// storing a group outside its own modelled constraint.
#[tokio::test]
async fn neither_write_path_stores_a_group_without_a_subnet() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "subnets-cluster", "subnets-nodes").await;
    let settled = stored_group(&router, &cluster_id, &group_id).await;

    let mut creating = create_compute_node_group(&cluster_id, "no-subnet-nodes");
    creating["subnetIds"] = json!([]);
    // (case, operation, request)
    let rows: [(&str, &str, Value); 2] = [
        ("CreateComputeNodeGroup", "CreateComputeNodeGroup", creating),
        (
            "UpdateComputeNodeGroup",
            "UpdateComputeNodeGroup",
            update_request(&cluster_id, &group_id, &json!({"subnetIds": []})),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, refused) = call(&router, operation, &request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case}: expected the client-error status, got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case {case}: expected a validation failure, got {refused}"
        );
        assert_eq!(
            refused["fieldList"][0]["name"],
            json!("subnetIds"),
            "case {case}: expected the refusal to name subnetIds, got {refused}"
        );
    }

    // Neither refusal left anything behind: no group was created, and the existing one is
    // exactly what the last read answered.
    let (status, listed) = call(
        &router,
        "ListComputeNodeGroups",
        &json!({"clusterIdentifier": &cluster_id}),
    )
    .await;
    assert_eq!(status, 200, "case the listing: got {status} with {listed}");
    let names: Vec<&str> = listed["computeNodeGroups"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["subnets-nodes"],
        "case the listing: expected only the group the create stored, got {listed}"
    );
    let read = stored_group(&router, &cluster_id, &group_id).await;
    assert_eq!(
        read, settled,
        "case the stored group: expected it left alone, got {read}"
    );
}

/// RULE: a repeated `@idempotencyToken` returns the first call's result and has no further
/// effect; the same token on another group is a conflict rather than a second update.
#[tokio::test]
async fn a_repeated_client_token_replays_the_first_update() {
    let router = emulator();
    let (cluster_id, group_id) = active_group(&router, "token-cluster", "token-nodes").await;
    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_compute_node_group(&cluster_id, "token-other-nodes"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateComputeNodeGroup got {status} with {created}"
    );
    let other_group_id = created["computeNodeGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // `SBClientToken` is `@length(min: 8, max: 100)`.
    let token = "update-token-0001";
    let (status, first) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_request(
            &cluster_id,
            &group_id,
            &json!({"amiId": "ami-00000000000000001", "clientToken": token}),
        ),
    )
    .await;
    assert_eq!(status, 200, "case first update: got {status} with {first}");
    assert_eq!(
        first["computeNodeGroup"]["amiId"],
        json!("ami-00000000000000001"),
        "case first update: expected the member applied, got {first}"
    );

    // The retry carries a DIFFERENT value under the same token: it is replayed, so that
    // value is not stored.
    let (status, replayed) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_request(
            &cluster_id,
            &group_id,
            &json!({"amiId": "ami-00000000000000002", "clientToken": token}),
        ),
    )
    .await;
    assert_eq!(status, 200, "case retry: got {status} with {replayed}");
    assert_eq!(
        replayed["computeNodeGroup"]["amiId"],
        json!("ami-00000000000000001"),
        "case retry: expected the first call's result, got {replayed}"
    );
    let read = stored_group(&router, &cluster_id, &group_id).await;
    assert_eq!(
        read["amiId"],
        json!("ami-00000000000000001"),
        "case retry: expected the retry to have no further effect, got {read}"
    );

    // The same token naming another group conflicts: the token names the group the first
    // call changed.
    let (status, refused) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_request(
            &cluster_id,
            &other_group_id,
            &json!({"amiId": "ami-00000000000000003", "clientToken": token}),
        ),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case token on another group: expected the client-error status, got {status} with \
         {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ConflictException"),
        "case token on another group: expected a conflict, got {refused}"
    );
    assert_eq!(
        refused["resourceId"],
        json!(group_id),
        "case token on another group: expected the refusal to name the group the token was \
         used for, got {refused}"
    );
    let read = stored_group(&router, &cluster_id, &other_group_id).await;
    assert_eq!(
        read["amiId"],
        Value::Null,
        "case token on another group: expected nothing stored on it, got {read}"
    );

    // A fresh token applies the change.
    let (status, applied) = call(
        &router,
        "UpdateComputeNodeGroup",
        &update_request(
            &cluster_id,
            &group_id,
            &json!({"amiId": "ami-00000000000000004", "clientToken": "update-token-0002"}),
        ),
    )
    .await;
    assert_eq!(status, 200, "case a fresh token: got {status} with {applied}");
    assert_eq!(
        applied["computeNodeGroup"]["amiId"],
        json!("ami-00000000000000004"),
        "case a fresh token: expected the member applied, got {applied}"
    );
}
