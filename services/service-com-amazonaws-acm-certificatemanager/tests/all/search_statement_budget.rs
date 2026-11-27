// SPDX-License-Identifier: CC0-1.0

//! The budget `SearchCertificates` READS a filter statement under, end to end over the
//! wire (`POST /`, `X-Amz-Target: CertificateManager.SearchCertificates`, awsJson1_1).
//!
//! RULE under test: the service, not the request, decides how deep and how large a
//! filter statement it will build. A statement inside both bounds is answered; one past
//! either is refused with the modelled 400 `ValidationException` whose message states
//! THAT bound — however far past it the request is, because the bound is enforced while
//! the statement is being read rather than after the whole body has been turned into a
//! tree.
//!
//! The two bounds are the service's own choice rather than the model's, and this suite
//! names each as a literal so it is not asserted against the constant that implements
//! it: `crate::ops::search_certificates::MAX_STATEMENT_DEPTH` is 20 levels and
//! `MAX_STATEMENT_NODES` is 10000 filter nodes.
//!
//! The tables span each bound at it, inside it, one past it, and FAR past it — the last
//! row being the one that distinguishes a bound the reader enforces from one only a
//! later walk would.

use crate::certificate_lifecycle::{call_raw, error_code, error_message, router, REGION};

/// The deepest nesting of `And`/`Or`/`Not` the service will read, counting the leaf
/// filter as a level (`crate::ops::search_certificates::MAX_STATEMENT_DEPTH`).
const MAX_STATEMENT_DEPTH: usize = 20;

/// The most filter nodes the service will read in one statement
/// (`crate::ops::search_certificates::MAX_STATEMENT_NODES`).
const MAX_STATEMENT_NODES: usize = 10_000;

/// One leaf filter, as the exact bytes a request carries it as.
const LEAF: &str = "{\"Filter\":{\"AcmCertificateMetadataFilter\":{\"Status\":\"ISSUED\"}}}";

/// A statement `levels` nodes deep: `levels - 1` `Not` operators around one leaf.
fn nested(levels: usize) -> String {
    let mut statement = LEAF.to_string();
    for _ in 1..levels {
        statement = format!("{{\"Not\":{statement}}}");
    }
    statement
}

/// A statement of `nodes` nodes, two levels deep: one `And` over `nodes - 1` leaves.
fn wide(nodes: usize) -> String {
    let leaves: Vec<&str> = ::std::iter::repeat_n(LEAF, nodes - 1).collect();
    format!("{{\"And\":[{}]}}", leaves.join(","))
}

/// Send `statement` as `FilterStatement` and return `(status, code, message)`.
async fn search(statement: String) -> (u16, String, String) {
    let router = router();
    let (status, response) = call_raw(
        &router,
        REGION,
        "SearchCertificates",
        format!("{{\"FilterStatement\":{statement}}}").into_bytes(),
    )
    .await;
    (status, error_code(&response), error_message(&response))
}

/// RULE: a statement nested past the depth bound is refused with the sentence that
/// states that bound, however much deeper than the bound it is.
#[tokio::test]
async fn a_statement_past_the_depth_bound_is_refused_however_deep_it_is() {
    // At the bound and inside it: answered. One past it, far past it, and past the
    // nesting any generic codec limit would admit: refused with the service's sentence.
    for levels in [1, 2, MAX_STATEMENT_DEPTH - 1, MAX_STATEMENT_DEPTH] {
        let (status, code, message) = search(nested(levels)).await;
        assert_eq!(
            status, 200,
            "case {levels} levels: a statement inside the depth bound must be answered, \
             got {status} {code} {message:?}"
        );
    }
    for levels in [
        MAX_STATEMENT_DEPTH + 1,
        50,
        500,
        2_000,
    ] {
        let (status, code, message) = search(nested(levels)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {levels} levels: a statement past the depth bound must be refused, \
             got {status} {code} {message:?}"
        );
        assert!(
            message.contains(&format!("nest at most {MAX_STATEMENT_DEPTH}")),
            "case {levels} levels: expected the refusal to state the depth bound, \
             got {message:?}"
        );
    }
}

/// RULE: a statement carrying more nodes than the service will build is refused with
/// the sentence that states that bound, however many more nodes it carries.
#[tokio::test]
async fn a_statement_past_the_node_bound_is_refused_however_large_it_is() {
    // At the bound: answered, so the refusals below are about the bound and not about
    // the shape of a wide statement.
    for nodes in [2, 100, MAX_STATEMENT_NODES] {
        let (status, code, message) = search(wide(nodes)).await;
        assert_eq!(
            status, 200,
            "case {nodes} nodes: a statement inside the node bound must be answered, \
             got {status} {code} {message:?}"
        );
    }
    for nodes in [MAX_STATEMENT_NODES + 1, 4 * MAX_STATEMENT_NODES] {
        let (status, code, message) = search(wide(nodes)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {nodes} nodes: a statement past the node bound must be refused, \
             got {status} {code} {message:?}"
        );
        assert!(
            message.contains(&format!("at most {MAX_STATEMENT_NODES} filters")),
            "case {nodes} nodes: expected the refusal to state the node bound, \
             got {message:?}"
        );
    }
}

/// RULE: the budget is ONE statement's, so a request refused for its size leaves
/// nothing behind for the next one — a statement inside the bounds is answered straight
/// after a statement past them, on the same connection and the same thread.
#[tokio::test]
async fn the_budget_is_restored_after_a_statement_past_it() {
    let router = router();
    let refused = [nested(2_000), wide(4 * MAX_STATEMENT_NODES)];
    for statement in refused {
        let (status, _code, _message) = search(statement).await;
        assert_eq!(status, 400, "a statement past a bound must be refused");
    }
    // The same router, the same thread: a statement well inside both bounds is answered
    // as if the refusals had never happened.
    let (status, response) = call_raw(
        &router,
        REGION,
        "SearchCertificates",
        format!("{{\"FilterStatement\":{}}}", wide(MAX_STATEMENT_NODES)).into_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "a statement inside both bounds must be answered after refused ones, \
         got {status} {response}"
    );
}
