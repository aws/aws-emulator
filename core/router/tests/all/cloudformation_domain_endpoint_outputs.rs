// SPDX-License-Identifier: CC0-1.0

//! Where a stack's outputs say a deployed search domain is REACHED, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **an endpoint return value of a deployed resource carries the address the
//! OWNING service publishes for that resource, whichever way the resource is reached — so a
//! stack's output over `AWS::OpenSearchService::Domain`'s `DomainEndpoint` ("the
//! domain-specific endpoint used to submit index, search, and data upload requests to the
//! domain") carries the public host of a domain that has one and, for a domain provisioned
//! INSIDE a VPC, which has no public host at all, the host that service publishes in its VPC
//! endpoint map.** The consequence the reported symptom is about: once the deployment has
//! completed, every output the template declares is reported WITH a resolved `OutputValue`,
//! which is what the standard `{o["OutputKey"]: o["OutputValue"]}` read of a deployed stack
//! needs — an output carrying a key and no value raised `KeyError: 'OutputValue'` in the
//! deploy phase of a five-phase diagnosis lifecycle and abandoned it at phase 1 of 5.
//!
//! The rows differ along what the rule generalises over — WHERE the domain is reached and over
//! which address family, which is what decides which addresses its owning service publishes
//! and under which member of `DomainStatus`: a public IPv4 domain, a public `dualstack` one
//! (which is provisioned a version 2 endpoint beside its IPv4 host), a domain reached inside a
//! VPC, and a `dualstack` domain inside a VPC (whose two hosts are both published in the
//! endpoint map) — and along the SPELLING of the body, two rows declaring the same estate as
//! YAML with the short-form intrinsic tags.
//!
//! No expected address is a literal: each is read out of Amazon OpenSearch Service through
//! `DescribeDomain` on its own wire, so a row cannot drift from the service the stack created
//! the domain in. Every row asserts BOTH directions — an address that service publishes is
//! carried by the output over it, and an address it publishes none of leaves its output
//! without a value rather than carrying an invented host.
//!
//! Two services on one engine: CloudFormation reaches Amazon OpenSearch Service through the
//! in-process callback to create the domain, which is why the suite lives here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for, and so the Region the stack's domain and its
/// ARN belong to. It is the Region [`authorization`] signs for.
const REGION: &str = "us-east-1";

/// How often a deployment is asked whether it has settled, and how long it goes on being
/// asked: a stack whose domain the owning service has not reported ready yet completes from
/// the instant that service does, which no fixed span names, and a loaded machine is slower
/// than that service's spans without the rule being any different.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SETTLE_POLLS: usize = 600;

/// Amazon OpenSearch Service's signing name (`sigv4Name: es`) and the URI its domain
/// operations are bound to (`DescribeDomain` is `GET {DomainName}` under it).
const OPENSEARCH_SIGNING_NAME: &str = "es";
const OPENSEARCH_DOMAIN_URI: &str = "/2021-01-01/opensearch/domain";

/// The two keys `DomainStatus$Endpoints` publishes a VPC-reached domain's hosts under: the
/// IPv4 host and, for a `dualstack` domain, its version 2 host
/// (`smithy_query(query="doc",shape="DomainStatus",member="Endpoints")` of that service's
/// model — "IPv4 IP addresses — `'vpc', …`", "Dual stack IP addresses — `'vpcv2': …`").
const ENDPOINTS_KEY_VPC: &str = "vpc";
const ENDPOINTS_KEY_VPC_DUALSTACK: &str = "vpcv2";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: the stack really creates its domain in Amazon
/// OpenSearch Service.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(engine),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// `DescribeDomain` on Amazon OpenSearch Service's own `restJson1` wire: the `DomainStatus`
/// that service publishes for the domain the stack created.
async fn describe_domain(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("{OPENSEARCH_DOMAIN_URI}/{name}"))
        .header("host", format!("es.{REGION}.amazonaws.com"))
        .header("authorization", authorization(OPENSEARCH_SIGNING_NAME))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain({name}), got {status} with {parsed}"
    );
    parsed
        .get("DomainStatus")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no DomainStatus in {parsed}"))
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeStacks, got {status} with {body}"
    );
    body
}

