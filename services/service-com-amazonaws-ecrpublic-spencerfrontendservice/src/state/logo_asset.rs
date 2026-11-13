// SPDX-License-Identifier: CC0-1.0

//! The repository logo as a HOSTED ASSET: the payload a catalog write uploads, the URL the
//! service mints for it, and the route that serves those bytes back.
//!
//! `RepositoryCatalogDataInput$logoImageBlob` is an INPUT-only member — the reported
//! `RepositoryCatalogData` models no payload and carries `logoUrl` instead — so the
//! uploaded bytes are observable only by FETCHING the URL the service minted. That makes
//! the payload part of this registry's state: it is stored with the repository that
//! published it, and an anonymous GET of the minted URL answers it verbatim with the media
//! type the bytes themselves announce.
//!
//! Two things keep the URL and the route from drifting: both are built from
//! [`LOGO_ASSET_PLANE`] here, and both read the asset id out of the payload's own sha256
//! digest — so the same bytes always have the same URL, different bytes never do, and a
//! replacement payload is published at a URL of its own.
//!
//! The URL names the endpoint THIS EMULATOR is served at ([`logo_url`]), not the real
//! gallery host: a URL a client is told to fetch has to lead back to the emulator, which is
//! also why the asset's path sits under the reserved namespace every emulated data plane is
//! addressed in (`service_common::data_plane_addressing`).
//!
//! The path also names the REGISTRY that published the asset, which the route is resolved
//! against ([`super::ComAmazonawsEcrpublicSpencerfrontendserviceState::logo_asset`]): the
//! asset is part of a repository of one registry, and the fetch carries no credential of
//! its own (AWS serves a gallery logo anonymously), so the scope it is read in has to come
//! from the path the service itself minted. A path naming any other registry resolves to
//! nothing rather than falling back to whatever registry the store happens to hold.

use super::*;

/// The segment under the emulator's reserved data-plane namespace that names this
/// service's logo assets: a request path of the form
/// `/_aws/<this>/<registryId>/<repositoryName>/<asset id>`.
///
/// Shared by the URL [`logo_url`] hands out and by the route that serves it
/// ([`logo_asset_route`], read in `crate::service`), so a locator this service published
/// and the path it is served on cannot disagree.
pub const LOGO_ASSET_PLANE: &str = "ecr-public-logos";

/// The smallest logo, in pixels, this registry publishes — per axis.
///
/// Beyond the modelled byte cap on `logoImageBlob` (`@length(0..512000)`), the service
/// decides the payload's PIXEL size: an image below this floor in either axis is refused
/// and nothing about the repository's catalog record changes. The boundary is exact and
/// per-axis, so an image that meets it in one axis and misses it in the other is still
/// refused.
pub const MIN_LOGO_PIXELS: usize = 60;

/// The clause a payload outside the dimension floor is refused with: the member at fault
/// and the rule it failed, in the form this service words a refused parameter in, and
/// NEITHER the rejected dimensions nor the byte cap — the size rule is a different
/// constraint with its own clause.
pub const INVALID_LOGO_DIMENSIONS_MESSAGE: &str =
    "Invalid parameter at 'logoImageBlob' failed to satisfy constraint: 'Invalid image \
     dimensions.'";

/// A logo payload a repository published: the asset id it is addressed by and the bytes
/// themselves, kept so the minted URL can serve them back byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLogo {
    /// The payload's own sha256 digest, hex: the asset id the URL's last segment carries.
    pub asset_id: String,
    /// The payload exactly as it was uploaded. Never echoed into a response body — the
    /// output shape models no payload member — only served on this asset's own route.
    pub bytes: Vec<u8>,
}

impl StoredLogo {
    /// The payload `blob` publishes, keyed by its own digest.
    pub fn new(blob: &[u8]) -> Self {
        Self {
            asset_id: format!("{:x}", Sha256::digest(blob)),
            bytes: blob.to_vec(),
        }
    }

    /// The media type these bytes are served as, read from the payload's own leading
    /// bytes: the formats a browser renders a gallery logo in each announce themselves
    /// there (PNG, RFC 2083 §5.2; JPEG, ISO/IEC 10918-1; GIF87a and GIF89a; the RIFF
    /// container WebP rides in; the BMP file header). Bytes that announce none of them are
    /// served as unnamed binary rather than as an image they are not.
    pub fn media_type(&self) -> &'static str {
        let bytes = self.bytes.as_slice();
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else if bytes.starts_with(b"BM") {
            "image/bmp"
        } else {
            ::service_common::web_media_type::GENERIC_MEDIA_TYPE
        }
    }
}

