// SPDX-License-Identifier: CC0-1.0

//! The capacity Amazon EKS provisions for a managed node group really exists in the
//! services that own it, and a Fargate profile is only accepted in private subnets.
//!
//! The RULES under test:
//!
//! * a managed node group RUNS on an Amazon EC2 Auto Scaling group that Amazon EKS
//!   creates for it, and the node group reports that group — "the resources
//!   associated with the node group, such as Auto Scaling groups and security groups
//!   for remote access" (`Nodegroup$resources`) — so the name it reports is the name
//!   Auto Scaling itself answers a describe for, and deleting the node group releases
//!   it;
//! * a Fargate profile may only use PRIVATE subnets ("the subnets must be private
//!   subnets", `CreateFargateProfileRequest$subnets`), and a subnet is public exactly
//!   when the route table that governs it carries a route to an internet gateway, so
//!   a profile naming one is refused 400 `InvalidParameterException` naming that
//!   subnet and no profile is left behind.
//!
//! Both rules span services on one engine — EKS creates the capacity in Auto Scaling
//! and EC2, and reads the network from EC2 — which is why the suite lives here rather
//! than in the EKS crate. The request harness is the test crate's own (`eks_wire`),
//! and the rows differ along how a subnet becomes public (the VPC's main route table,
//! a route table associated with the subnet alone) and along what a node group asks
//! for (sizes, remote access).

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{autoscaling, cluster_body, ec2, eks, role_arn, Answer};
use crate::query_wire::{element, elements, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// How often a clock-owed read is retried, and for how long in all: nothing inside
/// the emulator sleeps for a due instant, so a status is polled rather than waited
/// for, with a generous deadline.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 200;

/// Read a cluster until it is `ACTIVE`, which is the precondition a node group and a
/// Fargate profile both have.
fn wait_for_active_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) {
    for _ in 0..POLLS {
        let read = eks(router, runtime, "GET", &format!("/clusters/{cluster}"), None);
        if read.body["cluster"]["status"] == serde_json::json!("ACTIVE") {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("setup: cluster {cluster:?} never reached ACTIVE");
}

/// The node group `nodegroup` of `cluster`, as Amazon EKS reports it.
fn describe_nodegroup(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
    nodegroup: &str,
) -> Answer {
    eks(
        router,
        runtime,
        "GET",
        &format!("/clusters/{cluster}/node-groups/{nodegroup}"),
        None,
    )
}

#[test]
fn a_managed_node_group_reports_the_auto_scaling_group_that_really_backs_it() {
    std::thread::Builder::new()
        .name("eks-nodegroup-capacity".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The account's own default subnets, read from EC2 rather than invented.
            let defaults = ec2(
                &router,
                &runtime,
                "DescribeSubnets",
                &[
                    ("Filter.1.Name", "default-for-az"),
                    ("Filter.1.Value.1", "true"),
                ],
            );
            assert_eq!(
                defaults.status, 200,
                "setup: DescribeSubnets must succeed, got {defaults:?}"
            );
            let available = elements(&defaults.text, "subnetId");
            assert!(
                available.len() >= 2,
                "setup: the account must have default subnets, got {available:?}"
            );
            let subnets = vec![available[0].clone(), available[1].clone()];

            let cluster = "nodegroup-capacity-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(cluster, &subnets)),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster must be created, got {created:?}"
            );
            wait_for_active_cluster(&router, &runtime, cluster);

            // The rows differ along what the node group asks for: the default sizes,
            // explicit sizes, and remote access (which is what gives the node group a
            // security group of its own beside the Auto Scaling group).
            let rows: Vec<(&str, &str, serde_json::Value, i32, bool)> = vec![
                (
                    "a node group with the default scaling configuration",
                    "ng-defaults",
                    serde_json::json!({}),
                    2,
                    false,
                ),
                (
                    "a node group with explicit sizes",
                    "ng-sized",
                    serde_json::json!({
                        "scalingConfig": { "minSize": 1, "maxSize": 4, "desiredSize": 3 },
                    }),
                    3,
                    false,
                ),
                (
                    "a node group that asks for remote access",
                    "ng-remote",
                    serde_json::json!({ "remoteAccess": { "ec2SshKey": "capacity-key" } }),
                    2,
                    true,
                ),
            ];

            for (case, nodegroup, extra, desired, remote_access) in &rows {
                let mut body = serde_json::json!({
                    "nodegroupName": nodegroup,
                    "nodeRole": role_arn("eksNodeRole"),
                    "subnets": subnets,
                });
                for (key, value) in extra.as_object().expect("an object of overrides") {
                    body[key] = value.clone();
                }
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{cluster}/node-groups"),
                    Some(body),
                );
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the node group must be created, got {answer:?} ({})",
                    answer.message()
                );

                // The create's own answer already reports the capacity it provisioned,
                // and so does the read: the resources live on the stored record.
                let read = describe_nodegroup(&router, &runtime, cluster, nodegroup);
                for (source, reported) in [
                    ("the create", &answer.body["nodegroup"]),
                    ("the describe", &read.body["nodegroup"]),
                ] {
                    let groups = reported["resources"]["autoScalingGroups"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    assert_eq!(
                        groups.len(),
                        1,
                        "case {case:?}: {source} must report the node group's Auto Scaling \
                         group, got {reported}"
                    );
                    let name = groups[0]["name"].as_str().unwrap_or_default();
                    assert!(
                        name.contains(nodegroup),
                        "case {case:?}: {source} must name a group of this node group, got \
                         {name:?}"
                    );

                    // And the remote-access security group exactly when the node group
                    // asked for remote access.
                    let group = reported["resources"]["remoteAccessSecurityGroup"].as_str();
                    assert_eq!(
                        group.is_some(),
                        *remote_access,
                        "case {case:?}: {source} must report a remote-access security group \
                         only for a node group that asked for remote access, got {reported}"
                    );
                }

                // THE BACKING IS REAL: the group the node group names is a group Auto
                // Scaling itself answers for, with the sizes the node group asked for.
                let group_name = read.body["nodegroup"]["resources"]["autoScalingGroups"][0]
                    ["name"]
                    .as_str()
                    .expect("the reported group name")
                    .to_string();
                let described = autoscaling(
                    &router,
                    &runtime,
                    "DescribeAutoScalingGroups",
                    &[("AutoScalingGroupNames.member.1", group_name.as_str())],
                );
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: Auto Scaling must answer for {group_name:?}, got \
                     {described:?}"
                );
                assert_eq!(
                    opt_element(&described.text, "AutoScalingGroupName").as_deref(),
                    Some(group_name.as_str()),
                    "case {case:?}: Auto Scaling must hold the group the node group names, \
                     got {}",
                    described.text
                );
                assert_eq!(
                    element(&described.text, "DesiredCapacity"),
                    desired.to_string(),
                    "case {case:?}: the group must carry the node group's desired size, got \
                     {}",
                    described.text
                );

                // And the security group it reports is a group EC2 really holds.
                if let Some(security_group) =
                    read.body["nodegroup"]["resources"]["remoteAccessSecurityGroup"].as_str()
                {
                    let groups = ec2(
                        &router,
                        &runtime,
                        "DescribeSecurityGroups",
                        &[("GroupId.1", security_group)],
                    );
                    assert_eq!(
                        groups.status, 200,
                        "case {case:?}: EC2 must hold {security_group:?}, got {groups:?}"
                    );
                }

                // DELETING the node group releases that capacity: nothing holds it once
                // the node group is gone.
                let deleted = eks(
                    &router,
                    &runtime,
                    "DELETE",
                    &format!("/clusters/{cluster}/node-groups/{nodegroup}"),
                    None,
                );
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: the node group must be deleted, got {deleted:?}"
                );
                let after = autoscaling(
                    &router,
                    &runtime,
                    "DescribeAutoScalingGroups",
                    &[("AutoScalingGroupNames.member.1", group_name.as_str())],
                );
                assert_eq!(
                    opt_element(&after.text, "AutoScalingGroupName"),
                    None,
                    "case {case:?}: the deleted node group's Auto Scaling group must be \
                     released, got {}",
                    after.text
                );
            }
        })
        .expect("spawn the eks nodegroup-capacity thread")
        .join()
        .expect("the eks nodegroup-capacity thread");
}

