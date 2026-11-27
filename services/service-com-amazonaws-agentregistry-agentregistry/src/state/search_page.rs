// SPDX-License-Identifier: CC0-1.0

//! The page of a ranked search a request is served at, and the opaque `nextToken` that
//! names it.
//!
//! `SearchDiscoverableRegistryRecords` is `@paginated` over the `maxResults` / `nextToken`
//! pair, and that output member's own doc states the two rules this module holds: a page is
//! resumed only by a request that REPEATS the search the token was issued for — "send the
//! same `searchQuery`, `registryIds`, `filters`, and `searchType` that you used for the
//! first page. If any of them differ, the request fails with a `ValidationException`" — and
//! "[a] pagination token expires 15 minutes after the service returns the first page".
//!
//! NOTHING IS STORED. A search is a read, so it may not change what a later caller sees:
//! the token itself carries the position it resumes at, the instant this search's pages
//! stop being resumable, and a DIGEST of the search it belongs to. The digest is what the
//! two rules above are decided by, and it is a digest rather than the parameters themselves
//! so that a token hands no part of a `@sensitive` search query or metadata filter back to
//! whoever holds it.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{Document, Number};
use sha2::{Digest, Sha256};

/// How long a search's tokens name a page of it: `SearchDiscoverableRegistryRecords`'s own
/// `nextToken` doc — "A pagination token expires 15 minutes after the service returns the
/// first page."
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// Version tag inside the opaque token, so a string in any other shape is refused rather
/// than misread as a position in a result set.
const TOKEN_PREFIX: &str = "v1:";

/// The search a token belongs to: the digest of the members a resuming request must repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFingerprint(String);

impl SearchFingerprint {
    /// The fingerprint of one search: its query, the registries it searches, its metadata
    /// filter and the ranking method it asked for — the four members `nextToken` names, and
    /// nothing else, so `maxResults` may differ from page to page as any paginated read
    /// allows.
    pub fn of(
        search_query: &str,
        registry_ids: &[String],
        filters: Option<&Document>,
        search_type: &str,
    ) -> Self {
        let mut canonical = String::new();
        field(&mut canonical, "q", search_query);
        let _ = write!(canonical, "n{};", registry_ids.len());
        for registry_id in registry_ids {
            field(&mut canonical, "r", registry_id);
        }
        field(&mut canonical, "t", search_type);
        match filters {
            Some(filters) => canonical_document(filters, &mut canonical),
            None => canonical.push_str("none;"),
        }
        Self(hex::encode(Sha256::digest(canonical.as_bytes())))
    }

    /// The digest as it is carried inside a token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where in one search's ranked records a request is served, and when that search's pages
/// stop being resumable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// How many of the search's ranked records the earlier pages already served.
    pub served: usize,
    /// The instant the search's tokens expire, in seconds since the Unix epoch. Carried
    /// from page to page, so the lifetime is counted from the FIRST page as documented and
    /// is not extended by walking.
    pub expires_at: u64,
}

/// Why a `nextToken` names no page of the search that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRefusal {
    /// The string is in no shape this service issues, so it names no position at all.
    NotIssued,
    /// The token was issued, for a search asking something else: the resuming request
    /// does not repeat the query, the registries, the filter or the ranking method.
    OtherSearch,
    /// The token was issued for this search, and its documented lifetime has passed.
    Expired,
}

/// The page a request that carries no token is served: the first, with this search's
/// documented lifetime starting now.
pub fn first_page() -> Page {
    first_page_at(now_secs())
}

/// [`first_page`] as of `now`, in seconds since the Unix epoch.
pub fn first_page_at(now: u64) -> Page {
    Page {
        served: 0,
        expires_at: now.saturating_add(TOKEN_LIFETIME.as_secs()),
    }
}

/// The page `token` names for the search fingerprinted by `of`, or why it names none.
pub fn resume(token: &str, of: &SearchFingerprint) -> Result<Page, TokenRefusal> {
    resume_at(token, of, now_secs())
}

