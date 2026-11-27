// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Network Flow Monitor tag plane — `TagResource`,
//! `UntagResource` and `ListTagsForResource` — driven over the service's own
//! `restJson1` wire protocol through the same `MultiServiceRouter` the production
//! emulator dispatches with.
//!
//! The rule under test: the three tagging operations address EVERY taggable
//! resource of this service through one `resourceArn`, `ListTagsForResource`
//! answers exactly the tag set the store holds for that resource, `TagResource`
//! adds or replaces only the keys it names, `UntagResource` removes only the keys
//! it names, a successful mutation answers 204 (it reports no content), and an ARN
//! the store does not hold is the modelled 404 for all three.
//! Every test asserts that over a SET of inputs — every taggable resource type
//! (scope, monitor, performance monitor) and tag sets differing in count, key and
//! value — so no row is privileged.
//!
//! The harness (`Emulator`, `Answer`, `targets`, `client_token`) is the one
//! `tests/all/harness.rs` owns and every suite in this crate dispatches through;
//! only the tag-plane calls are added here.

use serde_json::{json, Map, Value};

use crate::harness::{
    client_token, extra_region, targets, Answer, Emulator, ACCOUNT_ID, EXTRA_REGIONS, REGION,
    SIGNING_NAME,
};

/// `TagKey`'s own `@length` maximum, from the model
/// (`smithy_query(query="shape",shape="TagMap")`: key target `TagKey`, len 1..128).
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue`'s own `@length` maximum, from the same shape (len 0..256).
const MAX_TAG_VALUE_LEN: usize = 256;

/// The status a successful tag mutation answers with: its output declares no
/// members, so there is no content to report (`state::NO_CONTENT_SUCCESS_STATUS`).
const NO_CONTENT: u16 = 204;

/// `TagMap`'s own `@length` maximum (0..200): the most tags one resource can carry,
/// and so the most any one request may leave it carrying.
const MAX_TAGS: usize = 200;

/// The `/tags/{resourceArn}` path for an ARN. `resourceArn` is an `@httpLabel` on a
/// NON-greedy label, so an SDK percent-encodes the ARN's `:` and `/` — this is what
/// the service sees on the wire.
pub(crate) fn tags_path(arn: &str) -> String {
    let encoded: String = arn
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect();
    format!("/tags/{encoded}")
}

/// The `/tags/{resourceArn}?tagKeys=...` path `UntagResource` is called on:
/// `tagKeys` is an `@httpQuery` list, so each key is its own repeated parameter.
pub(crate) fn untag_path(arn: &str, keys: &[&str]) -> String {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={key}")).collect();
    if query.is_empty() {
        tags_path(arn)
    } else {
        format!("{}?{}", tags_path(arn), query.join("&"))
    }
}

impl Emulator {
    pub(crate) async fn list_tags(&self, arn: &str) -> Answer {
        self.call("GET", &tags_path(arn), None).await
    }

    pub(crate) async fn tag_resource(&self, arn: &str, tags: &Value) -> Answer {
        self.call("POST", &tags_path(arn), Some(json!({ "tags": tags })))
            .await
    }

    pub(crate) async fn untag_resource(&self, arn: &str, keys: &[&str]) -> Answer {
        self.call("DELETE", &untag_path(arn, keys), None).await
    }
}

/// A created, taggable resource: the ARN the tagging operations address it by, and
/// how to read the record back through the reader that already serves it.
pub(crate) enum Resource {
    Scope { arn: String, scope_id: String },
    Monitor { arn: String, name: String },
    PerformanceMonitor { arn: String, name: String },
}

impl Resource {
    pub(crate) fn arn(&self) -> &str {
        match self {
            Self::Scope { arn, .. }
            | Self::Monitor { arn, .. }
            | Self::PerformanceMonitor { arn, .. } => arn.as_str(),
        }
    }

    /// The record itself, through `GetScope`/`GetMonitor`/`GetPerformanceMonitor` —
    /// so a tag write is proved to have landed on the resource and not in a store
    /// of its own.
    pub(crate) async fn read(&self, emulator: &Emulator) -> Answer {
        match self {
            Self::Scope { scope_id, .. } => emulator.get_scope(scope_id).await,
            Self::Monitor { name, .. } => emulator.get_monitor(name).await,
            Self::PerformanceMonitor { name, .. } => {
                emulator.get_performance_monitor(name).await
            }
        }
    }
}

/// The taggable resource types this service mints, as a table dimension: every
/// rule below holds for all of them, because all of them are reached through
/// `resourceArn`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    Scope,
    Monitor,
    PerformanceMonitor,
}

pub(crate) const KINDS: &[Kind] = &[Kind::Scope, Kind::Monitor, Kind::PerformanceMonitor];

/// Create a resource of `kind` carrying `tags` (absent when `None`), and answer
/// with what addresses and reads it.
///
/// A scope is signed for the Region `seed` names ([`extra_region`]): the account
/// holds one scope per Region, so distinct seeds give one store distinct scopes.
pub(crate) async fn create(
    emulator: &Emulator,
    kind: Kind,
    seed: u32,
    tags: Option<&Value>,
) -> Resource {
    match kind {
        Kind::Scope => {
            let mut body = json!({
                "targets": targets(&[("123456789012", "us-east-1")]),
                "clientToken": client_token(seed),
            });
            if let Some(tags) = tags {
                body["tags"] = tags.clone();
            }
            let created = emulator
                .create_scope_in(extra_region(seed as usize % EXTRA_REGIONS.len()), body)
                .await;
            assert_eq!(
                created.status, 200,
                "case scope/{seed}: the scope must be created, got {} with body {}",
                created.status, created.body
            );
            Resource::Scope {
                arn: created.text("scopeArn"),
                scope_id: created.text("scopeId"),
            }
        }
        Kind::Monitor => {
            let scope_arn = emulator.scope_for_monitors().await;
            let name = format!("monitor-{seed}");
            let mut body = json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": format!("vpc-{seed}") }],
                "clientToken": client_token(seed),
            });
            if let Some(tags) = tags {
                body["tags"] = tags.clone();
            }
            let created = emulator.create_monitor(body).await;
            assert_eq!(
                created.status, 200,
                "case monitor/{seed}: the monitor must be created, got {} with body {}",
                created.status, created.body
            );
            Resource::Monitor {
                arn: created.text("monitorArn"),
                name,
            }
        }
        Kind::PerformanceMonitor => {
            // A performance monitor names NO scope: `CreatePerformanceMonitor`'s
            // `scopeArn` is optional, so this resource needs no parent to be
            // taggable.
            let name = format!("performance-monitor-{seed}");
            let mut body = json!({
                "monitorName": name,
                "targets": [{
                    "type": "AWS::EC2::VPC",
                    "identifier": format!(
                        "arn:aws:ec2:{REGION}:123456789012:vpc/vpc-{seed:017x}"
                    ),
                }],
                "clientToken": client_token(seed),
            });
            if let Some(tags) = tags {
                body["tags"] = tags.clone();
            }
            let created = emulator.create_performance_monitor(body).await;
            assert_eq!(
                created.status, 201,
                "case performance-monitor/{seed}: the monitor must be created, got {} with \
                 body {}",
                created.status, created.body
            );
            Resource::PerformanceMonitor {
                arn: created.text("monitorArn"),
                name,
            }
        }
    }
}

/// A tag map of `count` distinct keys, for the count dimension of the tables.
fn many_tags(count: usize, value: &str) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index:03}"), Value::String(value.to_string()));
    }
    Value::Object(map)
}

/// One tag set the tables span, differing from the others in count, key form or
/// value form.
fn tag_sets() -> Vec<(&'static str, Value)> {
    let mut longest = Map::new();
    // The published bounds of `TagKey` (1..128) and `TagValue` (0..256), exactly.
    longest.insert(
        "k".repeat(MAX_TAG_KEY_LEN),
        Value::String("v".repeat(MAX_TAG_VALUE_LEN)),
    );
    vec![
        ("no tags at all", json!({})),
        ("one tag", json!({ "env": "prod" })),
        (
            "three tags",
            json!({ "owner": "net-team", "env": "prod", "cost-center": "4711" }),
        ),
        // `TagValue`'s minimum length is 0, so an empty value is a legal tag.
        ("a one-character key and an empty value", json!({ "k": "" })),
        (
            "the longest key and value TagKey/TagValue allow",
            Value::Object(longest),
        ),
        ("25 tags", many_tags(25, "bulk")),
        ("the 200 tags TagMap allows at most", many_tags(MAX_TAGS, "x")),
    ]
}

/// The `tags` member of a listing, as a map — a listing always carries the member,
/// so a resource with no tags answers `{}`.
fn listed(answer: &Answer) -> Value {
    answer.member("tags").clone()
}

#[tokio::test]
async fn tags_set_at_creation_are_what_the_listing_answers() {
    for kind in KINDS {
        let emulator = Emulator::new();
        for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
            let resource = create(&emulator, *kind, 100 + index as u32, Some(&tags)).await;
            let listing = emulator.list_tags(resource.arn()).await;
            assert_eq!(
                listing.status, 200,
                "case {kind:?}/{case:?}: expected 200, got {} with body {}",
                listing.status, listing.body
            );
            assert_eq!(
                listed(&listing),
                tags,
                "case {kind:?}/{case:?}: expected exactly the tags the create carried, got {}",
                listed(&listing)
            );

            // A listing is a read: it changes nothing a later reader would see.
            let again = emulator.list_tags(resource.arn()).await;
            assert_eq!(
                again.body, listing.body,
                "case {kind:?}/{case:?}: two listings must answer the same body, got {} then {}",
                listing.body, again.body
            );
        }
    }
}

#[tokio::test]
async fn a_resource_created_without_tags_lists_an_empty_set() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let resource = create(&emulator, *kind, 1, None).await;
        let listing = emulator.list_tags(resource.arn()).await;
        assert_eq!(
            listing.status, 200,
            "case {kind:?}: expected 200, got {} with body {}",
            listing.status, listing.body
        );
        assert_eq!(
            listed(&listing),
            json!({}),
            "case {kind:?}: an untagged resource has no tags, got {}",
            listed(&listing)
        );
    }
}

/// One step of the tag/untag walk: the call to make, and the whole tag set the
/// resource must carry afterwards.
struct Step {
    case: &'static str,
    call: Call,
    expected: Value,
}

enum Call {
    Tag(Value),
    Untag(Vec<&'static str>),
}

fn walk() -> Vec<Step> {
    vec![
        Step {
            case: "tagging with an empty map changes nothing",
            call: Call::Tag(json!({})),
            expected: json!({ "env": "prod", "owner": "net" }),
        },
        Step {
            case: "a new key is added, the existing ones kept",
            call: Call::Tag(json!({ "team": "net-eng" })),
            expected: json!({ "env": "prod", "owner": "net", "team": "net-eng" }),
        },
        Step {
            case: "the same request replayed leaves the same set (@idempotent)",
            call: Call::Tag(json!({ "team": "net-eng" })),
            expected: json!({ "env": "prod", "owner": "net", "team": "net-eng" }),
        },
        Step {
            case: "an existing key takes the new value, the others untouched",
            call: Call::Tag(json!({ "env": "staging" })),
            expected: json!({ "env": "staging", "owner": "net", "team": "net-eng" }),
        },
        Step {
            case: "several keys at once, adding and replacing together",
            call: Call::Tag(json!({ "owner": "net-team", "note": "" })),
            expected: json!({
                "env": "staging", "owner": "net-team", "team": "net-eng", "note": ""
            }),
        },
        Step {
            case: "untagging one key removes only it",
            call: Call::Untag(vec!["team"]),
            expected: json!({ "env": "staging", "owner": "net-team", "note": "" }),
        },
        Step {
            case: "untagging a key the resource does not carry changes nothing",
            call: Call::Untag(vec!["team"]),
            expected: json!({ "env": "staging", "owner": "net-team", "note": "" }),
        },
        // No row for an EMPTY `tagKeys`: it is a `@required` `@httpQuery` binding,
        // so a request carrying no `tagKeys` parameter does not route to
        // `UntagResource` at all (the framework answers before any handler runs).
        Step {
            case: "untagging the rest, one known and one unknown key together",
            call: Call::Untag(vec!["env", "owner", "note", "never-set"]),
            expected: json!({}),
        },
        Step {
            case: "a resource emptied of tags can be tagged again",
            call: Call::Tag(json!({ "env": "prod" })),
            expected: json!({ "env": "prod" }),
        },
    ]
}

#[tokio::test]
async fn tagging_and_untagging_change_exactly_the_keys_they_name() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let created_with = json!({ "env": "prod", "owner": "net" });
        let resource = create(&emulator, *kind, 7, Some(&created_with)).await;

        for step in walk() {
            let case = step.case;
            let answer = match &step.call {
                Call::Tag(tags) => emulator.tag_resource(resource.arn(), tags).await,
                Call::Untag(keys) => emulator.untag_resource(resource.arn(), keys).await,
            };
            assert_eq!(
                answer.status, NO_CONTENT,
                "case {kind:?}/{case:?}: expected {NO_CONTENT}, got {} with body {}",
                answer.status, answer.body
            );

            let listing = emulator.list_tags(resource.arn()).await;
            assert_eq!(
                listed(&listing),
                step.expected,
                "case {kind:?}/{case:?}: expected {}, got {}",
                step.expected,
                listed(&listing)
            );

            // The same tags on the record itself: the write landed on the resource,
            // so the reader that serves it reports them too. A record reports
            // `tags` only when it has any.
            let read = resource.read(&emulator).await;
            let expected_on_record = if step.expected == json!({}) {
                Value::Null
            } else {
                step.expected.clone()
            };
            assert_eq!(
                read.member("tags"),
                &expected_on_record,
                "case {kind:?}/{case:?}: expected the record to carry {expected_on_record}, got {}",
                read.member("tags")
            );
        }
    }
}

#[tokio::test]
async fn tags_of_one_resource_are_not_the_tags_of_another() {
    // Two resources of each type, tagged differently: a tag write addresses ONE
    // ARN, so neither listing can answer the other's set.
    let emulator = Emulator::new();
    let first = create(&emulator, Kind::Scope, 11, Some(&json!({ "env": "prod" }))).await;
    let second = create(&emulator, Kind::Scope, 12, Some(&json!({ "env": "dev" }))).await;
    let monitor = create(&emulator, Kind::Monitor, 13, None).await;

    emulator
        .tag_resource(second.arn(), &json!({ "only-on-second": "yes" }))
        .await;
    emulator
        .tag_resource(monitor.arn(), &json!({ "only-on-monitor": "yes" }))
        .await;

    for (case, resource, expected) in [
        ("first scope", &first, json!({ "env": "prod" })),
        (
            "second scope",
            &second,
            json!({ "env": "dev", "only-on-second": "yes" }),
        ),
        ("monitor", &monitor, json!({ "only-on-monitor": "yes" })),
    ] {
        let listing = emulator.list_tags(resource.arn()).await;
        assert_eq!(
            listed(&listing),
            expected,
            "case {case:?}: expected {expected}, got {}",
            listed(&listing)
        );
    }
}

#[tokio::test]
async fn an_arn_the_store_does_not_hold_is_404_for_every_tagging_operation() {
    let emulator = Emulator::new();
    // A real, tagged resource exists, so a 404 is about the ARN and not about an
    // empty store.
    let known = create(&emulator, Kind::Scope, 21, Some(&json!({ "env": "prod" }))).await;

    // A monitor that existed and was deleted: its ARN is still well-formed, so a
    // 404 here is the store answering rather than the ARN's shape.
    let deleted = create(&emulator, Kind::Monitor, 22, None).await;
    let deleted_arn = deleted.arn().to_string();
    let Resource::Monitor { name, .. } = &deleted else {
        unreachable!("create(Kind::Monitor) answers with a monitor");
    };
    let removed = emulator.delete_monitor(name).await;
    assert_eq!(
        removed.status, 200,
        "the monitor must be deleted first, got {} with body {}",
        removed.status, removed.body
    );

    let unknown_scope = format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:scope/00000000-0000-4000-8000-ffffffffffff"
    );
    let unknown_monitor =
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:monitor/never-created");
    // The real resource's own ARN with its Region segment swapped for another: an
    // ARN this store minted nothing under.
    let other_region = {
        let mut segments: Vec<&str> = known.arn().split(':').collect();
        segments[3] = if segments[3] == "eu-central-1" {
            "us-east-2"
        } else {
            "eu-central-1"
        };
        segments.join(":")
    };
    let other_service = format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-123");

    for (case, arn) in [
        ("a scope id nothing was minted for", unknown_scope.as_str()),
        ("a monitor name nothing was minted for", unknown_monitor.as_str()),
        ("the ARN of a deleted monitor", deleted_arn.as_str()),
        ("a real resource's ARN in another Region", other_region.as_str()),
        ("another service's ARN", other_service.as_str()),
    ] {
        assert_ne!(
            arn,
            known.arn(),
            "the unknown-ARN table must not name the real resource"
        );
        for (operation, answer) in [
            ("ListTagsForResource", emulator.list_tags(arn).await),
            (
                "TagResource",
                emulator.tag_resource(arn, &json!({ "env": "prod" })).await,
            ),
            ("UntagResource", emulator.untag_resource(arn, &["env"]).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected 404, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains("ResourceNotFoundException")
                    || answer.text("__type").contains("ResourceNotFoundException"),
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {:?} / body {}",
                answer.error_type,
                answer.body
            );
        }
    }

    // And the refusals left the real resource's tags alone.
    let listing = emulator.list_tags(known.arn()).await;
    assert_eq!(
        listed(&listing),
        json!({ "env": "prod" }),
        "a refused tagging call must not touch another resource, got {}",
        listed(&listing)
    );
}

#[tokio::test]
async fn a_merge_past_the_published_tag_maximum_is_refused() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let resource = create(&emulator, *kind, 31, None).await;

        // At the bound: 200 tags is exactly what `TagMap` allows.
        let full = many_tags(MAX_TAGS, "x");
        let accepted = emulator.tag_resource(resource.arn(), &full).await;
        assert_eq!(
            accepted.status, NO_CONTENT,
            "case {kind:?}: {MAX_TAGS} tags is at the bound, got {} with body {}",
            accepted.status, accepted.body
        );

        // Still at the bound: replacing a key the resource already carries leaves
        // the set the same size, so it is accepted.
        let replace = emulator
            .tag_resource(resource.arn(), &json!({ "key-000": "replaced" }))
            .await;
        assert_eq!(
            replace.status, NO_CONTENT,
            "case {kind:?}: replacing a key keeps the set at {MAX_TAGS}, got {} with body {}",
            replace.status, replace.body
        );

        // One past the bound: a single NEW key would leave 201.
        let refused = emulator
            .tag_resource(resource.arn(), &json!({ "one-too-many": "x" }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {kind:?}: a {}th tag is past the bound, got {} with body {}",
            MAX_TAGS + 1,
            refused.status,
            refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException")
                || refused.text("__type").contains("ValidationException"),
            "case {kind:?}: expected ValidationException, got {:?} / body {}",
            refused.error_type,
            refused.body
        );

        // The refusal changed nothing: the set is still the 200 keys, with the
        // replaced value.
        let listing = emulator.list_tags(resource.arn()).await;
        let tags = listed(&listing);
        let map = tags.as_object().expect("a listing answers a tag map");
        assert_eq!(
            map.len(),
            MAX_TAGS,
            "case {kind:?}: expected {MAX_TAGS} tags after the refusal, got {}",
            map.len()
        );
        assert_eq!(
            map.get("key-000").and_then(Value::as_str),
            Some("replaced"),
            "case {kind:?}: expected the replaced value to have stuck, got {:?}",
            map.get("key-000")
        );
        assert!(
            !map.contains_key("one-too-many"),
            "case {kind:?}: the refused tag must not be stored, got {tags}"
        );
    }
}
