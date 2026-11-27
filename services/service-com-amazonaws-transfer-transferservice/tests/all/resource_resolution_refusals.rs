// SPDX-License-Identifier: CC0-1.0

//! The four operations that resolve a Transfer resource before they act on it:
//! `TestIdentityProvider`, `DeleteAccess`, `SendWorkflowStepState` and
//! `StartFileTransfer`. (`DescribeWebApp`, the fifth operation of the issue this suite
//! was written for, is covered by the web-app lifecycle suite that landed with
//! `CreateWebApp`.)
//!
//! RULES under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. An operation whose answer depends on a server's `IdentityProviderType` decides on
//!    that type alone: `TestIdentityProvider` is refused 400 `InvalidRequestException`
//!    on every server that delegates authentication to nothing (`SERVICE_MANAGED`), and
//!    gets past that gate on every server that delegates it to a customer-run provider
//!    (`API_GATEWAY`, `AWS_LAMBDA`, `AWS_DIRECTORY_SERVICE`); `DeleteAccess` is refused
//!    400 `InvalidRequestException` on every server whose type is not
//!    `AWS_DIRECTORY_SERVICE`, before the `ExternalId` is looked up.
//! 2. An operation that models `AccessDeniedException` answers 403
//!    `AccessDeniedException` for an identifier naming no resource of this account
//!    (`SendWorkflowStepState`'s `WorkflowId` — the resource the action's permission
//!    names) — a refusal that is identical for every such request, so it discloses
//!    nothing about whether the resource exists.
//! 3. An operation that models no `AccessDeniedException`, and an operation past its
//!    authorization decision, answers `ResourceNotFoundException` for an identifier
//!    naming no resource (`TestIdentityProvider` on an unminted server, `DeleteAccess`
//!    for an absent grant, `StartFileTransfer` on an absent connector,
//!    `SendWorkflowStepState` for an execution a resolved workflow is not running),
//!    with `ResourceType` the resource name capitalized and `Resource` the identifier
//!    the request named — at this service's recorded status for an unresolvable
//!    reference (see `state::RESOURCE_NOT_FOUND_HTTP_STATUS`).
//! 4. A resource that IS stored is acted on: `DeleteAccess` removes the grant (and a
//!    second delete then cannot find it), and `SendWorkflowStepState` records the
//!    reported `Status` on the custom step its `Token` addresses, ONCE — a replay of a
//!    token whose step has reported addresses no running execution any more, so it is
//!    refused as not found and changes nothing.
//!
//! The rows differ along what each rule generalises over — the identity-provider type,
//! the id's value, which optional members the request carries, the reported status, the
//! number of pending steps — never along a renamed probe.
//!
//! Resources whose own create operation is still a scaffolded stub (`CreateAccess`)
//! and the workflow executions no emulated data plane starts are SEEDED into the store
//! the handlers read ([`crate::harness::Emulator::store`]), so rules 1 and 4 are
//! asserted on both answers; everything a create DOES serve (servers, connectors,
//! workflows) is created over the wire.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state as st;

use crate::harness::{member_of, Answer, Emulator, ACCOUNT, REGION, ROLE};

/// Syntactically valid identifiers that nothing ever minted, at the low end, a mid
/// value and the high end of each id space. The patterns are the modelled ones:
/// `ServerId` `^s-([0-9a-f]{17})$`, `ConnectorId` `^c-([0-9a-f]{17})$`.
const UNMINTED_SERVERS: [&str; 3] = [
    "s-00000000000000000",
    "s-0000000000000dead",
    "s-fffffffffffffffff",
];
const UNMINTED_CONNECTORS: [&str; 3] = [
    "c-00000000000000000",
    "c-0000000000000f00d",
    "c-fffffffffffffffff",
];
/// `WorkflowId`s satisfying the modelled `len 19..19` pattern `^w-([a-z0-9]{17})$`:
/// the low end of the id space, one of letters, and the high end. No `CreateWorkflow`
/// ever minted them, so they name no workflow of this account — the workflows these
/// suites DO resolve are created over the wire by [`create_custom_step_workflow`].
const WORKFLOW_IDS: [&str; 3] = [
    "w-00000000000000000",
    "w-abcdefghijklmnopq",
    "w-zzzzzzzzzzzzzzzzz",
];

