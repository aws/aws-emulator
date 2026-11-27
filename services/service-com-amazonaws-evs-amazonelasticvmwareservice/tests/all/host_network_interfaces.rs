// SPDX-License-Identifier: CC0-1.0

//! The elastic network interfaces a host reports are interfaces the account's own Amazon
//! EC2 store HOLDS.
//!
//! The rule under test: an id this service reports for a resource it created in another
//! service's store names a record that store holds. `networkInterfaces` is "The elastic
//! network interfaces that are attached to the host" and "Amazon EVS provisions two
//! identically configured ENIs in the VMkernel management subnet during host creation. One
//! ENI is active, and the other is in standby mode for automatic switchover during a
//! failure scenario" (`NetworkInterface` shape doc), so for every host added to an
//! environment:
//!
//! * each reported `networkInterfaceId` resolves in Amazon EC2 — `DescribeNetworkInterfaces`
//!   answers with that interface rather than `InvalidNetworkInterfaceID.NotFound`;
//! * each interface sits in the environment's own `vmkManagement` VLAN subnet, attached to
//!   the host's own instance, at device index 0 (the active ENI, carrying the host's
//!   management address) and 1 (the standby one);
//! * the instance the host runs on reports exactly those interfaces, so the two stores
//!   cannot disagree about one host; and
//! * no address of that subnet is held twice, however many hosts the environment holds.
//!
//! The rows span the subnet block size the hosts are addressed out of and how many hosts
//! an environment holds, because that is what the rule generalises over; the negative
//! control (an id the account never minted) is what makes the resolutions above
//! meaningful.
//!
//! Both services are mounted on ONE shared engine here and every call goes over the wire
//! the way a client makes it — awsJson1_0 for Amazon EVS, ec2Query for Amazon EC2 — so
//! what is asserted is the cross-service fact, not an internal call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, error_type, poll_environment_created, register_key_pair, register_network,
    vlans_with_prefix, HOST_KEY_PAIR_NAME, POLL_DEADLINE, POLL_INTERVAL, REGION,
};
// The ec2Query client and the XML readers the sibling EC2-facing suites already own:
// this suite asks Amazon EC2 the same questions they do.
use crate::vlan_subnet_propagation::{call_ec2, element, elements, nested};

/// A well-formed `NetworkInterfaceId` the account never minted: the negative control that
/// shows `DescribeNetworkInterfaces` really refuses an id nothing holds, so resolving the
/// ids a host reports asserts something.
const UNMINTED_INTERFACE_ID: &str = "eni-0123456789abcdef0";

