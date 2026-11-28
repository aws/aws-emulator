// SPDX-License-Identifier: CC0-1.0

//! A REPEAT of an `AssociateFeed` that was already served, end to end over the wire.
//!
//! Every case here drives the real request path — the router picks the service from the
//! SigV4 credential scope, the framework deserializes and validates the body, the
//! handler runs, and the response is serialized as `restJson1` — so what these tests
//! assert is what a client sees.
//!
//! THE RULE under test: `AssociateFeed` is `@idempotent` and its
//! `associatedResourceName` is the operation's `@idempotencyToken`
//! (`smithy_query(query="operation",operation="AssociateFeed")`), so a request that
//! repeats one the feed has already served is that one request once more rather than a
//! second append: its effect is applied ONCE. An output the feed already holds exactly
//! as the request describes it is therefore in place already — it is neither appended a
//! second time nor refused — so:
//!
//! * a repeat of the whole request answers the first call's success and leaves the feed
//!   exactly as that call left it, with ONE copy of each output;
//! * a request that repeats some outputs and carries one the feed does not hold yet
//!   appends exactly that one ("New outputs will be appended");
//! * an output whose name the feed holds under a DIFFERENT definition is not a repeat:
//!   appending it would MODIFY an existing output, which this operation cannot do, so it
//!   stays the modelled `ConflictException` (HTTP 409) and nothing is written — including
//!   when the output the feed holds under that name did not enter through an
//!   association.
//!
//! The first rule is asserted over a TABLE whose rows differ along what it generalises
//! over — how many outputs the repeated request carries (none, one, several), which
//! `OutputConfig` member each carries, their statuses and descriptions, the form and
//! length of the resource name (the character classes its pattern `^[\w \-\.',@:;/]*$`
//! admits, at the minimum and the maximum of its `@length` 1..2048), and how many times
//! the request is repeated, with and without `dryRun` on the repeat.

use crate::harness::{call, output, output_config, router};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Create one feed carrying `outputs` and return its id. A created feed is settled and
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

/// `POST /v1/feed/{id}/associate`, claiming the feed for `resource` and asking for
/// `outputs` — as a dry run when `dry_run`.
async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    resource: &str,
    outputs: &Value,
    dry_run: bool,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({
            "associatedResourceName": resource,
            "outputs": outputs,
            "dryRun": dry_run
        })),
    )
    .await
}

/// `GET /v1/feed/{id}`, asserting it is served, and returning the record it reports.
async fn get_feed(router: &MultiServiceRouter, id: &str, label: &str) -> Value {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    feed
}

/// One output named `name`, carrying the `kind` config, `status` and a description —
/// the four members `CreateOutput` carries
/// (`smithy_query(query="shape",shape="CreateOutput")`).
fn described_output(name: &str, kind: &str, status: &str, description: &str) -> Value {
    json!({
        "name": name,
        "outputConfig": output_config(kind),
        "status": status,
        "description": description
    })
}

/// The `GetOutput` projection of `submitted`, with `fromAssociation` set to
/// `from_association` (`GetOutput$fromAssociation`: true only for an output that entered
/// the feed through `AssociateFeed`).
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

/// One row of the repeat table: the feed's own outputs, the resource that claims it, the
/// outputs the claim asks for, and how many times the whole request is repeated.
struct RepeatCase {
    label: &'static str,
    feed_outputs: Value,
    resource: String,
    requested: Value,
    repeats: usize,
}

