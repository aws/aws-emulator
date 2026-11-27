// SPDX-License-Identifier: CC0-1.0

//! An Amazon EC2 instance launched into an Amazon ECS cluster becomes an `ACTIVE`,
//! agent-connected container instance of it on the emulator a client STARTS: the built binary,
//! on its own port, with request authentication on, reached over TLS with SigV4-signed calls.
//!
//! The RULE under test: **a RUNNING instance of the account whose user data configures the
//! container agent with a cluster of the account is a registered container instance of that
//! cluster — reported by `ListContainerInstances` and `DescribeContainerInstances` with its own
//! `ec2InstanceId`, `status` `ACTIVE` and `agentConnected` true — and everything Amazon ECS
//! defines over a registered instance then resolves against it: a `distinctInstance` placement
//! caps a run at one task per host, `DeregisterContainerInstance` without `force` refuses a host
//! that still runs a task, and `DiscoverPollEndpoint` hands that host's agent its endpoints.**
//!
//! The service crate's own suites pin each half in process, with authentication off, on a
//! router the test assembles. The emulator a client runs is a different configuration of the
//! same code: the credentials are verified, each call arrives over its own TLS connection, the
//! registration the clock owes is materialised on the binary's own multi-thread runtime, and one
//! process serves AWS IAM, AWS Systems Manager, Amazon EC2 and Amazon ECS together. This suite
//! is the one that pins the plane THERE, so a host that registers in process and not on the
//! started emulator is a failure a test reports rather than one only a client sees.
//!
//! The rows span HOW the client addresses that emulator, because that is what decides how a
//! call is routed and which Region each service reads it for: a client addressing the AWS
//! service host names the service and the Region in its `Host`, while a client configured with
//! the emulator's endpoint url carries the emulator's own address there and nothing else, so
//! every service has to read the same Region out of the credential scope alone. One estate is
//! built per row, on an emulator of its own.
//!
//! Every wait here ends on the CONDITION a client polls for — the hosts registering, the tasks
//! reaching `RUNNING` — never on a span of the clock; the deadlines are only there so a
//! convergence that never happens fails the case instead of hanging it.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::Started;
use crate::query_wire::{element, elements};

/// The Region the started emulator's calls are signed for. It must stay the one [`Started`]
/// signs with.
const REGION: &str = "us-east-1";

/// The cluster the hosts of a row join.
const CLUSTER: &str = "startedec2fleet";

/// The family the tasks placed below run.
const FAMILY: &str = "startedec2fleet-family";

/// The instance role and the profile a launch carries it in, as the documented ECS-on-EC2 setup
/// names them.
const INSTANCE_ROLE: &str = "startedEcsInstanceRole";
const INSTANCE_PROFILE: &str = "startedEcsInstanceProfile";

/// The managed policy AWS publishes for an ECS container instance's role.
const INSTANCE_ROLE_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role";

/// The trust policy an instance role carries: Amazon EC2 is what assumes it.
const EC2_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
 "Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The instance type every host is launched as.
const INSTANCE_TYPE: &str = "t3.micro";

/// The public parameter AWS publishes the ECS-optimized AMI's id under, which is where a launch
/// reads the image the container agent ships in.
const ECS_OPTIMIZED_AMI_PARAMETER: &str =
    "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id";

/// The statuses asserted, spelled out here rather than imported from the service crate, so a
/// wrong constant in the implementation cannot make this suite pass.
const ACTIVE: &str = "ACTIVE";
const RUNNING: &str = "RUNNING";

/// The resources a registration reports for a host, which a placement is made against
/// (`ContainerInstance$registeredResources`).
const REPORTED_RESOURCES: [&str; 3] = ["CPU", "MEMORY", "PORTS"];

/// The `PlacementConstraintType` the run below carries.
const DISTINCT_INSTANCE: &str = "distinctInstance";

/// The fixed sentence a `Failure` of a host a `distinctInstance` constraint rejected carries.
const DISTINCT_INSTANCE_UNSATISFIED: &str = "DistinctInstance placement constraint unsatisfied.";

/// The fixed sentence a `DeregisterContainerInstance` without `force` is refused with while the
/// host still runs a task.
const RUNNING_TASKS_ON_INSTANCE: &str = "Found running tasks on the instance.";