/// `ExecutionId`s satisfying the modelled `len 36..36` UUID pattern.
const EXECUTION_IDS: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2a3b4c5d",
    "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
];

/// `ExternalId`s satisfying the modelled `len 1..256` pattern `^S-1-[\d-]+$`: the
/// shortest legal SID, a domain group SID, and a long one.
const EXTERNAL_IDS: [&str; 3] = [
    "S-1-1",
    "S-1-5-21-3623811015-3361044348-30300820-1013",
    "S-1-5-21-1111111111-2222222222-3333333333-4444444444-5555555555-666666",
];

/// `CallbackToken`s satisfying the modelled `len 1..64` pattern `^\w+$`.
const TOKENS: [&str; 3] = [
    "t",
    "callback_token_1",
    "AAAAAAAAAABBBBBBBBBBCCCCCCCCCCDDDDDDDDDDEEEEEEEEEEFFFFFFFFFFGGGG",
];

/// The `CustomStepStatus` enum, in full (`smithy_query(query="shape",
/// shape="CustomStepStatus")`: `[SUCCESS, FAILURE]`).
const CUSTOM_STEP_STATUSES: [&str; 2] = ["SUCCESS", "FAILURE"];

/// A policy allowing an API Gateway invoke on every resource.
///
/// The `API_GATEWAY` row of [`external_provider_servers`] names an `InvocationRole`,
/// and `CreateServer` VERIFIES that the role can invoke the API before it accepts the
/// server (the rule and its whole table live in `identity_and_authorization.rs`), so
/// a suite that creates such a server seeds a role that may.
const ALLOW_INVOKE_ANY_API: &str = r#"{"Version":"2012-10-17","Statement":[
    {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"}]}"#;

/// A `CreateServer` request for each `IdentityProviderType` that delegates
/// authentication to a customer-run provider, with the `IdentityProviderDetails` the
/// model documents as required for it. The values satisfy the modelled patterns:
/// `DirectoryId` `^d-[0-9a-f]{10}$`, `Function` `^arn:[a-z-]+:lambda:.*$`,
/// `InvocationRole` `^arn:.*role/\S+$`.
fn external_provider_servers() -> Vec<(&'static str, Value)> {
    vec![
        (
            "API_GATEWAY",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {
                    "Url": "https://api.example.com/transfer",
                    "InvocationRole": ROLE,
                },
            }),
        ),
        (
            "AWS_LAMBDA",
            json!({
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {
                    "Function": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:transfer-idp"),
                },
            }),
        ),
        (
            "AWS_DIRECTORY_SERVICE",
            json!({
                "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                "IdentityProviderDetails": {"DirectoryId": "d-0123456789"},
            }),
        ),
    ]
}

/// `CreateServer` requests that all resolve to the `SERVICE_MANAGED` user store: by
/// omitting `IdentityProviderType` (its documented default), by naming it, and by
/// naming it alongside other configuration.
fn service_managed_servers() -> Vec<(&'static str, Value)> {
    vec![
        ("the documented default", json!({})),
        (
            "SERVICE_MANAGED named",
            json!({"IdentityProviderType": "SERVICE_MANAGED"}),
        ),
        (
            "SERVICE_MANAGED with other configuration",
            json!({
                "IdentityProviderType": "SERVICE_MANAGED",
                "Protocols": ["SFTP"],
                "Domain": "EFS",
                "Tags": [{"Key": "team", "Value": "platform"}],
            }),
        ),
    ]
}

