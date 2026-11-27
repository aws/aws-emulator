// SPDX-License-Identifier: CC0-1.0

//! The digest the container images AWS ITSELF publishes carry, as every resource built around
//! one of them reports it.
//!
//! # Why a published image's digest is a fact about the image
//!
//! A container image a request names is either the caller's own or one AWS publishes for
//! SageMaker — the algorithm, framework and deep-learning images of
//! [`super::aws_container_registry`]. The two differ in who PUSHED the image. The caller's own
//! image is pushed to the caller's registry, which this emulator serves, so the digest reported
//! for it is the one that registry minted for the manifest the push carried
//! ([`super::registry_image_digest`]). An image AWS publishes is pushed by AWS and pulled by
//! every account: the digest its manifest carries is a property of that published image, and no
//! registry this emulator serves holds the image to be asked for it.
//!
//! What a reader reports is the digest of the image the reference NAMES:
//! `DeployedImage$ResolvedImage` is "The specific digest path of the image hosted in this
//! ProductionVariant", over a reference written in one of the two documented path formats,
//! `registry/repository[:tag]` and `registry/repository[@digest]`
//! (`smithy_query(query="shape",shape="DeployedImage")`,
//! `smithy_query(query="doc",shape="AlgorithmSpecification",member="TrainingImage")`). For a
//! reference into a registry AWS publishes from, that is the published image's own digest, not
//! an identifier of the reference text.
//!
//! So the digests below are carried here the way [`super::builtin_algorithm_metrics`] carries
//! the metric set a built-in algorithm publishes and [`super::aws_container_registry`] the
//! registries AWS publishes its containers from: facts about AWS's own catalogue of containers,
//! which an emulator of the services that reference them answers from rather than invents.
//!
//! # Which references are answered from it
//!
//! A digest names "a specific version of an image" (`DeployedImage$ResolvedImage`), so what a
//! digest is known for is one TAG of a published repository: two tags of one repository are two
//! manifests, each with its own digest, and knowing one says nothing about the other. The
//! record below is therefore keyed by the published repository AND the tag, and a reference
//! naming any other tag of the same repository — including one naming no tag at all, which asks
//! for the registry's own default tag rather than for a published version — resolves to no
//! published digest at all. Reporting one image's digest for another is a value every
//! comparison a caller makes with it gets wrong, so such a reference keeps this emulator's own
//! identifier for it ([`super::derived_image_digest`]) instead.
//!
//! A digest identifies the MANIFEST and not the registry holding it, so the same published
//! repository and tag carries one digest in every Region AWS publishes it in: AWS publishes a
//! Region's copy of an image from that Region's own registry account, and a copy of a manifest
//! is the same manifest. What the host of a reference has to satisfy is therefore that it
//! addresses a registry AWS publishes SageMaker's containers from IN the Region it names
//! ([`super::aws_publishes_from`], the same question a pull of such an image is admitted on): a
//! repository of the same name in the caller's own registry holds a different image, and is
//! reported with what that registry holds for it.

use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state as ecr_store;

use super::BOOSTED_TREES_REPOSITORY;

/// The tag AWS publishes its built-in gradient-boosted tree algorithm's image under.
///
/// That algorithm is the one published image this crate models the behaviour of: the metric set
/// it publishes ([`super::builtin_algorithm_metrics`]) and the artifacts its container writes
/// ([`super::boosted_model`]) are carried beside its digest here.
const BOOSTED_TREES_IMAGE_TAG: &str = "1";

/// The digest the image published as [`BOOSTED_TREES_IMAGE_TAG`] of that algorithm's
/// repository carries.
///
/// Recorded behaviour, which is where the digest of an image held in a registry nothing here
/// serves is stated at all: a read of a resource built around that published version reports
/// this value, whichever published registry the reference pulled the version from.
const BOOSTED_TREES_IMAGE_DIGEST: &str =
    "sha256:0c8f830ac408e6dee08445fb60392e9c3f05f790a4b3c07ec22327c08f75bcbf";

