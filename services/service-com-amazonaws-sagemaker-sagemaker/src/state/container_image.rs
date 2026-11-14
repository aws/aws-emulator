// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of a container image path a SageMaker request names: is it a reference an
//! image registry can resolve at all?
//!
//! Every container image member of this service targets `ContainerImage`
//! (`smithy_query(query="shape",shape="ContainerImage")`: `@length 0..255`,
//! `@pattern ^[\S]+$`), and the member's own doc states what the value has to BE: "SageMaker
//! supports both `registry/repository[:tag]` and `registry/repository[@digest]` image path
//! formats" (`ModelPackageContainerDefinition$Image`, and the same sentence on
//! `ContainerDefinition$Image`). The modelled pattern only keeps whitespace out of it, so the
//! FORM of the reference is this service's own rule — stated here once, because it is a
//! property of that shape rather than of any one operation.
//!
//! The form is the published image-reference grammar (the OCI distribution reference
//! grammar every registry client parses a path with): an optional registry host, one or more
//! lowercase path components, and an optional `:tag` or `@algorithm:digest`. It is matched
//! with the maintained regex engine, against the grammar's own expression, rather than by a
//! hand-rolled walk over the characters.
//!
//! Two questions are asked of a path, and which of them an operation asks is decided by what
//! the recordings show that operation doing:
//!
//! * is the value a reference AT ALL ([`malformed_container_image`]) — the question the
//!   hosting creates ask, since their recordings show a model registered around the image
//!   paths they name;
//! * does it carry the REGISTRY and repository both documented formats name
//!   ([`first_image_outside_the_documented_formats`]) — the question the model-package
//!   registration asks, which is recorded refusing a path without them.
//!
//! What neither of them decides: whether any registry HOLDS the image. The recorded service
//! registers a resource around a path whose repository this account's registry does not hold,
//! and around one in another account's registry, so existence is not part of admission — only
//! the form is.

// This slice needs nothing from [`super`]'s records: a container image path is read out of
// the request alone. The regex engine is the crate's, named in full below.

/// The published image-reference grammar, as the one expression a path is matched against.
///
/// Written from the OCI distribution reference grammar, part by part, so each line is the
/// grammar's own production:
///
/// * `domain` — an optional registry host and port: dot-separated components of letters,
///   digits and hyphens, or a bracketed IPv6 address, followed by `/`;
/// * `path` — one or more components of lowercase letters and digits, which may be separated
///   inside a component by `.`, `_`, `__` or runs of `-`, and between components by `/`;
/// * `tag` — an optional `:` followed by up to 128 word characters, dots and hyphens;
/// * `digest` — an optional `@` followed by an algorithm name and the hex digits that name
///   the content. The grammar's own hex floor (32 digits) is NOT imposed: how long a digest
///   is belongs to the algorithm that produced it, and the recorded service registers a
///   resource around a short one, so only the FORM — an algorithm, a colon and hex digits —
///   is held to.
///
/// Anchored at both ends, because the whole value has to be a reference.
const IMAGE_REFERENCE_PATTERN: &str = concat!(
    r"^",
    // domain
    r"(?:(?:(?:[a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9-]*[a-zA-Z0-9])",
    r"(?:\.(?:[a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9-]*[a-zA-Z0-9]))*",
    r"|\[[a-fA-F0-9:]+\])(?::[0-9]+)?/)?",
    // path
    r"[a-z0-9]+(?:(?:[._]|__|-+)[a-z0-9]+)*",
    r"(?:/[a-z0-9]+(?:(?:[._]|__|-+)[a-z0-9]+)*)*",
    // tag
    r"(?::[\w][\w.-]{0,127})?",
    // digest
    r"(?:@[A-Za-z][A-Za-z0-9]*(?:[-_+.][A-Za-z][A-Za-z0-9]*)*:[0-9a-fA-F]+)?",
    r"$",
);

/// The form a container image's digest has to carry to be REPORTED as one, which is the
/// `ImageDigest` shape's own modelled pattern
/// (`smithy_query(query="shape",shape="ImageDigest")`: `@length 0..72`,
/// `@pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`): the SHA-256 algorithm name, a colon and the
/// 64 hex digits of the content hash.
///
/// The reference grammar above admits other algorithms and shorter hex runs; a digest outside
/// this pattern is one no `ImageDigest`-typed member can carry, so it is not reported as one.
const REPORTABLE_IMAGE_DIGEST_PATTERN: &str = r"^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$";

/// The content digest an image reference PINS, and `None` for a reference that pins none or
/// pins one outside [`REPORTABLE_IMAGE_DIGEST_PATTERN`].
///
/// A reference in the `registry/repository[@digest]` format the image member documents names
/// the image by its content digest, so that digest IS the digest of the image the resource is
/// registered against — nothing has to be resolved from a registry to know it. A reference in
/// the `registry/repository[:tag]` format pins nothing, so it names no digest.
#[must_use]
pub fn pinned_image_digest(image: &str) -> Option<&str> {
    static DIGEST: ::std::sync::OnceLock<::regex::Regex> = ::std::sync::OnceLock::new();
    let reportable = DIGEST.get_or_init(|| {
        ::regex::Regex::new(REPORTABLE_IMAGE_DIGEST_PATTERN)
            .expect("the modelled pattern compiles")
    });
    let (_, digest) = image.rsplit_once('@')?;
    reportable.is_match(digest).then_some(digest)
}

