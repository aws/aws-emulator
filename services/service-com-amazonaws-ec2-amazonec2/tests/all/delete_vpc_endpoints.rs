// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVpcEndpoints`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `DeleteVpcEndpoints` is a BULK call — it removes every endpoint of
//! the ids it resolves, taking with each one what cannot outlive it ("when you delete a
//! gateway endpoint, we delete the endpoint routes in the route tables for the endpoint …
//! when you delete an interface endpoint, we delete its endpoint network interfaces"), and
//! reports each id it could NOT delete in `Unsuccessful` — "information about the VPC
//! endpoints that were not successfully deleted" — with that cause's own per-item code and
//! sentence, rather than refusing the whole request for one id. A deleted endpoint then stops
//! resolving, an endpoint the request did not name is untouched, and an authorized `DryRun`
//! answers `DryRunOperation` having deleted nothing.
//!
//! Each table row is a distinct input along what that rule generalises over: how many ids one
//! request carries, which of them resolve to an endpoint, WHERE in the list the ones that do
//! not sit, which form an id that resolves to nothing takes, which endpoint type is being
//! deleted, how many subnets its interfaces were created in, how often one id is repeated,
//! and how long the id list is next to the ceiling a caller-chosen list is held to. The
//! routes half of the rule is asserted over its own tables by
//! [`super::vpc_internet_path`]; what a delete leaves readable, by
//! [`super::dependency_and_not_found_refusals`].

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, error_message, indexed, unescaped};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// The code `DeleteVpcEndpoints` reports an id it resolves no endpoint for with, PER ITEM —
/// spelt for the resource rather than for its id member, which is what a client that tells
/// "not mine / already gone" from a real failure branches on.
const INVALID_VPC_ENDPOINT_NOT_FOUND: &str = "InvalidVpcEndpoint.NotFound";

/// The code a value that is not an endpoint id of this kind at all is reported with.
const INVALID_VPC_ENDPOINT_ID_MALFORMED: &str = "InvalidVpcEndpointId.Malformed";

/// The code `DescribeVpcEndpoints` refuses an id it holds no endpoint for with — the read
/// that resolves an endpoint as the SUBJECT of the call, which is a different code from the
/// per-item one above, and so what the round trip of a delete is read back through.
const INVALID_VPC_ENDPOINT_ID_NOT_FOUND: &str = "InvalidVpcEndpointId.NotFound";

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
    super::ec2_query::element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One entry of a response's `Unsuccessful` set, as a client reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Reported {
    /// `ResourceId`: the id the request named.
    resource_id: String,
    /// `Error.Code`.
    code: String,
    /// `Error.Message`, with the XML entities resolved.
    message: String,
}

/// The `Unsuccessful` entries of a `DeleteVpcEndpoints` body, in the order it reported them.
///
/// Read through the XML parser from the response ENVELOPE's own `unsuccessful` child, so an
/// entry is the envelope's rather than a set of the same shape nested inside something else,
/// and an empty report (no set at all, or a set with no items) reads as no entries.
fn unsuccessful(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "unsuccessful")
        .into_iter()
        .map(|item| {
            let error = child(item, "error").unwrap_or_else(|| {
                panic!("case {case:?}: an unsuccessful item carries no <error> in {body}")
            });
            Reported {
                resource_id: child_text(item, "resourceId").unwrap_or_default(),
                code: child_text(error, "code").unwrap_or_default(),
                message: unescaped(&child_text(error, "message").unwrap_or_default()),
            }
        })
        .collect()
}

/// What a well-formed id this account holds no endpoint for must be reported as: the code
/// above and the sentence EC2 words that cause with — the fixed `user ` and ` does not own a
/// resource `, around the account the lookup was made in and the id from the request.
fn not_found_report(endpoint_id: &str) -> Reported {
    Reported {
        resource_id: endpoint_id.to_string(),
        code: INVALID_VPC_ENDPOINT_NOT_FOUND.to_string(),
        message: format!(
            "user {owner} does not own a resource {endpoint_id}",
            owner = engine::emulator_account_id(),
        ),
    }
}

/// What a value that is not an endpoint id of this kind must be reported as: EC2's one
/// `Invalid id: "<value>"` sentence, with the value from the request in double quotes.
fn malformed_report(value: &str) -> Reported {
    Reported {
        resource_id: value.to_string(),
        code: INVALID_VPC_ENDPOINT_ID_MALFORMED.to_string(),
        message: format!("Invalid id: \"{value}\""),
    }
}

