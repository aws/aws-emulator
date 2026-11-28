// SPDX-License-Identifier: CC0-1.0

//! The feed lifecycle, end to end over the wire: `CreateFeed`, `GetFeed`, `ListFeeds`,
//! `UpdateFeed`, `DeleteFeed`.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * a feed is MINTED and PERSISTED: the create response is built from the stored
//!   record, and `GetFeed` returns that same record member for member — with the
//!   lifecycle status the accepted creation has since settled into, and the feed's
//!   inputs, which only a read reports;
//! * `ListFeeds` enumerates the persisted feeds in one stable order, at most
//!   `maxResults` per page (10 when the request omits it), resuming from `nextToken`;
//! * `UpdateFeed` overwrites the stored feed, so later reads report the new values,
//!   and answers 200, so the response reports the feed's settled status;
//! * `DeleteFeed` retires the feed, so once the deletion falls due `GetFeed` is a 404,
//!   `ListFeeds` no longer lists it and a repeat of the delete is a 404 as well
//!   (`tests/all/completed_deletion.rs`);
//! * an id no live feed has is `ResourceNotFoundException` (HTTP 404) on every feed
//!   operation that takes one;
//! * the feed store is BOUNDED: past the quota `CreateFeed` answers the modelled
//!   `ServiceQuotaExceededException` (HTTP 402), and `maxResults` outside the modelled
//!   range is refused, so no request can ask for an unbounded page;
//! * a feed whose deletion has been accepted cannot be updated (`ConflictException`).

use crate::harness::{
    call, create_feed, expected_data_endpoint, message_of,
    output_config, past_the_transition_span, router, REGION, SIGNING_NAME,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_FEEDS, PAGE_TOKEN_MALFORMED_MESSAGE, TRANSITION_SPAN,
};

/// Whether `id` satisfies the modelled `FeedId` pattern `^[a-z0-9]{19}$`.
fn is_feed_id(id: &str) -> bool {
    id.len() == 19
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// The ARN form the model gives a feed: `feed/<id>` under this service's namespace,
/// in the request's region and the emulator's account.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{}:feed/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `GetOutput` list a feed reports for outputs that entered it through CreateFeed
/// or UpdateFeed: the submitted outputs, each carrying `fromAssociation`.
fn expected_outputs(submitted: &Value) -> Value {
    Value::Array(
        submitted
            .as_array()
            .expect("the submitted outputs are a list")
            .iter()
            .map(|output| {
                let mut output = output.clone();
                let members = output.as_object_mut().expect("an output is an object");
                let from_association = members
                    .get("fromAssociation")
                    .cloned()
                    .unwrap_or(Value::Bool(false));
                members.insert("fromAssociation".to_string(), from_association);
                let config = members
                    .get("outputConfig")
                    .cloned()
                    .expect("an output carries its outputConfig");
                members.insert("outputConfig".to_string(), config);
                output
            })
            .collect(),
    )
}

/// One row of the create/read table: a feed request that differs from the others along
/// what the rule generalises over — output count, which `OutputConfig` member each
/// output carries, output status, whether a description, an access role or tags are
/// supplied, and the length of the name.
struct FeedCase {
    label: &'static str,
    body: Value,
}

fn create_cases() -> Vec<FeedCase> {
    vec![
        FeedCase {
            label: "one cropping output, no role, no tags",
            body: json!({
                "name": "feed-one",
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
                ]
            }),
        },
        FeedCase {
            label: "two outputs (clipping, subtitling) with a role and tags",
            body: json!({
                "name": "Feed_Two-2",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [
                    {"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED",
                     "description": "clips for the highlight reel"},
                    {"name": "subs1", "outputConfig": output_config("subtitling"), "status": "DISABLED"}
                ],
                "tags": {"team": "broadcast", "env": "test"}
            }),
        },
        FeedCase {
            label: "a single-character name and a contextualMetadata output",
            body: json!({
                "name": "a",
                "outputs": [
                    {"name": "meta1", "outputConfig": output_config("contextualMetadata"),
                     "status": "DISABLED", "description": "contextual metadata"}
                ]
            }),
        },
        FeedCase {
            label: "four outputs, one of every OutputConfig member",
            body: json!({
                "name": "feed-with-every-output-kind",
                "accessRoleArn": "arn:aws:iam::000000000000:role/every-output",
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                    {"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED"},
                    {"name": "subs1", "outputConfig": output_config("subtitling"), "status": "ENABLED"},
                    {"name": "meta1", "outputConfig": output_config("contextualMetadata"), "status": "ENABLED"}
                ],
                "tags": {"only": "one-tag"}
            }),
        },
    ]
}

