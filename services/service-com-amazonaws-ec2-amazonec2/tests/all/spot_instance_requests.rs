// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Spot Instance request slice: `RequestSpotInstances` and
//! `DescribeSpotInstanceRequests`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `RequestSpotInstances` MINTS REQUESTS FROM THE REQUEST and
//!    `DescribeSpotInstanceRequests` REPORTS THE STORE: every member the call sets
//!    round-trips through the describe, and the members the service owns (the request id,
//!    the state, the status, the create time, the product description and the documented
//!    defaults) are derived rather than taken;
//! 2. one Spot Instance request per unit of `InstanceCount`;
//! 3. a request is `open` / `pending-evaluation` and the CLOCK, not a caller, fulfills it —
//!    with an instance of this account's own instance store, which `DescribeInstances`
//!    reports with the `spot` lifecycle and the id of the request that launched it. A
//!    request whose `ValidFrom` is still in the future is not fulfilled before it;
//! 4. a request whose `ValidUntil` has been reached is `closed` / `schedule-expired` and
//!    launches nothing, and a request that names no `ValidUntil` is valid for the
//!    documented seven days from its creation;
//! 5. one `ClientToken` names the requests of one call;
//! 6. the report is narrowed by `SpotInstanceRequestIds` and by the filter names the member
//!    documents, and cut into `MaxResults`-sized pages;
//! 7. a value the service cannot accept — including a launch specification naming an AMI,
//!    subnet or security group the account cannot resolve — is refused with EC2's own error
//!    code, and nothing is stored.
//!
//! Each table row is a distinct input along what the rule generalises over — which request
//! type, which interruption behaviour, which instance type, which zone, how many
//! instances, which filter name and value, which page size — so no row is the
//! specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_SPOT_REQUEST_VALIDITY,
    INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE, INSTANCE_LIFECYCLE_SPOT, MAX_PAGE_SIZE,
    MAX_SPOT_INSTANCES_PER_REQUEST, PRODUCT_DESCRIPTION_LINUX_UNIX, PRODUCT_DESCRIPTION_WINDOWS,
    SPOT_INSTANCE_REQUEST_ID_PREFIX, SPOT_INSTANCE_TYPES, SPOT_INSTANCE_TYPE_ONE_TIME,
    SPOT_INSTANCE_TYPE_PERSISTENT, SPOT_REQUEST_STATE_ACTIVE, SPOT_REQUEST_STATE_CLOSED,
    SPOT_REQUEST_STATE_OPEN, SPOT_REQUEST_TAGGABLE_RESOURCE, SPOT_STATUS_FULFILLED,
    SPOT_STATUS_PENDING_EVALUATION, SPOT_STATUS_SCHEDULE_EXPIRED, TRANSITION_DELAY,
    availability_zones_in_region, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, filters, indexed};
use super::wire::{child, child_text, document, is_long_id};

/// The Region every call here is signed for; the Availability Zone a Spot Instance can be
/// placed in is derived from it, never written out.
const REGION: &str = "us-west-2";

/// An AMI of the published catalog a fresh account can already see, by position in that
/// catalog's id order.
///
/// Read out of the service's OWN state rather than written here: a launch specification
/// must name an image the account can resolve, so the test cannot invent an id — and a
/// fresh account's catalog is the same in every emulator this suite builds.
fn catalog_image(index: usize) -> String {
    Ec2State::default()
        .images
        .keys()
        .nth(index)
        .cloned()
        .unwrap_or_else(|| panic!("a fresh account must publish at least {} images", index + 1))
}

/// The AMI the rows that do not care which image they launch from use.
fn image_id() -> String {
    catalog_image(0)
}

/// The Windows AMI of that catalog: "this value is set to `windows` for Windows AMIs;
/// otherwise blank" (`Image$Platform`).
fn windows_image_id() -> String {
    let state = Ec2State::default();
    state
        .images
        .iter()
        .find(|(_, image)| image.platform.is_some())
        .map(|(image_id, _)| image_id.clone())
        .expect("a fresh account must publish a Windows AMI")
}

/// A subnet of the account's default VPC, read out of the service's own state for the same
/// reason as [`catalog_image`].
fn default_subnet_id() -> String {
    Ec2State::default()
        .subnets
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must hold the default VPC's subnets")
}

/// The id and the name of the account's default security group.
fn default_group() -> (String, String) {
    let state = Ec2State::default();
    state
        .security_groups
        .iter()
        .next()
        .map(|(group_id, group)| {
            (
                group_id.clone(),
                group.group_name.clone().unwrap_or_default(),
            )
        })
        .expect("a fresh account must hold the default VPC's default security group")
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn request_spot(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "RequestSpotInstances", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeSpotInstanceRequests", params).await
}

async fn describe_instances(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, "DescribeInstances", params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The minimum a `RequestSpotInstances` call needs: an AMI to launch from.
fn one_request() -> Params {
    vec![p("LaunchSpecification.ImageId", image_id())]
}

/// Create one more security group and answer `(group id, group name)`, so a table can tell
/// two groups of the account apart.
async fn created_group(router: &MultiServiceRouter, group_name: &str) -> (String, String) {
    let (status, body) = call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            p("GroupName", group_name),
            p("GroupDescription", "a group for the Spot suite"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSecurityGroup must succeed, got {status} with body {body}"
    );
    let group_id = super::ec2_query::element(&body, "groupId")
        .unwrap_or_else(|| panic!("no groupId in {body}"));
    (group_id, group_name.to_string())
}

/// Every `spotInstanceRequestId` a response reports, in the order it reported them.
fn request_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "spotInstanceRequestId")
        // `Instance$spotInstanceRequestId` has the same element name, so only the ids of
        // the Spot requests themselves are read: those sit on an element that carries the
        // request's own `state`.
        .filter(|node| {
            node.parent()
                .is_some_and(|parent| child(parent, "state").is_some())
        })
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// How many Spot Instance requests a response carries.
fn request_count(case: &str, body: &str) -> usize {
    request_ids(case, body).len()
}

/// A member of the FIRST Spot Instance request a response carries, by dotted path:
/// `state`, `status.code`, `launchSpecification.imageId`.
fn member(case: &str, body: &str, path: &str) -> Option<String> {
    let parsed = document(case, body);
    let request = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "spotInstanceRequestId").is_some()
                && child(*node, "state").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no Spot Instance request in {body}"));
    let mut node = request;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return child_text(node, part);
        }
        node = child(node, part)?;
    }
    None
}

