// SPDX-License-Identifier: CC0-1.0

//! Which other resource of this service each output of a feed is BOUND to, end to end
//! over the wire.
//!
//! This suite covered the per-output `audioInput` binding an earlier model of the service
//! declared. That member is gone from every member of the `OutputConfig` union
//! (`smithy_query(query="shape",shape="OutputConfig")`), and the bindings an output config
//! carries now are references to other resources: a function output names a function by
//! its ARN (`FunctionConfig$functionArn`) and a contextual-metadata output's custom
//! questions name one version of a question set (`QuestionSetReference`). So the suite
//! covers those. (The crate declares its suites by hand in `tests/all.rs`, which a fix
//! may not edit, so the suite keeps its file name.)
//!
//! Every case drives the real request path — the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler runs,
//! and the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees.
//!
//! THE RULE under test: an output config that names another resource of this service is
//! stored only when that reference RESOLVES in the Region the request was signed for, so
//!
//! * a reference that resolves is stored and reported back by every later read of the
//!   feed, and the resource it names reports the feed under its own `references` list and
//!   the `REFERENCED` status that list decides;
//! * a reference that resolves to nothing is REFUSED with the modelled
//!   `ValidationException` (HTTP 400) and writes nothing — the same answer a dictionary or
//!   fixture reference that resolves to nothing earns;
//! * every operation that writes outputs into a feed applies it: `CreateFeed`,
//!   `UpdateFeed` and `AssociateFeed`.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! which kind of reference the config carries, which version of a question set it names,
//! which operation writes the output, and the form of a value that resolves to no
//! resource.
//!
//! This file also DECLARES the suites for the resources the model update added, which
//! live under `tests/all/new_resources/`. The crate declares its suites by hand in
//! `tests/all.rs`, which a fix may not edit, and a file deeper than `tests/all/` is
//! declared by its own parent module rather than there — so the declarations are here,
//! beside the bindings those resources are reached through.

// `#[path]` on a module declared in `tests/all/feed_audio_binding.rs` resolves against
// that file's own directory, `tests/all/`, so each suite below is the file of the same
// name under `tests/all/new_resources/`.
#[path = "new_resources/lifecycles.rs"]
mod lifecycles;

use crate::harness::{
    call, create_function, create_question_set, custom_questions_config, function_output_config,
    poll_until, router, REGION, SIGNING_NAME,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    cannot_find_function_message, cannot_find_question_set_message,
    cannot_find_question_set_version_message,
};
use smithy_server_runtime::MultiServiceRouter;

/// One row of the binding table: how the output config is built, and where a read of the
/// feed finds the reference it carries.
struct Binding {
    /// What the case is about, named in every assertion.
    label: String,
    /// The `outputConfig` the write path sends.
    config: Value,
}

/// `POST /v1/feed` with one output named `output_name` carrying `config`.
async fn create_feed(
    router: &MultiServiceRouter,
    name: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": name,
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `PUT /v1/feed/{id}` replacing the feed's payload with one output carrying `config`.
async fn update_feed(
    router: &MultiServiceRouter,
    id: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v1/feed/{id}"),
        Some(json!({
            "name": "bound-feed",
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `POST /v1/feed/{id}/associate` appending one output carrying `config`.
async fn associate_feed(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    output_name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": resource,
            "outputs": [{"name": output_name, "outputConfig": config, "status": "ENABLED"}]
        })),
    )
    .await
}

/// `GET /v1/feed/{id}`, asserting it is served.
async fn get_feed(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// `GET /v1/feed/{id}`, looking again until the feed reports a SETTLED status.
///
/// A reconfiguration suspends the feed's status for the span of the work, and an append
/// runs on a settled feed, so a suite that appends after a PUT waits for the condition
/// rather than for a fixed time.
async fn settled_feed(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    poll_until(router, &format!("/v1/feed/{id}"), case, |status, feed| {
        status == 200 && feed["status"] != json!("UPDATING")
    })
    .await
    .1
}

/// The `outputConfig` of the output named `output_name` in `feed`.
fn stored_config(feed: &Value, output_name: &str, case: &str) -> Value {
    feed["outputs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the feed reports its outputs, got {feed}"))
        .iter()
        .find(|output| output["name"] == json!(output_name))
        .map(|output| output["outputConfig"].clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the feed holds an output named {output_name:?}, got {feed}")
        })
}

