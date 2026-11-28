// SPDX-License-Identifier: CC0-1.0

//! The FUNCTION resource of Elemental Inference, end to end over the assembled wire:
//! `CreateFunction`, `GetFunction`, `ListFunctions`, `UpdateFunction` and
//! `DeleteFunction`, plus the media context the service derives from a function's request
//! expression.
//!
//! The rules under test, each over a TABLE of inputs rather than one case:
//!
//! * `contextTypes` is DERIVED from the media paths the request expression references and
//!   cannot be set by the caller: "FRAMES - Derived from media.frames. SUBTITLES -
//!   Derived from media.transcript. CONTEXTUAL_METADATA - Derived from media.contextual.
//!   CLIP_METADATA - Derived from media.clip" (`ContextType`), and "an empty list means
//!   that the expression references no media";
//! * the two rules stated over those paths are refused with the modelled
//!   `ValidationException` (HTTP 400): "the expression can reference at most one of
//!   media.transcript, media.contextual, and media.clip", and "a passthrough function
//!   can't reference media.frames";
//! * a created function is read back through `GetFunction` and listed by `ListFunctions`
//!   with the same values, so what a create reports is what the store holds;
//! * `UpdateFunction` is a PATCH — "include only the parameters that you want to change;
//!   parameters that you don't include aren't affected" — and rewriting the request
//!   expression rewrites the derived media context with it;
//! * `DeleteFunction` accepts the removal (HTTP 202) and the function is gone from the
//!   store once the deletion falls due.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::elementalinference_wire::{
    bedrock_target, call, error_code, function_body, passthrough_target, poll_until, router,
};

/// One row of the media-context table: an expression, and the `contextTypes` the service
/// derives from it.
struct ContextCase {
    label: &'static str,
    expression: &'static str,
    derived: Value,
}

/// Expressions differing along which documented media paths they reference, how many,
/// and how they are written — a bare path, a path inside a function call, a path with
/// space around its separator, and names that merely CONTAIN a documented path.
///
/// The paths and the context types they map to come from
/// `smithy_query(query="doc",shape="ContextType")`; `media.contextual` is a row the
/// issue's own examples do not cite.
fn context_cases() -> Vec<ContextCase> {
    vec![
        ContextCase {
            label: "an expression that references no media at all",
            expression: "{\"label\": \"constant\"}",
            derived: json!([]),
        },
        ContextCase {
            label: "a bare reference to media.frames",
            expression: "{\"frames\": media.frames}",
            derived: json!(["FRAMES"]),
        },
        ContextCase {
            label: "media.transcript inside a function call",
            expression: "{\"text\": $join(media.transcript.lines, \" \")}",
            derived: json!(["SUBTITLES"]),
        },
        ContextCase {
            label: "media.contextual with space around the separator",
            expression: "{\"scene\": media . contextual}",
            derived: json!(["CONTEXTUAL_METADATA"]),
        },
        ContextCase {
            label: "media.clip",
            expression: "{\"clip\": media.clip.id}",
            derived: json!(["CLIP_METADATA"]),
        },
        ContextCase {
            label: "frames beside one of the exclusive contexts",
            expression: "{\"f\": media.frames, \"t\": media.transcript}",
            derived: json!(["FRAMES", "SUBTITLES"]),
        },
        ContextCase {
            label: "names that only contain a documented path",
            expression: "{\"a\": mymedia.frames, \"b\": media.framesPerSecond}",
            derived: json!([]),
        },
    ]
}

/// Create one function, asserting it is accepted, and return the create's answer.
async fn create_function(router: &MultiServiceRouter, body: Value, case: &str) -> Value {
    let (status, created) = call(router, "POST", "/v1/function", Some(body)).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateFunction answers 202, got {status} with {created}"
    );
    created
}

/// The media context a function reports is derived from its request expression, and the
/// same value comes back from the create, from a read and from a listing.
#[tokio::test]
async fn the_derived_media_context_is_read_off_the_request_expression() {
    let router = router();
    for (index, case) in context_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("ctxfn{index}");
        let created = create_function(
            &router,
            function_body(&name, bedrock_target(), case.expression),
            label,
        )
        .await;
        assert_eq!(
            created["contextTypes"], case.derived,
            "case {label:?}: the create reports the derived media context, got {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, read) = call(&router, "GET", &format!("/v1/function/{id}"), None).await;
        assert_eq!(
            (status, read["contextTypes"].clone(), read["requestExpression"].clone()),
            (200, case.derived.clone(), json!(case.expression)),
            "case {label:?}: a read reports the stored expression and its derived context, \
             got {status} with {read}"
        );

        let (status, listed) = call(&router, "GET", "/v1/functions?maxResults=100", None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListFunctions answers 200, got {status} with {listed}"
        );
        let summary = listed["functions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: the listing reports an array, got {listed}"))
            .iter()
            .find(|summary| summary["id"] == json!(id))
            .unwrap_or_else(|| panic!("case {label:?}: the listing holds the function, got {listed}"))
            .clone();
        assert_eq!(
            (summary["contextTypes"].clone(), summary["name"].clone()),
            (case.derived.clone(), json!(name.clone())),
            "case {label:?}: the summary reports the derived context and the name, got {summary}"
        );
        // Both answers describe the same record, so the status the summary reports is one
        // the function's own lifecycle produces rather than a value of the listing's own.
        assert!(
            ["CREATING", "AVAILABLE"].contains(&summary["status"].as_str().unwrap_or("")),
            "case {label:?}: the summary reports a status the create produces, got {summary}"
        );
    }
}

