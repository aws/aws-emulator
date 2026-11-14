// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request that violates a PUBLISHED parameter limit — the
//! page-size bounds of a paged describe, the instance-count range of a Capacity
//! Reservation, the order of a launch's two counts, the one attribute type a modify names,
//! the attributes a reset acts on, and the hardware a Dedicated Host can be configured for
//! — end to end over the wire the service speaks (`ec2Query`, through the shared
//! [`super::ec2_query`] harness).
//!
//! The rule under test: a refusal carries EC2's OWN sentence for that violation class, with
//! the bound the operation publishes in it, rather than a description of the limit composed
//! by this service — a client that reads the limit out of the message, or branches on the
//! code, programs the real contract. The bound enforced is the published one, which for a
//! page size is per operation.
//!
//! Each table row is one input, and the rows differ along what the rule generalises over:
//! which operation is asked, which bound a value sits on (one below the minimum, the
//! minimum itself, one past the maximum), the SIGN and size of the value, which attributes
//! a request names and in which pairing, and which hardware a host asks for. No row is the
//! specification.

use ec2::state::availability_zones_in_region;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};

/// The Region every call here is signed for: each operation below reports per-account state
/// whose limits do not depend on the Region, so the Region only selects the ec2Query
/// envelope and the zone a volume or a reservation sits in.
const REGION: &str = "us-east-1";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The first Availability Zone of [`REGION`], read from the service rather than written
/// here.
fn zone() -> String {
    availability_zones_in_region(REGION)[0].zone_name.clone()
}

