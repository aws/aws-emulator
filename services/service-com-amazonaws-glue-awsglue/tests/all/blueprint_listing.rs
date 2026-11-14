// SPDX-License-Identifier: CC0-1.0

//! `ListBlueprints`: the names of the blueprints this account holds.
//!
//! RULE: the operation "lists all the blueprint names in an account", so it reports the
//! name of every blueprint the store holds — one `MaxResults`-sized page at a time in
//! name order, with a `NextToken` that resumes the walk where the previous page stopped,
//! and nothing but the blueprints carrying every pair a `Tags` filter names. It reports
//! only: a listing changes nothing a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many blueprints are stored,
//! the page size asked for (including the published bounds 1 and 25, a size past the
//! stored count, and one past each bound), and which tags the filter names.
//!
//! The blueprints are seeded into the store directly, through the same
//! [`BlueprintEntry::from_create_input`] a `CreateBlueprint` registers one with, so a
//! listing suite seeds its rows without going through that operation first.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateBlueprintInput;
use service_com_amazonaws_glue_awsglue::state::{
    BlueprintArchive, BlueprintEntry, ComAmazonawsGlueAwsglueState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, ok};

/// An emulator and the engine its store is held on, so a suite can seed blueprints.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Record one blueprint, with the tags the row asks for.
fn given_blueprint(engine: &Arc<engine::InMemoryEngine>, name: &str, tags: &[(&str, &str)]) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    state
        .lock()
        .insert_blueprint(BlueprintEntry::from_create_input(
            &CreateBlueprintInput {
                name: name.to_string(),
                description: None,
                blueprint_location: format!("s3://glue-blueprints/{name}.zip"),
                tags: Some(
                    tags.iter()
                        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                        .collect(),
                ),
            },
            SystemTime::now(),
            // These rows are about which blueprints a listing reports, so each is seeded as
            // a registration whose published archive was read.
            BlueprintArchive::Published,
        ));
}

/// The names one answer reports, in the order the service returned them.
fn listed_names(page: &Value) -> Vec<String> {
    page["Blueprints"]
        .as_array()
        .expect("Blueprints is a list")
        .iter()
        .map(|name| {
            name.as_str()
                .expect("a blueprint name is a string")
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for `base`, following its own `NextToken`, and
/// return the names in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<String> {
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListBlueprints", &request).await;
        let names = listed_names(&page);
        if let Some(size) = page_size {
            assert!(
                names.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {names:?}"
            );
        }
        listed.extend(names);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {listed:?} for {limit} blueprints"
        );
    }
    listed
}

