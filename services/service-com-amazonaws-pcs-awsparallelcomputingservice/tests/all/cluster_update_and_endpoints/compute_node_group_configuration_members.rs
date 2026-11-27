// SPDX-License-Identifier: CC0-1.0

//! The configuration members a compute node group carries, and the job submit plugins a
//! cluster carries, driven over the awsJson1_0 wire through the same router the emulator
//! serves from, on the harness [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: a configuration member a create or an update carries is PERSISTED on the
//! resource and reported by every later read of it; a member the request omits keeps its
//! stored value; a member that holds a configuration made DIRECTLY on the resource is
//! reported only while the resource carries one, so the EMPTY value its update target admits
//! removes it; and a member the model reports only in one mode is reported only in that mode
//! — "This value is present when `launchTemplateMode` is `CUSTOM` and isn't present when
//! `launchTemplateMode` is `MANAGED`" (`ComputeNodeGroup$customLaunchTemplate`).
//!
//! The rows span what the rule generalises over: which member the request carries, how many
//! entries a list member holds (none, one, several, and the published maximum plus one past
//! it), whether a nested settings object carries one branch or both, which mode a group runs
//! its launch template in, and whether the value arrives on the create or on a later update.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;

use crate::cluster_lifecycle::{call, create_cluster, create_compute_node_group, emulator};

/// The launch template `crate::cluster_lifecycle::create_compute_node_group` names, which a
/// group running in the caller's own mode reports back.
const BASE_LAUNCH_TEMPLATE: &str = r#"{"id": "lt-0123456789abcdef0", "version": "1"}"#;

/// A security group id of the form `SecurityGroupId`'s `@pattern` admits (`^sg-\w{8,17}$`).
fn security_group(index: usize) -> String {
    format!("sg-0123456789abcde{index}")
}

/// One job submit plugin, carrying only the three members `JobSubmitPlugin` declares
/// `@required` (`smithy_query(query="shape",shape="JobSubmitPlugin")`): `STARLARK` is the one
/// member `JobSubmitPluginType` declares, and the location satisfies
/// `JobSubmitScriptSource$scriptLocation`'s `@pattern` (`^s3://...`).
fn job_submit_plugin(name: &str) -> Value {
    json!({
        "name": name,
        "type": "STARLARK",
        "scriptSource": {"scriptLocation": format!("s3://pcs-plugins/{name}.star")},
    })
}