/// [`resume`] as of `now`, in seconds since the Unix epoch — which is what the documented
/// expiry is read against.
pub fn resume_at(token: &str, of: &SearchFingerprint, now: u64) -> Result<Page, TokenRefusal> {
    let decoded = hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or(TokenRefusal::NotIssued)?;
    let payload = decoded
        .strip_prefix(TOKEN_PREFIX)
        .ok_or(TokenRefusal::NotIssued)?;
    let mut parts = payload.split(':');
    let (Some(expires_at), Some(served), Some(fingerprint), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(TokenRefusal::NotIssued);
    };
    let (Ok(expires_at), Ok(served)) = (expires_at.parse::<u64>(), served.parse::<usize>()) else {
        return Err(TokenRefusal::NotIssued);
    };
    if fingerprint != of.as_str() {
        return Err(TokenRefusal::OtherSearch);
    }
    // A token names a page only inside the lifetime a first page could have given it: an
    // expiry already passed, and one further out than one whole lifetime from now — which
    // no first page of this search can have issued — are both read as expired, so the
    // documented lifetime bounds every token this read honours.
    if now >= expires_at || expires_at > now.saturating_add(TOKEN_LIFETIME.as_secs()) {
        return Err(TokenRefusal::Expired);
    }
    Ok(Page { served, expires_at })
}

/// The token that resumes the search fingerprinted by `of` after the `served` records its
/// pages have answered with so far, expiring when `page` already does.
///
/// Hex-encoded, as every opaque token of this service is, so a caller reads it as nothing
/// but a cursor.
pub fn token(page: &Page, served: usize, of: &SearchFingerprint) -> String {
    hex::encode(format!(
        "{TOKEN_PREFIX}{}:{served}:{}",
        page.expires_at,
        of.as_str()
    ))
}

/// The current wall-clock instant, in seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One field of the canonical text a fingerprint is taken over: length-prefixed, so no two
/// different searches can write the same text.
fn field(into: &mut String, tag: &str, value: &str) {
    let _ = write!(into, "{tag}{}:{value};", value.len());
}

/// One step of the walk over a filter expression: a value still to write, a member key, or
/// the delimiter that closes a collection.
enum Step<'a> {
    Value(&'a Document),
    Key(&'a str),
    Close(&'static str),
}

/// A filter expression's canonical text: an object's members in KEY ORDER, because a
/// `Document` object is an unordered map — so one filter expression has one text whatever
/// order a request wrote its members in, and a resuming request that repeats its filter is
/// recognised however its JSON was serialized.
///
/// The walk keeps its own work stack ON THE HEAP and never recurses: the expression comes
/// from the request, so how deeply it nests is the caller's to choose, and a walk that grew
/// the thread's stack with it could overflow it — which aborts the process rather than
/// failing the one request. The heap stack holds at most one entry per value of the
/// expression, which the request body already bounds.
fn canonical_document(document: &Document, into: &mut String) {
    let mut pending = vec![Step::Value(document)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Close(delimiter) => into.push_str(delimiter),
            Step::Key(key) => field(into, "k", key),
            Step::Value(Document::Null) => into.push_str("null;"),
            Step::Value(Document::Bool(flag)) => {
                let _ = write!(into, "b{flag};");
            }
            Step::Value(Document::Number(Number::PosInt(value))) => {
                let _ = write!(into, "u{value};");
            }
            Step::Value(Document::Number(Number::NegInt(value))) => {
                let _ = write!(into, "i{value};");
            }
            Step::Value(Document::Number(Number::Float(value))) => {
                let _ = write!(into, "f{value};");
            }
            Step::Value(Document::String(text)) => field(into, "s", text),
            Step::Value(Document::Array(elements)) => {
                let _ = write!(into, "[{};", elements.len());
                // Pushed in reverse, so the elements are written in their own order and
                // the closing delimiter last.
                pending.push(Step::Close("]"));
                for element in elements.iter().rev() {
                    pending.push(Step::Value(element));
                }
            }
            Step::Value(Document::Object(members)) => {
                let mut keys: Vec<&String> = members.keys().collect();
                keys.sort_unstable();
                let _ = write!(into, "{{{};", keys.len());
                pending.push(Step::Close("}"));
                for key in keys.into_iter().rev() {
                    if let Some(value) = members.get(key) {
                        pending.push(Step::Value(value));
                    }
                    pending.push(Step::Key(key.as_str()));
                }
            }
        }
    }
}
