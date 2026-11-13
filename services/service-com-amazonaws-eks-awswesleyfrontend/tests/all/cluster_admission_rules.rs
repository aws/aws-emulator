// SPDX-License-Identifier: CC0-1.0

//! The cluster configurations Amazon EKS has NO cluster for, the access-entry
//! username it specifies itself, and the dependencies that keep a cluster from
//! being deleted — driven end to end through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Four rules are under test, each over a TABLE whose rows differ along what the
//! rule generalises over rather than along one recorded request:
//!
//! * A `CreateCluster` body that describes a cluster Amazon EKS cannot have is
//!   refused 400 `InvalidParameterException` and writes nothing: both API endpoints
//!   switched off, a `publicAccessCidrs` entry that is not a CIDR block, a service
//!   CIDR outside the private ranges or outside the published netmask bounds or
//!   named at all for an `ipv6` cluster, a partial EKS Auto Mode configuration, a
//!   Kubernetes version the catalog does not offer, and a version past standard
//!   support asked for under `STANDARD` support. The mirror table pins the accepted
//!   side of each one, including both published netmask bounds.
//! * A `username` is the caller's to choose only on a `STANDARD` access entry: for
//!   every other type Amazon EKS specifies it, so a create or an update that names
//!   one is refused and leaves the entry as it was.
//! * `UpdateClusterConfig` is judged on the configuration the cluster ENDS UP with,
//!   so the same endpoint, allow-list and EKS Auto Mode rules refuse a change that
//!   would leave it in a configuration Amazon EKS cannot have — and that refusal
//!   starts no update.
//! * `DeleteCluster` is refused while managed node groups or Fargate profiles are
//!   attached to the cluster, and a delete that arrives while an accepted deletion
//!   is still in flight finds the cluster in use rather than absent.
//!
//! Every assertion names its case and prints the actual value. The expected
//! sentences are written out here rather than read from the crate's own constants,
//! so this suite pins the WORDING a client sees instead of re-asserting whatever
//! the handler happens to carry.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, ComAmazonawsEksAwswesleyfrontendState, SupportStatus,
    ACCESS_ENTRY_TYPES, ACCESS_ENTRY_TYPE_STANDARD, CLUSTER_ACTIVATION_DELAY,
    CLUSTER_DELETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::wire::{call, emulator, emulator_with_engine, encode_label, error_code, Answer};

/// A role of the single account this emulator serves, which is what `iam:PassRole`
/// allows.
const ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// The node role an EKS Auto Mode compute configuration assigns its instances.
const NODE_ROLE: &str = "arn:aws:iam::000000000000:role/eksNodeRole";

/// The sentence a partial EKS Auto Mode configuration earns, written out here
/// rather than read from the crate's own constant so this suite pins the WORDING a
/// client sees.
const AUTO_MODE_ALL_OR_NOTHING: &str =
    "All of computeConfig.enabled, kubernetesNetworkConfig.elasticLoadBalancing.enabled, and \
     storageConfig.blockStorage.enabled must be set to either true or false.";

/// The two subnets every cluster here is placed in.
fn subnets() -> serde_json::Value {
    serde_json::json!(["subnet-aaaa1111", "subnet-bbbb2222"])
}

/// A `CreateCluster` body: the smallest accepted request with the row's own members
/// written over it.
fn request(name: &str, overrides: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    for (member, value) in overrides
        .as_object()
        .expect("a row's overrides are an object")
    {
        body[member.as_str()] = value.clone();
    }
    body
}

async fn create_cluster(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(router, REGION, "POST", "/clusters", &body.to_string()).await
}

async fn describe_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

async fn delete_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

/// The `message` an answer carries, which is what a client reads to see what it did
/// wrong.
fn message_of(answer: &Answer) -> String {
    answer.message()
}

/// Assert that one `CreateCluster` body is refused with `expected` and that nothing
/// of the cluster it named was written.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    body: &serde_json::Value,
    expected: &str,
) {
    let answer = create_cluster(router, body).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {answer:?}"
    );
    assert_eq!(
        message_of(&answer),
        expected,
        "case {case:?}: expected message {expected:?}, got {:?}",
        message_of(&answer)
    );
    let described = describe_cluster(router, name).await;
    assert_eq!(
        described.status, 404,
        "case {case:?}: a refused create must leave no cluster, got {described:?}"
    );
    assert_eq!(
        message_of(&described),
        cluster_not_found_message(name),
        "case {case:?}: expected the missing-cluster sentence, got {:?}",
        message_of(&described)
    );
}

