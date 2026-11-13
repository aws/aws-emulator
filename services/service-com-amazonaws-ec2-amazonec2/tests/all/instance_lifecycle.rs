// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EC2 instance lifecycle slice: `RunInstances`,
//! `DescribeInstances`, `CreateTags`, `StartInstances`, `StopInstances` and
//! `TerminateInstances`.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness, so every suite
//! posts an ec2Query request one way; this module adds only the structured readers
//! the lifecycle assertions need — an instance or a state change is a RECORD, not one
//! element, so it is read with an XML parser rather than by finding a tag.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{MAX_INSTANCES_PER_RUN, TRANSITION_DELAY};

use super::ec2_query;
// The entries of an ec2Query set, from the one shared definition.
use super::wire::set_items;

/// The Region every call here is signed for; the zone names and the private DNS
/// hostnames the assertions expect are derived from it.
const REGION: &str = "us-west-2";

/// Wait until every clock-owed transition started before now has certainly fallen
/// due. Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One EC2 emulator over its own engine, as [`super::ec2_query::emulator`] builds it.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Ec2 {
    fn new() -> Self {
        Self {
            router: ec2_query::emulator().0,
        }
    }

    /// Post one ec2Query request through the shared harness.
    async fn call(&self, action: &str, params: Vec<(String, String)>) -> Answer {
        let (status, body) = ec2_query::call(&self.router, REGION, action, &params).await;
        Answer { status, body }
    }
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One response, kept as text so each assertion parses only what it needs.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    fn document(&self) -> roxmltree::Document<'_> {
        roxmltree::Document::parse(&self.body)
            .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {}", self.body))
    }

    /// The `<Code>` of the ec2Query error envelope.
    fn error_code(&self) -> String {
        ec2_query::error_code(&self.body)
    }

    /// Every reservation in the response — the whole `RunInstances` envelope is one,
    /// and each `reservationSet/item` of a `DescribeInstances` response is one.
    fn reservations(&self) -> Vec<ReservationRow> {
        let document = self.document();
        document
            .descendants()
            .filter(|node| node.is_element() && has_child(*node, "reservationId"))
            .map(|node| ReservationRow {
                id: child_text(node, "reservationId").unwrap_or_default(),
                owner_id: child_text(node, "ownerId"),
                instances: node
                    .descendants()
                    .filter(|inner| inner.is_element() && has_child(*inner, "instanceId"))
                    .map(instance_row)
                    .collect(),
            })
            .collect()
    }

    /// Every instance in the response, across all reservations.
    fn instances(&self) -> Vec<InstanceRow> {
        self.reservations()
            .into_iter()
            .flat_map(|reservation| reservation.instances)
            .collect()
    }

    /// The `instanceId` of every instance in the response.
    fn instance_ids(&self) -> Vec<String> {
        self.instances()
            .into_iter()
            .map(|instance| instance.id)
            .collect()
    }

    /// The `InstanceStateChange` items a Start/Stop/Terminate response carries.
    fn state_changes(&self) -> Vec<StateChangeRow> {
        let document = self.document();
        document
            .descendants()
            .filter(|node| node.is_element() && has_child(*node, "currentState"))
            .map(|node| StateChangeRow {
                id: child_text(node, "instanceId").unwrap_or_default(),
                previous: nested_text(node, "previousState", "name").unwrap_or_default(),
                current: nested_text(node, "currentState", "name").unwrap_or_default(),
            })
            .collect()
    }

    fn next_token(&self) -> Option<String> {
        let document = self.document();
        document
            .root_element()
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "nextToken")
            .and_then(|node| node.text())
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReservationRow {
    id: String,
    owner_id: Option<String>,
    instances: Vec<InstanceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstanceRow {
    id: String,
    state: String,
    state_code: String,
    image_id: Option<String>,
    instance_type: Option<String>,
    key_name: Option<String>,
    subnet_id: Option<String>,
    private_ip: Option<String>,
    private_dns_name: Option<String>,
    architecture: Option<String>,
    monitoring: Option<String>,
    availability_zone: Option<String>,
    launch_index: Option<String>,
    launch_time: Option<String>,
    client_token: Option<String>,
    reason: Option<String>,
    group_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateChangeRow {
    id: String,
    previous: String,
    current: String,
}

fn instance_row(node: roxmltree::Node<'_, '_>) -> InstanceRow {
    InstanceRow {
        id: child_text(node, "instanceId").unwrap_or_default(),
        state: nested_text(node, "instanceState", "name").unwrap_or_default(),
        state_code: nested_text(node, "instanceState", "code").unwrap_or_default(),
        image_id: child_text(node, "imageId"),
        instance_type: child_text(node, "instanceType"),
        key_name: child_text(node, "keyName"),
        subnet_id: child_text(node, "subnetId"),
        private_ip: child_text(node, "privateIpAddress"),
        private_dns_name: child_text(node, "privateDnsName"),
        architecture: child_text(node, "architecture"),
        monitoring: nested_text(node, "monitoring", "state"),
        availability_zone: nested_text(node, "placement", "availabilityZone"),
        launch_index: child_text(node, "amiLaunchIndex"),
        launch_time: child_text(node, "launchTime"),
        client_token: child_text(node, "clientToken"),
        reason: child_text(node, "reason"),
        group_ids: set_items(node, "groupSet")
            .into_iter()
            .filter_map(|item| child_text(item, "groupId"))
            .collect(),
        tags: set_items(node, "tagSet")
            .into_iter()
            .filter_map(|item| Some((child_text(item, "key")?, child_text(item, "value")?)))
            .collect(),
    }
}

fn has_child(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    child(node, name).is_some()
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|inner| inner.is_element() && inner.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|inner| inner.text().unwrap_or_default().to_string())
}

fn nested_text(node: roxmltree::Node<'_, '_>, outer: &str, inner: &str) -> Option<String> {
    child_text(child(node, outer)?, inner)
}

// --- launch helpers ---------------------------------------------------------

const AMI: &str = "ami-0123456789abcdef0";

/// Launch `count` instances and return the whole answer. `extra` wins over the
/// defaults, so a case that varies the image id simply passes its own.
async fn launch(ec2: &Ec2, count: i32, extra: Vec<(String, String)>) -> Answer {
    let mut params = vec![
        p("MinCount", count.to_string()),
        p("MaxCount", count.to_string()),
    ];
    if !extra.iter().any(|(key, _)| key == "ImageId") {
        params.push(p("ImageId", AMI));
    }
    params.extend(extra);
    let answer = ec2.call("RunInstances", params).await;
    assert_eq!(
        answer.status, 200,
        "RunInstances must succeed, got {}: {}",
        answer.status, answer.body
    );
    answer
}

/// Launch one instance and return its id.
async fn launch_one(ec2: &Ec2, extra: Vec<(String, String)>) -> String {
    launch(ec2, 1, extra)
        .await
        .instance_ids()
        .pop()
        .expect("RunInstances must report the instance it minted")
}

/// Describe `instance_id` and return the single instance reported for it.
async fn describe_one(ec2: &Ec2, instance_id: &str) -> InstanceRow {
    let answer = ec2
        .call("DescribeInstances", vec![p("InstanceId.1", instance_id)])
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeInstances for {instance_id} must succeed, got {}: {}",
        answer.status, answer.body
    );
    let mut instances = answer.instances();
    assert_eq!(
        instances.len(),
        1,
        "DescribeInstances for {instance_id} must report exactly one instance, got {instances:?}"
    );
    instances.pop().expect("checked just above")
}

