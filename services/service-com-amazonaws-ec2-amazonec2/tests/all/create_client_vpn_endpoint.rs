// SPDX-License-Identifier: CC0-1.0

//! What `CreateClientVpnEndpoint` persists, and what it refuses — driven over the wire the
//! service actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness), with Certificate Manager mounted on the SAME engine, because the certificates a
//! Client VPN endpoint presents are that service's resources.
//!
//! The rules under test:
//!
//! 1. a create STORES one endpoint of the account from the members the request carries, mints
//!    its id and its DNS name, answers it `pending-associate`, and everything it stored is
//!    what `DescribeClientVpnEndpoints` reports back;
//! 2. the client CIDR range has to be an IPv4 CIDR block naming its own range, of a size
//!    inside the range AWS publishes for it — at least /22 and no greater than /12 — and
//!    every other value is refused `InvalidParameterValue`;
//! 3. every certificate ARN the request names has to resolve to a certificate the ACCOUNT
//!    holds in ACM, and one that does not is refused `InvalidParameterValue` `Certificate not
//!    found` — whether it is the server certificate or a mutual-authentication option's client
//!    root certificate chain;
//! 4. the closed value sets the model publishes (the VPN port, the session duration, how many
//!    DNS servers) are refused outside those sets, and so is a VPC or security group the
//!    account does not hold;
//! 5. a `ClientToken` makes the create idempotent, and `DryRun` stores nothing.
//!
//! Each table row is a distinct input along what its rule generalises over — the block size
//! and spelling, which authentication kind and how many, which optional members are set, which
//! bound of a published value set, and which part of the request names the unresolvable
//! resource — so no row is the specification.

use std::sync::Arc;

use service_com_amazonaws_acm_certificatemanager as acm;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    CLIENT_VPN_DEFAULT_IP_ADDRESS_TYPE, CLIENT_VPN_DEFAULT_PORT,
    CLIENT_VPN_DEFAULT_SESSION_TIMEOUT_HOURS, CLIENT_VPN_DEFAULT_TRANSPORT_PROTOCOL,
    CLIENT_VPN_ENDPOINT_ID_PREFIX, CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE,
    CLIENT_VPN_PROTOCOL_OPENVPN,
};

use super::ec2_query::{call, element, elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "CreateClientVpnEndpoint";

/// The `ClientVpnAuthenticationType` of the mutual-authentication option, which is the one
/// that names a second certificate.
const CERTIFICATE_AUTHENTICATION: &str = "certificate-authentication";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that adds members to [`base_params`] and expects a refusal code, or none
/// when the row is one the service must serve: the case, the members, and the code.
type ExtraRow = (&'static str, Params, Option<&'static str>);

/// One row of a table that adds members to [`base_params`] and expects values back: the case,
/// the members, and the values the response must carry.
type ExpectingRow = (&'static str, Params, Vec<&'static str>);

/// One row of a table that adds members to [`base_params`] and expects a refusal: the case, the
/// members, and the code.
type RefusalRow = (&'static str, Params, &'static str);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// EC2 and Certificate Manager over ONE shared engine, exactly as the assembled router mounts
/// them: the certificate an endpoint presents is a resource of that service, created through
/// its own API.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere; these
    // cases are about these services' behaviour.
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(acm::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One certificate created through Certificate Manager's OWN `RequestCertificate`, answering
/// the ARN that service minted for it — so what a create resolves is a certificate the owning
/// service holds, not one this suite made up.
async fn certificate(router: &MultiServiceRouter, case: &str, domain: &str) -> String {
    let body = ::serde_json::json!({ "DomainName": domain, "ValidationMethod": "DNS" });
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://acm.{REGION}.amazonaws.com/"))
        .header("host", format!("acm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "CertificateManager.RequestCertificate")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(::bytes::Bytes::from(
            ::serde_json::to_vec(&body).expect("the certificate request body serializes"),
        ))
        .expect("a well-formed certificate request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: ::serde_json::Value =
        ::serde_json::from_slice(response.body()).unwrap_or(::serde_json::Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: RequestCertificate must answer 200, got {status} with body {parsed}"
    );
    parsed["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {parsed}"))
        .to_string()
}

/// The members every create here carries unless a row says otherwise: a client range of a
/// published size, the server certificate, one authentication option and connection logging
/// off.
fn base_params(server_certificate_arn: &str, client_root_certificate_arn: &str) -> Params {
    vec![
        p("ClientCidrBlock", "10.100.0.0/22"),
        p("ServerCertificateArn", server_certificate_arn),
        p("Authentication.1.Type", CERTIFICATE_AUTHENTICATION),
        p(
            "Authentication.1.MutualAuthentication.ClientRootCertificateChainArn",
            client_root_certificate_arn,
        ),
        p("ConnectionLogOptions.Enabled", "false"),
    ]
}

/// One create call.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// `params` with every member of `extra` set, REPLACING any the base already carries — a form
/// body naming one parameter twice is a different request from the one a row describes.
fn with(params: &[(String, String)], extra: &[(String, String)]) -> Params {
    let mut merged: Params = params
        .iter()
        .filter(|(key, _)| !extra.iter().any(|(named, _)| named == key))
        .cloned()
        .collect();
    merged.extend(extra.iter().cloned());
    merged
}

/// One create that must succeed, answering the endpoint id it minted and the body it answered.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> (String, String) {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let endpoint_id = element(&body, "clientVpnEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no clientVpnEndpointId in {body}"));
    (endpoint_id, body)
}

/// The body `DescribeClientVpnEndpoints` answers for ONE endpoint.
async fn described(router: &MultiServiceRouter, case: &str, endpoint_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeClientVpnEndpoints",
        &[p("ClientVpnEndpointId.1", endpoint_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeClientVpnEndpoints must report {endpoint_id}, got {status} with \
         body {body}"
    );
    body
}

/// Every endpoint id the account holds, as the describe reports them.
async fn stored_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, REGION, "DescribeClientVpnEndpoints", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeClientVpnEndpoints must answer 200, got {status} with body {body}"
    );
    elements(&body, "clientVpnEndpointId")
}