/// Whether the answer reports a stack no operation is still in flight on.
fn is_settled(described: &str) -> bool {
    opt_element(described, "StackStatus")
        .is_some_and(|status| status.ends_with("_COMPLETE") || status.ends_with("_FAILED"))
}

/// Poll `DescribeStacks` until the stack reports no operation in flight, and answer the body
/// of that observation. Nothing sleeps inside a handler: each poll collects what the clock
/// already owed.
async fn settle_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut described = describe_stack(router, case, stack_name).await;
    for _ in 0..SETTLE_POLLS {
        if is_settled(&described) {
            return described;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        described = describe_stack(router, case, stack_name).await;
    }
    described
}

/// Every `ResourceStatusReason` the stack's events carry — where an owning service's own
/// refusal of a resource is reported, which is what a deployment that did not complete must
/// be quoted with.
async fn resource_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&events, "ResourceStatusReason")
}

/// Where a row's domain is reached, and over which address family: what decides which
/// addresses its owning service publishes, and under which member of `DomainStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// A public domain reached over IPv4 only: the service publishes `Endpoint` and nothing
    /// else.
    Public,
    /// A public `dualstack` domain: the service publishes `Endpoint` and `EndpointV2`.
    PublicDualStack,
    /// A domain provisioned inside a VPC: no public host at all, and one entry in the
    /// endpoint map.
    Vpc,
    /// A `dualstack` domain inside a VPC: no public host, and both entries in the map.
    VpcDualStack,
}

impl Reach {
    /// Whether a domain of this reachability is provisioned a version 2 (dual-stack) address
    /// at all: "if `IPAddressType` [is] set to `dualstack`, a version 2 domain endpoint is
    /// provisioned".
    fn dual_stack(self) -> bool {
        matches!(self, Self::PublicDualStack | Self::VpcDualStack)
    }

    /// Whether the domain is reached inside a VPC, which is what decides whether its
    /// addresses are published as the public members or in the endpoint map.
    fn in_a_vpc(self) -> bool {
        matches!(self, Self::Vpc | Self::VpcDualStack)
    }
}

/// How the body of a row is spelled: the spelling is no part of what an output resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// JSON with the long-form intrinsic functions.
    Json,
    /// YAML with the short-form intrinsic tags.
    Yaml,
}

/// One row: a stack, how its domain is reached and how its body is spelled.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// Inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and 3..28 characters.
    domain: &'static str,
    reach: Reach,
    spelling: Spelling,
}

/// The rows differ along what the rule generalises over — which addresses the owning service
/// publishes for the domain, from one public host through two to the one and the two it
/// publishes in its VPC endpoint map — and along the spelling of the body.
const ROWS: &[Row] = &[
    Row {
        case: "a public search domain, JSON body",
        stack: "search-public",
        domain: "search-public",
        reach: Reach::Public,
        spelling: Spelling::Json,
    },
    Row {
        case: "a public dualstack search domain, YAML short-form body",
        stack: "search-dual",
        domain: "search-dual",
        reach: Reach::PublicDualStack,
        spelling: Spelling::Yaml,
    },
    Row {
        case: "a search domain reached inside a VPC, JSON body",
        stack: "search-vpc",
        domain: "search-vpc",
        reach: Reach::Vpc,
        spelling: Spelling::Json,
    },
    Row {
        case: "a dualstack search domain inside a VPC, YAML short-form body",
        stack: "search-vpcdual",
        domain: "search-vpcdual",
        reach: Reach::VpcDualStack,
        spelling: Spelling::Yaml,
    },
];

/// The outputs every row declares, whose value the rule holds to being resolved: the domain's
/// identity, the address it is reached on, and one member of the service-software record the
/// owning service reports for every domain it holds.
const DECLARED_OUTPUTS: &[&str] = &[
    "DomainRef",
    "DomainArn",
    "DomainIdOut",
    "DomainEndpoint",
    "SoftwareCurrentVersion",
];

