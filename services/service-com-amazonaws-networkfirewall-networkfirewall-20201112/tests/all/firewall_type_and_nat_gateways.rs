// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two members the model update added to the firewall
//! operations: `FirewallType`, which names which KIND of firewall a request
//! targets, and the NAT gateway members (`NatGatewayCreateMappings`,
//! `DeleteAssociatedNatGateways`) a proxy mode firewall's gateways are created
//! and deleted through.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{error_code, error_message, string_at, Harness, NOT_FOUND_MESSAGE};

/// The error code a bad request carries.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The error code a miss carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The refusal both NAT gateway members carry, as the model states the rule on
/// each of them.
const NOT_PROXY_MODE: &str = "You can't specify this for a firewall that isn't a proxy mode firewall.";

/// Create a firewall named `name` on a policy of its own, and return the policy
/// ARN so a caller can name it again.
async fn create_firewall(harness: &Harness, name: &str) -> String {
    let policy_arn = harness.shared_policy(&format!("{name}-policy")).await;
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": format!("subnet-{name}") }],
            }),
        )
        .await;
    policy_arn
}

/// RULE: `FirewallType` is part of the firewall a request identifies — "if you
/// don't specify a type, the default is `FIREWALL`" — so a request naming
/// `IGW_FIREWALL`, the internet gateway firewall kind Amazon Web Services
/// Network Security Manager creates and this service holds none of, identifies no
/// firewall and is answered with the modelled miss, never with the standard
/// firewall of that name.
///
/// The rows span every operation that takes the member and resolves a firewall
/// through it, each with the other members that operation requires.
#[tokio::test]
async fn an_internet_gateway_firewall_type_identifies_no_firewall_here() {
    let harness = Harness::new();
    let policy_arn = create_firewall(&harness, "igw-typed").await;
    let token = string_at(
        &harness
            .ok("DescribeFirewall", &json!({ "FirewallName": "igw-typed" }))
            .await,
        &["UpdateToken"],
    )
    .to_string();

    // (operation, the members it needs beside `FirewallName` and `FirewallType`)
    let rows: [(&str, Value); 9] = [
        ("DescribeFirewall", json!({})),
        ("DescribeLoggingConfiguration", json!({})),
        (
            "AssociateFirewallPolicy",
            json!({ "FirewallPolicyArn": policy_arn, "UpdateToken": token }),
        ),
        (
            "UpdateFirewallDeleteProtection",
            json!({ "DeleteProtection": true, "UpdateToken": token }),
        ),
        (
            "UpdateFirewallDescription",
            json!({ "Description": "changed", "UpdateToken": token }),
        ),
        (
            "UpdateFirewallAnalysisSettings",
            json!({ "EnabledAnalysisTypes": ["TLS_SNI"], "UpdateToken": token }),
        ),
        (
            "UpdateFirewallPolicyChangeProtection",
            json!({ "FirewallPolicyChangeProtection": true, "UpdateToken": token }),
        ),
        ("UpdateLoggingConfiguration", json!({})),
        ("DeleteFirewall", json!({})),
    ];

    for (operation, members) in rows {
        let mut request = json!({ "FirewallName": "igw-typed", "FirewallType": "IGW_FIREWALL" });
        for (member, value) in members.as_object().expect("the row's members") {
            request[member] = value.clone();
        }
        let (status, body) = harness.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (400, NOT_FOUND, NOT_FOUND_MESSAGE),
            "case {operation}: expected the internet gateway firewall kind to name no firewall, \
             got {status} {body:?}"
        );
    }

    // Every one of those requests left the standard firewall of that name
    // exactly as it was: still there, still with the token and the settings the
    // create gave it.
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "igw-typed" }))
        .await;
    assert_eq!(
        (
            described["UpdateToken"].as_str(),
            described["Firewall"]["Description"].as_str(),
            described["Firewall"]["DeleteProtection"].as_bool(),
        ),
        (Some(token.as_str()), None, Some(false)),
        "expected the refused requests to change nothing, got {described:?}"
    );

    // The standard kind, named or defaulted, still identifies it.
    for firewall_type in [None, Some("FIREWALL")] {
        let request = match firewall_type {
            Some(kind) => json!({ "FirewallName": "igw-typed", "FirewallType": kind }),
            None => json!({ "FirewallName": "igw-typed" }),
        };
        let read = harness.ok("DescribeFirewall", &request).await;
        assert_eq!(
            read["Firewall"]["FirewallName"],
            json!("igw-typed"),
            "case {firewall_type:?}: expected the standard firewall, got {read:?}"
        );
    }
}

