// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAgentProfiles`: the page of `AgentProfileSummary`
//! projections of the profiles `CreateAgentProfile` minted.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. Every stored profile is listed, and the summary the list reports is the SAME
//!    profile `GetAgentProfile` reports: the members the request carried, the
//!    service-owned `arn` / `createdBy` / `createdAt`, and the derived
//!    `eligibleFor…` / `fieldErrors` / `tags`.
//! 2. Paging enumerates every profile EXACTLY ONCE: no page is longer than the
//!    `maxResults` asked for, the ARNs are strictly ascending across the pages, the union
//!    of the pages is the whole stored set, and only the last page omits `nextToken`.
//! 3. `maxResults` is held to its published `@range(min: 1, max: 1000)`: both bounds are
//!    served, and a value one past either bound is refused `400 ValidationException`.
//! 4. A `nextToken` this service did not issue is refused `400 ValidationException` with
//!    reason `CANNOT_PARSE` naming the member — never an empty page, and never the first
//!    page again. A cursor another paginated operation issued is not a cursor here.
//! 5. The list is a REPORT: an account with no profiles answers an empty `items` and no
//!    `nextToken`, and listing (once, repeatedly, or page by page) leaves the stored set
//!    exactly as it was, so a later caller sees what it would have seen.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profiles are
//! minted through `super::agent_goals`'s `CreateAgentProfile` helpers, because a profile
//! is created by exactly one operation.

use serde_json::{json, Value};

use super::agent_goals::{
    assert_profile_projection, create_profile, get_profile, member, profile_body, tags_by_key,
    PILLARS,
};
use super::workload_harness::{Answer, Client};

