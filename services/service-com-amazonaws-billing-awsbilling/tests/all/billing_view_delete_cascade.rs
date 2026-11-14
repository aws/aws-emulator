// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteBillingView` leaves behind, end to end over the awsJson1_0
//! wire.
//!
//! One rule: a deleted billing view is referenced nowhere in the store
//! afterwards — not by its own record, and not by the source-view set of any
//! view that drew from it. So no read reports an association that names a view
//! the store no longer holds (the state `AssociateSourceViews` refuses to
//! write), every other source a view held stays where it was, and the per-view
//! source slot the deleted view occupied is free for another association.
//!
//! The rows span what the rule generalises over: how many source views the
//! drawing view holds (1 through 10, the `BillingViewSourceViewsList` `@length`
//! maximum), where in that stored order the deleted view sits (only, first,
//! middle, last), and how many views draw from the deleted one.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns and
//! the view creation is `source_view_disassociation.rs`'s.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator, error_code, PRIMARY_ARN};
use crate::source_view_disassociation::create_view;

/// One row: a view created holding `held` real source views, `derived` views
/// drawing from all of them, and the stored position of the source that is then
/// deleted.
struct Row {
    case: &'static str,
    held: usize,
    delete_index: usize,
    derived: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only source view",
            held: 1,
            delete_index: 0,
            derived: 1,
        },
        Row {
            case: "the first of three",
            held: 3,
            delete_index: 0,
            derived: 1,
        },
        Row {
            case: "the middle of three, two views drawing from it",
            held: 3,
            delete_index: 1,
            derived: 2,
        },
        Row {
            case: "the last of three",
            held: 3,
            delete_index: 2,
            derived: 1,
        },
        Row {
            case: "one of ten, the modeled maximum, three views drawing from it",
            held: 10,
            delete_index: 4,
            derived: 3,
        },
        Row {
            case: "the first of two",
            held: 2,
            delete_index: 0,
            derived: 2,
        },
    ]
}

/// The source views the STORE holds for `arn`, in order, read back through the
/// list API. No `maxResults` is sent and a view holds at most ten sources, well
/// under the page size this read serves by default, so the whole set is one
/// page.
async fn listed_sources(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<String> {
    let (status, listed) =
        call(router, "ListSourceViewsForBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the source-view list to answer 200, got {status} {listed:?}"
    );
    listed
        .get("sourceViews")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The persisted projection of `arn`, as `GetBillingView` reports it.
async fn view_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, got) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBillingView to answer 200 for {arn}, got {status} {got:?}"
    );
    got["billingView"].clone()
}

/// Whether `view`'s projection carries no `updatedAt` — a view no write has
/// touched since it was created.
fn never_updated(view: &Value) -> bool {
    view.get("updatedAt").is_none_or(Value::is_null)
        && view
            .get("viewDefinitionLastUpdatedAt")
            .is_none_or(Value::is_null)
}

#[tokio::test]
async fn a_forced_delete_leaves_no_view_drawing_from_the_deleted_one() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        // Every source is a view this store really holds, in the order the
        // drawing views are created with.
        let mut sources = Vec::new();
        for slot in 0..row.held {
            sources.push(
                create_view(
                    &router,
                    case,
                    &format!("casc-{index}-src-{slot}"),
                    &[PRIMARY_ARN.to_string()],
                )
                .await,
            );
        }
        let mut drawing = Vec::new();
        for nth in 0..row.derived {
            drawing.push(
                create_view(
                    &router,
                    case,
                    &format!("casc-{index}-derived-{nth}"),
                    &sources,
                )
                .await,
            );
        }

        let target = sources[row.delete_index].clone();
        let expected: Vec<String> = sources
            .iter()
            .filter(|source| **source != target)
            .cloned()
            .collect();

        // Before the delete: every drawing view holds the target, and none of
        // them has been written since it was created.
        for arn in &drawing {
            let listed = listed_sources(&router, case, arn).await;
            assert_eq!(
                listed, sources,
                "case {case:?}: expected the view to be created holding every source, got {listed:?}"
            );
            let view = view_of(&router, case, arn).await;
            assert!(
                never_updated(&view),
                "case {case:?}: a freshly created view carries no update instant, got {view:?}"
            );
        }

        let (status, forced) = call(
            &router,
            "DeleteBillingView",
            &json!({ "arn": target, "force": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the forced delete to answer 200, got {status} {forced:?}"
        );

        let (status, gone) = call(&router, "GetBillingView", &json!({ "arn": target })).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted view must be gone, got {status} {gone:?}"
        );

        for arn in &drawing {
            let listed = listed_sources(&router, case, arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the sources it held minus the deleted one, got {listed:?}"
            );
            // Every ARN a read reports resolves to a view the store holds: one
            // read of this service cannot contradict another.
            for source in &listed {
                let (status, resolved) =
                    call(&router, "GetBillingView", &json!({ "arn": source })).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the reported source {source} must resolve, got {status} {resolved:?}"
                );
            }
            let view = view_of(&router, case, arn).await;
            assert_eq!(
                view["sourceViewCount"].as_u64(),
                Some(expected.len() as u64),
                "case {case:?}: sourceViewCount counts only the sources that remain, got {:?}",
                view["sourceViewCount"]
            );
            // The source set is part of the view definition, and it changed, so
            // both instants a definition change moves are now reported.
            assert!(
                view["updatedAt"].is_number() && view["viewDefinitionLastUpdatedAt"].is_number(),
                "case {case:?}: a view whose source set changed reports both instants, got {view:?}"
            );
        }

        // The views the delete did not name keep their own definition, their
        // derived count and their instants.
        for kept in &expected {
            let view = view_of(&router, case, kept).await;
            assert_eq!(
                view["derivedViewCount"].as_u64(),
                Some(row.derived as u64),
                "case {case:?}: {kept} is still drawn from by every drawing view, got {:?}",
                view["derivedViewCount"]
            );
            assert!(
                never_updated(&view),
                "case {case:?}: a view the delete did not name moves no instant, got {view:?}"
            );
        }

        // The per-view source slot the deleted view occupied is free again: one
        // more real source fits, taking each drawing view back up to the count
        // it was created with — even at the modeled maximum of ten.
        let replacement = create_view(
            &router,
            case,
            &format!("casc-{index}-replacement"),
            &[PRIMARY_ARN.to_string()],
        )
        .await;
        for arn in &drawing {
            let (status, associated) = call(
                &router,
                "AssociateSourceViews",
                &json!({ "arn": arn, "sourceViews": [replacement] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the slot the deleted view held is free again, got {status} {associated:?}"
            );
            let view = view_of(&router, case, arn).await;
            assert_eq!(
                view["sourceViewCount"].as_u64(),
                Some(row.held as u64),
                "case {case:?}: expected the view back at {} sources, got {:?}",
                row.held,
                view["sourceViewCount"]
            );
        }
    }
}

