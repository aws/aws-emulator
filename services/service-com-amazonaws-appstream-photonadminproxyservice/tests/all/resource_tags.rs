// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's resource tags over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a resource's tags are ONE key-value map living on that resource: `TagResource` adds
//!   to it and overwrites the keys it already carries, `UntagResource` removes only the
//!   keys it is given, and `ListTagsForResource` reports whatever the create and those
//!   calls left there — for every taggable resource kind this service holds;
//! * a resource carries at most 50 tags, so a tag request that would take it past that
//!   is refused with `LimitExceededException` (HTTP 400) and persists nothing, while one
//!   that only overwrites keys it already carries is not growth and is accepted;
//! * tags belong TO a resource, so they survive every update of the resource they are
//!   on, and an ARN addressing no resource of this service is refused with
//!   `ResourceNotFoundException` (HTTP 400) by every tag operation, with AWS's sentence
//!   for a resource the account does not hold.

use serde_json::{json, Map, Value};

use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region the harness signs its requests for, and so the Region the ARNs this
/// service mints carry.
const REGION: &str = "us-east-1";

/// How many tags one resource may carry: the `Tags` map's own bound
/// (`smithy_query(query="shape", shape="Tags")`: `length 1..50`), which is also the
/// per-resource ceiling.
const MAX_TAGS: usize = 50;

/// AWS's `LimitExceededException` sentence for a tag write that would take a resource
/// past its tag ceiling: fixed text whose only variable part is the ceiling itself.
fn tag_limit_sentence() -> String {
    format!(
        "The resource already has the maximum of {MAX_TAGS} tags. Delete tags from the \
         specified resource as needed."
    )
}

/// The generic prose the model carries for the `LimitExceededException` SHAPE
/// (`smithy_query(query="doc", shape="LimitExceededException")`), which every limit
/// refusal of the service would share: the tag ceiling's refusal must NOT be this.
const GENERIC_ACCOUNT_LIMIT_SENTENCE: &str =
    "The requested limit exceeds the permitted limit for an account.";

/// `arn:aws:appstream:<region>:<account>:<resource-type>/<name>` — the ARN this service
/// mints for a resource of that type, in this emulator's account.
fn resource_arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:appstream:{REGION}:{ACCOUNT}:{resource_type}/{name}")
}

/// One taggable resource: the word AWS's refusals use for it, the `resource-type`
/// segment its ARN carries, and the create that persists one.
struct Resource {
    kind: &'static str,
    resource_type: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
}

/// The resource kinds this service holds, so every rule below is asserted on each of
/// them rather than on the one kind a recorded request happened to tag. The required
/// members of each create come from
/// `smithy_query(query="operation", operation="CreateStack,CreateFleet")`.
fn resources() -> Vec<Resource> {
    vec![
        Resource {
            kind: "stack",
            resource_type: "stack",
            operation: "CreateStack",
            request: |name| json!({ "Name": name }),
        },
        Resource {
            kind: "fleet",
            resource_type: "fleet",
            operation: "CreateFleet",
            request: |name| json!({ "Name": name, "InstanceType": "stream.standard.small" }),
        },
    ]
}

/// Tag maps spanning the class: the rows differ along entry count, value emptiness and
/// the character classes `TagKey`/`TagValue` admit
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape", shape="TagKey,TagValue")`).
fn tag_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("one tag", json!({ "Environment": "Test" })),
        ("an empty value", json!({ "Environment": "" })),
        (
            "the special characters a tag admits",
            json!({ "team.name_1": "a/b:c=d+e-f@g" }),
        ),
        (
            "letters and spaces outside ASCII",
            json!({ "Besitzer": "Groß Team" }),
        ),
        (
            "several tags at once",
            json!({
                "Environment": "Production",
                "Owner": "platform",
                "cost.center": "4711",
                "Project": "WorkSpaces Applications",
                "tier": "1",
            }),
        ),
    ]
}

/// `count` tags whose keys are `<prefix>-<n>`, for the rows that count tags rather than
/// caring what they say.
fn numbered_tags(prefix: &str, count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| (format!("{prefix}-{index}"), json!(format!("value-{index}"))))
        .collect();
    Value::Object(map)
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(emulator: &Emulator, case: &str, arn: &str) -> Map<String, Value> {
    let payload = emulator
        .ok(case, "ListTagsForResource", json!({ "ResourceArn": arn }))
        .await;
    payload
        .get("Tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags collection, got {payload}"))
        .clone()
}

/// The object a tag row carries, as a map.
fn object(tags: &Value) -> Map<String, Value> {
    tags.as_object().expect("a tag row is an object").clone()
}

