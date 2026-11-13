// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a PCS cluster publishes once it is provisioned, the
//! scheduler versions the service accepts, and `UpdateCluster`.
//!
//! Driven over the awsJson1_0 wire through the same router the emulator serves from, on
//! the harness [`crate::cluster_lifecycle`] already builds.

use serde_json::{json, Value};

use crate::cluster_lifecycle::{call, emulator, wait_past_the_owed_transition};

/// Values that LABEL a scheduler release, so the service carries on with them.
///
/// The first four rows are the "Valid Values" `SchedulerRequest$version` prints today
/// (`smithy_query(query="shape",shape="SchedulerRequest")`); the rest are release labels
/// that list does not name — an older Slurm release, a patch release, a one-digit minor and
/// a far-future major — because the member is a plain `String` with no enum, `@pattern` or
/// `@length`, so the printed list is prose rather than a wire constraint.
const RELEASE_LABELS: &[&str] = &[
    "24.11", "25.05", "25.11", "26.05", "23.11", "25.05.1", "25.5", "9999.1",
];

/// Values that label no scheduler release at all: an empty value, a word, a label with a
/// component that is not digits, too few and too many components, an empty component, a
/// component past four digits, and a label with surrounding space.
const NOT_RELEASE_LABELS: &[&str] = &[
    "",
    "latest",
    "1.0-not-a-real-version",
    "24",
    "24.11.2.3",
    "24.",
    "12345.1",
    " 25.05",
];

/// Which daemon port each `EndpointType` is published on.
///
/// Source: the modelled `EndpointType` members (`SLURMCTLD | SLURMDBD | SLURMRESTD`,
/// `smithy_query(query="shape",shape="EndpointType")`) paired with Slurm's own default
/// port for each of those daemons.
const ENDPOINT_PORTS: &[(&str, &str)] = &[
    ("SLURMCTLD", "6817"),
    ("SLURMDBD", "6819"),
    ("SLURMRESTD", "6820"),
];

/// A `CreateCluster` request carrying every `@required` member, on `version` and
/// `network_type`, plus `slurm` as its `slurmConfiguration` when one is given.
fn create_cluster(name: &str, version: &str, network_type: &str, slurm: Option<Value>) -> Value {
    let mut body = json!({
        "clusterName": name,
        "scheduler": {"type": "SLURM", "version": version},
        "size": "SMALL",
        "networking": {
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "networkType": network_type,
        },
    });
    if let Some(slurm) = slurm {
        body["slurmConfiguration"] = slurm;
    }
    body
}

/// The `endpoints` member `GetCluster` reports for `identifier`, as a list.
async fn endpoints_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GetCluster",
        &json!({"clusterIdentifier": identifier}),
    )
    .await;
    assert_eq!(status, 200, "case GetCluster {identifier}: got {body}");
    body["cluster"]["endpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The port [`ENDPOINT_PORTS`] publishes `endpoint_type` on.
fn published_port(endpoint_type: &str) -> &'static str {
    ENDPOINT_PORTS
        .iter()
        .find(|(kind, _)| *kind == endpoint_type)
        .map(|(_, port)| *port)
        .unwrap_or_else(|| panic!("case {endpoint_type}: not a modelled EndpointType"))
}

