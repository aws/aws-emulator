// SPDX-License-Identifier: CC0-1.0

//! What an `s3://` URI a caller supplied NAMES, and whether this emulator's own Amazon S3
//! HOLDS it — the sibling of [`crate::s3_bucket_name`] (which name S3 holds a bucket
//! under), of [`crate::s3_client`] (the hop these questions are asked over) and of
//! [`crate::s3_object_read`] (what the store's answer about an object established).
//!
//! Several emulated services take an S3 location as an input member and then do work that
//! READS the object there or WRITES into that bucket — a sample a job learns from, an
//! asset to process, the place a job's result lands. A location the account's object
//! storage does not hold describes work that cannot be carried out, so a service that
//! refuses such a submission has to ask the store, and asking means two things stated
//! once:
//!
//! * what the URI NAMES ([`object_address`], [`bucket_name`]) — the bucket, which is the
//!   authority the maintained URL parser reports for a caller-supplied URI rather than
//!   anything split by hand, and the key, which is the raw text after that authority
//!   because an Amazon S3 key is a byte string and not a URL path;
//! * whether the store HOLDS it ([`object_is_absent`], [`bucket_is_absent`]) — answered by
//!   Amazon S3 itself over the in-process hop, so absence is what the STORE said and never
//!   what an unanswered call implied. With no object store mounted to ask, nothing is
//!   established and nothing is the caller's fault. The Region that hop is addressed with
//!   is held to the Region-label grammar ([`crate::arn::region_label`]), and nothing else
//!   of the request reaches the address at all.
//!
//! Each service keeps only the envelope its own API refuses with (the status, the error
//! shape and the wording); nothing about which locations exist is decided per service.
//!
//! The per-service copies that predate this module are the ones to be RETIRED onto it, each
//! in the change that owns its service: the `s3://` readings in
//! `service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice`
//! (`state::ingestion_locations`, which asks about a bucket alone for the ingestion job's
//! own recorded refusal) and
//! `service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice`
//! (`state::s3_object_address`/`s3_bucket_and_tail`, with its own reading of an absent
//! object), the splitters in `service-com-amazonaws-bedrockagent-…`
//! (`state::pushed_documents`), `service-com-amazonaws-entityresolution-…`
//! (`state::matching_run`), `service-com-amazonaws-emrserverless-…`
//! (`state::job_run_logs`), `service-com-amazonaws-transcribe-…` (`state::jobs`),
//! `service-com-amazonaws-athena-…` (`state::notebooks`) and
//! `service-com-amazonaws-route53resolver-…` (`state::domain_imports`), and the
//! bucket-existence reads in `service-com-amazonaws-datasync-…` (`state::transfer`) and
//! `service-com-amazonaws-cloudwatchlogs-…` (`state::export_destination`). Nothing NEW
//! should restate either reading — call this module.

use ::smithy_server_runtime::EmbeddedEmulator;

/// The scheme an S3 location is written with.
///
/// Public so the readers that accept the `s3://` spelling BESIDE another one
/// ([`crate::feature_store_offline_store`], whose member admits an `https://` endpoint URL
/// too) name the same scheme as this module rather than their own copy of it.
pub const S3_SCHEME: &str = "s3";

/// The schemes an Amazon S3 ENDPOINT URL is written with, which a member admitting "the S3,
/// HTTP, or HTTPS URL" of an object carries beside the `s3://` spelling.
const ENDPOINT_SCHEMES: [&str; 2] = ["https", "http"];

/// The service label an Amazon S3 endpoint's host carries.
const ENDPOINT_LABEL: &str = "s3";

/// What separates a VIRTUAL-HOSTED bucket from the endpoint in a host, where the endpoint
/// spells the Region with a `.` (`<bucket>.s3.<region>.amazonaws.com`), and the same for the
/// legacy `-` spelling (`<bucket>.s3-<region>.amazonaws.com`).
const HOST_LABEL_SEPARATORS: [&str; 2] = [".s3.", ".s3-"];

/// The bucket a VIRTUAL-HOSTED Amazon S3 endpoint host names — the label ahead of the
/// service name in `<bucket>.s3.<region>.amazonaws.com` — or `None` when the host carries
/// no bucket.
///
/// Public, and the ONE reading of such a host, because a member that admits an endpoint URL
/// has to recover the same bucket from it whichever service reads it. The host is only ever
/// read FOR the bucket and then dropped: every call this emulator makes about the object
/// goes over the in-process hop ([`crate::s3_client`]), whose address comes from the Region
/// alone, so no part of a caller's URL is ever a destination.
#[must_use]
pub fn endpoint_host_bucket(host: &str) -> Option<&str> {
    HOST_LABEL_SEPARATORS
        .into_iter()
        .find_map(|separator| host.split_once(separator))
        .map(|(bucket, _endpoint)| bucket)
        .filter(|bucket| !bucket.is_empty())
}

