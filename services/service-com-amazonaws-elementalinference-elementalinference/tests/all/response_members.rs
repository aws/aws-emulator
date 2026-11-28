// SPDX-License-Identifier: CC0-1.0

//! What a feed and a dictionary response CARRY, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from
//! the SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * a map member the resource HAS is serialized even when it is empty: an untagged
//!   feed or dictionary reports `"tags": {}`, never an absent member, so the member
//!   does not start existing the moment the first tag is added;
//! * every `dataEndpoints` entry is an absolute URL rooted at the endpoint this
//!   emulator is REACHED on, with the root path and the trailing slash a client
//!   concatenates a media path onto — the address a caller holding the response can
//!   actually send media to and read metadata from;
//! * an optional output-config member the service resolves itself is MATERIALISED on
//!   the stored output, so every later read reports the configuration the service
//!   applied and a `GetFeed` -> `UpdateFeed` round trip cannot drop it (the audio
//!   binding every output config carries is one of them);
//! * a minted id identifies exactly ONE resource: two creates that differ in any
//!   member never report the same id.

use crate::harness::{
    call, expected_data_endpoint, output_config, past_the_transition_span, router,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The aspect ratio a subtitling output created without one comes back with.
///
/// `SubtitlingConfig$aspectRatio` is optional and the model declares no default
/// (`smithy_query(query="shape",shape="SubtitlingConfig")`), so this is the
/// service-chosen geometry a created subtitling output is observed to report: 16:9,
/// the standard broadcast frame.
fn default_aspect_ratio() -> Value {
    json!({"width": 16, "height": 9})
}

/// A `CreateFeed` body carrying `outputs` and, when some are given, `tags`.
fn feed_body(name: &str, outputs: Value, tags: Option<Value>) -> Value {
    let mut body = json!({"name": name, "outputs": outputs});
    if let Some(tags) = tags {
        body.as_object_mut()
            .expect("the body is an object")
            .insert("tags".to_string(), tags);
    }
    body
}

/// Create one feed over the wire and return (id, create response).
async fn create_feed(router: &MultiServiceRouter, body: Value, label: &str) -> (String, Value) {
    let (status, created) = call(router, "POST", "/v1/feed", Some(body)).await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreateFeed answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the created feed carries an id: {created}"))
        .to_string();
    (id, created)
}

/// `GET /v1/feed/{id}`, asserting it is served.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

// ---------------------------------------------------------------------------
// An empty map is a map: `tags` is reported, not omitted.
// ---------------------------------------------------------------------------

/// The tag sets the table below spans: none at all, the empty map spelled out, one
/// tag, and several.
fn tag_cases() -> Vec<(&'static str, Option<Value>, Value)> {
    vec![
        ("no tags member at all", None, json!({})),
        ("an explicitly empty tag map", Some(json!({})), json!({})),
        (
            "one tag",
            Some(json!({"team": "broadcast"})),
            json!({"team": "broadcast"}),
        ),
        (
            "three tags",
            Some(json!({"team": "broadcast", "env": "test", "owner": "ops"})),
            json!({"team": "broadcast", "env": "test", "owner": "ops"}),
        ),
    ]
}

/// Every feed response carries the feed's tag set, empty map included.
#[tokio::test]
async fn feed_responses_carry_an_empty_tag_map_rather_than_omitting_tags() {
    for (label, submitted, expected) in tag_cases() {
        let router = router();
        let outputs = json!([
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]);
        let (id, created) = create_feed(
            &router,
            feed_body("tagged-feed", outputs.clone(), submitted.clone()),
            label,
        )
        .await;
        assert_eq!(
            created["tags"], expected,
            "case {label:?}: CreateFeed reports the tag set, got {:?}",
            created["tags"]
        );

        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["tags"], expected,
            "case {label:?}: GetFeed reports the stored tag set, got {:?}",
            fetched["tags"]
        );

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({"name": "tagged-feed", "outputs": outputs})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["tags"], expected,
            "case {label:?}: UpdateFeed reports the tag set it did not change, got {:?}",
            updated["tags"]
        );
    }
}

