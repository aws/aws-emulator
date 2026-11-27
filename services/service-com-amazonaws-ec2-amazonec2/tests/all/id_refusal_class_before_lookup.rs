// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the TWO-STEP way EC2 refuses a referenced resource id, driven over
//! the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rule under test: an operation asks the SYNTAX question about a referenced id before
//! the EXISTENCE one, so a value that can never name a resource of that kind is answered
//! that kind's `Invalid<Kind>.Malformed` (or, where the operation's own recorded refusal
//! says so, `InvalidParameterValue`) and only a value the operation CAN read is looked up
//! and reported with that kind's `Invalid<Kind>.NotFound`. WHICH values an operation can
//! read, and which sentence each half carries, is a fact of that operation's validator read
//! off its own recorded refusal — three different screens appear below:
//!
//! * the ISSUED-id screen, which decodes a long-form id (`AttachVolume`): an identifier the
//!   mint never handed out names no volume that could ever have existed, and that half of the
//!   screen carries that operation's own code where the printed-form half carries the id
//!   kind's;
//! * the published-FORM screen (`DescribeNetworkInterfaces`,
//!   `DescribeTransitGateways`, the transit gateway attachment describes): the kind's prefix
//!   and 8 or 17 lowercase hexadecimal digits;
//! * the ID-SPACE screen (`DescribeIpamPools`, `DescribeInstanceConnectEndpoints`,
//!   `DescribeVpcEndpoints`): the kind's prefix and lowercase alphanumeric characters, which
//!   is the character rule `DescribeVpcEndpoints`' own refusal states, so a value of that
//!   form is reported ABSENT whatever its length and alphabet.
//!
//! Each row is a distinct input along what the rule generalises over: the id KIND and the
//! operation resolving it, the SCREEN that operation applies, the WAY the value falls
//! outside it (another resource family's prefix, an extended prefix of the same family, a
//! character outside the class, upper case, the prefix alone, a suffix of neither published
//! length, an identifier above the mint), and — for the existence half — both published id
//! lengths and the all-zero placeholder.
//!
//! Every row runs against an EMPTY store, so a well-formed id names nothing and the
//! existence half is reached without seeding anything, and two rows carry a value far longer
//! than any id EC2 issues, where the sentence names only the leading
//! [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters.

use ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS;
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call, emulator, error_code, error_message};

/// The Region every call here is signed for. Nothing in the rule depends on it: an id's form
/// is read before anything regional is looked up.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers every refusal of this class with.
const REFUSAL_STATUS: u16 = 400;

/// One form field.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A refused value as the service's refusals name one: its leading
/// [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters, the bound every echoed id of this crate is held
/// to.
fn echoed(value: &str) -> String {
    value.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect()
}

/// One row: what the request names, the operation that resolves it, and the exact code and
/// sentence that operation answers.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
    message: String,
}

/// The sentence every operation that reads an id through EC2's shared id parser answers for a
/// value it cannot read: imported rather than restated, so this suite cannot drift from it.
fn parser_sentence(value: &str) -> String {
    ec2::state::invalid_id_message(value)
}

/// `DescribeNetworkInterfaces`: the published-form screen, and the interface's own not-found
/// sentence behind it.
fn network_interface_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // Outside the published form: a dotted address behind the right prefix (the value a
    // client reaches for when it confuses an interface with the address on it), a
    // non-hexadecimal suffix, a suffix of neither published length, an upper-case suffix, the
    // prefix alone, and another resource family's id.
    for (case, value) in [
        ("an address behind the interface prefix", "eni-10.80.4.37"),
        ("a non-hexadecimal suffix", "eni-zzzzzzzz"),
        ("a suffix of neither published length", "eni-0a1b2c3d4e"),
        ("an upper-case suffix", "eni-0A1B2C3D"),
        ("the prefix alone", "eni-"),
        ("a subnet id", "subnet-0a1b2c3d4e5f60718"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeNetworkInterfaces",
            params: vec![p("NetworkInterfaceId.1", value)],
            code: "InvalidNetworkInterfaceId.Malformed",
            message: parser_sentence(value),
        });
    }
    // Inside it: both published lengths and the all-zero placeholder, each reported absent.
    for (case, value) in [
        ("a long id naming no interface", "eni-0a1b2c3d4e5f60718"),
        ("a short id naming no interface", "eni-0a1b2c3d"),
        ("the all-zero placeholder", "eni-00000000000000000"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeNetworkInterfaces",
            params: vec![p("NetworkInterfaceId.1", value)],
            code: "InvalidNetworkInterfaceID.NotFound",
            message: format!("The networkInterface ID '{value}' does not exist"),
        });
    }
    rows
}

