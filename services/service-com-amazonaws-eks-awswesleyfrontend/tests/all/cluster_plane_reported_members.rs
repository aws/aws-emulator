// SPDX-License-Identifier: CC0-1.0

//! What the cluster plane REPORTS beyond the values a request carried: the service
//! CIDR Amazon EKS assigns, the address the cluster's OIDC issuer is published on,
//! the list members a reported structure carries when they are empty, the order a
//! log-type set is echoed in, and the status a tag write answers.
//!
//! Each suite asserts ONE rule over a TABLE of inputs rather than one recorded
//! request, and every assertion names its case and prints the actual value:
//!
//! * `serviceIpv4Cidr` on a cluster created without one is one of the two blocks
//!   Kubernetes assigns from, chosen so it does not overlap the address space of the
//!   VPC the cluster is created in. The choice itself is driven over the rows of
//!   `default_service_ipv4_cidr`, because the VPC's ranges are Amazon EC2's to report
//!   — the end-to-end half, with EC2 mounted beside this service, is
//!   `core/router/tests/all/eks_cluster_default_service_cidr.rs`.
//! * `identity.oidc.issuer` is a URL a relying party FETCHES, so it names the
//!   endpoint of THIS emulator the create was made through, under the `/id/<resource
//!   id>` path Amazon EKS publishes a cluster's issuer at — only a client that
//!   resolves AWS's own hostnames, on an emulator served on no port of its own, is
//!   told the documented regional form.
//! * a list member a STORED resource reports is reported as the list it is, EMPTY
//!   rather than absent: `computeConfig.nodePools` on a cluster that runs no node
//!   pools, and `update.errors` on an update nothing has failed. The call that
//!   CREATES a cluster answers with the configuration its request carried instead,
//!   so a create that named no node pools reports none.
//! * an `UpdateParam` echoing a submitted value carries the JSON TEXT of that value,
//!   with a `LogType` set in the order the model declares those types, whatever order
//!   the request listed them in.
//! * `TagResource` and `UntagResource` carry an empty output, so they answer 204 No
//!   Content with no body and no `content-length`.
//!
//! Nothing here waits out a span: the clock-owed `CREATING` -> `ACTIVE` move is
//! POLLED through the shared `wire::read_until`, with a generous deadline.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`; the cluster builders are `cluster_lifecycle`'s.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    default_service_ipv4_cidr, SERVICE_IPV4_CIDR_BLOCKS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, delete, describe, minimal_request, REGION};
use crate::nodegroup_lifecycle::{
    create_cluster, create_nodegroup_in, minimal_request as nodegroup_request,
};
use crate::wire::{call, emulator, encode_label, read_until, Answer, TAG_MUTATION_STATUS};

/// The block Amazon EKS assigns when the VPC's address space leaves it free: the
/// first of the two the model documents, as this service orders them.
fn preferred_block() -> &'static str {
    SERVICE_IPV4_CIDR_BLOCKS[0]
}

/// The other of the two: what a cluster gets when the preferred one would overlap the
/// VPC it is created in.
fn alternate_block() -> &'static str {
    SERVICE_IPV4_CIDR_BLOCKS[1]
}

// ---------------------------------------------------------------------------
// RULE: the service CIDR a cluster is assigned is one of the two blocks
// Kubernetes assigns service addresses from, and it is the first of them that
// overlaps none of the VPC's own CIDR blocks.
// ---------------------------------------------------------------------------