/// The state the clock owes an instance currently in `state`, or `state` itself when
/// it owes none. A transient state is only transient, so an assertion about a stored
/// state must admit the successor the clock may already have applied.
fn owed_next(state: &str) -> &str {
    match state {
        "pending" => "running",
        "stopping" => "stopped",
        "shutting-down" => "terminated",
        settled => settled,
    }
}

/// One instance parked in `state`, reached only through the public API.
async fn instance_in_state(ec2: &Ec2, state: &str) -> String {
    let instance_id = launch_one(ec2, Vec::new()).await;
    match state {
        "pending" => {}
        "running" => past_the_transition().await,
        "stopping" | "stopped" => {
            past_the_transition().await;
            let stop = ec2
                .call("StopInstances", vec![p("InstanceId.1", &instance_id)])
                .await;
            assert_eq!(stop.status, 200, "StopInstances must succeed: {}", stop.body);
            if state == "stopped" {
                past_the_transition().await;
            }
        }
        "shutting-down" | "terminated" => {
            past_the_transition().await;
            let terminate = ec2
                .call("TerminateInstances", vec![p("InstanceId.1", &instance_id)])
                .await;
            assert_eq!(
                terminate.status, 200,
                "TerminateInstances must succeed: {}",
                terminate.body
            );
            if state == "terminated" {
                past_the_transition().await;
            }
        }
        other => panic!("no way to park an instance in state {other:?}"),
    }
    let observed = describe_one(ec2, &instance_id).await;
    assert_eq!(
        observed.state, state,
        "the instance was meant to be parked in {state}, got {:?}",
        observed.state
    );
    instance_id
}

// --- tests ------------------------------------------------------------------

/// `RunInstances` mints `MaxCount` instances, each with its own id and primary
/// private address, inside one fresh reservation — never a canned id.
#[tokio::test]
async fn run_instances_mints_max_count_instances_in_a_fresh_reservation() {
    let ec2 = Ec2::new();
    let mut seen_reservations: BTreeSet<String> = BTreeSet::new();
    let mut seen_instances: BTreeSet<String> = BTreeSet::new();
    let mut seen_addresses: BTreeSet<String> = BTreeSet::new();

    for count in [1_i32, 2, 3, 5, MAX_INSTANCES_PER_RUN] {
        let case = format!("MaxCount={count}");
        let answer = launch(&ec2, count, Vec::new()).await;
        let reservations = answer.reservations();
        assert_eq!(
            reservations.len(),
            1,
            "case {case}: one launch is one reservation, got {reservations:?}"
        );
        let reservation = &reservations[0];
        assert!(
            reservation.id.starts_with("r-"),
            "case {case}: expected an r- reservation id, got {:?}",
            reservation.id
        );
        assert!(
            seen_reservations.insert(reservation.id.clone()),
            "case {case}: reservation id {:?} was already minted",
            reservation.id
        );
        assert_eq!(
            reservation.owner_id.as_deref(),
            Some(engine::emulator_account_id()),
            "case {case}: expected the emulator account as owner, got {:?}",
            reservation.owner_id
        );
        assert_eq!(
            reservation.instances.len(),
            usize::try_from(count).expect("a small count"),
            "case {case}: expected {count} instances, got {:?}",
            reservation.instances.len()
        );
        for (index, instance) in reservation.instances.iter().enumerate() {
            assert!(
                instance.id.starts_with("i-"),
                "case {case}: expected an i- instance id, got {:?}",
                instance.id
            );
            assert!(
                seen_instances.insert(instance.id.clone()),
                "case {case}: instance id {:?} was already minted",
                instance.id
            );
            assert_eq!(
                instance.state, "pending",
                "case {case}: a just-launched instance is pending, got {:?}",
                instance.state
            );
            assert_eq!(
                instance.state_code, "0",
                "case {case}: pending is state code 0, got {:?}",
                instance.state_code
            );
            assert_eq!(
                instance.launch_index.as_deref(),
                Some(index.to_string().as_str()),
                "case {case}: expected amiLaunchIndex {index}, got {:?}",
                instance.launch_index
            );
            let address = instance
                .private_ip
                .clone()
                .unwrap_or_else(|| panic!("case {case}: {} has no private address", instance.id));
            assert!(
                seen_addresses.insert(address.clone()),
                "case {case}: private address {address:?} was already assigned"
            );
            assert!(
                instance.launch_time.is_some(),
                "case {case}: {} must report a launch time",
                instance.id
            );
        }

        // The whole reservation is readable back out of stored state.
        let described = ec2
            .call(
                "DescribeInstances",
                vec![p("Filter.1.Name", "reservation-id"), p(
                    "Filter.1.Value.1",
                    reservation.id.clone(),
                )],
            )
            .await;
        assert_eq!(
            described.instance_ids(),
            reservation
                .instances
                .iter()
                .map(|instance| instance.id.clone())
                .collect::<Vec<_>>(),
            "case {case}: DescribeInstances must report the instances RunInstances minted"
        );
    }
}