/// A well-formed VPC endpoint id, distinct per `slot`, far above anything this store's id
/// counter can reach — so it resolves to no endpoint however many endpoints a test created.
fn unresolvable_id(slot: usize) -> String {
    format!(
        "{prefix}-{suffix:017x}",
        prefix = ec2::state::VPC_ENDPOINT_ID_PREFIX,
        suffix = 0x0f00_0000_0000_0000_u64 + slot as u64,
    )
}

/// The forms a value that is NO endpoint id takes, one per slot that needs one.
///
/// Both are values a client plausibly sends: a `vpce-` prefix with a suffix that is not
/// hexadecimal, and a VPC endpoint SERVICE id — which extends the endpoint's own prefix, so
/// only the suffix separates the two kinds
/// (`ec2::state::vpc_endpoint_id_is_well_formed`).
const MALFORMED_VALUES: [&str; 2] = ["vpce-not-hex", "vpce-svc-0a1b2c3d4e5f6a7b8"];

/// What one slot of a request's `VpcEndpointIds` list carries.
#[derive(Clone, Copy, Debug)]
enum Slot {
    /// An endpoint this row created, which the delete has to remove.
    Real,
    /// A well-formed id this account holds no endpoint for.
    Unresolvable,
    /// A value that is not an endpoint id of this kind at all.
    Malformed,
}

/// The endpoint types a row's real ids are created from, in turn: both the type that stands
/// up no endpoint network interface and the type that stands one up per subnet, each to an
/// endpoint service of its own so no two endpoints of a row are the same endpoint twice.
const REAL_ENDPOINTS: [(&str, &str); 4] = [
    ("Gateway", "com.amazonaws.us-east-1.s3"),
    ("Interface", "com.amazonaws.us-east-1.ssm"),
    ("Gateway", "com.amazonaws.us-east-1.dynamodb"),
    ("Interface", "com.amazonaws.us-east-1.logs"),
];

/// The one VPC and subnet a row builds its endpoints in.
struct Network {
    vpc_id: String,
    subnet_id: String,
}

/// Create the VPC and the subnet of a row through the API, so every endpoint below is created
/// against resources this emulator actually stores.
async fn network(case: &str, router: &MultiServiceRouter, vpc_cidr: &str) -> Network {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", vpc_cidr)]).await;
    let vpc_id = member(case, &body, "vpcId");
    let subnet_cidr = vpc_cidr.replace("0.0/16", "1.0/24");
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", &subnet_cidr),
            param("AvailabilityZone", &format!("{REGION}a")),
        ],
    )
    .await;
    let subnet_id = member(case, &body, "subnetId");
    Network { vpc_id, subnet_id }
}

/// Create the `index`th endpoint of [`REAL_ENDPOINTS`] in `built`, as the body the create
/// answered and the id it minted. An interface endpoint is created in the subnet, which is
/// what gives it endpoint network interfaces.
async fn create_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    built: &Network,
    index: usize,
) -> (String, String) {
    let (endpoint_type, service_name) = REAL_ENDPOINTS[index % REAL_ENDPOINTS.len()];
    let mut params = vec![
        param("VpcId", &built.vpc_id),
        param("ServiceName", service_name),
        param("VpcEndpointType", endpoint_type),
    ];
    if endpoint_type == "Interface" {
        params.push(param("SubnetId.1", &built.subnet_id));
    }
    let body = ok(case, router, "CreateVpcEndpoint", &params).await;
    let endpoint_id = member(case, &body, "vpcEndpointId");
    (endpoint_id, body)
}

/// Whether `DescribeVpcEndpoints` still reports `endpoint_id`, read BOTH ways a client reads
/// it: by id, where an endpoint that is gone answers its own `NotFound`, and unfiltered,
/// where it is simply not carried. The two have to agree, which is what keeps a delete from
/// being reported one way to one reader and another way to the other.
async fn still_reported(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> bool {
    let (status, body) = call(
        router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[endpoint_id]),
    )
    .await;
    let by_id = match status {
        200 => true,
        400 => {
            assert_eq!(
                error_code(&body),
                INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
                "case {case:?}: expected {INVALID_VPC_ENDPOINT_ID_NOT_FOUND} for the endpoint \
                 {endpoint_id} that is gone, got {body}"
            );
            false
        }
        other => panic!(
            "case {case:?}: expected HTTP 200 or 400 reading {endpoint_id} back, got {other} \
             with body {body}"
        ),
    };
    let all = ok(case, router, "DescribeVpcEndpoints", &[]).await;
    let unfiltered = elements(&all, "vpcEndpointId").contains(&endpoint_id.to_string());
    assert_eq!(
        by_id, unfiltered,
        "case {case:?}: the read by id ({by_id}) and the unfiltered report ({unfiltered}) must \
         agree about {endpoint_id}, got {all}"
    );
    by_id
}

