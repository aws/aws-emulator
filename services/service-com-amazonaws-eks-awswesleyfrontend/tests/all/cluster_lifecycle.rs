// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS cluster lifecycle slice: `CreateCluster`,
//! `DescribeCluster`, `ListClusters`, `TagResource`, `ListTagsForResource` and
//! `DeleteCluster`, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which optional
//! members are supplied, name shape and length, page size, tag-merge shape, IP
//! family), and each assertion names its case and prints the actual value.
//!
//! The request harness ([`Answer`], [`emulator`], [`call`], [`encode_label`],
//! [`create`], [`describe`], [`minimal_request`]) is `pub(crate)` so the other
//! suites in this test binary drive the same router the same way instead of each
//! growing its own copy.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, MAX_CLUSTERS, MAX_STORED_CLUSTERS, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing (one shared engine behind the production router, label
// encoding, the reduced `Answer`) is shared with every other suite in this
// crate — see `tests/all/wire.rs`.
use crate::wire::{call, emulator, encode_label, Answer};

pub(crate) const REGION: &str = "us-west-2";

pub(crate) async fn create(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(router, REGION, "POST", "/clusters", &body.to_string()).await
}

pub(crate) async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

/// How long a suite waits for the clock-owed `CREATING` -> `ACTIVE` move the emulator
/// owes in [`CLUSTER_ACTIVATION_DELAY`]: far above that span, so the deadline is reached
/// only when the move is not coming at all rather than when the host is slow.
const ACTIVATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How often the condition is looked at again while waiting for it.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// `DescribeCluster` the cluster `name` until it reports `ACTIVE`, or the deadline passes
/// — the wait is for the CONDITION, so a loaded host only makes it slower.
pub(crate) async fn describe_when_active(router: &MultiServiceRouter, name: &str) -> Answer {
    let deadline = std::time::Instant::now() + ACTIVATION_DEADLINE;
    loop {
        let described = describe(router, name).await;
        if described.cluster()["status"] == serde_json::json!("ACTIVE") {
            return described;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cluster {name:?}: the clock owes a new cluster ACTIVE, got {described:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

pub(crate) async fn delete(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

pub(crate) async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/clusters".to_string()
    } else {
        format!("/clusters?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// The smallest request `CreateCluster` accepts: just the three required members.
pub(crate) fn minimal_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

/// A `CreateCluster` body for a cluster that SERVES the access-entry API: the
/// smallest request plus an `authenticationMode` that includes the API.
///
/// A cluster created through this API defaults to `CONFIG_MAP` ("if you create a
/// cluster by using the EKS API... the default is `CONFIG_MAP`",
/// `CreateAccessConfigRequest$authenticationMode`), and a cluster in that mode
/// authenticates only through the `aws-auth` ConfigMap, so its access-entry API is
/// refused. Every suite that exercises access entries therefore asks for a cluster
/// whose mode serves them, and does it through this one body rather than spelling
/// the mode out per suite.
pub(crate) fn access_entry_cluster_request(name: &str) -> serde_json::Value {
    let mut body = minimal_request(name);
    body["accessConfig"] = serde_json::json!({ "authenticationMode": "API_AND_CONFIG_MAP" });
    body
}

/// One Kubernetes version Amazon EKS offers a control plane for, read out of the
/// catalog a caller picks one from (`DescribeClusterVersions`) — the newest row it
/// publishes.
///
/// A suite that pins a `version` on a create reads it from here rather than writing
/// one, because a version the service does not publish is refused.
pub(crate) async fn an_offered_version(router: &MultiServiceRouter) -> String {
    let answer = call(router, REGION, "GET", "/cluster-versions?maxResults=1", "").await;
    assert_eq!(
        answer.status, 200,
        "setup: expected HTTP 200 from the version catalog, got {answer:?}"
    );
    answer.body["clusterVersions"]
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row["clusterVersion"].as_str())
        .unwrap_or_else(|| {
            panic!(
                "setup: the catalog must publish a Kubernetes version, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

pub(crate) fn names_of(answer: &Answer) -> Vec<String> {
    answer.body["clusters"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: a cluster read back is the record that was written — every member the
// request supplied, plus the server-generated ones, unchanged.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members are
/// supplied at all, and the values that switch a derived member (`ipFamily`
/// choosing between the two service CIDRs, `endpointPublicAccess` defaulting).
#[tokio::test]
async fn a_created_cluster_reads_back_exactly_as_stored() {
    let router = emulator();
    // A Kubernetes version Amazon EKS offers a control plane for, read out of the
    // catalog a caller picks one from rather than written here: `CreateCluster`
    // refuses a version the service does not publish.
    let offered_version = an_offered_version(&router).await;

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("only the required members", minimal_request("minimal")),
        (
            "every optional member this slice stores",
            serde_json::json!({
                "name": "fully-specified",
                "version": offered_version,
                "roleArn": "arn:aws:iam::000000000000:role/other",
                "resourcesVpcConfig": {
                    "subnetIds": ["subnet-11111111", "subnet-22222222", "subnet-33333333"],
                    "securityGroupIds": ["sg-11111111"],
                    "endpointPublicAccess": false,
                    "endpointPrivateAccess": true,
                    "publicAccessCidrs": ["10.0.0.0/8"],
                },
                "kubernetesNetworkConfig": { "serviceIpv4Cidr": "172.20.0.0/16", "ipFamily": "ipv4" },
                "logging": { "clusterLogging": [{ "types": ["api", "audit"], "enabled": true }] },
                "clientRequestToken": "token-fully-specified",
                "tags": { "env": "dev", "team": "platform" },
                "encryptionConfig": [{
                    "resources": ["secrets"],
                    "provider": { "keyArn": "arn:aws:kms:us-west-2:000000000000:key/abcd" },
                }],
                "accessConfig": { "authenticationMode": "API", "bootstrapClusterCreatorAdminPermissions": false },
                "deletionProtection": false,
            }),
        ),
        (
            "an ipv6 cluster, where the service CIDR is server-assigned",
            serde_json::json!({
                "name": "ipv6-cluster",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
                "kubernetesNetworkConfig": { "ipFamily": "ipv6" },
            }),
        ),
        (
            "a single tag, the low end of the modelled 1..50 tag map",
            serde_json::json!({
                "name": "one_tag",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
                "tags": { "only": "one" },
            }),
        ),
        (
            "deletion protection on",
            serde_json::json!({
                "name": "Protected9",
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
                "deletionProtection": true,
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["name"].as_str().expect("the case names its cluster");
        let created = create(&router, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );

        // The echo-back of everything the request supplied.
        assert_eq!(
            created.cluster()["name"],
            serde_json::json!(name),
            "case {case:?}: expected name {name:?}, got {:?}",
            created.cluster()["name"]
        );
        assert_eq!(
            created.cluster()["roleArn"],
            request["roleArn"],
            "case {case:?}: expected the stored roleArn, got {:?}",
            created.cluster()["roleArn"]
        );
        assert_eq!(
            created.cluster()["resourcesVpcConfig"]["subnetIds"],
            request["resourcesVpcConfig"]["subnetIds"],
            "case {case:?}: expected the stored subnetIds, got {:?}",
            created.cluster()["resourcesVpcConfig"]["subnetIds"]
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        assert_eq!(
            created.cluster()["tags"],
            expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            created.cluster()["tags"]
        );

        // The server-generated members.
        let expected_arn = format!("arn:aws:eks:{REGION}:000000000000:cluster/{name}");
        assert_eq!(
            created.cluster()["arn"],
            serde_json::json!(expected_arn),
            "case {case:?}: expected arn {expected_arn:?}, got {:?}",
            created.cluster()["arn"]
        );
        assert!(
            created.cluster()["createdAt"].is_number(),
            "case {case:?}: expected an epoch-seconds createdAt, got {:?}",
            created.cluster()["createdAt"]
        );
        assert_eq!(
            created.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a brand new cluster is CREATING, got {:?}",
            created.cluster()["status"]
        );
        // The members Amazon EKS fills in only once it has PROVISIONED the control
        // plane behind the cluster. A cluster still `CREATING` reports none of
        // them, which is what tells a client building a kubeconfig to keep
        // waiting; they are read back further down, once the clock owes the
        // cluster `ACTIVE`.
        for absent in ["endpoint", "identity"] {
            assert!(
                created.cluster()[absent].is_null(),
                "case {case:?}: a CREATING cluster reports no {absent}, got {:?}",
                created.cluster()[absent]
            );
        }
        let authority = &created.cluster()["certificateAuthority"];
        assert!(
            authority["data"].is_null() && authority["active"].is_null(),
            "case {case:?}: a CREATING cluster has no certificate authority data or \
             signer, got {authority:?}"
        );
        assert!(
            created.cluster()["resourcesVpcConfig"]["clusterSecurityGroupId"].is_null(),
            "case {case:?}: the cluster security group is created with the control \
             plane, so a CREATING cluster reports none, got {:?}",
            created.cluster()["resourcesVpcConfig"]
        );
        // The idempotency token of the CALL is not a member of the cluster that
        // call created, so no answer reports it — including this one, which was
        // given one in two of the rows.
        assert!(
            created.cluster()["clientRequestToken"].is_null(),
            "case {case:?}: a cluster reports no clientRequestToken, got {:?}",
            created.cluster()["clientRequestToken"]
        );

        // `version` is echoed when given and defaulted when not; either way it is
        // a Kubernetes minor version.
        let version = created.cluster()["version"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        match request.get("version").and_then(serde_json::Value::as_str) {
            Some(requested) => assert_eq!(
                version, requested,
                "case {case:?}: expected the requested version {requested:?}, got {version:?}"
            ),
            None => assert!(
                version.starts_with("1."),
                "case {case:?}: expected a defaulted 1.x version, got {version:?}"
            ),
        }

        // `ipFamily` decides WHICH service CIDR is reported.
        let ip_family = request["kubernetesNetworkConfig"]["ipFamily"]
            .as_str()
            .unwrap_or("ipv4");
        let network = &created.cluster()["kubernetesNetworkConfig"];
        assert_eq!(
            network["ipFamily"],
            serde_json::json!(ip_family),
            "case {case:?}: expected ipFamily {ip_family:?}, got {:?}",
            network["ipFamily"]
        );
        if ip_family == "ipv6" {
            assert!(
                network["serviceIpv6Cidr"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("fd"),
                "case {case:?}: an ipv6 cluster gets a server-assigned ULA service \
                 CIDR, got {network:?}"
            );
            assert!(
                network["serviceIpv4Cidr"].is_null(),
                "case {case:?}: an ipv6 cluster reports no IPv4 service CIDR, got {network:?}"
            );
        } else {
            assert!(
                network["serviceIpv4Cidr"].is_string(),
                "case {case:?}: an ipv4 cluster reports an IPv4 service CIDR, got {network:?}"
            );
            assert!(
                network["serviceIpv6Cidr"].is_null(),
                "case {case:?}: an ipv4 cluster reports no IPv6 service CIDR, got {network:?}"
            );
        }

        // The creation-time access flag belongs to this CALL rather than to the
        // cluster: the create answers with it and a later read of the cluster does
        // not, so it is asserted here and taken out of the comparison below.
        let requested_bootstrap = request["accessConfig"]
            ["bootstrapClusterCreatorAdminPermissions"]
            .as_bool()
            .unwrap_or(true);
        assert_eq!(
            created.cluster()["accessConfig"]["bootstrapClusterCreatorAdminPermissions"],
            serde_json::json!(requested_bootstrap),
            "case {case:?}: the create reports whether it bootstrapped the creator's \
             admin access, got {:?}",
            created.cluster()["accessConfig"]
        );

        // THE ROUND TRIP: the describe is the same object — status aside, and
        // aside from the members provisioning the control plane fills in, either
        // of which the clock may already have moved between the two calls.
        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeCluster, got {described:?}"
        );
        assert!(
            described.cluster()["accessConfig"]
                ["bootstrapClusterCreatorAdminPermissions"]
                .is_null(),
            "case {case:?}: a read of the cluster reports no creation-time access \
             flag, got {:?}",
            described.cluster()["accessConfig"]
        );
        let mut from_create = created.cluster().clone();
        let mut from_describe = described.cluster().clone();
        for value in [&mut from_create, &mut from_describe] {
            if let Some(object) = value.as_object_mut() {
                object.remove("status");
                object.remove("endpoint");
                object.remove("identity");
                object.remove("certificateAuthority");
                if let Some(vpc_config) = object
                    .get_mut("resourcesVpcConfig")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    vpc_config.remove("clusterSecurityGroupId");
                }
                if let Some(access_config) = object
                    .get_mut("accessConfig")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    access_config.remove("bootstrapClusterCreatorAdminPermissions");
                }
            }
        }
        assert_eq!(
            from_create, from_describe,
            "case {case:?}: DescribeCluster must return what CreateCluster stored"
        );
    }

    // Every case is listed, exactly once, in sorted order.
    let listed = names_of(&list(&router, "").await);
    let mut expected: Vec<String> = cases
        .iter()
        .map(|(_, request)| request["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "ListClusters must report every stored cluster once, sorted; got {listed:?}"
    );

    // Once the clock has provisioned the control plane, every member that
    // describes it is reported — for every row, whatever that row asked for. Each
    // row is read again until the move the clock owes it has been materialised.
    for (case, request) in &cases {
        let name = request["name"].as_str().expect("the case names its cluster");
        let described = describe_when_active(&router, name).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: the clock owes a new cluster ACTIVE, got {:?}",
            described.cluster()["status"]
        );
        let endpoint = described.cluster()["endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        // The endpoint is the address this emulator SERVES the cluster's
        // Kubernetes API on — a host and port of its own, not a name under
        // Amazon EKS's zone, which resolves nowhere on this machine. The
        // `kubernetes_api` suite drives the API behind it.
        let authority = endpoint
            .strip_prefix("https://")
            .unwrap_or_default()
            .to_owned();
        assert!(
            endpoint.starts_with("https://")
                && authority
                    .rsplit_once(':')
                    .is_some_and(|(_host, port)| port.parse::<u16>().is_ok()),
            "case {case:?}: an ACTIVE cluster reports the host and port its API \
             server is served at, got {endpoint:?}"
        );
        assert!(
            described.cluster()["certificateAuthority"]["data"]
                .as_str()
                .is_some_and(|data| !data.is_empty()),
            "case {case:?}: an ACTIVE cluster reports its certificate authority \
             data, got {:?}",
            described.cluster()["certificateAuthority"]
        );
        assert!(
            described.cluster()["identity"]["oidc"]["issuer"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("oidc.eks.{REGION}.amazonaws.com/id/")),
            "case {case:?}: an ACTIVE cluster reports its OIDC issuer, got {:?}",
            described.cluster()["identity"]
        );
    }
}

/// The ARN a cluster gets is regional: it carries the region the caller's own SigV4
/// credential scope names, not a constant.
#[tokio::test]
async fn the_cluster_arn_carries_the_region_the_request_is_signed_for() {
    // Source for the rows: the SigV4 credential scope region field
    // (`<akid>/<date>/<region>/<service>/aws4_request`), which is where every
    // regional value in a response comes from. `eu-central-1` is not a region the
    // issue cited.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = emulator();
        let answer = call(
            &router,
            region,
            "POST",
            "/clusters",
            &minimal_request("regional").to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected 200 from CreateCluster, got {answer:?}"
        );
        let expected = format!("arn:aws:eks:{region}:000000000000:cluster/regional");
        assert_eq!(
            answer.cluster()["arn"],
            serde_json::json!(expected),
            "case {region:?}: expected arn {expected:?}, got {:?}",
            answer.cluster()["arn"]
        );
        // The endpoint is reported once the clock has provisioned the control
        // plane behind it, and names the address this emulator serves that
        // cluster's Kubernetes API at, whichever region the cluster lives in:
        // the cluster is read back in the region it was created in, until the
        // move the clock owes it has been materialised.
        let deadline = std::time::Instant::now() + ACTIVATION_DEADLINE;
        let described = loop {
            let described = call(
                &router,
                region,
                "GET",
                &format!("/clusters/{}", encode_label("regional")),
                "",
            )
            .await;
            if described.cluster()["endpoint"].is_string() {
                break described;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {region:?}: the clock owes this cluster its published endpoint, got \
                 {described:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        let endpoint = described.cluster()["endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            endpoint
                .strip_prefix("https://")
                .and_then(|authority| authority.rsplit_once(':'))
                .is_some_and(|(_host, port)| port.parse::<u16>().is_ok()),
            "case {region:?}: expected the address the API server is served at, got \
             {endpoint:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a name the model's `ClusterName` constraints reject is refused 400
// `InvalidParameterException`; every name they accept is created.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_names_are_accepted_or_refused_by_the_modelled_constraints() {
    // Source for both tables: `ClusterName`'s own constraints, from
    // `smithy_query query="operation" operation="CreateCluster"` — len 1..100,
    // pattern `^[0-9A-Za-z][A-Za-z0-9\-_]*$`. The length rows cover each published
    // bound (1 and 100) and one past it (101).
    let accepted: Vec<(&str, String)> = vec![
        ("one character, the low length bound", "a".to_string()),
        ("starts with a digit", "9cluster".to_string()),
        ("mixed case", "MyCluster".to_string()),
        ("hyphen and underscore inside", "a-b_c-9".to_string()),
        ("100 characters, the high length bound", "z".repeat(100)),
    ];
    let refused: Vec<(&str, String)> = vec![
        ("empty, one below the low length bound", String::new()),
        ("101 characters, one past the high bound", "z".repeat(101)),
        ("starts with a hyphen", "-leading".to_string()),
        ("starts with an underscore", "_leading".to_string()),
        ("contains a dot", "has.dot".to_string()),
        ("contains a space", "has space".to_string()),
        ("contains a slash", "has/slash".to_string()),
    ];

    let router = emulator();
    for (case, name) in &accepted {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} (name len {}): expected 200, got {answer:?}",
            name.len()
        );
        assert_eq!(
            answer.cluster()["name"],
            serde_json::json!(name),
            "case {case:?}: expected the name echoed, got {:?}",
            answer.cluster()["name"]
        );
    }

    for (case, name) in &refused {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (name len {}): expected 400, got {answer:?}",
            name.len()
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        // Nothing was stored for a refused request. (Skipped for the empty name:
        // `GET /clusters/` is the ListClusters route, not a DescribeCluster for a
        // zero-length name, so it cannot answer this question.)
        if !name.is_empty() {
            let described = describe(&router, name).await;
            assert_ne!(
                described.status, 200,
                "case {case:?}: a refused CreateCluster must store nothing, got {described:?}"
            );
        }
    }

    // Across every row, the store holds exactly the accepted names — so no refused
    // request wrote anything, including the empty one the route above cannot probe.
    let mut expected: Vec<String> = accepted.iter().map(|(_, name)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        names_of(&list(&router, "maxResults=100").await),
        expected,
        "only the accepted names may be in the store"
    );
}

/// A name already in the store conflicts; an unknown name is not found.
#[tokio::test]
async fn names_conflict_on_create_and_are_not_found_on_describe() {
    let router = emulator();
    for case in ["alpha", "bravo9", "c_d-e"] {
        let first = create(&router, &minimal_request(case)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first CreateCluster succeeds, got {first:?}"
        );
        let again = create(&router, &minimal_request(case)).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: re-creating the same name conflicts, got {again:?}"
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {again:?}"
        );
    }

    for case in ["nope", "alpha2", "ALPHA"] {
        let answer = describe(&router, case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown name is not found, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `ListClusters` walks the stored names in sorted order; whatever page size
// it is driven at, following `nextToken` to exhaustion yields each name exactly
// once, and the last page carries no token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_cluster_exactly_once() {
    let router = emulator();
    let stored: Vec<String> = (0..7).map(|index| format!("cluster-{index}")).collect();
    for name in &stored {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(answer.status, 200, "setup create {name:?}: got {answer:?}");
    }

    // Rows differ along page size: each modelled bound (1 and 100), the sizes that
    // divide and do not divide the 7 stored clusters, and the exact count.
    for page_size in [1, 2, 3, 6, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {answer:?}"
            );
            let page = names_of(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page must not exceed the limit, got {page:?}"
            );
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every name once, in \
             sorted order; got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a name was repeated across pages: {seen:?}"
        );
    }

    // A token this service did not issue is a bad parameter, not an empty page. The
    // rows differ along HOW the token is foreign: not this encoding's alphabet, an
    // odd length, a well-formed encoding of bytes that are not a name, and — the
    // case a plain hex-of-the-name token would have honoured — the hex of a real
    // cluster name carrying no provenance tag.
    let untagged: String = "page-0001"
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for case in ["not-hex", "zz", "abc", "ffffffffff", untagged.as_str()] {
        let answer = list(&router, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a foreign nextToken, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }

    // An empty store lists nothing and offers no token.
    let empty = emulator();
    let answer = list(&empty, "").await;
    assert_eq!(answer.status, 200, "an empty store lists 200: {answer:?}");
    assert!(
        names_of(&answer).is_empty(),
        "an empty store lists no clusters, got {answer:?}"
    );
    assert!(
        answer.body["nextToken"].is_null(),
        "an empty store offers no nextToken, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `TagResource` MERGES into the resource's stored tag map and
// `ListTagsForResource` reports that map — create-time tags plus every merge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tagging_merges_into_the_stored_map_and_is_read_back() {
    type Tags = BTreeMap<&'static str, &'static str>;

    // Rows differ along the shape of the merge: no prior tags, disjoint keys, an
    // overwritten key, several merges in sequence, and the modelled 1..50 low bound.
    let cases: Vec<(&str, Tags, Vec<Tags>, Tags)> = vec![
        (
            "no create-time tags, one added",
            Tags::new(),
            vec![Tags::from([("team", "platform")])],
            Tags::from([("team", "platform")]),
        ),
        (
            "disjoint keys merge",
            Tags::from([("env", "dev")]),
            vec![Tags::from([("team", "platform")])],
            Tags::from([("env", "dev"), ("team", "platform")]),
        ),
        (
            "an existing key is overwritten, the others survive",
            Tags::from([("env", "dev"), ("keep", "me")]),
            vec![Tags::from([("env", "prod")])],
            Tags::from([("env", "prod"), ("keep", "me")]),
        ),
        (
            "several merges accumulate",
            Tags::from([("a", "1")]),
            vec![
                Tags::from([("b", "2")]),
                Tags::from([("c", "3")]),
                Tags::from([("a", "9")]),
            ],
            Tags::from([("a", "9"), ("b", "2"), ("c", "3")]),
        ),
    ];

    for (index, (case, create_tags, merges, expected)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("tagged-{index}");
        let mut request = minimal_request(&name);
        if !create_tags.is_empty() {
            request["tags"] = serde_json::to_value(create_tags).expect("tags serialize");
        }
        let created = create(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        let arn = created.cluster()["arn"]
            .as_str()
            .expect("a created cluster carries an arn")
            .to_owned();
        let tags_uri = format!("/tags/{}", encode_label(&arn));

        for merge in merges {
            let body = serde_json::json!({ "tags": merge });
            let answer = call(&router, REGION, "POST", &tags_uri, &body.to_string()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from TagResource for {merge:?}, got {answer:?}"
            );
        }

        let listed = call(&router, REGION, "GET", &tags_uri, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {listed:?}"
        );
        let expected_json = serde_json::to_value(expected).expect("tags serialize");
        assert_eq!(
            listed.body["tags"], expected_json,
            "case {case:?}: expected the merged map {expected_json:?}, got {:?}",
            listed.body["tags"]
        );

        // The same merged map is what the cluster itself reports.
        let described = describe(&router, &name).await;
        assert_eq!(
            described.cluster()["tags"], expected_json,
            "case {case:?}: DescribeCluster must report the merged map, got {:?}",
            described.cluster()["tags"]
        );
    }
}

/// A WELL-FORMED resource ARN of this service that no resource in the store owns is a
/// 404 on both tag operations — including the ARN of a cluster that has been deleted.
///
/// A value that is not a resource ARN of this service at all is a different answer (400
/// `BadRequestException`, refused on the input before any lookup); that half of the
/// rule, and the boundary between the two, is `tests/all/error_class_selection.rs`.
#[tokio::test]
async fn an_unowned_resource_arn_is_not_found_on_both_tag_operations() {
    let router = emulator();
    let created = create(&router, &minimal_request("gone-soon")).await;
    assert_eq!(created.status, 200, "setup create: got {created:?}");
    let deleted_arn = created.cluster()["arn"]
        .as_str()
        .expect("an arn")
        .to_owned();
    let removed = delete(&router, "gone-soon").await;
    assert_eq!(removed.status, 200, "setup delete: got {removed:?}");

    let cases = vec![
        ("a cluster ARN for a name never created", format!("arn:aws:eks:{REGION}:000000000000:cluster/never")),
        ("the ARN of a deleted cluster", deleted_arn),
        ("a nodegroup ARN, a resource kind this slice does not hold", format!("arn:aws:eks:{REGION}:000000000000:nodegroup/c/n/uuid")),
        ("a cluster ARN in another partition", "arn:aws-cn:eks:cn-north-1:000000000000:cluster/never".to_string()),
        ("a partition-global cluster ARN, whose region and account fields are empty", "arn:aws:eks:::cluster/never".to_string()),
    ];

    for (case, arn) in &cases {
        let uri = format!("/tags/{}", encode_label(arn));
        let tagged = call(
            &router,
            REGION,
            "POST",
            &uri,
            &serde_json::json!({ "tags": { "k": "v" } }).to_string(),
        )
        .await;
        assert_eq!(
            tagged.status, 404,
            "case {case:?}: TagResource on an unowned ARN is 404, got {tagged:?}"
        );
        assert_eq!(
            tagged.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {tagged:?}"
        );

        let listed = call(&router, REGION, "GET", &uri, "").await;
        assert_eq!(
            listed.status, 404,
            "case {case:?}: ListTagsForResource on an unowned ARN is 404, got {listed:?}"
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DeleteCluster` removes the cluster and reports it `DELETING`; after that
// no read finds it, and a second delete is a not-found. A cluster with
// `deletionProtection` is refused and kept.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_the_cluster_and_deletion_protection_refuses() {
    // Rows differ along `deletionProtection`: absent (the documented default
    // `false`), explicitly `false`, and `true`.
    let cases: Vec<(&str, Option<bool>, bool)> = vec![
        ("deletionProtection absent", None, true),
        ("deletionProtection false", Some(false), true),
        ("deletionProtection true", Some(true), false),
    ];

    for (index, (case, protection, deletable)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("deletable-{index}");
        let mut request = minimal_request(&name);
        if let Some(value) = protection {
            request["deletionProtection"] = serde_json::json!(value);
        }
        let created = create(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );

        let removed = delete(&router, &name).await;
        if !deletable {
            assert_eq!(
                removed.status, 400,
                "case {case:?}: a protected cluster is refused, got {removed:?}"
            );
            assert_eq!(
                removed.error_type.as_deref(),
                Some("InvalidRequestException"),
                "case {case:?}: expected InvalidRequestException, got {removed:?}"
            );
            // And it is still there.
            let still = describe(&router, &name).await;
            assert_eq!(
                still.status, 200,
                "case {case:?}: a refused delete must keep the cluster, got {still:?}"
            );
            assert_eq!(
                names_of(&list(&router, "").await),
                vec![name.clone()],
                "case {case:?}: a refused delete must keep the cluster listed"
            );
            continue;
        }

        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected 200 from DeleteCluster, got {removed:?}"
        );
        assert_eq!(
            removed.cluster()["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: the deleted cluster reports DELETING, got {:?}",
            removed.cluster()["status"]
        );
        assert_eq!(
            removed.cluster()["name"],
            serde_json::json!(name),
            "case {case:?}: the deleted cluster is the one asked for, got {:?}",
            removed.cluster()["name"]
        );

        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a deleted cluster is not found, got {described:?}"
        );
        // The accepted delete finished the work it carried, so the cluster is absent
        // rather than being deleted: a second delete is a not-found. (A call that
        // OVERLAPS one still in flight is the conflict `cluster_admission_rules`
        // pins.)
        let again = delete(&router, &name).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete is not found, got {again:?}"
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {again:?}"
        );
        assert!(
            names_of(&list(&router, "").await).is_empty(),
            "case {case:?}: a deleted cluster is no longer listed"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a status move the clock owes is materialised by whichever read first runs
// after it is due — and not before.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_cluster_reports_creating_until_the_clock_owes_it_active() {
    let router = emulator();
    for case in ["settling-a", "settling-b"] {
        let created = create(&router, &minimal_request(case)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        assert_eq!(
            created.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: CreateCluster reports CREATING, got {:?}",
            created.cluster()["status"]
        );

        // A caller acting straight after the create still observes the transient
        // state: the span is never zero.
        let immediately = describe(&router, case).await;
        assert_eq!(
            immediately.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a describe straight after the create still sees \
             CREATING, got {:?}",
            immediately.cluster()["status"]
        );
    }

    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + std::time::Duration::from_millis(250)).await;

    for case in ["settling-a", "settling-b"] {
        let described = describe(&router, case).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: once the activation delay has passed the cluster is \
             ACTIVE, got {:?}",
            described.cluster()["status"]
        );
        // Re-reading does not move it again or disturb anything else.
        let twice = describe(&router, case).await;
        assert_eq!(
            twice.cluster(),
            described.cluster(),
            "case {case:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the two cardinality rules `resourcesVpcConfig` documents — at least two
// subnets (waived for a local cluster on an Outpost, created in one subnet) and at
// most five security groups — are refused 400 `InvalidParameterException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_vpc_config_cardinality_rules_are_enforced() {
    // Rows cover each published bound and one past it, on both members, plus the
    // Outpost waiver. Source: `CreateCluster$resourcesVpcConfig`'s documentation —
    // "You must specify at least two subnets. You can specify up to five security
    // groups." The security group ids carry EC2's own identifier form, because a
    // value outside it is refused on its own account.
    let subnets = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("subnet-{index:017x}"))
            .collect()
    };
    let groups = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("sg-{index:017x}"))
            .collect()
    };

    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        ("no subnets at all", serde_json::json!({ "subnetIds": subnets(0) }), false),
        ("one subnet, one below the minimum", serde_json::json!({ "subnetIds": subnets(1) }), false),
        ("two subnets, the minimum", serde_json::json!({ "subnetIds": subnets(2) }), true),
        ("six subnets, well above the minimum", serde_json::json!({ "subnetIds": subnets(6) }), true),
        (
            "five security groups, the maximum",
            serde_json::json!({ "subnetIds": subnets(2), "securityGroupIds": groups(5) }),
            true,
        ),
        (
            "six security groups, one past the maximum",
            serde_json::json!({ "subnetIds": subnets(2), "securityGroupIds": groups(6) }),
            false,
        ),
    ];

    for (index, (case, vpc, accepted)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("vpc-{index}");
        let mut request = minimal_request(&name);
        request["resourcesVpcConfig"] = vpc.clone();
        let answer = create(&router, &request).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {answer:?}"
            );
            assert!(
                names_of(&list(&router, "").await).is_empty(),
                "case {case:?}: a refused create must store nothing"
            );
        }
    }

    // A local cluster on an Outpost is created in ONE subnet, so the minimum does
    // not apply to it.
    let router = emulator();
    let mut request = minimal_request("outpost-local");
    request["resourcesVpcConfig"] = serde_json::json!({ "subnetIds": subnets(1) });
    request["outpostConfig"] = serde_json::json!({
        "outpostArns": ["arn:aws:outposts:us-west-2:000000000000:outpost/op-1234"],
        "controlPlaneInstanceType": "m5.large",
    });
    let answer = create(&router, &request).await;
    assert_eq!(
        answer.status, 200,
        "case \"an Outpost local cluster in one subnet\": expected 200, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a service limit is reported as a modelled limit error, not by growing the
// store without bound.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_quota_is_reported_as_resource_limit_exceeded() {
    let router = emulator();
    for index in 0..MAX_CLUSTERS {
        let name = format!("quota-{index:04}");
        let answer = create(&router, &minimal_request(&name)).await;
        assert_eq!(
            answer.status, 200,
            "case {index} of {MAX_CLUSTERS} within the quota: expected 200, got {answer:?}"
        );
    }

    // Rows differ only in the name, which is the point: once the quota is reached
    // NO new name is accepted.
    for case in ["one-too-many", "another", "z-last"] {
        let answer = create(&router, &minimal_request(case)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: past the quota expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceLimitExceededException"),
            "case {case:?}: expected ResourceLimitExceededException, got {answer:?}"
        );
    }

    // Deleting one frees exactly one slot.
    let removed = delete(&router, "quota-0000").await;
    assert_eq!(removed.status, 200, "freeing a slot: got {removed:?}");
    let accepted = create(&router, &minimal_request("after-a-delete")).await;
    assert_eq!(
        accepted.status, 200,
        "case \"after a delete freed a slot\": expected 200, got {accepted:?}"
    );
    let full_again = create(&router, &minimal_request("still-too-many")).await;
    assert_eq!(
        full_again.status, 400,
        "case \"the freed slot is used up\": expected 400, got {full_again:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the store is bounded WHATEVER region a caller claims. The region a
// record is filed under comes from the request's own credential scope, so the
// per-Region quota alone is not a bound on the store; the store-wide ceiling is,
// and it is reported with the same modelled limit error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_store_wide_ceiling_holds_across_caller_chosen_regions() {
    let router = emulator();
    let regions = MAX_STORED_CLUSTERS / MAX_CLUSTERS;
    // Fill the ceiling by walking distinct, caller-chosen region strings, each one
    // up to its own per-Region quota.
    for region_index in 0..regions {
        let region = format!("us-emul-{region_index}");
        for index in 0..MAX_CLUSTERS {
            let name = format!("r{region_index}-{index:04}");
            let answer = call(
                &router,
                &region,
                "POST",
                "/clusters",
                &minimal_request(&name).to_string(),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {region}/{name}: within both bounds expected 200, got {answer:?}"
            );
        }
    }

    // A region that holds NOTHING is refused all the same: the rows differ in the
    // region claimed, which is exactly the multiplier the ceiling has to dominate.
    for case in ["us-emul-99", "eu-emul-1", "ap-emul-7"] {
        let answer = call(
            &router,
            case,
            "POST",
            "/clusters",
            &minimal_request("fresh-region").to_string(),
        )
        .await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceLimitExceededException"),
            "case {case:?}: an empty region past the store ceiling must still be \
             refused, got {answer:?}"
        );
        let listed = call(&router, case, "GET", "/clusters", "").await;
        assert!(
            names_of(&listed).is_empty(),
            "case {case:?}: the refused create must store nothing, got {listed:?}"
        );
    }
}

/// The 50-tag ceiling applies to the resource's ACCUMULATED map, so a merge that
/// would exceed it is refused and nothing is written.
#[tokio::test]
async fn the_tag_ceiling_applies_to_the_accumulated_map() {
    let router = emulator();
    let at_ceiling: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key{index:02}"), format!("value{index:02}")))
        .collect();
    let mut request = minimal_request("saturated");
    request["tags"] = serde_json::to_value(&at_ceiling).expect("tags serialize");
    let created = create(&router, &request).await;
    assert_eq!(created.status, 200, "setup create: got {created:?}");
    let arn = created.cluster()["arn"].as_str().expect("an arn").to_owned();
    let tags_uri = format!("/tags/{}", encode_label(&arn));

    // Rows differ along whether the merge would GROW the map: overwriting existing
    // keys never does, so it is accepted at the ceiling; any new key does.
    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "overwriting one existing key at the ceiling",
            serde_json::json!({ "key00": "rewritten" }),
            true,
        ),
        (
            "overwriting several existing keys at the ceiling",
            serde_json::json!({ "key01": "a", "key02": "b" }),
            true,
        ),
        (
            "one new key, one past the ceiling",
            serde_json::json!({ "brand-new": "x" }),
            false,
        ),
        (
            "a mix of an existing and a new key",
            serde_json::json!({ "key03": "kept", "also-new": "y" }),
            false,
        ),
    ];

    for (case, tags, accepted) in &cases {
        let body = serde_json::json!({ "tags": tags });
        let answer = call(&router, REGION, "POST", &tags_uri, &body.to_string()).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {answer:?}"
            );
        }
        // Either way the map never grows past the ceiling.
        let listed = call(&router, REGION, "GET", &tags_uri, "").await;
        let size = listed.body["tags"]
            .as_object()
            .map_or(0, serde_json::Map::len);
        assert_eq!(
            size, MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected {MAX_TAGS_PER_RESOURCE} tags, got {size}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster is identified by its NAME, so a second create of a name the
// Region already holds conflicts — 409 `ResourceInUseException` — whatever
// `clientRequestToken` it carries. The token is `@idempotencyToken`, which is
// what keeps a retried call from creating a SECOND cluster; it does not turn the
// answer into the first call's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repeat_create_of_one_name_conflicts_whatever_token_it_carries() {
    // Rows differ along the relationship between the two requests' tokens: equal,
    // different, present-then-absent, absent-then-present, absent on both, and
    // blank on both.
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("the same token twice", Some("tok-1"), Some("tok-1")),
        ("a different token", Some("tok-1"), Some("tok-2")),
        ("a token, then none", Some("tok-1"), None),
        ("no token, then one", None, Some("tok-1")),
        ("no token either time", None, None),
        ("an empty token twice", Some(""), Some("")),
        ("a blank token twice", Some("   "), Some("   ")),
    ];

    for (index, (case, first_token, second_token)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("idempotent-{index}");
        let mut first_request = minimal_request(&name);
        if let Some(token) = first_token {
            first_request["clientRequestToken"] = serde_json::json!(token);
        }
        let first = create(&router, &first_request).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create succeeds, got {first:?}"
        );

        let mut second_request = minimal_request(&name);
        if let Some(token) = second_token {
            second_request["clientRequestToken"] = serde_json::json!(token);
        }
        let second = create(&router, &second_request).await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected 409, got {second:?}"
        );
        assert_eq!(
            second.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {second:?}"
        );
        assert_eq!(
            second.body["clusterName"],
            serde_json::json!(name),
            "case {case:?}: the conflict names the cluster, got {:?}",
            second.body
        );
        assert_eq!(
            second.body["message"],
            serde_json::json!(format!("Cluster already exists with name: {name}")),
            "case {case:?}: the conflict states the name that is taken, got {:?}",
            second.body
        );
        assert!(
            second.body["cluster"].is_null(),
            "case {case:?}: a refused create answers with no cluster, got {:?}",
            second.body
        );
        // Exactly one cluster exists either way: the repeat created nothing.
        assert_eq!(
            names_of(&list(&router, "").await),
            vec![name.clone()],
            "case {case:?}: a repeat create must never store a second cluster"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster name identifies a cluster only within ONE region — "The name
// must be unique within the Amazon Web Services Region and Amazon Web Services
// account that you're creating the cluster in" (`CreateCluster$name`). So the
// same name created in two regions is two clusters, each read, listed, tagged
// and deleted independently of the other.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_name_in_two_regions_is_two_independent_clusters() {
    // Source for the rows: the SigV4 credential scope's region field, the only
    // place a request states its region. The pairs differ in partition geography
    // and in name shape, and none is a region/name the issue cited.
    let cases = [
        ("us-east-2", "eu-west-3", "shared"),
        ("ap-northeast-1", "sa-east-1", "Shared_Name-1"),
    ];
    for (left, right, name) in cases {
        let case = format!("{left} vs {right} / {name}");
        // ONE router, so both regions share the one store.
        let router = emulator();
        let body = minimal_request(name).to_string();

        for region in [left, right] {
            let created = call(&router, region, "POST", "/clusters", &body).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: creating {name:?} in {region} must succeed, got {created:?}"
            );
            let expected_arn = format!("arn:aws:eks:{region}:000000000000:cluster/{name}");
            assert_eq!(
                created.cluster()["arn"],
                serde_json::json!(expected_arn),
                "case {case:?}: expected arn {expected_arn:?}, got {:?}",
                created.cluster()["arn"]
            );
        }

        // A second create in the SAME region is still the conflict.
        let repeat = call(&router, left, "POST", "/clusters", &body).await;
        assert_eq!(
            repeat.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: a repeat create in {left} must conflict, got {repeat:?}"
        );

        // Each region describes and lists its own cluster, and only that one.
        for region in [left, right] {
            let described = call(
                &router,
                region,
                "GET",
                &format!("/clusters/{}", encode_label(name)),
                "",
            )
            .await;
            let expected_arn = format!("arn:aws:eks:{region}:000000000000:cluster/{name}");
            assert_eq!(
                described.cluster()["arn"],
                serde_json::json!(expected_arn),
                "case {case:?}: DescribeCluster in {region} must read that region's \
                 record, got {:?}",
                described.cluster()["arn"]
            );
            let listed = call(&router, region, "GET", "/clusters", "").await;
            assert_eq!(
                names_of(&listed),
                vec![name.to_owned()],
                "case {case:?}: ListClusters in {region} must list exactly its own \
                 cluster, got {listed:?}"
            );
        }

        // Deleting in one region leaves the other region's cluster untouched.
        let deleted = call(
            &router,
            left,
            "DELETE",
            &format!("/clusters/{}", encode_label(name)),
            "",
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting {name:?} in {left} must succeed, got {deleted:?}"
        );
        let gone = call(
            &router,
            left,
            "GET",
            &format!("/clusters/{}", encode_label(name)),
            "",
        )
        .await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: {name:?} must be gone in {left}, got {gone:?}"
        );
        let survivor = call(
            &router,
            right,
            "GET",
            &format!("/clusters/{}", encode_label(name)),
            "",
        )
        .await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: {name:?} must survive in {right}, got {survivor:?}"
        );

        // A tag call names the survivor by ARN, which carries its region, and the
        // deleted cluster's ARN is no longer known to the store.
        let survivor_arn = survivor.cluster()["arn"].as_str().unwrap_or_default().to_owned();
        let tagged = call(
            &router,
            left,
            "POST",
            &format!("/tags/{}", encode_label(&survivor_arn)),
            &serde_json::json!({ "tags": { "owner": "platform" } }).to_string(),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: tagging {survivor_arn:?} must succeed, got {tagged:?}"
        );
        let tags = call(
            &router,
            right,
            "GET",
            &format!("/tags/{}", encode_label(&survivor_arn)),
            "",
        )
        .await;
        assert_eq!(
            tags.body["tags"],
            serde_json::json!({ "owner": "platform" }),
            "case {case:?}: expected the merged map for {survivor_arn:?}, got {tags:?}"
        );
        let deleted_arn = format!("arn:aws:eks:{left}:000000000000:cluster/{name}");
        let orphan = call(
            &router,
            left,
            "GET",
            &format!("/tags/{}", encode_label(&deleted_arn)),
            "",
        )
        .await;
        assert_eq!(
            orphan.status, 404,
            "case {case:?}: {deleted_arn:?} must be unknown after the delete, got {orphan:?}"
        );
    }
}
