// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeNetworkInsightsAccessScopes`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeNetworkInsightsAccessScopes`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's Network Access Scopes narrowed by the `NetworkInsightsAccessScopeIds` the
//! request names and cut into `MaxResults`-sized pages, and it writes nothing. Every table
//! row is a distinct input along what that generalises over — which ids, how many, in which
//! order, which page size, which filter name (the operation documents none, so every one is
//! refused) — so no single recorded call is the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    AccessScopePaths, ComAmazonawsEc2Amazonec2State as Ec2State, MAX_ECHOED_PAGE_TOKEN_CHARS,
    MAX_RESOURCE_IDS, MAX_STORED_RECORDS_PER_STORE, RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for, and the Region the seeded scopes' ARNs name.
const REGION: &str = "us-west-2";

/// One `DescribeNetworkInsightsAccessScopes` call carrying `params`, as its HTTP status and
/// XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeNetworkInsightsAccessScopes", params).await
}

/// The scope ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "networkInsightsAccessScopeId")
}

/// The XML of the one scope `scope_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, scope_id: &str) -> String {
    let (status, body) = describe(
        router,
        &indexed("NetworkInsightsAccessScopeId", &[scope_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded scopes carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// Whether `value` is a timestamp in the form ec2Query writes a modelled
/// `MillisecondDateTime` in — the form `aws_smithy_types` reads back as
/// [`::aws_smithy_types::date_time::Format::DateTime`].
///
/// The INSTANT a scope carries is minted by the store when the scope is created, so it is a
/// per-run value: what is asserted about it is its modelled form, that it is stable across
/// reads, and that an unmodified scope reports its two dates equal.
fn is_wire_timestamp(value: &str) -> bool {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .is_ok()
}

/// One filter-table row: the case name and the `Filters` entries the request carries as
/// `(name, values)` pairs. Every row is refused, because the operation documents no filter.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>);

/// One dry-run refusal row: the case name, the request parameters beside `DryRun=true`, and
/// the EC2 error code the refusal carries.
type DryRunRefusalCase<'a> = (&'a str, Vec<(String, String)>, &'a str);

/// What one seeded scope is created with, beyond the id, the ARN and the dates the service
/// mints itself: the tags a writer chooses.
struct ScopeFixture {
    tags: Vec<Tag>,
}

/// The scopes the id and paging tables are written against. They differ along the one thing
/// the modelled `NetworkInsightsAccessScope` carries that a writer chooses: how many tags
/// (none, one, two, three) and what text they carry (including a value with a space in it).
fn seeded_fixtures() -> Vec<ScopeFixture> {
    vec![
        ScopeFixture {
            tags: vec![tag("Name", "all-traffic")],
        },
        ScopeFixture { tags: Vec::new() },
        ScopeFixture {
            tags: vec![tag("Name", "prod-egress"), tag("Tier", "prod")],
        },
        ScopeFixture {
            tags: vec![
                tag("Name", "dev-egress"),
                tag("Tier", "dev"),
                tag("Owner", "network-team"),
            ],
        },
        ScopeFixture {
            tags: vec![tag("Name", "shared services scope")],
        },
        ScopeFixture {
            tags: vec![tag("Tier", "sandbox")],
        },
    ]
}

/// Write the seeded scopes into the engine's store, the way a creating operation would, and
/// report the ids they were given, in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    seeded_fixtures()
        .into_iter()
        .map(|fixture| {
            guard
                .insert_network_insights_access_scope(
                    REGION,
                    fixture.tags,
                    AccessScopePaths::default(),
                    None,
                )
                .expect("the store must have room for the seeded scope")
        })
        .collect()
}

/// The seeded ids at `positions`, as the expectation tables name them.
fn ids_at(seeded: &[String], positions: &[usize]) -> Vec<String> {
    positions
        .iter()
        .map(|position| seeded[*position].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds no Network Access Scope — nothing has created one — so the
/// operation answers HTTP 200 with an empty set and no token, rather than refusing the call.
#[tokio::test]
async fn a_fresh_account_reports_no_access_scope() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "networkInsightsAccessScopeId").is_empty(),
        "fresh account: expected no scope, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty set, so no token is due, got {body}"
    );
}

/// Every scope the store holds is reported, in store order, and a scope written after a call
/// shows up in the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_stored_scope_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all scopes", &[]).await;
    assert_eq!(
        reported, seeded,
        "all scopes: expected every stored scope in store order, got {reported:?}"
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_network_insights_access_scope(
            REGION,
            vec![tag("Name", "late")],
            AccessScopePaths::default(),
            None,
        )
        .expect("the store must have room for one more scope");

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// Each member the modelled `NetworkInsightsAccessScope` carries round-trips out of the
/// store: the scope id, the ARN built from it, both dates and every tag it was stored with.
#[tokio::test]
async fn every_stored_member_round_trips() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (position, fixture) in seeded_fixtures().into_iter().enumerate() {
        let scope_id = &seeded[position];
        let case = format!("scope {position} ({scope_id})");
        let body = describe_one(&router, &case, scope_id).await;

        let reported_id = element(&body, "networkInsightsAccessScopeId").unwrap_or_default();
        assert_eq!(
            &reported_id, scope_id,
            "case {case:?}: expected networkInsightsAccessScopeId {scope_id}, got \
             {reported_id:?}"
        );

        // "The Amazon Resource Name (ARN) of the Network Access Scope"
        // (`NetworkInsightsAccessScope$NetworkInsightsAccessScopeArn`), built from the
        // scope's own id in the Region the scope was created in and the emulator's canonical
        // account.
        let expected_arn = ::engine::emulator_arn(
            "ec2",
            REGION,
            &format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE}/{scope_id}"),
        );
        let reported_arn = element(&body, "networkInsightsAccessScopeArn").unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {case:?}: expected networkInsightsAccessScopeArn {expected_arn}, got \
             {reported_arn:?}"
        );

        // "The creation date" and "The last updated date"
        // (`NetworkInsightsAccessScope$CreatedDate` / `$UpdatedDate`) are stamped by the
        // store at creation, so each is reported in the modelled timestamp's wire form, a
        // scope nothing has modified reports the two equal, and a second read reports the
        // same instants — the value is minted once, not per call.
        let reported_created = element(&body, "createdDate").unwrap_or_default();
        assert!(
            is_wire_timestamp(&reported_created),
            "case {case:?}: expected createdDate in the modelled timestamp form, got \
             {reported_created:?}"
        );
        let reported_updated = element(&body, "updatedDate").unwrap_or_default();
        assert_eq!(
            reported_updated, reported_created,
            "case {case:?}: an unmodified scope reports updatedDate equal to createdDate \
             {reported_created:?}, got {reported_updated:?}"
        );

        let again = describe_one(&router, &case, scope_id).await;
        let created_again = element(&again, "createdDate").unwrap_or_default();
        assert_eq!(
            created_again, reported_created,
            "case {case:?}: expected the same createdDate {reported_created:?} on a second \
             read, got {created_again:?}"
        );

        let tag_set = element(&body, "tagSet").unwrap_or_default();
        let reported_tags: Vec<(String, String)> = elements(&tag_set, "key")
            .into_iter()
            .zip(elements(&tag_set, "value"))
            .map(|(key, value)| (unescaped(&key), unescaped(&value)))
            .collect();
        let expected_tags: Vec<(String, String)> = fixture
            .tags
            .iter()
            .map(|entry| {
                (
                    entry.key.clone().unwrap_or_default(),
                    entry.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected tagSet {expected_tags:?}, got {reported_tags:?}"
        );
    }
}

/// Describing is reporting: two identical calls report the same scopes and the store is left
/// as it was.
#[tokio::test]
async fn describing_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let first = described_ids(&router, "first read", &[]).await;
    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_insights_access_scopes
            .len(),
        seeded.len(),
        "describing scopes must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// NetworkInsightsAccessScopeIds
// ---------------------------------------------------------------------------

/// `NetworkInsightsAccessScopeIds` narrows the store to the scopes it names, in store order
/// however the request ordered them and however many times it named one.
#[tokio::test]
async fn scope_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: one id, two in store order, the same two reversed, a repeat of one id, and every
    // id at once.
    let cases: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("one id", vec![0], vec![0]),
        ("two ids", vec![1, 4], vec![1, 4]),
        ("two ids, reversed", vec![4, 1], vec![1, 4]),
        ("the same id twice", vec![2, 2], vec![2]),
        ("every id", vec![0, 1, 2, 3, 4, 5], vec![0, 1, 2, 3, 4, 5]),
    ];
    for (case, requested, expected) in cases {
        let asked = ids_at(&seeded, &requested);
        let params = indexed(
            "NetworkInsightsAccessScopeId",
            &asked.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the scopes at {expected:?}, got {reported:?}"
        );
    }
}