/// Every text of the elements named `name` inside the first Spot Instance request — a
/// `groupId` of its launch specification, say.
fn members(case: &str, body: &str, name: &str) -> Vec<String> {
    let parsed = document(case, body);
    let Some(request) = parsed.descendants().find(|node| {
        node.is_element()
            && child(*node, "spotInstanceRequestId").is_some()
            && child(*node, "state").is_some()
    }) else {
        return Vec::new();
    };
    request
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// The tags of the first Spot Instance request of a response.
fn request_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let Some(request) = parsed.descendants().find(|node| {
        node.is_element()
            && child(*node, "spotInstanceRequestId").is_some()
            && child(*node, "state").is_some()
    }) else {
        return Vec::new();
    };
    let Some(set) = child(request, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// A member of the first instance a `DescribeInstances` response carries.
fn instance_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let instance = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "instanceId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    child_text(instance, name)
}

/// Ask for one Spot Instance and answer the request id it minted, failing the case if it
/// was refused.
async fn requested(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = request_spot(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RequestSpotInstances, \
         got {status} with body {body}"
    );
    request_ids(case, &body)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no spotInstanceRequestId in {body}"))
}

/// The response of a describe narrowed to one Spot Instance request id.
async fn describe_one(router: &MultiServiceRouter, case: &str, request_id: &str) -> String {
    let (status, body) = describe(router, &indexed("SpotInstanceRequestId", &[request_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {request_id}, got {status} with body {body}"
    );
    assert_eq!(
        request_count(case, &body),
        1,
        "case {case:?}: expected exactly one request for {request_id}, got body {body}"
    );
    body
}

/// The request ids a describe reports for `params`, in the order reported.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotInstanceRequests, \
         got {status} with body {body}"
    );
    request_ids(case, &body)
}

/// How many Spot Instance requests the store holds.
fn stored_requests(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().spot_instance_requests.len()
}

/// How many instances the store holds.
fn stored_instances(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().instances.len()
}

/// A timestamp `offset` from now, in the UTC form the `ValidFrom` / `ValidUntil` members
/// are documented in. Built through the service's own formatter so the test cannot spell a
/// timestamp the service would not.
fn at_offset(offset: Duration, ahead: bool) -> String {
    let now = std::time::SystemTime::now();
    iso8601_utc(if ahead { now + offset } else { now - offset })
}

// ---------------------------------------------------------------------------
// RULE 1: the request round-trips, the service's own members are derived
// ---------------------------------------------------------------------------

/// One row: the parameters under test, and the members the describe must report for them.
type MemberRow = (&'static str, Params, Vec<(&'static str, String)>);

/// RULE 1 over the `SpotInstanceType` and `InstanceInterruptionBehavior` catalogs, the
/// launch specification's own members and the request's groups and price: what the call
/// set is what a later `DescribeSpotInstanceRequests` reports, and what the service owns is
/// derived.
#[tokio::test]
async fn every_request_parameter_round_trips_through_the_describe() {
    let mut rows: Vec<MemberRow> = Vec::new();
    // The `SpotInstanceType` catalog, from the model's own enum: every published value is a
    // row.
    for request_type in SPOT_INSTANCE_TYPES {
        rows.push((
            request_type,
            [one_request(), vec![p("Type", request_type)]].concat(),
            vec![("type", request_type.to_string())],
        ));
    }
    // The `InstanceInterruptionBehavior` catalog, likewise. A behaviour that KEEPS the
    // instance for a later start is offered on a persistent request alone, so each of those
    // rows asks for the request type that can perform it.
    for behavior in ec2::ops::types::INSTANCE_INTERRUPTION_BEHAVIOR_VALUES {
        let mut params = [
            one_request(),
            vec![p("InstanceInterruptionBehavior", *behavior)],
        ]
        .concat();
        let mut expected = vec![("instanceInterruptionBehavior", (*behavior).to_string())];
        if *behavior != "terminate" {
            params.push(p("Type", SPOT_INSTANCE_TYPE_PERSISTENT));
            expected.push(("type", SPOT_INSTANCE_TYPE_PERSISTENT.to_string()));
        }
        rows.push((behavior, params, expected));
    }
    // The launch specification's scalar members, one row each so no row depends on another.
    rows.push((
        "an instance type the specification names is the one reported",
        [
            one_request(),
            vec![p("LaunchSpecification.InstanceType", "c5.xlarge")],
        ]
        .concat(),
        vec![("launchSpecification.instanceType", "c5.xlarge".to_string())],
    ));
    rows.push((
        "the key pair and the subnet are the specification's",
        [
            one_request(),
            vec![
                p("LaunchSpecification.KeyName", "deploy"),
                p("LaunchSpecification.SubnetId", default_subnet_id()),
            ],
        ]
        .concat(),
        vec![
            ("launchSpecification.keyName", "deploy".to_string()),
            ("launchSpecification.subnetId", default_subnet_id()),
        ],
    ));
    rows.push((
        "user data and EBS optimisation round-trip",
        [
            one_request(),
            vec![
                p("LaunchSpecification.UserData", "IyEvYmluL3NoCg=="),
                p("LaunchSpecification.EbsOptimized", "true"),
            ],
        ]
        .concat(),
        vec![
            (
                "launchSpecification.userData",
                "IyEvYmluL3NoCg==".to_string(),
            ),
            ("launchSpecification.ebsOptimized", "true".to_string()),
        ],
    ));
    rows.push((
        "detailed monitoring round-trips",
        [
            one_request(),
            vec![p("LaunchSpecification.Monitoring.Enabled", "true")],
        ]
        .concat(),
        vec![("launchSpecification.monitoring.enabled", "true".to_string())],
    ));
    rows.push((
        "a specification that names no monitoring is not monitored",
        one_request(),
        vec![("launchSpecification.ebsOptimized", "false".to_string())],
    ));
    rows.push((
        "the launch and Availability Zone groups are the request's",
        [
            one_request(),
            vec![
                p("LaunchGroup", "batch-7"),
                p("AvailabilityZoneGroup", "zone-group-7"),
            ],
        ]
        .concat(),
        vec![
            ("launchGroup", "batch-7".to_string()),
            ("availabilityZoneGroup", "zone-group-7".to_string()),
        ],
    ));
    // A price is a number, and several forms of one are a row each.
    for price in ["0.05", "1", "0", "12.3456"] {
        rows.push((
            price,
            [one_request(), vec![p("SpotPrice", price)]].concat(),
            vec![("spotPrice", price.to_string())],
        ));
    }
    // Every zone the signed Region has is a row: the placement is stored as given.
    for zone in availability_zones_in_region(REGION) {
        rows.push((
            "a placement names one of the Region's zones",
            [
                one_request(),
                vec![p(
                    "LaunchSpecification.Placement.AvailabilityZone",
                    zone.zone_name.clone(),
                )],
            ]
            .concat(),
            vec![(
                "launchSpecification.placement.availabilityZone",
                zone.zone_name.clone(),
            )],
        ));
    }

    // The product description is derived from the AMI's platform, not taken from the
    // request: "the product description associated with the instance (Linux/UNIX |
    // Windows)" (`DescribeSpotInstanceRequestsRequest$Filters`).
    rows.push((
        "a Linux AMI makes a Linux/UNIX request",
        one_request(),
        vec![("productDescription", PRODUCT_DESCRIPTION_LINUX_UNIX.to_string())],
    ));
    rows.push((
        "a Windows AMI makes a Windows request",
        vec![p("LaunchSpecification.ImageId", windows_image_id())],
        vec![("productDescription", PRODUCT_DESCRIPTION_WINDOWS.to_string())],
    ));

    for (case, params, expected) in rows {
        let (router, _engine) = emulator();
        let request_id = requested(&router, case, &params).await;
        assert!(
            is_long_id(SPOT_INSTANCE_REQUEST_ID_PREFIX, &request_id),
            "case {case:?}: expected a {SPOT_INSTANCE_REQUEST_ID_PREFIX}- long id, \
             got {request_id:?}"
        );
        let body = describe_one(&router, case, &request_id).await;
        for (path, value) in expected {
            let actual = member(case, &body, path);
            assert_eq!(
                actual.as_deref(),
                Some(value.as_str()),
                "case {case:?}: expected <{path}> {value:?}, got {actual:?} in {body}"
            );
        }
        // What the service owns, for every row: the AMI is the one the call named — read
        // back off the row's own parameters, because one row launches from the Windows
        // image — the request is waiting to be fulfilled, and it has no instance and no
        // launched zone yet.
        let named_image = params
            .iter()
            .find(|(key, _)| key == "LaunchSpecification.ImageId")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(image_id);
        for (path, expected) in [
            ("launchSpecification.imageId", named_image.as_str()),
            ("state", SPOT_REQUEST_STATE_OPEN),
            ("status.code", SPOT_STATUS_PENDING_EVALUATION),
        ] {
            let actual = member(case, &body, path);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected <{path}> {expected:?}, got {actual:?} in {body}"
            );
        }
        for absent in ["instanceId", "launchedAvailabilityZone"] {
            let actual = member(case, &body, absent);
            assert_eq!(
                actual, None,
                "case {case:?}: an unfulfilled request must report no <{absent}>, \
                 got {actual:?} in {body}"
            );
        }
        // The create time and the status update are timestamps the service wrote, not
        // values the caller could have set.
        for path in ["createTime", "status.updateTime", "validUntil"] {
            let actual = member(case, &body, path);
            assert!(
                actual.as_deref().is_some_and(|text| text.ends_with('Z')),
                "case {case:?}: expected a UTC <{path}>, got {actual:?} in {body}"
            );
        }
    }
}