/// Whether `host` is an Amazon S3 ENDPOINT host itself, which is how a PATH-STYLE URL
/// addresses a bucket: the bucket is then the first segment of the path rather than a label
/// of the host.
#[must_use]
pub fn host_is_endpoint(host: &str) -> bool {
    host == ENDPOINT_LABEL
        || host.starts_with(&format!("{ENDPOINT_LABEL}."))
        || host.starts_with(&format!("{ENDPOINT_LABEL}-"))
}

/// The bucket and the key the URL `url` names, in any spelling a member admitting "the S3,
/// HTTP, or HTTPS URL" of a media file or an asset carries, or `None` when it names no
/// object: the `s3://` URI ([`object_address`]), or an Amazon S3 endpoint URL, virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<key>`) or path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<key>`).
///
/// The ONE reading of that member, so the same object is reached whichever spelling a caller
/// wrote and whichever service reads it. Each spelling keeps its own key rule, which is the
/// difference between the two: an `s3://` URI's key is the RAW text after the authority (see
/// [`split`]), while an endpoint URL's key is its path percent-DECODED
/// ([`decoded_key`]) — in a URL the escaping is the URL's, and what it spells is a key.
#[must_use]
pub fn url_object_address(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    let parsed = ::url::Url::parse(trimmed).ok()?;
    if parsed.scheme() == S3_SCHEME {
        return object_address(trimmed);
    }
    if !ENDPOINT_SCHEMES.contains(&parsed.scheme()) {
        return None;
    }
    let host = parsed.host_str()?;
    let path = decoded_key(parsed.path().trim_start_matches('/'));
    let (bucket, key) = match endpoint_host_bucket(host) {
        // Virtual-hosted: the whole path is the key.
        Some(bucket) => (bucket.to_string(), path),
        // Path-style: the first segment is the bucket and the rest is the key.
        None if host_is_endpoint(host) => {
            let (bucket, key) = path.split_once('/')?;
            (bucket.to_string(), key.to_string())
        }
        None => return None,
    };
    (!key.is_empty() && crate::s3_bucket_name::is_well_formed(&bucket)).then_some((bucket, key))
}

/// What separates a URI's scheme from its authority.
const SCHEME_SEPARATOR: &str = "://";

/// The bucket and the key the `s3://` URI `s3_uri` names, or `None` when it names no
/// object: another scheme, no bucket, a name Amazon S3 holds no bucket under
/// ([`crate::s3_bucket_name::is_well_formed`]), or a bucket with nothing under it.
///
/// A URI naming a bucket and no key (`s3://bucket`, `s3://bucket/`) addresses a PLACE
/// rather than an object, which is [`bucket_name`]'s question.
#[must_use]
pub fn object_address(s3_uri: &str) -> Option<(String, String)> {
    let (bucket, key) = split(s3_uri)?;
    (!key.is_empty()).then_some((bucket, key))
}

/// The bucket the `s3://` URI `s3_uri` names, with or without anything under it, or `None`
/// when it names no bucket Amazon S3 holds one under.
#[must_use]
pub fn bucket_name(s3_uri: &str) -> Option<String> {
    split(s3_uri).map(|(bucket, _)| bucket)
}

/// The bucket and the key path the `s3://` URI `s3_uri` names, where the key MAY be empty,
/// or `None` when it names no bucket Amazon S3 holds one under.
///
/// What to read for the PLACE a job WRITES into, which is a bucket and the path under it
/// rather than one object: a location naming the bucket alone (`s3://bucket`) is the bucket
/// root, which [`object_address`] answers `None` for because it names no object. Same one
/// reading as its two siblings ([`split`]), so a destination and a source are resolved
/// alike.
#[must_use]
pub fn bucket_and_prefix(s3_uri: &str) -> Option<(String, String)> {
    split(s3_uri)
}

