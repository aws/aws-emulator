// SPDX-License-Identifier: CC0-1.0

//! The opaque `nextToken` every paginated operation of this service hands out and reads
//! back.
//!
//! A token is the hex encoding of this service's layout marker, a digest of the criteria
//! the page was computed over, and the id the page ended on — so a client cannot read a
//! resource id out of it, and the service can both resume from the next id in order and
//! tell whether the token belongs to the request that submitted it. Hex and SHA-256 come
//! from the `hex` and `sha2` crates rather than being hand-rolled, and decoding is TOTAL:
//! a value this service never issued is a refusal ([`PageTokenRejection`]), never a panic.

/// The layout marker every `nextToken` this service issues carries, so a token
/// shaped by an older or a different scheme is recognised as none of this
/// service's.
pub(super) const PAGE_TOKEN_LAYOUT: &str = "1";

/// How many bytes of the request digest a `nextToken` carries — 128 bits of SHA-256,
/// enough that no two requests a caller can build share a digest.
pub(super) const PAGE_TOKEN_DIGEST_BYTES: usize = 16;

/// The refusal for a `nextToken` that is not a pagination token of this service at
/// all: it names no page, so it cannot be read as the first one.
pub const PAGE_TOKEN_MALFORMED_MESSAGE: &str = "nextToken is not a valid pagination token.";

/// The refusal for a well-formed `nextToken` that this service issued for a
/// DIFFERENT request: a continuation token continues the request that produced it,
/// so the only way to use it is to send that request again ("submit the
/// SearchFixtures request a second time, with the same search criteria, and specify
/// the nextToken value", `SearchFixturesRequest$nextToken`).
pub const PAGE_TOKEN_OTHER_REQUEST_MESSAGE: &str =
    "nextToken was issued for a different request. To see the next batch of results, submit the \
     same request again, with the same criteria, and specify the nextToken value.";

/// Why a `nextToken` cannot be used for the request that submitted it.
///
/// Both outcomes are a `ValidationException` (HTTP 400) in every paginated operation
/// of this service; they differ only in what the message tells the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenRejection {
    /// Not a token this service issues: not hex as this service writes hex, or hex
    /// that carries none of the fields a token has.
    Malformed,
    /// A token of this service, issued for another request — another operation,
    /// another endpoint, or other search criteria.
    OtherRequest,
}

impl PageTokenRejection {
    /// The `ValidationException` message this rejection is reported with.
    pub fn message(self) -> &'static str {
        match self {
            Self::Malformed => PAGE_TOKEN_MALFORMED_MESSAGE,
            Self::OtherRequest => PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
        }
    }
}

/// The canonical form of the criteria one page was computed over, built from
/// `fields`.
///
/// Each field is length-prefixed, so no two different field lists share a canonical
/// form however the values they carry are punctuated — which is what makes the
/// digest in a `nextToken` a faithful fingerprint of the request. A caller never
/// sees this string: only its digest travels, inside the token.
pub fn page_criteria(fields: &[&str]) -> String {
    let mut canonical = String::new();
    for field in fields {
        canonical.push_str(&field.len().to_string());
        canonical.push(':');
        canonical.push_str(field);
    }
    canonical
}

/// The criteria a `nextToken` of a listing that enumerates ONE Region's records
/// binds: the operation that issued the token and the Region the request was signed
/// for.
///
/// Stated here once, beside the codec that encodes and decodes the token, so every
/// region-scoped listing of this service binds the same two fields: the service is
/// regional, so another Region's endpoint reports another set of records and a token
/// issued by one does not continue a listing of the other. `operation` is the listing's
/// own name, which is what keeps one listing's token from continuing another's.
pub fn region_page_criteria(operation: &str, region: &str) -> String {
    page_criteria(&[operation, region])
}

/// The criteria a `nextToken` of a listing that enumerates the records of ONE PARENT
/// resource binds: the operation, the Region and the parent's id.
///
/// The parent's counterpart of [`region_page_criteria`], for `ListScheduleActions` (the
/// actions of one feed) and `ListQuestionSetVersions` (the versions of one question set):
/// such a listing's selection is that one parent's records, so a token issued for one
/// parent does not continue a listing of another, any more than one Region's continues
/// another's.
pub fn scoped_page_criteria(operation: &str, region: &str, parent_id: &str) -> String {
    page_criteria(&[operation, region, parent_id])
}

