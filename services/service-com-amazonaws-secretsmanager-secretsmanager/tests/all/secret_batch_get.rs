// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetSecretValue`, driven over the wire through the
//! multi-service router and cross-checked against `GetSecretValue`, `ListSecrets`
//! and `PutSecretValue`.
//!
//! The rules under test:
//!
//! * a problem with the REQUEST fails the whole call — neither selector, both
//!   selectors, a repeated `SecretId`, or a list longer than one batch — while a
//!   problem with ONE selected secret is one entry in `Errors` inside a 200;
//! * every selected secret is answered with the value of its current version,
//!   the same value the single read resolves for it;
//! * the filter form selects the secrets the listing's filters select, and pages
//!   with `MaxResults` / `NextToken` over that selection.
//!
//! Every test is a TABLE over the class its rule generalises: which selector the
//! request carries, where in the list an id repeats, how long the list is, the
//! state of each selected secret, the filter key and value, the page size, and
//! the type of the stored value.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::SECRETS_PER_BATCH;
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service answers a secret it cannot find with.
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The sentence an operation a secret's state forbids carries.
const MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail with a 400, and return its wire error code and
/// message.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The `SecretValues` of a batched read, as (Name, SecretString) pairs in the
/// order they were answered.
fn values_of(parsed: &Value, case: &str) -> Vec<(String, Option<String>)> {
    parsed
        .get("SecretValues")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {parsed}"))
        .iter()
        .map(|entry| {
            (
                text(entry, "Name", case),
                entry
                    .get("SecretString")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// The `Errors` of a batched read, as (SecretId, ErrorCode, Message) triples.
fn errors_of(parsed: &Value, case: &str) -> Vec<(String, String, String)> {
    parsed
        .get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Errors in {parsed}"))
        .iter()
        .map(|entry| {
            (
                text(entry, "SecretId", case),
                text(entry, "ErrorCode", case),
                text(entry, "Message", case),
            )
        })
        .collect()
}

/// Create a secret holding one string value, and report its ARN.
async fn create(router: &MultiServiceRouter, name: &str, value: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": value}),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

#[tokio::test]
async fn a_batched_read_needs_exactly_one_selector() {
    // "You must include Filters or SecretIdList, but not both."
    // (`BatchGetSecretValueRequest$SecretIdList`, this service's Smithy model.)
    // The rows run over what the request carries beside the selectors, since the
    // selection is refused before anything else in the request is read.
    const NEITHER: &str = "You must specify a list of Secret IDs or a filter value.";
    const BOTH: &str = "You can't specify a secret ID list and filters in the same request.";
    let name = "batch-selector";
    let rows: [(&str, Value, &str); 5] = [
        ("neither selector", json!({}), NEITHER),
        ("neither selector, with MaxResults", json!({"MaxResults": 5}), NEITHER),
        (
            "both selectors",
            json!({"SecretIdList": [name], "Filters": [{"Key": "name", "Values": [name]}]}),
            BOTH,
        ),
        (
            "both selectors, no filter value",
            json!({"SecretIdList": [name], "Filters": []}),
            BOTH,
        ),
        (
            "both selectors, several filters",
            json!({
                "SecretIdList": [name],
                "Filters": [
                    {"Key": "name", "Values": [name]},
                    {"Key": "all", "Values": ["batch"]},
                ],
            }),
            BOTH,
        ),
    ];

    for (case, body, expected) in rows {
        let router = emulator();
        create(&router, name, "stored", case).await;
        let (code, message) = err(&router, "BatchGetSecretValue", body, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?} with message \
             {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_is_refused_without_the_filter_form() {
    // "To use this parameter, you must also use the Filters parameter."
    // (`BatchGetSecretValueRequest$MaxResults`, this service's Smithy model.) A
    // page size bounds a selection the service made, so a caller that spelled the
    // ids out has nothing for it to bound. The rows run over the page size — each
    // published bound of `MaxResultsBatchType` (1..20) and a size between them —
    // and over how many ids the list carries.
    const MAX_RESULTS_WITH_A_LIST: &str = "Max results can only be used with filters.";
    let router = emulator();
    let case = "setup";
    create(&router, "capped-one", "one", case).await;
    create(&router, "capped-two", "two", case).await;

    for page_size in [1usize, 7, SECRETS_PER_BATCH] {
        for ids in [vec!["capped-one"], vec!["capped-one", "capped-two"]] {
            let case = format!("MaxResults {page_size} beside {} id(s)", ids.len());
            let (code, message) = err(
                &router,
                "BatchGetSecretValue",
                json!({"SecretIdList": ids, "MaxResults": page_size}),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code:?} with message \
                 {message:?}"
            );
            assert_eq!(
                message, MAX_RESULTS_WITH_A_LIST,
                "case {case:?}: expected {MAX_RESULTS_WITH_A_LIST:?}, got {message:?}"
            );
        }
        // The INVARIANT the refusal must not disturb: the same page size is what the
        // FILTER form pages its selection by.
        let case = format!("MaxResults {page_size} beside a filter");
        let paged = ok(
            &router,
            "BatchGetSecretValue",
            json!({
                "Filters": [{"Key": "name", "Values": ["capped-"]}],
                "MaxResults": page_size,
            }),
            &case,
        )
        .await;
        let answered = values_of(&paged, &case).len();
        let expected = page_size.min(2);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected a page of {expected} entries, got {paged}"
        );
    }
}

#[tokio::test]
async fn a_repeated_secret_id_fails_the_whole_batch() {
    // "You cannot enter duplicate secrets in the secret ID list." The rows run
    // over WHERE the repeat sits, how many ids surround it, how often it
    // repeats, and the form the repeated id takes (name or ARN).
    let router = emulator();
    let case = "setup";
    let first = create(&router, "batch-one", "one", case).await;
    create(&router, "batch-two", "two", case).await;
    create(&router, "batch-three", "three", case).await;

    // (label, the list the call carries, the id the message must name)
    let rows: [(&str, Vec<String>, String); 5] = [
        (
            "the only two ids",
            vec!["batch-one".to_string(), "batch-one".to_string()],
            "batch-one".to_string(),
        ),
        (
            "the last of three",
            vec![
                "batch-two".to_string(),
                "batch-three".to_string(),
                "batch-two".to_string(),
            ],
            "batch-two".to_string(),
        ),
        (
            "straddling another id",
            vec![
                "batch-three".to_string(),
                "batch-one".to_string(),
                "batch-two".to_string(),
                "batch-three".to_string(),
            ],
            "batch-three".to_string(),
        ),
        (
            "three times over",
            vec![
                "batch-one".to_string(),
                "batch-one".to_string(),
                "batch-one".to_string(),
            ],
            "batch-one".to_string(),
        ),
        (
            "the ARN form repeated",
            vec![first.clone(), "batch-two".to_string(), first.clone()],
            first.clone(),
        ),
    ];

    for (label, ids, repeated) in rows {
        let case = format!("{label}: {ids:?}");
        let (code, message) = err(
            &router,
            "BatchGetSecretValue",
            json!({"SecretIdList": ids}),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException for a repeated id, got {code:?} with \
             message {message:?}"
        );
        let expected = format!(
            "Duplicate secret ID encountered: {repeated}. You cannot enter duplicate secrets in \
             the secret ID list."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_secret_id_list_longer_than_one_batch_is_refused() {
    // "Retrieves the contents of the encrypted fields SecretString or
    // SecretBinary for up to 20 secrets" (`BatchGetSecretValue`), and
    // `SecretIdListType` is modelled with a maximum length of 20 — so the rows
    // run over the published bound, one past it, and further lengths above it,
    // against the bound itself being accepted.
    let expected = format!(
        "The secret ID list has too many IDs. You can include up to {SECRETS_PER_BATCH} secret \
         IDs."
    );
    for length in [SECRETS_PER_BATCH + 1, SECRETS_PER_BATCH + 5, 64] {
        let case = format!("a list of {length} ids");
        let router = emulator();
        create(&router, "batch-capped", "stored", &case).await;
        let ids: Vec<String> = (0..length).map(|index| format!("absent-{index}")).collect();
        let (code, message) = err(
            &router,
            "BatchGetSecretValue",
            json!({"SecretIdList": ids}),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException for an over-long list, got \
             {code:?} with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The bound itself, and one id, are accepted: the refusal is the length's.
    for length in [1, SECRETS_PER_BATCH] {
        let case = format!("a list of {length} ids (inside the bound)");
        let router = emulator();
        create(&router, "batch-accepted", "stored", &case).await;
        let mut ids: Vec<String> = vec!["batch-accepted".to_string()];
        ids.extend((1..length).map(|index| format!("absent-{index}")));
        let read = ok(
            &router,
            "BatchGetSecretValue",
            json!({"SecretIdList": ids}),
            &case,
        )
        .await;
        assert_eq!(
            values_of(&read, &case),
            vec![("batch-accepted".to_string(), Some("stored".to_string()))],
            "case {case:?}: expected the one stored secret to be read, got {read}"
        );
        assert_eq!(
            errors_of(&read, &case).len(),
            length - 1,
            "case {case:?}: expected one Errors entry per absent id, got {read}"
        );
    }
}

#[tokio::test]
async fn one_bad_secret_is_an_errors_entry_inside_a_two_hundred() {
    // "If Secrets Manager encounters errors ... while attempting to retrieve any
    // of the secrets, you can see the errors in Errors in the response."
    // (`BatchGetSecretValue`, this service's Smithy model.) The rows run over the
    // STATE that makes one selected secret unreadable, and over the form of the
    // id selecting it.
    let router = emulator();
    let case = "setup";
    let readable_arn = create(&router, "batch-readable", "readable", case).await;
    create(&router, "batch-deleted", "doomed", case).await;
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "batch-deleted", "RecoveryWindowInDays": 7}),
        case,
    )
    .await;
    // A secret created with no value holds no version, so it has no current
    // value to read.
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "batch-valueless"}),
        case,
    )
    .await;

    // (the id selecting the secret, the error code and message it must carry)
    let account = engine::EMULATOR_ACCOUNT_ID;
    let absent_arn =
        format!("arn:aws:secretsmanager:us-west-2:{account}:secret:batch-absent-AbCdEf");
    let rows: [(&str, bool, &str, String); 4] = [
        (
            "batch-absent",
            false,
            "ResourceNotFoundException",
            SECRET_NOT_FOUND.to_string(),
        ),
        (
            &absent_arn,
            true,
            "ResourceNotFoundException",
            SECRET_NOT_FOUND.to_string(),
        ),
        (
            "batch-deleted",
            true,
            "InvalidRequestException",
            MARKED_FOR_DELETION.to_string(),
        ),
        (
            "batch-valueless",
            false,
            "ResourceNotFoundException",
            "Secrets Manager can't find the specified secret value for staging label: AWSCURRENT"
                .to_string(),
        ),
    ];

    for (secret_id, by_arn, expected_code, expected_message) in rows {
        let case = format!("a batch holding {secret_id:?}");
        // The readable secret is named by its ARN in some rows and by its name in
        // the others, so the entry channel is exercised for both forms.
        let readable: &str = if by_arn { &readable_arn } else { "batch-readable" };
        let read = ok(
            &router,
            "BatchGetSecretValue",
            json!({"SecretIdList": [readable, secret_id]}),
            &case,
        )
        .await;
        assert_eq!(
            values_of(&read, &case),
            vec![("batch-readable".to_string(), Some("readable".to_string()))],
            "case {case:?}: expected the readable secret to still be answered, got {read}"
        );
        assert_eq!(
            errors_of(&read, &case),
            vec![(
                secret_id.to_string(),
                expected_code.to_string(),
                expected_message.clone(),
            )],
            "case {case:?}: expected one Errors entry naming the id as sent, got {read}"
        );
        assert!(
            read.get("NextToken").is_none(),
            "case {case:?}: expected no NextToken for a single page, got {read}"
        );
    }
}

#[tokio::test]
async fn a_batched_read_answers_the_value_the_single_read_answers() {
    // The batched read resolves the current version of each secret, so it agrees
    // with `GetSecretValue` on every one. The rows run over the type and size of
    // the stored value and over how the current version got there — created with
    // it, or moved to a later version by `PutSecretValue`.
    let values: [(&str, Value, Option<&str>); 4] = [
        ("a short string", json!({"SecretString": "short"}), Some("short")),
        (
            "a long string",
            json!({"SecretString": "x".repeat(4096)}),
            None, // compared against GetSecretValue rather than a literal
        ),
        ("binary", json!({"SecretBinary": "AAECAwQ="}), None),
        ("a JSON document", json!({"SecretString": "{\"user\":\"a\"}"}), Some("{\"user\":\"a\"}")),
    ];

    for (label, value, expected) in values {
        for rotated in [false, true] {
            let case = format!("{label}, a later version put: {rotated}");
            let router = emulator();
            let name = "batch-round-trip";
            let mut create_body = value.clone();
            create_body
                .as_object_mut()
                .expect("every row's value is an object")
                .insert("Name".to_string(), json!(name));
            ok(&router, "CreateSecret", create_body, &case).await;
            if rotated {
                let mut put = value.clone();
                let members = put.as_object_mut().expect("a request body is an object");
                members.insert("SecretId".to_string(), json!(name));
                members.insert("ClientRequestToken".to_string(), json!("b".repeat(32)));
                ok(&router, "PutSecretValue", put, &case).await;
            }

            let single = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
            let batched = ok(
                &router,
                "BatchGetSecretValue",
                json!({"SecretIdList": [name]}),
                &case,
            )
            .await;
            let entries = batched
                .get("SecretValues")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {batched}"));
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: expected one entry for one id, got {batched}"
            );
            let entry = &entries[0];
            for member in ["ARN", "Name", "VersionId", "SecretString", "SecretBinary"] {
                assert_eq!(
                    entry.get(member),
                    single.get(member),
                    "case {case:?}: expected {member} to agree with GetSecretValue, got \
                     {entry:?} against {single:?}"
                );
            }
            assert_eq!(
                entry.get("VersionStages"),
                single.get("VersionStages"),
                "case {case:?}: expected the staging labels to agree with GetSecretValue, got \
                 {entry:?}"
            );
            if let Some(expected) = expected {
                assert_eq!(
                    entry.get("SecretString").and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected the stored value {expected:?}, got {entry:?}"
                );
            }
            assert_eq!(
                errors_of(&batched, &case),
                vec![],
                "case {case:?}: expected no Errors entry for a readable secret, got {batched}"
            );
        }
    }
}

#[tokio::test]
async fn the_filter_form_selects_what_the_listing_selects() {
    // The filter form "chooses which secrets to retrieve" with the same `Filter`
    // shape `ListSecrets` filters on, so the two agree on every key. The rows run
    // over the filter key (`Filter$Key`: name, description, tag-key, tag-value,
    // all), over a negated value, and over a key that selects nothing.
    let router = emulator();
    let case = "setup";
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "prod/alpha",
            "SecretString": "alpha",
            "Description": "Alpha service credentials",
            "Tags": [{"Key": "env", "Value": "prod"}],
        }),
        case,
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": "prod/beta",
            "SecretString": "beta",
            "Description": "Beta service credentials",
            "Tags": [{"Key": "env", "Value": "staging"}],
        }),
        case,
    )
    .await;
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "dev/gamma", "SecretString": "gamma", "Description": "Scratch"}),
        case,
    )
    .await;
    // A secret inside its recovery window is not selected by a filter, the same
    // way the listing leaves it out unless the caller asks for it.
    ok(
        &router,
        "CreateSecret",
        json!({"Name": "prod/deleted", "SecretString": "doomed"}),
        case,
    )
    .await;
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "prod/deleted", "RecoveryWindowInDays": 30}),
        case,
    )
    .await;

    // (the filters the call carries, the names it must answer, in name order)
    let rows: [(Value, Vec<&str>); 7] = [
        (json!([{"Key": "name", "Values": ["prod/"]}]), vec!["prod/alpha", "prod/beta"]),
        (json!([{"Key": "name", "Values": ["dev/"]}]), vec!["dev/gamma"]),
        (json!([{"Key": "description", "Values": ["alpha"]}]), vec!["prod/alpha"]),
        (json!([{"Key": "tag-key", "Values": ["env"]}]), vec!["prod/alpha", "prod/beta"]),
        (json!([{"Key": "tag-value", "Values": ["prod"]}]), vec!["prod/alpha"]),
        (json!([{"Key": "name", "Values": ["!prod/"]}]), vec!["dev/gamma"]),
        (json!([{"Key": "name", "Values": ["nothing-matches"]}]), vec![]),
    ];

    for (filters, expected) in rows {
        let case = format!("filters {filters}");
        let read = ok(
            &router,
            "BatchGetSecretValue",
            json!({"Filters": filters.clone()}),
            &case,
        )
        .await;
        let selected: Vec<String> = values_of(&read, &case)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the filters to select {expected:?}, got {selected:?}"
        );
        assert_eq!(
            errors_of(&read, &case),
            vec![],
            "case {case:?}: expected the filter form to report no per-entry error, got {read}"
        );

        // The same filters through the listing select the same secrets.
        let listed = ok(&router, "ListSecrets", json!({"Filters": filters}), &case).await;
        let listed_names: Vec<String> = listed
            .get("SecretList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
            .iter()
            .map(|entry| text(entry, "Name", &case))
            .collect();
        assert_eq!(
            listed_names, expected,
            "case {case:?}: expected the listing to select the same secrets, got {listed_names:?}"
        );
    }
}