// ---------------------------------------------------------------------------
// What one bulk delete removes, and what it reports
// ---------------------------------------------------------------------------

/// Every row: a request naming a mixture of ids deletes exactly the endpoints its ids resolve,
/// reports exactly the other ids in `Unsuccessful` — in the order the request named them, each
/// with its own cause's code and sentence — and leaves an endpoint it did not name alone.
#[tokio::test]
async fn a_bulk_delete_removes_what_it_resolves_and_reports_the_rest_per_item() {
    // Rows: the VPC's block and the list the request carries, differing in how many ids
    // resolve, where in the list the ones that do not sit, and which form each of those takes.
    let rows: [(&str, &str, &[Slot]); 6] = [
        (
            "one endpoint and one id naming nothing",
            "10.60.0.0/16",
            &[Slot::Real, Slot::Unresolvable],
        ),
        (
            "the ids naming nothing around the one endpoint",
            "10.61.0.0/16",
            &[Slot::Unresolvable, Slot::Real, Slot::Unresolvable],
        ),
        (
            "two endpoints with a value that is no id between them",
            "10.62.0.0/16",
            &[Slot::Real, Slot::Malformed, Slot::Real, Slot::Unresolvable],
        ),
        (
            "nothing but ids naming nothing",
            "10.63.0.0/16",
            &[Slot::Unresolvable, Slot::Unresolvable],
        ),
        (
            "nothing but values that are no ids at all",
            "10.64.0.0/16",
            &[Slot::Malformed, Slot::Malformed],
        ),
        ("one endpoint alone", "10.65.0.0/16", &[Slot::Real]),
    ];

    for (case, vpc_cidr, slots) in rows {
        let router = emulator();
        let built = network(case, &router, vpc_cidr).await;

        // The endpoint the request does NOT name, created first so it is in the store
        // throughout: a bulk delete acts on the ids it was given and on nothing else.
        let (untouched, _) = create_endpoint(case, &router, &built, 3).await;

        let mut named: Vec<String> = Vec::new();
        let mut real: Vec<String> = Vec::new();
        let mut expected: Vec<Reported> = Vec::new();
        for (slot_index, slot) in slots.iter().enumerate() {
            match slot {
                Slot::Real => {
                    let (endpoint_id, _) =
                        create_endpoint(case, &router, &built, real.len()).await;
                    real.push(endpoint_id.clone());
                    named.push(endpoint_id);
                }
                Slot::Unresolvable => {
                    let endpoint_id = unresolvable_id(slot_index);
                    expected.push(not_found_report(&endpoint_id));
                    named.push(endpoint_id);
                }
                Slot::Malformed => {
                    let value = MALFORMED_VALUES[expected.len() % MALFORMED_VALUES.len()];
                    expected.push(malformed_report(value));
                    named.push(value.to_string());
                }
            }
        }

        // Every endpoint the row created is readable before the delete.
        for endpoint_id in &real {
            assert!(
                still_reported(case, &router, endpoint_id).await,
                "case {case:?}: the created endpoint {endpoint_id} must be readable before the \
                 delete"
            );
        }

        let references: Vec<&str> = named.iter().map(String::as_str).collect();
        let body = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &indexed("VpcEndpointId", &references),
        )
        .await;
        let reported = unsuccessful(case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the ids that could not be deleted, each with its \
             own cause, got {reported:?} in {body}"
        );

        for endpoint_id in &real {
            assert!(
                !still_reported(case, &router, endpoint_id).await,
                "case {case:?}: the endpoint {endpoint_id} the delete resolved must be gone"
            );
        }
        assert!(
            still_reported(case, &router, &untouched).await,
            "case {case:?}: the endpoint {untouched} the request never named must be untouched"
        );
    }
}