#[tokio::test]
async fn every_stored_blueprint_name_is_listed_and_paged() {
    // (case, how many blueprints are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no blueprints at all", 0, None),
        ("one blueprint, no page size", 1, None),
        // The published lower bound of `MaxResults` (`OrchestrationPageSize25`, 1..25).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(20)),
        // The published upper bound, with more stored than one page holds.
        ("the largest page the model accepts", 30, Some(25)),
        ("no page size, more stored than one page holds", 28, None),
    ];

    for (case, stored, page_size) in rows {
        let (router, engine) = emulator_with_store();
        // Seeded out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("blueprint-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_blueprint(&engine, name, &[]);
        }

        let listed = walk(&router, case, &json!({}), *page_size, *stored).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored blueprint name in name order, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_page_reports_at_most_max_results_names_and_a_token_only_when_more_remain() {
    let (router, engine) = emulator_with_store();
    for index in 0..4 {
        given_blueprint(&engine, &format!("paged-{index}"), &[]);
    }

    // (case, MaxResults, the names that page reports, whether a token follows)
    let rows: &[(&str, i32, &[&str], bool)] = &[
        ("one at a time", 1, &["paged-0"], true),
        ("two at a time", 2, &["paged-0", "paged-1"], true),
        (
            "exactly the stored count",
            4,
            &["paged-0", "paged-1", "paged-2", "paged-3"],
            false,
        ),
        (
            "more than the stored count",
            25,
            &["paged-0", "paged-1", "paged-2", "paged-3"],
            false,
        ),
    ];

    for (case, max_results, expected, more) in rows {
        let page = ok(
            &router,
            "ListBlueprints",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        let names = listed_names(&page);
        assert_eq!(
            names,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the first {max_results} stored names, got {page}"
        );
        assert_eq!(
            page.get("NextToken").and_then(Value::as_str).is_some(),
            *more,
            "case {case:?}: a token must be reported only while names remain, got {page}"
        );
        // The token resumes the walk: the next page starts strictly after the last name
        // of this one.
        if let Some(token) = page.get("NextToken").and_then(Value::as_str) {
            let next = ok(
                &router,
                "ListBlueprints",
                &json!({ "MaxResults": max_results, "NextToken": token }),
            )
            .await;
            let continued = listed_names(&next);
            assert!(
                continued
                    .first()
                    .is_some_and(|name| name.as_str() > *expected.last().expect("a listed name")),
                "case {case:?}: the next page must resume after {:?}, got {continued:?}",
                expected.last()
            );
        }
    }
}

#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_blueprints_carrying_every_pair() {
    let (router, engine) = emulator_with_store();
    given_blueprint(
        &engine,
        "tagged-both",
        &[("team", "data"), ("env", "prod")],
    );
    given_blueprint(&engine, "tagged-team", &[("team", "data")]);
    given_blueprint(&engine, "tagged-other", &[("team", "ops")]);
    given_blueprint(&engine, "untagged", &[]);

    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no filter: every blueprint",
            Value::Null,
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "an empty filter narrows nothing",
            json!({}),
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "one pair",
            json!({ "team": "data" }),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({ "team": "data", "env": "prod" }),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({ "team": "finance" }), &[]),
        ("a key nothing carries", json!({ "owner": "data" }), &[]),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = ok(&router, "ListBlueprints", &request).await;
        assert_eq!(
            listed_names(&page),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the blueprints carrying every pair of {filter}, got {page}"
        );
    }

    // A filter also narrows a PAGED walk, so the token resumes within the filtered set
    // rather than over the whole store.
    let listed = walk(
        &router,
        "one pair, one name per page",
        &json!({ "Tags": { "team": "data" } }),
        Some(1),
        4,
    )
    .await;
    assert_eq!(
        listed,
        vec!["tagged-both".to_string(), "tagged-team".to_string()],
        "case one pair, one name per page: expected only the tagged blueprints, got {listed:?}"
    );
}

#[tokio::test]
async fn the_listing_reports_what_the_store_holds_now() {
    let (router, engine) = emulator_with_store();
    for name in ["kept-one", "kept-two", "removed"] {
        given_blueprint(&engine, name, &[]);
    }

    let before = listed_names(&ok(&router, "ListBlueprints", &json!({})).await);
    assert_eq!(
        before,
        vec![
            "kept-one".to_string(),
            "kept-two".to_string(),
            "removed".to_string()
        ],
        "case before the record is dropped: expected every stored blueprint, got {before:?}"
    );

    // `DeleteBlueprint` is still a scaffold, so the record is dropped from the store the
    // listing reads rather than through that operation.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().blueprints.remove("removed");
    }

    let after = listed_names(&ok(&router, "ListBlueprints", &json!({})).await);
    assert_eq!(
        after,
        vec!["kept-one".to_string(), "kept-two".to_string()],
        "case after the record is dropped: a blueprint the store no longer holds must not be \
         listed, got {after:?}"
    );

    // Listing is reporting only: two listings in a row agree, so a read changed nothing a
    // later caller sees — including the store itself.
    let again = listed_names(&ok(&router, "ListBlueprints", &json!({})).await);
    assert_eq!(
        again, after,
        "case listed twice: a listing must change nothing, got {again:?} then {after:?}"
    );
    let held: Vec<String> = engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .blueprints
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        held, after,
        "case listed twice: the store must hold exactly what was listed, got {held:?}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, engine) = emulator_with_store();
    given_blueprint(&engine, "bounded-blueprint", &[]);

    // `MaxResults` is `OrchestrationPageSize25`, range 1..25: one past each published
    // bound is refused by the modelled constraint rather than served as some other page
    // size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 26)] {
        let (status, body) = call(
            &router,
            "ListBlueprints",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }

    // Each published bound itself is served.
    for (case, max_results) in [("the minimum", 1), ("the maximum", 25)] {
        let page = ok(
            &router,
            "ListBlueprints",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            vec!["bounded-blueprint".to_string()],
            "case {case:?}: MaxResults={max_results} must be served, got {page}"
        );
    }
}
