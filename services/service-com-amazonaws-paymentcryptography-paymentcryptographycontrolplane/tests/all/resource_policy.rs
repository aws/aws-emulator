// SPDX-License-Identifier: CC0-1.0

//! A key's resource-based policy, end to end over the wire: `PutResourcePolicy` and
//! `GetResourcePolicy`.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what these
//! tests assert is what a signed SDK request observes.
//!
//! The rules under test:
//!
//! * a key carries ONE resource-based policy, attached to that key's stored record: an
//!   attach replaces whatever document was there, and the read answers exactly the stored
//!   text — in the layout an attached policy is reported in, so the document is
//!   re-serialized rather than echoed and re-attaching what was read changes nothing;
//! * a key no call has attached a policy to reports the empty document rather than
//!   refusing the read: the read refuses only an ARN that names no key;
//! * a document that would grant access to a principal nobody named is refused with
//!   `PublicPolicyException` and nothing is written — a grant reaches everybody when its
//!   principal element names the wildcard (bare or under any principal type) or excludes a
//!   named set, unless a condition binds the request to a caller something names;
//! * a text that is not a policy document this service attaches is refused with
//!   `ValidationException`, and an ARN naming no key with `ResourceNotFoundException`.
//!
//! Each case is a row of a table spanning what those rules generalise over: the form of the
//! principal element, the effect, how many statements a document carries, the whitespace it
//! was submitted with, which condition narrows a wildcard, and which clause a refused
//! document breaks.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    key_arn, NO_POLICY_DOCUMENT, PUBLIC_POLICY_MESSAGE,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{call, error_code, ok_call, router_with_engine, string, REGION};

/// One `CreateKey` body: a symmetric data-encryption key. The rules under test are about a
/// key's policy rather than about one algorithm, so every case uses the same key kind.
fn create_key_body() -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": "AES_128",
            "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
        },
        "Exportable": true,
    })
}

/// Create one key and answer its `KeyArn`.
async fn create_key(router: &MultiServiceRouter) -> String {
    let created = ok_call(router, "CreateKey", &create_key_body()).await;
    let key = created.get("Key").expect("CreateKey answers a Key");
    string(key, "KeyArn").to_string()
}

/// A one-statement document carrying `statement`.
fn policy(statement: &str) -> String {
    document(&[statement])
}

/// A document carrying `statements`, in the order given.
fn document(statements: &[&str]) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        statements.join(",")
    )
}

/// The layout an attached policy is reported in — the one rendering shared by every
/// emulated service that attaches a policy document
/// (`::service_common::iam_policy::canonical_document`), which is what the handler stores.
fn reported(submitted: &str) -> String {
    ::service_common::iam_policy::canonical_document(submitted)
}

/// The policy the engine actually holds for `arn`, so a response is never richer than the
/// record it came from.
fn stored_policy(engine: &Arc<engine::InMemoryEngine>, case: &str, arn: &str) -> Option<String> {
    let store = engine.state::<KeyStore>();
    let guard = store.lock();
    guard
        .keys
        .get(arn)
        .unwrap_or_else(|| panic!("case {case}: no key stored under {arn}"))
        .resource_policy
        .clone()
}

/// A call that must be refused with the modelled error class `code`.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert!(
        (400..500).contains(&status),
        "case {case}: expected {operation} to refuse with a client status, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        code,
        "case {case}: expected {code}, got {status} {parsed}"
    );
    parsed
}

