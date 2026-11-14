// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE sentence EC2 refuses an unreadable resource id with,
//! driven over the wire the service actually speaks — `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rule under test: every `Invalid<Kind>.Malformed` this service answers out of the
//! SHARED template carries HTTP 400, that kind's own error code, and a message built from
//! the REQUEST — the fixed prefix `Invalid id: ` followed by the offending value in DOUBLE
//! quotes. One template, whatever kind of id the parameter names and whichever operation
//! resolves it, so a client that parses the refusal to learn WHICH value it could not read
//! reads every one of them the same way. [`ec2::state::invalid_id_message`] is where the
//! service words it, and this suite drives it over the wire rather than restating the text
//! per row.
//!
//! The id kinds AWS words with a sentence of their OWN are outside this template and so
//! outside this suite — an Amazon FPGA Image id (`Image ID '<id>' is invalid.`, pinned by
//! [`super::fpga_image_id_form`]), an AMI usage report id, and an EBS VOLUME id (`The volume
//! ID '<id>' is malformed`, pinned by [`super::volume_id_issued_form`] and
//! [`super::delete_volume`]) — each pinned by the suite that owns its resource.
//!
//! Each row is a distinct input along what the rule generalises over:
//!
//! * the ID KIND and the operation that resolves it — an instance, an AMI, a
//!   security group, a Spot Instance request, a Dedicated Host, an event window, a VPC
//!   endpoint and an Elastic IP allocation, each named through a different operation,
//!   so no row is the specification and no operation's own wording is;
//! * the WAY the value fails to be an id of that kind — the prefix alone, a suffix of
//!   neither published length, a non-hexadecimal digit, an upper-case digit, another
//!   resource kind's id, a value with no prefix at all, and a value that is inside the
//!   generic long form but outside the one the kind issues;
//! * the SHAPE of the value — one carrying characters that are significant in the XML
//!   envelope it is reported back inside, and one far longer than any id EC2 issues, which
//!   the message names the leading [`ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS`] characters
//!   of.
//!
//! Every row is answered by a request that names ONLY the id, so what the row pins is the
//! id refusal and not another member's validation.

use ec2::state::{
    INVALID_ALLOCATION_ID_MALFORMED, INVALID_AMI_ID_MALFORMED, INVALID_GROUP_ID_MALFORMED,
    INVALID_HOST_ID_MALFORMED, INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED,
    INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED, INVALID_VPC_ENDPOINT_ID_MALFORMED,
    MAX_ECHOED_RESOURCE_ID_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call, emulator, error_code, error_message};

/// The Region every call here is signed for. Nothing in the rule depends on it: an id is
/// read for its form before anything regional is looked up.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The code `DescribeInstances` refuses a value that is no instance id with. Named here
/// rather than imported because the instance id refusals live on the error channel
/// (`crate::scaffolded`) instead of in `ec2::state`, where every other kind's code is.
const INVALID_INSTANCE_ID_MALFORMED: &str = "InvalidInstanceID.Malformed";

