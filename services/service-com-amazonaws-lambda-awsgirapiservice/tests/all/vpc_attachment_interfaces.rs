// SPDX-License-Identifier: CC0-1.0

//! The VPC attachment of a function, as the ACCOUNT's own Amazon EC2 inventory holds it.
//!
//! RULE: "To connect a function to a VPC, Lambda creates an elastic network interface for
//! each subnet in your function's VPC configuration" — so a write that applies a
//! `VpcConfig` naming subnets
//!
//! * stands up one service-managed network interface per named subnet in the account's EC2
//!   inventory, of the modelled `lambda` interface type, carrying the security groups the
//!   configuration names, in the VPC those subnets belong to; and
//! * reports that VPC as `VpcConfigResponse$VpcId` ("The ID of the VPC") on every read of
//!   the configuration — an id resolved against the subnets, never one the request carried.
//!
//! Those interfaces hold a place in the subnet they sit in for as long as the attachment
//! does, so the subnet cannot be deleted while the function is attached; removing the
//! attachment, and deleting the function, give them back.
//!
//! Every VPC, subnet and security group below was MINTED by Amazon EC2 itself, through its
//! own `CreateVpc`/`CreateSubnet`/`CreateSecurityGroup` driven over the in-process callback,
//! and every interface is read back through its own `DescribeNetworkInterfaces` — so nothing
//! here passes off a record of its own as one the account holds. The two services run on one
//! shared engine, exactly as the router mounts them.
//!
//! The rows differ along what the rule generalises over: how many subnets a configuration
//! names, which VPC they belong to, how many security groups it names, whether the subnet is
//! one a client minted or one of the account's default network, and whether a named subnet
//! resolves at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_sdk_ec2::error::ProvideErrorMetadata as _;
use aws_sdk_ec2::types::{Filter, NetworkInterface};
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::function_lifecycle::{
    create_function, json_call, make_update_due, package_bytes, zip_file, REGION, ROLE,
};

/// A form-valid subnet id no account ever minted: the id form a `SubnetIds` member takes,
/// naming nothing.
const UNKNOWN_SUBNET: &str = "subnet-0000000000bad0bad";

/// The `NetworkInterfaceType` a function's own interfaces are of, from the modelled EC2
/// enum's `lambda` member (`smithy_query(query="shape",shape="NetworkInterfaceType")`).
const LAMBDA_INTERFACE_TYPE: &str = "lambda";

/// The principal that creates those interfaces: the Lambda service principal, which
/// `NetworkInterface$RequesterId` names. Written out here rather than taken from the crate, so
/// the assertion is independent of the constant the implementation carries.
const LAMBDA_SERVICE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// One case of the create table: its name, the subnets the configuration names, the groups it
/// names, the `VpcId` it must report, and the subnets an interface must sit in.
type CreateRow = (
    &'static str,
    Vec<String>,
    Vec<String>,
    Option<String>,
    BTreeSet<String>,
);

/// One case of the refusal table: its name, the function name the request uses, the extra
/// request members that make it refusable, and the status and error code it must be refused
/// with.
type RefusalRow = (
    &'static str,
    &'static str,
    Vec<(&'static str, serde_json::Value)>,
    u16,
    &'static str,
);

/// Lambda and the Amazon EC2 service that owns VPCs, subnets, security groups and network
/// interfaces, on ONE shared engine, plus an EC2 client over the router's own in-process
/// callback.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    ec2: aws_sdk_ec2::Client,
}

