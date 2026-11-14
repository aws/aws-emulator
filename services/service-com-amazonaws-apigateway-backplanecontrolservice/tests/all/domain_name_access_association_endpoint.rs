// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource a domain name access association's
//! `accessAssociationSource` names: the VPC endpoint it binds a private custom domain name
//! to.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — with Amazon EC2 mounted beside this
//! service on the same engine, so every endpoint a case names is created and removed
//! through EC2's OWN API rather than written into a store by hand.
//!
//! The RULE under test, one level above the recorded call:
//!
//! > A member that identifies a resource owned by ANOTHER service is resolved against that
//! > service before a create stores anything, and an identifier naming no such resource is
//! > the operation's modelled refusal with nothing written. For
//! > `CreateDomainNameAccessAssociation` with `accessAssociationSourceType` `VPCE`, the
//! > `accessAssociationSource` "is the VPC endpoint ID", so it must name a VPC endpoint the
//! > account has in the Region the caller addressed: one that does is created (201) and
//! > listed, and one that does not is the modelled `BadRequestException` (400) whose message
//! > is the identifier the request sent followed by the fixed text " is not a valid VPC
//! > endpoint id", with no association stored for it.
//!
//! The rows span what that rule generalises over: which endpoints the account holds (two,
//! created through EC2, each binding its own association), each hexadecimal width EC2 mints
//! an id under for an id the account has none of, an id one digit away from one it does
//! hold, the id of an endpoint that was created and then REMOVED — which is what shows the
//! identifier is resolved live rather than remembered — and the forms that are not an
//! endpoint id at all (the uppercase spelling of a real one, an endpoint SERVICE id, the ARN
//! of an endpoint rather than its id, a VPC id, a bare id with no prefix).
//!
//! [`Gateway`], [`gateway_with_ec2`], [`page_items`] and [`string_member`] are this crate's
//! one wire-test harness, and the create, list and private-domain helpers are the access
//! association suite's own.

use bytes::Bytes;

use crate::domain_name_access_associations::{
    association_body, create_association, list_associations, private_domain,
};
use crate::rest_api_lifecycle::{gateway_with_ec2, page_items, string_member, Gateway};

const BAD_REQUEST: &str = "BadRequestException";

/// The Region every request in this suite is signed for, which is the one the harness's
/// credential scope carries — so the apigateway create and the EC2 call that stood the
/// endpoint up address the same Region.
const REGION: &str = "us-east-1";

/// The Amazon EC2 API version an `ec2Query` body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The refusal AWS answers an `accessAssociationSource` naming no VPC endpoint of the
/// account with: the identifier the request sent, then the fixed text.
fn not_a_valid_endpoint_id(source: &str) -> String {
    format!("{source} is not a valid VPC endpoint id")
}

/// One `ec2Query` call to the Amazon EC2 service mounted beside this one, answering the XML
/// body it answered with.
///
/// EC2 speaks `ec2Query`: an `application/x-www-form-urlencoded` POST carrying
/// `Action=<Operation>`, answered with XML. The request goes through the SAME router the
/// apigateway calls go through, so both services read one engine.
async fn ec2(gateway: &Gateway, action: &str, params: &[(&str, &str)], case: &str) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}\
                 /ec2/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected EC2 {action} to answer 200, got {status} with {payload}"
    );
    payload
}

