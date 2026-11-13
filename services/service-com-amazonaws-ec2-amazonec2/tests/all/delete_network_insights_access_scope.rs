// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteNetworkInsightsAccessScope`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DeleteNetworkInsightsAccessScope`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. the delete TAKES THE SCOPE THE REQUEST NAMES OUT OF THE STORE and answers that id: the
//!    content stored beside it goes with it, every other scope of the account is left where it
//!    was, the bytes the removed records held go back to the store's budget, the client token
//!    that named the scope names it no longer, and the scope is gone for every later caller —
//!    `DescribeNetworkInsightsAccessScopes` and `GetNetworkInsightsAccessScopeContent`, which
//!    read the same store, no longer resolve its id;
//! 2. a value this service could never have ISSUED is refused BEFORE any lookup, with EC2's
//!    `InvalidParameterValue` stating that the `NetworkInsightsAccessScopeId` it was given is
//!    not a valid one, while an id it DID issue that the account holds no scope under —
//!    including the id of a scope an earlier call already deleted — is refused with this
//!    resource's own `InvalidNetworkInsightsAccessScopeId.NotFound` naming it; either way
//!    nothing is removed;
//! 3. `DryRun` asks whether the request WOULD have succeeded, so it is answered only once the
//!    scope the request names has been resolved: a dry run of a request that would have worked
//!    reports `DryRunOperation` and removes nothing, a dry run naming a scope the account does
//!    not hold is refused with THAT reason instead, and `DryRun=false` is an ordinary delete.
//!
//! Each table row is a distinct input along what the rules generalise over — which content
//! paths and tags the deleted scope carries, which of several scopes of the account is deleted
//! and in which order, and each way a value can fail to name a scope of the account — so no
//! one recorded call is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX, NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX,
    RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn reported(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One `DeleteNetworkInsightsAccessScope` call carrying `params`.
async fn delete(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DeleteNetworkInsightsAccessScope", params).await
}

/// One `DeleteNetworkInsightsAccessScope` call naming `scope_id`.
async fn delete_scope(router: &MultiServiceRouter, scope_id: &str) -> (u16, String) {
    delete(router, &[param("NetworkInsightsAccessScopeId", scope_id)]).await
}

/// One `DeleteNetworkInsightsAccessScope` call naming `scope_id` as a dry run.
async fn dry_run_delete(router: &MultiServiceRouter, scope_id: &str) -> (u16, String) {
    delete(
        router,
        &[
            param("NetworkInsightsAccessScopeId", scope_id),
            param("DryRun", "true"),
        ],
    )
    .await
}

/// The form fields of one `MatchPath`/`ExcludePath` whose source names `resource_type`, as
/// ec2Query flattens the nested request shapes.
fn path_params(list_member: &str, resource_type: &str) -> Vec<Param> {
    vec![param(
        &format!("{list_member}.1.Source.ResourceStatement.ResourceType.1"),
        resource_type,
    )]
}

/// The form fields of one tag on the scope a create mints.
fn scope_tag_params(key: &str, value: &str) -> Vec<Param> {
    vec![
        param(
            "TagSpecification.1.ResourceType",
            RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
        ),
        param("TagSpecification.1.Tag.1.Key", key),
        param("TagSpecification.1.Tag.1.Value", value),
    ]
}

/// What makes two stored scopes differ in the caller-chosen bytes they hold, which is what the
/// delete has to give back: the content paths the scope carries and the tags on it.
fn content_rows() -> Vec<(&'static str, Vec<Param>)> {
    vec![
        ("a scope with neither paths nor tags", Vec::new()),
        (
            "a scope with one match path",
            path_params("MatchPath", "AWS::EC2::VPC"),
        ),
        (
            "a scope with one exclude path and a tag",
            [
                path_params("ExcludePath", "AWS::EC2::NetworkInterface"),
                scope_tag_params("Team", "platform"),
            ]
            .concat(),
        ),
        (
            "a scope with paths in both directions",
            [
                path_params("MatchPath", "AWS::EC2::InternetGateway"),
                path_params("ExcludePath", "AWS::EC2::Subnet"),
            ]
            .concat(),
        ),
    ]
}

/// One scope of the account, as the id the create answered.
async fn create_scope(
    case: &str,
    router: &MultiServiceRouter,
    client_token: &str,
    content: &[Param],
) -> String {
    let mut params = vec![param("ClientToken", client_token)];
    params.extend_from_slice(content);
    let body = ok(case, router, "CreateNetworkInsightsAccessScope", &params).await;
    reported(case, &body, "networkInsightsAccessScopeId")
}

/// Every Network Access Scope id the store holds, in store order.
fn held_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scopes
        .keys()
        .cloned()
        .collect()
}

