// SPDX-License-Identifier: CC0-1.0

//! What a preview page token binds itself to: the SELECTION the walk is
//! performed over, not the request's spelling of it.
//!
//! The rule under test: a paginated preview's token carries the selection the
//! handler derives from the request — the accounts the rule type and
//! `organizationScope` cover, the requested action types, and the criteria after
//! defaulting, with a criterion that holds no condition read as the absent one
//! it is equivalent to — so any two requests that derive the SAME selection
//! continue one walk, while a request deriving a different selection (or the
//! sibling preview operation) is refused the token.
//!
//! Both preview operations are driven for every row, over the wire (an
//! awsJson1_0 `POST /` through the same router the emulator binary mounts), so
//! the two cannot drift apart over which requests name one selection: the
//! unsummarised one through a page walk, the summarising one through the single
//! aggregate row it reports per selection (one row, so it has no walk to
//! continue). The recorded case the issue reports is one row of the equivalence
//! table; the rows differ along what the rule generalises over: which optional
//! member is spelled two ways (`criteria`, a criterion inside it,
//! `organizationScope`), and whether the spelling difference is an absent
//! member, an empty object or a rule type that derives the same account scope.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    Criteria, RecommendedAction,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    caller_account_id, encode_page_token, preview_account_scope, preview_selection_fingerprint,
    PageCursor,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{call, call_ok, emulator_with_engine, error_code};
use crate::error_payloads::error_message;
use crate::recommended_actions::{record, stored, MEMBER_ACCOUNT_ID};

/// The two `RecommendedActionType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`).
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// Every paginated preview operation of this service, as `(operation, the output
/// member its page lands in)`.
const PREVIEWS: [(&str, &str); 2] = [
    ("ListAutomationRulePreview", "previewResults"),
    ("ListAutomationRulePreviewSummaries", "previewResultSummaries"),
];

/// The preview whose pages are single actions, which is the one a multi-page
/// walk can be taken over.
const PAGED_PREVIEW: (&str, &str) = PREVIEWS[0];

/// The preview that reports ONE aggregate row per selection.
const SUMMARISING_PREVIEW: (&str, &str) = PREVIEWS[1];

/// The page size every walk below is taken at: the smallest one, so a selection
/// of more than one item leaves a token on its first page whichever preview
/// operation served it.
const ONE_ITEM: i32 = 1;

/// A page size above every total this fixture leaves, inside the modelled
/// `maxResults` range (`range(min: 1, max: 100)`), so one request serves a whole
/// selection.
const WHOLE_SELECTION: i32 = 100;

/// A ceiling on the pages of one walk, so a non-advancing token fails a case
/// instead of hanging it.
const MAX_PAGES: usize = 50;

/// The requirement a token taken for another selection is refused with.
const DIFFERENT_SELECTION: &str =
    "Member must be a token returned for the same rule type, action types and criteria";

/// The analysis inventory the previews report over: four actions in the calling
/// account spanning both action types and two regions, plus one in a member
/// account, so a walk takes more than one page at `maxResults=1` and an
/// organization-wide selection differs from an account-scoped one.
fn inventory() -> Vec<RecommendedAction> {
    vec![
        stored(
            1,
            "vol-aaa1",
            engine::EMULATOR_ACCOUNT_ID,
            "us-east-1",
            14,
            UPGRADE,
            Some("gp2"),
            false,
            &[("Environment", "Production")],
        ),
        stored(
            2,
            "vol-aaa2",
            engine::EMULATOR_ACCOUNT_ID,
            "us-west-2",
            32,
            SNAPSHOT_AND_DELETE,
            Some("gp2"),
            true,
            &[("Environment", "Staging")],
        ),
        stored(
            3,
            "vol-aaa3",
            engine::EMULATOR_ACCOUNT_ID,
            "us-east-1",
            14,
            SNAPSHOT_AND_DELETE,
            Some("io1"),
            false,
            &[],
        ),
        stored(
            4,
            "vol-aaa4",
            engine::EMULATOR_ACCOUNT_ID,
            "us-west-2",
            93,
            UPGRADE,
            Some("gp3"),
            true,
            &[("Environment", "Production")],
        ),
        stored(
            5,
            "vol-aaa5",
            MEMBER_ACCOUNT_ID,
            "us-east-1",
            14,
            UPGRADE,
            Some("gp2"),
            false,
            &[("Environment", "Production")],
        ),
    ]
}

