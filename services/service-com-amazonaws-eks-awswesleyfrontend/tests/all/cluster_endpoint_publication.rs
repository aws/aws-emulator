// SPDX-License-Identifier: CC0-1.0

//! What the ADDRESS a cluster publishes for its Kubernetes API server is, as every
//! read of that cluster publishes it.
//!
//! The RULE under test: `Cluster$endpoint` is "the endpoint for your Kubernetes API
//! server", and `DescribeCluster` states that "the API server endpoint and
//! certificate authority data are required for kubelet and kubectl to communicate
//! with your Kubernetes API server" and "aren't available until the cluster reaches
//! the ACTIVE state". A client therefore takes the member AS PUBLISHED and talks to
//! it — so the value every read of a cluster publishes has to be an address THIS
//! emulator answers THAT cluster's Kubernetes API on, and a cluster with no control
//! plane yet publishes no address at all rather than one nothing serves.
//!
//! Each suite drives the rule over a TABLE whose rows differ along what it
//! generalises over: the region the cluster was created in, the form and length of
//! its name, the read the value was taken from (the create's own answer while the
//! cluster is still `CREATING`, the describe that follows, and a later describe),
//! and — for what answers at the address — the namespace a client creates through
//! it, whether that namespace already existed, and which cluster's address the
//! request was sent to.
//!
//! The client is the same raw one the Kubernetes API suite uses
//! ([`crate::kubernetes_api::dial`]): a TCP connection to the published port, TLS
//! whose only trust anchor is the published certificate authority, and an HTTP/1.1
//! request written by hand — nothing but what the emulator published is used to
//! reach it.

use service_com_amazonaws_eks_awswesleyfrontend::state::MAX_CLUSTER_NAME_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{create_cluster, describe_cluster, str_member, REGION};
use crate::kubernetes_api::{dial, kubeconfig_of, poll_until_active, Kubeconfig};
use crate::wire::emulator;

/// One cluster of a table: the case it stands for, where it lives and what it is
/// called.
struct ClusterCase {
    label: &'static str,
    region: &'static str,
    name: String,
}

/// The hosts this emulator answers on, which is what a published address has to
/// name: the loopback interface the core binds a per-resource listener to unless
/// the operator chose otherwise.
const OWN_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "[::1]", "::1"];

/// Create every cluster of the table FIRST and only then wait for any of them: the
/// move to `ACTIVE` is one the clock owes each cluster from its own create, so the
/// waits overlap instead of adding up. Answers one kubeconfig per row, in the
/// table's order, beside the `DescribeCluster` answer it was read from.
async fn provisioned(
    router: &MultiServiceRouter,
    cases: &[ClusterCase],
) -> Vec<(Kubeconfig, crate::wire::Answer)> {
    for case in cases {
        let created = create_cluster(router, case.region, &case.name).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200 from CreateCluster, got {created:?}",
            case.label
        );
        // "The API server endpoint and certificate authority data aren't available
        // until the cluster reaches the ACTIVE state": the create answers a cluster
        // whose control plane does not exist yet, so it publishes NO address —
        // never a placeholder a client could be pointed at.
        assert_eq!(
            created.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {:?}: expected a CREATING cluster from CreateCluster, got {:?}",
            case.label,
            created.cluster()["status"]
        );
        assert!(
            created.cluster()["endpoint"].is_null(),
            "case {:?}: a cluster with no control plane yet publishes no endpoint, got {:?}",
            case.label,
            created.cluster()["endpoint"]
        );
    }
    let mut provisioned = Vec::new();
    for case in cases {
        let described = poll_until_active(router, case.region, &case.name).await;
        let config = kubeconfig_of(&case.name, &described);
        provisioned.push((config, described));
    }
    provisioned
}

/// The `host` and `port` of a published endpoint, asserted to be an address of this
/// emulator's own: an `https` URL on one of [`OWN_HOSTS`] with the port its listener
/// answers on, and never a name under AWS's own zone — which resolves to AWS
/// production or to nothing on the machine the emulator runs on.
fn own_address(label: &str, endpoint: &str) -> (String, u16) {
    let authority = endpoint.strip_prefix("https://").unwrap_or_else(|| {
        panic!("case {label:?}: the endpoint must be an https URL, got {endpoint:?}")
    });
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {label:?}: the endpoint must carry a port, got {endpoint:?}"));
    assert!(
        !host.ends_with("amazonaws.com"),
        "case {label:?}: the endpoint must be an address this emulator serves, not a name under \
         AWS's own zone, got {endpoint:?}"
    );
    assert!(
        OWN_HOSTS.contains(&host),
        "case {label:?}: expected one of this emulator's own hosts {OWN_HOSTS:?}, got {host:?} in \
         {endpoint:?}"
    );
    let port: u16 = port
        .parse()
        .unwrap_or_else(|error| panic!("case {label:?}: {port:?} must be a port: {error}"));
    assert_ne!(
        port, 0,
        "case {label:?}: the endpoint must name the port its listener answers on, got {endpoint:?}"
    );
    (host.to_string(), port)
}

