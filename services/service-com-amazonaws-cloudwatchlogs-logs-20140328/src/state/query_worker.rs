// SPDX-License-Identifier: CC0-1.0

//! Running a started CloudWatch Logs Insights query at the instant the clock completes it,
//! which is what FIXES the results that query answers from then on.
//!
//! `StartQuery` "schedules a query of a log group using CloudWatch Logs Insights" and answers
//! a `queryId` while the query is still owed, so the query RUNS off the response path — here —
//! and records what it produced on its own record
//! ([`super::QueryRecord::record_snapshot`]). `GetQueryResults` then "returns the results
//! FROM the specified query": it reports that snapshot rather than running the stored query
//! again, so a log group deleted, or an event ingested, after the query ran changes neither
//! the rows nor the statistics the query answers. That is what makes the `queryId` a durable
//! handle on one result set.
//!
//! The run happens at the instant the query is due to reach `Complete` (the spans
//! [`super::QUERY_RUNNING_DELAY_MS`] and [`super::QUERY_COMPLETION_DELAY_MS`] the start
//! armed), so the events it reads are the ones the store held then — not the ones it holds
//! whenever a caller first looks.
//!
//! The store's guard is never held across the run: everything the run needs is read under it
//! ([`super::insights::prepare`]), the query then runs with it released and inside the budget
//! every Insights run is bounded by ([`super::insights::Budget`]), and the guard is taken
//! again only to record what the run produced.
//!
//! A query the store no longer holds, and one a caller STOPPED before it completed, leave
//! nothing to run: the record lives in the very store this reads, so a store it cannot reach
//! holds no query either, and a cancelled query is terminal and produced no results.
//!
//! WHAT BOUNDS IT: one run per query the store accepted, and `StartQuery` accepts no query
//! past the concurrent-query quota ([`super::MAX_CONCURRENT_QUERIES`]) — which counts exactly
//! the queries whose span this waits out — so the tasks in flight are bounded by that quota
//! rather than by how often a caller calls. Each run then spends the same bounded budget every
//! Insights run spends (steps, computed bytes and wall-clock time), yields as it goes, and ends
//! after recording what it produced: there is no loop over the clock and no retry.

use std::sync::Arc;

/// Run the query the call that returns this accepted.
///
/// Returns at once: the query runs in a task of its own, because `StartQuery` answers with the
/// query's id while the run is still owed.
pub fn start<Eng: Send + Sync + 'static>(engine: Arc<Eng>, query_id: String) {
    ::tokio::spawn(async move {
        run(&engine, &query_id).await;
    });
}

/// Wait until the query is due to be complete, run it, and record what it produced.
async fn run<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, query_id: &str) {
    ::tokio::time::sleep(::std::time::Duration::from_millis(
        u64::try_from(super::QUERY_RUNNING_DELAY_MS + super::QUERY_COMPLETION_DELAY_MS)
            .unwrap_or(0),
    ))
    .await;

    // The query's record lives in this very store, so a store that is not there holds no query
    // whose run is owed.
    let Some(state) = super::service_state(&**engine) else {
        return;
    };

    // Everything the run needs is read under the guard, which is then released: the query
    // below runs with no lock held.
    let prepared = {
        let mut store = state.lock();
        // The statuses the clock already owes are materialised first, so the status read here
        // is the current one.
        store.materialize_due_query_transitions(super::now_ms());
        // A query the store no longer holds was dropped from it, so there is no run left to do
        // and nothing left to record it on.
        let Some(query) = store.queries.get(query_id) else {
            return;
        };
        // A query a caller STOPPED produced no results, and one that already holds its
        // snapshot has run: neither has a run left to do.
        if query.snapshot.is_some() || query.status != super::QUERY_STATUS_COMPLETE {
            return;
        }
        let query = query.clone();
        super::insights::prepare(&store, &query)
    };
    let execution = super::insights::run(prepared).await;

    // What the run produced is recorded on the query, which is what every later read of its
    // results reports ([`super::QueryRecord::snapshot`]).
    let mut store = state.lock();
    if let Some(query) = store.queries.get_mut(query_id) {
        query.record_snapshot(execution);
    }
}