/// The published bounds of `ListAgentProfilesRequest$maxResults`
/// (`@range(min: 1, max: 1000)` —
/// `smithy_query(query="operation", operation="ListAgentProfiles")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// `GET /api/v1/agent-profiles`, with the paging members bound to the query string
/// (`maxResults` / `nextToken` are `@httpQuery`). `query` is passed as sent, empty for a
/// request that names none.
///
/// `pub(crate)` because `super::agent_profile_tag_writes` reads a tagged profile back
/// through the LIST as well as through the single-profile read, and a profile is listed by
/// exactly one operation: the two suites must not disagree about how that request is
/// spelled.
pub(crate) async fn list_profiles(client: &Client, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/api/v1/agent-profiles".to_string()
    } else {
        format!("/api/v1/agent-profiles?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `items` a `ListAgentProfiles` answer carries.
fn items(answer: &Answer) -> &[Value] {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("expected an items list in {answer:?}"))
}

fn arn_of(value: &Value) -> String {
    value
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `nextToken` an answer carries, if any.
fn next_token_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// How many profiles the store holds, read through the state the handlers share: what a
/// read must leave untouched.
fn stored_profile_count(client: &Client) -> usize {
    client.store().lock().agent_profiles.len()
}

/// The rows every listing test spans. They differ along what the rule generalises over:
/// how many `aggregationConfiguration` scopes the profile names (none — the modelled
/// `@length(min: 0)` floor — one, two), how many pillars it names (one, two, the whole
/// published catalog, which is the modelled `@length(1, 5)` maximum), whether it carries
/// tags, and whether `deletionProtection` is sent or left to its modelled `default: true`.
fn profile_rows() -> Vec<(&'static str, Value)> {
    let mut rows: Vec<(&'static str, Value)> = Vec::new();

    rows.push((
        "no-aggregation-scope-no-tags",
        profile_body("listed-no-scope", &["SECURITY"], 0),
    ));

    let mut tagged = profile_body("listed-one-scope", &["COST_OPTIMIZATION", "PERFORMANCE"], 1);
    tagged["tags"] = json!([
        { "key": "team", "value": "platform" },
        { "key": "cost-centre", "value": "4711" },
    ]);
    rows.push(("one-scope-two-pillars-tagged", tagged));

    let mut unprotected = profile_body("listed-two-scopes", &PILLARS, 2);
    unprotected["deletionProtection"] = json!(false);
    rows.push(("two-scopes-every-pillar-unprotected", unprotected));

    // A pillar the issue did not cite, and a single tag — `AgentProfileSummary$tags` is a
    // list of `Tag` key/value pairs, both `@length(min: 1)`
    // (`smithy_query(query="shape", shape="Tag")`).
    let mut sparse = profile_body("listed-operational", &["OPERATIONAL_EXCELLENCE"], 1);
    sparse["tags"] = json!([{ "key": "owner", "value": "platform-team" }]);
    rows.push(("pillar-the-issue-did-not-cite", sparse));

    rows
}

/// Rule 1: every created profile is listed, and its listed summary is the profile
/// `GetAgentProfile` reports, member for member.
#[tokio::test]
async fn every_created_profile_is_listed_as_the_profile_a_read_reports() {
    let client = Client::new();

    let mut created: Vec<(&str, Value, String)> = Vec::new();
    for (case, body) in profile_rows() {
        let answer = create_profile(&client, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAgentProfile 200, got {answer:?}"
        );
        created.push((case, body, member(&answer, "arn").to_string()));
    }

    // One page wide enough for the whole set, so the rule is asserted on every row.
    let listed = list_profiles(&client, &format!("maxResults={}", created.len())).await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentProfiles 200, got {listed:?}"
    );
    assert_eq!(
        items(&listed).len(),
        created.len(),
        "expected every created profile to be listed, got {listed:?}"
    );
    assert!(
        next_token_of(&listed).is_none(),
        "a page that served the whole set must omit nextToken, got {listed:?}"
    );

    for (case, body, arn) in &created {
        let summary = items(&listed)
            .iter()
            .find(|item| &arn_of(item) == arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} must be listed, got {listed:?}"));

        // The stored, service-owned and derived members of the listed summary, against
        // the body THIS row sent — pinned by the crate's one profile-projection helper,
        // so the list is held to the same projection `CreateAgentProfile` and
        // `GetAgentProfile` are. `deletionProtection` is modelled `default: true`, so a
        // row that sends nothing must be listed protected.
        let expected_protection = body
            .get("deletionProtection")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        // A profile is eligible for generation exactly when it names a scope to analyse.
        let eligible = body
            .get("aggregationConfiguration")
            .and_then(Value::as_array)
            .is_some_and(|scopes| !scopes.is_empty());
        assert_profile_projection(
            summary,
            body,
            case,
            "ListAgentProfiles",
            expected_protection,
            eligible,
        );
        // Nothing has modified a freshly created profile, so the `lastModified…` members
        // are absent rather than echoing the create.
        for absent in ["lastModifiedBy", "lastModifiedAt"] {
            assert!(
                summary.get(absent).is_none_or(Value::is_null),
                "case {case:?}: {absent} must be absent on an unmodified profile, \
                 got {summary:?}"
            );
        }

        // The listed summary and the single-profile read are projections of ONE stored
        // profile, so every member they share agrees.
        let fetched = get_profile(&client, arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetAgentProfile 200, got {fetched:?}"
        );
        for shared in [
            "name",
            "displayName",
            "description",
            "businessOverview",
            "pillars",
            "deletionProtection",
            "executionRoleArn",
            "aggregationConfiguration",
            "arn",
            "eligibleForScheduledGeneration",
            "eligibleForArchitectureGeneration",
            "fieldErrors",
            "profileType",
            "createdBy",
            "createdAt",
        ] {
            assert_eq!(
                summary.get(shared),
                fetched.body.get(shared),
                "case {case:?}: listed {shared} must be what a read of the profile \
                 reports, got {summary:?} and {fetched:?}"
            );
        }
        // A listing projects `AgentProfileSummary`, which declares no `tags` member, while
        // `GetAgentProfileResponse` does: the tags a read reports are the ones the create
        // sent, and the listing carries none.
        assert_eq!(
            summary.get("tags"),
            None,
            "case {case:?}: a listed profile carries no tags member, got {summary:?}"
        );
        assert_eq!(
            tags_by_key(fetched.body.get("tags")),
            tags_by_key(body.get("tags")),
            "case {case:?}: a read of the profile reports the tags as sent, got {fetched:?}"
        );
    }
}

/// Rule 2: paging over a stored set with several page sizes enumerates every profile
/// exactly once, in ascending ARN order, and only the last page omits `nextToken`.
#[tokio::test]
async fn paging_enumerates_every_profile_exactly_once() {
    let client = Client::new();

    let stored = 7;
    let mut all_arns: Vec<String> = Vec::new();
    for index in 0..stored {
        let answer = create_profile(
            &client,
            profile_body(&format!("paged-profile-{index}"), &["SECURITY"], index % 2),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentProfile 200 for profile {index}, got {answer:?}"
        );
        all_arns.push(member(&answer, "arn").to_string());
    }
    all_arns.sort();

    // A page size below, at and above the stored count, plus the request that names none
    // (the member's `@default(100)`), so the rule is asserted where the page divides the
    // set evenly, unevenly and not at all.
    for page_size in [None, Some(1), Some(2), Some(3), Some(stored), Some(stored + 4)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match page_size {
                Some(size) => format!("maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_profiles(&client, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListAgentProfiles 200, got {answer:?}"
            );
            let page: Vec<String> = items(&answer).iter().map(arn_of).collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page must not be longer than maxResults, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {stored} profiles"
            );
            next_token = next_token_of(&answer);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_arns,
            "case {case:?}: paging must enumerate every stored profile exactly once, in \
             ascending ARN order"
        );
    }
}

/// Rule 3: `maxResults` at each published bound is served; one past either bound is
/// refused by the boundary as a `400 ValidationException`.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let client = Client::new();
    for index in 0..2 {
        let answer = create_profile(
            &client,
            profile_body(&format!("bounded-profile-{index}"), &["RESILIENCE"], 1),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentProfile 200 for profile {index}, got {answer:?}"
        );
    }

    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let answer = list_profiles(&client, &format!("maxResults={max_results}")).await;
        assert_eq!(
            answer.status, 200,
            "case {max_results}: a published bound must be served, got {answer:?}"
        );
        let served = items(&answer).len();
        // Two profiles are stored, so the page is exactly as long as the smaller of the
        // bound and the stored set.
        let expected = usize::try_from(max_results).expect("a positive bound").min(2);
        assert_eq!(
            served, expected,
            "case {max_results}: expected {expected} of the 2 stored profiles, got {served}"
        );
    }

    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = list_profiles(&client, &format!("maxResults={max_results}")).await;
        assert_eq!(
            answer.status, 400,
            "case {max_results}: a value past a published bound must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {max_results}: expected a ValidationException, got {answer:?}"
        );
    }
}

