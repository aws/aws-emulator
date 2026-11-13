// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 portal product sharing policy —
//! `PutPortalProductSharingPolicy`, `GetPortalProductSharingPolicy` and
//! `DeletePortalProductSharingPolicy`.
//!
//! The rules these cases assert:
//!
//! * **The sharing policy is a sub-resource of ONE portal product**
//!   (`/v2/portalproducts/{PortalProductId}/sharingpolicy`): a PUT stores the request's
//!   `PolicyDocument` under the product the URI names and answers 200, and a GET reports
//!   that stored document back verbatim beside the product's own id. A policy written on
//!   one product is not visible on another.
//! * **A PUT replaces the sub-resource wholesale and is idempotent**: the last document
//!   written is the whole policy afterwards, whatever the product held before.
//! * **A sharing policy a product does not hold reads as the EMPTY document**: a
//!   product nothing has shared reports an empty `PolicyDocument`, and so does one whose
//!   policy was deleted — `DeletePortalProductSharingPolicy` answers the modelled 204 and
//!   resets the product to holding none, idempotently, leaving the product itself intact.
//! * **The product is the resource that has to exist**: a `PortalProductId` nothing
//!   minted is 404 `NotFoundException` from all three operations.
//! * **A `PolicyDocument` that is not a resource-based IAM policy document is refused
//!   400** and persists nothing, as is one outside the modelled bound (`len 1..307200`).
//! * **A GET is a report, not a write**: reading twice reports the same policy and
//!   leaves what a later caller sees unchanged.
//!
//! The rows vary what the rules generalise over: the policy document's form (one statement,
//! several statements with conditions, a statement written as an object rather than a list,
//! the complement elements, each policy version there is), its size (up to the modelled
//! maximum length), its formatting (compact and pretty-printed, which must round-trip byte
//! for byte) and its script (outside ASCII) — and, for the refusals, each clause of the
//! policy grammar a document can break. The recorded lifecycle that filed the issue — one
//! put, read back, then deleted — is ONE row of the first table.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::portal_product_not_found_message;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The portal-products collection URI (`CreatePortalProduct`'s modelled
/// `POST /v2/portalproducts`).
const PRODUCTS: &str = "/v2/portalproducts";

/// The modelled maximum length of a `PolicyDocument`
/// (`PutPortalProductSharingPolicy$PolicyDocument` targets `__stringMin1Max307200`,
/// len 1..307200).
const MAX_POLICY_LENGTH: usize = 307_200;

/// The `PolicyDocument` a product's sharing policy reads as while the product holds no
/// policy of its own: the empty JSON policy document, a policy granting nothing.
const EMPTY_POLICY: &str = "{}";

/// The sharing-policy sub-resource URI of one product.
fn policy_uri(product_id: &str) -> String {
    format!("{PRODUCTS}/{product_id}/sharingpolicy")
}

/// Create a portal product and return its minted id.
async fn create_product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    display_name: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": display_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "product {display_name:?}: expected 201 from CreatePortalProduct, got {status} {created}"
    );
    as_str(&created, "portalProductId")
}

/// A resource policy exactly `length` characters long: one statement whose `Sid` is padded
/// to size. The bound this exercises is on the DOCUMENT's length, so what fills it does not
/// matter — but it still has to BE a policy, which is the other rule a put applies.
fn policy_of_length(length: usize) -> String {
    let prefix = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"";
    let suffix = "\",\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"apigateway:GET\"}]}";
    let padding = length - prefix.chars().count() - suffix.chars().count();
    format!("{prefix}{}{suffix}", "p".repeat(padding))
}

/// One statement every accepted document below can be built around: the three elements a
/// RESOURCE-based policy's statement has to name (an effect, an action, a principal).
fn one_statement() -> Value {
    json!({
        "Effect": "Allow",
        "Principal": "*",
        "Action": "apigateway:GET"
    })
}

