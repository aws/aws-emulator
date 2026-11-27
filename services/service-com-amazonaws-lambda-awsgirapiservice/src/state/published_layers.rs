// SPDX-License-Identifier: CC0-1.0

//! The layer versions LAMBDA ITSELF publishes, and the public parameter names they are
//! advertised under.
//!
//! # What this is
//!
//! A function's `Layers` entry is "the ARN of a layer version", and not every layer version
//! a function mounts was published by the account that mounts it: Lambda publishes layers of
//! its own into every Region and advertises each one's ARN as an AWS-published public
//! Systems Manager parameter, so a client resolves the ARN by NAME instead of pinning a
//! Region- and version-bound string. The AWS Parameters and Secrets Lambda Extension is the
//! one such layer this emulator publishes (AWS Secrets Manager User Guide, "Use AWS Secrets
//! Manager secrets in AWS Lambda functions": the extension is added to a function as a layer
//! whose ARN is read from
//! `/aws/service/aws-parameters-and-secrets-lambda-extension/<architecture>/latest`).
//!
//! # Where it is held
//!
//! Here, and in one place: the catalog is a published FACT of this endpoint, not a resource
//! anybody created, so it is projected on each read rather than stored
//! ([`published_layer_record`]) — nothing writes it, and two reads of the same name report
//! the same version, the same ARN and the same instant.
//!
//! It is also not among the layers of the account this endpoint serves. Nothing in this
//! account published it, so it is absent from that account's own layer inventory
//! (`ListLayers`, `ListLayerVersions`, and `GetLayerVersion`'s bare `LayerName`, which
//! enumerate and address what the account published); it is reached the way AWS advertises
//! it, by the ARN of the version — the by-ARN read (`GetLayerVersionByArn`) and a function's
//! `Layers`, whose entries the member's own documentation requires to be layer version ARNs.
//!
//! # What a parameter resolves to
//!
//! [`published_layer_parameters`] is the ONE projection Parameter Store reads this catalog
//! through, so the ARN a public parameter resolves to is the very ARN this crate's own reads
//! answer for and a function's `Layers` accepts: the parameter cannot advertise a version
//! Lambda does not serve.

use super::*;

/// The hierarchy of AWS's public parameter namespace the extension's layer ARN is published
/// under, as the guide section in this module's header spells it.
pub const PARAMETERS_AND_SECRETS_EXTENSION_PATH: &str =
    "/aws/service/aws-parameters-and-secrets-lambda-extension";

/// The bare layer name the extension's 64-bit x86 build is published under.
pub const PARAMETERS_AND_SECRETS_EXTENSION_LAYER: &str =
    "AWS-Parameters-and-Secrets-Lambda-Extension";

/// The bare layer name the extension's 64-bit Arm build is published under — a build of its
/// own, because a layer archive carries native code for one instruction set architecture.
pub const PARAMETERS_AND_SECRETS_EXTENSION_ARM_LAYER: &str =
    "AWS-Parameters-and-Secrets-Lambda-Extension-Arm64";

/// The 64-bit Arm architecture, as the model's `Architecture` enum spells it
/// (`crate::ops::types::ARCHITECTURE_VALUES`).
pub const ARM64_ARCHITECTURE: &str = "arm64";

/// The short form of the 64-bit x86 architecture's name — the other spelling of
/// [`DEFAULT_ARCHITECTURE`], which is the enum's own.
pub const X86_ARCHITECTURE_SHORT_FORM: &str = "x86";

/// The level of a published layer's hierarchy that names the newest version published for an
/// architecture, which is what a client resolving the ARN by name asks for.
pub const LATEST_VERSION_LEAF: &str = "latest";

/// The version of a published layer this endpoint publishes.
///
/// AWS republishes the extension whenever it ships a new build, and each Region is at a
/// version of its own; this endpoint publishes ONE build of it, which is that layer's first
/// and only version ([`FIRST_LAYER_VERSION`]) — the version `latest` therefore names.
pub const PUBLISHED_LAYER_VERSION: i64 = FIRST_LAYER_VERSION;

/// When a published layer version was published, which is what a read reports as its
/// `CreatedDate`.
///
/// Reference data rather than a resource of this endpoint: nothing here holds the instant AWS
/// published a build, so the instant reported is the epoch rather than one that would move
/// between two reads of the same version.
pub const PUBLISHED_AT: SystemTime = SystemTime::UNIX_EPOCH;