/// [`job_submit_plugin`] carrying its two optional script-source members as well: an Amazon
/// S3 version id and the 64-character hexadecimal `@pattern` `checksum` takes.
fn pinned_job_submit_plugin(name: &str) -> Value {
    json!({
        "name": name,
        "type": "STARLARK",
        "scriptSource": {
            "scriptLocation": format!("s3://pcs-plugins/{name}.star"),
            "s3VersionId": "3HL4kqtJlcpXroDTDmJ-Version",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
    })
}

/// The launch template, as JSON, that a group created by
/// `crate::cluster_lifecycle::create_compute_node_group` reports in the caller's own mode.
fn base_launch_template() -> Value {
    serde_json::from_str(BASE_LAUNCH_TEMPLATE).expect("the template literal must be JSON")
}

/// `crate::cluster_lifecycle::create_compute_node_group` with `patch` applied to it: a member
/// `patch` names is added or replaced, and a member it names `null` is REMOVED, so a row can
/// drive a create that carries no launch template at all.
fn create_group_with(cluster: &str, name: &str, patch: &Value) -> Value {
    let mut request = create_compute_node_group(cluster, name);
    let (Some(target), Some(members)) = (request.as_object_mut(), patch.as_object()) else {
        return request;
    };
    for (member, value) in members {
        if value.is_null() {
            target.remove(member);
        } else {
            target.insert(member.clone(), value.clone());
        }
    }
    request
}

/// `patch` as a whole `UpdateComputeNodeGroup` request about `group` in `cluster`.
fn update_group_with(cluster: &str, group: &str, patch: &Value) -> Value {
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

/// A cluster that has finished provisioning, so it can take a compute node group: the request
/// after a create observes the resource settled, so there is nothing to wait for.
async fn active_cluster(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(router, "CreateCluster", &create_cluster(name, "SMALL")).await;
    assert_eq!(
        status, 200,
        "setup {name}: expected CreateCluster to be served, got {status} with {created}"
    );
    let id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, read) = call(router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
    assert_eq!(
        status, 200,
        "setup {name}: expected GetCluster to be served, got {status} with {read}"
    );
    id
}

/// The stored compute node group, as `GetComputeNodeGroup` reports it.
async fn stored_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    cluster: &str,
    group: &str,
) -> Value {
    let (status, read) = call(
        router,
        "GetComputeNodeGroup",
        &json!({"clusterIdentifier": cluster, "computeNodeGroupIdentifier": group}),
    )
    .await;
    assert_eq!(
        status, 200,
        "read back {group}: expected GetComputeNodeGroup to be served, got {status} with {read}"
    );
    read["computeNodeGroup"].clone()
}

/// Assert that `reported` carries exactly what `expected` says for every member `expected`
/// names: a `null` expectation means the member is ABSENT from the answer.
fn assert_members(case: &str, where_from: &str, reported: &Value, expected: &Value) {
    for (member, want) in expected.as_object().expect("the expectation is an object") {
        let got = &reported[member.as_str()];
        assert_eq!(
            got, want,
            "case {case} ({where_from}): expected {member} to be {want}, got {got:?} in \
             {reported}"
        );
    }
}

#[tokio::test]
async fn every_configuration_member_a_create_carries_is_reported_by_the_group_reader() {
    let router = emulator();
    let cluster = active_cluster(&router, "group-members").await;

    // (case, the members the create carries beyond the required ones, what every answer
    // reports for the members under test)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "nothing configured directly on the group",
            json!({}),
            json!({
                "securityGroupIds": null,
                "sshKeyName": null,
                "launchTemplateMode": "CUSTOM",
                "customLaunchTemplate": base_launch_template(),
                "instanceHealthChecks": null,
            }),
        ),
        (
            "one security group configured directly",
            json!({"securityGroupIds": [security_group(1)]}),
            json!({
                "securityGroupIds": [security_group(1)],
                "sshKeyName": null,
                "launchTemplateMode": "CUSTOM",
            }),
        ),
        (
            "three security groups configured directly",
            json!({"securityGroupIds": [
                security_group(1), security_group(2), security_group(3),
            ]}),
            json!({"securityGroupIds": [
                security_group(1), security_group(2), security_group(3),
            ]}),
        ),
        (
            "an empty security group list configures none",
            json!({"securityGroupIds": []}),
            json!({"securityGroupIds": null}),
        ),
        (
            "a key pair configured directly",
            json!({"sshKeyName": "pcs-key-alpha"}),
            json!({"sshKeyName": "pcs-key-alpha", "securityGroupIds": null}),
        ),
        (
            "health monitoring alone, with no repair action",
            json!({"instanceHealthChecks": {"healthMonitoring": {"mode": "STANDARD"}}}),
            json!({"instanceHealthChecks": {"healthMonitoring": {"mode": "STANDARD"}}}),
        ),
        (
            "health monitoring with the repair action it may name",
            json!({"instanceHealthChecks": {
                "healthMonitoring": {"mode": "STANDARD", "repairAction": "REPLACE"},
            }}),
            json!({"instanceHealthChecks": {
                "healthMonitoring": {"mode": "STANDARD", "repairAction": "REPLACE"},
            }}),
        ),
        (
            "deep health checks alone",
            json!({"instanceHealthChecks": {
                "deepHealthChecks": {"onStart": {"profile": "QUICK"}},
            }}),
            json!({"instanceHealthChecks": {
                "deepHealthChecks": {"onStart": {"profile": "QUICK"}},
            }}),
        ),
        (
            "both health check settings at once",
            json!({"instanceHealthChecks": {
                "healthMonitoring": {"mode": "NONE"},
                "deepHealthChecks": {"onStart": {"profile": "LONG", "repairAction": "ISOLATE"}},
            }}),
            json!({"instanceHealthChecks": {
                "healthMonitoring": {"mode": "NONE"},
                "deepHealthChecks": {"onStart": {"profile": "LONG", "repairAction": "ISOLATE"}},
            }}),
        ),
        (
            "the caller's own launch template mode, named explicitly",
            json!({"launchTemplateMode": "CUSTOM"}),
            json!({
                "launchTemplateMode": "CUSTOM",
                "customLaunchTemplate": base_launch_template(),
            }),
        ),
        (
            "a launch template PCS manages",
            json!({"launchTemplateMode": "MANAGED", "customLaunchTemplate": null}),
            json!({"launchTemplateMode": "MANAGED", "customLaunchTemplate": null}),
        ),
        (
            "every member under test at once",
            json!({
                "securityGroupIds": [security_group(4)],
                "sshKeyName": "pcs-key-omega",
                "launchTemplateMode": "CUSTOM",
                "instanceHealthChecks": {
                    "healthMonitoring": {"mode": "STANDARD", "repairAction": "AUTO"},
                    "deepHealthChecks": {"onStart": {"profile": "MEDIUM"}},
                },
            }),
            json!({
                "securityGroupIds": [security_group(4)],
                "sshKeyName": "pcs-key-omega",
                "launchTemplateMode": "CUSTOM",
                "customLaunchTemplate": base_launch_template(),
                "instanceHealthChecks": {
                    "healthMonitoring": {"mode": "STANDARD", "repairAction": "AUTO"},
                    "deepHealthChecks": {"onStart": {"profile": "MEDIUM"}},
                },
            }),
        ),
    ];

    for (index, (case, patch, expected)) in rows.iter().enumerate() {
        let name = format!("members-{index}");
        let (status, created) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_group_with(&cluster, &name, patch),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the create to be served, got {status} with {created}"
        );
        assert_members(case, "the create's answer", &created["computeNodeGroup"], expected);

        // The create answered from the stored record, so every later read answers the same.
        let read = stored_group(&router, &cluster, &name).await;
        assert_members(case, "GetComputeNodeGroup", &read, expected);
        let again = stored_group(&router, &cluster, &name).await;
        assert_eq!(
            again, read,
            "case {case}: expected the group to be stable across reads, got {again}"
        );
    }
}