/// Rule 4: a `nextToken` this service did not issue is a refusal, not a page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let created = create_profile(&client, profile_body("token-profile", &["SECURITY"], 1)).await;
    assert_eq!(
        created.status, 200,
        "expected CreateAgentProfile 200, got {created:?}"
    );

    // Each row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary passes it to the handler: what is under test is the handler's own refusal.
    // The last two rows are cursors OTHER paginated operations of this service issue,
    // which are not cursors here.
    let rows: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        (
            "an-agent-context-cursor",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::encode_agent_context_next_token(
                "9f8e7d6c-5b4a-3210-9f8e-7d6c5b4a3210",
            ),
        ),
        (
            "a-workload-share-cursor",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::encode_share_next_token(
                "0123456789abcdef0123456789abcdef",
            ),
        ),
    ];
    for (case, token) in rows {
        let answer = list_profiles(&client, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued nextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable cursor is CANNOT_PARSE, got {answer:?}"
        );
        let fields: Vec<&str> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["NextToken"],
            "case {case:?}: the finding must name the member whose value could not be \
             decoded, got {answer:?}"
        );
    }
}

/// Rule 5: the list reports and never changes what a later caller sees — an account with
/// no profiles, and the stored set before and after every shape of listing.
#[tokio::test]
async fn listing_reports_without_changing_the_stored_set() {
    let client = Client::new();

    let empty = list_profiles(&client, "").await;
    assert_eq!(
        empty.status, 200,
        "an account with no profiles must be served, got {empty:?}"
    );
    assert!(
        items(&empty).is_empty() && next_token_of(&empty).is_none(),
        "an account with no profiles reports an empty items and no nextToken, got {empty:?}"
    );
    assert_eq!(
        stored_profile_count(&client),
        0,
        "listing an empty account must store nothing, got {empty:?}"
    );

    let rows = profile_rows();
    let mut arns: Vec<String> = Vec::new();
    for (case, body) in &rows {
        let answer = create_profile(&client, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAgentProfile 200, got {answer:?}"
        );
        arns.push(member(&answer, "arn").to_string());
    }
    arns.sort();

    let before = stored_profile_count(&client);
    // Every shape of read: the default page, a page that cuts the set, a walk of every
    // page, and the same request twice — none of them may consume, expire or mint
    // anything.
    let queries = [
        String::new(),
        "maxResults=1".to_string(),
        format!("maxResults={}", rows.len()),
        format!("maxResults={MAX_RESULTS_MAX}"),
    ];
    for query in &queries {
        for repeat in 0..2 {
            let answer = list_profiles(&client, query).await;
            assert_eq!(
                answer.status, 200,
                "case {query:?}/{repeat}: expected ListAgentProfiles 200, got {answer:?}"
            );
            let served: Vec<String> = items(&answer).iter().map(arn_of).collect();
            let expected: Vec<String> = arns
                .iter()
                .take(served.len())
                .cloned()
                .collect();
            assert_eq!(
                served, expected,
                "case {query:?}/{repeat}: a repeated read must serve the same page from \
                 the start of the set, got {answer:?}"
            );
            assert_eq!(
                stored_profile_count(&client),
                before,
                "case {query:?}/{repeat}: a read must leave the stored set as it was, \
                 got {answer:?}"
            );
        }
    }

    // And every profile is still readable, unchanged, after all that listing.
    for arn in &arns {
        let fetched = get_profile(&client, arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {arn:?}: a listed profile must still be readable, got {fetched:?}"
        );
    }
}
