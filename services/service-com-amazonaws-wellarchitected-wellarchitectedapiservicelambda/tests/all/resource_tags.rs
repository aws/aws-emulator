// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `ListTagsForResource` reports exactly the tag map PERSISTED on the resource its ARN
//!    names. The rows differ along the two things the rule generalises over: the resource
//!    TYPE the ARN names — "a workload ARN, a custom lens ARN, a profile ARN, or review
//!    template ARN" (the operation's doc), plus the agent profile this service's model
//!    also tags (`CreateAgentProfile$tags`) — and the tag map itself: none, one, keys
//!    carrying the punctuation and the non-ASCII letters `TagKey`'s pattern allows, an
//!    empty value, the longest key and value the published bounds allow, and the 50 tags
//!    `TagMap`'s `@length(1, 50)` tops out at.
//! 2. An ARN that names no resource this store holds is a `404
//!    ResourceNotFoundException` carrying the ARN AS SENT and the resource type its ARN
//!    names — one row per taggable type, plus a string that is not an ARN at all.
//! 3. The tags reported FOLLOW the stored resource: a second `ImportLens` that associates
//!    another tag with a lens changes what the next list reports, and listing is
//!    reporting-only — two lists in a row agree, and the resource's own read reports the
//!    same tags afterwards.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`; the lens document and the `ImportLens` wire helper are
//! `super::custom_lens_import`'s.
//!
//! The taggable-resource table and the wire helpers below (`Kind`, `KINDS`,
//! `create_tagged`, `list_tags`, `tag_map`, `tags_of`, `reported_tags`, `created_arn`) are
//! `pub(crate)` because `super::untag_resource` is about the same resources and the same
//! tag maps, and the two suites must not disagree about how one is created or read back.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::agent_goals::create_profile as create_agent_profile;
use super::custom_lens_import::{import, lens_document};
use super::workload_harness::{
    encode_path_segment, minimal_create, minimal_template, template_choice, template_question,
    Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The published bounds of one tag, from the model:
/// `TagKey` is `@length(1, 128)` with `@pattern("^[\p{L}\p{N}\p{Z}_.:/=+@-]+$")` and
/// `TagValue` is `@length(max: 256)`
/// (`smithy_query(query="shape", shape="TagKey,TagValue")`).
const MAX_TAG_KEY: usize = 128;
/// See [`MAX_TAG_KEY`].
const MAX_TAG_VALUE: usize = 256;

/// The wire error code the TAG family answers an ARN that names no resource with.
///
/// Not the `ResourceNotFoundException` this service's model declares for these three
/// operations and the rest of the service answers: the ARN-addressed tag operations are
/// served by the shared tagging frontend, whose own error shape this is. Spelled out here
/// rather than read off the implementation, so the tag suites pin the code itself.
pub(crate) const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The whole `Message` that refusal carries: the fixed text of that failure class, with no
/// variable part — no ARN, no account id and no bracketed class tag. Spelled out here for
/// the same reason as the code above.
pub(crate) const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// `GET /tags/{WorkloadArn}`, the wire form of `ListTagsForResource`. The ARN is a
/// non-greedy `@httpLabel`, so it travels percent-encoded in ONE path segment, exactly as
/// an SDK sends it.
pub(crate) async fn list_tags(client: &Client, resource_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!("/tags/{}", encode_path_segment(resource_arn)),
            None,
        )
        .await
}

/// The resource types a tag ARN can name: the four the operation's doc lists, plus the
/// agent profile, the fifth resource of this service whose create takes tags
/// (`smithy_query(query="operation", operation="CreateAgentProfile")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Workload,
    CustomLens,
    Profile,
    ReviewTemplate,
    AgentProfile,
}

pub(crate) const KINDS: [Kind; 5] = [
    Kind::Workload,
    Kind::CustomLens,
    Kind::Profile,
    Kind::ReviewTemplate,
    Kind::AgentProfile,
];

impl Kind {
    /// Whether a create of this kind can record the tags of a `tag_map_only` row. An
    /// agent profile's tags are a `Tags` LIST of `Tag`, whose `value` is
    /// `@length(1, 256)` with a pattern (`smithy_query(query="shape", shape="Tag")`),
    /// where a `TagMap` value is `@length(max: 256)` with no pattern and no minimum — so
    /// an empty value is a tag only a `TagMap` carries, and boundary validation refuses it
    /// for an agent profile before any handler runs.
    pub(crate) fn carries_tag_map_only_rows(self) -> bool {
        self != Kind::AgentProfile
    }
}

