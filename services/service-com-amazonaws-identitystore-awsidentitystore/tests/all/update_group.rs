// SPDX-License-Identifier: CC0-1.0

//! `UpdateGroup`, driven end to end over HTTP against the production router.
//!
//! The rule under test: an `UpdateGroup` applies each `AttributeOperation` to the
//! stored group in request order — setting the group attribute its `AttributePath`
//! names to the value it carries, or removing that attribute when it carries none —
//! and then moves the group's revision, so a later `DescribeGroup` reports exactly
//! what was applied; a request this service cannot apply changes nothing.
//!
//! Every suite is a TABLE whose rows differ along what that rule generalises over:
//! which attribute, set versus remove, how many operations one call carries, their
//! order, the value's length (at both modelled bounds and past the upper one), its
//! type, its character class, and the form of the `GroupId` naming the group. The
//! recorded `description` case is one row.
//!
//! The harness (`emulator`, `ok`, `refused`, `string`) is the one
//! `identity_store_lifecycle` already owns, so a request here goes through the same
//! front door a client uses and nothing reaches into the state struct.

use serde_json::{json, Value};

use crate::harness::{
    emulator, error_message, ok, refused, string, MISSING_STORE, NOT_FOUND, PRECONDITION_ROWS,
    STORE,
};
use crate::identity_store_lifecycle::assert_resource_arn;

/// The upper bound of the `@length` both group text attributes carry: `DisplayName`
/// targets `GroupDisplayName` and `Description` targets `SensitiveStringType`, each
/// `@length(1..1024)` (`smithy_query(query="shape",shape="Group")`).
const GROUP_TEXT_MAX_LEN: usize = 1024;

/// The lower bound of that same `@length`.
const GROUP_TEXT_MIN_LEN: usize = 1;

/// The description every group in these suites is created with, so a row that does not
/// touch `description` has something to leave alone.
const CREATED_DESCRIPTION: &str = "the description this group was created with";

/// A well-formed `ResourceId` no group holds: the modelled pattern demands a
/// hexadecimal UUID (`smithy_query(query="shape",shape="ResourceId")`).
const ABSENT_GROUP_ID: &str = "a1b2c3d4-5678-90ab-cdef-aaaaaaaaaaaa";

/// A second `IdentityStoreId` that names no store this emulator serves: the ARN form
/// carries the owning account, and this one names a store of another account (see
/// [`crate::harness::MISSING_STORE`], which names a different one).
const ABSENT_STORE: &str = "arn:aws:identitystore::210987654321:identitystore/d-0000000099";

/// One created group: what `CreateGroup` minted for it.
struct Created {
    group_id: String,
    group_arn: String,
    revision: String,
}

/// Create a group carrying `display_name` and [`CREATED_DESCRIPTION`].
async fn create(
    router: &smithy_server_runtime::MultiServiceRouter,
    store: &str,
    display_name: &str,
    case: &str,
) -> Created {
    let created = ok(
        router,
        "CreateGroup",
        json!({
            "IdentityStoreId": store,
            "DisplayName": display_name,
            "Description": CREATED_DESCRIPTION,
        }),
        case,
    )
    .await;
    Created {
        group_id: string(&created, "GroupId", case),
        group_arn: string(&created, "GroupArn", case),
        revision: string(&created, "Revision", case),
    }
}

/// What `DescribeGroup` reports for `group_id`.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    store: &str,
    group_id: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "DescribeGroup",
        json!({ "IdentityStoreId": store, "GroupId": group_id }),
        case,
    )
    .await
}

