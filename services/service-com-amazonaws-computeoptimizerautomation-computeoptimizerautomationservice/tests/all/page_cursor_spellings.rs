// SPDX-License-Identifier: CC0-1.0

//! Which CURSORS a paginated listing of this service accepts: the ones it could
//! itself have issued, and nothing else.
//!
//! The rule under test: a `nextToken` is this service's only when it is exactly
//! what `state::encode_page_token` writes for a cursor — the lowercase hex of
//! `<offset>:<fingerprint>`, the offset in the single spelling `usize`'s
//! `Display` renders — and only when the sequence it resumes REACHES the position
//! that cursor names, because a token is minted only while the walk still had an
//! item to resume at. Every other spelling of the same cursor, and every position
//! beyond the end of the walked sequence, is a token the listing never issued and
//! gets the one refusal it words for those: HTTP 400,
//! `InvalidParameterValueException`, and the crate's validation report on
//! `nextToken`.
//!
//! The table spans the class rather than one token: the half of the token that is
//! re-spelled (the decimal offset — a leading zero, several, a sign, surrounding
//! space, a digit separator, a radix prefix — and the hex around it, in the
//! uppercase digits the encoder never writes), the positions past the end
//! (just past it, well past it, and the largest a `usize` can carry), and the
//! operation it is driven through: EVERY paginated listing of this service, the
//! ones whose pages are the selected items and the ones that reduce them to a
//! single summary row, each driven the way a client drives it — an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts.
//! The decoder itself is then driven over a span of
//! offsets directly, where the round trip is the property: every cursor the
//! encoder writes decodes back to itself, and no re-spelling of one decodes at
//! all.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    Criteria, RecommendedActionFilter,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, PageCursor,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{call, call_ok, is_next_token};
use crate::page_token_refusals::{assert_token_refused, paginated_operations};
use crate::shared_page_walk::emulator_with_a_fixture;

/// The two `RecommendedActionType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), both of which
/// the fixture's inventory holds actions of.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// A page size above every total the fixture leaves, inside the smallest
/// `maxResults` range this service models (`range(min: 1, max: 100)`), so one
/// request serves a whole sequence and reports its length.
const WHOLE_LISTING: i32 = 100;

/// The page size that makes a listing with more than one item mint a token.
const SMALLEST_PAGE: i32 = 1;

/// The request each listing is walked with: the one carrying every required
/// member, with both modelled action types where the listing previews them.
///
/// Naming both types leaves the previews more than one selected action, exactly
/// as the fixture leaves every other listing something to page.
fn walk_request(operation: &str, required: &Value) -> Value {
    let mut request = required.clone();
    if operation.starts_with("ListAutomationRulePreview") {
        request["recommendedActionTypes"] = json!([UPGRADE, SNAPSHOT_AND_DELETE]);
    }
    request
}

/// The listings that report ONE account-wide aggregate row per selection
/// (`crate::recommended_action_summaries`,
/// `crate::automation_rule_preview_summaries`): a page of theirs is never cut,
/// so they mint no token, and the cursor their walk can carry is computed from
/// the service's own fingerprint of the query below rather than read out of a
/// minted one.
const AGGREGATE_SUMMARIES: [&str; 2] = [
    "ListRecommendedActionSummaries",
    "ListAutomationRulePreviewSummaries",
];

/// The fingerprint one of `AGGREGATE_SUMMARIES` binds into the cursors of the
/// walk [`walk_request`] describes for it: its filter list and the OU
/// restriction (which that request leaves out) for the recommended-action
/// summaries, and the derived preview
/// selection — the accounts an `AccountRule` covers, both action types, no
/// criteria — for the preview summaries.
fn aggregate_fingerprint(operation: &str) -> String {
    if operation == AGGREGATE_SUMMARIES[0] {
        return state::page_fingerprint(
            operation,
            &(
                Vec::<RecommendedActionFilter>::new(),
                None::<Vec<String>>,
            ),
        );
    }
    state::preview_selection_fingerprint(
        operation,
        &state::preview_account_scope("AccountRule", None),
        &[UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()],
        &Criteria::default(),
    )
}

