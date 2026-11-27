// SPDX-License-Identifier: CC0-1.0

//! An Amazon EVS host IS an EC2 bare metal instance, so adding one to an environment
//! puts a real instance in the ACCOUNT's own Amazon EC2 store.
//!
//! The rule under test, from the model's own documentation: "An ESX host that runs on an
//! Amazon EC2 bare metal instance" (`Host` doc) with `ec2InstanceId` "The unique ID of
//! the EC2 instance that represents the host" and `networkInterfaces` "The elastic
//! network interfaces that are attached to the host" — "Amazon EVS provisions two
//! identically configured ENIs in the VMkernel management subnet during host creation"
//! (`NetworkInterface` doc). So `CreateEnvironmentHost`:
//!
//! * launches ONE instance in the account's EC2 store per host, from the `keyName` and
//!   `instanceType` the request gave, addressed at the host's own management address and
//!   in the environment's `vmkManagement` VLAN subnet — the instance EC2's own
//!   `DescribeInstances` then reports for that id;
//! * attaches exactly two elastic network interfaces to it and reports their ids;
//! * reports the same id and interfaces on every later read of the host; and
//! * launches nothing at all for a request it refuses.
//!
//! Both services are mounted on ONE shared engine here, and every call goes over the
//! wire the way a client makes it — awsJson1_0 for Amazon EVS, ec2Query for Amazon EC2 —
//! so what is asserted is the cross-service workflow (add a host, look its instance up
//! in EC2), not an internal call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, error_type, register_key_pair, register_network, vlans_with_prefix, HOST_KEY_PAIR_NAME,
    REGION,
};
use crate::host_instance_type_catalogue::vcf_version_providing;