/// An optional string member of a response body: `None` when it is absent or null.
fn optional(body: &Value, member: &str) -> Option<String> {
    body.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A `@timestamp` member of a response body, as the epoch seconds the `awsJson1_1`
/// protocol renders one with.
fn timestamp(body: &Value, member: &str, case: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {body}"))
}

/// Assert `DescribeGroup` reports exactly `display_name` and `description`.
fn assert_attributes(
    reported: &Value,
    display_name: Option<&str>,
    description: Option<&str>,
    case: &str,
) {
    let actual_display = optional(reported, "DisplayName");
    assert_eq!(
        actual_display.as_deref(),
        display_name,
        "case {case:?}: expected DisplayName {display_name:?}, got {actual_display:?}"
    );
    let actual_description = optional(reported, "Description");
    assert_eq!(
        actual_description.as_deref(),
        description,
        "case {case:?}: expected Description {description:?}, got {actual_description:?}"
    );
}

/// An `UpdateGroup` request that sets `group_id`'s `displayName` to `name`.
fn rename(store: &str, group_id: &str, name: &str) -> Value {
    json!({
        "IdentityStoreId": store,
        "GroupId": group_id,
        "Operations": [ { "AttributePath": "displayName", "AttributeValue": name } ],
    })
}

/// One row of the apply table: the operations one `UpdateGroup` carries, and the two
/// group attributes the stored group must hold afterwards.
struct ApplyCase {
    case: &'static str,
    operations: Vec<Value>,
    display_name: Option<String>,
    description: Option<String>,
}

/// The apply table. `created_display_name` is the name the row's group is created
/// with, so a row that renames it can state the name it must end up with.
fn apply_cases(created_display_name: &str) -> Vec<ApplyCase> {
    let renamed = format!("{created_display_name}-renamed");
    // The `@length` bounds of a group text attribute, as values: one character, and
    // exactly the 1024 the model admits.
    let shortest = "x".repeat(GROUP_TEXT_MIN_LEN);
    let longest = "l".repeat(GROUP_TEXT_MAX_LEN);
    // The `@pattern` admits marks, symbols, numbers, punctuation, tab, newline,
    // carriage return and spaces, so a value is not limited to ASCII words.
    let mixed = "héllo\tworld\nsecond line — ✓ 42%";
    // The three space characters the class lists, other than the ASCII one.
    let spaced = "no-break\u{a0}and\u{3000}ideographic".to_string();
    vec![
        ApplyCase {
            case: "replace description",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": "an updated description" }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: Some("an updated description".to_string()),
        },
        ApplyCase {
            case: "remove description by omitting the value",
            operations: vec![json!({ "AttributePath": "description" })],
            display_name: Some(created_display_name.to_string()),
            description: None,
        },
        ApplyCase {
            case: "remove description with an explicit null value",
            operations: vec![json!({ "AttributePath": "description", "AttributeValue": null })],
            display_name: Some(created_display_name.to_string()),
            description: None,
        },
        ApplyCase {
            case: "replace displayName",
            operations: vec![
                json!({ "AttributePath": "displayName", "AttributeValue": renamed.clone() }),
            ],
            display_name: Some(renamed.clone()),
            description: Some(CREATED_DESCRIPTION.to_string()),
        },
        ApplyCase {
            case: "a path spelled the way the model spells the member",
            operations: vec![
                json!({ "AttributePath": "DisplayName", "AttributeValue": renamed.clone() }),
            ],
            display_name: Some(renamed.clone()),
            description: Some(CREATED_DESCRIPTION.to_string()),
        },
        ApplyCase {
            case: "remove displayName",
            operations: vec![json!({ "AttributePath": "displayName" })],
            display_name: None,
            description: Some(CREATED_DESCRIPTION.to_string()),
        },
        ApplyCase {
            case: "both attributes in one call",
            operations: vec![
                json!({ "AttributePath": "displayName", "AttributeValue": renamed.clone() }),
                json!({ "AttributePath": "description", "AttributeValue": "both at once" }),
            ],
            display_name: Some(renamed.clone()),
            description: Some("both at once".to_string()),
        },
        ApplyCase {
            case: "two operations on one path: the later one wins",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": "first" }),
                json!({ "AttributePath": "description", "AttributeValue": "second" }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: Some("second".to_string()),
        },
        ApplyCase {
            case: "set then remove in one call",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": "set" }),
                json!({ "AttributePath": "description" }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: None,
        },
        ApplyCase {
            case: "a value at the modelled minimum length",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": shortest.clone() }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: Some(shortest),
        },
        ApplyCase {
            case: "a value at the modelled maximum length",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": longest.clone() }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: Some(longest),
        },
        ApplyCase {
            case: "a value of marks, symbols, punctuation and whitespace",
            operations: vec![json!({ "AttributePath": "description", "AttributeValue": mixed })],
            display_name: Some(created_display_name.to_string()),
            description: Some(mixed.to_string()),
        },
        ApplyCase {
            // The class lists exactly three space characters: U+0020, U+00A0 and
            // U+3000 (`smithy_query(query="shape",shape="Group")`). This row carries
            // the two the issue never mentions.
            case: "a value carrying the other two space characters the class lists",
            operations: vec![
                json!({ "AttributePath": "description", "AttributeValue": spaced.clone() }),
            ],
            display_name: Some(created_display_name.to_string()),
            description: Some(spaced),
        },
    ]
}

#[tokio::test]
async fn attribute_operations_are_applied_to_the_stored_group() {
    let router = emulator();
    // One group per row, so a row only ever observes its own writes. Each row's group
    // is created with a name carrying the row index, because a `DisplayName` is a
    // uniqueness claim within the store — which is why the table is rebuilt once per
    // row with that name: a row that renames the group states the name it must end
    // with. The row count comes from one build outside the loop.
    let row_count = apply_cases("probe").len();
    for index in 0..row_count {
        let created_display_name = format!("the group as created {index}");
        let row = apply_cases(&created_display_name)
            .into_iter()
            .nth(index)
            .expect("the table is built the same way every time");
        let case = row.case;
        let group = create(&router, STORE, &created_display_name, case).await;
        // The timestamps as created, so the update's effect on them is observable.
        let as_created = describe(&router, STORE, &group.group_id, case).await;
        let created_at = timestamp(&as_created, "CreatedAt", case);
        let updated_at_before = timestamp(&as_created, "UpdatedAt", case);

        let updated = ok(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.group_id,
                "Operations": row.operations,
            }),
            case,
        )
        .await;

        // The response reports the group that was updated, not a fresh one.
        let identity_store_id = string(&updated, "IdentityStoreId", case);
        assert_eq!(
            identity_store_id, STORE,
            "case {case:?}: expected IdentityStoreId {STORE:?}, got {identity_store_id:?}"
        );
        let group_id = string(&updated, "GroupId", case);
        assert_eq!(
            group_id, group.group_id,
            "case {case:?}: expected GroupId {:?}, got {group_id:?}",
            group.group_id
        );
        let group_arn = string(&updated, "GroupArn", case);
        assert_eq!(
            group_arn, group.group_arn,
            "case {case:?}: expected the group's own ARN {:?}, got {group_arn:?}",
            group.group_arn
        );
        assert_resource_arn(&group_arn, "group", &group.group_id, case);

        // `Revision` "changes each time the group is modified", and is an opaque
        // token, so the assertion is that it moved — never what it spells.
        let revision = string(&updated, "Revision", case);
        assert_ne!(
            revision, group.revision,
            "case {case:?}: expected the revision to change from {:?}, got {revision:?}",
            group.revision
        );

        let reported = describe(&router, STORE, &group.group_id, case).await;
        assert_attributes(
            &reported,
            row.display_name.as_deref(),
            row.description.as_deref(),
            case,
        );
        let reported_revision = string(&reported, "Revision", case);
        assert_eq!(
            reported_revision, revision,
            "case {case:?}: expected DescribeGroup to report the revision \
             UpdateGroup answered ({revision:?}), got {reported_revision:?}"
        );
        // An update moves `UpdatedAt` and leaves `CreatedAt` where it was. The clock
        // may not resolve two calls, so `UpdatedAt` is asserted not to go backwards
        // rather than to differ.
        let created_at_after = timestamp(&reported, "CreatedAt", case);
        assert_eq!(
            created_at_after, created_at,
            "case {case:?}: expected CreatedAt to stay at {created_at}, \
             got {created_at_after}"
        );
        let updated_at_after = timestamp(&reported, "UpdatedAt", case);
        assert!(
            updated_at_after >= updated_at_before && updated_at_after >= created_at,
            "case {case:?}: expected UpdatedAt at or after both the previous \
             {updated_at_before} and CreatedAt {created_at}, got {updated_at_after}"
        );
    }
}

#[tokio::test]
async fn each_update_moves_the_revision_and_the_reader_agrees() {
    let router = emulator();
    // The axis is how many updates one group takes.
    for updates in [1usize, 2, 5] {
        let case = format!("{updates} sequential updates");
        let case = case.as_str();
        let group = create(&router, STORE, &format!("sequential {updates}"), case).await;
        let mut seen = vec![group.revision.clone()];
        for round in 0..updates {
            let updated = ok(
                &router,
                "UpdateGroup",
                json!({
                    "IdentityStoreId": STORE,
                    "GroupId": group.group_id,
                    "Operations": [
                        { "AttributePath": "description", "AttributeValue": format!("round {round}") },
                    ],
                }),
                case,
            )
            .await;
            let revision = string(&updated, "Revision", case);
            assert!(
                !seen.contains(&revision),
                "case {case:?}: expected round {round} to answer a revision none of \
                 {seen:?} already carried, got {revision:?}"
            );
            let reported = describe(&router, STORE, &group.group_id, case).await;
            assert_attributes(
                &reported,
                Some(&format!("sequential {updates}")),
                Some(&format!("round {round}")),
                case,
            );
            let reported_revision = string(&reported, "Revision", case);
            assert_eq!(
                reported_revision, revision,
                "case {case:?}: expected DescribeGroup at round {round} to report \
                 {revision:?}, got {reported_revision:?}"
            );
            seen.push(revision);
        }
    }
}

#[tokio::test]
async fn a_revision_precondition_is_honoured() {
    let router = emulator();
    // Each row names the `Revision` the request sends. `None` sends none at all.
    enum Sent {
        /// The group's current revision.
        Current,
        /// The revision the group carried before its last update.
        Stale,
        /// A literal no group of this store ever carried.
        Literal(&'static str),
        /// No `Revision` member.
        Absent,
    }
    let rows: [(&str, Sent, bool); 5] = [
        ("the group's current revision", Sent::Current, true),
        ("no revision at all", Sent::Absent, true),
        ("the revision before the last update", Sent::Stale, false),
        ("a revision no group ever carried", Sent::Literal("999999"), false),
        ("the smallest well-formed revision", Sent::Literal("0"), false),
    ];
    for (case, sent, applies) in rows {
        let group = create(&router, STORE, &format!("precondition {case}"), case).await;
        // One update, so the group has both a current and a stale revision.
        let settled = ok(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.group_id,
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": "before the row" },
                ],
            }),
            case,
        )
        .await;
        let current = string(&settled, "Revision", case);

        let mut request = json!({
            "IdentityStoreId": STORE,
            "GroupId": group.group_id,
            "Operations": [
                { "AttributePath": "description", "AttributeValue": "the row's own value" },
            ],
        });
        match sent {
            Sent::Current => request["Revision"] = json!(current),
            Sent::Stale => request["Revision"] = json!(group.revision),
            Sent::Literal(revision) => request["Revision"] = json!(revision),
            Sent::Absent => {}
        }

        if applies {
            let updated = ok(&router, "UpdateGroup", request, case).await;
            let revision = string(&updated, "Revision", case);
            assert_ne!(
                revision, current,
                "case {case:?}: expected an applied update to move the revision from \
                 {current:?}, got {revision:?}"
            );
            let reported = describe(&router, STORE, &group.group_id, case).await;
            assert_attributes(
                &reported,
                Some(&format!("precondition {case}")),
                Some("the row's own value"),
                case,
            );
        } else {
            refused(&router, "UpdateGroup", request, 409, "ConflictException", case).await;
            // A refused precondition changes nothing: neither the attributes nor the
            // revision moved.
            let reported = describe(&router, STORE, &group.group_id, case).await;
            assert_attributes(
                &reported,
                Some(&format!("precondition {case}")),
                Some("before the row"),
                case,
            );
            let reported_revision = string(&reported, "Revision", case);
            assert_eq!(
                reported_revision, current,
                "case {case:?}: expected the revision to stay at {current:?}, \
                 got {reported_revision:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_operation_this_api_cannot_apply_is_refused_and_changes_nothing() {
    let router = emulator();
    // Rows differ along WHY the operation cannot be applied: the path names something
    // that is not a mutable group attribute, or the value is not one the attribute's
    // modelled shape admits.
    let too_long = "p".repeat(GROUP_TEXT_MAX_LEN + 1);
    let rows: Vec<(&str, Vec<Value>)> = vec![
        (
            "a path naming the group's identity",
            vec![json!({ "AttributePath": "groupId", "AttributeValue": ABSENT_GROUP_ID })],
        ),
        (
            "a path naming the service-maintained revision",
            vec![json!({ "AttributePath": "revision", "AttributeValue": "7" })],
        ),
        (
            "a path naming the provider-issued external ids",
            vec![json!({ "AttributePath": "externalIds", "AttributeValue": "issuer" })],
        ),
        (
            "a path naming a user attribute",
            vec![json!({ "AttributePath": "nickName", "AttributeValue": "nick" })],
        ),
        (
            "a sub-attribute of a group attribute",
            vec![json!({ "AttributePath": "displayName.formatted", "AttributeValue": "x" })],
        ),
        (
            "a number where the attribute is a string",
            vec![json!({ "AttributePath": "description", "AttributeValue": 42 })],
        ),
        (
            "a boolean where the attribute is a string",
            vec![json!({ "AttributePath": "description", "AttributeValue": true })],
        ),
        (
            "an object where the attribute is a string",
            vec![json!({ "AttributePath": "description", "AttributeValue": { "value": "x" } })],
        ),
        (
            "a list where the attribute is a string",
            vec![json!({ "AttributePath": "description", "AttributeValue": ["x"] })],
        ),
        (
            "a value below the modelled minimum length",
            vec![json!({ "AttributePath": "description", "AttributeValue": "" })],
        ),
        (
            "a value one character past the modelled maximum length",
            vec![json!({ "AttributePath": "description", "AttributeValue": too_long })],
        ),
        (
            "a value carrying a character the modelled pattern excludes",
            vec![json!({ "AttributePath": "description", "AttributeValue": "bell\u{7}value" })],
        ),
        (
            // U+2000 EN QUAD is a space character, but not one of the three the
            // modelled class lists.
            "a value carrying a space character the modelled class omits",
            vec![json!({ "AttributePath": "description", "AttributeValue": "en\u{2000}quad" })],
        ),
        (
            "a second operation this service cannot apply",
            vec![
                json!({ "AttributePath": "description", "AttributeValue": "would have applied" }),
                json!({ "AttributePath": "externalIds", "AttributeValue": "issuer" }),
            ],
        ),
    ];
    for (case, operations) in rows {
        let display_name = format!("refusal {case}");
        let group = create(&router, STORE, &display_name, case).await;
        refused(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.group_id,
                "Operations": operations,
            }),
            400,
            "ValidationException",
            case,
        )
        .await;

        // Nothing was applied — not even the operations that preceded the one that
        // could not be.
        let reported = describe(&router, STORE, &group.group_id, case).await;
        assert_attributes(
            &reported,
            Some(&display_name),
            Some(CREATED_DESCRIPTION),
            case,
        );
        let revision = string(&reported, "Revision", case);
        assert_eq!(
            revision, group.revision,
            "case {case:?}: expected a refused update to leave the revision at {:?}, \
             got {revision:?}",
            group.revision
        );
    }
}