/// The text of the FIRST `<name>` element of an `ec2Query` response — the minted id a create
/// answers with.
fn ec2_element(xml: &str, name: &str, case: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {xml}"))
        + open.len();
    let end = xml[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{name}> in {xml}"))
        + start;
    xml[start..end].to_string()
}

/// The network an interface endpoint is created in: a VPC and one subnet of it.
struct Network {
    vpc: String,
    subnet: String,
}

/// One VPC with one subnet, created through EC2's own API.
async fn network(gateway: &Gateway) -> Network {
    let case = "network";
    let vpc = ec2_element(
        &ec2(gateway, "CreateVpc", &[("CidrBlock", "10.0.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let subnet = ec2_element(
        &ec2(
            gateway,
            "CreateSubnet",
            &[
                ("VpcId", &vpc),
                ("CidrBlock", "10.0.0.0/24"),
                ("AvailabilityZone", &format!("{REGION}a")),
            ],
            case,
        )
        .await,
        "subnetId",
        case,
    );
    Network { vpc, subnet }
}

/// One INTERFACE endpoint to the Region's `execute-api` service, which is the endpoint a
/// private custom domain name is reached through, answering the id EC2 minted for it.
async fn execute_api_endpoint(gateway: &Gateway, network: &Network, case: &str) -> String {
    let answered = ec2(
        gateway,
        "CreateVpcEndpoint",
        &[
            ("VpcId", &network.vpc),
            ("ServiceName", &format!("com.amazonaws.{REGION}.execute-api")),
            ("VpcEndpointType", "Interface"),
            ("SubnetId.1", &network.subnet),
            // A VPC you create has DNS hostnames off, which a private hosted zone needs; this
            // endpoint is only what the private domain name is reached through, so it asks for
            // no private DNS.
            ("PrivateDnsEnabled", "false"),
        ],
        case,
    )
    .await;
    ec2_element(&answered, "vpcEndpointId", case)
}

/// The `accessAssociationSource` of every association the account holds, in page order.
async fn listed_sources(gateway: &Gateway, case: &str) -> Vec<String> {
    let listed = list_associations(gateway, "limit=500").await;
    page_items(&listed.body, case)
        .iter()
        .map(|item| string_member(item, "accessAssociationSource", case))
        .collect()
}

#[tokio::test]
async fn an_access_association_source_must_name_a_vpc_endpoint_the_account_has() {
    let gateway = gateway_with_ec2();
    let network = network(&gateway).await;
    let domain = private_domain(&gateway, "endpoint-bound.example.com").await;

    // The positive control, twice over: each endpoint the account HAS is admitted, and each
    // is its own source, so each stores its own association.
    let held: Vec<String> = vec![
        execute_api_endpoint(&gateway, &network, "the first endpoint of the account").await,
        execute_api_endpoint(&gateway, &network, "the second endpoint of the account").await,
    ];
    for (index, source) in held.iter().enumerate() {
        let case = format!("an endpoint the account has (number {})", index + 1);
        let answer = create_association(&gateway, association_body(&domain, source)).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["accessAssociationSource"].as_str(),
            Some(source.as_str()),
            "case {case:?}: expected the source it was created for, got {:?}",
            answer.body
        );
    }

    // Read back through the implemented reader: what the creates reported is what the
    // collection holds.
    let sources = listed_sources(&gateway, "after the admitted creates").await;
    for source in &held {
        assert!(
            sources.contains(source),
            "case \"after the admitted creates\": expected {source:?} among the listed \
             sources, got {sources:?}"
        );
    }

    // An endpoint the account HAD and no longer has: the identifier is resolved against what
    // EC2 holds NOW, so the create is refused once the endpoint is gone.
    let removed = execute_api_endpoint(&gateway, &network, "an endpoint that is then removed").await;
    ec2(
        &gateway,
        "DeleteVpcEndpoints",
        &[("VpcEndpointId.1", &removed)],
        "an endpoint that is then removed",
    )
    .await;

    // Rows differ along WHY the identifier names no endpoint of the account: a well-formed
    // id of each width EC2 mints that nothing was created under, an id one digit away from
    // one the account does hold, an endpoint that was removed, and the forms that are not an
    // endpoint id at all.
    let one_digit_off = {
        let held = &held[0];
        let (body, last) = held.split_at(held.len() - 1);
        let shifted = if last == "0" { "1" } else { "0" };
        format!("{body}{shifted}")
    };
    let refused: Vec<(String, String)> = vec![
        (
            "a well-formed 17-digit id the account has no endpoint under".to_string(),
            "vpce-0123456789abcdef0".to_string(),
        ),
        (
            "a well-formed 8-digit id the account has no endpoint under".to_string(),
            "vpce-0a1b2c3d".to_string(),
        ),
        (
            "an id one digit away from one the account does hold".to_string(),
            one_digit_off,
        ),
        (
            "the id of an endpoint that was created and then removed".to_string(),
            removed.clone(),
        ),
        (
            "the uppercase spelling of an id the account holds".to_string(),
            held[0].to_uppercase(),
        ),
        (
            "an endpoint SERVICE id rather than an endpoint id".to_string(),
            "vpce-svc-0123456789abcdef0".to_string(),
        ),
        (
            "the ARN of an endpoint rather than its id".to_string(),
            format!("arn:aws:ec2:{REGION}:000000000000:vpc-endpoint/{}", held[0]),
        ),
        (
            "a VPC id rather than an endpoint id".to_string(),
            "vpc-0123456789abcdef0".to_string(),
        ),
        (
            "an id the account holds with its prefix stripped".to_string(),
            held[0]
                .strip_prefix("vpce-")
                .expect("an endpoint id carries the prefix")
                .to_string(),
        ),
    ];

    for (case, source) in &refused {
        let answer = create_association(&gateway, association_body(&domain, source)).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some(BAD_REQUEST)),
            "case {case:?}: expected 400 {BAD_REQUEST}, got {} {:?} with {:?}",
            answer.status,
            answer.error,
            answer.body
        );
        let message = string_member(&answer.body, "message", case);
        assert_eq!(
            message,
            not_a_valid_endpoint_id(source),
            "case {case:?}: expected the identifier refusal naming the source sent, got \
             {message:?}"
        );
    }

    // NOTHING the refusals named was stored: the collection still holds exactly the
    // associations the admitted creates made.
    let sources = listed_sources(&gateway, "after every refusal").await;
    assert_eq!(
        sources.len(),
        held.len(),
        "case \"after every refusal\": expected only the {} admitted associations, got \
         {sources:?}",
        held.len()
    );
    for (case, source) in &refused {
        assert!(
            !sources.contains(source),
            "case {case:?}: expected no association stored for it, got {sources:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_source_leaves_the_pair_free_for_the_endpoint_that_does_exist() {
    let gateway = gateway_with_ec2();
    let network = network(&gateway).await;
    let domain = private_domain(&gateway, "retry-after-refusal.example.com").await;
    let case = "a create retried with an endpoint the account has";

    // A typo is refused, and the SAME pair is then created once the identifier names an
    // endpoint the account holds: the refusal reserved nothing.
    let typo = "vpce-00000000000000000";
    let answer = create_association(&gateway, association_body(&domain, typo)).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some(BAD_REQUEST)),
        "case {case:?}: expected the typo to be refused with 400 {BAD_REQUEST}, got {} {:?}",
        answer.status,
        answer.error
    );

    let endpoint = execute_api_endpoint(&gateway, &network, case).await;
    let answer = create_association(&gateway, association_body(&domain, &endpoint)).await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let listed: Vec<String> = listed_sources(&gateway, case).await;
    assert_eq!(
        listed,
        vec![endpoint.clone()],
        "case {case:?}: expected exactly the association for {endpoint:?}, got {listed:?}"
    );

    // And the pair is now taken: a second create of it is the already-exists refusal rather
    // than a second record, which is the same rule the source resolution sits in front of.
    let answer = create_association(&gateway, association_body(&domain, &endpoint)).await;
    assert_eq!(
        (answer.status, answer.error.as_deref()),
        (400, Some(BAD_REQUEST)),
        "case \"a second create of one pair\": expected 400 {BAD_REQUEST}, got {} {:?}",
        answer.status,
        answer.error
    );
    let message = string_member(&answer.body, "message", "a second create of one pair");
    assert!(
        message.to_lowercase().contains("already exists"),
        "case \"a second create of one pair\": expected the already-exists sentence, got \
         {message:?}"
    );
}