/// One VPC of the account, through EC2's own `CreateVpc`.
async fn vpc(router: &MultiServiceRouter, case: &str, cidr_block: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateVpc",
        &[p("CidrBlock", cidr_block)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpc must answer 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"))
}

/// One security group of `vpc_id`, through EC2's own `CreateSecurityGroup`.
async fn security_group(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            p("GroupName", name),
            p("GroupDescription", "the Client VPN target network"),
            p("VpcId", vpc_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSecurityGroup must answer 200, got {status} with body {body}"
    );
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no groupId in {body}"))
}

// ---------------------------------------------------------------------------
// Rule 1: a create stores the endpoint the request describes
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the case, the members it adds to
/// [`base_params`], and the `(xmlName, value)` pairs the describe must report for it.
struct Row {
    case: &'static str,
    extra: Vec<(String, String)>,
    expected: Vec<(&'static str, String)>,
}

/// The rows the round trip is written against. They differ along the block size, the transport
/// protocol, the port, the session duration, how many DNS servers, whether split tunnel and
/// the session-timeout disconnect were asked for, the address types and the description — every
/// member a caller chooses that the describe reports back.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the published defaults",
            extra: Vec::new(),
            expected: vec![
                ("clientCidrBlock", "10.100.0.0/22".to_string()),
                (
                    "transportProtocol",
                    CLIENT_VPN_DEFAULT_TRANSPORT_PROTOCOL.to_string(),
                ),
                ("vpnPort", CLIENT_VPN_DEFAULT_PORT.to_string()),
                (
                    "sessionTimeoutHours",
                    CLIENT_VPN_DEFAULT_SESSION_TIMEOUT_HOURS.to_string(),
                ),
                ("splitTunnel", "false".to_string()),
                ("disconnectOnSessionTimeout", "true".to_string()),
                ("vpnProtocol", CLIENT_VPN_PROTOCOL_OPENVPN.to_string()),
                (
                    "endpointIpAddressType",
                    CLIENT_VPN_DEFAULT_IP_ADDRESS_TYPE.to_string(),
                ),
                (
                    "trafficIpAddressType",
                    CLIENT_VPN_DEFAULT_IP_ADDRESS_TYPE.to_string(),
                ),
            ],
        },
        Row {
            case: "the widest published range, over tcp on the alternate port",
            extra: vec![
                p("ClientCidrBlock", "10.0.0.0/12"),
                p("TransportProtocol", "tcp"),
                p("VpnPort", "1194"),
                p("Description", "the engineering endpoint"),
            ],
            expected: vec![
                ("clientCidrBlock", "10.0.0.0/12".to_string()),
                ("transportProtocol", "tcp".to_string()),
                ("vpnPort", "1194".to_string()),
                ("description", "the engineering endpoint".to_string()),
            ],
        },
        Row {
            case: "a split-tunnel endpoint with two DNS servers and the shortest session",
            extra: vec![
                p("ClientCidrBlock", "172.31.0.0/16"),
                p("SplitTunnel", "true"),
                p("DnsServers.1", "10.0.0.2"),
                p("DnsServers.2", "10.0.0.3"),
                p("SessionTimeoutHours", "8"),
                p("DisconnectOnSessionTimeout", "false"),
            ],
            expected: vec![
                ("clientCidrBlock", "172.31.0.0/16".to_string()),
                ("splitTunnel", "true".to_string()),
                ("sessionTimeoutHours", "8".to_string()),
                ("disconnectOnSessionTimeout", "false".to_string()),
            ],
        },
        Row {
            case: "connection logging to a log group and stream",
            extra: vec![
                p("ConnectionLogOptions.Enabled", "true"),
                p("ConnectionLogOptions.CloudwatchLogGroup", "client-vpn"),
                p("ConnectionLogOptions.CloudwatchLogStream", "connections"),
                p("SessionTimeoutHours", "12"),
            ],
            // `ConnectionLogResponseOptions`'s members carry no `xmlName` in this model
            // (`smithy_query(query="shape",shape="ConnectionLogResponseOptions")`), so they
            // reach the wire under their member names.
            expected: vec![
                ("CloudwatchLogGroup", "client-vpn".to_string()),
                ("CloudwatchLogStream", "connections".to_string()),
                ("sessionTimeoutHours", "12".to_string()),
            ],
        },
        Row {
            case: "a dual-stack endpoint with a login banner",
            extra: vec![
                p("EndpointIpAddressType", "dual-stack"),
                p("TrafficIpAddressType", "dual-stack"),
                p("ClientLoginBannerOptions.Enabled", "true"),
                p("ClientLoginBannerOptions.BannerText", "authorized use only"),
                p("ClientRouteEnforcementOptions.Enforced", "true"),
            ],
            expected: vec![
                ("endpointIpAddressType", "dual-stack".to_string()),
                ("trafficIpAddressType", "dual-stack".to_string()),
                ("bannerText", "authorized use only".to_string()),
                ("enforced", "true".to_string()),
            ],
        },
    ]
}

