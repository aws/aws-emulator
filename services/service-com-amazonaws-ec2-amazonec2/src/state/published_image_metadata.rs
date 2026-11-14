// SPDX-License-Identifier: CC0-1.0

//! The image METADATA hierarchies AWS publishes in Parameter Store beside an image's id.
//!
//! An AMI alias publishes ONE value — the id of the image it names
//! ([`super::published_amazon_linux`]) — but a service that recommends an image for its own
//! workload publishes a HIERARCHY about it: the recommended ECS-optimized AMI and the
//! recommended Bottlerocket build are each published as a path whose `image_id` leaf names
//! the image and whose sibling leaves carry the facts a client reads beside it (the image's
//! name and build version, the operating system it is built on, and the container-runtime
//! versions it ships). A client reads the hierarchy, not one name: that is how an ECS
//! container instance or a Bottlerocket node group picks an image without naming a
//! per-Region id.
//!
//! ONE ROW PER PUBLISHED VARIANT. A service that recommends an image publishes a hierarchy
//! for every variant it recommends one for — the operating-system family, the architecture
//! and the accelerator the build carries drivers for — so the TABLE below is the population
//! of the namespace: a variant with no row here is a path a client reading the namespace
//! never sees, and reading it would be refused as a name nothing publishes.
//!
//! Where the publishing service publishes its hierarchy as a MANIFEST, the path itself
//! resolves too, to the JSON document whose members are the very leaves published under it
//! (Amazon ECS Developer Guide, "Retrieve Amazon ECS-optimized AMI metadata": the
//! `recommended` path carries the metadata of the recommended image, and its `image_id`
//! member is the AMI id). The document is rendered FROM those leaves rather than written
//! beside them, so a client that reads the manifest and one that reads a leaf cannot be told
//! two different things.
//!
//! One source of truth, as for the aliases: the `image_id` leaf is the
//! `Image$PublicSsmParameterName` of an image of the published catalog, so the id a read
//! reports is an AMI the account can really describe and launch, and the `image_name` leaf is
//! that image's own `Image$Name` rather than a second copy of it. Only the facts no image
//! attribute holds are written in the table below, and they are service-published data that
//! no model describes: recorded behaviour.
//!
//! Nothing here writes: [`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_values`]
//! projects the hierarchies from the image catalogue on each read, so a hierarchy whose image
//! has been deregistered resolves to nothing, exactly as the describe reports none.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    published_amazon_image, ComAmazonawsEc2Amazonec2State, ImageSpec, BOOT_MODE_LEGACY_BIOS,
    BOOT_MODE_UEFI, BOOT_MODE_UEFI_PREFERRED, IMDS_SUPPORT_V2,
};

/// What a public Systems Manager parameter of the published catalog resolves to.
#[derive(Debug, Clone)]
pub struct PublicSsmParameterValue {
    /// The value the parameter carries.
    pub value: String,
    /// The `CreationDate` of the image the parameter is published beside — the instant that
    /// publication last changed. `None` for an image that reports none.
    pub published_at: Option<String>,
}

/// The leaf an image's id is published under in a metadata hierarchy, and the one the
/// hierarchy is reached through: it is the `Image$PublicSsmParameterName` of the image the
/// hierarchy describes.
const IMAGE_ID_LEAF: &str = "image_id";

/// The leaf the image's own name is published under.
const IMAGE_NAME_LEAF: &str = "image_name";

/// The leaf the schema version of a published hierarchy is carried under — the one member a
/// manifest carries as a JSON NUMBER, a document's own version being a number in it while
/// every other member is the string its own leaf publishes.
const SCHEMA_VERSION_LEAF: &str = "schema_version";

/// One published image-metadata hierarchy.
struct ImageMetadataHierarchy {
    /// The path the hierarchy is published at.
    path: &'static str,
    /// The image the hierarchy describes, which the catalog publishes as a public AMI.
    image: PublishedImage,
    /// Whether it publishes the image's own name beside its id.
    publishes_image_name: bool,
    /// Whether the path itself resolves to the JSON manifest of everything published under
    /// it.
    publishes_manifest: bool,
    /// Every other leaf, as `(leaf, value)`: the facts published about the image that no
    /// image attribute holds.
    facts: &'static [(&'static str, &'static str)],
}