/// The payload a `RepositoryCatalogDataInput` publishes, or `None` for a document that
/// supplies none — which is what leaves a repository with no logo at all.
pub fn logo_of(input: Option<&RepositoryCatalogDataInput>) -> Option<StoredLogo> {
    input
        .and_then(|input| input.logo_image_blob.as_deref())
        .map(StoredLogo::new)
}

/// Why `logo` cannot be published, or `None` when it can.
///
/// The one rule this decides is the dimension floor ([`MIN_LOGO_PIXELS`]), over the pixel
/// size the payload's own header states ([`pixel_size`]). A payload whose header states no
/// size is left to the modelled byte constraint alone: there is no dimension to compare,
/// so there is no dimension rule to fail.
pub fn logo_violation(logo: Option<&StoredLogo>) -> Option<&'static str> {
    let (width, height) = pixel_size(&logo?.bytes)?;
    (width < MIN_LOGO_PIXELS || height < MIN_LOGO_PIXELS).then_some(INVALID_LOGO_DIMENSIONS_MESSAGE)
}

/// The `(width, height)` in pixels the payload's own header states, or `None` for bytes
/// whose format states none.
///
/// Read by the `imagesize` crate, which probes the header of each image format it knows
/// rather than decoding the picture, so even the largest payload the model admits costs
/// nothing to measure.
fn pixel_size(blob: &[u8]) -> Option<(usize, usize)> {
    let size = ::imagesize::blob_size(blob).ok()?;
    Some((size.width, size.height))
}

/// Where a client fetches the logo `asset_id` of `repository_name` in the registry
/// `registry_id`: `RepositoryCatalogData$logoUrl`, "the URL that contains the logo that's
/// associated with the repository".
///
/// AWS publishes it on the gallery's own host, which this emulator does not serve. The URL
/// therefore names the endpoint THIS EMULATOR answers on
/// ([`::smithy_server_runtime::OperationContext::endpoint`], passed in as `endpoint_base`),
/// with the asset's identity in the path — a URL a client of the emulator can really fetch
/// the stored bytes from, rather than one pointing at the real service.
pub fn logo_url(
    endpoint_base: &str,
    registry_id: &str,
    repository_name: &str,
    asset_id: &str,
) -> String {
    format!(
        "{}{}",
        endpoint_base.trim_end_matches('/'),
        logo_asset_path(registry_id, repository_name, asset_id)
    )
}