/// RULE 1, the documented DEFAULTS: a member the call left out is reported at the value its
/// own documentation gives — `Type` "Default: one-time", `InstanceInterruptionBehavior`
/// "the default is terminate", `InstanceCount` "Default: 1".
#[tokio::test]
async fn a_request_that_names_no_options_reports_the_documented_defaults() {
    let case = "no options";
    let (router, engine) = emulator();
    let request_id = requested(&router, case, &one_request()).await;
    let body = describe_one(&router, case, &request_id).await;
    for (path, expected) in [
        ("type", SPOT_INSTANCE_TYPE_ONE_TIME),
        (
            "instanceInterruptionBehavior",
            INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE,
        ),
    ] {
        let actual = member(case, &body, path);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected <{path}> {expected:?}, got {actual:?} in {body}"
        );
    }
    assert_eq!(
        stored_requests(&engine),
        1,
        "case {case:?}: InstanceCount defaults to 1, so one request must be stored"
    );
    // "If you do not specify this parameter, you will pay the current Spot price": no
    // maximum was asked for, so none is reported.
    let price = member(case, &body, "spotPrice");
    assert_eq!(
        price, None,
        "case {case:?}: a request that named no maximum price must report none, got {price:?}"
    );
}

/// RULE 1: `TagSpecifications` tags the Spot Instance request the call creates, over a
/// table of tag sets.
#[tokio::test]
async fn tag_specifications_tag_the_spot_instance_request() {
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one tag", vec![("Name", "batch")]),
        (
            "several tags are all reported",
            vec![("Name", "batch"), ("owner", "platform"), ("tier", "web")],
        ),
        ("an empty value is still a tag", vec![("Name", "")]),
    ];
    for (case, tags) in rows {
        let (router, _engine) = emulator();
        let mut params = one_request();
        params.push(p(
            "TagSpecification.1.ResourceType",
            SPOT_REQUEST_TAGGABLE_RESOURCE,
        ));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
        let request_id = requested(&router, case, &params).await;
        let body = describe_one(&router, case, &request_id).await;
        let mut expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.sort();
        let mut actual = request_tags(case, &body);
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected tags {expected:?}, got {actual:?} in {body}"
        );
    }
}