/// One emulator serving BOTH services off one shared engine: Amazon EVS creates the host,
/// Amazon EC2 is asked about the interfaces that host runs on.
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
        router.register_service(
            service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        // A host's `keyName` names an Amazon EC2 key pair of the caller's account
        // (`tests/all/host_key_pair_references.rs` owns that rule), so the one every host
        // below names is in that store.
        register_key_pair(&engine, HOST_KEY_PAIR_NAME);
        Self { router, engine }
    }

    /// A VPC and a subnet inside it in the account's own EC2 store, as `CreateEnvironment`
    /// requires — a fresh pair per environment, because Amazon EVS allows one deployed
    /// environment per VPC.
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

    /// Create an environment whose VLAN subnets are `/prefix_len` blocks and wait for the
    /// deployment the clock owes it — by looking at the environment again and again until
    /// it IS `CREATED`, so a loaded host cannot fail the test — and answer its id.
    async fn deployed_environment(&self, case: &str, prefix_len: u8) -> String {
        let environment_name: String = case
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        let (vpc_id, subnet_id) = self.fresh_network();
        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": "VCF-5.2.1",
            "termsAccepted": true,
            "environmentName": environment_name,
            "initialVlans": vlans_with_prefix(prefix_len),
        });
        let (status, body) = call(&self.router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironment expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string();
        // The deployment the clock owes it, waited for through the crate's own
        // poll-until helper ([`poll_environment_created`]).
        poll_environment_created(&self.router, case, &environment).await;
        environment
    }

    /// Add one host and answer the `host` object the create reported.
    async fn create_host(&self, case: &str, environment: &str, host_name: &str) -> Value {
        let request = json!({
            "environmentId": environment,
            "host": {
                "hostName": host_name,
                "keyName": HOST_KEY_PAIR_NAME,
                "instanceType": "i4i.metal",
            },
        });
        let (status, body) = call(&self.router, "CreateEnvironmentHost", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironmentHost expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        body["host"].clone()
    }

    /// Delete one host and answer nothing: what it leaves behind is read in EC2.
    async fn delete_host(&self, case: &str, environment: &str, host_name: &str) {
        let (status, body) = call(
            &self.router,
            "DeleteEnvironmentHost",
            &json!({ "environmentId": environment, "hostName": host_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEnvironmentHost expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
    }

    /// The `subnetId` of the environment's `vmkManagement` VLAN, as `ListEnvironmentVlans`
    /// reports it.
    async fn vmk_management_subnet(&self, environment: &str) -> String {
        let (status, body) = call(
            &self.router,
            "ListEnvironmentVlans",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(status, 200, "ListEnvironmentVlans: got {status} {body}");
        body["environmentVlans"]
            .as_array()
            .unwrap_or_else(|| panic!("ListEnvironmentVlans: no environmentVlans in {body}"))
            .iter()
            .find(|vlan| vlan["functionName"] == json!("vmkManagement"))
            .and_then(|vlan| vlan["subnetId"].as_str())
            .unwrap_or_else(|| panic!("ListEnvironmentVlans: no vmkManagement VLAN in {body}"))
            .to_string()
    }

    /// `DescribeNetworkInterfaces` for exactly the ids given, through the crate's shared
    /// ec2Query client ([`call_ec2`]).
    async fn describe_interfaces(&self, ids: &[&str]) -> (u16, String) {
        let params: Vec<(String, String)> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| (format!("NetworkInterfaceId.{}", index + 1), (*id).to_string()))
            .collect();
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        call_ec2(
            &self.router,
            REGION,
            "DescribeNetworkInterfaces",
            &borrowed,
        )
        .await
    }

    /// The one interface EC2 holds for `interface_id`, as `DescribeNetworkInterfaces`
    /// reports it — the call that fails the test when the id resolves to nothing.
    async fn describe_interface(&self, case: &str, interface_id: &str) -> String {
        let (status, body) = self.describe_interfaces(&[interface_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeNetworkInterfaces for {interface_id} expected 200, got \
             {status} {body}"
        );
        assert_eq!(
            elements(&body, "networkInterfaceId"),
            vec![interface_id.to_string()],
            "case {case:?}: expected exactly that interface, got {body}"
        );
        body
    }

    /// The one instance EC2 holds for `instance_id`, as `DescribeInstances` reports it.
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
            "case {case:?}: DescribeInstances for {instance_id} expected 200, got {status} {body}"
        );
        body
    }
}

/// The `ec2InstanceId` a host reports, which must be there.
fn instance_id_of(host: &Value, case: &str) -> String {
    host["ec2InstanceId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a host must report the id of the EC2 instance it runs on, got {host}")
        })
        .to_string()
}

/// The `networkInterfaces` ids a host reports, in the order reported.
fn interface_ids_of(host: &Value, case: &str) -> Vec<String> {
    host["networkInterfaces"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a host must report the ENIs attached to it, got {host}")
        })
        .iter()
        .map(|interface| {
            interface["networkInterfaceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported ENI carries an id, got {interface}")
                })
                .to_string()
        })
        .collect()
}