/// `TestIdentityProvider` request shapes, over the optional members the operation
/// documents as "all optional" (`ServerProtocol`, `SourceIp`, `UserPassword`) and over
/// `UserName`'s modelled `len 3..100` pattern `^[\w][\w@.-]{2,99}$`.
fn test_identity_provider_requests(server_id: &str) -> Vec<(String, Value)> {
    vec![
        (
            "only the required members".to_string(),
            json!({"ServerId": server_id, "UserName": "bob"}),
        ),
        (
            "with ServerProtocol".to_string(),
            json!({"ServerId": server_id, "UserName": "user@example", "ServerProtocol": "SFTP"}),
        ),
        (
            "with every optional member".to_string(),
            json!({
                "ServerId": server_id,
                "UserName": "a.very-long_user@example",
                "ServerProtocol": "FTPS",
                "SourceIp": "192.0.2.10",
                "UserPassword": "not-a-real-password",
            }),
        ),
    ]
}

/// Assert one answer is the modelled not-found for `resource_type`/`resource` at
/// `status`: this service answers `ResourceNotFoundException` with the status its
/// recorded wire did (`state::RESOURCE_NOT_FOUND_HTTP_STATUS`), not the shape's
/// `@httpError`.
fn assert_not_found_at(
    case: &str,
    answer: &Answer,
    status: u16,
    resource_type: &str,
    resource: &str,
) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "ResourceType").as_str(),
        Some(resource_type),
        "case {case:?}: expected ResourceType {resource_type:?}, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Resource").as_str(),
        Some(resource),
        "case {case:?}: expected Resource {resource:?}, got {}",
        answer.body
    );
}

/// Assert one answer is the modelled not-found for `resource_type`/`resource` at this
/// service's recorded status for an unresolvable reference.
fn assert_not_found(case: &str, answer: &Answer, resource_type: &str, resource: &str) {
    assert_not_found_at(
        case,
        answer,
        st::RESOURCE_NOT_FOUND_HTTP_STATUS,
        resource_type,
        resource,
    );
}

/// Assert one answer is `SendWorkflowStepState`'s refusal of a callback that resolves
/// to no running execution: this service's not-found status, the fixed `Message` it
/// answers for an execution reference it cannot resolve, and the `ExecutionId` in
/// `Resource`.
fn assert_callback_not_found(case: &str, answer: &Answer, execution_id: &str) {
    assert_not_found_at(
        case,
        answer,
        st::RESOURCE_NOT_FOUND_HTTP_STATUS,
        "Execution",
        execution_id,
    );
    assert_eq!(
        member_of(&answer.body, "Message").as_str(),
        Some(st::NOT_FOUND_EXECUTION.message),
        "case {case:?}: expected the fixed execution not-found message, got {}",
        answer.body
    );
}

/// Assert one answer is the modelled authorization refusal: HTTP 403,
/// `AccessDeniedException`, and the one fixed message.
fn assert_access_denied(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Message").as_str(),
        Some(st::INSUFFICIENT_ACCESS_MESSAGE),
        "case {case:?}: expected the modelled insufficient-access message, got {}",
        answer.body
    );
}

/// Assert one answer is the modelled `InvalidRequestException` (HTTP 400), and hand
/// back its `Message` so a caller can assert what it names.
fn assert_invalid_request(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty(),
        "case {case:?}: InvalidRequestException models Message as required, got {}",
        answer.body
    );
    message
}

/// RULE 1 (`TestIdentityProvider`, refused): a server whose `IdentityProviderType`
/// delegates authentication to nothing has no identity provider to test, so the call
/// is refused 400 `InvalidRequestException` naming that server — whatever else the
/// request carries.
#[tokio::test]
async fn test_identity_provider_refuses_every_service_managed_server() {
    let emulator = Emulator::new();

    for (configuration, request) in service_managed_servers() {
        let server_id = emulator.create_server(request).await;
        for (shape, probe) in test_identity_provider_requests(&server_id) {
            let case = format!("{configuration}, {shape}");
            let answer = emulator.call("TestIdentityProvider", probe).await;
            let message = assert_invalid_request(&case, &answer);
            assert!(
                message.contains(&server_id) && message.contains("not configured for external auth"),
                "case {case:?}: expected the documented refusal naming {server_id}, got {message:?}"
            );
        }
    }
}

