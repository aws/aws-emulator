// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Traffic Mirror target slice `CreateTrafficMirrorTarget` /
//! `DescribeTrafficMirrorTargets`, driven over the wire the service actually speaks —
//! `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol"),
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. "A Traffic Mirror target can be a network interface, a Network Load Balancer, or a
//!    Gateway Load Balancer endpoint" (`CreateTrafficMirrorTarget`): the create MINTS A
//!    TARGET pointing at the ONE resource the request named, reports it with the `Type` that
//!    resource implies and the account that owns it, and `DescribeTrafficMirrorTargets`
//!    reports the same record afterwards — so the answer is the store rather than the
//!    request object.
//! 2. A target points at exactly one resource: a request naming more than one of the three
//!    members is refused `InvalidParameterCombination`, one naming none `InvalidParameter`,
//!    and neither stores anything.
//! 3. A resource the account does not hold is refused with that resource's own code, and a
//!    value that is not an id (or not a load balancer ARN) at all with the code for a value
//!    of the wrong shape — never as a target that mirrors nowhere.
//! 4. One `ClientToken` names one target: a repeat of the same create reports the target the
//!    first one minted and stores nothing, while a create carrying no token is never a
//!    replay. `DryRun` answers `DryRunOperation` and stores nothing, and a dry run that
//!    would have been refused is refused with that reason instead.
//! 5. The describe NARROWS by `TrafficMirrorTargetIds` and by every filter name it answers
//!    (values ORed within one entry, entries ANDed with each other), refuses a filter name
//!    it does not answer and an id that names no target, and PAGES: `MaxResults` bounds a
//!    page, `NextToken` resumes it, and walking the pages visits every target once.
//!
//! Each table row is a distinct input along what the rule generalises over — which kind of
//! target, which members the request carries, which combination is refused, which shape of
//! unresolvable destination, which filter selects, which page size — so no row is the
//! specification. `DeleteTrafficMirrorTarget`'s own refusals live in
//! [`super::traffic_mirror_targets`].

use ec2::ops::types::TrafficMirrorTarget;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_NETWORK_INTERFACE_ID_MALFORMED,
    INVALID_NETWORK_INTERFACE_ID_NOT_FOUND, INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND,
    INVALID_VPC_ENDPOINT_ID_MALFORMED, INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
    MAX_TRAFFIC_MIRROR_TARGETS_PAGE_SIZE, TRAFFIC_MIRROR_TARGET_ID_PREFIX,
    TRAFFIC_MIRROR_TARGET_TAGGABLE_RESOURCE,
    TRAFFIC_MIRROR_TARGET_TYPE_GATEWAY_LOAD_BALANCER_ENDPOINT,
    TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_INTERFACE,
    TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_LOAD_BALANCER,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, error_code, error_message, filters};
use super::wire::{child_text, document, is_long_id, set_items};

/// The Region every call here is signed for. A Traffic Mirror target is named by an id
/// unique across this store and the modelled shape names no Region, so the Region only
/// selects the envelope.
const REGION: &str = "us-west-2";

/// The error code the BOUNDARY validator answers for a value outside a modelled constraint —
/// a `MaxResults` outside `TrafficMirroringMaxResults`'s `range 5..1000` is refused before
/// any handler runs, so it carries the boundary's code rather than one this crate chose.
const BOUNDARY_CONSTRAINT_CODE: &str = "ValidationError";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A Traffic Mirror target as a response reported it: every member of the modelled
/// `TrafficMirrorTarget` shape (`smithy_query(query="shape",shape="TrafficMirrorTarget")`),
/// so a create and a describe of one target are compared member for member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Reported {
    id: Option<String>,
    network_interface_id: Option<String>,
    network_load_balancer_arn: Option<String>,
    gateway_load_balancer_endpoint_id: Option<String>,
    kind: Option<String>,
    description: Option<String>,
    owner_id: Option<String>,
    tags: Vec<(String, String)>,
}

/// Every Traffic Mirror target a response body reports, in document order: the element that
/// carries a `trafficMirrorTargetId` child, whether it is the create's own
/// `<trafficMirrorTarget>` or one `<item>` of the describe's `<trafficMirrorTargetSet>`.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element() && child_text(*node, "trafficMirrorTargetId").is_some()
        })
        .map(|node| Reported {
            id: child_text(node, "trafficMirrorTargetId"),
            network_interface_id: child_text(node, "networkInterfaceId"),
            network_load_balancer_arn: child_text(node, "networkLoadBalancerArn"),
            gateway_load_balancer_endpoint_id: child_text(
                node,
                "gatewayLoadBalancerEndpointId",
            ),
            kind: child_text(node, "type"),
            description: child_text(node, "description"),
            owner_id: child_text(node, "ownerId"),
            tags: set_items(node, "tagSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The one target a body reports, or a panic naming the case.
fn one(case: &str, body: &str) -> Reported {
    let mut found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one Traffic Mirror target in {body}"
    );
    found.remove(0)
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateTrafficMirrorTarget", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeTrafficMirrorTargets", params).await
}

/// The ids the store holds, in its own order — the fact a create or a refusal is checked
/// against, read through the engine rather than through the operation under test.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .traffic_mirror_targets
        .keys()
        .cloned()
        .collect()
}

/// The resources of this account a Traffic Mirror target can point at, created through the
/// operations that mint them, so no destination under test is a value this suite invented.
struct Destinations {
    /// A network interface of the account, from `CreateNetworkInterface`.
    network_interface_id: String,
    /// A Gateway Load Balancer endpoint of the account, from `CreateVpcEndpoint`.
    gateway_load_balancer_endpoint_id: String,
}

