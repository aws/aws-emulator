// SPDX-License-Identifier: CC0-1.0

//! The POLICY a VPC endpoint carries over the whole round trip a client makes, driven over
//! `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the `PolicyDocument` a VPC endpoint reports is the document LAST
//! INSTALLED on it — the create's own document, byte for byte, when its request named one,
//! otherwise the default document the service installs for that endpoint TYPE, which allows
//! full access to the service; `ModifyVpcEndpoint` with `PolicyDocument` replaces it with
//! exactly the bytes that request carried, and with `ResetPolicy` puts the type's default
//! back. Each of those is reported by `DescribeVpcEndpoints` from the moment the call that
//! installed it answered, and the endpoint reaches the terminal `available` state, so a client
//! polling the describe for any of them terminates instead of waiting forever.
//!
//! The rows differ along what that rule generalises over: the endpoint TYPE (a type whose
//! default document differs, and which is created in a different state), which endpoint
//! service of the Region, how many of the VPC's route tables a gateway endpoint is reached
//! from, whether the create named a document at all, and how the documents are laid out
//! (compact, pretty-printed with its keys in another order, several statements, a statement
//! carrying a condition) — because the document is BYTES the caller chose rather than a form
//! the service imposes.
//!
//! Nothing here compares a default document against one spelling written out in this file:
//! the document the service installs is read back off an endpoint whose create named none,
//! and it is that document a reset has to put back — asserted to be JSON granting full access
//! ("we attach a default policy that allows full access to the service",
//! `CreateVpcEndpointRequest$PolicyDocument`).
//!
//! Siblings, each pinning one leg of that rule rather than the trip: `create_vpc_endpoint.rs`
//! (the document a create installs, per type, and the default when its request named none),
//! `modify_vpc_endpoint.rs` (a document replaced and reset on ONE gateway endpoint, among
//! every other member a modify changes), `vpc_endpoint_id_after_deletion.rs` (the document
//! surviving a describe of a deleted-and-recreated endpoint) and
//! `gateway_endpoint_availability.rs` (the state a gateway endpoint is created in). What is
//! this suite's own is the TRIP a client makes across all three calls, over endpoint type x
//! endpoint service x route-table count, which is what the recorded poll could not finish.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, emulator, indexed, unescaped};
use super::wire::{param, Param};

/// The Region every call here is signed for, and the Region of the endpoint services below.
const REGION: &str = "us-east-1";

/// How long a poll waits for the state the clock owes an endpoint before giving up. Far past
/// [`ec2::state::TRANSITION_DELAY`], so a loaded or slow host cannot fail the assertion: the
/// loop waits for the CONDITION and this is only its ceiling.
const CONVERGENCE_DEADLINE: Duration = Duration::from_secs(30);

/// How long each turn of a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, with the two DNS attributes an interface endpoint's private hosted
/// zone needs turned on — "to use private DNS, the VPC must have DNS hostnames and DNS
/// resolution enabled" (`CreateVpcEndpointRequest$PrivateDnsEnabled`) — so every row is about
/// the policy rather than about the VPC's DNS attributes.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    let vpc_id = member(case, &body, "vpcId");
    for attribute in ["EnableDnsSupport", "EnableDnsHostnames"] {
        ok(
            case,
            router,
            "ModifyVpcAttribute",
            &[
                param("VpcId", &vpc_id),
                param(&format!("{attribute}.Value"), "true"),
            ],
        )
        .await;
    }
    vpc_id
}

/// A route table of `vpc_id`, as its id.
async fn create_route_table(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(case, router, "CreateRouteTable", &[param("VpcId", vpc_id)]).await;
    member(case, &body, "routeTableId")
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", "us-east-1a"),
        ],
    )
    .await;
    member(case, &body, "subnetId")
}

/// The body `DescribeVpcEndpoints` answers for the one endpoint `endpoint_id`.
async fn describe(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[endpoint_id]),
    )
    .await
}