/// The harness with that inventory recorded where Compute Optimizer's analysis
/// would have left it.
fn emulator_with_inventory() -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    record(&engine, inventory());
    router
}

/// An account-scoped preview of both action types, with `members` spliced in.
fn account_preview(members: Value) -> Value {
    let mut request = json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });
    splice(&mut request, members);
    request
}

/// An organization-wide preview of both action types, with `members` spliced in.
fn organization_preview(members: Value) -> Value {
    let mut request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });
    splice(&mut request, members);
    request
}

/// Copy every member of `members` onto `request`.
fn splice(request: &mut Value, members: Value) {
    let (Some(target), Some(members)) = (request.as_object_mut(), members.as_object()) else {
        return;
    };
    for (name, value) in members {
        target.insert(name.clone(), value.clone());
    }
}

/// The items one response reports under `member_name`.
fn items_of(body: &Value, member_name: &str) -> Vec<Value> {
    body.get(member_name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `nextToken` a response carries, if any.
fn token_of(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Everything `request` selects, in one page.
async fn whole_selection(
    router: &MultiServiceRouter,
    operation: &str,
    items: &str,
    request: &Value,
    case: &str,
) -> Vec<Value> {
    let mut whole = request.clone();
    whole["maxResults"] = json!(WHOLE_SELECTION);
    let body = call_ok(router, operation, &whole, case).await;
    assert_eq!(
        token_of(&body),
        None,
        "case {case:?}: the fixture must leave this selection fewer than {WHOLE_SELECTION} items, \
         got {body}"
    );
    items_of(&body, items)
}

/// Walk `operation` one item at a time, taking the first page with `first` and
/// every later page with `resumed_with`, and answer the items served. A page
/// that is refused fails the case with the body it was refused with.
async fn walk_resuming_with(
    router: &MultiServiceRouter,
    operation: &str,
    items: &str,
    first: &Value,
    resumed_with: &Value,
    case: &str,
) -> Vec<Value> {
    let mut served: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = if token.is_none() {
            first.clone()
        } else {
            resumed_with.clone()
        };
        request["maxResults"] = json!(ONE_ITEM);
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {} of the walk must be served, got {status} and {body}",
            pages + 1
        );
        served.extend(items_of(&body, items));
        pages += 1;
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: the walk must terminate, got {} items over {pages} pages",
            served.len()
        );
        token = token_of(&body);
        if token.is_none() {
            break;
        }
    }
    served
}

/// Two spellings of ONE selection, as `(case, one spelling, the other)`.
///
/// Every row names requests whose derived selection is identical — the accounts
/// covered, the requested action types and the conditions applied are the same —
/// so a token taken under either spelling is a window onto the same sequence.
fn equivalent_spellings() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "criteria absent and criteria spelled as an empty object",
            account_preview(json!({})),
            account_preview(json!({ "criteria": {} })),
        ),
        (
            "criteria absent and a criterion carrying no condition",
            account_preview(json!({})),
            account_preview(json!({ "criteria": { "region": [] } })),
        ),
        (
            "an empty criteria object and several criteria carrying no condition",
            account_preview(json!({ "criteria": {} })),
            account_preview(json!({
                "criteria": {
                    "resourceTag": [],
                    "ebsVolumeSizeInGib": [],
                    "restartNeeded": [],
                }
            })),
        ),
        (
            "one constraining criterion, beside a criterion carrying no condition",
            account_preview(json!({
                "criteria": { "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }] }
            })),
            account_preview(json!({
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }],
                    "ebsVolumeType": [],
                }
            })),
        ),
        (
            "an organization scope absent and spelled as an empty object",
            organization_preview(json!({})),
            organization_preview(json!({ "organizationScope": {} })),
        ),
        (
            "an account-scoped preview and an organization scope naming that one account",
            account_preview(json!({})),
            organization_preview(json!({
                "organizationScope": { "accountIds": [caller_account_id()] }
            })),
        ),
    ]
}

