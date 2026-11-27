// SPDX-License-Identifier: CC0-1.0

//! WHERE a layer version's archive and a function's deployment package are downloaded
//! from: the URL a read announces for one, and the object a request to that URL names.
//!
//! `GetLayerVersion` reports `Content$Location`, "A link to the layer archive in Amazon S3
//! that is valid for 10 minutes", and `GetFunction` reports `Code$Location`, "A presigned
//! URL that you can use to download the deployment package". Both are addresses the CALLER
//! follows, so each has to name a host this emulator answers on: it is reached on its own
//! endpoint alone, and no production AWS hostname resolves to it. So a location is
//! PRESIGNED against this emulator's own base URL (`ctx.url_signer()`), and the object's
//! S3 address travels in the PATH, the way a path-style S3 URL carries it:
//! `/<bucket>/snapshots/<account>/<key>`, over the per-region bucket Lambda keeps these
//! objects in ([`download_bucket`]) and the key under it.
//!
//! [`parse_download_target`] reads that path back, so the URL this service hands out and
//! the request `crate::service`'s download plane claims cannot disagree about either half.
//! A presigned URL is followed with no `Authorization` header — it carries its SigV4
//! credential in the QUERY instead — so the plane declares no scheme of its own and the
//! emulator's front door verifies that signature, its expiry and the method and path it was
//! minted for before the plane serves an object: the signature is what makes the announced
//! address a presigned one, and what authorizes the fetch of it.

use super::*;

/// The download location a layer version's `Content$Location` reports ("A link to the
/// layer archive in Amazon S3 that is valid for 10 minutes").
///
/// Lambda keeps the archive in its own per-region bucket and vends a URL into it. Every
/// segment of the object's address comes from stored state — the layer's region, its name
/// and the version number — so each read of one published version addresses the same
/// object, and no read changes what a later one sees.
///
/// The URL is one this emulator ANSWERS, exactly as [`code_location_url`] is.
#[must_use]
pub fn layer_archive_location(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    version: i64,
) -> String {
    download_location(
        ctx,
        region,
        DOWNLOAD_BUCKET_LAYERS,
        &format!("{name}-{version}"),
    )
}

/// The download location `GetFunction` reports for a .zip file archive package.
///
/// Lambda keeps the archive in its own per-region task bucket and vends a URL into it.
/// Every segment of the object's address comes from stored state — the function's region,
/// its name and the revision the stored code belongs to — so each read of an unchanged
/// function addresses the same object, and a new revision addresses a new one.
///
/// The URL is one this emulator ANSWERS: [`parse_download_target`] reads it back and
/// `crate::service` serves the stored bytes from it.
#[must_use]
pub fn code_location_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    revision_id: &str,
) -> String {
    download_location(
        ctx,
        region,
        DOWNLOAD_BUCKET_TASKS,
        &format!("{name}-{revision_id}"),
    )
}

/// How long a download location this service announces stays valid: the model documents a
/// layer archive's link as "valid for 10 minutes", and AWS documents the same span for the
/// presigned URL a deployment package is downloaded from.
pub const DOWNLOAD_URL_EXPIRY: Duration = Duration::from_secs(10 * 60);

/// One announced download URL: the object's own path, presigned against the base URL this
/// emulator is reached on.
///
/// Signed through the framework's URL signer, which is what makes the URL both a presigned
/// one and one this emulator answers: it signs for the emulator's current base URL, so the
/// announced host is the host the caller reached this service on rather than a host only
/// AWS serves.
fn download_location(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
) -> String {
    let path = download_object_path(region, bucket, key);
    let request = smithy_server_runtime::SignedUrlRequest::new(
        smithy_server_runtime::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    );
    match ctx.url_signer().sign(request, DOWNLOAD_URL_EXPIRY) {
        Ok(signed) => signed.as_str().to_string(),
        // Nothing about a stored name or this emulator's own endpoint makes a request
        // unsignable, so this is unreachable in practice; the object is still served at that
        // address, so the address is what is announced rather than no location at all. The
        // endpoint is `scheme://host[:port]` with no path of its own, and a trailing
        // separator is dropped so the object's path is the only one the URL carries.
        Err(_) => format!(
            "{}{path}",
            ctx.endpoint().as_str().trim_end_matches('/')
        ),
    }
}

/// The path one download object is addressed at: its bucket, then its key, as a path-style
/// S3 URL lays them out.
///
/// ONE place states the form, so the URL this service hands out and the request it claims
/// back cannot disagree about it (see [`parse_download_target`]).
#[must_use]
pub fn download_object_path(region: &str, bucket: &str, key: &str) -> String {
    format!(
        "/{bucket}{DOWNLOAD_PATH_PREFIX}/{account}/{key}",
        bucket = download_bucket(region, bucket),
        account = ::engine::emulator_account_id(),
    )
}

/// The bucket label of the per-region store a function's deployment PACKAGE is served
/// from, as [`code_location_url`] spells it.
pub const DOWNLOAD_BUCKET_TASKS: &str = "tasks";