/// An EC2 client for [`REGION`] over `callback`, the in-process seam this emulator routes on:
/// no socket is opened and the endpoint URL is only the address that transport routes by.
fn ec2_client(callback: &EmbeddedEmulator) -> aws_sdk_ec2::Client {
    let config = aws_sdk_ec2::Config::builder()
        .behavior_version(aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(aws_sdk_ec2::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_ec2::Client::from_conf(config)
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert the attachment, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let callback = EmbeddedEmulator::from_router(router.clone());
    Emulator {
        ec2: ec2_client(&callback),
        engine,
        router,
    }
}

/// One VPC with one subnet per entry of `subnet_cidrs` and one security group, created
/// through EC2's own API; the ids are the ones EC2 MINTED.
struct Network {
    vpc_id: String,
    subnet_ids: Vec<String>,
    group_id: String,
}

async fn network(ec2: &aws_sdk_ec2::Client, case: &str, prefix: u8, subnets: usize) -> Network {
    let created = ec2
        .create_vpc()
        .cidr_block(format!("10.{prefix}.0.0/16"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 must create the VPC: {error:?}"));
    let vpc_id = created
        .vpc()
        .and_then(aws_sdk_ec2::types::Vpc::vpc_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted"))
        .to_string();

    let mut subnet_ids = Vec::with_capacity(subnets);
    for index in 0..subnets {
        let created = ec2
            .create_subnet()
            .vpc_id(&vpc_id)
            .cidr_block(format!("10.{prefix}.{index}.0/24"))
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: EC2 must create subnet {index}: {error:?}")
            });
        subnet_ids.push(
            created
                .subnet()
                .and_then(aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: CreateSubnet must report the subnet it minted")
                })
                .to_string(),
        );
    }

    let created = ec2
        .create_security_group()
        .vpc_id(&vpc_id)
        .group_name(format!("fn-access-{prefix}"))
        .description("the group a VPC-attached function carries")
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 must create the group: {error:?}"));
    let group_id = created
        .group_id()
        .unwrap_or_else(|| panic!("case {case:?}: CreateSecurityGroup must report the group"))
        .to_string();

    Network {
        vpc_id,
        subnet_ids,
        group_id,
    }
}

/// A subnet of the account's DEFAULT network in [`REGION`], read back from EC2 — the network
/// EC2 materialises for every Region without anyone creating one. Selected by the flag EC2
/// reports for exactly those subnets (`DefaultForAz`), so a subnet a case created cannot be
/// mistaken for one of them.
async fn default_subnet(ec2: &aws_sdk_ec2::Client) -> (String, String) {
    let described = ec2
        .describe_subnets()
        .send()
        .await
        .expect("EC2 must report the account's subnets");
    let mut reported: Vec<(String, String)> = described
        .subnets()
        .iter()
        .filter(|subnet| subnet.default_for_az() == Some(true))
        .filter(|subnet| {
            subnet
                .availability_zone()
                .is_some_and(|zone| zone.starts_with(REGION))
        })
        .filter_map(|subnet| {
            Some((
                subnet.subnet_id()?.to_string(),
                subnet.vpc_id()?.to_string(),
            ))
        })
        .collect();
    reported.sort();
    reported
        .into_iter()
        .next()
        .expect("the account must have a default subnet in this Region")
}

/// Every interface of the account's inventory that belongs to the function `function_name`,
/// read through EC2's own `DescribeNetworkInterfaces` and selected by the interface type the
/// model gives a function's interfaces and the description that names the function.
async fn function_interfaces(
    ec2: &aws_sdk_ec2::Client,
    function_name: &str,
) -> Vec<NetworkInterface> {
    let described = ec2
        .describe_network_interfaces()
        .filters(
            Filter::builder()
                .name("interface-type")
                .values(LAMBDA_INTERFACE_TYPE)
                .build(),
        )
        .send()
        .await
        .expect("EC2 must report the account's network interfaces");
    let description = format!("AWS Lambda VPC ENI-{function_name}");
    described
        .network_interfaces()
        .iter()
        .filter(|interface| interface.description() == Some(description.as_str()))
        .cloned()
        .collect()
}

/// Every interface of the account's inventory that is one of a function's, whichever function
/// — what a case counts to see that a refused create left nothing behind.
async fn all_lambda_interfaces(ec2: &aws_sdk_ec2::Client) -> Vec<NetworkInterface> {
    let described = ec2
        .describe_network_interfaces()
        .filters(
            Filter::builder()
                .name("interface-type")
                .values(LAMBDA_INTERFACE_TYPE)
                .build(),
        )
        .send()
        .await
        .expect("EC2 must report the account's network interfaces");
    described.network_interfaces().to_vec()
}

/// The subnets of `interfaces`, sorted, as a set compares them.
fn subnets_of(interfaces: &[NetworkInterface]) -> BTreeSet<String> {
    interfaces
        .iter()
        .filter_map(|interface| interface.subnet_id().map(str::to_string))
        .collect()
}

/// The security groups of one interface, in the order the describe reports them.
fn groups_of(interface: &NetworkInterface) -> Vec<String> {
    interface
        .groups()
        .iter()
        .filter_map(|group| group.group_id().map(str::to_string))
        .collect()
}

/// `CreateFunction` attaching a function of `name` to `subnet_ids` with `group_ids`, carrying
/// the extra request members `extra` names (none, for the rows that are accepted).
async fn create_attached_with(
    router: &MultiServiceRouter,
    name: &str,
    subnet_ids: &[String],
    group_ids: &[String],
    extra: &[(&str, serde_json::Value)],
) -> crate::function_lifecycle::Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        "VpcConfig": { "SubnetIds": subnet_ids, "SecurityGroupIds": group_ids },
    });
    for (member, value) in extra {
        body[*member] = value.clone();
    }
    create_function(router, &body).await
}

