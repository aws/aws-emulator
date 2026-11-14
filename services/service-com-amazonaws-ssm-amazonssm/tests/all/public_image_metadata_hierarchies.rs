// SPDX-License-Identifier: CC0-1.0

//! The image-METADATA hierarchies of AWS's published `/aws/service` namespace, end to end
//! over the front door.
//!
//! RULE under test: where a service publishes a recommended image, it publishes a HIERARCHY
//! about it rather than one alias — a path whose `image_id` leaf names the image and whose
//! sibling leaves carry the facts published beside it — so a recursive read of that path
//! enumerates one parameter per published fact, each a `String`/`text` parameter with the
//! account-less ARN of a parameter AWS publishes, and the `image_id` leaf resolves to an id
//! Amazon EC2 itself describes while `image_name` is that image's own name. This is the same
//! rule the AMI alias namespace follows (`public_amazon_linux_alias_namespace`), read one
//! level further: what a path inside AWS's hierarchy enumerates is everything AWS publishes
//! under it.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body for Parameter Store, and an
//! `ec2Query` form POST for the Amazon EC2 read. Nothing reaches into a state struct.
//!
//! The rows are the hierarchies this emulator publishes, written out with the leaves each one
//! holds: the recommended ECS-optimized AMIs of every variant Amazon ECS recommends a
//! container host for (Amazon ECS Developer Guide, "Retrieve Amazon ECS-optimized AMI
//! metadata", whose recommended path publishes the image's id and name, its build version, the
//! operating system it is built on, the base AMI it was built from, the container agent and
//! runtime versions it ships and the schema version of the hierarchy) and the recommended
//! Bottlerocket builds (which publish an id and a version). WHICH builds and versions are
//! published is service-published data no model describes: recorded behaviour.
//!
//! What the path ITSELF resolves to for a hierarchy published as a manifest is
//! `ecs_optimized_image_manifests`; this suite owns the leaves.

use super::parameter_store::REGION;
use super::public_parameter_catalog::{
    describe_image, element, emulator, is_ami_id, listing, names, read_published,
    recursive_listing, PUBLISHED_DATA_TYPE, PUBLISHED_TYPE,
};

/// One published image-metadata hierarchy: the path, and every leaf it publishes.
struct Hierarchy {
    /// The path a client reads.
    path: &'static str,
    /// Every leaf under it, in no particular order.
    leaves: &'static [&'static str],
}

/// The leaves an ECS-optimized `recommended` path publishes.
const ECS_OPTIMIZED_LEAVES: &[&str] = &[
    "ecs_agent_version",
    "ecs_runtime_version",
    "image_id",
    "image_name",
    "image_version",
    "os",
    "schema_version",
    "source_image_name",
];

/// The leaves a Bottlerocket `latest` path publishes.
const BOTTLEROCKET_LEAVES: &[&str] = &["image_id", "image_version"];

/// The hierarchies under test: every ECS-optimized variant and both architectures of the
/// Bottlerocket build for Amazon ECS, so the rows differ in the publishing service, the path
/// form, the operating-system family, the architecture and the accelerator the build carries
/// drivers for.
const HIERARCHIES: &[Hierarchy] = &[
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/arm64/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/gpu/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/inf/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/gpu/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/neuron/recommended",
        leaves: ECS_OPTIMIZED_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/x86_64/latest",
        leaves: BOTTLEROCKET_LEAVES,
    },
    Hierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/arm64/latest",
        leaves: BOTTLEROCKET_LEAVES,
    },
];

/// Every name a hierarchy publishes, as a client writes it, in the ascending name order a
/// hierarchy read reports them in.
fn published_names(hierarchy: &Hierarchy) -> Vec<String> {
    let mut names: Vec<String> = hierarchy
        .leaves
        .iter()
        .map(|leaf| format!("{}/{leaf}", hierarchy.path))
        .collect();
    names.sort();
    names
}

