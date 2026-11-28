// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal a reused `clientToken` earns when the rest
//! of the request changed.
//!
//! The rule the rows below pin: every operation that takes a `clientToken` refuses a
//! later call that reuses a token with any other parameter changed as
//! `IdempotentParameterMismatchException` with HTTP 400, and the `message` it carries
//! is one fixed sentence — the same one for every such operation and every changed
//! parameter, interpolating nothing from the request (no token, no name, no ARN, no
//! member name).
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH tokened operation is retried — the create, the update, the two association
//!   operations, the delete and the managed-permission create, i.e. every operation in
//!   this service that remembers a `clientToken`;
//! * WHICH parameter the retry changed, and of what kind: a string, a boolean, a list's
//!   contents, a list's length, a map of tags, a nested configuration structure, an ARN
//!   naming another resource, a document-valued template.
//!
//! Each row also drives the two neighbouring paths, so the refusal is pinned to a
//! changed parameter rather than to token reuse: the first call succeeds, and a verbatim
//! repeat of it under the same token succeeds too.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The sentence AWS words this refusal with, from the recorded RAM response for a
/// client-token mismatch. It is entirely fixed text: the error's only member is
/// `message` (see `IdempotentParameterMismatchException` in this service's model), and
/// no part of it comes from the request.
const EXPECTED_MESSAGE: &str =
    "The request has the same client token as a previous request, but the requests are not the \
     same.";

/// A resource type a customer managed permission can be created for, and a template
/// for it, as `CreatePermission`'s members document them.
const PERMISSION_RESOURCE_TYPE: &str = "codebuild:Project";
const PERMISSION_TEMPLATE: &str = r#"{"Effect":"ALLOW","Action":["codebuild:ListProjects"]}"#;

/// One request: everything `call` needs to make it.
struct Call {
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

impl Call {
    fn post(uri: &'static str, body: Value) -> Self {
        Self {
            method: "POST",
            uri: uri.to_string(),
            body: Some(body),
        }
    }

    async fn send(&self, router: &MultiServiceRouter) -> (u16, Value) {
        call(router, self.method, &self.uri, self.body.as_ref()).await
    }
}

/// A row: a call that must succeed, and a call that reuses its token with one other
/// parameter changed and so must be refused.
struct Row {
    case: &'static str,
    first: Call,
    changed: Call,
}

/// The `DELETE /deleteresourceshare` call for a share, under a token.
fn delete_call(arn: &str, token: &str) -> Call {
    Call {
        method: "DELETE",
        uri: format!(
            "/deleteresourceshare?resourceShareArn={}&clientToken={}",
            query_escape(arn),
            query_escape(token)
        ),
        body: None,
    }
}

/// The rows, over the two shares the test created.
fn rows(share: &str, other_share: &str) -> Vec<Row> {
    vec![
        Row {
            case: "CreateResourceShare, another name",
            first: Call::post(
                "/createresourceshare",
                json!({ "name": "mismatch-name", "clientToken": "token-name" }),
            ),
            changed: Call::post(
                "/createresourceshare",
                json!({ "name": "mismatch-name-other", "clientToken": "token-name" }),
            ),
        },
        Row {
            case: "CreateResourceShare, another tag value",
            first: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-tags",
                    "tags": [{"key": "env", "value": "test"}],
                    "clientToken": "token-tags",
                }),
            ),
            changed: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-tags",
                    "tags": [{"key": "env", "value": "prod"}],
                    "clientToken": "token-tags",
                }),
            ),
        },
        Row {
            case: "CreateResourceShare, one more resourceArn",
            first: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-resources",
                    "resourceArns": [LOCAL_RESOURCE],
                    "clientToken": "token-resources",
                }),
            ),
            changed: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-resources",
                    "resourceArns": [LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
                    "clientToken": "token-resources",
                }),
            ),
        },
        Row {
            case: "CreateResourceShare, another allowExternalPrincipals",
            first: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-external",
                    "allowExternalPrincipals": true,
                    "clientToken": "token-external",
                }),
            ),
            changed: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-external",
                    "allowExternalPrincipals": false,
                    "clientToken": "token-external",
                }),
            ),
        },
        Row {
            case: "CreateResourceShare, another resourceShareConfiguration",
            first: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-configuration",
                    "resourceShareConfiguration": { "retainSharingOnAccountLeaveOrganization": false },
                    "clientToken": "token-configuration",
                }),
            ),
            changed: Call::post(
                "/createresourceshare",
                json!({
                    "name": "mismatch-configuration",
                    "resourceShareConfiguration": { "retainSharingOnAccountLeaveOrganization": true },
                    "clientToken": "token-configuration",
                }),
            ),
        },
        Row {
            case: "AssociateResourceShare, another principal",
            first: Call::post(
                "/associateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "principals": ["777788889999"],
                    "clientToken": "token-associate",
                }),
            ),
            changed: Call::post(
                "/associateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "principals": ["555566667777"],
                    "clientToken": "token-associate",
                }),
            ),
        },
        Row {
            case: "DisassociateResourceShare, another resourceArn",
            first: Call::post(
                "/disassociateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "resourceArns": [LOCAL_RESOURCE_TWO],
                    "clientToken": "token-disassociate",
                }),
            ),
            changed: Call::post(
                "/disassociateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "resourceArns": [LOCAL_RESOURCE],
                    "clientToken": "token-disassociate",
                }),
            ),
        },
        Row {
            case: "UpdateResourceShare, another name",
            first: Call::post(
                "/updateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "name": "mismatch-renamed",
                    "clientToken": "token-update-name",
                }),
            ),
            changed: Call::post(
                "/updateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "name": "mismatch-renamed-again",
                    "clientToken": "token-update-name",
                }),
            ),
        },
        Row {
            case: "UpdateResourceShare, allowExternalPrincipals now named too",
            first: Call::post(
                "/updateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "name": "mismatch-flagged",
                    "clientToken": "token-update-external",
                }),
            ),
            changed: Call::post(
                "/updateresourceshare",
                json!({
                    "resourceShareArn": share,
                    "name": "mismatch-flagged",
                    "allowExternalPrincipals": false,
                    "clientToken": "token-update-external",
                }),
            ),
        },
        Row {
            case: "CreatePermission, another policyTemplate",
            first: Call::post(
                "/createpermission",
                json!({
                    "name": "MismatchTemplate",
                    "resourceType": PERMISSION_RESOURCE_TYPE,
                    "policyTemplate": PERMISSION_TEMPLATE,
                    "clientToken": "token-permission-template",
                }),
            ),
            changed: Call::post(
                "/createpermission",
                json!({
                    "name": "MismatchTemplate",
                    "resourceType": PERMISSION_RESOURCE_TYPE,
                    "policyTemplate":
                        r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#,
                    "clientToken": "token-permission-template",
                }),
            ),
        },
        Row {
            case: "CreatePermission, another resourceType",
            first: Call::post(
                "/createpermission",
                json!({
                    "name": "MismatchType",
                    "resourceType": PERMISSION_RESOURCE_TYPE,
                    "policyTemplate": PERMISSION_TEMPLATE,
                    "clientToken": "token-permission-type",
                }),
            ),
            changed: Call::post(
                "/createpermission",
                json!({
                    "name": "MismatchType",
                    "resourceType": "codebuild:ReportGroup",
                    "policyTemplate": PERMISSION_TEMPLATE,
                    "clientToken": "token-permission-type",
                }),
            ),
        },
        // Last, because its successful call tombstones the share it names.
        Row {
            case: "DeleteResourceShare, another resourceShareArn",
            first: delete_call(other_share, "token-delete"),
            changed: delete_call(share, "token-delete"),
        },
    ]
}