fn repeat_cases() -> Vec<RepeatCase> {
    vec![
        RepeatCase {
            label: "one output appended, a plain resource name",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "source-media".to_string(),
            requested: json!([output("subs1", "subtitling", "ENABLED")]),
            repeats: 1,
        },
        RepeatCase {
            label: "one output, a resource name carrying every punctuation class, repeated twice",
            feed_outputs: json!([output("crop1", "cropping", "DISABLED")]),
            resource: "media@venue:cam/1;take-2".to_string(),
            requested: json!([output("meta1", "contextualMetadata", "ENABLED")]),
            repeats: 2,
        },
        RepeatCase {
            label: "three outputs, one per remaining OutputConfig member",
            feed_outputs: json!([output("crop1", "cropping", "ENABLED")]),
            resource: "Stadium cam 2 - east.side's feed".to_string(),
            requested: json!([
                output("clip1", "clipping", "ENABLED"),
                output("subs1", "subtitling", "DISABLED"),
                described_output("meta1", "contextualMetadata", "ENABLED", "contextual metadata")
            ]),
            repeats: 3,
        },
        RepeatCase {
            label: "an output carrying a description, a one-character resource name",
            feed_outputs: json!([output("clip1", "clipping", "ENABLED")]),
            resource: "a".to_string(),
            requested: json!([described_output("crop9", "cropping", "DISABLED", "wide crop")]),
            repeats: 2,
        },
        RepeatCase {
            label: "a 2048-character resource name, the maximum its @length admits",
            feed_outputs: json!([output("meta1", "contextualMetadata", "ENABLED")]),
            resource: "m".repeat(2048),
            requested: json!([output("subs2", "subtitling", "DISABLED")]),
            repeats: 1,
        },
        RepeatCase {
            label: "a resource name of non-ASCII word characters, which its \\w class admits",
            feed_outputs: json!([output("subs1", "subtitling", "ENABLED")]),
            resource: "Estádio-Ñ-日本語".to_string(),
            requested: json!([
                output("crop1", "cropping", "ENABLED"),
                output("clip2", "clipping", "DISABLED")
            ]),
            repeats: 2,
        },
    ]
}

/// A repeated identical association answers the first call's success again — the same
/// `arn` and `id` — and leaves the feed exactly as the first call left it: ONE copy of
/// each output.
#[tokio::test]
async fn a_repeated_association_answers_the_first_calls_success_and_leaves_one_copy() {
    let router = router();

    for case in repeat_cases() {
        let label = case.label;
        let id = create_feed(&router, "repeat-feed", case.feed_outputs.clone()).await;

        let (status, served) =
            associate(&router, &id, &case.resource, &case.requested, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first association is served, got {status} with {served}"
        );

        // What that call left behind: the record every repeat below must still find.
        let claimed = get_feed(&router, &id, label).await;
        let mut expected = expected_outputs(&case.feed_outputs, false);
        expected.extend(expected_outputs(&case.requested, true));
        assert_eq!(
            claimed["outputs"],
            Value::Array(expected.clone()),
            "case {label:?}: the first call appended its outputs once, got {}",
            claimed["outputs"]
        );

        for attempt in 1..=case.repeats {
            // An SDK retry repeats the request it sent, so the plain repeat carries the
            // same `dryRun: false`; the odd attempts send it as a dry run, which asks
            // whether the real request would succeed — and it would, identically.
            let dry_run = attempt % 2 == 1;
            let (status, replayed) =
                associate(&router, &id, &case.resource, &case.requested, dry_run).await;
            assert_eq!(
                status, 200,
                "case {label:?}: repeat {attempt} (dryRun={dry_run}) answers 200, got {status} \
                 with {replayed}"
            );
            assert_eq!(
                replayed, served,
                "case {label:?}: repeat {attempt} (dryRun={dry_run}) answers the first call's \
                 body {served}, got {replayed}"
            );
            let after = get_feed(&router, &id, label).await;
            assert_eq!(
                after, claimed,
                "case {label:?}: repeat {attempt} (dryRun={dry_run}) leaves the feed as the \
                 first call left it ({claimed}), got {after}"
            );
        }
    }
}