/// The `ipAddress` a host reports: its management address.
fn management_ip_of(host: &Value, case: &str) -> String {
    host["ipAddress"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a host must report its management address, got {host}"))
        .to_string()
}

/// Every ENI a host reports resolves in Amazon EC2, in the environment's VMkernel
/// management subnet and attached to that host's own instance — whatever the size of the
/// subnet block and however many hosts the environment holds.
#[tokio::test]
async fn every_reported_host_interface_is_an_interface_ec2_holds() {
    // Each row differs in what the rule generalises over: the block the hosts are
    // addressed out of (the whole `/28`…`/24` range Amazon EVS allows for a VLAN subnet,
    // `InitialVlans` doc) and how many hosts share that subnet.
    let rows: [(&str, u8, usize); 3] = [
        ("one host in a /28 block", 28, 1),
        ("two hosts in a /27 block", 27, 2),
        ("three hosts in a /24 block", 24, 3),
    ];

    for (case, prefix_len, host_count) in rows {
        let emulator = Emulator::new();
        let environment = emulator.deployed_environment(case, prefix_len).await;
        let management_subnet = emulator.vmk_management_subnet(&environment).await;
        // Every address and every interface id this environment hands out, so no two
        // interfaces of the one subnet can be found holding one address or one id.
        let mut addresses: Vec<String> = Vec::new();
        let mut interface_ids: Vec<String> = Vec::new();

        for index in 1..=host_count {
            let case = &format!("{case}, host {index}");
            let host_name = format!("esx-{index}");
            let host = emulator.create_host(case, &environment, &host_name).await;
            let instance_id = instance_id_of(&host, case);
            let management_ip = management_ip_of(&host, case);
            let reported = interface_ids_of(&host, case);
            assert_eq!(
                reported.len(),
                2,
                "case {case:?}: expected the two ENIs Amazon EVS provisions, got {reported:?}"
            );

            // Each reported id RESOLVES: the interface it names is in the VMkernel
            // management subnet, attached to this host's instance, and the active one (device
            // index 0) carries the host's own management address.
            for (position, interface_id) in reported.iter().enumerate() {
                let described = emulator.describe_interface(case, interface_id).await;
                assert_eq!(
                    element(&described, "subnetId").as_deref(),
                    Some(management_subnet.as_str()),
                    "case {case:?}: {interface_id} must live in the vmkManagement subnet, got \
                     {described}"
                );
                assert_eq!(
                    nested(&described, "attachment", "instanceId").as_deref(),
                    Some(instance_id.as_str()),
                    "case {case:?}: {interface_id} must be attached to the host's instance, got \
                     {described}"
                );
                assert_eq!(
                    nested(&described, "attachment", "deviceIndex"),
                    Some(position.to_string()),
                    "case {case:?}: {interface_id} must be attached at device index {position}, \
                     got {described}"
                );
                let address = element(&described, "privateIpAddress").unwrap_or_else(|| {
                    panic!("case {case:?}: {interface_id} must hold a private address, got {described}")
                });
                if position == 0 {
                    assert_eq!(
                        address, management_ip,
                        "case {case:?}: the active ENI carries the host's management address, \
                         got {address:?}"
                    );
                }
                addresses.push(address);
                interface_ids.push(interface_id.clone());
            }

            // And the instance the host runs on reports exactly those interfaces, so
            // neither store names an interface the other does not.
            let described = emulator.describe_instance(case, &instance_id).await;
            let mut held = elements(&described, "networkInterfaceId");
            held.sort();
            let mut expected = reported.clone();
            expected.sort();
            assert_eq!(
                held, expected,
                "case {case:?}: the instance must report the host's own ENIs, got {described}"
            );
        }

        let distinct_ids: BTreeSet<&String> = interface_ids.iter().collect();
        assert_eq!(
            distinct_ids.len(),
            interface_ids.len(),
            "case {case:?}: no interface id may be handed out twice, got {interface_ids:?}"
        );
        let distinct_addresses: BTreeSet<&String> = addresses.iter().collect();
        assert_eq!(
            distinct_addresses.len(),
            addresses.len(),
            "case {case:?}: no address of the subnet may be held twice, got {addresses:?}"
        );
    }
}

/// An id the account never minted is refused — the control that gives the resolutions
/// above their meaning — and so is one of a host's interfaces once that host is deleted,
/// because the interfaces go with the instance an ESX host IS.
#[tokio::test]
async fn an_interface_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let environment = emulator
        .deployed_environment("unminted-interface", 28)
        .await;

    let (status, body) = emulator.describe_interfaces(&[UNMINTED_INTERFACE_ID]).await;
    assert_eq!(
        status, 400,
        "an id the account never minted must be refused, got {status} {body}"
    );
    assert!(
        body.contains("InvalidNetworkInterfaceID.NotFound"),
        "expected InvalidNetworkInterfaceID.NotFound, got {body}"
    );

    // A host's own interfaces, by contrast, resolve — and stop resolving once the host,
    // and so the instance they are attached to, is gone.
    let host = emulator
        .create_host("deleted-host", &environment, "esx-1")
        .await;
    let reported = interface_ids_of(&host, "deleted-host");
    for interface_id in &reported {
        emulator.describe_interface("deleted-host", interface_id).await;
    }
    emulator
        .delete_host("deleted-host", &environment, "esx-1")
        .await;

    // Look again until the account holds NEITHER interface — each id asked for on its own,
    // so the loop cannot exit while one of the two is still held. The deletion is owed by
    // the clock (it follows the termination of the host's instance), so the test waits for
    // the CONDITION with generous room rather than for a span.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let mut still_held: Vec<String> = Vec::new();
        let mut last = String::new();
        for interface_id in &reported {
            let (status, body) = emulator.describe_interfaces(&[interface_id]).await;
            if !(status == 400 && body.contains("InvalidNetworkInterfaceID.NotFound")) {
                still_held.push(interface_id.clone());
                last = format!("{status} {body}");
            }
        }
        if still_held.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a deleted host's ENIs must stop being held by the account; {still_held:?} still \
             resolve; last read {last}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}