/// The bucket label of the per-region store a LAYER version's archive is served from, as
/// [`layer_archive_location`] spells it.
pub const DOWNLOAD_BUCKET_LAYERS: &str = "layers";

/// The path prefix every download object's key carries, under its bucket.
pub const DOWNLOAD_PATH_PREFIX: &str = "/snapshots";

/// The first label of every bucket Lambda keeps these objects in.
const DOWNLOAD_BUCKET_PREFIX: &str = "awslambda-";

/// The name of the bucket `bucket`'s objects are held in for `region`, which is
/// per-region: a layer archive published into one Region is not an object of another's.
#[must_use]
pub fn download_bucket(region: &str, bucket: &str) -> String {
    format!("{DOWNLOAD_BUCKET_PREFIX}{region}-{bucket}")
}

/// The Region and the bucket label `bucket` names, or `None` when it is not one of the
/// buckets this service publishes objects in.
///
/// The Region is part of the answer because both stores are PER-REGION (see
/// [`download_bucket`]): a layer archive is held under the Region it was published into,
/// so the object a URL names is not resolved by its key alone.
#[must_use]
pub fn download_region_and_bucket(bucket: &str) -> Option<(String, &'static str)> {
    let rest = bucket.strip_prefix(DOWNLOAD_BUCKET_PREFIX)?;
    [DOWNLOAD_BUCKET_TASKS, DOWNLOAD_BUCKET_LAYERS]
        .into_iter()
        .find_map(|label| {
            let region = rest.strip_suffix(&format!("-{label}"))?;
            (!region.is_empty()).then(|| (region.to_string(), label))
        })
}

/// Whether `path` addresses one of the published download stores: its first segment names a
/// bucket this service publishes objects in.
///
/// The BUCKET is what makes a request one of this plane's, rather than the key: a key that
/// bucket holds no object at is then answered as the object it does not hold, rather than as
/// a request naming nothing at all.
#[must_use]
pub fn addresses_download_store(path: &str) -> bool {
    path.strip_prefix('/')
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(bucket, _)| download_region_and_bucket(bucket).is_some())
}

/// What a download URL this service handed out names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadTarget {
    /// A function's deployment package, at the revision the location was reported for.
    FunctionPackage {
        /// The bare function name.
        name: String,
        /// The revision the reported location belongs to, which is the unpublished
        /// configuration's or one published version's.
        revision_id: String,
    },
    /// One layer version's archive.
    LayerArchive {
        /// The Region the archive is held in, which the download bucket names: a layer is
        /// a regional resource, so this is part of WHICH archive is named.
        region: String,
        /// The bare layer name.
        name: String,
        /// The version number.
        version: i64,
    },
}

/// The object a download request names, or `None` when it names none this service
/// publishes.
///
/// Read from the SAME parts the URL is built from — the bucket the path opens with and the
/// key under it — so a URL [`code_location_url`] or [`layer_archive_location`] minted
/// resolves here and nothing else does. Any signature the URL carries is not part of WHICH
/// object it names, so it plays no part here.
#[must_use]
pub fn parse_download_target(path: &str) -> Option<DownloadTarget> {
    let (bucket, rest) = path.strip_prefix('/')?.split_once(DOWNLOAD_PATH_PREFIX)?;
    let (region, bucket) = download_region_and_bucket(bucket)?;
    let (account, key) = rest.strip_prefix('/')?.split_once('/')?;
    if account != ::engine::emulator_account_id() || key.contains('/') {
        return None;
    }
    let key = smithy_server_runtime::percent_decode(key);
    // The key is `<name>-<suffix>`, and a name may itself hold `-`, so the SUFFIX is
    // what is peeled off: a revision id is a UUID, whose rendering is a fixed width, and
    // a layer version is the decimal number the archive location carried.
    match bucket {
        DOWNLOAD_BUCKET_TASKS => {
            let split = key.len().checked_sub(UUID_RENDERED_LEN)?;
            // `get` rather than `split_at`: the key is UNTRUSTED input, and a byte offset
            // that falls inside a multi-byte character is not a split point — it answers
            // `None` here instead of panicking.
            let (name, revision_id) = (key.get(..split)?, key.get(split..)?);
            if ::uuid::Uuid::parse_str(revision_id).is_err() {
                return None;
            }
            Some(DownloadTarget::FunctionPackage {
                name: name.strip_suffix('-')?.to_string(),
                revision_id: revision_id.to_string(),
            })
        }
        _ => {
            let (name, version) = key.rsplit_once('-')?;
            Some(DownloadTarget::LayerArchive {
                region,
                name: name.to_string(),
                version: version.parse::<i64>().ok().filter(|version| *version > 0)?,
            })
        }
    }
}

/// How many characters a UUID's canonical rendering takes, which is the width
/// [`mint_revision_id`] produces and so the width a download key's revision suffix has.
const UUID_RENDERED_LEN: usize = 36;