#[tokio::test]
async fn an_active_cluster_publishes_the_scheduler_endpoints_its_configuration_enables() {
    // (case, networkType, slurmConfiguration, the endpoint types the cluster publishes)
    let rows: [(&str, &str, Option<Value>, &[&str]); 6] = [
        ("plain-ipv4", "IPV4", None, &["SLURMCTLD"]),
        (
            "accounting-on",
            "IPV4",
            Some(json!({"accounting": {"mode": "STANDARD"}})),
            &["SLURMCTLD", "SLURMDBD"],
        ),
        (
            "accounting-off",
            "IPV4",
            Some(json!({"accounting": {"mode": "NONE"}})),
            &["SLURMCTLD"],
        ),
        (
            "slurmrest-on",
            "IPV4",
            Some(json!({"slurmRest": {"mode": "STANDARD"}})),
            &["SLURMCTLD", "SLURMRESTD"],
        ),
        (
            "every-daemon",
            "IPV4",
            Some(json!({
                "accounting": {"mode": "STANDARD"},
                "slurmRest": {"mode": "STANDARD"},
            })),
            &["SLURMCTLD", "SLURMDBD", "SLURMRESTD"],
        ),
        (
            "every-daemon-ipv6",
            "IPV6",
            Some(json!({
                "accounting": {"mode": "STANDARD"},
                "slurmRest": {"mode": "STANDARD"},
            })),
            &["SLURMCTLD", "SLURMDBD", "SLURMRESTD"],
        ),
    ];

    let router = emulator();
    let mut controller_addresses: Vec<String> = Vec::new();

    for (case, network_type, slurm, expected) in rows {
        let name = format!("endpoints-{case}");
        let (status, created) = call(
            &router,
            "CreateCluster",
            &create_cluster(&name, "25.05", network_type, slurm),
        )
        .await;
        assert_eq!(status, 200, "case {case}: CreateCluster got {created}");
        // A cluster that has not finished provisioning publishes no endpoint yet.
        assert!(
            created["cluster"]["endpoints"].is_null(),
            "case {case}: expected no endpoints while CREATING, got {created}"
        );

        wait_past_the_owed_transition().await;

        let published = endpoints_of(&router, &name).await;
        let types: Vec<&str> = published
            .iter()
            .map(|endpoint| endpoint["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            types, expected,
            "case {case}: expected the endpoint types {expected:?}, got {types:?} \
             ({published:?})"
        );

        for endpoint in &published {
            let endpoint_type = endpoint["type"].as_str().unwrap_or_default();
            assert_eq!(
                endpoint["port"],
                json!(published_port(endpoint_type)),
                "case {case}/{endpoint_type}: expected the daemon's own port, got {endpoint}"
            );
            let private = endpoint["privateIpAddress"].as_str().unwrap_or_default();
            if network_type == "IPV6" {
                // "For clusters configured to use IPv6, this is an empty string."
                assert_eq!(
                    private, "",
                    "case {case}/{endpoint_type}: expected an empty privateIpAddress on an \
                     IPv6 cluster, got {endpoint}"
                );
                let ipv6 = endpoint["ipv6Address"].as_str().unwrap_or_default();
                assert!(
                    ipv6.parse::<std::net::Ipv6Addr>().is_ok(),
                    "case {case}/{endpoint_type}: expected an IPv6 address, got {endpoint}"
                );
            } else {
                assert!(
                    private.parse::<std::net::Ipv4Addr>().is_ok(),
                    "case {case}/{endpoint_type}: expected an IPv4 privateIpAddress, got \
                     {endpoint}"
                );
                assert!(
                    endpoint["ipv6Address"].is_null(),
                    "case {case}/{endpoint_type}: expected no ipv6Address on an IPv4 \
                     cluster, got {endpoint}"
                );
                if endpoint_type == "SLURMCTLD" {
                    controller_addresses.push(private.to_string());
                }
            }
        }

        // A read never changes what the next caller sees.
        let again = endpoints_of(&router, &name).await;
        assert_eq!(
            again, published,
            "case {case}: expected the endpoints to be stable across reads, got {again:?}"
        );
    }

    let mut distinct = controller_addresses.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        controller_addresses.len(),
        "case controller addresses: expected one per cluster, got {controller_addresses:?}"
    );
}