/// Every member a create stored is reported by the describe, and the endpoint it answers is
/// the endpoint the account holds.
#[tokio::test]
async fn a_created_endpoint_round_trips_through_the_describe() {
    for row in rows() {
        let router = emulator();
        let server = certificate(&router, row.case, "vpn.example.com").await;
        let client_root = certificate(&router, row.case, "clients.example.com").await;
        let mut params = with(&base_params(&server, &client_root), &row.extra);
        params.push(p("TagSpecification.1.ResourceType", "client-vpn-endpoint"));
        params.push(p("TagSpecification.1.Tag.1.Key", "Name"));
        params.push(p("TagSpecification.1.Tag.1.Value", "remote-access"));

        let (endpoint_id, body) = created(&router, row.case, &params).await;
        assert!(
            endpoint_id.starts_with(&format!("{CLIENT_VPN_ENDPOINT_ID_PREFIX}-")),
            "case {:?}: expected an id under {CLIENT_VPN_ENDPOINT_ID_PREFIX}-, got \
             {endpoint_id:?}",
            row.case
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE),
            "case {:?}: an endpoint with no target network must be reported \
             {CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE}, got {body}",
            row.case
        );
        let expected_dns_name = format!("{endpoint_id}.prod.clientvpn.{REGION}.amazonaws.com");
        assert_eq!(
            element(&body, "dnsName").as_deref(),
            Some(expected_dns_name.as_str()),
            "case {:?}: expected the DNS name {expected_dns_name:?}, got {body}",
            row.case
        );

        // The create and the describe report ONE endpoint: every member the request chose, and
        // the id, status and DNS name the service minted for it.
        let reported = described(&router, row.case, &endpoint_id).await;
        assert_eq!(
            elements(&reported, "clientVpnEndpointId"),
            vec![endpoint_id.clone()],
            "case {:?}: expected exactly the endpoint created, got {reported}",
            row.case
        );
        for (xml_name, value) in row
            .expected
            .iter()
            .map(|(name, value)| (*name, value.clone()))
            .chain([
                ("dnsName", expected_dns_name.clone()),
                ("serverCertificateArn", server.clone()),
                ("clientRootCertificateChain", client_root.clone()),
                ("type", CERTIFICATE_AUTHENTICATION.to_string()),
                ("code", CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE.to_string()),
                ("key", "Name".to_string()),
                ("value", "remote-access".to_string()),
            ])
        {
            assert_eq!(
                element(&reported, xml_name).as_deref(),
                Some(value.as_str()),
                "case {:?}: expected <{xml_name}>{value}</{xml_name}>, got {reported}",
                row.case
            );
        }
        // A member the request named as a LIST is reported in full.
        for dns_server in row
            .extra
            .iter()
            .filter(|(key, _)| key.starts_with("DnsServers."))
            .map(|(_, value)| value.clone())
        {
            assert!(
                reported.contains(&format!("<item>{dns_server}</item>")),
                "case {:?}: expected the DNS server {dns_server} to be reported, got {reported}",
                row.case
            );
        }
        assert!(
            !reported.contains("<creationTime></creationTime>")
                && element(&reported, "creationTime").is_some(),
            "case {:?}: expected the endpoint to report when it was created, got {reported}",
            row.case
        );
    }
}

