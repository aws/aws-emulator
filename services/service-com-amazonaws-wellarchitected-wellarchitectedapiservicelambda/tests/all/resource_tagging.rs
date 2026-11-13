// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the tag WRITE, `TagResource`, read back through
//! `ListTagsForResource`.
//!
//! `super::resource_tags` covers the read side on its own (it was written with
//! `ListTagsForResource`); this suite is about what a WRITE records, which is why every
//! rule below ends in a read of the resource that was written.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `TagResource` MERGES the request's pairs into the tag set of the resource its
//!    `WorkloadArn` resolves to, and `ListTagsForResource` of that ARN reports exactly
//!    the stored set. Rows differ along what the rule generalises over: the KIND of
//!    resource the ARN names (`TagResource` doc: "a workload ARN, a custom lens ARN, a
//!    profile ARN, or review template ARN") and the SHAPE of the tag map — how many
//!    pairs, and which key and value forms the modelled `TagMap` constraints admit.
//! 2. A second `TagResource` ADDS to what the resource carries: a key it does not carry
//!    appears, a key it carries takes the new value, and every other key survives.
//! 3. The ARN is RESOLVED against the store: one naming no stored resource — of a kind
//!    this emulator holds, of a kind it does not, in another account or Region, or not
//!    an ARN at all — is answered the shared tagging frontend's `404 NotFoundException`,
//!    stating that frontend's fixed sentence and nothing the request sent, and records
//!    nothing.
//! 4. A resource holds at most `TagMap`'s own maximum of tags: a request that would
//!    raise the stored count past it is refused `400 ValidationException` and changes
//!    nothing, while one that stays at the maximum (overwriting a key already there) is
//!    recorded.
//! 5. The tags of one resource are its own: tagging one resource leaves every other
//!    resource's tags alone, and reading tags changes nothing a later caller sees.
//! 6. The tag WRITE and the tag READ resolve the SAME ARNs: the ARN a write lands on is
//!    the one a read reports, and an ARN that resolves to nothing is refused the same
//!    way by both.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the workload builders are the shared
//! `super::workload_harness`; a resource of each taggable kind is minted through the one
//! operation that mints it, which is why this suite reaches into the import and agent
//! profile suites' `pub(crate)` request helpers rather than spelling those requests a
//! second time.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::resource_tags::{TAGGING_NOT_FOUND_CODE, TAGGING_NOT_FOUND_MESSAGE};
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, template_choice, template_question, Answer,
    Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `POST /tags/{WorkloadArn}`, the wire form of `TagResource`. The ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in ONE path segment, exactly as an SDK
/// sends it.
///
/// `pub(crate)` because `super::create_agent_profile` tags a profile its own create
/// stored, and a resource is tagged by exactly one operation: the two suites must not
/// disagree about how that request is spelled.
pub(crate) async fn tag(client: &Client, arn: &str, tags: &BTreeMap<String, String>) -> Answer {
    client
        .send(
            "POST",
            &format!("/tags/{}", encode_path_segment(arn)),
            Some(json!({ "Tags": tags })),
        )
        .await
}

/// `GET /tags/{WorkloadArn}`, the wire form of `ListTagsForResource`.
async fn list_tags(client: &Client, arn: &str) -> Answer {
    client
        .send("GET", &format!("/tags/{}", encode_path_segment(arn)), None)
        .await
}

