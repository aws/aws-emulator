// SPDX-License-Identifier: CC0-1.0

//! The paged walk every list operation of this service answers from, in ONE place.
//!
//! Every `List…` of this API is `@paginated` over the same three members — `MaxResults`
//! (range 1..100), an opaque `NextToken` in and the same `NextToken` out
//! (`smithy_query(query="operation",operation="ListUsers")` and the sibling listings) —
//! so they all answer one rule: a page is at most `MaxResults` consecutive records of the
//! listing's own order, a request resumes at the record its `NextToken` names, a token
//! this listing cannot resume at is refused, and a `NextToken` is minted only when a
//! record follows the page.
//!
//! [`encode_cursor`]/[`decode_cursor`] already held the token CODEC for all of them; this
//! module holds the WALK that uses it, so the listings cannot drift in how they bound a
//! page, resume one or mint the next cursor. What stays per operation is only what the
//! listing is OF: its records, the id that names one, how a record renders on the wire,
//! and the sentence a token it cannot resume at is refused with.

use super::{decode_cursor, encode_cursor, validation, DEFAULT_MAX_RESULTS, MAX_MAX_RESULTS};
use crate::ops::types::ValidationException;

/// One page of a listing: the records it reports, rendered, and the cursor the page after
/// it resumes at — `None` when this page ends the walk.
#[derive(Debug)]
pub struct Page<Item> {
    /// The rendered records of this page, in the listing's order.
    pub items: Vec<Item>,
    /// The `NextToken` the next page is asked for with, when a record follows this page.
    pub next_token: Option<String>,
}

/// The page of `records` a request for `max_results` records resuming at `next_token`
/// reports, each rendered by `render`.
///
/// `id_of` names a record — the value a cursor into this listing carries — and
/// `unresumable_token_message` is what a `NextToken` this listing cannot resume at is
/// refused with, which is the only request-visible difference between the listings.
///
/// An absent `MaxResults` asks for the largest page the service answers with
/// ([`DEFAULT_MAX_RESULTS`] — the `@default` four of the five listings declare, and the
/// page size the fifth, whose member declares none, uses), and a value outside the
/// modelled 1..100 range is refused rather than coerced into it: that constraint is
/// checked at the request boundary before any handler runs, so this cannot be reached
/// from the wire today, and refusing keeps it a refusal rather than a silently resized
/// page if it ever is.
pub fn page_of<Record, Item>(
    records: &[Record],
    max_results: Option<i32>,
    next_token: Option<&str>,
    unresumable_token_message: &str,
    id_of: impl Fn(&Record) -> &str,
    render: impl FnMut(&Record) -> Item,
) -> Result<Page<Item>, ValidationException> {
    let requested = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
    if !(1..=MAX_MAX_RESULTS).contains(&requested) {
        return Err(validation(format!(
            "MaxResults must be between 1 and {MAX_MAX_RESULTS}, got {requested}"
        )));
    }
    let page_size = requested as usize;

    // A `NextToken` is this service's own opaque cursor: it names the record the next page
    // starts at. One this service did not mint, or one naming a record this listing does
    // not reach, is refused rather than silently restarting the walk.
    let start = match next_token {
        None => 0,
        Some(token) => decode_cursor(token)
            .and_then(|resource_id| {
                records
                    .iter()
                    .position(|record| id_of(record) == resource_id)
            })
            .ok_or_else(|| validation(unresumable_token_message))?,
    };

    let items: Vec<Item> = records[start..].iter().take(page_size).map(render).collect();
    // A `NextToken` only when something is left: the record the next page starts at.
    let next_token = records
        .get(start + items.len())
        .map(|record| encode_cursor(id_of(record)));

    Ok(Page { items, next_token })
}