/// The fixed sentence a container-instance identifier of neither length an id has is refused
/// with, and the one the agent plane answers when it cannot locate what the call is about.
const INSTANCE_ID_LENGTH_MESSAGE: &str = "instanceId length should be one of [32,36]";
const RESOURCES_NOT_LOCATED_MESSAGE: &str = "Could not locate resources";

/// The `startedBy` tag the placement below carries, which the listing is narrowed to.
const STARTED_BY: &str = "distinct-run";

/// How long a wait keeps looking, and how long it leaves between looks. Generous on purpose:
/// every wait is on a CONDITION, looked for again until it holds.
const DEADLINE: Duration = Duration::from_secs(90);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// How a client addresses the emulator — the two positions a client is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressing {
    /// The AWS service host, which names the service and the Region.
    ServiceHost,
    /// The emulator's own endpoint url, which names neither.
    Endpoint,
}

/// The rows this suite spans: one estate per client position.
const ADDRESSINGS: [Addressing; 2] = [Addressing::ServiceHost, Addressing::Endpoint];

/// One signed call, made the way `addressing` says a client makes it, answered with its status
/// and body text.
fn send(
    emulator: &Started,
    addressing: Addressing,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, String) {
    match addressing {
        Addressing::ServiceHost => emulator.call(signing_name, "POST", "/", headers, body),
        Addressing::Endpoint => emulator
            .call_at(
                &emulator.authority(),
                signing_name,
                "POST",
                "/",
                headers,
                body,
            )
            .unwrap_or_else(|error| panic!("the emulator answers: {error}")),
    }
}

/// One `X-Amz-Target` RPC call, answered with its status and parsed body.
fn rpc(
    emulator: &Started,
    addressing: Addressing,
    signing_name: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, text) = send(
        emulator,
        addressing,
        signing_name,
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        body.to_string().into_bytes(),
    );
    let parsed: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "__unparsed": text.clone() }));
    (status, parsed)
}

/// One Amazon ECS call, answered with its status and parsed body.
fn ecs_call(
    emulator: &Started,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        emulator,
        addressing,
        "ecs",
        &format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        body,
    )
}

/// One Amazon ECS call that must answer 200.
fn ecs(
    emulator: &Started,
    addressing: Addressing,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = ecs_call(emulator, addressing, operation, body);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon ECS {operation}, got {status} with \
         {parsed}; the log:\n{}",
        emulator.log()
    );
    parsed
}

/// One AWS Systems Manager call that must answer 200.
fn ssm(
    emulator: &Started,
    addressing: Addressing,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = rpc(
        emulator,
        addressing,
        "ssm",
        &format!("AmazonSSM.{operation}"),
        body,
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AWS Systems Manager {operation}, got {status} \
         with {parsed}; the log:\n{}",
        emulator.log()
    );
    parsed
}

/// One form-wire (`awsQuery` / `ec2Query`) call that must answer 200, as its XML body.
fn form(
    emulator: &Started,
    addressing: Addressing,
    case: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(name));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let (status, text) = send(
        emulator,
        addressing,
        signing_name,
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {action}, got {status} with \
         {text}; the log:\n{}",
        emulator.log()
    );
    text
}

/// One Amazon EC2 call that must answer 200.
fn ec2(
    emulator: &Started,
    addressing: Addressing,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    form(
        emulator,
        addressing,
        case,
        "ec2",
        "2016-11-15",
        action,
        params,
    )
}

/// One AWS IAM call that must answer 200.
fn iam(
    emulator: &Started,
    addressing: Addressing,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    form(
        emulator,
        addressing,
        case,
        "iam",
        "2010-05-08",
        action,
        params,
    )
}

/// A string member of a JSON answer, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// The user data a launch carries: the documented shell line that appends the container agent's
/// `ECS_CLUSTER` assignment to its configuration file, base64 as a launch carries it ("user data
/// must be base64-encoded", `RunInstancesRequest$UserData`).
fn launched_into(cluster: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "#!/bin/bash\necho ECS_CLUSTER={cluster} >> /etc/ecs/ecs.config\n"
    ))
}