/// Create one resource of `resource` named `name`, and answer the ARN it is addressed by.
async fn create(emulator: &Emulator, case: &str, resource: &Resource, name: &str) -> String {
    emulator
        .ok(case, resource.operation, (resource.request)(name))
        .await;
    resource_arn(resource.resource_type, name)
}

// ---------------------------------------------------------------------------
// TagResource / ListTagsForResource
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tagging_a_resource_persists_exactly_the_tags_a_later_read_reports() {
    for resource in resources() {
        for (label, tags) in tag_rows() {
            let case = format!("{} tagged with {label}", resource.kind);
            let emulator = Emulator::new();
            let arn = create(&emulator, &case, &resource, "tagged").await;

            // A resource created without tags carries none: an empty collection is an
            // answer, not a refusal.
            let before = listed_tags(&emulator, &case, &arn).await;
            assert!(
                before.is_empty(),
                "case {case:?}: a resource created without tags must report none, got \
                 {before:?}"
            );

            let tagged = emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": tags }),
                )
                .await;
            assert_eq!(
                tagged,
                json!({}),
                "case {case:?}: TagResource reports no members, got {tagged}"
            );

            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(&tags),
                "case {case:?}: the read must report exactly the tags the write \
                 persisted, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn tags_a_create_persisted_are_the_ones_the_tag_store_reports() {
    // Rows: the tags a create carries. A create's `Tags` and a later `TagResource` write
    // to the SAME tag set, so a read reports the union rather than one of two copies.
    for (label, tags) in tag_rows() {
        for resource in resources() {
            let case = format!("{} created with {label}", resource.kind);
            let emulator = Emulator::new();
            let mut request = (resource.request)("created-tagged");
            request["Tags"] = tags.clone();
            emulator.ok(&case, resource.operation, request).await;
            let arn = resource_arn(resource.resource_type, "created-tagged");

            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(&tags),
                "case {case:?}: the read must report the tags the create persisted, got \
                 {listed:?}"
            );

            emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": { "added-later": "yes" } }),
                )
                .await;
            let mut expected = object(&tags);
            expected.insert("added-later".to_string(), json!("yes"));
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                expected,
                "case {case:?}: a later tag write must add to the tags the create \
                 persisted, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn tagging_again_overwrites_the_keys_a_resource_carries_and_adds_the_rest() {
    // Rows: (tags already carried, tags written next, what the resource must then
    // carry). They differ along which keys collide, how many are new, and whether the
    // new value is empty.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "one key overwritten",
            json!({ "Environment": "Test" }),
            json!({ "Environment": "Production" }),
            json!({ "Environment": "Production" }),
        ),
        (
            "overwritten with an empty value",
            json!({ "Environment": "Test" }),
            json!({ "Environment": "" }),
            json!({ "Environment": "" }),
        ),
        (
            "one overwritten, one added",
            json!({ "Environment": "Test", "Owner": "platform" }),
            json!({ "Owner": "data", "tier": "2" }),
            json!({ "Environment": "Test", "Owner": "data", "tier": "2" }),
        ),
        (
            "nothing in common",
            json!({ "Environment": "Test" }),
            json!({ "Owner": "platform" }),
            json!({ "Environment": "Test", "Owner": "platform" }),
        ),
        (
            "the same tags written twice",
            json!({ "Environment": "Test", "Owner": "platform" }),
            json!({ "Environment": "Test", "Owner": "platform" }),
            json!({ "Environment": "Test", "Owner": "platform" }),
        ),
    ];

    for resource in resources() {
        for (label, first, second, expected) in &rows {
            let case = format!("{} with {label}", resource.kind);
            let emulator = Emulator::new();
            let arn = create(&emulator, &case, &resource, "retagged").await;

            for tags in [first, second] {
                emulator
                    .ok(
                        &case,
                        "TagResource",
                        json!({ "ResourceArn": arn, "Tags": tags }),
                    )
                    .await;
            }

            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(expected),
                "case {case:?}: a second write must overwrite the keys it names and \
                 leave the rest, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// UntagResource
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untagging_removes_only_the_keys_it_is_given() {
    // Rows: the keys one `UntagResource` names, and what the resource must then carry.
    // They differ along how many keys are removed, whether a key the resource never
    // carried is among them, and whether every key goes.
    let carried = json!({ "Environment": "Test", "Owner": "platform", "tier": "1" });
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "one key",
            json!(["Owner"]),
            json!({ "Environment": "Test", "tier": "1" }),
        ),
        (
            "two keys",
            json!(["Owner", "tier"]),
            json!({ "Environment": "Test" }),
        ),
        (
            "every key",
            json!(["Environment", "Owner", "tier"]),
            json!({}),
        ),
        ("a key the resource never carried", json!(["absent"]), carried.clone()),
        (
            "a carried key beside one that was never carried",
            json!(["Owner", "absent"]),
            json!({ "Environment": "Test", "tier": "1" }),
        ),
        (
            "the same key twice",
            json!(["Owner", "Owner"]),
            json!({ "Environment": "Test", "tier": "1" }),
        ),
    ];

    for resource in resources() {
        for (label, keys, expected) in &rows {
            let case = format!("{} untagged by {label}", resource.kind);
            let emulator = Emulator::new();
            let arn = create(&emulator, &case, &resource, "untagged").await;
            emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": carried }),
                )
                .await;

            let untagged = emulator
                .ok(
                    &case,
                    "UntagResource",
                    json!({ "ResourceArn": arn, "TagKeys": keys }),
                )
                .await;
            assert_eq!(
                untagged,
                json!({}),
                "case {case:?}: UntagResource reports no members, got {untagged}"
            );

            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(expected),
                "case {case:?}: untagging must remove only the keys it names, got \
                 {listed:?}"
            );

            // A key that is gone can be written again, so the removal freed the key
            // rather than blocking it.
            emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": { "Owner": "platform" } }),
                )
                .await;
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed.get("Owner"),
                Some(&json!("platform")),
                "case {case:?}: a removed key must be writable again, got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The per-resource tag ceiling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_carries_at_most_fifty_tags() {
    // Rows: (tags already carried, new tags written next). Every row sits on one side of
    // the published bound of 50 tags per resource — at it, one short of it, and past it
    // by one and by many.
    let rows: Vec<(usize, usize)> = vec![
        (0, MAX_TAGS),
        (0, MAX_TAGS - 1),
        (MAX_TAGS / 2, MAX_TAGS / 2),
        (MAX_TAGS - 2, 2),
        (MAX_TAGS - 1, 1),
        (MAX_TAGS - 1, 2),
        (MAX_TAGS, 1),
        (MAX_TAGS - 5, 10),
        (MAX_TAGS, MAX_TAGS),
    ];

    for (carried, added) in rows {
        let total = carried + added;
        let case = format!("{carried} tags carried, {added} added");
        let emulator = Emulator::new();
        let arn = create(&emulator, &case, &resources()[0], "bounded").await;

        let existing = numbered_tags("carried", carried);
        if carried > 0 {
            emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": existing }),
                )
                .await;
        }

        let request = json!({ "ResourceArn": arn, "Tags": numbered_tags("added", added) });
        if total <= MAX_TAGS {
            emulator.ok(&case, "TagResource", request).await;
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed.len(),
                total,
                "case {case:?}: a resource may carry up to {MAX_TAGS} tags, so all \
                 {total} must be stored, got {listed:?}"
            );
        } else {
            let message = refusal(
                &emulator,
                &case,
                "TagResource",
                request,
                400,
                "LimitExceededException",
            )
            .await;
            assert_eq!(
                message,
                tag_limit_sentence(),
                "case {case:?}: the refusal must carry AWS's tag-limit sentence, got \
                 {message:?}"
            );

            // The refusal persisted nothing: the resource still carries exactly what it
            // had before, and not one of the refused keys.
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(&existing),
                "case {case:?}: a refused tag write must leave the tags as they were, \
                 got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_ceiling_refusal_names_the_tag_ceiling_rather_than_the_generic_account_limit() {
    // Rows: ways of crossing the per-resource ceiling. They differ along the resource
    // kind, how many tags the resource already carries, how many keys NEW to it the
    // write brings, whether the write also overwrites keys it already carries, and what
    // the tag keys and values say. The sentence is fixed text about the ceiling the
    // caller crossed, so none of that may change it — and no request value appears in
    // it. A single request is itself bounded at 50 entries, so every row crosses the
    // ceiling through what the resource already carries plus what the write adds.
    let mut overwriting_half = object(&numbered_tags("carried", MAX_TAGS / 2));
    overwriting_half.insert("one.more/key".to_string(), json!(""));

    let rows: Vec<(&str, usize, Value)> = vec![
        (
            "at the ceiling, gaining one key",
            MAX_TAGS,
            json!({ "one-more": "v" }),
        ),
        (
            "at the ceiling, gaining one key while overwriting half of the carried ones",
            MAX_TAGS,
            Value::Object(overwriting_half),
        ),
        (
            "one short of the ceiling, gaining two keys",
            MAX_TAGS - 1,
            json!({ "team.name_1": "a/b:c=d+e-f@g", "Besitzer": "Groß Team" }),
        ),
        (
            "nearly empty, gaining a full request of new keys",
            1,
            numbered_tags("added", MAX_TAGS),
        ),
        (
            "at the ceiling, gaining a full request of new keys",
            MAX_TAGS,
            numbered_tags("added", MAX_TAGS),
        ),
    ];

    for resource in resources() {
        for (label, carried, write) in &rows {
            let case = format!("{} {label}", resource.kind);
            let emulator = Emulator::new();
            let arn = create(&emulator, &case, &resource, "at-the-ceiling").await;

            let existing = numbered_tags("carried", *carried);
            emulator
                .ok(
                    &case,
                    "TagResource",
                    json!({ "ResourceArn": arn, "Tags": existing }),
                )
                .await;

            let message = refusal(
                &emulator,
                &case,
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": write }),
                400,
                "LimitExceededException",
            )
            .await;
            assert_eq!(
                message,
                tag_limit_sentence(),
                "case {case:?}: the refusal must name the {MAX_TAGS}-tag ceiling the \
                 caller crossed, got {message:?}"
            );
            assert_ne!(
                message, GENERIC_ACCOUNT_LIMIT_SENTENCE,
                "case {case:?}: the tag ceiling must not be reported with the generic \
                 account-limit sentence, got {message:?}"
            );
            // Nothing of the request is interpolated into the sentence.
            for key in object(write).keys() {
                assert!(
                    !message.contains(key.as_str()),
                    "case {case:?}: the refusal must carry no tag key from the request, \
                     got {message:?}"
                );
            }
            assert!(
                !message.contains(&arn),
                "case {case:?}: the refusal must carry no ARN from the request, got \
                 {message:?}"
            );

            // And the refusal persisted nothing.
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                object(&existing),
                "case {case:?}: a refused tag write must leave the tags as they were, \
                 got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_that_only_overwrites_keys_a_full_resource_carries_is_accepted() {
    // A resource at the ceiling is not growing when every key written is one it already
    // carries, so the write is accepted and the values change. Rows: how many of the 50
    // carried keys are overwritten.
    for overwritten in [1_usize, MAX_TAGS / 2, MAX_TAGS] {
        let case = format!("{overwritten} of {MAX_TAGS} keys overwritten");
        let emulator = Emulator::new();
        let arn = create(&emulator, &case, &resources()[0], "full").await;
        emulator
            .ok(
                &case,
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": numbered_tags("carried", MAX_TAGS) }),
            )
            .await;

        let rewritten: Map<String, Value> = (0..overwritten)
            .map(|index| (format!("carried-{index}"), json!("rewritten")))
            .collect();
        emulator
            .ok(
                &case,
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": Value::Object(rewritten.clone()) }),
            )
            .await;

        let listed = listed_tags(&emulator, &case, &arn).await;
        assert_eq!(
            listed.len(),
            MAX_TAGS,
            "case {case:?}: overwriting carried keys must not grow the tag set, got \
             {listed:?}"
        );
        for (key, value) in &rewritten {
            assert_eq!(
                listed.get(key),
                Some(value),
                "case {case:?}: {key} must carry its new value, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_request_may_not_carry_more_tags_or_keys_than_the_model_admits() {
    // The `Tags` map and the `TagKeys` list are bounded at 50 entries each, so a single
    // request past that bound is refused at the boundary. Rows: one entry past the
    // bound, and far past it.
    let case = "one request past the modelled bound";
    let emulator = Emulator::new();
    let arn = create(&emulator, case, &resources()[0], "bounded-request").await;

    for count in [MAX_TAGS + 1, MAX_TAGS * 3] {
        let case = format!("{count} tags in one request");
        assert_error(
            &emulator,
            &case,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": numbered_tags("many", count) }),
            400,
            "ValidationException",
        )
        .await;

        let keys: Vec<String> = (0..count).map(|index| format!("many-{index}")).collect();
        assert_error(
            &emulator,
            &case,
            "UntagResource",
            json!({ "ResourceArn": arn, "TagKeys": keys }),
            400,
            "ValidationException",
        )
        .await;
    }

    // The refused requests persisted nothing.
    let listed = listed_tags(&emulator, case, &arn).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a request refused at the boundary must persist nothing, got \
         {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// The tag set belongs to the resource, not to one of its members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tags_survive_every_update_of_the_resource_they_belong_to() {
    // Rows: updates to the stack the tags belong to. An update writes the members it
    // names and clears the attributes it names; neither is a tag, so the tag set must
    // come through every one of them unchanged.
    let carried = json!({ "Environment": "Test", "Owner": "platform" });
    let updates: Vec<(&str, Value)> = vec![
        (
            "a member set",
            json!({ "Name": "updated", "Description": "a new description" }),
        ),
        (
            "an access endpoint attached",
            json!({
                "Name": "updated",
                "AccessEndpoints": [{ "EndpointType": "STREAMING", "VpceId": "vpce-00000000" }],
            }),
        ),
        (
            "an attribute deleted",
            json!({ "Name": "updated", "AttributesToDelete": ["ACCESS_ENDPOINTS"] }),
        ),
        (
            "several attributes deleted",
            json!({
                "Name": "updated",
                "AttributesToDelete": ["REDIRECT_URL", "FEEDBACK_URL", "USER_SETTINGS"],
            }),
        ),
    ];

    let case = "tags across updates";
    let emulator = Emulator::new();
    emulator
        .ok(
            case,
            "CreateStack",
            json!({
                "Name": "updated",
                "RedirectURL": "https://example.invalid/after",
                "FeedbackURL": "https://example.invalid/feedback",
                "Tags": carried,
            }),
        )
        .await;
    let arn = resource_arn("stack", "updated");

    for (label, update) in &updates {
        emulator.ok(label, "UpdateStack", update.clone()).await;
        let listed = listed_tags(&emulator, label, &arn).await;
        assert_eq!(
            listed,
            object(&carried),
            "case {label:?}: an update of the resource must leave its tags alone, got \
             {listed:?}"
        );
    }

    // And a tag write after all of that still MERGES into the set the create persisted.
    emulator
        .ok(
            case,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": { "Owner": "data", "tier": "1" } }),
        )
        .await;
    let listed = listed_tags(&emulator, case, &arn).await;
    assert_eq!(
        listed,
        object(&json!({ "Environment": "Test", "Owner": "data", "tier": "1" })),
        "case {case:?}: a later tag write must merge into the surviving tag set, got \
         {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// An ARN that addresses no resource
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tag_operation_refuses_an_arn_that_addresses_no_resource() {
    let emulator = Emulator::new();
    // Two resources that DO exist, so each row below is missing for exactly the reason
    // it names rather than because the account holds nothing.
    for resource in resources() {
        create(&emulator, "stored", &resource, "stored").await;
    }
    emulator
        .ok("deleted", "CreateStack", json!({ "Name": "deleted" }))
        .await;
    emulator
        .ok("deleted", "DeleteStack", json!({ "Name": "deleted" }))
        .await;

    // Rows: (why the ARN addresses nothing, the ARN, the kind and name AWS's sentence
    // names). They differ along the reason — a name nothing holds, a resource that was
    // deleted, another Region's and another account's ARN for a name that IS held here,
    // a resource type this service holds none of, and an ARN with no resource type at
    // all.
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a stack name nothing holds",
            resource_arn("stack", "absent"),
            "stack",
            "absent",
        ),
        (
            "a fleet name nothing holds",
            resource_arn("fleet", "absent"),
            "fleet",
            "absent",
        ),
        (
            "a stack that was deleted",
            resource_arn("stack", "deleted"),
            "stack",
            "deleted",
        ),
        (
            "another Region's ARN for a stack this Region holds",
            format!("arn:aws:appstream:eu-west-1:{ACCOUNT}:stack/stored"),
            "stack",
            "stored",
        ),
        (
            "another account's ARN for a stack this account holds",
            "arn:aws:appstream:us-east-1:123456789012:fleet/stored".to_string(),
            "fleet",
            "stored",
        ),
        (
            "a resource type this service holds none of",
            resource_arn("image-builder", "builder"),
            "image builder",
            "builder",
        ),
        (
            "an ARN with no resource type",
            "arn:aws:s3:::a-bucket".to_string(),
            "resource",
            "a-bucket",
        ),
    ];

    for (label, arn, kind, name) in rows {
        for (operation, request) in [
            (
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": { "Environment": "Test" } }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "TagKeys": ["Environment"] }),
            ),
            (
                "ListTagsForResource",
                json!({ "ResourceArn": arn.clone() }),
            ),
        ] {
            let case = format!("{operation} on {label}");
            let message = refusal(
                &emulator,
                &case,
                operation,
                request,
                400,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message,
                not_found_sentence(kind, name),
                "case {case:?}: the refusal must name the resource that was searched \
                 for, got {message:?}"
            );
        }
    }

    // None of those refusals touched the resources that do exist.
    for resource in resources() {
        let case = format!("the stored {}", resource.kind);
        let arn = resource_arn(resource.resource_type, "stored");
        let listed = listed_tags(&emulator, &case, &arn).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused tag call must leave the stored resource untagged, \
             got {listed:?}"
        );
    }
}