/// Two shares to retry the share-scoped operations against: the first carrying the
/// resources and principals those rows change, the second only so a delete has another
/// ARN to name.
async fn two_shares(router: &MultiServiceRouter) -> (String, String) {
    let share = create_ok(
        router,
        "setup",
        &json!({
            "name": "mismatch-subject",
            "resourceArns": [LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
            "principals": ["444455556666"],
        }),
    )
    .await;
    let other = create_ok(router, "setup", &json!({ "name": "mismatch-other" })).await;
    (
        arn_of("setup", &share),
        arn_of("setup", &other),
    )
}

#[tokio::test]
async fn a_reused_client_token_with_changed_parameters_is_refused_as_ram_words_it() {
    let router = emulator();
    let (share, other_share) = two_shares(&router).await;

    let mut reported: Vec<(&str, String)> = Vec::new();
    for row in rows(&share, &other_share) {
        let case = row.case;

        let (status, first) = row.first.send(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first call expected 200, got {status} with {first:?}"
        );

        // A verbatim repeat under the same token is the idempotent path, not a
        // mismatch: the refusal below must come from the changed parameter.
        let (status, repeated) = row.first.send(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a verbatim repeat expected 200, got {status} with {repeated:?}"
        );

        let (status, refused) = row.changed.send(&router).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "IdempotentParameterMismatchException".to_string()),
            "case {case:?}: expected 400 IdempotentParameterMismatchException, got {status} with \
             {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, EXPECTED_MESSAGE,
            "case {case:?}: expected the message {EXPECTED_MESSAGE:?}, got {message:?}"
        );
        reported.push((case, message));
    }

    // The sentence carries nothing request-specific, so every row reported the very
    // same bytes — including rows of different operations.
    let distinct: std::collections::BTreeSet<&str> = reported
        .iter()
        .map(|(_, message)| message.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "case \"every tokened operation\": expected one message across all rows, got {reported:?}"
    );
    assert!(
        reported.len() >= 12,
        "case \"every tokened operation\": expected every row to have been exercised, got \
         {reported:?}"
    );
}

/// The refusal must not leak the changed parameter, the token or the resource: a fixed
/// sentence names none of them.
#[tokio::test]
async fn the_refusal_names_nothing_from_the_request() {
    let router = emulator();
    let token = "a-token-nobody-should-see";
    let first = json!({
        "name": "a-share-nobody-should-see",
        "resourceArns": [LOCAL_RESOURCE],
        "clientToken": token,
    });
    let created = create_ok(&router, "no leaked values", &first).await;
    let arn = arn_of("no leaked values", &created);

    let (status, refused) = create(
        &router,
        &json!({
            "name": "another-share-nobody-should-see",
            "resourceArns": [FOREIGN_RESOURCE],
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "IdempotentParameterMismatchException".to_string()),
        "case \"no leaked values\": expected 400 IdempotentParameterMismatchException, got \
         {status} with {refused:?}"
    );
    let message = error_message(&refused);
    assert_eq!(
        message, EXPECTED_MESSAGE,
        "case \"no leaked values\": expected the message {EXPECTED_MESSAGE:?}, got {message:?}"
    );
    for leaked in [
        token,
        "a-share-nobody-should-see",
        "another-share-nobody-should-see",
        arn.as_str(),
        LOCAL_RESOURCE,
        FOREIGN_RESOURCE,
        "clientToken",
    ] {
        assert!(
            !message.contains(leaked),
            "case \"no leaked values\": the message must not carry {leaked:?}, got {message:?}"
        );
    }
}