/// A request that repeats outputs already in place AND carries one the feed does not
/// hold yet appends exactly that one: the repeated part is applied once, the new part is
/// appended.
#[tokio::test]
async fn a_repeat_carrying_a_further_output_appends_only_that_one() {
    let router = router();
    let resource = "partially-repeated-media";
    let own = json!([output("crop1", "cropping", "ENABLED")]);

    // The rows differ in WHERE the output that is not yet held sits in the repeated
    // request — last, first, between two repeated ones — and in how many of the
    // request's outputs are already in place.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the new output last",
            json!([output("subs1", "subtitling", "ENABLED")]),
            json!([
                output("subs1", "subtitling", "ENABLED"),
                output("clip1", "clipping", "DISABLED")
            ]),
            json!([output("clip1", "clipping", "DISABLED")]),
        ),
        (
            "the new output first",
            json!([output("subs1", "subtitling", "DISABLED")]),
            json!([
                output("meta1", "contextualMetadata", "ENABLED"),
                output("subs1", "subtitling", "DISABLED")
            ]),
            json!([output("meta1", "contextualMetadata", "ENABLED")]),
        ),
        (
            "the new output between two that are in place",
            json!([
                output("subs1", "subtitling", "ENABLED"),
                described_output("meta1", "contextualMetadata", "DISABLED", "metadata")
            ]),
            json!([
                output("subs1", "subtitling", "ENABLED"),
                output("clip7", "clipping", "ENABLED"),
                described_output("meta1", "contextualMetadata", "DISABLED", "metadata")
            ]),
            json!([output("clip7", "clipping", "ENABLED")]),
        ),
    ];

    for (label, first, repeated, newly_appended) in rows {
        let id = create_feed(&router, "partial-repeat-feed", own.clone()).await;
        let (status, served) = associate(&router, &id, resource, &first, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first association is served, got {status} with {served}"
        );

        let (status, body) = associate(&router, &id, resource, &repeated, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the repeat carrying a further output is accepted, got {status} \
             with {body}"
        );
        assert_eq!(
            body, served,
            "case {label:?}: it answers the feed's arn and id {served}, got {body}"
        );

        let mut expected = expected_outputs(&own, false);
        expected.extend(expected_outputs(&first, true));
        expected.extend(expected_outputs(&newly_appended, true));
        let feed = get_feed(&router, &id, label).await;
        assert_eq!(
            feed["outputs"],
            Value::Array(expected),
            "case {label:?}: only the output that was not in place is appended, got {}",
            feed["outputs"]
        );
    }
}

/// An output whose name the feed already holds under a DIFFERENT definition is not a
/// repeat: appending it would MODIFY the stored output, so the call stays the modelled
/// `ConflictException` (HTTP 409) and writes nothing.
#[tokio::test]
async fn an_output_of_one_name_that_differs_is_still_a_conflict() {
    let router = router();
    let resource = "conflicting-media";
    let own = json!([output("crop1", "cropping", "ENABLED")]);
    let appended = described_output("subs1", "subtitling", "ENABLED", "english subtitles");

    // A subtitling config that differs from `output_config("subtitling")` in ONE member,
    // so the row below differs from the stored output only in its configuration.
    let other_language = {
        let mut config = output_config("subtitling");
        config["subtitling"]["language"] = json!("fra");
        config
    };

    // (how the output differs from the one the feed holds, the output, expected status)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "the same name under another OutputConfig member",
            output("subs1", "cropping", "ENABLED"),
            409,
        ),
        (
            "the same config member carrying a different value",
            json!({
                "name": "subs1",
                "outputConfig": other_language,
                "status": "ENABLED",
                "description": "english subtitles"
            }),
            409,
        ),
        (
            "the same output under the other status",
            described_output("subs1", "subtitling", "DISABLED", "english subtitles"),
            409,
        ),
        (
            "the same output under another description",
            described_output("subs1", "subtitling", "ENABLED", "french subtitles"),
            409,
        ),
        (
            "the same output with its description dropped",
            output("subs1", "subtitling", "ENABLED"),
            409,
        ),
        (
            // The feed's own output did NOT enter through an association
            // (`fromAssociation: false`), so a request that asks for it as an
            // association's own output would change that, which is a modification too.
            "an output of the feed's own, identical but never associated",
            output("crop1", "cropping", "ENABLED"),
            409,
        ),
        (
            // The control row: exactly what the feed holds, which is in place already.
            "the output exactly as the feed holds it",
            appended.clone(),
            200,
        ),
    ];

    for (label, requested, expected) in rows {
        let id = create_feed(&router, "conflict-repeat-feed", own.clone()).await;
        let (status, served) =
            associate(&router, &id, resource, &json!([appended.clone()]), false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first association is served, got {status} with {served}"
        );
        let claimed = get_feed(&router, &id, label).await;

        let (status, body) = associate(&router, &id, resource, &json!([requested]), false).await;
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
        }
        // Either way the feed is untouched: a refused call writes nothing, and a request
        // whose outputs are all in place has nothing left to apply.
        let after = get_feed(&router, &id, label).await;
        assert_eq!(
            after, claimed,
            "case {label:?}: the feed is left as the served call left it ({claimed}), got \
             {after}"
        );
    }
}