/// Every launch parameter the slice stores is reported back by `DescribeInstances`,
/// for launches that differ in image, type, key pair, subnet, groups, monitoring and
/// tags — not only for one recorded shape.
#[tokio::test]
async fn launch_parameters_round_trip_through_describe_instances() {
    struct Case {
        name: &'static str,
        image_id: &'static str,
        instance_type: &'static str,
        key_name: Option<&'static str>,
        subnet_id: Option<&'static str>,
        group_ids: &'static [&'static str],
        monitoring: Option<bool>,
        tags: &'static [(&'static str, &'static str)],
    }

    let cases = [
        Case {
            name: "bare launch",
            image_id: "ami-00000000000000001",
            instance_type: "t3.micro",
            key_name: None,
            subnet_id: None,
            group_ids: &[],
            monitoring: None,
            tags: &[],
        },
        Case {
            name: "keyed, subnetted, one group, monitored",
            image_id: "ami-00000000000000002",
            instance_type: "m5.large",
            key_name: Some("deploy-key"),
            subnet_id: Some("subnet-0123456789abcdef0"),
            group_ids: &["sg-0123456789abcdef0"],
            monitoring: Some(true),
            tags: &[("Name", "demo")],
        },
        Case {
            name: "two groups, three tags, monitoring off",
            image_id: "ami-00000000000000003",
            instance_type: "c7g.2xlarge",
            key_name: Some("other-key"),
            subnet_id: Some("subnet-00000000000000abc"),
            group_ids: &["sg-aaaaaaaaaaaaaaaaa", "sg-bbbbbbbbbbbbbbbbb"],
            monitoring: Some(false),
            tags: &[("Name", "two"), ("env", "test"), ("empty", "")],
        },
    ];

    for case in cases {
        let ec2 = Ec2::new();
        let mut extra = vec![
            p("ImageId", case.image_id),
            p("InstanceType", case.instance_type),
        ];
        if let Some(key_name) = case.key_name {
            extra.push(p("KeyName", key_name));
        }
        if let Some(subnet_id) = case.subnet_id {
            extra.push(p("SubnetId", subnet_id));
        }
        for (index, group_id) in case.group_ids.iter().enumerate() {
            extra.push(p(&format!("SecurityGroupId.{}", index + 1), *group_id));
        }
        if let Some(monitoring) = case.monitoring {
            extra.push(p("Monitoring.Enabled", monitoring.to_string()));
        }
        if !case.tags.is_empty() {
            extra.push(p("TagSpecification.1.ResourceType", "instance"));
            for (index, (key, value)) in case.tags.iter().enumerate() {
                extra.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
                extra.push(p(
                    &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                    *value,
                ));
            }
        }

        let instance_id = launch_one(&ec2, extra).await;
        let described = describe_one(&ec2, &instance_id).await;

        assert_eq!(
            described.image_id.as_deref(),
            Some(case.image_id),
            "case {}: expected image {:?}, got {:?}",
            case.name,
            case.image_id,
            described.image_id
        );
        assert_eq!(
            described.instance_type.as_deref(),
            Some(case.instance_type),
            "case {}: expected type {:?}, got {:?}",
            case.name,
            case.instance_type,
            described.instance_type
        );
        assert_eq!(
            described.key_name.as_deref(),
            case.key_name,
            "case {}: expected key {:?}, got {:?}",
            case.name,
            case.key_name,
            described.key_name
        );
        match case.subnet_id {
            Some(subnet_id) => assert_eq!(
                described.subnet_id.as_deref(),
                Some(subnet_id),
                "case {}: expected subnet {:?}, got {:?}",
                case.name,
                subnet_id,
                described.subnet_id
            ),
            // "If you don't specify a subnet ID, we choose a default subnet from your
            // default VPC for you" (`RunInstances`), so a bare launch is placed in one and
            // reports it — asserted against the subnet store, so the row cannot pass for
            // any subnet that happens to be reported.
            None => {
                let reported = described.subnet_id.clone().unwrap_or_else(|| {
                    panic!(
                        "case {}: a launch that named no subnet is placed in the default one \
                         and reports it, got {:?}",
                        case.name, described.subnet_id
                    )
                });
                let subnets = ec2
                    .call("DescribeSubnets", vec![p("SubnetId.1", &reported)])
                    .await;
                let document = subnets.document();
                let subnet = document
                    .descendants()
                    .find(|node| node.is_element() && has_child(*node, "subnetId"))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {}: DescribeSubnets must report {reported}, got {}",
                            case.name, subnets.body
                        )
                    });
                assert_eq!(
                    child_text(subnet, "defaultForAz").as_deref(),
                    Some("true"),
                    "case {}: the subnet a bare launch is placed in is its zone's DEFAULT \
                     subnet, got {}",
                    case.name,
                    subnets.body
                );
            }
        }
        if case.group_ids.is_empty() {
            // "Default: Amazon EC2 uses the default security group"
            // (`RunInstancesRequest$SecurityGroupIds`): a launch that named no group is
            // given its VPC's `default` group, which is what the instance reports —
            // asserted against the security group store by NAME, so the row cannot pass
            // for any other single group.
            assert_eq!(
                described.group_ids.len(),
                1,
                "case {}: a launch that named no group reports the default group it was \
                 given, got {:?}",
                case.name,
                described.group_ids
            );
            let groups = ec2
                .call(
                    "DescribeSecurityGroups",
                    vec![p("GroupId.1", &described.group_ids[0])],
                )
                .await;
            assert_eq!(
                ec2_query::element(&groups.body, "groupName").as_deref(),
                Some("default"),
                "case {}: that one group is the VPC's default group, got {}",
                case.name,
                groups.body
            );
        } else {
            assert_eq!(
                described.group_ids,
                case.group_ids
                    .iter()
                    .map(|group| (*group).to_string())
                    .collect::<Vec<_>>(),
                "case {}: expected groups {:?}, got {:?}",
                case.name,
                case.group_ids,
                described.group_ids
            );
        }
        assert_eq!(
            described.monitoring.as_deref(),
            Some(if case.monitoring == Some(true) {
                "enabled"
            } else {
                "disabled"
            }),
            "case {}: expected the monitoring state the launch asked for, got {:?}",
            case.name,
            described.monitoring
        );
        assert_eq!(
            described.tags,
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<BTreeMap<_, _>>(),
            "case {}: expected the launch tags, got {:?}",
            case.name,
            described.tags
        );
        // The availability zone defaults into the region the request was signed for.
        assert_eq!(
            described.availability_zone.as_deref(),
            Some("us-west-2a"),
            "case {}: expected a zone in the signed region, got {:?}",
            case.name,
            described.availability_zone
        );
    }
}

/// `architecture` follows the instance-type naming rule — a `g` among a family's
/// capability letters (plus `a1`) is Graviton/arm64, a `mac` family from its second
/// generation on is Apple silicon, everything else is x86_64.
///
/// The expected values are the members of `ARCHITECTURE_VALUES_VALUES` in
/// `src/ops/types/shared_types.rs` that an instance type can select.
#[tokio::test]
async fn architecture_follows_the_instance_type_naming_rule() {
    let cases = [
        ("m5.large", "x86_64"),
        ("t3.micro", "x86_64"),
        ("m5dn.xlarge", "x86_64"),
        ("r6i.large", "x86_64"),
        ("a1.medium", "arm64"),
        ("t4g.nano", "arm64"),
        ("c7gn.xlarge", "arm64"),
        ("im4gn.large", "arm64"),
        ("x2gd.medium", "arm64"),
        ("mac1.metal", "x86_64_mac"),
        ("mac2.metal", "arm64_mac"),
    ];

    let ec2 = Ec2::new();
    for (instance_type, expected) in cases {
        let instance_id = launch_one(&ec2, vec![p("InstanceType", instance_type)]).await;
        let described = describe_one(&ec2, &instance_id).await;
        assert_eq!(
            described.architecture.as_deref(),
            Some(expected),
            "case {instance_type:?}: expected architecture {expected:?}, got {:?}",
            described.architecture
        );
    }
}

/// A transition the clock owes is not instant and is not skipped: the state the
/// caller sees straight after the call is the transient one, and the next state
/// appears on its own once the span has passed.
#[tokio::test]
async fn a_clock_owed_transition_lands_only_once_it_is_due() {
    let ec2 = Ec2::new();

    let instance_id = launch_one(&ec2, Vec::new()).await;
    assert_eq!(
        describe_one(&ec2, &instance_id).await.state,
        "pending",
        "case launch: a describe straight after the launch still sees pending"
    );
    assert_eq!(
        describe_one(&ec2, &instance_id)
            .await
            .private_dns_name
            .as_deref(),
        Some("ip-172-31-0-4.us-west-2.compute.internal"),
        "case launch: the private hostname names the address the launch assigned, so a \
         describe straight after the launch already reports it"
    );
    past_the_transition().await;
    let running = describe_one(&ec2, &instance_id).await;
    assert_eq!(
        running.state, "running",
        "case launch: pending must become running once the span has passed, got {:?}",
        running.state
    );
    assert_eq!(
        running.private_dns_name.as_deref(),
        Some("ip-172-31-0-4.us-west-2.compute.internal"),
        "case launch: a running instance reports the ip-name private DNS hostname for \
         its address in the signed region, got {:?}",
        running.private_dns_name
    );

    let stop = ec2
        .call("StopInstances", vec![p("InstanceId.1", &instance_id)])
        .await;
    assert_eq!(stop.status, 200, "StopInstances must succeed: {}", stop.body);
    assert_eq!(
        describe_one(&ec2, &instance_id).await.state,
        "stopping",
        "case stop: a describe straight after the stop still sees stopping"
    );
    past_the_transition().await;
    assert_eq!(
        describe_one(&ec2, &instance_id).await.state,
        "stopped",
        "case stop: stopping must become stopped once the span has passed"
    );

    let terminate = ec2
        .call("TerminateInstances", vec![p("InstanceId.1", &instance_id)])
        .await;
    assert_eq!(
        terminate.status, 200,
        "TerminateInstances must succeed: {}",
        terminate.body
    );
    assert_eq!(
        describe_one(&ec2, &instance_id).await.state,
        "shutting-down",
        "case terminate: a describe straight after the terminate still sees shutting-down"
    );
    past_the_transition().await;
    let terminated = describe_one(&ec2, &instance_id).await;
    assert_eq!(
        terminated.state, "terminated",
        "case terminate: shutting-down must become terminated once the span has passed"
    );
    assert_eq!(
        terminated.state_code, "48",
        "case terminate: terminated is state code 48, got {:?}",
        terminated.state_code
    );
}

