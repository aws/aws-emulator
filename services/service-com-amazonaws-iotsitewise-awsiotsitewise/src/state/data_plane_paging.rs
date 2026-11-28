// SPDX-License-Identifier: CC0-1.0

//! What the DATA-PLANE reads of this service share about paging: how a `nextToken`
//! they cannot resolve is refused, and how one page of a BATCH read is cut across
//! the entries of its request.
//!
//! ONE place for both, so the readers that page a time range
//! (`GetAssetPropertyValueHistory`, `GetAssetPropertyAggregates`,
//! `GetInterpolatedAssetPropertyValues`) and the ones that page a batch of entries
//! (`BatchGetAssetPropertyValue`, `BatchGetAssetPropertyValueHistory`,
//! `BatchGetAssetPropertyAggregates`) cannot drift in what they tell a caller about
//! a token, or in which results a page of a batch carries.

use super::*;

/// What a data-plane read answers for a `nextToken` it cannot resolve at all: text
/// no read of this service issued as a token.
///
/// The message is entirely FIXED — it names neither the token nor anything else of
/// the request — so a caller that keys on it reads the same sentence whatever it
/// sent.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid next token provided";

/// What a data-plane read answers for a token it DID issue that is replayed with a
/// different query: the token names a position in another enumeration than the one
/// this request asks for.
///
/// A different refusal from [`INVALID_NEXT_TOKEN_MESSAGE`] because the remedies
/// differ: a caller that changed the query mid-walk restarts the walk, where a
/// caller holding text the service never issued has nothing to resume.
pub const NEXT_TOKEN_FILTER_MISMATCH_MESSAGE: &str =
    "The provided next token does not match query filtering parameters";

/// Which of the two refusals above `token` earns, given that the read could not
/// resolve it against the enumeration `scope` identifies.
///
/// A token of this service is the hex of a resume point beside the
/// [`pagination_scope`] it was minted in ([`encode_scoped_token`]), and a scope is
/// a fixed-width lowercase hex digest. So text that spells that form and carries
/// ANOTHER scope is a token this service issued for another query — the
/// filter-mismatch refusal — and everything else (text that is not one of this
/// service's tokens, or one carrying THIS scope whose resume point the enumeration
/// no longer holds) is the unissued refusal.
pub fn next_token_refusal_message(token: &str, scope: &str) -> &'static str {
    if issued_for_another_scope(token, scope) {
        NEXT_TOKEN_FILTER_MISMATCH_MESSAGE
    } else {
        INVALID_NEXT_TOKEN_MESSAGE
    }
}

/// Whether `token` is one of this service's tokens minted in a DIFFERENT
/// enumeration than `scope`.
fn issued_for_another_scope(token: &str, scope: &str) -> bool {
    let Some(decoded) = decode_token_text(token) else {
        return false;
    };
    let Some((_, carried)) = decoded.rsplit_once(TOKEN_SCOPE_SEPARATOR) else {
        return false;
    };
    carried != scope
        && carried.len() == PAGINATION_SCOPE_LENGTH
        && carried
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
}

/// Where a page of a batch read that pages RESULTS resumes: how many of each
/// entry's results the earlier pages already carried, one count per entry of the
/// request, in order.
///
/// Every entry of a batch contributes to the pages from the first page on, under
/// one `maxResults` budget spread across them ([`batch_page`]), so a resume point
/// is a position in EVERY entry rather than one entry's index: a one-point entry
/// is answered on the first page and reported finished on the later ones, instead
/// of waiting for the entries before it to drain.
///
/// A cursor is a position in the entry list ONE operation was sent, so the token
/// carries the [`pagination_scope`] of that operation and that list beside the
/// positions: honoured against another list it would report that list's entries as
/// already processed and drop their data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEntryCursor {
    /// How many results of each entry previous pages already reported, in the
    /// order the request lists the entries.
    pub served: Vec<usize>,
}

impl BatchEntryCursor {
    /// The cursor of a request that carries no `nextToken`: nothing of any of its
    /// `entries` has been reported yet.
    pub fn start(entries: usize) -> Self {
        Self {
            served: vec![0; entries],
        }
    }

    /// This cursor as a `nextToken` of the `operation` request whose entry list is
    /// `entries`: hex, so the token stays opaque to the caller and inside the
    /// `^[A-Za-z0-9+/=]+$` pattern the paging members of this service carry.
    pub fn encode(&self, operation: &str, entries: &[impl ::std::fmt::Debug]) -> String {
        let resume = self
            .served
            .iter()
            .map(usize::to_string)
            .collect::<Vec<String>>()
            .join(BATCH_ENTRY_SEPARATOR);
        encode_scoped_token(&resume, &batch_entries_scope(operation, entries))
    }

