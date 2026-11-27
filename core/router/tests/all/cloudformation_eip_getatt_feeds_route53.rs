// SPDX-License-Identifier: CC0-1.0

//! What an `Fn::GetAtt` over an Elastic IP address resolves to in a CloudFormation stack, and
//! what a read of an attribute the resource type has no such member for is reported as.
//!
//! Two rules are under test, one per section:
//!
//! * **A resource type publishes every attribute its own "Return values" section documents,
//!   and an `Fn::GetAtt` that reads one resolves to the value the service that OWNS the
//!   resource really assigned** — so `AWS::EC2::EIP` publishes `AllocationId` AND `PublicIp`,
//!   both reading back as what `AllocateAddress` answered, and a consuming resource of the
//!   same stack is created with that value: the canonical Elastic IP plus Route 53 A-record
//!   pattern puts the allocated address in the record's `ResourceRecords`, so Route 53 itself
//!   holds a record whose value is the address Amazon EC2 reports for the allocation.
//! * **An `Fn::GetAtt` naming an attribute the named resource's type does not declare fails
//!   the CONSUMING resource, before its owning service is called, with CloudFormation's own
//!   schema diagnostic** — `Requested attribute <attribute> does not exist in schema for
//!   <resource type>` — which rolls the stack back, so the caller is told which attribute of
//!   which type does not exist rather than being handed a downstream service's complaint about
//!   a request member the template never wrote.
//!
//! Each section's rows differ along what its rule generalises over: which published attribute
//! the template reads, how it SPELLS the read (`Fn::GetAtt`'s two-element form, its dotted
//! single-string form, and the `${Resource.Attribute}` variable of an `Fn::Sub`, which is an
//! `Fn::GetAtt` by another spelling), which attribute name no schema declares, and which type
//! it is read off.
//!
//! Every expected value is read from the OWNING service — Amazon EC2's own `DescribeAddresses`
//! and Route 53's own `ListResourceRecordSets` — never out of the stack answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::template::published_attribute_names;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element and list readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    block_of, blocks_of, elements, form_encode, members_of, opt_element, outputs_of,
};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// How long a poll of a stack's status waits between reads, and how many times it reads: a
/// generous deadline for the CONDITION (a status that is no longer in progress), never a
/// timing assertion.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_ATTEMPTS: usize = 200;

/// The resource type whose published attributes this suite is about, and the two attributes
/// its "Return values" section documents.
const EIP_TYPE: &str = "AWS::EC2::EIP";
const PUBLIC_IP_ATTRIBUTE: &str = "PublicIp";
const ALLOCATION_ID_ATTRIBUTE: &str = "AllocationId";

/// The hosted-zone type the consuming record belongs to, which the second section also reads a
/// nonexistent attribute off.
const HOSTED_ZONE_TYPE: &str = "AWS::Route53::HostedZone";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from. The signature is never checked:
/// this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One form-encoded call on a query wire, as its HTTP status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon EC2 operation over its ec2Query wire.
async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "ec2", "2016-11-15", action, members).await
}

/// Call one Route 53 operation over its restXml wire.
async fn route53(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("route53.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("route53"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Read a stack until it reports a status that is no longer in progress, and answer that
/// read's body. The wait is for the CONDITION, bounded by a generous deadline.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            return described;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    described
}

/// Create a stack and answer the `DescribeStacks` body of the status it settled on.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &Value,
) -> String {
    let template = template.to_string();
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    poll_to_terminal(router, case, stack_name).await
}