// ---------------------------------------------------------------------------
// RULE: a `CreateCluster` body that describes a cluster Amazon EKS cannot have is
// refused 400 `InvalidParameterException` and writes nothing.
// ---------------------------------------------------------------------------

/// The rows differ along WHICH constraint the body breaks and along the VALUE that
/// breaks it: which endpoint switches are off, how a CIDR fails to parse and how
/// many of them do, how a service CIDR misses the private ranges or the netmask
/// bounds, which of the three EKS Auto Mode capabilities is out of step, and which
/// part of a compute configuration is missing.
#[tokio::test]
async fn a_cluster_configuration_eks_cannot_have_is_refused_and_writes_nothing() {
    let router = emulator();

    let rows = vec![
        (
            "both endpoint switches off",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "endpointPublicAccess": false,
                    "endpointPrivateAccess": false,
                },
            }),
            "Private and public endpoint access cannot be false".to_string(),
        ),
        (
            "public access off and private access left at its default",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "endpointPublicAccess": false,
                },
            }),
            "Private and public endpoint access cannot be false".to_string(),
        ),
        (
            "a publicAccessCidrs entry whose first octet is out of range",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "publicAccessCidrs": ["300.1.1.1/32"],
                },
            }),
            "The following CIDRs are invalid in publicAccessCidrs: [300.1.1.1/32]".to_string(),
        ),
        (
            "two unparseable publicAccessCidrs entries beside a valid one",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "publicAccessCidrs": ["10.0.0.0/8", "not-a-cidr", "192.168.0.0"],
                },
            }),
            "The following CIDRs are invalid in publicAccessCidrs: [not-a-cidr, 192.168.0.0]"
                .to_string(),
        ),
        (
            "a service CIDR outside the private ranges",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "8.8.0.0/16" } }),
            "Invalid Cluster Service Ipv4 CIDR 8.8.0.0/16 for the cluster refused-4, must be \
             within RFC1918 range"
                .to_string(),
        ),
        (
            "a service CIDR just outside the 172.16.0.0/12 private range",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "172.32.0.0/16" } }),
            "Invalid Cluster Service Ipv4 CIDR 172.32.0.0/16 for the cluster refused-5, must be \
             within RFC1918 range"
                .to_string(),
        ),
        (
            "a service CIDR one bit narrower than the /24 bound",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.100.0.0/25" } }),
            "Invalid Cluster Service Ipv4 CIDR 10.100.0.0/25 for the cluster refused-6, must \
             have netmask size between /12 and /24 inclusive"
                .to_string(),
        ),
        (
            "a service CIDR one bit wider than the /12 bound",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.0.0.0/11" } }),
            "Invalid Cluster Service Ipv4 CIDR 10.0.0.0/11 for the cluster refused-7, must have \
             netmask size between /12 and /24 inclusive"
                .to_string(),
        ),
        (
            "a service CIDR far narrower than the bounds",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.100.0.0/28" } }),
            "Invalid Cluster Service Ipv4 CIDR 10.100.0.0/28 for the cluster refused-8, must \
             have netmask size between /12 and /24 inclusive"
                .to_string(),
        ),
        (
            "a service CIDR that is not a CIDR block at all",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.100.0.0/33" } }),
            "Invalid Cluster Service Ipv4 CIDR 10.100.0.0/33 for the cluster refused-9, must be \
             a valid IPv4 CIDR block"
                .to_string(),
        ),
        (
            "a service CIDR on an ipv6 cluster",
            serde_json::json!({
                "kubernetesNetworkConfig": {
                    "ipFamily": "ipv6",
                    "serviceIpv4Cidr": "10.100.0.0/16",
                },
            }),
            "serviceIpv4Cidr cannot be specified when creating an \"ipv6\" cluster".to_string(),
        ),
        (
            "EKS Auto Mode compute enabled on its own",
            serde_json::json!({
                "computeConfig": { "enabled": true, "nodeRoleArn": NODE_ROLE },
            }),
            AUTO_MODE_ALL_OR_NOTHING.to_string(),
        ),
        (
            "EKS Auto Mode block storage enabled on its own",
            serde_json::json!({ "storageConfig": { "blockStorage": { "enabled": true } } }),
            AUTO_MODE_ALL_OR_NOTHING.to_string(),
        ),
        (
            "EKS Auto Mode load balancing enabled on its own",
            serde_json::json!({
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            AUTO_MODE_ALL_OR_NOTHING.to_string(),
        ),
        (
            "EKS Auto Mode compute and block storage enabled without load balancing",
            serde_json::json!({
                "computeConfig": { "enabled": true, "nodeRoleArn": NODE_ROLE },
                "storageConfig": { "blockStorage": { "enabled": true } },
            }),
            AUTO_MODE_ALL_OR_NOTHING.to_string(),
        ),
        (
            "EKS Auto Mode compute disabled while the other two are enabled",
            serde_json::json!({
                "computeConfig": { "enabled": false },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            AUTO_MODE_ALL_OR_NOTHING.to_string(),
        ),
        (
            "node pools with no node role for them to run as",
            serde_json::json!({
                "computeConfig": { "enabled": true, "nodePools": ["general-purpose"] },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            "computeConfig.nodeRoleArn must be specified when computeConfig.nodePools is \
             specified."
                .to_string(),
        ),
        (
            "a Kubernetes version Amazon EKS never published",
            serde_json::json!({ "version": "1.10" }),
            "unsupported Kubernetes version 1.10".to_string(),
        ),
        (
            "a Kubernetes version no release of Kubernetes carries",
            serde_json::json!({ "version": "9.99" }),
            "unsupported Kubernetes version 9.99".to_string(),
        ),
    ];

    for (index, (case, overrides, expected)) in rows.iter().enumerate() {
        let name = format!("refused-{index}");
        let body = request(&name, overrides.clone());
        assert_refused(&router, case, &name, &body, expected).await;
    }

    // Nothing of any refused row reached the store: the listing is still empty.
    let listed = call(&router, REGION, "GET", "/clusters", "").await;
    assert_eq!(
        listed.body["clusters"],
        serde_json::json!([]),
        "a table of refused creates must leave no cluster listed, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE, from the other side: the configurations Amazon EKS DOES serve are created
// and read back with exactly the values the request named.
// ---------------------------------------------------------------------------

/// The rows are the mirror of the refusals above: one endpoint reachable rather
/// than none, CIDR lists of both families, a service CIDR at each published
/// netmask bound and inside each private range, an `ipv6` cluster that names no
/// service CIDR, and all three EKS Auto Mode capabilities enabled together.
#[tokio::test]
async fn a_cluster_configuration_eks_serves_is_created_and_reads_back() {
    let router = emulator();

    // (case, overrides, the JSON-pointer/value pairs the cluster must report)
    let rows = [
        (
            "private access only, so one endpoint is reachable",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "endpointPublicAccess": false,
                    "endpointPrivateAccess": true,
                },
            }),
            vec![
                (
                    "/resourcesVpcConfig/endpointPublicAccess",
                    serde_json::json!(false),
                ),
                (
                    "/resourcesVpcConfig/endpointPrivateAccess",
                    serde_json::json!(true),
                ),
            ],
        ),
        (
            "public access cidrs of both families",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "publicAccessCidrs": ["10.0.0.0/8", "0.0.0.0/0", "2001:db8::/32", "::/0"],
                },
            }),
            vec![(
                "/resourcesVpcConfig/publicAccessCidrs",
                serde_json::json!(["10.0.0.0/8", "0.0.0.0/0", "2001:db8::/32", "::/0"]),
            )],
        ),
        (
            "a service CIDR at the widest published netmask, in the 10.0.0.0/8 range",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.0.0.0/12" } }),
            vec![(
                "/kubernetesNetworkConfig/serviceIpv4Cidr",
                serde_json::json!("10.0.0.0/12"),
            )],
        ),
        (
            "a service CIDR at the narrowest published netmask, in the 192.168.0.0/16 range",
            serde_json::json!({
                "kubernetesNetworkConfig": { "serviceIpv4Cidr": "192.168.1.0/24" },
            }),
            vec![(
                "/kubernetesNetworkConfig/serviceIpv4Cidr",
                serde_json::json!("192.168.1.0/24"),
            )],
        ),
        (
            "a service CIDR in the 172.16.0.0/12 range",
            serde_json::json!({ "kubernetesNetworkConfig": { "serviceIpv4Cidr": "172.20.0.0/16" } }),
            vec![(
                "/kubernetesNetworkConfig/serviceIpv4Cidr",
                serde_json::json!("172.20.0.0/16"),
            )],
        ),
        (
            "an ipv6 cluster, which names no service CIDR of its own",
            serde_json::json!({ "kubernetesNetworkConfig": { "ipFamily": "ipv6" } }),
            vec![
                (
                    "/kubernetesNetworkConfig/ipFamily",
                    serde_json::json!("ipv6"),
                ),
                (
                    "/kubernetesNetworkConfig/serviceIpv4Cidr",
                    serde_json::Value::Null,
                ),
            ],
        ),
        (
            "the three EKS Auto Mode capabilities enabled together",
            serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose", "system"],
                    "nodeRoleArn": NODE_ROLE,
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            vec![
                ("/computeConfig/enabled", serde_json::json!(true)),
                (
                    "/computeConfig/nodePools",
                    serde_json::json!(["general-purpose", "system"]),
                ),
                (
                    "/storageConfig/blockStorage/enabled",
                    serde_json::json!(true),
                ),
                (
                    "/kubernetesNetworkConfig/elasticLoadBalancing/enabled",
                    serde_json::json!(true),
                ),
            ],
        ),
        (
            "the three EKS Auto Mode capabilities disabled together",
            serde_json::json!({
                "computeConfig": { "enabled": false },
                "storageConfig": { "blockStorage": { "enabled": false } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": false } },
            }),
            vec![
                ("/computeConfig/enabled", serde_json::json!(false)),
                (
                    "/storageConfig/blockStorage/enabled",
                    serde_json::json!(false),
                ),
                (
                    "/kubernetesNetworkConfig/elasticLoadBalancing/enabled",
                    serde_json::json!(false),
                ),
            ],
        ),
    ];

    for (index, (case, overrides, expected)) in rows.iter().enumerate() {
        let name = format!("served-{index}");
        let body = request(&name, overrides.clone());
        let created = create_cluster(&router, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {created:?}"
        );
        // Read back through `DescribeCluster`, so what is asserted is the record the
        // create wrote rather than the answer it happened to compose.
        let described = describe_cluster(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the cluster must read back, got {described:?}"
        );
        for (pointer, value) in expected {
            let reported = described
                .body
                .pointer(&format!("/cluster{pointer}"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            assert_eq!(
                &reported, value,
                "case {case:?}: expected {pointer} to report {value:?}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster is created only at a Kubernetes version Amazon EKS OFFERS, and a
// version that is past standard support is created only under extended support.
// ---------------------------------------------------------------------------

/// The table is the published calendar itself — every row of it, offered or not,
/// under both `supportType` values — so it spans whatever the calendar carries at
/// the instant the suite runs rather than a snapshot of it.
#[tokio::test]
async fn a_cluster_is_created_only_at_an_offered_version_and_a_support_type_it_has() {
    let (router, engine) = emulator_with_engine();
    let now = SystemTime::now();
    // (version, whether it is released, its support status)
    let calendar: Vec<(String, bool, SupportStatus)> = {
        let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
        let guard = store.lock();
        guard
            .cluster_versions
            .iter()
            .map(|row| {
                (
                    row.cluster_version.clone(),
                    row.is_released(now),
                    row.support_status(now),
                )
            })
            .collect()
    };
    assert!(
        calendar
            .iter()
            .any(|(_, released, status)| *released && *status == SupportStatus::StandardSupport),
        "setup: the calendar must offer a version in standard support, got {calendar:?}"
    );

    for (index, (version, released, status)) in calendar.iter().enumerate() {
        for (support_index, support_type) in ["EXTENDED", "STANDARD"].into_iter().enumerate() {
            let case = format!("{version} ({status:?}, released {released}) as {support_type}");
            let name = format!("version-{index}-{support_index}");
            let body = request(
                &name,
                serde_json::json!({
                    "version": version,
                    "upgradePolicy": { "supportType": support_type },
                }),
            );
            // A version the calendar does not offer is refused whatever support type
            // the request accepts; one that is only in extended support is refused
            // only for a request that asked for standard support.
            let offered = *released && *status != SupportStatus::Unsupported;
            let expected = if !offered {
                Some(format!("unsupported Kubernetes version {version}"))
            } else if *status == SupportStatus::ExtendedSupport && support_type == "STANDARD" {
                Some(format!(
                    "The requested Kubernetes version \"{version}\" is only supported under \
                     extended support. Please change the supportType to extended support to \
                     create the cluster."
                ))
            } else {
                None
            };
            match expected {
                Some(expected) => {
                    assert_refused(&router, &case, &name, &body, &expected).await;
                }
                None => {
                    let created = create_cluster(&router, &body).await;
                    assert_eq!(
                        created.status, 200,
                        "case {case:?}: expected HTTP 200, got {created:?}"
                    );
                    let described = describe_cluster(&router, &name).await;
                    assert_eq!(
                        described.body["cluster"]["version"],
                        serde_json::json!(version),
                        "case {case:?}: the cluster must run the version it was created at, \
                         got {described:?}"
                    );
                    assert_eq!(
                        described.body["cluster"]["upgradePolicy"]["supportType"],
                        serde_json::json!(support_type),
                        "case {case:?}: the cluster must report the support type it was \
                         created with, got {described:?}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE, on the other call that sets the same members: `UpdateClusterConfig` is
// judged on the configuration the cluster ENDS UP with, so a change that would
// leave it in a configuration Amazon EKS cannot have is refused and starts no
// update.
// ---------------------------------------------------------------------------

async fn update_config(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

/// The rows differ along which member the change carries and whether the value it
/// merges onto the cluster's own leaves a configuration Amazon EKS has: the
/// endpoint switches (one row refused because the cluster's other switch is off,
/// one served because it is on), the allow-list, and the EKS Auto Mode
/// capabilities (one refused for enabling one alone, one served for enabling all
/// three together).
#[tokio::test]
async fn an_update_that_would_leave_a_configuration_eks_cannot_have_is_refused() {
    let router = emulator();

    // (case, the cluster's own create overrides, the change, the refusal or `None`)
    let rows = [
        (
            "switching off public access on a cluster with no private access",
            serde_json::json!({}),
            serde_json::json!({ "resourcesVpcConfig": { "endpointPublicAccess": false } }),
            Some("Private and public endpoint access cannot be false"),
        ),
        (
            "switching off public access on a cluster that has private access",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "endpointPrivateAccess": true,
                },
            }),
            serde_json::json!({ "resourcesVpcConfig": { "endpointPublicAccess": false } }),
            None,
        ),
        (
            "switching off both access members at once",
            serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": subnets(),
                    "endpointPrivateAccess": true,
                },
            }),
            serde_json::json!({
                "resourcesVpcConfig": {
                    "endpointPublicAccess": false,
                    "endpointPrivateAccess": false,
                },
            }),
            Some("Private and public endpoint access cannot be false"),
        ),
        (
            "an allow-list entry that is not a CIDR block",
            serde_json::json!({}),
            serde_json::json!({
                "resourcesVpcConfig": { "publicAccessCidrs": ["10.0.0.0/8", "10.0.0.0/33"] },
            }),
            Some("The following CIDRs are invalid in publicAccessCidrs: [10.0.0.0/33]"),
        ),
        (
            "enabling the EKS Auto Mode block storage capability on its own",
            serde_json::json!({}),
            serde_json::json!({ "storageConfig": { "blockStorage": { "enabled": true } } }),
            Some(AUTO_MODE_ALL_OR_NOTHING),
        ),
        (
            "enabling the three EKS Auto Mode capabilities together",
            serde_json::json!({}),
            serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose"],
                    "nodeRoleArn": NODE_ROLE,
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            None,
        ),
    ];

    for (index, (case, overrides, _, _)) in rows.iter().enumerate() {
        let name = format!("updated-{index}");
        let created = create_cluster(&router, &request(&name, overrides.clone())).await;
        assert_eq!(created.status, 200, "case {case:?}: setup: {created:?}");
    }
    // An update is only started on a cluster that is running.
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;

    for (index, (case, _, change, expected)) in rows.iter().enumerate() {
        let name = format!("updated-{index}");
        let answer = update_config(&router, &name, change).await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {answer:?}"
                );
                assert_eq!(
                    message_of(&answer),
                    *expected,
                    "case {case:?}: expected message {expected:?}, got {:?}",
                    message_of(&answer)
                );
                // A refused change starts no update, so the cluster is not updating.
                let described = describe_cluster(&router, &name).await;
                assert_eq!(
                    described.body["cluster"]["status"],
                    serde_json::json!("ACTIVE"),
                    "case {case:?}: a refused update must leave the cluster ACTIVE, got \
                     {described:?}"
                );
                let updates = call(
                    &router,
                    REGION,
                    "GET",
                    &format!("/clusters/{}/updates", encode_label(&name)),
                    "",
                )
                .await;
                assert_eq!(
                    updates.body["updateIds"],
                    serde_json::json!([]),
                    "case {case:?}: a refused update must not be recorded, got {updates:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {answer:?}"
                );
                assert!(
                    answer.body["update"]["id"].is_string(),
                    "case {case:?}: an accepted change starts one update, got {answer:?}"
                );
            }
        }
    }
}

async fn create_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(cluster),
            encode_label(principal)
        ),
        "",
    )
    .await
}

async fn update_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(cluster),
            encode_label(principal)
        ),
        &body.to_string(),
    )
    .await
}