/// Start/Stop/Terminate report the state the instance ACTUALLY held before the call
/// beside the one it holds after, and refuse a transition the current state does not
/// admit — over every (state, operation) pair the six states allow.
#[tokio::test]
async fn lifecycle_calls_report_the_state_actually_held() {
    struct Case {
        from: &'static str,
        action: &'static str,
        expected: Result<(&'static str, &'static str), &'static str>,
    }

    let cases = [
        Case { from: "pending", action: "StopInstances", expected: Ok(("pending", "stopping")) },
        Case { from: "running", action: "StopInstances", expected: Ok(("running", "stopping")) },
        Case { from: "stopping", action: "StopInstances", expected: Ok(("stopping", "stopping")) },
        Case { from: "stopped", action: "StopInstances", expected: Ok(("stopped", "stopped")) },
        Case {
            from: "shutting-down",
            action: "StopInstances",
            expected: Err("IncorrectInstanceState"),
        },
        Case {
            from: "terminated",
            action: "StopInstances",
            expected: Err("IncorrectInstanceState"),
        },
        Case { from: "stopped", action: "StartInstances", expected: Ok(("stopped", "pending")) },
        Case { from: "pending", action: "StartInstances", expected: Ok(("pending", "pending")) },
        Case { from: "running", action: "StartInstances", expected: Ok(("running", "running")) },
        Case {
            from: "stopping",
            action: "StartInstances",
            expected: Err("IncorrectInstanceState"),
        },
        Case {
            from: "terminated",
            action: "StartInstances",
            expected: Err("IncorrectInstanceState"),
        },
        Case {
            from: "running",
            action: "TerminateInstances",
            expected: Ok(("running", "shutting-down")),
        },
        Case {
            from: "stopped",
            action: "TerminateInstances",
            expected: Ok(("stopped", "shutting-down")),
        },
        Case {
            from: "shutting-down",
            action: "TerminateInstances",
            expected: Ok(("shutting-down", "shutting-down")),
        },
        Case {
            from: "terminated",
            action: "TerminateInstances",
            expected: Ok(("terminated", "terminated")),
        },
    ];

    for case in cases {
        let label = format!("{} from {}", case.action, case.from);
        let ec2 = Ec2::new();
        let instance_id = instance_in_state(&ec2, case.from).await;
        let answer = ec2
            .call(case.action, vec![p("InstanceId.1", &instance_id)])
            .await;
        match case.expected {
            Ok((previous, current)) => {
                assert_eq!(
                    answer.status, 200,
                    "case {label}: expected success, got {}: {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.state_changes(),
                    vec![StateChangeRow {
                        id: instance_id.clone(),
                        previous: previous.to_string(),
                        current: current.to_string(),
                    }],
                    "case {label}: expected previous {previous:?} and current {current:?}"
                );
                let stored = describe_one(&ec2, &instance_id).await.state;
                assert!(
                    stored == current || stored == owed_next(current),
                    "case {label}: the stored state must be {current:?}, or the state the \
                     clock then owes it, got {stored:?}"
                );
            }
            Err(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {label}: expected a 400 refusal, got {}: {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {label}: expected error code {code:?}, got {}",
                    answer.body
                );
                let stored = describe_one(&ec2, &instance_id).await.state;
                assert!(
                    stored == case.from || stored == owed_next(case.from),
                    "case {label}: a refused call must leave the state alone, got {stored:?}"
                );
            }
        }
    }
}

/// `CreateTags` writes through to stored state: a later `DescribeInstances` reports
/// the tags and the `tag:<key>` and `tag-key` filters match on them. An existing key
/// is overwritten rather than duplicated.
#[tokio::test]
async fn create_tags_writes_through_to_describe_and_to_tag_filters() {
    let cases: [&[(&str, &str)]; 4] = [
        &[("env", "test")],
        &[("env", "prod"), ("owner", "team-a")],
        &[("Name", "demo")],
        &[("blank", "")],
    ];

    for tags in cases {
        let ec2 = Ec2::new();
        let label = format!("{tags:?}");
        let instance_id = launch_one(
            &ec2,
            vec![
                p("TagSpecification.1.ResourceType", "instance"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "at-launch"),
            ],
        )
        .await;

        let mut params = vec![p("ResourceId.1", &instance_id)];
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(p(&format!("Tag.{}.Key", index + 1), *key));
            params.push(p(&format!("Tag.{}.Value", index + 1), *value));
        }
        let created = ec2.call("CreateTags", params).await;
        assert_eq!(
            created.status, 200,
            "case {label}: CreateTags must succeed, got {}: {}",
            created.status, created.body
        );

        let mut expected: BTreeMap<String, String> =
            [("Name".to_string(), "at-launch".to_string())]
                .into_iter()
                .collect();
        for (key, value) in tags {
            expected.insert((*key).to_string(), (*value).to_string());
        }
        let described = describe_one(&ec2, &instance_id).await;
        assert_eq!(
            described.tags, expected,
            "case {label}: expected the launch tags plus the created ones, got {:?}",
            described.tags
        );

        for (key, value) in tags {
            let by_tag = ec2
                .call(
                    "DescribeInstances",
                    vec![
                        p("Filter.1.Name", format!("tag:{key}")),
                        p("Filter.1.Value.1", *value),
                    ],
                )
                .await;
            assert_eq!(
                by_tag.instance_ids(),
                vec![instance_id.clone()],
                "case {label}: tag:{key}={value} must select the tagged instance, got {}",
                by_tag.body
            );
            let by_key = ec2
                .call(
                    "DescribeInstances",
                    vec![p("Filter.1.Name", "tag-key"), p("Filter.1.Value.1", *key)],
                )
                .await;
            assert_eq!(
                by_key.instance_ids(),
                vec![instance_id.clone()],
                "case {label}: tag-key={key} must select the tagged instance, got {}",
                by_key.body
            );
        }

        // Re-tagging an existing key overwrites it.
        let overwritten = ec2
            .call(
                "CreateTags",
                vec![
                    p("ResourceId.1", &instance_id),
                    p("Tag.1.Key", "Name"),
                    p("Tag.1.Value", "after"),
                ],
            )
            .await;
        assert_eq!(
            overwritten.status, 200,
            "case {label}: the overwriting CreateTags must succeed: {}",
            overwritten.body
        );
        assert_eq!(
            describe_one(&ec2, &instance_id).await.tags.get("Name"),
            Some(&"after".to_string()),
            "case {label}: an existing tag key must be overwritten"
        );
    }
}