/// Whether `image` is NOT a reference an image registry can resolve: a value outside the
/// published grammar ([`IMAGE_REFERENCE_PATTERN`]).
///
/// The expression is compiled ONCE for the process, on first use: it is a constant, so
/// compiling it per request would be work repeated for every container of every create.
#[must_use]
pub fn malformed_container_image(image: &str) -> bool {
    static REFERENCE: ::std::sync::OnceLock<::regex::Regex> = ::std::sync::OnceLock::new();
    let reference = REFERENCE.get_or_init(|| {
        ::regex::Regex::new(IMAGE_REFERENCE_PATTERN).expect("the grammar's own expression compiles")
    });
    !reference.is_match(image)
}

/// The FIRST container image path of `images` that is no reference at all, in the order the
/// request carries them — and `None` when every one of them is well formed.
///
/// A create carrying SEVERAL containers settles them in the order the request wrote them, so
/// the refusal names the same image whichever others the request also carries.
#[must_use]
pub fn first_malformed_container_image<'image>(
    images: impl IntoIterator<Item = &'image str>,
) -> Option<&'image str> {
    images
        .into_iter()
        .find(|image| malformed_container_image(image))
}

/// Whether `image` names no REGISTRY and repository pair, which is what both image path
/// formats the member's doc documents carry: "SageMaker supports both
/// `registry/repository[:tag]` and `registry/repository[@digest]` image path formats".
///
/// Which component IS the registry is the published grammar's own rule: the first component
/// of the path is the registry when it carries a `.` or a `:`, or when it is `localhost` —
/// otherwise the whole path is a repository name and the reference names no registry at all.
#[must_use]
pub fn image_names_no_registry(image: &str) -> bool {
    let Some((first, repository)) = image.split_once('/') else {
        return true;
    };
    let names_a_host = first.contains('.') || first.contains(':') || first == "localhost";
    !names_a_host || repository.is_empty()
}

/// The FIRST container image path of `images` outside the two formats the member's doc
/// documents, in the order the request carries them — and `None` when every one of them is
/// one of those.
///
/// Two conditions, both about the FORM of the path and neither about what any registry holds:
/// the value has to be a reference at all ([`malformed_container_image`]) and it has to carry
/// the registry and repository those formats name ([`image_names_no_registry`]).
#[must_use]
pub fn first_image_outside_the_documented_formats<'image>(
    images: impl IntoIterator<Item = &'image str>,
) -> Option<&'image str> {
    images
        .into_iter()
        .find(|image| malformed_container_image(image) || image_names_no_registry(image))
}

/// How SageMaker words a container image path it cannot resolve as an Amazon ECR image: a
/// FIXED sentence carrying nothing from the request.
pub const INVALID_IMAGE_PATH: &str = "Provided image is not a valid ECR image.";

#[cfg(test)]
mod tests {
    use super::{malformed_container_image, pinned_image_digest};

    /// RULE: a reference in the documented `registry/repository[@digest]` format names the
    /// image by the digest it pins, and one that pins no digest a member can carry names
    /// none.
    ///
    /// The rows span what that generalises over: the presence of a registry, a port and a
    /// tag beside the digest, the case of the hex and of the algorithm name, and the forms
    /// that pin nothing reportable (a tag, no suffix at all, another algorithm, a hex run
    /// of the wrong length).
    #[test]
    fn a_reference_names_the_digest_it_pins() {
        let sha = "8ebddbc1".repeat(8);
        let upper = sha.to_uppercase();
        // (case, the path, the digest the reference pins)
        let rows: Vec<(&str, String, Option<String>)> = vec![
            (
                "an ECR digest path",
                format!("000000000000.dkr.ecr.us-east-1.amazonaws.com/inference@sha256:{sha}"),
                Some(format!("sha256:{sha}")),
            ),
            (
                "a bare repository with a digest",
                format!("inference@sha256:{sha}"),
                Some(format!("sha256:{sha}")),
            ),
            (
                "a registry with a port and a digest",
                format!("registry.example.com:5000/team/inference@sha256:{sha}"),
                Some(format!("sha256:{sha}")),
            ),
            (
                "uppercase hex",
                format!("registry.example.com/inference@sha256:{upper}"),
                Some(format!("sha256:{upper}")),
            ),
            (
                "an uppercased algorithm name",
                format!("registry.example.com/inference@SHA256:{sha}"),
                Some(format!("SHA256:{sha}")),
            ),
            (
                "a tagged reference",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1".to_string(),
                None,
            ),
            ("no tag and no digest", "inference".to_string(), None),
            (
                "another algorithm",
                format!("inference@sha512:{sha}{sha}"),
                None,
            ),
            ("a hex run that is too short", "inference@sha256:0123abcd".to_string(), None),
            (
                "a hex run that is too long",
                format!("inference@sha256:{sha}00"),
                None,
            ),
        ];

        for (case, image, expected) in rows {
            let pinned = pinned_image_digest(&image).map(str::to_string);
            assert_eq!(
                pinned, expected,
                "case {case:?}: expected {expected:?} for {image:?}, got {pinned:?}"
            );
        }
    }