/// The rows are every access-entry `type` the service accepts
/// (`crate::state::ACCESS_ENTRY_TYPES`, the set
/// `CreateAccessEntryRequest$type` documents), so the table spans the whole enum
/// rather than the one type the issue cited — and each type is driven through the
/// create AND the update, because the rule is about the entry's type rather than
/// about which call sets the username.
#[tokio::test]
async fn a_username_is_refused_on_every_access_entry_type_eks_specifies_it_for() {
    let router = emulator();
    let created = create_cluster(
        &router,
        &crate::cluster_lifecycle::access_entry_cluster_request("entry-usernames"),
    )
    .await;
    assert_eq!(created.status, 200, "setup: {created:?}");

    for (index, entry_type) in ACCESS_ENTRY_TYPES.iter().enumerate() {
        let principal = format!("arn:aws:iam::000000000000:role/username-case-{index}");
        let case = format!("type {entry_type}");
        let chosen = create_entry(
            &router,
            "entry-usernames",
            &serde_json::json!({
                "principalArn": principal,
                "type": entry_type,
                "username": "not-allowed",
            }),
        )
        .await;
        if *entry_type == ACCESS_ENTRY_TYPE_STANDARD {
            assert_eq!(
                chosen.status, 200,
                "case {case:?}: a STANDARD entry authenticates as the username the caller \
                 chose, got {chosen:?}"
            );
            assert_eq!(
                chosen.body["accessEntry"]["username"],
                serde_json::json!("not-allowed"),
                "case {case:?}: expected the username the request named, got {chosen:?}"
            );
            continue;
        }
        let expected = format!(
            "The specified username is invalid: setting username is not allowed when the type \
             is \"{entry_type}\"."
        );
        assert_eq!(
            chosen.status, 400,
            "case {case:?}: expected HTTP 400, got {chosen:?}"
        );
        assert_eq!(
            error_code(&chosen),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {chosen:?}"
        );
        assert_eq!(
            message_of(&chosen),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message_of(&chosen)
        );
        // And no access entry was left behind.
        let described = describe_entry(&router, "entry-usernames", &principal).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must leave no access entry, got {described:?}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {described:?}"
        );

        // The same entry without a username is created — Amazon EKS specifies the
        // username — and an UPDATE that names one is refused the same way, leaving
        // the username EKS specified in place.
        let accepted = create_entry(
            &router,
            "entry-usernames",
            &serde_json::json!({ "principalArn": principal, "type": entry_type }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same entry without a username must be created, got {accepted:?}"
        );
        let specified = accepted.body["accessEntry"]["username"].clone();
        let renamed = update_entry(
            &router,
            "entry-usernames",
            &principal,
            &serde_json::json!({ "username": "also-not-allowed" }),
        )
        .await;
        assert_eq!(
            renamed.status, 400,
            "case {case:?}: an update must not set a username either, got {renamed:?}"
        );
        assert_eq!(
            message_of(&renamed),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message_of(&renamed)
        );
        let after = describe_entry(&router, "entry-usernames", &principal).await;
        assert_eq!(
            after.body["accessEntry"]["username"], specified,
            "case {case:?}: a refused update must leave the username Amazon EKS specified \
             ({specified:?}), got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DeleteCluster` is refused while compute is attached to the cluster, and a
// delete that arrives while an accepted deletion is still running finds the cluster
// in use rather than absent.
// ---------------------------------------------------------------------------

async fn create_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/node-groups", encode_label(cluster)),
        &serde_json::json!({
            "nodegroupName": name,
            "nodeRole": NODE_ROLE,
            "subnets": subnets(),
        })
        .to_string(),
    )
    .await
}

async fn delete_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn create_fargate_profile(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/fargate-profiles", encode_label(cluster)),
        &serde_json::json!({
            "fargateProfileName": name,
            "podExecutionRoleArn": ROLE,
            "subnets": subnets(),
            // "Fargate Profile must have at least one selector": a profile selects
            // the pods it runs by namespace.
            "selectors": [{ "namespace": "default" }],
        })
        .to_string(),
    )
    .await
}

