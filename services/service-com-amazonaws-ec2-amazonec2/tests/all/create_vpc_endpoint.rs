// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpcEndpoint`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a create stores the endpoint the request describes in the VPC it
//! names — the endpoint service it connects to, the subnets and security groups it was
//! given, the policy attached to it and the endpoint network interface created in each
//! subnet — mints it a `vpce-` id, answers it in the non-terminal `Pending` state with the
//! clock owing it `Available`, and `DescribeVpcEndpoints` reports exactly that record; a
//! request the service cannot satisfy is refused with the EC2 code for its cause and stores
//! nothing.
//!
//! Each table row is a distinct input along what that rule generalises over: which endpoint
//! type, which form of endpoint service name, how many subnets the endpoint is created in,
//! whether the policy and the security groups were chosen or defaulted, and which member of
//! the request is at fault.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, elements, error_code, filters, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that a move the clock owed a created
/// endpoint is certainly overdue: the deadline the poll below gives up at, generous because
/// what is under test is the state the endpoint reaches rather than how fast it reaches it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll below waits between two reads of an endpoint's state.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The emulator the tests drive. The store is reached only through the wire here, so the
/// engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The text of every `<item>` of the first set element named `set`.
///
/// Read through the XML parser rather than by substring, so the items of ONE set are never
/// confused with the identically-shaped items of another set of the same response.
fn set_items(case: &str, body: &str, set: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `member` of every `<item>` of the first set element named `set` — for a set whose
/// items are structures (`dnsEntrySet`, `groupSet`).
fn set_members(case: &str, body: &str, set: &str, name: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .filter_map(|item| super::wire::child(item, name))
                .map(|found| found.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block` in the Availability Zone `zone`, as its id.
///
/// The zone is named rather than left to the service: "we do not necessarily select a
/// different zone for each subnet" (`CreateSubnetRequest$AvailabilityZone`), and an
/// endpoint's one-interface-per-zone rule is about which zones its subnets are in.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", zone),
        ],
    )
    .await;
    member(case, &body, "subnetId")
}

/// The one VPC, with one subnet per Availability Zone letter in `zones`, every test below
/// builds its endpoints in.
struct Network {
    vpc_id: String,
    /// The subnets, in the order their zones were given.
    subnets: Vec<String>,
}

/// Create the VPC and its subnets through the API, so every endpoint below is created
/// against resources this emulator actually stores.
async fn network(case: &str, router: &MultiServiceRouter, zones: &[&str]) -> Network {
    let vpc_id = create_vpc(case, router, "10.40.0.0/16").await;
    // A VPC you create has DNS hostnames OFF (Amazon VPC User Guide, "DNS attributes in
    // your VPC"), and an interface endpoint's private hosted zone needs them ON — so the
    // network every endpoint below is created in turns them on, leaving the rows free to be
    // about the endpoint rather than about the VPC's DNS attributes
    // (`private_dns_follows_the_vpcs_dns_attributes` is the row that is about those).
    let _ = ok(
        case,
        router,
        "ModifyVpcAttribute",
        &[
            param("VpcId", &vpc_id),
            param("EnableDnsHostnames.Value", "true"),
        ],
    )
    .await;
    let mut subnets = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        let cidr_block = format!("10.40.{}.0/24", index + 1);
        subnets.push(create_subnet(case, router, &vpc_id, &cidr_block, zone).await);
    }
    Network { vpc_id, subnets }
}

/// The ids `DescribeVpcEndpoints` reports.
async fn described_endpoints(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(case, router, "DescribeVpcEndpoints", &[]).await;
    elements(&body, "vpcEndpointId")
}

/// The body `DescribeVpcEndpoints` answers for the one endpoint `endpoint_id`.
async fn describe_endpoint(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[endpoint_id]),
    )
    .await
}

// ---------------------------------------------------------------------------
// What a create stores
// ---------------------------------------------------------------------------

/// What one row of the create table asks for, and what the endpoint it creates must report.
struct CreateCase {
    case: &'static str,
    /// `VpcEndpointType`, or `None` to leave the member out — "Default: Gateway".
    endpoint_type: Option<&'static str>,
    /// The endpoint service name, in one of the forms the `ServiceName` doc gives.
    service_name: &'static str,
    /// How many of the network's subnets the endpoint is created in.
    subnets: usize,
    /// The type the created endpoint must report.
    reported_type: &'static str,
    /// The state the created endpoint must report: a gateway endpoint stands up no endpoint
    /// network interface, so it is in service at once, while an endpoint that does is
    /// `pending` until the clock has carried its interfaces into service.
    reported_state: &'static str,
}

/// The rows: the two spellings of a gateway endpoint (named and defaulted), an interface
/// endpoint in one and in two subnets, a service name of the partner form, and a Gateway
/// Load Balancer endpoint — which carries no policy and no security groups.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "a gateway endpoint to a service of this Region",
            endpoint_type: Some("Gateway"),
            service_name: "com.amazonaws.us-east-1.s3",
            subnets: 0,
            reported_type: "Gateway",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
        },
        CreateCase {
            case: "no type at all, which defaults to Gateway",
            endpoint_type: None,
            service_name: "com.amazonaws.us-east-1.dynamodb",
            subnets: 0,
            reported_type: "Gateway",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
        },
        CreateCase {
            case: "an interface endpoint in one subnet",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.logs",
            subnets: 1,
            reported_type: "Interface",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
        CreateCase {
            case: "an interface endpoint in two subnets",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.execute-api",
            subnets: 2,
            reported_type: "Interface",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
        CreateCase {
            case: "an interface endpoint to a service name of the partner form",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.vpce.us-east-1.vpce-svc-0a1b2c3d4e5f6a7b8",
            subnets: 1,
            reported_type: "Interface",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
        CreateCase {
            case: "a Gateway Load Balancer endpoint in its one subnet",
            endpoint_type: Some("GatewayLoadBalancer"),
            service_name: "com.amazonaws.vpce.us-east-1.vpce-svc-1234567890abcdef0",
            subnets: 1,
            reported_type: "GatewayLoadBalancer",
            reported_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
    ]
}

/// Every row is created, answered in the state its type is created in with a minted `vpce-`
/// id, and read back member for member through `DescribeVpcEndpoints` — so the answer is the
/// record the store holds rather than a value assembled for the reply.
#[tokio::test]
async fn each_endpoint_type_is_created_persisted_and_read_back() {
    let router = emulator();
    let built = network("the create table", &router, &["us-east-1a", "us-east-1b"]).await;

    let mut created: Vec<String> = Vec::new();
    for row in create_cases() {
        let case = row.case;
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", row.service_name),
        ];
        if let Some(endpoint_type) = row.endpoint_type {
            params.push(param("VpcEndpointType", endpoint_type));
        }
        let subnets: Vec<&str> = built
            .subnets
            .iter()
            .take(row.subnets)
            .map(String::as_str)
            .collect();
        params.extend(indexed("SubnetId", &subnets));

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        assert!(
            endpoint_id.starts_with("vpce-") && endpoint_id.len() == "vpce-".len() + 17,
            "case {case:?}: expected a minted vpce- id, got {endpoint_id:?}"
        );
        assert!(
            !created.contains(&endpoint_id),
            "case {case:?}: expected a fresh id, got {endpoint_id:?} again"
        );

        // The create ANSWERS the stored record, so each member of the answer must be what
        // the request asked for, and the describe must report the same.
        for (element_name, expected) in [
            ("vpcEndpointType", row.reported_type),
            ("vpcId", built.vpc_id.as_str()),
            ("serviceName", row.service_name),
            ("state", row.reported_state),
            ("serviceRegion", REGION),
            ("ownerId", engine::emulator_account_id()),
            ("requesterManaged", "false"),
        ] {
            assert_eq!(
                element(&body, element_name).as_deref(),
                Some(expected),
                "case {case:?}: expected <{element_name}> to carry {expected:?}, got {body}"
            );
        }
        let created_at = member(case, &body, "creationTimestamp");
        assert!(
            as_timestamp(&created_at).is_some(),
            "case {case:?}: expected an ISO 8601 creationTimestamp, got {created_at:?}"
        );
        assert_eq!(
            set_items(case, &body, "subnetIdSet"),
            subnets
                .iter()
                .map(|subnet| (*subnet).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the subnets the request named, got {body}"
        );
        // "An endpoint network interface is created … in an Availability Zone"
        // (`SubnetIds`): one per subnet, and none for a gateway endpoint, which has no
        // interfaces at all.
        assert_eq!(
            set_items(case, &body, "networkInterfaceIdSet").len(),
            subnets.len(),
            "case {case:?}: expected one endpoint network interface per subnet, got {body}"
        );
        // "(Interface endpoint) The DNS entries for the endpoint" (`DnsEntries`): the
        // regional name plus one per zone, the service's own default public name beside them
        // when the endpoint associates a private hosted zone for it — which an interface
        // endpoint to a service that publishes one does by default — and nothing at all for
        // the other types.
        let dns_names = set_members(case, &body, "dnsEntrySet", "dnsName");
        let publishes_private_name = row.service_name.starts_with("com.amazonaws.")
            && !row.service_name.starts_with("com.amazonaws.vpce.");
        let expected_names = if row.reported_type == "Interface" {
            subnets.len() + 1 + usize::from(publishes_private_name)
        } else {
            0
        };
        assert_eq!(
            dns_names.len(),
            expected_names,
            "case {case:?}: expected {expected_names} DNS entries, got {dns_names:?}"
        );
        // "(Interface and gateway endpoints) A policy to attach … If this parameter is not
        // specified, we attach a default policy that allows full access to the service"
        // (`PolicyDocument`) — and a Gateway Load Balancer endpoint is neither.
        let policy = element(&body, "policyDocument");
        if row.reported_type == "GatewayLoadBalancer" {
            assert_eq!(
                policy, None,
                "case {case:?}: a GatewayLoadBalancer endpoint carries no policy, got {body}"
            );
        } else {
            let policy = policy.unwrap_or_default();
            assert!(
                policy.contains("Allow"),
                "case {case:?}: expected the default full-access policy, got {policy:?}"
            );
        }

        let described = describe_endpoint(case, &router, &endpoint_id).await;
        for element_name in [
            "vpcEndpointType",
            "vpcId",
            "serviceName",
            "serviceRegion",
            "ownerId",
            "policyDocument",
        ] {
            assert_eq!(
                element(&described, element_name),
                element(&body, element_name),
                "case {case:?}: the describe must report what the create answered for \
                 <{element_name}>: {described}"
            );
        }
        assert_eq!(
            set_members(case, &described, "dnsEntrySet", "dnsName"),
            dns_names,
            "case {case:?}: the describe must report the endpoint's DNS entries: {described}"
        );
        created.push(endpoint_id);
    }

    // Every endpoint the table created is held, each exactly once.
    let reported: BTreeSet<String> = described_endpoints("every endpoint", &router)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        reported,
        created.iter().cloned().collect::<BTreeSet<String>>(),
        "expected every created endpoint to be reported, got {reported:?}"
    );

    // The move the clock owed each of them falls due on its own, with nothing called in
    // between to make it happen — so each endpoint is READ until it reports the state it was
    // owed rather than read once after a span of this test's choosing.
    for endpoint_id in &created {
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = describe_endpoint("after the transition", &router, endpoint_id).await;
            let state = element(&described, "state");
            if state.as_deref() == Some(ec2::state::VPC_ENDPOINT_STATE_AVAILABLE) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "expected the clock to owe {endpoint_id} its available state, last read \
                 {state:?} in {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// An interface endpoint's security groups: the ones the request named, or its VPC's own
/// default group when it named none — "If this parameter is not specified, we use the
/// default security group for the VPC" (`SecurityGroupIds`).
#[tokio::test]
async fn the_endpoint_carries_the_security_groups_asked_for_or_the_vpcs_default() {
    let router = emulator();
    let built = network("the group table", &router, &["us-east-1a"]).await;
    let chosen = {
        let body = ok(
            "a group of its own",
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &built.vpc_id),
                param("GroupName", "endpoint-access"),
                param("GroupDescription", "the group the endpoint suite created"),
            ],
        )
        .await;
        member("a group of its own", &body, "groupId")
    };
    let default_group = {
        let body = ok(
            "the VPC's default group",
            &router,
            "DescribeSecurityGroups",
            &filters(&[("vpc-id", &[built.vpc_id.as_str()]), ("group-name", &["default"])]),
        )
        .await;
        member("the VPC's default group", &body, "groupId")
    };

    // Rows: a named group, and no `SecurityGroupIds` member at all.
    for (case, named) in [
        ("a group the request named", Some(chosen.as_str())),
        ("no group named at all", None),
    ] {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", "com.amazonaws.us-east-1.logs"),
            param("VpcEndpointType", "Interface"),
        ];
        params.extend(indexed("SubnetId", &[built.subnets[0].as_str()]));
        if let Some(group_id) = named {
            params.extend(indexed("SecurityGroupId", &[group_id]));
        }
        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let expected = named.unwrap_or(default_group.as_str());
        assert_eq!(
            set_members(case, &body, "groupSet", "groupId"),
            vec![expected.to_string()],
            "case {case:?}: expected the endpoint to carry {expected:?}, got {body}"
        );
    }
}

/// A policy the request supplies is stored as given; one it does not is the documented
/// default; and a policy that is not JSON is refused.
#[tokio::test]
async fn the_policy_is_the_one_supplied_or_the_documented_default() {
    let router = emulator();
    let built = network("the policy table", &router, &["us-east-1a"]).await;
    let supplied = r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"*","Resource":"*"}]}"#;

    // Rows: a policy of the caller's own, no policy (the full-access default), and a value
    // that is not JSON at all.
    for (case, policy, expected) in [
        ("a policy of the caller's own", Some(supplied), Some(supplied)),
        ("no policy, so the default", None, None),
    ] {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", "com.amazonaws.us-east-1.s3"),
        ];
        if let Some(policy) = policy {
            params.push(param("PolicyDocument", policy));
        }
        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let stored = member(case, &body, "policyDocument");
        let stored = super::ec2_query::unescaped(&stored);
        match expected {
            Some(expected) => assert_eq!(
                stored, expected,
                "case {case:?}: expected the policy as supplied, got {stored:?}"
            ),
            None => assert!(
                ::serde_json::from_str::<::serde_json::Value>(&stored).is_ok()
                    && stored.contains("Allow"),
                "case {case:?}: expected a JSON full-access policy, got {stored:?}"
            ),
        }
    }
}

/// Private DNS is refused while the VPC's DNS attributes do not admit a private hosted zone
/// — "you must set the following VPC attributes to true: enableDnsHostnames and
/// enableDnsSupport" (`PrivateDnsEnabled`) — and once they do, the endpoint reports the
/// association and the service's default public name among its DNS entries.
#[tokio::test]
async fn private_dns_follows_the_vpcs_dns_attributes() {
    let router = emulator();
    let built = network("private DNS", &router, &["us-east-1a"]).await;
    // THIS row is about the VPC's DNS attributes, so the network's hostnames go back off:
    // every other row is created in a VPC that has them on.
    ok(
        "disable DNS hostnames",
        &router,
        "ModifyVpcAttribute",
        &[
            param("VpcId", &built.vpc_id),
            param("EnableDnsHostnames.Value", "false"),
        ],
    )
    .await;
    let params = || {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", "com.amazonaws.us-east-1.logs"),
            param("VpcEndpointType", "Interface"),
            param("PrivateDnsEnabled", "true"),
        ];
        params.extend(indexed("SubnetId", &[built.subnets[0].as_str()]));
        params
    };

    let (status, body) = call(&router, "CreateVpcEndpoint", &params()).await;
    assert_eq!(
        status, 400,
        "a VPC without DNS hostnames cannot hold a private hosted zone, got {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PARAMETER,
        "expected InvalidParameter for the VPC's DNS attributes, got {body}"
    );
    assert!(
        described_endpoints("after the refusal", &router)
            .await
            .is_empty(),
        "a refused create must store nothing"
    );

    ok(
        "enable DNS hostnames",
        &router,
        "ModifyVpcAttribute",
        &[
            param("VpcId", &built.vpc_id),
            param("EnableDnsHostnames.Value", "true"),
        ],
    )
    .await;
    let body = ok("with private DNS", &router, "CreateVpcEndpoint", &params()).await;
    assert_eq!(
        element(&body, "privateDnsEnabled").as_deref(),
        Some("true"),
        "expected the endpoint to report the private hosted zone, got {body}"
    );
    let dns_names = set_members("with private DNS", &body, "dnsEntrySet", "dnsName");
    assert!(
        dns_names.contains(&format!("logs.{REGION}.amazonaws.com")),
        "expected the service's default public name among {dns_names:?}"
    );
    // Two hosted zones, because the private name is served by a zone of its own.
    let zones: BTreeSet<String> = set_members("with private DNS", &body, "dnsEntrySet", "hostedZoneId")
        .into_iter()
        .collect();
    assert_eq!(
        zones.len(),
        2,
        "expected the private name in a hosted zone of its own, got {zones:?}"
    );
}

/// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
/// request" (`ClientToken`): the same request twice creates one endpoint, and the same
/// token with a different request is refused rather than answered with an endpoint the
/// caller did not ask for.
#[tokio::test]
async fn a_repeated_client_token_answers_the_endpoint_it_already_created() {
    let router = emulator();
    let built = network("idempotency", &router, &["us-east-1a"]).await;
    let request = |service_name: &str| {
        vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", service_name),
            param("ClientToken", "the-one-token"),
        ]
    };

    let first = ok(
        "the first call",
        &router,
        "CreateVpcEndpoint",
        &request("com.amazonaws.us-east-1.s3"),
    )
    .await;
    let endpoint_id = member("the first call", &first, "vpcEndpointId");
    assert_eq!(
        element(&first, "clientToken").as_deref(),
        Some("the-one-token"),
        "expected the create to echo the client token, got {first}"
    );

    let again = ok(
        "the same call again",
        &router,
        "CreateVpcEndpoint",
        &request("com.amazonaws.us-east-1.s3"),
    )
    .await;
    assert_eq!(
        member("the same call again", &again, "vpcEndpointId"),
        endpoint_id,
        "expected the repeat to answer the endpoint the first call created, got {again}"
    );
    assert_eq!(
        described_endpoints("after the repeat", &router).await,
        vec![endpoint_id.clone()],
        "expected the repeat to create nothing"
    );

    let (status, body) = call(
        &router,
        "CreateVpcEndpoint",
        &request("com.amazonaws.us-east-1.dynamodb"),
    )
    .await;
    assert_eq!(
        status, 400,
        "the token names another request: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::IDEMPOTENT_PARAMETER_MISMATCH,
        "expected IdempotentParameterMismatch, got {body}"
    );
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// Every refusal this operation makes, each with the EC2 code and HTTP status it answers,
/// and nothing stored for any of them.
///
/// The rows differ along what makes the request impossible to serve: the resource it names,
/// the form of the value it carries, the member it combines with the endpoint's type, and
/// the state of the VPC it is created in. The request shapes this slice holds no state for
/// are rows too — a 501 says "nobody has written this yet", which is a different claim from
/// a refusal.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_the_code_for_its_cause() {
    let router = emulator();
    let built = network(
        "the refusal table",
        &router,
        &["us-east-1a", "us-east-1a", "us-east-1b"],
    )
    .await;
    let other_vpc = create_vpc("another VPC", &router, "10.80.0.0/16").await;
    let other_subnet =
        create_subnet("another VPC", &router, &other_vpc, "10.80.1.0/24", "us-east-1a").await;
    /// A VPC id of the right shape that no VPC of the account has.
    const ABSENT_VPC: &str = "vpc-0000000000000beef";
    /// A subnet id of the right shape that no subnet of the account has.
    const ABSENT_SUBNET: &str = "subnet-0000000000000beef";
    /// The service name every row that is not about the name itself uses.
    const SERVICE: &str = "com.amazonaws.us-east-1.logs";
    // A policy one character past the published VPC endpoint policy quota, and valid JSON
    // otherwise — so its size is the only thing wrong with it.
    let oversize_policy = {
        let (opening, closing) = (r#"{"Statement":""#, r#""}"#);
        let padding = "x".repeat(
            ec2::state::MAX_VPC_ENDPOINT_POLICY_CHARS + 1 - opening.len() - closing.len(),
        );
        format!("{opening}{padding}{closing}")
    };

    let interface_in = |subnets: &[&str]| -> Vec<Param> {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", SERVICE),
            param("VpcEndpointType", "Interface"),
        ];
        params.extend(indexed("SubnetId", subnets));
        params
    };

    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a VPC no VPC of the account has",
            vec![param("VpcId", ABSENT_VPC), param("ServiceName", SERVICE)],
            400,
            ec2::state::INVALID_VPC_ID_NOT_FOUND,
        ),
        (
            "no service named at all",
            vec![param("VpcId", &built.vpc_id)],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a service name that is not one",
            vec![param("VpcId", &built.vpc_id), param("ServiceName", "logs")],
            400,
            ec2::state::INVALID_SERVICE_NAME,
        ),
        (
            "a service name with no Region label",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", "com.amazonaws.logs"),
            ],
            400,
            ec2::state::INVALID_SERVICE_NAME,
        ),
        (
            "a service name whose Region label is not a Region",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", "com.amazonaws.not-a-region.logs"),
            ],
            400,
            ec2::state::INVALID_SERVICE_NAME,
        ),
        (
            "a service name with no service label",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", "com.amazonaws.us-east-1."),
            ],
            400,
            ec2::state::INVALID_SERVICE_NAME,
        ),
        (
            "a subnet no subnet of the account has",
            interface_in(&[ABSENT_SUBNET]),
            400,
            ec2::state::INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a subnet of another VPC",
            interface_in(&[other_subnet.as_str()]),
            400,
            ec2::state::INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "two subnets of one Availability Zone",
            interface_in(&[built.subnets[0].as_str(), built.subnets[1].as_str()]),
            400,
            ec2::state::DUPLICATE_SUBNETS_IN_SAME_ZONE,
        ),
        (
            "subnets for a gateway endpoint",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("VpcEndpointType", "Gateway"),
                param("SubnetId.1", &built.subnets[0]),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "security groups for a gateway endpoint",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("VpcEndpointType", "Gateway"),
                param("SecurityGroupId.1", "sg-0000000000000beef"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "two subnets for a Gateway Load Balancer endpoint",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("VpcEndpointType", "GatewayLoadBalancer"),
                param("SubnetId.1", &built.subnets[0]),
                param("SubnetId.2", &built.subnets[2]),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a security group no group of the account has",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("SecurityGroupId.1", "sg-0000000000000beef"));
                params
            },
            400,
            ec2::state::INVALID_GROUP_NOT_FOUND,
        ),
        (
            "a value that is not a security group id",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("SecurityGroupId.1", "not-a-group"));
                params
            },
            400,
            ec2::state::INVALID_GROUP_ID_MALFORMED,
        ),
        (
            "a policy that is not JSON",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("PolicyDocument", "allow everything"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a policy past the published size quota",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("PolicyDocument", &oversize_policy),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "two configurations for one subnet",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("SubnetConfiguration.1.SubnetId", &built.subnets[0]));
                params.push(param("SubnetConfiguration.1.Ipv4", "10.40.1.10"));
                params.push(param("SubnetConfiguration.2.SubnetId", &built.subnets[0]));
                params.push(param("SubnetConfiguration.2.Ipv4", "10.40.1.11"));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a subnet configuration for a subnet the endpoint is not in",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("SubnetConfiguration.1.SubnetId", &built.subnets[2]));
                params.push(param("SubnetConfiguration.1.Ipv4", "10.40.3.10"));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "an address outside the subnet's assignable range",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("SubnetConfiguration.1.SubnetId", &built.subnets[0]));
                params.push(param("SubnetConfiguration.1.Ipv4", "10.40.9.10"));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a create that would have succeeded",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "an endpoint type that points at a resource configuration",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", SERVICE),
                param("VpcEndpointType", "Resource"),
            ],
            501,
            "NotImplemented",
        ),
        (
            "a route table the account does not hold",
            vec![
                param("VpcId", &built.vpc_id),
                // A route table is a GATEWAY endpoint's member, so the service named here is
                // one the Region offers a gateway endpoint to: a gateway endpoint to an
                // interface-only service is refused for its TYPE before any table is looked
                // up.
                param("ServiceName", "com.amazonaws.us-east-1.s3"),
                param("RouteTableId.1", "rtb-0000000000000beef"),
            ],
            400,
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "an endpoint reached over IPv6",
            {
                let mut params = interface_in(&[built.subnets[0].as_str()]);
                params.push(param("IpAddressType", "dualstack"));
                params
            },
            501,
            "NotImplemented",
        ),
    ];

    for (case, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, "CreateVpcEndpoint", &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code_expected,
            "case {case:?}: expected the code {code_expected}, got {body}"
        );
        assert!(
            described_endpoints(case, &router).await.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// `SubnetConfiguration$Ipv4` places the endpoint network interface on the address the
/// request chose, out of the subnet's own assignable range.
#[tokio::test]
async fn a_subnet_configuration_places_the_interface_on_the_address_it_names() {
    // The engine handle is kept here, unlike the suites above: the address an endpoint
    // network interface took is a member of the interface, and `DescribeNetworkInterfaces`
    // is still a scaffold, so the store is where it can be read at all.
    let (router, engine) = super::ec2_query::emulator();
    let built = network("subnet configurations", &router, &["us-east-1a"]).await;
    // Rows: two addresses of the subnet's assignable range, so the one reported is the one
    // asked for rather than whichever the service would have chosen.
    for (index, address) in ["10.40.1.42", "10.40.1.200"].into_iter().enumerate() {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", "com.amazonaws.us-east-1.logs"),
            param("VpcEndpointType", "Interface"),
            param("SubnetConfiguration.1.SubnetId", &built.subnets[0]),
            param("SubnetConfiguration.1.Ipv4", address),
        ];
        params.extend(indexed("SubnetId", &[built.subnets[0].as_str()]));
        let body = ok(address, &router, "CreateVpcEndpoint", &params).await;
        let interfaces = set_items(address, &body, "networkInterfaceIdSet");
        assert_eq!(
            interfaces.len(),
            1,
            "case {address:?}: expected one endpoint network interface, got {body}"
        );
        // The interface the create stored carries the address the request named — not one
        // the service chose for itself.
        let taken = engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .network_interfaces
            .get(&interfaces[0])
            .and_then(|interface| interface.private_ip_address.clone());
        assert_eq!(
            taken.as_deref(),
            Some(address),
            "case {address:?}: expected the interface on the address the request named, \
             got {taken:?}"
        );
        // And that address left the subnet: a /24 offers 251 assignable addresses (256 less
        // the five AWS reserves), one fewer per endpoint created in it.
        let described = ok(
            address,
            &router,
            "DescribeSubnets",
            &indexed("SubnetId", &[built.subnets[0].as_str()]),
        )
        .await;
        let available = member(address, &described, "availableIpAddressCount");
        let expected = 251 - (index + 1);
        assert_eq!(
            available,
            expected.to_string(),
            "case {address:?}: expected {expected} addresses left in the subnet, \
             got {available}"
        );
    }
}

/// `ServiceName` is RESOLVED against the one catalogue of endpoint services, so the create
/// and `DescribeVpcEndpointServices` agree about which services exist: a name that describe
/// offers can have an endpoint created to it, and a name it refuses is refused here too —
/// with the same `InvalidServiceName` code and the same sentence, naming the value the
/// request sent — rather than yielding an endpoint to a service nothing could reach.
///
/// The rows span the published name families
/// (`smithy_query(query="doc",shape="CreateVpcEndpointRequest",member="ServiceName")`, whose
/// examples give `com.amazonaws.<region>.<service>`): a gateway service of the Region's
/// catalogue, an interface service of it, a service whose label carries a dot, a member of
/// the `aws.api` family and one of the `api.aws` family — and, for each family, a label the
/// catalogue does not publish, plus a label published under ANOTHER family and an
/// upper-cased label.
#[tokio::test]
async fn a_service_name_is_resolved_against_the_one_endpoint_service_catalogue() {
    let router = emulator();
    let built = network("the service-name table", &router, &["us-east-1a"]).await;

    // `offered` is what the catalogue is expected to hold; every row is checked against the
    // describe as well, so a row that disagreed with the catalogue would fail there rather
    // than silently assert this test's own opinion. Each row also names the endpoint TYPE its
    // service is available through (`None` leaves the member out, which is the documented
    // `Gateway` default), since a type the service does not offer is a refusal of its own.
    let rows: [(&str, &str, Option<&str>, bool); 11] = [
        (
            "a gateway service",
            "com.amazonaws.us-east-1.s3",
            None,
            true,
        ),
        (
            "an interface service",
            "com.amazonaws.us-east-1.logs",
            Some("Interface"),
            true,
        ),
        (
            "a service label carrying a dot",
            "com.amazonaws.us-east-1.ecr.dkr",
            Some("Interface"),
            true,
        ),
        (
            "a member of the aws.api family",
            "aws.api.us-east-1.emr-service-cell01",
            Some("Interface"),
            true,
        ),
        (
            "a member of the api.aws family",
            "api.aws.us-east-1.cassandra-streams",
            Some("Interface"),
            true,
        ),
        (
            "a label the catalogue does not publish",
            "com.amazonaws.us-east-1.not-a-real-service",
            None,
            false,
        ),
        (
            "a near-miss of a published label",
            "com.amazonaws.us-east-1.s3-express",
            None,
            false,
        ),
        (
            "an aws.api label the catalogue does not publish",
            "aws.api.us-east-1.emr-service-cell02",
            None,
            false,
        ),
        (
            "an api.aws label published under com.amazonaws instead",
            "api.aws.us-east-1.cassandra",
            None,
            false,
        ),
        (
            "an upper-cased label",
            "com.amazonaws.us-east-1.LOGS",
            None,
            false,
        ),
        (
            "a partner-form name whose label is no service id",
            "com.amazonaws.vpce.us-east-1.not-a-service-id",
            None,
            false,
        ),
    ];

    for (case, service_name, endpoint_type, offered) in rows {
        // What the catalogue says, read through the describe that reports it.
        let (describe_status, describe_body) = call(
            &router,
            "DescribeVpcEndpointServices",
            &indexed("ServiceName", &[service_name]),
        )
        .await;
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", service_name),
        ];
        if let Some(endpoint_type) = endpoint_type {
            params.push(param("VpcEndpointType", endpoint_type));
            // An interface endpoint stands up an endpoint network interface, which takes a
            // subnet; the defaulted gateway type stands up none and refuses one.
            params.extend(indexed("SubnetId", &[built.subnets[0].as_str()]));
        }
        let (status, body) = call(&router, "CreateVpcEndpoint", &params).await;
        assert_eq!(
            describe_status == 200,
            offered,
            "case {case:?}: expected the describe to {} {service_name:?}, got \
             {describe_status}: {describe_body}",
            if offered { "offer" } else { "refuse" }
        );
        assert_eq!(
            status == 200,
            describe_status == 200,
            "case {case:?}: the create and the describe must agree about {service_name:?}, \
             got create {status} and describe {describe_status}: {body}"
        );
        if offered {
            continue;
        }
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_SERVICE_NAME,
            "case {case:?}: expected {code}, got {body}",
            code = ec2::state::INVALID_SERVICE_NAME
        );
        assert_eq!(
            super::ec2_query::error_message(&body),
            format!("The Vpc Endpoint Service '{service_name}' does not exist"),
            "case {case:?}: expected the refusal to name the value the request sent, got {body}"
        );
        assert_eq!(
            super::ec2_query::error_message(&body),
            super::ec2_query::error_message(&describe_body),
            "case {case:?}: the create and the describe must word one refusal one way, got \
             {body}"
        );
    }

    // Exactly the offered rows were created, each reported by `DescribeVpcEndpoints` against
    // the service name it was created to: a refused create stored nothing.
    let created = ok(
        "the service-name table",
        &router,
        "DescribeVpcEndpoints",
        &[],
    )
    .await;
    let names: BTreeSet<String> = elements(&created, "serviceName").into_iter().collect();
    let expected: BTreeSet<String> = rows
        .iter()
        .filter(|(_, _, _, offered)| *offered)
        .map(|(_, service_name, _, _)| (*service_name).to_string())
        .collect();
    assert_eq!(
        names, expected,
        "expected one endpoint per offered service name, got {created}"
    );
}