/// CreateFeed mints and persists a feed, and GetFeed returns that stored record.
#[tokio::test]
async fn create_feed_persists_a_feed_that_get_feed_reports_member_for_member() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        let label = case.label;
        let (status, created) = call(&router, "POST", "/v1/feed", Some(case.body.clone())).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateFeed answers 202, got {status} with {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_feed_id(&id),
            "case {label:?}: the minted id matches ^[a-z0-9]{{19}}$, got {:?}",
            created["id"]
        );
        assert!(
            !minted.contains(&id),
            "case {label:?}: each create mints a fresh id, got the already-minted {id:?}"
        );
        minted.push(id.clone());

        assert_eq!(
            created["arn"],
            json!(expected_arn(&id)),
            "case {label:?}: the feed arn is the modelled form, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"], case.body["name"],
            "case {label:?}: the create response repeats the requested name, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {label:?}: the create is ACCEPTED, so its own answer reports CREATING, \
             got {:?}",
            created["status"]
        );
        let endpoints = created["dataEndpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            endpoints.len() == 1 && endpoints[0] == json!(expected_data_endpoint()),
            "case {label:?}: dataEndpoints is the endpoint this emulator is reached on, \
             got {:?}",
            created["dataEndpoints"]
        );
        assert_eq!(
            created["outputs"],
            expected_outputs(&case.body["outputs"]),
            "case {label:?}: the outputs are repeated with fromAssociation, got {:?}",
            created["outputs"]
        );
        assert_eq!(
            created.get("accessRoleArn"),
            case.body.get("accessRoleArn"),
            "case {label:?}: accessRoleArn is echoed only when supplied, got {:?}",
            created.get("accessRoleArn")
        );
        assert_eq!(
            created.get("tags"),
            Some(case.body.get("tags").unwrap_or(&json!({}))),
            "case {label:?}: the tag set is reported, empty when the request carried \
             none, got {:?}",
            created.get("tags")
        );
        assert_eq!(
            created.get("association"),
            None,
            "case {label:?}: a new feed has no association, got {:?}",
            created.get("association")
        );

        // The round trip: every member the accept reported comes back from storage
        // unchanged, except the lifecycle status — the accepted creation has settled, so
        // the read reports AVAILABLE — and the feed's inputs, which only a read reports.
        let (status, fetched) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetFeed on the created id answers 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["status"],
            json!("AVAILABLE"),
            "case {label:?}: the created feed has settled by the next request, got {:?}",
            fetched["status"]
        );
        for member in [
            "arn",
            "name",
            "id",
            "dataEndpoints",
            "outputs",
            "accessRoleArn",
            "association",
            "tags",
        ] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {label:?}: GetFeed reports the stored {member}, got {fetched}"
            );
        }
    }

    // Every feed created above is still there, and no other.
    let (status, listed) = call(&router, "GET", "/v1/feeds?maxResults=100", None).await;
    assert_eq!(status, 200, "ListFeeds answers 200, got {status}: {listed}");
    let mut listed_ids: Vec<String> = listed["feeds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|feed| feed["id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected_ids = minted.clone();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "ListFeeds enumerates exactly the persisted feeds, got {listed}"
    );
}

/// A created feed is USABLE as soon as CreateFeed answers: a `GetFeed` taken in the
/// very next request reports `AVAILABLE`, and it is still `AVAILABLE` later — nothing
/// about the feed is waiting on a clock.
#[tokio::test]
async fn a_created_feed_is_available_at_once_and_stays_available() {
    let router = router();
    let body = json!({
        "name": "settled-feed",
        "outputs": [{"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}]
    });
    let (_, created) = call(&router, "POST", "/v1/feed", Some(body)).await;
    let id = created["id"].as_str().unwrap().to_string();

    let (_, immediately) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        immediately["status"],
        json!("AVAILABLE"),
        "case {id:?}: a feed read straight after creation is AVAILABLE, got {:?}",
        immediately["status"]
    );

    past_the_transition_span();
    let (_, later) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        later["status"],
        json!("AVAILABLE"),
        "case {id:?}: and it is still AVAILABLE once any clock-owed move would have \
         fallen due, got {:?}",
        later["status"]
    );
    let (_, listed) = call(&router, "GET", "/v1/feeds", None).await;
    assert_eq!(
        listed["feeds"][0]["status"],
        json!("AVAILABLE"),
        "case {id:?}: ListFeeds reports that same status, got {listed}"
    );
}