/// `DescribeIpamPools`: the id-space screen, and the IPAM family's parenthesised not-found
/// sentence behind it.
fn ipam_pool_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (case, value) in [
        ("a value with no pool prefix at all", "my-pool"),
        ("another IPAM resource's id", "ipam-scope-0a1b2c3d4e5f60718"),
        ("a further dash inside the suffix", "ipam-pool-not-an-id"),
        ("an upper-case suffix", "ipam-pool-0A1B2C3D"),
        ("the prefix alone", "ipam-pool-"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeIpamPools",
            params: vec![p("IpamPoolId.1", value)],
            code: ec2::state::INVALID_IPAM_POOL_ID_MALFORMED,
            message: parser_sentence(value),
        });
    }
    // Inside the id space: a non-hexadecimal suffix, a suffix of neither published length,
    // both published lengths and the all-zero placeholder are each READ and reported absent.
    for (case, value) in [
        ("a non-hexadecimal suffix", "ipam-pool-bogus"),
        ("a suffix of neither published length", "ipam-pool-0000000000000000f"),
        ("a long id naming no pool", "ipam-pool-0a1b2c3d4e5f60718"),
        ("a short id naming no pool", "ipam-pool-0a1b2c3d"),
        ("the all-zero placeholder", "ipam-pool-00000000000000000"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeIpamPools",
            params: vec![p("IpamPoolId.1", value)],
            code: ec2::state::INVALID_IPAM_POOL_ID_NOT_FOUND,
            message: format!("The IPAM pool ({value}) does not exist."),
        });
    }
    rows
}

/// `DescribeInstanceConnectEndpoints`: the id-space screen, and the endpoint's own not-found
/// sentence — verdict plus remedy — behind it.
fn instance_connect_endpoint_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (case, value) in [
        ("a value with no endpoint prefix at all", "not-an-endpoint"),
        ("a subnet id", "subnet-0a1b2c3d4e5f60718"),
        ("a further dash inside the suffix", "eice-not-an-id"),
        ("an upper-case suffix", "eice-0A1B2C3D"),
        ("the prefix alone", "eice-"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeInstanceConnectEndpoints",
            params: vec![p("InstanceConnectEndpointId.1", value)],
            code: "InvalidInstanceConnectEndpointId.Malformed",
            message: parser_sentence(value),
        });
    }
    for (case, value) in [
        ("a short non-hexadecimal suffix", "eice-zzz"),
        ("a suffix of neither published length", "eice-0123456789"),
        ("a long id naming no endpoint", "eice-0a1b2c3d4e5f60718"),
        ("a short id naming no endpoint", "eice-0a1b2c3d"),
        ("the all-zero placeholder", "eice-00000000000000000"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeInstanceConnectEndpoints",
            params: vec![p("InstanceConnectEndpointId.1", value)],
            code: "InvalidInstanceConnectEndpointId.NotFound",
            message: format!(
                "The Instance Connect Endpoint {value} does not exist. Add an existing \
                 Instance Connect Endpoint ID and retry your request."
            ),
        });
    }
    // The same bound on the echo as every other refusal of this crate, for the id space this
    // kind reads: a value far longer than any id EC2 issues is named only to its leading
    // `MAX_ECHOED_RESOURCE_ID_CHARS` characters.
    let overlong = format!("eice-{}", "b".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2));
    rows.push(Row {
        case: "a value inside the id space longer than the message repeats back",
        action: "DescribeInstanceConnectEndpoints",
        params: vec![p("InstanceConnectEndpointId.1", &overlong)],
        code: "InvalidInstanceConnectEndpointId.NotFound",
        message: format!(
            "The Instance Connect Endpoint {} does not exist. Add an existing Instance \
             Connect Endpoint ID and retry your request.",
            echoed(&overlong)
        ),
    });
    rows
}

/// `DescribeTransitGateways`: the published-form screen, and the transit gateway family's own
/// not-found sentence behind it.
fn transit_gateway_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (case, value) in [
        ("a short non-hexadecimal suffix", "tgw-xyz"),
        ("a suffix of neither published length", "tgw-0a1b2c3d4e"),
        ("an upper-case suffix", "tgw-0A1B2C3D"),
        ("the prefix alone", "tgw-"),
        ("another transit gateway resource's id", "tgw-rtb-0a1b2c3d4e5f60718"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeTransitGateways",
            params: vec![p("TransitGatewayIds.1", value)],
            code: "InvalidTransitGatewayID.Malformed",
            message: format!("Invalid Transit Gateway id {value}."),
        });
    }
    for (case, value) in [
        ("a long id naming no gateway", "tgw-0a1b2c3d4e5f60718"),
        ("a short id naming no gateway", "tgw-0a1b2c3d"),
        ("the all-zero placeholder", "tgw-00000000000000000"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeTransitGateways",
            params: vec![p("TransitGatewayIds.1", value)],
            code: "InvalidTransitGatewayID.NotFound",
            message: format!("Transit Gateway {value} was deleted or does not exist."),
        });
    }
    rows
}