/// A tag map as a `TagMap` travels the wire.
pub(crate) fn tag_map(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The same tags as the `Tags` LIST of `Tag` structures an agent profile takes.
fn tag_list(tags: &BTreeMap<String, String>) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

pub(crate) fn tags_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the tag table: what it is called, the tags the create records, and whether
/// only a `TagMap` can carry them (see [`Kind::carries_tag_map_only_rows`]).
struct TagCase {
    case: &'static str,
    tags: BTreeMap<String, String>,
    tag_map_only: bool,
}

/// The tag maps every taggable resource type is tested with. They differ along what the
/// rule generalises over — the COUNT of tags and the FORM of each key and value — rather
/// than along one recorded example.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no tags at all",
            tags: BTreeMap::new(),
            tag_map_only: false,
        },
        TagCase {
            case: "one tag",
            tags: tags_of(&[("Environment", "Production")]),
            tag_map_only: false,
        },
        TagCase {
            // Every punctuation character `TagKey`'s pattern allows beside letters,
            // digits and separators.
            case: "punctuation and spaces in the key",
            tags: tags_of(&[
                ("cost.center:team/1=2+3@4-5_6", "owned by platform"),
                ("Zone One", "us west"),
            ]),
            tag_map_only: false,
        },
        TagCase {
            // `\p{L}` and `\p{N}` are not ASCII-only.
            case: "non-ASCII letters and digits",
            tags: tags_of(&[("Ωμέγα", "Δοκιμή"), ("данные1", "значение")]),
            tag_map_only: false,
        },
        TagCase {
            case: "an empty value",
            tags: tags_of(&[("NoValue", "")]),
            tag_map_only: true,
        },
        TagCase {
            case: "the longest key and value",
            tags: [("k".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE))]
                .into_iter()
                .collect(),
            tag_map_only: false,
        },
        TagCase {
            case: "the most tags a TagMap carries",
            tags: (0..state::MAX_TAGS_PER_RESOURCE)
                .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
                .collect(),
            tag_map_only: false,
        },
    ]
}

/// The ARN a create answered with, under the member that carries it for that kind.
pub(crate) fn created_arn(answer: &Answer, member: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the create to answer 200, got {answer:?}"
    );
    let arn = answer
        .body
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with("arn:"),
        "case {case:?}: expected the create to answer an ARN in {member}, got {answer:?}"
    );
    arn
}

/// Create a resource of `kind` carrying `tags` and answer its ARN. `slug` keys the names
/// and idempotency tokens apart, so two rows never collide on a unique name.
pub(crate) async fn create_tagged(
    client: &Client,
    kind: Kind,
    case: &str,
    slug: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    // `TagMap` is `@length(min: 1)`, so a row with no tags omits the member rather than
    // sending an empty map.
    let tag_member = (!tags.is_empty()).then(|| tag_map(tags));
    match kind {
        Kind::Workload => {
            let mut body = minimal_create(&format!("workload-{slug}"), &format!("tok-wl-{slug}"));
            if let Some(value) = tag_member {
                body["Tags"] = value;
            }
            created_arn(&client.create(body).await, "WorkloadArn", case)
        }
        Kind::ReviewTemplate => {
            let mut body = minimal_template(&format!("template-{slug}"), &format!("tok-rt-{slug}"));
            if let Some(value) = tag_member {
                body["Tags"] = value;
            }
            created_arn(
                &client.create_review_template(body).await,
                "TemplateArn",
                case,
            )
        }
        Kind::Profile => {
            let mut body = json!({
                "ProfileName": format!("profile-{slug}"),
                "ProfileDescription": "a profile under test",
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [template_choice(0, 0)],
                }],
                "ClientRequestToken": format!("tok-pr-{slug}"),
            });
            if let Some(value) = tag_member {
                body["Tags"] = value;
            }
            created_arn(&client.create_profile(body).await, "ProfileArn", case)
        }
        Kind::CustomLens => {
            let document =
                lens_document(&format!("Lens {slug}"), None, &[("governance", 1, 2)]).to_string();
            let answer = import(
                client,
                &document,
                None,
                &format!("tok-lens-{slug}"),
                tag_member,
            )
            .await;
            created_arn(&answer, "LensArn", case)
        }
        Kind::AgentProfile => {
            let mut body = json!({
                "name": format!("agent-{slug}"),
                "pillars": ["SECURITY"],
                "executionRoleArn": format!(
                    "arn:aws:iam::{}:role/AgentProfileExecution",
                    engine::emulator_account_id()
                ),
                "aggregationConfiguration": [{
                    "accountId": engine::emulator_account_id(),
                    "regions": ["us-east-1"],
                    "accessRoleArn": format!(
                        "arn:aws:iam::{}:role/AgentProfileAccess",
                        engine::emulator_account_id()
                    ),
                }],
                "clientToken": format!("tok-ap-{slug}"),
            });
            if !tags.is_empty() {
                body["tags"] = tag_list(tags);
            }
            // `POST /api/v1/agent-profiles` through the agent-profile suite's own wire
            // helper, so there is one spelling of the route in this crate's tests.
            created_arn(&create_agent_profile(client, body).await, "arn", case)
        }
    }
}