#[tokio::test]
async fn an_update_replaces_what_it_carries_and_the_empty_value_removes_a_direct_setting() {
    let router = emulator();
    let cluster = active_cluster(&router, "group-member-updates").await;
    let group = "updated-members";
    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_group_with(
            &cluster,
            group,
            &json!({
                "securityGroupIds": [security_group(1)],
                "sshKeyName": "pcs-key-alpha",
                "instanceHealthChecks": {
                    "healthMonitoring": {"mode": "STANDARD"},
                    "deepHealthChecks": {"onStart": {"profile": "QUICK"}},
                },
            }),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected the create to be served, got {status} with {created}"
    );

    // (case, the members the update carries, what every answer reports afterwards). The rows
    // run in order against the one group, so each starts from what the row before it stored.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "another security group membership",
            json!({"securityGroupIds": [security_group(2), security_group(3)]}),
            json!({
                "securityGroupIds": [security_group(2), security_group(3)],
                "sshKeyName": "pcs-key-alpha",
            }),
        ),
        (
            "the empty membership removes the direct configuration",
            json!({"securityGroupIds": []}),
            json!({"securityGroupIds": null, "sshKeyName": "pcs-key-alpha"}),
        ),
        (
            "a key pair of its own",
            json!({"sshKeyName": "pcs-key-beta"}),
            json!({"sshKeyName": "pcs-key-beta", "securityGroupIds": null}),
        ),
        (
            "the empty key pair name removes the direct configuration",
            json!({"sshKeyName": ""}),
            json!({"sshKeyName": null}),
        ),
        (
            "a launch template PCS manages",
            json!({"launchTemplateMode": "MANAGED"}),
            json!({"launchTemplateMode": "MANAGED", "customLaunchTemplate": null}),
        ),
        (
            "back to the caller's own template",
            json!({"launchTemplateMode": "CUSTOM"}),
            json!({
                "launchTemplateMode": "CUSTOM",
                "customLaunchTemplate": base_launch_template(),
            }),
        ),
        (
            "a template of the caller's own, replacing the stored one",
            json!({"customLaunchTemplate": {"id": "lt-0abcdef1234567890", "version": "7"}}),
            json!({"customLaunchTemplate": {"id": "lt-0abcdef1234567890", "version": "7"}}),
        ),
        (
            "a whole new set of instance health checks",
            json!({"instanceHealthChecks": {"healthMonitoring": {"mode": "NONE"}}}),
            json!({"instanceHealthChecks": {"healthMonitoring": {"mode": "NONE"}}}),
        ),
        (
            "an update that carries none of these members",
            json!({"amiId": "ami-0123456789abcdef0"}),
            json!({
                "amiId": "ami-0123456789abcdef0",
                "securityGroupIds": null,
                "sshKeyName": null,
                "launchTemplateMode": "CUSTOM",
                "customLaunchTemplate": {"id": "lt-0abcdef1234567890", "version": "7"},
                "instanceHealthChecks": {"healthMonitoring": {"mode": "NONE"}},
            }),
        ),
    ];

    for (case, patch, expected) in &rows {
        let (status, updated) = call(
            &router,
            "UpdateComputeNodeGroup",
            &update_group_with(&cluster, group, patch),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the update to be served, got {status} with {updated}"
        );
        assert_members(case, "the update's answer", &updated["computeNodeGroup"], expected);

        // The update answered from the stored record, so the next read answers the same —
        // and that read is what settles the group back to ACTIVE for the next row.
        let read = stored_group(&router, &cluster, group).await;
        assert_members(case, "GetComputeNodeGroup", &read, expected);
    }
}

