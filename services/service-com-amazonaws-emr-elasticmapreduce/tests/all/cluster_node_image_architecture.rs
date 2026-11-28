// SPDX-License-Identifier: CC0-1.0

//! The IMAGE a cluster's nodes boot, end to end over the wire, read back in the service that
//! owns images.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, resolves each instance Amazon EMR reports in Amazon EC2 itself, and then asks
//! Amazon EC2 for the architecture of the image that instance was launched from. Nothing
//! reaches into either service's state struct.
//!
//! THE RULE: an instance runs the machine code its image holds, so a cluster's nodes are
//! launched from an image built for the CPU architecture of the instance type their collection
//! asks for — "the CPU architecture, for example `X86_64` or `AARCH64`"
//! (`SupportedInstanceType$Architecture`), which Amazon EMR publishes per instance type
//! through `ListSupportedInstanceTypes`. A cluster asking for Graviton instance types
//! therefore really launches its capacity, exactly as one asking for x86-64 types does.
//!
//! The rows span what the rule generalises over: both architectures the catalog publishes, the
//! instance-type families of each (general purpose and compute optimised), and both node roles
//! of the cluster — the primary and the core group, which may ask for different types and are
//! then launched from different images.
//!
//! Amazon EC2 is mounted BESIDE Amazon EMR on one engine, through this crate's own
//! instance harness ([`crate::cluster_instances`]), exactly as the assembled router mounts
//! them.

use std::time::Instant;

use serde_json::{json, Value};

use crate::cluster_instances::{
    described_in_ec2, emulator, emr, list_instances, subnet, Emulator, POLL_INTERVAL,
    WAIT_DEADLINE,
};

/// One row: the instance types a cluster's two groups ask for, and the Amazon EC2 image
/// architecture each of them must therefore be launched from.
struct Case {
    label: &'static str,
    primary_type: &'static str,
    core_type: &'static str,
    /// The architecture Amazon EMR publishes for `primary_type` / `core_type`
    /// (`SupportedInstanceType$Architecture`, read back through
    /// `ListSupportedInstanceTypes` below), and the Amazon EC2 `Architecture` of the image
    /// an instance of that type has to boot.
    primary_architecture: Architecture,
    core_architecture: Architecture,
}

/// The two architectures the catalog publishes, with the spelling each service uses for them:
/// Amazon EMR's `SupportedInstanceType$Architecture` and Amazon EC2's `Image$Architecture`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Architecture {
    /// `SupportedInstanceType$Architecture`, as `ListSupportedInstanceTypes` reports it.
    emr: &'static str,
    /// `Image$Architecture`, as `DescribeImages` reports it.
    ec2: &'static str,
}

const X86_64: Architecture = Architecture {
    emr: "X86_64",
    ec2: "x86_64",
};
const AARCH64: Architecture = Architecture {
    emr: "AARCH64",
    ec2: "arm64",
};

/// The release every launch below names: one this service's catalog publishes, and one that
/// offers every family a row names.
const RELEASE_LABEL: &str = "emr-7.9.0";

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "x86-64 primaries and x86-64 core nodes",
            primary_type: "m5.xlarge",
            core_type: "c6i.xlarge",
            primary_architecture: X86_64,
            core_architecture: X86_64,
        },
        Case {
            label: "Graviton primaries and Graviton core nodes",
            primary_type: "m7g.xlarge",
            core_type: "c7g.xlarge",
            primary_architecture: AARCH64,
            core_architecture: AARCH64,
        },
        Case {
            // The two collections of ONE cluster may ask for different architectures, and each
            // is then launched from its own image.
            label: "Graviton primaries and x86-64 core nodes",
            primary_type: "c6g.xlarge",
            core_type: "m5.xlarge",
            primary_architecture: AARCH64,
            core_architecture: X86_64,
        },
    ]
}

/// The `Instances` member one row launches: one primary node and one core node, each of its
/// own instance type.
fn instances(case: &Case) -> Value {
    json!({
        "InstanceGroups": [
            {
                "Name": "primary",
                "InstanceRole": "MASTER",
                "InstanceType": case.primary_type,
                "InstanceCount": 1,
            },
            {
                "Name": "core",
                "InstanceRole": "CORE",
                "InstanceType": case.core_type,
                "InstanceCount": 1,
            },
        ],
    })
}