/// RULE 1 (`TestIdentityProvider`, past the gate): a server that DOES delegate
/// authentication to a customer-run provider is not refused as misconfigured — it is
/// answered with the outcome of an authentication request against that provider. What
/// that answer REPORTS is [`crate::identity_provider_tests`]' subject; the gate is what
/// this asserts: the answer is not the `InvalidRequestException` of the rule above, and
/// it carries no status for a response nothing received.
#[tokio::test]
async fn test_identity_provider_passes_the_gate_for_an_external_identity_provider() {
    let emulator = Emulator::new();
    emulator.seed_role(ROLE, ALLOW_INVOKE_ANY_API);

    for (provider_type, request) in external_provider_servers() {
        let server_id = emulator.create_server(request).await;
        for (shape, probe) in test_identity_provider_requests(&server_id) {
            let case = format!("{provider_type}, {shape}");
            let answer = emulator.call("TestIdentityProvider", probe).await;
            assert_ne!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: a server with an external identity provider must pass the \
                 SERVICE_MANAGED gate, got {}",
                answer.body
            );
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the test of an external identity provider to be \
                 answered, got {} {}",
                answer.status, answer.body
            );
            assert_eq!(
                member_of(&answer.body, "StatusCode").as_i64(),
                Some(0),
                "case {case:?}: no provider answered, so the modelled default status is the \
                 one that may be reported; got {}",
                answer.body
            );
            assert!(
                member_of(&answer.body, "Response").is_null(),
                "case {case:?}: no provider answered, so no Response may be reported; got {}",
                answer.body
            );
        }
    }
}

/// RULE 3 (`TestIdentityProvider`): a `ServerId` nothing minted is not found, so the
/// type gate is never reached.
#[tokio::test]
async fn test_identity_provider_on_an_unminted_server_is_not_found() {
    let emulator = Emulator::new();
    // A server that DOES exist, so the store is not simply empty.
    emulator.create_server(json!({})).await;

    for absent in UNMINTED_SERVERS {
        for (shape, probe) in test_identity_provider_requests(absent) {
            let case = format!("TestIdentityProvider on the unminted {absent}, {shape}");
            let answer = emulator.call("TestIdentityProvider", probe).await;
            assert_not_found(&case, &answer, "Server", absent);
        }
    }
}