/// The container instances of the cluster, as a client reads them: `ListContainerInstances`,
/// then `DescribeContainerInstances` over what it answered.
fn described_hosts(emulator: &Started, addressing: Addressing, case: &str) -> Vec<Value> {
    let listed = ecs(
        emulator,
        addressing,
        case,
        "ListContainerInstances",
        &json!({ "cluster": CLUSTER }),
    );
    let arns: Vec<Value> = listed["containerInstanceArns"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ecs(
        emulator,
        addressing,
        case,
        "DescribeContainerInstances",
        &json!({ "cluster": CLUSTER, "containerInstances": arns }),
    );
    described["containerInstances"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Build the estate ONE row drives: the cluster, the instance profile, the network, the image
/// and two hosts launched into the cluster, answering the launched instance ids.
fn estate(emulator: &Started, addressing: Addressing, case: &str) -> Vec<String> {
    ecs(
        emulator,
        addressing,
        case,
        "CreateCluster",
        &json!({ "clusterName": CLUSTER }),
    );

    // The instance role and the profile the launch carries it in, through AWS IAM's own API.
    iam(
        emulator,
        addressing,
        case,
        "CreateRole",
        &[
            ("RoleName", INSTANCE_ROLE),
            ("AssumeRolePolicyDocument", EC2_TRUST),
        ],
    );
    iam(
        emulator,
        addressing,
        case,
        "AttachRolePolicy",
        &[
            ("RoleName", INSTANCE_ROLE),
            ("PolicyArn", INSTANCE_ROLE_POLICY),
        ],
    );
    let profile_arn = element(
        &iam(
            emulator,
            addressing,
            case,
            "CreateInstanceProfile",
            &[("InstanceProfileName", INSTANCE_PROFILE)],
        ),
        "Arn",
    );
    iam(
        emulator,
        addressing,
        case,
        "AddRoleToInstanceProfile",
        &[
            ("InstanceProfileName", INSTANCE_PROFILE),
            ("RoleName", INSTANCE_ROLE),
        ],
    );

    // The account's DEFAULT VPC, a subnet of it and its default security group: the network a
    // client that picked none of its own launches a host into.
    let vpc = element(
        &ec2(
            emulator,
            addressing,
            case,
            "DescribeVpcs",
            &[("Filter.1.Name", "isDefault"), ("Filter.1.Value.1", "true")],
        ),
        "vpcId",
    );
    let subnets = elements(
        &ec2(
            emulator,
            addressing,
            case,
            "DescribeSubnets",
            &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc)],
        ),
        "subnetId",
    );
    let subnet = subnets.first().cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected the default VPC to hold a subnet, got {subnets:?}")
    });
    let group = element(
        &ec2(
            emulator,
            addressing,
            case,
            "DescribeSecurityGroups",
            &[
                ("Filter.1.Name", "vpc-id"),
                ("Filter.1.Value.1", &vpc),
                ("Filter.2.Name", "group-name"),
                ("Filter.2.Value.1", "default"),
            ],
        ),
        "groupId",
    );

    // The image, resolved the way a client resolves the ECS-optimized AMI.
    let resolved = ssm(
        emulator,
        addressing,
        case,
        "GetParameter",
        &json!({ "Name": ECS_OPTIMIZED_AMI_PARAMETER }),
    );
    let image = member(case, &resolved, "/Parameter/Value");

    // Two hosts, each launched into the cluster the documented way. One launch asks for both,
    // which is what a client that wants a two-host estate sends.
    let user_data = launched_into(CLUSTER);
    let launched = elements(
        &ec2(
            emulator,
            addressing,
            case,
            "RunInstances",
            &[
                ("ImageId", &image),
                ("InstanceType", INSTANCE_TYPE),
                ("SubnetId", &subnet),
                ("SecurityGroupId.1", &group),
                ("IamInstanceProfile.Arn", &profile_arn),
                ("UserData", &user_data),
                ("MinCount", "2"),
                ("MaxCount", "2"),
            ],
        ),
        "instanceId",
    );
    assert_eq!(
        launched.len(),
        2,
        "case {case:?}: expected the launch to mint two hosts, got {launched:?}"
    );
    launched
}