/// The policy documents a put must store and a get must report back unchanged.
fn accepted_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "a resource policy with one statement",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
                    "Action": "apigateway:GET",
                    "Resource": "*"
                }]
            })
            .to_string(),
        ),
        (
            "a policy with several statements and a condition",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "ShareWithOrg",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": ["apigateway:GET", "apigateway:POST"],
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": { "aws:PrincipalOrgID": "o-exampleorgid" }
                        }
                    },
                    {
                        "Sid": "DenyEveryoneElse",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "apigateway:*",
                        "Resource": "*"
                    }
                ]
            })
            .to_string(),
        ),
        (
            // The language writes ONE statement either as a list of one or as the object
            // itself, and a document may leave `Version` out.
            "a single statement written as an object rather than a list",
            json!({ "Statement": one_statement() }).to_string(),
        ),
        (
            // `NotAction` and `NotPrincipal` are the complements of two of those elements.
            "a statement naming its action and principal by their complements",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": { "AWS": "arn:aws:iam::111122223333:root" },
                    "NotAction": "apigateway:*"
                }]
            })
            .to_string(),
        ),
        (
            "the legacy policy version, which the language still admits",
            json!({ "Version": "2008-10-17", "Statement": [one_statement()] }).to_string(),
        ),
        (
            "a pretty-printed document, which round-trips byte for byte",
            ::serde_json::to_string_pretty(
                &json!({ "Version": "2012-10-17", "Statement": [one_statement()] }),
            )
            .expect("a JSON value renders"),
        ),
        (
            "a document carrying text outside ASCII",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "共有ポリシー — ünïcödé, emoji 🎉",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "apigateway:GET"
                }]
            })
            .to_string(),
        ),
        (
            "a document at the modelled maximum length",
            policy_of_length(MAX_POLICY_LENGTH),
        ),
    ]
}

#[tokio::test]
async fn a_put_policy_is_reported_back_by_get_for_every_document_form() {
    let emulator = emulator();

    for (case, document) in accepted_documents() {
        let product_id = create_product(&emulator, "sharing policy host").await;
        let uri = policy_uri(&product_id);

        // Before the put, the product holds no sharing policy: its policy reads as the
        // empty document, beside the product's own id.
        let (status, before) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProductSharingPolicy on a product \
             with no policy, got {status} {before}"
        );
        assert_eq!(
            before.get("policyDocument"),
            Some(&Value::String(EMPTY_POLICY.to_string())),
            "case {case:?}: expected an empty policyDocument before the put, got {before}"
        );
        assert_eq!(
            as_str(&before, "portalProductId"),
            product_id,
            "case {case:?}: expected the product's own id, got {before}"
        );

        let (status, put) = call(
            &emulator,
            "PUT",
            &uri,
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from PutPortalProductSharingPolicy, got {status} {put}"
        );

        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProductSharingPolicy, got \
             {status} {fetched}"
        );
        assert_eq!(
            fetched.get("policyDocument"),
            Some(&Value::String(document.clone())),
            "case {case:?}: expected the stored document reported back verbatim, got \
             {}",
            as_str(&fetched, "policyDocument")
        );
        assert_eq!(
            as_str(&fetched, "portalProductId"),
            product_id,
            "case {case:?}: expected the policy's own product id, got {fetched}"
        );

        // Reading the policy changes nothing a later caller sees.
        let (status, again) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            (status, &again),
            (200, &fetched),
            "case {case:?}: expected two reads to report the same policy, got \
             {status} {again}"
        );
    }
}

#[tokio::test]
async fn a_put_replaces_the_whole_policy_and_repeating_it_changes_nothing() {
    let emulator = emulator();
    let product_id = create_product(&emulator, "replaced policy").await;
    let uri = policy_uri(&product_id);

    // Each document in turn is the WHOLE policy after its put, whatever the product held
    // before: a put is a replace, not a merge.
    let documents: Vec<String> = accepted_documents()
        .into_iter()
        .map(|(_, document)| document)
        .collect();
    for (index, document) in documents.iter().enumerate() {
        for attempt in 0..2 {
            let (status, put) = call(
                &emulator,
                "PUT",
                &uri,
                Some(json!({ "policyDocument": document })),
            )
            .await;
            assert_eq!(
                status, 200,
                "document {index}, attempt {attempt}: expected 200 from \
                 PutPortalProductSharingPolicy, got {status} {put}"
            );

            let (status, fetched) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "document {index}, attempt {attempt}: expected 200 from \
                 GetPortalProductSharingPolicy, got {status} {fetched}"
            );
            assert_eq!(
                fetched.get("policyDocument"),
                Some(&Value::String(document.clone())),
                "document {index}, attempt {attempt}: expected the last document written \
                 to be the whole policy, got {}",
                as_str(&fetched, "policyDocument")
            );
        }
    }
}