/// Every `(logical id, status, reason)` triple the stack's own events report, newest first.
async fn resource_events(
    router: &MultiServiceRouter,
    stack_name: &str,
) -> Vec<(String, String, String)> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    members_of(&events)
        .into_iter()
        .map(|member| {
            (
                opt_element(member, "LogicalResourceId").unwrap_or_default(),
                opt_element(member, "ResourceStatus").unwrap_or_default(),
                opt_element(member, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// The ONE Elastic IP address Amazon EC2 reports for the account, as
/// `(PublicIp, AllocationId)` — the values the service that owns the resource really assigned,
/// which every expectation below is read from.
async fn allocated_address(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = ec2(router, "DescribeAddresses", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddresses, got {status} with body {body}"
    );
    let reported = block_of(&body, "addressesSet").unwrap_or_else(|| {
        panic!("case {case:?}: expected DescribeAddresses to report an addressesSet, got {body}")
    });
    let addresses = elements(reported, "publicIp");
    let allocations = elements(reported, "allocationId");
    assert_eq!(
        (addresses.len(), allocations.len()),
        (1, 1),
        "case {case:?}: expected Amazon EC2 to report exactly the one address the stack \
         allocated, got {body}"
    );
    (addresses[0].clone(), allocations[0].clone())
}

/// The values Route 53 itself holds for the record set named `name` of the zone `zone_id`.
async fn record_values(
    router: &MultiServiceRouter,
    case: &str,
    zone_id: &str,
    name: &str,
) -> Vec<String> {
    let (status, body) = route53(router, "GET", &format!("/2013-04-01{zone_id}/rrset")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListResourceRecordSets, got {status} with body \
         {body}"
    );
    let holding = blocks_of(&body, "ResourceRecordSet")
        .into_iter()
        .find(|set| opt_element(set, "Name").as_deref() == Some(name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected Route 53 to hold a record set named {name:?}, got {body}")
        });
    let records = block_of(holding, "ResourceRecords").unwrap_or_else(|| {
        panic!("case {case:?}: expected the record set to carry ResourceRecords, got {holding}")
    });
    elements(records, "Value")
}

/// The id of the one hosted zone Route 53 reports, in the `/hostedzone/<id>` form every read
/// addresses a zone by.
async fn hosted_zone_id(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = route53(router, "GET", "/2013-04-01/hostedzone").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListHostedZones, got {status} with body {body}"
    );
    let ids = elements(&body, "Id");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected Route 53 to hold exactly the one zone the stack created, got \
         {body}"
    );
    ids[0].clone()
}

// ---------------------------------------------------------------------------
// How a template spells a read of an attribute
// ---------------------------------------------------------------------------

/// The three spellings a template reads one resource's attribute in.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// `Fn::GetAtt`'s documented two-element form: the logical id and the attribute.
    GetAttPair,
    /// `Fn::GetAtt`'s single dotted string, the form a YAML `!GetAtt Eip.PublicIp` carries.
    GetAttDotted,
    /// An `Fn::Sub` whose `${Eip.PublicIp}` variable reads the same attribute.
    SubVariable,
}

impl Spelling {
    /// The template value that reads `attribute` off the resource `logical_id`.
    fn read(self, logical_id: &str, attribute: &str) -> Value {
        match self {
            Self::GetAttPair => json!({ "Fn::GetAtt": [logical_id, attribute] }),
            Self::GetAttDotted => json!({ "Fn::GetAtt": format!("{logical_id}.{attribute}") }),
            Self::SubVariable => json!({ "Fn::Sub": format!("${{{logical_id}.{attribute}}}") }),
        }
    }
}

// ---------------------------------------------------------------------------
// A published attribute resolves to what the owning service assigned
// ---------------------------------------------------------------------------

/// One row of the published-attribute table: which attribute of the Elastic IP the record and
/// the output read, how the template spells that read, and the record type the value goes
/// into.
struct PublishedRow {
    case: &'static str,
    attribute: &'static str,
    spelling: Spelling,
    /// The DNS record type whose value the attribute is written as: an address goes into an
    /// `A` record, an opaque identifier into a `TXT` one.
    record_type: &'static str,
    /// Whether the value Route 53 holds is the attribute's own text, or that text wrapped in
    /// the quotes a `TXT` record's value is written with.
    quoted: bool,
}

fn published_rows() -> Vec<PublishedRow> {
    vec![
        // The recorded pattern: the allocated address feeds the A record of the same stack.
        PublishedRow {
            case: "PublicIp, read through Fn::GetAtt's two-element form",
            attribute: PUBLIC_IP_ATTRIBUTE,
            spelling: Spelling::GetAttPair,
            record_type: "A",
            quoted: false,
        },
        PublishedRow {
            case: "PublicIp, read through the dotted Fn::GetAtt string",
            attribute: PUBLIC_IP_ATTRIBUTE,
            spelling: Spelling::GetAttDotted,
            record_type: "A",
            quoted: false,
        },
        PublishedRow {
            case: "PublicIp, read through an Fn::Sub variable",
            attribute: PUBLIC_IP_ATTRIBUTE,
            spelling: Spelling::SubVariable,
            record_type: "A",
            quoted: false,
        },
        // The type's OTHER published attribute, which is not the value `Ref` returns: the same
        // rule, a different value, so the row also pins that the two do not collapse together.
        PublishedRow {
            case: "AllocationId, read through Fn::GetAtt's two-element form",
            attribute: ALLOCATION_ID_ATTRIBUTE,
            spelling: Spelling::GetAttPair,
            record_type: "TXT",
            quoted: true,
        },
        PublishedRow {
            case: "AllocationId, read through an Fn::Sub variable",
            attribute: ALLOCATION_ID_ATTRIBUTE,
            spelling: Spelling::SubVariable,
            record_type: "TXT",
            quoted: true,
        },
    ]
}

/// Every row: the stack completes, the attribute the output publishes is the value Amazon EC2
/// reports for the allocation, and the record Route 53 holds carries that same value.
#[tokio::test]
async fn an_elastic_ips_published_attributes_reach_the_record_that_reads_them() {
    for (index, row) in published_rows().iter().enumerate() {
        let case = row.case;
        // One emulator per row, so the one address and the one zone each read reports are this
        // row's own.
        let router = build_router();
        let stack_name = format!("eip-published-attribute-{index}");
        let zone_name = format!("eip-published-{index}.example.");
        let record_name = format!("read.eip-published-{index}.example.");
        let read = row.spelling.read("Eip", row.attribute);
        let value = if row.quoted {
            json!({ "Fn::Join": ["", ["\"", read.clone(), "\""]] })
        } else {
            read.clone()
        };
        let template = json!({
            "Resources": {
                "Eip": { "Type": EIP_TYPE, "Properties": { "Domain": "vpc" } },
                "Zone": { "Type": HOSTED_ZONE_TYPE, "Properties": { "Name": zone_name } },
                "Rec": {
                    "Type": "AWS::Route53::RecordSet",
                    "Properties": {
                        "HostedZoneId": { "Ref": "Zone" },
                        "Name": record_name,
                        "Type": row.record_type,
                        "TTL": "60",
                        "ResourceRecords": [value]
                    }
                }
            },
            "Outputs": { "Read": { "Value": read } }
        });

        let described = deploy(&router, case, &stack_name, &template).await;
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reached, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {reached:?}; its \
             events: {:?}",
            resource_events(&router, &stack_name).await
        );

        // What the OWNING service says the address and its allocation are.
        let (address, allocation) = allocated_address(&router, case).await;
        let assigned = if row.attribute == PUBLIC_IP_ATTRIBUTE {
            address.clone()
        } else {
            allocation.clone()
        };
        assert!(
            !assigned.is_empty(),
            "case {case:?}: expected Amazon EC2 to report a {} for the allocation, got {assigned:?}",
            row.attribute
        );

        // The stack publishes that value for the attribute...
        let published = outputs_of(&described).get("Read").cloned();
        assert_eq!(
            published.as_deref(),
            Some(assigned.as_str()),
            "case {case:?}: expected the stack to publish {assigned:?} for {}, got \
             {published:?} in {described}",
            row.attribute
        );

        // ...and so does the record Route 53 itself holds, which is what a resolver answers.
        let zone_id = hosted_zone_id(&router, case).await;
        let held = record_values(&router, case, &zone_id, &record_name).await;
        let expected = if row.quoted {
            format!("\"{assigned}\"")
        } else {
            assigned.clone()
        };
        assert_eq!(
            held,
            vec![expected.clone()],
            "case {case:?}: expected Route 53 to hold the one value {expected:?} for \
             {record_name}, got {held:?}"
        );
    }
}