/// An expression the service does not admit is refused with the modelled
/// `ValidationException`, and no function is minted for it.
#[tokio::test]
async fn an_expression_outside_the_media_context_rules_is_refused() {
    let router = router();

    // The rows differ along which rule the expression breaks and which pair of exclusive
    // paths it names.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a transcript and a contextual reference together",
            bedrock_target(),
            "{\"t\": media.transcript, \"c\": media.contextual}",
            "The expression can reference at most one of media.transcript, media.contextual, \
             and media.clip.",
        ),
        (
            "a contextual and a clip reference together",
            bedrock_target(),
            "{\"c\": media.contextual, \"k\": media.clip}",
            "The expression can reference at most one of media.transcript, media.contextual, \
             and media.clip.",
        ),
        (
            "all three exclusive references together",
            passthrough_target(),
            "{\"t\": media.transcript, \"c\": media.contextual, \"k\": media.clip}",
            "The expression can reference at most one of media.transcript, media.contextual, \
             and media.clip.",
        ),
        (
            "a passthrough function referencing frames",
            passthrough_target(),
            "{\"f\": media.frames}",
            "A passthrough function can't reference media.frames.",
        ),
    ];

    for (index, (label, target, expression, expected)) in rows.into_iter().enumerate() {
        let (status, refused) = call(
            &router,
            "POST",
            "/v1/function",
            Some(function_body(&format!("badfn{index}"), target, expression)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused), refused["message"].as_str().unwrap_or("")),
            (400, "ValidationException", expected),
            "case {label:?}: the create is refused with the modelled 400, got {status} with \
             {refused}"
        );
    }

    let (status, listed) = call(&router, "GET", "/v1/functions?maxResults=100", None).await;
    assert_eq!(
        (status, listed["functions"].as_array().map(Vec::len)),
        (200, Some(0)),
        "a refused create mints no function, got {status} with {listed}"
    );
}

/// `UpdateFunction` writes only what the request carried, and a rewritten request
/// expression rewrites the derived media context with it.
#[tokio::test]
async fn an_update_writes_only_the_members_it_carries() {
    let router = router();
    let created = create_function(
        &router,
        function_body(
            "patchedfn",
            bedrock_target(),
            "{\"text\": media.transcript}",
        ),
        "the function an update patches",
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    // Each row: what the PATCH carries, and what the function then reports. A member no
    // row mentions keeps the value the create stored, which the final assertion reads.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a description alone",
            json!({"description": "reshapes the transcript"}),
            json!({"description": "reshapes the transcript", "contextTypes": ["SUBTITLES"]}),
        ),
        (
            "a name alone",
            json!({"name": "renamedfn"}),
            json!({"name": "renamedfn", "contextTypes": ["SUBTITLES"]}),
        ),
        (
            "a request expression that references another context",
            json!({"requestExpression": "{\"scene\": media.contextual}"}),
            json!({
                "requestExpression": "{\"scene\": media.contextual}",
                "contextTypes": ["CONTEXTUAL_METADATA"]
            }),
        ),
        (
            "eventBridgeDelivery alone",
            json!({"eventBridgeDelivery": "ENABLED"}),
            json!({"eventBridgeDelivery": "ENABLED", "contextTypes": ["CONTEXTUAL_METADATA"]}),
        ),
    ];

    for (label, patch, expected) in rows {
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/v1/function/{id}"),
            Some(patch.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFunction answers 200, got {status} with {updated}"
        );
        let expected_members = expected.as_object().expect("the expectation is an object");
        for (member, value) in expected_members {
            assert_eq!(
                &updated[member], value,
                "case {label:?}: the update reports {member} as {value}, got {updated}"
            );
        }
        // A read reports what the update wrote, so the PATCH reached the store.
        let (status, read) = call(&router, "GET", &format!("/v1/function/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetFunction answers 200, got {status} with {read}"
        );
        for (member, value) in expected_members {
            assert_eq!(
                &read[member], value,
                "case {label:?}: the stored function reports {member} as {value}, got {read}"
            );
        }
    }

    // The target no row carried is still the one the create stored.
    let (_status, read) = call(&router, "GET", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        read["target"], created["target"],
        "a member no PATCH carried is unaffected, got {read}"
    );
}

/// A delete is ACCEPTED (HTTP 202) with the function reporting `DELETING`, and once the
/// removal falls due the id names no function at all.
#[tokio::test]
async fn a_deleted_function_leaves_the_store() {
    let router = router();
    let created = create_function(
        &router,
        function_body("doomedfn", passthrough_target(), "{\"k\": media.clip}"),
        "the function a delete removes",
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let (status, accepted) = call(&router, "DELETE", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        (status, accepted["status"].clone(), accepted["id"].clone()),
        (202, json!("DELETING"), json!(id.clone())),
        "the delete is accepted with the function reporting DELETING, got {status} with \
         {accepted}"
    );

    // Once the removal falls due the record is GONE: the id resolves for no operation,
    // this delete included.
    let (status, gone) = poll_until(
        &router,
        &format!("/v1/function/{id}"),
        "a function whose deletion has completed",
        |status, _body| status == 404,
    )
    .await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "a removed function resolves to nothing, got {status} with {gone}"
    );
    let (status, repeated) = call(&router, "DELETE", &format!("/v1/function/{id}"), None).await;
    assert_eq!(
        (status, error_code(&repeated)),
        (404, "ResourceNotFoundException"),
        "a repeat of the delete answers as an id this store never held does, got {status} with \
         {repeated}"
    );
    let (status, listed) = call(&router, "GET", "/v1/functions?maxResults=100", None).await;
    assert_eq!(
        (status, listed["functions"].as_array().map(Vec::len)),
        (200, Some(0)),
        "the listing no longer reports it, got {status} with {listed}"
    );
}