/// Requests whose selections really DIFFER, as `(case, the request a token is
/// taken under, the request it is then presented to)`.
fn different_selections() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "fewer action types",
            account_preview(json!({})),
            account_preview(json!({ "recommendedActionTypes": [UPGRADE] })),
        ),
        (
            "a constraining criterion added",
            account_preview(json!({})),
            account_preview(json!({
                "criteria": { "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }] }
            })),
        ),
        (
            "another value in that criterion",
            account_preview(json!({
                "criteria": { "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }] }
            })),
            account_preview(json!({
                "criteria": { "region": [{ "comparison": "StringEquals", "values": ["us-west-2"] }] }
            })),
        ),
        (
            "another comparison on the same values",
            account_preview(json!({
                "criteria": { "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }] }
            })),
            account_preview(json!({
                "criteria": {
                    "region": [{ "comparison": "StringNotEquals", "values": ["us-east-1"] }]
                }
            })),
        ),
        (
            "an account-scoped preview resumed organization-wide",
            account_preview(json!({})),
            organization_preview(json!({})),
        ),
        (
            "an organization scope naming another account",
            organization_preview(json!({
                "organizationScope": { "accountIds": [caller_account_id()] }
            })),
            organization_preview(json!({
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT_ID] }
            })),
        ),
    ]
}

/// The cursor `operation` would mint, at `offset`, for the selection a request
/// of `rule_type` over `action_types` under `criteria` derives — built from the
/// service's own fingerprint of that selection, so presenting it is presenting a
/// token that listing could have issued.
fn selection_cursor(
    operation: &str,
    rule_type: &str,
    action_types: &[String],
    criteria: &Criteria,
    offset: usize,
) -> String {
    let scope = preview_account_scope(rule_type, None);
    encode_page_token(&PageCursor {
        offset,
        filter_fingerprint: preview_selection_fingerprint(
            operation,
            &scope,
            action_types,
            criteria,
        ),
    })
}

/// Assert that `operation` refuses `token` under `request` with the crate's
/// validation report for a token taken for another selection, echoing nothing.
async fn assert_selection_token_refused(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    token: &str,
    case: &str,
) {
    let mut refused = request.clone();
    refused["nextToken"] = json!(token);
    let (status, body) = call(router, operation, &refused).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValueException",
        "case {case:?}: expected error code InvalidParameterValueException, got {body}"
    );
    let message = error_message(&body);
    let expected = format!(
        "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
         {DIFFERENT_SELECTION}"
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(token),
        "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
    );
}

/// Two spellings of one selection are one walk: a token minted under either is
/// served by the other, and the walk that switches spelling after its first page
/// reports exactly the items the selection holds, in order, none served twice
/// and none skipped.
#[tokio::test]
async fn one_selection_spelled_two_ways_continues_one_page_walk() {
    let router = emulator_with_inventory();
    let (operation, items) = PAGED_PREVIEW;

    for (spellings, one, other) in equivalent_spellings() {
        // Both directions: either spelling may be the one that minted the
        // token the other resumes with.
        for (first, resumed_with) in [(&one, &other), (&other, &one)] {
            let case = format!("{operation}: {spellings}");
            let case = case.as_str();
            let expected = whole_selection(&router, operation, items, first, case).await;
            assert!(
                expected.len() > 1,
                "case {case:?}: the fixture must leave this selection more than one item, so \
                 its first page carries a token, got {expected:?}"
            );
            let also_expected =
                whole_selection(&router, operation, items, resumed_with, case).await;
            assert_eq!(
                also_expected, expected,
                "case {case:?}: the two spellings must select the same items; expected \
                 {expected:?}, got {also_expected:?}"
            );

            let served =
                walk_resuming_with(&router, operation, items, first, resumed_with, case).await;
            assert_eq!(
                served, expected,
                "case {case:?}: a walk that switches spelling after its first page must serve \
                 the selection exactly once, in order; expected {expected:?}, got {served:?}"
            );
        }
    }
}

/// The summarising preview reads the SAME selection, so the same two spellings
/// are one selection for it too: it reports one account-wide aggregate row, and
/// both spellings must report that row identically — the same count and the same
/// totals, so neither spelling selects more or fewer actions than the other.
#[tokio::test]
async fn one_selection_spelled_two_ways_reports_one_aggregate_row() {
    let router = emulator_with_inventory();
    let (operation, items) = SUMMARISING_PREVIEW;

    for (spellings, one, other) in equivalent_spellings() {
        let case = format!("{operation}: {spellings}");
        let case = case.as_str();
        let reported = whole_selection(&router, operation, items, &one, case).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the summarising preview reports one aggregate row per selection, got \
             {reported:?}"
        );
        let also_reported = whole_selection(&router, operation, items, &other, case).await;
        assert_eq!(
            also_reported, reported,
            "case {case:?}: the two spellings must report the same row; expected {reported:?}, \
             got {also_reported:?}"
        );
    }
}