impl ImageMetadataHierarchy {
    /// The public parameter the hierarchy's image is published under.
    fn image_id_parameter(&self) -> String {
        format!("{}/{IMAGE_ID_LEAF}", self.path)
    }
}

/// The image one hierarchy describes, as the published catalog holds it: the facts
/// [`published_amazon_image`] does not derive.
struct PublishedImage {
    /// `Image$Name`, carrying the release the build is of.
    name: &'static str,
    /// `Image$Description`.
    description: &'static str,
    /// `Image$Architecture`.
    architecture: &'static str,
    /// `Image$CreationDate`, the instant the build was published.
    creation_date: &'static str,
    /// `Image$BootMode`.
    boot_mode: &'static str,
    /// Whether the build requires IMDSv2 (`Image$ImdsSupport`).
    requires_imdsv2: bool,
}

/// The release of the recommended ECS-optimized Amazon Linux 2 builds, which their names
/// carry and which the `image_version` their hierarchies publish is — an Amazon Linux 2 name
/// spells a release `2.0.<build date>`.
const ECS_AL2_IMAGE_VERSION: &str = "2.0.20251104";

/// The same for the Amazon Linux 2023 builds, whose names spell a release
/// `<year>.<minor>.<build date>`.
const ECS_AL2023_IMAGE_VERSION: &str = "2023.8.20251104";

/// The `CreationDate` of every ECS-optimized build the catalog publishes: one publication of
/// the recommended builds, so the hierarchies of a family cannot report two different
/// publication instants.
const ECS_OPTIMIZED_CREATION_DATE: &str = "2025-11-04T07:11:29.000Z";

/// The base Amazon Linux 2023 image the ECS-optimized x86 builds are built from, published as
/// their `source_image_name`.
const ECS_AL2023_X86_64_SOURCE_IMAGE: &str = "al2023-ami-2023.8.20251104.0-kernel-6.18-x86_64";

/// The same for 64-bit Arm.
const ECS_AL2023_ARM64_SOURCE_IMAGE: &str = "al2023-ami-2023.8.20251104.0-kernel-6.18-arm64";

/// The base Amazon Linux 2 image the ECS-optimized x86 builds of that family are built from:
/// the family's own last base build of this catalog ([`super::published_amazon_linux`]), that
/// family having reached the end of its standard support.
const ECS_AL2_X86_64_SOURCE_IMAGE: &str = "amzn2-ami-hvm-2.0.20241113.1-x86_64-gp2";

/// The same for 64-bit Arm.
const ECS_AL2_ARM64_SOURCE_IMAGE: &str = "amzn2-ami-hvm-2.0.20241113.1-arm64-gp2";

/// The release of the recommended Bottlerocket builds, carried by their names as well as by
/// the `image_version` the hierarchies publish.
const BOTTLEROCKET_VERSION: &str = "1.46.0";

/// The `CreationDate` of those images.
const BOTTLEROCKET_CREATION_DATE: &str = "2025-10-28T09:03:51.000Z";

/// The container-runtime facts the ECS-optimized hierarchies publish beside their image:
/// the agent and the runtime the build ships, and the schema the hierarchy itself is
/// published under.
const ECS_AGENT_VERSION: &str = "1.97.0";

/// See [`ECS_AGENT_VERSION`].
const ECS_RUNTIME_VERSION: &str = "Docker version 25.0.8";

/// See [`ECS_AGENT_VERSION`].
const ECS_SCHEMA_VERSION: &str = "1";

/// The operating system the ECS-optimized Amazon Linux 2023 builds are built on, as the
/// hierarchy publishes it.
const ECS_AL2023_OS: &str = "Amazon Linux 2023";

/// The same for the Amazon Linux 2 builds.
const ECS_AL2_OS: &str = "Amazon Linux 2";