/// RULE: a recursive read of a published metadata hierarchy enumerates exactly the facts
/// published under it, and the one-level read of the same path reports the same population.
#[tokio::test]
async fn every_metadata_hierarchy_enumerates_the_facts_published_under_it() {
    let router = emulator();

    for hierarchy in HIERARCHIES {
        let case = format!("the hierarchy at {}", hierarchy.path);
        let recursive = names(&recursive_listing(&router, &case, hierarchy.path).await);
        assert_eq!(
            recursive,
            published_names(hierarchy),
            "case {case:?}: expected one parameter per published fact, got {recursive:?}"
        );
        let one_level = names(&listing(&router, &case, hierarchy.path, false).await);
        assert_eq!(
            one_level, recursive,
            "case {case:?}: expected the one-level read to report the same population, got \
             {one_level:?}"
        );
    }
}

/// RULE: each leaf is reported as a published parameter — a `String` carrying `text`, a
/// server-assigned version, a last-modified instant and the ARN of a parameter that belongs
/// to no account — and carries a value.
#[tokio::test]
async fn every_leaf_is_shaped_as_a_published_parameter() {
    let router = emulator();

    for hierarchy in HIERARCHIES {
        for name in published_names(hierarchy) {
            let case = format!("leaf {name}");
            let parameter = read_published(&router, &name).await;
            assert_eq!(
                parameter["Type"].as_str(),
                Some(PUBLISHED_TYPE),
                "case {case:?}: expected Type {PUBLISHED_TYPE}, got {parameter}"
            );
            assert_eq!(
                parameter["DataType"].as_str(),
                Some(PUBLISHED_DATA_TYPE),
                "case {case:?}: expected DataType {PUBLISHED_DATA_TYPE}, got {parameter}"
            );
            assert!(
                parameter["Version"].as_i64().unwrap_or_default() >= 1,
                "case {case:?}: expected a server-assigned version of at least 1, got \
                 {parameter}"
            );
            assert!(
                !parameter["LastModifiedDate"].is_null(),
                "case {case:?}: expected a last-modified instant, got {parameter}"
            );
            assert_eq!(
                parameter["ARN"].as_str(),
                Some(format!("arn:aws:ssm:{REGION}::parameter{name}").as_str()),
                "case {case:?}: expected the account-less published-parameter ARN, got \
                 {parameter}"
            );
            assert!(
                !parameter["Value"].as_str().unwrap_or_default().is_empty(),
                "case {case:?}: expected the published fact to carry a value, got {parameter}"
            );
        }
    }
}

/// RULE (the backed outcome): the `image_id` leaf resolves to an id Amazon EC2 describes, with
/// the architecture the path names, and the `image_name` leaf beside it is that very image's
/// own name rather than a second copy of it.
#[tokio::test]
async fn the_image_id_leaf_names_an_image_ec2_describes() {
    let router = emulator();

    for hierarchy in HIERARCHIES {
        let case = format!("the image published at {}", hierarchy.path);
        let image_id = read_published(&router, &format!("{}/image_id", hierarchy.path)).await
            ["Value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_ami_id(&image_id),
            "case {case:?}: expected the image_id leaf to resolve to an AMI id, got {image_id:?}"
        );
        let (status, body) = describe_image(&router, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeImages for {image_id}, got {status} \
             with {body}"
        );
        assert_eq!(
            element(&body, "imageId"),
            Some(image_id.clone()),
            "case {case:?}: expected DescribeImages to report {image_id}, got {body}"
        );

        // The architecture level of the path names the architecture of the image it
        // resolves to; a path that names none publishes the 64-bit x86 build.
        let architecture = if hierarchy.path.contains("arm64") {
            "arm64"
        } else {
            "x86_64"
        };
        assert_eq!(
            element(&body, "architecture").as_deref(),
            Some(architecture),
            "case {case:?}: expected an {architecture} image, got {body}"
        );

        if hierarchy.leaves.contains(&"image_name") {
            let published_name = read_published(&router, &format!("{}/image_name", hierarchy.path))
                .await["Value"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                element(&body, "name"),
                Some(published_name.clone()),
                "case {case:?}: expected the image_name leaf to carry the image's own name \
                 {published_name:?}, got {body}"
            );
        }
    }
}