#[tokio::test]
async fn a_display_name_stays_a_uniqueness_claim_within_one_identity_store() {
    let router = emulator();
    let case = "a name another group of this store holds";
    // Two groups in one store: one holding the name, one to be renamed onto it.
    let held = "the held display name";
    let taken = create(&router, STORE, held, case).await;
    let group = create(&router, STORE, "the group being renamed", case).await;

    // A name another group of this store holds conflicts.
    refused(
        &router,
        "UpdateGroup",
        rename(STORE, &group.group_id, held),
        409,
        "ConflictException",
        case,
    )
    .await;
    let reported = describe(&router, STORE, &group.group_id, case).await;
    assert_attributes(
        &reported,
        Some("the group being renamed"),
        Some(CREATED_DESCRIPTION),
        case,
    );

    // A group's own current name is already its own claim.
    let case = "its own current display name";
    ok(
        &router,
        "UpdateGroup",
        rename(STORE, &group.group_id, "the group being renamed"),
        case,
    )
    .await;
    let reported = describe(&router, STORE, &group.group_id, case).await;
    assert_attributes(
        &reported,
        Some("the group being renamed"),
        Some(CREATED_DESCRIPTION),
        case,
    );

    // A claim is scoped to one identity store, and a name nothing in this store holds
    // is free.
    let case = "a name no group of this store holds";
    ok(
        &router,
        "UpdateGroup",
        rename(STORE, &group.group_id, "a name held nowhere"),
        case,
    )
    .await;
    // The rename was applied.
    let reported = describe(&router, STORE, &group.group_id, case).await;
    assert_attributes(
        &reported,
        Some("a name held nowhere"),
        Some(CREATED_DESCRIPTION),
        case,
    );

    // The rename above released "the group being renamed", so another group of the
    // same store can claim it now.
    let case = "a name an earlier rename released";
    ok(
        &router,
        "UpdateGroup",
        rename(STORE, &taken.group_id, "the group being renamed"),
        case,
    )
    .await;
    let reported = describe(&router, STORE, &taken.group_id, case).await;
    assert_attributes(
        &reported,
        Some("the group being renamed"),
        Some(CREATED_DESCRIPTION),
        case,
    );
}