/// Every dictionary response carries the dictionary's tag set, empty map included.
#[tokio::test]
async fn dictionary_responses_carry_an_empty_tag_map_rather_than_omitting_tags() {
    for (label, submitted, expected) in tag_cases() {
        let router = router();
        let mut body = json!({"name": "tagged-dictionary", "language": "eng"});
        if let Some(tags) = submitted {
            body.as_object_mut().unwrap().insert("tags".into(), tags);
        }
        let (status, created) = call(&router, "POST", "/v1/dictionary", Some(body)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateDictionary answers 202, got {status} with {created}"
        );
        assert_eq!(
            created["tags"], expected,
            "case {label:?}: CreateDictionary reports the tag set, got {:?}",
            created["tags"]
        );

        let id = created["id"].as_str().expect("a minted dictionary id");
        let (status, fetched) =
            call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary answers 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["tags"], expected,
            "case {label:?}: GetDictionary reports the stored tag set, got {:?}",
            fetched["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// The data-plane endpoint a feed advertises.
// ---------------------------------------------------------------------------

/// Every `dataEndpoints` entry is an absolute URL rooted at the endpoint this emulator
/// is reached on, whose path is the root and which ends in the slash a client
/// concatenates a media path onto — reported alike by the create, the read and the
/// update.
#[tokio::test]
async fn data_endpoints_are_rooted_at_the_endpoint_the_emulator_is_reached_on() {
    let router = router();
    // The rows differ along what the rule generalises over: how many outputs the feed
    // carries, which features those outputs are for and how long its name is — the
    // endpoint depends on none of them, because it is a property of where this process
    // serves and not of the record.
    let rows: [(&str, &str, Value); 3] = [
        (
            "one cropping output",
            "endpoint-feed-one",
            json!([{"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}]),
        ),
        (
            "a single-character name",
            "e",
            json!([{"name": "clip1", "outputConfig": output_config("clipping"), "status": "ENABLED"}]),
        ),
        (
            "two outputs",
            "endpoint_feed-three",
            json!([
                {"name": "subs1", "outputConfig": output_config("subtitling"), "status": "ENABLED"},
                {"name": "meta1", "outputConfig": output_config("contextualMetadata"), "status": "DISABLED"}
            ]),
        ),
    ];

    let expected = json!([expected_data_endpoint()]);
    for (label, name, outputs) in rows {
        let (id, created) = create_feed(&router, feed_body(name, outputs.clone(), None), label).await;
        assert_eq!(
            created["dataEndpoints"], expected,
            "case {label:?}: CreateFeed advertises the endpoint this emulator is reached \
             on, got {:?}",
            created["dataEndpoints"]
        );
        let endpoint = created["dataEndpoints"][0]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the entry is a string"));
        assert!(
            endpoint.ends_with('/') && endpoint.starts_with("https://"),
            "case {label:?}: the endpoint is an absolute URL ending in the slash a client \
             concatenates a path onto, got {endpoint:?}"
        );
        assert_eq!(
            endpoint.trim_end_matches('/').matches('/').count(),
            2,
            "case {label:?}: the endpoint's path is the root, so the only slashes are the \
             scheme's and the trailing one, got {endpoint:?}"
        );
        assert!(
            !endpoint.contains("amazonaws.com") && !endpoint.contains("api.aws"),
            "case {label:?}: the endpoint names no host of AWS's own, got {endpoint:?}"
        );

        // Stable for the life of the feed: a read and an update report the very same
        // endpoint, so a client that re-reads the feed after reconfiguring it is handed
        // the address it was already using.
        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["dataEndpoints"], expected,
            "case {label:?}: GetFeed reports the same endpoint, got {:?}",
            fetched["dataEndpoints"]
        );
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({"name": name, "outputs": outputs})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["dataEndpoints"], expected,
            "case {label:?}: UpdateFeed echoes the same endpoint, got {:?}",
            updated["dataEndpoints"]
        );
    }
}

// ---------------------------------------------------------------------------
// Output-config defaults are materialised on the stored output.
// ---------------------------------------------------------------------------

/// One row of the output-config table: the config a request carries and the config
/// the service is expected to report back for it.
struct ConfigCase {
    label: &'static str,
    submitted: Value,
    expected: Value,
}

/// The rows span every member of the `OutputConfig` union
/// (`smithy_query(query="shape",shape="OutputConfig")`: cropping, clipping,
/// subtitling, contextualMetadata, function) that carries a service-resolved member, in
/// both the omitted and the supplied case.
fn config_cases() -> Vec<ConfigCase> {
    vec![
        ConfigCase {
            label: "subtitling without an aspect ratio or a profanity filter",
            submitted: json!({"subtitling": {"language": "eng"}}),
            expected: json!({"subtitling": {
                "language": "eng",
                "aspectRatio": default_aspect_ratio(),
                "profanityFilter": "DISABLED"
            }}),
        },
        ConfigCase {
            label: "subtitling with a supplied 4:3 aspect ratio and no profanity filter",
            submitted: json!({"subtitling": {
                "language": "fra",
                "aspectRatio": {"width": 4, "height": 3}
            }}),
            expected: json!({"subtitling": {
                "language": "fra",
                "aspectRatio": {"width": 4, "height": 3},
                "profanityFilter": "DISABLED"
            }}),
        },
        ConfigCase {
            label: "subtitling with a supplied 1:1 aspect ratio and a profanity filter",
            submitted: json!({"subtitling": {
                "language": "spa",
                "aspectRatio": {"width": 1, "height": 1},
                "profanityFilter": "DROP"
            }}),
            expected: json!({"subtitling": {
                "language": "spa",
                "aspectRatio": {"width": 1, "height": 1},
                "profanityFilter": "DROP"
            }}),
        },
        ConfigCase {
            label: "cropping, which models no service-resolved member",
            submitted: output_config("cropping"),
            expected: output_config("cropping"),
        },
        ConfigCase {
            label: "clipping, which models no service-resolved member",
            submitted: output_config("clipping"),
            expected: output_config("clipping"),
        },
        ConfigCase {
            label: "contextualMetadata with both modes supplied",
            submitted: output_config("contextualMetadata"),
            expected: output_config("contextualMetadata"),
        },
        ConfigCase {
            label: "contextualMetadata stating neither of its modes",
            submitted: json!({"contextualMetadata": {}}),
            expected: json!({"contextualMetadata": {
                "summaryGeneration": "ENABLED",
                "extendedAnalysis": "ENABLED"
            }}),
        },
    ]
}

/// CreateFeed stores the output with the service's defaults applied, so the create
/// response AND every later read report them.
#[tokio::test]
async fn create_feed_materialises_output_config_defaults_in_storage() {
    for case in config_cases() {
        let label = case.label;
        let router = router();
        let outputs = json!([
            {"name": "out1", "outputConfig": case.submitted, "status": "ENABLED"}
        ]);
        let (id, created) = create_feed(&router, feed_body("default-feed", outputs, None), label)
            .await;
        assert_eq!(
            created["outputs"][0]["outputConfig"], case.expected,
            "case {label:?}: CreateFeed reports the configured output, got {:?}",
            created["outputs"][0]["outputConfig"]
        );

        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["outputs"][0]["outputConfig"], case.expected,
            "case {label:?}: GetFeed reports the stored output config, got {:?}",
            fetched["outputs"][0]["outputConfig"]
        );
    }
}

/// UpdateFeed (a full-replacement PUT) and AssociateFeed (an append) store the same
/// defaults, so a read-modify-write round trip cannot drop them.
#[tokio::test]
async fn update_and_associate_materialise_output_config_defaults_too() {
    for case in config_cases() {
        let label = case.label;
        let router = router();
        let (id, _) = create_feed(
            &router,
            feed_body(
                "round-trip-feed",
                json!([
                    {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
                ]),
                None,
            ),
            label,
        )
        .await;

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/v1/feed/{id}"),
            Some(json!({
                "name": "round-trip-feed",
                "outputs": [{"name": "out1", "outputConfig": case.submitted, "status": "ENABLED"}]
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateFeed answers 200, got {status} with {updated}"
        );
        assert_eq!(
            updated["outputs"][0]["outputConfig"], case.expected,
            "case {label:?}: UpdateFeed reports the configured output, got {:?}",
            updated["outputs"][0]["outputConfig"]
        );
        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["outputs"][0]["outputConfig"], case.expected,
            "case {label:?}: GetFeed after the update reports the stored config, got {:?}",
            fetched["outputs"][0]["outputConfig"]
        );

        // AssociateFeed appends to a settled feed, so wait for the update to fall due.
        past_the_transition_span();
        let (status, associated) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/associate"),
            Some(json!({
                "associatedResourceName": "source-media",
                "outputs": [{"name": "out2", "outputConfig": case.submitted, "status": "ENABLED"}]
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: AssociateFeed answers 200, got {status} with {associated}"
        );
        let fetched = get_feed(&router, &id, label).await;
        assert_eq!(
            fetched["outputs"][1]["outputConfig"], case.expected,
            "case {label:?}: the appended output is stored with the defaults applied, \
             got {:?}",
            fetched["outputs"][1]["outputConfig"]
        );
    }
}

// ---------------------------------------------------------------------------
// A minted id identifies exactly one resource.
// ---------------------------------------------------------------------------

/// Repeated creates mint DISTINCT ids whenever the requests differ, for feeds and for
/// dictionaries alike: an id addresses one resource, so a second create may never
/// alias the first. (Two requests that are identical in every member are ONE create
/// replayed — both operations are `@idempotent` — and that rule is asserted in
/// `tests/all/create_replay_is_idempotent.rs`.)
#[tokio::test]
async fn repeated_creates_mint_distinct_ids() {
    let router = router();
    // Several creates per resource kind: two whose requests differ only in the name,
    // and two that share a name and differ only in another member.
    let feed_rows = [
        ("feed-a", "crop1"),
        ("feed-b", "crop1"),
        ("feed-b", "crop2"),
    ];
    let feed_labels: Vec<String> = feed_rows
        .iter()
        .map(|(name, output)| format!("{name}/{output}"))
        .collect();
    let mut feed_ids: Vec<String> = Vec::new();
    for ((name, output), label) in feed_rows.iter().zip(&feed_labels) {
        let outputs = json!([
            {"name": output, "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]);
        let (id, _) = create_feed(&router, feed_body(name, outputs, None), label).await;
        assert!(
            !feed_ids.contains(&id),
            "case {label:?}: each CreateFeed mints a fresh id, got the already-minted {id:?}"
        );
        feed_ids.push(id);
    }

    let dictionary_rows = [("dict-a", "eng"), ("dict-b", "eng"), ("dict-b", "fra")];
    let dictionary_labels: Vec<String> = dictionary_rows
        .iter()
        .map(|(name, language)| format!("{name}/{language}"))
        .collect();
    let mut dictionary_ids: Vec<String> = Vec::new();
    for ((name, language), label) in dictionary_rows.iter().zip(&dictionary_labels) {
        let (status, created) = call(
            &router,
            "POST",
            "/v1/dictionary",
            Some(json!({"name": name, "language": language})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateDictionary answers 202, got {status} with {created}"
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a minted dictionary id: {created}"))
            .to_string();
        assert!(
            !dictionary_ids.contains(&id),
            "case {label:?}: each CreateDictionary mints a fresh id, got the already-minted \
             {id:?}"
        );
        dictionary_ids.push(id);
    }

    // The resources are addressable apart: each id reads back the resource it names.
    for (id, label) in feed_ids.iter().zip(&feed_labels) {
        let feed = get_feed(&router, id, label).await;
        assert_eq!(
            feed["id"],
            json!(id),
            "case {label:?}: the feed read by id reports that id, got {:?}",
            feed["id"]
        );
    }
    for (id, label) in dictionary_ids.iter().zip(&dictionary_labels) {
        let (status, dictionary) =
            call(&router, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetDictionary on the minted id answers 200, got {status}"
        );
        assert_eq!(
            dictionary["id"],
            json!(id),
            "case {label:?}: the dictionary read by id reports that id, got {:?}",
            dictionary["id"]
        );
    }
}

/// An id is unique across STORES as well as within one: the first feed and the first
/// dictionary of a fresh emulator do not carry the id a previous one handed out.
#[tokio::test]
async fn a_fresh_store_does_not_reissue_the_ids_of_another() {
    // Several independent stores, so the assertion is over a set rather than a single
    // pair, each asked for the FIRST id it mints — the one a per-store counter alone
    // would repeat.
    let mut feed_ids: Vec<String> = Vec::new();
    let mut dictionary_ids: Vec<String> = Vec::new();
    for store in 0..4 {
        let label = format!("store {store}");
        let router = router();
        let outputs = json!([
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]);
        let (feed_id, _) = create_feed(&router, feed_body("first-feed", outputs, None), &label).await;
        assert!(
            !feed_ids.contains(&feed_id),
            "case {label:?}: the first feed id of a fresh store is new, got the \
             already-issued {feed_id:?}"
        );
        feed_ids.push(feed_id);

        let (status, created) = call(
            &router,
            "POST",
            "/v1/dictionary",
            Some(json!({"name": "first-dictionary", "language": "eng"})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: CreateDictionary answers 202, got {status} with {created}"
        );
        let dictionary_id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a minted dictionary id: {created}"))
            .to_string();
        assert!(
            !dictionary_ids.contains(&dictionary_id),
            "case {label:?}: the first dictionary id of a fresh store is new, got the \
             already-issued {dictionary_id:?}"
        );
        dictionary_ids.push(dictionary_id);
    }
}