/// The tags an answer reports, as a map: the stored set, whatever order the wire carried
/// it in.
fn reported(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .body
        .get("Tags")
        .and_then(Value::as_object)
        .map(|pairs| {
            pairs
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tag map from pairs, so a row can spell one inline.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Tag `arn` and assert the request was recorded, then answer what the resource carries
/// as reported by `ListTagsForResource` — the round trip every rule below reads through.
async fn tag_then_read(
    client: &Client,
    case: &str,
    arn: &str,
    requested: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let tagged = tag(client, arn, requested).await;
    assert_eq!(
        tagged.status, 204,
        "case {case:?}: expected TagResource 204, got {tagged:?}"
    );
    let read = list_tags(client, arn).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected ListTagsForResource 200, got {read:?}"
    );
    reported(&read)
}

/// The resources of this service a tagging ARN can name: "The WorkloadArn parameter can
/// be a workload ARN, a custom lens ARN, a profile ARN, or review template ARN"
/// (`TagResource` / `ListTagsForResource` / `UntagResource` docs), plus the agent profile
/// this service also mints an ARN for.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Workload,
    ReviewTemplate,
    Profile,
    CustomLens,
    AgentProfile,
}

/// Every kind a row is run against.
const KINDS: [Kind; 5] = [
    Kind::Workload,
    Kind::ReviewTemplate,
    Kind::Profile,
    Kind::CustomLens,
    Kind::AgentProfile,
];

impl Kind {
    /// Whether a tag pair only a `TagMap` can carry can be RECORDED on this kind of
    /// resource.
    ///
    /// An agent profile also reports its tags as modelled `Tag` structures, whose `value`
    /// is `@length(min: 1, max: 256)` over a character class where a `TagMap` value is
    /// `@length(max: 256)` with no minimum and no pattern
    /// (`smithy_query(query="shape", shape="Tag,TagMap")`) — so a value only a `TagMap`
    /// carries is refused for a profile rather than stored where its own reads could only
    /// report it outside the modelled constraints. `super::agent_profile_tag_writes` pins
    /// that rule over the whole class of such values; the rows below name which of them
    /// are of that class.
    fn carries_tag_map_only_rows(self) -> bool {
        !matches!(self, Kind::AgentProfile)
    }
}

/// Mint a resource of `kind` through the operation that mints one, and answer its ARN.
/// `label` keeps the resources of one test apart, because a workload name and a review
/// template name are unique within an account and Region.
async fn resource_arn(client: &Client, kind: Kind, label: &str) -> String {
    match kind {
        Kind::Workload => {
            let created = client
                .create(minimal_create(label, &format!("{label}-create")))
                .await;
            let workload_id = created_id(&created, label);
            let read = client.get(&workload_id).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: expected GetWorkload 200, got {read:?}"
            );
            let arn = read
                .body
                .get("Workload")
                .and_then(|workload| workload.get("WorkloadArn"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.contains(":workload/"),
                "case {label:?}: expected a workload ARN, got {arn:?}"
            );
            arn
        }
        Kind::ReviewTemplate => {
            let created = client
                .create_review_template(json!({
                    "TemplateName": label,
                    "Description": "a review template under test",
                    "Lenses": ["wellarchitected"],
                    "ClientRequestToken": format!("{label}-create"),
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected CreateReviewTemplate 200, got {created:?}"
            );
            let arn = created
                .body
                .get("TemplateArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.contains(":review-template/"),
                "case {label:?}: expected a review template ARN, got {arn:?}"
            );
            arn
        }
        Kind::Profile => {
            let created = client
                .send(
                    "POST",
                    "/profiles",
                    Some(json!({
                        "ProfileName": label,
                        "ProfileDescription": "a profile under test",
                        "ProfileQuestions": [{
                            "QuestionId": template_question(0),
                            "SelectedChoiceIds": [template_choice(0, 0)],
                        }],
                        "ClientRequestToken": format!("{label}-create"),
                    })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected CreateProfile 200, got {created:?}"
            );
            let arn = created
                .body
                .get("ProfileArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.contains(":profile/"),
                "case {label:?}: expected a profile ARN, got {arn:?}"
            );
            arn
        }
        Kind::CustomLens => {
            let document =
                super::custom_lens_import::lens_document(label, None, &[("governance", 1, 2)])
                    .to_string();
            let imported = super::custom_lens_import::import(
                client,
                &document,
                None,
                &format!("{label}-create"),
                None,
            )
            .await;
            super::custom_lens_import::imported_arn(&imported, label)
        }
        Kind::AgentProfile => super::agent_goals::profile_arn_of(client, label, 1).await,
    }
}

/// One row of rule 1's table: what it is called, the tag map the write carries, and
/// whether only a `TagMap` can carry it (see [`Kind::carries_tag_map_only_rows`]).
type TagMapRow = (String, BTreeMap<String, String>, bool);

/// The tag maps rule 1 is asserted over.
///
/// Rows differ along the SHAPE of the map rather than along its values: the pair count,
/// and the key and value forms the modelled constraints admit — `TagKey` is
/// `@length(1, 128)` with `@pattern("^[\p{L}\p{N}\p{Z}_.:/=+@-]+$")` and `TagValue` is
/// `@length(max: 256)` with no minimum, so an empty value, a key of letters outside
/// ASCII and a key or value at its length bound all have to round-trip
/// (`smithy_query(query="shape", shape="TagMap")`) — on a resource whose tags are reported
/// through that map alone.
fn tag_map_rows() -> Vec<TagMapRow> {
    let mut rows: Vec<TagMapRow> = vec![
        ("one pair".to_string(), tags(&[("env", "production")]), false),
        (
            "three pairs".to_string(),
            tags(&[
                ("env", "production"),
                ("owner", "platform-team"),
                ("cost:center", "4711"),
            ]),
            false,
        ),
        (
            "an empty value, which TagValue's @length has no minimum for".to_string(),
            tags(&[("marker", "")]),
            true,
        ),
        (
            "every character class the TagKey pattern admits".to_string(),
            tags(&[(
                "Ünïcode Lëttérs 09_.:/=+@-",
                // The comma in this value is a character `Tag$value`'s pattern does not
                // admit, so only a `TagMap` carries the pair.
                "a value with spaces, punctuation and =+@-/ in it",
            )]),
            true,
        ),
    ];
    // The published bounds of the key and the value, and the map's own maximum: a value
    // AT a bound must round-trip unchanged.
    rows.push((
        "a key at TagKey's 128-character maximum".to_string(),
        [("k".repeat(128), "at the key length bound".to_string())]
            .into_iter()
            .collect(),
        false,
    ));
    rows.push((
        "a value at TagValue's 256-character maximum".to_string(),
        [("long-value".to_string(), "v".repeat(256))]
            .into_iter()
            .collect(),
        false,
    ));
    rows.push((
        format!(
            "TagMap's maximum of {} pairs",
            state::MAX_TAGS_PER_RESOURCE
        ),
        numbered_tags(0, state::MAX_TAGS_PER_RESOURCE),
        false,
    ));
    rows
}

/// `count` distinct pairs, numbered from `first` — how a row reaches a pair count
/// without spelling fifty pairs out.
fn numbered_tags(first: usize, count: usize) -> BTreeMap<String, String> {
    (first..first + count)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect()
}

/// Rule 1: what `TagResource` records, `ListTagsForResource` reports — for every kind of
/// resource a tagging ARN names and every shape of tag map.
#[tokio::test]
async fn tags_recorded_on_a_resource_are_reported_back_for_every_resource_kind() {
    for kind in KINDS {
        let client = Client::new();
        for (index, (shape, requested, tag_map_only)) in tag_map_rows().into_iter().enumerate() {
            let case = format!("{kind:?}/{shape}");
            let arn = resource_arn(&client, kind, &format!("tagged-{index}")).await;

            // Before anything is tagged the resource carries no tags: the read reports
            // the stored set, so a resource nobody tagged reports an empty one.
            let before = list_tags(&client, &arn).await;
            assert_eq!(
                before.status, 200,
                "case {case:?}: expected ListTagsForResource 200 before tagging, got {before:?}"
            );
            assert!(
                reported(&before).is_empty(),
                "case {case:?}: an untagged resource carries no tags, got {:?}",
                reported(&before)
            );

            // A pair only a `TagMap` carries is refused for a resource that also reports
            // its tags as `Tag` structures, and records nothing: the rule and the whole
            // class of such values live in `super::agent_profile_tag_writes`.
            if tag_map_only && !kind.carries_tag_map_only_rows() {
                let refused = tag(&client, &arn, &requested).await;
                assert_eq!(
                    (refused.status, refused.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: a pair this resource's own tag shape cannot carry must \
                     be refused, got {refused:?}"
                );
                let after = list_tags(&client, &arn).await;
                assert!(
                    reported(&after).is_empty(),
                    "case {case:?}: a refused tag must record nothing, got {after:?}"
                );
                continue;
            }

            let stored = tag_then_read(&client, &case, &arn, &requested).await;
            assert_eq!(
                stored, requested,
                "case {case:?}: expected the tags as sent, got {stored:?}"
            );
        }
    }
}

/// One merge row: what it is called, the tags of the first request, of the second, the
/// set the resource must end up carrying, and whether the SECOND request's tags are ones
/// only a `TagMap` carries (see [`Kind::carries_tag_map_only_rows`]).
type MergeRow = (
    &'static str,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    bool,
);

/// Rule 2: a second `TagResource` adds to what the resource carries, overwriting only
/// the keys it names. Rows differ along the overlap between the two requests.
#[tokio::test]
async fn tagging_merges_into_the_tags_a_resource_already_carries() {
    let rows: Vec<MergeRow> = vec![
        (
            "a key the resource does not carry is added",
            tags(&[("env", "production")]),
            tags(&[("owner", "platform-team")]),
            tags(&[("env", "production"), ("owner", "platform-team")]),
            false,
        ),
        (
            "a key the resource carries takes the new value",
            tags(&[("env", "production"), ("owner", "platform-team")]),
            tags(&[("env", "staging")]),
            tags(&[("env", "staging"), ("owner", "platform-team")]),
            false,
        ),
        (
            "the same pair again changes nothing",
            tags(&[("env", "production")]),
            tags(&[("env", "production")]),
            tags(&[("env", "production")]),
            false,
        ),
        (
            "several keys at once, some new and some overwritten",
            tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            tags(&[("b", "9"), ("d", "4")]),
            tags(&[("a", "1"), ("b", "9"), ("c", "3"), ("d", "4")]),
            false,
        ),
        (
            "an existing key overwritten with an empty value",
            tags(&[("env", "production")]),
            tags(&[("env", "")]),
            tags(&[("env", "")]),
            true,
        ),
    ];

    for kind in KINDS {
        let client = Client::new();
        for (index, (shape, first, second, expected, tag_map_only)) in rows.iter().enumerate() {
            let case = format!("{kind:?}/{shape}");
            let arn = resource_arn(&client, kind, &format!("merged-{index}")).await;

            let after_first = tag_then_read(&client, &case, &arn, first).await;
            assert_eq!(
                &after_first, first,
                "case {case:?}: expected the first request's tags, got {after_first:?}"
            );

            // A second request whose pairs only a `TagMap` carries is refused for a
            // resource that also reports its tags as `Tag` structures, and leaves the tags
            // the first request recorded exactly as they were.
            if *tag_map_only && !kind.carries_tag_map_only_rows() {
                let refused = tag(&client, &arn, second).await;
                assert_eq!(
                    (refused.status, refused.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: a pair this resource's own tag shape cannot carry must \
                     be refused, got {refused:?}"
                );
                let after = list_tags(&client, &arn).await;
                assert_eq!(
                    &reported(&after),
                    first,
                    "case {case:?}: a refused merge must change nothing, got {after:?}"
                );
                continue;
            }

            let after_second = tag_then_read(&client, &case, &arn, second).await;
            assert_eq!(
                &after_second, expected,
                "case {case:?}: expected the merged tags, got {after_second:?}"
            );
        }
    }
}

/// Rule 3: the ARN is resolved against the store, and a miss is the shared tagging
/// frontend's 404 `NotFoundException` — its fixed sentence, and nothing of the request
/// anywhere in the body.
///
/// The code and the sentence are spelled out in `super::resource_tags` rather than read
/// back out of the code under test.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_a_not_found_and_records_nothing() {
    let client = Client::new();
    // One resource of each kind exists, so every row below is a real miss rather than an
    // empty store — and the last assertion can show the miss wrote nothing.
    let mut present: Vec<(Kind, String)> = Vec::new();
    for kind in KINDS {
        let arn = resource_arn(&client, kind, &format!("present-{}", present.len())).await;
        let stored = tag_then_read(
            &client,
            &format!("{kind:?}/present"),
            &arn,
            &tags(&[("kept", "yes")]),
        )
        .await;
        assert_eq!(
            stored,
            tags(&[("kept", "yes")]),
            "case {kind:?}/present: expected the resource to be tagged, got {stored:?}"
        );
        present.push((kind, arn));
    }

    let account = engine::emulator_account_id();
    let absent_id = "0123456789abcdef0123456789abcdef";
    let workload_arn = present
        .iter()
        .find(|(kind, _)| matches!(kind, Kind::Workload))
        .map(|(_, arn)| arn.clone())
        .unwrap_or_default();

    let rows: Vec<(&str, String)> = vec![
        (
            "a workload ARN no workload carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:workload/{absent_id}"),
        ),
        (
            "a review template ARN no template carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:review-template/{absent_id}"),
        ),
        (
            "a custom lens ARN no lens carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:lens/{absent_id}"),
        ),
        (
            "an agent profile ARN no profile carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:agent-profile/{absent_id}"),
        ),
        (
            "a profile ARN no profile carries",
            format!("arn:aws:wellarchitected:us-west-2:{account}:profile/{absent_id}"),
        ),
        (
            "a stored workload's ARN under another account",
            workload_arn.replace(account, "210987654321"),
        ),
        (
            "a stored workload's ARN in another Region",
            workload_arn.replace("us-west-2", "eu-west-1"),
        ),
        (
            "an Amazon Web Services official lens ARN, which names no account's resource",
            "arn:aws:wellarchitected:us-west-2::lens/serverless".to_string(),
        ),
        // A value whose resource part names no taggable kind of this service, and one that
        // is not an ARN at all, name no resource either and are answered the same way.
        (
            "an ARN of another service entirely",
            format!("arn:aws:s3:::a-bucket/{absent_id}"),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (shape, arn) in rows {
        assert!(
            !arn.is_empty() && arn != workload_arn,
            "case {shape:?}: the row must not name the resource that exists, got {arn:?}"
        );
        for (operation, answer) in [
            ("TagResource", tag(&client, &arn, &tags(&[("k", "v")])).await),
            ("ListTagsForResource", list_tags(&client, &arn).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {shape:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, TAGGING_NOT_FOUND_CODE,
                "case {shape:?}/{operation}: expected {TAGGING_NOT_FOUND_CODE}, got {answer:?}"
            );
            assert_eq!(
                answer.body.get("Message").and_then(Value::as_str),
                Some(TAGGING_NOT_FOUND_MESSAGE),
                "case {shape:?}/{operation}: the message is the frontend's fixed text, got \
                 {answer:?}"
            );
            assert!(
                !answer.body.to_string().contains(arn.as_str()),
                "case {shape:?}/{operation}: the refusal must disclose nothing the request \
                 sent, got {answer:?}"
            );
        }
    }

    // A refused request records nothing: every resource still carries exactly the tag it
    // was given above.
    for (kind, arn) in &present {
        let read = list_tags(&client, arn).await;
        assert_eq!(
            reported(&read),
            tags(&[("kept", "yes")]),
            "case {kind:?}/after-the-misses: a refused tag must change nothing, got {read:?}"
        );
    }
}

/// Rule 4: the stored tag count is bounded by `TagMap`'s own maximum — asserted AT the
/// bound, one past it, and at the bound again through an overwrite.
#[tokio::test]
async fn a_request_that_would_push_the_resource_past_the_tag_maximum_is_refused() {
    let maximum = state::MAX_TAGS_PER_RESOURCE;
    for kind in KINDS {
        let client = Client::new();
        let case = format!("{kind:?}/tag-ceiling");
        let arn = resource_arn(&client, kind, "ceiling").await;

        // At the bound: the maximum in one request is recorded.
        let full = numbered_tags(0, maximum);
        let stored = tag_then_read(&client, &case, &arn, &full).await;
        assert_eq!(
            stored.len(),
            maximum,
            "case {case:?}: expected {maximum} stored tags, got {}",
            stored.len()
        );

        // One past it: a single NEW key would make the resource hold one more than the
        // maximum, so the request is refused and nothing changes.
        let refused = tag(&client, &arn, &numbered_tags(maximum, 1)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 past the tag maximum, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException past the tag maximum, got {refused:?}"
        );
        let after_refusal = list_tags(&client, &arn).await;
        assert_eq!(
            reported(&after_refusal),
            full,
            "case {case:?}: a refused tag must change nothing, got {after_refusal:?}"
        );

        // At the bound again: a key the resource already carries keeps the count where it
        // is, so the overwrite is recorded.
        let overwrite: BTreeMap<String, String> = [("key-000".to_string(), "rewritten".to_string())]
            .into_iter()
            .collect();
        let stored = tag_then_read(&client, &case, &arn, &overwrite).await;
        assert_eq!(
            stored.len(),
            maximum,
            "case {case:?}: an overwrite keeps the tag count at {maximum}, got {}",
            stored.len()
        );
        assert_eq!(
            stored.get("key-000").map(String::as_str),
            Some("rewritten"),
            "case {case:?}: the overwritten key takes the new value, got {stored:?}"
        );
    }
}

/// Rule 5: a resource's tags are its own, and reading them changes nothing — the same
/// read repeated, and every other resource's tags, answer exactly as before.
#[tokio::test]
async fn tags_belong_to_one_resource_and_reading_them_changes_nothing() {
    let client = Client::new();

    let mut resources: Vec<(Kind, String, BTreeMap<String, String>)> = Vec::new();
    for (index, kind) in KINDS.into_iter().enumerate() {
        let arn = resource_arn(&client, kind, &format!("isolated-{index}")).await;
        let expected = numbered_tags(index * 10, index + 1);
        let stored = tag_then_read(&client, &format!("{kind:?}/isolated"), &arn, &expected).await;
        assert_eq!(
            stored, expected,
            "case {kind:?}/isolated: expected the tags as sent, got {stored:?}"
        );
        resources.push((kind, arn, expected));
    }

    // Three reads of every resource, interleaved: a read reports the stored set and
    // writes nothing, so the answers are identical and no resource sees another's tags.
    for round in 0..3 {
        for (kind, arn, expected) in &resources {
            let read = list_tags(&client, arn).await;
            assert_eq!(
                read.status, 200,
                "case {kind:?}/round-{round}: expected ListTagsForResource 200, got {read:?}"
            );
            assert_eq!(
                &reported(&read),
                expected,
                "case {kind:?}/round-{round}: a read reports the stored tags unchanged, got \
                 {read:?}"
            );
        }
    }
}

/// Rule 6: the WRITE and the READ resolve the SAME ARNs.
///
/// The two sides are separate accessors on the store (`resource_tags_mut` for the write,
/// `resource_tags` for the read), so this pins them against each other over every
/// taggable kind: the ARN a write lands on is the ARN a read reports, and an ARN that
/// resolves to nothing is refused the same way by both. Rows differ along the kind of
/// resource and along HOW the ARN misses — a resource id nothing carries, another
/// account, another Region.
#[tokio::test]
async fn the_tag_write_and_the_tag_read_resolve_the_same_arns() {
    let client = Client::new();
    let account = engine::emulator_account_id();

    for (index, kind) in KINDS.into_iter().enumerate() {
        let arn = resource_arn(&client, kind, &format!("agreed-{index}")).await;
        let requested = numbered_tags(index * 100, 2);

        // The ARN that RESOLVES: the write records, and the read of the same ARN reports
        // exactly what was recorded, so both accessors found the same resource.
        let written = tag(&client, &arn, &requested).await;
        let read = list_tags(&client, &arn).await;
        assert_eq!(
            (written.status, read.status),
            (204, 200),
            "case {kind:?}: write and read must both resolve {arn:?}, got {written:?} / {read:?}"
        );
        assert_eq!(
            reported(&read),
            requested,
            "case {kind:?}: the read reports what the write recorded, got {read:?}"
        );

        // The ARNs that MISS: each is the resolving ARN with one segment changed, so the
        // row differs from the hit only in what the resolver has to reject.
        for (miss, absent) in [
            ("another account", arn.replace(account, "210987654321")),
            ("another Region", arn.replace("us-west-2", "eu-west-1")),
            (
                "a resource id nothing carries",
                format!("{arn}-no-such-resource"),
            ),
        ] {
            let written = tag(&client, &absent, &requested).await;
            let read = list_tags(&client, &absent).await;
            assert_eq!(
                (written.status, read.status),
                (404, 404),
                "case {kind:?}/{miss}: write and read must both refuse {absent:?}, got \
                 {written:?} / {read:?}"
            );
            assert_eq!(
                (written.error_code.as_str(), read.error_code.as_str()),
                (TAGGING_NOT_FOUND_CODE, TAGGING_NOT_FOUND_CODE),
                "case {kind:?}/{miss}: both refusals are the tagging frontend's not-found, \
                 got {written:?} / {read:?}"
            );
            assert_eq!(
                written.body.get("Message"),
                read.body.get("Message"),
                "case {kind:?}/{miss}: both refusals state the same sentence, got \
                 {written:?} / {read:?}"
            );
        }

        // The resolving ARN still carries exactly what the write recorded: the misses
        // above wrote nothing.
        let read = list_tags(&client, &arn).await;
        assert_eq!(
            reported(&read),
            requested,
            "case {kind:?}: a refused write must change nothing, got {read:?}"
        );
    }
}