#[test]
fn the_assigned_service_cidr_avoids_the_vpc_address_space() {
    // The rows span what the rule generalises over: a VPC nothing reported ranges
    // for, ranges that overlap neither block, ranges that overlap the preferred one
    // (equal to it, containing it, contained in it), ranges that overlap only the
    // other one, several ranges of one VPC, and a range that is not a block at all.
    let rows: Vec<(&str, Vec<String>, &str)> = vec![
        ("a VPC nothing reported a range for", vec![], preferred_block()),
        (
            "a VPC that overlaps neither block",
            vec!["192.168.0.0/16".to_string()],
            preferred_block(),
        ),
        (
            "a VPC whose range IS the preferred block",
            vec![preferred_block().to_string()],
            alternate_block(),
        ),
        (
            "a VPC whose range CONTAINS the preferred block",
            vec!["172.16.0.0/12".to_string()],
            alternate_block(),
        ),
        (
            "a VPC whose range sits INSIDE the preferred block",
            vec!["172.20.5.0/24".to_string()],
            alternate_block(),
        ),
        (
            "a VPC that overlaps only the other block",
            vec!["10.100.0.0/24".to_string()],
            preferred_block(),
        ),
        (
            "a VPC with several ranges, one of them overlapping the preferred block",
            vec![
                "172.20.0.0/20".to_string(),
                "172.16.0.0/20".to_string(),
                "192.168.8.0/22".to_string(),
            ],
            alternate_block(),
        ),
        (
            "a VPC with several ranges, none of them overlapping either block",
            vec!["10.0.0.0/16".to_string(), "192.168.0.0/16".to_string()],
            preferred_block(),
        ),
        (
            "ranges that overlap both blocks, where there is no third to assign",
            vec!["172.16.0.0/12".to_string(), "10.0.0.0/8".to_string()],
            preferred_block(),
        ),
        (
            "a reported range that is not a CIDR block at all",
            vec!["not-a-block".to_string()],
            preferred_block(),
        ),
    ];

    for (case, vpc_cidr_blocks, expected) in rows {
        let assigned = default_service_ipv4_cidr(&vpc_cidr_blocks);
        assert_eq!(
            assigned, expected,
            "case {case:?}: expected {expected} for {vpc_cidr_blocks:?}, got {assigned:?}"
        );
        assert!(
            SERVICE_IPV4_CIDR_BLOCKS.contains(&assigned.as_str()),
            "case {case:?}: the assigned block must be one the model documents, got {assigned:?}"
        );
    }
}