/// The ONE reading of an `s3://` URI: its bucket, and the key under it, which may be empty.
///
/// The URI's SHAPE is read by the maintained URL parser — a caller-supplied URI is not
/// split by hand — and the bucket is the authority that parser reports. The KEY is then
/// taken from the RAW text after that authority, because an Amazon S3 key is a byte string
/// rather than a URL path: a key carrying `%` means that character (decoding it would name
/// a different object), and one carrying `?` or `#` is still part of the key, where a URL
/// reader would take the rest as a query or a fragment and drop it. Anything between the
/// authority and the key that is not the one `/` separating them — a port, user
/// information — is no S3 location at all.
fn split(s3_uri: &str) -> Option<(String, String)> {
    let trimmed = s3_uri.trim();
    let url = ::url::Url::parse(trimmed).ok()?;
    if url.scheme() != S3_SCHEME {
        return None;
    }
    let bucket = url.host_str()?;
    if !crate::s3_bucket_name::is_well_formed(bucket) {
        return None;
    }
    // The authority is `[a-z0-9.-]` by the rule above, which the URL parser neither escapes
    // nor rewrites, so it stands in the raw text exactly as it was reported.
    let after_authority = trimmed
        .get(S3_SCHEME.len() + SCHEME_SEPARATOR.len()..)?
        .strip_prefix(bucket)?;
    let key = match after_authority {
        "" => "",
        rest => rest.strip_prefix('/')?,
    };
    Some((bucket.to_string(), key.to_string()))
}

/// A URI path as the Amazon S3 KEY it spells: percent-decoded, since a URL path's escaping
/// is the URL's while what it names is a key, whose own character set is wider.
///
/// Public for the same reason [`S3_SCHEME`] is: the readers that accept the `s3://` spelling
/// beside another one resolve a path's escapes through this one function.
#[must_use]
pub fn decoded_key(path: &str) -> String {
    ::percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .into_owned()
}

/// Whether the OBJECT `s3_uri` names is one nothing holds: a URI that names no object at
/// all ([`object_address`] — there is nothing for any store to hold), or one this
/// emulator's own Amazon S3 ANSWERED that it holds no object at.
///
/// The answer is read by [`crate::s3_object_read::answered_refusal`], so only the store
/// itself saying there is nothing there counts: a call nothing answered — no object store
/// mounted, or a call that could not be dispatched — establishes nothing, and a store
/// answering something else about the object (a denied read, say) has not said the object
/// is missing.
pub async fn object_is_absent(callback: &EmbeddedEmulator, region: &str, s3_uri: &str) -> bool {
    let Some((bucket, key)) = object_address(s3_uri) else {
        return true;
    };
    let Some(region) = crate::arn::region_label(region) else {
        return false;
    };
    // A HEAD, not a GET: what is asked is whether the store holds an object there, so none
    // of its bytes are read. The hop is the shared in-process one — nothing outside this
    // process is reachable through it.
    match crate::s3_client::s3_client(callback, region)
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        Ok(_) => false,
        Err(error) => {
            crate::s3_object_read::answered_refusal(&error) == crate::s3_object_read::ObjectRead::Absent
        }
    }
}