/// How many items a response reports.
///
/// Every paginated output of this service carries its page in its ONE list
/// member beside the `nextToken` string, so the items are the entries of the
/// only array the body holds — which keeps the cases below from repeating each
/// listing's member name.
fn reported_count(body: &Value) -> usize {
    body.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, value)| value.as_array())
        .map(Vec::len)
        .sum()
}

/// The requirement a listing reports for a token it never issued.
fn unissued_requirement(operation: &str) -> String {
    format!("Member must be a token returned by a previous {operation} call")
}

/// Present `token` to `operation` over `request` and answer the status and body.
async fn present(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    token: &str,
) -> (u16, Value) {
    let mut resumed = request.clone();
    resumed["nextToken"] = json!(token);
    call(router, operation, &resumed).await
}

/// The spellings of `offset` that `usize::from_str` reads but `usize`'s
/// `Display` — the one renderer `state::encode_page_token` writes the offset
/// through — never emits, as `(case, spelling)`.
fn respellings(offset: usize) -> Vec<(String, String)> {
    vec![
        ("a leading zero".to_string(), format!("0{offset}")),
        ("three leading zeros".to_string(), format!("000{offset}")),
        ("a leading plus".to_string(), format!("+{offset}")),
        ("a leading minus".to_string(), format!("-{offset}")),
        ("a leading space".to_string(), format!(" {offset}")),
        ("a trailing space".to_string(), format!("{offset} ")),
        ("a digit separator".to_string(), format!("{offset}_0")),
        ("a radix prefix".to_string(), format!("0x{offset}")),
        ("a trailing decimal point".to_string(), format!("{offset}.0")),
        ("the position written in words".to_string(), "one".to_string()),
    ]
}

