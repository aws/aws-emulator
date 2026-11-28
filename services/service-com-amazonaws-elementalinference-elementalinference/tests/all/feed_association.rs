// SPDX-License-Identifier: CC0-1.0

//! The feed ASSOCIATION, end to end over the wire: `AssociateFeed` and
//! `DisassociateFeed`, read back through `GetFeed` and `ListFeeds`.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! case:
//!
//! * `AssociateFeed` CLAIMS a feed for one resource and APPENDS the request's outputs to
//!   the stored ones ("New outputs will be appended"), each appended output reporting
//!   `fromAssociation: true`; the claimed feed reports `ACTIVE` and its association;
//! * a feed takes an association only from a status that admits one — `AVAILABLE`, or
//!   `ACTIVE` for the resource that already holds it; from any other status, and for a
//!   second, different resource ("you can associate only one resource with each feed"),
//!   the call is refused with the modelled `ValidationException` carrying that rule's
//!   invariant sentence, and the stored feed is left exactly as it was (how a refusal is
//!   WORDED is `error_message_rule.rs`);
//! * an output that would MODIFY an existing one (same name) is a `ConflictException`,
//!   and a feed's output list is BOUNDED: an append past the quota answers the modelled
//!   `ServiceQuotaExceededException`;
//! * `associatedResourceName` "must not resemble an ARN", a rule its own pattern cannot
//!   express — and one judged over the whole value, including the non-ASCII word
//!   characters that pattern admits;
//! * `DisassociateFeed` RELEASES the named resource, disables every output and archives
//!   the feed, irreversibly: an `ARCHIVED` feed takes no further association and no other
//!   resource can be released from it (a RETRY of the release that was served is
//!   `feed_release_retry.rs`); a name that is not the associated resource's is refused;
//! * `dryRun` on either operation validates and persists NOTHING, so a later read
//!   reports the feed unchanged;
//! * an id no live feed has is `ResourceNotFoundException` (HTTP 404) on both.

use crate::harness::{
    call, output, output_config, past_the_transition_span, router, REGION, SIGNING_NAME,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    feed_not_associated_message, FeedStatus, FEED_ALREADY_ASSOCIATED_MESSAGE,
    FEED_NOT_ASSOCIABLE_MESSAGE, MAX_OUTPUTS_PER_FEED,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Create a feed carrying `outputs` and return its id. A created feed is settled and
/// `AVAILABLE` at once, so the caller can associate it in the very next request.
async fn create_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": name, "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreateFeed answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// Create a feed and assert it is `AVAILABLE` — the only status from which a
/// never-associated feed takes an association, and the one a create leaves it in.
async fn available_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let id = create_feed(router, name, outputs).await;
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        (status, &feed["status"]),
        (200, &json!("AVAILABLE")),
        "case {name:?}: a created feed is AVAILABLE without any wait, got {feed}"
    );
    id
}

/// `POST /v1/feed/{id}/associate` with `associatedResourceName` and `outputs`.
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    outputs: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({"associatedResourceName": resource, "outputs": outputs})),
    )
    .await
}

/// `POST /v1/feed/{id}/disassociate` with `associatedResourceName`.
async fn disassociate(router: &MultiServiceRouter, id: &str, resource: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/disassociate"),
        Some(json!({"associatedResourceName": resource})),
    )
    .await
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

/// The ARN form the model gives a feed: `feed/<id>` under this service's namespace,
/// in the request's region and the emulator's account.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{}:feed/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `GetOutput` projection of `submitted`, with `fromAssociation` set to
/// `from_association` (`GetOutput$fromAssociation`: true only for an output that
/// entered the feed through `AssociateFeed`).
fn expected_outputs(submitted: &Value, from_association: bool) -> Vec<Value> {
    submitted
        .as_array()
        .expect("the submitted outputs are a list")
        .iter()
        .map(|output| {
            let mut output = output.clone();
            let members = output.as_object_mut().expect("an output is an object");
            members.insert("fromAssociation".to_string(), json!(from_association));
            let config = members
                .get("outputConfig")
                .cloned()
                .expect("an output carries its outputConfig");
            members.insert("outputConfig".to_string(), config);
            output
        })
        .collect()
}

/// One row of the association table: a feed's own outputs, the resource that claims it
/// and the outputs that claim appends. The rows differ along what the rule generalises
/// over — how many outputs are appended (one, several), which `OutputConfig` member each
/// carries, the output statuses, and the form and length of the resource name (every
/// character class its pattern `^[\w \-\.',@:;/]*$` admits, at the minimum and the
/// maximum of its `@length` 1..2048). Every row appends at least one output, which is
/// what `outputs` carries ("an array of one or more outputs"); a request carrying none is
/// refused, which `supplied_output_cardinality.rs` pins.
struct AssociationCase {
    label: &'static str,
    feed_outputs: Value,
    resource: String,
    appended: Value,
}

fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            label: "one appended output onto a one-output feed",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "source-media-1".to_string(),
            appended: json!([output("subs1", "subtitling", "ENABLED")]),
        },
        AssociationCase {
            label: "three appended outputs, one per remaining OutputConfig member",
            feed_outputs: json!([output("crop1", "cropping", "DISABLED")]),
            resource: "Stadium cam 2 - east.side's feed".to_string(),
            appended: json!([
                output("clip1", "clipping", "ENABLED"),
                output("subs1", "subtitling", "DISABLED"),
                {"name": "meta1", "outputConfig": output_config("contextualMetadata"),
                 "status": "ENABLED", "description": "contextual metadata"}
            ]),
        },
        AssociationCase {
            label: "a resource name using every character class its pattern admits",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "media@venue:cam/1;take-2".to_string(),
            appended: json!([output("meta1", "contextualMetadata", "DISABLED")]),
        },
        AssociationCase {
            label: "a one-character resource name, the minimum its @length admits",
            feed_outputs: json!([output("clip1", "clipping", "ENABLED")]),
            resource: "a".to_string(),
            appended: json!([output("crop9", "cropping", "ENABLED")]),
        },
        AssociationCase {
            label: "a 2048-character resource name, the maximum its @length admits",
            feed_outputs: json!([output("meta1", "contextualMetadata", "ENABLED")]),
            resource: "m".repeat(2048),
            appended: json!([output("subs2", "subtitling", "DISABLED")]),
        },
    ]
}