/// RULE: on the emulator a client starts, an instance launched into the cluster registers as an
/// `ACTIVE`, agent-connected container instance, and the placement, the deregistration and the
/// agent's endpoint discovery all resolve against it — however the client addresses that
/// emulator.
#[test]
fn an_ec2_launch_into_a_cluster_serves_the_whole_agent_plane_on_a_started_emulator() {
    for addressing in ADDRESSINGS {
        let case = format!("the ECS-on-EC2 estate, addressed by {addressing:?}");
        let case = case.as_str();
        let emulator = Started::start();
        let launched = estate(&emulator, addressing, case);

        // The registration the clock owes: both launched instances, `ACTIVE` with their agent
        // connected, each reporting its own instance id.
        let registered = |hosts: &[Value]| -> bool {
            launched.iter().all(|id| {
                hosts.iter().any(|host| {
                    host["ec2InstanceId"].as_str() == Some(id.as_str())
                        && host["status"].as_str() == Some(ACTIVE)
                        && host["agentConnected"].as_bool() == Some(true)
                })
            })
        };
        let giving_up_at = Instant::now() + DEADLINE;
        let mut hosts = described_hosts(&emulator, addressing, case);
        while !registered(&hosts) && Instant::now() < giving_up_at {
            std::thread::sleep(LOOK_INTERVAL);
            hosts = described_hosts(&emulator, addressing, case);
        }
        assert!(
            registered(&hosts),
            "case {case:?}: expected {launched:?} to register as {ACTIVE}, agent-connected \
             container instances, got {hosts:?}; the log:\n{}",
            emulator.log()
        );
        let host_arns: Vec<String> = hosts
            .iter()
            .filter_map(|host| host["containerInstanceArn"].as_str().map(str::to_string))
            .collect();
        assert!(
            host_arns
                .iter()
                .all(|arn| arn.contains(&format!(":{REGION}:"))),
            "case {case:?}: expected every container-instance ARN to name the Region the calls \
             are signed for ({REGION}), got {host_arns:?}"
        );

        // The resources the registration reported, which a placement is made against.
        for host in &hosts {
            let names: BTreeSet<String> = host["registeredResources"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|resource| resource["name"].as_str().map(str::to_string))
                .collect();
            let expected: BTreeSet<String> =
                REPORTED_RESOURCES.into_iter().map(str::to_string).collect();
            assert!(
                expected.is_subset(&names),
                "case {case:?}: expected the registration to report {expected:?}, got {names:?} \
                 in {host}"
            );
        }

        // One revision the EC2 launch type runs.
        let revision = ecs(
            &emulator,
            addressing,
            case,
            "RegisterTaskDefinition",
            &json!({
                "family": FAMILY,
                "networkMode": "bridge",
                "containerDefinitions": [{
                    "name": "app",
                    "image": "busybox",
                    "cpu": 64,
                    "memory": 64,
                    "command": ["sh", "-c", "sleep infinity"],
                    "essential": true,
                }],
            }),
        );
        let definition = member(case, &revision["taskDefinition"], "/taskDefinitionArn");

        // A `distinctInstance` run of three tasks over two hosts: two placed on two different
        // hosts, and one failure per host the constraint then rejects.
        let answered = ecs(
            &emulator,
            addressing,
            case,
            "RunTask",
            &json!({
                "cluster": CLUSTER,
                "taskDefinition": &definition,
                "count": 3,
                "launchType": "EC2",
                "startedBy": STARTED_BY,
                "placementConstraints": [{ "type": DISTINCT_INSTANCE }],
            }),
        );
        let placed = answered["tasks"].as_array().cloned().unwrap_or_default();
        let on: BTreeSet<String> = placed
            .iter()
            .filter_map(|task| task["containerInstanceArn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            (placed.len(), on.len()),
            (2, 2),
            "case {case:?}: expected two tasks on two distinct hosts, got {answered}"
        );
        let failures = answered["failures"].as_array().cloned().unwrap_or_default();
        let reasons: Vec<&str> = failures
            .iter()
            .filter_map(|failure| failure["reason"].as_str())
            .collect();
        assert!(
            !reasons.is_empty()
                && reasons
                    .iter()
                    .all(|reason| *reason == DISTINCT_INSTANCE_UNSATISFIED),
            "case {case:?}: expected every failure to carry {DISTINCT_INSTANCE_UNSATISFIED:?}, \
             got {failures:?}"
        );
        for failure in &failures {
            let arn = failure["arn"].as_str().unwrap_or_default().to_string();
            assert!(
                host_arns.contains(&arn),
                "case {case:?}: expected every failure to name a host of the cluster \
                 {host_arns:?}, got {failure}"
            );
        }

        // The tasks a client then lists, which reach RUNNING on the hosts they were placed on.
        let listed = ecs(
            &emulator,
            addressing,
            case,
            "ListTasks",
            &json!({ "cluster": CLUSTER, "startedBy": STARTED_BY }),
        );
        let task_arns: Vec<Value> = listed["taskArns"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            task_arns.len(),
            2,
            "case {case:?}: expected the listing to report the two placed tasks, got {listed}"
        );
        let running = |tasks: &[Value]| -> bool {
            tasks.len() == 2
                && tasks
                    .iter()
                    .all(|task| task["lastStatus"].as_str() == Some(RUNNING))
        };
        let giving_up_at = Instant::now() + DEADLINE;
        let mut described;
        loop {
            described = ecs(
                &emulator,
                addressing,
                case,
                "DescribeTasks",
                &json!({ "cluster": CLUSTER, "tasks": &task_arns }),
            )["tasks"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if running(&described) || Instant::now() >= giving_up_at {
                break;
            }
            std::thread::sleep(LOOK_INTERVAL);
        }
        assert!(
            running(&described),
            "case {case:?}: expected both placed tasks to reach {RUNNING}, got {described:?}; \
             the log:\n{}",
            emulator.log()
        );

        // DeregisterContainerInstance without `force` on a host that still runs a task:
        // refused, and the host is left where it was.
        let busy = described[0]["containerInstanceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, refused) = ecs_call(
            &emulator,
            addressing,
            "DeregisterContainerInstance",
            &json!({ "cluster": CLUSTER, "containerInstance": &busy }),
        );
        assert_eq!(
            (status, refused["message"].as_str()),
            (400, Some(RUNNING_TASKS_ON_INSTANCE)),
            "case {case:?}: expected HTTP 400 and {RUNNING_TASKS_ON_INSTANCE:?}, got {status} \
             with {refused}"
        );
        let still = described_hosts(&emulator, addressing, case);
        let left_active = still.iter().any(|host| {
            host["containerInstanceArn"].as_str() == Some(busy.as_str())
                && host["status"].as_str() == Some(ACTIVE)
        });
        assert!(
            left_active,
            "case {case:?}: expected the refused deregistration to leave {busy:?} {ACTIVE}, got \
             {still:?}"
        );

        // DiscoverPollEndpoint: the agent of a registered host is handed endpoints, the same
        // ones on every call, and a malformed identifier is refused.
        let first = ecs(
            &emulator,
            addressing,
            case,
            "DiscoverPollEndpoint",
            &json!({ "cluster": CLUSTER, "containerInstance": &busy }),
        );
        let second = ecs(
            &emulator,
            addressing,
            case,
            "DiscoverPollEndpoint",
            &json!({ "cluster": CLUSTER, "containerInstance": &busy }),
        );
        let endpoint = member(case, &first, "/endpoint");
        let telemetry = member(case, &first, "/telemetryEndpoint");
        assert!(
            !endpoint.is_empty() && !telemetry.is_empty(),
            "case {case:?}: expected non-empty endpoints, got {first}"
        );
        assert_eq!(
            (
                second["endpoint"].as_str(),
                second["telemetryEndpoint"].as_str()
            ),
            (Some(endpoint.as_str()), Some(telemetry.as_str())),
            "case {case:?}: expected the same endpoints on a repeated call, got {second}"
        );
        let (status, refused) = ecs_call(
            &emulator,
            addressing,
            "DiscoverPollEndpoint",
            &json!({ "cluster": CLUSTER, "containerInstance": "not a valid @@@ identifier" }),
        );
        assert_eq!(
            (status, refused["message"].as_str()),
            (400, Some(INSTANCE_ID_LENGTH_MESSAGE)),
            "case {case:?}: expected a malformed container-instance identifier to be refused \
             with HTTP 400 and {INSTANCE_ID_LENGTH_MESSAGE:?}, got {status} with {refused}"
        );
        let (status, refused) = ecs_call(
            &emulator,
            addressing,
            "DiscoverPollEndpoint",
            &json!({ "cluster": CLUSTER, "containerInstance": "0".repeat(32) }),
        );
        assert_eq!(
            (status, refused["message"].as_str()),
            (400, Some(RESOURCES_NOT_LOCATED_MESSAGE)),
            "case {case:?}: expected an id that names no registered instance to be refused with \
             HTTP 400 and {RESOURCES_NOT_LOCATED_MESSAGE:?}, got {status} with {refused}"
        );
    }
}