/// One layer version Lambda itself publishes into every Region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedLayer {
    /// The bare layer name AWS publishes it under, which its ARNs carry.
    pub name: &'static str,
    /// The hierarchy of the public parameter namespace it is advertised in.
    pub parameter_path: &'static str,
    /// The architecture levels of that hierarchy which address THIS build.
    ///
    /// A published layer is built for one instruction set architecture and the hierarchy's
    /// architecture level names which, so a build is addressed by the names of its
    /// architecture: the name the `Architecture` enum spells (`ARCHITECTURE_VALUES`) and, for
    /// the 64-bit x86 one, its short form as well.
    pub parameter_architectures: &'static [&'static str],
    /// The instruction set architectures the version declares itself compatible with, from
    /// the same enum.
    pub compatible_architectures: &'static [&'static str],
}

impl PublishedLayer {
    /// The ARN of this layer's published version in `region`.
    #[must_use]
    pub fn version_arn(&self, region: &str) -> String {
        format!(
            "{}:{PUBLISHED_LAYER_VERSION}",
            layer_arn(region, self.name)
        )
    }

    /// Every public parameter name this layer is advertised under, with the leaf that names
    /// the newest published version.
    fn parameter_names(&self) -> Vec<String> {
        self.parameter_architectures
            .iter()
            .map(|architecture| {
                format!(
                    "{}/{architecture}/{LATEST_VERSION_LEAF}",
                    self.parameter_path
                )
            })
            .collect()
    }
}

/// Every layer version Lambda itself publishes at this endpoint.
pub const PUBLISHED_LAYERS: &[PublishedLayer] = &[
    PublishedLayer {
        name: PARAMETERS_AND_SECRETS_EXTENSION_LAYER,
        parameter_path: PARAMETERS_AND_SECRETS_EXTENSION_PATH,
        // The 64-bit x86 architecture, under the name the enum spells and its short form.
        parameter_architectures: &[X86_ARCHITECTURE_SHORT_FORM, DEFAULT_ARCHITECTURE],
        compatible_architectures: &[DEFAULT_ARCHITECTURE],
    },
    PublishedLayer {
        name: PARAMETERS_AND_SECRETS_EXTENSION_ARM_LAYER,
        parameter_path: PARAMETERS_AND_SECRETS_EXTENSION_PATH,
        parameter_architectures: &[ARM64_ARCHITECTURE],
        compatible_architectures: &[ARM64_ARCHITECTURE],
    },
];

/// The layer Lambda publishes under the bare name `name`, or `None` for a name it publishes
/// nothing under.
#[must_use]
pub fn published_layer(name: &str) -> Option<&'static PublishedLayer> {
    PUBLISHED_LAYERS.iter().find(|layer| layer.name == name)
}

/// The record of the layer Lambda publishes under `name` in `region`, as the reads report a
/// stored layer — so a published version is reported through [`LayerRecord`]'s own
/// renderings rather than through a second one that could disagree with them.
///
/// One version ([`PUBLISHED_LAYER_VERSION`]) of no archive this endpoint holds bytes for: it
/// therefore reports the sizes and the digest of a version whose bytes are not held, exactly
/// as a version published from an S3 object this emulator does not resolve does. It carries
/// no description, no license and no compatible runtime: an extension layer runs beside any
/// runtime, and a runtime list would say it runs beside only the runtimes listed.
#[must_use]
pub fn published_layer_record(region: &str, name: &str) -> Option<LayerRecord> {
    let published = published_layer(name)?;
    let version = LayerVersionRecord {
        version: PUBLISHED_LAYER_VERSION,
        description: None,
        license_info: None,
        compatible_runtimes: Vec::new(),
        compatible_architectures: published
            .compatible_architectures
            .iter()
            .map(|architecture| (*architecture).to_string())
            .collect(),
        archive: LayerArchiveRecord::default(),
        code_size: 0,
        code_sha256: None,
        signing_profile_version_arn: None,
        created: iso8601(PUBLISHED_AT),
        // AWS publishes this version and no account can grant anything on it here, so it
        // holds no resource-based policy of this account's making.
        policy: None,
    };
    Some(LayerRecord {
        arn: layer_arn(region, published.name),
        region: region.to_string(),
        name: published.name.to_string(),
        latest_version: PUBLISHED_LAYER_VERSION,
        versions: [(PUBLISHED_LAYER_VERSION, version)].into_iter().collect(),
    })
}