/// The tags a successful `ListTagsForResource` reported, as a map.
pub(crate) fn reported_tags(answer: &Answer, case: &str) -> BTreeMap<String, String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListTagsForResource 200, got {answer:?}"
    );
    let Some(tags) = answer.body.get("Tags").and_then(Value::as_object) else {
        panic!("case {case:?}: expected a Tags map in the response, got {answer:?}");
    };
    tags.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Rule 1: every taggable resource type reports the tag map it was created with, for every
/// shape of tag map.
#[tokio::test]
async fn every_taggable_resource_reports_the_tags_it_carries() {
    let client = Client::new();

    for (row, tag_case) in tag_cases().into_iter().enumerate() {
        for (column, kind) in KINDS.into_iter().enumerate() {
            if tag_case.tag_map_only && !kind.carries_tag_map_only_rows() {
                continue;
            }
            let case = format!("{} / {kind:?}", tag_case.case);
            let slug = format!("{row}{column}");
            let arn = create_tagged(&client, kind, &case, &slug, &tag_case.tags).await;

            let listed = list_tags(&client, &arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                tag_case.tags,
                "case {case:?}: ListTagsForResource must report the tags the resource \
                 carries, got {listed:?}"
            );
        }
    }
}

/// Rule 2: an ARN naming no stored resource is the tagging frontend's 404
/// `NotFoundException`, stating that frontend's fixed sentence and nothing the request
/// sent. One row per taggable type, plus a value that is not an ARN at all — including the
/// bare lens id of a lens that DOES exist, which is not an ARN and so names no resource
/// here.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_a_not_found() {
    let client = Client::new();
    let account = engine::emulator_account_id();

    // One resource of every kind exists, so each row below is a real miss rather than an
    // empty store.
    let mut present: Vec<(Kind, String)> = Vec::new();
    for (column, kind) in KINDS.into_iter().enumerate() {
        let case = format!("present / {kind:?}");
        let arn = create_tagged(
            &client,
            kind,
            &case,
            &format!("p{column}"),
            &tags_of(&[("Present", "yes")]),
        )
        .await;
        present.push((kind, arn));
    }

    let absent_id = "0123456789abcdef0123456789abcdef";
    let mut rows: Vec<(&str, String)> = vec![
        (
            "absent workload",
            format!("arn:aws:wellarchitected:us-west-2:{account}:workload/{absent_id}"),
        ),
        (
            "absent custom lens",
            format!("arn:aws:wellarchitected:us-west-2:{account}:lens/{absent_id}"),
        ),
        (
            "absent profile",
            format!("arn:aws:wellarchitected:us-west-2:{account}:profile/{absent_id}"),
        ),
        (
            "absent review template",
            format!("arn:aws:wellarchitected:us-west-2:{account}:review-template/{absent_id}"),
        ),
        (
            "absent agent profile",
            format!("arn:aws:wellarchitected:us-west-2:{account}:agent-profile/{absent_id}"),
        ),
        (
            // A stored resource of ANOTHER account is a different resource, however much
            // of the ARN agrees.
            "another account's workload",
            format!("arn:aws:wellarchitected:us-west-2:210987654321:workload/{absent_id}"),
        ),
        (
            // A value the frontend cannot read as an ARN names no resource either, and is
            // answered the same way: the sentence says nothing about what was sent.
            "not an ARN at all",
            "not-an-arn".to_string(),
        ),
    ];
    // The bare lens id of the lens that exists: a lens is named by its ARN, so its id on
    // its own names no resource to report the tags of.
    let lens_arn = present
        .iter()
        .find(|(kind, _)| *kind == Kind::CustomLens)
        .map(|(_, arn)| arn.clone())
        .expect("the suite created a custom lens");
    let bare_lens_id = lens_arn
        .rsplit_once("lens/")
        .map(|(_, id)| id.to_string())
        .expect("a custom lens ARN ends in lens/<id>");
    rows.push(("a bare lens id, not an ARN", bare_lens_id));

    for (case, resource_arn) in rows {
        let answer = list_tags(&client, &resource_arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, TAGGING_NOT_FOUND_CODE,
            "case {case:?}: expected {TAGGING_NOT_FOUND_CODE}, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(TAGGING_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the frontend's fixed sentence, got {answer:?}"
        );
        // The sentence, and the body around it, say nothing about what was asked for.
        assert!(
            !answer.body.to_string().contains(resource_arn.as_str()),
            "case {case:?}: the refusal must disclose nothing the request sent, got {answer:?}"
        );
    }

    // Every resource that DOES exist still resolves, so the rows above failed on
    // resolution and not on a broken route.
    for (kind, arn) in present {
        let case = format!("present / {kind:?}");
        let answer = list_tags(&client, &arn).await;
        assert_eq!(
            reported_tags(&answer, &case),
            tags_of(&[("Present", "yes")]),
            "case {case:?}: the stored resource must still report its tags, got {answer:?}"
        );
    }
}