/// An id that names no scope of the account is refused with
/// `InvalidNetworkInsightsAccessScopeId.NotFound` — whatever else the request names — rather
/// than reported as an absent row.
#[tokio::test]
async fn a_scope_id_that_names_no_scope_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a well-formed id nothing was created for, a known id beside an unknown one
    // (order reversed in the next row, so the answer cannot depend on position), and a
    // string that is not a scope id at all.
    let unknown = "nis-00000000000000000";
    let known = seeded[0].clone();
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("only an unknown id", vec![unknown]),
        ("a known id then an unknown one", vec![&known, unknown]),
        ("an unknown id then a known one", vec![unknown, &known]),
        ("not an id at all", vec!["not-a-scope"]),
    ];
    for (case, asked) in cases {
        let (status, body) =
            describe(&router, &indexed("NetworkInsightsAccessScopeId", &asked)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkInsightsAccessScopeId.NotFound",
            "case {case:?}: expected InvalidNetworkInsightsAccessScopeId.NotFound, got {body}"
        );
        assert!(
            elements(&body, "networkInsightsAccessScopeId").is_empty(),
            "case {case:?}: a refused request reports no scope, got {body}"
        );
    }
}

/// The id list is caller-sized, so a request AT this crate's ceiling is answered and one
/// PAST it is refused with `InvalidParameterValue` and reports no scope. The ceiling is
/// named by the constant it is derived from rather than by a literal.
#[tokio::test]
async fn an_id_list_at_the_ceiling_is_answered_and_past_it_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded[0].clone();

    let at_ceiling = vec![known.as_str(); MAX_RESOURCE_IDS];
    let (status, body) = describe(
        &router,
        &indexed("NetworkInsightsAccessScopeId", &at_ceiling),
    )
    .await;
    assert_eq!(
        status, 200,
        "MAX_RESOURCE_IDS ids: expected HTTP 200 at the ceiling, got {status} with body {body}"
    );

    let past_ceiling = vec![known.as_str(); MAX_RESOURCE_IDS + 1];
    let (status, body) = describe(
        &router,
        &indexed("NetworkInsightsAccessScopeId", &past_ceiling),
    )
    .await;
    assert_eq!(
        status, 400,
        "one id past MAX_RESOURCE_IDS: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "one id past MAX_RESOURCE_IDS: expected InvalidParameterValue, got {body}"
    );
    assert!(
        elements(&body, "networkInsightsAccessScopeId").is_empty(),
        "one id past MAX_RESOURCE_IDS: a refused request reports no scope, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// "There are no supported filters"
/// (`smithy_query(query="doc",shape="DescribeNetworkInsightsAccessScopesRequest",member="Filters")`),
/// so ANY filter a request names is refused with `InvalidParameterValue` and nothing is
/// reported — a filter this operation cannot evaluate must never read as "no results". Rows
/// vary what a caller might plausibly try: a member name, EC2's `tag:` form, a name borrowed
/// from a sibling describe, one with no value, and several filters at once.
#[tokio::test]
async fn any_filter_is_refused_because_the_operation_documents_none() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let first_id = [seeded[0].as_str()];
    let third_id = [seeded[2].as_str()];
    let cases: Vec<FilterCase<'_>> = vec![
        (
            "a member name",
            vec![("network-insights-access-scope-id", &first_id[..])],
        ),
        ("the tag form", vec![("tag:Name", &["all-traffic"][..])]),
        ("tag-key", vec![("tag-key", &["Tier"][..])]),
        (
            "a sibling describe's name",
            vec![("coip-pool.pool-id", &["ipv4pool-coip-*"][..])],
        ),
        ("a filter with no value", vec![("created-date", &[][..])]),
        (
            "two filters",
            vec![
                ("tag:Tier", &["prod"][..]),
                ("network-insights-access-scope-id", &third_id[..]),
            ],
        ),
    ];
    for (case, entries) in cases {
        let params = filters(&entries);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        // The message is built from the request: it names the filter that was refused.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        let named = entries[0].0;
        assert_eq!(
            message,
            format!("The filter '{named}' is invalid"),
            "case {case:?}: expected the message to name the filter, got {message:?}"
        );
        assert!(
            elements(&body, "networkInsightsAccessScopeId").is_empty(),
            "case {case:?}: a refused request reports no scope, got {body}"
        );
    }

    // A refused filter is refused however long its name is, and the message repeats back no
    // more of it than the echo bound.
    let oversize = "z".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS * 4);
    let (status, body) = describe(&router, &filters(&[(oversize.as_str(), &["x"][..])])).await;
    assert_eq!(
        status, 400,
        "an oversize filter name: expected HTTP 400, got {status} with body {body}"
    );
    let message = unescaped(&element(&body, "Message").unwrap_or_default());
    assert_eq!(
        message,
        format!(
            "The filter '{}' is invalid",
            "z".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS)
        ),
        "an oversize filter name: expected the echo bounded at MAX_ECHOED_PAGE_TOKEN_CHARS, \
         got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Whatever page size a caller asks for, walking the tokens reports every stored scope