/// Long enough for the transitions the two services owe a new environment, host and
/// instance to come due (each span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// One emulator serving BOTH services off one shared engine: Amazon EVS creates the
/// host, Amazon EC2 is asked about the instance that host runs on.
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
        // The SSH key pair the host fixtures here name, in the account's own EC2 key pair
        // store: a host's `keyName` names a key pair of the caller's account. A row that
        // names another pair holds that one too ([`Emulator::create_host`]).
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

    /// Create an environment whose VLAN subnets are `/prefix_len` blocks and wait for
    /// the deployment the clock owes it, so a host can be added to it.
    ///
    /// `name` names the CASE in every assertion message; the environment name it is
    /// given is that case with anything outside the modelled
    /// `^[a-zA-Z0-9_-]+$` replaced, so a case can be described in prose.
    ///
    /// The environment runs a VCF version Amazon EVS provides `instance_type` on, because a
    /// host is built only on hardware its environment's version provides — "EC2 instance
    /// types provided by Amazon EVS for this VCF version for creating environments"
    /// (`VcfVersionInfo$instanceTypes`), the rule
    /// `tests/all/host_instance_type_catalogue.rs` owns.
    async fn deployed_environment(
        &self,
        name: &str,
        prefix_len: u8,
        instance_type: &str,
    ) -> String {
        let environment_name: String = name
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
            "vcfVersion": vcf_version_providing(instance_type),
            "termsAccepted": true,
            "environmentName": environment_name,
            "initialVlans": vlans_with_prefix(prefix_len),
        });
        let (status, body) = call(&self.router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateEnvironment expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no environmentId in {body}"))
            .to_string();
        std::thread::sleep(PAST_THE_TRANSITION);
        environment
    }

    /// Add one host and answer the `host` object the create reported.
    ///
    /// The account holds the SSH key pair the host names, because `keyName` names a key
    /// pair of the caller's own Amazon EC2 store.
    async fn create_host(
        &self,
        environment: &str,
        host_name: &str,
        key_name: &str,
        instance_type: &str,
    ) -> Value {
        register_key_pair(&self.engine, key_name);
        let request = json!({
            "environmentId": environment,
            "host": {
                "hostName": host_name,
                "keyName": key_name,
                "instanceType": instance_type,
            },
        });
        let (status, body) = call(&self.router, "CreateEnvironmentHost", &request).await;
        assert_eq!(
            status, 200,
            "case {host_name:?}: CreateEnvironmentHost expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        body["host"].clone()
    }

    /// The `subnetId` of the environment's `vmkManagement` VLAN, as
    /// `ListEnvironmentVlans` reports it.
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

    /// Every host of the environment, as `ListEnvironmentHosts` reports them.
    async fn list_hosts(&self, environment: &str) -> Vec<Value> {
        let (status, body) = call(
            &self.router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(status, 200, "ListEnvironmentHosts: got {status} {body}");
        body["environmentHosts"]
            .as_array()
            .unwrap_or_else(|| panic!("ListEnvironmentHosts: no environmentHosts in {body}"))
            .clone()
    }

    /// One ec2Query call: `application/x-www-form-urlencoded` in, XML out.
    async fn ec2_call(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
        for (name, value) in params {
            body.push_str(&format!("&{name}={value}"));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            // The `ec2` signing name in the credential scope is what routes the request
            // to Amazon EC2 and selects the ec2Query envelope.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/ec2/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(::bytes::Bytes::from(body))
            .expect("a well-formed ec2Query request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// The one instance EC2 holds for `instance_id`, as `DescribeInstances` reports it.
    ///
    /// The describe is FILTERED to that id, so the response carries exactly one instance
    /// and each member below is read unambiguously out of it.
    async fn describe_instance(&self, instance_id: &str) -> String {
        let (status, body) = self
            .ec2_call("DescribeInstances", &[("InstanceId.1", instance_id)])
            .await;
        assert_eq!(
            status, 200,
            "DescribeInstances for {instance_id}: expected 200, got {status} {body}"
        );
        assert_eq!(
            elements(&body, "instanceId"),
            vec![instance_id.to_string()],
            "DescribeInstances for {instance_id}: expected exactly that instance, got {body}"
        );
        body
    }

    /// Every instance id the account's EC2 store reports, unfiltered.
    async fn all_instance_ids(&self) -> Vec<String> {
        let (status, body) = self.ec2_call("DescribeInstances", &[]).await;
        assert_eq!(
            status, 200,
            "DescribeInstances: expected 200, got {status} {body}"
        );
        elements(&body, "instanceId")
    }
}

/// The text of the first `<name>…</name>` element of `xml`.
fn element(xml: &str, name: &str) -> Option<String> {
    elements(xml, name).into_iter().next()
}

/// The text of every `<name>…</name>` element of `xml`, in document order.
fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// The text of `<child>` inside the first `<parent>` of `xml` — `instanceState/name`,
/// which is where an instance's state word lives.
fn nested(xml: &str, parent: &str, child: &str) -> Option<String> {
    let open = format!("<{parent}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{parent}>"))? + start;
    element(&xml[start..end], child)
}

/// Whether `id` is `<prefix>-` followed by exactly 17 lowercase hex digits — the long
/// EC2 resource id form every id this emulator mints takes.
fn is_long_ec2_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(&format!("{prefix}-")) {
        Some(suffix) => {
            suffix.len() == 17
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }
        None => false,
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

/// The `networkInterfaces` ids a host reports.
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

/// One host per row, each differing in the instance type the model offers, the SSH key
/// pair and the size of the VLAN subnet block the host is addressed out of: the
/// instance EC2 holds describes the host Amazon EVS reports, whatever the request asked
/// for.
///
/// The three instance types are the WHOLE modelled `InstanceType` enum
/// (`smithy_query(query="shape", shape="InstanceType")` reports exactly `i4i.metal`,
/// `i7i.metal-24xl` and `i7i.metal-48xl`), so the row the existing host suite already
/// covers (`i4i.metal`) is one of three here.
#[tokio::test]
async fn every_created_host_runs_on_an_ec2_instance_the_account_holds() {
    let rows: [(&str, &str, &str, u8); 3] = [
        ("i4i.metal in a /28 block", "i4i.metal", "evs-key", 28),
        (
            "i7i.metal-24xl in a /27 block",
            "i7i.metal-24xl",
            "second_key-2",
            27,
        ),
        (
            "i7i.metal-48xl in a /24 block",
            "i7i.metal-48xl",
            "THIRD-KEY",
            24,
        ),
    ];

    for (case, instance_type, key_name, prefix_len) in rows {
        let emulator = Emulator::new();
        let environment = emulator
            .deployed_environment(case, prefix_len, instance_type)
            .await;
        let management_subnet = emulator.vmk_management_subnet(&environment).await;

        let host = emulator
            .create_host(&environment, "esx-1", key_name, instance_type)
            .await;
        let instance_id = instance_id_of(&host, case);
        assert!(
            is_long_ec2_id(&instance_id, "i"),
            "case {case:?}: expected an EC2 instance id, got {instance_id:?}"
        );
        let interfaces = interface_ids_of(&host, case);
        // "Amazon EVS provisions two identically configured ENIs" (`NetworkInterface`
        // doc), which is also the modelled `@length(max: 2)` on `Host$networkInterfaces`.
        assert_eq!(
            interfaces.len(),
            2,
            "case {case:?}: expected two ENIs on the host, got {interfaces:?}"
        );
        for interface in &interfaces {
            assert!(
                is_long_ec2_id(interface, "eni"),
                "case {case:?}: expected an ENI id, got {interface:?}"
            );
        }
        assert_ne!(
            interfaces[0], interfaces[1],
            "case {case:?}: the two ENIs are distinct interfaces, got {interfaces:?}"
        );

        // The instance the account's own EC2 store holds for that id describes this host.
        let management_ip = host["ipAddress"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ipAddress on {host}"))
            .to_string();
        let described = emulator.describe_instance(&instance_id).await;
        for (member, expected) in [
            ("instanceType", instance_type.to_string()),
            ("keyName", key_name.to_string()),
            // The host's management address IS the instance's primary private address,
            // and it lives in the `vmkManagement` VLAN subnet.
            ("privateIpAddress", management_ip.clone()),
            ("subnetId", management_subnet.clone()),
        ] {
            assert_eq!(
                element(&described, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: DescribeInstances {member}: expected {expected:?}, got {:?} in {described}",
                element(&described, member)
            );
        }
        let state = nested(&described, "instanceState", "name").unwrap_or_default();
        assert!(
            state == "pending" || state == "running",
            "case {case:?}: a just-launched instance is pending or running, got {state:?}"
        );

        // Every later read of the host reports the SAME instance and interfaces, and the
        // instance EC2 launched reaches `running` on EC2's own clock.
        std::thread::sleep(PAST_THE_TRANSITION);
        let listed = emulator.list_hosts(&environment).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the one host, got {listed:?}"
        );
        assert_eq!(
            instance_id_of(&listed[0], case),
            instance_id,
            "case {case:?}: the host must keep reporting the instance it runs on"
        );
        assert_eq!(
            interface_ids_of(&listed[0], case),
            interfaces,
            "case {case:?}: the host must keep reporting the same ENIs"
        );
        assert_eq!(
            listed[0]["hostState"],
            json!("CREATED"),
            "case {case:?}: expected the host to have reached CREATED, got {}",
            listed[0]["hostState"]
        );
        let described = emulator.describe_instance(&instance_id).await;
        assert_eq!(
            nested(&described, "instanceState", "name").as_deref(),
            Some("running"),
            "case {case:?}: the launched instance must reach running, got {described}"
        );
    }
}

/// Each host of an environment is its OWN instance with its OWN interfaces: nothing is
/// shared between hosts, and the account's EC2 store holds exactly one instance per host
/// and no more.
#[tokio::test]
async fn hosts_do_not_share_an_instance_or_an_interface() {
    let emulator = Emulator::new();
    let environment = emulator
        .deployed_environment("many-hosts", 27, "i4i.metal")
        .await;
    // Before the first host, the environment's own creation has launched nothing.
    assert!(
        emulator.all_instance_ids().await.is_empty(),
        "creating an environment must launch no instance, got {:?}",
        emulator.all_instance_ids().await
    );

    let mut instance_ids: Vec<String> = Vec::new();
    let mut interfaces: Vec<String> = Vec::new();
    // Three hosts, which is what the rule generalises over: one per host, every time.
    for index in 1..=3 {
        let case = format!("host {index}");
        let host_name = format!("esx-{index}");
        let host = emulator
            .create_host(&environment, &host_name, "evs-key", "i4i.metal")
            .await;
        let instance_id = instance_id_of(&host, &case);
        // The instance carries the address of ITS host, not of another.
        let described = emulator.describe_instance(&instance_id).await;
        assert_eq!(
            element(&described, "privateIpAddress"),
            host["ipAddress"].as_str().map(str::to_string),
            "case {case:?}: the instance must carry its own host's address, got {described}"
        );
        instance_ids.push(instance_id);
        interfaces.extend(interface_ids_of(&host, &case));
    }

    let distinct_instances: BTreeSet<&String> = instance_ids.iter().collect();
    assert_eq!(
        distinct_instances.len(),
        instance_ids.len(),
        "no two hosts may run on one instance, got {instance_ids:?}"
    );
    let distinct_interfaces: BTreeSet<&String> = interfaces.iter().collect();
    assert_eq!(
        distinct_interfaces.len(),
        interfaces.len(),
        "no two hosts may share an ENI, got {interfaces:?}"
    );
    let mut held = emulator.all_instance_ids().await;
    held.sort();
    let mut expected = instance_ids.clone();
    expected.sort();
    assert_eq!(
        held, expected,
        "the account's EC2 store must hold exactly one instance per host, got {held:?}"
    );
}

/// A `CreateEnvironmentHost` the service REFUSES launches nothing: the account's EC2
/// store is left exactly as it was, whichever precondition the request broke.
#[tokio::test]
async fn a_refused_host_request_launches_no_instance() {
    let emulator = Emulator::new();
    let deployed = emulator
        .deployed_environment("refusals", 28, "i4i.metal")
        .await;
    let creating = {
        // An environment whose deployment has not come due yet, so a host cannot be
        // added to it.
        let (vpc_id, subnet_id) = emulator.fresh_network();
        let (status, body) = call(
            &emulator.router,
            "CreateEnvironment",
            &json!({
                "vpcId": vpc_id,
                "serviceAccessSubnetId": subnet_id,
                "vcfVersion": "VCF-5.2.1",
                "termsAccepted": true,
                "environmentName": "still-creating",
                "initialVlans": vlans_with_prefix(28),
            }),
        )
        .await;
        assert_eq!(status, 200, "the second create: got {status} {body}");
        body["environment"]["environmentId"]
            .as_str()
            .expect("an environmentId")
            .to_string()
    };
    let taken = emulator
        .create_host(&deployed, "esx-taken", "evs-key", "i4i.metal")
        .await;
    let before = emulator.all_instance_ids().await;
    assert_eq!(
        before,
        vec![instance_id_of(&taken, "the accepted host")],
        "the accepted host launched exactly one instance, got {before:?}"
    );

    // Each row is one refused request: a precondition of the operation, broken.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an environment this service holds no record of",
            json!({
                "environmentId": "env-0000000000",
                "host": { "hostName": "esx-absent", "keyName": "evs-key", "instanceType": "i4i.metal" },
            }),
        ),
        (
            "an environment that has not finished deploying",
            json!({
                "environmentId": creating,
                "host": { "hostName": "esx-early", "keyName": "evs-key", "instanceType": "i4i.metal" },
            }),
        ),
        (
            "a host name another host already carries",
            json!({
                "environmentId": deployed,
                "host": { "hostName": "esx-taken", "keyName": "evs-key", "instanceType": "i4i.metal" },
            }),
        ),
        (
            "both placement mechanisms on one host",
            json!({
                "environmentId": deployed,
                "host": {
                    "hostName": "esx-placement",
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                    "dedicatedHostId": "h-0123456789abcdef0",
                    "placementGroupId": "pg-0123456789abcdef0",
                },
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&emulator.router, "CreateEnvironmentHost", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        let after = emulator.all_instance_ids().await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused host request must launch no instance, got {after:?}"
        );
    }
}