/// Whether the BUCKET `s3_uri` names is one nothing holds: a URI that names no bucket at
/// all ([`bucket_name`]), or one this emulator's own Amazon S3 ANSWERED that it does not
/// hold.
///
/// What to ask about the place a job WRITES: the object under it is not there yet, so the
/// bucket is what has to be. The question itself is the tree's one reading of it
/// ([`crate::s3_client::bucket_is_absent_over`]), so a destination named by a URI and one
/// named by a bucket name cannot be resolved differently; same grounding as
/// [`object_is_absent`].
pub async fn bucket_is_absent(callback: &EmbeddedEmulator, region: &str, s3_uri: &str) -> bool {
    let Some(bucket) = bucket_name(s3_uri) else {
        return true;
    };
    let Some(region) = crate::arn::region_label(region) else {
        return false;
    };
    crate::s3_client::bucket_is_absent_over(callback, region, &bucket).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: the case, the URI, the object address it names and the
    /// bucket it names.
    type Row = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
        Option<&'static str>,
    );

    /// RULE: an `s3://` URI names an object when its authority is a name Amazon S3 holds a
    /// bucket under and something follows it; it names a place when the authority alone is
    /// such a name.
    ///
    /// The rows span the scheme, the authority (a well-formed name, each class of name the
    /// published rules refuse, a port, and none at all), and the key (absent, empty, nested,
    /// with a trailing separator, and carrying each character a URL reader would read as
    /// something other than key text).
    #[test]
    fn an_s3_uri_names_the_bucket_and_key_it_spells() {
        let rows: &[Row] = &[
            (
                "an object under a bucket",
                "s3://bda-samples/assets/a.pdf",
                Some(("bda-samples", "assets/a.pdf")),
                Some("bda-samples"),
            ),
            (
                "a prefix-shaped key",
                "s3://bda-samples/out/",
                Some(("bda-samples", "out/")),
                Some("bda-samples"),
            ),
            (
                "a key carrying a percent sign, which is that character of the key",
                "s3://bda-samples/100%25-done/a.pdf",
                Some(("bda-samples", "100%25-done/a.pdf")),
                Some("bda-samples"),
            ),
            (
                "a key carrying a question mark, which a URL reader would take as a query",
                "s3://bda-samples/report?draft=1.json",
                Some(("bda-samples", "report?draft=1.json")),
                Some("bda-samples"),
            ),
            (
                "a key carrying a hash, which a URL reader would take as a fragment",
                "s3://bda-samples/notes#2.json",
                Some(("bda-samples", "notes#2.json")),
                Some("bda-samples"),
            ),
            (
                "a key written with the characters it spells",
                "s3://bda-samples/Odd Key_1/ASSET.PDF",
                Some(("bda-samples", "Odd Key_1/ASSET.PDF")),
                Some("bda-samples"),
            ),
            (
                "an authority carrying a port, which addresses no S3 location",
                "s3://bda-samples:443/a.pdf",
                None,
                None,
            ),
            (
                "a bucket with nothing under it",
                "s3://bda-samples",
                None,
                Some("bda-samples"),
            ),
            (
                "a bucket with a bare separator",
                "s3://bda-samples/",
                None,
                Some("bda-samples"),
            ),
            ("another scheme", "https://bda-samples/a.pdf", None, None),
            ("no scheme at all", "bda-samples/a.pdf", None, None),
            ("no authority", "s3:///a.pdf", None, None),
            (
                "an upper-case authority, which S3 holds no bucket under",
                "s3://BDA-Samples/a.pdf",
                None,
                None,
            ),
            (
                "an authority written as an IP address",
                "s3://192.168.5.4/a.pdf",
                None,
                None,
            ),
            (
                "two adjacent periods in the authority",
                "s3://bda..samples/a.pdf",
                None,
                None,
            ),
            (
                "a reserved prefix in the authority",
                "s3://xn--bda-samples/a.pdf",
                None,
                None,
            ),
            (
                "a reserved suffix in the authority",
                "s3://bda-samples-s3alias/a.pdf",
                None,
                None,
            ),
            (
                "an authority one character under the published floor",
                "s3://ab/a.pdf",
                None,
                None,
            ),
        ];
        for (case, uri, address, bucket) in rows {
            let expected = address.map(|(b, k)| (b.to_string(), k.to_string()));
            let actual = object_address(uri);
            assert_eq!(
                actual, expected,
                "case {case:?} ({uri}): expected object address {expected:?}, got {actual:?}"
            );
            let expected = bucket.map(ToString::to_string);
            let actual = bucket_name(uri);
            assert_eq!(
                actual, expected,
                "case {case:?} ({uri}): expected bucket {expected:?}, got {actual:?}"
            );
        }
    }

    /// One row of the URL table below: the case, the URL and the object address it names.
    type UrlRow = (&'static str, &'static str, Option<(&'static str, &'static str)>);

    /// One row of the place table below: the case, the URI and the place it names.
    type PlaceRow = (&'static str, &'static str, Option<(&'static str, &'static str)>);

    /// RULE: the PLACE an `s3://` URI names is its bucket and the path under it, which may be
    /// empty — the bucket root — so a destination naming a bucket alone is a place while
    /// naming no object.
    ///
    /// The rows span the key path (absent, a bare separator, a prefix, a nested prefix, one
    /// naming an object) and the authorities that name no bucket at all.
    #[test]
    fn an_s3_uri_names_the_place_it_spells() {
        let rows: &[PlaceRow] = &[
            (
                "a bucket with nothing under it, which is its root",
                "s3://bda-samples",
                Some(("bda-samples", "")),
            ),
            (
                "a bucket with a bare separator, the same root",
                "s3://bda-samples/",
                Some(("bda-samples", "")),
            ),
            (
                "a prefix under the bucket",
                "s3://bda-samples/out",
                Some(("bda-samples", "out")),
            ),
            (
                "a nested prefix with a trailing separator",
                "s3://bda-samples/out/2026/",
                Some(("bda-samples", "out/2026/")),
            ),
            (
                "a key naming one object, which is a place too",
                "s3://bda-samples/out/a.json",
                Some(("bda-samples", "out/a.json")),
            ),
            (
                "an upper-case authority, which S3 holds no bucket under",
                "s3://BDA-Samples/out",
                None,
            ),
            (
                "an authority carrying a port, which addresses no S3 location",
                "s3://bda-samples:443/out",
                None,
            ),
            ("another scheme", "https://bda-samples/out", None),
            ("no authority", "s3:///out", None),
        ];
        for (case, uri, expected) in rows {
            let expected = expected.map(|(bucket, key)| (bucket.to_string(), key.to_string()));
            let actual = bucket_and_prefix(uri);
            assert_eq!(
                actual, expected,
                "case {case:?} ({uri}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a member admitting "the S3, HTTP, or HTTPS URL" of an object names that object
    /// in any of the spellings Amazon S3 publishes for it — the `s3://` URI, a
    /// virtual-hosted endpoint URL and a path-style one — and each spelling keeps its own
    /// key rule.
    ///
    /// The rows span the scheme, the addressing style (virtual-hosted, path-style, both
    /// Region spellings, the global endpoint), the key (nested, escaped, absent) and the
    /// hosts that are no Amazon S3 endpoint at all.
    #[test]
    fn an_object_url_names_the_bucket_and_key_it_spells() {
        let rows: &[UrlRow] = &[
            (
                "the s3 URI, whose key is the raw text after the authority",
                "s3://bda-samples/in/one%20two.mp4",
                Some(("bda-samples", "in/one%20two.mp4")),
            ),
            (
                "a virtual-hosted endpoint URL",
                "https://bda-samples.s3.us-west-2.amazonaws.com/in/one.mp4",
                Some(("bda-samples", "in/one.mp4")),
            ),
            (
                "a virtual-hosted URL on the global endpoint",
                "https://bda-samples.s3.amazonaws.com/in/one.mp4",
                Some(("bda-samples", "in/one.mp4")),
            ),
            (
                "a virtual-hosted URL spelling the Region the legacy way",
                "https://bda-samples.s3-us-west-2.amazonaws.com/in/one.mp4",
                Some(("bda-samples", "in/one.mp4")),
            ),
            (
                "a path-style endpoint URL",
                "https://s3.us-west-2.amazonaws.com/bda-samples/in/one.mp4",
                Some(("bda-samples", "in/one.mp4")),
            ),
            (
                "a path-style URL over http",
                "http://s3-us-west-2.amazonaws.com/bda-samples/in/one.mp4",
                Some(("bda-samples", "in/one.mp4")),
            ),
            (
                "an endpoint URL whose path is escaped, which spells the key it decodes to",
                "https://s3.us-west-2.amazonaws.com/bda-samples/in%2Fone%20two.mp4",
                Some(("bda-samples", "in/one two.mp4")),
            ),
            (
                "a virtual-hosted URL naming the bucket alone",
                "https://bda-samples.s3.us-west-2.amazonaws.com/",
                None,
            ),
            (
                "a path-style URL naming the bucket alone",
                "https://s3.us-west-2.amazonaws.com/bda-samples",
                None,
            ),
            (
                "a path-style URL whose first segment is no bucket name",
                "https://s3.us-west-2.amazonaws.com/BDA-Samples/in/one.mp4",
                None,
            ),
            (
                "a host that is no Amazon S3 endpoint",
                "https://example.com/in/one.mp4",
                None,
            ),
            (
                "a host whose label only ends in the service name",
                "https://not-s3.amazonaws.com/in/one.mp4",
                None,
            ),
            ("no scheme at all", "bda-samples/in/one.mp4", None),
            ("no URL at all", "", None),
        ];
        for (case, url, expected) in rows {
            let expected = expected.map(|(bucket, key)| (bucket.to_string(), key.to_string()));
            let actual = url_object_address(url);
            assert_eq!(
                actual, expected,
                "case {case:?} ({url}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE, at each published bucket-length bound: a name at the floor or the ceiling
    /// names a bucket, one character past either names none.
    #[test]
    fn the_published_bucket_length_bounds_decide_the_authority() {
        let rows: [(&str, usize, bool); 4] = [
            (
                "one under the floor",
                crate::s3_bucket_name::MIN_BUCKET_NAME_CHARS - 1,
                false,
            ),
            ("the floor", crate::s3_bucket_name::MIN_BUCKET_NAME_CHARS, true),
            (
                "the ceiling",
                crate::s3_bucket_name::MAX_BUCKET_NAME_CHARS,
                true,
            ),
            (
                "one over the ceiling",
                crate::s3_bucket_name::MAX_BUCKET_NAME_CHARS + 1,
                false,
            ),
        ];
        for (case, length, names_one) in rows {
            let uri = format!("s3://{}/a.pdf", "b".repeat(length));
            let actual = object_address(&uri).is_some();
            assert_eq!(
                actual, names_one,
                "case {case:?} ({length} characters): expected {names_one}, got {actual}"
            );
        }
    }
}