/// A token taken for a selection that really is different is still refused, with
/// the crate's validation report for `nextToken` and without echoing the token.
#[tokio::test]
async fn a_token_taken_for_a_different_selection_is_refused() {
    let router = emulator_with_inventory();
    let (operation, items) = PAGED_PREVIEW;

    for (selections, issuing, other) in different_selections() {
        let case = format!("{operation}: {selections}");
        let case = case.as_str();
        let mut first = issuing.clone();
        first["maxResults"] = json!(ONE_ITEM);
        let first_page = call_ok(&router, operation, &first, case).await;
        let token = token_of(&first_page).unwrap_or_else(|| {
            panic!("case {case:?}: the first page must carry a token, got {first_page}")
        });

        let mut resumed = other.clone();
        resumed["maxResults"] = json!(ONE_ITEM);
        assert_selection_token_refused(&router, operation, &resumed, &token, case).await;

        // The same token on the selection that issued it is served, so the
        // refusal above is the selection check and not a blanket rejection.
        let mut resumed_by_its_own = issuing.clone();
        resumed_by_its_own["maxResults"] = json!(ONE_ITEM);
        resumed_by_its_own["nextToken"] = json!(token);
        let served = call_ok(&router, operation, &resumed_by_its_own, case).await;
        assert!(
            !items_of(&served, items).is_empty(),
            "case {case:?}: the issuing selection must resume with its own token, got {served}"
        );
    }
}

/// The summarising preview binds its cursor to the DERIVED selection in the same
/// way: a cursor fingerprinted for one selection is refused under every request
/// that derives another, and served by the request that derives its own. One
/// aggregate row leaves nothing after it, so the position such a cursor names is
/// the end of that row sequence — where a walk whose tail was removed resumes.
#[tokio::test]
async fn a_summary_cursor_is_bound_to_the_selection_it_names() {
    let router = emulator_with_inventory();
    let (operation, items) = SUMMARISING_PREVIEW;
    let both_types = vec![UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()];
    let cursor = selection_cursor(
        operation,
        "AccountRule",
        &both_types,
        &Criteria::default(),
        1,
    );

    let mut own = account_preview(json!({}));
    own["nextToken"] = json!(cursor);
    let served = call_ok(
        &router,
        operation,
        &own,
        "the cursor presented with the selection it names",
    )
    .await;
    assert!(
        items_of(&served, items).is_empty() && token_of(&served).is_none(),
        "case \"the cursor presented with the selection it names\": the remainder after the only \
         row is empty and ends the walk, got {served}"
    );

    for (selections, _, other) in different_selections() {
        let case = format!("{operation}: {selections}");
        assert_selection_token_refused(&router, operation, &other, &cursor, case.as_str()).await;
    }
}

/// The two preview operations page different sequences for one selection —
/// single actions against the one aggregate of them — so each refuses a cursor
/// the other's walk would carry, even when the selection is spelled identically.
#[tokio::test]
async fn the_sibling_preview_operation_refuses_a_token_for_the_same_selection() {
    let router = emulator_with_inventory();
    let request = account_preview(json!({ "criteria": {} }));
    let both_types = vec![UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()];

    // The token the paged preview really minted, presented to the summarising
    // one, and the cursor the summarising one's walk carries, presented to the
    // paged one.
    let mut minting = request.clone();
    minting["maxResults"] = json!(ONE_ITEM);
    let first_page = call_ok(&router, PAGED_PREVIEW.0, &minting, "minting").await;
    let minted = token_of(&first_page).unwrap_or_else(|| {
        panic!("case \"minting\": the first page must carry a token, got {first_page}")
    });
    let summary_cursor = selection_cursor(
        SUMMARISING_PREVIEW.0,
        "AccountRule",
        &both_types,
        &Criteria::default(),
        1,
    );

    for (operation, token) in [
        (SUMMARISING_PREVIEW.0, minted),
        (PAGED_PREVIEW.0, summary_cursor),
    ] {
        let case = format!("{operation} resuming the sibling preview's cursor");
        let mut resumed = request.clone();
        resumed["maxResults"] = json!(ONE_ITEM);
        resumed["nextToken"] = json!(token);
        let (status, body) = call(&router, operation, &resumed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected error code InvalidParameterValueException, got {body}"
        );
    }
}