/// The policy document a describe body reports, as the bytes the endpoint carries — the XML
/// entities the document's quotes reach the wire as resolved.
fn reported_policy(case: &str, body: &str) -> String {
    unescaped(&member(case, body, "policyDocument"))
}

/// `DescribeVpcEndpoints` once the endpoint reports the terminal `available` state, as its
/// body — a poll for the CONDITION rather than a wait for a span, so the assertion cannot
/// depend on how fast the host runs.
async fn available(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let deadline = std::time::Instant::now() + CONVERGENCE_DEADLINE;
    loop {
        let body = describe(case, router, endpoint_id).await;
        if element(&body, "state").as_deref() == Some(ec2::state::VPC_ENDPOINT_STATE_AVAILABLE) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {endpoint_id} to reach the available state, got {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `document` is JSON that grants full access to the service: every statement allows
/// every action, by every principal, on every resource.
///
/// What "a default policy that allows full access to the service"
/// (`CreateVpcEndpointRequest$PolicyDocument`) asserts about the installed default, read off
/// the parsed document rather than off one spelling of it.
///
/// Deliberately INDEPENDENT of the crate's own reading of that question
/// (`state::policy_document_grants_full_access`, which the create and the modify judge a
/// caller's document with): a test that asked the implementation whether its own default
/// satisfies it would re-assert the implementation. Only the grant is asked here — the crate's
/// reading goes further and admits no member beside the ones that MAKE the grant — so this
/// predicate is the weaker of the two and cannot pass a document that one would refuse.
fn grants_full_access(document: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(document) else {
        return false;
    };
    let Some(statement) = parsed.get("Statement") else {
        return false;
    };
    let statements: Vec<&serde_json::Value> = match statement.as_array() {
        Some(entries) => entries.iter().collect(),
        None => vec![statement],
    };
    let everything = |value: Option<&serde_json::Value>| match value {
        Some(serde_json::Value::String(text)) => text == "*",
        Some(serde_json::Value::Array(entries)) => {
            entries.len() == 1 && entries[0].as_str() == Some("*")
        }
        _ => false,
    };
    !statements.is_empty()
        && statements.iter().all(|statement| {
            statement.get("Effect").and_then(serde_json::Value::as_str) == Some("Allow")
                && everything(statement.get("Action"))
                && everything(statement.get("Principal"))
                && everything(statement.get("Resource"))
        })
}

/// A document of the caller's own, laid out compactly, that allows one read action.
const READ_ONLY_DOCUMENT: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","#,
    r#""Action":["s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
);

/// The same grant, laid out the way a client's own JSON writer lays one out: pretty-printed,
/// its statement's keys in another order, and no policy-grammar date at all. A DIFFERENT
/// document from [`READ_ONLY_DOCUMENT`] as bytes, which is what the endpoint reports.
const PRETTY_DOCUMENT: &str = concat!(
    "{\n",
    "  \"Statement\" : [ {\n",
    "    \"Action\" : \"dynamodb:GetItem\",\n",
    "    \"Effect\" : \"Allow\",\n",
    "    \"Principal\" : \"*\",\n",
    "    \"Resource\" : \"*\"\n",
    "  } ]\n",
    "}",
);

/// A document that denies everything — the narrowest document there is, and the one the
/// recorded round trip installs over the create's.
const DENY_ALL_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"*","Resource":"*"}]}"#;

/// A document carrying SEVERAL statements, one of them conditional: a form neither of the
/// others has.
const CONDITIONAL_DOCUMENT: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadOnly","Effect":"Allow","#,
    r#""Principal":"*","Action":"s3:GetObject","Resource":"*"},"#,
    r#"{"Sid":"NoDeletes","Effect":"Deny","Principal":"*","Action":"s3:DeleteObject","#,
    r#""Resource":"*","Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#,
);