/// Every filter name the slice resolves selects on the value the launch actually
/// stored, values are OR-ed, filters are AND-ed, `*` and `?` wildcards apply, and a
/// name the service does not resolve is refused rather than ignored.
#[tokio::test]
async fn describe_instances_resolves_each_supported_filter() {
    let ec2 = Ec2::new();
    let launched = launch(
        &ec2,
        1,
        vec![
            p("InstanceType", "t4g.small"),
            p("KeyName", "filter-key"),
            p("SubnetId", "subnet-0123456789abcdef0"),
            p("SecurityGroupId.1", "sg-0123456789abcdef0"),
            p("SecurityGroup.1", "web-tier"),
            p("ClientToken", "filter-token"),
            p("Monitoring.Enabled", "true"),
            p("TagSpecification.1.ResourceType", "instance"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "filtered"),
        ],
    )
    .await;
    let reservation_id = launched.reservations()[0].id.clone();
    let instance_id = launched
        .instance_ids()
        .pop()
        .expect("RunInstances must report the instance it minted");
    // The instance is left to reach `running`, a state the clock owes nothing
    // further, so the filter rows below are stable however long the test takes.
    past_the_transition().await;
    let described = describe_one(&ec2, &instance_id).await;

    // (filter name, matching value, a non-matching value)
    let cases: Vec<(&str, String, &str)> = vec![
        ("architecture", "arm64".to_string(), "x86_64"),
        (
            "availability-zone",
            described.availability_zone.clone().unwrap_or_default(),
            "eu-north-1c",
        ),
        ("client-token", "filter-token".to_string(), "other-token"),
        ("ebs-optimized", "false".to_string(), "true"),
        ("image-id", AMI.to_string(), "ami-99999999999999999"),
        (
            "instance-id",
            instance_id.clone(),
            "i-99999999999999999",
        ),
        ("instance-state-code", "16".to_string(), "0"),
        ("instance-state-name", "running".to_string(), "pending"),
        ("instance-type", "t4g.small".to_string(), "m5.large"),
        (
            "instance.group-id",
            "sg-0123456789abcdef0".to_string(),
            "sg-99999999999999999",
        ),
        ("instance.group-name", "web-tier".to_string(), "db-tier"),
        ("key-name", "filter-key".to_string(), "other-key"),
        ("launch-index", "0".to_string(), "1"),
        ("monitoring-state", "enabled".to_string(), "disabled"),
        (
            "owner-id",
            engine::emulator_account_id().to_string(),
            "999999999999",
        ),
        (
            "private-ip-address",
            described.private_ip.clone().unwrap_or_default(),
            "10.255.255.254",
        ),
        // Only reported once the instance runs, which it does by now.
        (
            "private-dns-name",
            described.private_dns_name.clone().unwrap_or_default(),
            "ip-10-255-255-254.us-west-2.compute.internal",
        ),
        (
            "reservation-id",
            reservation_id.clone(),
            "r-99999999999999999",
        ),
        (
            "subnet-id",
            "subnet-0123456789abcdef0".to_string(),
            "subnet-99999999999999999",
        ),
        ("tag:Name", "filtered".to_string(), "other"),
        ("tag-key", "Name".to_string(), "env"),
        // Wildcards: `*` spans any run of characters, `?` exactly one.
        ("instance-type", "t4g.*".to_string(), "m5.*"),
        ("key-name", "filter-ke?".to_string(), "filter-ke??"),
    ];

    for (name, matching, missing) in cases {
        let hit = ec2
            .call(
                "DescribeInstances",
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", &matching)],
            )
            .await;
        assert_eq!(
            hit.instance_ids(),
            vec![instance_id.clone()],
            "case {name}={matching:?}: must select the instance, got {}",
            hit.body
        );

        let miss = ec2
            .call(
                "DescribeInstances",
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", missing)],
            )
            .await;
        assert_eq!(
            miss.instance_ids(),
            Vec::<String>::new(),
            "case {name}={missing:?}: must select nothing, got {}",
            miss.body
        );

        // Values within one filter are OR-ed, so the non-matching value beside the
        // matching one still selects the instance.
        let either = ec2
            .call(
                "DescribeInstances",
                vec![
                    p("Filter.1.Name", name),
                    p("Filter.1.Value.1", missing),
                    p("Filter.1.Value.2", &matching),
                ],
            )
            .await;
        assert_eq!(
            either.instance_ids(),
            vec![instance_id.clone()],
            "case {name}: a filter's values are OR-ed, got {}",
            either.body
        );
    }

    // Filters are AND-ed: one that does not match excludes the instance.
    let both = ec2
        .call(
            "DescribeInstances",
            vec![
                p("Filter.1.Name", "instance-state-name"),
                p("Filter.1.Value.1", "running"),
                p("Filter.2.Name", "instance-type"),
                p("Filter.2.Value.1", "m5.large"),
            ],
        )
        .await;
    assert_eq!(
        both.instance_ids(),
        Vec::<String>::new(),
        "two filters are AND-ed, so a non-matching one must exclude the instance, got {}",
        both.body
    );

    // Real EC2 filter names this slice stores no value for are refused for the same
    // reason as a name that is not an EC2 filter at all: answering 200 with an empty
    // set would report absence as success. (`vpc-id` is NOT one of them: the slice stores
    // the instance's subnet and that subnet's VPC, so the filter resolves — see
    // `describe_instances_vpc_and_network_interfaces`.)
    for name in ["not-a-filter", "root-device-type", "hypervisor"] {
        let unsupported = ec2
            .call(
                "DescribeInstances",
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", "x")],
            )
            .await;
        assert_eq!(
            unsupported.status, 400,
            "case {name}: an unresolvable filter name must be refused, got {}: {}",
            unsupported.status, unsupported.body
        );
        assert_eq!(
            unsupported.error_code(),
            "InvalidParameterValue",
            "case {name}: expected InvalidParameterValue, got {}",
            unsupported.body
        );
    }
}

/// One row of the unknown-id table: the action, the form fields it carries, and the error
/// code EC2 refuses it with.
type UnknownIdCase = (&'static str, Vec<(String, String)>, &'static str);