/// The same rule read off the CATALOGUE rather than off a deployment: the attributes the type
/// publishes are exactly the ones its "Return values" section documents, so a template reading
/// either of them reads an attribute that exists.
#[test]
fn the_elastic_ip_type_publishes_both_of_its_documented_attributes() {
    let published = published_attribute_names(EIP_TYPE);
    assert_eq!(
        published,
        [ALLOCATION_ID_ATTRIBUTE, PUBLIC_IP_ATTRIBUTE],
        "case {EIP_TYPE:?}: expected the attributes the type's Return values section names, got \
         {published:?}"
    );
}

// ---------------------------------------------------------------------------
// An attribute no schema declares fails the resource that reads it
// ---------------------------------------------------------------------------

/// One row of the unknown-attribute table: the resource whose attribute is read, the attribute
/// name no schema of that type declares, and the spelling the read is written in.
struct UnknownRow {
    case: &'static str,
    /// The logical id the read names, and the type of the resource it names.
    logical_id: &'static str,
    resource_type: &'static str,
    attribute: &'static str,
    spelling: Spelling,
}

const UNKNOWN_ROWS: &[UnknownRow] = &[
    // The recorded instance: a template asking an Elastic IP for a private address, which is
    // not one of that type's return values.
    UnknownRow {
        case: "a private address asked of an Elastic IP",
        logical_id: "Eip",
        resource_type: EIP_TYPE,
        attribute: "PrivateIpAddress",
        spelling: Spelling::GetAttPair,
    },
    UnknownRow {
        case: "the same read written as the dotted Fn::GetAtt string",
        logical_id: "Eip",
        resource_type: EIP_TYPE,
        attribute: "PrivateIpAddress",
        spelling: Spelling::GetAttDotted,
    },
    UnknownRow {
        case: "the same read written as an Fn::Sub variable",
        logical_id: "Eip",
        resource_type: EIP_TYPE,
        attribute: "PrivateIpAddress",
        spelling: Spelling::SubVariable,
    },
    // A different attribute name of the same type, so the message's variable part is the name
    // the template asked for rather than one spelling the suite happens to use.
    UnknownRow {
        case: "an IPv6 address asked of an Elastic IP",
        logical_id: "Eip",
        resource_type: EIP_TYPE,
        attribute: "Ipv6Address",
        spelling: Spelling::GetAttPair,
    },
    // ...and a different TYPE, so the message's other variable part is the type of the
    // resource the read names: a hosted zone publishes `Id` and `NameServers`, not a DNS name.
    UnknownRow {
        case: "a DNS name asked of a hosted zone",
        logical_id: "Zone",
        resource_type: HOSTED_ZONE_TYPE,
        attribute: "DNSName",
        spelling: Spelling::GetAttPair,
    },
];

