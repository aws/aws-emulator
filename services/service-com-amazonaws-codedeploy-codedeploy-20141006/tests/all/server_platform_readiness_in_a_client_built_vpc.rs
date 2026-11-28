// SPDX-License-Identifier: CC0-1.0

//! The fleet-readiness gate a client puts in FRONT of a `Server` deployment, driven
//! on a host launched into a VPC THE CLIENT CREATED rather than into the Region's
//! default one.
//!
//! RULE under test: what makes a launched instance a host a `Server` deployment can
//! reach is the instance itself — running, carrying an instance profile whose role
//! may register a managed node, with security groups that let its agent call out —
//! and NOT which VPC or subnet it was launched into. So a client that builds its own
//! VPC, public subnet, internet gateway and route, launches one tagged instance into
//! that subnet with an SSM-capable instance profile, and then asks the three
//! questions a readiness gate asks, gets the same answers as one launching into the
//! default VPC:
//!
//! 1. Amazon EC2 reports the instance `running`;
//! 2. Systems Manager reports it in `DescribeInstanceInformation` with `PingStatus`
//!    `Online` — the registration its own agent made;
//! 3. Run Command delivers a shell command to the agent in its guest and the
//!    invocation reaches `Success`, so the init system's answer about the CodeDeploy
//!    agent's unit (`systemctl is-active codedeploy-agent`) is readable there.
//!
//! The sibling suites assert the same three answers for a host of the DEFAULT VPC
//! (`server_platform_rollback_read_through_run_command.rs`,
//! `server_platform_ready_fleet_paced_by_minimum_healthy_hosts.rs`); this one differs
//! in exactly one dimension — where the host sits — because a readiness gate that
//! answers only for the default VPC leaves a client who built its own network polling
//! for ever instead of deploying.
//!
//! Nothing here waits a fixed time: every span is a stored due instant, so each step
//! looks again until the condition holds or a generous deadline passes, and nothing
//! asserts on elapsed time.
//!
//! The second case drives what such a host is FOR: a deployment group that rolls back
//! on a deployment failure, and the whole record an automatic rollback leaves — the
//! rollback deployment's own id on the failed one, the failed one's id on the
//! rollback, the creator CodeDeploy gives a deployment it created itself, the revision
//! the rollback redeploys, that rollback's instance target's `Install` event, and the
//! bytes back on the host — with the failure coming from the LAST hook of the
//! sequence (`ValidateService`), where everything the revision installed has already
//! succeeded.

use serde_json::{json, Value};

use crate::server_platform_instance_deployments::{app_spec, bundle, element};
use crate::server_platform_rollback_read_through_run_command::{
    run_command, trust_policy, when_managed, when_running, when_terminal, Estate, AMI,
    EC2_API_VERSION, EC2_PRINCIPAL, IAM_API_VERSION, SSM_CORE_POLICY,
};

/// The instance profile the host runs as, and the role it carries: a managed node
/// registers as the role its profile names, which is what lets a command be
/// delivered to the agent in its guest at all.
const HOST_PROFILE: &str = "customer-vpc-host-profile";
/// See [`HOST_PROFILE`] — deliberately not the profile's own name.
const HOST_ROLE: &str = "customer-vpc-host-role";

/// The program the host's guest runs when it comes up: the documented installation
/// of the CodeDeploy agent, which is what makes the readiness answer below `active`.
const USER_DATA: &str = "#!/bin/bash\ndnf install -y codedeploy-agent\n\
                         systemctl enable --now codedeploy-agent\n";

/// The agent whose unit the readiness gate asks about, and the name its init system
/// knows the unit by.
const CODEDEPLOY_AGENT: &str = "codedeploy-agent";

/// The tag the host carries, the one a deployment group would select it by.
const FLEET_TAG: (&str, &str) = ("Role", "customer-vpc-fleet");

/// The network the host is launched into, which this client builds itself.
const VPC_CIDR: &str = "10.42.0.0/16";
/// See [`VPC_CIDR`]: the public subnet inside it.
const SUBNET_CIDR: &str = "10.42.1.0/24";
/// Where the subnet's route table sends everything that is not local.
const DEFAULT_ROUTE: &str = "0.0.0.0/0";