#[tokio::test]
async fn create_cluster_accepts_every_scheduler_release_label() {
    let router = emulator();

    for (index, version) in RELEASE_LABELS.iter().enumerate() {
        let name = format!("supported-version-{index}");
        let (status, body) = call(
            &router,
            "CreateCluster",
            &create_cluster(&name, version, "IPV4", None),
        )
        .await;
        assert_eq!(status, 200, "case version {version}: got {body}");
        assert_eq!(
            body["cluster"]["scheduler"]["version"],
            json!(version),
            "case version {version}: expected the stored scheduler version, got {body}"
        );
    }

    // A value that labels no scheduler release is refused in the create path's own
    // wording, which names the scheduler type and the version the request carried.
    for (index, version) in NOT_RELEASE_LABELS.iter().enumerate() {
        let name = format!("unsupported-version-{index}");
        let (status, refused) = call(
            &router,
            "CreateCluster",
            &create_cluster(&name, version, "IPV4", None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case version {version:?}: expected the create to be refused, got {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case version {version:?}: got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!(format!(
                "Scheduler version not found: type=SLURM, version={version}"
            )),
            "case version {version:?}: got {refused}"
        );

        // Nothing was persisted for the refused request.
        let (status, listed) = call(&router, "ListClusters", &json!({"maxResults": 100})).await;
        assert_eq!(status, 200, "case version {version:?}: ListClusters {listed}");
        let names: Vec<&str> = listed["clusters"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row["name"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !names.contains(&name.as_str()),
            "case version {version:?}: expected no cluster to have been created, got {names:?}"
        );
        assert_eq!(
            names.len(),
            RELEASE_LABELS.len(),
            "case version {version:?}: expected only the accepted clusters, got {names:?}"
        );
    }
}

#[tokio::test]
async fn update_cluster_accepts_every_scheduler_release_label() {
    let router = emulator();
    let name = "update-versions";
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster(name, "24.11", "IPV4", None),
    )
    .await;
    assert_eq!(status, 200, "case create: got {created}");
    let id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_past_the_owed_transition().await;

    // The update path holds `version` to the same rule, so every release label is applied
    // and reads back — including the ones no printed list names.
    for version in RELEASE_LABELS {
        let (status, updated) = call(
            &router,
            "UpdateCluster",
            &json!({"clusterIdentifier": &id, "scheduler": {"version": version}}),
        )
        .await;
        assert_eq!(status, 200, "case update to {version}: got {updated}");
        assert_eq!(
            updated["cluster"]["status"],
            json!("UPDATING"),
            "case update to {version}: expected the transient status, got {updated}"
        );
        assert_eq!(
            updated["cluster"]["scheduler"]["version"],
            json!(version),
            "case update to {version}: got {updated}"
        );

        wait_past_the_owed_transition().await;

        let (status, settled) =
            call(&router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
        assert_eq!(status, 200, "case read back {version}: got {settled}");
        assert_eq!(
            settled["cluster"]["status"],
            json!("ACTIVE"),
            "case read back {version}: expected the cluster to settle back, got {settled}"
        );
        assert_eq!(
            settled["cluster"]["scheduler"]["version"],
            json!(version),
            "case read back {version}: expected the updated version to persist, got {settled}"
        );
    }

    // The version the cluster is left on by the loop above.
    let settled_version = RELEASE_LABELS.last().copied().unwrap_or_default();

    // A value that labels no release is refused, and the cluster is untouched.
    for version in NOT_RELEASE_LABELS {
        let (status, refused) = call(
            &router,
            "UpdateCluster",
            &json!({"clusterIdentifier": &id, "scheduler": {"version": version}}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case update to {version:?}: expected a refusal, got {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ValidationException"),
            "case update to {version:?}: got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!("AWS PCS doesn't support the scheduler version you provided."),
            "case update to {version:?}: got {refused}"
        );

        let (status, unchanged) =
            call(&router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
        assert_eq!(status, 200, "case update to {version:?}: got {unchanged}");
        assert_eq!(
            unchanged["cluster"]["status"],
            json!("ACTIVE"),
            "case update to {version:?}: expected the cluster left ACTIVE, got {unchanged}"
        );
        assert_eq!(
            unchanged["cluster"]["scheduler"]["version"],
            json!(settled_version),
            "case update to {version:?}: expected the stored version untouched, got \
             {unchanged}"
        );
    }
}

#[tokio::test]
async fn update_cluster_persists_the_slurm_options_the_request_carries() {
    let router = emulator();
    let name = "update-slurm";
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster(
            name,
            "25.05",
            "IPV4",
            Some(json!({"scaleDownIdleTimeInSeconds": 600})),
        ),
    )
    .await;
    assert_eq!(status, 200, "case create: got {created}");
    wait_past_the_owed_transition().await;

    // `scaleDownIdleTimeInSeconds` is `@range(min: 1, max: 10000000)`: both published
    // bounds and a value between them are accepted and persist.
    for seconds in [1_i64, 700, 10_000_000] {
        let (status, updated) = call(
            &router,
            "UpdateCluster",
            &json!({
                "clusterIdentifier": name,
                "slurmConfiguration": {"scaleDownIdleTimeInSeconds": seconds},
            }),
        )
        .await;
        assert_eq!(status, 200, "case scaleDownIdleTime {seconds}: got {updated}");
        assert_eq!(
            updated["cluster"]["status"],
            json!("UPDATING"),
            "case scaleDownIdleTime {seconds}: expected the transient status, got {updated}"
        );

        wait_past_the_owed_transition().await;

        let (status, settled) =
            call(&router, "GetCluster", &json!({"clusterIdentifier": name})).await;
        assert_eq!(status, 200, "case scaleDownIdleTime {seconds}: got {settled}");
        assert_eq!(
            settled["cluster"]["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
            json!(seconds),
            "case scaleDownIdleTime {seconds}: expected the value to persist, got {settled}"
        );
        assert_eq!(
            settled["cluster"]["status"],
            json!("ACTIVE"),
            "case scaleDownIdleTime {seconds}: expected the cluster to settle back, got \
             {settled}"
        );
    }

    // One past each published bound is refused by the modelled range.
    for seconds in [0_i64, 10_000_001] {
        let (status, refused) = call(
            &router,
            "UpdateCluster",
            &json!({
                "clusterIdentifier": name,
                "slurmConfiguration": {"scaleDownIdleTimeInSeconds": seconds},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case scaleDownIdleTime {seconds}: expected a refusal past the bound, got \
             {refused}"
        );
    }

    // Turning accounting on through an update publishes the daemon's endpoint once the
    // cluster settles: the endpoints follow the STORED configuration.
    let before = endpoints_of(&router, name).await;
    assert_eq!(
        before.len(),
        1,
        "case accounting off: expected only the controller endpoint, got {before:?}"
    );
    let (status, updated) = call(
        &router,
        "UpdateCluster",
        &json!({
            "clusterIdentifier": name,
            "slurmConfiguration": {"accounting": {"mode": "STANDARD", "defaultPurgeTimeInDays": 7}},
        }),
    )
    .await;
    assert_eq!(status, 200, "case accounting on: got {updated}");
    wait_past_the_owed_transition().await;

    let (status, settled) = call(&router, "GetCluster", &json!({"clusterIdentifier": name})).await;
    assert_eq!(status, 200, "case accounting on: got {settled}");
    let slurm = &settled["cluster"]["slurmConfiguration"];
    assert_eq!(
        slurm["accounting"],
        json!({"mode": "STANDARD", "defaultPurgeTimeInDays": 7}),
        "case accounting on: expected the accounting options to persist, got {settled}"
    );
    // A member the update did not name keeps the value the previous call stored.
    assert_eq!(
        slurm["scaleDownIdleTimeInSeconds"],
        json!(10_000_000),
        "case accounting on: expected the untouched member to keep its value, got {settled}"
    );
    let after: Vec<&str> = settled["cluster"]["endpoints"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["type"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        after,
        vec!["SLURMCTLD", "SLURMDBD"],
        "case accounting on: expected the accounting endpoint to be published, got {settled}"
    );

    // An update that asks for nothing changes nothing, including the status.
    let (status, nothing) = call(
        &router,
        "UpdateCluster",
        &json!({"clusterIdentifier": name}),
    )
    .await;
    assert_eq!(status, 200, "case empty update: got {nothing}");
    assert_eq!(
        nothing["cluster"]["status"],
        json!("ACTIVE"),
        "case empty update: expected the cluster left ACTIVE, got {nothing}"
    );
}

#[tokio::test]
async fn update_cluster_refuses_a_cluster_it_cannot_find_or_cannot_update() {
    let router = emulator();

    // Nothing by that identifier, in either of the two forms `ClusterIdentifier` allows.
    for identifier in ["no-such-cluster", "pcs_0123456789abcdef"] {
        let (status, missing) = call(
            &router,
            "UpdateCluster",
            &json!({
                "clusterIdentifier": identifier,
                "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 700},
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case UpdateCluster {identifier}: expected a not-found, got {missing}"
        );
        assert_eq!(
            missing["__type"],
            json!("ResourceNotFoundException"),
            "case UpdateCluster {identifier}: got {missing}"
        );
    }

    // A cluster that has not finished provisioning cannot be updated.
    let name = "still-creating";
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster(name, "25.05", "IPV4", None),
    )
    .await;
    assert_eq!(status, 200, "case create: got {created}");
    let (status, refused) = call(
        &router,
        "UpdateCluster",
        &json!({
            "clusterIdentifier": name,
            "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 700},
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case update while CREATING: expected a conflict, got {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ConflictException"),
        "case update while CREATING: got {refused}"
    );
    assert_eq!(
        refused["message"],
        json!("A cluster isn't in ACTIVE status."),
        "case update while CREATING: got {refused}"
    );

    wait_past_the_owed_transition().await;

    // Once it is ACTIVE the same request is served, resolved by name OR by id.
    let (status, updated) = call(
        &router,
        "UpdateCluster",
        &json!({
            "clusterIdentifier": name,
            "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 700},
        }),
    )
    .await;
    assert_eq!(status, 200, "case update once ACTIVE: got {updated}");
    let id = updated["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_past_the_owed_transition().await;
    let (status, by_id) = call(
        &router,
        "UpdateCluster",
        &json!({
            "clusterIdentifier": &id,
            "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 800},
        }),
    )
    .await;
    assert_eq!(status, 200, "case update by id: got {by_id}");
    assert_eq!(
        by_id["cluster"]["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
        json!(800),
        "case update by id: expected the new value, got {by_id}"
    );
}

#[tokio::test]
async fn a_not_found_refusal_names_the_resource_kind_and_no_identifier() {
    let router = emulator();
    let name = "lookup-cluster";
    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster(name, "25.05", "IPV4", None),
    )
    .await;
    assert_eq!(status, 200, "case create: got {created}");
    wait_past_the_owed_transition().await;

    // Two identifiers of each of the forms `ClusterIdentifier` allows: the sentence is
    // the same for every one of them, so it carries none of them.
    let unknown = ["no-such-cluster", "AnotherMissingOne", "pcs_0123456789abcdef"];
    for identifier in unknown {
        // (operation, request, the kind the refusal names)
        let cases: [(&str, Value, &str); 5] = [
            (
                "GetCluster",
                json!({"clusterIdentifier": identifier}),
                "cluster",
            ),
            (
                "DeleteCluster",
                json!({"clusterIdentifier": identifier}),
                "cluster",
            ),
            (
                "ListQueues",
                json!({"clusterIdentifier": identifier}),
                "cluster",
            ),
            (
                "GetQueue",
                json!({"clusterIdentifier": name, "queueIdentifier": identifier}),
                "queue",
            ),
            (
                "GetComputeNodeGroup",
                json!({
                    "clusterIdentifier": name,
                    "computeNodeGroupIdentifier": identifier,
                }),
                "compute node group",
            ),
        ];
        for (operation, request, kind) in cases {
            let (status, missing) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {operation} {identifier}: expected a not-found, got {missing}"
            );
            assert_eq!(
                missing["__type"],
                json!("ResourceNotFoundException"),
                "case {operation} {identifier}: got {missing}"
            );
            assert_eq!(
                missing["message"],
                json!(format!("AWS PCS couldn't find the {kind} you specified.")),
                "case {operation} {identifier}: got {missing}"
            );
            assert_eq!(
                missing["resourceId"],
                json!(identifier),
                "case {operation} {identifier}: expected the identifier in resourceId, got \
                 {missing}"
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_cluster_name_is_refused_with_the_services_own_sentence() {
    let router = emulator();
    // The sentence is fixed text: it is the same for every name, and names none of them.
    for name in ["dup-one", "AnotherCluster", "x-9-cluster-name"] {
        let (status, created) = call(
            &router,
            "CreateCluster",
            &create_cluster(name, "25.05", "IPV4", None),
        )
        .await;
        assert_eq!(status, 200, "case create {name}: got {created}");
        let created_at = created["cluster"]["createdAt"].clone();

        let (status, refused) = call(
            &router,
            "CreateCluster",
            &create_cluster(name, "25.11", "IPV4", None),
        )
        .await;
        assert_eq!(
            status, 409,
            "case duplicate {name}: expected a conflict, got {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("ConflictException"),
            "case duplicate {name}: got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!("A cluster with the name you specified already exists."),
            "case duplicate {name}: got {refused}"
        );
        assert!(
            !refused["message"]
                .as_str()
                .unwrap_or_default()
                .contains(name),
            "case duplicate {name}: the message must not echo the requested name, got \
             {refused}"
        );

        // The first cluster survives the refusal untouched.
        let (status, survivor) =
            call(&router, "GetCluster", &json!({"clusterIdentifier": name})).await;
        assert_eq!(status, 200, "case duplicate {name}: got {survivor}");
        assert_eq!(
            survivor["cluster"]["createdAt"], created_at,
            "case duplicate {name}: expected the first cluster untouched, got {survivor}"
        );
        assert_eq!(
            survivor["cluster"]["scheduler"]["version"],
            json!("25.05"),
            "case duplicate {name}: expected the first cluster's version, got {survivor}"
        );
    }
}