/// exactly once in store order, and no page is longer than `MaxResults`.
#[tokio::test]
async fn paging_walks_every_scope_exactly_once_at_any_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a page size that does not divide the set, one that does, one that is the whole
    // set, and the largest the model publishes.
    for max_results in [1, 2, 3, 4, 6, 100] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "networkInsightsAccessScopeId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every scope exactly once in store order, got {walked:?}"
        );
    }
}

/// A token is issued exactly when a scope follows the page, resuming from it starts at that
/// scope, and a page is cut from the SELECTED set rather than from the whole store.
#[tokio::test]
async fn a_token_is_issued_only_when_a_scope_follows_the_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(status, 200, "first page: got {status} {body}");
    let first = elements(&body, "networkInsightsAccessScopeId");
    assert_eq!(
        first,
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "first page: expected the first five scopes, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut params = max_results_param(5);
    params.push(("NextToken".to_string(), token));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "second page: got {status} {body}");
    let second = elements(&body, "networkInsightsAccessScopeId");
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "second page: expected the scope after the first five, got {second:?}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "second page: nothing follows it, so no token is due, got {body}"
    );

    // The same page size over a set narrowed by ids that ends inside the page: no token is
    // due.
    let asked = ids_at(&seeded, &[0, 2]);
    let mut narrowed = indexed(
        "NetworkInsightsAccessScopeId",
        &asked.iter().map(String::as_str).collect::<Vec<&str>>(),
    );
    narrowed.extend(max_results_param(5));
    let (status, body) = describe(&router, &narrowed).await;
    assert_eq!(status, 200, "narrowed page: got {status} {body}");
    assert_eq!(
        elements(&body, "networkInsightsAccessScopeId"),
        asked,
        "narrowed page: expected the two scopes named, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "narrowed page: the selected set ends here, so no token is due, got {body}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`NetworkInsightsMaxResults`, `range 1..100`) and one past each.
    let cases = [(0, 400), (1, 200), (100, 200), (101, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and a token AT the length the message
    // echoes in full.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded[0].clone(),
        "z".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        // EC2's message for this refusal is one fixed sentence, naming no token.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {token:?}: expected EC2's own pagination-token sentence, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ceiling on the store itself
// ---------------------------------------------------------------------------

/// Nothing in this emulator reclaims a stored scope, so the store is bounded by its own
/// ceiling rather than by the host's memory: the writer stores a scope while there is room
/// and refuses one past `MAX_STORED_RECORDS_PER_STORE` without storing anything, and the
/// describe keeps reporting what is there, a published page at a time.
#[tokio::test]
async fn the_store_refuses_a_scope_past_its_ceiling_and_the_describe_still_pages() {
    let (router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();

    {
        let mut guard = store.lock();
        for ordinal in 0..MAX_STORED_RECORDS_PER_STORE {
            assert!(
                guard
                    .insert_network_insights_access_scope(
                        REGION,
                        Vec::new(),
                        AccessScopePaths::default(),
                        None,
                    )
                    .is_some(),
                "scope {ordinal}: expected the store to have room below its ceiling"
            );
        }
        let past_ceiling = guard.insert_network_insights_access_scope(
            REGION,
            Vec::new(),
            AccessScopePaths::default(),
            None,
        );
        assert!(
            past_ceiling.is_none(),
            "past the ceiling: expected no scope to be stored, got {past_ceiling:?}"
        );
        let held = guard.network_insights_access_scopes.len();
        assert_eq!(
            held, MAX_STORED_RECORDS_PER_STORE,
            "past the ceiling: the store must hold exactly its ceiling, got {held}"
        );
    }

    // The largest page the model publishes, out of a store at its ceiling: a full page and a
    // token for the rest.
    let (status, body) = describe(&router, &max_results_param(100)).await;
    assert_eq!(
        status, 200,
        "a store at its ceiling: expected HTTP 200, got {status} with body {body}"
    );
    let page = elements(&body, "networkInsightsAccessScopeId");
    assert_eq!(
        page.len(),
        100,
        "a store at its ceiling: expected a full page of 100, got {} ids",
        page.len()
    );
    assert_eq!(
        elements(&body, "nextToken").len(),
        1,
        "a store at its ceiling: scopes follow the page, so a token is due, got {body}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "networkInsightsAccessScopeId").is_empty(),
        "DryRun=true: a dry run reports no scope, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}

/// A dry run asks whether the request WOULD have succeeded, so one that would have been
/// refused for a parameter is refused with THAT reason rather than reported as a clean
/// `DryRunOperation`. Rows differ along which parameter is wrong: the token, the filter name
/// and the size of the id list.
#[tokio::test]
async fn a_dry_run_of_a_request_that_would_be_refused_reports_the_refusal() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded[0].clone();

    let dry_run = ("DryRun".to_string(), "true".to_string());
    let oversize_ids: Vec<&str> = vec![known.as_str(); MAX_RESOURCE_IDS + 1];
    let cases: Vec<DryRunRefusalCase<'_>> = vec![
        (
            "a bogus NextToken",
            vec![("NextToken".to_string(), "not-a-token".to_string())],
            "InvalidPaginationToken",
        ),
        (
            "an unsupported filter",
            filters(&[("tag:Name", &["all-traffic"][..])]),
            "InvalidParameterValue",
        ),
        (
            "an id list past the ceiling",
            indexed("NetworkInsightsAccessScopeId", &oversize_ids),
            "InvalidParameterValue",
        ),
        (
            "an id naming no scope of the account",
            indexed("NetworkInsightsAccessScopeId", &["nis-00000000000000000"]),
            "InvalidNetworkInsightsAccessScopeId.NotFound",
        ),
    ];
    for (case, params, expected_code) in cases {
        let mut with_dry_run = vec![dry_run.clone()];
        with_dry_run.extend(params);
        let (status, body) = describe(&router, &with_dry_run).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 rather than the dry-run outcome, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}