// ---------------------------------------------------------------------------
// RULE: every read that publishes a cluster's API server address publishes one
// this emulator answers THAT cluster's Kubernetes API on — the same value for
// every read of one cluster, a different one per cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_read_publishes_an_address_this_emulator_serves() {
    let router = emulator();
    // The rows differ along what the rule generalises over: the region, the form of
    // the name (hyphens, underscores, a single character, the modelled 100-character
    // ceiling) and one name used in two regions, which is two clusters and so two
    // addresses.
    let cases = [
        ClusterCase {
            label: "a cluster in the suite's region",
            region: REGION,
            name: "endpoint-published-one".to_string(),
        },
        ClusterCase {
            label: "a name spelled with underscores",
            region: REGION,
            name: "endpoint_published_two".to_string(),
        },
        ClusterCase {
            label: "the same name in another region",
            region: "eu-central-1",
            name: "endpoint-published-one".to_string(),
        },
        ClusterCase {
            label: "a one-character name",
            region: "ap-southeast-2",
            name: "e".to_string(),
        },
        ClusterCase {
            // `CreateCluster$name` is `@length(1..100)`
            // (`service_com_amazonaws_eks_awswesleyfrontend::state::MAX_CLUSTER_NAME_LEN`),
            // so a name at that ceiling is a cluster the API admits.
            label: "a name at the modelled length ceiling",
            region: REGION,
            name: "e".repeat(MAX_CLUSTER_NAME_LEN),
        },
    ];

    let provisioned = provisioned(&router, &cases).await;
    let mut addresses: Vec<(&str, String)> = Vec::new();
    for (case, (config, described)) in cases.iter().zip(provisioned.iter()) {
        let label = case.label;
        let (_host, _port) = own_address(label, &config.endpoint);

        // The value is the cluster's, not one call's: a later read of the same
        // cluster publishes the identical address.
        let again = describe_cluster(&router, case.region, &case.name).await;
        assert_eq!(
            again.cluster()["endpoint"],
            described.cluster()["endpoint"],
            "case {label:?}: every read of one cluster publishes one address, got {:?} then {:?}",
            described.cluster()["endpoint"],
            again.cluster()["endpoint"]
        );

        // And what answers at the address is THIS cluster's API server: the
        // Kubernetes version it serves is the version the control plane reports for
        // this very cluster. The TLS handshake inside `dial` is itself the assertion
        // that the certificate served there is the one the published authority
        // issued.
        let served = dial(config, label, "GET", "/version", None).await;
        assert_eq!(
            served.status, 200,
            "case {label:?}: the published address must serve the Kubernetes API, got {served:?}"
        );
        let reported = str_member(label, "cluster.version", &described.cluster()["version"]);
        let mut parts = reported.split('.');
        let expected = format!(
            "{}.{}",
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default()
        );
        let answered = format!(
            "{}.{}",
            served.body["major"].as_str().unwrap_or_default(),
            served.body["minor"].as_str().unwrap_or_default()
        );
        assert_eq!(
            answered, expected,
            "case {label:?}: the API server at the published address is this cluster's, whose \
             version is {expected:?}, got {answered:?} in {:?}",
            served.body
        );

        addresses.push((label, config.endpoint.clone()));
    }

    let mut distinct: Vec<String> = addresses
        .iter()
        .map(|(_label, endpoint)| endpoint.clone())
        .collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        addresses.len(),
        "each cluster is reached at an address of its own, got {addresses:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the Kubernetes API served at a cluster's published address is that
// cluster's own — a namespace created through it is created there, a second create
// of the same name is the conflict it is, and a read of it on another cluster's
// address finds nothing.
// ---------------------------------------------------------------------------

/// One row of the namespace table: the namespace a client addresses through the
/// published endpoint, and whether the cluster already holds it.
struct NamespaceCase {
    label: &'static str,
    name: &'static str,
    /// `true` for a namespace Kubernetes creates with the cluster, which a create
    /// therefore conflicts with straight away.
    created_with_the_cluster: bool,
}