/// Repeating one id asks for it ONCE: a real id named several times deletes that endpoint and
/// reports nothing, and an id that resolves to nothing is reported once however often it is
/// named — never once as deleted and once as missing.
#[tokio::test]
async fn one_id_named_several_times_is_asked_for_once() {
    // Rows: how often the id is repeated, and whether it resolves to an endpoint.
    let rows: [(&str, &str, usize, bool); 4] = [
        ("an endpoint named twice", "10.66.0.0/16", 2, true),
        ("an endpoint named three times", "10.67.0.0/16", 3, true),
        ("an id naming nothing, named twice", "10.68.0.0/16", 2, false),
        (
            "an id naming nothing, named four times",
            "10.69.0.0/16",
            4,
            false,
        ),
    ];

    for (case, vpc_cidr, repeats, resolves) in rows {
        let router = emulator();
        let built = network(case, &router, vpc_cidr).await;
        let (endpoint_id, _) = create_endpoint(case, &router, &built, 1).await;
        let named = if resolves {
            endpoint_id.clone()
        } else {
            unresolvable_id(0)
        };

        let references: Vec<&str> = (0..repeats).map(|_| named.as_str()).collect();
        let body = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &indexed("VpcEndpointId", &references),
        )
        .await;
        let reported = unsuccessful(case, &body);
        let expected: Vec<Reported> = if resolves {
            Vec::new()
        } else {
            vec![not_found_report(&named)]
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the id named {repeats} times to be reported once at most, \
             got {reported:?} in {body}"
        );
        assert_eq!(
            still_reported(case, &router, &endpoint_id).await,
            !resolves,
            "case {case:?}: expected the endpoint {endpoint_id} gone exactly when the request \
             resolved it"
        );
    }
}

/// An authorized `DryRun` is answered `DryRunOperation` and deletes nothing — "checks whether
/// you have the required permissions for the action, without actually making the request"
/// (`DryRun`) — whether or not the ids it carries resolve, because a per-item cause is not a
/// refusal of the call.
#[tokio::test]
async fn a_dry_run_reports_dry_run_operation_and_deletes_nothing() {
    // Rows: the list the dry run carries.
    let rows: [(&str, &str, &[Slot]); 4] = [
        ("the one endpoint", "10.70.0.0/16", &[Slot::Real]),
        (
            "the endpoint beside an id naming nothing",
            "10.71.0.0/16",
            &[Slot::Real, Slot::Unresolvable],
        ),
        (
            "an id naming nothing alone",
            "10.72.0.0/16",
            &[Slot::Unresolvable],
        ),
        (
            "a value that is no id at all",
            "10.73.0.0/16",
            &[Slot::Malformed],
        ),
    ];

    for (case, vpc_cidr, slots) in rows {
        let router = emulator();
        let built = network(case, &router, vpc_cidr).await;
        let (endpoint_id, _) = create_endpoint(case, &router, &built, 1).await;

        let named: Vec<String> = slots
            .iter()
            .enumerate()
            .map(|(slot_index, slot)| match slot {
                Slot::Real => endpoint_id.clone(),
                Slot::Unresolvable => unresolvable_id(slot_index),
                Slot::Malformed => MALFORMED_VALUES[0].to_string(),
            })
            .collect();
        let references: Vec<&str> = named.iter().map(String::as_str).collect();
        let mut params = indexed("VpcEndpointId", &references);
        params.push(param("DryRun", "true"));

        let (status, body) = call(&router, "DeleteVpcEndpoints", &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from an authorized dry run, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            still_reported(case, &router, &endpoint_id).await,
            "case {case:?}: a dry run must leave the endpoint {endpoint_id} in place"
        );
    }
}