    /// RULE: a container image path is admitted exactly when it is a reference the published
    /// grammar admits — a repository, optionally behind a registry host and followed by a tag
    /// or a digest — and refused otherwise.
    ///
    /// The rows span what the grammar generalises over: the presence of a registry and of a
    /// port, the number of path components, the separators inside a component, a tag versus a
    /// digest, and the characters each part admits — each refused row breaking exactly one of
    /// those productions.
    #[test]
    fn an_image_path_is_admitted_exactly_when_it_is_a_reference() {
        // (case, the path, whether the grammar admits it)
        let rows: [(&str, &str, bool); 24] = [
            ("a bare repository", "inference", true),
            ("a repository and a tag", "inference:1", true),
            ("a namespaced repository", "team/inference:latest", true),
            (
                "an ECR registry path",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1",
                true,
            ),
            (
                "an ECR digest path",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference@sha256:\
                 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                true,
            ),
            ("a registry with a port", "registry.example.com:5000/inference:1", true),
            ("separators inside a component", "my.team_inference--v2:1", true),
            ("deep path components", "a/b/c/d:1", true),
            ("a tag of word characters", "inference:v1.2.3-rc1", true),
            ("prose", "this is not an ecr image uri", false),
            ("a leading space", " inference:1", false),
            ("an uppercase repository", "Inference:1", false),
            ("an underscore-led repository", "_inference:1", false),
            ("a scheme", "https://registry.example.com/inference:1", false),
            ("an empty path component", "registry.example.com//inference:1", false),
            ("a trailing colon", "inference:", false),
            ("a trailing slash", "inference/", false),
            ("colons alone", "::::", false),
            ("an empty value", "", false),
            ("a digest that is not hex", "inference@sha256:nothex", false),
            ("a digest shorter than the hash", "inference@sha256:0", true),
            ("a digest with no algorithm", "inference@:0123abcd", false),
            ("a digest with no hex at all", "inference@sha256:", false),
            ("a tag with a space", "inference:tag with a space", false),
        ];

        for (case, image, admitted) in rows {
            let refused = malformed_container_image(image);
            assert!(
                refused != admitted,
                "case {case:?}: expected admitted={admitted} for {image:?}, got \
                 admitted={}",
                !refused
            );
        }
    }

    /// RULE: a request carrying several images is refused for the FIRST one that is no
    /// reference, and for none when every one of them is.
    #[test]
    fn the_first_malformed_image_of_a_request_is_the_one_named() {
        let rows: [(&str, Vec<&str>, Option<&str>); 4] = [
            ("every image well formed", vec!["a:1", "b/c:2"], None),
            ("the first malformed", vec!["A:1", "B:2"], Some("A:1")),
            ("a later one malformed", vec!["a:1", "B:2", "C:3"], Some("B:2")),
            ("no image at all", Vec::new(), None),
        ];
        for (case, images, expected) in rows {
            let named = super::first_malformed_container_image(images.iter().copied());
            assert_eq!(
                named, expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: a path is one of the two DOCUMENTED formats only when it carries a registry
    /// host and a repository — which is also the published grammar's own rule for which
    /// component is the registry: the first one, when it carries a dot or a port, or is
    /// `localhost`.
    ///
    /// The rows span what that generalises over: an ECR host, another registry's host, a
    /// host with a port, `localhost`, a namespaced repository behind a host, and the forms
    /// that name no host at all (a bare repository, a tagged one, a two-component path whose
    /// first component is no host, and a host with no repository behind it).
    #[test]
    fn a_documented_image_path_carries_a_registry_and_a_repository() {
        // (case, the path, whether it is one of the documented formats)
        let rows: [(&str, &str, bool); 11] = [
            (
                "an ECR host and a repository",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1",
                true,
            ),
            ("another registry's host", "registry.example.com/inference:1", true),
            ("a host with a port", "registry.example.com:5000/inference", true),
            ("localhost", "localhost/inference:1", true),
            (
                "a namespaced repository behind a host",
                "registry.example.com/team/inference@sha256:cafe",
                true,
            ),
            ("a bare repository", "inference", false),
            ("a tagged repository", "inference:1", false),
            ("a first component that is no host", "library/inference:1", false),
            ("a host with no repository behind it", "registry.example.com/", false),
            ("prose", "this is not an ecr image uri", false),
            ("an uppercase repository behind a host", "registry.example.com/Inference", false),
        ];
        for (case, image, documented) in rows {
            let named = super::first_image_outside_the_documented_formats([image]);
            assert_eq!(
                named.is_none(),
                documented,
                "case {case:?}: expected documented={documented} for {image:?}, got {named:?}"
            );
        }
    }
}