#[tokio::test]
async fn a_policy_belongs_to_the_one_product_it_was_written_on() {
    let emulator = emulator();

    // One policy per product: writing on one product leaves every other product's policy
    // exactly as it was, including a product that holds none.
    let mut written: Vec<(String, String)> = Vec::new();
    for (index, (_, document)) in accepted_documents().into_iter().enumerate() {
        let product_id = create_product(&emulator, &format!("product-{index}")).await;
        let (status, put) = call(
            &emulator,
            "PUT",
            &policy_uri(&product_id),
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "product {index}: expected 200 from PutPortalProductSharingPolicy, got \
             {status} {put}"
        );
        written.push((product_id, document));
    }
    let bare = create_product(&emulator, "no policy of its own").await;

    for (index, (product_id, document)) in written.iter().enumerate() {
        let (status, fetched) = call(&emulator, "GET", &policy_uri(product_id), None).await;
        assert_eq!(
            status, 200,
            "product {index}: expected 200 from GetPortalProductSharingPolicy, got \
             {status} {fetched}"
        );
        assert_eq!(
            fetched.get("policyDocument"),
            Some(&Value::String(document.clone())),
            "product {index}: expected its own document, got {}",
            as_str(&fetched, "policyDocument")
        );
    }

    let (status, fetched) = call(&emulator, "GET", &policy_uri(&bare), None).await;
    assert_eq!(
        status, 200,
        "expected 200 for a product with no policy of its own, got {status} {fetched}"
    );
    assert_eq!(
        fetched.get("policyDocument"),
        Some(&Value::String(EMPTY_POLICY.to_string())),
        "expected a product nothing wrote a policy on to report the empty document, got \
         {fetched}"
    );
}

#[tokio::test]
async fn deleting_the_policy_resets_the_product_to_holding_none() {
    let emulator = emulator();

    for (case, document) in accepted_documents() {
        let product_id = create_product(&emulator, "deleted policy").await;
        let uri = policy_uri(&product_id);

        let (status, put) = call(
            &emulator,
            "PUT",
            &uri,
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from PutPortalProductSharingPolicy, got \
             {status} {put}"
        );

        // The delete is idempotent: a product carrying no policy is already in the state
        // the call asks for, so both calls answer the modelled 204 with no body.
        for attempt in 0..2 {
            let (status, deleted) = call(&emulator, "DELETE", &uri, None).await;
            assert_eq!(
                status, 204,
                "case {case:?}, attempt {attempt}: expected 204 from \
                 DeletePortalProductSharingPolicy, got {status} {deleted}"
            );
            assert_eq!(
                deleted,
                Value::Null,
                "case {case:?}, attempt {attempt}: expected an empty 204 body, got \
                 {deleted}"
            );

            let (status, fetched) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}, attempt {attempt}: expected 200 from \
                 GetPortalProductSharingPolicy after the delete, got {status} {fetched}"
            );
            assert_eq!(
                fetched.get("policyDocument"),
                Some(&Value::String(EMPTY_POLICY.to_string())),
                "case {case:?}, attempt {attempt}: expected the policy to read as the \
                 empty document after the delete, got {fetched}"
            );
            assert_eq!(
                as_str(&fetched, "portalProductId"),
                product_id,
                "case {case:?}, attempt {attempt}: expected the product to survive the \
                 delete of its policy, got {fetched}"
            );
        }

        // The product itself is untouched by the delete of its sub-resource, and it can
        // take a policy again.
        let (status, product) =
            call(&emulator, "GET", &format!("{PRODUCTS}/{product_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the product to still exist, got {status} {product}"
        );
        assert_eq!(
            as_str(&product, "portalProductId"),
            product_id,
            "case {case:?}: expected the product to survive the delete of its policy, got \
             {product}"
        );
        let (status, put) = call(
            &emulator,
            "PUT",
            &uri,
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a policy to be writable again after a delete, got \
             {status} {put}"
        );
    }
}