/// Documents this service attaches: the rows differ in the form of the principal element
/// (a named account, a list of them, a service principal, a wildcard narrowed by a
/// condition), the effect, how many statements the document carries and the whitespace it
/// was submitted with.
fn attachable_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one named account",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Resource\":\"*\"}",
            ),
        ),
        (
            "two named accounts and every action of this service",
            policy(
                "{\"Sid\":\"cross-account\",\"Effect\":\"Allow\",\
                 \"Action\":\"payment-cryptography:*\",\
                 \"Principal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\",\
                 \"arn:aws:iam::000000000001:root\"]},\"Resource\":\"*\"}",
            ),
        ),
        (
            "a service principal and a list of actions",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":[\"payment-cryptography:GenerateMac\",\
                 \"payment-cryptography:VerifyMac\"],\
                 \"Principal\":{\"Service\":\"events.amazonaws.com\"}}",
            ),
        ),
        (
            "an allow beside an explicit deny",
            document(&[
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:*\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
                "{\"Effect\":\"Deny\",\"Action\":\"payment-cryptography:DeleteKey\",\
                 \"Principal\":\"*\"}",
            ]),
        ),
        (
            "a wildcard principal narrowed to one organization",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Condition\":{\"StringEquals\":\
                 {\"aws:PrincipalOrgID\":\"o-abc123\"}}}",
            ),
        ),
        (
            "a wildcard principal narrowed to one account",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":{\"AWS\":\"*\"},\"Condition\":{\"StringEquals\":\
                 {\"aws:PrincipalAccount\":\"000000000000\"}}}",
            ),
        ),
        (
            "the legacy policy language version, with one statement object",
            "{\"Version\":\"2008-10-17\",\"Id\":\"one\",\"Statement\":\
             {\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
             \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}}"
                .to_string(),
        ),
        (
            "the same document submitted with newlines and indentation",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Effect\": \"Allow\",\n      \"Action\": \"payment-cryptography:DecryptData\",\n      \
             \"Principal\": { \"AWS\": \"arn:aws:iam::000000000000:root\" }\n    }\n  ]\n}"
                .to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_attached_policy_is_the_one_document_every_read_reports() {
    let (router, engine) = router_with_engine();
    let arn = create_key(&router).await;

    // Before any attach the key carries no policy of its own, and the read says so with
    // the empty document rather than refusing.
    let empty = ok_call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        string(&empty, "Policy"),
        NO_POLICY_DOCUMENT,
        "a key no call has attached a policy to reports the empty document, got {empty}"
    );
    assert_eq!(
        stored_policy(&engine, "a key with no attached policy", &arn),
        None,
        "a read attaches nothing"
    );

    for (case, document) in attachable_documents() {
        let expected = reported(&document);
        let put = ok_call(
            &router,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": document}),
        )
        .await;
        assert_eq!(
            string(&put, "ResourceArn"),
            arn,
            "case {case}: the attach names the key it wrote to, got {put}"
        );
        assert_eq!(
            string(&put, "Policy"),
            expected,
            "case {case}: expected the attached document, got {put}"
        );

        // The read answers the one stored document, and the store holds exactly that: a
        // replace leaves no trace of the policy the key carried before.
        let got = ok_call(
            &router,
            "GetResourcePolicy",
            &json!({"ResourceArn": arn}),
        )
        .await;
        assert_eq!(
            string(&got, "Policy"),
            expected,
            "case {case}: expected the read to answer the attached document, got {got}"
        );
        assert_eq!(
            string(&got, "ResourceArn"),
            arn,
            "case {case}: the read names the key, got {got}"
        );
        assert_eq!(
            stored_policy(&engine, case, &arn).as_deref(),
            Some(expected.as_str()),
            "case {case}: the response must come from the stored record"
        );

        // Re-attaching what the read answered changes nothing: the reported layout is the
        // layout a document is stored in, so the round trip is idempotent.
        let again = ok_call(
            &router,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": expected}),
        )
        .await;
        assert_eq!(
            string(&again, "Policy"),
            expected,
            "case {case}: re-attaching the reported document answers it unchanged, got {again}"
        );
    }

    // The empty document is what "no policy" is REPORTED as, not a document a caller may
    // attach: it declares no statement, so attaching it is refused and the policy the key
    // carries is left alone.
    let held = reported(
        &attachable_documents()
            .into_iter()
            .next()
            .expect("the table is not empty")
            .1,
    );
    ok_call(
        &router,
        "PutResourcePolicy",
        &json!({"ResourceArn": arn, "Policy": held}),
    )
    .await;
    refused(
        &router,
        "the empty document",
        "PutResourcePolicy",
        &json!({"ResourceArn": arn, "Policy": NO_POLICY_DOCUMENT}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        stored_policy(&engine, "the empty document", &arn).as_deref(),
        Some(held.as_str()),
        "a refused attach leaves the policy the key had"
    );
}

/// Documents that would open the key to a principal nobody named: the rows differ in the
/// form the wildcard takes, whether it is reached through the negated principal element,
/// which statement of the document is the public one, and which condition fails to narrow
/// it to a named caller.
fn public_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "a bare wildcard principal",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Resource\":\"*\"}",
            ),
        ),
        (
            "the wildcard under a principal type",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:*\",\
                 \"Principal\":{\"AWS\":\"*\"}}",
            ),
        ),
        (
            "the wildcard among a type's identifiers",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\",\"*\"]}}",
            ),
        ),
        (
            "everyone outside a named set",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"NotPrincipal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
            ),
        ),
        (
            "a public second statement beside a narrow first one",
            document(&[
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:EncryptData\",\
                 \"Principal\":\"*\"}",
            ]),
        ),
        (
            "a condition on something that is not the caller",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Condition\":{\"StringEquals\":\
                 {\"aws:ResourceAccount\":\"000000000000\"}}}",
            ),
        ),
        (
            "a condition that says only what the caller is NOT",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Condition\":{\"StringNotEquals\":\
                 {\"aws:PrincipalAccount\":\"000000000001\"}}}",
            ),
        ),
        (
            "a caller condition accepting every value",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Condition\":{\"StringLike\":\
                 {\"aws:PrincipalArn\":\"*\"}}}",
            ),
        ),
        (
            "a condition naming an address range rather than a caller",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"*\",\"Condition\":{\"IpAddress\":\
                 {\"aws:SourceIp\":\"198.51.100.0/24\"}}}",
            ),
        ),
    ]
}