/// One row of the round-trip table.
struct PolicyRoundTrip {
    case: &'static str,
    /// The endpoint service the endpoint is created to.
    service_name: &'static str,
    /// `VpcEndpointType`.
    endpoint_type: &'static str,
    /// How many of the VPC's route tables a gateway endpoint is created for.
    route_tables: usize,
    /// How many subnets an interface endpoint is created in.
    subnets: usize,
    /// The `PolicyDocument` the CREATE names, or `None` to leave the member out.
    created: Option<&'static str>,
    /// The document a later `ModifyVpcEndpoint` installs over it.
    modified: &'static str,
    /// Whether the row goes on to reset the policy — "(Gateway endpoint) Specify true to reset
    /// the policy document to the default policy" (`ModifyVpcEndpointRequest$ResetPolicy`), so
    /// only a gateway endpoint's row does.
    resets: bool,
}

/// The rows: both gateway services the Region publishes a prefix list for, with and without a
/// document of the caller's own and reached from none, one and two route tables, and an
/// interface endpoint — the other type that carries a policy, whose default document differs
/// and which is created in a non-terminal state.
fn round_trips() -> Vec<PolicyRoundTrip> {
    vec![
        PolicyRoundTrip {
            case: "a gateway endpoint to the object store, created with a read-only document",
            service_name: "com.amazonaws.us-east-1.s3",
            endpoint_type: "Gateway",
            route_tables: 1,
            subnets: 0,
            created: Some(READ_ONLY_DOCUMENT),
            modified: DENY_ALL_DOCUMENT,
            resets: true,
        },
        PolicyRoundTrip {
            case: "a gateway endpoint to the key-value store, created with a pretty-printed \
                   document and reached from two tables",
            service_name: "com.amazonaws.us-east-1.dynamodb",
            endpoint_type: "Gateway",
            route_tables: 2,
            subnets: 0,
            created: Some(PRETTY_DOCUMENT),
            modified: READ_ONLY_DOCUMENT,
            resets: true,
        },
        PolicyRoundTrip {
            case: "a gateway endpoint created with no document at all, reached from no table",
            service_name: "com.amazonaws.us-east-1.s3",
            endpoint_type: "Gateway",
            route_tables: 0,
            subnets: 0,
            created: None,
            modified: CONDITIONAL_DOCUMENT,
            resets: true,
        },
        PolicyRoundTrip {
            case: "an interface endpoint to the log service, created with a document of its own",
            service_name: "com.amazonaws.us-east-1.logs",
            endpoint_type: "Interface",
            route_tables: 0,
            subnets: 1,
            created: Some(PRETTY_DOCUMENT),
            modified: DENY_ALL_DOCUMENT,
            resets: false,
        },
    ]
}

/// The document the service installs on an endpoint of `endpoint_type` to `service_name` whose
/// create named NO policy, read back off such an endpoint in its own VPC — what a reset has to
/// put back, and what an endpoint created without a document carries.
async fn installed_default(
    case: &str,
    router: &MultiServiceRouter,
    service_name: &str,
    endpoint_type: &str,
) -> String {
    let vpc_id = create_vpc(case, router, "10.91.0.0/16").await;
    let mut params = vec![
        param("VpcId", &vpc_id),
        param("ServiceName", service_name),
        param("VpcEndpointType", endpoint_type),
    ];
    if endpoint_type == "Interface" {
        let subnet_id = create_subnet(case, router, &vpc_id, "10.91.1.0/24").await;
        params.extend(indexed("SubnetId", &[subnet_id.as_str()]));
    }
    let created = ok(case, router, "CreateVpcEndpoint", &params).await;
    let endpoint_id = member(case, &created, "vpcEndpointId");
    let body = available(case, router, &endpoint_id).await;
    reported_policy(case, &body)
}