/// `DescribeVpcEndpoints`: the id-space screen, in the sentence that names the form EXPECTED
/// and the character rule behind it, with the endpoint's own not-found sentence on the other
/// side.
fn vpc_endpoint_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (case, value) in [
        ("another resource family's id", "vpc-0a1b2c3d4e5f60718"),
        ("the endpoint SERVICE's extended prefix", "vpce-svc-0a1b2c3d4e5f60718"),
        ("the endpoint CONNECTION's extended prefix", "vpce-conn-0a1b2c3d4e5f60718"),
        ("the prefix in upper case", "VPCE-0a1b2c3d4e5f60718"),
        ("the prefix alone", "vpce-"),
        ("a value with no prefix at all", "endpoint"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeVpcEndpoints",
            params: vec![p("VpcEndpointId.1", value)],
            code: ec2::state::INVALID_VPC_ENDPOINT_ID_MALFORMED,
            message: format!(
                "Invalid Id: '{value}' (expecting 'vpce-...; the Id may only contain \
                 lowercase alphanumeric characters and a single dash')"
            ),
        });
    }
    for (case, value) in [
        ("a short decimal suffix", "vpce-123"),
        ("a non-hexadecimal suffix", "vpce-firewall"),
        ("a long id naming no endpoint", "vpce-0a1b2c3d4e5f60718"),
        ("a short id naming no endpoint", "vpce-0a1b2c3d"),
        ("the all-zero placeholder", "vpce-00000000000000000"),
    ] {
        rows.push(Row {
            case,
            action: "DescribeVpcEndpoints",
            params: vec![p("VpcEndpointId.1", value)],
            code: ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
            message: format!("The Vpc Endpoint Id '{value}' does not exist"),
        });
    }
    // A value inside the id space FAR longer than any id EC2 issues: the sentence names its
    // leading `MAX_ECHOED_RESOURCE_ID_CHARS` characters, so no request can be turned into an
    // arbitrarily long response through an id kind whose reader takes a value of any length.
    let overlong = format!("vpce-{}", "a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2));
    rows.push(Row {
        case: "a value inside the id space longer than the message repeats back",
        action: "DescribeVpcEndpoints",
        params: vec![p("VpcEndpointId.1", &overlong)],
        code: ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
        message: format!(
            "The Vpc Endpoint Id '{}' does not exist",
            echoed(&overlong)
        ),
    });
    rows
}

/// The three describes whose SUBJECT is a transit gateway attachment: one id space, one
/// fixed malformed sentence, and the transit gateway family's not-found sentence naming the
/// attachment.
fn transit_gateway_attachment_rows() -> Vec<Row> {
    let actions = [
        "DescribeTransitGatewayAttachments",
        "DescribeTransitGatewayVpcAttachments",
        "DescribeTransitGatewayPeeringAttachments",
    ];
    let mut rows: Vec<Row> = Vec::new();
    for action in actions {
        for (case, value) in [
            ("a non-hexadecimal suffix", "tgw-attach-not-hex"),
            ("a suffix of neither published length", "tgw-attach-0123456789ab"),
            ("the transit gateway's own id", "tgw-0a1b2c3d4e5f60718"),
        ] {
            rows.push(Row {
                case,
                action,
                params: vec![p("TransitGatewayAttachmentIds.1", value)],
                code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
                message: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MESSAGE.to_string(),
            });
        }
        for (case, value) in [
            ("a long id naming no attachment", "tgw-attach-0a1b2c3d4e5f60718"),
            ("a short id naming no attachment", "tgw-attach-0a1b2c3d"),
            ("the all-zero placeholder", "tgw-attach-00000000000000000"),
        ] {
            rows.push(Row {
                case,
                action,
                params: vec![p("TransitGatewayAttachmentIds.1", value)],
                code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                message: format!("Transit Gateway Attachment {value} was deleted or does not exist."),
            });
        }
    }
    rows
}

