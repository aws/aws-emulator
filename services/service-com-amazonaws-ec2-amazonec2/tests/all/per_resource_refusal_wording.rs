// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE this service refuses a request with, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rule under test: a refusal's `<Message>` is the sentence the real service words THAT
//! REFUSAL CLASS with — its fixed text verbatim, and only the variable parts the service
//! itself interpolates, taken from the request — never a service-generic template standing in
//! for it. Which sentence a class carries is read off EC2's answer for that class and not
//! guessed from the resource's name. Three consequences the rows below pin:
//!
//! * a class whose sentence names the resource in ITS OWN words keeps those words, whichever
//!   operation answers it: the transit gateway family's `<Resource> <id> was deleted or does
//!   not exist.` for the attachments the transit gateway owns as resources, the endpoint
//!   service's `VpcEndpointService Id`, and the network interface's `interface ID`;
//! * where EC2 answers ONE refusal class with two sentences, each operation keeps the one its
//!   own validator words: a client VPN attachment — the Client VPN endpoint's resource, not the
//!   transit gateway's — keeps the service's shared `<kind> ID '<id>'` template, and so do the
//!   local gateway route table's own operations, while the creates of a resource belonging to
//!   that table answer a value SHAPED like a resource id with `Invalid LocalGateway Route Table
//!   id <id>` and keep the shared template for a value that is no identifier at all; the rows
//!   drive both halves of each split;
//! * a class whose sentence interpolates the request's value does so in that class's own
//!   QUOTING and POSITION — unquoted and in the middle for the attachment, single-quoted for
//!   the address and the interface, unquoted and last for the route table;
//! * a class whose sentence interpolates NOTHING repeats nothing back: the malformed
//!   attachment id, the pair of members `AllocateHosts` may not both be given, and the range
//!   IPAM does not manage are each refused with fixed text, so the value the caller sent must
//!   be absent from the message;
//! * a request for a Dedicated Host CONFIGURATION EC2 cannot provide is a different class from
//!   a parameter it could not read: it carries `InvalidHostConfiguration` and that class's
//!   `The instance <type|family> '<value>' is not supported.`, through both operations that
//!   configure a host.
//!
//! The rows differ along what the rule generalises over — the RESOURCE class, the OPERATION
//! that answers it (every class whose refusal lives in one shared writer is driven through
//! more than one), the REFUSAL kind (not-found, malformed, a pair of members, a range that is
//! not managed), and the FORM of the offending value (both published id lengths, the all-zero
//! id, a suffix of neither length, an upper-case digit, a non-hexadecimal digit, the prefix
//! alone, another resource kind's id, text that is no id at all; both address families and
//! several prefix lengths for a range) — so no single row is the specification.
//!
//! The addresses and ranges are from the blocks RFC 5737 and RFC 3849 set aside for
//! documentation, so no row names an address that belongs to anybody.

use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_ADDRESS_NOT_FOUND, INVALID_AMI_ID_MALFORMED, INVALID_HOST_CONFIGURATION,
    INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED, INVALID_NETWORK_INTERFACE_ID_MALFORMED,
    INVALID_REQUEST, INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND, INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
    availability_zones_in_region,
};

use super::ec2_query::{call, emulator, error_code, error_message};

/// The Region every call here is signed for. Only the `AllocateHosts` rows depend on it, and
/// they derive the zone they name from it rather than writing one out.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// EC2's code for a range the account has not brought into IPAM. Named here rather than
/// imported under its generic `InvalidCidr.NotFound` constant so the row reads as the class it
/// is about.
const INVALID_CIDR_NOT_FOUND: &str = "InvalidCidr.NotFound";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// How one refusal class words the sentence it carries, built from the value the request sent.
type Wording = fn(&str) -> String;