#[tokio::test]
async fn the_filter_form_pages_over_its_selection() {
    // "The number of results to include in the response. If there are more
    // results available, in the response, Secrets Manager includes NextToken."
    // (`BatchGetSecretValueRequest$MaxResults`, this service's Smithy model.) The
    // rows run over the page size — each published bound of
    // `MaxResultsBatchType` (1..20) and sizes between them — and over how many
    // secrets the filter selects.
    for population in [1usize, 3, 7] {
        for page_size in [1usize, 2, 5, SECRETS_PER_BATCH] {
            let case = format!("{population} secrets in pages of {page_size}");
            let router = emulator();
            let expected: Vec<String> = (0..population)
                .map(|index| format!("paged/{index:02}"))
                .collect();
            for name in &expected {
                create(&router, name, "stored", &case).await;
            }

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({
                    "Filters": [{"Key": "name", "Values": ["paged/"]}],
                    "MaxResults": page_size,
                });
                if let Some(token) = &token {
                    body.as_object_mut()
                        .expect("a request body is an object")
                        .insert("NextToken".to_string(), json!(token));
                }
                let page = ok(&router, "BatchGetSecretValue", body, &case).await;
                let names: Vec<String> = values_of(&page, &case)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
                assert!(
                    !names.is_empty() && names.len() <= page_size,
                    "case {case:?}: expected a non-empty page of at most {page_size}, got {page}"
                );
                seen.extend(names);
                token = page
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    seen.len() <= population,
                    "case {case:?}: paging answered {} entries for {population} secrets",
                    seen.len()
                );
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: expected paging to walk the selection exactly once, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_pagination_token_this_read_cannot_resume_from_is_refused() {
    // A token that names no selection of this read resumes nothing, so it is
    // `InvalidNextTokenException` carrying the one constant sentence. The rows
    // run over the forms such a token takes; `NextTokenType` is modelled with a
    // length bound of 1..4096, so every row stays inside it.
    const INVALID_NEXT_TOKEN: &str = "The pagination token is no longer valid.";
    let tokens = [
        "not-a-token".to_string(),
        "zzzz".to_string(),
        "6e6f742d696e2d746869732d6c697374696e67".to_string(),
        "f".repeat(4000),
    ];
    for token in tokens {
        let case = format!("NextToken {:.32?}", token);
        let router = emulator();
        create(&router, "batch-paged", "stored", &case).await;
        let (code, message) = err(
            &router,
            "BatchGetSecretValue",
            json!({
                "Filters": [{"Key": "name", "Values": ["batch-"]}],
                "NextToken": token,
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {code:?} with message \
             {message:?}"
        );
        assert_eq!(
            message, INVALID_NEXT_TOKEN,
            "case {case:?}: expected {INVALID_NEXT_TOKEN:?}, got {message:?}"
        );
    }
}