/// RULE: the three readiness answers hold for a host of a VPC the client created.
/// Build the network, the instance profile and the host, and wait until the host is
/// a reachable managed node — the readiness gate a client runs before deploying —
/// answering the instance id.
async fn given_ready_host(estate: &Estate, case: &str) -> String {
    // The network: a VPC, one public subnet in it, an internet gateway attached to
    // it, and a route table that sends the subnet's egress through that gateway.
    let vpc = element(
        &estate
            .form(
                case,
                "ec2",
                "CreateVpc",
                EC2_API_VERSION,
                &[("CidrBlock", VPC_CIDR)],
            )
            .await,
        "vpcId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateVpc reports a vpcId"));
    let subnet = element(
        &estate
            .form(
                case,
                "ec2",
                "CreateSubnet",
                EC2_API_VERSION,
                &[("VpcId", &vpc), ("CidrBlock", SUBNET_CIDR)],
            )
            .await,
        "subnetId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateSubnet reports a subnetId"));
    let gateway = element(
        &estate
            .form(case, "ec2", "CreateInternetGateway", EC2_API_VERSION, &[])
            .await,
        "internetGatewayId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateInternetGateway reports an id"));
    estate
        .form(
            case,
            "ec2",
            "AttachInternetGateway",
            EC2_API_VERSION,
            &[("InternetGatewayId", &gateway), ("VpcId", &vpc)],
        )
        .await;
    let route_table = element(
        &estate
            .form(
                case,
                "ec2",
                "CreateRouteTable",
                EC2_API_VERSION,
                &[("VpcId", &vpc)],
            )
            .await,
        "routeTableId",
    )
    .unwrap_or_else(|| panic!("case {case:?}: CreateRouteTable reports an id"));
    estate
        .form(
            case,
            "ec2",
            "CreateRoute",
            EC2_API_VERSION,
            &[
                ("RouteTableId", &route_table),
                ("DestinationCidrBlock", DEFAULT_ROUTE),
                ("GatewayId", &gateway),
            ],
        )
        .await;
    estate
        .form(
            case,
            "ec2",
            "AssociateRouteTable",
            EC2_API_VERSION,
            &[("RouteTableId", &route_table), ("SubnetId", &subnet)],
        )
        .await;

    // The identity the host's agent registers as: a role Amazon EC2 may act as,
    // carrying the managed policy that lets a node register, in an instance profile.
    let trust = trust_policy(EC2_PRINCIPAL);
    for (action, params) in [
        (
            "CreateRole",
            vec![
                ("RoleName", HOST_ROLE),
                ("AssumeRolePolicyDocument", &trust[..]),
            ],
        ),
        (
            "AttachRolePolicy",
            vec![("RoleName", HOST_ROLE), ("PolicyArn", SSM_CORE_POLICY)],
        ),
        (
            "CreateInstanceProfile",
            vec![("InstanceProfileName", HOST_PROFILE)],
        ),
        (
            "AddRoleToInstanceProfile",
            vec![
                ("InstanceProfileName", HOST_PROFILE),
                ("RoleName", HOST_ROLE),
            ],
        ),
    ] {
        estate
            .form(case, "iam", action, IAM_API_VERSION, &params)
            .await;
    }

    // The host: one tagged instance in that subnet, running as that profile, whose
    // user data installs the agent. "The user data to make available to the
    // instance" travels base64 encoded on the wire.
    let user_data = ::aws_smithy_types::base64::encode(USER_DATA);
    let launched = estate
        .form(
            case,
            "ec2",
            "RunInstances",
            EC2_API_VERSION,
            &[
                ("ImageId", AMI),
                ("InstanceType", "t3.micro"),
                ("MinCount", "1"),
                ("MaxCount", "1"),
                ("SubnetId", &subnet),
                ("IamInstanceProfile.Name", HOST_PROFILE),
                ("UserData", &user_data),
                ("TagSpecification.1.ResourceType", "instance"),
                ("TagSpecification.1.Tag.1.Key", FLEET_TAG.0),
                ("TagSpecification.1.Tag.1.Value", FLEET_TAG.1),
            ],
        )
        .await;
    let host = element(&launched, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reports an id: {launched}"));

    // The instance runs, and Systems Manager reports the node its own agent
    // registered.
    when_running(estate, case, &host).await;
    when_managed(estate, case, &host).await;
    host
}