/// The output the version 2 address of a `dualstack` domain is published under; declared only
/// by the rows whose domain is provisioned one, since an output over an address the estate
/// does not hold is one that legitimately resolves to nothing.
const DUAL_STACK_OUTPUT: &str = "DomainEndpointV2";

/// The output that reads ONE ENTRY of the VPC endpoint map, through the dotted path
/// `Fn::GetAtt` addresses a map-valued attribute with. Declared by every row, so each row
/// asserts the other direction too: a key the service publishes no host under resolves to
/// nothing.
fn endpoint_map_output(key: &str) -> String {
    format!("HostIn{key}")
}

/// The domain the row declares, as the properties the template states for it.
fn domain_properties(row: &Row) -> Value {
    let mut properties = json!({
        "DomainName": row.domain,
        "EngineVersion": "OpenSearch_2.11",
        "ClusterConfig": {"InstanceType": "t3.small.search", "InstanceCount": 1},
        "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10},
    });
    let object = properties
        .as_object_mut()
        .expect("the properties are an object");
    if row.reach.in_a_vpc() {
        // The subnet and the security group a VPC-reached domain is placed in. They are
        // stated as identifiers rather than declared beside the domain because what this
        // suite is about is which ADDRESS the domain then publishes, which does not depend on
        // who created the network it is placed in.
        object.insert(
            "VPCOptions".to_string(),
            json!({
                "SubnetIds": ["subnet-00000000000000001"],
                "SecurityGroupIds": ["sg-00000000000000001"],
            }),
        );
    }
    if row.reach.dual_stack() {
        object.insert("IPAddressType".to_string(), json!("dualstack"));
    }
    properties
}

/// The `Outputs` section of a row: `Ref`, the identity attributes, the addresses the domain is
/// reached on, and one entry of the endpoint map per key that map can carry.
fn outputs_section(row: &Row) -> Value {
    let mut outputs = json!({
        "DomainRef": {"Value": {"Ref": "SearchDomain"}},
        "DomainArn": {"Value": {"Fn::GetAtt": ["SearchDomain", "Arn"]}},
        "DomainIdOut": {"Value": {"Fn::GetAtt": ["SearchDomain", "Id"]}},
        "DomainEndpoint": {"Value": {"Fn::GetAtt": ["SearchDomain", "DomainEndpoint"]}},
        "SoftwareCurrentVersion": {
            "Value": {"Fn::GetAtt": ["SearchDomain", "ServiceSoftwareOptions.CurrentVersion"]}
        },
    });
    let object = outputs.as_object_mut().expect("the outputs are an object");
    for key in [ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK] {
        object.insert(
            endpoint_map_output(key),
            json!({"Value": {"Fn::GetAtt": ["SearchDomain", format!("DomainEndpoints.{key}")]}}),
        );
    }
    if row.reach.dual_stack() {
        object.insert(
            DUAL_STACK_OUTPUT.to_string(),
            json!({"Value": {"Fn::GetAtt": ["SearchDomain", "DomainEndpointV2"]}}),
        );
    }
    outputs
}

/// The row's body as JSON with the long-form intrinsic functions.
fn json_template(row: &Row) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a search domain and the addresses it publishes",
        "Resources": {
            "SearchDomain": {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": domain_properties(row),
            },
        },
        "Outputs": outputs_section(row),
    })
    .to_string()
}