/// Every authentication kind the model declares is stored and reported with the thing it
/// authenticates against, including two options on one endpoint.
#[tokio::test]
async fn each_authentication_kind_is_stored_as_it_was_given() {
    // The kinds come from `smithy_query(query="shape",shape="ClientVpnAuthenticationType")`:
    // certificate-authentication, directory-service-authentication, federated-authentication.
    let cases: Vec<ExpectingRow> = vec![
        (
            "an Active Directory",
            vec![
                p("Authentication.1.Type", "directory-service-authentication"),
                p("Authentication.1.ActiveDirectory.DirectoryId", "d-926example"),
            ],
            vec!["directory-service-authentication", "d-926example"],
        ),
        (
            "a SAML provider",
            vec![
                p("Authentication.1.Type", "federated-authentication"),
                p(
                    "Authentication.1.FederatedAuthentication.SAMLProviderArn",
                    "arn:aws:iam::000000000000:saml-provider/clients",
                ),
            ],
            vec![
                "federated-authentication",
                "arn:aws:iam::000000000000:saml-provider/clients",
            ],
        ),
    ];
    for (case, options, expected) in cases {
        let router = emulator();
        let server = certificate(&router, case, "vpn.example.com").await;
        let mut params = vec![
            p("ClientCidrBlock", "10.100.0.0/22"),
            p("ServerCertificateArn", &server),
            p("ConnectionLogOptions.Enabled", "false"),
        ];
        params.extend(options);
        let (endpoint_id, _) = created(&router, case, &params).await;
        let reported = described(&router, case, &endpoint_id).await;
        for value in expected {
            assert!(
                reported.contains(&format!(">{value}<")),
                "case {case:?}: expected {value} to be reported, got {reported}"
            );
        }
    }

    // Two options on one endpoint: both are stored, in the order they were given — and a
    // SECOND option of a kind the request already named is refused, because an endpoint
    // presents one certificate chain, joins one directory and trusts one SAML provider.
    let case = "two authentication options";
    let router = emulator();
    let server = certificate(&router, case, "vpn.example.com").await;
    let client_root = certificate(&router, case, "clients.example.com").await;
    let mut params = base_params(&server, &client_root);
    params.push(p("Authentication.2.Type", "federated-authentication"));
    params.push(p(
        "Authentication.2.FederatedAuthentication.SAMLProviderArn",
        "arn:aws:iam::000000000000:saml-provider/clients",
    ));
    let (endpoint_id, _) = created(&router, case, &params).await;
    let reported = described(&router, case, &endpoint_id).await;
    assert_eq!(
        elements(&reported, "type"),
        vec![
            CERTIFICATE_AUTHENTICATION.to_string(),
            "federated-authentication".to_string(),
        ],
        "case {case:?}: expected both options in the order given, got {reported}"
    );

    let case = "two options of one kind";
    let mut repeated = base_params(&server, &client_root);
    repeated.push(p("Authentication.2.Type", CERTIFICATE_AUTHENTICATION));
    repeated.push(p(
        "Authentication.2.MutualAuthentication.ClientRootCertificateChainArn",
        &client_root,
    ));
    let (status, body) = create(&router, &repeated).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "case {case:?}: expected 400 InvalidParameterValue, got {status} with body {body}"
    );
    assert_eq!(
        stored_ids(&router, case).await,
        vec![endpoint_id],
        "case {case:?}: a refused create must store nothing"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the client CIDR range
// ---------------------------------------------------------------------------

/// The message AWS words the published size rule with
/// (`CreateClientVpnEndpointRequest$ClientCidrBlock`: "Client CIDR range must have a size of at
/// least /22 and must not be greater than /12"), spelled here independently of the handler.
const CIDR_SIZE_MESSAGE: &str =
    "Client CIDR range must have a size of at least /22 and must not be greater than /12";

/// Both published bounds are accepted, and one past each is refused with AWS's own sentence.
#[tokio::test]
async fn the_client_range_must_be_of_a_published_size() {
    // Rows span the two published bounds, the sizes between them, and one past each bound.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("the smallest published range", "10.0.0.0/22", true),
        ("a range between the bounds", "10.0.0.0/18", true),
        ("the largest published range", "10.0.0.0/12", true),
        ("one size past the small bound", "10.0.0.0/23", false),
        ("far past the small bound", "10.0.0.0/32", false),
        ("one size past the large bound", "10.0.0.0/11", false),
        ("far past the large bound", "0.0.0.0/0", false),
    ];
    for (case, block, accepted) in rows {
        let router = emulator();
        let server = certificate(&router, case, "vpn.example.com").await;
        let client_root = certificate(&router, case, "clients.example.com").await;
        let params = with(
            &base_params(&server, &client_root),
            &[p("ClientCidrBlock", block)],
        );

        let (status, body) = create(&router, &params).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {block} is a published size, got {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValue"),
            "case {case:?}: expected 400 InvalidParameterValue for {block}, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_message(&body),
            CIDR_SIZE_MESSAGE,
            "case {case:?}: expected AWS's own sentence for {block}, got {:?}",
            error_message(&body)
        );
        assert!(
            stored_ids(&router, case).await.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// A value that is not a client range at all is refused on its own terms, and nothing is
/// stored.
#[tokio::test]
async fn a_value_that_is_not_a_client_range_is_refused() {
    // Rows span the forms a value can fail in: absent, empty, not a block, an address inside a
    // range rather than the range, and a prefix no address has.
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no client range at all", None, "MissingParameter"),
        ("an empty client range", Some(""), "MissingParameter"),
        ("an address with no prefix", Some("10.0.0.0"), "InvalidParameterValue"),
        ("text", Some("ten dot nothing"), "InvalidParameterValue"),
        (
            "an address inside a range",
            Some("10.0.0.1/22"),
            "InvalidParameterValue",
        ),
        ("a prefix past the address", Some("10.0.0.0/33"), "InvalidParameterValue"),
        ("an IPv6 range", Some("2001:db8::/32"), "InvalidParameterValue"),
    ];
    for (case, block, expected_code) in rows {
        let router = emulator();
        let server = certificate(&router, case, "vpn.example.com").await;
        let client_root = certificate(&router, case, "clients.example.com").await;
        let mut params = base_params(&server, &client_root);
        params.retain(|(key, _)| key != "ClientCidrBlock");
        if let Some(block) = block {
            params.push(p("ClientCidrBlock", block));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with body {body}"
        );
        assert!(
            stored_ids(&router, case).await.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the certificates are ACM's
// ---------------------------------------------------------------------------

/// The message this surface answers for a certificate the account does not hold.
const CERTIFICATE_NOT_FOUND: &str = "Certificate not found";

/// A certificate ARN ACM holds nothing under is refused, whichever member names it — and the
/// same request with certificates that service does hold is served.
#[tokio::test]
async fn a_certificate_the_account_does_not_hold_is_refused() {
    // Rows span the ways an ARN can name no certificate of the account: an id nothing was
    // created under, another Region's, another account's, another partition's, another
    // service's resource, and a value that is not an ARN at all — for each of the two members
    // that name a certificate.
    let absent: Vec<(&str, &str)> = vec![
        (
            "an id nothing was created under",
            "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555",
        ),
        (
            "another Region's certificate",
            "arn:aws:acm:ap-southeast-2:000000000000:certificate/0123abcd-ef01-2345-6789-abcdef012345",
        ),
        (
            "another account's certificate",
            "arn:aws:acm:us-east-1:123456789012:certificate/0123abcd-ef01-2345-6789-abcdef012345",
        ),
        (
            "another partition's certificate",
            "arn:aws-us-gov:acm:us-gov-west-1:000000000000:certificate/0123abcd-ef01-2345-6789-abcdef012345",
        ),
        (
            "another service's resource",
            "arn:aws:iam::000000000000:server-certificate/clients",
        ),
        ("a value that is not an ARN", "not-a-certificate"),
    ];
    for (case, arn) in absent {
        let router = emulator();
        let held = certificate(&router, case, "vpn.example.com").await;

        // The SERVER certificate is the one every endpoint presents.
        let mut params = base_params(arn, &held);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, "InvalidParameterValue", CERTIFICATE_NOT_FOUND.to_string()),
            "case {case:?} as the server certificate: expected 400 InvalidParameterValue \
             {CERTIFICATE_NOT_FOUND:?}, got {status} with body {body}"
        );

        // And so is the client root certificate chain a mutual-authentication option names.
        params = base_params(&held, arn);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, "InvalidParameterValue", CERTIFICATE_NOT_FOUND.to_string()),
            "case {case:?} as the client root certificate chain: expected 400 \
             InvalidParameterValue {CERTIFICATE_NOT_FOUND:?}, got {status} with body {body}"
        );
        assert!(
            stored_ids(&router, case).await.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );

        // The same request against certificates that service DOES hold is served, so the
        // refusal is about the certificate and nothing else.
        let client_root = certificate(&router, case, "clients.example.com").await;
        let (endpoint_id, _) = created(&router, case, &base_params(&held, &client_root)).await;
        assert_eq!(
            stored_ids(&router, case).await,
            vec![endpoint_id],
            "case {case:?}: expected the endpoint created against held certificates"
        );
    }

    // A request that carries NO server certificate, or sends its ARN empty, names no
    // certificate of the account either, so it is answered the same way — while an
    // authentication option that names no certificate chain AT ALL is reported as the parameter
    // it left out, which is a different fix for the caller.
    let case = "an empty server certificate";
    let router = emulator();
    let held = certificate(&router, case, "vpn.example.com").await;
    let (status, body) = create(&router, &base_params("", &held)).await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (
            400,
            "InvalidParameterValue",
            CERTIFICATE_NOT_FOUND.to_string()
        ),
        "case {case:?}: expected 400 InvalidParameterValue {CERTIFICATE_NOT_FOUND:?}, got \
         {status} with body {body}"
    );
    for (case, chain) in [
        ("a mutual-authentication option with no chain", None),
        ("a mutual-authentication option with an empty chain", Some("")),
    ] {
        let mut params = base_params(&held, "");
        params.retain(|(key, _)| {
            key != "Authentication.1.MutualAuthentication.ClientRootCertificateChainArn"
        });
        if let Some(chain) = chain {
            params.push(p(
                "Authentication.1.MutualAuthentication.ClientRootCertificateChainArn",
                chain,
            ));
        }
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "MissingParameter"),
            "case {case:?}: expected 400 MissingParameter, got {status} with body {body}"
        );
    }
    assert!(
        stored_ids(&router, case).await.is_empty(),
        "case {case:?}: a refused create must store nothing"
    );
}