/// One call, as its HTTP status and body.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = op(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Assert one refusal: its status, EC2 error code and the exact sentence it carries.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
    expected_code: &str,
    expected_message: &str,
) {
    let (status, body) = op(router, action, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected the code {expected_code:?} from {action}, got {code:?} in \
         {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the message {expected_message:?} from {action}, got \
         {message:?}"
    );
}

/// EC2's sentence for a page size below the minimum an operation publishes: the value the
/// request sent, in spaced parentheses, and that operation's published minimum. Written
/// here rather than read from the service, so the suite pins the WIRE text.
fn below_minimum_message(value: i32, minimum: i32) -> String {
    format!(
        "Value ( {value} ) for parameter maxResults is invalid. \
         Expecting a value greater than {minimum}."
    )
}

/// EC2's sentence for a page size outside the range an operation publishes: the range, and
/// nothing of the request.
fn outside_range_message(minimum: i32, maximum: i32) -> String {
    format!("Maximum results allowed are between {minimum} and {maximum}")
}

// ---------------------------------------------------------------------------
// Page sizes
// ---------------------------------------------------------------------------

/// The paged describes that publish a page-size MINIMUM the model declares no `@range`
/// for, with that minimum: `(action, minimum)`.
///
/// Where each minimum comes from: `DescribeInstances` and `DescribeVolumes` publish five —
/// EC2's own refusal of a smaller page states the minimum it expects, and the model's own
/// `@range(min: 5)` on the `MaxResults` of the describes it DOES constrain
/// (`smithy_query(query="operation",operation="DescribeClientVpnEndpoints")`) is the same
/// bound; `DescribeVpcEndpointConnections` and `DescribeVpcEndpointServiceConfigurations`
/// document "this value can be between 5 and 1,000" on the member itself; the rest publish
/// no minimum above one item, and a page of NONE is not a page.
const PUBLISHED_PAGE_SIZE_MINIMUMS: [(&str, i32); 7] = [
    ("DescribeInstances", 5),
    ("DescribeVolumes", 5),
    ("DescribeVpcEndpointConnections", 5),
    ("DescribeVpcEndpointServiceConfigurations", 5),
    ("DescribeImages", 1),
    ("DescribeSnapshots", 1),
    ("DescribeVpcEndpointServices", 1),
];

/// A page size below the minimum an operation publishes is refused with EC2's sentence for
/// that violation, carrying the value the request sent and that operation's own minimum;
/// the minimum itself, and one above it, are SERVED.
#[tokio::test]
async fn published_page_size_minimums() {
    let router = emulator();

    for (action, minimum) in PUBLISHED_PAGE_SIZE_MINIMUMS {
        // Rows: one below the published minimum, the smallest page any describe serves, a
        // page of none, a negative page and a value far below it — the rule generalises
        // over the value's size and its sign.
        for value in [minimum - 1, 1, 0, -1, -1_000] {
            if value >= minimum {
                continue;
            }
            let case = format!("{action} with MaxResults={value}");
            refused(
                &router,
                &case,
                action,
                &[p("MaxResults", value.to_string())],
                "InvalidParameterValue",
                &below_minimum_message(value, minimum),
            )
            .await;
        }
        for value in [minimum, minimum + 1] {
            let case = format!("{action} with MaxResults={value}");
            ok(&router, &case, action, &[p("MaxResults", value.to_string())]).await;
        }
    }
}

/// The paged describes that publish a page-size RANGE the model declares no `@range` for,
/// with the bounds they publish and the fields a request of that operation needs beside the
/// page size: `(action, minimum, maximum, required fields)`.
///
/// Each range comes from the operation's own `MaxResults` documentation:
/// "this value can be between 1 and 200" (`DescribeLaunchTemplateVersions`), "this value can
/// be between 5 and 500" (`DescribeHosts`), "valid range: 5 - 500"
/// (`ListVolumesInRecycleBin`) and "the maximum number of results (up to a limit of 500)"
/// (`DescribeVolumesModifications`, whose floor is one item).
/// `DescribeLaunchTemplateVersions` describes versions of a template or of the version
/// aliases, so its row names the `$Latest` alias — a request naming neither is refused for
/// that, whatever its page size.
/// One row of [`PUBLISHED_PAGE_SIZE_RANGES`]: the operation, the minimum and maximum page
/// size it publishes, and the fields a request of it needs beside the page size.
type PageSizeRange = (&'static str, i32, i32, &'static [(&'static str, &'static str)]);

const PUBLISHED_PAGE_SIZE_RANGES: [PageSizeRange; 4] = [
    (
        "DescribeLaunchTemplateVersions",
        1,
        200,
        &[("LaunchTemplateVersion.1", "$Latest")],
    ),
    ("DescribeHosts", 5, 500, &[]),
    ("ListVolumesInRecycleBin", 5, 500, &[]),
    ("DescribeVolumesModifications", 1, 500, &[]),
];

/// A page size outside the range an operation publishes is refused with EC2's sentence for
/// that violation, which states the range and names no value; each published bound itself
/// is served.
#[tokio::test]
async fn published_page_size_ranges() {
    let router = emulator();

    for (action, minimum, maximum, required) in PUBLISHED_PAGE_SIZE_RANGES {
        let fields = |max_results: i32| {
            let mut params = vec![p("MaxResults", max_results.to_string())];
            params.extend(required.iter().map(|(key, value)| p(key, *value)));
            params
        };
        // Rows: one below the minimum, a page of none, a negative page, one past the
        // maximum and far past it.
        for value in [minimum - 1, 0, -1, maximum + 1, maximum * 1_000] {
            let case = format!("{action} with MaxResults={value}");
            refused(
                &router,
                &case,
                action,
                &fields(value),
                "InvalidParameterValue",
                &outside_range_message(minimum, maximum),
            )
            .await;
        }
        for value in [minimum, maximum] {
            let case = format!("{action} with MaxResults={value}");
            ok(&router, &case, action, &fields(value)).await;
        }
    }
}

/// The minimum page an operation publishes is a page that WORKS: it reports that many items
/// and a token that walks to the rest.
#[tokio::test]
async fn the_published_minimum_page_is_served_and_its_token_walks() {
    let router = emulator();
    let availability_zone = zone();

    // Six volumes, one more than the published minimum page of five, so the walk spans two
    // pages. Walked BEFORE any instance is launched: a launch creates a root volume of its
    // own, which this operation reports beside these.
    let mut volumes: Vec<String> = Vec::new();
    for size in [4_i32, 8, 12, 16, 20, 24] {
        let body = ok(
            &router,
            "seeding a volume",
            "CreateVolume",
            &[
                p("AvailabilityZone", availability_zone.clone()),
                p("Size", size.to_string()),
            ],
        )
        .await;
        volumes.push(element(&body, "volumeId").unwrap_or_default());
    }
    volumes.sort();
    walk_at_the_minimum_page(&router, "volumes", "DescribeVolumes", "volumeId", &volumes).await;

    // Six instances, the same way: one more than the page the operation publishes.
    let launched = ok(
        &router,
        "seeding six instances",
        "RunInstances",
        &[
            p("ImageId", LAUNCHABLE_AMI),
            p("MinCount", "6"),
            p("MaxCount", "6"),
        ],
    )
    .await;
    let instances = elements(&launched, "instanceId");
    assert_eq!(
        instances.len(),
        6,
        "seeding six instances: expected six ids, got {instances:?}"
    );
    walk_at_the_minimum_page(
        &router,
        "instances",
        "DescribeInstances",
        "instanceId",
        &instances,
    )
    .await;
}

/// Walk `action` at a page size of five — the minimum `DescribeInstances` and
/// `DescribeVolumes` publish — and assert the two pages cover `expected` exactly once, in
/// report order, with a token on the first page and none on the last.
async fn walk_at_the_minimum_page(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    item: &str,
    expected: &[String],
) {
    let first = ok(router, case, action, &[p("MaxResults", "5")]).await;
    let page = elements(&first, item);
    assert_eq!(
        page.len(),
        5,
        "case {case:?}: a page of five holds five items, got {page:?}"
    );
    let token = element(&first, "nextToken").unwrap_or_else(|| {
        panic!("case {case:?}: a sixth item follows the page, so a token is due: {first}")
    });
    let second = ok(
        router,
        case,
        action,
        &[p("MaxResults", "5"), p("NextToken", &token)],
    )
    .await;
    let mut walked = page;
    walked.extend(elements(&second, item));
    assert_eq!(
        walked, expected,
        "case {case:?}: the walk covers every item exactly once, in report order"
    );
    assert!(
        element(&second, "nextToken").is_none(),
        "case {case:?}: nothing follows the last page, so it carries no token: {second}"
    );
}

/// The rest of a paged describe's parameter refusals carry EC2's own code and sentence too,
/// not a description of the rule composed here: an unusable filter, a `Filters` entry that
/// names no filter, a pagination token the service never issued, and a page size beside an
/// explicit id selection.
///
/// `DescribeInstances` is the operation asserted, because it is the one whose refusals the
/// recorded run exercised; each sentence is the one every other paged describe of this
/// service already answers for the same refusal.
#[tokio::test]
async fn a_paged_describes_other_parameter_refusals_carry_ec2s_own_sentences() {
    let router = emulator();

    // (case, the form fields, the EC2 code, the sentence)
    type RefusalRow = (&'static str, Vec<(String, String)>, &'static str, &'static str);
    let rows: [RefusalRow; 4] = [
        (
            "a filter the operation does not resolve",
            vec![
                p("Filter.1.Name", "not-a-filter"),
                p("Filter.1.Value.1", "anything"),
            ],
            "InvalidParameterValue",
            "The filter 'not-a-filter' is invalid",
        ),
        (
            "a filter entry that names no filter",
            vec![p("Filter.1.Value.1", "anything")],
            "InvalidParameterValue",
            "The filter 'null' is invalid",
        ),
        (
            "a pagination token the service never issued",
            vec![p("NextToken", "r-nope")],
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        ),
        (
            "a page size beside an explicit id selection",
            vec![
                p("InstanceId.1", "i-0123456789abcdef0"),
                p("MaxResults", "5"),
            ],
            "InvalidParameterCombination",
            "The parameter instancesSet cannot be used with the parameter maxResults",
        ),
    ];
    for (case, params, expected_code, expected_message) in rows {
        refused(
            &router,
            case,
            "DescribeInstances",
            &params,
            expected_code,
            expected_message,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Instance counts
// ---------------------------------------------------------------------------

/// An AMI id of the form EC2 issues, so a launch below is refused for the counts it names
/// rather than for its image id.
const LAUNCHABLE_AMI: &str = "ami-0123456789abcdef0";

/// The instance type, platform and zone a Capacity Reservation below reserves.
const RESERVED_INSTANCE_TYPE: &str = "c5.xlarge";
const RESERVED_INSTANCE_PLATFORM: &str = "Linux/UNIX";

/// The form fields that reserve `instance_count` instances.
fn reservation_params(instance_count: i64) -> Vec<(String, String)> {
    vec![
        p("InstanceType", RESERVED_INSTANCE_TYPE),
        p("InstancePlatform", RESERVED_INSTANCE_PLATFORM),
        p("AvailabilityZone", zone()),
        p("InstanceCount", instance_count.to_string()),
    ]
}

/// EC2's sentence for an instance count outside the range the action publishes: the range
/// stated as the rule, with nothing of the request in it.
fn instance_count_message(range: &std::ops::RangeInclusive<i32>) -> String {
    format!(
        "The instance count isn't valid. Enter a value between {} and {} instances, \
         and try again",
        range.start(),
        range.end()
    )
}

/// An instance count outside the published range is refused with the sentence that states
/// that range, both where a reservation is created and where capacity is allocated out of
/// one — and a count INSIDE it is served.
#[tokio::test]
async fn published_instance_count_range() {
    let router = emulator();
    let published = ec2::state::CAPACITY_RESERVATION_INSTANCE_COUNT_RANGE;
    let expected = instance_count_message(&published);
    let lowest = *published.start();
    let highest = *published.end();

    // Rows: a count of none, a negative count, one past the published maximum and far past
    // it — the rule generalises over the value, not over one recorded number.
    for value in [i64::from(lowest) - 1, -1, i64::from(highest) + 1, 1_000_000] {
        let case = format!("CreateCapacityReservation with InstanceCount={value}");
        refused(
            &router,
            &case,
            "CreateCapacityReservation",
            &reservation_params(value),
            "InvalidParameterValue",
            &expected,
        )
        .await;
    }
    for value in [i64::from(lowest), i64::from(highest)] {
        let case = format!("CreateCapacityReservation with InstanceCount={value}");
        ok(
            &router,
            &case,
            "CreateCapacityReservation",
            &reservation_params(value),
        )
        .await;
    }

    // The same range on the allocation that takes capacity OUT of a reservation: the
    // parameter's own range is answered before the reservation's available capacity is
    // looked at, so a count far above both reports the range rather than the availability.
    let source = ok(
        &router,
        "the source reservation",
        "CreateCapacityReservation",
        &reservation_params(2),
    )
    .await;
    let source_id = element(&source, "capacityReservationId").unwrap_or_default();
    // The same range on the MODIFY that changes a reservation's size: a count below the
    // published floor reserves nothing, and the modify states that range in the one sentence
    // the create states it in rather than a second wording of its own.
    for value in [i64::from(lowest) - 1, -1] {
        let case = format!("ModifyCapacityReservation with InstanceCount={value}");
        refused(
            &router,
            &case,
            "ModifyCapacityReservation",
            &[
                p("CapacityReservationId", &source_id),
                p("InstanceCount", value.to_string()),
            ],
            "InvalidParameterValue",
            &expected,
        )
        .await;
    }
    for value in [i64::from(lowest) - 1, i64::from(highest) + 1, 1_000_000] {
        let case = format!("an allocation of InstanceCount={value}");
        refused(
            &router,
            &case,
            "CreateInterruptibleCapacityReservationAllocation",
            &[
                p("CapacityReservationId", &source_id),
                p("InstanceCount", value.to_string()),
            ],
            "InvalidParameterValue",
            &expected,
        )
        .await;
    }

    // A count INSIDE the published range that the source cannot satisfy is a different
    // refusal: what is available is a fact of the reservation, not of the parameter.
    let (status, body) = op(
        &router,
        "CreateInterruptibleCapacityReservationAllocation",
        &[
            p("CapacityReservationId", &source_id),
            p("InstanceCount", "3"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a count above the available capacity: expected HTTP 400, got {status} with {body}"
    );
    let message = error_message(&body);
    assert_ne!(
        message, expected,
        "a count above the available capacity reports the reservation's capacity, not the \
         published range, got {message:?}"
    );

    // And one the source CAN satisfy is served, and the capacity it took is read back off
    // the source through the describe.
    let allocated = ok(
        &router,
        "an allocation the source can satisfy",
        "CreateInterruptibleCapacityReservationAllocation",
        &[
            p("CapacityReservationId", &source_id),
            p("InstanceCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        element(&allocated, "targetInstanceCount").as_deref(),
        Some("1"),
        "an allocation the source can satisfy reports the count it allocated, got {allocated}"
    );
    let described = ok(
        &router,
        "reading the source back",
        "DescribeCapacityReservations",
        &[p("CapacityReservationId.1", &source_id)],
    )
    .await;
    assert_eq!(
        element(&described, "availableInstanceCount").as_deref(),
        Some("1"),
        "the source of two instances has one left after allocating one, got {described}"
    );
}

/// A launch whose `MaxCount` is below its `MinCount` is refused with EC2's one fixed
/// sentence for that violation, whatever the two counts are; a pair in order is served.
#[tokio::test]
async fn run_instances_refuses_a_max_count_below_the_min_count() {
    let router = emulator();
    let expected = "Maximum instance count must not be smaller than minimum instance count";

    // Rows: the pair adjacent, far apart, and at the largest counts a launch may name — the
    // sentence is fixed, so no row's values appear in it.
    for (min_count, max_count) in [(2, 1), (5, 4), (10, 1), (1_000, 999)] {
        let case = format!("MinCount={min_count} with MaxCount={max_count}");
        refused(
            &router,
            &case,
            "RunInstances",
            &[
                p("ImageId", LAUNCHABLE_AMI),
                p("MinCount", min_count.to_string()),
                p("MaxCount", max_count.to_string()),
            ],
            "InvalidParameterValue",
            expected,
        )
        .await;
    }
    for (min_count, max_count) in [(1, 1), (1, 3), (2, 2)] {
        let case = format!("MinCount={min_count} with MaxCount={max_count}");
        let body = ok(
            &router,
            &case,
            "RunInstances",
            &[
                p("ImageId", LAUNCHABLE_AMI),
                p("MinCount", min_count.to_string()),
                p("MaxCount", max_count.to_string()),
            ],
        )
        .await;
        assert_eq!(
            elements(&body, "instanceId").len(),
            max_count as usize,
            "case {case:?}: a launch in order mints MaxCount instances, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Attribute selection
// ---------------------------------------------------------------------------

/// Every attribute `ModifyVpcAttribute` can set, as the request's member and as the
/// attribute name the refusal below names it by — the three members of
/// `ModifyVpcAttributeRequest`, whose names are the `VpcAttributeName` values
/// (`smithy_query(query="shape",shape="VpcAttributeName")`).
const VPC_ATTRIBUTES: [(&str, &str); 3] = [
    ("EnableDnsSupport.Value", "enableDnsSupport"),
    ("EnableDnsHostnames.Value", "enableDnsHostnames"),
    (
        "EnableNetworkAddressUsageMetrics.Value",
        "enableNetworkAddressUsageMetrics",
    ),
];

/// A request that names more than one attribute type is refused with EC2's sentence for it,
/// listing the attributes the REQUEST named; a request naming one is applied and reads back.
#[tokio::test]
async fn modify_vpc_attribute_refuses_fields_for_multiple_attribute_types() {
    let router = emulator();
    let created = ok(
        &router,
        "the VPC the attributes are set on",
        "CreateVpc",
        &[p("CidrBlock", "10.7.0.0/16")],
    )
    .await;
    let vpc_id = element(&created, "vpcId").unwrap_or_default();

    // Rows: every PAIR of the three attributes, and all three at once — the rule is about
    // naming more than one type, not about one documented pair.
    let mut pairings: Vec<Vec<usize>> = vec![vec![0, 1], vec![0, 2], vec![1, 2], vec![0, 1, 2]];
    // The same pair the other way round reaches the service as the same request: ec2Query
    // fields are named, not ordered, so the refusal lists the attributes in the order the
    // request's members are declared.
    pairings.push(vec![2, 0]);
    for named in pairings {
        let mut params = vec![p("VpcId", &vpc_id)];
        for index in &named {
            params.push(p(VPC_ATTRIBUTES[*index].0, "true"));
        }
        let mut listed: Vec<usize> = named.clone();
        listed.sort_unstable();
        let expected = format!(
            "Fields for multiple attribute types specified: {}",
            listed
                .iter()
                .map(|index| VPC_ATTRIBUTES[*index].1)
                .collect::<Vec<&str>>()
                .join(", ")
        );
        let case = format!("the attributes {listed:?} in one request");
        refused(
            &router,
            &case,
            "ModifyVpcAttribute",
            &params,
            "InvalidParameterCombination",
            &expected,
        )
        .await;
    }

    // One attribute at a time IS applied, and the VPC reports it back.
    for (member, attribute) in VPC_ATTRIBUTES {
        let case = format!("{attribute} alone");
        ok(
            &router,
            &case,
            "ModifyVpcAttribute",
            &[p("VpcId", &vpc_id), p(member, "true")],
        )
        .await;
        let described = ok(
            &router,
            &case,
            "DescribeVpcAttribute",
            &[p("VpcId", &vpc_id), p("Attribute", attribute)],
        )
        .await;
        assert_eq!(
            element(&described, "value").as_deref(),
            Some("true"),
            "case {case:?}: the attribute reads back as it was set, got {described}"
        );
    }
}

/// The `InstanceAttributeName` values `ResetInstanceAttribute` cannot act on — every member
/// of that enum (`smithy_query(query="shape",shape="InstanceAttributeName"`) except the
/// three the operation documents as resettable.
const NON_RESETTABLE_INSTANCE_ATTRIBUTES: [&str; 13] = [
    "instanceType",
    "userData",
    "disableApiTermination",
    "instanceInitiatedShutdownBehavior",
    "rootDeviceName",
    "blockDeviceMapping",
    "productCodes",
    "groupSet",
    "ebsOptimized",
    "sriovNetSupport",
    "enaSupport",
    "enclaveOptions",
    "disableApiStop",
];

/// An `attribute` the operation cannot act on is refused with EC2's sentence for an unknown
/// attribute — the value the request sent, and no enumeration of the ones it does accept;
/// an attribute it DOES reset is applied and reads back.
#[tokio::test]
async fn reset_instance_attribute_refuses_an_unknown_attribute() {
    let router = emulator();
    let launched = ok(
        &router,
        "the instance whose attributes are reset",
        "RunInstances",
        &[
            p("ImageId", LAUNCHABLE_AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    let instance_id = element(&launched, "instanceId").unwrap_or_default();

    for attribute in NON_RESETTABLE_INSTANCE_ATTRIBUTES {
        let case = format!("a reset of {attribute}");
        let expected = format!(
            "Value ({attribute}) for parameter attribute is invalid. Unknown attribute."
        );
        refused(
            &router,
            &case,
            "ResetInstanceAttribute",
            &[
                p("InstanceId", &instance_id),
                p("Attribute", attribute),
            ],
            "InvalidParameterValue",
            &expected,
        )
        .await;
    }

    // The attribute this instance's state admits IS reset, and the reset value reads back.
    ok(
        &router,
        "a reset of sourceDestCheck",
        "ResetInstanceAttribute",
        &[
            p("InstanceId", &instance_id),
            p("Attribute", "sourceDestCheck"),
        ],
    )
    .await;
    let described = ok(
        &router,
        "reading sourceDestCheck back",
        "DescribeInstanceAttribute",
        &[
            p("InstanceId", &instance_id),
            p("Attribute", "sourceDestCheck"),
        ],
    )
    .await;
    assert_eq!(
        element(&described, "value").as_deref(),
        Some("true"),
        "the reset attribute reads back at its default, got {described}"
    );
}

// ---------------------------------------------------------------------------
// Host configuration
// ---------------------------------------------------------------------------

/// Hardware EC2 does not offer makes a host CONFIGURATION impossible, which carries EC2's
/// configuration code and its own sentence rather than the parameter-value code — a client
/// that branches on the code must see the one the real service answers. A type EC2 does
/// offer is allocated.
#[tokio::test]
async fn allocate_hosts_refuses_hardware_ec2_does_not_offer() {
    let router = emulator();
    let availability_zone = zone();

    // Rows: a type in no family EC2 offers, a type whose family exists at another size, a
    // value that is no instance type at all, and an unknown FAMILY — the other way one
    // request configures one host.
    let rows: [(&str, &str, &str, String); 4] = [
        (
            "a type EC2 does not offer",
            "InstanceType",
            "mac9.metal",
            "The instance type 'mac9.metal' is not supported.".to_string(),
        ),
        (
            "a size no family offers",
            "InstanceType",
            "c5.enormous",
            "The instance type 'c5.enormous' is not supported.".to_string(),
        ),
        (
            "a value that is no instance type at all",
            "InstanceType",
            "not-a-type",
            "The instance type 'not-a-type' is not supported.".to_string(),
        ),
        (
            "a family EC2 does not offer",
            "InstanceFamily",
            "mac9",
            "The instance family 'mac9' is not supported.".to_string(),
        ),
    ];
    for (case, member, value, expected) in rows {
        refused(
            &router,
            case,
            "AllocateHosts",
            &[
                p(member, value),
                p("AvailabilityZone", availability_zone.clone()),
                p("Quantity", "1"),
            ],
            ec2::state::INVALID_HOST_CONFIGURATION,
            &expected,
        )
        .await;
    }

    let allocated = ok(
        &router,
        "a type EC2 does offer",
        "AllocateHosts",
        &[
            p("InstanceType", "mac2.metal"),
            p("AvailabilityZone", availability_zone.clone()),
            p("Quantity", "1"),
        ],
    )
    .await;
    let host_ids = elements(&allocated, "item");
    assert_eq!(
        host_ids.len(),
        1,
        "a supported type allocates one host, got {allocated}"
    );
}

/// A value that names no Availability Zone of the signed Region leaves the request with
/// nowhere to put the host, so the REQUEST is what is invalid: EC2's `InvalidRequest` with
/// the sentence that names the member as the model declares it and the value the request
/// sent, for either of the two members a zone may be named by. A zone the Region DOES have
/// allocates.
#[tokio::test]
async fn allocate_hosts_refuses_a_zone_the_region_does_not_have() {
    let router = emulator();

    // Rows: a zone of another Region, a zone letter this Region does not reach, a value
    // that is no zone name at all, and the same three shapes for the zone ID member — the
    // rule is about the value naming no zone here, whichever member carries it.
    let rows: [(&str, &str, &str); 6] = [
        ("another Region's zone name", "AvailabilityZone", "eu-west-1a"),
        ("a zone letter this Region lacks", "AvailabilityZone", "us-east-1z"),
        ("a value that is no zone name", "AvailabilityZone", "not-a-zone"),
        ("another Region's zone id", "AvailabilityZoneId", "euw1-az1"),
        ("a zone id index this Region lacks", "AvailabilityZoneId", "use1-az99"),
        ("a value that is no zone id", "AvailabilityZoneId", "not-a-zone-id"),
    ];
    for (case, member, value) in rows {
        refused(
            &router,
            case,
            "AllocateHosts",
            &[
                p("InstanceType", "mac2.metal"),
                p(member, value),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
            &format!("Invalid value '{value}' for {member}."),
        )
        .await;
    }

    // The zone the Region does have is served, by either member.
    let zones = availability_zones_in_region(REGION);
    let here = &zones[0];
    for (member, value) in [
        ("AvailabilityZone", here.zone_name.clone()),
        ("AvailabilityZoneId", here.zone_id.clone()),
    ] {
        let case = format!("{member}={value}");
        let allocated = ok(
            &router,
            &case,
            "AllocateHosts",
            &[
                p("InstanceType", "mac2.metal"),
                p(member, value.clone()),
                p("Quantity", "1"),
            ],
        )
        .await;
        assert_eq!(
            elements(&allocated, "item").len(),
            1,
            "case {case:?}: a zone the Region has allocates one host, got {allocated}"
        );
    }
}