#[tokio::test]
async fn the_group_an_update_names_must_exist_in_the_store_it_names() {
    let router = emulator();
    let group = create(&router, STORE, "the group to address", "setup").await;

    // Rows differ along the FORM of the identifiers: every `ResourceId` member also
    // accepts the resource's ARN, and the modelled pattern admits either letter case.
    let found: [(&str, String); 3] = [
        ("the group's own id", group.group_id.clone()),
        ("the group's ARN", group.group_arn.clone()),
        ("the group's id in upper case", group.group_id.to_uppercase()),
    ];
    for (case, group_id) in found {
        let updated = ok(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group_id,
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": case },
                ],
            }),
            case,
        )
        .await;
        let reported_id = string(&updated, "GroupId", case);
        assert_eq!(
            reported_id, group.group_id,
            "case {case:?}: expected the stored GroupId {:?}, got {reported_id:?}",
            group.group_id
        );
        let reported = describe(&router, STORE, &group.group_id, case).await;
        assert_attributes(&reported, Some("the group to address"), Some(case), case);
    }

    let missing: [(&str, &str, String); 3] = [
        (
            "a group id the store does not hold",
            STORE,
            ABSENT_GROUP_ID.to_string(),
        ),
        (
            "a store of another account",
            ABSENT_STORE,
            group.group_id.clone(),
        ),
        (
            "another account's other store",
            MISSING_STORE,
            group.group_id.clone(),
        ),
    ];
    for (case, store, group_id) in missing {
        refused(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": store,
                "GroupId": group_id,
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": "never applied" },
                ],
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
    }

    // None of the refusals above touched the group that does exist.
    let reported = describe(&router, STORE, &group.group_id, "after the refusals").await;
    assert_attributes(
        &reported,
        Some("the group to address"),
        Some("the group's id in upper case"),
        "after the refusals",
    );
}