/// RULE: `ListFirewalls$FirewallType` "specifies the firewall type to filter the
/// results by", and "if you don't specify a type, the default is to return
/// firewalls of both types" — so the listing reports a stored firewall for the
/// standard type and for no type at all, and reports none for the internet
/// gateway type this service holds none of.
///
/// The rows span the three values the member can arrive as, against a store that
/// holds two firewalls.
#[tokio::test]
async fn the_firewall_listing_filters_by_the_firewall_type() {
    let harness = Harness::new();
    for name in ["listed-one", "listed-two"] {
        create_firewall(&harness, name).await;
    }

    // (case, the request, the names the listing reports)
    let rows: [(&str, Value, Vec<&str>); 3] = [
        ("no type at all", json!({}), vec!["listed-one", "listed-two"]),
        (
            "the standard type",
            json!({ "FirewallType": "FIREWALL" }),
            vec!["listed-one", "listed-two"],
        ),
        (
            "the internet gateway type",
            json!({ "FirewallType": "IGW_FIREWALL" }),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in rows {
        let listed = harness.ok("ListFirewalls", &request).await;
        let mut names: Vec<String> = listed["Firewalls"]
            .as_array()
            .unwrap_or_else(|| panic!("expected Firewalls in {listed:?}"))
            .iter()
            .map(|entry| entry["FirewallName"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names, expected,
            "case {case}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: the NAT gateways a request asks this service to create or delete belong
/// to a proxy mode firewall — "you can't specify this for a firewall that isn't a
/// proxy mode firewall" — and every firewall this service holds is a
/// firewall-endpoint one, so such a request is refused as the bad request it is
/// and the firewall it named is left untouched.
///
/// The rows span both members: the mappings an association adds, and the delete's
/// own flag; and, on the other side, the flag's documented default, which deletes
/// the firewall as before.
#[tokio::test]
async fn nat_gateway_members_are_refused_for_a_firewall_that_is_not_proxy_mode() {
    let harness = Harness::new();
    create_firewall(&harness, "nat-gateways").await;

    // (case, the operation, the request)
    let rows: [(&str, &str, Value); 3] = [
        (
            "one mapping on an association",
            "AssociateSubnets",
            json!({
                "FirewallName": "nat-gateways",
                "SubnetMappings": [{ "SubnetId": "subnet-added-one" }],
                "NatGatewayCreateMappings": [{
                    "SubnetId": "subnet-nat-one",
                    "PrivateIps": ["10.0.1.10"],
                }],
            }),
        ),
        (
            "two mappings on an association",
            "AssociateSubnets",
            json!({
                "FirewallName": "nat-gateways",
                "SubnetMappings": [{ "SubnetId": "subnet-added-two" }],
                "NatGatewayCreateMappings": [
                    { "SubnetId": "subnet-nat-two", "PrivateIps": ["10.0.2.10"] },
                    {
                        "SubnetId": "subnet-nat-three",
                        "AllocationIds": ["eipalloc-0123abcd"],
                        "PrivateIps": ["10.0.3.10"],
                    },
                ],
            }),
        ),
        (
            "the delete's own flag",
            "DeleteFirewall",
            json!({ "FirewallName": "nat-gateways", "DeleteAssociatedNatGateways": true }),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = harness.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body).as_str()),
            (400, INVALID_REQUEST, NOT_PROXY_MODE),
            "case {case}: expected the NAT gateway request refused, got {status} {body:?}"
        );

        // The firewall is still there, with the one subnet the create gave it:
        // neither the association nor the delete happened.
        let described = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": "nat-gateways" }))
            .await;
        assert_eq!(
            described["Firewall"]["SubnetMappings"],
            json!([{ "SubnetId": "subnet-nat-gateways" }]),
            "case {case}: expected the firewall untouched, got {described:?}"
        );
    }

    // The flag's documented default — "Network Firewall detaches the NAT gateways
    // from the firewall and keeps them. This is the default" — is the delete this
    // service serves.
    for request in [
        json!({ "FirewallName": "nat-gateways", "DeleteAssociatedNatGateways": false }),
        json!({ "FirewallName": "nat-gateways" }),
    ] {
        let (status, body) = harness.call("DeleteFirewall", &request).await;
        // The first row deletes the firewall; the second then misses, which is
        // the proof the first one landed.
        assert!(
            status == 200 || (status, error_code(&body).as_str()) == (400, NOT_FOUND),
            "case {request}: expected the delete to be served, got {status} {body:?}"
        );
    }
    let (status, body) = harness
        .call("DescribeFirewall", &json!({ "FirewallName": "nat-gateways" }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "expected the firewall deleted by the default-flag request, got {status} {body:?}"
    );
}