/// RULE 1 (`DeleteAccess`): an access grant is a group in the server's directory, so
/// on every server whose `IdentityProviderType` is not `AWS_DIRECTORY_SERVICE` the
/// request is refused 400 `InvalidRequestException` — before the `ExternalId` is
/// looked up, so the refusal does not depend on whether a grant exists.
#[tokio::test]
async fn delete_access_refuses_every_server_without_a_directory() {
    let emulator = Emulator::new();
    emulator.seed_role(ROLE, ALLOW_INVOKE_ANY_API);

    let mut non_directory: Vec<(&'static str, Value)> = service_managed_servers();
    non_directory.extend(
        external_provider_servers()
            .into_iter()
            .filter(|(provider_type, _)| *provider_type != "AWS_DIRECTORY_SERVICE"),
    );

    for (configuration, request) in non_directory {
        let server_id = emulator.create_server(request).await;
        for external_id in EXTERNAL_IDS {
            let case = format!("DeleteAccess on {configuration} for {external_id}");
            let answer = emulator
                .call(
                    "DeleteAccess",
                    json!({"ServerId": &server_id, "ExternalId": external_id}),
                )
                .await;
            let message = assert_invalid_request(&case, &answer);
            assert_eq!(
                message,
                format!(
                    "Cannot delete access on server with IdP type: {}",
                    idp_type_of(&emulator, &server_id)
                ),
                "case {case:?}: expected the fixed directory-provider refusal for this \
                 operation, got {message:?}"
            );
            assert!(
                !message.contains(external_id) && !message.contains(server_id.as_str()),
                "case {case:?}: the refusal carries nothing the request sent; got {message:?}"
            );
        }
    }
}

/// RULE 3 and RULE 4 (`DeleteAccess`): on a directory-backed server the grant itself
/// is resolved — an `ExternalId` the server holds no grant for is not found, a stored
/// grant is removed, and a second delete of the removed grant is then not found.
#[tokio::test]
async fn delete_access_removes_a_stored_grant_and_cannot_find_an_absent_one() {
    let emulator = Emulator::new();

    let server_id = emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": "d-0123456789"},
        }))
        .await;

    // Nothing granted yet: every `ExternalId` is absent.
    for external_id in EXTERNAL_IDS {
        let answer = emulator
            .call(
                "DeleteAccess",
                json!({"ServerId": &server_id, "ExternalId": external_id}),
            )
            .await;
        assert_not_found(
            &format!("DeleteAccess for the never-granted {external_id}"),
            &answer,
            "Access",
            external_id,
        );
    }

    // Seed one grant per id, then delete them one at a time: each delete must remove
    // exactly the grant it named and leave the others.
    for external_id in EXTERNAL_IDS {
        seed_access(&emulator, &server_id, external_id);
    }

    let mut remaining: Vec<&str> = EXTERNAL_IDS.to_vec();
    for external_id in EXTERNAL_IDS {
        let case = format!("DeleteAccess for the stored grant {external_id}");
        let answer = emulator
            .call(
                "DeleteAccess",
                json!({"ServerId": &server_id, "ExternalId": external_id}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.body.clone()),
            (200, json!({})),
            "case {case:?}: DeleteAccess answers an empty 200, got {} {}",
            answer.status,
            answer.body
        );

        remaining.retain(|id| *id != external_id);
        let stored = stored_access_ids(&emulator, &server_id);
        assert_eq!(
            stored, remaining,
            "case {case:?}: expected the remaining grants {remaining:?}, got {stored:?}"
        );

        // The teardown path a client retries: the grant is gone the same way one that
        // never existed is.
        let again = emulator
            .call(
                "DeleteAccess",
                json!({"ServerId": &server_id, "ExternalId": external_id}),
            )
            .await;
        assert_not_found(
            &format!("second DeleteAccess for {external_id}"),
            &again,
            "Access",
            external_id,
        );
    }
}

/// RULE 3 (`DeleteAccess`): an unminted `ServerId` is not found, so the type gate is
/// never reached.
#[tokio::test]
async fn delete_access_on_an_unminted_server_is_not_found() {
    let emulator = Emulator::new();

    for absent in UNMINTED_SERVERS {
        for external_id in EXTERNAL_IDS {
            let answer = emulator
                .call(
                    "DeleteAccess",
                    json!({"ServerId": absent, "ExternalId": external_id}),
                )
                .await;
            assert_not_found(
                &format!("DeleteAccess on the unminted {absent} for {external_id}"),
                &answer,
                "Server",
                absent,
            );
        }
    }
}

/// RULE 2 (`SendWorkflowStepState`): a `WorkflowId` naming no workflow of this account
/// is refused by authorization, whatever `ExecutionId`, `Token` and `Status` the
/// callback carries — the workflow is the resource the action's permission names.
#[tokio::test]
async fn send_workflow_step_state_refuses_a_callback_for_an_unknown_workflow() {
    let emulator = Emulator::new();

    // A real workflow with a running execution, so the refusals below are not "nothing
    // is stored".
    let live_workflow = create_custom_step_workflow(&emulator).await;
    seed_execution(&emulator, &live_workflow, EXECUTION_IDS[0], &["live"]);

    for (index, workflow_id) in WORKFLOW_IDS.iter().enumerate() {
        for execution_id in EXECUTION_IDS {
            let token = TOKENS[index % TOKENS.len()];
            let status = CUSTOM_STEP_STATUSES[index % CUSTOM_STEP_STATUSES.len()];
            let case =
                format!("SendWorkflowStepState for {workflow_id}/{execution_id} ({status})");
            let answer =
                callback(&emulator, workflow_id, execution_id, token, status).await;
            assert_access_denied(&case, &answer);
        }
    }
}