/// One member, one value set: a value `CreateGroup` refuses for one of its string members
/// is refused by the `UpdateGroup` that writes that same member, and with the SAME clause.
/// The length bound and the character class belong to the member, not to the operation that
/// writes it, so the untyped `AttributeValue` an update carries is checked against exactly
/// the traits a create's modelled member carries.
///
/// The rows differ along which attribute is written and which constraint of its shape the
/// value fails: a control character the class excludes, a space character it does not list,
/// the length maximum and the length minimum.
#[tokio::test]
async fn an_update_refuses_a_value_the_create_of_the_same_member_refuses() {
    let router = emulator();
    // U+0007 BELL is a control character the modelled class excludes; U+2000 EN QUAD is a
    // space character, but not one of the three the class lists
    // (`smithy_query(query="shape",shape="GroupDisplayName,SensitiveStringType")`).
    let bell = format!("bell{}value", '\u{7}');
    let en_quad = format!("en{}quad", '\u{2000}');

    // (case, the CreateGroup member, the AttributePath an UpdateGroup names it by, value)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a description carrying a control character",
            "Description",
            "description",
            bell.clone(),
        ),
        (
            "a display name carrying a control character",
            "DisplayName",
            "displayName",
            bell,
        ),
        (
            "a description carrying a space character the class omits",
            "Description",
            "description",
            en_quad.clone(),
        ),
        (
            "a display name carrying a space character the class omits",
            "DisplayName",
            "displayName",
            en_quad,
        ),
        (
            "a description one past the modelled maximum length",
            "Description",
            "description",
            "d".repeat(GROUP_TEXT_MAX_LEN + 1),
        ),
        (
            "a description below the modelled minimum length",
            "Description",
            "description",
            String::new(),
        ),
    ];

    for (index, (case, member, path, value)) in rows.iter().enumerate() {
        let display_name = format!("parity {index}");
        let group = create(&router, STORE, &display_name, case).await;

        // The create side: the same value at the same member, refused at the request
        // boundary because a create carries it in a modelled string member.
        let mut request = json!({
            "IdentityStoreId": STORE,
            "DisplayName": format!("parity create {index}"),
        });
        request[*member] = json!(value);
        let created =
            refused(&router, "CreateGroup", request, 400, "ValidationException", case).await;

        let updated = refused(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.group_id,
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            400,
            "ValidationException",
            case,
        )
        .await;

        let (on_create, on_update) = (error_message(&created), error_message(&updated));
        assert_eq!(
            on_update, on_create,
            "case {case:?}: expected the update to be refused as the create is, \
             with {on_create:?}, got {on_update:?}"
        );
        // Both members are `@sensitive`, so neither refusal echoes what was sent.
        assert!(
            value.is_empty() || !on_update.contains(value.as_str()),
            "case {case:?}: expected no fragment of the rejected value in {on_update:?}"
        );
        // And nothing was applied: the group still holds what it was created with.
        let reported = describe(&router, STORE, &group.group_id, case).await;
        assert_attributes(
            &reported,
            Some(&display_name),
            Some(CREATED_DESCRIPTION),
            case,
        );
    }
}