/// The report a by-ARN read answers for the published version `version` of `name` in
/// `region`, or `None` when Lambda publishes no such version.
#[must_use]
pub fn published_layer_detail(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    version: i64,
) -> Option<GetLayerVersionOutput> {
    let record = published_layer_record(region, name)?;
    let held = record.versions.get(&version)?;
    Some(record.version_detail(ctx, held))
}

/// The published version `version` of `name` in `region` as a MOUNTED layer of a function's
/// execution environment, or `None` when Lambda publishes no such version.
///
/// The ARN is the published version's own, and both sizes are the ones that version reports:
/// nothing of the request is echoed back as a resolved layer, exactly as a version resolved
/// out of the store is reported from the stored record.
#[must_use]
pub fn published_function_layer(region: &str, name: &str, version: i64) -> Option<FunctionLayer> {
    let record = published_layer_record(region, name)?;
    let held = record.versions.get(&version)?;
    Some(FunctionLayer {
        arn: record.version_arn(held.version),
        code_size: held.code_size,
        uncompressed_code_size: held.unzipped_code_size(),
        // No archive of it is held here, so there are no bytes to mount and no signature of
        // them to have been verified.
        zip_bytes: None,
        signing_profile_version_arn: None,
    })
}

/// Every public Systems Manager parameter the layers Lambda publishes are advertised under in
/// `region`, with the layer version ARN each resolves to.
///
/// The ONE projection Parameter Store reads this catalog through (see this module's header).
#[must_use]
pub fn published_layer_parameters(region: &str) -> Vec<(String, String)> {
    PUBLISHED_LAYERS
        .iter()
        .flat_map(|layer| {
            let arn = layer.version_arn(region);
            layer
                .parameter_names()
                .into_iter()
                .map(move |name| (name, arn.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every public parameter this catalog publishes resolves to the ARN of a version
    /// this catalog serves, in the Region the parameter was read in.
    #[test]
    fn every_published_parameter_names_a_served_version() {
        for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
            let advertised = published_layer_parameters(region);
            assert!(
                !advertised.is_empty(),
                "case {region:?}: expected the catalog to advertise a parameter, got none"
            );
            for (name, value) in advertised {
                let case = format!("{name} in {region}");
                assert!(
                    name.starts_with(&format!("{PARAMETERS_AND_SECRETS_EXTENSION_PATH}/")),
                    "case {case}: expected the name under the published hierarchy, got {name:?}"
                );
                let served = PUBLISHED_LAYERS
                    .iter()
                    .find(|layer| layer.version_arn(region) == value)
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a published version's ARN, got {value:?}")
                    });
                assert!(
                    published_function_layer(region, served.name, PUBLISHED_LAYER_VERSION)
                        .is_some_and(|mounted| mounted.arn == value),
                    "case {case}: expected the advertised version to resolve as a mounted \
                     layer of {value:?}, got {:?}",
                    published_function_layer(region, served.name, PUBLISHED_LAYER_VERSION),
                );
            }
        }
    }

    /// RULE: a published layer is built for one instruction set architecture, and every
    /// architecture level it is addressed by is a name of that architecture.
    #[test]
    fn each_published_layer_declares_one_architecture() {
        for layer in PUBLISHED_LAYERS {
            let case = layer.name;
            assert_eq!(
                layer.compatible_architectures.len(),
                1,
                "case {case:?}: expected one declared architecture, got {:?}",
                layer.compatible_architectures
            );
            let declared = layer.compatible_architectures[0];
            assert!(
                crate::ops::types::ARCHITECTURE_VALUES.contains(&declared),
                "case {case:?}: expected a modelled architecture, got {declared:?}"
            );
            assert!(
                layer
                    .parameter_architectures
                    .iter()
                    .all(|segment| declared.contains(segment)),
                "case {case:?}: expected every architecture level to name {declared:?}, got {:?}",
                layer.parameter_architectures
            );
        }
    }
}