/// The facts an ECS-optimized hierarchy publishes beside its image, for a build of
/// `release` on `os` built from `source_image`: the container agent and runtime the build
/// ships, its build version, the operating system it is built on, the schema version of the
/// hierarchy itself and the base image it was built from (Amazon ECS Developer Guide,
/// "Retrieve Amazon ECS-optimized AMI metadata", which lists the same members for each
/// family).
///
/// `release` is the release the image's own name carries, so the `image_version` leaf and
/// `Image$Name` cannot name two different builds.
const fn ecs_optimized_facts(
    os: &'static str,
    release: &'static str,
    source_image: &'static str,
) -> [(&'static str, &'static str); 6] {
    [
        ("ecs_agent_version", ECS_AGENT_VERSION),
        ("ecs_runtime_version", ECS_RUNTIME_VERSION),
        ("image_version", release),
        ("os", os),
        (SCHEMA_VERSION_LEAF, ECS_SCHEMA_VERSION),
        ("source_image_name", source_image),
    ]
}

/// The facts the ECS-optimized Amazon Linux 2 hierarchies of 64-bit x86 publish.
const ECS_AL2_X86_64_FACTS: &[(&str, &str)] = &ecs_optimized_facts(
    ECS_AL2_OS,
    ECS_AL2_IMAGE_VERSION,
    ECS_AL2_X86_64_SOURCE_IMAGE,
);

/// The same for 64-bit Arm.
const ECS_AL2_ARM64_FACTS: &[(&str, &str)] = &ecs_optimized_facts(
    ECS_AL2_OS,
    ECS_AL2_IMAGE_VERSION,
    ECS_AL2_ARM64_SOURCE_IMAGE,
);

/// The same for the Amazon Linux 2023 hierarchies of 64-bit x86.
const ECS_AL2023_X86_64_FACTS: &[(&str, &str)] = &ecs_optimized_facts(
    ECS_AL2023_OS,
    ECS_AL2023_IMAGE_VERSION,
    ECS_AL2023_X86_64_SOURCE_IMAGE,
);

/// And for 64-bit Arm.
const ECS_AL2023_ARM64_FACTS: &[(&str, &str)] = &ecs_optimized_facts(
    ECS_AL2023_OS,
    ECS_AL2023_IMAGE_VERSION,
    ECS_AL2023_ARM64_SOURCE_IMAGE,
);

/// The one fact a Bottlerocket hierarchy publishes beside its image: the version of the
/// build, which is what that project publishes.
const BOTTLEROCKET_FACTS: &[(&str, &str)] = &[("image_version", BOTTLEROCKET_VERSION)];