/// The architecture Amazon EMR publishes for `instance_type`, out of the catalog
/// `ListSupportedInstanceTypes` projects for the release — followed to the end of its paging,
/// so the answer is the catalog's and not one page of it.
async fn published_architecture(emulator: &Emulator, instance_type: &str) -> Option<String> {
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "ReleaseLabel": RELEASE_LABEL });
        if let Some(token) = marker.as_ref() {
            request["Marker"] = json!(token);
        }
        let out = emr(&emulator.router, "ListSupportedInstanceTypes", request).await;
        for published in out["SupportedInstanceTypes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            if published["Type"].as_str() == Some(instance_type) {
                return published["Architecture"].as_str().map(str::to_string);
            }
        }
        match out["Marker"].as_str() {
            Some(token) => marker = Some(token.to_string()),
            None => return None,
        }
    }
}

/// The instances of `cluster_id` once it reports as many as `expected`, looked for until the
/// deadline passes rather than waited out.
async fn launched_instances(
    emulator: &Emulator,
    case: &str,
    cluster_id: &str,
    expected: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        let listed = list_instances(emulator, case, cluster_id, json!({})).await;
        if listed.len() == expected {
            return listed;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {expected} instances, got {listed:?}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every row: each collection's nodes are launched, and the image Amazon EC2 launched each of
/// them from is built for the architecture Amazon EMR publishes for that collection's instance
/// type.
#[tokio::test]
async fn a_clusters_nodes_boot_an_image_of_their_instance_types_architecture() {
    let emulator = emulator();
    let subnet_id = subnet(&emulator).await;

    for case in cases() {
        let label = case.label;
        let cluster_id = crate::cluster_instances::launch(
            &emulator,
            label,
            &subnet_id,
            instances(&case),
        )
        .await;

        // Amazon EMR publishes the architecture of each type, and the rows are written against
        // what it publishes rather than against a guess.
        for (instance_type, expected) in [
            (case.primary_type, case.primary_architecture),
            (case.core_type, case.core_architecture),
        ] {
            assert_eq!(
                published_architecture(&emulator, instance_type).await.as_deref(),
                Some(expected.emr),
                "case {label:?}: ListSupportedInstanceTypes must publish {} for {instance_type}",
                expected.emr
            );
        }

        // One node per collection really exists in the service that owns instances.
        let listed = launched_instances(&emulator, label, &cluster_id, 2).await;
        let ec2_ids: Vec<String> = listed
            .iter()
            .filter_map(|instance| instance["Ec2InstanceId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            ec2_ids.len(),
            2,
            "case {label:?}: every reported instance names the Amazon EC2 instance it is, got \
             {listed:?}"
        );
        let described = described_in_ec2(&emulator, &ec2_ids).await;

        for instance in &listed {
            let ec2_instance_id = instance["Ec2InstanceId"].as_str().unwrap_or_default();
            let instance_type = instance["InstanceType"].as_str().unwrap_or_default();
            let expected = if instance_type == case.primary_type {
                case.primary_architecture
            } else {
                case.core_architecture
            };
            assert!(
                described.contains_key(ec2_instance_id),
                "case {label:?}: Amazon EC2 must hold {ec2_instance_id}, got {:?}",
                described.keys().collect::<Vec<_>>()
            );
            let image_id = image_of(&emulator, ec2_instance_id).await;
            let architecture = image_architecture(&emulator, &image_id).await;
            assert_eq!(
                architecture.as_deref(),
                Some(expected.ec2),
                "case {label:?}: {instance_type} on {ec2_instance_id} must boot a {} image, got \
                 {architecture:?} for {image_id}",
                expected.ec2
            );
        }
    }
}

/// The image Amazon EC2 launched `ec2_instance_id` from.
async fn image_of(emulator: &Emulator, ec2_instance_id: &str) -> String {
    let answer = emulator
        .ec2
        .describe_instances()
        .instance_ids(ec2_instance_id)
        .send()
        .await
        .expect("EC2 must describe the instance EMR launched");
    answer
        .reservations()
        .iter()
        .flat_map(::aws_sdk_ec2::types::Reservation::instances)
        .find(|instance| instance.instance_id() == Some(ec2_instance_id))
        .and_then(::aws_sdk_ec2::types::Instance::image_id)
        .unwrap_or_else(|| panic!("EC2 must report the image {ec2_instance_id} was launched from"))
        .to_string()
}

/// `Image$Architecture` as Amazon EC2 reports it for `image_id` — the service that owns the
/// image, so the architecture asserted on is not this suite's own idea of it.
async fn image_architecture(emulator: &Emulator, image_id: &str) -> Option<String> {
    let answer = emulator
        .ec2
        .describe_images()
        .image_ids(image_id)
        .send()
        .await
        .expect("EC2 must describe the image a cluster's node booted");
    answer
        .images()
        .iter()
        .find(|image| image.image_id() == Some(image_id))
        .and_then(::aws_sdk_ec2::types::Image::architecture)
        .map(|architecture| architecture.as_str().to_string())
}