/// Every Network Access Scope id the store holds CONTENT for: a scope and its content enter
/// and leave the store together, so this is the second record the delete has to take out.
fn held_content_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scope_contents
        .keys()
        .cloned()
        .collect()
}

/// The caller-chosen bytes the store currently holds against its one byte budget.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The `<Message>` of a refusal, read out of the PARSED XML envelope: the body has to be
/// well-formed XML and the element's own text is what is answered, so a caller-controlled
/// value the service wrote into the markup unescaped cannot read back as the message it
/// belongs to.
fn refusal_message(case: &str, body: &str) -> String {
    document(case, body)
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Message")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"))
        .to_string()
}

/// The refusal a value this service could never have ISSUED gets from this operation.
fn unreadable_message(scope_id: &str) -> String {
    format!("NetworkInsightsAccessScopeId ({scope_id}) is not valid")
}

/// The refusal an id this service DID issue that names no scope of the account gets: the one
/// wording every read of this surface answers for an id it cannot resolve.
fn absent_message(scope_id: &str) -> String {
    format!("{scope_id} does not exist")
}

/// Each way a value can be one this service could never have ISSUED as a Network Access Scope
/// id: the all-zero placeholder a client reaches for in both published EC2 id suffix lengths,
/// a suffix of neither length, a suffix whose leading digit is not `0` (the long form carries
/// 68 bits for a 64-bit identifier, so an issued id always reads `nis-0…`), an upper-case
/// suffix, a non-hexadecimal suffix, a well-formed long-form id astronomically above
/// everything the mint has counted, the id of the ANALYSIS resource of the same surface,
/// another resource type's id, free text, and a value carrying the characters that are
/// significant in the XML envelope the refusal is written into.
fn unreadable_ids() -> Vec<(&'static str, String)> {
    let prefix = NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX;
    vec![
        ("the all-zero long form", format!("{prefix}-00000000000000000")),
        ("the all-zero short form", format!("{prefix}-00000000")),
        ("neither length", format!("{prefix}-0000000000000000")),
        (
            "a suffix outside the issued form",
            format!("{prefix}-123456789abcdef01"),
        ),
        ("an upper-case suffix", format!("{prefix}-0ABCDEF01")),
        ("a non-hex suffix", format!("{prefix}-zzzzzzzz")),
        (
            "an id the mint has not reached",
            format!("{prefix}-0123456789abcdef1"),
        ),
        (
            "the analysis resource's id",
            format!("{NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX}-0123456789abcdef1"),
        ),
        ("another resource's id", "vpc-0123456789abcdef1".to_string()),
        (
            "free text",
            "not-a-network-insights-access-scope".to_string(),
        ),
        ("XML metacharacters", "nis-<tag>&'\"end".to_string()),
    ]
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is deleted, and the id is what is reported
// ---------------------------------------------------------------------------

/// RULE 1: deleting a scope removes exactly it — and the content stored beside it — and answers
/// its id; every other scope of the account is left where it was, and the scope is gone for
/// every later reader: neither the describe narrowed to its id nor the content get resolves it
/// any more, and the unfiltered describe stops listing it. One row per content shape, crossed
/// with which of three scopes of the account is deleted.
#[tokio::test]
async fn deleting_a_scope_removes_exactly_it_and_answers_its_id() {
    for (row, content) in content_rows() {
        for position in 0..3usize {
            let case = format!("{row}, deleted at position {position} of three");
            let case = case.as_str();
            let (router, engine) = super::ec2_query::emulator();

            // Three scopes, so a scope the request did not name has to be left alone; the
            // row's own content sits on the one at `position`.
            let mut scopes = Vec::new();
            for index in 0..3usize {
                let row_content: &[Param] = if index == position { &content } else { &[] };
                scopes
                    .push(create_scope(case, &router, &format!("token-{index}"), row_content).await);
            }
            let target = scopes[position].clone();
            let by_id = indexed("NetworkInsightsAccessScopeId", &[target.as_str()]);
            let named = vec![param("NetworkInsightsAccessScopeId", &target)];

            // Both reads report the scope BEFORE the delete, so the refusals below are the
            // delete's doing rather than a read that never reported it.
            for (read, params) in [
                ("DescribeNetworkInsightsAccessScopes", by_id.clone()),
                ("GetNetworkInsightsAccessScopeContent", named.clone()),
            ] {
                let body = ok(case, &router, read, &params).await;
                assert!(
                    body.contains(&target),
                    "case {case:?}: {read} must report {target} before the delete, got {body}"
                );
            }

            let (status, body) = delete_scope(&router, &target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let answered = element(&body, "networkInsightsAccessScopeId");
            assert_eq!(
                answered.as_deref(),
                Some(target.as_str()),
                "case {case:?}: expected the response to name {target}, got {answered:?} in \
                 {body}"
            );

            // The store holds exactly the scopes the request did not name, and holds content
            // for exactly those: the scope and its content left together.
            let remaining: Vec<String> =
                scopes.iter().filter(|id| **id != target).cloned().collect();
            let held = held_ids(&engine);
            assert_eq!(
                held, remaining,
                "case {case:?}: expected the other scopes {remaining:?} to be left alone, got \
                 {held:?}"
            );
            let contents = held_content_ids(&engine);
            assert_eq!(
                contents, remaining,
                "case {case:?}: expected the content of {remaining:?} to be what is left, got \
                 {contents:?}"
            );

            // Every read of that id now refuses, with this resource's own not-found code and
            // the id the REQUEST carried.
            let not_found_message = format!("{target} does not exist");
            for (read, params) in [
                ("DescribeNetworkInsightsAccessScopes", by_id.clone()),
                ("GetNetworkInsightsAccessScopeContent", named.clone()),
            ] {
                let (status, body) = api(&router, read, &params).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 from {read} after the delete, got \
                     {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
                    "case {case:?}: expected \
                     {INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND} from {read}, got \
                     {body}"
                );
                assert_eq!(
                    error_message(&body),
                    not_found_message,
                    "case {case:?}: expected {read} to name {target}, got {body}"
                );
            }

            // The unfiltered describe reports the store: the deleted scope is gone from it and
            // the two the account still holds are both still there.
            let body = ok(case, &router, "DescribeNetworkInsightsAccessScopes", &[]).await;
            let listed = elements(&body, "networkInsightsAccessScopeId");
            assert!(
                !listed.contains(&target),
                "case {case:?}: the unfiltered describe must not list the deleted {target}, got \
                 {listed:?}"
            );
            for scope in &remaining {
                assert!(
                    listed.contains(scope),
                    "case {case:?}: the unfiltered describe must still list {scope}, got \
                     {listed:?}"
                );
            }
        }
    }
}

/// RULE 1: the bytes a scope held go back to the store's one byte budget when it is deleted,
/// so a store that holds no scope holds none of their bytes either. One row per content shape:
/// how many bytes a scope holds is what the row varies.
#[tokio::test]
async fn deleting_a_scope_gives_back_the_bytes_it_held() {
    for (case, content) in content_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let before = stored_bytes(&engine);

        let scope_id = create_scope(case, &router, "a-client-token", &content).await;
        let charged = stored_bytes(&engine);
        assert!(
            charged > before,
            "case {case:?}: creating a scope must charge the byte budget, got {charged} after \
             {before}"
        );

        let (status, body) = delete_scope(&router, &scope_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = stored_bytes(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: expected the delete to give back the {} bytes the scope held, got \
             {after} against {before}",
            charged - before
        );
    }
}

/// RULE 1: the `ClientToken` that named a deleted scope names it no longer, so a create
/// replaying that token mints a NEW scope instead of reporting one nothing holds. One row per
/// content shape.
#[tokio::test]
async fn the_client_token_of_a_deleted_scope_names_it_no_longer() {
    for (case, content) in content_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let token = "one-token-for-both-creates";

        let first = create_scope(case, &router, token, &content).await;
        // The token is idempotent while the scope is held: the replay reports that one.
        let replayed = create_scope(case, &router, token, &content).await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replay before the delete must report {first}, got {replayed}"
        );

        let (status, body) = delete_scope(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let minted = create_scope(case, &router, token, &content).await;
        assert_ne!(
            minted, first,
            "case {case:?}: a create replayed after the delete must mint a new scope, got \
             {minted}"
        );
        let held = held_ids(&engine);
        assert_eq!(
            held,
            vec![minted.clone()],
            "case {case:?}: the store must hold exactly the new scope {minted}, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a value the mint never issued, and an id it did whose scope has gone
// ---------------------------------------------------------------------------

/// RULE 2, first half: a value this service could never have ISSUED is refused before any
/// lookup — HTTP 400 `InvalidParameterValue`, stating that the `NetworkInsightsAccessScopeId`
/// it was given is not a valid one and naming the value the request sent — and nothing is
/// removed. One row per way a value can be one the mint could never have handed out.
#[tokio::test]
async fn a_scope_id_the_mint_never_issued_is_refused_as_an_invalid_value() {
    for (case, scope_id) in unreadable_ids() {
        let (router, engine) = super::ec2_query::emulator();
        let held = create_scope(case, &router, "a-held-scope", &[]).await;

        let (status, body) = delete_scope(&router, &scope_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        // Read the message out of the PARSED envelope rather than by substring: a value whose
        // metacharacters reached the markup verbatim would either stop the body from parsing
        // or cut the element's text short at that character, so this is what pins that a
        // caller-controlled id is written into the envelope escaped.
        let message = refusal_message(case, &body);
        assert_eq!(
            message,
            unreadable_message(&scope_id),
            "case {case:?}: expected this operation's one sentence, got {message:?}"
        );
        assert_eq!(
            message,
            error_message(&body),
            "case {case:?}: the parsed message and the one on the wire must agree, got \
             {message:?} and {:?}",
            error_message(&body)
        );
        let remaining = held_ids(&engine);
        assert_eq!(
            remaining,
            vec![held.clone()],
            "case {case:?}: a refused delete must remove nothing, got {remaining:?}"
        );
    }
}

/// RULE 2, second half: a delete is not idempotent — the scope is gone after the first call,
/// so a second call naming the same id names an id this service DID issue that the account
/// holds no scope under, and is answered with this resource's own not-found code naming that
/// id. One row per content shape.
#[tokio::test]
async fn deleting_the_same_scope_twice_refuses_the_second_call_as_absent() {
    for (case, content) in content_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = create_scope(case, &router, "a-token", &content).await;

        let (status, body) = delete_scope(&router, &scope_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete expected HTTP 200, got {status} with body {body}"
        );

        let (status, body) = delete_scope(&router, &scope_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the second delete expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
            "case {case:?}: the second delete expected \
             {INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            absent_message(&scope_id),
            "case {case:?}: the second delete must name {scope_id}, got {body}"
        );
        assert!(
            held_ids(&engine).is_empty(),
            "case {case:?}: the refused second call must remove nothing, got {:?}",
            held_ids(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: DryRun
// ---------------------------------------------------------------------------

/// RULE 3: `DryRun` on a request that would have worked reports the permitted outcome —
/// `DryRunOperation`, HTTP 412 — and removes nothing, so the scope is still there and still
/// deletable afterwards; `DryRun=false` is an ordinary delete. One row per content shape.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_without_deleting() {
    for (case, content) in content_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = create_scope(case, &router, "a-token", &content).await;

        let (status, body) = dry_run_delete(&router, &scope_id).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        let held = held_ids(&engine);
        assert_eq!(
            held,
            vec![scope_id.clone()],
            "case {case:?}: a dry run must delete nothing, got {held:?}"
        );

        // `DryRun=false` is no dry run at all: the delete happens.
        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsAccessScopeId", &scope_id),
                param("DryRun", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DryRun=false expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            held_ids(&engine).is_empty(),
            "case {case:?}: DryRun=false must delete the scope, got {:?}",
            held_ids(&engine)
        );
    }
}

/// RULE 3: a dry run asks whether the request WOULD have worked, so one naming a value this
/// service could never have issued is refused with THAT reason rather than reported as
/// permitted. The rows are the same values the refusal table covers.
#[tokio::test]
async fn a_dry_run_naming_no_scope_of_the_account_is_refused_with_that_reason() {
    for (case, scope_id) in unreadable_ids() {
        let (router, engine) = super::ec2_query::emulator();
        let held = create_scope(case, &router, "a-held-scope", &[]).await;

        let (status, body) = dry_run_delete(&router, &scope_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} rather than a permitted dry run, \
             got {body}"
        );
        assert_eq!(
            error_message(&body),
            unreadable_message(&scope_id),
            "case {case:?}: expected the refusal to name {scope_id}, got {body}"
        );
        let remaining = held_ids(&engine);
        assert_eq!(
            remaining,
            vec![held.clone()],
            "case {case:?}: a refused dry run must remove nothing, got {remaining:?}"
        );
    }
}

/// RULE 3: a dry run of a scope an earlier call already deleted is refused for the id it names
/// too — the dry run of a delete that cannot happen is not a permitted outcome, and the reason
/// is the absent half of RULE 2. One row per content shape.
#[tokio::test]
async fn a_dry_run_of_an_already_deleted_scope_is_refused() {
    for (case, content) in content_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let scope_id = create_scope(case, &router, "a-token", &content).await;
        let (status, body) = delete_scope(&router, &scope_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected HTTP 200, got {status} with body {body}"
        );

        let (status, body) = dry_run_delete(&router, &scope_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND}, got \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            absent_message(&scope_id),
            "case {case:?}: expected the refusal to name {scope_id}, got {body}"
        );
        assert!(
            held_ids(&engine).is_empty(),
            "case {case:?}: nothing was held, so nothing can be removed, got {:?}",
            held_ids(&engine)
        );
    }
}