/// An instance id nothing was launched for is refused with EC2's own code for it by every
/// operation that takes one — `InvalidInstanceID.NotFound` where the operation looks the id
/// up, and `InvalidInstanceID.Malformed` where it DECODES a long-form id first and this one
/// is a value the service never issued — and a resource id of a kind the slice does not
/// store is refused with `InvalidID`.
#[tokio::test]
async fn an_unknown_resource_id_is_refused_by_every_operation() {
    let ec2 = Ec2::new();
    let known = launch_one(&ec2, Vec::new()).await;
    // Well formed and not the null id (which EC2 refuses as malformed), so every
    // operation here has to resolve it and find nothing.
    let unknown = "i-00000000000000999";

    let cases: Vec<UnknownIdCase> = vec![
        // `DescribeInstances` decodes a long-form id before it looks anything up, so an
        // identifier above everything this service has issued is one it cannot decode.
        (
            "DescribeInstances",
            vec![p("InstanceId.1", unknown)],
            "InvalidInstanceID.Malformed",
        ),
        (
            "StartInstances",
            vec![p("InstanceId.1", unknown)],
            "InvalidInstanceID.NotFound",
        ),
        (
            "StopInstances",
            vec![p("InstanceId.1", unknown)],
            "InvalidInstanceID.NotFound",
        ),
        (
            "TerminateInstances",
            vec![p("InstanceId.1", unknown)],
            "InvalidInstanceID.NotFound",
        ),
        (
            "CreateTags",
            vec![
                p("ResourceId.1", unknown),
                p("Tag.1.Key", "k"),
                p("Tag.1.Value", "v"),
            ],
            "InvalidInstanceID.NotFound",
        ),
        // A batch that mixes a known id with an unknown one is refused whole.
        (
            "TerminateInstances",
            vec![p("InstanceId.1", &known), p("InstanceId.2", unknown)],
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (action, params, expected) in cases {
        let answer = ec2.call(action, params).await;
        assert_eq!(
            answer.status, 400,
            "case {action}: an unknown instance id must be a 400, got {}: {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {action}: expected {expected}, got {}",
            answer.body
        );
    }

    // The batch refusal left the known instance alone.
    let stored = describe_one(&ec2, &known).await.state;
    assert!(
        stored == "pending" || stored == "running",
        "a refused batch must not have terminated the known instance, got {stored:?}"
    );

    // An id of a KIND EC2 publishes names a resource that does not exist, which is that
    // kind's own not-found refusal; a value that is no resource id at all is `InvalidID`.
    for (resource_id, expected) in [
        ("vol-0123456789abcdef0", "InvalidVolume.NotFound"),
        ("not-an-id", "InvalidID"),
    ] {
        let answer = ec2
            .call(
                "CreateTags",
                vec![
                    p("ResourceId.1", resource_id),
                    p("Tag.1.Key", "k"),
                    p("Tag.1.Value", "v"),
                ],
            )
            .await;
        assert_eq!(
            answer.error_code(),
            expected,
            "case {resource_id}: expected {expected}, got {}",
            answer.body
        );
    }

    // An id that is not an instance id at all is refused BEFORE any lookup, with a
    // different code: EC2 instance ids are `i-` plus 8 or 17 lowercase hex digits.
    for malformed in ["i-xyz", "i-0123", "i-ZZZZZZZZ", "i-0123456789abcdef"] {
        for action in [
            "DescribeInstances",
            "StartInstances",
            "StopInstances",
            "TerminateInstances",
        ] {
            let answer = ec2.call(action, vec![p("InstanceId.1", malformed)]).await;
            assert_eq!(
                answer.error_code(),
                "InvalidInstanceID.Malformed",
                "case {action} {malformed}: expected InvalidInstanceID.Malformed, got {}",
                answer.body
            );
        }
    }

    // Naming one instance twice in a batch answers once for it, not twice.
    let repeated = ec2
        .call(
            "StopInstances",
            vec![p("InstanceId.1", &known), p("InstanceId.2", &known)],
        )
        .await;
    assert_eq!(
        repeated.state_changes().len(),
        1,
        "a repeated instance id must yield one state change, got {}",
        repeated.body
    );
}

/// `MaxResults`/`NextToken` page over whole reservations, in launch order, and
/// `MaxResults` cannot be combined with `InstanceIds`.
#[tokio::test]
async fn max_results_pages_over_reservations() {
    let ec2 = Ec2::new();
    let mut launched: Vec<String> = Vec::new();
    for _ in 0..12 {
        let answer = launch(&ec2, 1, Vec::new()).await;
        launched.push(answer.reservations()[0].id.clone());
    }

    // Page sizes at and above the smallest page this operation pages with (AWS publishes a
    // minimum of 5 for its `MaxResults`): one that divides the set, ones that do not, and
    // one larger than the set. A page size below that minimum is refused rather than
    // served, which the rows further down cover.
    for page_size in [5_usize, 6, 7, 12, 14] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &token {
                params.push(p("NextToken", token));
            }
            let answer = ec2.call("DescribeInstances", params).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: DescribeInstances must succeed: {}",
                answer.body
            );
            let ids: Vec<String> = answer
                .reservations()
                .into_iter()
                .map(|reservation| reservation.id)
                .collect();
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size} \
                 reservations, got {}",
                ids.len()
            );
            let held = ids.len();
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= 20,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            token = answer.next_token();
            // The token means "resume AFTER what I just gave you", and this operation
            // answers one while a further reservation follows the page.
            let due = seen.len() < launched.len();
            assert_eq!(
                token.is_some(),
                due,
                "case page size {page_size}: page {pages} reported {held} of at most \
                 {page_size} reservations, {seen} of {all} in all, so a token \
                 {expectation}; got {token:?}",
                seen = seen.len(),
                all = launched.len(),
                expectation = if due { "is due" } else { "is not due" }
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, launched,
            "case page size {page_size}: paging must yield every reservation once, in \
             launch order"
        );
    }

    let combined = ec2
        .call(
            "DescribeInstances",
            vec![p("MaxResults", "5"), p("InstanceId.1", "i-00000000000000001")],
        )
        .await;
    assert_eq!(
        combined.error_code(),
        "InvalidParameterCombination",
        "MaxResults with InstanceIds must be refused, got {}",
        combined.body
    );

    let bad_token = ec2
        .call("DescribeInstances", vec![p("NextToken", "r-nope")])
        .await;
    assert_eq!(
        bad_token.error_code(),
        "InvalidPaginationToken",
        "a NextToken that names no reservation must be refused, got {}",
        bad_token.body
    );

    // A page size below the smallest this operation pages with is refused as a value the
    // service cannot use, whether it asks for a smaller page or for no page at all.
    for max_results in ["0", "-1", "1", "4"] {
        let answer = ec2
            .call("DescribeInstances", vec![p("MaxResults", max_results)])
            .await;
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case MaxResults={max_results}: a page below the published minimum must be \
             refused, got {}",
            answer.body
        );
    }

    // The model declares no `@range` on `DescribeInstancesRequest$MaxResults`, so a page
    // size above the largest page this service answers asks for that page rather than for
    // a refusal.
    for max_results in ["1001", "2147483647"] {
        let answer = ec2
            .call("DescribeInstances", vec![p("MaxResults", max_results)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case MaxResults={max_results}: a page size above the ceiling must be served, \
             got {}",
            answer.body
        );
        let reported: Vec<String> = answer
            .reservations()
            .into_iter()
            .map(|reservation| reservation.id)
            .collect();
        assert_eq!(
            reported, launched,
            "case MaxResults={max_results}: expected every reservation in one page, got \
             {reported:?}"
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case MaxResults={max_results}: a page that holds everything carries no token, \
             got {}",
            answer.body
        );
    }
}

/// One `ClientToken` names one launch: repeating the request returns the reservation
/// it already minted rather than launching again.
#[tokio::test]
async fn a_client_token_makes_run_instances_idempotent() {
    let ec2 = Ec2::new();
    for count in [1_i32, 2, 4] {
        let token = format!("token-for-{count}");
        let first = launch(&ec2, count, vec![p("ClientToken", &token)]).await;
        let second = launch(&ec2, count, vec![p("ClientToken", &token)]).await;

        assert_eq!(
            second.reservations()[0].id,
            first.reservations()[0].id,
            "case {token}: a repeated client token must return the same reservation"
        );
        assert_eq!(
            second.instance_ids(),
            first.instance_ids(),
            "case {token}: a repeated client token must return the same instances"
        );
        assert_eq!(
            first.instances()[0].client_token.as_deref(),
            Some(token.as_str()),
            "case {token}: the instance must report the client token it was launched with"
        );

        let fresh = launch(&ec2, count, vec![p("ClientToken", format!("{token}-other"))]).await;
        assert_ne!(
            fresh.reservations()[0].id,
            first.reservations()[0].id,
            "case {token}: a different client token must launch a different reservation"
        );
    }

    // Every launch above is still exactly one reservation per distinct token.
    let all = ec2.call("DescribeInstances", Vec::new()).await;
    assert_eq!(
        all.reservations().len(),
        6,
        "three tokens plus three other tokens is six reservations, got {}",
        all.body
    );
}