/// RULE 3 (`SendWorkflowStepState`): once the workflow resolves, an `ExecutionId` it is
/// not running is reported as not found — so a caller can tell an unresolvable workflow
/// from an execution that is not running.
#[tokio::test]
async fn send_workflow_step_state_cannot_find_an_execution_the_workflow_is_not_running() {
    let emulator = Emulator::new();

    let workflow_id = create_custom_step_workflow(&emulator).await;
    // One execution IS running, so the rows below are about the execution named, not
    // about the workflow having no executions at all.
    seed_execution(&emulator, &workflow_id, EXECUTION_IDS[0], &["live"]);

    for execution_id in EXECUTION_IDS.into_iter().skip(1) {
        for token in TOKENS {
            for status in CUSTOM_STEP_STATUSES {
                let case = format!(
                    "SendWorkflowStepState for the execution {execution_id} ({token}, {status})"
                );
                let answer =
                    callback(&emulator, &workflow_id, execution_id, token, status).await;
                assert_callback_not_found(&case, &answer, execution_id);
            }
        }
    }
}

/// RULE 4 (`SendWorkflowStepState`): the callback records the reported `Status` on the
/// custom step its `Token` addresses, exactly once — a token that addresses no pending
/// step of the execution is refused 400 `InvalidRequestException`, a REPLAY of a token
/// whose step has reported is refused as a callback that resolves to no running
/// execution, and no other step is touched.
#[tokio::test]
async fn send_workflow_step_state_records_each_pending_step_once() {
    let emulator = Emulator::new();

    let workflow_id = create_custom_step_workflow(&emulator).await;
    let workflow_id = workflow_id.as_str();
    let execution_id = EXECUTION_IDS[1];
    seed_execution(&emulator, workflow_id, execution_id, &TOKENS);

    // A token that addresses no pending step of this execution: the execution resolved,
    // so this is a malformed callback rather than a missing resource.
    let case = "a token addressing no pending step";
    let unknown_token = callback(&emulator, workflow_id, execution_id, "no_such_token", "SUCCESS")
        .await;
    let message = assert_invalid_request(case, &unknown_token);
    assert!(
        message.contains("Token"),
        "case {case:?}: expected the refusal to name the member, got {message:?}"
    );

    // Each pending step reports once, and the rows differ in the status reported.
    let mut expected: BTreeMap<String, Option<String>> =
        TOKENS.iter().map(|t| ((*t).to_string(), None)).collect();
    for (index, token) in TOKENS.iter().enumerate() {
        let status = CUSTOM_STEP_STATUSES[index % CUSTOM_STEP_STATUSES.len()];
        let case = format!("{token} reports {status}");
        let answer = callback(&emulator, workflow_id, execution_id, token, status).await;
        assert_eq!(
            (answer.status, answer.body.clone()),
            (200, json!({})),
            "case {case:?}: expected an empty 200, got {} {}",
            answer.status,
            answer.body
        );

        expected.insert((*token).to_string(), Some(status.to_string()));
        let recorded = reported_step_states(&emulator, workflow_id, execution_id);
        assert_eq!(
            recorded, expected,
            "case {case:?}: expected only the steps reported so far to carry an outcome, \
             got {recorded:?}"
        );

        // The callback is consumed ONCE: a replay of the same token addresses no
        // pending step of a running execution any more, so it is refused as not found,
        // and the recorded outcome stays the first one.
        let repeat = callback(
            &emulator,
            workflow_id,
            execution_id,
            token,
            CUSTOM_STEP_STATUSES[1],
        )
        .await;
        assert_callback_not_found(&format!("{case}, replayed"), &repeat, execution_id);
        let recorded = reported_step_states(&emulator, workflow_id, execution_id);
        assert_eq!(
            recorded, expected,
            "case {case:?}: a refused replay must change nothing, got {recorded:?}"
        );
    }
}