#[tokio::test]
async fn the_published_address_serves_the_clusters_own_namespaces() {
    let router = emulator();
    let cases = [
        ClusterCase {
            label: "the cluster the client talks to",
            region: REGION,
            name: "endpoint-namespaces".to_string(),
        },
        ClusterCase {
            label: "another cluster beside it",
            region: REGION,
            name: "endpoint-namespaces-peer".to_string(),
        },
    ];
    let provisioned = provisioned(&router, &cases).await;
    let (served, _described) = &provisioned[0];
    let (peer, _peer_described) = &provisioned[1];

    // The rows differ along what the rule generalises over: the form of the
    // namespace name, and whether the cluster already held it.
    let namespaces = [
        NamespaceCase {
            label: "a namespace the client names",
            name: "team-alpha",
            created_with_the_cluster: false,
        },
        NamespaceCase {
            label: "a namespace name carrying digits",
            name: "shard-2026",
            created_with_the_cluster: false,
        },
        NamespaceCase {
            label: "a namespace Kubernetes creates with the cluster",
            name: "default",
            created_with_the_cluster: true,
        },
    ];

    for case in &namespaces {
        let label = case.label;
        let manifest = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": { "name": case.name },
        });
        let created = dial(
            served,
            label,
            "POST",
            "/api/v1/namespaces",
            Some(&manifest),
        )
        .await;
        if case.created_with_the_cluster {
            // Nothing to create: the cluster came with it, so the first create is
            // already the conflict.
            assert_eq!(
                created.status, 409,
                "case {label:?}: a namespace the cluster already holds cannot be created again, \
                 got {created:?}"
            );
        } else {
            assert_eq!(
                created.status, 201,
                "case {label:?}: expected the namespace to be created at the published address, \
                 got {created:?}"
            );
            assert_eq!(
                created.body["kind"],
                serde_json::json!("Namespace"),
                "case {label:?}: expected a Namespace object, got {:?}",
                created.body
            );
            assert_eq!(
                created.body["metadata"]["name"],
                serde_json::json!(case.name),
                "case {label:?}: expected the namespace the manifest named, got {:?}",
                created.body
            );
            assert!(
                created.body["metadata"]["uid"]
                    .as_str()
                    .is_some_and(|uid| !uid.is_empty()),
                "case {label:?}: a created object carries the uid the server minted, got {:?}",
                created.body
            );
            assert_eq!(
                created.body["status"]["phase"],
                serde_json::json!("Active"),
                "case {label:?}: expected an Active namespace, got {:?}",
                created.body
            );

            // The SAME request again addresses a namespace that now exists.
            let replayed = dial(
                served,
                label,
                "POST",
                "/api/v1/namespaces",
                Some(&manifest),
            )
            .await;
            assert_eq!(
                replayed.status, 409,
                "case {label:?}: a second create of one namespace is a conflict, got {replayed:?}"
            );
            assert_eq!(
                replayed.body["reason"],
                serde_json::json!("AlreadyExists"),
                "case {label:?}: the conflict states that the namespace already exists, got {:?}",
                replayed.body
            );
        }

        // It reads back at the same address, as the object the create reported.
        let path = format!("/api/v1/namespaces/{}", case.name);
        let read = dial(served, label, "GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the namespace reads back at the published address, got {read:?}"
        );
        assert_eq!(
            read.body["metadata"]["name"],
            serde_json::json!(case.name),
            "case {label:?}: expected the namespace that was addressed, got {:?}",
            read.body
        );
        if !case.created_with_the_cluster {
            assert_eq!(
                read.body["metadata"]["uid"],
                created.body["metadata"]["uid"],
                "case {label:?}: the read object is the one the create minted, got {:?} after {:?}",
                read.body,
                created.body
            );

            // The address is the CLUSTER's: another cluster's address holds none of
            // this one's objects.
            let elsewhere = dial(peer, label, "GET", &path, None).await;
            assert_eq!(
                elsewhere.status, 404,
                "case {label:?}: another cluster's address must not hold this cluster's \
                 namespace, got {elsewhere:?}"
            );
            assert_eq!(
                elsewhere.body["reason"],
                serde_json::json!("NotFound"),
                "case {label:?}: expected the Kubernetes not-found Status, got {:?}",
                elsewhere.body
            );
        }
    }

    // Every namespace the client created through the published address is in the
    // listing that address serves.
    let listed = dial(served, "the cluster's namespaces", "GET", "/api/v1/namespaces", None).await;
    assert_eq!(
        listed.status, 200,
        "the published address lists the cluster's namespaces, got {listed:?}"
    );
    let names: Vec<String> = listed.body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for case in &namespaces {
        assert!(
            names.iter().any(|name| name == case.name),
            "case {:?}: expected {:?} among the namespaces the address lists, got {names:?}",
            case.label,
            case.name
        );
    }
}