/// ListFeeds pages over every persisted feed: at most `maxResults` per page (10 when
/// the request omits it), one stable order, and a `nextToken` only while more remain.
#[tokio::test]
async fn list_feeds_pages_over_every_persisted_feed() {
    // (feeds created, maxResults requested) — spanning no pagination, a page size of
    // one, a size that does not divide the count, the documented default of 10, and a
    // page larger than the whole collection.
    let cases: [(usize, Option<i32>); 6] = [
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (5, None),
        (12, None),
        (4, Some(100)),
    ];

    for (count, max_results) in cases {
        let label = format!("{count} feeds, maxResults={max_results:?}");
        let router = router();
        let mut created_ids: Vec<String> = Vec::new();
        for index in 0..count {
            let body = json!({
                "name": format!("feed-{index}"),
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
                ]
            });
            let (status, created) = call(&router, "POST", "/v1/feed", Some(body)).await;
            assert_eq!(status, 202, "case {label:?}: CreateFeed answers 202, got {status}");
            created_ids.push(created["id"].as_str().unwrap().to_string());
        }

        // 10 is the page size the model's `maxResults` documentation gives for a
        // request that omits it.
        let page_size = max_results.unwrap_or(10) as usize;
        let expected_pages = count.div_ceil(page_size);

        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut next_token: Option<String> = None;
        loop {
            let mut uri = "/v1/feeds".to_string();
            let mut query: Vec<String> = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            if !query.is_empty() {
                uri.push('?');
                uri.push_str(&query.join("&"));
            }

            let (status, page) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: ListFeeds answers 200, got {status} with {page}"
            );
            pages += 1;
            assert!(
                pages <= expected_pages,
                "case {label:?}: pagination terminates in {expected_pages} pages, got page {pages}: {page}"
            );

            let feeds = page["feeds"].as_array().cloned().unwrap_or_default();
            assert!(
                feeds.len() <= page_size,
                "case {label:?}: a page holds at most {page_size} feeds, got {}",
                feeds.len()
            );
            for feed in &feeds {
                let id = feed["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_feed_id(&id),
                    "case {label:?}: a summary carries a modelled feed id, got {feed}"
                );
                assert_eq!(
                    feed["arn"],
                    json!(expected_arn(&id)),
                    "case {label:?}: a summary carries the feed's arn, got {feed}"
                );
                assert!(
                    feed["name"].is_string() && feed["status"].is_string(),
                    "case {label:?}: a summary carries the feed's name and status, got {feed}"
                );
                seen.push(id);
            }

            next_token = page
                .get("nextToken")
                .and_then(|token| token.as_str())
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            pages, expected_pages,
            "case {label:?}: {count} feeds at {page_size} per page is {expected_pages} pages, got {pages}"
        );
        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        sorted_seen.dedup();
        assert_eq!(
            sorted_seen.len(),
            seen.len(),
            "case {label:?}: no feed is returned twice across pages, got {seen:?}"
        );
        let mut expected = created_ids.clone();
        expected.sort();
        assert_eq!(
            sorted_seen, expected,
            "case {label:?}: paging through every page yields every persisted feed, got {seen:?}"
        );
        let mut in_order = seen.clone();
        in_order.sort();
        assert_eq!(
            seen, in_order,
            "case {label:?}: pages follow one stable order, got {seen:?}"
        );
    }
}