/// One row: a malformed id of one kind, the operation and form field that names it, and the
/// error code that kind carries.
struct Row {
    /// What makes this value fall outside the form the kind issues.
    case: &'static str,
    action: &'static str,
    /// The form field the id is sent as — the indexed `<Member>.1` for a list member, the
    /// bare member name for a scalar one.
    field: &'static str,
    id: String,
    code: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a value with no resource-id prefix at all, as an instance id",
            action: "DescribeInstances",
            field: "InstanceId.1",
            id: "not-an-instance-id".to_string(),
            code: INVALID_INSTANCE_ID_MALFORMED,
        },
        Row {
            // Inside the generic 17-digit long form, outside the one EC2 issues an
            // instance id in: the kind's own reader decides, and the refusal is worded
            // the same whatever made the value unreadable.
            case: "an instance id whose leading digit is above zero",
            action: "DescribeInstances",
            field: "InstanceId.1",
            id: "i-1234567890abcdef0".to_string(),
            code: INVALID_INSTANCE_ID_MALFORMED,
        },
        Row {
            case: "the prefix alone, as a VPC endpoint id",
            action: "DescribeVpcEndpoints",
            field: "VpcEndpointId.1",
            id: "vpce-".to_string(),
            code: INVALID_VPC_ENDPOINT_ID_MALFORMED,
        },
        Row {
            case: "a suffix of neither published length, as an AMI id",
            action: "DescribeImages",
            field: "ImageId.1",
            id: "ami-0123456789abcde".to_string(),
            code: INVALID_AMI_ID_MALFORMED,
        },
        Row {
            case: "another resource kind's id, as a security group id",
            action: "DescribeSecurityGroupReferences",
            field: "GroupId.1",
            id: "vpc-0a1b2c3d4e5f60718".to_string(),
            code: INVALID_GROUP_ID_MALFORMED,
        },
        Row {
            case: "a non-hexadecimal digit, as a Spot Instance request id",
            action: "DescribeSpotInstanceRequests",
            field: "SpotInstanceRequestId.1",
            id: "sir-0a1b2c3d4e5f6071z".to_string(),
            code: INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED,
        },
        Row {
            case: "an upper-case suffix, as a Dedicated Host id",
            action: "DescribeMacHosts",
            field: "HostId.1",
            id: "h-0A1B2C3D".to_string(),
            code: INVALID_HOST_ID_MALFORMED,
        },
        Row {
            case: "the prefix without its separator, as an allocation id",
            action: "DescribeAddressTransfers",
            field: "AllocationId.1",
            id: "eipalloc0a1b2c3d".to_string(),
            code: INVALID_ALLOCATION_ID_MALFORMED,
        },
        Row {
            case: "a suffix one digit too long, as an event window id",
            action: "DescribeInstanceEventWindows",
            field: "InstanceEventWindowId.1",
            id: "iew-0a1b2c3d4e5f60718a".to_string(),
            code: INVALID_INSTANCE_EVENT_WINDOW_ID_MALFORMED,
        },
        Row {
            case: "a non-hexadecimal digit, as a VPC endpoint id",
            action: "DescribeVpcEndpoints",
            field: "VpcEndpointId.1",
            id: "vpce-0a1b2c3z".to_string(),
            code: INVALID_VPC_ENDPOINT_ID_MALFORMED,
        },
        Row {
            case: "a suffix of neither published length, as an allocation id",
            action: "DescribeAddressTransfers",
            field: "AllocationId.1",
            id: "eipalloc-0a1b2c3".to_string(),
            code: INVALID_ALLOCATION_ID_MALFORMED,
        },
        Row {
            // The value is reported back inside an XML envelope, so one carrying the
            // characters that are significant there is a form the rule has to hold for:
            // the quoting of the template is the DOUBLE quote whatever the value quotes
            // itself with.
            case: "a value carrying XML metacharacters, as an instance id",
            action: "DescribeInstances",
            field: "InstanceId.1",
            id: "i-<&'\"0a1b2c3d".to_string(),
            code: INVALID_INSTANCE_ID_MALFORMED,
        },
        Row {
            // Far longer than any id EC2 issues: the message names the leading
            // `MAX_ECHOED_RESOURCE_ID_CHARS` characters, so a refusal cannot be made to
            // carry an unbounded value the caller chose.
            case: "a value longer than the message repeats back, as an instance id",
            action: "DescribeInstances",
            field: "InstanceId.1",
            id: "i-".to_string() + &"a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2),
            code: INVALID_INSTANCE_ID_MALFORMED,
        },
    ]
}

/// RULE: every malformed resource id is refused HTTP 400 with its kind's own code and the
/// ONE message `Invalid id: "<value>"`, built from the value the request sent.
#[tokio::test]
async fn a_malformed_resource_id_is_refused_with_the_value_in_double_quotes() {
    let (router, _engine) = emulator();

    for Row {
        case,
        action,
        field,
        id,
        code,
    } in rows()
    {
        let params = vec![(field.to_string(), id.clone())];
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

        // The value is echoed from the request, bounded the way the service bounds it, and
        // quoted with the DOUBLE quote of EC2's template — a single-quoted value is the
        // divergence this suite exists to pin.
        let echoed: String = id.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
        let expected = format!("Invalid id: \"{echoed}\"");
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?} from {action}, got {message:?}"
        );
        assert!(
            !message.contains(&format!("'{echoed}'")),
            "case {case:?}: the value must not be single-quoted, got {message:?}"
        );
    }
}