#[tokio::test]
async fn a_policy_that_would_grant_public_access_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();
    let arn = create_key(&router).await;

    // A policy the key really carries, so a refused attach is observably a no-op rather
    // than a write onto an empty slot.
    let (held_case, held) = attachable_documents()
        .into_iter()
        .next()
        .expect("the table is not empty");
    ok_call(
        &router,
        "PutResourcePolicy",
        &json!({"ResourceArn": arn, "Policy": held}),
    )
    .await;
    let held = reported(&held);

    for (case, document) in public_documents() {
        let parsed = refused(
            &router,
            case,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": document}),
            "PublicPolicyException",
        )
        .await;
        assert_eq!(
            parsed.get("Message").and_then(Value::as_str),
            Some(PUBLIC_POLICY_MESSAGE),
            "case {case}: expected the public-policy sentence, got {parsed}"
        );
        assert_eq!(
            stored_policy(&engine, case, &arn).as_deref(),
            Some(held.as_str()),
            "case {case}: a refused attach leaves the policy the key had"
        );
        let got = ok_call(
            &router,
            "GetResourcePolicy",
            &json!({"ResourceArn": arn}),
        )
        .await;
        assert_eq!(
            string(&got, "Policy"),
            held,
            "case {case}: the read still answers the policy that was attached, got {got}"
        );
    }

    // The same holds for a key no call has attached a policy to: a refused attach does not
    // write one, so the read still reports the empty document.
    let fresh = create_key(&router).await;
    for (case, document) in public_documents() {
        refused(
            &router,
            case,
            "PutResourcePolicy",
            &json!({"ResourceArn": fresh, "Policy": document}),
            "PublicPolicyException",
        )
        .await;
        assert_eq!(
            stored_policy(&engine, case, &fresh),
            None,
            "case {case}: a refused attach stores no policy"
        );
        let got = ok_call(
            &router,
            "GetResourcePolicy",
            &json!({"ResourceArn": fresh}),
        )
        .await;
        assert_eq!(
            string(&got, "Policy"),
            NO_POLICY_DOCUMENT,
            "case {case}: the key still carries no policy, got {got}"
        );
    }

    assert_eq!(
        held_case, "one named account",
        "the held document is the first row of the attachable table"
    );
}