/// One `(action, wording)` pair of a table whose rows differ in which validator answers.
type ActionWording = (&'static str, Wording);

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One row: a request, the code it must be refused with, the sentence it must be refused
/// with, and — for a class whose sentence interpolates nothing — a value of the request that
/// must NOT appear in that sentence.
struct Row {
    /// What this row is a case of, named in every assertion about it.
    case: String,
    action: &'static str,
    params: Params,
    code: &'static str,
    message: String,
    /// A value the request carried that this refusal class does not repeat back.
    withheld: Option<String>,
}

/// The zone of `REGION` the `AllocateHosts` rows allocate into, derived from the service's own
/// naming of a Region's zones so the row and the handler cannot drift.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

/// Class 1: a transit gateway attachment id no attachment of the account has, through every
/// operation that decides one, over the id forms EC2 issues.
///
/// The sentence is a fact of the attachment KIND the operation acts on: a VPC attachment and a
/// transit gateway peering attachment are the transit gateway's OWN resources and carry that
/// family's prose, while a client VPN attachment — the Client VPN endpoint's resource — keeps
/// the service's shared `transit-gateway-attachment ID` template, the same one every operation
/// that takes an attachment id as a parameter of another action answers.
fn unknown_attachment_rows() -> Vec<Row> {
    // The operations that decide a `TransitGatewayAttachmentId`, across both wordings and both
    // decisions, so each writer is driven through more than one caller.
    let actions: [ActionWording; 4] = [
        ("AcceptTransitGatewayPeeringAttachment", family_wording),
        ("AcceptTransitGatewayVpcAttachment", family_wording),
        ("RejectTransitGatewayVpcAttachment", family_wording),
        ("AcceptTransitGatewayClientVpnAttachment", shared_wording),
    ];
    // Both published id lengths, and the all-zero long id every client reaches for as a
    // placeholder — which is WELL FORMED, so it is this refusal rather than the malformed one.
    let ids = [
        "tgw-attach-0a1b2c3d4e5f60718",
        "tgw-attach-0a1b2c3d",
        "tgw-attach-00000000000000000",
    ];
    actions
        .into_iter()
        .flat_map(|(action, wording)| {
            ids.into_iter().map(move |id| Row {
                case: format!("{action} naming the unheld attachment {id}"),
                action,
                params: vec![p("TransitGatewayAttachmentId", id)],
                code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                message: wording(id),
                withheld: None,
            })
        })
        .collect()
}

/// The transit gateway family's sentence for one of its OWN resources that is not there.
fn family_wording(id: &str) -> String {
    format!("Transit Gateway Attachment {id} was deleted or does not exist.")
}

/// The service's shared sentence for an attachment id it holds no attachment for.
fn shared_wording(id: &str) -> String {
    format!("The transit-gateway-attachment ID '{id}' does not exist")
}

/// Class 2: a value that is no transit gateway attachment id at all. The sentence is entirely
/// fixed, so every row expects the SAME message and none of them may name its own value.
fn malformed_attachment_rows() -> Vec<Row> {
    // One scalar member and one list member, so the class is pinned where the id is the whole
    // subject of the call and where it is one of several ids a describe selects on.
    let scalar_actions = [
        "CreateTransitGatewayConnectPeer",
        "AcceptTransitGatewayPeeringAttachment",
        "RejectTransitGatewayVpcAttachment",
    ];
    // A suffix of neither published length, a non-hexadecimal suffix, an upper-case suffix,
    // the prefix alone, and another transit gateway resource's id.
    let values = [
        "tgw-attach-0123456789ab",
        "tgw-attach-not-hex",
        "tgw-attach-0A1B2C3D",
        "tgw-attach-",
        "tgw-0a1b2c3d4e5f60718",
    ];
    let mut rows: Vec<Row> = scalar_actions
        .into_iter()
        .flat_map(|action| {
            values.into_iter().map(move |value| {
                let mut params = vec![p("TransitGatewayAttachmentId", value)];
                // `CreateTransitGatewayConnectPeer` models two further members `@required`,
                // which the boundary validator refuses a request for before any handler runs,
                // so they are carried here; the attachment id's own form is what this row is
                // about and the handler reads it first.
                if action == "CreateTransitGatewayConnectPeer" {
                    params.push(p("PeerAddress", "192.0.2.1"));
                    params.push(p("InsideCidrBlocks.1", "169.254.6.0/29"));
                }
                Row {
                    case: format!("{action} given the unreadable attachment id {value}"),
                    action,
                    params,
                    code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
                    message: "Invalid Transit Gateway Attachment id.".to_string(),
                    withheld: Some(value.to_string()),
                }
            })
        })
        .collect();
    for value in values {
        rows.push(Row {
            case: format!("DescribeTransitGatewayAttachments selecting on {value}"),
            action: "DescribeTransitGatewayAttachments",
            params: vec![p("TransitGatewayAttachmentIds.1", value)],
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            message: "Invalid Transit Gateway Attachment id.".to_string(),
            withheld: Some(value.to_string()),
        });
    }
    rows
}

/// Class 3: a VPC endpoint service id this account holds no endpoint service for. The
/// resource is named the way the service names it in this sentence, which is not the prose a
/// handler might reach for.
fn unknown_endpoint_service_rows() -> Vec<Row> {
    ["vpce-svc-00000000000000000", "vpce-svc-0a1b2c3d", "vpce-svc-0a1b2c3d4e5f60718"]
        .into_iter()
        .map(|service_id| Row {
            case: format!("AcceptVpcEndpointConnections for the unheld service {service_id}"),
            action: "AcceptVpcEndpointConnections",
            params: vec![
                p("ServiceId", service_id),
                p("VpcEndpointId.1", "vpce-0a1b2c3d4e5f60718"),
            ],
            code: INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
            message: format!("The VpcEndpointService Id '{service_id}' does not exist"),
            withheld: None,
        })
        .collect()
}

/// Class 4: an Elastic IP address this account holds no allocation for — one sentence for that
/// code, whether the call tried to RECOVER the address or to RELEASE it.
fn unknown_address_rows() -> Vec<Row> {
    let addresses = ["192.0.2.55", "198.51.100.1", "203.0.113.200"];
    let mut rows = Vec::new();
    for address in addresses {
        rows.push(Row {
            case: format!("AllocateAddress recovering the unheld address {address}"),
            action: "AllocateAddress",
            params: vec![p("Address", address)],
            code: INVALID_ADDRESS_NOT_FOUND,
            message: format!(
                "The Elastic IP address '{address}' does not exist or is not available."
            ),
            withheld: None,
        });
        rows.push(Row {
            case: format!("ReleaseAddress releasing the unheld address {address}"),
            action: "ReleaseAddress",
            params: vec![p("PublicIp", address)],
            code: INVALID_ADDRESS_NOT_FOUND,
            message: format!(
                "The Elastic IP address '{address}' does not exist or is not available."
            ),
            withheld: None,
        });
    }
    rows
}

/// Class 5: the two `AllocateHosts` members that may not both be given. The sentence names the
/// MEMBERS, not the values, so no row's own values may appear in it.
fn both_host_dimensions_rows() -> Vec<Row> {
    // A type with its own family, a type with another family, and a metal type with its
    // family: the refusal is the same whatever the pair says.
    [
        ("m5.large", "m5", 0usize),
        ("c5.xlarge", "m5", 1),
        ("mac2.metal", "mac2", 2),
    ]
    .into_iter()
    .map(|(instance_type, instance_family, zone)| Row {
        case: format!("AllocateHosts given both {instance_type} and {instance_family}"),
        action: "AllocateHosts",
        params: vec![
            p("InstanceType", instance_type),
            p("InstanceFamily", instance_family),
            p("AvailabilityZone", &zone_name(zone)),
            p("Quantity", "1"),
        ],
        code: INVALID_REQUEST,
        message: "You cannot set both 'InstanceType' and 'InstanceFamily'.".to_string(),
        withheld: Some(instance_type.to_string()),
    })
    .collect()
}

/// Class 6: a value that is no network interface id at all, through two operations that
/// resolve one.
fn malformed_interface_rows() -> Vec<Row> {
    let actions = ["AssignIpv6Addresses", "DeleteNetworkInterface"];
    // Text that is no id at all, a suffix one digit short of each published length, an
    // upper-case suffix, the prefix alone, and another resource kind's id.
    let values = [
        "not-an-eni-id",
        "eni-0a1b2c3",
        "eni-0123456789abcdef",
        "eni-0123456789ABCDEF0",
        "eni-",
        "subnet-0a1b2c3d4e5f60718",
    ];
    actions
        .into_iter()
        .flat_map(|action| {
            values.into_iter().map(move |value| Row {
                case: format!("{action} given the unreadable interface id {value}"),
                action,
                params: vec![p("NetworkInterfaceId", value)],
                code: INVALID_NETWORK_INTERFACE_ID_MALFORMED,
                message: format!("The interface ID '{value}' is malformed"),
                withheld: None,
            })
        })
        .collect()
}

/// Class 7: a value `GetImageAncestry` cannot read as an AMI id. Its validator words the
/// refusal as the IMAGE resource's own sentence.
fn malformed_image_rows() -> Vec<Row> {
    [
        "not-a-valid-ami-id",
        "ami-zzzzzzzz",
        "ami-0123456789abcde",
        "snap-0a1b2c3d4e5f60718",
    ]
    .into_iter()
    .map(|image_id| Row {
        case: format!("GetImageAncestry given the unreadable image id {image_id}"),
        action: "GetImageAncestry",
        params: vec![p("ImageId", image_id)],
        code: INVALID_AMI_ID_MALFORMED,
        message: format!("The image ID '{image_id}' is malformed"),
        withheld: None,
    })
    .collect()
}

/// Class 8: a value that is no local gateway route table id at all.
///
/// Two sentences: a create of a resource BELONGING to the table — a static route, a
/// customer-owned address pool — words a value SHAPED like a resource id (a prefix, a separator
/// and a lowercase hexadecimal suffix) with a fixed prefix and the id UNQUOTED and last, while
/// the readers of the table itself — the search and the delete of a route, the describe of the
/// tables themselves — keep the service's `The <kind> ID '<id>' is malformed` shape, which is
/// also what EVERY one of them answers a value that is no identifier at all with. The rows
/// drive both, over the same values, so the split is pinned rather than assumed.
fn malformed_route_table_rows() -> Vec<Row> {
    // `(action, form field, whether this action CREATES a resource belonging to the table)`:
    // only a create's own validator words this class in the owning service's sentence, and
    // only for a value shaped like an identifier.
    let actions: [(&'static str, &'static str, bool); 5] = [
        ("CreateLocalGatewayRoute", "LocalGatewayRouteTableId", true),
        ("CreateCoipPool", "LocalGatewayRouteTableId", true),
        (
            "SearchLocalGatewayRoutes",
            "LocalGatewayRouteTableId",
            false,
        ),
        (
            "DeleteLocalGatewayRoute",
            "LocalGatewayRouteTableId",
            false,
        ),
        (
            "DescribeLocalGatewayRouteTables",
            "LocalGatewayRouteTableId.1",
            false,
        ),
    ];
    // The prefix without its separator, another local gateway resource's id, a suffix of
    // neither published length, and text that is no id at all. The second column is whether
    // the value is SHAPED like an EC2 resource id — a prefix, a separator and a lowercase
    // hexadecimal suffix — which is what decides whether a create's own validator sees it at
    // all; it is written out per row rather than recomputed, so the table says which sentence
    // each row drives.
    let values = [
        ("lgwrtb-00000000000000000", true),
        ("lgw-00000000000000001", true),
        ("lgw-rtb-0a1b2c3", true),
        ("not-a-route-table-id", false),
    ];
    actions
        .into_iter()
        .flat_map(|(action, field, creates_in_table)| {
            values.into_iter().map(move |(value, shaped)| Row {
                case: format!("{action} given the unreadable route table id {value}"),
                action,
                params: vec![p(field, value)],
                code: INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
                message: if creates_in_table && shaped {
                    create_route_wording(value)
                } else {
                    table_wording(value)
                },
                withheld: None,
            })
        })
        .collect()
}

/// The sentence the validator of a create of a resource belonging to the table words a value
/// SHAPED like a resource id with: the words of the service that owns the local gateway, which
/// is the only one that sees such a value. A value that is no identifier at all is refused
/// before it, with [`table_wording`].
fn create_route_wording(id: &str) -> String {
    format!("Invalid LocalGateway Route Table id {id}")
}

/// The sentence the readers of the table itself word the same class with.
fn table_wording(id: &str) -> String {
    format!("The local-gateway-route-table ID '{id}' is malformed")
}

/// Class 10: a Dedicated Host configured for hardware EC2 does not offer, through both
/// operations that configure one.
///
/// A configuration EC2 cannot provide is its own class — `InvalidHostConfiguration`, not the
/// `InvalidParameterValue` of a value a parameter cannot take — and its sentence names the
/// MEMBER's resource (an instance type or an instance family) and the value the request sent.
fn unsupported_host_configuration_rows() -> Vec<Row> {
    // Rows: a size no family offers, a value that is no instance type at all, an instance
    // family given where a type belongs, a family no offered type belongs to, and a whole
    // instance type given where a family belongs.
    let values: [(&'static str, &'static str); 5] = [
        ("InstanceType", "m5.enormous"),
        ("InstanceType", "not-an-instance-type"),
        ("InstanceType", "m5"),
        ("InstanceFamily", "zz9"),
        ("InstanceFamily", "m5.large"),
    ];
    let mut rows = Vec::new();
    for (member, value) in values {
        let message = if member == "InstanceType" {
            format!("The instance type '{value}' is not supported.")
        } else {
            format!("The instance family '{value}' is not supported.")
        };
        rows.push(Row {
            case: format!("AllocateHosts for the unoffered {member} {value}"),
            action: "AllocateHosts",
            params: vec![
                p(member, value),
                p("AvailabilityZone", &zone_name(0)),
                p("Quantity", "1"),
            ],
            code: INVALID_HOST_CONFIGURATION,
            message: message.clone(),
            withheld: None,
        });
        rows.push(Row {
            case: format!("ModifyHosts for the unoffered {member} {value}"),
            action: "ModifyHosts",
            // No host of this account is named: the configuration the request asks for is
            // refused before any host id is read, so the id below is never resolved.
            params: vec![p(member, value), p("HostId.1", "h-0a1b2c3d4e5f60718")],
            code: INVALID_HOST_CONFIGURATION,
            message,
            withheld: None,
        });
    }
    rows
}

/// Class 9: a range IPAM does not manage. The sentence is fixed and names no range, so no row
/// may find its own range in the message.
fn cidr_not_in_ipam_rows() -> Vec<Row> {
    // Both address families and three prefix lengths, so the fixed sentence is pinned across
    // what the class generalises over.
    ["192.0.2.0/24", "203.0.113.0/25", "2001:db8::/32"]
        .into_iter()
        .map(|cidr| Row {
            case: format!("AssociateIpamByoasn naming the unmanaged range {cidr}"),
            action: "AssociateIpamByoasn",
            params: vec![p("Asn", "64496"), p("Cidr", cidr)],
            code: INVALID_CIDR_NOT_FOUND,
            message: "The requested CIDR could not be found in IPAM.".to_string(),
            withheld: Some(cidr.to_string()),
        })
        .collect()
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    rows.extend(unknown_attachment_rows());
    rows.extend(malformed_attachment_rows());
    rows.extend(unknown_endpoint_service_rows());
    rows.extend(unknown_address_rows());
    rows.extend(both_host_dimensions_rows());
    rows.extend(malformed_interface_rows());
    rows.extend(malformed_image_rows());
    rows.extend(malformed_route_table_rows());
    rows.extend(cidr_not_in_ipam_rows());
    rows.extend(unsupported_host_configuration_rows());
    rows
}

/// RULE: every refusal above carries HTTP 400, that class's own error code, and that class's
/// own sentence — fixed text verbatim, with only the variable parts the class interpolates.
#[tokio::test]
async fn every_refusal_carries_its_own_classs_sentence() {
    let (router, _engine) = emulator();

    for Row {
        case,
        action,
        params,
        code,
        message,
        withheld,
    } in rows()
    {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} from {action}, got {status} with \
             body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, code,
            "case {case:?}: expected the code {code:?} from {action}, got {answered_code:?} in \
             {body}"
        );
        let answered = error_message(&body);
        assert_eq!(
            answered, message,
            "case {case:?}: expected the message {message:?} from {action}, got {answered:?}"
        );
        if let Some(withheld) = withheld {
            assert!(
                !answered.contains(&withheld),
                "case {case:?}: this refusal interpolates no value, so {withheld:?} must be \
                 absent from {answered:?}"
            );
        }
    }
}