/// One `Revision` value held against one group gets one answer whichever operation
/// takes it, and that answer turns on the NUMBER the value spells rather than on its
/// spelling: `UpdateGroup` and `DeleteGroup` document the same member with the same
/// meaning, and the modelled `ResourceRevision` is a decimal string.
///
/// The rows are [`crate::harness::PRECONDITION_ROWS`], the same table the user suites
/// walk, so a user and a group are held to one rule.
#[tokio::test]
async fn a_group_revision_value_means_the_same_to_the_update_and_to_the_delete() {
    for (case, precondition) in PRECONDITION_ROWS {
        let router = emulator();
        let display_name = format!("revision {case}");
        let group = create(&router, STORE, &display_name, case).await;
        // One update first, so the group has a current revision and a stale one.
        let settled = ok(
            &router,
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.group_id,
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": "before the row" },
                ],
            }),
            case,
        )
        .await;
        let current: u64 = string(&settled, "Revision", case)
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a decimal revision, got {settled}"));

        let mut update = json!({
            "IdentityStoreId": STORE,
            "GroupId": group.group_id,
            "Operations": [
                { "AttributePath": "description", "AttributeValue": "the row's own value" },
            ],
        });
        let mut delete = json!({ "IdentityStoreId": STORE, "GroupId": group.group_id });
        if let Some(member) = precondition.member(current) {
            update["Revision"] = json!(member.clone());
            delete["Revision"] = json!(member);
        }

        if precondition.holds() {
            let updated = ok(&router, "UpdateGroup", update, case).await;
            let revision = string(&updated, "Revision", case);
            assert_eq!(
                revision,
                (current + 1).to_string(),
                "case {case:?}: expected the applied update to report revision {}, \
                 got {revision:?}",
                current + 1
            );
            let reported = describe(&router, STORE, &group.group_id, case).await;
            assert_attributes(
                &reported,
                Some(&display_name),
                Some("the row's own value"),
                case,
            );
            // The same spelling of the revision the group is NOW at deletes it.
            if let Some(member) = precondition.member(current + 1) {
                delete["Revision"] = json!(member);
            }
            ok(&router, "DeleteGroup", delete, case).await;
            refused(
                &router,
                "DescribeGroup",
                json!({ "IdentityStoreId": STORE, "GroupId": group.group_id }),
                NOT_FOUND,
                "ResourceNotFoundException",
                case,
            )
            .await;
        } else {
            refused(&router, "UpdateGroup", update, 409, "ConflictException", case).await;
            refused(&router, "DeleteGroup", delete, 409, "ConflictException", case).await;
            // Neither refusal is a write: the group still holds what it had, at the
            // revision it had.
            let reported = describe(&router, STORE, &group.group_id, case).await;
            assert_attributes(
                &reported,
                Some(&display_name),
                Some("before the row"),
                case,
            );
            let reported_revision = string(&reported, "Revision", case);
            assert_eq!(
                reported_revision,
                current.to_string(),
                "case {case:?}: expected the group to stay at revision {current}, \
                 got {reported_revision:?}"
            );
        }
    }
}