#[tokio::test]
async fn a_forced_delete_in_a_chain_drops_only_the_deleted_link() {
    let router = emulator();
    // A three-link chain: `head` draws from `middle`, `middle` draws from
    // `tail`, `tail` draws from the account's PRIMARY view.
    let case = "chain";
    let tail = create_view(&router, case, "chain-tail", &[PRIMARY_ARN.to_string()]).await;
    let middle = create_view(&router, case, "chain-middle", std::slice::from_ref(&tail)).await;
    let head = create_view(&router, case, "chain-head", std::slice::from_ref(&middle)).await;

    let (status, forced) = call(
        &router,
        "DeleteBillingView",
        &json!({ "arn": middle, "force": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the forced delete of the middle link to answer 200, \
         got {status} {forced:?}"
    );

    let listed = listed_sources(&router, case, &head).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: the head drew only from the deleted link, got {listed:?}"
    );
    assert_eq!(
        view_of(&router, case, &head).await["sourceViewCount"].as_u64(),
        Some(0),
        "case {case:?}: the head holds no source view now"
    );

    // The link BELOW the deleted one is untouched: it keeps its own source and
    // simply has nothing drawing from it any more.
    let listed = listed_sources(&router, case, &tail).await;
    assert_eq!(
        listed,
        vec![PRIMARY_ARN.to_string()],
        "case {case:?}: the tail keeps the source it was created with, got {listed:?}"
    );
    let tail_view = view_of(&router, case, &tail).await;
    assert_eq!(
        tail_view["derivedViewCount"].as_u64(),
        Some(0),
        "case {case:?}: the only view that drew from the tail is gone, got {:?}",
        tail_view["derivedViewCount"]
    );
    assert!(
        never_updated(&tail_view),
        "case {case:?}: the tail's own definition did not change, got {tail_view:?}"
    );
}

#[tokio::test]
async fn only_the_sets_that_named_the_deleted_view_are_rewritten() {
    let router = emulator();
    let case = "untouched";
    let source = create_view(&router, case, "ctl-source", &[PRIMARY_ARN.to_string()]).await;
    let drawing = create_view(&router, case, "ctl-drawing", std::slice::from_ref(&source)).await;
    let lone = create_view(&router, case, "ctl-lone", &[PRIMARY_ARN.to_string()]).await;

    // A view nothing draws from needs no `force`, and deleting it rewrites
    // nobody else's set.
    let (status, deleted) = call(&router, "DeleteBillingView", &json!({ "arn": lone })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to answer 200, got {status} {deleted:?}"
    );

    // And a delete the service REFUSED writes nothing at all: the same view,
    // without `force`, is a conflict.
    let (status, refused) = call(&router, "DeleteBillingView", &json!({ "arn": source })).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConflictException"),
        "case {case:?}: a view another view draws from needs force, got {status} {refused:?}"
    );

    let listed = listed_sources(&router, case, &drawing).await;
    assert_eq!(
        listed,
        vec![source.clone()],
        "case {case:?}: an unrelated delete and a refused one leave the set alone, got {listed:?}"
    );
    let view = view_of(&router, case, &drawing).await;
    assert!(
        never_updated(&view),
        "case {case:?}: no instant moves on a view no delete touched, got {view:?}"
    );
    let source_view = view_of(&router, case, &source).await;
    assert_eq!(
        source_view["derivedViewCount"].as_u64(),
        Some(1),
        "case {case:?}: the refused delete left the association in place, got {:?}",
        source_view["derivedViewCount"]
    );
}