/// The id list is capped BEFORE any per-item work: a list one id past the ceiling a
/// caller-chosen id list is held to is refused `InvalidParameterValue` naming that ceiling
/// and deletes nothing, while a list AT the ceiling is carried out in full.
#[tokio::test]
async fn an_id_list_past_the_cap_is_refused_before_anything_is_deleted() {
    let maximum = ec2::state::MAX_RESOURCE_IDS;
    // Rows: the length of the list, and whether it is within the cap.
    let rows: [(&str, &str, usize, bool); 2] = [
        ("one id past the ceiling", "10.74.0.0/16", maximum + 1, false),
        ("the list at the ceiling", "10.75.0.0/16", maximum, true),
    ];

    for (case, vpc_cidr, length, within) in rows {
        let router = emulator();
        let built = network(case, &router, vpc_cidr).await;
        let (endpoint_id, _) = create_endpoint(case, &router, &built, 0).await;

        // The real endpoint first, then one distinct well-formed id per remaining slot, so
        // nothing but the LENGTH decides.
        let mut named = vec![endpoint_id.clone()];
        named.extend((1..length).map(unresolvable_id));
        let references: Vec<&str> = named.iter().map(String::as_str).collect();
        let (status, body) = call(
            &router,
            "DeleteVpcEndpoints",
            &indexed("VpcEndpointId", &references),
        )
        .await;

        if within {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the list at the ceiling to be carried out, got {status}"
            );
            assert_eq!(
                unsuccessful(case, &body).len(),
                length - 1,
                "case {case:?}: expected one report per id that resolved nothing"
            );
            assert!(
                !still_reported(case, &router, &endpoint_id).await,
                "case {case:?}: expected the endpoint {endpoint_id} deleted"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 past the ceiling, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&maximum.to_string()) && message.contains(&length.to_string()),
                "case {case:?}: expected the refusal to name the {length} ids and the maximum \
                 of {maximum}, got {message:?}"
            );
            assert!(
                still_reported(case, &router, &endpoint_id).await,
                "case {case:?}: a refused request must leave the endpoint {endpoint_id} in place"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What goes with the endpoint
// ---------------------------------------------------------------------------

/// "When you delete an interface endpoint, we delete its endpoint network interfaces"
/// (`DeleteVpcEndpoints`): the interfaces the create stood up stop being reported by
/// `DescribeNetworkInterfaces`, and the private addresses they held are released — so nothing
/// is left for a read to report as an interface of an endpoint that no longer exists.
///
/// The rows differ in how many subnets the endpoint's interfaces were created in, which is
/// what the interface set generalises over.
#[tokio::test]
async fn deleting_an_interface_endpoint_takes_its_network_interfaces_with_it() {
    // Rows: the VPC's block and how many subnets the endpoint is created in.
    let rows: [(&str, &str, usize); 2] = [
        ("one subnet", "10.76.0.0/16", 1),
        ("two subnets", "10.77.0.0/16", 2),
    ];

    for (case, vpc_cidr, subnets) in rows {
        let router = emulator();
        let body = ok(case, &router, "CreateVpc", &[param("CidrBlock", vpc_cidr)]).await;
        let vpc_id = member(case, &body, "vpcId");
        let mut subnet_ids = Vec::new();
        for index in 0..subnets {
            let zone = format!("{REGION}{}", char::from(b'a' + index as u8));
            let block = vpc_cidr.replace("0.0/16", &format!("{}.0/24", index + 1));
            let body = ok(
                case,
                &router,
                "CreateSubnet",
                &[
                    param("VpcId", &vpc_id),
                    param("CidrBlock", &block),
                    param("AvailabilityZone", &zone),
                ],
            )
            .await;
            subnet_ids.push(member(case, &body, "subnetId"));
        }

        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", "com.amazonaws.us-east-1.ssm"),
            param("VpcEndpointType", "Interface"),
        ];
        let subnet_references: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
        params.extend(indexed("SubnetId", &subnet_references));
        let created = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &created, "vpcEndpointId");

        // "An endpoint network interface is created … in an Availability Zone" (`SubnetIds`):
        // one per subnet, each of them reported by the interface store before the delete.
        let parsed = document(case, &created);
        let interfaces: Vec<String> = parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "networkInterfaceIdSet")
            .map(|node| {
                node.children()
                    .filter(|item| item.is_element())
                    .map(|item| item.text().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            interfaces.len(),
            subnets,
            "case {case:?}: expected one endpoint network interface per subnet, got \
             {interfaces:?} in {created}"
        );
        let described = ok(case, &router, "DescribeNetworkInterfaces", &[]).await;
        let reported = elements(&described, "networkInterfaceId");
        for network_interface_id in &interfaces {
            assert!(
                reported.contains(network_interface_id),
                "case {case:?}: expected {network_interface_id} reported before the delete, got \
                 {described}"
            );
        }

        let deleted = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &indexed("VpcEndpointId", &[endpoint_id.as_str()]),
        )
        .await;
        assert_eq!(
            unsuccessful(case, &deleted),
            Vec::new(),
            "case {case:?}: expected the delete to report nothing unsuccessful, got {deleted}"
        );

        let described = ok(case, &router, "DescribeNetworkInterfaces", &[]).await;
        let reported = elements(&described, "networkInterfaceId");
        for network_interface_id in &interfaces {
            assert!(
                !reported.contains(network_interface_id),
                "case {case:?}: expected {network_interface_id} gone with the endpoint, got \
                 {described}"
            );
            let (status, body) = call(
                &router,
                "DescribeNetworkInterfaces",
                &indexed("NetworkInterfaceId", &[network_interface_id.as_str()]),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the read of {network_interface_id} refused once the \
                 endpoint is gone, got {status} with body {body}"
            );
        }
        assert!(
            !still_reported(case, &router, &endpoint_id).await,
            "case {case:?}: expected the endpoint {endpoint_id} gone"
        );
    }
}