/// The same member END TO END on this service alone, where no EC2 is mounted and so
/// no VPC range rules a block out: a cluster that named no block reports the
/// preferred one, a cluster that named one reports exactly that, and an `ipv6`
/// cluster reports none at all.
#[tokio::test]
async fn a_cluster_reports_the_service_cidr_it_was_assigned() {
    let router = emulator();

    // (case, the `kubernetesNetworkConfig` the create carries, the `serviceIpv4Cidr`
    // the cluster must report) — the rows differ along whether the request named a
    // block, which block it named inside the private ranges the member admits, and
    // the IP family the cluster runs.
    let rows: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "no kubernetesNetworkConfig at all",
            serde_json::Value::Null,
            serde_json::json!(preferred_block()),
        ),
        (
            "an ipv4 family and no block",
            serde_json::json!({ "ipFamily": "ipv4" }),
            serde_json::json!(preferred_block()),
        ),
        (
            "a block of its own in the 10/8 range",
            serde_json::json!({ "serviceIpv4Cidr": "10.7.0.0/16" }),
            serde_json::json!("10.7.0.0/16"),
        ),
        (
            "a block of its own in the 192.168/16 range, at the narrow netmask bound",
            serde_json::json!({ "serviceIpv4Cidr": "192.168.9.0/24" }),
            serde_json::json!("192.168.9.0/24"),
        ),
        (
            "the other documented block, named explicitly",
            serde_json::json!({ "serviceIpv4Cidr": alternate_block() }),
            serde_json::json!(alternate_block()),
        ),
        (
            "an ipv6 cluster, which is assigned no IPv4 service block",
            serde_json::json!({ "ipFamily": "ipv6" }),
            serde_json::Value::Null,
        ),
    ];

    for (index, (case, network, expected)) in rows.iter().enumerate() {
        let name = format!("service-cidr-{index}");
        let mut body = minimal_request(&name);
        if !network.is_null() {
            body["kubernetesNetworkConfig"] = network.clone();
        }
        let created = create(&router, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must be served, got {created:?} ({})",
            created.message()
        );
        let reported = &created.cluster()["kubernetesNetworkConfig"]["serviceIpv4Cidr"];
        assert_eq!(
            reported, expected,
            "case {case:?}: CreateCluster must report {expected:?}, got {reported:?}"
        );
        // ROUND TRIP: the block is settled at create and read back from the record.
        let described = describe(&router, &name).await;
        let read = &described.cluster()["kubernetesNetworkConfig"]["serviceIpv4Cidr"];
        assert_eq!(
            read, expected,
            "case {case:?}: DescribeCluster must report {expected:?}, got {read:?}"
        );
        let deleted = delete(&router, &name).await;
        let on_delete = &deleted.cluster()["kubernetesNetworkConfig"]["serviceIpv4Cidr"];
        assert_eq!(
            on_delete, expected,
            "case {case:?}: DeleteCluster must report {expected:?}, got {on_delete:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the OIDC issuer a cluster publishes is an address of THIS emulator — the
// endpoint the create was made through — under the path EKS publishes a cluster's
// issuer at.
// ---------------------------------------------------------------------------

/// One `CreateCluster` addressed at `host`, which is the endpoint the creating client
/// reached. The harness's own driver names no host, so this suite builds the request:
/// WHICH endpoint a request arrived on is exactly what the rule is about.
async fn create_at_host(
    router: &MultiServiceRouter,
    host: &str,
    body: &serde_json::Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/clusters"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/eks/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        status, 200,
        "setup: a create addressed at {host:?} must be served, got {status} {parsed}"
    );
    Answer {
        status,
        error_type: None,
        body: parsed,
    }
}

#[tokio::test]
async fn the_oidc_issuer_names_the_endpoint_the_create_reached() {
    let router = emulator();

    // The rows span how a client addresses this emulator: a loopback literal with a
    // port, a loopback NAME on another port, a bracketed IPv6 literal — each an
    // address that names no service, which is the endpoint the cluster then announces
    // — and, as the boundary, a client that resolves AWS's own hostnames and reached
    // this service's regional endpoint, where an emulator served on no port of its
    // own has no address to publish and the documented regional form stands.
    let rows: [(&str, bool); 4] = [
        ("127.0.0.1:45923", true),
        ("localhost:9297", true),
        ("[::1]:9297", true),
        ("eks.us-west-2.amazonaws.com", false),
    ];

    for (index, (host, reaches_only_the_emulator)) in rows.iter().enumerate() {
        let name = format!("issuer-host-{index}");
        create_at_host(&router, host, &minimal_request(&name)).await;

        // `identity` is reported once the control plane behind the cluster is
        // provisioned, so the condition is polled rather than waited out.
        let described = read_until(
            host,
            "the cluster to report the identity its control plane issues tokens under",
            || describe(&router, &name),
            |answer| answer.cluster()["identity"]["oidc"]["issuer"].is_string(),
        )
        .await;
        let issuer = described.cluster()["identity"]["oidc"]["issuer"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let expected_prefix = if *reaches_only_the_emulator {
            format!("https://{host}/id/")
        } else {
            format!("https://oidc.eks.{REGION}.amazonaws.com/id/")
        };
        assert!(
            issuer.starts_with(&expected_prefix),
            "case {host:?}: expected the issuer to be published under \
             {expected_prefix}…, got {issuer:?}"
        );
        // The PATH is the one AWS gives it: `/id/<32 hex characters>`.
        let resource_id = issuer.trim_start_matches(&expected_prefix);
        assert!(
            resource_id.len() == 32 && resource_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {host:?}: the issuer's last segment must be the cluster's 32-hex \
             resource id, got {resource_id:?}"
        );
        // For a client that resolves nothing but this emulator, the issuer is NOT an
        // address only AWS answers on — which is the whole point: a relying party
        // that fetches it has to reach this emulator.
        if *reaches_only_the_emulator {
            assert!(
                !issuer.contains("amazonaws.com"),
                "case {host:?}: the issuer must not name a host AWS serves, got {issuer:?}"
            );
        }

        // The cluster announces ONE issuer: the delete answers with the same value
        // the describe reported, whatever endpoint a later caller addresses.
        let deleted = delete(&router, &name).await;
        assert_eq!(
            deleted.cluster()["identity"]["oidc"]["issuer"],
            serde_json::json!(issuer),
            "case {host:?}: DeleteCluster must report the issuer the cluster was \
             created with, got {}",
            deleted.cluster()["identity"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a list member a STORED resource reports is reported as the list it is —
// EMPTY, not absent — while the call that creates the resource answers with the
// configuration its own request carried.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reported_compute_capability_carries_its_node_pool_list() {
    let router = emulator();
    let node_role = "arn:aws:iam::000000000000:role/eksNodeRole";

    // One row: the case, the members the create carries, what the CREATE's own answer
    // reports for `nodePools` (the configuration the request carried, so absent when
    // the request named none) and what every READ of the stored cluster reports (the
    // list the cluster runs).
    type Row = (
        &'static str,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    );

    // The rows differ along how the request configures the compute capability: the
    // capability switched off (which names no pools at all), switched off with an
    // explicitly empty list, and EKS Auto Mode on with one and with two pools — the
    // three capabilities are enabled together, so an Auto Mode row carries all three.
    let rows: Vec<Row> = vec![
        (
            "the compute capability explicitly off",
            serde_json::json!({ "computeConfig": { "enabled": false } }),
            serde_json::Value::Null,
            serde_json::json!([]),
        ),
        (
            "the capability off with an explicitly empty node pool list",
            serde_json::json!({ "computeConfig": { "enabled": false, "nodePools": [] } }),
            serde_json::json!([]),
            serde_json::json!([]),
        ),
        (
            "EKS Auto Mode with one node pool",
            serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose"],
                    "nodeRoleArn": node_role,
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            serde_json::json!(["general-purpose"]),
            serde_json::json!(["general-purpose"]),
        ),
        (
            "EKS Auto Mode with two node pools",
            serde_json::json!({
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose", "system"],
                    "nodeRoleArn": node_role,
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
            }),
            serde_json::json!(["general-purpose", "system"]),
            serde_json::json!(["general-purpose", "system"]),
        ),
    ];

    for (index, (case, extra, on_create, reported)) in rows.iter().enumerate() {
        let name = format!("node-pools-{index}");
        let mut body = minimal_request(&name);
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            body[key] = value;
        }
        let created = create(&router, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must be served, got {created:?} ({})",
            created.message()
        );
        // The CREATE answers with the configuration its request carried.
        assert_eq!(
            created.cluster()["computeConfig"]["nodePools"], *on_create,
            "case {case:?}: CreateCluster must report {on_create} for the configuration \
             its request carried, got {}",
            created.cluster()["computeConfig"]
        );
        // Every READ of the stored cluster reports the node pool list it runs.
        let described = describe(&router, &name).await;
        assert_eq!(
            described.cluster()["computeConfig"]["nodePools"], *reported,
            "case {case:?}: DescribeCluster must report {reported}, got {}",
            described.cluster()["computeConfig"]
        );
        let deleted = delete(&router, &name).await;
        assert_eq!(
            deleted.cluster()["computeConfig"]["nodePools"], *reported,
            "case {case:?}: DeleteCluster must report {reported}, got {}",
            deleted.cluster()["computeConfig"]
        );
    }
}

/// The same rule on the OTHER member it covers: every update reports `errors` as the
/// list it is, which on an update nothing has failed is the empty list — on the call
/// that started it and on the `DescribeUpdate` that reads it back.
#[tokio::test]
async fn every_update_reports_its_error_list() {
    let router = emulator();
    let cluster = "update-errors";
    let created = create(&router, &minimal_request(cluster)).await;
    assert_eq!(created.status, 200, "setup: got {created:?}");
    read_until(
        cluster,
        "the cluster to reach ACTIVE so a configuration change is taken",
        || describe(&router, cluster),
        |answer| answer.cluster()["status"] == serde_json::json!("ACTIVE"),
    )
    .await;

    // The rows differ along which category of configuration the update changes, so
    // the member is asserted on updates of different types rather than on one.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "a logging change",
            serde_json::json!({
                "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
            }),
        ),
        (
            "a deletion-protection change",
            serde_json::json!({ "deletionProtection": true }),
        ),
        (
            "an endpoint-access change",
            serde_json::json!({ "resourcesVpcConfig": { "endpointPrivateAccess": true } }),
        ),
    ];

    for (case, body) in rows {
        let answer = update_config(&router, cluster, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the update must be served, got {answer:?} ({})",
            answer.message()
        );
        assert_eq!(
            answer.body["update"]["errors"],
            serde_json::json!([]),
            "case {case:?}: UpdateClusterConfig must report an empty error list, got {}",
            answer.body["update"]
        );
        // And the record the update was written to reports the same list.
        let update_id = answer.body["update"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = describe_update(&router, cluster, &update_id).await;
        assert_eq!(
            read.body["update"]["errors"],
            serde_json::json!([]),
            "case {case:?}: DescribeUpdate must report an empty error list, got {}",
            read.body["update"]
        );
        // Each configuration change settles before the next one is asked for: a
        // cluster takes one update at a time.
        read_until(
            case,
            "the cluster to settle back to ACTIVE after the update",
            || describe(&router, cluster),
            |answer| answer.cluster()["status"] == serde_json::json!("ACTIVE"),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// RULE: an `UpdateParam` echoing a submitted `LogType` set reports those types in
// the order the model declares them.
// ---------------------------------------------------------------------------

/// `LogType`'s members, in the order the model declares them
/// (`smithy_query(query="shape",shape="LogType",
/// model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`).
const LOG_TYPES_IN_ENUM_ORDER: [&str; 5] = [
    "api",
    "audit",
    "authenticator",
    "controllerManager",
    "scheduler",
];

/// One row of the log-type table: the case, the `types` lists the request carries
/// (one per `clusterLogging` entry), and the lists the update's param must report.
type LogTypeRow = (&'static str, Vec<Vec<&'static str>>, Vec<Vec<&'static str>>);

#[tokio::test]
async fn a_cluster_logging_param_reports_log_types_in_enum_order() {
    let router = emulator();

    // The rows differ along the order and the number of types the request lists: a
    // pair listed backwards (the recorded case is one such row), three types out of
    // order, every type in reverse, one type on its own, and a request that names
    // two entries, so the normalization is per entry rather than per request.
    let rows: Vec<LogTypeRow> = vec![
        (
            "a pair listed backwards",
            vec![vec!["audit", "api"]],
            vec![vec!["api", "audit"]],
        ),
        (
            "three types out of order",
            vec![vec!["scheduler", "api", "controllerManager"]],
            vec![vec!["api", "controllerManager", "scheduler"]],
        ),
        (
            "every type in reverse",
            vec![LOG_TYPES_IN_ENUM_ORDER.iter().rev().copied().collect()],
            vec![LOG_TYPES_IN_ENUM_ORDER.to_vec()],
        ),
        (
            "one type on its own",
            vec![vec!["authenticator"]],
            vec![vec!["authenticator"]],
        ),
        (
            "two entries, each listed out of order",
            vec![
                vec!["audit", "api"],
                vec!["scheduler", "authenticator", "controllerManager"],
            ],
            vec![
                vec!["api", "audit"],
                vec!["authenticator", "controllerManager", "scheduler"],
            ],
        ),
    ];

    for (index, (case, requested, expected)) in rows.iter().enumerate() {
        let cluster = format!("logging-order-{index}");
        let created = create(&router, &minimal_request(&cluster)).await;
        assert_eq!(created.status, 200, "case {case:?}: setup got {created:?}");
        read_until(
            case,
            "the cluster to reach ACTIVE so a logging change is taken",
            || describe(&router, &cluster),
            |answer| answer.cluster()["status"] == serde_json::json!("ACTIVE"),
        )
        .await;

        let cluster_logging: Vec<serde_json::Value> = requested
            .iter()
            .enumerate()
            .map(|(position, types)| {
                serde_json::json!({ "types": types, "enabled": position == 0 })
            })
            .collect();
        let body = serde_json::json!({ "logging": { "clusterLogging": cluster_logging } });
        let answer = update_config(&router, &cluster, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the update must be served, got {answer:?} ({})",
            answer.message()
        );

        let expected_value: Vec<serde_json::Value> = expected
            .iter()
            .enumerate()
            .map(|(position, types)| {
                serde_json::json!({ "types": types, "enabled": position == 0 })
            })
            .collect();
        let expected_json = serde_json::json!({ "clusterLogging": expected_value });
        let reported = logging_param(&answer);
        assert_eq!(
            reported, expected_json,
            "case {case:?}: the ClusterLogging param must report the types in enum \
             order, expected {expected_json}, got {reported}"
        );

        // The param the record PERSISTED reports the same order.
        let update_id = answer.body["update"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = describe_update(&router, &cluster, &update_id).await;
        let read_back = logging_param(&read);
        assert_eq!(
            read_back, expected_json,
            "case {case:?}: DescribeUpdate must report the same order, expected \
             {expected_json}, got {read_back}"
        );
    }
}

/// The value one `UpdateParam` carries, parsed back out of the JSON TEXT
/// `UpdateParam$value` holds — `Null` when the update reports no param of that type.
fn update_param(answer: &Answer, param_type: &str) -> serde_json::Value {
    answer.body["update"]["params"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|param| param["type"] == serde_json::json!(param_type))
        .and_then(|param| param["value"].as_str().map(str::to_owned))
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// The `ClusterLogging` param's value — see [`update_param`].
fn logging_param(answer: &Answer) -> serde_json::Value {
    update_param(answer, "ClusterLogging")
}

// ---------------------------------------------------------------------------
// RULE (the same member, one level up): an `UpdateParam$value` is the JSON TEXT
// of the value the request submitted — on the node group changes that echo a
// label or a taint list as well as on the cluster logging change above.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_nodegroup_update_param_carries_the_json_text_of_the_submitted_value() {
    let router = emulator();
    let cluster = "ng-update-params";
    create_cluster(&router, cluster).await;

    // One row: the case, the change the request carries, and the `UpdateParamType`
    // keys it must report with the JSON value of each.
    type Row = (
        &'static str,
        serde_json::Value,
        Vec<(&'static str, serde_json::Value)>,
    );

    // The rows differ along which collection the change carries and how many entries
    // it names: one label key, several label keys, one taint, and a taint list on both
    // sides of a replacement.
    let rows: Vec<Row> = vec![
        (
            "one label key removed",
            serde_json::json!({ "labels": { "removeLabels": ["tier"] } }),
            vec![("LabelsToRemove", serde_json::json!(["tier"]))],
        ),
        (
            "several label keys removed",
            serde_json::json!({ "labels": { "removeLabels": ["tier", "team"] } }),
            vec![(
                "LabelsToRemove",
                serde_json::json!(["tier", "team"]),
            )],
        ),
        (
            "a label added and another removed",
            serde_json::json!({
                "labels": { "addOrUpdateLabels": { "tier": "spot" }, "removeLabels": ["team"] },
            }),
            vec![
                ("LabelsToAdd", serde_json::json!({ "tier": "spot" })),
                ("LabelsToRemove", serde_json::json!(["team"])),
            ],
        ),
        (
            "one taint added",
            serde_json::json!({
                "taints": {
                    "addOrUpdateTaints": [
                        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    ],
                },
            }),
            vec![(
                "TaintsToAdd",
                serde_json::json!([
                    { "effect": "NO_SCHEDULE", "value": "gpu", "key": "dedicated" },
                ]),
            )],
        ),
        (
            "two taints added and one removed",
            serde_json::json!({
                "taints": {
                    "addOrUpdateTaints": [
                        { "key": "a", "value": "1", "effect": "NO_EXECUTE" },
                        { "key": "b", "value": "2", "effect": "PREFER_NO_SCHEDULE" },
                    ],
                    "removeTaints": [
                        { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
                    ],
                },
            }),
            vec![
                (
                    "TaintsToAdd",
                    serde_json::json!([
                        { "effect": "NO_EXECUTE", "value": "1", "key": "a" },
                        { "effect": "PREFER_NO_SCHEDULE", "value": "2", "key": "b" },
                    ]),
                ),
                (
                    "TaintsToRemove",
                    serde_json::json!([
                        { "effect": "NO_SCHEDULE", "value": "gpu", "key": "dedicated" },
                    ]),
                ),
            ],
        ),
    ];

    for (index, (case, change, expected)) in rows.iter().enumerate() {
        // One node group per row, so each change is made against the same starting
        // labels and taints.
        let nodegroup = format!("ng-{index}");
        let mut body = nodegroup_request(&nodegroup);
        body["labels"] = serde_json::json!({ "tier": "spot", "team": "platform" });
        body["taints"] = serde_json::json!([
            { "key": "dedicated", "value": "gpu", "effect": "NO_SCHEDULE" },
        ]);
        let created = create_nodegroup_in(&router, REGION, cluster, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the node group must be created, got {created:?} ({})",
            created.message()
        );
        let nodegroup_uri = format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(&nodegroup)
        );
        read_until(
            case,
            "the node group to reach ACTIVE so a configuration change is taken",
            || call(&router, REGION, "GET", &nodegroup_uri, ""),
            |answer| answer.body["nodegroup"]["status"] == serde_json::json!("ACTIVE"),
        )
        .await;

        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("{nodegroup_uri}/update-config"),
            &change.to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the update must be served, got {answer:?} ({})",
            answer.message()
        );
        let update_id = answer.body["update"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = describe_nodegroup_update(&router, cluster, &nodegroup, &update_id).await;

        for (param_type, value) in expected {
            let reported = update_param(&answer, param_type);
            assert_eq!(
                reported, *value,
                "case {case:?}: the {param_type} param must carry the JSON text of the \
                 submitted value {value}, got {reported}"
            );
            // The param the record PERSISTED carries the same JSON.
            let read_back = update_param(&read, param_type);
            assert_eq!(
                read_back, *value,
                "case {case:?}: DescribeUpdate must report {value} for {param_type}, got \
                 {read_back}"
            );
        }
        // And the member the rule is about on every update: its error list.
        assert_eq!(
            answer.body["update"]["errors"],
            serde_json::json!([]),
            "case {case:?}: a node group update must report an empty error list, got {}",
            answer.body["update"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the two tag MUTATIONS carry an empty output, so they answer 204 No
// Content with no body — and the write they performed is what the tag read then
// reports.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tag_write_answers_no_content() {
    let router = emulator();
    let cluster = "tag-status";
    let created = create(&router, &minimal_request(cluster)).await;
    assert_eq!(created.status, 200, "setup: got {created:?}");
    let arn = created.cluster()["arn"]
        .as_str()
        .expect("a created cluster carries an arn")
        .to_string();
    let path = format!("/tags/{}", encode_label(&arn));

    // The rows differ along which mutation is made and how much it writes: one key,
    // several keys, a key that overwrites, and the two shapes of removal.
    let writes: Vec<(&str, &str, String, String)> = vec![
        (
            "TagResource with one key",
            "POST",
            path.clone(),
            serde_json::json!({ "tags": { "owner": "platform" } }).to_string(),
        ),
        (
            "TagResource with several keys",
            "POST",
            path.clone(),
            serde_json::json!({ "tags": { "env": "test", "team": "sre" } }).to_string(),
        ),
        (
            "TagResource overwriting a key it already carries",
            "POST",
            path.clone(),
            serde_json::json!({ "tags": { "owner": "sre" } }).to_string(),
        ),
        (
            "UntagResource removing one key",
            "DELETE",
            format!("{path}?tagKeys=team"),
            String::new(),
        ),
        (
            "UntagResource removing several keys",
            "DELETE",
            format!("{path}?tagKeys=env&tagKeys=owner"),
            String::new(),
        ),
    ];

    for (case, method, uri, body) in writes {
        // Driven here rather than through the harness's `call`, because what this
        // rule is about is the FRAMING — the status, the bytes and the header that
        // would describe them — and the harness hands back a parsed body.
        let request = http::Request::builder()
            .method(method)
            .uri(&uri)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/eks/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed test request");
        let response = router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            TAG_MUTATION_STATUS,
            "case {case:?}: a tag write must answer {TAG_MUTATION_STATUS}, got {} {}",
            response.status().as_u16(),
            String::from_utf8_lossy(response.body())
        );
        assert!(
            response.body().is_empty(),
            "case {case:?}: a no-content answer carries no body, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        // The framing HEADER is the transport's, not this service's (see
        // `crate::service::lenient_json`'s note on `serialize_output`): what this
        // service decides, and what this row asserts, is that the answer carries no
        // content and states none of its own beyond the length of nothing.
        assert_eq!(
            response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok()),
            Some("0"),
            "case {case:?}: a no-content answer states the length of nothing, got {:?}",
            response.headers().get("content-length")
        );
    }

    // The writes above really happened: every key one of them added was removed
    // again by one of the removals, so the read reports the empty map.
    let listed = call(&router, REGION, "GET", &path, "").await;
    assert_eq!(
        listed.status, 200,
        "a tag READ carries a body, so it answers 200, got {listed:?}"
    );
    assert_eq!(
        listed.body["tags"],
        serde_json::json!({}),
        "the tag writes must be the map the read reports, got {}",
        listed.body["tags"]
    );
}

// ---------------------------------------------------------------------------
// RULE: an epoch timestamp this API reports carries at most MILLISECOND
// resolution, whichever resolution the host clock ticks at.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reported_timestamps_carry_at_most_milliseconds() {
    let router = emulator();
    let cluster = "timestamp-precision";
    let created = create(&router, &minimal_request(cluster)).await;
    assert_eq!(created.status, 200, "setup: got {created:?}");
    assert_millisecond_resolution("CreateCluster cluster.createdAt", &created.cluster()["createdAt"]);

    let described = read_until(
        cluster,
        "the cluster to reach ACTIVE so a configuration change is taken",
        || describe(&router, cluster),
        |answer| answer.cluster()["status"] == serde_json::json!("ACTIVE"),
    )
    .await;
    assert_millisecond_resolution(
        "DescribeCluster cluster.createdAt",
        &described.cluster()["createdAt"],
    );

    let updated = update_config(
        &router,
        cluster,
        &serde_json::json!({ "deletionProtection": true }),
    )
    .await;
    assert_eq!(updated.status, 200, "setup update: got {updated:?}");
    assert_millisecond_resolution(
        "UpdateClusterConfig update.createdAt",
        &updated.body["update"]["createdAt"],
    );
    let update_id = updated.body["update"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let read = describe_update(&router, cluster, &update_id).await;
    assert_millisecond_resolution(
        "DescribeUpdate update.createdAt",
        &read.body["update"]["createdAt"],
    );
}

/// Assert that `reported` is epoch seconds carrying no finer resolution than a
/// millisecond — at most three fractional digits, which is the finest an AWS epoch
/// timestamp carries.
fn assert_millisecond_resolution(case: &str, reported: &serde_json::Value) {
    assert!(
        reported.is_number(),
        "case {case:?}: expected epoch seconds, got {reported}"
    );
    let rendered = reported.to_string();
    let fractional_digits = rendered
        .split_once('.')
        .map_or(0, |(_whole, fraction)| fraction.len());
    assert!(
        fractional_digits <= 3,
        "case {case:?}: expected at most millisecond resolution (3 fractional digits), \
         got {rendered}"
    );
}

/// `UpdateClusterConfig` for `cluster`.
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

/// `DescribeUpdate` for one update of `cluster`.
async fn describe_update(router: &MultiServiceRouter, cluster: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{}",
            encode_label(cluster),
            encode_label(update_id)
        ),
        "",
    )
    .await
}

/// `DescribeUpdate` for one update of a NODE GROUP of `cluster`, which selects the
/// updates of that node group rather than the cluster's own (`nodegroupName`).
pub(crate) async fn describe_nodegroup_update(
    router: &MultiServiceRouter,
    cluster: &str,
    nodegroup: &str,
    update_id: &str,
) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{}?nodegroupName={}",
            encode_label(cluster),
            encode_label(update_id),
            encode_label(nodegroup)
        ),
        "",
    )
    .await
}
