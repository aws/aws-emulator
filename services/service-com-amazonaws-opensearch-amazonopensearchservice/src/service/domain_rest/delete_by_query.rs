// SPDX-License-Identifier: CC0-1.0

//! The DELETE BY QUERY API: `POST /{index}/_delete_by_query`, which "deletes documents that
//! match the specified query" — the one way a client empties part of an index without
//! knowing the ids it holds.
//!
//! The documents it deletes are the ones the QUERY selects, resolved through the data
//! plane's own search ([`super::selected_documents`]): the same query vocabulary, the same
//! index-pattern resolution and the same analysis chains, so a delete removes exactly the
//! documents a search with that query reports. The counts it answers are what it really did —
//! the documents the query matched, the ones removed, and the batches that took — and a
//! request naming `max_docs` has that many removed and no more.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Value};

use crate::state::{fgac, ingest, Domain};

use super::{
    error, guarded, ok, wrong_method, Answering, CrossCluster, Request, MAX_RESULT_WINDOW,
};

/// How many batches one request may delete in.
///
/// The engine deletes in batches of its scroll window and keeps going until the query
/// selects nothing more; this is the emulator's own ceiling on how many of those batches ONE
/// request performs, so the work it does is bounded by a constant and not by the documents a
/// pattern resolves to. A request that reaches it reports the batches it really performed and
/// the documents it really deleted, which is what tells a client there is more to delete.
const MAX_BATCHES: usize = 16;

/// `POST /{index}/_delete_by_query`.
pub(super) fn delete_by_query_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    files: &crate::state::analysis::PackageFiles,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_DELETE_BY_QUERY, |domain| {
            delete_by_query(domain, request, index_name, files)
        }),
        _ => wrong_method(request, "POST"),
    }
}

/// Delete every document of `index_name` the request's query selects.
fn delete_by_query(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    files: &crate::state::analysis::PackageFiles,
) -> RtResponse {
    // "The request body must contain a query" — a request carrying none selects nothing to
    // delete, and the engine refuses it rather than deleting the whole index.
    let body = match request.json() {
        Ok(body) => body.unwrap_or(Value::Null),
        Err(refusal) => return *refusal,
    };
    if !body.get(QUERY_MEMBER).is_some_and(Value::is_object) {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("[{QUERY_MEMBER}] required property is missing"),
        );
    }
    let sorting = super::search::Sorting::default();
    let highlighting = super::search::Highlighting::default();
    // "max_docs — the maximum number of documents to delete", which a client sends as a
    // safety limit on a destructive delete: the documents past it are left, and the answer
    // reports what was really deleted.
    let limit = match requested_limit(&body, request) {
        Ok(limit) => limit,
        Err(refusal) => return *refusal,
    };
    let asked = Answering {
        from: 0,
        // One batch is the engine's own result window, which is also the most documents one
        // index of this emulator holds.
        size: MAX_RESULT_WINDOW,
        sorting: &sorting,
        highlighting: &highlighting,
    };
    let mut matched = 0usize;
    let mut deleted = 0usize;
    let mut batches = 0usize;
    // The documents still selected when the loop stopped, which is what says whether the
    // delete finished.
    let mut remaining = 0usize;
    while batches < MAX_BATCHES {
        let selected = match super::selected_documents(
            domain,
            request,
            index_name,
            &CrossCluster::default(),
            files,
            &asked,
        ) {
            Ok(selected) => selected,
            Err(refusal) => return *refusal,
        };
        // The total is read from EVERY batch, since each one is a fresh search over what the
        // indexes hold now: the figure reported is the largest the delete ever saw selected.
        matched = matched.max(selected.0);
        remaining = selected.0;
        if selected.1.is_empty() {
            break;
        }
        batches += 1;
        for selection in selected.1 {
            if deleted >= limit {
                break;
            }
            let (index_of_hit, id, _source) = selection.hit;
            let Some(index) = domain.indexes.get_mut(&index_of_hit) else {
                continue;
            };
            if index.documents.remove(&id).is_some() {
                // A delete is a write, so it takes a sequence number of its own: a client
                // reading the index afterwards can tell the revision it is at.
                let _seq_no = index.take_seq_no();
                deleted += 1;
                remaining = remaining.saturating_sub(1);
            }
        }
        if deleted >= limit {
            break;
        }
    }
    ok(&json!({
        "took": request.took().as_millis(),
        // A delete that stopped with documents it had not reached is one that did not
        // finish, which the engine signals rather than reporting an unqualified success. A
        // delete that stopped because the request's own `max_docs` was reached DID finish
        // what it was asked for.
        "timed_out": remaining > 0 && deleted < limit,
        "total": matched,
        "deleted": deleted,
        "batches": batches,
        "version_conflicts": 0,
        "noops": 0,
        "retries": { "bulk": 0, "search": 0 },
        "throttled_millis": 0,
        "requests_per_second": UNTHROTTLED_REQUESTS_PER_SECOND,
        "throttled_until_millis": 0,
        "failures": Vec::<Value>::new(),
    }))
}

/// The most documents this request asked to delete: its own `max_docs`, in the body or in
/// the query string, and everything the query selects for a request naming none.
///
/// A value that is not a count at all is refused rather than read as "every document": a
/// client that sent one asked for a LIMIT, and ignoring it would delete more than it asked
/// for.
fn requested_limit(body: &Value, request: &Request<'_>) -> Result<usize, Box<RtResponse>> {
    let named = body
        .get(MAX_DOCS_MEMBER)
        .cloned()
        .or_else(|| request.parameter(MAX_DOCS_MEMBER).map(Value::String));
    let Some(named) = named else {
        return Ok(usize::MAX);
    };
    let counted = match &named {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    };
    match counted {
        Some(counted) => Ok(usize::try_from(counted).unwrap_or(usize::MAX)),
        None => Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("[{MAX_DOCS_MEMBER}] must be a non-negative number, and this request \
                      names {named}"),
        ))),
    }
}

/// The member a request names its deletion limit in.
const MAX_DOCS_MEMBER: &str = "max_docs";

/// The member the request carries the query to delete by in.
const QUERY_MEMBER: &str = "query";

/// What the answer reports for `requests_per_second` when nothing throttled the request,
/// which is the engine's own value for "no throttling".
const UNTHROTTLED_REQUESTS_PER_SECOND: f64 = -1.0;