/// The SAME estate as YAML with the short-form intrinsic tags, so a row can assert that the
/// spelling of the body is no part of what an output resolves to.
///
/// The domain's properties are spliced in as the JSON that section also is: YAML 1.2 is a
/// superset of JSON, so a nested flow mapping keeps the two spellings one estate rather than
/// two — while every output is written with the short-form `!Ref` / `!GetAtt` tags, which is
/// what the two spellings differ in.
fn yaml_template(row: &Row) -> String {
    format!(
        "AWSTemplateFormatVersion: '2010-09-09'\n\
         Description: a search domain and the addresses it publishes\n\
         Resources:\n\
         \x20 SearchDomain:\n\
         \x20   Type: AWS::OpenSearchService::Domain\n\
         \x20   Properties: {domain}\n\
         Outputs:\n\
         \x20 DomainRef:\n\
         \x20   Value: !Ref SearchDomain\n\
         \x20 DomainArn:\n\
         \x20   Value: !GetAtt SearchDomain.Arn\n\
         \x20 DomainIdOut:\n\
         \x20   Value: !GetAtt SearchDomain.Id\n\
         \x20 DomainEndpoint:\n\
         \x20   Value: !GetAtt SearchDomain.DomainEndpoint\n\
         \x20 SoftwareCurrentVersion:\n\
         \x20   Value: !GetAtt SearchDomain.ServiceSoftwareOptions.CurrentVersion\n\
         \x20 {vpc_output}:\n\
         \x20   Value: !GetAtt SearchDomain.DomainEndpoints.{ENDPOINTS_KEY_VPC}\n\
         \x20 {vpc_dual_output}:\n\
         \x20   Value: !GetAtt SearchDomain.DomainEndpoints.{ENDPOINTS_KEY_VPC_DUALSTACK}\n\
         {dual_stack}",
        domain = domain_properties(row),
        vpc_output = endpoint_map_output(ENDPOINTS_KEY_VPC),
        vpc_dual_output = endpoint_map_output(ENDPOINTS_KEY_VPC_DUALSTACK),
        dual_stack = if row.reach.dual_stack() {
            format!(
                "\x20 {DUAL_STACK_OUTPUT}:\n\x20   Value: !GetAtt SearchDomain.DomainEndpointV2\n"
            )
        } else {
            String::new()
        },
    )
}

/// The body a row deploys, in its own spelling.
fn template(row: &Row) -> String {
    match row.spelling {
        Spelling::Json => json_template(row),
        Spelling::Yaml => yaml_template(row),
    }
}

/// The output keys a row declares whose value must be resolved: every output over something
/// the estate HOLDS.
fn resolved_outputs(row: &Row) -> Vec<String> {
    let mut keys: Vec<String> = DECLARED_OUTPUTS.iter().map(|key| key.to_string()).collect();
    if row.reach.dual_stack() {
        keys.push(DUAL_STACK_OUTPUT.to_string());
    }
    for key in published_keys(row.reach) {
        keys.push(endpoint_map_output(key));
    }
    keys
}

/// The keys of the VPC endpoint map a domain of this reachability is published under.
fn published_keys(reach: Reach) -> Vec<&'static str> {
    match reach {
        Reach::Public | Reach::PublicDualStack => Vec::new(),
        Reach::Vpc => vec![ENDPOINTS_KEY_VPC],
        Reach::VpcDualStack => vec![ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK],
    }
}

/// The address the OWNING service publishes for the domain as its ENDPOINT — "the
/// domain-specific endpoint used to submit index, search, and data upload requests to the
/// domain" — read off the `DomainStatus` it reports rather than spelled here: the public host
/// for a domain that has one, and the host it is reached on inside a VPC for one that has no
/// public host at all.
fn published_endpoint(status: &Value) -> Option<String> {
    reported_text(status, "Endpoint")
        .or_else(|| endpoint_map(status).get(ENDPOINTS_KEY_VPC).cloned())
}

/// The same for the domain's VERSION 2 (dual-stack) address: the public `EndpointV2` where
/// the service reports one, and otherwise the dual-stack entry of its VPC endpoint map.
fn published_endpoint_v2(status: &Value) -> Option<String> {
    reported_text(status, "EndpointV2")
        .or_else(|| endpoint_map(status).get(ENDPOINTS_KEY_VPC_DUALSTACK).cloned())
}