/// AssociateFeed claims the feed for one resource and appends the request's outputs to
/// the stored list; GetFeed and ListFeeds report that stored record.
#[tokio::test]
async fn associate_feed_claims_the_feed_and_appends_its_outputs() {
    let router = router();

    for case in association_cases() {
        let label = case.label;
        let id = available_feed(&router, "assoc-feed", case.feed_outputs.clone()).await;

        let (status, associated) = associate(&router, &id, &case.resource, case.appended.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: AssociateFeed answers 200, got {status} with {associated}"
        );
        assert_eq!(
            associated,
            json!({"arn": expected_arn(&id), "id": id}),
            "case {label:?}: the response is the feed's arn and id, got {associated}"
        );

        // The round trip: the claim and the appended outputs are what storage holds.
        let feed = get_feed(&router, &id, label).await;
        assert_eq!(
            feed["status"],
            json!("ACTIVE"),
            "case {label:?}: an associated feed reports ACTIVE, got {feed}"
        );
        assert_eq!(
            feed["association"],
            json!({"associatedResourceName": case.resource}),
            "case {label:?}: the feed reports the associated resource, got {}",
            feed["association"]
        );
        let mut expected = expected_outputs(&case.feed_outputs, false);
        expected.extend(expected_outputs(&case.appended, true));
        assert_eq!(
            feed["outputs"],
            Value::Array(expected),
            "case {label:?}: the appended outputs follow the feed's own, each marked \
             fromAssociation, got {}",
            feed["outputs"]
        );

        // ListFeeds projects the same association.
        let (status, listed) = call(&router, "GET", "/v1/feeds?maxResults=100", None).await;
        let summary = listed["feeds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|feed| feed["id"] == json!(id));
        assert_eq!(
            summary.as_ref().map(|feed| feed["association"].clone()),
            Some(json!({"associatedResourceName": case.resource})),
            "case {label:?}: ListFeeds reports the association too (HTTP {status}), got {listed}"
        );
    }
}

/// AssociateFeed is a PATCH: the SAME resource associating again only appends more
/// outputs, and leaves everything else as it was.
#[tokio::test]
async fn re_associating_the_same_resource_appends_more_outputs() {
    let router = router();
    let resource = "source-media-patch";
    let id = available_feed(
        &router,
        "patch-feed",
        json!([output("crop1", "cropping", "ENABLED")]),
    )
    .await;

    // (what the round asks for, what it really adds): each row appends a different number
    // of further outputs, and the expected list grows by exactly the ones the feed does
    // not hold yet — the second round asks again for the output the first one appended,
    // which is in place already and so is not appended a second time.
    let rounds: [(Value, Value); 3] = [
        (
            json!([output("subs1", "subtitling", "ENABLED")]),
            json!([output("subs1", "subtitling", "ENABLED")]),
        ),
        (json!([output("subs1", "subtitling", "ENABLED")]), json!([])),
        (
            json!([
                output("clip1", "clipping", "DISABLED"),
                output("meta1", "contextualMetadata", "ENABLED")
            ]),
            json!([
                output("clip1", "clipping", "DISABLED"),
                output("meta1", "contextualMetadata", "ENABLED")
            ]),
        ),
    ];
    let mut expected = expected_outputs(&json!([output("crop1", "cropping", "ENABLED")]), false);

    for (round, (appended, added)) in rounds.iter().enumerate() {
        let label = format!(
            "round {round} asking for {} outputs, {} of them new",
            appended.as_array().unwrap().len(),
            added.as_array().unwrap().len()
        );
        let (status, body) = associate(&router, &id, resource, appended.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: re-associating the same resource is accepted, got {status} with {body}"
        );
        expected.extend(expected_outputs(added, true));

        let feed = get_feed(&router, &id, &label).await;
        assert_eq!(
            feed["outputs"],
            Value::Array(expected.clone()),
            "case {label:?}: the new outputs are appended to the existing ones, got {}",
            feed["outputs"]
        );
        assert_eq!(
            feed["association"],
            json!({"associatedResourceName": resource}),
            "case {label:?}: the association is unchanged, got {}",
            feed["association"]
        );
        assert_eq!(
            feed["status"],
            json!("ACTIVE"),
            "case {label:?}: the feed stays ACTIVE, got {}",
            feed["status"]
        );
    }
}

/// "You can associate only one resource with each feed": a second, different resource is
/// refused with the modelled `ValidationException`, and the first claim stands.
#[tokio::test]
async fn a_second_resource_is_refused_and_the_first_claim_stands() {
    let router = router();
    let holder = "source-media-holder";
    // The rows differ in how the second name differs from the holder's: a plain other
    // name, one that only differs in case, and one that is a prefix of the holder's.
    for other in [
        "source-media-other",
        "SOURCE-MEDIA-HOLDER",
        "source-media",
        "a",
    ] {
        let id = available_feed(
            &router,
            "single-resource-feed",
            json!([output("crop1", "cropping", "ENABLED")]),
        )
        .await;
        let (status, _) = associate(
            &router,
            &id,
            holder,
            json!([output("subs1", "subtitling", "ENABLED")]),
        )
        .await;
        assert_eq!(status, 200, "case {other:?}: the first claim is accepted");

        let (status, error) = associate(
            &router,
            &id,
            other,
            json!([output("clip1", "clipping", "ENABLED")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {other:?}: a second resource is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {other:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(FEED_ALREADY_ASSOCIATED_MESSAGE),
            "case {other:?}: the refusal carries the rule's invariant sentence, got {}",
            error["message"]
        );

        // The refusal changed nothing: neither the holder nor its outputs.
        let feed = get_feed(&router, &id, other).await;
        assert_eq!(
            feed["association"],
            json!({"associatedResourceName": holder}),
            "case {other:?}: the first claim still holds, got {}",
            feed["association"]
        );
        assert_eq!(
            feed["outputs"].as_array().map(Vec::len),
            Some(2),
            "case {other:?}: the refused outputs were not appended, got {}",
            feed["outputs"]
        );
    }
}

/// A feed takes an association only from a status that admits one: from every other
/// status the call is refused with the modelled `ValidationException` carrying the
/// rule's invariant sentence, and nothing is written.
#[tokio::test]
async fn associate_is_refused_from_every_status_that_takes_no_association() {
    let router = router();
    let feed_outputs = json!([output("crop1", "cropping", "ENABLED")]);

    // One row per non-associable status the service produces (`FeedStatus`: DELETING,
    // UPDATING, ARCHIVED — AVAILABLE and ACTIVE are the associable ones, covered above;
    // CREATING is never reported, because a created feed is AVAILABLE at once). The
    // UPDATING row is a feed whose reconfiguration is still in flight, which is pinned
    // in `feed_reconfiguration_status` and is here so the table spans the whole status
    // dimension.
    for status_under_test in ["DELETING", "UPDATING", "ARCHIVED"] {
        let id = match status_under_test {
            "DELETING" => {
                let id = available_feed(&router, "deleting-feed", feed_outputs.clone()).await;
                let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
                assert_eq!(status, 202, "case {status_under_test:?}: DeleteFeed answers 202");
                id
            }
            "UPDATING" => {
                let id = available_feed(&router, "updating-feed", feed_outputs.clone()).await;
                let (status, _) = call(
                    &router,
                    "PUT",
                    &format!("/v1/feed/{id}"),
                    Some(json!({"name": "updating-feed", "outputs": feed_outputs.clone()})),
                )
                .await;
                assert_eq!(status, 200, "case {status_under_test:?}: UpdateFeed answers 200");
                id
            }
            "ARCHIVED" => {
                let id = available_feed(&router, "archived-feed", feed_outputs.clone()).await;
                let (status, _) = associate(
                    &router,
                    &id,
                    "released-media",
                    json!([output("subs1", "subtitling", "ENABLED")]),
                )
                .await;
                assert_eq!(status, 200, "case {status_under_test:?}: the claim is accepted");
                let (status, _) = disassociate(&router, &id, "released-media").await;
                assert_eq!(
                    status, 200,
                    "case {status_under_test:?}: DisassociateFeed answers 200"
                );
                id
            }
            other => panic!("no such status row: {other}"),
        };

        let before = get_feed(&router, &id, status_under_test).await;
        assert_eq!(
            before["status"],
            json!(status_under_test),
            "case {status_under_test:?}: the feed is in the status under test, got {before}"
        );

        let (status, error) = associate(
            &router,
            &id,
            "would-be-media",
            json!([output("subs1", "subtitling", "ENABLED")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {status_under_test:?}: the association is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {status_under_test:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(FEED_NOT_ASSOCIABLE_MESSAGE),
            "case {status_under_test:?}: the refusal carries the rule's invariant sentence, \
             got {}",
            error["message"]
        );

        // Nothing was written: no association, and the outputs are untouched.
        let (code, feed) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        if code == 200 {
            assert_eq!(
                feed.get("association").cloned().unwrap_or(Value::Null),
                Value::Null,
                "case {status_under_test:?}: the refused call associated nothing, got {feed}"
            );
            assert_eq!(
                feed["outputs"],
                before["outputs"],
                "case {status_under_test:?}: the refused outputs were not appended, got {}",
                feed["outputs"]
            );
        }
    }
}

/// `associatedResourceName` "must not resemble an ARN" — a rule its own pattern cannot
/// express, since that pattern admits `:` and `/`.
#[tokio::test]
async fn a_resource_name_that_resembles_an_arn_is_refused() {
    let router = router();
    // (name, accepted) — the ARN-like rows open with the `arn:` prefix in any case; the
    // accepted rows use the same characters without being an ARN, including non-ASCII
    // word characters, which the member's `\w` class admits and which must be judged
    // (not byte-sliced) by the ARN rule.
    let cases = [
        ("arn:aws:s3:::media-bucket/live.ts", false),
        ("ARN:aws:elemental-inference:us-west-2:000000000000:feed/x", false),
        ("arn:", false),
        ("not-an-arn:just a name", true),
        ("media/arn:aws:s3", true),
        // Multi-byte from the first byte: no `arn:` prefix, and no byte boundary at 4.
        ("日本語の素材", true),
        // A multi-byte character STRADDLING byte offset 4.
        ("ab日本語", true),
        ("héllo-média", true),
        // Shorter than the prefix it is compared against.
        ("ar", true),
    ];

    for (name, accepted) in cases {
        let id = available_feed(
            &router,
            "arn-name-feed",
            json!([output("crop1", "cropping", "ENABLED")]),
        )
        .await;
        let (status, body) = associate(
            &router,
            &id,
            name,
            json!([output("subs1", "subtitling", "ENABLED")]),
        )
        .await;
        assert!(
            status == 200 || status == 400,
            "case {name:?}: the name is judged, never crashing the handler, got {status} \
             with {body}"
        );
        if accepted {
            assert_eq!(
                status, 200,
                "case {name:?}: a name that is not an ARN is accepted, got {status} with {body}"
            );
            let feed = get_feed(&router, &id, name).await;
            assert_eq!(
                feed["association"],
                json!({"associatedResourceName": name}),
                "case {name:?}: the accepted name is what is stored, got {}",
                feed["association"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {name:?}: a name resembling an ARN is a 400, got {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case {name:?}: that 400 is a ValidationException, got {body}"
            );
            let feed = get_feed(&router, &id, name).await;
            assert_eq!(
                feed.get("association").cloned().unwrap_or(Value::Null),
                Value::Null,
                "case {name:?}: the refused name was not stored, got {feed}"
            );
        }
    }
}

/// A resource name outside the member's modelled `@length` 1..2048 never reaches the
/// handler: the boundary refuses it, at each published bound and one past it.
#[tokio::test]
async fn a_resource_name_outside_the_modelled_length_is_refused() {
    let router = router();
    let id = available_feed(
        &router,
        "length-feed",
        json!([output("crop1", "cropping", "ENABLED")]),
    )
    .await;

    // (length, expected status): 1 and 2048 are the bounds the model publishes; 0 and
    // 2049 are one past each.
    for (length, expected) in [(0_usize, 400), (1, 200), (2048, 200), (2049, 400)] {
        let name = "n".repeat(length);
        // Each accepted row re-associates the SAME feed under a fresh name, so the
        // rows after the first run against an ACTIVE feed; use a new feed per row.
        let id = if expected == 200 {
            available_feed(
                &router,
                "length-feed",
                json!([output("crop1", "cropping", "ENABLED")]),
            )
            .await
        } else {
            id.clone()
        };
        let (status, body) = associate(
            &router,
            &id,
            &name,
            json!([output("subs1", "subtitling", "ENABLED")]),
        )
        .await;
        assert_eq!(
            status, expected,
            "case length {length}: expected HTTP {expected}, got {status} with {body}"
        );
    }
}

/// AssociateFeed can only APPEND outputs: one whose name an existing output already has
/// would modify it, so the call is a `ConflictException` (HTTP 409) — as is a request
/// that names one output twice.
#[tokio::test]
async fn an_output_name_that_already_exists_is_a_conflict() {
    let router = router();
    let feed_outputs = json!([
        output("crop1", "cropping", "ENABLED"),
        output("subs1", "subtitling", "DISABLED")
    ]);

    // The rows differ in where the collision is: with the feed's first output, with a
    // later one, within the request itself, and (the control row) nowhere.
    let cases = [
        ("collides with the feed's first output", json!([output("crop1", "clipping", "ENABLED")]), 409),
        ("collides with a later output of the feed", json!([output("subs1", "cropping", "ENABLED")]), 409),
        (
            "names one output twice in the request",
            json!([
                output("clip1", "clipping", "ENABLED"),
                output("clip1", "cropping", "DISABLED")
            ]),
            409,
        ),
        ("collides with nothing", json!([output("clip1", "clipping", "ENABLED")]), 200),
    ];

    for (label, appended, expected) in cases {
        let id = available_feed(&router, "conflict-feed", feed_outputs.clone()).await;
        let (status, body) = associate(&router, &id, "source-media", appended).await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {body}"
        );
        if expected == 409 {
            assert_eq!(
                body["__type"],
                json!("ConflictException"),
                "case {label:?}: that 409 is a ConflictException, got {body}"
            );
            let feed = get_feed(&router, &id, label).await;
            assert_eq!(
                feed["outputs"].as_array().map(Vec::len),
                Some(2),
                "case {label:?}: the conflicting call appended nothing, got {}",
                feed["outputs"]
            );
            assert_eq!(
                feed["status"],
                json!("AVAILABLE"),
                "case {label:?}: the conflicting call did not claim the feed, got {}",
                feed["status"]
            );
        }
    }
}

/// A feed's output list is bounded: an append that would exceed the quota answers the
/// modelled `ServiceQuotaExceededException` (HTTP 402) instead of growing state.
#[tokio::test]
async fn associate_refuses_an_append_past_the_output_quota() {
    let router = router();
    let own: Vec<Value> = (0..MAX_OUTPUTS_PER_FEED - 1)
        .map(|index| output(&format!("own{index}"), "cropping", "ENABLED"))
        .collect();
    let id = available_feed(&router, "quota-feed", json!(own)).await;

    // The bound itself is accepted: MAX_OUTPUTS_PER_FEED - 1 own outputs plus one.
    let (status, body) = associate(
        &router,
        &id,
        "source-media",
        json!([output("fill1", "subtitling", "ENABLED")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at the quota ({MAX_OUTPUTS_PER_FEED} outputs): the append is accepted, got {status} with {body}"
    );

    // One past it is not.
    let (status, error) = associate(
        &router,
        &id,
        "source-media",
        json!([output("fill2", "subtitling", "ENABLED")]),
    )
    .await;
    assert_eq!(
        status, 402,
        "case past the quota ({} outputs): the append is a 402, got {status} with {error}",
        MAX_OUTPUTS_PER_FEED + 1
    );
    assert_eq!(
        error["__type"],
        json!("ServiceQuotaExceededException"),
        "case past the quota: that 402 is a ServiceQuotaExceededException, got {error}"
    );
    let feed = get_feed(&router, &id, "past the quota").await;
    assert_eq!(
        feed["outputs"].as_array().map(Vec::len),
        Some(MAX_OUTPUTS_PER_FEED),
        "case past the quota: the refused append grew nothing, got {}",
        feed["outputs"]
    );

    // The bound is on what the call really ADDS: a repeat of a request whose outputs are
    // all in place already adds nothing, so the quota never refuses it — not even on a
    // feed whose list is AT the bound, which is as far as any operation that replaces a
    // feed's output list may take it.
    let (status, body) = associate(
        &router,
        &id,
        "source-media",
        json!([output("fill1", "subtitling", "ENABLED")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a repeat at the quota ({MAX_OUTPUTS_PER_FEED} outputs): the request is served, \
         got {status} with {body}"
    );
    let feed = get_feed(&router, &id, "a repeat at the quota").await;
    assert_eq!(
        (&feed["status"], feed["outputs"].as_array().map(Vec::len)),
        (&json!("ACTIVE"), Some(MAX_OUTPUTS_PER_FEED)),
        "case a repeat at the quota: the feed is claimed and holds one copy of each output, \
         got {feed}"
    );
}

/// DisassociateFeed releases the named resource, disables every output and archives the
/// feed; the archive is irreversible.
#[tokio::test]
async fn disassociate_releases_the_resource_and_archives_the_feed_irreversibly() {
    let router = router();

    for case in association_cases() {
        let label = case.label;
        let id = available_feed(&router, "release-feed", case.feed_outputs.clone()).await;
        let (status, _) = associate(&router, &id, &case.resource, case.appended.clone()).await;
        assert_eq!(status, 200, "case {label:?}: the claim is accepted");

        let (status, released) = disassociate(&router, &id, &case.resource).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DisassociateFeed answers 200, got {status} with {released}"
        );
        assert_eq!(
            released,
            json!({"arn": expected_arn(&id), "id": id}),
            "case {label:?}: the response is the feed's arn and id, got {released}"
        );

        let feed = get_feed(&router, &id, label).await;
        assert_eq!(
            feed["status"],
            json!("ARCHIVED"),
            "case {label:?}: a released feed reports ARCHIVED, got {feed}"
        );
        assert_eq!(
            feed.get("association").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {label:?}: the resource is released, got {}",
            feed["association"]
        );
        let statuses: Vec<&str> = feed["outputs"]
            .as_array()
            .expect("the outputs are a list")
            .iter()
            .map(|output| output["status"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !statuses.is_empty() && statuses.iter().all(|status| *status == "DISABLED"),
            "case {label:?}: every output in the feed becomes DISABLED, got {statuses:?}"
        );

        // Irreversible: the feed takes no further association, and no OTHER resource can
        // be released from it. (Repeating the release that WAS served is that one release
        // again under its idempotency token, not a second release — `feed_release_retry.rs`.)
        let (status, error) = associate(
            &router,
            &id,
            &case.resource,
            json!([output("post1", "cropping", "ENABLED")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: re-associating an ARCHIVED feed is a 400, got {status} with {error}"
        );
        // A name no row releases, and well inside the member's `@length` 1..2048.
        let other_resource = "another-media";
        assert_ne!(
            other_resource, case.resource,
            "case {label:?}: the other name must not be the released resource's"
        );
        let (status, error) = disassociate(&router, &id, other_resource).await;
        assert_eq!(
            status, 400,
            "case {label:?}: releasing another resource from it is a 400, got {status} with \
             {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
    }
}

/// A feed whose deletion has been accepted refuses both association calls, and the
/// removal the clock already owes still completes.
#[tokio::test]
async fn a_feed_that_is_being_deleted_refuses_both_association_calls() {
    let router = router();
    let feed_outputs = json!([output("crop1", "cropping", "ENABLED")]);

    // Whether the feed holds an association when the deletion is accepted.
    for associated in [false, true] {
        let id = available_feed(&router, "deleting-feed", feed_outputs.clone()).await;
        if associated {
            let (status, _) = associate(
                &router,
                &id,
                "source-media",
                json!([output("subs1", "subtitling", "ENABLED")]),
            )
            .await;
            assert_eq!(
                status, 200,
                "case associated={associated}: the claim is accepted"
            );
        }
        let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case associated={associated}: DeleteFeed answers 202"
        );

        for (operation, answer) in [
            (
                "associate",
                associate(
                    &router,
                    &id,
                    "too-late",
                    json!([output("meta1", "contextualMetadata", "ENABLED")]),
                )
                .await,
            ),
            (
                "disassociate",
                disassociate(&router, &id, "source-media").await,
            ),
        ] {
            let label = format!("{operation} on a DELETING feed (associated={associated})");
            let (status, error) = answer;
            assert_eq!(
                status, 400,
                "case {label:?}: a feed being deleted takes neither call, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ValidationException"),
                "case {label:?}: that 400 is a ValidationException, got {error}"
            );
        }

        // And the removal the clock was owed still happens: neither refused call
        // cancelled the accepted deletion.
        past_the_transition_span();
        let (status, gone) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case associated={associated}: the accepted deletion still completed, got \
             {status} with {gone}"
        );
    }
}

/// DisassociateFeed takes "the name of the resource CURRENTLY associated with the feed":
/// any other name is refused and the association stands.
#[tokio::test]
async fn disassociate_with_a_name_that_is_not_the_associated_resource_is_refused() {
    let router = router();
    let holder = "source-media-holder";
    // The rows differ in how the name differs from the holder's: an unrelated name, one
    // differing only in case, a prefix, and the shortest name the member admits.
    for other in [
        "source-media-other",
        "Source-Media-Holder",
        "source-media",
        "a",
    ] {
        let id = available_feed(
            &router,
            "wrong-name-feed",
            json!([output("crop1", "cropping", "ENABLED")]),
        )
        .await;
        let (status, _) = associate(
            &router,
            &id,
            holder,
            json!([output("subs1", "subtitling", "ENABLED")]),
        )
        .await;
        assert_eq!(status, 200, "case {other:?}: the claim is accepted");

        let (status, error) = disassociate(&router, &id, other).await;
        assert_eq!(
            status, 400,
            "case {other:?}: the wrong resource name is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {other:?}: that 400 is a ValidationException, got {error}"
        );

        let feed = get_feed(&router, &id, other).await;
        assert_eq!(
            (&feed["status"], &feed["association"]),
            (
                &json!("ACTIVE"),
                &json!({"associatedResourceName": holder})
            ),
            "case {other:?}: the refused release left the association in place, got {feed}"
        );
        assert_eq!(
            feed["outputs"][0]["status"],
            json!("ENABLED"),
            "case {other:?}: the refused release disabled nothing, got {}",
            feed["outputs"]
        );
    }
}

/// A feed that is not currently associated has nothing to release, so DisassociateFeed
/// is refused with the modelled `ValidationException` — whose message reports the status
/// the feed was found in (how a refusal is WORDED is `error_message_rule.rs`).
#[tokio::test]
async fn disassociate_is_refused_from_every_status_that_holds_no_resource() {
    let router = router();
    let feed_outputs = json!([output("crop1", "cropping", "ENABLED")]);

    // One row per status that holds no resource: a created feed, one whose deletion has
    // been accepted, and one already released.
    for (status_under_test, feed_status) in [
        ("AVAILABLE", FeedStatus::Available),
        ("DELETING", FeedStatus::Deleting),
        ("ARCHIVED", FeedStatus::Archived),
    ] {
        let id = match status_under_test {
            "AVAILABLE" => available_feed(&router, "never-associated", feed_outputs.clone()).await,
            "DELETING" => {
                let id = available_feed(&router, "never-associated", feed_outputs.clone()).await;
                let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
                assert_eq!(status, 202, "case {status_under_test:?}: DeleteFeed answers 202");
                id
            }
            "ARCHIVED" => {
                let id = available_feed(&router, "released-feed", feed_outputs.clone()).await;
                let (status, _) = associate(
                    &router,
                    &id,
                    "released-media",
                    json!([output("subs1", "subtitling", "ENABLED")]),
                )
                .await;
                assert_eq!(status, 200, "case {status_under_test:?}: the claim is accepted");
                let (status, _) = disassociate(&router, &id, "released-media").await;
                assert_eq!(
                    status, 200,
                    "case {status_under_test:?}: DisassociateFeed answers 200"
                );
                id
            }
            other => panic!("no such status row: {other}"),
        };

        let (status, error) = disassociate(&router, &id, "never-associated-media").await;
        assert_eq!(
            status, 400,
            "case {status_under_test:?}: releasing an unassociated feed is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {status_under_test:?}: that 400 is a ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(feed_not_associated_message(feed_status)),
            "case {status_under_test:?}: the refusal reports the status the feed was found \
             in, got {}",
            error["message"]
        );
    }
}

/// `dryRun` validates the request and persists NOTHING, on either operation and whether
/// the real call would succeed or fail.
#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let router = router();
    let feed_outputs = json!([output("crop1", "cropping", "ENABLED")]);

    // (label, uri suffix, body, expected status) — a dry-run associate on an
    // associable feed, on a feed that already holds another resource, and a dry-run
    // release of the resource that does hold it.
    let resource = "source-media-dry";
    let dry_cases: Vec<(&str, &str, Value, u16, bool)> = vec![
        (
            "dry-run associate on an AVAILABLE feed",
            "associate",
            json!({"associatedResourceName": resource, "outputs": [output("subs1", "subtitling", "ENABLED")], "dryRun": true}),
            200,
            false,
        ),
        (
            "dry-run associate of a second resource",
            "associate",
            json!({"associatedResourceName": "other-media",
                   "outputs": [output("subs1", "subtitling", "ENABLED")], "dryRun": true}),
            400,
            true,
        ),
        (
            "dry-run disassociate of the associated resource",
            "disassociate",
            json!({"associatedResourceName": resource, "dryRun": true}),
            200,
            true,
        ),
        (
            "dry-run disassociate under the wrong name",
            "disassociate",
            json!({"associatedResourceName": "other-media", "dryRun": true}),
            400,
            true,
        ),
    ];

    for (label, verb, body, expected, pre_associate) in dry_cases {
        let id = available_feed(&router, "dry-run-feed", feed_outputs.clone()).await;
        if pre_associate {
            let (status, _) = associate(
                &router,
                &id,
                resource,
                json!([output("assoc1", "contextualMetadata", "ENABLED")]),
            )
            .await;
            assert_eq!(status, 200, "case {label:?}: the setup claim is accepted");
        }
        let before = get_feed(&router, &id, label).await;

        let (status, answered) = call(
            &router,
            "POST",
            &format!("/v1/feed/{id}/{verb}"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {answered}"
        );
        if expected == 200 {
            assert_eq!(
                answered,
                json!({"arn": expected_arn(&id), "id": id}),
                "case {label:?}: a dry run answers the same arn and id, got {answered}"
            );
        }

        let after = get_feed(&router, &id, label).await;
        assert_eq!(
            after, before,
            "case {label:?}: a dry run persists nothing, got {after}"
        );
    }
}

/// An id no live feed has is a 404 `ResourceNotFoundException` on both association
/// operations, and the refusal never reports the id that was probed.
#[tokio::test]
async fn an_unknown_feed_id_is_a_404_on_both_association_operations() {
    let router = router();
    // One live feed, so the 404 is about the id and not about an empty store.
    let live_id = available_feed(
        &router,
        "the-only-feed",
        json!([output("crop1", "cropping", "ENABLED")]),
    )
    .await;

    // Every id below satisfies ^[a-z0-9]{19}$, so the boundary admits it and the
    // handler is what refuses it.
    for id in [
        "0000000000000000000",
        "zzzzzzzzzzzzzzzzzzz",
        "abcdefghij123456789",
    ] {
        assert_ne!(id, live_id, "the unknown ids must not be the live feed's");
        for (verb, body) in [
            (
                "associate",
                json!({"associatedResourceName": "source-media",
                       "outputs": [output("subs1", "subtitling", "ENABLED")]}),
            ),
            (
                "disassociate",
                json!({"associatedResourceName": "source-media"}),
            ),
        ] {
            let label = format!("POST /v1/feed/{id}/{verb}");
            let (status, error) = call(
                &router,
                "POST",
                &format!("/v1/feed/{id}/{verb}"),
                Some(body),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {label:?}: an unknown feed id is a 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
            );
            assert!(
                error["message"].as_str().is_some_and(|m| !m.contains(id)),
                "case {label:?}: the refusal does not report the probed id, got {}",
                error["message"]
            );
        }
    }
}

/// Concurrent associations on ONE feed resolve to exactly one winner: the claim is taken
/// under the state lock, so every other caller is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_associations_on_one_feed_have_exactly_one_winner() {
    let router = Arc::new(router());
    // The rows differ in how many callers race for the one feed.
    for racers in [2_usize, 4, 8] {
        let id = available_feed(
            &router,
            "race-feed",
            json!([output("crop1", "cropping", "ENABLED")]),
        )
        .await;

        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let id = id.clone();
            tasks.push(tokio::spawn(async move {
                let resource = format!("racing-media-{racer}");
                let (status, body) = associate(
                    &router,
                    &id,
                    &resource,
                    json!([output(&format!("out{racer}"), "subtitling", "ENABLED")]),
                )
                .await;
                (resource, status, body)
            }));
        }

        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for task in tasks {
            let (resource, status, body) = task.await.expect("the racing task completed");
            match status {
                200 => winners.push(resource),
                400 => {
                    assert_eq!(
                        body["__type"],
                        json!("ValidationException"),
                        "case {racers} racers: a loser is refused with ValidationException, got {body}"
                    );
                    losers.push(resource);
                }
                other => panic!(
                    "case {racers} racers: a racer answered HTTP {other} with {body}"
                ),
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "case {racers} racers: expected exactly 1 winner, got {winners:?} with losers {losers:?}"
        );
        assert_eq!(
            losers.len(),
            racers - 1,
            "case {racers} racers: every other racer is a loser, got {losers:?}"
        );

        // The feed holds the winner's claim, and only the winner's output was appended.
        let feed = get_feed(&router, &id, "the race").await;
        assert_eq!(
            feed["association"],
            json!({"associatedResourceName": winners[0]}),
            "case {racers} racers: the feed holds the winner's resource, got {feed}"
        );
        assert_eq!(
            feed["outputs"].as_array().map(Vec::len),
            Some(2),
            "case {racers} racers: only the winner's output was appended, got {}",
            feed["outputs"]
        );
    }
}