/// UpdateFeed overwrites the stored feed, so later reads report the new values.
#[tokio::test]
async fn update_feed_overwrites_the_stored_feed() {
    // Each row changes the feed along a different axis: fewer outputs, more outputs,
    // a different config member, and an access role that the PUT payload drops.
    // `fromAssociation` is not among the axes: it is service-owned, so what an update
    // may do with it is asserted in `output_provenance`.
    let rows: Vec<(&str, Value)> = vec![
        (
            "same output count, new name",
            json!({
                "name": "renamed-feed",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "DISABLED"}
                ]
            }),
        ),
        (
            "grown to three outputs",
            json!({
                "name": "grown-feed",
                "accessRoleArn": "arn:aws:iam::000000000000:role/elemental-inference-feed",
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                    {"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED",
                     "description": "clips"},
                    {"name": "subs1", "outputConfig": output_config("subtitling"), "status": "DISABLED"}
                ]
            }),
        ),
        (
            "a different config member, and the access role dropped",
            json!({
                "name": "feed-without-a-role",
                "outputs": [
                    {"name": "meta1", "outputConfig": output_config("contextualMetadata"),
                     "status": "ENABLED"}
                ]
            }),
        ),
    ];

    for (label, update) in rows {
        let router = router();
        let (_, created) = call(
            &router,
            "POST",
            "/v1/feed",
            Some(json!({
                "name": "original-feed",
                "accessRoleArn": "arn:aws:iam::000000000000:role/original",
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"},
                    {"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED"}
                ],
                "tags": {"env": "test"}
            })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        // Let the creation fall due first, so what the update reports is about the
        // UPDATE and not about a creation the feed was still waiting out.
        past_the_transition_span();

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(update.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["name"], update["name"],
            "case {label:?}: the update response carries the new name, got {:?}",
            updated["name"]
        );
        assert_eq!(
            updated["id"],
            json!(id),
            "case {label:?}: the feed keeps its id, got {:?}",
            updated["id"]
        );
        assert_eq!(
            updated["arn"], created["arn"],
            "case {label:?}: the feed keeps its arn, got {:?}",
            updated["arn"]
        );
        assert_eq!(
            updated["status"],
            json!("AVAILABLE"),
            "case {label:?}: the update is COMPLETE when it answers 200, so it reports the \
             feed's settled status, got {:?}",
            updated["status"]
        );
        assert_eq!(
            updated.get("tags"),
            created.get("tags"),
            "case {label:?}: UpdateFeed takes no tags, so the feed keeps its own, got {:?}",
            updated.get("tags")
        );

        // The mutation is in STORAGE, not just in the response.
        let (status, fetched) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetFeed after the update answers 200, got {status}"
        );
        assert_eq!(
            fetched["name"], update["name"],
            "case {label:?}: GetFeed reports the new name, got {:?}",
            fetched["name"]
        );
        assert_eq!(
            fetched["outputs"],
            expected_outputs(&update["outputs"]),
            "case {label:?}: GetFeed reports the new outputs, got {:?}",
            fetched["outputs"]
        );
        assert_eq!(
            fetched.get("accessRoleArn"),
            update.get("accessRoleArn"),
            "case {label:?}: a PUT payload overwrites the access role, got {:?}",
            fetched.get("accessRoleArn")
        );
        let (_, listed) = call(&router, "GET", "/v1/feeds", None).await;
        assert_eq!(
            listed["feeds"][0]["name"], update["name"],
            "case {label:?}: ListFeeds reports the new name, got {listed}"
        );

        past_the_transition_span();
        let (_, settled) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            settled["status"],
            json!("AVAILABLE"),
            "case {label:?}: the reconfigured feed settles back into AVAILABLE, got {:?}",
            settled["status"]
        );
    }
}

/// DeleteFeed retires the feed: it reports DELETING, and once the deletion falls due
/// the feed is gone from GetFeed and from ListFeeds.
#[tokio::test]
async fn delete_feed_retires_the_feed_from_state() {
    // Deleting one of several feeds must leave the others alone, so the table spans
    // which feed of how many is deleted.
    for (count, target) in [(1usize, 0usize), (3, 0), (3, 2)] {
        let label = format!("deleting feed {target} of {count}");
        let router = router();
        let mut ids: Vec<String> = Vec::new();
        for index in 0..count {
            let body = json!({
                "name": format!("feed-{index}"),
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
                ]
            });
            let (_, created) = call(&router, "POST", "/v1/feed", Some(body)).await;
            ids.push(created["id"].as_str().unwrap().to_string());
        }
        let id = ids[target].clone();

        let (status, deleted) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {label:?}: DeleteFeed answers 202, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["id"],
            json!(id),
            "case {label:?}: the delete response names the feed, got {deleted}"
        );
        assert_eq!(
            deleted["arn"],
            json!(expected_arn(&id)),
            "case {label:?}: the delete response carries the feed arn, got {deleted}"
        );
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {label:?}: a deletion accepted with 202 reports DELETING, got {deleted}"
        );

        // Inside the window the feed is still reported, in its transient status.
        let (status, deleting) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a feed whose deletion is still pending is readable, got {status}"
        );
        assert_eq!(
            deleting["status"],
            json!("DELETING"),
            "case {label:?}: that read reports DELETING, got {deleting}"
        );

        past_the_transition_span();
        let (status, gone) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: once the deletion is due GetFeed is a 404, got {status} with {gone}"
        );
        assert_eq!(
            gone["__type"],
            json!("ResourceNotFoundException"),
            "case {label:?}: that 404 is a ResourceNotFoundException, got {gone}"
        );

        let (_, listed) = call(&router, "GET", "/v1/feeds?maxResults=100", None).await;
        let remaining: Vec<String> = listed["feeds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|feed| feed["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut expected: Vec<String> = ids.iter().filter(|other| **other != id).cloned().collect();
        expected.sort();
        let mut sorted_remaining = remaining.clone();
        sorted_remaining.sort();
        assert_eq!(
            sorted_remaining, expected,
            "case {label:?}: ListFeeds drops the deleted feed and keeps the rest, got {remaining:?}"
        );
    }
}