/// RULE: the three readiness answers hold for a host of a VPC the client created.
#[tokio::test]
async fn a_host_of_a_client_built_vpc_becomes_a_reachable_managed_node() {
    let estate = Estate::new();
    let case = "a host launched into a VPC, subnet and route the client created";
    let host = given_ready_host(&estate, case).await;

    // ANSWER 1 and 2: the node the instance registered is the instance, reported
    // Online as the role its profile carries.
    let reported = estate
        .ssm(
            case,
            "DescribeInstanceInformation",
            json!({"Filters": [{"Key": "InstanceIds", "Values": [host]}]}),
        )
        .await;
    let node = reported["InstanceInformationList"][0].clone();
    assert_eq!(
        (
            node["InstanceId"].as_str(),
            node["PingStatus"].as_str(),
            node["IamRole"].as_str(),
        ),
        (Some(host.as_str()), Some("Online"), Some(HOST_ROLE)),
        "case {case:?}: expected the launched instance to be reported Online as the role its \
         profile carries, got {reported:?}"
    );

    // ANSWER 3: a command reaches the agent in that guest, and the init system
    // answers for the unit the user data enabled.
    let invocation = run_command(
        &estate,
        case,
        &host,
        &format!("systemctl is-active {CODEDEPLOY_AGENT}"),
    )
    .await;
    assert_eq!(
        (
            invocation["Status"].as_str(),
            invocation["StandardOutputContent"]
                .as_str()
                .map(str::trim)
                .unwrap_or_default(),
        ),
        (Some("Success"), "active"),
        "case {case:?}: expected the agent's unit to read active through Run Command, got \
         {invocation:?}"
    );
}

/// The bucket both revisions of the rollback case are uploaded to.
const BUCKET: &str = "client-vpc-revisions";

/// Where both revisions install their payload on the host.
const DESTINATION: &str = "/opt/client-vpc";

/// What the first revision's payload carries, and what the rollback has to put back.
const MARKER_A: &str = "revision-a-payload";
/// What the failing revision's payload carries.
const MARKER_B: &str = "revision-b-payload";

/// The revision bundle of one leg: a `files` section copying its payload to
/// [`DESTINATION`], and — for the leg that must FAIL — a `ValidateService` hook that
/// exits non-zero, the LAST event of the sequence, so everything the revision
/// installed has already succeeded when the deployment fails.
fn revision(marker: &str, failing: bool) -> Vec<u8> {
    let hooks: &[(&str, &str)] = if failing {
        &[("ValidateService", "scripts/validate.sh")]
    } else {
        &[]
    };
    let document = app_spec(&[("payload.txt", DESTINATION)], hooks);
    let mut entries: Vec<(&str, &str)> = vec![
        ("appspec.yml", document.as_str()),
        ("payload.txt", marker),
    ];
    if failing {
        entries.push(("scripts/validate.sh", "#!/bin/bash\nexit 1\n"));
    }
    bundle(&entries)
}

/// The `revision` member a deployment of `key` carries.
fn s3_revision(key: &str) -> Value {
    json!({
        "revisionType": "S3",
        "s3Location": {"bucket": BUCKET, "key": key, "bundleType": "zip"},
    })
}

