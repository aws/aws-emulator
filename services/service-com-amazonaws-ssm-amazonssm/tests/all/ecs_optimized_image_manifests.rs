// SPDX-License-Identifier: CC0-1.0

//! The MANIFEST a recommended-container-host hierarchy of AWS's published `/aws/service`
//! namespace publishes at its own path, end to end over the front door.
//!
//! RULE under test: a service that recommends a container-host image publishes one hierarchy
//! per VARIANT it recommends an image for — the operating-system family, the architecture and
//! the accelerator the build carries drivers for — and where it publishes that hierarchy as a
//! manifest, the path ITSELF resolves too, to the JSON document whose members are exactly the
//! leaves published under it, carrying exactly their values (Amazon ECS Developer Guide,
//! "Retrieve Amazon ECS-optimized AMI metadata": the `recommended` path carries the metadata of
//! the recommended image, and its `image_id` member is the AMI id). None of it is the
//! account's: every one of those names resolves in any account, through a plain read, to facts
//! about an image Amazon EC2 itself describes. A publisher that publishes no manifest publishes
//! nothing at the path, and a name nothing publishes is still the modelled miss.
//!
//! `public_image_metadata_hierarchies` owns the LEAVES of these same hierarchies — their
//! population and the shape each one is reported with; this suite owns the document at the path
//! and the population of the publishing namespaces.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body for Parameter Store, and an
//! `ec2Query` form POST for the Amazon EC2 read. Nothing reaches into a state struct.
//!
//! The rows span the published variants: both Amazon Linux families, both architectures each
//! is built for, and the accelerator builds (a GPU build of either family, an Inferentia build
//! of Amazon Linux 2 and a Neuron build of Amazon Linux 2023) — the names Amazon ECS documents
//! under `/aws/service/ecs/optimized-ami/`. The Bottlerocket rows are the contrast: that
//! project publishes the same kind of hierarchy and NO manifest. WHICH builds and versions are
//! published is service-published data no model describes: recorded behaviour.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{call, error_code};
use super::public_parameter_catalog::{
    describe_image, element, emulator, is_ami_id, names, read_published, recursive_listing,
    PUBLISHED_TYPE,
};

/// The namespace Amazon ECS publishes its recommended container-host hierarchies in.
const ECS_OPTIMIZED_NAMESPACE: &str = "/aws/service/ecs/optimized-ami";

/// The namespace the Bottlerocket project publishes its recommended builds in.
const BOTTLEROCKET_NAMESPACE: &str = "/aws/service/bottlerocket";

/// The leaf a hierarchy's image id is published under, and the one member of a manifest that
/// names an image.
const IMAGE_ID_LEAF: &str = "image_id";

/// The leaf the image's own name is published under.
const IMAGE_NAME_LEAF: &str = "image_name";

/// The leaf the operating system of the build is published under.
const OS_LEAF: &str = "os";

/// The leaf the schema version of the hierarchy is published under: the one member a manifest
/// carries as a JSON number rather than as the string its leaf publishes.
const SCHEMA_VERSION_LEAF: &str = "schema_version";

/// One published recommended-image hierarchy.
struct Variant {
    /// The path a client reads.
    path: &'static str,
    /// The architecture of the image it recommends (`Image$Architecture`).
    architecture: &'static str,
    /// The operating system its `os` leaf publishes, or `None` for a publisher that
    /// publishes none.
    os: Option<&'static str>,
    /// Whether the path itself resolves to the JSON manifest of its leaves.
    manifest: bool,
}

