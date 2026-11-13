// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY of the `ConditionalToken` the non-mutating paths
//! serve: the token `CreateApplication` answers with and the one `DescribeApplication`
//! reports.
//!
//! A `ConditionalToken` is the handle a caller uses "to implement strong concurrency for
//! application updates" (the member's doc), so it is per-application state the create
//! MINTS and a read merely REPORTS — never a value computed from the request. These
//! tests pin that: a token is of the modelled form, is not written in the layout of a
//! server-assigned id, is stable across repeated reads of one application, and two
//! applications that differ only in which store they live in, or in whether one was
//! deleted and created again under the same name with the same request, never share a
//! token.
//!
//! Every test drives the service the way a client does — one `POST /` per call through
//! the real `MultiServiceRouter`, with `X-Amz-Target` naming the operation — so what is
//! asserted is the wire behaviour, not a handler's return value.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_mode_and_maintenance_window::{is_modeled_token, is_uuid_layout, token_of};
use super::harness::{call, create_body, describe, router, router_on, with_code_content};

/// One row of the table: a create request the class spans, named so a failure says which.
struct Case {
    /// The case's name, and the `ApplicationName` it creates — rows differ in length and
    /// in which of `ApplicationName`'s characters they use (`@length` 1..128, `@pattern`
    /// `^[a-zA-Z0-9_.-]+$`, `smithy_query(query="shape",shape="ApplicationName")`).
    name: String,
    /// A value of the `RuntimeEnvironment` enum
    /// (`smithy_query(query="shape",shape="RuntimeEnvironment")`); rows span the runtime
    /// families this API serves, including values the issue report did not cite.
    runtime: &'static str,
    /// Whether the create carries an `ApplicationConfiguration` at all, so the table
    /// spans both shapes of create request.
    with_configuration: bool,
}

/// The create requests every property below is asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The shortest name `ApplicationName`'s `@length` admits.
            name: "a".to_string(),
            runtime: "FLINK-1_20",
            with_configuration: false,
        },
        Case {
            name: "Token_Identity.Mixed-129".to_string(),
            runtime: "FLINK-2_3",
            with_configuration: true,
        },
        Case {
            name: "zeppelin-notebook".to_string(),
            runtime: "ZEPPELIN-FLINK-2_0",
            with_configuration: false,
        },
        Case {
            name: "studio-3-0".to_string(),
            runtime: "ZEPPELIN-FLINK-3_0",
            with_configuration: false,
        },
        Case {
            // The longest name `ApplicationName`'s `@length` admits: 128 characters.
            name: "l".repeat(128),
            runtime: "FLINK-1_11",
            with_configuration: true,
        },
    ]
}

/// The create body a case sends.
fn body_of(case: &Case) -> Value {
    let body = create_body(&case.name, case.runtime);
    if case.with_configuration {
        with_code_content(body)
    } else {
        body
    }
}

/// Create a case's application on `router` and answer the `ConditionalToken` the create
/// responded with, having checked it is of the modelled form.
async fn created_token(router: &MultiServiceRouter, case: &Case) -> String {
    let (status, created) = call(router, "CreateApplication", &body_of(case)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication got {status} {created}",
        case.name
    );
    let token = token_of(&created["ApplicationDetail"], &case.name);
    assert!(
        is_modeled_token(&token),
        "case {:?}: expected a token of the modelled form (1..512 of [a-zA-Z0-9-_+/=]), \
         got {token:?}",
        case.name
    );
    assert!(
        !is_uuid_layout(&token),
        "case {:?}: expected an opaque token rather than 8-4-4-4-12 id text, got {token:?}",
        case.name
    );
    token
}

/// `CreateApplication` mints the token and `DescribeApplication` reports the one stored:
/// repeated reads of one application answer the token the create handed out, unchanged.
#[tokio::test]
async fn a_read_reports_the_minted_token_and_never_mints_another() {
    let router = router();
    for case in cases() {
        let minted = created_token(&router, &case).await;
        for read in 1..=3 {
            let token = token_of(&describe(&router, &case.name).await, &case.name);
            assert_eq!(
                token, minted,
                "case {:?}: read {read} expected the stored token {minted:?}, got {token:?}",
                case.name
            );
        }
    }
}

/// A token is per-application state, so no two applications share one: every create in
/// one store hands out a token no other application there was given.
#[tokio::test]
async fn every_application_in_one_store_gets_its_own_token() {
    let router = router();
    let mut seen: Vec<(String, String)> = Vec::new();
    for case in cases() {
        let token = created_token(&router, &case).await;
        if let Some((owner, _)) = seen.iter().find(|(_, other)| *other == token) {
            panic!(
                "case {:?}: token {token:?} was already handed out to {owner:?}",
                case.name
            );
        }
        // The same token must come back from the read path too, so the uniqueness a
        // caller observes through DescribeApplication is the same one.
        let described = token_of(&describe(&router, &case.name).await, &case.name);
        assert_eq!(
            described, token,
            "case {:?}: expected describe to report {token:?}, got {described:?}",
            case.name
        );
        seen.push((case.name.clone(), token));
    }
}

/// A token is MINTED, not computed from the request: the identical create request issued
/// against an independent store yields a different token, and each store keeps reporting
/// its own.
#[tokio::test]
async fn an_identical_request_against_another_store_gets_another_token() {
    let first = router();
    let second = router_on(Arc::new(engine::InMemoryEngine::new()));
    for case in cases() {
        let here = created_token(&first, &case).await;
        let there = created_token(&second, &case).await;
        assert_ne!(
            here, there,
            "case {:?}: expected the second store to mint its own token, got {here:?} twice",
            case.name
        );
        for (label, router, expected) in [("first", &first, &here), ("second", &second, &there)] {
            let token = token_of(&describe(router, &case.name).await, &case.name);
            assert_eq!(
                &token, expected,
                "case {:?}: the {label} store must report its own token {expected:?}, got \
                 {token:?}",
                case.name
            );
        }
    }
}

/// A token belongs to the application that was created, not to its NAME: an application
/// deleted and created again from the same request is a new application, and is handed a
/// token the deleted one never had.
#[tokio::test]
async fn a_recreated_application_gets_a_fresh_token() {
    let router = router();
    for case in cases() {
        let mut seen: Vec<String> = Vec::new();
        for round in 1..=3 {
            let token = created_token(&router, &case).await;
            assert!(
                !seen.contains(&token),
                "case {:?}: round {round} reused token {token:?} from an earlier \
                 incarnation",
                case.name
            );
            seen.push(token);

            let described = describe(&router, &case.name).await;
            let create_timestamp = described["CreateTimestamp"].as_i64().unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a CreateTimestamp to delete by, got {described}",
                    case.name
                )
            });
            let (status, deleted) = call(
                &router,
                "DeleteApplication",
                &json!({"ApplicationName": case.name, "CreateTimestamp": create_timestamp}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: round {round} DeleteApplication got {status} {deleted}",
                case.name
            );
        }
    }
}