/// Rule 3: the list FOLLOWS the stored resource. A second `ImportLens` under the lens's
/// own ARN associates another tag with it — "Tags to associate to a lens" — and the next
/// list reports the lens's tags as they now stand, so the answer is read from state rather
/// than from the create that happened to carry them.
#[tokio::test]
async fn the_tag_list_follows_the_stored_resource() {
    let client = Client::new();
    let document = lens_document("Tagged lens", None, &[("governance", 1, 2)]).to_string();

    let first = import(
        &client,
        &document,
        None,
        "tok-lens-tags-1",
        Some(tag_map(&tags_of(&[("Team", "platform")]))),
    )
    .await;
    let lens_arn = created_arn(&first, "LensArn", "imported");
    assert_eq!(
        reported_tags(&list_tags(&client, &lens_arn).await, "imported"),
        tags_of(&[("Team", "platform")]),
        "case \"imported\": the import's tags must be what the lens reports"
    );

    let updated = import(
        &client,
        &document,
        Some(&lens_arn),
        "tok-lens-tags-2",
        Some(tag_map(&tags_of(&[("Tier", "gold")]))),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case \"re-imported\": the update must answer 200, got {updated:?}"
    );
    assert_eq!(
        reported_tags(&list_tags(&client, &lens_arn).await, "re-imported"),
        tags_of(&[("Team", "platform"), ("Tier", "gold")]),
        "case \"re-imported\": the list must report the tags the lens now carries"
    );
}

/// Rule 3, the reporting half: listing tags changes nothing a later caller sees. Two lists
/// in a row agree, and the resource's own read reports the same tags after them.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let tags = tags_of(&[("Environment", "Production"), ("Owner", "platform")]);

    let mut body = minimal_create("reporting-only", "tok-reporting-only");
    body["Tags"] = tag_map(&tags);
    let created = client.create(body).await;
    let workload_arn = created_arn(&created, "WorkloadArn", "reporting-only");
    let workload_id = created
        .body
        .get("WorkloadId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let first = reported_tags(&list_tags(&client, &workload_arn).await, "first list");
    let second = reported_tags(&list_tags(&client, &workload_arn).await, "second list");
    assert_eq!(
        first, second,
        "case \"repeat list\": two lists in a row must report the same tags"
    );
    assert_eq!(
        first, tags,
        "case \"first list\": the list must report the workload's tags"
    );

    let fetched = client.get(&workload_id).await;
    let reported: BTreeMap<String, String> = fetched
        .body
        .get("Workload")
        .and_then(|workload| workload.get("Tags"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, tags,
        "case \"read back\": GetWorkload must still report the same tags, got {fetched:?}"
    );
}

/// `TagMap`'s `@length(1, 50)` bound, on both sides: a create carrying the 50 tags it
/// tops out at records them all, and one carrying 51 is refused 400 and records nothing.
#[tokio::test]
async fn a_tag_map_past_its_published_bound_is_refused() {
    let client = Client::new();

    let at_bound: BTreeMap<String, String> = (0..state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let arn = create_tagged(&client, Kind::Workload, "at the bound", "bound", &at_bound).await;
    assert_eq!(
        reported_tags(&list_tags(&client, &arn).await, "at the bound").len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case \"at the bound\": a resource carries up to {} tags",
        state::MAX_TAGS_PER_RESOURCE
    );

    let past_bound: BTreeMap<String, String> = (0..=state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let mut body = minimal_create("past-the-bound", "tok-past-the-bound");
    body["Tags"] = tag_map(&past_bound);
    let refused = client.create(body).await;
    assert_eq!(
        refused.status, 400,
        "case \"past the bound\": a TagMap of {} tags must be refused 400, got {refused:?}",
        past_bound.len()
    );
    assert!(
        client
            .store()
            .lock()
            .workloads
            .values()
            .all(|workload| workload.workload_name != "past-the-bound"),
        "case \"past the bound\": the refused create must record nothing"
    );
}