/// The variants under test.
const VARIANTS: &[Variant] = &[
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/arm64/recommended",
        architecture: "arm64",
        os: Some("Amazon Linux 2"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/gpu/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/inf/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2023"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/recommended",
        architecture: "arm64",
        os: Some("Amazon Linux 2023"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/gpu/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2023"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/neuron/recommended",
        architecture: "x86_64",
        os: Some("Amazon Linux 2023"),
        manifest: true,
    },
    Variant {
        path: "/aws/service/bottlerocket/aws-ecs-2/x86_64/latest",
        architecture: "x86_64",
        os: None,
        manifest: false,
    },
    Variant {
        path: "/aws/service/bottlerocket/aws-ecs-2/arm64/latest",
        architecture: "arm64",
        os: None,
        manifest: false,
    },
];

/// Every leaf a recursive read reports under `path`, as `(leaf, value)` pairs.
async fn leaves(router: &MultiServiceRouter, case: &str, path: &str) -> Vec<(String, String)> {
    recursive_listing(router, case, path)
        .await
        .iter()
        .map(|parameter| {
            let name = parameter["Name"].as_str().unwrap_or_default().to_string();
            let leaf = name
                .strip_prefix(&format!("{path}/"))
                .unwrap_or(&name)
                .to_string();
            (
                leaf,
                parameter["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE: a hierarchy published as a manifest resolves at its OWN path too, to the JSON
/// document whose members are exactly the leaves published under it, carrying exactly their
/// values — the schema version of the document being a number in it.
#[tokio::test]
async fn the_manifest_carries_every_leaf_published_under_its_path() {
    let router = emulator();

    for variant in VARIANTS.iter().filter(|variant| variant.manifest) {
        let case = format!("the manifest at {}", variant.path);
        let published = leaves(&router, &case, variant.path).await;
        // Every variant Amazon ECS recommends a host for publishes the image's id, the image's
        // own name and the operating system it is built on, so a dropped leaf fails here
        // rather than going unnoticed below.
        for required in [IMAGE_ID_LEAF, IMAGE_NAME_LEAF, OS_LEAF, SCHEMA_VERSION_LEAF] {
            assert!(
                published.iter().any(|(leaf, _)| leaf == required),
                "case {case:?}: expected a {required} leaf under the path, got {published:?}"
            );
        }

        let parameter = read_published(&router, variant.path).await;
        let document = parameter["Value"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            parameter["Type"].as_str(),
            Some(PUBLISHED_TYPE),
            "case {case:?}: expected Type {PUBLISHED_TYPE}, got {parameter}"
        );
        let parsed: Value = serde_json::from_str(&document).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a JSON document, got {document:?} ({error})")
        });
        let members = parsed
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {document:?}"));

        let mut published_leaves: Vec<String> =
            published.iter().map(|(leaf, _)| leaf.clone()).collect();
        published_leaves.sort();
        let mut member_names: Vec<String> = members.keys().cloned().collect();
        member_names.sort();
        assert_eq!(
            member_names, published_leaves,
            "case {case:?}: expected one member per published leaf, got {document:?}"
        );

        for (leaf, published_value) in &published {
            let member = &members[leaf.as_str()];
            // A document's own schema version is a number in it; every other member carries
            // the string its leaf publishes.
            let expected = if leaf == SCHEMA_VERSION_LEAF {
                published_value
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| json!(published_value))
            } else {
                json!(published_value)
            };
            assert_eq!(
                member, &expected,
                "case {case:?}: expected member {leaf:?} to carry {expected}, got {member}"
            );
        }

        // The operating system the family publishes.
        if let Some(expected) = variant.os {
            assert_eq!(
                members[OS_LEAF].as_str(),
                Some(expected),
                "case {case:?}: expected the os member to publish {expected:?}, got {document:?}"
            );
        }

        // The backed outcome: the id the manifest names is an image Amazon EC2 describes, with
        // the architecture the path names — the value a client passes as the `ImageId` of the
        // launch template its container instances come from.
        let image_id = members[IMAGE_ID_LEAF].as_str().unwrap_or_default();
        assert!(
            is_ami_id(image_id),
            "case {case:?}: expected the {IMAGE_ID_LEAF} member to be an AMI id, got \
             {document:?}"
        );
        let (status, body) = describe_image(&router, image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeImages for {image_id}, got {status} \
             with {body}"
        );
        assert_eq!(
            element(&body, "imageId").as_deref(),
            Some(image_id),
            "case {case:?}: expected DescribeImages to report {image_id}, got {body}"
        );
        assert_eq!(
            element(&body, "architecture").as_deref(),
            Some(variant.architecture),
            "case {case:?}: expected a {} image, got {body}",
            variant.architecture
        );

        // Reading the path as a HIERARCHY still reports only what is UNDER it: the manifest is
        // the parameter AT the path, which no hierarchy read of that path reports.
        let under = names(&recursive_listing(&router, &case, variant.path).await);
        assert!(
            !under.contains(&variant.path.to_string()),
            "case {case:?}: the parameter AT a path is not under it, got {under:?}"
        );
    }
}

/// RULE, the other side of it: a publisher that publishes no manifest publishes nothing at
/// the path itself, so reading it is the modelled `ParameterNotFound` — the catalog serves
/// the names AWS publishes and no name it does not.
#[tokio::test]
async fn a_hierarchy_published_without_a_manifest_serves_nothing_at_its_path() {
    let router = emulator();

    let unpublished: Vec<String> = VARIANTS
        .iter()
        .filter(|variant| !variant.manifest)
        .map(|variant| variant.path.to_string())
        // A variant level that names no published hierarchy, a variant nothing is published
        // for, and a leaf nothing publishes under one that is.
        .chain([
            format!("{ECS_OPTIMIZED_NAMESPACE}/amazon-linux-2"),
            format!("{ECS_OPTIMIZED_NAMESPACE}/amazon-linux-2/no-such-variant/recommended"),
            format!("{ECS_OPTIMIZED_NAMESPACE}/amazon-linux-2/recommended/no_such_fact"),
        ])
        .collect();

    for name in &unpublished {
        let case = format!("the unpublished name {name}");
        let (status, body) = call(&router, "GetParameter", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the read to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ParameterNotFound",
            "case {case:?}: expected ParameterNotFound, got {body}"
        );
    }
}

/// RULE: a recursive read of the publishing namespace enumerates every variant it publishes —
/// each manifest and every leaf beneath it — so a client discovers the variants by reading the
/// namespace rather than by naming one.
#[tokio::test]
async fn the_publishing_namespaces_enumerate_every_variant() {
    let router = emulator();

    for namespace in [ECS_OPTIMIZED_NAMESPACE, BOTTLEROCKET_NAMESPACE] {
        let case = format!("the recursive read of {namespace}");
        let published = names(&recursive_listing(&router, &case, namespace).await);
        for variant in VARIANTS
            .iter()
            .filter(|variant| variant.path.starts_with(namespace))
        {
            let image_id = format!("{}/{IMAGE_ID_LEAF}", variant.path);
            assert!(
                published.contains(&image_id),
                "case {case:?}: expected {image_id} among the published names, got \
                 {published:?}"
            );
            assert_eq!(
                published.contains(&variant.path.to_string()),
                variant.manifest,
                "case {case:?}: expected the manifest of {} to be published only where its \
                 publisher publishes one, got {published:?}",
                variant.path
            );
        }
        assert!(
            published
                .iter()
                .all(|name| name.starts_with(&format!("{namespace}/"))),
            "case {case:?}: expected every reported name to lie under the namespace, got \
             {published:?}"
        );
    }
}