/// One row of the dependency table: what is attached to the cluster, and the
/// sentence the refusal that keeps it carries.
struct Dependents {
    case: &'static str,
    nodegroups: &'static [&'static str],
    profiles: &'static [&'static str],
    refusal: &'static str,
}

/// The rows differ along WHICH child is attached and HOW MANY of them: one node
/// group, several node groups, one Fargate profile, and both kinds at once — and
/// each row then removes them and deletes the cluster, so the refusal is shown to
/// be about the children rather than about the cluster.
#[tokio::test]
async fn a_cluster_is_deleted_only_once_nothing_is_attached_to_it() {
    let router = emulator();

    let rows = [
        Dependents {
            case: "one managed node group",
            nodegroups: &["ng-a"],
            profiles: &[],
            refusal: "Cluster has nodegroups attached",
        },
        Dependents {
            case: "three managed node groups",
            nodegroups: &["ng-a", "ng-b", "ng-c"],
            profiles: &[],
            refusal: "Cluster has nodegroups attached",
        },
        Dependents {
            case: "one Fargate profile",
            nodegroups: &[],
            profiles: &["fp-a"],
            refusal: "Cluster has Fargate Profiles in ACTIVE/CREATING state",
        },
        Dependents {
            case: "a node group and a Fargate profile",
            nodegroups: &["ng-a"],
            profiles: &["fp-a"],
            refusal: "Cluster has nodegroups attached",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (case, nodegroups, profiles, expected) =
            (row.case, row.nodegroups, row.profiles, row.refusal);
        let cluster = format!("dependents-{index}");
        let created = create_cluster(&router, &minimal_request(&cluster)).await;
        assert_eq!(created.status, 200, "case {case:?}: setup: {created:?}");
        for name in nodegroups {
            let answer = create_nodegroup(&router, &cluster, name).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: setup: the node group {name:?} must be created, got {answer:?}"
            );
        }
        for name in profiles {
            let answer = create_fargate_profile(&router, &cluster, name).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: setup: the profile {name:?} must be created, got {answer:?}"
            );
        }

        let refused = delete_cluster(&router, &cluster).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {refused:?}"
        );
        assert_eq!(
            message_of(&refused),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message_of(&refused)
        );
        // The refused delete tore nothing down: the cluster is still there, and so
        // is everything attached to it.
        let still = describe_cluster(&router, &cluster).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: a refused delete must keep the cluster, got {still:?}"
        );
        for name in nodegroups {
            let read = call(
                &router,
                REGION,
                "GET",
                &format!(
                    "/clusters/{}/node-groups/{}",
                    encode_label(&cluster),
                    encode_label(name)
                ),
                "",
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: a refused delete must keep the node group {name:?}, got {read:?}"
            );
        }

        // Once the children are gone the same call is accepted, which is what makes
        // the refusal above a dependency guard and not a cluster that cannot be
        // deleted.
        for name in nodegroups {
            let removed = delete_nodegroup(&router, &cluster, name).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: the node group {name:?} must delete, got {removed:?}"
            );
        }
        for name in profiles {
            let removed = call(
                &router,
                REGION,
                "DELETE",
                &format!(
                    "/clusters/{}/fargate-profiles/{}",
                    encode_label(&cluster),
                    encode_label(name)
                ),
                "",
            )
            .await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: the profile {name:?} must delete, got {removed:?}"
            );
        }
        let deleted = delete_cluster(&router, &cluster).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 once nothing is attached, got {deleted:?}"
        );
        assert_eq!(
            deleted.body["cluster"]["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: the accepted delete reports DELETING, got {deleted:?}"
        );
    }
}