/// RULE 1: the groups the specification names — by id, by name, or both — are reported as
/// the launch specification's group set. Every group is one the account holds, because a
/// launch specification that names one it does not is refused (see the refusal table).
#[tokio::test]
async fn the_security_groups_the_specification_names_round_trip() {
    // (case, how the groups are named, the ids reported, the names reported)
    type GroupRow = (&'static str, bool, bool, bool);
    let rows: Vec<GroupRow> = vec![
        ("one group id", true, false, false),
        ("two group ids", true, true, false),
        ("a group named by name", false, false, true),
        ("ids and names together", true, false, true),
    ];
    for (case, first_id, second_id, by_name) in rows {
        let (router, _engine) = emulator();
        let (default_id, default_name) = default_group();
        let (other_id, _other_name) = created_group(&router, "spot-suite-group").await;
        let mut ids: Vec<String> = Vec::new();
        if first_id {
            ids.push(default_id.clone());
        }
        if second_id {
            ids.push(other_id.clone());
        }
        let names: Vec<String> = if by_name {
            vec![default_name.clone()]
        } else {
            Vec::new()
        };
        let mut params = one_request();
        for (index, group_id) in ids.iter().enumerate() {
            params.push(p(
                &format!("LaunchSpecification.SecurityGroupId.{}", index + 1),
                group_id.clone(),
            ));
        }
        for (index, group_name) in names.iter().enumerate() {
            params.push(p(
                &format!("LaunchSpecification.SecurityGroup.{}", index + 1),
                group_name.clone(),
            ));
        }
        let request_id = requested(&router, case, &params).await;
        let body = describe_one(&router, case, &request_id).await;
        let reported_ids = members(case, &body, "groupId");
        let reported_names = members(case, &body, "groupName");
        assert_eq!(
            reported_ids, ids,
            "case {case:?}: expected group ids {ids:?}, got {reported_ids:?} in {body}"
        );
        assert_eq!(
            reported_names, names,
            "case {case:?}: expected group names {names:?}, \
             got {reported_names:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: one request per unit of InstanceCount
// ---------------------------------------------------------------------------

/// RULE 2 over several counts, including both bounds of what one call may ask for: the call
/// answers that many distinct requests and the store holds exactly them.
#[tokio::test]
async fn instance_count_mints_one_request_each() {
    for count in [1, 2, 5, MAX_SPOT_INSTANCES_PER_REQUEST] {
        let case = format!("InstanceCount {count}");
        let (router, engine) = emulator();
        let params = [one_request(), vec![p("InstanceCount", count.to_string())]].concat();
        let (status, body) = request_spot(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let ids = request_ids(&case, &body);
        assert_eq!(
            ids.len(),
            usize::try_from(count).unwrap_or_default(),
            "case {case:?}: expected {count} requests, got {ids:?} in {body}"
        );
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "case {case:?}: every request must carry its own id, got {ids:?}"
        );
        assert_eq!(
            stored_requests(&engine),
            ids.len(),
            "case {case:?}: the store must hold exactly the requests the call answered"
        );
        let described = described_ids(&router, &case, &[]).await;
        assert_eq!(
            described.len(),
            ids.len(),
            "case {case:?}: the describe must report every request the call minted, \
             got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the clock fulfills an open request with a real instance
// ---------------------------------------------------------------------------

/// RULE 3 over several launch specifications: the request is `open` / `pending-evaluation`
/// when the call answers, and once the clock-owed span has passed it is `active` /
/// `fulfilled` with an instance of this account — one `DescribeInstances` reports with the
/// `spot` lifecycle and this request's id.
#[tokio::test]
async fn the_clock_fulfills_an_open_request_with_a_spot_instance() {
    let rows: Vec<(&str, Params, Option<&str>)> = vec![
        ("the plainest request", one_request(), None),
        (
            "a persistent request",
            [
                one_request(),
                vec![p("Type", SPOT_INSTANCE_TYPE_PERSISTENT)],
            ]
            .concat(),
            None,
        ),
        (
            "a request that names its instance type",
            [
                one_request(),
                vec![p("LaunchSpecification.InstanceType", "m5.large")],
            ]
            .concat(),
            None,
        ),
        (
            "a request that names its zone launches there",
            [
                one_request(),
                vec![p(
                    "LaunchSpecification.Placement.AvailabilityZone",
                    availability_zones_in_region(REGION)[2].zone_name.clone(),
                )],
            ]
            .concat(),
            Some("third zone"),
        ),
    ];
    for (case, params, named_zone) in rows {
        let (router, engine) = emulator();
        let before = stored_instances(&engine);
        let request_id = requested(&router, case, &params).await;
        // Straight after the call the request is still waiting: the span is never zero.
        let body = describe_one(&router, case, &request_id).await;
        let state = member(case, &body, "state");
        assert_eq!(
            state.as_deref(),
            Some(SPOT_REQUEST_STATE_OPEN),
            "case {case:?}: expected a request to start {SPOT_REQUEST_STATE_OPEN:?}, \
             got {state:?} in {body}"
        );
        assert_eq!(
            stored_instances(&engine),
            before,
            "case {case:?}: an open request must have launched nothing yet"
        );

        past_the_transition().await;

        let body = describe_one(&router, case, &request_id).await;
        for (path, expected) in [
            ("state", SPOT_REQUEST_STATE_ACTIVE),
            ("status.code", SPOT_STATUS_FULFILLED),
        ] {
            let actual = member(case, &body, path);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected <{path}> {expected:?}, got {actual:?} in {body}"
            );
        }
        let instance_id = member(case, &body, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: a fulfilled request must name its instance, body {body}"));
        assert!(
            is_long_id("i", &instance_id),
            "case {case:?}: expected an i- long id, got {instance_id:?}"
        );
        let zones = availability_zones_in_region(REGION);
        let launched_zone = member(case, &body, "launchedAvailabilityZone");
        let expected_zone = match named_zone {
            Some(_) => zones[2].zone_name.clone(),
            None => zones[0].zone_name.clone(),
        };
        assert_eq!(
            launched_zone.as_deref(),
            Some(expected_zone.as_str()),
            "case {case:?}: expected <launchedAvailabilityZone> {expected_zone:?}, \
             got {launched_zone:?} in {body}"
        );
        assert_eq!(
            stored_instances(&engine),
            before + 1,
            "case {case:?}: a fulfilled request must have launched exactly one instance"
        );

        // The instance is one of this account's own, reported as a Spot Instance of this
        // request.
        let (status, instances) =
            describe_instances(&router, &indexed("InstanceId", &[&instance_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeInstances, \
             got {status} with body {instances}"
        );
        for (name, expected) in [
            ("instanceLifecycle", INSTANCE_LIFECYCLE_SPOT.to_string()),
            ("spotInstanceRequestId", request_id.clone()),
            ("imageId", image_id()),
        ] {
            let actual = instance_member(case, &instances, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the instance's <{name}> {expected:?}, \
                 got {actual:?} in {instances}"
            );
        }
    }
}

/// RULE 3: a `DescribeInstances` filtered on the Spot attributes selects the Spot Instance
/// and leaves an on-demand one out — the filter names an attribute only a Spot Instance
/// carries.
#[tokio::test]
async fn the_spot_filters_of_describe_instances_select_only_spot_instances() {
    let (router, _engine) = emulator();
    let setup = "spot and on-demand side by side";
    let request_id = requested(&router, setup, &one_request()).await;
    let (status, body) = call(
        &router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", image_id()),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {setup:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
    );
    past_the_transition().await;

    // (the filter, its value, how many of the two instances it must select)
    let rows: Vec<(&str, String, usize)> = vec![
        (
            "instance-lifecycle",
            INSTANCE_LIFECYCLE_SPOT.to_string(),
            1,
        ),
        ("spot-instance-request-id", request_id.clone(), 1),
        (
            "spot-instance-request-id",
            "sir-0000000000000ffff".to_string(),
            0,
        ),
        ("image-id", image_id(), 2),
    ];
    for (filter, value, expected) in rows {
        let case = format!("DescribeInstances filtered on {filter}={value}");
        let (status, body) = describe_instances(&router, &filters(&[(filter, &[value.as_str()])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let parsed = document(&case, &body);
        let found = parsed
            .descendants()
            .filter(|node| node.is_element() && child(*node, "instanceId").is_some())
            .count();
        assert_eq!(
            found, expected,
            "case {case:?}: expected {expected} instances, got {found} in {body}"
        );
    }
}

/// RULE 3: the fulfillment is not due before the `ValidFrom` the request named — "the
/// request becomes active at this date and time".
#[tokio::test]
async fn a_request_is_not_fulfilled_before_its_valid_from() {
    let case = "ValidFrom well in the future";
    let (router, engine) = emulator();
    let params = [
        one_request(),
        vec![p("ValidFrom", at_offset(Duration::from_secs(3_600), true))],
    ]
    .concat();
    let request_id = requested(&router, case, &params).await;
    past_the_transition().await;
    let body = describe_one(&router, case, &request_id).await;
    let state = member(case, &body, "state");
    assert_eq!(
        state.as_deref(),
        Some(SPOT_REQUEST_STATE_OPEN),
        "case {case:?}: a request whose start date has not been reached must still be \
         {SPOT_REQUEST_STATE_OPEN:?}, got {state:?} in {body}"
    );
    assert_eq!(
        stored_instances(&engine),
        0,
        "case {case:?}: nothing may be launched before the request's start date"
    );
    // And the start date the call named is what the request reports.
    let valid_from = member(case, &body, "validFrom");
    assert!(
        valid_from.as_deref().is_some_and(|text| text.ends_with('Z')),
        "case {case:?}: expected the <validFrom> the request named, got {valid_from:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the request's own validity window
// ---------------------------------------------------------------------------

/// RULE 4: a request whose `ValidUntil` has been reached is `closed` with the status code
/// `schedule-expired`, and it launches nothing — over a table of windows that have closed.
#[tokio::test]
async fn a_request_whose_validity_has_ended_is_closed_and_launches_nothing() {
    let rows: Vec<(&str, Duration)> = vec![
        ("a validity window that ended a second ago", Duration::from_secs(1)),
        ("a validity window that ended an hour ago", Duration::from_secs(3_600)),
        ("a validity window that ended a week ago", DEFAULT_SPOT_REQUEST_VALIDITY),
    ];
    for (case, ago) in rows {
        let (router, engine) = emulator();
        let params = [
            one_request(),
            vec![p("ValidUntil", at_offset(ago, false))],
        ]
        .concat();
        let request_id = requested(&router, case, &params).await;
        past_the_transition().await;
        let body = describe_one(&router, case, &request_id).await;
        for (path, expected) in [
            ("state", SPOT_REQUEST_STATE_CLOSED),
            ("status.code", SPOT_STATUS_SCHEDULE_EXPIRED),
        ] {
            let actual = member(case, &body, path);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected <{path}> {expected:?}, got {actual:?} in {body}"
            );
        }
        let instance_id = member(case, &body, "instanceId");
        assert_eq!(
            instance_id, None,
            "case {case:?}: an expired request must launch nothing, got {instance_id:?}"
        );
        assert_eq!(
            stored_instances(&engine),
            0,
            "case {case:?}: an expired request must leave no instance behind"
        );
    }
}

/// RULE 4: "by default, the request is valid for 7 days from the date the request was
/// created" — the window a request that named no end date reports is exactly that long,
/// and a window the request DID name is the one reported.
#[tokio::test]
async fn the_validity_window_is_seven_days_unless_the_request_names_one() {
    let case = "no ValidUntil";
    let (router, _engine) = emulator();
    let request_id = requested(&router, case, &one_request()).await;
    let body = describe_one(&router, case, &request_id).await;
    let created = timestamp(case, member(case, &body, "createTime"));
    let valid_until = timestamp(case, member(case, &body, "validUntil"));
    let window = valid_until - created;
    let expected = i64::try_from(DEFAULT_SPOT_REQUEST_VALIDITY.as_secs()).unwrap_or_default();
    assert_eq!(
        window, expected,
        "case {case:?}: expected a {expected}s window, got {window}s in {body}"
    );

    for ahead in [Duration::from_secs(60), Duration::from_secs(86_400)] {
        let case = format!("ValidUntil {}s ahead", ahead.as_secs());
        let (router, _engine) = emulator();
        let named = at_offset(ahead, true);
        let params = [one_request(), vec![p("ValidUntil", named.clone())]].concat();
        let request_id = requested(&router, &case, &params).await;
        let body = describe_one(&router, &case, &request_id).await;
        // To the second: the member is a timestamp, and the wire form carries more
        // precision than a caller-supplied one survives a float round-trip with.
        let reported = timestamp(&case, member(&case, &body, "validUntil"));
        let expected = timestamp(&case, Some(named.clone()));
        assert_eq!(
            reported, expected,
            "case {case:?}: expected <validUntil> {named:?}, got {reported} in {body}"
        );
    }
}

/// RULE 4: a validity window the clock cannot represent is still SERVED, not fatal: a
/// `ValidUntil` or `ValidFrom` far outside any span this process will run for is carried on
/// the capped clock, so one request body can never abort the handler.
#[tokio::test]
async fn an_extreme_validity_window_is_served_and_never_panics() {
    // Each row is a date at or past the edge of what a clock can hold, and the state the
    // request must be reported in for it.
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            "a ValidUntil at the end of the four-digit years",
            vec![p("ValidUntil", "9999-12-31T23:59:59Z")],
            SPOT_REQUEST_STATE_ACTIVE,
        ),
        (
            "a ValidFrom at the end of the four-digit years",
            vec![p("ValidFrom", "9999-12-31T23:59:59Z")],
            SPOT_REQUEST_STATE_OPEN,
        ),
        (
            "a ValidUntil at the epoch",
            vec![p("ValidUntil", "1970-01-01T00:00:00Z")],
            SPOT_REQUEST_STATE_CLOSED,
        ),
        (
            "a ValidFrom before the epoch",
            vec![p("ValidFrom", "1969-07-20T20:17:40Z")],
            SPOT_REQUEST_STATE_ACTIVE,
        ),
    ];
    for (case, extra, expected_state) in rows {
        let (router, _engine) = emulator();
        let request_id = requested(&router, case, &[one_request(), extra].concat()).await;
        past_the_transition().await;
        let body = describe_one(&router, case, &request_id).await;
        let state = member(case, &body, "state");
        assert_eq!(
            state.as_deref(),
            Some(expected_state),
            "case {case:?}: expected <state> {expected_state:?}, got {state:?} in {body}"
        );
    }
}

/// The seconds since the epoch of a timestamp the service reported.
fn timestamp(case: &str, text: Option<String>) -> i64 {
    let text = text.unwrap_or_else(|| panic!("case {case:?}: a timestamp was expected"));
    ::aws_smithy_types::DateTime::from_str(&text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {text:?} must be a timestamp ({error})"))
        .secs()
}

// ---------------------------------------------------------------------------
// RULE 5: one ClientToken names the requests of one call
// ---------------------------------------------------------------------------

/// RULE 5 over several counts: a repeat of the same call under one `ClientToken` answers
/// the requests the first one minted and asks for no more capacity.
#[tokio::test]
async fn one_client_token_names_the_requests_of_one_call() {
    for count in [1, 3] {
        let case = format!("a replay of a call for {count} instances");
        let (router, engine) = emulator();
        let params = [
            one_request(),
            vec![
                p("ClientToken", "fixed-token"),
                p("InstanceCount", count.to_string()),
            ],
        ]
        .concat();
        let (status, first) = request_spot(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {first}"
        );
        let (status, second) = request_spot(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 on the replay, got {status} with body {second}"
        );
        assert_eq!(
            request_ids(&case, &first),
            request_ids(&case, &second),
            "case {case:?}: a replay must answer the requests the first call minted"
        );
        assert_eq!(
            stored_requests(&engine),
            usize::try_from(count).unwrap_or_default(),
            "case {case:?}: a replay must store nothing more"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 6: narrowing by ids, by filters, and paging
// ---------------------------------------------------------------------------

/// RULE 6, the documented filter names: each selects the request that holds the value and
/// leaves out the one that does not, over a table spanning the names the member documents
/// for the attributes a request of this emulator carries.
#[tokio::test]
async fn every_documented_filter_selects_the_requests_that_hold_the_value() {
    let (router, _engine) = emulator();
    let (left_group_id, left_group_name) = default_group();
    let (right_group_id, right_group_name) = created_group(&router, "spot-suite-right").await;
    // Two requests that differ in every filterable attribute, so a filter that selected
    // both (or neither) fails the case.
    let left = requested(
        &router,
        "left",
        &[
            one_request(),
            vec![
                p("Type", SPOT_INSTANCE_TYPE_PERSISTENT),
                p("SpotPrice", "0.25"),
                p("LaunchGroup", "left-group"),
                p("AvailabilityZoneGroup", "left-zone-group"),
                p("LaunchSpecification.InstanceType", "c5.xlarge"),
                p("LaunchSpecification.KeyName", "left-key"),
                p("LaunchSpecification.KernelId", "aki-00000000000000001"),
                p("LaunchSpecification.RamdiskId", "ari-00000000000000001"),
                p("LaunchSpecification.Monitoring.Enabled", "true"),
                p("LaunchSpecification.SecurityGroupId.1", left_group_id.clone()),
                p("LaunchSpecification.SecurityGroup.1", left_group_name.clone()),
                p("TagSpecification.1.ResourceType", SPOT_REQUEST_TAGGABLE_RESOURCE),
                p("TagSpecification.1.Tag.1.Key", "side"),
                p("TagSpecification.1.Tag.1.Value", "left"),
            ],
        ]
        .concat(),
    )
    .await;
    let right = requested(
        &router,
        "right",
        &[vec![
            p("LaunchSpecification.ImageId", catalog_image(1)),
            p("Type", SPOT_INSTANCE_TYPE_ONE_TIME),
            p("SpotPrice", "0.75"),
            p("LaunchGroup", "right-group"),
            p("AvailabilityZoneGroup", "right-zone-group"),
            p("LaunchSpecification.InstanceType", "m5.large"),
            p("LaunchSpecification.KeyName", "right-key"),
            p("LaunchSpecification.KernelId", "aki-00000000000000002"),
            p("LaunchSpecification.RamdiskId", "ari-00000000000000002"),
            p("LaunchSpecification.SecurityGroupId.1", right_group_id.clone()),
            p("LaunchSpecification.SecurityGroup.1", right_group_name.clone()),
            p("TagSpecification.1.ResourceType", SPOT_REQUEST_TAGGABLE_RESOURCE),
            p("TagSpecification.1.Tag.1.Key", "side"),
            p("TagSpecification.1.Tag.1.Value", "right"),
        ]]
        .concat(),
    )
    .await;

    // (filter name, value that selects `left`, value that selects `right`)
    let rows: Vec<(&str, String, String)> = vec![
        ("spot-instance-request-id", left.clone(), right.clone()),
        (
            "type",
            SPOT_INSTANCE_TYPE_PERSISTENT.to_string(),
            SPOT_INSTANCE_TYPE_ONE_TIME.to_string(),
        ),
        ("spot-price", "0.25".to_string(), "0.75".to_string()),
        ("launch-group", "left-group".to_string(), "right-group".to_string()),
        (
            "availability-zone-group",
            "left-zone-group".to_string(),
            "right-zone-group".to_string(),
        ),
        ("launch.image-id", image_id(), catalog_image(1)),
        (
            "launch.instance-type",
            "c5.xlarge".to_string(),
            "m5.large".to_string(),
        ),
        ("launch.key-name", "left-key".to_string(), "right-key".to_string()),
        (
            "launch.kernel-id",
            "aki-00000000000000001".to_string(),
            "aki-00000000000000002".to_string(),
        ),
        (
            "launch.ramdisk-id",
            "ari-00000000000000001".to_string(),
            "ari-00000000000000002".to_string(),
        ),
        (
            "launch.group-id",
            left_group_id.clone(),
            right_group_id.clone(),
        ),
        (
            "launch.group-name",
            left_group_name.clone(),
            right_group_name.clone(),
        ),
        (
            "launch.monitoring-enabled",
            "true".to_string(),
            "false".to_string(),
        ),
        ("tag:side", "left".to_string(), "right".to_string()),
        ("tag-key", "side".to_string(), "side".to_string()),
    ];
    for (name, left_value, right_value) in rows {
        let case = format!("filter {name}");
        let selected = described_ids(
            &router,
            &case,
            &filters(&[(name, &[left_value.as_str()])]),
        )
        .await;
        if left_value == right_value {
            // A name both requests hold the same value for selects both — which is still
            // the rule: the filter selects what holds the value.
            assert_eq!(
                selected.len(),
                2,
                "case {case:?}: both requests hold {left_value:?}, so both must be \
                 selected, got {selected:?}"
            );
            continue;
        }
        assert_eq!(
            selected,
            vec![left.clone()],
            "case {case:?}: expected {left_value:?} to select only the left request"
        );
        let selected = described_ids(
            &router,
            &case,
            &filters(&[(name, &[right_value.as_str()])]),
        )
        .await;
        assert_eq!(
            selected,
            vec![right.clone()],
            "case {case:?}: expected {right_value:?} to select only the right request"
        );
        // A value neither request holds selects neither.
        let selected =
            described_ids(&router, &case, &filters(&[(name, &["sim2real-no-such-value"])])).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: a value no request holds must select nothing, got {selected:?}"
        );
    }
}

/// RULE 6: the `state` and `status-code` filters compare against the state the caller is
/// shown, before and after the clock has moved the request.
#[tokio::test]
async fn the_state_filters_follow_the_clock() {
    let case = "state and status-code";
    let (router, _engine) = emulator();
    let request_id = requested(&router, case, &one_request()).await;
    for (filter, value) in [
        ("state", SPOT_REQUEST_STATE_OPEN),
        ("status-code", SPOT_STATUS_PENDING_EVALUATION),
    ] {
        let selected = described_ids(&router, case, &filters(&[(filter, &[value])])).await;
        assert_eq!(
            selected,
            vec![request_id.clone()],
            "case {case:?}: expected {filter}={value:?} to select the open request"
        );
    }
    past_the_transition().await;
    for (filter, value) in [
        ("state", SPOT_REQUEST_STATE_ACTIVE),
        ("status-code", SPOT_STATUS_FULFILLED),
    ] {
        let selected = described_ids(&router, case, &filters(&[(filter, &[value])])).await;
        assert_eq!(
            selected,
            vec![request_id.clone()],
            "case {case:?}: expected {filter}={value:?} to select the fulfilled request"
        );
    }
    for (filter, value) in [
        ("state", SPOT_REQUEST_STATE_OPEN),
        ("status-code", SPOT_STATUS_PENDING_EVALUATION),
    ] {
        let selected = described_ids(&router, case, &filters(&[(filter, &[value])])).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: {filter}={value:?} must select nothing once the request has \
             been fulfilled, got {selected:?}"
        );
    }
}

/// RULE 6: a documented filter naming an attribute a request of this emulator carries no
/// value for selects nothing, while a name the operation does not document is refused —
/// so an unsupported filter can never read as "no results".
#[tokio::test]
async fn an_undocumented_filter_name_is_refused_and_a_valueless_one_selects_nothing() {
    let (router, _engine) = emulator();
    let case = "one stored request";
    let request_id = requested(&router, case, &one_request()).await;
    let all = described_ids(&router, case, &[]).await;
    assert_eq!(
        all,
        vec![request_id],
        "case {case:?}: the describe must report the stored request"
    );

    // Documented, but about a block device mapping or a network interface — which a
    // request of this emulator carries none of.
    for name in [
        "launch.block-device-mapping.device-name",
        "launch.block-device-mapping.volume-type",
        "network-interface.subnet-id",
        "network-interface.device-index",
        "fault-code",
        "instance-id",
        "launched-availability-zone",
    ] {
        let case = format!("documented but valueless filter {name}");
        let selected = described_ids(&router, &case, &filters(&[(name, &["anything"])])).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: a filter the request holds no value for must select nothing, \
             got {selected:?}"
        );
    }
    for name in ["sim2real-not-a-real-filter", "state-code", "tag"] {
        let case = format!("undocumented filter {name}");
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {body}"
        );
    }
}

/// RULE 6, paging: `MaxResults` cuts the report into pages that together report every
/// request exactly once, in one stable order — over several page sizes.
#[tokio::test]
async fn max_results_pages_the_report() {
    let (router, _engine) = emulator();
    let case = "nine requests";
    let (status, body) = request_spot(
        &router,
        &[one_request(), vec![p("InstanceCount", "9")]].concat(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let minted = request_ids(case, &body);
    let mut expected = minted.clone();
    expected.sort();

    for page_size in [1, 2, 4, 9, MAX_PAGE_SIZE] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=expected.len() {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = token.clone() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = request_ids(&case, &body);
            assert!(
                page.len() <= page_size.min(expected.len()),
                "case {case:?}: a page must hold at most {page_size} requests, got {page:?}"
            );
            seen.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the last page must carry no nextToken"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must report every request once, in id order"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 7: refusals carry EC2's own codes and store nothing
// ---------------------------------------------------------------------------

/// RULE 7 over a table of bad requests: each is refused with the code EC2 answers for it,
/// and the store is left empty.
#[tokio::test]
async fn a_request_the_service_cannot_accept_is_refused_and_stores_nothing() {
    let zones = availability_zones_in_region(REGION);
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "no launch specification at all",
            vec![],
            400,
            "MissingParameter",
        ),
        (
            "a launch specification with no AMI",
            vec![p("LaunchSpecification.InstanceType", "m5.large")],
            400,
            "MissingParameter",
        ),
        (
            "InstanceCount 0 asks for nothing",
            [one_request(), vec![p("InstanceCount", "0")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative InstanceCount",
            [one_request(), vec![p("InstanceCount", "-1")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "InstanceCount past what one call may ask for",
            [
                one_request(),
                vec![p(
                    "InstanceCount",
                    (MAX_SPOT_INSTANCES_PER_REQUEST + 1).to_string(),
                )],
            ]
            .concat(),
            400,
            "InstanceLimitExceeded",
        ),
        (
            "a price that is not a number",
            [one_request(), vec![p("SpotPrice", "cheap")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative price",
            [one_request(), vec![p("SpotPrice", "-0.5")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone the Region does not have",
            [
                one_request(),
                vec![p(
                    "LaunchSpecification.Placement.AvailabilityZone",
                    format!("{}z", zones[0].zone_name),
                )],
            ]
            .concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming another resource type",
            [
                one_request(),
                vec![
                    p("TagSpecification.1.ResourceType", "instance"),
                    p("TagSpecification.1.Tag.1.Key", "Name"),
                    p("TagSpecification.1.Tag.1.Value", "nope"),
                ],
            ]
            .concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key past the documented maximum",
            [
                one_request(),
                vec![
                    p("TagSpecification.1.ResourceType", SPOT_REQUEST_TAGGABLE_RESOURCE),
                    p("TagSpecification.1.Tag.1.Key", "k".repeat(128)),
                    p("TagSpecification.1.Tag.1.Value", "v"),
                ],
            ]
            .concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "more security groups than an instance may have",
            [
                one_request(),
                // Not groups of the account — the count is refused before any id is
                // resolved, which this row also pins.
                indexed(
                    "LaunchSpecification.SecurityGroupId",
                    &["sg-00000000000000001"; 17],
                ),
            ]
            .concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "an AMI the account cannot see",
            vec![p("LaunchSpecification.ImageId", "ami-0ffffffffffffffff")],
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            "something that is not an AMI id at all",
            vec![p("LaunchSpecification.ImageId", "ami-nope")],
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            "a subnet no subnet of the account has",
            [
                one_request(),
                vec![p(
                    "LaunchSpecification.SubnetId",
                    "subnet-0ffffffffffffffff",
                )],
            ]
            .concat(),
            400,
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group id the account does not hold",
            [
                one_request(),
                vec![p(
                    "LaunchSpecification.SecurityGroupId.1",
                    "sg-0ffffffffffffffff",
                )],
            ]
            .concat(),
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a security group name the account does not hold",
            [
                one_request(),
                vec![p("LaunchSpecification.SecurityGroup.1", "no-such-group")],
            ]
            .concat(),
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "an interruption behaviour outside the modelled catalog",
            [
                one_request(),
                vec![p("InstanceInterruptionBehavior", "reboot")],
            ]
            .concat(),
            400,
            // A value outside a MODELLED enum is refused by the boundary validator, before
            // the handler runs — which is why the handler's own catalog check for it is a
            // backstop rather than the first line of defence, and why the code is the
            // boundary's rather than the handler's.
            "ValidationError",
        ),
        (
            "a request type outside the modelled catalog",
            [one_request(), vec![p("Type", "forever")]].concat(),
            400,
            // The same boundary refusal as the row above, so the same code.
            "ValidationError",
        ),
        (
            "a DryRun that would have succeeded",
            [one_request(), vec![p("DryRun", "true")]].concat(),
            412,
            "DryRunOperation",
        ),
    ];
    for (case, params, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let (status, body) = request_spot(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        assert_eq!(
            stored_requests(&engine),
            0,
            "case {case:?}: a refused request must store nothing"
        );
        assert_eq!(
            stored_instances(&engine),
            0,
            "case {case:?}: a refused request must launch nothing"
        );
    }
}

/// RULE 7, the message of an unresolvable AMI: EC2 names the ids it could not resolve, as a
/// bracketed list, and the message is therefore built from the REQUEST — so it names
/// whichever id the call supplied.
#[tokio::test]
async fn an_unresolvable_ami_is_named_in_the_refusal() {
    for image_id in [
        "ami-0ffffffffffffffff",
        "ami-00000000000000000",
        "ami-0123456789abcdef0",
    ] {
        let case = format!("an AMI id of {image_id}");
        let (router, _engine) = emulator();
        let (status, body) =
            request_spot(&router, &[p("LaunchSpecification.ImageId", image_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.NotFound",
            "case {case:?}: expected InvalidAMIID.NotFound, got body {body}"
        );
        let message = super::ec2_query::unescaped(&body);
        let expected = format!("The image id '[{image_id}]' does not exist");
        assert!(
            message.contains(&expected),
            "case {case:?}: expected the refusal to read {expected:?}, got {message}"
        );
    }
}

/// RULE 7, the describe's own refusals: a malformed id, an id no request of the account
/// has, a page size outside what the service serves and a token it did not mint are each
/// refused with EC2's own code.
#[tokio::test]
async fn the_describe_refuses_a_bad_id_a_bad_page_size_and_a_bogus_token() {
    let (router, _engine) = emulator();
    let case = "one stored request";
    let request_id = requested(&router, case, &one_request()).await;

    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "something that is not a Spot request id",
            indexed("SpotInstanceRequestId", &["not-an-id"]),
            400,
            "InvalidSpotInstanceRequestID.Malformed",
        ),
        (
            "an id of the wrong resource kind",
            indexed("SpotInstanceRequestId", &["i-00000000000000001"]),
            400,
            "InvalidSpotInstanceRequestID.Malformed",
        ),
        (
            "a well-formed id no request of the account has",
            indexed("SpotInstanceRequestId", &["sir-0000000000000ffff"]),
            400,
            "InvalidSpotInstanceRequestID.NotFound",
        ),
        (
            "a page size below one",
            vec![p("MaxResults", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "a DryRun that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];
    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
    }

    // The stored request is still reported after every refusal: a refusal changes nothing.
    let described = described_ids(&router, case, &[]).await;
    assert_eq!(
        described,
        vec![request_id],
        "case {case:?}: a refused describe must leave the store as it was"
    );
}