/// RULE 3 (`StartFileTransfer`): a `ConnectorId` the store does not hold is not found —
/// never a service fault — whatever paths the transfer names.
#[tokio::test]
async fn start_file_transfer_on_an_absent_connector_is_not_found() {
    let emulator = Emulator::new();

    // Rows differ along the direction of the transfer and the number of paths.
    let transfers: Vec<(&str, Value)> = vec![
        ("one outbound file", json!({"SendFilePaths": ["bucket/one.txt"]})),
        (
            "several outbound files with a remote directory",
            json!({
                "SendFilePaths": ["bucket/one.txt", "bucket/two.txt", "bucket/three.txt"],
                "RemoteDirectoryPath": "/inbox",
            }),
        ),
        (
            "an inbound retrieve",
            json!({
                "RetrieveFilePaths": ["/outbox/one.txt"],
                "LocalDirectoryPath": "/bucket/landing",
            }),
        ),
        ("no paths at all", json!({})),
    ];

    for absent in UNMINTED_CONNECTORS {
        for (shape, extra) in &transfers {
            let mut request = json!({"ConnectorId": absent});
            let object = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the row is an object") {
                object.insert(member.clone(), value.clone());
            }
            let case = format!("StartFileTransfer on the absent {absent} with {shape}");
            let answer = emulator.call("StartFileTransfer", request).await;
            assert_not_found(&case, &answer, "Connector", absent);
        }
    }
}

/// RULE 3 (`StartFileTransfer`): once the connector resolves, a request naming no file is
/// refused 400 `InvalidRequestException`; a request that does name files is ACCEPTED, and
/// the identifier it answers with addresses the record whose per-file results
/// `ListFileTransferResults` reports (`crate::connector_transfers` is that rule's suite).
#[tokio::test]
async fn start_file_transfer_on_a_created_connector_refuses_a_request_naming_no_file() {
    let emulator = Emulator::new();
    let connector_id = create_connector(&emulator).await;

    let message = assert_invalid_request(
        "a created connector with neither SendFilePaths nor RetrieveFilePaths",
        &emulator
            .call("StartFileTransfer", json!({"ConnectorId": &connector_id}))
            .await,
    );
    // The service's fixed text for that condition: it reports that the list of files to
    // transfer is empty and names no member at all.
    assert_eq!(
        message, "Empty send file list",
        "expected the service's fixed text for a transfer naming no file, got {message:?}"
    );

    for (case, request) in [
        (
            "outbound",
            json!({"ConnectorId": &connector_id, "SendFilePaths": ["bucket/one.txt"]}),
        ),
        (
            "outbound, several files with a remote directory",
            json!({
                "ConnectorId": &connector_id,
                "SendFilePaths": ["bucket/one.txt", "bucket/two.txt"],
                "RemoteDirectoryPath": "/inbox",
            }),
        ),
    ] {
        let answer = emulator.call("StartFileTransfer", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the resolved transfer to be accepted, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .get("TransferId")
                .as_str()
                .is_some_and(|transfer_id| !transfer_id.is_empty()),
            "case {case:?}: expected the accepted transfer to answer a TransferId, got {}",
            answer.body
        );
    }

    // This connector is an AS2 one, and "for an AS2 connector, you specify the
    // ConnectorId and one or more SendFilePaths" — so a RETRIEVE is a direction it
    // cannot be asked for, which is a malformed request rather than a transfer.
    let message = assert_invalid_request(
        "an inbound transfer asked of an AS2 connector",
        &emulator
            .call(
                "StartFileTransfer",
                json!({
                    "ConnectorId": &connector_id,
                    "RetrieveFilePaths": ["/outbox/one.txt"],
                    "LocalDirectoryPath": "/bucket/landing",
                }),
            )
            .await,
    );
    assert!(
        message.contains("RetrieveFilePaths"),
        "expected the refusal to name RetrieveFilePaths, got {message:?}"
    );
}

/// The `IdentityProviderType` the STORED server resolved to — what the
/// directory-provider refusal names, read off the record rather than restated by the
/// test, so a row that sends no `IdentityProviderType` is checked against the type its
/// server actually has.
fn idp_type_of(emulator: &Emulator, server_id: &str) -> String {
    let state = emulator.store();
    let guard = state.lock();
    guard
        .servers
        .get(server_id)
        .map(|server| server.identity_provider_type.clone())
        .unwrap_or_else(|| panic!("the test created server {server_id}"))
}