/// The losing side of two `DeleteCluster` calls that OVERLAP is told the cluster is
/// IN USE — it is being deleted — while a call that arrives after the first one has
/// finished its work is told the cluster is not there at all.
///
/// The overlap cannot be produced in process: a handler that has nothing to await
/// runs to completion before the next request is polled, so the in-flight window is
/// staged on the store the handler reads — the same seam the suites that need a
/// precondition no implemented call writes use — and the sequential case is driven
/// end to end.
#[tokio::test]
async fn a_delete_that_overlaps_an_accepted_deletion_is_a_conflict() {
    let (router, engine) = emulator_with_engine();
    let cluster = "race-overlap";
    let created = create_cluster(&router, &minimal_request(cluster)).await;
    assert_eq!(created.status, 200, "setup: {created:?}");
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;

    let first = delete_cluster(&router, cluster).await;
    assert_eq!(
        first.status, 200,
        "the first delete must be accepted, got {first:?}"
    );
    assert_eq!(
        first.body["cluster"]["status"],
        serde_json::json!("DELETING"),
        "the accepted delete reports DELETING, got {first:?}"
    );

    // That call has finished its work, so the cluster is ABSENT: a delete that
    // arrives now is a not-found, which is what a caller re-sending the delete of a
    // cluster it has already deleted is told.
    let resent = delete_cluster(&router, cluster).await;
    assert_eq!(
        resent.status, 404,
        "a delete of a cluster whose deletion has finished is a not-found, got {resent:?}"
    );
    assert_eq!(
        error_code(&resent),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {resent:?}"
    );
    assert_eq!(
        message_of(&resent),
        cluster_not_found_message(cluster),
        "expected the missing-cluster sentence, got {:?}",
        message_of(&resent)
    );

    // And while a deletion IS in flight — the window a second, overlapping call
    // arrives in — the cluster is reported in use rather than absent.
    let key = (REGION.to_string(), cluster.to_string());
    {
        let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
        let mut guard = store.lock();
        let now = SystemTime::now();
        guard.start_cluster_deletion(key.clone(), now, now + CLUSTER_DELETION_DELAY);
    }
    let loser = delete_cluster(&router, cluster).await;
    assert_eq!(
        loser.status, 409,
        "a delete that overlaps an accepted deletion is a conflict, got {loser:?}"
    );
    assert_eq!(
        error_code(&loser),
        "ResourceInUseException",
        "expected ResourceInUseException, got {loser:?}"
    );
    assert_eq!(
        message_of(&loser),
        format!("Cluster is being deleted: {cluster}"),
        "expected the in-flight-deletion sentence, got {:?}",
        message_of(&loser)
    );

    // Once that deletion reports back, the cluster is absent again.
    {
        let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
        let mut guard = store.lock();
        guard.finish_cluster_deletion(&key);
    }
    let after = delete_cluster(&router, cluster).await;
    assert_eq!(
        after.status, 404,
        "expected HTTP 404 once the deletion reported back, got {after:?}"
    );
    assert_eq!(
        error_code(&after),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {after:?}"
    );
}