/// `DryRun` answers 412 `DryRunOperation` and leaves state exactly as it was, for
/// every operation in the slice that takes the flag.
#[tokio::test]
async fn dry_run_refuses_with_412_and_changes_nothing() {
    let ec2 = Ec2::new();
    let instance_id = launch_one(&ec2, Vec::new()).await;
    // Let the instance settle in `running`, so the only thing that could change the
    // snapshot below is a DryRun call wrongly doing its work.
    past_the_transition().await;
    let before = describe_one(&ec2, &instance_id).await;

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "RunInstances",
            vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "3"),
                p("DryRun", "true"),
            ],
        ),
        (
            "DescribeInstances",
            vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
        ),
        (
            "CreateTags",
            vec![
                p("ResourceId.1", &instance_id),
                p("Tag.1.Key", "should-not-land"),
                p("Tag.1.Value", "x"),
                p("DryRun", "true"),
            ],
        ),
        (
            "StopInstances",
            vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
        ),
        (
            "StartInstances",
            vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
        ),
        (
            "TerminateInstances",
            vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
        ),
    ];

    for (action, params) in cases {
        let answer = ec2.call(action, params).await;
        assert_eq!(
            answer.status, 412,
            "case {action}: DryRun must answer 412, got {}: {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "DryRunOperation",
            "case {action}: expected DryRunOperation, got {}",
            answer.body
        );
        let after = describe_one(&ec2, &instance_id).await;
        assert_eq!(
            after, before,
            "case {action}: a DryRun must leave the instance exactly as it was"
        );
        let all = ec2.call("DescribeInstances", Vec::new()).await;
        assert_eq!(
            all.instance_ids(),
            vec![instance_id.clone()],
            "case {action}: a DryRun must not have launched anything, got {}",
            all.body
        );
    }

    // DryRun says "this WOULD have succeeded", so a request that would in fact have
    // been refused is refused with that reason rather than answered 412. The id
    // resolution the five id-taking operations do only reads state. Each row carries the
    // code `DescribeInstances` answers and the code the operations that LOOK the id up
    // answer: the describe decodes a long-form id first, so one this service never issued
    // is malformed there while a lookup reports it absent.
    let unresolvable: [(&str, &str, &str); 3] = [
        (
            "i-00000000000000999",
            "InvalidInstanceID.Malformed",
            "InvalidInstanceID.NotFound",
        ),
        (
            "i-nope",
            "InvalidInstanceID.Malformed",
            "InvalidInstanceID.Malformed",
        ),
        // The null id is not an id EC2 issues, so it is refused by syntax rather
        // than reported missing.
        (
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
            "InvalidInstanceID.Malformed",
        ),
    ];
    for (bad_id, described, resolved) in unresolvable {
        for (action, code) in [
            ("DescribeInstances", described),
            ("StartInstances", resolved),
            ("StopInstances", resolved),
            ("TerminateInstances", resolved),
        ] {
            let answer = ec2
                .call(action, vec![p("InstanceId.1", bad_id), p("DryRun", "true")])
                .await;
            assert_eq!(
                answer.error_code(),
                code,
                "case {action} {bad_id} under DryRun: expected {code}, got {}",
                answer.body
            );
        }
        let tagged = ec2
            .call(
                "CreateTags",
                vec![
                    p("ResourceId.1", bad_id),
                    p("Tag.1.Key", "k"),
                    p("Tag.1.Value", "v"),
                    p("DryRun", "true"),
                ],
            )
            .await;
        let expected = if resolved == "InvalidInstanceID.Malformed" {
            // CreateTags takes any EC2 resource id, so something that is not an
            // instance id is an unresolvable resource rather than a malformed
            // instance id.
            "InvalidID"
        } else {
            resolved
        };
        assert_eq!(
            tagged.error_code(),
            expected,
            "case CreateTags {bad_id} under DryRun: expected {expected}, got {}",
            tagged.body
        );
    }

    // And none of those refusals changed anything.
    assert_eq!(
        describe_one(&ec2, &instance_id).await,
        before,
        "a refused DryRun must leave the instance exactly as it was"
    );

    // Same rule on the launch path: a DryRun onto an address already in use is told
    // the address is in use, not that it would have succeeded.
    let taken = before
        .private_ip
        .clone()
        .expect("the instance reports its private address");
    let conflict = ec2
        .call(
            "RunInstances",
            vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("PrivateIpAddress", &taken),
                p("DryRun", "true"),
            ],
        )
        .await;
    assert_eq!(
        conflict.error_code(),
        "InvalidIPAddress.InUse",
        "a DryRun launch onto a used address must be refused with that reason, got {}",
        conflict.body
    );
}

/// `RunInstances` refuses a request that cannot be served, with the code the real
/// service answers for it, and launches nothing.
#[tokio::test]
async fn run_instances_refuses_an_unservable_request() {
    struct Case {
        label: &'static str,
        params: Vec<(String, String)>,
        status: u16,
        code: &'static str,
    }

    let cases = vec![
        Case {
            label: "no ImageId",
            params: vec![p("MinCount", "1"), p("MaxCount", "1")],
            status: 400,
            code: "MissingParameter",
        },
        Case {
            label: "MinCount below one",
            params: vec![p("ImageId", AMI), p("MinCount", "0"), p("MaxCount", "1")],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "MaxCount below MinCount",
            params: vec![p("ImageId", AMI), p("MinCount", "3"), p("MaxCount", "2")],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "PrivateIpAddress with more than one instance",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "2"),
                p("PrivateIpAddress", "172.31.0.9"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        Case {
            label: "a tag specification for a resource RunInstances cannot tag",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "k"),
                p("TagSpecification.1.Tag.1.Value", "v"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "a PrivateIpAddress that is not an IPv4 address",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("PrivateIpAddress", "not-an-ip"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "MaxCount above the launch ceiling",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", (MAX_INSTANCES_PER_RUN + 1).to_string()),
            ],
            status: 400,
            code: "InstanceLimitExceeded",
        },
        Case {
            label: "user data above the documented 16 KiB",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("UserData", "u".repeat(16 * 1024 + 1)),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "more security groups than an instance may carry",
            params: {
                let mut params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
                for index in 1..=17 {
                    params.push(p(
                        &format!("SecurityGroupId.{index}"),
                        format!("sg-{index:017x}"),
                    ));
                }
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "a launch tag key above the documented 127 characters",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("TagSpecification.1.ResourceType", "instance"),
                p("TagSpecification.1.Tag.1.Key", "k".repeat(128)),
                p("TagSpecification.1.Tag.1.Value", "v"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        Case {
            label: "a network interface specification naming a subnet of no account",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("NetworkInterface.1.DeviceIndex", "0"),
                p("NetworkInterface.1.SubnetId", "subnet-0123456789abcdef0"),
            ],
            status: 400,
            code: "InvalidSubnetID.NotFound",
        },
        Case {
            label: "IPv6 addresses, which come from a subnet range this slice does not model",
            params: vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("Ipv6AddressCount", "1"),
            ],
            status: 501,
            code: "NotImplemented",
        },
        Case {
            label: "a launch template id no template of the account was created under",
            params: vec![
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("LaunchTemplate.LaunchTemplateId", "lt-0123456789abcdef0"),
            ],
            status: 400,
            code: "InvalidLaunchTemplateId.NotFound",
        },
    ];

    for case in cases {
        let ec2 = Ec2::new();
        let answer = ec2.call("RunInstances", case.params).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected HTTP {}, got {}: {}",
            case.label, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            case.code,
            "case {}: expected error code {:?}, got {}",
            case.label,
            case.code,
            answer.body
        );
        let all = ec2.call("DescribeInstances", Vec::new()).await;
        assert_eq!(
            all.instance_ids(),
            Vec::<String>::new(),
            "case {}: a refused launch must store nothing, got {}",
            case.label,
            all.body
        );
    }
}

/// An explicit `PrivateIpAddress` is honoured once and refused the second time: no
/// two instances ever report the same primary private address.
#[tokio::test]
async fn an_explicit_private_address_is_used_once_and_then_refused() {
    let ec2 = Ec2::new();
    // The second row is the first address the allocator would hand out on its own,
    // so taking it explicitly also proves the allocator skips an address in use.
    for address in ["172.31.9.9", "172.31.0.4", "10.0.1.200"] {
        let first = launch(&ec2, 1, vec![p("PrivateIpAddress", address)]).await;
        assert_eq!(
            first.instances()[0].private_ip.as_deref(),
            Some(address),
            "case {address}: the launch must use the address it asked for, got {:?}",
            first.instances()[0].private_ip
        );
        let again = ec2
            .call(
                "RunInstances",
                vec![
                    p("ImageId", AMI),
                    p("MinCount", "1"),
                    p("MaxCount", "1"),
                    p("PrivateIpAddress", address),
                ],
            )
            .await;
        assert_eq!(
            again.error_code(),
            "InvalidIPAddress.InUse",
            "case {address}: a second launch onto the same address must be refused, got {}",
            again.body
        );
    }

    let auto = launch_one(&ec2, Vec::new()).await;
    let taken = describe_one(&ec2, &auto)
        .await
        .private_ip
        .expect("an auto-allocated instance must report an address");
    assert_ne!(
        taken, "172.31.0.4",
        "the allocator must skip an address an explicit launch already took"
    );
    let refused = ec2
        .call(
            "RunInstances",
            vec![
                p("ImageId", AMI),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("PrivateIpAddress", &taken),
            ],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidIPAddress.InUse",
        "case {taken}: an auto-allocated address must also be refused when asked for, got {}",
        refused.body
    );
}

/// `Placement.AvailabilityZone` must name a zone the signed Region actually has,
/// and the zone an instance reports is the one it launched into.
#[tokio::test]
async fn placement_must_name_a_zone_the_region_has() {
    let ec2 = Ec2::new();
    // The Region's zones, by the naming rule `state::availability_zones_in_region`
    // owns: `<region>` plus a letter from `a`, one per zone.
    for zone in ["us-west-2a", "us-west-2b", "us-west-2c"] {
        let instance_id = launch_one(&ec2, vec![p("Placement.AvailabilityZone", zone)]).await;
        assert_eq!(
            describe_one(&ec2, &instance_id).await.availability_zone.as_deref(),
            Some(zone),
            "case {zone}: the instance must report the zone it launched into"
        );
    }

    for zone in ["us-west-2d", "eu-north-1a", "not-a-zone"] {
        let answer = ec2
            .call(
                "RunInstances",
                vec![
                    p("ImageId", AMI),
                    p("MinCount", "1"),
                    p("MaxCount", "1"),
                    p("Placement.AvailabilityZone", zone),
                ],
            )
            .await;
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case {zone}: a zone the Region does not have must be refused, got {}",
            answer.body
        );
    }
}