/// Every row's endpoint reports the document the create installed, then the document a modify
/// installs over it, then — for the type the member belongs to — the installed default a reset
/// puts back; and the endpoint is `available` at each of those reads, so each of a client's
/// three polls terminates.
#[tokio::test]
async fn an_endpoints_policy_document_converges_through_create_modify_and_reset() {
    for row in round_trips() {
        let case = row.case;
        // One emulator per row, so the endpoints, tables and subnets a row builds are its own.
        let (router, _engine) = emulator();
        let default_document =
            installed_default(case, &router, row.service_name, row.endpoint_type).await;
        assert!(
            grants_full_access(&default_document),
            "case {case:?}: expected the installed default to grant full access, got \
             {default_document:?}"
        );

        let vpc_id = create_vpc(case, &router, "10.90.0.0/16").await;
        let mut tables: Vec<String> = Vec::new();
        for _ in 0..row.route_tables {
            tables.push(create_route_table(case, &router, &vpc_id).await);
        }
        let mut subnets: Vec<String> = Vec::new();
        for index in 0..row.subnets {
            let cidr_block = format!("10.90.{}.0/24", index + 1);
            subnets.push(create_subnet(case, &router, &vpc_id, &cidr_block).await);
        }

        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
            param("VpcEndpointType", row.endpoint_type),
        ];
        let named_tables: Vec<&str> = tables.iter().map(String::as_str).collect();
        params.extend(indexed("RouteTableId", &named_tables));
        let named_subnets: Vec<&str> = subnets.iter().map(String::as_str).collect();
        params.extend(indexed("SubnetId", &named_subnets));
        if let Some(document) = row.created {
            params.push(param("PolicyDocument", document));
        }
        let created = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &created, "vpcEndpointId");

        // 1. What the create installed: the caller's own bytes when it named a document, and
        //    the service's default for the type when it named none — never the one replaced by
        //    the other.
        let expected_created = row.created.unwrap_or(default_document.as_str()).to_string();
        let body = available(case, &router, &endpoint_id).await;
        assert_eq!(
            reported_policy(case, &body),
            expected_created,
            "case {case:?}: expected the create's own document to be reported, got {body}"
        );
        // The endpoint is still the one the create made: the tables a gateway endpoint is
        // reached from are reported beside the document, so a converged policy is not a policy
        // reported for an endpoint that dropped them.
        let reported_tables: Vec<String> = element(&body, "routeTableIdSet")
            .map(|set| super::ec2_query::elements(&set, "item"))
            .unwrap_or_default();
        assert_eq!(
            reported_tables, tables,
            "case {case:?}: expected the endpoint to report the tables the create named, got \
             {body}"
        );

        // 2. The document a modify installs replaces it, byte for byte.
        let modified = ok(
            case,
            &router,
            "ModifyVpcEndpoint",
            &[
                param("VpcEndpointId", &endpoint_id),
                param("PolicyDocument", row.modified),
            ],
        )
        .await;
        assert_eq!(
            element(&modified, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return> from the modify, got {modified}"
        );
        let body = available(case, &router, &endpoint_id).await;
        assert_eq!(
            reported_policy(case, &body),
            row.modified,
            "case {case:?}: expected the modified document to be reported, got {body}"
        );

        // 3. And a reset puts the installed default back — the same document an endpoint
        //    created without one carries, whatever the create and the modify had installed.
        if row.resets {
            let reset = ok(
                case,
                &router,
                "ModifyVpcEndpoint",
                &[
                    param("VpcEndpointId", &endpoint_id),
                    param("ResetPolicy", "true"),
                ],
            )
            .await;
            assert_eq!(
                element(&reset, "return").as_deref(),
                Some("true"),
                "case {case:?}: expected <return>true</return> from the reset, got {reset}"
            );
            let body = available(case, &router, &endpoint_id).await;
            assert_eq!(
                reported_policy(case, &body),
                default_document,
                "case {case:?}: expected the installed default back after the reset, got {body}"
            );
        }
    }
}