#[test]
fn a_fargate_profile_is_accepted_only_in_subnets_with_no_route_to_an_internet_gateway() {
    std::thread::Builder::new()
        .name("eks-fargate-private-subnets".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // A VPC of this suite's own, so the rows decide which of its subnets are
            // public: a subnet is public exactly when the table that governs it reaches
            // an internet gateway.
            let vpc = ec2(
                &router,
                &runtime,
                "CreateVpc",
                &[("CidrBlock", "10.20.0.0/16")],
            );
            assert_eq!(vpc.status, 200, "setup: CreateVpc must succeed, got {vpc:?}");
            let vpc_id = element(&vpc.text, "vpcId");

            let subnet = |cidr: &str| {
                let created = ec2(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[("VpcId", vpc_id.as_str()), ("CidrBlock", cidr)],
                );
                assert_eq!(
                    created.status, 200,
                    "setup: CreateSubnet {cidr} must succeed, got {created:?}"
                );
                element(&created.text, "subnetId")
            };
            // Two private subnets for the cluster itself (a cluster takes public ones
            // too, so these are simply the ones the profile rows build on), one subnet
            // made public by a route table of its own, and one more private subnet.
            let private_one = subnet("10.20.1.0/24");
            let private_two = subnet("10.20.2.0/24");
            let public = subnet("10.20.3.0/24");

            // The internet gateway that makes a subnet public, and the route table that
            // carries the route to it — associated with the public subnet ALONE, so the
            // VPC's main table (which every other subnet uses) reaches nothing.
            let gateway = ec2(&router, &runtime, "CreateInternetGateway", &[]);
            assert_eq!(
                gateway.status, 200,
                "setup: CreateInternetGateway must succeed, got {gateway:?}"
            );
            let gateway_id = element(&gateway.text, "internetGatewayId");
            let attached = ec2(
                &router,
                &runtime,
                "AttachInternetGateway",
                &[
                    ("InternetGatewayId", gateway_id.as_str()),
                    ("VpcId", vpc_id.as_str()),
                ],
            );
            assert_eq!(
                attached.status, 200,
                "setup: AttachInternetGateway must succeed, got {attached:?}"
            );
            let table = ec2(
                &router,
                &runtime,
                "CreateRouteTable",
                &[("VpcId", vpc_id.as_str())],
            );
            assert_eq!(
                table.status, 200,
                "setup: CreateRouteTable must succeed, got {table:?}"
            );
            let table_id = element(&table.text, "routeTableId");
            let route = ec2(
                &router,
                &runtime,
                "CreateRoute",
                &[
                    ("RouteTableId", table_id.as_str()),
                    ("DestinationCidrBlock", "0.0.0.0/0"),
                    ("GatewayId", gateway_id.as_str()),
                ],
            );
            assert_eq!(
                route.status, 200,
                "setup: CreateRoute must succeed, got {route:?}"
            );
            let associated = ec2(
                &router,
                &runtime,
                "AssociateRouteTable",
                &[
                    ("RouteTableId", table_id.as_str()),
                    ("SubnetId", public.as_str()),
                ],
            );
            assert_eq!(
                associated.status, 200,
                "setup: AssociateRouteTable must succeed, got {associated:?}"
            );

            let cluster = "fargate-subnets-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(
                    cluster,
                    &[private_one.clone(), private_two.clone()],
                )),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster must be created, got {created:?} ({})",
                created.message()
            );
            wait_for_active_cluster(&router, &runtime, cluster);

            // (case, the profile's name, the subnets it names, the subnet refused — if
            // any). The rows differ along how many subnets the profile names and where
            // the public one sits in the list.
            let rows: Vec<(&str, &str, Vec<String>, Option<String>)> = vec![
                (
                    "one private subnet",
                    "fp-private",
                    vec![private_one.clone()],
                    None,
                ),
                (
                    "two private subnets",
                    "fp-private-pair",
                    vec![private_one.clone(), private_two.clone()],
                    None,
                ),
                (
                    "a public subnet alone",
                    "fp-public",
                    vec![public.clone()],
                    Some(public.clone()),
                ),
                (
                    "a public subnet after a private one",
                    "fp-mixed",
                    vec![private_one.clone(), public.clone()],
                    Some(public.clone()),
                ),
                (
                    "a public subnet before a private one",
                    "fp-mixed-first",
                    vec![public.clone(), private_two.clone()],
                    Some(public.clone()),
                ),
            ];

            for (case, profile, profile_subnets, refused) in &rows {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{cluster}/fargate-profiles"),
                    Some(serde_json::json!({
                        "fargateProfileName": profile,
                        "podExecutionRoleArn": role_arn("eksFargatePodExecutionRole"),
                        "subnets": profile_subnets,
                        "selectors": [{ "namespace": "default" }],
                    })),
                );
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{cluster}/fargate-profiles/{profile}"),
                    None,
                );
                match refused {
                    None => {
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: a profile in private subnets must be created, got \
                             {answer:?} ({})",
                            answer.message()
                        );
                        assert_eq!(
                            read.status, 200,
                            "case {case:?}: the profile must read back, got {read:?}"
                        );
                        assert_eq!(
                            read.body["fargateProfile"]["subnets"],
                            serde_json::json!(profile_subnets),
                            "case {case:?}: the stored profile must carry its subnets, got {}",
                            read.text
                        );
                    }
                    Some(subnet_id) => {
                        assert_eq!(
                            (answer.status, answer.error_type.as_deref()),
                            (400, Some("InvalidParameterException")),
                            "case {case:?}: a public subnet must be refused 400 \
                             InvalidParameterException, got {answer:?}"
                        );
                        assert_eq!(
                            answer.message(),
                            format!(
                                "Subnet {subnet_id} provided in Fargate Profile is not a \
                                 private subnet"
                            ),
                            "case {case:?}: expected the sentence naming the subnet at fault, \
                             got {answer:?}"
                        );
                        assert_eq!(
                            read.status, 404,
                            "case {case:?}: a refused create must leave no profile, got {read:?}"
                        );
                    }
                }
            }
        })
        .expect("spawn the eks fargate-private-subnets thread")
        .join()
        .expect("the eks fargate-private-subnets thread");
}
