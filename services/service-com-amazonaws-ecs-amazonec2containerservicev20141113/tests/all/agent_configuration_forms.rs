// SPDX-License-Identifier: CC0-1.0

//! WHICH CLUSTER an Amazon EC2 instance joins, over the forms the container agent's
//! configuration can reach the instance in — driven end to end over the shared wire harness in
//! [`crate::ecs_wire`] with the Amazon EC2 service mounted beside this one on the same engine,
//! so every host is launched through EC2's OWN API and every value asserted is read back
//! through `ListContainerInstances` and `DescribeContainerInstances`.
//!
//! # The rule under test
//!
//! > The cluster a launched instance's container agent joins is the one its agent
//! > CONFIGURATION FILE names on the instance — the file the instance's own user-data program
//! > wrote, whatever that program did to write it and whatever encodings the document carrying
//! > it arrived in — and not a pattern matched against the text a launch happened to carry.
//!
//! The agent "runs on the instance" and reads `ECS_CLUSTER` out of `/etc/ecs/ecs.config`, which
//! the documented way of joining a cluster appends to ("echo ECS_CLUSTER=<cluster> >>
//! /etc/ecs/ecs.config"); Amazon EC2 RUNS that program in the instance's guest when it comes up
//! ("the user data to make available to the instance", `RunInstancesRequest$UserData`) and
//! decodes the document the payload carries before it reads it, so a document a client encoded
//! itself — which an SDK's own encoding then wrapped a second time — configures the agent
//! exactly as a plain one does.
//!
//! # What the rows span
//!
//! The FORM the configuration is carried in: the documented shell line as the API carries it; the
//! same script encoded a second time, as a client that encodes its own payload sends it; a
//! `#cloud-config` that WRITES the configuration file rather than running a shell line; a script
//! that assembles the value from a variable of its own, so only RUNNING it yields the cluster; the
//! configuration file's own `NAME=value` text, which gives the guest no program at all; and a
//! document that configures no cluster, which joins none. Each row names a cluster of its own, so
//! what a row joined is read off that cluster alone.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, ec2_element, ec2_form, emulator_with_ec2, list, ok, SIGNED_REGION,
};

/// The AMI every launch boots. Well-formed (`ami-<17 hexadecimal digits>`), which is all a
/// launch needs; which AMI carries the container agent is not a fact this emulator publishes.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type every host is launched as.
const INSTANCE_TYPE: &str = "t3.micro";

/// `ContainerInstance$status` of a registered host.
const ACTIVE: &str = "ACTIVE";

/// The file the agent reads its configuration out of on the instance it runs on.
const AGENT_CONFIG_PATH: &str = "/etc/ecs/ecs.config";