/// `CreateFunction` attaching a function of `name` to `subnet_ids` with `group_ids`.
async fn create_attached(
    router: &MultiServiceRouter,
    name: &str,
    subnet_ids: &[String],
    group_ids: &[String],
) -> crate::function_lifecycle::Answer {
    create_attached_with(router, name, subnet_ids, group_ids, &[]).await
}

/// The `VpcConfig` a read of the function's configuration reports.
async fn read_vpc_config(
    router: &MultiServiceRouter,
    name: &str,
) -> serde_json::Value {
    let read = json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "expected 200 from GetFunctionConfiguration for {name}, got {} {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    read.json()
        .get("VpcConfig")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// RULE: a create that attaches the function reports the VPC its subnets belong to and holds
/// one interface per resolvable subnet, carrying the groups it named.
#[tokio::test]
async fn an_attached_function_reports_its_vpc_and_holds_one_interface_per_subnet() {
    let emulator = emulator();
    let one = network(&emulator.ec2, "one subnet", 21, 1).await;
    let two = network(&emulator.ec2, "two subnets", 22, 2).await;
    let (default_subnet_id, default_vpc_id) = default_subnet(&emulator.ec2).await;

    // (case, the subnets the configuration names, the groups it names, the VpcId the
    // configuration must report, the subnets an interface must sit in)
    let rows: Vec<CreateRow> = vec![
        (
            "one subnet of a VPC a client minted, with one group",
            one.subnet_ids.clone(),
            vec![one.group_id.clone()],
            Some(one.vpc_id.clone()),
            one.subnet_ids.iter().cloned().collect(),
        ),
        (
            "two subnets of one VPC, with two groups",
            two.subnet_ids.clone(),
            vec![two.group_id.clone(), one.group_id.clone()],
            Some(two.vpc_id.clone()),
            two.subnet_ids.iter().cloned().collect(),
        ),
        (
            "a subnet of the account's default network, naming no group",
            vec![default_subnet_id.clone()],
            Vec::new(),
            Some(default_vpc_id.clone()),
            [default_subnet_id.clone()].into_iter().collect(),
        ),
        (
            "one subnet named twice: one subnet, so one interface",
            vec![one.subnet_ids[0].clone(), one.subnet_ids[0].clone()],
            vec![one.group_id.clone()],
            Some(one.vpc_id.clone()),
            [one.subnet_ids[0].clone()].into_iter().collect(),
        ),
        (
            "an id of the right form naming no subnet at all",
            vec![UNKNOWN_SUBNET.to_string()],
            vec![one.group_id.clone()],
            None,
            BTreeSet::new(),
        ),
    ];

    for (index, (case, subnet_ids, group_ids, expected_vpc, expected_subnets)) in
        rows.into_iter().enumerate()
    {
        let name = format!("attached-{index}");
        let answer = create_attached(&emulator.router, &name, &subnet_ids, &group_ids).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201 from CreateFunction, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // The round trip: the configuration is read back through GetFunctionConfiguration,
        // so what is asserted is what the create PERSISTED.
        let vpc_config = read_vpc_config(&emulator.router, &name).await;
        assert_eq!(
            vpc_config.get("SubnetIds"),
            Some(&json!(subnet_ids)),
            "case {case:?}: expected the stored SubnetIds {subnet_ids:?}, got {vpc_config:?}"
        );
        assert_eq!(
            vpc_config.get("SecurityGroupIds"),
            Some(&json!(group_ids)),
            "case {case:?}: expected the stored SecurityGroupIds {group_ids:?}, got \
             {vpc_config:?}"
        );
        let reported_vpc = vpc_config
            .get("VpcId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        assert_eq!(
            reported_vpc, expected_vpc,
            "case {case:?}: expected VpcId {expected_vpc:?}, got {reported_vpc:?}"
        );

        // The attachment itself, where a client reads it: the account's EC2 inventory. One
        // interface per DISTINCT subnet the configuration named, so the COUNT is asserted
        // beside the set.
        let interfaces = function_interfaces(&emulator.ec2, &name).await;
        let placed = subnets_of(&interfaces);
        assert_eq!(
            placed, expected_subnets,
            "case {case:?}: expected an interface in each of {expected_subnets:?}, got {placed:?}"
        );
        assert_eq!(
            interfaces.len(),
            expected_subnets.len(),
            "case {case:?}: expected {} interface(s), got {interfaces:?}",
            expected_subnets.len()
        );
        for interface in &interfaces {
            assert_eq!(
                interface.vpc_id().map(str::to_string),
                expected_vpc,
                "case {case:?}: expected the interface in {expected_vpc:?}, got {interface:?}"
            );
            assert_eq!(
                groups_of(interface),
                group_ids,
                "case {case:?}: expected the interface to carry {group_ids:?}, got {interface:?}"
            );
            assert_eq!(
                interface.requester_managed(),
                Some(false),
                "case {case:?}: an interface created in the account by the function's own \
                 execution role is not AWS-managed, got {interface:?}"
            );
            // Who created it: Lambda, on the function's behalf — the service principal a
            // function's execution role lets assume it, and not the caller that applied the
            // configuration nor a session of one function.
            let requester = interface.requester_id();
            assert_eq!(
                requester,
                Some(LAMBDA_SERVICE_PRINCIPAL),
                "case {case:?}: expected RequesterId {LAMBDA_SERVICE_PRINCIPAL:?}, got \
                 {requester:?}"
            );
            assert!(
                !requester.is_some_and(|requester| requester.ends_with(&format!(":{name}"))),
                "case {case:?}: the requester must not be a session of the function itself, \
                 got {requester:?}"
            );
            // And the description names the function, as fixed text around that one name.
            assert_eq!(
                interface.description(),
                Some(format!("AWS Lambda VPC ENI-{name}").as_str()),
                "case {case:?}: expected the description to name the function, got {interface:?}"
            );
            let attachment = interface
                .attachment()
                .unwrap_or_else(|| panic!("case {case:?}: expected an attachment on {interface:?}"));
            assert_eq!(
                (attachment.instance_id(), attachment.instance_owner_id(), attachment.status()),
                (None, Some("amazon-aws"), Some(&aws_sdk_ec2::types::AttachmentStatus::Attached)),
                "case {case:?}: expected the interface attached to the service and to no \
                 instance, got {attachment:?}"
            );
            assert_eq!(
                interface.interface_type(),
                Some(&aws_sdk_ec2::types::NetworkInterfaceType::Lambda),
                "case {case:?}: expected the modelled lambda interface type, got {interface:?}"
            );
            assert!(
                interface
                    .private_ip_address()
                    .is_some_and(|address| !address.is_empty()),
                "case {case:?}: expected the interface to hold an address of its subnet, got \
                 {interface:?}"
            );
        }
    }
}

/// RULE (the other write path): `UpdateFunctionConfiguration` makes the attachment agree with
/// the configuration it installs — a subnet kept keeps its interface, a subnet left gives it
/// back, a subnet newly named gets one of its own, and the reported VpcId follows the subnets.
#[tokio::test]
async fn a_configuration_change_moves_the_attachment_and_the_reported_vpc() {
    let emulator = emulator();
    let first = network(&emulator.ec2, "the first VPC", 31, 2).await;
    let second = network(&emulator.ec2, "the second VPC", 32, 1).await;
    let name = "reattached";

    let answer = create_attached(
        &emulator.router,
        name,
        &first.subnet_ids[..1],
        std::slice::from_ref(&first.group_id),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "the first attachment: expected 201 from CreateFunction, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let placed = function_interfaces(&emulator.ec2, name).await;
    let kept_id = placed
        .first()
        .and_then(NetworkInterface::network_interface_id)
        .expect("the first attachment must place an interface")
        .to_string();
    let kept_address = placed[0]
        .private_ip_address()
        .expect("the placed interface must hold an address")
        .to_string();

    // (case, the configuration the update installs, the VpcId it must report, the subnets an
    // interface must sit in afterwards)
    let rows: Vec<(&str, serde_json::Value, Option<String>, BTreeSet<String>)> = vec![
        (
            "the same subnet, another group: the interface stays, its groups change",
            json!({
                "SubnetIds": [first.subnet_ids[0].clone()],
                "SecurityGroupIds": [second.group_id.clone()],
            }),
            Some(first.vpc_id.clone()),
            [first.subnet_ids[0].clone()].into_iter().collect(),
        ),
        (
            "a second subnet of the same VPC: the kept one stays, the new one is added",
            json!({
                "SubnetIds": first.subnet_ids.clone(),
                "SecurityGroupIds": [first.group_id.clone()],
            }),
            Some(first.vpc_id.clone()),
            first.subnet_ids.iter().cloned().collect(),
        ),
        (
            "a subnet of another VPC: both interfaces are given back for one in it",
            json!({
                "SubnetIds": second.subnet_ids.clone(),
                "SecurityGroupIds": [second.group_id.clone()],
            }),
            Some(second.vpc_id.clone()),
            second.subnet_ids.iter().cloned().collect(),
        ),
        (
            "empty lists, which remove the function's VPC configuration",
            json!({ "SubnetIds": [], "SecurityGroupIds": [] }),
            None,
            BTreeSet::new(),
        ),
    ];

    for (case, vpc_config, expected_vpc, expected_subnets) in rows {
        make_update_due(&emulator.engine);
        let updated = json_call(
            &emulator.router,
            "PUT",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &json!({ "VpcConfig": vpc_config }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200 from UpdateFunctionConfiguration, got {} {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );

        make_update_due(&emulator.engine);
        let read = read_vpc_config(&emulator.router, name).await;
        let reported_vpc = read
            .get("VpcId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        assert_eq!(
            reported_vpc, expected_vpc,
            "case {case:?}: expected VpcId {expected_vpc:?}, got {read:?}"
        );

        let interfaces = function_interfaces(&emulator.ec2, name).await;
        let placed = subnets_of(&interfaces);
        assert_eq!(
            placed, expected_subnets,
            "case {case:?}: expected an interface in each of {expected_subnets:?}, got {placed:?}"
        );
        let expected_groups: Vec<String> = vpc_config
            .get("SecurityGroupIds")
            .and_then(serde_json::Value::as_array)
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|group| group.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for interface in &interfaces {
            assert_eq!(
                groups_of(interface),
                expected_groups,
                "case {case:?}: expected the interface to carry {expected_groups:?}, got \
                 {interface:?}"
            );
        }
        // A subnet the configuration kept keeps the very interface that was there, with the
        // address it already held: the function never stopped being reachable in it.
        if expected_subnets.contains(&first.subnet_ids[0]) {
            let same = interfaces.iter().any(|interface| {
                interface.network_interface_id() == Some(kept_id.as_str())
                    && interface.private_ip_address() == Some(kept_address.as_str())
            });
            assert!(
                same,
                "case {case:?}: expected the kept subnet to keep interface {kept_id} at \
                 {kept_address}, got {interfaces:?}"
            );
        }
    }
}

/// RULE: those interfaces hold a place in their subnet, so the subnet cannot be deleted while
/// the function is attached to it — and deleting the function gives them back, which frees it.
#[tokio::test]
async fn the_subnet_of_an_attached_function_cannot_be_deleted_until_the_function_is() {
    let emulator = emulator();
    let held = network(&emulator.ec2, "the attached VPC", 41, 1).await;
    let name = "holds-its-subnet";

    let answer = create_attached(
        &emulator.router,
        name,
        &held.subnet_ids,
        std::slice::from_ref(&held.group_id),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "expected 201 from CreateFunction, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        function_interfaces(&emulator.ec2, name).await.len(),
        1,
        "the attachment must place one interface in the one subnet it names"
    );

    // Both resources the interface holds a place in refuse to be deleted while it is there,
    // each with EC2's own code for that class.
    let error = emulator
        .ec2
        .delete_subnet()
        .subnet_id(&held.subnet_ids[0])
        .send()
        .await
        .expect_err("the subnet an attached function holds an interface in must not delete");
    assert_eq!(
        error.code(),
        Some("DependencyViolation"),
        "expected DependencyViolation from DeleteSubnet, got {error:?}"
    );
    let error = emulator
        .ec2
        .delete_security_group()
        .group_id(&held.group_id)
        .send()
        .await
        .expect_err("the group an attached function's interface carries must not delete");
    assert_eq!(
        error.code(),
        Some("DependencyViolation"),
        "expected DependencyViolation from DeleteSecurityGroup, got {error:?}"
    );

    // Deleting the function gives the interfaces back, and the subnet is then free.
    let deleted = json_call(
        &emulator.router,
        "DELETE",
        &format!("/2015-03-31/functions/{name}"),
        &serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "expected 204 from DeleteFunction, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    let left = function_interfaces(&emulator.ec2, name).await;
    assert!(
        left.is_empty(),
        "the interfaces of a deleted function must not outlive it, got {left:?}"
    );
    emulator
        .ec2
        .delete_subnet()
        .subnet_id(&held.subnet_ids[0])
        .send()
        .await
        .expect("the subnet must delete once the function that held it is gone");
}

/// RULE (the complement): a create the emulator REFUSES stands nothing up — no interface is
/// left holding a place in a subnet for a function that does not exist.
///
/// The rows differ along WHICH refusal the request earns, each one a different reason reached
/// at a different point of the handler, with a VpcConfig that would otherwise have been
/// attached.
#[tokio::test]
async fn a_refused_create_leaves_no_interface_behind() {
    let emulator = emulator();
    let held = network(&emulator.ec2, "the refused VPC", 51, 2).await;

    // The name of a function that already exists, so one row can conflict with it.
    let taken = "already-created";
    let answer = create_attached(
        &emulator.router,
        taken,
        &held.subnet_ids[..1],
        std::slice::from_ref(&held.group_id),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "the function a later row conflicts with: expected 201, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    // Everything standing after that one create, which no refused row may add to.
    let before = all_lambda_interfaces(&emulator.ec2).await.len();

    // (case, the function name the request uses, the extra members that make it refusable,
    // the status and the error code it must be refused with — each from the model's own
    // binding for that error on this operation)
    let rows: Vec<RefusalRow> = vec![
        (
            "an environment variable naming a key Lambda reserves",
            "refused-environment",
            vec![(
                "Environment",
                json!({ "Variables": { "AWS_REGION": "us-east-1" } }),
            )],
            400,
            "InvalidParameterValueException",
        ),
        (
            "a name the account already holds a function under",
            taken,
            Vec::new(),
            409,
            "ResourceConflictException",
        ),
        (
            "a code-signing configuration ARN naming none of the account's",
            "refused-code-signing",
            vec![(
                "CodeSigningConfigArn",
                json!(format!(
                    "arn:aws:lambda:{REGION}:000000000000:code-signing-config:csc-0123456789abcdef0"
                )),
            )],
            404,
            "CodeSigningConfigNotFoundException",
        ),
        (
            "a layer ARN naming no layer version of this Region",
            "refused-layer",
            vec![(
                "Layers",
                json!([format!("arn:aws:lambda:{REGION}:000000000000:layer:absent:1")]),
            )],
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (case, name, extra, expected_status, expected_code) in rows {
        let answer =
            create_attached_with(&emulator.router, name, &held.subnet_ids, &[], &extra).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status} from CreateFunction, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains(expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            answer.error_code()
        );
        // Nothing was stood up for this request. The conflicting row names the function that
        // already exists, whose own one interface is untouched by the refusal; every other row
        // names a function that does not exist, so it holds none at all.
        let left = function_interfaces(&emulator.ec2, name).await;
        let expected_left = usize::from(name == taken);
        assert_eq!(
            left.len(),
            expected_left,
            "case {case:?}: expected {expected_left} interface(s) under that function's name \
             after the refusal, got {left:?}"
        );
        let after = all_lambda_interfaces(&emulator.ec2).await.len();
        assert_eq!(
            after, before,
            "case {case:?}: the account held {before} function interface(s) before the refused \
             create and {after} after it"
        );
    }

    // And the subnets are still the account's to delete, because nothing is holding them: the
    // one function that WAS created is attached to the first subnet alone.
    emulator
        .ec2
        .delete_subnet()
        .subnet_id(&held.subnet_ids[1])
        .send()
        .await
        .expect("a subnet only refused creates named must still delete");
}