/// Repeating DeleteFeed inside the DELETING window is idempotent: it re-reports
/// DELETING and leaves the removal due at the instant the first call set, so a client
/// that polls the delete cannot keep the feed alive.
#[tokio::test]
async fn a_repeated_delete_does_not_postpone_the_deletion() {
    let router = router();
    let (_, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "polled-feed",
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    let (status, first) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 202,
        "case {id:?}: the first DeleteFeed answers 202, got {status} with {first}"
    );

    // Poll part-way through the window, then wait out the REST of the original span:
    // each sleep is shorter than the span, so the feed may only be gone if the
    // repeated call left the original due instant alone.
    let half = TRANSITION_SPAN / 2 + std::time::Duration::from_millis(50);
    std::thread::sleep(half);
    let (status, repeat) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 202,
        "case {id:?}: the repeated DeleteFeed answers 202, got {status} with {repeat}"
    );
    assert_eq!(
        repeat["status"],
        json!("DELETING"),
        "case {id:?}: the repeated DeleteFeed re-reports DELETING, got {repeat}"
    );
    std::thread::sleep(half);

    let (status, gone) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {id:?}: polling the delete did not postpone the removal, got {status} with {gone}"
    );
}

/// An id no live feed has is a 404 `ResourceNotFoundException` on every feed operation
/// that takes one — never a 501 and never a 200.
#[tokio::test]
async fn an_unknown_feed_id_is_a_404_on_every_feed_operation() {
    let router = router();
    // Create one feed, so the 404 is about the id and not about an empty store.
    let (_, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "the-only-feed",
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        })),
    )
    .await;
    let live_id = created["id"].as_str().unwrap().to_string();

    // Every id below satisfies ^[a-z0-9]{19}$, so the boundary admits it and the
    // handler is what refuses it.
    let unknown_ids = [
        "0000000000000000000",
        "zzzzzzzzzzzzzzzzzzz",
        "abcdefghij123456789",
    ];
    let update = json!({
        "name": "whatever",
        "outputs": [
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]
    });

    for id in unknown_ids {
        assert_ne!(id, live_id, "the unknown ids must not be the live feed's");
        for (method, body) in [
            ("GET", None),
            ("PUT", Some(update.clone())),
            ("DELETE", None),
        ] {
            let label = format!("{method} /v1/feed/{id}");
            let (status, error) = call(&router, method, &format!("/v1/feed/{id}"), body).await;
            assert_eq!(
                status, 404,
                "case {label:?}: an unknown feed id is a 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
            );
        }
    }

    // And the live feed is untouched by all of that.
    let (status, fetched) = call(&router, "GET", &format!("/v1/feed/{live_id}"), None).await;
    assert_eq!(
        status, 200,
        "the live feed is still there, got {status} with {fetched}"
    );
}