/// A cursor under `fingerprint`, as the opaque token a client presents.
fn token_at(offset: usize, fingerprint: &str) -> String {
    state::encode_page_token(&PageCursor {
        offset,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// On every paginated listing of this service: a cursor re-spelled a way the
/// encoder cannot write, and a cursor naming a position past the end of the
/// sequence, are refused exactly as a token that does not decode at all — while
/// the token the listing really minted resumes it.
#[tokio::test]
async fn a_cursor_this_listing_could_not_have_issued_is_refused() {
    let (router, event_id) = emulator_with_a_fixture().await;

    for (operation, required, _) in paginated_operations(&event_id) {
        let request = walk_request(operation, &required);

        // How far a cursor of this listing can reach: the whole sequence in one
        // page.
        let mut whole = request.clone();
        whole["maxResults"] = json!(WHOLE_LISTING);
        let whole_page = call_ok(&router, operation, &whole, operation).await;
        let length = reported_count(&whole_page);

        // The cursor this listing's walk carries after its first item, and the
        // fingerprint inside it: every token below is that same cursor,
        // re-spelled or moved past the end.
        let fingerprint = if AGGREGATE_SUMMARIES.contains(&operation) {
            assert_eq!(
                length, 1,
                "case {operation:?}: this listing reports one aggregate row, got {whole_page}"
            );
            aggregate_fingerprint(operation)
        } else {
            assert!(
                length > 1,
                "case {operation:?}: the fixture must leave this listing more than one item, so \
                 its first page mints a token, got {whole_page}"
            );
            let mut first = request.clone();
            first["maxResults"] = json!(SMALLEST_PAGE);
            let first_page = call_ok(&router, operation, &first, operation).await;
            let minted = first_page
                .get("nextToken")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {operation:?}: a page of one of {length} items must mint a token, got {first_page}")
                })
                .to_string();
            let cursor = state::decode_page_token(&minted).unwrap_or_else(|| {
                panic!("case {operation:?}: a minted token must decode to a cursor, got {minted:?}")
            });
            assert_eq!(
                cursor.offset, 1,
                "case {operation:?}: a first page of one item stops before the second, so its \
                 cursor resumes at 1, got {cursor:?}"
            );

            // The control: the cursor as the service spelled it resumes the
            // walk, so every refusal below is about the spelling or the position
            // and not a blanket rejection.
            let (status, parsed) = present(&router, operation, &request, &minted).await;
            assert_eq!(
                status, 200,
                "case {operation:?}: the token this listing minted must resume it, got {status} \
                 and {parsed}"
            );
            cursor.filter_fingerprint
        };

        // The cursor itself, as the encoder writes it: the position a walk of
        // this listing stops at after one item.
        let reference = token_at(1, &fingerprint);

        // (case, the token presented): that cursor re-spelled, the same
        // bytes in the uppercase hex the encoder never writes, and the positions
        // no page of this sequence ever stopped at.
        let mut rows: Vec<(String, String)> = respellings(1)
            .into_iter()
            .map(|(case, spelling)| {
                (
                    format!("the offset with {case}"),
                    hex::encode(format!("{spelling}:{fingerprint}")),
                )
            })
            .collect();
        if reference.to_uppercase() != reference {
            rows.push((
                "the same bytes in uppercase hex".to_string(),
                reference.to_uppercase(),
            ));
        }
        for beyond in [length + 1, length + 2, length * 2 + 1, 9_999, usize::MAX] {
            rows.push((
                format!("the position {beyond} past a sequence of {length}"),
                token_at(beyond, &fingerprint),
            ));
        }

        for (row, token) in rows {
            let case = format!("{operation}: {row}");
            assert!(
                is_next_token(&token),
                "case {case:?}: the case must satisfy the modelled NextToken pattern, so it \
                 reaches the handler, got {token:?}"
            );
            let message = assert_token_refused(
                &router,
                operation,
                &request,
                &token,
                &unissued_requirement(operation),
                &case,
            )
            .await;
            assert!(
                !message.contains(&token),
                "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
            );
        }

        // The end of the sequence itself is the one position a cursor may name
        // without an item there: that is where a walk whose tail was removed
        // after the token was minted resumes, and what it resumes is the empty
        // remainder — no items, and no token, because nothing follows.
        let (status, body) =
            present(&router, operation, &request, &token_at(length, &fingerprint)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a cursor at the end of the sequence must serve its empty \
             remainder, got {status} and {body}"
        );
        assert_eq!(
            reported_count(&body),
            0,
            "case {operation:?}: the empty remainder holds no items, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {operation:?}: nothing follows the empty remainder, so it carries no token, got \
             {body}"
        );
    }
}

/// The decoder is the strict inverse of the encoder: every cursor the encoder
/// writes decodes back to itself, and no other spelling of that cursor decodes
/// to anything.
///
/// Driven over a span of offsets — every position a short sequence can hold, the
/// sizes a long one reaches, and the largest a `usize` can carry — against a
/// fingerprint this service really mints, so the round trip is asserted on the
/// values a listing puts in a token rather than on one of them.
#[test]
fn only_the_spelling_the_encoder_writes_decodes_to_a_cursor() {
    // A fingerprint of a real query of this service: the one `ListAccounts`
    // binds into its tokens, which takes no query member at all.
    let fingerprint = state::page_fingerprint("ListAccounts", &());

    let offsets: Vec<usize> = (0..=32)
        .chain([99, 1_000, 65_536, 4_294_967_296, usize::MAX])
        .collect();
    for offset in offsets {
        let token = token_at(offset, &fingerprint);
        assert!(
            is_next_token(&token),
            "case {offset}: a minted token must match ^[A-Za-z0-9+/=]+$, got {token:?}"
        );
        assert_eq!(
            state::decode_page_token(&token),
            Some(PageCursor {
                offset,
                filter_fingerprint: fingerprint.clone(),
            }),
            "case {offset}: the cursor the encoder wrote must decode back to itself, got {token:?}"
        );

        for (case, spelling) in respellings(offset) {
            let respelled = hex::encode(format!("{spelling}:{fingerprint}"));
            let decoded = state::decode_page_token(&respelled);
            assert_eq!(
                decoded, None,
                "case {offset} with {case}: {spelling:?} is a spelling the encoder cannot write, \
                 so it must decode to no cursor, got {decoded:?}"
            );
        }

        // The hex half of the same spelling rule: the bytes of a real token,
        // written with the uppercase digits `hex::encode` never emits.
        let uppercase = token.to_uppercase();
        assert_ne!(
            uppercase, token,
            "case {offset}: a token carries the hex of a colon, so it always holds a letter"
        );
        let decoded = state::decode_page_token(&uppercase);
        assert_eq!(
            decoded, None,
            "case {offset}: uppercase hex is a spelling the encoder cannot write, so it must \
             decode to no cursor, got {decoded:?}"
        );
    }
}