/// The request path a logo asset is served on: the reserved data-plane namespace, this
/// service's asset plane, then the registry, the repository and the asset id.
///
/// The namespace and the split into segments are
/// `service_common::data_plane_addressing`'s, so this service addresses its plane the way
/// every other emulated plane is addressed.
///
/// Neither identifier needs escaping: a `RegistryId` is 12 digits and a `RepositoryName`
/// is admitted only by `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$`
/// (both enforced at the request boundary by the model), so every character either can
/// carry is already a path character — and the `/` a namespaced name carries is the
/// separator the route reads it back over.
pub fn logo_asset_path(registry_id: &str, repository_name: &str, asset_id: &str) -> String {
    format!(
        "/{}/{LOGO_ASSET_PLANE}/{registry_id}/{repository_name}/{asset_id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The registry, the repository and the asset id a request `uri` addresses, for a path of
/// the form [`logo_asset_path`] builds, and `None` for every other path.
///
/// A repository name may itself carry `/` (the modelled `RepositoryName` pattern admits
/// namespaced names), so the FIRST segment is the registry, the LAST is the asset id and
/// everything between them is the repository. Both ids have to have the form this service
/// mints them in — a 12-digit account id, and the hex of a sha256 digest — so a path of the
/// same shape that names no asset of this service is not claimed here, and the registry the
/// asset is read in comes from the path rather than from a credential the fetch does not
/// carry.
pub fn logo_asset_route(uri: &str) -> Option<(&str, &str, &str)> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let path = match path.split_once("://") {
        Some((_scheme, authority_and_path)) => authority_and_path
            .find('/')
            .map(|at| &authority_and_path[at..])?,
        None => path,
    };
    let (namespace, rest) = ::service_common::data_plane_addressing::first_segment(path)?;
    if namespace != ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = ::service_common::data_plane_addressing::first_segment(rest)?;
    if plane != LOGO_ASSET_PLANE {
        return None;
    }
    let (registry_id, rest) = rest.strip_prefix('/')?.split_once('/')?;
    let (repository_name, asset_id) = rest.rsplit_once('/')?;
    (is_registry_id_shaped(registry_id)
        && !repository_name.is_empty()
        && is_asset_id_shaped(asset_id))
    .then_some((registry_id, repository_name, asset_id))
}

/// Whether `candidate` has the form the model gives a `RegistryId`: the 12 digits of an
/// account id, as its `^[0-9]{12}$` pattern spells it
/// (`smithy_query(query="shape", shape="RegistryId")`).
fn is_registry_id_shaped(candidate: &str) -> bool {
    candidate.len() == 12 && candidate.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `candidate` has the form [`StoredLogo::new`] mints an asset id in: the 64
/// lower-case hex digits of a sha256 digest.
fn is_asset_id_shaped(candidate: &str) -> bool {
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logo_url_and_the_route_that_serves_it_read_the_same_path() {
        // The rows span what the addressing generalises over: a flat repository name, a
        // namespaced one (whose own slashes the registry and the asset id are read past), a
        // name at the shortest the model admits, and an endpoint base written with a
        // trailing slash.
        let asset_id = StoredLogo::new(b"logo").asset_id;
        let registry = ::engine::EMULATOR_ACCOUNT_ID;
        let cases: [(&str, &str); 4] = [
            ("https://127.0.0.1:9297", "nginx"),
            ("https://127.0.0.1:9297", "org/team/app"),
            ("https://emulator.invalid", "ab"),
            ("https://127.0.0.1:9297/", "0mixed9/with.0-1_2"),
        ];
        for (base, repository) in cases {
            let url = logo_url(base, registry, repository, &asset_id);
            let case = format!("{base} {repository}");
            assert!(
                url.starts_with(base.trim_end_matches('/')),
                "case {case:?}: expected the URL on the endpoint base, got {url:?}"
            );
            let route = logo_asset_route(&url);
            assert_eq!(
                route,
                Some((registry, repository, asset_id.as_str())),
                "case {case:?}: expected the route to read the asset back, got {route:?}"
            );
        }
    }

    #[test]
    fn a_path_that_names_no_logo_asset_is_not_claimed() {
        // The rows span the paths this route must leave alone: another plane's, the
        // modelled control-plane route, a last segment that is no asset id, a first segment
        // that is no registry id, a path with no repository between the two ids, this plane
        // with nothing under it, and the path the real gallery serves a logo on.
        let asset_id = StoredLogo::new(b"logo").asset_id;
        let registry = ::engine::EMULATOR_ACCOUNT_ID;
        let cases: Vec<String> = vec![
            format!("/_aws/es/{registry}/nginx/{asset_id}"),
            "/".to_string(),
            format!("/_aws/ecr-public-logos/{registry}/nginx/not-a-digest"),
            format!("/_aws/ecr-public-logos/not-a-registry/nginx/{asset_id}"),
            format!("/_aws/ecr-public-logos/{registry}/{asset_id}"),
            "/_aws/ecr-public-logos".to_string(),
            format!("/logos/nginx/{asset_id}"),
        ];
        for path in cases {
            let route = logo_asset_route(&path);
            assert!(
                route.is_none(),
                "case {path:?}: expected no logo asset, got {route:?}"
            );
        }
    }

    #[test]
    fn a_payload_is_served_as_the_type_its_own_bytes_announce() {
        // One row per format a logo is published in, plus bytes that announce none.
        let cases: [(&str, &[u8], &str); 6] = [
            ("a PNG", b"\x89PNG\r\n\x1a\n and the rest", "image/png"),
            ("a JPEG", b"\xff\xd8\xff\xe0 and the rest", "image/jpeg"),
            ("a GIF", b"GIF89a and the rest", "image/gif"),
            ("a WebP", b"RIFF\x00\x00\x00\x00WEBPVP8 ", "image/webp"),
            ("a BMP", b"BM and the rest", "image/bmp"),
            (
                "bytes of no image format",
                b"\x00\x01\x02",
                ::service_common::web_media_type::GENERIC_MEDIA_TYPE,
            ),
        ];
        for (case, bytes, expected) in cases {
            let answered = StoredLogo::new(bytes).media_type();
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
}