/// How often a poll re-reads the surface it waits on, and how long it keeps trying: the agent's
/// registration span with generous margin for a loaded test machine.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// `bytes` as the base64 text that stands for them — the form `UserData` travels in ("user data
/// must be base64-encoded", `RunInstancesRequest$UserData`).
fn encoded(bytes: &[u8]) -> String {
    use ::base64::Engine as _;

    ::base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// How one row's launch carries the agent's configuration.
#[derive(Debug, Clone, Copy)]
enum Carried {
    /// The documented shell line that appends the assignment to the configuration file.
    ShellLine,
    /// The same script, encoded by the client before the API's own encoding wrapped it again.
    EncodedShellLine,
    /// A `#cloud-config` whose `write_files` module writes the configuration file itself.
    CloudConfigWrittenFile,
    /// A script that holds the cluster in a variable of its own, so the assignment exists only
    /// once the guest has RUN it.
    ScriptAssembledValue,
    /// The configuration file's own text, carried as plain user data: no program at all.
    PlainAssignment,
    /// A document that configures no cluster.
    NoCluster,
}

impl Carried {
    /// The `UserData` a launch of this form carries for `cluster`.
    fn user_data(self, cluster: &str) -> String {
        match self {
            Self::ShellLine => encoded(
                format!("#!/bin/sh\necho ECS_CLUSTER={cluster} >> {AGENT_CONFIG_PATH}\n")
                    .as_bytes(),
            ),
            Self::EncodedShellLine => encoded(
                encoded(
                    format!("#!/bin/sh\necho ECS_CLUSTER={cluster} >> {AGENT_CONFIG_PATH}\n")
                        .as_bytes(),
                )
                .as_bytes(),
            ),
            Self::CloudConfigWrittenFile => encoded(
                format!(
                    "#cloud-config\nwrite_files:\n  - path: {AGENT_CONFIG_PATH}\n    \
                     content: |\n      ECS_CLUSTER={cluster}\n"
                )
                .as_bytes(),
            ),
            Self::ScriptAssembledValue => encoded(
                format!(
                    "#!/bin/sh\nmkdir -p /etc/ecs\nJOIN={cluster}\n\
                     echo ECS_CLUSTER=$JOIN >> {AGENT_CONFIG_PATH}\n"
                )
                .as_bytes(),
            ),
            // Plain text, which is what an instance whose launch did not encode its user data
            // carries — and the form the configuration file itself holds.
            Self::PlainAssignment => format!("ECS_CLUSTER={cluster}"),
            Self::NoCluster => encoded(b"#!/bin/sh\nyum update -y\n"),
        }
    }
}

/// One row: the form the configuration is carried in, and the cluster the host must join (empty
/// for a row that joins none).
struct Row {
    case: &'static str,
    carried: Carried,
    joins: &'static str,
}

/// The clusters the rows join — one per row, so what a row joined is read off its own cluster.
const SHELL_CLUSTER: &str = "formsshell";
const ENCODED_CLUSTER: &str = "formsencoded";
const WRITTEN_CLUSTER: &str = "formswritten";
const ASSEMBLED_CLUSTER: &str = "formsassembled";
const PLAIN_CLUSTER: &str = "formsplain";

const ROWS: [Row; 6] = [
    Row {
        case: "the documented shell line, as the API carries it",
        carried: Carried::ShellLine,
        joins: SHELL_CLUSTER,
    },
    Row {
        case: "the same script the client encoded before the SDK encoded it again",
        carried: Carried::EncodedShellLine,
        joins: ENCODED_CLUSTER,
    },
    Row {
        case: "a cloud-config that writes the agent's configuration file",
        carried: Carried::CloudConfigWrittenFile,
        joins: WRITTEN_CLUSTER,
    },
    Row {
        case: "a script that assembles the value from a variable of its own",
        carried: Carried::ScriptAssembledValue,
        joins: ASSEMBLED_CLUSTER,
    },
    Row {
        case: "the configuration file's own text, which runs nothing",
        carried: Carried::PlainAssignment,
        joins: PLAIN_CLUSTER,
    },
    Row {
        case: "a document that configures no cluster",
        carried: Carried::NoCluster,
        joins: "",
    },
];

/// Create the one VPC and subnet every launch below is placed in, and answer the subnet's id.
async fn network(router: &MultiServiceRouter, case: &str) -> String {
    let vpc = ec2_element(
        &ec2_form(router, "CreateVpc", &[("CidrBlock", "10.70.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let zone = format!("{SIGNED_REGION}a");
    let subnet = ec2_form(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc.as_str()),
            ("CidrBlock", "10.70.0.0/20"),
            ("AvailabilityZone", zone.as_str()),
        ],
        case,
    )
    .await;
    ec2_element(&subnet, "subnetId", case)
}

/// Launch ONE instance carrying `row`'s configuration into `subnet`, answering its id.
async fn launch(router: &MultiServiceRouter, row: &Row, subnet: &str) -> String {
    let user_data = row.carried.user_data(row.joins);
    let launched = ec2_form(
        router,
        "RunInstances",
        &[
            ("ImageId", IMAGE_ID),
            ("InstanceType", INSTANCE_TYPE),
            ("SubnetId", subnet),
            ("MinCount", "1"),
            ("MaxCount", "1"),
            ("UserData", user_data.as_str()),
        ],
        row.case,
    )
    .await;
    ec2_element(&launched, "instanceId", row.case)
}

/// The `ec2InstanceId`s `cluster` reports `ACTIVE`, agent-connected capacity for, read back
/// through the listing a client walks first and the describe over what it answered.
async fn capacity_of(router: &MultiServiceRouter, cluster: &str, case: &str) -> BTreeSet<String> {
    let listed = ok(
        router,
        "ListContainerInstances",
        json!({ "cluster": cluster }),
        case,
    )
    .await;
    let arns: Vec<String> = list(&listed, "containerInstanceArns", case)
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    if arns.is_empty() {
        return BTreeSet::new();
    }
    let described = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": cluster, "containerInstances": arns }),
        case,
    )
    .await;
    list(&described, "containerInstances", case)
        .iter()
        .filter(|instance| {
            instance["status"].as_str() == Some(ACTIVE)
                && instance["agentConnected"].as_bool() == Some(true)
        })
        .filter_map(|instance| instance["ec2InstanceId"].as_str().map(str::to_string))
        .collect()
}

/// What a client waiting for its hosts does: read the instances back through Amazon EC2's own
/// describe. The guest of an instance comes up when its launch has finished starting, which is
/// a move that service's own clock owes it, so a client that reads its instances is what
/// materialises the boot that RUNS the user-data program.
async fn describe_instances(router: &MultiServiceRouter, case: &str) {
    let _ = ec2_form(router, "DescribeInstances", &[], case).await;
}

/// Wait until `cluster` reports capacity for exactly `wanted`, answering what it last read.
async fn settled_capacity(
    router: &MultiServiceRouter,
    cluster: &str,
    wanted: &BTreeSet<String>,
    case: &str,
) -> BTreeSet<String> {
    let deadline = Instant::now() + POLL_BUDGET;
    let mut last = capacity_of(router, cluster, case).await;
    while &last != wanted && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        describe_instances(router, case).await;
        last = capacity_of(router, cluster, case).await;
    }
    last
}

/// RULE: the cluster a host joins is the one its agent's configuration names on the instance,
/// however the document carrying that configuration reached it.
#[tokio::test]
async fn a_host_joins_the_cluster_its_agent_configuration_names_in_every_form() {
    let case = "the configuration-form table";
    let router = emulator_with_ec2();
    for cluster in [
        SHELL_CLUSTER,
        ENCODED_CLUSTER,
        WRITTEN_CLUSTER,
        ASSEMBLED_CLUSTER,
        PLAIN_CLUSTER,
    ] {
        create_cluster(&router, cluster, case).await;
    }
    let subnet = network(&router, case).await;

    // Every row's host is launched before anything is read, so the whole table shares one
    // registration span rather than waiting one out per row.
    let mut launched: Vec<String> = Vec::new();
    for row in &ROWS {
        launched.push(launch(&router, row, &subnet).await);
    }

    for (row, instance_id) in ROWS.iter().zip(&launched) {
        if row.joins.is_empty() {
            continue;
        }
        let wanted: BTreeSet<String> = [instance_id.clone()].into_iter().collect();
        let read = settled_capacity(&router, row.joins, &wanted, row.case).await;
        assert_eq!(
            read, wanted,
            "case {:?}: expected {} to report capacity for {instance_id:?}, got {read:?}",
            row.case, row.joins
        );
    }

    // And the row that configures no cluster is capacity of none of them: read after every
    // other row has settled, so this is not merely a registration that had not happened yet.
    let unconfigured: Vec<&String> = ROWS
        .iter()
        .zip(&launched)
        .filter(|(row, _)| row.joins.is_empty())
        .map(|(_, instance_id)| instance_id)
        .collect();
    for cluster in [
        SHELL_CLUSTER,
        ENCODED_CLUSTER,
        WRITTEN_CLUSTER,
        ASSEMBLED_CLUSTER,
        PLAIN_CLUSTER,
    ] {
        let held = capacity_of(&router, cluster, case).await;
        for instance_id in &unconfigured {
            assert!(
                !held.contains(instance_id.as_str()),
                "case {case:?}: expected {cluster} to report no capacity for the host whose \
                 user data configures no cluster ({instance_id:?}), got {held:?}"
            );
        }
    }
}