/// A create that leaves out a parameter the operation requires is refused and stores nothing —
/// whichever required member it is.
///
/// The refusal's CODE is the boundary validator's rather than this operation's: every
/// `@required` member of the request is refused before the handler runs, so what this pins is
/// that such a request is refused and persists nothing.
#[tokio::test]
async fn a_request_missing_a_required_parameter_is_refused() {
    // Rows: each `@required` member of the request, left out entirely.
    let rows: Vec<(&str, &str)> = vec![
        ("no authentication options", "Authentication.1."),
        ("no connection log options", "ConnectionLogOptions."),
        ("no server certificate", "ServerCertificateArn"),
    ];
    for (case, dropped) in rows {
        let router = emulator();
        let held = certificate(&router, case, "vpn.example.com").await;
        let params: Params = base_params(&held, &held)
            .into_iter()
            .filter(|(key, _)| !key.starts_with(dropped))
            .collect();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert!(
            stored_ids(&router, case).await.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// A certificate created AFTER a refused create is resolvable, so the handler asks the owning
/// service rather than remembering an answer.
#[tokio::test]
async fn a_certificate_created_after_a_refusal_resolves() {
    let case = "a certificate imported between two creates";
    let router = emulator();
    let held = certificate(&router, case, "vpn.example.com").await;
    // The ARN of a certificate the account does not hold yet: a second certificate's ARN,
    // taken from the service that mints them, after the first create was refused for it.
    let (status, body) = create(&router, &base_params(&held, &held)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: one certificate may serve both members, got {status} with body {body}"
    );
    let later = certificate(&router, case, "clients.example.com").await;
    let (_, body) = created(&router, case, &base_params(&held, &later)).await;
    assert!(
        !body.is_empty(),
        "case {case:?}: the later certificate must resolve too"
    );
    assert_eq!(
        stored_ids(&router, case).await.len(),
        2,
        "case {case:?}: expected both endpoints to be stored"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the published value sets, and the resources the request names
// ---------------------------------------------------------------------------

/// Each closed value set the model publishes is served inside it and refused outside it.
#[tokio::test]
async fn the_published_value_sets_are_held_to() {
    // `VpnPort` ("Valid Values: 443 | 1194"), `SessionTimeoutHours` ("Valid values: 8 | 10 |
    // 12 | 24") and `DnsServers` ("up to two DNS servers"), each at a published value, past
    // its set, and — for the DNS servers — with a value that is not an address.
    let rows: Vec<ExtraRow> = vec![
        ("the other published port", vec![p("VpnPort", "1194")], None),
        (
            "a port outside the set",
            vec![p("VpnPort", "1195")],
            Some("InvalidParameterValue"),
        ),
        (
            "a port nothing listens on",
            vec![p("VpnPort", "0")],
            Some("InvalidParameterValue"),
        ),
        (
            "every published session duration",
            vec![p("SessionTimeoutHours", "10")],
            None,
        ),
        (
            "a session duration outside the set",
            vec![p("SessionTimeoutHours", "9")],
            Some("InvalidParameterValue"),
        ),
        (
            "the most DNS servers an endpoint may have",
            vec![p("DnsServers.1", "10.0.0.2"), p("DnsServers.2", "10.0.0.3")],
            None,
        ),
        (
            "one DNS server too many",
            vec![
                p("DnsServers.1", "10.0.0.2"),
                p("DnsServers.2", "10.0.0.3"),
                p("DnsServers.3", "10.0.0.4"),
            ],
            Some("InvalidParameterValue"),
        ),
        (
            "a DNS server that is not an address",
            vec![p("DnsServers.1", "resolver.example.com")],
            Some("InvalidParameterValue"),
        ),
        (
            "connection logging with nowhere to log to",
            vec![p("ConnectionLogOptions.Enabled", "true")],
            Some("MissingParameter"),
        ),
    ];
    for (case, extra, expected_code) in rows {
        let router = emulator();
        let server = certificate(&router, case, "vpn.example.com").await;
        let client_root = certificate(&router, case, "clients.example.com").await;
        let params = with(&base_params(&server, &client_root), &extra);

        let (status, body) = create(&router, &params).await;
        match expected_code {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a published value must be served, got {status} with body {body}"
            ),
            Some(code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, code),
                    "case {case:?}: expected 400 {code}, got {status} with body {body}"
                );
                assert!(
                    stored_ids(&router, case).await.is_empty(),
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

/// The VPC and the security groups are resolved against the account: the endpoint reports the
/// groups it was given, the VPC's default group when it was given none, and an unresolvable
/// reference is refused.
#[tokio::test]
async fn the_vpc_and_its_security_groups_are_resolved() {
    let case = "a VPC of the account";
    let router = emulator();
    let server = certificate(&router, case, "vpn.example.com").await;
    let client_root = certificate(&router, case, "clients.example.com").await;
    let vpc_id = vpc(&router, case, "10.9.0.0/16").await;
    let group_id = security_group(&router, case, &vpc_id, "client-vpn").await;
    let other_vpc = vpc(&router, case, "10.10.0.0/16").await;
    let other_group = security_group(&router, case, &other_vpc, "elsewhere").await;

    // A VPC with no group named: the VPC's own default group applies.
    let mut params = base_params(&server, &client_root);
    params.push(p("VpcId", &vpc_id));
    let (endpoint_id, _) = created(&router, case, &params).await;
    let reported = described(&router, case, &endpoint_id).await;
    assert_eq!(
        element(&reported, "vpcId").as_deref(),
        Some(vpc_id.as_str()),
        "case {case:?}: expected the endpoint in {vpc_id}, got {reported}"
    );
    assert!(
        reported.contains("<securityGroupIdSet>") && reported.contains("<item>sg-"),
        "case {case:?}: expected the VPC's default security group to be applied, got {reported}"
    );

    // A group of that VPC named explicitly is the group applied.
    let mut params = base_params(&server, &client_root);
    params.push(p("VpcId", &vpc_id));
    params.push(p("SecurityGroupId.1", &group_id));
    let (endpoint_id, _) = created(&router, case, &params).await;
    let reported = described(&router, case, &endpoint_id).await;
    assert!(
        reported.contains(&format!("<item>{group_id}</item>")),
        "case {case:?}: expected the named group {group_id} to be applied, got {reported}"
    );

    // Each unresolvable reference is refused on its own terms, and stores nothing.
    let refusals: Vec<RefusalRow> = vec![
        (
            "a VPC the account does not hold",
            vec![p("VpcId", "vpc-0fedcba987654321f")],
            "InvalidVpcID.NotFound",
        ),
        (
            "a security group the account does not hold",
            vec![
                p("VpcId", &vpc_id),
                p("SecurityGroupId.1", "sg-0fedcba987654321f"),
            ],
            "InvalidGroup.NotFound",
        ),
        (
            "a security group of another VPC",
            vec![p("VpcId", &vpc_id), p("SecurityGroupId.1", &other_group)],
            "InvalidParameterValue",
        ),
        (
            "security groups with no VPC",
            vec![p("SecurityGroupId.1", &group_id)],
            "InvalidParameterCombination",
        ),
    ];
    let before = stored_ids(&router, case).await;
    for (case, extra, expected_code) in refusals {
        let mut params = base_params(&server, &client_root);
        params.extend(extra);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with body {body}"
        );
        assert_eq!(
            stored_ids(&router, case).await,
            before,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the client token, and the dry run
// ---------------------------------------------------------------------------

/// One token names the one endpoint it created: a replay reports that endpoint and creates
/// nothing, and a token reused with other members is refused.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let case = "a replayed client token";
    let router = emulator();
    let server = certificate(&router, case, "vpn.example.com").await;
    let client_root = certificate(&router, case, "clients.example.com").await;
    let mut params = base_params(&server, &client_root);
    params.push(p("ClientToken", "one-request"));
    params.push(p("Description", "the shared endpoint"));

    let (endpoint_id, first) = created(&router, case, &params).await;
    let (replayed_id, replayed) = created(&router, case, &params).await;
    assert_eq!(
        (replayed_id.as_str(), element(&replayed, "dnsName")),
        (endpoint_id.as_str(), element(&first, "dnsName")),
        "case {case:?}: a replay must report the endpoint the token created, got {replayed}"
    );
    assert_eq!(
        stored_ids(&router, case).await,
        vec![endpoint_id.clone()],
        "case {case:?}: a replay must create nothing"
    );

    // The same token with other members is not the same request.
    let mut different = params.clone();
    different.retain(|(key, _)| key != "Description");
    different.push(p("Description", "another endpoint"));
    let (status, body) = create(&router, &different).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IdempotentParameterMismatch"),
        "case {case:?}: expected 400 IdempotentParameterMismatch, got {status} with body {body}"
    );
    assert_eq!(
        stored_ids(&router, case).await,
        vec![endpoint_id],
        "case {case:?}: a refused replay must create nothing"
    );

    // A DIFFERENT token is a different request, so it creates a second endpoint.
    let mut second = params.clone();
    second.retain(|(key, _)| key != "ClientToken");
    second.push(p("ClientToken", "another-request"));
    let (other_id, _) = created(&router, case, &second).await;
    assert_eq!(
        stored_ids(&router, case).await.len(),
        2,
        "case {case:?}: a second token must create a second endpoint, not {other_id}"
    );
}

/// A dry run reports the outcome and stores nothing, and a dry run of a request that would
/// have been refused is refused with that reason instead.
#[tokio::test]
async fn a_dry_run_creates_nothing() {
    let case = "a dry run";
    let router = emulator();
    let server = certificate(&router, case, "vpn.example.com").await;
    let client_root = certificate(&router, case, "clients.example.com").await;
    let mut params = base_params(&server, &client_root);
    params.push(p("DryRun", "true"));

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "case {case:?}: expected 412 DryRunOperation, got {status} with body {body}"
    );
    assert!(
        stored_ids(&router, case).await.is_empty(),
        "case {case:?}: a dry run must store nothing"
    );

    // The flag is answered BEFORE the request's other members are weighed, as the sibling
    // `CreateClientVpnRoute` answers its own: a dry run asks whether the caller MAY make the
    // call, so one carrying members the service never got to is still answered for the flag.
    let unpublished_size = with(&params, &[p("ClientCidrBlock", "10.0.0.0/24")]);
    let (status, body) = create(&router, &unpublished_size).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "case {case:?} with an unpublished size: expected the flag to be answered first, got \
         {status} with body {body}"
    );
    // Without the flag the same request is refused on its own terms.
    let refused = with(
        &unpublished_size,
        &[p("DryRun", "false"), p("ClientCidrBlock", "10.0.0.0/24")],
    );
    let (status, body) = create(&router, &refused).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "case {case:?} without the flag: expected the size refusal, got {status} with body {body}"
    );
    assert!(
        stored_ids(&router, case).await.is_empty(),
        "case {case:?}: a refused dry run must store nothing"
    );
}