/// The digest each image AWS publishes carries, by the repository AWS publishes it as and the
/// tag that published version is named by.
///
/// The repository is matched in FULL and so is the tag: a built-in algorithm's own repository
/// and the repository of the framework image whose name carries the same segment (`xgboost`
/// beside `sagemaker-xgboost`), and two tags of one repository, are each a different image with
/// a manifest and a digest of its own.
const PUBLISHED_IMAGE_DIGESTS: &[(&str, &str, &str)] = &[(
    BOOSTED_TREES_REPOSITORY,
    BOOSTED_TREES_IMAGE_TAG,
    BOOSTED_TREES_IMAGE_DIGEST,
)];

/// The digest the image the reference `image` names carries, for a reference that addresses a
/// registry AWS publishes SageMaker's containers from and names a repository and tag a digest is
/// held for — and `None` for every other reference, which states nothing about the image and
/// leaves its digest to the other steps [`super::image_digest`] documents.
///
/// The path is read by Amazon ECR's OWN reading of a pull reference
/// (`state::registry_pull::parse_registry_reference`), so a path this service resolves and a
/// path that registry serves are the same paths, and a path naming no tag asks that registry
/// for its default tag rather than for a published version. The registry account and the Region
/// both come out of the host, so an AWS registry account written into the host of another
/// Region — addressing a registry AWS publishes nothing from — is not answered for, and neither
/// is a path that addresses no Amazon ECR registry at all.
#[must_use]
pub fn published_image_digest(image: &str) -> Option<&'static str> {
    let (registry_id, reference) = ecr_store::registry_pull::parse_registry_reference(image)?;
    if !super::aws_publishes_from(&reference.region, &registry_id) {
        return None;
    }
    let tag = reference.tag.as_deref()?;
    PUBLISHED_IMAGE_DIGESTS
        .iter()
        .find(|(repository, published, _)| {
            *repository == reference.repository_name && *published == tag
        })
        .map(|(_, _, digest)| *digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest the published version of the built-in gradient-boosted tree algorithm's image
    /// carries, written out here rather than read from the record under test, so this test
    /// states the expectation independently of the value the record holds.
    const PUBLISHED: &str =
        "sha256:0c8f830ac408e6dee08445fb60392e9c3f05f790a4b3c07ec22327c08f75bcbf";

    /// RULE: a digest is known for ONE published version of an image — one repository and one
    /// tag of one published registry family — so a reference naming that version is answered
    /// with it and a reference naming anything else is answered by nothing here.
    #[test]
    fn only_the_published_version_a_digest_is_held_for_is_answered_with_it() {
        // Rows differ along what the rule generalises over: which published registry (and so
        // which Region) the path names the version in, which tag of the repository it names,
        // whether it names a tag at all, whether the repository is the published one, and whose
        // registry the host addresses.
        let rows: [(&str, &str, Option<&str>); 12] = [
            (
                "the published version in one Region's registry",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/xgboost:1",
                Some(PUBLISHED),
            ),
            (
                "the same published version in another Region's registry",
                "811284229777.dkr.ecr.us-east-1.amazonaws.com/xgboost:1",
                Some(PUBLISHED),
            ),
            (
                "another tag of the same published repository",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/xgboost:1.7-1",
                None,
            ),
            (
                "the repository's default tag",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/xgboost:latest",
                None,
            ),
            (
                "the same repository naming no tag at all",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/xgboost",
                None,
            ),
            (
                "the same repository addressed by a digest instead of a tag",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/xgboost@sha256:abc",
                None,
            ),
            (
                "a framework repository whose name carries the same segment",
                "246618743249.dkr.ecr.us-west-2.amazonaws.com/sagemaker-xgboost:1",
                None,
            ),
            (
                "a published repository no digest is held for",
                "433757028032.dkr.ecr.us-west-2.amazonaws.com/linear-learner:1",
                None,
            ),
            (
                "the same repository and tag in the caller's own registry",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com/xgboost:1",
                None,
            ),
            (
                "a published registry account inside another Region's host",
                "433757028032.dkr.ecr.us-east-1.amazonaws.com/xgboost:1",
                None,
            ),
            (
                "a registry that is no Amazon ECR registry",
                "registry.example.com/xgboost:1",
                None,
            ),
            ("a bare repository naming no registry", "xgboost:1", None),
        ];
        for (case, image, expected) in rows {
            let resolved = published_image_digest(image);
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected {expected:?} for {image:?}, got {resolved:?}"
            );
        }
    }
}