/// A `nextToken` no `ListFeeds` response ever carried cannot name a page, so the
/// handler refuses it as a 400 `ValidationException` rather than silently serving a
/// page from it; a token the service DID issue keeps paging from its position.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let router = router();
    // Not hex at all, hex digits that do not pair up, and hex that decodes to text
    // carrying none of a token's fields — the hex of a bare id, which is what a caller
    // forging a cursor would reach for.
    let hex_of_an_id = hex::encode("0".repeat(19));
    for token in ["not-a-token", "zz", "0", hex_of_an_id.as_str()] {
        let label = format!("nextToken={token}");
        let (status, error) = call(
            &router,
            "GET",
            &format!("/v1/feeds?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a nextToken this service never issued is a 400, got {status} \
             with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            message_of(&error),
            PAGE_TOKEN_MALFORMED_MESSAGE,
            "case {label:?}: expected the invariant sentence for a token that is none of \
             this service's, got {error}"
        );
    }

    // A token the service issued names a POSITION in the id order, not a feed that must
    // still exist: the feed it would resume at is deleted, and paging continues at the
    // next one.
    let mut ids = [
        create_feed(&router, "first-feed").await,
        create_feed(&router, "second-feed").await,
        create_feed(&router, "third-feed").await,
    ];
    ids.sort();
    let (status, first_page) = call(&router, "GET", "/v1/feeds?maxResults=1", None).await;
    assert_eq!(
        status, 200,
        "the first page answers 200, got {status} with {first_page}"
    );
    assert_eq!(
        first_page["feeds"][0]["id"],
        json!(ids[0]),
        "the first page of one reports the first id in order, got {first_page}"
    );
    let token = first_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("two feeds remain, so a nextToken comes back: {first_page}"))
        .to_string();

    let (status, deleted) = call(&router, "DELETE", &format!("/v1/feed/{}", ids[1]), None).await;
    assert_eq!(
        status, 202,
        "DeleteFeed answers 202, got {status} with {deleted}"
    );
    past_the_transition_span();

    let (status, page) = call(
        &router,
        "GET",
        &format!("/v1/feeds?nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {token:?}: a token this listing issued is served, got {status} with {page}"
    );
    assert_eq!(
        page["feeds"][0]["id"],
        json!(ids[2]),
        "case {token:?}: paging resumes at the next id in order, past the deleted one, \
         got {page}"
    );
}

/// The feed store is bounded: past the quota CreateFeed refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) instead of growing state further, and
/// the feeds already stored keep working.
#[tokio::test]
async fn create_feed_refuses_past_the_feed_quota() {
    let router = router();
    let feed = |index: usize| {
        json!({
            "name": format!("feed-{index}"),
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        })
    };

    let mut last_id = String::new();
    for index in 0..MAX_FEEDS {
        let (status, created) = call(&router, "POST", "/v1/feed", Some(feed(index))).await;
        assert_eq!(
            status, 202,
            "case feed {index} of {MAX_FEEDS}: CreateFeed answers 202 up to the quota, got {status} with {created}"
        );
        last_id = created["id"].as_str().unwrap().to_string();
    }

    let (status, refused) = call(&router, "POST", "/v1/feed", Some(feed(MAX_FEEDS))).await;
    assert_eq!(
        status, 402,
        "case feed {} of {MAX_FEEDS}: the create past the quota is a 402, got {status} with {refused}",
        MAX_FEEDS + 1
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case past the quota: that 402 is a ServiceQuotaExceededException, got {refused}"
    );

    // The refusal did not disturb what is stored.
    let (status, fetched) = call(&router, "GET", &format!("/v1/feed/{last_id}"), None).await;
    assert_eq!(
        status, 200,
        "case past the quota: the stored feeds still read back, got {status} with {fetched}"
    );
}

/// A `maxResults` outside the modelled range never reaches the handler: the boundary
/// refuses it, so no request can ask for an unbounded page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    // `smithy_query(query="operation",operation="ListFeeds")` gives maxResults the
    // range {min: 1, max: 100}: each published bound is accepted, one past it is not.
    for (max_results, expected) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let label = format!("maxResults={max_results}");
        let (status, body) = call(
            &router,
            "GET",
            &format!("/v1/feeds?maxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {body}"
        );
    }
}

/// A feed whose deletion has been accepted cannot be updated: that would cancel a
/// deletion the clock already owes, so UpdateFeed answers the modelled
/// `ConflictException` (HTTP 409).
#[tokio::test]
async fn update_feed_refuses_a_feed_that_is_being_deleted() {
    let router = router();
    let (_, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "doomed-feed",
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();

    let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(status, 202, "case {id:?}: DeleteFeed answers 202, got {status}");

    let (status, error) = call(
        &router,
        "PUT",
        &format!("/v1/feed/{id}"),
        Some(json!({
            "name": "revived-feed",
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {id:?}: updating a DELETING feed is a 409, got {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ConflictException"),
        "case {id:?}: that 409 is a ConflictException, got {error}"
    );

    // And the deletion still completes on the clock.
    past_the_transition_span();
    let (status, gone) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {id:?}: the refused update did not cancel the deletion, got {status} with {gone}"
    );
}