/// What `cat` of the installed payload reports on the host.
async fn installed_payload(estate: &Estate, case: &str, host: &str) -> String {
    let invocation = run_command(estate, case, host, &format!("cat {DESTINATION}/payload.txt")).await;
    assert_eq!(
        invocation["Status"].as_str(),
        Some("Success"),
        "case {case:?}: reading the installed payload off {host} must succeed, got {invocation:?}"
    );
    invocation["StandardOutputContent"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// RULE: a deployment of a group whose `autoRollbackConfiguration` names
/// DEPLOYMENT_FAILURE, failing its last hook on the host, is followed by a SEPARATE
/// rollback deployment — the failed deployment reports its id, the rollback reports
/// the failed one's, the rollback's creator is the one CodeDeploy gives a deployment
/// it created itself, its revision is the last one that succeeded, it succeeds, its
/// instance target's `Install` event succeeds, and the bytes of that revision are
/// back on the host.
#[tokio::test]
async fn a_deployment_failure_rolls_the_host_back_to_the_last_good_revision() {
    let estate = Estate::new();
    let case = "a DEPLOYMENT_FAILURE rollback on a host of a client-built VPC";
    let host = given_ready_host(&estate, case).await;

    // The two revisions, each under its own key: the rollback redeploys the first
    // one's object, so the second cannot replace it.
    estate
        .upload(case, BUCKET, "revision-a.zip", &revision(MARKER_A, false))
        .await;
    estate
        .upload(case, BUCKET, "revision-b.zip", &revision(MARKER_B, true))
        .await;

    estate
        .codedeploy(
            case,
            "CreateApplication",
            json!({"applicationName": "client-vpc", "computePlatform": "Server"}),
        )
        .await;
    estate
        .codedeploy(
            case,
            "CreateDeploymentGroup",
            json!({
                "applicationName": "client-vpc",
                "deploymentGroupName": "fleet",
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                "deploymentConfigName": "CodeDeployDefault.AllAtOnce",
                "ec2TagFilters": [
                    {"Key": FLEET_TAG.0, "Value": FLEET_TAG.1, "Type": "KEY_AND_VALUE"},
                ],
                "autoRollbackConfiguration": {"enabled": true, "events": ["DEPLOYMENT_FAILURE"]},
            }),
        )
        .await;

    // The positive control: the first revision deploys and its payload is on the
    // host.
    let first = estate
        .codedeploy(
            case,
            "CreateDeployment",
            json!({
                "applicationName": "client-vpc",
                "deploymentGroupName": "fleet",
                "revision": s3_revision("revision-a.zip"),
                "fileExistsBehavior": "OVERWRITE",
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let info = when_terminal(&estate, case, &first).await;
    assert_eq!(
        info["status"].as_str(),
        Some("Succeeded"),
        "case {case:?}: the first deployment must succeed, got {info:?}"
    );
    let read = installed_payload(&estate, case, &host).await;
    assert_eq!(
        read, MARKER_A,
        "case {case:?}: expected the first revision's payload on the host, got {read:?}"
    );

    // The failing leg, and the record its rollback leaves.
    let second = estate
        .codedeploy(
            case,
            "CreateDeployment",
            json!({
                "applicationName": "client-vpc",
                "deploymentGroupName": "fleet",
                "revision": s3_revision("revision-b.zip"),
                "fileExistsBehavior": "OVERWRITE",
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let failed = when_terminal(&estate, case, &second).await;
    assert_eq!(
        failed["status"].as_str(),
        Some("Failed"),
        "case {case:?}: the second deployment must fail on its host, got {failed:?}"
    );
    let rollback = failed["rollbackInfo"]["rollbackDeploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a rollback deployment must be named: {failed:?}"))
        .to_string();
    assert_ne!(
        rollback, second,
        "case {case:?}: the rollback must be a deployment of its own, got {failed:?}"
    );

    let rolled_back = when_terminal(&estate, case, &rollback).await;
    assert_eq!(
        (
            rolled_back["status"].as_str(),
            rolled_back["creator"].as_str(),
            rolled_back["rollbackInfo"]["rollbackTriggeringDeploymentId"].as_str(),
            rolled_back["revision"]["s3Location"]["key"].as_str(),
        ),
        (
            Some("Succeeded"),
            Some("codeDeployRollback"),
            Some(second.as_str()),
            Some("revision-a.zip"),
        ),
        "case {case:?}: expected the rollback to succeed as a service-created deployment of the \
         last good revision, naming the deployment it rolled back, got {rolled_back:?}"
    );

    // The rollback's own instance target: the revision was installed on the host
    // again, which is the event a client reads it off.
    let target = estate
        .codedeploy(
            case,
            "GetDeploymentTarget",
            json!({"deploymentId": rollback, "targetId": host}),
        )
        .await["deploymentTarget"]
        .clone();
    let install = target["instanceTarget"]["lifecycleEvents"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|event| event["lifecycleEventName"].as_str() == Some("Install"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the rollback's target reports Install: {target:?}"));
    assert_eq!(
        install["status"].as_str(),
        Some("Succeeded"),
        "case {case:?}: expected the rollback's Install event to succeed, got {target:?}"
    );

    // And the host itself: the last good revision's bytes are back.
    let read = installed_payload(&estate, case, &host).await;
    assert_eq!(
        read, MARKER_A,
        "case {case:?}: expected the rollback to put the last good revision's payload back on \
         the host, got {read:?}"
    );
}