/// `AttachVolume`: the ISSUED-id screen, in TWO classes — a value outside the PRINTED id form
/// carries the id kind's own `InvalidVolumeID.Malformed`, while a value inside that form whose
/// identifier the mint never handed out carries THIS operation's own `InvalidParameterValue`
/// (the value is one the service will not take). Both carry the volume kind's one sentence.
///
/// Every row names an instance id no instance of the account has, so a row that is answered
/// about the VOLUME proves the id's form was read before any resource was resolved.
fn attach_volume_rows() -> Vec<Row> {
    let instance = "i-0a1b2c3d4e5f60718";
    let attach = |value: &'static str, code: &'static str, case: &'static str| Row {
        case,
        action: "AttachVolume",
        params: vec![
            p("VolumeId", value),
            p("InstanceId", instance),
            p("Device", "/dev/sdf"),
        ],
        code,
        message: format!("The volume ID '{value}' is malformed"),
    };
    let mut rows: Vec<Row> = Vec::new();
    // Outside the printed form: the id kind's own malformed code.
    for (case, value) in [
        ("a non-hexadecimal suffix", "vol-nothexdigits"),
        ("a suffix of neither published length", "vol-0a1b2c3d4e"),
        ("an upper-case suffix", "vol-0A1B2C3D"),
        ("the all-zero placeholder", "vol-00000000000000000"),
        ("a snapshot id", "snap-0a1b2c3d4e5f60718"),
        ("a long form whose leading digit no issued id carries", "vol-1a1b2c3d4e5f60718"),
    ] {
        rows.push(attach(value, "InvalidVolumeID.Malformed", case));
    }
    // Inside the printed form, with an identifier the mint never reached: this operation's own
    // code.
    for (case, value) in [
        (
            "a fabricated documentation-shaped identifier the mint never handed out",
            "vol-0123456789abcdef0",
        ),
        ("another identifier above the mint", "vol-0fedcba987654321f"),
    ] {
        rows.push(attach(value, ec2::state::INVALID_PARAMETER_VALUE, case));
    }
    // The other side of the screen: a SHORT-form id carries no identifier to decode, so it is
    // READ — the call goes on to resolve the instance, and the refusal is that resource's.
    rows.push(Row {
        case: "a short-form volume id, which the screen reads rather than refuses",
        action: "AttachVolume",
        params: vec![
            p("VolumeId", "vol-0a1b2c3d"),
            p("InstanceId", instance),
            p("Device", "/dev/sdf"),
        ],
        code: "InvalidInstanceID.NotFound",
        message: format!("The instance ID '{instance}' does not exist"),
    });
    rows
}

/// RULE: every referenced id is refused HTTP 400 with the class that says WHICH question
/// failed — the syntax one before the existence one — and the exact sentence that operation
/// answers for it.
#[tokio::test]
async fn an_unresolvable_id_is_refused_by_class_and_in_its_operations_own_sentence() {
    let (router, _engine) = emulator();

    let rows: Vec<Row> = network_interface_rows()
        .into_iter()
        .chain(ipam_pool_rows())
        .chain(instance_connect_endpoint_rows())
        .chain(transit_gateway_rows())
        .chain(vpc_endpoint_rows())
        .chain(transit_gateway_attachment_rows())
        .chain(attach_volume_rows())
        .collect();

    for Row {
        case,
        action,
        params,
        code,
        message,
    } in rows
    {
        let case = format!("{action} given {case}");
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, code,
            "case {case:?}: expected the code {code:?}, got {answered_code:?} in {body}"
        );
        let answered_message = error_message(&body);
        assert_eq!(
            answered_message, message,
            "case {case:?}: expected the message {message:?}, got {answered_message:?}"
        );
    }
}

/// The same rule read as an INVARIANT about the two classes: the class an id is refused with
/// never swaps. A value outside the operation's screen is never reported as a resource that
/// does not exist, and a value inside it is never reported as unreadable.
#[tokio::test]
async fn the_refusal_class_never_swaps_between_syntax_and_existence() {
    let (router, _engine) = emulator();

    for Row {
        case,
        action,
        params,
        code,
        ..
    } in network_interface_rows()
        .into_iter()
        .chain(ipam_pool_rows())
        .chain(instance_connect_endpoint_rows())
        .chain(transit_gateway_rows())
        .chain(vpc_endpoint_rows())
        .chain(transit_gateway_attachment_rows())
    {
        let case = format!("{action} given {case}");
        let (_, body) = call(&router, REGION, action, &params).await;
        let answered = error_code(&body);
        let syntax = code.ends_with(".Malformed");
        assert_eq!(
            answered.ends_with(".Malformed"),
            syntax,
            "case {case:?}: expected a {} refusal, got {answered:?} in {body}",
            if syntax { "syntax" } else { "existence" }
        );
        assert_eq!(
            answered.ends_with(".NotFound"),
            !syntax,
            "case {case:?}: expected a {} refusal, got {answered:?} in {body}",
            if syntax { "syntax" } else { "existence" }
        );
    }
}