/// `DomainStatus$Endpoints` as the map a client reads it as — empty for a domain that
/// publishes none.
fn endpoint_map(status: &Value) -> BTreeMap<String, String> {
    status
        .get("Endpoints")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, host)| Some((key.clone(), host.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// One member of a reported record as the single text an attribute reads as, or `None` where
/// the record carries no such member.
fn reported_text(status: &Value, member: &str) -> Option<String> {
    status
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_deployed_domains_outputs_carry_the_address_it_is_reached_on() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: expected the domain to deploy, got {stack_status:?} with reasons \
             {:?}",
            resource_reasons(&router, row.stack).await
        );
        let outputs = outputs_of(&described);

        // What the OWNING service publishes for the domain, which every expected address
        // below is read from rather than spelled here.
        let status_document = describe_domain(&router, case, row.domain).await;
        let published = endpoint_map(&status_document);
        let expected_keys: Vec<String> = published_keys(row.reach)
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            published.keys().cloned().collect::<Vec<String>>(),
            expected_keys,
            "case {case:?}: the row's premise is that the service publishes {expected_keys:?} \
             in its endpoint map, got {published:?}"
        );
        let public = reported_text(&status_document, "Endpoint");
        assert_eq!(
            public.is_some(),
            !row.reach.in_a_vpc(),
            "case {case:?}: the row's premise is that only a domain outside a VPC has a \
             public endpoint, got {status_document}"
        );

        // The symptom itself: every output whose expression names something the estate holds
        // is reported WITH its value, so the standard
        // `{o["OutputKey"]: o["OutputValue"]}` read of a deployed stack resolves.
        for key in resolved_outputs(row) {
            let carried = outputs.get(&key);
            assert!(
                carried.is_some_and(|value| !value.is_empty()),
                "case {case:?}: every Output of a CREATE_COMPLETE stack naming something the \
                 estate holds carries its OutputValue; {key:?} carries {carried:?} \
                 ({outputs:?})"
            );
        }

        // The rule: the endpoint attribute carries the address the owning service publishes
        // for the domain, however the domain is reached.
        let expected_endpoint = published_endpoint(&status_document);
        assert_eq!(
            outputs.get("DomainEndpoint").cloned(),
            expected_endpoint,
            "case {case:?}: expected DomainEndpoint to be the address the owning service \
             publishes for the domain ({expected_endpoint:?}), got {outputs:?}"
        );
        // ...and the same over the other address family, for the rows whose domain is
        // provisioned a version 2 address at all.
        let expected_v2 = published_endpoint_v2(&status_document);
        assert_eq!(
            expected_v2.is_some(),
            row.reach.dual_stack(),
            "case {case:?}: the row's premise is that only a dualstack domain is provisioned \
             a version 2 address, got {status_document}"
        );
        if row.reach.dual_stack() {
            assert_eq!(
                outputs.get(DUAL_STACK_OUTPUT).cloned(),
                expected_v2,
                "case {case:?}: expected {DUAL_STACK_OUTPUT} to be the version 2 address the \
                 owning service publishes ({expected_v2:?}), got {outputs:?}"
            );
            assert_ne!(
                outputs.get(DUAL_STACK_OUTPUT),
                outputs.get("DomainEndpoint"),
                "case {case:?}: the two address families are different hosts, got {outputs:?}"
            );
        }

        // The other direction, over every entry of the endpoint map: an entry the service
        // publishes a host under is carried by the output that reads it, and one it publishes
        // nothing under carries nothing rather than an invented host.
        for key in [ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK] {
            let name = endpoint_map_output(key);
            assert_eq!(
                outputs.get(&name).cloned(),
                published.get(key).cloned(),
                "case {case:?}: expected the output {name:?} to be the service's own {key:?} \
                 host ({:?}), got {:?}",
                published.get(key),
                outputs.get(&name)
            );
        }

        // "Ref returns the domain name", whatever the domain is reached on — so the identity
        // half of the outputs is held to the service's own record too.
        assert_eq!(
            outputs.get("DomainRef").map(String::as_str),
            Some(row.domain),
            "case {case:?}: Ref returns the domain name, got {outputs:?}"
        );
        for (key, member) in [("DomainArn", "ARN"), ("DomainIdOut", "DomainId")] {
            let reported = reported_text(&status_document, member);
            assert_eq!(
                outputs.get(key).cloned(),
                reported,
                "case {case:?}: expected the output {key:?} to be the owning service's own \
                 DomainStatus${member} ({reported:?}), got {outputs:?}"
            );
        }
    }
}