#[tokio::test]
async fn a_cluster_keeps_the_job_submit_plugins_the_request_carried() {
    let router = emulator();

    // (case, the `jobSubmitPlugins` the create carries, what every read reports for it)
    let rows: Vec<(&str, Value, Value)> = vec![
        ("no jobSubmitPlugins member at all", json!(null), json!(null)),
        (
            "an empty plugin list, the published minimum",
            json!([]),
            json!([]),
        ),
        (
            "one plugin, carrying only its required members",
            json!([job_submit_plugin("validate-jobs")]),
            json!([job_submit_plugin("validate-jobs")]),
        ),
        (
            "one plugin pinned to a script version and checksum",
            json!([pinned_job_submit_plugin("pin-jobs")]),
            json!([pinned_job_submit_plugin("pin-jobs")]),
        ),
    ];

    for (index, (case, plugins, expected)) in rows.iter().enumerate() {
        let name = format!("plugins-{index}");
        let mut request = create_cluster(&name, "SMALL");
        if !plugins.is_null() {
            request["slurmConfiguration"] = json!({"jobSubmitPlugins": plugins});
        }
        let (status, created) = call(&router, "CreateCluster", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the create to be served, got {status} with {created}"
        );
        let reported = &created["cluster"]["slurmConfiguration"]["jobSubmitPlugins"];
        assert_eq!(
            reported, expected,
            "case {case}: expected the create to report {expected}, got {reported:?} in \
             {created}"
        );

        let (status, read) = call(&router, "GetCluster", &json!({"clusterIdentifier": &name})).await;
        assert_eq!(status, 200, "case {case}: GetCluster got {status} with {read}");
        let reported = &read["cluster"]["slurmConfiguration"]["jobSubmitPlugins"];
        assert_eq!(
            reported, expected,
            "case {case}: expected the read to report {expected}, got {reported:?} in {read}"
        );
    }

    // "The job submit plugins to run on the cluster controller" is a list the model bounds at
    // one entry (`@length(min: 0, max: 1)` on `JobSubmitPluginList`), so a request past that
    // maximum is refused and nothing of it is stored.
    let mut request = create_cluster("plugins-past-the-bound", "SMALL");
    request["slurmConfiguration"] = json!({"jobSubmitPlugins": [
        job_submit_plugin("first-plugin"), job_submit_plugin("second-plugin"),
    ]});
    let (status, refused) = call(&router, "CreateCluster", &request).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case two plugins: expected the create to be refused, got {status} with {refused}"
    );
    let (status, read) = call(
        &router,
        "GetCluster",
        &json!({"clusterIdentifier": "plugins-past-the-bound"}),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case two plugins: expected no cluster to have been stored, got {status} with {read}"
    );
}

#[tokio::test]
async fn an_update_replaces_the_job_submit_plugins_a_cluster_runs() {
    let router = emulator();
    let name = "plugin-updates";
    let mut request = create_cluster(name, "SMALL");
    request["slurmConfiguration"] = json!({"jobSubmitPlugins": [job_submit_plugin("first-set")]});
    let (status, created) = call(&router, "CreateCluster", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected the create to be served, got {status} with {created}"
    );

    // (case, the `slurmConfiguration` the update carries, the plugins every answer reports)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "an update that names no plugin list",
            json!({"scaleDownIdleTimeInSeconds": 900}),
            json!([job_submit_plugin("first-set")]),
        ),
        (
            "a plugin list that replaces the stored one",
            json!({"jobSubmitPlugins": [pinned_job_submit_plugin("second-set")]}),
            json!([pinned_job_submit_plugin("second-set")]),
        ),
        (
            "an empty plugin list, which runs no plugin",
            json!({"jobSubmitPlugins": []}),
            json!([]),
        ),
    ];

    for (case, slurm, expected) in &rows {
        let (status, updated) = call(
            &router,
            "UpdateCluster",
            &json!({"clusterIdentifier": name, "slurmConfiguration": slurm}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the update to be served, got {status} with {updated}"
        );
        let reported = &updated["cluster"]["slurmConfiguration"]["jobSubmitPlugins"];
        assert_eq!(
            reported, expected,
            "case {case}: expected the update to report {expected}, got {reported:?} in \
             {updated}"
        );

        // The next read answers the stored record — and settles the cluster back to ACTIVE
        // for the row that follows.
        let (status, read) = call(&router, "GetCluster", &json!({"clusterIdentifier": name})).await;
        assert_eq!(status, 200, "case {case}: GetCluster got {status} with {read}");
        let reported = &read["cluster"]["slurmConfiguration"]["jobSubmitPlugins"];
        assert_eq!(
            reported, expected,
            "case {case}: expected the read to report {expected}, got {reported:?} in {read}"
        );
    }
}