/// Every image-metadata hierarchy the catalog publishes.
///
/// The ECS-optimized rows are the variants Amazon ECS recommends a container-host image for —
/// each Amazon Linux family, each architecture it is built for, and the accelerator builds
/// that carry an accelerator's drivers — and each publishes its JSON manifest at its own path
/// beside the leaves; the Bottlerocket rows publish the build's version beside its id, which
/// is what that project publishes, and no manifest.
const PUBLISHED_IMAGE_METADATA: &[ImageMetadataHierarchy] = &[
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/recommended",
        image: PublishedImage {
            name: "amzn2-ami-ecs-hvm-2.0.20251104-x86_64-ebs",
            description: "Amazon Linux 2 ECS-Optimized AMI 2.0.20251104 x86_64 HVM ebs",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_LEGACY_BIOS,
            requires_imdsv2: false,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/arm64/recommended",
        image: PublishedImage {
            name: "amzn2-ami-ecs-arm64-hvm-2.0.20251104-arm64-ebs",
            description: "Amazon Linux 2 ECS-Optimized AMI 2.0.20251104 arm64 HVM ebs",
            architecture: "arm64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI,
            requires_imdsv2: false,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2_ARM64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/gpu/recommended",
        image: PublishedImage {
            name: "amzn2-ami-ecs-gpu-hvm-2.0.20251104-x86_64-ebs",
            description: "Amazon Linux 2 ECS-Optimized GPU AMI 2.0.20251104 x86_64 HVM ebs",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_LEGACY_BIOS,
            requires_imdsv2: false,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2/inf/recommended",
        image: PublishedImage {
            name: "amzn2-ami-ecs-inf-hvm-2.0.20251104-x86_64-ebs",
            description: "Amazon Linux 2 ECS-Optimized Inferentia AMI 2.0.20251104 x86_64 HVM \
                           ebs",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_LEGACY_BIOS,
            requires_imdsv2: false,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended",
        image: PublishedImage {
            name: "al2023-ami-ecs-hvm-2023.8.20251104-kernel-6.18-x86_64",
            description: "Amazon Linux 2023 ECS-Optimized AMI 2023.8.20251104 x86_64 HVM \
                          kernel-6.18",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI_PREFERRED,
            requires_imdsv2: true,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2023_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/recommended",
        image: PublishedImage {
            name: "al2023-ami-ecs-arm64-2023.8.20251104-kernel-6.18-arm64",
            description: "Amazon Linux 2023 ECS-Optimized AMI 2023.8.20251104 arm64 HVM \
                          kernel-6.18",
            architecture: "arm64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI,
            requires_imdsv2: true,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2023_ARM64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/gpu/recommended",
        image: PublishedImage {
            name: "al2023-ami-ecs-gpu-hvm-2023.8.20251104-kernel-6.18-x86_64",
            description: "Amazon Linux 2023 ECS-Optimized GPU AMI 2023.8.20251104 x86_64 HVM \
                          kernel-6.18",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI_PREFERRED,
            requires_imdsv2: true,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2023_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/ecs/optimized-ami/amazon-linux-2023/neuron/recommended",
        image: PublishedImage {
            name: "al2023-ami-ecs-neuron-hvm-2023.8.20251104-kernel-6.18-x86_64",
            description: "Amazon Linux 2023 ECS-Optimized Neuron AMI 2023.8.20251104 x86_64 \
                          HVM kernel-6.18",
            architecture: "x86_64",
            creation_date: ECS_OPTIMIZED_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI_PREFERRED,
            requires_imdsv2: true,
        },
        publishes_image_name: true,
        publishes_manifest: true,
        facts: ECS_AL2023_X86_64_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/x86_64/latest",
        image: PublishedImage {
            // A Bottlerocket build's name carries its variant, its architecture, its version
            // and the commit it was built from.
            name: "bottlerocket-aws-ecs-2-x86_64-v1.46.0-7f7f0a6c",
            description: "Bottlerocket OS 1.46.0 (aws-ecs-2) x86_64",
            architecture: "x86_64",
            creation_date: BOTTLEROCKET_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI_PREFERRED,
            requires_imdsv2: true,
        },
        publishes_image_name: false,
        publishes_manifest: false,
        facts: BOTTLEROCKET_FACTS,
    },
    ImageMetadataHierarchy {
        path: "/aws/service/bottlerocket/aws-ecs-2/arm64/latest",
        image: PublishedImage {
            // The project spells the 64-bit Arm architecture `aarch64` in a build's name.
            name: "bottlerocket-aws-ecs-2-aarch64-v1.46.0-7f7f0a6c",
            description: "Bottlerocket OS 1.46.0 (aws-ecs-2) aarch64",
            architecture: "arm64",
            creation_date: BOTTLEROCKET_CREATION_DATE,
            boot_mode: BOOT_MODE_UEFI,
            requires_imdsv2: true,
        },
        publishes_image_name: false,
        publishes_manifest: false,
        facts: BOTTLEROCKET_FACTS,
    },
];

/// The images those hierarchies describe, as rows of the published catalog
/// ([`super::published_image_specs`]): each carries the `image_id` leaf of its own hierarchy
/// as its `Image$PublicSsmParameterName`, which is what makes the id a read reports an AMI of
/// this catalog.
///
/// A container-host image is no Free Tier image, unlike the Amazon Linux base images.
pub fn published_image_metadata_specs() -> Vec<ImageSpec> {
    PUBLISHED_IMAGE_METADATA
        .iter()
        .map(|hierarchy| ImageSpec {
            description: hierarchy.image.description.to_string(),
            boot_mode: Some(hierarchy.image.boot_mode.to_string()),
            imds_support: hierarchy
                .image
                .requires_imdsv2
                .then(|| IMDS_SUPPORT_V2.to_string()),
            free_tier_eligible: false,
            public_ssm_parameter_name: Some(hierarchy.image_id_parameter()),
            ..published_amazon_image(
                hierarchy.image.name,
                hierarchy.image.architecture,
                hierarchy.image.creation_date,
            )
        })
        .collect()
}

/// The JSON manifest of a hierarchy: the leaves published under it, as one document.
///
/// Rendered from those very leaves, so no member can disagree with the parameter beside it.
/// Each member carries the leaf's own string, except the schema version of the document
/// itself ([`SCHEMA_VERSION_LEAF`]), which is a number in it. The members are written in the
/// name order the leaves are held in; JSON gives an object's members no order of their own,
/// and a client reads a member by name.
fn manifest(leaves: &BTreeMap<&str, String>) -> String {
    let mut document = Map::new();
    for (leaf, value) in leaves {
        let member = if *leaf == SCHEMA_VERSION_LEAF {
            value
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::from(value.as_str()))
        } else {
            Value::from(value.as_str())
        };
        document.insert((*leaf).to_string(), member);
    }
    Value::Object(document).to_string()
}

impl ComAmazonawsEc2Amazonec2State {
    /// Every public Systems Manager parameter this account can resolve, and the value it
    /// resolves to.
    ///
    /// The ONE projection Parameter Store reads the published catalog through: the AMI
    /// aliases ([`Self::public_ssm_parameter_image_ids`]), each resolving to the id of the
    /// image it names, and the leaves of the metadata hierarchies published beside an image,
    /// each resolving to the fact published there — plus, for a hierarchy its publisher
    /// publishes as a manifest, the path itself, resolving to the JSON document of those
    /// leaves. A hierarchy whose image this account cannot see resolves to nothing at all —
    /// not to a hierarchy of facts about an image that is gone, and not to a manifest of
    /// them — the same way the alias of a deregistered image resolves to nothing.
    pub fn public_ssm_parameter_values(
        &self,
        account: &str,
    ) -> BTreeMap<String, PublicSsmParameterValue> {
        let ids = self.public_ssm_parameter_image_ids(account);
        let published_at = |image_id: &str| -> Option<String> {
            self.images
                .get(image_id)
                .and_then(|image| image.creation_date.clone())
        };
        let mut published: BTreeMap<String, PublicSsmParameterValue> = ids
            .iter()
            .map(|(name, image_id)| {
                (
                    name.clone(),
                    PublicSsmParameterValue {
                        value: image_id.clone(),
                        published_at: published_at(image_id),
                    },
                )
            })
            .collect();
        for hierarchy in PUBLISHED_IMAGE_METADATA {
            let Some((image_id, image)) = ids
                .get(&hierarchy.image_id_parameter())
                .and_then(|image_id| self.images.get(image_id).map(|image| (image_id, image)))
            else {
                continue;
            };
            let at = image.creation_date.clone();
            // Everything the hierarchy publishes, keyed by leaf: the id of the image, its own
            // name where the hierarchy publishes it, and each published fact. The id is
            // already published above, under this very name, as the image's alias.
            let mut leaves: BTreeMap<&str, String> = BTreeMap::new();
            leaves.insert(IMAGE_ID_LEAF, image_id.clone());
            if hierarchy.publishes_image_name {
                if let Some(name) = image.name.clone() {
                    leaves.insert(IMAGE_NAME_LEAF, name);
                }
            }
            for (leaf, value) in hierarchy.facts {
                leaves.insert(leaf, (*value).to_string());
            }
            let mut publish = |name: String, value: String| {
                published.insert(
                    name,
                    PublicSsmParameterValue {
                        value,
                        published_at: at.clone(),
                    },
                );
            };
            for (leaf, value) in &leaves {
                if *leaf == IMAGE_ID_LEAF {
                    continue;
                }
                publish(format!("{}/{leaf}", hierarchy.path), value.clone());
            }
            if hierarchy.publishes_manifest {
                publish(hierarchy.path.to_string(), manifest(&leaves));
            }
        }
        published
    }
}