/// The bindings a feed's outputs can carry, each built over resources this endpoint really
/// holds: a function named by its ARN, a question set named at DRAFT, and the same set
/// named at the version published from it.
///
/// The rows span the kinds of reference the `OutputConfig` union admits and, for a question
/// set, both forms the modelled `QuestionSetVersion` takes (`^(DRAFT|[1-9][0-9]*)$`).
async fn bindings(router: &MultiServiceRouter) -> Vec<Binding> {
    let (_function_id, function_arn) = create_function(router, "bound-function").await;
    let question_set_id = create_question_set(router, "bound-question-set").await;
    let (status, published) = call(
        router,
        "POST",
        &format!("/v1/question-set/{question_set_id}/versions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the question set publishes a version, got {status} with {published}"
    );
    let version = published["version"]
        .as_str()
        .unwrap_or_else(|| panic!("a published version reports its number, got {published}"))
        .to_string();

    vec![
        Binding {
            label: "a function output naming a held function".to_string(),
            config: function_output_config(&function_arn),
        },
        Binding {
            label: "custom questions naming the DRAFT version of a held question set"
                .to_string(),
            config: custom_questions_config(&question_set_id, "DRAFT"),
        },
        Binding {
            label: format!("custom questions naming published version {version}"),
            config: custom_questions_config(&question_set_id, &version),
        },
    ]
}

/// A reference that RESOLVES is stored and reported back, by every operation that writes
/// outputs into a feed.
#[tokio::test]
async fn a_resolving_reference_is_stored_by_every_write_path() {
    let router = router();
    for binding in bindings(&router).await {
        let case = binding.label.as_str();

        // CreateFeed stores it, and the create's own answer already reports it.
        let (status, created) = create_feed(&router, "bound-feed", "out1", &binding.config).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateFeed answers 202, got {status} with {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports an id, got {created}"))
            .to_string();
        assert_eq!(
            stored_config(&created, "out1", case),
            binding.config,
            "case {case:?}: the create reports the config it stored"
        );

        // A read reports the same config, so the binding survived the write.
        let feed = get_feed(&router, &id, case).await;
        assert_eq!(
            stored_config(&feed, "out1", case),
            binding.config,
            "case {case:?}: the stored output keeps the reference, got {feed}"
        );

        // UpdateFeed, a full-replacement PUT of the config just read, keeps it.
        let read_back = stored_config(&feed, "out1", case);
        let (status, updated) = update_feed(&router, &id, "out1", &read_back).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            stored_config(&updated, "out1", case),
            binding.config,
            "case {case:?}: the round trip keeps the reference"
        );

        // AssociateFeed appends an output carrying the reference; the append runs on a
        // settled feed, so the reconfiguration the PUT started is waited out first.
        settled_feed(&router, &id, case).await;
        let (status, claimed) =
            associate_feed(&router, &id, "source-media", "out2", &binding.config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateFeed answers 200, got {status} with {claimed}"
        );
        let feed = get_feed(&router, &id, case).await;
        assert_eq!(
            stored_config(&feed, "out2", case),
            binding.config,
            "case {case:?}: the appended output carries the reference too, got {feed}"
        );
    }
}

/// The resource an output names reports the FEED back: a function lists the feed under
/// `references` and reports `REFERENCED`, and a question set lists it under the
/// references of the version the output named — and of no other version.
#[tokio::test]
async fn the_named_resource_reports_the_feed_that_references_it() {
    let router = router();
    let (function_id, function_arn) = create_function(&router, "referenced-function").await;
    let question_set_id = create_question_set(&router, "referenced-question-set").await;
    let (status, published) = call(
        &router,
        "POST",
        &format!("/v1/question-set/{question_set_id}/versions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the question set publishes a version, got {status} with {published}"
    );
    let version = published["version"].as_str().unwrap_or_default().to_string();

    let (status, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "referencing-feed",
            "outputs": [
                {
                    "name": "fn1",
                    "outputConfig": function_output_config(&function_arn),
                    "status": "ENABLED"
                },
                {
                    "name": "meta1",
                    "outputConfig": custom_questions_config(&question_set_id, &version),
                    "status": "ENABLED"
                }
            ]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the referencing feed is created, got {status} with {created}"
    );
    let feed_id = created["id"].as_str().unwrap_or_default().to_string();

    let (status, function) = call(&router, "GET", &format!("/v1/function/{function_id}"), None)
        .await;
    assert_eq!(
        (status, function["references"].clone(), function["status"].clone()),
        (200, json!([feed_id.clone()]), json!("CREATING")),
        "the create's own status is CREATING while the accepted creation is still in flight, \
         got {status} with {function}"
    );
    // Once the clock has settled the function, the reference set is what decides the
    // status it reports: REFERENCED while a feed's function output names it.
    let function = poll_until(
        &router,
        &format!("/v1/function/{function_id}"),
        "a function a feed references",
        |status, function| status == 200 && function["status"] != json!("CREATING"),
    )
    .await
    .1;
    assert_eq!(
        (function["references"].clone(), function["status"].clone()),
        (json!([feed_id.clone()]), json!("REFERENCED")),
        "the settled function reports the feed that references it, got {function}"
    );

    // The references of a question set are per VERSION: the version the output named
    // lists the feed, and DRAFT — which no output named — lists nothing.
    for (asked, expected) in [
        (version.as_str(), json!([feed_id.clone()])),
        ("DRAFT", json!([])),
    ] {
        let (status, question_set) = call(
            &router,
            "GET",
            &format!("/v1/question-set/{question_set_id}?questionSetVersion={asked}"),
            None,
        )
        .await;
        assert_eq!(
            (status, question_set["references"].clone()),
            (200, expected),
            "case {asked:?}: the question set reports the feeds that reference THAT version, \
             got {status} with {question_set}"
        );
    }
}

/// A reference that resolves to nothing names no resource this endpoint holds: every write
/// path refuses it with the modelled `ValidationException` (HTTP 400) and stores nothing.
#[tokio::test]
async fn a_reference_that_resolves_to_nothing_is_refused() {
    let router = router();
    let (_function_id, function_arn) = create_function(&router, "probe-function").await;
    let question_set_id = create_question_set(&router, "probe-question-set").await;

    // A feed to refuse the writes against, carrying an output the refused calls must
    // leave untouched.
    let (status, probe) = create_feed(
        &router,
        "probe-feed",
        "out1",
        &function_output_config(&function_arn),
    )
    .await;
    assert_eq!(
        status, 202,
        "the probe feed is created, got {status} with {probe}"
    );
    let probe_id = probe["id"].as_str().unwrap_or_default().to_string();

    // An id of the form this service mints that names no resource of this store: 19
    // lower-case alphanumerics, as `FunctionId` and `QuestionSetId` are both spelled
    // (`^[a-z0-9]{19}$`).
    let absent_id = "aaaaaaaaaaaaaaaaaaa";
    let absent_arn = format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{}:function/{absent_id}",
        engine::EMULATOR_ACCOUNT_ID
    );
    // The rows differ along which field of a resolving reference is moved: the resource
    // kind, the id, the Region, and — for a question set — the version.
    let rows: Vec<(String, Value, String)> = vec![
        (
            "a function ARN whose id names no function".to_string(),
            function_output_config(&absent_arn),
            cannot_find_function_message(&absent_arn),
        ),
        (
            "a function ARN of another Region".to_string(),
            function_output_config(&function_arn.replace(REGION, "eu-west-1")),
            cannot_find_function_message(&function_arn.replace(REGION, "eu-west-1")),
        ),
        (
            "a function ARN under the question set resource kind".to_string(),
            function_output_config(&function_arn.replace(":function/", ":question-set/")),
            cannot_find_function_message(&function_arn.replace(":function/", ":question-set/")),
        ),
        (
            "custom questions naming no question set".to_string(),
            custom_questions_config(absent_id, "DRAFT"),
            cannot_find_question_set_message(absent_id),
        ),
        (
            "custom questions naming a version that was never published".to_string(),
            custom_questions_config(&question_set_id, "7"),
            cannot_find_question_set_version_message(&question_set_id, "7"),
        ),
    ];

    for (label, config, expected_message) in rows {
        let case = label.as_str();

        // CreateFeed: refused, and no feed is minted for it.
        let (status, refused) = create_feed(&router, "refused-feed", "out1", &config).await;
        assert_eq!(
            (status, refused["__type"].clone(), refused["message"].clone()),
            (
                400,
                json!("ValidationException"),
                json!(expected_message.clone())
            ),
            "case {case:?}: CreateFeed refuses it with the modelled 400 naming the value the \
             request supplied, got {status} with {refused}"
        );
        let (status, listed) = call(&router, "GET", "/v1/feeds?maxResults=100", None).await;
        assert_eq!(
            (status, listed["feeds"].as_array().map(Vec::len)),
            (200, Some(1)),
            "case {case:?}: the refused create minted no feed, got {status} with {listed}"
        );

        // UpdateFeed and AssociateFeed: refused on a stored feed, which keeps what it
        // already holds.
        let before = get_feed(&router, &probe_id, case).await;
        for (path, (status, refused)) in [
            (
                "UpdateFeed",
                update_feed(&router, &probe_id, "out1", &config).await,
            ),
            (
                "AssociateFeed",
                associate_feed(&router, &probe_id, "source-media", "out2", &config).await,
            ),
        ] {
            assert_eq!(
                (status, refused["__type"].clone(), refused["message"].clone()),
                (
                    400,
                    json!("ValidationException"),
                    json!(expected_message.clone())
                ),
                "case {case:?}: {path} refuses it with the modelled 400 naming the value the \
                 request supplied, got {status} with {refused}"
            );
        }
        let after = get_feed(&router, &probe_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused writes left the feed as it was, got {after}"
        );
    }
}