    /// The cursor `token` encodes for the `operation` request whose entry list is
    /// `entries`, or `None` for text this service never issued as a token OF THAT
    /// LIST — which its caller answers as the modelled refusal for an invalid
    /// `nextToken`.
    pub fn decode(
        token: &str,
        operation: &str,
        entries: &[impl ::std::fmt::Debug],
    ) -> Option<Self> {
        let resume = decode_scoped_token(token, &batch_entries_scope(operation, entries))?;
        let served: Vec<usize> = resume
            .split(BATCH_ENTRY_SEPARATOR)
            .map(token_number)
            .collect::<Option<Vec<usize>>>()?;
        (served.len() == entries.len()).then_some(Self { served })
    }
}

/// What joins one entry's resume point to the next in a token
/// [`BatchEntryCursor::encode`] builds. Neither a resume point nor the scope the
/// shared codec appends can contain it.
const BATCH_ENTRY_SEPARATOR: &str = ",";

/// What one page of a batch read does with one entry of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPage {
    /// Report `[from, to)` of this entry's results on this page, as a success
    /// entry. An entry no page has reported yet is served even when the span is
    /// empty: the entry WAS processed, and an empty answer is not a failure.
    Served {
        /// Index of the first result of this entry this page carries.
        from: usize,
        /// Index one past the last result this page carries.
        to: usize,
    },
    /// Every result of this entry was carried by an earlier page: it is "not
    /// processed by this batch request, because [it] had been completely processed
    /// by previous paginated requests" — a skipped entry whose completion status
    /// is SUCCESS.
    Finished,
    /// This entry cannot be served at all: its own error on the first page, and a
    /// skipped entry whose completion status is ERROR on every later one.
    Failed,
    /// This page's budget is spent before this entry's turn; a later page carries
    /// it, so this page reports it nowhere.
    Deferred,
}

/// One page of a batch read: what to do with each entry, and where the next page
/// resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPage {
    /// One decision per entry of the request, in order.
    pub entries: Vec<EntryPage>,
    /// Where the next page resumes, or `None` when this page exhausts every
    /// entry — "the token for the next set of results, or null if there are no
    /// additional results".
    pub next: Option<BatchEntryCursor>,
}

/// The page a batch read answers with: `budget` results spread ACROSS the entries
/// that still have some, resuming each where `cursor` says earlier pages left it.
///
/// `available` holds one element per entry of the request, in order: how many
/// results that entry has in total, or `None` for an entry that cannot be served.
/// `resumed` says whether the request carried a `nextToken`, which is what
/// separates an entry's FIRST appearance (reported, even with nothing to carry)
/// from one an earlier page already finished (reported skipped).
///
/// The budget is handed out one result at a time, each going to the unfinished
/// entry the pages have reported LEAST of so far — ties in the order the request
/// lists them — so every entry of a batch contributes from the first page on and an
/// entry with few results finishes early instead of waiting for the entries listed
/// before it to drain, whatever the page size. `None` when `cursor` resumes an
/// entry past the results it has — a position the entries of this request cannot
/// have produced, which its caller refuses rather than clamps, since clamping would
/// silently repeat or drop results.
pub fn batch_page(
    available: &[Option<usize>],
    cursor: &BatchEntryCursor,
    budget: usize,
    resumed: bool,
) -> Option<BatchPage> {
    if cursor.served.len() != available.len() {
        return None;
    }
    for (count, served) in available.iter().zip(cursor.served.iter()) {
        if count.is_some_and(|count| *served > count) {
            return None;
        }
    }

    let mut taken = vec![0usize; available.len()];
    for _ in 0..budget {
        // The entry the fewest results of which have been reported, counting what
        // earlier pages carried and what this page has taken so far.
        let next = available
            .iter()
            .enumerate()
            .filter(|(at, count)| {
                count.is_some_and(|count| cursor.served[*at] + taken[*at] < count)
            })
            .min_by_key(|(at, _)| cursor.served[*at] + taken[*at])
            .map(|(at, _)| at);
        match next {
            Some(at) => taken[at] += 1,
            // Nothing is left to report, however much budget remains.
            None => break,
        }
    }

    let mut next = cursor.clone();
    let entries: Vec<EntryPage> = available
        .iter()
        .enumerate()
        .map(|(at, count)| {
            let Some(count) = *count else {
                return EntryPage::Failed;
            };
            let from = cursor.served[at];
            next.served[at] = from + taken[at];
            if taken[at] > 0 {
                EntryPage::Served {
                    from,
                    to: from + taken[at],
                }
            } else if from < count {
                EntryPage::Deferred
            } else if resumed {
                EntryPage::Finished
            } else {
                // An entry whose answer is empty, reported on the page that first
                // processes it rather than held back as unfinished.
                EntryPage::Served { from, to: from }
            }
        })
        .collect();

    let more = available
        .iter()
        .zip(next.served.iter())
        .any(|(count, served)| count.is_some_and(|count| *served < count));
    Some(BatchPage {
        entries,
        next: more.then_some(next),
    })
}