#[tokio::test]
async fn a_product_id_nothing_minted_is_not_found_by_any_of_the_three_operations() {
    let emulator = emulator();
    let product_id = create_product(&emulator, "the only product").await;
    let document = json!({ "Version": "2012-10-17", "Statement": [one_statement()] }).to_string();

    for (case, id) in [
        ("an id nothing minted", "nosuchprod0".to_string()),
        ("the id's uppercase form", product_id.to_uppercase()),
        (
            "the id with one character dropped",
            product_id[1..].to_string(),
        ),
        ("a long id nothing minted", "z".repeat(30)),
    ] {
        let uri = policy_uri(&id);
        let calls: Vec<(&str, &str, Option<Value>)> = vec![
            (
                "PutPortalProductSharingPolicy",
                "PUT",
                Some(json!({ "policyDocument": document })),
            ),
            ("GetPortalProductSharingPolicy", "GET", None),
            ("DeletePortalProductSharingPolicy", "DELETE", None),
        ];
        for (operation, method, body) in calls {
            let (status, refused) = call(&emulator, method, &uri, body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from {operation}, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NotFoundException",
                "case {case:?}: expected NotFoundException from {operation}, got {refused}"
            );
            assert_eq!(
                as_str(&refused, "message"),
                portal_product_not_found_message(&id),
                "case {case:?}: expected the portal product's not-found sentence from \
                 {operation}, which names the account the lookup was made in and the \
                 identifier the request addressed, got {refused}"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_document_that_is_not_a_json_policy_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let product_id = create_product(&emulator, "refused policies").await;
    let uri = policy_uri(&product_id);

    // A policy already stored, so a refused put is observable as "the stored policy did
    // not move" rather than only as a status code.
    let stored = json!({ "Version": "2012-10-17", "Statement": [one_statement()] }).to_string();
    let (status, put) = call(
        &emulator,
        "PUT",
        &uri,
        Some(json!({ "policyDocument": stored })),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from the first PutPortalProductSharingPolicy, got {status} {put}"
    );

    // `PolicyDocument` is a RESOURCE-BASED IAM policy document bounded at `len 1..307200`.
    // Two different refusals, both pinned to the `BadRequestException` the operation
    // models: a document that is not a policy is refused by the handler, and a document
    // outside the modelled length bound is refused at the boundary, before the handler
    // runs. The rows span every clause of the grammar a document can break.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a document that is not JSON at all",
            "not a policy".to_string(),
            "BadRequestException",
        ),
        (
            "a truncated JSON object",
            "{\"Version\":".to_string(),
            "BadRequestException",
        ),
        (
            "a JSON array rather than an object",
            "[]".to_string(),
            "BadRequestException",
        ),
        (
            "a bare JSON string",
            "\"Allow\"".to_string(),
            "BadRequestException",
        ),
        ("a bare JSON number", "17".to_string(), "BadRequestException"),
        ("JSON null", "null".to_string(), "BadRequestException"),
        (
            "an object that declares no statement at all",
            "{}".to_string(),
            "BadRequestException",
        ),
        (
            "a statement list that holds no statement",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
            "BadRequestException",
        ),
        (
            "a policy version that does not exist",
            json!({ "Version": "2024-01-01", "Statement": [one_statement()] }).to_string(),
            "BadRequestException",
        ),
        (
            "a statement that is not an object",
            json!({ "Version": "2012-10-17", "Statement": ["Allow"] }).to_string(),
            "BadRequestException",
        ),
        (
            "a statement that declares no Effect",
            json!({
                "Statement": [{ "Principal": "*", "Action": "apigateway:GET" }]
            })
            .to_string(),
            "BadRequestException",
        ),
        (
            "a statement whose Effect is neither Allow nor Deny",
            json!({
                "Statement": [{
                    "Effect": "Maybe",
                    "Principal": "*",
                    "Action": "apigateway:GET"
                }]
            })
            .to_string(),
            "BadRequestException",
        ),
        (
            "a statement that names no action",
            json!({ "Statement": [{ "Effect": "Allow", "Principal": "*" }] }).to_string(),
            "BadRequestException",
        ),
        (
            // A resource policy is the one that states WHO the permission is granted to.
            "a statement that names no principal",
            json!({ "Statement": [{ "Effect": "Allow", "Action": "apigateway:GET" }] })
                .to_string(),
            "BadRequestException",
        ),
        (
            "an empty document, below the modelled minimum",
            String::new(),
            "BadRequestException",
        ),
        (
            "a document one character past the modelled maximum",
            policy_of_length(MAX_POLICY_LENGTH + 1),
            "BadRequestException",
        ),
    ];

    for (case, document, expected_code) in rows {
        let (status, refused) = call(
            &emulator,
            "PUT",
            &uri,
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from PutPortalProductSharingPolicy, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );

        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProductSharingPolicy, got \
             {status} {fetched}"
        );
        assert_eq!(
            fetched.get("policyDocument"),
            Some(&Value::String(stored.clone())),
            "case {case:?}: expected the refused put to have changed nothing, got {}",
            as_str(&fetched, "policyDocument")
        );
    }
}