/// `CreateTags` resolves every resource this crate stores, not only instances: the
/// account's default VPC can be tagged, and reads its tags back.
#[tokio::test]
async fn create_tags_also_tags_a_stored_vpc() {
    let ec2 = Ec2::new();
    let vpcs = ec2.call("DescribeVpcs", Vec::new()).await;
    assert_eq!(
        vpcs.status, 200,
        "DescribeVpcs must succeed, got {}: {}",
        vpcs.status, vpcs.body
    );
    let vpc_id = {
        let document = vpcs.document();
        document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "vpcId")
            .find_map(|node| node.text().map(str::to_string))
            .expect("a fresh account holds its default VPC")
    };

    let created = ec2
        .call(
            "CreateTags",
            vec![
                p("ResourceId.1", &vpc_id),
                p("Tag.1.Key", "Name"),
                p("Tag.1.Value", "default-vpc"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "tagging a stored VPC must succeed, got {}: {}",
        created.status, created.body
    );

    let after = ec2.call("DescribeVpcs", Vec::new()).await;
    let tags = {
        let document = after.document();
        document
            .descendants()
            .filter(|node| node.is_element() && has_child(*node, "vpcId"))
            .find(|node| child_text(*node, "vpcId").as_deref() == Some(vpc_id.as_str()))
            .map(|node| {
                set_items(node, "tagSet")
                    .into_iter()
                    .filter_map(|item| Some((child_text(item, "key")?, child_text(item, "value")?)))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_else(|| panic!("the VPC must still be reported: {}", after.body))
    };
    assert_eq!(
        tags.get("Name").map(String::as_str),
        Some("default-vpc"),
        "the VPC must report the tag CreateTags wrote, got {tags:?}"
    );

    // A `vpc-` id nothing holds names a VPC that does not exist, which is the refusal every
    // operation of that resource answers for the id.
    let unknown = ec2
        .call(
            "CreateTags",
            vec![
                p("ResourceId.1", "vpc-99999999999999999"),
                p("Tag.1.Key", "k"),
                p("Tag.1.Value", "v"),
            ],
        )
        .await;
    assert_eq!(
        unknown.error_code(),
        "InvalidVpcID.NotFound",
        "an unknown VPC id must be refused as a VPC that does not exist, got {}",
        unknown.body
    );
}

/// An instance launched with `DisableApiTermination` cannot be terminated, and a
/// reserved or empty tag key cannot be written.
#[tokio::test]
async fn attribute_and_tag_rules_are_enforced() {
    let ec2 = Ec2::new();
    let protected = launch_one(&ec2, vec![p("DisableApiTermination", "true")]).await;
    let answer = ec2
        .call("TerminateInstances", vec![p("InstanceId.1", &protected)])
        .await;
    assert_eq!(
        answer.error_code(),
        "OperationNotPermitted",
        "termination protection must refuse TerminateInstances, got {}",
        answer.body
    );
    let stored = describe_one(&ec2, &protected).await.state;
    assert!(
        stored == "pending" || stored == "running",
        "a refused terminate must leave the instance alone, got {stored:?}"
    );

    let cases: [(&str, &str); 2] = [("aws:owner", "reserved prefix"), ("", "empty key")];
    for (key, label) in cases {
        let refused = ec2
            .call(
                "CreateTags",
                vec![
                    p("ResourceId.1", &protected),
                    p("Tag.1.Key", key),
                    p("Tag.1.Value", "v"),
                ],
            )
            .await;
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {label}: expected InvalidParameterValue, got {}",
            refused.body
        );
        assert_eq!(
            describe_one(&ec2, &protected).await.tags,
            BTreeMap::new(),
            "case {label}: a refused CreateTags must write nothing"
        );
    }

    // Two entries for one key have no defined winner, so the request is refused.
    let duplicate = ec2
        .call(
            "CreateTags",
            vec![
                p("ResourceId.1", &protected),
                p("Tag.1.Key", "same"),
                p("Tag.1.Value", "a"),
                p("Tag.2.Key", "same"),
                p("Tag.2.Value", "b"),
            ],
        )
        .await;
    assert_eq!(
        duplicate.error_code(),
        "InvalidParameterValue",
        "a duplicate tag key must be refused, got {}",
        duplicate.body
    );
    assert_eq!(
        describe_one(&ec2, &protected).await.tags,
        BTreeMap::new(),
        "a refused CreateTags must write nothing"
    );

    // A count cap is not a byte cap: an over-long key or value is refused too.
    let oversize: [(String, String, &str); 2] = [
        ("k".repeat(128), "v".to_string(), "key above 127 characters"),
        ("k".to_string(), "v".repeat(257), "value above 256 characters"),
    ];
    for (key, value, label) in oversize {
        let refused = ec2
            .call(
                "CreateTags",
                vec![
                    p("ResourceId.1", &protected),
                    p("Tag.1.Key", key),
                    p("Tag.1.Value", value),
                ],
            )
            .await;
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {label}: expected InvalidParameterValue, got {}",
            refused.body
        );
        assert_eq!(
            describe_one(&ec2, &protected).await.tags,
            BTreeMap::new(),
            "case {label}: a refused CreateTags must write nothing"
        );
    }

    // Nor is a capped number of filter values a capped amount of matching work.
    let long_value = ec2
        .call(
            "DescribeInstances",
            vec![
                p("Filter.1.Name", "instance-id"),
                p("Filter.1.Value.1", "i".repeat(1025)),
            ],
        )
        .await;
    assert_eq!(
        long_value.error_code(),
        "InvalidParameterValue",
        "an over-long filter value must be refused, got {}",
        long_value.body
    );
}
