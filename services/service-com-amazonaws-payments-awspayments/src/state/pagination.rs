// SPDX-License-Identifier: CC0-1.0

//! The opaque `nextToken` the three paginated operations share.
//!
//! `ListPaymentInstruments`, `ListFinancialRecords` and `GetAccountBalance` all
//! document the same contract for it: "the pagination token from the `nextToken` of
//! the previous response. Omit it to get the first page. Tokens expire. If the token
//! is malformed or expired, or doesn't match the original request, the request fails
//! with `ValidationException` and the reason `INVALID_PAGINATION_TOKEN`." One
//! implementation serves all three, so their `nextToken` semantics cannot drift
//! apart.
//!
//! The token itself is minted and read by [`service_common::page_tokens`], the one
//! way the emulator's services tell a token they ISSUED from one a caller built: the
//! cursor travels inside the token under an HMAC the issuing store holds the key for
//! ([`service_common::page_tokens::PageTokenKey`]), so a caller cannot name a
//! position this service never offered, and a token minted by another store — another
//! emulator process, a restarted one — names no position either. What THIS module
//! adds is the cursor's own two parts: which listing, under which filters and
//! ordering, the token was minted for ([`scope_digest`] of a query scope), and the
//! position in that listing's results the next page starts at. The scope is what
//! makes "doesn't match the original request" decidable — a token minted for one
//! filter set does not resume another one, so a caller cannot page half of one
//! listing into the middle of a different one.

use service_common::page_tokens::{page_cursor, page_token, scope_digest, PageTokenKey};

/// The byte the cursor's two parts are separated by, and which no part contains: the
/// scope digest is hexadecimal and the position decimal.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The byte the parts of a query SCOPE are separated by, so that two different
/// queries cannot describe themselves with the same text.
const SCOPE_SEPARATOR: char = '\u{1e}';

/// The text describing one query: which listing, under which filters and ordering, a
/// token was minted for.
///
/// `parts` must start with the operation's name and then state every request member
/// that selects or orders the results, each in a canonical form, so that two requests
/// describe the same scope exactly when they ask the same question.
///
/// The page SIZE is deliberately not part of it: `maxResults` is "the maximum number
/// of results to return in one page", a property of the response and not of the
/// query, so a caller may ask for a different page size as it pages.
pub fn query_scope(parts: &[&str]) -> String {
    parts.join(&SCOPE_SEPARATOR.to_string())
}

/// The request named a `nextToken` this listing cannot resume from.
///
/// Carries no detail because the three operations state it identically, in the one
/// modelled form their models give it: a `ValidationException` whose `reason` is
/// [`Self::REASON`] and whose `message` is [`Self::MESSAGE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPaginationToken;

impl InvalidPaginationToken {
    /// `ValidationExceptionReason$INVALID_PAGINATION_TOKEN`.
    pub const REASON: &'static str = "INVALID_PAGINATION_TOKEN";

    /// What that reason means, as the model documents it — a token that cannot be
    /// read, has expired, or was minted for another request, and what the caller
    /// does about it.
    pub const MESSAGE: &'static str = "The nextToken is malformed or expired, or doesn't match the original request. Start again from the first page.";
}

/// One page of `items`: at most `page_size` of them starting where `next_token` left
/// off, and the token the NEXT page resumes from when more remain.
///
/// `next_token` of `None` is the first page. The answer carries a token only when
/// items remain after this page, which is how a caller "finds out whether more
/// results are available".
///
/// `key` is the issuing store's own ([`crate::state::ComAmazonawsPaymentsAwspaymentsState`]),
/// and `scope` the text [`query_scope`] built for this request.
pub fn page<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    page_size: usize,
    key: &PageTokenKey,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), InvalidPaginationToken> {
    let start = match next_token {
        None => 0,
        Some(token) => resume_position(token, key, scope)?,
    };
    // At least one item per page, whatever arrives: a page of none would hand back a
    // token that makes no progress, so a caller paging the listing would never reach
    // its end. The model's own `maxResults` range starts at 1.
    let end = start.saturating_add(page_size.max(1)).min(items.len());
    let page = items.get(start..end).unwrap_or_default().to_vec();
    let next = (end < items.len()).then(|| mint(key, scope, end));
    Ok((page, next))
}

/// The token a page of `scope` ending at `position` hands back.
fn mint(key: &PageTokenKey, scope: &str, position: usize) -> String {
    page_token(
        key,
        &format!("{}{CURSOR_SEPARATOR}{position}", scope_digest(scope)),
    )
}

/// The position `token` resumes `scope` at.
///
/// Three ways a value is refused, which is every way the model names: it is not a
/// token this store minted (`page_cursor` answers `None` — malformed, tampered with,
/// or minted by another store), it was minted for another query (its scope digest is
/// not this request's), or its position is not a position at all.
fn resume_position(
    token: &str,
    key: &PageTokenKey,
    scope: &str,
) -> Result<usize, InvalidPaginationToken> {
    let cursor = page_cursor(key, token).ok_or(InvalidPaginationToken)?;
    let (minted_for, position) = cursor
        .split_once(CURSOR_SEPARATOR)
        .ok_or(InvalidPaginationToken)?;
    if minted_for != scope_digest(scope) {
        return Err(InvalidPaginationToken);
    }
    position.parse().map_err(|_| InvalidPaginationToken)
}