/// Seed one access grant on `server_id` — `CreateAccess` is still a scaffolded stub, so
/// the grant is written straight into the store the handler reads.
fn seed_access(emulator: &Emulator, server_id: &str, external_id: &str) {
    let state = emulator.store();
    let mut guard = state.lock();
    guard.accesses.insert(
        (server_id.to_string(), external_id.to_string()),
        st::Access {
            server_id: server_id.to_string(),
            external_id: external_id.to_string(),
            role: Some(ROLE.to_string()),
            ..st::Access::default()
        },
    );
}

/// The `ExternalId`s `server_id` currently holds a grant for, in stored order.
fn stored_access_ids(emulator: &Emulator, server_id: &str) -> Vec<&'static str> {
    let state = emulator.store();
    let guard = state.lock();
    EXTERNAL_IDS
        .into_iter()
        .filter(|external_id| {
            guard
                .accesses
                .contains_key(&(server_id.to_string(), (*external_id).to_string()))
        })
        .collect()
}

/// Create one AS2 connector over the wire and return its `ConnectorId`.
///
/// `As2Config`'s profile ids satisfy `ProfileId`'s modelled `len 19..19`, pattern
/// `^p-([0-9a-f]{17})$`; `AccessRole` is the harness's modelled role ARN.
async fn create_connector(emulator: &Emulator) -> String {
    let answer = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": "p-0123456789abcdef0",
                    "PartnerProfileId": "p-00000000000000001",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                },
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateConnector: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnector returned no ConnectorId: {}", answer.body))
        .to_string()
}

/// Create the workflow whose custom step `SendWorkflowStepState` is the callback of,
/// over the wire, and return its `WorkflowId`.
///
/// `CustomStepDetails.Target` satisfies `CustomStepTarget`'s modelled `len 0..170`,
/// pattern `^arn:[a-z-]+:lambda:.*$`.
async fn create_custom_step_workflow(emulator: &Emulator) -> String {
    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({
                "Steps": [{
                    "Type": "CUSTOM",
                    "CustomStepDetails": {
                        "Name": "callback-step",
                        "Target": format!(
                            "arn:aws:lambda:{REGION}:{ACCOUNT}:function:workflow-callback"
                        ),
                        "TimeoutSeconds": 120,
                    },
                }],
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateWorkflow: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkflow returned no WorkflowId: {}", answer.body))
        .to_string()
}

/// Seed one workflow execution with a pending custom step per token — no emulated data
/// plane uploads a file, so nothing else starts an execution.
fn seed_execution(emulator: &Emulator, workflow_id: &str, execution_id: &str, tokens: &[&str]) {
    let state = emulator.store();
    let mut guard = state.lock();
    guard.workflow_executions.insert(
        (workflow_id.to_string(), execution_id.to_string()),
        st::WorkflowExecution {
            workflow_id: workflow_id.to_string(),
            execution_id: execution_id.to_string(),
            step_states: tokens
                .iter()
                .map(|token| ((*token).to_string(), None))
                .collect(),
            // The rest of the record (the run's status, the file it started on, the
            // session it arrived on) is what a reader reports, and this suite is about
            // the callback, so it takes the defaults.
            ..st::WorkflowExecution::default()
        },
    );
}

/// The outcome recorded for each of an execution's custom steps, by callback token.
fn reported_step_states(
    emulator: &Emulator,
    workflow_id: &str,
    execution_id: &str,
) -> BTreeMap<String, Option<String>> {
    let state = emulator.store();
    let guard = state.lock();
    guard
        .workflow_executions
        .get(&(workflow_id.to_string(), execution_id.to_string()))
        .map(|execution| execution.step_states.clone())
        .unwrap_or_default()
}

/// One `SendWorkflowStepState` callback, over the wire.
async fn callback(
    emulator: &Emulator,
    workflow_id: &str,
    execution_id: &str,
    token: &str,
    status: &str,
) -> Answer {
    emulator
        .call(
            "SendWorkflowStepState",
            json!({
                "WorkflowId": workflow_id,
                "ExecutionId": execution_id,
                "Token": token,
                "Status": status,
            }),
        )
        .await
}