/// The endpoint service a Gateway Load Balancer endpoint of this Region is created to: the
/// `com.amazonaws.vpce.<region>.vpce-svc-<hex>` form `CreateVpcEndpoint` reads a service name
/// in.
const GATEWAY_SERVICE_NAME: &str = "com.amazonaws.vpce.us-west-2.vpce-svc-0123456789abcdef0";

/// A Network Load Balancer ARN. A load balancer is a resource of ANOTHER service, which this
/// store holds none of, so the create holds the value to the FORM of a load balancer ARN and
/// no further — the reading `crate::state::load_balancer_arn_is_well_formed` records.
fn network_load_balancer_arn(n: usize) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{account}:loadbalancer/net/mirror-{n}/{n:017x}",
        account = engine::emulator_account_id(),
    )
}

/// Create the resources a target can point at, through the operations that mint them.
async fn destinations(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
) -> Destinations {
    let (vpc_id, subnet_id) = {
        let store = engine.state::<Ec2State>();
        let store = store.lock();
        let vpc_id = store
            .vpcs
            .values()
            .find(|vpc| vpc.is_default == Some(true))
            .and_then(|vpc| vpc.vpc_id.clone())
            .unwrap_or_else(|| panic!("case {case:?}: a fresh account has its default VPC"));
        let subnet_id = store
            .subnets
            .values()
            .find(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
            .and_then(|subnet| subnet.subnet_id.clone())
            .unwrap_or_else(|| {
                panic!("case {case:?}: a fresh account has the default VPC's subnets")
            });
        (vpc_id, subnet_id)
    };

    let (status, body) = call(
        router,
        REGION,
        "CreateNetworkInterface",
        &[p("SubnetId", subnet_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateNetworkInterface must succeed, got {status} with body {body}"
    );
    let network_interface_id = super::ec2_query::element(&body, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: no <networkInterfaceId> in {body}"));

    let (status, body) = call(
        router,
        REGION,
        "CreateVpcEndpoint",
        &[
            p("VpcId", vpc_id),
            p("ServiceName", GATEWAY_SERVICE_NAME),
            p("VpcEndpointType", "GatewayLoadBalancer"),
            p("SubnetId.1", subnet_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpcEndpoint must succeed, got {status} with body {body}"
    );
    let gateway_load_balancer_endpoint_id = super::ec2_query::element(&body, "vpcEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no <vpcEndpointId> in {body}"));

    Destinations {
        network_interface_id,
        gateway_load_balancer_endpoint_id,
    }
}

/// One row of the create table: which member names the destination, the `Type` that
/// destination implies, and the caller-chosen text the create carries beside it.
#[derive(Clone, Debug)]
struct CreateRow {
    case: &'static str,
    member: &'static str,
    kind: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The rows every create table here is driven over: one target of each
/// `TrafficMirrorTargetType` the model names
/// (`smithy_query(query="shape",shape="TrafficMirrorTargetType")`), carrying between them no
/// caller-chosen text at all, a description alone, one tag alone, and a description holding
/// punctuation and non-ASCII text beside two tags.
const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        case: "a network interface target carrying only what the service derives",
        member: "NetworkInterfaceId",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_INTERFACE,
        description: None,
        tags: &[],
    },
    CreateRow {
        case: "a network interface target with a description and two tags",
        member: "NetworkInterfaceId",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_INTERFACE,
        description: Some("mirror: ingress/egress — grün"),
        tags: &[("Team", "network"), ("Env", "staging")],
    },
    CreateRow {
        case: "a Network Load Balancer target with a description",
        member: "NetworkLoadBalancerArn",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_LOAD_BALANCER,
        description: Some("web tier mirror"),
        tags: &[],
    },
    CreateRow {
        case: "a Network Load Balancer target with one tag and no description",
        member: "NetworkLoadBalancerArn",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_LOAD_BALANCER,
        description: None,
        tags: &[("Team", "platform")],
    },
    CreateRow {
        case: "a Gateway Load Balancer endpoint target with a description and one tag",
        member: "GatewayLoadBalancerEndpointId",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_GATEWAY_LOAD_BALANCER_ENDPOINT,
        description: Some("appliance fleet"),
        tags: &[("Only", "")],
    },
    CreateRow {
        case: "a Gateway Load Balancer endpoint target carrying nothing else",
        member: "GatewayLoadBalancerEndpointId",
        kind: TRAFFIC_MIRROR_TARGET_TYPE_GATEWAY_LOAD_BALANCER_ENDPOINT,
        description: None,
        tags: &[],
    },
];

/// The value `row`'s member names, for the account's resources in `built`.
fn destination(row: &CreateRow, built: &Destinations, n: usize) -> String {
    match row.member {
        "NetworkInterfaceId" => built.network_interface_id.clone(),
        "GatewayLoadBalancerEndpointId" => built.gateway_load_balancer_endpoint_id.clone(),
        _ => network_load_balancer_arn(n),
    }
}

/// The form parameters one [`CreateRow`] is sent as.
fn create_params(row: &CreateRow, built: &Destinations, n: usize) -> Params {
    let mut params = vec![p(row.member, destination(row, built, n))];
    if let Some(description) = row.description {
        params.push(p("Description", description));
    }
    if !row.tags.is_empty() {
        params.push(p(
            "TagSpecification.1.ResourceType",
            TRAFFIC_MIRROR_TARGET_TAGGABLE_RESOURCE,
        ));
        for (index, (key, value)) in row.tags.iter().enumerate() {
            params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
    }
    params
}

/// What the row says the stored record must look like, beside the id the service mints.
fn expected(row: &CreateRow, built: &Destinations, n: usize, id: &str) -> Reported {
    let value = destination(row, built, n);
    Reported {
        id: Some(id.to_string()),
        network_interface_id: (row.member == "NetworkInterfaceId").then(|| value.clone()),
        network_load_balancer_arn: (row.member == "NetworkLoadBalancerArn")
            .then(|| value.clone()),
        gateway_load_balancer_endpoint_id: (row.member == "GatewayLoadBalancerEndpointId")
            .then(|| value.clone()),
        kind: Some(row.kind.to_string()),
        description: row.description.map(str::to_string),
        owner_id: Some(engine::emulator_account_id().to_string()),
        tags: row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

/// RULE 1: each kind of target is minted from the member that names its destination, reported
/// with the `Type` that destination implies and the owning account, and read back member for
/// member through `DescribeTrafficMirrorTargets` — so the create answers the store.
#[tokio::test]
async fn every_kind_of_target_is_created_and_read_back() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the create table").await;

    let mut minted: Vec<String> = Vec::new();
    for (n, row) in CREATE_ROWS.iter().enumerate() {
        let case = row.case;
        let (status, body) = create(&router, &create_params(row, &built, n)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateTrafficMirrorTarget, got {status} \
             with body {body}"
        );
        let created = one(case, &body);
        let id = created.id.clone().unwrap_or_default();
        assert!(
            is_long_id(TRAFFIC_MIRROR_TARGET_ID_PREFIX, &id),
            "case {case:?}: expected a minted Traffic Mirror target id, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh id, got {id:?} again"
        );
        assert_eq!(
            created,
            expected(row, &built, n, &id),
            "case {case:?}: the create must report the record it stored, got {body}"
        );
        // A create carrying no `ClientToken` echoes none.
        assert_eq!(
            super::ec2_query::member(&body, "clientToken"),
            None,
            "case {case:?}: a create carrying no ClientToken must echo none, got {body}"
        );
        minted.push(id.clone());

        // The round trip: the describe reports the same record, out of the same store.
        let (status, body) = describe(&router, &[p("TrafficMirrorTargetId.1", id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeTrafficMirrorTargets, got {status} \
             with body {body}"
        );
        assert_eq!(
            one(case, &body),
            expected(row, &built, n, &id),
            "case {case:?}: the describe must report what the create stored, got {body}"
        );
    }

    // Every target is still there, in the one order the store reports and pages over.
    let mut sorted = minted.clone();
    sorted.sort();
    assert_eq!(
        stored_ids(&engine),
        sorted,
        "the store must hold every target the table created"
    );
    let described = described_ids(&router, "the whole table", &[]).await;
    assert_eq!(
        described, sorted,
        "the describe must report every target the table created, got {described:?}"
    );
}

/// The ids `DescribeTrafficMirrorTargets` reports for `params`, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTrafficMirrorTargets, got {status} \
         with body {body}"
    );
    elements(&body, "trafficMirrorTargetId")
}

/// RULE 2: a target points at exactly one resource — more than one member is an
/// `InvalidParameterCombination`, none at all an `InvalidParameter`, and neither stores
/// anything.
#[tokio::test]
async fn a_target_points_at_exactly_one_resource() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the combination table").await;
    let eni = built.network_interface_id.clone();
    let vpce = built.gateway_load_balancer_endpoint_id.clone();
    let arn = network_load_balancer_arn(0);

    let rows: Vec<(&str, Params, &str, &str)> = vec![
        (
            "no destination at all",
            vec![p("Description", "a target of nothing")],
            "InvalidParameter",
            "Either networkInterfaceId, networkLoadBalancerArn or \
             gatewayLoadBalancerEndpointId must be specified",
        ),
        (
            "an interface and a load balancer",
            vec![p("NetworkInterfaceId", eni.clone()), p("NetworkLoadBalancerArn", arn.clone())],
            "InvalidParameterCombination",
            "You must specify one of networkInterfaceId, networkLoadBalancerArn or \
             gatewayLoadBalancerEndpointId, but not more than one",
        ),
        (
            "an interface and a Gateway Load Balancer endpoint",
            vec![
                p("NetworkInterfaceId", eni.clone()),
                p("GatewayLoadBalancerEndpointId", vpce.clone()),
            ],
            "InvalidParameterCombination",
            "You must specify one of networkInterfaceId, networkLoadBalancerArn or \
             gatewayLoadBalancerEndpointId, but not more than one",
        ),
        (
            "a load balancer and a Gateway Load Balancer endpoint",
            vec![
                p("NetworkLoadBalancerArn", arn.clone()),
                p("GatewayLoadBalancerEndpointId", vpce.clone()),
            ],
            "InvalidParameterCombination",
            "You must specify one of networkInterfaceId, networkLoadBalancerArn or \
             gatewayLoadBalancerEndpointId, but not more than one",
        ),
        (
            "all three destinations",
            vec![
                p("NetworkInterfaceId", eni.clone()),
                p("NetworkLoadBalancerArn", arn.clone()),
                p("GatewayLoadBalancerEndpointId", vpce.clone()),
            ],
            "InvalidParameterCombination",
            "You must specify one of networkInterfaceId, networkLoadBalancerArn or \
             gatewayLoadBalancerEndpointId, but not more than one",
        ),
    ];

    for (case, params, code, message) in rows {
        let before = stored_ids(&engine);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message to name the three members, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a refused create must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );
        // The same request as a dry run is refused with the reason it would have failed for,
        // rather than with `DryRunOperation`.
        let mut probe = params.clone();
        probe.push(p("DryRun", "true"));
        let (status, body) = create(&router, &probe).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a dry run of a refused create keeps that refusal, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code} from the dry run, got {body}"
        );
    }
}

/// RULE 3: a destination the account does not hold is refused with that resource's own code,
/// and a value of the wrong shape with the code for a malformed one — whichever member named
/// it, and nothing is stored.
///
/// The rows differ along the member, along whether the value is unreadable or simply names
/// nothing, and along the id form: the long 17-hex form this emulator mints, the short 8-hex
/// form of EC2's older ids, a non-hex suffix and text that is no id at all.
#[tokio::test]
async fn a_destination_the_account_does_not_hold_is_refused() {
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "a long interface id naming nothing",
            "NetworkInterfaceId",
            "eni-0123456789abcdef0",
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
            "The networkInterface ID 'eni-0123456789abcdef0' does not exist".to_string(),
        ),
        (
            "a short interface id naming nothing",
            "NetworkInterfaceId",
            "eni-0badf00d",
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
            "The networkInterface ID 'eni-0badf00d' does not exist".to_string(),
        ),
        (
            "an interface id with a non-hex suffix",
            "NetworkInterfaceId",
            "eni-not-an-id",
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
            "The interface ID 'eni-not-an-id' is malformed".to_string(),
        ),
        (
            "text that is no interface id at all",
            "NetworkInterfaceId",
            "not-an-id-at-all",
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
            "The interface ID 'not-an-id-at-all' is malformed".to_string(),
        ),
        (
            "an endpoint id naming nothing",
            "GatewayLoadBalancerEndpointId",
            "vpce-0123456789abcdef0",
            INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
            "The Vpc Endpoint Id 'vpce-0123456789abcdef0' does not exist".to_string(),
        ),
        (
            "an endpoint id with a non-hex suffix",
            "GatewayLoadBalancerEndpointId",
            "vpce-not-an-id",
            INVALID_VPC_ENDPOINT_ID_MALFORMED,
            "Invalid id: \"vpce-not-an-id\"".to_string(),
        ),
        (
            "the id of another resource kind",
            "GatewayLoadBalancerEndpointId",
            "tmt-0123456789abcdef0",
            INVALID_VPC_ENDPOINT_ID_MALFORMED,
            "Invalid id: \"tmt-0123456789abcdef0\"".to_string(),
        ),
        (
            "a value that is not an ARN",
            "NetworkLoadBalancerArn",
            "not-an-arn",
            "InvalidParameterValue",
            "Value (not-an-arn) for parameter networkLoadBalancerArn is invalid. \
             This is not the ARN of a load balancer."
                .to_string(),
        ),
        (
            "an ARN of another service",
            "NetworkLoadBalancerArn",
            "arn:aws:ec2:us-west-2:123456789012:loadbalancer/net/mirror/0",
            "InvalidParameterValue",
            "Value (arn:aws:ec2:us-west-2:123456789012:loadbalancer/net/mirror/0) for \
             parameter networkLoadBalancerArn is invalid. This is not the ARN of a load \
             balancer."
                .to_string(),
        ),
        (
            "a load balancer ARN naming no load balancer",
            "NetworkLoadBalancerArn",
            "arn:aws:elasticloadbalancing:us-west-2:123456789012:targetgroup/mirror/0",
            "InvalidParameterValue",
            "Value (arn:aws:elasticloadbalancing:us-west-2:123456789012:targetgroup/mirror/0) \
             for parameter networkLoadBalancerArn is invalid. This is not the ARN of a load \
             balancer."
                .to_string(),
        ),
    ];

    for (case, member, value, code, message) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = create(&router, &[p(member, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message to name the value the request gave, got {body}"
        );
        assert!(
            stored_ids(&engine).is_empty(),
            "case {case:?}: a refused create must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 3, the other half of resolving an endpoint: "The ID of the Gateway Load Balancer
/// endpoint" (`CreateTrafficMirrorTargetRequest$GatewayLoadBalancerEndpointId`) — an endpoint
/// of one of the other `VpcEndpointType` kinds is not one, so it is refused rather than
/// stored as a destination that mirrors nowhere.
///
/// The rows are the endpoint types `CreateVpcEndpoint` creates: the Gateway Load Balancer
/// endpoint this member takes, and the gateway and interface endpoints it does not.
#[tokio::test]
async fn only_a_gateway_load_balancer_endpoint_is_one() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, subnet_id) = {
        let store = engine.state::<Ec2State>();
        let store = store.lock();
        let vpc_id = store
            .vpcs
            .values()
            .find(|vpc| vpc.is_default == Some(true))
            .and_then(|vpc| vpc.vpc_id.clone())
            .expect("a fresh account has its default VPC");
        let subnet_id = store
            .subnets
            .values()
            .find(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
            .and_then(|subnet| subnet.subnet_id.clone())
            .expect("a fresh account has the default VPC's subnets");
        (vpc_id, subnet_id)
    };

    // `(the case, the VpcEndpointType, the service name of that kind, whether this member
    // takes it)`.
    let rows: [(&str, &str, &str, bool); 3] = [
        (
            "a Gateway Load Balancer endpoint",
            "GatewayLoadBalancer",
            GATEWAY_SERVICE_NAME,
            true,
        ),
        (
            "an interface endpoint",
            "Interface",
            "com.amazonaws.us-west-2.logs",
            false,
        ),
        (
            "a gateway endpoint",
            "Gateway",
            "com.amazonaws.us-west-2.s3",
            false,
        ),
    ];

    for (case, endpoint_type, service_name, takes_it) in rows {
        let mut params = vec![
            p("VpcId", vpc_id.clone()),
            p("ServiceName", service_name),
            p("VpcEndpointType", endpoint_type),
        ];
        if endpoint_type != "Gateway" {
            params.push(p("SubnetId.1", subnet_id.clone()));
        }
        let (status, body) = call(&router, REGION, "CreateVpcEndpoint", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateVpcEndpoint must succeed, got {status} with body {body}"
        );
        let endpoint_id = super::ec2_query::element(&body, "vpcEndpointId")
            .unwrap_or_else(|| panic!("case {case:?}: no <vpcEndpointId> in {body}"));

        let before = stored_ids(&engine);
        let (status, body) =
            create(&router, &[p("GatewayLoadBalancerEndpointId", endpoint_id.clone())]).await;
        if takes_it {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                one(case, &body).gateway_load_balancer_endpoint_id.as_deref(),
                Some(endpoint_id.as_str()),
                "case {case:?}: the target must point at the endpoint the request named, got \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an endpoint of another kind, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Value ({endpoint_id}) for parameter gatewayLoadBalancerEndpointId is \
                 invalid. This is not a Gateway Load Balancer endpoint."
            ),
            "case {case:?}: expected the message to name the endpoint the request gave, got \
             {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a refused create must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 4: one `ClientToken` names one target — a repeat reports that target and stores
/// nothing — while two creates carrying no token mint two targets. `DryRun` stores nothing
/// and reports nothing.
#[tokio::test]
async fn one_client_token_names_one_target() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the idempotency table").await;

    // The rows differ along which member names the destination, so idempotency is a fact of
    // the token rather than of one kind of target.
    for (n, row) in CREATE_ROWS.iter().enumerate() {
        let case = row.case;
        let token = format!("token-{n}");
        let mut params = create_params(row, &built, n);
        params.push(p("ClientToken", token.clone()));

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let first = one(case, &body);
        assert_eq!(
            super::ec2_query::member(&body, "clientToken").as_deref(),
            Some(token.as_str()),
            "case {case:?}: the create must echo the token the request carried, got {body}"
        );
        let after_first = stored_ids(&engine);

        // The replay: the same token, reported as the same target, with nothing stored.
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the replay, got {status} with body {body}"
        );
        assert_eq!(
            one(case, &body),
            first,
            "case {case:?}: a replay must report the target the first create minted, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            after_first,
            "case {case:?}: a replay must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );
    }

    // A create carrying NO token is never a replay: two identical requests mint two targets.
    let row = &CREATE_ROWS[0];
    let before = stored_ids(&engine).len();
    let params = create_params(row, &built, 0);
    let (status, first) = create(&router, &params).await;
    assert_eq!(status, 200, "a create without a token must succeed: {first}");
    let (status, second) = create(&router, &params).await;
    assert_eq!(status, 200, "a second create without a token must succeed: {second}");
    assert_ne!(
        one("no token, twice", &first).id,
        one("no token, twice", &second).id,
        "two creates carrying no token must mint two targets"
    );
    assert_eq!(
        stored_ids(&engine).len(),
        before + 2,
        "two creates carrying no token must store two targets, the store holds {:?}",
        stored_ids(&engine)
    );
}

/// RULE 4, the other side of a token: a token names ONE request, so a repeat of it describing
/// a DIFFERENT target is refused `IdempotentParameterMismatch` and stores nothing — the target
/// the token already named is left exactly as it was.
///
/// The rows differ along WHICH part of the request the repeat changed: the destination, the
/// kind of destination, the description, a tag's value, an added tag and a dropped tag. The
/// last row changes nothing but the ORDER two tags are listed in, which is not a difference:
/// "Tag keys must be unique per resource", so the set is the parameter.
#[tokio::test]
async fn a_token_reused_for_another_request_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the mismatch table").await;
    let eni = built.network_interface_id.clone();
    let vpce = built.gateway_load_balancer_endpoint_id.clone();

    // The request the token names: an interface target with a description and two tags.
    fn naming_request(network_interface_id: &str, token: &str) -> Params {
        vec![
            p("NetworkInterfaceId", network_interface_id.to_string()),
            p("Description", "the request the token names"),
            p(
                "TagSpecification.1.ResourceType",
                TRAFFIC_MIRROR_TARGET_TAGGABLE_RESOURCE,
            ),
            p("TagSpecification.1.Tag.1.Key", "Team"),
            p("TagSpecification.1.Tag.1.Value", "network"),
            p("TagSpecification.1.Tag.2.Key", "Env"),
            p("TagSpecification.1.Tag.2.Value", "staging"),
            p("ClientToken", token.to_string()),
        ]
    }

    /// One row: the repeat is that request with `dropped` form fields taken out and `added`
    /// put in, and `mismatch` says whether it is the TOKEN the repeat is refused over.
    struct Repeat {
        case: &'static str,
        dropped: &'static [&'static str],
        added: Params,
        mismatch: bool,
    }

    let rows: Vec<Repeat> = vec![
        Repeat {
            case: "another destination of the same kind",
            dropped: &["NetworkInterfaceId"],
            added: vec![p("NetworkInterfaceId", "eni-0123456789abcdef0")],
            mismatch: false,
        },
        Repeat {
            case: "a destination of another kind",
            dropped: &["NetworkInterfaceId"],
            added: vec![p("GatewayLoadBalancerEndpointId", vpce.clone())],
            mismatch: true,
        },
        Repeat {
            case: "another description",
            dropped: &["Description"],
            added: vec![p("Description", "a different request")],
            mismatch: true,
        },
        Repeat {
            case: "no description at all",
            dropped: &["Description"],
            added: Vec::new(),
            mismatch: true,
        },
        Repeat {
            case: "another value for one tag",
            dropped: &["TagSpecification.1.Tag.2.Value"],
            added: vec![p("TagSpecification.1.Tag.2.Value", "production")],
            mismatch: true,
        },
        Repeat {
            case: "one tag more",
            dropped: &[],
            added: vec![
                p("TagSpecification.1.Tag.3.Key", "Extra"),
                p("TagSpecification.1.Tag.3.Value", "yes"),
            ],
            mismatch: true,
        },
        Repeat {
            case: "one tag fewer",
            dropped: &[
                "TagSpecification.1.Tag.2.Key",
                "TagSpecification.1.Tag.2.Value",
            ],
            added: Vec::new(),
            mismatch: true,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let token = format!("mismatch-{index}");
        let (status, body) = create(&router, &naming_request(&eni, &token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the request the token names must succeed, got {status} with body \
             {body}"
        );
        let named = one(case, &body);
        let before = stored_ids(&engine);

        let mut repeat: Params = naming_request(&eni, &token)
            .into_iter()
            .filter(|(key, _)| !row.dropped.contains(&key.as_str()))
            .collect();
        repeat.extend(row.added.clone());
        let (status, body) = create(&router, &repeat).await;
        if !row.mismatch {
            // The destination the repeat named does not exist, so it is refused for THAT
            // reason rather than as a mismatched token: a refusal a request earns on its own
            // members is answered before the token is looked at.
            assert_eq!(
                error_code(&body),
                INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
                "case {case:?}: expected the destination's own refusal, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a token naming another request, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Parameters on this idempotent request are inconsistent with parameters used in \
             previous request(s)",
            "case {case:?}: expected EC2's sentence for a reused token, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a refused replay must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );

        // The target the token already named is untouched.
        let id = named.id.clone().unwrap_or_default();
        let (status, body) = describe(&router, &[p("TrafficMirrorTargetId.1", id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the named target must still be reported, got {status} with body \
             {body}"
        );
        assert_eq!(
            one(case, &body),
            named,
            "case {case:?}: a refused replay must leave the named target as it was, got {body}"
        );
    }

    // The same request with its two tags listed the other way round is the SAME request.
    let token = "tag-order";
    let (status, body) = create(&router, &naming_request(&eni, token)).await;
    assert_eq!(status, 200, "the request the token names must succeed: {body}");
    let named = one("tags in the other order", &body);
    let reordered: Params = naming_request(&eni, token)
        .into_iter()
        .map(|(key, value)| match key.as_str() {
            "TagSpecification.1.Tag.1.Key" => p("TagSpecification.1.Tag.1.Key", "Env"),
            "TagSpecification.1.Tag.1.Value" => p("TagSpecification.1.Tag.1.Value", "staging"),
            "TagSpecification.1.Tag.2.Key" => p("TagSpecification.1.Tag.2.Key", "Team"),
            "TagSpecification.1.Tag.2.Value" => p("TagSpecification.1.Tag.2.Value", "network"),
            _ => (key, value),
        })
        .collect();
    let (status, body) = create(&router, &reordered).await;
    assert_eq!(
        status, 200,
        "tags listed in another order are the same parameters, got {status} with body {body}"
    );
    assert_eq!(
        one("tags in the other order", &body).id,
        named.id,
        "a replay differing only in tag order must report the target the token named: {body}"
    );
}

/// RULE 4, the probe: `DryRun` asks whether the create WOULD have worked, so it is answered
/// `DryRunOperation` (HTTP 412) with nothing stored and nothing reported, and `DryRun=false`
/// is a real create.
#[tokio::test]
async fn a_dry_run_creates_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the dry run table").await;

    for (n, row) in CREATE_ROWS.iter().enumerate() {
        let case = row.case;
        let before = stored_ids(&engine);
        let mut params = create_params(row, &built, n);
        params.push(p("DryRun", "true"));

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a dry run must store nothing, the store holds {:?}",
            stored_ids(&engine)
        );

        // `DryRun=false` is not a probe at all.
        let mut params = create_params(row, &built, n);
        params.push(p("DryRun", "false"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 with DryRun=false, got {status} with body {body}"
        );
        assert_eq!(
            stored_ids(&engine).len(),
            before.len() + 1,
            "case {case:?}: DryRun=false must store the target, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 5: the describe narrows by ids and by every filter name it answers, ANDs the entries
/// and ORs the values within one, and refuses a name it does not answer.
#[tokio::test]
async fn the_describe_narrows_by_every_filter_it_answers() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "the filter table").await;

    // Three targets, each a different kind, so every filter name has something to select and
    // something to leave out.
    let mut ids: Vec<String> = Vec::new();
    for (n, row) in [&CREATE_ROWS[1], &CREATE_ROWS[2], &CREATE_ROWS[4]]
        .into_iter()
        .enumerate()
    {
        let (status, body) = create(&router, &create_params(row, &built, n)).await;
        assert_eq!(
            status, 200,
            "the filter table's fixtures must be created, got {status} with body {body}"
        );
        ids.push(one(row.case, &body).id.unwrap_or_default());
    }
    let [interface_target, load_balancer_target, endpoint_target] = [
        ids[0].clone(),
        ids[1].clone(),
        ids[2].clone(),
    ];
    let mut all = ids.clone();
    all.sort();

    let rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "every target, unnarrowed".to_string(),
            vec![],
            all.clone(),
        ),
        (
            "narrowed to one id".to_string(),
            vec![p("TrafficMirrorTargetId.1", load_balancer_target.clone())],
            vec![load_balancer_target.clone()],
        ),
        (
            "narrowed to two ids".to_string(),
            vec![
                p("TrafficMirrorTargetId.1", interface_target.clone()),
                p("TrafficMirrorTargetId.2", endpoint_target.clone()),
            ],
            {
                let mut both = vec![interface_target.clone(), endpoint_target.clone()];
                both.sort();
                both
            },
        ),
        (
            "by traffic-mirror-target-id".to_string(),
            filters(&[("traffic-mirror-target-id", &[interface_target.as_str()])]),
            vec![interface_target.clone()],
        ),
        (
            "by description".to_string(),
            filters(&[(
                "description",
                &[CREATE_ROWS[2].description.unwrap_or_default()],
            )]),
            vec![load_balancer_target.clone()],
        ),
        (
            "by network-interface-id".to_string(),
            filters(&[(
                "network-interface-id",
                &[built.network_interface_id.as_str()],
            )]),
            vec![interface_target.clone()],
        ),
        (
            "by network-load-balancer-arn".to_string(),
            filters(&[("network-load-balancer-arn", &[
                network_load_balancer_arn(1).as_str()
            ])]),
            vec![load_balancer_target.clone()],
        ),
        (
            "by owner-id, which every target of the account carries".to_string(),
            filters(&[("owner-id", &[engine::emulator_account_id()])]),
            all.clone(),
        ),
        (
            "by tag-key".to_string(),
            filters(&[("tag-key", &["Env"])]),
            vec![interface_target.clone()],
        ),
        (
            "by tag:<key>".to_string(),
            filters(&[("tag:Team", &["network"])]),
            vec![interface_target.clone()],
        ),
        (
            "two values of one entry are ORed".to_string(),
            filters(&[(
                "traffic-mirror-target-id",
                &[interface_target.as_str(), endpoint_target.as_str()],
            )]),
            {
                let mut both = vec![interface_target.clone(), endpoint_target.clone()];
                both.sort();
                both
            },
        ),
        (
            "two entries are ANDed".to_string(),
            filters(&[
                ("owner-id", &[engine::emulator_account_id()]),
                ("tag-key", &["Only"]),
            ]),
            vec![endpoint_target.clone()],
        ),
        (
            "an entry nothing matches selects nothing".to_string(),
            filters(&[("description", &["no target carries this"])]),
            vec![],
        ),
        (
            "two entries that cannot both hold select nothing".to_string(),
            filters(&[("tag-key", &["Env"]), ("tag-key", &["Only"])]),
            vec![],
        ),
    ];

    for (case, params, want) in rows {
        let got = described_ids(&router, &case, &params).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }

    // A filter name this operation does not answer is refused rather than read as "no
    // results" — "Filter names are case-sensitive" (`Filter$Name`), so a case-shifted
    // spelling is one of those names.
    for name in ["traffic-mirror-filter-id", "Description", "tag:", "", "type"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for a filter this operation does not answer, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {name:?}: expected the message to name the filter, got {body}"
        );
    }

    // An id that names no target of the account is an error, not an absent row.
    for id in ["tmt-0123456789abcdef0", "tmt-00000000", "not-an-id-at-all"] {
        let (status, body) = describe(&router, &[p("TrafficMirrorTargetId.1", id)]).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400 for an id naming no target, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND,
            "case {id:?}: expected {INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND}, got {body}"
        );
    }

    // A dry run of the describe asks about permission, and reports no target.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run of the describe is answered DryRunOperation, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "expected DryRunOperation from the describe's dry run, got {body}"
    );
}

/// RULE 5, the pages: `MaxResults` bounds a page, `NextToken` resumes where it stopped, and
/// walking the pages visits every target exactly once in id order. A page size outside the
/// modelled range is refused at the boundary.
#[tokio::test]
async fn the_report_pages_over_every_target() {
    let (router, _engine) = super::ec2_query::emulator();

    // Seven targets, so a page size of five leaves a second, shorter page.
    let total = 7usize;
    let mut minted: Vec<String> = Vec::new();
    for n in 0..total {
        let (status, body) = create(
            &router,
            &[p("NetworkLoadBalancerArn", network_load_balancer_arn(n))],
        )
        .await;
        assert_eq!(
            status, 200,
            "the paging fixtures must be created, got {status} with body {body}"
        );
        minted.push(one("the paging fixtures", &body).id.unwrap_or_default());
    }
    minted.sort();

    // The page sizes: the modelled minimum, one that leaves a single trailing row, one above
    // the number of targets, and the published maximum.
    for size in [5usize, 6, MAX_TRAFFIC_MIRROR_TARGETS_PAGE_SIZE] {
        let case = format!("MaxResults={size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = vec![p("MaxResults", size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "trafficMirrorTargetId");
            assert!(
                page.len() <= size,
                "case {case:?}: a page must hold at most {size} targets, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = super::ec2_query::member(&body, "nextToken").filter(|it| !it.is_empty());
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total,
                "case {case:?}: the walk must end, it has taken {pages} pages"
            );
        }
        assert_eq!(
            seen, minted,
            "case {case:?}: the walk must visit every target once in id order, got {seen:?}"
        );
        let expected_pages = total.div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), took {pages}"
        );
    }

    // The published bounds of `TrafficMirroringMaxResults` (`range 5..1000`): one below the
    // minimum is this service's own `InvalidMaxResults` — a positive page size it will not
    // page with — and one above the maximum keeps the boundary validator's own code, both
    // refused before this handler runs.
    for (size, code) in [("4", "InvalidMaxResults"), ("1001", BOUNDARY_CONSTRAINT_CODE)] {
        let (status, body) = describe(&router, &[p("MaxResults", size)]).await;
        assert_eq!(
            status, 400,
            "case {size:?}: a page size outside the modelled range is refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {size:?}: expected {code}, got {body}"
        );
    }

    // A `NextToken` this service did not mint names no position in the report.
    let (status, body) = describe(&router, &[p("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "a token this service did not mint is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "expected InvalidPaginationToken, got {body}"
    );
    assert_eq!(
        error_message(&body),
        ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        "expected EC2's own pagination-token sentence, got {body}"
    );
}

/// RULE 4, the token after a delete: the delete takes the token out with the record, so a
/// create carrying that token afterwards is a FRESH create — a new target, not the one the
/// token used to name and not a mismatch.
#[tokio::test]
async fn a_token_whose_target_was_deleted_names_no_target() {
    let (router, engine) = super::ec2_query::emulator();
    let built = destinations(&router, &engine, "a token after a delete").await;

    for (n, row) in CREATE_ROWS.iter().enumerate() {
        let case = row.case;
        let token = format!("after-delete-{n}");
        let mut params = create_params(row, &built, n);
        params.push(p("ClientToken", token.clone()));

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} with body {body}"
        );
        let first = one(case, &body).id.unwrap_or_default();

        let (status, body) = call(
            &router,
            REGION,
            "DeleteTrafficMirrorTarget",
            &[p("TrafficMirrorTargetId", first.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must succeed, got {status} with body {body}"
        );

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a create carrying the token of a deleted target must succeed, got \
             {status} with body {body}"
        );
        let second = one(case, &body).id.unwrap_or_default();
        assert_ne!(
            second, first,
            "case {case:?}: the create must mint a new target, not report the deleted one"
        );
        assert!(
            stored_ids(&engine).contains(&second),
            "case {case:?}: the new target must be in the store, which holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// The store's byte budget, the second of the two bounds a create is held to: a record whose
/// caller-chosen text would not fit is refused for THAT reason, with nothing stored and the
/// budget untouched, while the count ceiling has room to spare.
///
/// Driven at the store, because the budget is this emulator's whole caller-supplied-data
/// ceiling ([`ec2::state::MAX_STORED_BYTES`]) rather than a per-request one: the rows are a
/// description of exactly the budget and one past it, beside one that fits.
#[test]
fn a_target_past_the_stores_byte_budget_is_refused() {
    use ec2::state::{TrafficMirrorTargetRoom, MAX_STORED_BYTES};

    let (_router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    let mut store = store.lock();
    let held = store.stored_bytes();

    let target = |description_bytes: usize| TrafficMirrorTarget {
        traffic_mirror_target_id: None,
        network_interface_id: Some("eni-00000000000000001".to_string()),
        network_load_balancer_arn: None,
        r#type: Some(TRAFFIC_MIRROR_TARGET_TYPE_NETWORK_INTERFACE.to_string()),
        description: Some("d".repeat(description_bytes)),
        owner_id: Some(engine::emulator_account_id().to_string()),
        tags: Some(Vec::new()),
        gateway_load_balancer_endpoint_id: None,
    };

    for (case, description_bytes) in [
        ("a description of the whole byte budget", MAX_STORED_BYTES),
        ("a description one byte past it", MAX_STORED_BYTES + 1),
    ] {
        let record = target(description_bytes);
        assert_eq!(
            store.traffic_mirror_target_room(&record, None),
            TrafficMirrorTargetRoom::OverByteBudget {
                requested: description_bytes,
                held,
            },
            "case {case:?}: expected the byte budget to be the bound that refuses it, the \
             store holds {held} bytes"
        );
        assert!(
            store.insert_traffic_mirror_target(record, None).is_none(),
            "case {case:?}: a record past the byte budget must not be stored"
        );
        assert_eq!(
            store.stored_bytes(),
            held,
            "case {case:?}: a refused insert must charge nothing, the store holds {}",
            store.stored_bytes()
        );
        assert!(
            store.traffic_mirror_targets.is_empty(),
            "case {case:?}: a refused insert must store nothing, the store holds {:?}",
            store.traffic_mirror_targets.keys().collect::<Vec<_>>()
        );
    }

    // One that fits is stored, and its bytes are what the budget then holds.
    let record = target(1_024);
    assert_eq!(
        store.traffic_mirror_target_room(&record, None),
        TrafficMirrorTargetRoom::Available,
        "a record inside the budget must have room, the store holds {held} bytes"
    );
    assert!(
        store.insert_traffic_mirror_target(record, None).is_some(),
        "a record inside the budget must be stored"
    );
    assert_eq!(
        store.stored_bytes(),
        held + 1_024,
        "the insert must charge exactly the description it stored, the store holds {}",
        store.stored_bytes()
    );
}