/// The reason CloudFormation fails the consuming resource with: fixed text around the
/// attribute the template asked for and the type whose schema has no such member.
fn missing_schema_attribute_reason(attribute: &str, resource_type: &str) -> String {
    format!("Requested attribute {attribute} does not exist in schema for {resource_type}")
}

/// Every row: the consuming resource fails with CloudFormation's schema diagnostic naming the
/// attribute and the type, the stack rolls back, and nothing of it is left behind in Route 53.
#[tokio::test]
async fn a_read_of_an_attribute_no_schema_declares_fails_the_resource_that_reads_it() {
    for (index, row) in UNKNOWN_ROWS.iter().enumerate() {
        let case = row.case;
        let router = build_router();
        let stack_name = format!("eip-unknown-attribute-{index}");
        let zone_name = format!("eip-unknown-{index}.example.");
        let template = json!({
            "Resources": {
                "Eip": { "Type": EIP_TYPE, "Properties": { "Domain": "vpc" } },
                "Zone": { "Type": HOSTED_ZONE_TYPE, "Properties": { "Name": zone_name } },
                "Rec": {
                    "Type": "AWS::Route53::RecordSet",
                    "Properties": {
                        "HostedZoneId": { "Ref": "Zone" },
                        "Name": format!("bad.eip-unknown-{index}.example."),
                        "Type": "A",
                        "TTL": "60",
                        "ResourceRecords": [row.spelling.read(row.logical_id, row.attribute)]
                    }
                }
            }
        });

        let described = deploy(&router, case, &stack_name, &template).await;
        let events = resource_events(&router, &stack_name).await;
        let expected = missing_schema_attribute_reason(row.attribute, row.resource_type);
        let failures: Vec<&(String, String, String)> = events
            .iter()
            .filter(|(logical_id, status, _)| logical_id == "Rec" && status == "CREATE_FAILED")
            .collect();
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: expected the consuming resource to report exactly one failed \
             create, got {events:?}"
        );
        assert_eq!(
            failures[0].2, expected,
            "case {case:?}: expected the reason {expected:?}, got {:?}",
            failures[0].2
        );

        // The failed create rolls the stack back, as any resource's does.
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reached, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the stack to roll back, got {reached:?} with {described}"
        );

        // And the refusal was CloudFormation's own, reached before Route 53 was asked for the
        // record: that service holds no record set of the name the declaration stated — the
        // zone the rollback removed is gone with it.
        let (status, zones) = route53(&router, "GET", "/2013-04-01/hostedzone").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListHostedZones, got {status} with body \
             {zones}"
        );
        assert!(
            !elements(&zones, "Name").contains(&zone_name),
            "case {case:?}: expected the rolled-back stack to leave no hosted zone behind, got \
             {zones}"
        );
    }
}