#[tokio::test]
async fn a_text_that_is_not_a_policy_this_service_attaches_is_refused() {
    let (router, engine) = router_with_engine();
    let arn = create_key(&router).await;

    // The rows differ in the clause the document breaks: it is not a JSON object, it
    // carries a member the policy language does not define, it declares no statement or no
    // version the language defines, it names an action the language cannot read, or a
    // statement names no principal — which is the element a resource policy exists to
    // state.
    let documents: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        (
            "a JSON array rather than an object",
            "[{\"Effect\":\"Allow\"}]".to_string(),
        ),
        (
            "a member the language does not define",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statements\":[\
                 {{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":\"{}\"}}]}}",
                "arn:aws:iam::000000000000:root"
            ),
        ),
        (
            "no statement element",
            "{\"Version\":\"2012-10-17\"}".to_string(),
        ),
        (
            "an empty statement list",
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "no version string",
            "{\"Statement\":[{\"Effect\":\"Allow\",\
             \"Action\":\"payment-cryptography:DecryptData\",\
             \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}]}"
                .to_string(),
        ),
        (
            "a policy language version that does not exist",
            "{\"Version\":\"2525-01-01\",\"Statement\":[{\"Effect\":\"Allow\",\
             \"Action\":\"payment-cryptography:DecryptData\",\
             \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}]}"
                .to_string(),
        ),
        (
            "an action the language cannot read",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"decrypt some data\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
            ),
        ),
        (
            "a statement naming no principal",
            policy(
                "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Resource\":\"*\"}",
            ),
        ),
        (
            "a statement with no effect the language defines",
            policy(
                "{\"Effect\":\"Maybe\",\"Action\":\"payment-cryptography:DecryptData\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
            ),
        ),
        (
            "a statement naming no action",
            policy(
                "{\"Effect\":\"Allow\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
            ),
        ),
    ];

    for (case, document) in documents {
        let parsed = refused(
            &router,
            case,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": document}),
            "ValidationException",
        )
        .await;
        assert!(
            parsed
                .get("message")
                .or_else(|| parsed.get("Message"))
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case}: the refusal says what the document failed to be, got {parsed}"
        );
        assert_eq!(
            stored_policy(&engine, case, &arn),
            None,
            "case {case}: a refused attach stores no policy"
        );
    }
}

#[tokio::test]
async fn the_policy_operations_name_the_resource_that_was_not_found() {
    let (router, engine) = router_with_engine();
    let arn = create_key(&router).await;

    // A well-formed `KeyArn` (its pattern and 70..150 length are modelled) that names no
    // key: both operations of this slice answer the not-found class for it, naming the ARN
    // the request carried.
    let unknown = key_arn(REGION, "zzzzzzzzzzzzzzzz");
    let attachable = attachable_documents()
        .into_iter()
        .next()
        .expect("the table is not empty")
        .1;
    for (operation, body) in [
        (
            "PutResourcePolicy",
            json!({"ResourceArn": unknown, "Policy": attachable}),
        ),
        ("GetResourcePolicy", json!({"ResourceArn": unknown})),
    ] {
        let parsed = refused(
            &router,
            "an ARN naming no key",
            operation,
            &body,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            parsed.get("ResourceId").and_then(Value::as_str),
            Some(unknown.as_str()),
            "{operation}: the refusal names the resource that was not found, got {parsed}"
        );
    }

    // A key that exists and that no call has attached a policy to: the read answers the
    // empty document rather than refusing, and reading it writes nothing.
    let got = ok_call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        string(&got, "Policy"),
        NO_POLICY_DOCUMENT,
        "a key with no attached policy reports the empty document, got {got}"
    );
    assert_eq!(
        stored_policy(&engine, "a key carrying no attached policy", &arn),
        None,
        "a read attaches nothing"
    );
}
