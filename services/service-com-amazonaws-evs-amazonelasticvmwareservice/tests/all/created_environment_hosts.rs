// SPDX-License-Identifier: CC0-1.0

//! The ESX hosts a `CreateEnvironment` request asks Amazon EVS to ADD, over awsJson1_0.
//!
//! The rule under test, from the model's own documentation
//! (`smithy_query(query="operation",operation="CreateEnvironment")` and the member docs
//! it quotes): `hosts` is "The ESX hosts to add to the environment" and "When you specify
//! any other VCF version, Amazon EVS installs and configures VCF for you" — so a create
//! that carries hosts MATERIALISES them as hosts of the environment it mints, exactly as
//! `CreateEnvironmentHost` materialises the one host it adds:
//!
//! * `ListEnvironmentHosts` reports one `Host` per entry, carrying the `hostName`,
//!   `keyName`, `instanceType`, `placementGroupId` and `dedicatedHostId` the entry
//!   supplied, each `CREATING` until the build the clock owes it comes due;
//! * each host is addressed out of the environment's own `vmkManagement` VLAN subnet and
//!   runs on an EC2 bare metal instance of the account's own Amazon EC2 store ("An ESX
//!   host that runs on an Amazon EC2 bare metal instance", `Host` doc);
//! * the names those hosts take are taken: "DNS hostnames for hosts must be unique across
//!   Amazon EVS environments and within VCF" (`HostInfoForCreate$hostName` doc), so a
//!   later `CreateEnvironmentHost` for one of them, and a later create naming one of them,
//!   are refused; and
//! * `SELF_DEPLOYED` adds none of them: "no hosts are added and no VCF installation is
//!   performed. After the environment is created, you can add hosts with
//!   CreateEnvironmentHost".
//!
//! Both services are mounted on ONE shared engine here, and every call goes over the wire
//! a client makes it on — awsJson1_0 for Amazon EVS, ec2Query for Amazon EC2 — so the
//! placement groups and Dedicated Hosts the rows name are minted by Amazon EC2 itself and
//! the instances the hosts run on are read back from it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, error_type, register_key_pair, register_network, vlans_with_prefix, HOST_KEY_PAIR_NAME,
    POLL_DEADLINE, POLL_INTERVAL, REGION, ZONE,
};
// The ec2Query client and the XML readers the sibling EC2-facing suites already own:
// this suite asks Amazon EC2 the same questions they do.
use crate::vlan_subnet_propagation::{call_ec2, element, elements};

/// Long enough for the transitions the two services owe a new environment, host and
/// instance to come due (each span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The first usable address of a VLAN subnet block that starts at `10.0.0.0`, which is
/// where every `vmkManagement` block below starts: AWS reserves the first four addresses
/// of a subnet, so the hosts of a create take `10.0.0.4` upwards in request order.
const FIRST_HOST_ADDRESS: u8 = 4;

/// How many addresses of that block ONE host takes: one per elastic network interface
/// Amazon EVS provisions for it — "Amazon EVS provisions two identically configured ENIs
/// in the VMkernel management subnet during host creation" (`NetworkInterface` doc) —
/// since no two interfaces of a subnet can hold one address.
const ADDRESSES_PER_HOST: usize = 2;

/// The management address the host at `position` of a create's `hosts` takes: the first
/// usable address of the block, moved on by the addresses the hosts before it hold.
fn host_address(position: usize) -> String {
    format!(
        "10.0.0.{}",
        FIRST_HOST_ADDRESS as usize + position * ADDRESSES_PER_HOST
    )
}