/// The digest of a canonical criteria string ([`page_criteria`]) that a token binds.
///
/// SHA-256 from the `sha2` crate, truncated to [`PAGE_TOKEN_DIGEST_BYTES`]: nothing
/// here hashes by hand, and the digest carries no readable part of the request.
pub(super) fn page_criteria_digest(criteria: &str) -> String {
    use ::sha2::Digest;
    let digest = ::sha2::Sha256::digest(criteria.as_bytes());
    ::hex::encode(&digest[..PAGE_TOKEN_DIGEST_BYTES])
}

/// The bytes one hex field of a `nextToken` carries, or `None` when the field is not
/// hex AS THIS SERVICE WRITES IT.
///
/// A `nextToken` is opaque, so the only tokens there are, are the byte strings the
/// encoder handed out, and the encoder writes one spelling of hex per byte string (two
/// lower-case characters a byte, as `::hex::encode` writes it). A field is therefore
/// read only when
/// it is byte-identical to the encoding of the bytes it decodes to: any other spelling
/// of the same bytes — an upper-case or mixed-case re-spelling — is a string this
/// service never issued in any request, so it names no page. Used for BOTH hex layers
/// of a token, the outer envelope and the criteria digest inside it, so neither layer
/// accepts a spelling the encoder cannot produce.
pub(super) fn token_hex(field: &str) -> Option<Vec<u8>> {
    let bytes = ::hex::decode(field).ok()?;
    (::hex::encode(&bytes) == field).then_some(bytes)
}

/// Encode an opaque `nextToken` that resumes the request whose criteria are
/// `criteria` after `last_id`.
///
/// The token is opaque to the caller: it is the hex encoding of this service's
/// layout marker, the digest of the criteria the page was computed over, and the id
/// the page ended on — so a client cannot read a resource id out of it, and the
/// service can both resume from the next id in order and tell whether the token
/// belongs to the request that submitted it. Hex and SHA-256 come from the `hex` and
/// `sha2` crates rather than being hand-rolled. Shared by every paginated operation
/// of this service, each of which passes the criteria ITS selection depends on.
pub fn encode_page_token(criteria: &str, last_id: &str) -> String {
    ::hex::encode(format!(
        "{PAGE_TOKEN_LAYOUT}:{}:{last_id}",
        page_criteria_digest(criteria)
    ))
}

/// Decode a `nextToken` back into the id a page resumes after, for a request whose
/// criteria are `criteria`.
///
/// [`PageTokenRejection::Malformed`] for a token this service never issues in any
/// request — one that is not hex as this service writes it ([`token_hex`]), or that
/// carries none of a token's fields — and
/// [`PageTokenRejection::OtherRequest`] for one whose digest is another request's;
/// the handlers refuse both as a `ValidationException`. A token that does decode
/// names a POSITION in the id order rather than an item that must still be there, so
/// paging survives an item that disappeared between two pages.
pub fn decode_page_token(criteria: &str, token: &str) -> Result<String, PageTokenRejection> {
    let bytes = token_hex(token).ok_or(PageTokenRejection::Malformed)?;
    let decoded = String::from_utf8(bytes).map_err(|_| PageTokenRejection::Malformed)?;
    // The id a page ends on may itself hold the separator, so it is whatever
    // follows the first two fields.
    let mut fields = decoded.splitn(3, ':');
    let (Some(layout), Some(digest), Some(last_id)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return Err(PageTokenRejection::Malformed);
    };
    if layout != PAGE_TOKEN_LAYOUT
        || last_id.is_empty()
        || token_hex(digest).map(|raw| raw.len()) != Some(PAGE_TOKEN_DIGEST_BYTES)
    {
        return Err(PageTokenRejection::Malformed);
    }
    if digest != page_criteria_digest(criteria) {
        return Err(PageTokenRejection::OtherRequest);
    }
    Ok(last_id.to_string())
}