/// One emulator serving BOTH services off one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise behaviour, not signing.
        router.set_request_authentication(false);
        // An environment is created only for an account holding a Premium Support
        // subscription (`tests/all/premium_support_entitlement.rs` owns that rule).
        crate::premium_support_entitlement::grant_premium_support(&engine);
        router.register_service(evs::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        // The SSH key pair the host entries here name, in the account's own EC2 key pair
        // store: a `hosts` entry's `keyName` names a key pair of the caller's account. A
        // row that names another pair holds that one too.
        register_key_pair(&engine, HOST_KEY_PAIR_NAME);
        Self { router, engine }
    }

    /// A VPC and a subnet inside it in the account's own EC2 store, as
    /// `CreateEnvironment` requires — a fresh pair per environment, because Amazon EVS
    /// allows one deployed environment per VPC.
    fn fresh_network(&self) -> (String, String) {
        let ec2_state = ec2::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's EC2 store");
        let (vpc_id, subnet_id) = {
            let mut store = ec2_state.lock();
            (store.next_id("vpc"), store.next_id("subnet"))
        };
        register_network(&self.engine, &vpc_id, &subnet_id);
        (vpc_id, subnet_id)
    }

    /// A `CreateEnvironment` request that satisfies every rule, in a fresh network of
    /// this account, carrying `hosts` and the VCF version and VLAN block size given.
    fn create_request(
        &self,
        environment_name: &str,
        vcf_version: &str,
        prefix_len: u8,
        hosts: Option<&Value>,
    ) -> Value {
        let (vpc_id, subnet_id) = self.fresh_network();
        let mut request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": vcf_version,
            "termsAccepted": true,
            "environmentName": environment_name,
            "initialVlans": vlans_with_prefix(prefix_len),
        });
        if let Some(hosts) = hosts {
            request["hosts"] = hosts.clone();
        }
        request
    }

    /// Create an environment and answer its id, failing the case if it is refused.
    async fn create(&self, case: &str, request: &Value) -> String {
        let (status, body) = call(&self.router, "CreateEnvironment", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironment expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string()
    }

    /// `ListEnvironmentHosts` again and again until the environment reports `expected`
    /// hosts and every one of them has reached `CREATED`.
    ///
    /// How this suite waits for the build the clock owes each host of a create: it waits
    /// for the state to BE what it waits for, with [`POLL_DEADLINE`] of room, rather than
    /// for a span of wall clock that a faster or busier host measures differently.
    async fn hosts_once_created(
        &self,
        case: &str,
        environment: &str,
        expected: usize,
    ) -> Vec<Value> {
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            // A list addressed to an environment that is still initializing is REFUSED
            // (`tests/all/list_while_initializing.rs` owns that rule), so the poll looks
            // again at that answer too rather than failing the case on it.
            let (status, body) = call(
                &self.router,
                "ListEnvironmentHosts",
                &json!({ "environmentId": environment }),
            )
            .await;
            let listed: Vec<Value> = if status == 200 {
                body["environmentHosts"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if status == 200
                && listed.len() == expected
                && listed
                    .iter()
                    .all(|host| host["hostState"] == json!("CREATED"))
            {
                return listed;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {expected} host(s), every one CREATED; last read \
                 {status} {body}"
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The `subnetId` of the environment's `vmkManagement` VLAN, as
    /// `ListEnvironmentVlans` reports it.
    async fn vmk_management_subnet(&self, case: &str, environment: &str) -> String {
        let (status, body) = call(
            &self.router,
            "ListEnvironmentVlans",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEnvironmentVlans expected HTTP 200, got {status} {body}"
        );
        body["environmentVlans"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentVlans in {body}"))
            .iter()
            .find(|vlan| vlan["functionName"] == json!("vmkManagement"))
            .and_then(|vlan| vlan["subnetId"].as_str())
            .unwrap_or_else(|| panic!("case {case:?}: no vmkManagement VLAN in {body}"))
            .to_string()
    }

    /// Add one host to an environment, and answer what `CreateEnvironmentHost` answered.
    async fn create_host(
        &self,
        environment: &str,
        host_name: &str,
        instance_type: &str,
    ) -> (u16, Value) {
        call(
            &self.router,
            "CreateEnvironmentHost",
            &json!({
                "environmentId": environment,
                "host": {
                    "hostName": host_name,
                    "keyName": "evs-key",
                    "instanceType": instance_type,
                },
            }),
        )
        .await
    }

    /// A placement group the account really holds, minted by Amazon EC2's own
    /// `CreatePlacementGroup`, and its `GroupId`.
    async fn placement_group(&self, group_name: &str) -> String {
        let (status, body) = call_ec2(
            &self.router,
            REGION,
            "CreatePlacementGroup",
            &[("GroupName", group_name), ("Strategy", "cluster")],
        )
        .await;
        assert_eq!(
            status, 200,
            "CreatePlacementGroup {group_name:?}: expected HTTP 200, got {status} {body}"
        );
        elements(&body, "groupId")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("CreatePlacementGroup {group_name:?}: no groupId in {body}"))
    }

    /// A Dedicated Host the account really holds, allocated by Amazon EC2's own
    /// `AllocateHosts`, and its `HostId`.
    async fn dedicated_host(&self, instance_type: &str) -> String {
        let (status, body) = call_ec2(
            &self.router,
            REGION,
            "AllocateHosts",
            &[
                ("InstanceType", instance_type),
                ("AvailabilityZone", ZONE),
                ("Quantity", "1"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "AllocateHosts {instance_type:?}: expected HTTP 200, got {status} {body}"
        );
        let allocated = elements(&body, "item");
        assert_eq!(
            allocated.len(),
            1,
            "AllocateHosts {instance_type:?}: expected one allocated host, got {allocated:?}"
        );
        allocated[0].clone()
    }

    /// The instance the account's EC2 store holds for `instance_id`, as
    /// `DescribeInstances` reports it.
    async fn describe_instance(&self, case: &str, instance_id: &str) -> String {
        let (status, body) = call_ec2(
            &self.router,
            REGION,
            "DescribeInstances",
            &[("InstanceId.1", instance_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstances {instance_id} expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            elements(&body, "instanceId"),
            vec![instance_id.to_string()],
            "case {case:?}: expected exactly the instance {instance_id}, got {body}"
        );
        body
    }

    /// How many environments this service holds, so a refused create can be shown to
    /// have created nothing.
    async fn environment_count(&self) -> usize {
        let (status, body) = call(&self.router, "ListEnvironments", &json!({})).await;
        assert_eq!(status, 200, "ListEnvironments: got {status} {body}");
        body["environmentSummaries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    }

    /// The `HostState` of every host of the environment, and whether it still owes a
    /// transition, read straight off the store — the one fact a wire read cannot show,
    /// because a list addressed to an environment that is still initializing is refused.
    fn stored_host_states(&self, environment: &str) -> Vec<(String, String, bool)> {
        let state = evs::state::service_state(&*self.engine)
            .expect("the in-memory engine carries this service's store");
        let store = state.lock();
        store
            .environments
            .get(environment)
            .map(|environment| {
                environment
                    .hosts
                    .values()
                    .map(|host| {
                        (
                            host.host_name.clone(),
                            host.state.clone(),
                            host.pending.is_some(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Four `HostInfoForCreate` entries (`hosts` is modelled `len 4..4`), named
/// `<prefix>-1` … `<prefix>-4` so the request order is also the `hostName` order
/// `ListEnvironmentHosts` pages in, with `extra` merged into the entry at each index of
/// `at`.
fn four_hosts(prefix: &str, key_name: &str, instance_type: &str, extra: &Value, at: &[usize]) -> Value {
    let mut hosts: Vec<Value> = (1..=4)
        .map(|index| {
            json!({
                "hostName": format!("{prefix}-{index}"),
                "keyName": key_name,
                "instanceType": instance_type,
            })
        })
        .collect();
    for index in at {
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            hosts[*index][member] = value.clone();
        }
    }
    Value::Array(hosts)
}

/// The `fieldList` member names a refusal carries.
fn refused_members(body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a fieldList in {body}"))
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .map(str::to_string)
        .collect()
}

/// One row of the materialisation table: what the create carries, and what the
/// environment must hold afterwards.
struct Row {
    case: &'static str,
    /// The `VcfVersion` the environment is created with.
    vcf_version: &'static str,
    /// The prefix length of every VLAN subnet block, so the hosts are addressed out of
    /// blocks of different sizes.
    prefix_len: u8,
    /// The EC2 bare metal instance type every host of the row runs on.
    instance_type: &'static str,
    /// The SSH key pair every host of the row names.
    key_name: &'static str,
    /// Which placement mechanism the row's second host names, if any.
    placement: Placement,
    /// How many hosts the environment must hold once the create is served.
    expected_hosts: usize,
}

/// The placement reference a row's second host carries — each a resource the account
/// really holds, minted through Amazon EC2 itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    None,
    Group,
    Dedicated,
}

/// RULE: a create that carries `hosts` adds those hosts to the environment it mints, with
/// the values each entry supplied — and `SELF_DEPLOYED` adds none.
///
/// Each row differs along what the rule generalises over: the VCF version, the size of the
/// VLAN subnet block the hosts are addressed out of, the instance type and key pair the
/// entries name, and the placement mechanism one of them names. The recorded case
/// (`VCF-5.2.1`, `/28` blocks, `i4i.metal`, no placement) is the first row.
#[tokio::test]
async fn a_create_that_carries_hosts_adds_them_to_the_environment() {
    let rows = vec![
        Row {
            case: "VCF-5.2.1 in /28 blocks, no placement reference",
            vcf_version: "VCF-5.2.1",
            prefix_len: 28,
            instance_type: "i4i.metal",
            key_name: "evs-key",
            placement: Placement::None,
            expected_hosts: 4,
        },
        Row {
            case: "VCF-5.2.2 in /27 blocks, one host in a placement group",
            vcf_version: "VCF-5.2.2",
            prefix_len: 27,
            instance_type: "i7i.metal-24xl",
            key_name: "second_key-2",
            placement: Placement::Group,
            expected_hosts: 4,
        },
        Row {
            case: "VCF-5.2.1 in /24 blocks, one host on a Dedicated Host",
            vcf_version: "VCF-5.2.1",
            prefix_len: 24,
            instance_type: "i7i.metal-48xl",
            key_name: "THIRD-KEY",
            placement: Placement::Dedicated,
            expected_hosts: 4,
        },
        Row {
            // "no hosts are added and no VCF installation is performed" — the member is
            // accepted and does nothing, so the environment holds no host at all.
            case: "SELF_DEPLOYED adds none of the hosts it accepts",
            vcf_version: "SELF_DEPLOYED",
            prefix_len: 28,
            instance_type: "i4i.metal",
            key_name: "evs-key",
            placement: Placement::None,
            expected_hosts: 0,
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let prefix = format!("esx-{index}");
        let (extra, at) = match row.placement {
            Placement::None => (json!({}), Vec::new()),
            Placement::Group => (
                json!({
                    "placementGroupId": emulator
                        .placement_group(&format!("group-{index}"))
                        .await,
                }),
                vec![1usize],
            ),
            Placement::Dedicated => (
                json!({
                    "dedicatedHostId": emulator.dedicated_host(row.instance_type).await,
                }),
                vec![1usize],
            ),
        };
        let hosts = four_hosts(&prefix, row.key_name, row.instance_type, &extra, &at);
        register_key_pair(&emulator.engine, row.key_name);
        let request = emulator.create_request(
            &format!("hosts-env-{index}"),
            row.vcf_version,
            row.prefix_len,
            Some(&hosts),
        );
        let environment = emulator.create(case, &request).await;

        // Each host is CREATING, with the move to CREATED the clock owes it still
        // pending: a host of a create is built, not ready on the instant.
        let stored = emulator.stored_host_states(&environment);
        assert_eq!(
            stored.len(),
            row.expected_hosts,
            "case {case:?}: expected {} host(s) on the stored environment, got {stored:?}",
            row.expected_hosts
        );
        for (host_name, state, pending) in &stored {
            assert_eq!(
                (state.as_str(), *pending),
                ("CREATING", true),
                "case {case:?}: host {host_name}: a just-created host is CREATING with a \
                 transition still owed, got ({state}, pending {pending})"
            );
        }

        // The move to CREATED the clock owes each host comes due: looked for again and
        // again until the environment reports them all built ([`Emulator::hosts_once_created`]).
        let listed = emulator
            .hosts_once_created(case, &environment, row.expected_hosts)
            .await;
        assert_eq!(
            listed.len(),
            row.expected_hosts,
            "case {case:?}: expected ListEnvironmentHosts to report {} host(s), got {listed:?}",
            row.expected_hosts
        );
        if row.expected_hosts == 0 {
            // The names the request named are FREE in this mode, because no host took
            // them: the caller adds hosts with CreateEnvironmentHost instead.
            let (status, body) = emulator
                .create_host(&environment, &format!("{prefix}-1"), row.instance_type)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a mode that added no host leaves its names free, got \
                 {status} ({}) {body}",
                error_type(&body)
            );
            continue;
        }

        let management_subnet = emulator.vmk_management_subnet(case, &environment).await;
        let supplied = hosts.as_array().expect("the hosts are an array");
        let mut instance_ids: Vec<String> = Vec::new();
        for (position, host) in listed.iter().enumerate() {
            let expected = &supplied[position];
            for member in [
                "hostName",
                "keyName",
                "instanceType",
                "placementGroupId",
                "dedicatedHostId",
            ] {
                assert_eq!(
                    host[member],
                    expected
                        .get(member)
                        .cloned()
                        .unwrap_or(Value::Null),
                    "case {case:?}: host {position}: {member}: expected {}, got {}",
                    expected.get(member).cloned().unwrap_or(Value::Null),
                    host[member]
                );
            }
            // The build the clock owed the host has come due.
            assert_eq!(
                host["hostState"],
                json!("CREATED"),
                "case {case:?}: host {position}: expected CREATED, got {}",
                host["hostState"]
            );
            // The management address comes out of the environment's own vmkManagement
            // VLAN subnet, in request order from its first usable address, each host
            // taking one address per interface ([`host_address`]).
            let expected_address = host_address(position);
            assert_eq!(
                host["ipAddress"],
                json!(expected_address),
                "case {case:?}: host {position}: expected the address {expected_address} of \
                 the vmkManagement VLAN subnet, got {}",
                host["ipAddress"]
            );
            let instance_id = host["ec2InstanceId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: host {position}: a host runs on an EC2 instance: {host}")
            });
            let interfaces = host["networkInterfaces"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: host {position}: expected ENIs on {host}")
                })
                .len();
            assert_eq!(
                interfaces, 2,
                "case {case:?}: host {position}: Amazon EVS attaches two ENIs to a host, got \
                 {interfaces}"
            );
            instance_ids.push(instance_id.to_string());
        }
        let distinct: BTreeSet<&String> = instance_ids.iter().collect();
        assert_eq!(
            distinct.len(),
            instance_ids.len(),
            "case {case:?}: no two hosts may run on one instance, got {instance_ids:?}"
        );

        // The instance the account's own EC2 store holds for the first host describes
        // that host: the launch is real, not a reported id.
        let described = emulator.describe_instance(case, &instance_ids[0]).await;
        for (member, expected) in [
            ("instanceType", row.instance_type.to_string()),
            ("keyName", row.key_name.to_string()),
            ("privateIpAddress", format!("10.0.0.{FIRST_HOST_ADDRESS}")),
            ("subnetId", management_subnet.clone()),
        ] {
            assert_eq!(
                element(&described, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: DescribeInstances {member}: expected {expected:?}, got {:?}",
                element(&described, member)
            );
        }

        // The names those hosts took are TAKEN, and the next host added takes the next
        // address of the same block.
        let (status, body) = emulator
            .create_host(&environment, &format!("{prefix}-1"), row.instance_type)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a host name a created host carries must be refused, got {status} \
             ({}) {body}",
            error_type(&body)
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {}",
            error_type(&body)
        );
        assert_eq!(
            refused_members(&body),
            vec!["host.hostName".to_string()],
            "case {case:?}: expected the refusal to name host.hostName, got {body}"
        );
        let (status, body) = emulator
            .create_host(&environment, &format!("{prefix}-added"), row.instance_type)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fresh host name must be accepted, got {status} ({}) {body}",
            error_type(&body)
        );
        let expected_address = host_address(listed.len());
        assert_eq!(
            body["host"]["ipAddress"],
            json!(expected_address),
            "case {case:?}: the added host takes the address after the created hosts \
             ({expected_address}), got {}",
            body["host"]["ipAddress"]
        );
    }
}

/// RULE: a `hostName` is unique across Amazon EVS environments, so a create whose `hosts`
/// name one that is already taken — or name one twice — is refused with the entry that
/// named it, and creates nothing.
///
/// Each row differs in WHERE the collision comes from and at which index of the list it
/// sits, and the last row is the mode that adds no host and so takes no name.
#[tokio::test]
async fn a_create_may_not_take_a_host_name_that_is_already_taken() {
    let emulator = Emulator::new();
    // One environment whose create took four names, and one host added to it afterwards:
    // both populations are the same names for this rule.
    let held = emulator
        .create(
            "the environment holding the taken names",
            &emulator.create_request(
                "names-held",
                "VCF-5.2.1",
                24,
                Some(&four_hosts("held", "evs-key", "i4i.metal", &json!({}), &[])),
            ),
        )
        .await;
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, body) = emulator
        .create_host(&held, "added-later", "i4i.metal")
        .await;
    assert_eq!(
        status, 200,
        "the host added afterwards: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );

    // Each row: the case, the four host names the create carries, the VCF version, and
    // the `fieldList` names the refusal must carry (empty for a row that is served).
    let rows: Vec<(&str, [&str; 4], &str, Vec<&str>)> = vec![
        (
            "four free names",
            ["free-1", "free-2", "free-3", "free-4"],
            "VCF-5.2.1",
            vec![],
        ),
        (
            "the first entry names a host another environment's create added",
            ["held-1", "free-5", "free-6", "free-7"],
            "VCF-5.2.1",
            vec!["hosts[0].hostName"],
        ),
        (
            "the last entry names a host CreateEnvironmentHost added",
            ["free-8", "free-9", "free-10", "added-later"],
            "VCF-5.2.1",
            vec!["hosts[3].hostName"],
        ),
        (
            "two entries name hosts another environment holds",
            ["held-2", "free-11", "held-4", "free-12"],
            "VCF-5.2.1",
            vec!["hosts[0].hostName", "hosts[2].hostName"],
        ),
        (
            "one name is given twice in the same request",
            ["twice", "free-13", "twice", "free-14"],
            "VCF-5.2.1",
            vec!["hosts[2].hostName"],
        ),
        (
            // The mode adds no host, so it takes no name and collides with nothing.
            "SELF_DEPLOYED carrying names that are taken",
            ["held-1", "held-2", "held-3", "held-4"],
            "SELF_DEPLOYED",
            vec![],
        ),
    ];

    for (index, (case, names, vcf_version, expected_members)) in rows.into_iter().enumerate() {
        let hosts = Value::Array(
            names
                .iter()
                .map(|host_name| {
                    json!({
                        "hostName": host_name,
                        "keyName": "evs-key",
                        "instanceType": "i4i.metal",
                    })
                })
                .collect(),
        );
        let before = emulator.environment_count().await;
        let request = emulator.create_request(
            &format!("names-{index}"),
            vcf_version,
            24,
            Some(&hosts),
        );
        let (status, body) = call(&emulator.router, "CreateEnvironment", &request).await;
        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {}",
            error_type(&body)
        );
        assert_eq!(
            refused_members(&body),
            expected_members,
            "case {case:?}: expected one entry per taken name ({expected_members:?}), got {body}"
        );
        let after = emulator.environment_count().await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must create nothing, got {after} environment(s) \
             where there were {before}"
        );
    }
}
