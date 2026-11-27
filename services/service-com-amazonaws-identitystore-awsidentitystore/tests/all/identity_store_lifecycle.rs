// SPDX-License-Identifier: CC0-1.0

//! The identity-store user / group / membership slice, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses, through the shared
//! harness in [`crate::harness`]: nothing reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass.
//!
//! The suites are TABLES: each row is one input along the axis the rule under test
//! generalises over (which profile members are present, name form and length, page
//! size, membership count, which item is deleted, id form). The `alice` /
//! `engineering` case from the bootstrap issue is only one row.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::harness::{emulator, error_message, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
/// The crate under test, aliased so a capacity bound can be named rather than restated.
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

/// The bounds of the modelled `MaxResults` range every `List*` operation of this
/// service shares (`smithy_query(query="operation",operation="ListUsers")` and
/// `operation="ListGroupMemberships"`: range 1..100, `@default: 100`).
pub(crate) const MAX_RESULTS_MIN: i64 = 1;
pub(crate) const MAX_RESULTS_MAX: i64 = 100;

/// The bounds of the modelled `UserName` length (1..128).
const USER_NAME_MIN_LEN: usize = 1;
const USER_NAME_MAX_LEN: usize = 128;

/// The user-profile members `CreateUser` accepts, from its modelled input
/// (`smithy_query(query="operation",operation="CreateUser")`). A row that leaves one
/// out must get a response that leaves it out too.
const PROFILE_MEMBERS: &[&str] = &[
    "UserName",
    "Name",
    "DisplayName",
    "NickName",
    "ProfileUrl",
    "Emails",
    "Addresses",
    "PhoneNumbers",
    "UserType",
    "Title",
    "PreferredLanguage",
    "Locale",
    "Timezone",
    "Photos",
    "Website",
    "Birthdate",
    "Roles",
];

/// Assert `id` is a lower-case hexadecimal UUID, the form the modelled `ResourceId`
/// pattern demands (`[0-9a-f]{8}-…-[0-9a-f]{12}`, from
/// `smithy_query(query="shape",shape="ResourceId")`).
pub(crate) fn assert_uuid(id: &str, member: &str, case: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    let shaped = groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
        && id == id.to_ascii_lowercase();
    assert!(
        shaped,
        "case {case:?}: expected {member} to be a lower-case hexadecimal UUID, got {id:?}"
    );
}

/// Assert `arn` is the ARN of `kind`/`id`: `arn:aws:identitystore:::<kind>/<id>`, the
/// form the modelled `ResourceArn` pattern demands, within its 60..100 length.
pub(crate) fn assert_resource_arn(arn: &str, kind: &str, id: &str, case: &str) {
    assert_eq!(
        arn,
        format!("arn:aws:identitystore:::{kind}/{id}"),
        "case {case:?}: expected the {kind} ARN of {id}, got {arn:?}"
    );
    assert!(
        (60..=100).contains(&arn.len()),
        "case {case:?}: expected an ARN within the modelled 60..100 length, \
         got {} characters in {arn:?}",
        arn.len()
    );
}

/// Whether `got` carries everything `sent` does.
///
/// Equality for scalars; for an object every key of `sent` must be present in `got`
/// with a value that in turn contains it. A member the model gives a default (an
/// `Email`'s `Primary`, say) may come back on a response that did not send it, which
/// is why this is containment rather than equality — the assertion that nothing was
/// invented is [`assert_absent`] below.
fn contains(sent: &Value, got: &Value) -> bool {
    match (sent, got) {
        (Value::Object(sent), Value::Object(got)) => sent
            .iter()
            .all(|(key, value)| got.get(key).is_some_and(|actual| contains(value, actual))),
        (Value::Array(sent), Value::Array(got)) => {
            sent.len() == got.len()
                && sent
                    .iter()
                    .zip(got)
                    .all(|(value, actual)| contains(value, actual))
        }
        _ => sent == got,
    }
}

/// Assert every member `sent` carries comes back in `got` unchanged.
pub(crate) fn assert_round_tripped(sent: &Map<String, Value>, got: &Value, case: &str) {
    for (member, value) in sent {
        if member == "IdentityStoreId" {
            continue;
        }
        let actual = got.get(member).unwrap_or(&Value::Null);
        assert!(
            contains(value, actual),
            "case {case:?}: expected {member} to come back as {value}, got {actual}"
        );
    }
}

/// Assert the response carries no member the request did not send: a profile is
/// reported, not synthesised.
pub(crate) fn assert_absent(sent: &Map<String, Value>, got: &Value, case: &str) {
    for member in PROFILE_MEMBERS {
        if sent.contains_key(*member) {
            continue;
        }
        let actual = got.get(*member).unwrap_or(&Value::Null);
        assert!(
            actual.is_null(),
            "case {case:?}: expected no {member} on a user created without one, got {actual}"
        );
    }
    // A user this API creates is ENABLED: `CreateUser` models no `UserStatus` member,
    // and the modelled enum's other value belongs to a status change no operation of
    // this service carries.
    let status = got.get("UserStatus").unwrap_or(&Value::Null);
    assert_eq!(
        status.as_str().unwrap_or_default(),
        "ENABLED",
        "case {case:?}: expected a created user to report UserStatus ENABLED, got {status}"
    );
}

/// A `UserName` for `case`'s row: the modelled `UserName` pattern
/// (`^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) admits letters, marks, symbols, numbers and
/// punctuation but NOT whitespace, so a case's spaces become hyphens.
pub(crate) fn user_name(case: &str, index: usize) -> String {
    format!("user-{}-{index}", case.replace(' ', "-"))
}

/// Create a user and return its `UserId`.
pub(crate) async fn create_user(
    router: &MultiServiceRouter,
    store: &str,
    user_name: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateUser",
        json!({ "IdentityStoreId": store, "UserName": user_name }),
        case,
    )
    .await;
    string(&created, "UserId", case)
}

/// Create a group and return its `GroupId`.
pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    store: &str,
    display_name: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateGroup",
        json!({ "IdentityStoreId": store, "DisplayName": display_name }),
        case,
    )
    .await;
    string(&created, "GroupId", case)
}

/// The `MembershipId`s `ListGroupMemberships` reports for `group`, walking every page
/// with `max_results` and asserting no page exceeds it and nothing repeats.
pub(crate) async fn walk_memberships(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({ "IdentityStoreId": store, "GroupId": group });
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListGroupMemberships", request, case).await;
        let items = page["GroupMemberships"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a GroupMemberships list, got {page}"))
            .clone();
        if let Some(size) = max_results {
            assert!(
                items.len() as i64 <= size,
                "case {case:?}: expected at most {size} memberships in a page, got {}",
                items.len()
            );
        }
        for item in &items {
            let membership_id = string(item, "MembershipId", case);
            assert!(
                !seen.contains(&membership_id),
                "case {case:?}: expected every membership exactly once, saw {membership_id} twice"
            );
            assert_eq!(
                item["GroupId"].as_str().unwrap_or_default(),
                group,
                "case {case:?}: expected the page's memberships to belong to {group}, got {item}"
            );
            assert_eq!(
                item["IdentityStoreId"].as_str().unwrap_or_default(),
                store,
                "case {case:?}: expected the page's memberships to name store {store}, got {item}"
            );
            assert_resource_arn(
                item["MembershipArn"].as_str().unwrap_or_default(),
                "membership",
                &membership_id,
                case,
            );
            seen.push(membership_id);
        }
        pages += 1;
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn a_created_user_is_reported_exactly_as_it_was_stored() {
    let router = emulator();
    // Rows differ along WHICH profile members the request carries, and the form and
    // size of their values; `Emails`, `Addresses`, `PhoneNumbers` and `Roles` are
    // `length 1..1` in the model and `Photos` is `1..3`.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the issue's recorded case",
            json!({
                "IdentityStoreId": STORE,
                "UserName": "alice",
                "DisplayName": "Alice Example",
                "Name": { "GivenName": "Alice", "FamilyName": "Example" },
                "Emails": [{ "Value": "alice@example.com", "Primary": true }],
            }),
        ),
        (
            "every modelled profile member at once",
            json!({
                "IdentityStoreId": STORE,
                "UserName": "bob.builder",
                "DisplayName": "Bob Builder",
                "Name": {
                    "Formatted": "Mr Bob T Builder Jr",
                    "GivenName": "Bob",
                    "MiddleName": "T",
                    "FamilyName": "Builder",
                    "HonorificPrefix": "Mr",
                    "HonorificSuffix": "Jr",
                },
                "NickName": "bobby",
                "ProfileUrl": "https://example.com/bob",
                "Emails": [{ "Value": "bob@example.com", "Type": "work", "Primary": false }],
                "Addresses": [{
                    "StreetAddress": "1 Example Way",
                    "Locality": "Exampleville",
                    "Region": "EX",
                    "PostalCode": "99999",
                    "Country": "US",
                    "Formatted": "1 Example Way, Exampleville EX 99999",
                    "Type": "work",
                    "Primary": true,
                }],
                "PhoneNumbers": [{ "Value": "+15550100", "Type": "mobile", "Primary": true }],
                "UserType": "employee",
                "Title": "Builder",
                "PreferredLanguage": "American English",
                "Locale": "en-US",
                "Timezone": "America/Los_Angeles",
                "Photos": [
                    { "Value": "https://example.com/1.png", "Type": "photo", "Display": "one", "Primary": true },
                    { "Value": "https://example.com/2.png" },
                    { "Value": "https://example.com/3.png" },
                ],
                "Website": "https://bob.example.com",
                "Birthdate": "1979-02-28",
                "Roles": [{ "Value": "builder", "Type": "primary", "Primary": true }],
            }),
        ),
        (
            "nothing but the required IdentityStoreId",
            json!({ "IdentityStoreId": STORE }),
        ),
        (
            "no UserName, so no uniqueness claim",
            json!({ "IdentityStoreId": STORE, "DisplayName": "Nameless One" }),
        ),
        (
            "non-ASCII letters and marks",
            json!({
                "IdentityStoreId": STORE,
                "UserName": "Ωmega.Ñuñez-Öst",
                "DisplayName": "Ωmega Ñuñez-Öst",
                "Name": { "GivenName": "Ωmega", "FamilyName": "Ñuñez-Öst" },
            }),
        ),
        (
            "UserName at the lower length bound",
            json!({ "IdentityStoreId": STORE, "UserName": "a".repeat(USER_NAME_MIN_LEN) }),
        ),
        (
            "UserName at the upper length bound",
            json!({ "IdentityStoreId": STORE, "UserName": "z".repeat(USER_NAME_MAX_LEN) }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request) in &rows {
        let sent = request.as_object().expect("a JSON object request").clone();
        let created = ok(&router, "CreateUser", request.clone(), case).await;

        let user_id = string(&created, "UserId", case);
        assert_uuid(&user_id, "UserId", case);
        assert_resource_arn(
            &string(&created, "UserArn", case),
            "user",
            &user_id,
            case,
        );
        assert_eq!(
            string(&created, "Revision", case),
            "1",
            "case {case:?}: expected a created user to be revision 1"
        );
        assert_eq!(
            string(&created, "IdentityStoreId", case),
            STORE,
            "case {case:?}: expected the response to name the identity store it was created in"
        );
        assert!(
            minted.insert(user_id.clone()),
            "case {case:?}: expected a fresh UserId, got {user_id} again"
        );

        // The round trip: read the user back through DescribeUser.
        let described = ok(
            &router,
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": user_id }),
            case,
        )
        .await;
        assert_round_tripped(&sent, &described, case);
        assert_absent(&sent, &described, case);
        assert_eq!(
            string(&described, "UserArn", case),
            string(&created, "UserArn", case),
            "case {case:?}: expected DescribeUser to report the ARN the create minted"
        );
        assert_eq!(
            string(&described, "Revision", case),
            "1",
            "case {case:?}: expected DescribeUser to report the stored revision"
        );
        for member in ["CreatedAt", "UpdatedAt"] {
            assert!(
                described.get(member).is_some_and(|value| value.is_number()),
                "case {case:?}: expected an epoch {member} timestamp, got {}",
                described.get(member).unwrap_or(&Value::Null)
            );
        }
    }
}

#[tokio::test]
async fn a_user_name_is_a_uniqueness_claim_and_a_store_that_does_not_exist_holds_none() {
    let router = emulator();
    // Rows differ along the form of the claimed name.
    let rows = [
        ("ASCII", "alice"),
        ("punctuation", "alice.o'hara-smith_1"),
        ("non-ASCII", "Ωmega.Ñuñez"),
        ("the longest name the model admits", &"q".repeat(USER_NAME_MAX_LEN)),
    ];

    for (case, user_name) in rows {
        let first = create_user(&router, STORE, user_name, case).await;

        refused(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": user_name }),
            409,
            "ConflictException",
            case,
        )
        .await;

        // The refused create stored nothing: the name still resolves to the first user
        // and that user is still readable.
        let described = ok(
            &router,
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": first }),
            case,
        )
        .await;
        assert_eq!(
            described["UserName"].as_str().unwrap_or_default(),
            user_name,
            "case {case:?}: expected the first user to keep the claimed UserName"
        );

        // The claim is scoped to the identity store: a store that does not exist has no
        // namespace to claim a name in, so the same create addressed to one is refused
        // as not found rather than stored.
        refused(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": MISSING_STORE, "UserName": user_name }),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_created_group_is_reported_exactly_as_it_was_stored() {
    let router = emulator();
    // Rows differ along which optional members the request carries and their size.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the issue's recorded case",
            json!({
                "IdentityStoreId": STORE,
                "DisplayName": "engineering",
                "Description": "Eng team",
            }),
        ),
        (
            "no description",
            json!({ "IdentityStoreId": STORE, "DisplayName": "security" }),
        ),
        (
            "nothing but the required IdentityStoreId",
            json!({ "IdentityStoreId": STORE }),
        ),
        (
            "non-ASCII display name and a long description",
            json!({
                "IdentityStoreId": STORE,
                "DisplayName": "Ωperations — Ñorth",
                "Description": "d".repeat(1024),
            }),
        ),
    ];

    for (case, request) in &rows {
        let sent = request.as_object().expect("a JSON object request").clone();
        let created = ok(&router, "CreateGroup", request.clone(), case).await;
        let group_id = string(&created, "GroupId", case);
        assert_uuid(&group_id, "GroupId", case);
        assert_resource_arn(
            &string(&created, "GroupArn", case),
            "group",
            &group_id,
            case,
        );
        assert_eq!(
            string(&created, "Revision", case),
            "1",
            "case {case:?}: expected a created group to be revision 1"
        );

        let described = ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": group_id }),
            case,
        )
        .await;
        assert_round_tripped(&sent, &described, case);
        for member in ["DisplayName", "Description"] {
            if !sent.contains_key(member) {
                let actual = described.get(member).unwrap_or(&Value::Null);
                assert!(
                    actual.is_null(),
                    "case {case:?}: expected no {member} on a group created without one, got {actual}"
                );
            }
        }
        assert_eq!(
            string(&described, "GroupArn", case),
            string(&created, "GroupArn", case),
            "case {case:?}: expected DescribeGroup to report the ARN the create minted"
        );
    }

    // A second group claiming a display name another group holds conflicts.
    for (case, display_name) in [("ASCII", "engineering"), ("non-ASCII", "Ωperations — Ñorth")] {
        refused(
            &router,
            "CreateGroup",
            json!({ "IdentityStoreId": STORE, "DisplayName": display_name }),
            409,
            "ConflictException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn memberships_paginate_in_creation_order() {
    let router = emulator();
    // Rows differ along how many memberships the group holds and the page size the
    // client asks for — including both published `MaxResults` bounds.
    let rows: [(&str, usize, Option<i64>); 6] = [
        ("one member, one per page", 1, Some(MAX_RESULTS_MIN)),
        ("two members, one per page", 2, Some(MAX_RESULTS_MIN)),
        ("three members, two per page", 3, Some(2)),
        ("five members, the modelled default page", 5, None),
        ("four members, the largest page", 4, Some(MAX_RESULTS_MAX)),
        ("seven members, three per page", 7, Some(3)),
    ];

    for (case, members, max_results) in rows {
        let group = create_group(&router, STORE, &format!("group-{case}"), case).await;
        let mut created: Vec<String> = Vec::new();
        for index in 0..members {
            let user = create_user(&router, STORE, &user_name(case, index), case).await;
            let membership = ok(
                &router,
                "CreateGroupMembership",
                json!({
                    "IdentityStoreId": STORE,
                    "GroupId": group,
                    "MemberId": { "UserId": user },
                }),
                case,
            )
            .await;
            let membership_id = string(&membership, "MembershipId", case);
            assert_uuid(&membership_id, "MembershipId", case);
            assert_resource_arn(
                &string(&membership, "MembershipArn", case),
                "membership",
                &membership_id,
                case,
            );
            created.push(membership_id);
        }

        let walked = walk_memberships(&router, STORE, &group, max_results, case).await;
        assert_eq!(
            walked, created,
            "case {case:?}: expected the memberships in creation order, got {walked:?}"
        );
    }
}

/// Every delete of this family is `@idempotent`
/// (`smithy_query(query="operation",operation="DeleteUser,DeleteGroup,DeleteGroupMembership")`:
/// `"idempotent": true`, each with an empty output shape), so a delete of a resource the
/// store no longer holds removes nothing and answers HTTP 200 with an empty body — and
/// the resource stays absent, which its reader still reports as not found.
#[tokio::test]
async fn an_idempotent_delete_of_a_resource_the_store_no_longer_holds_is_an_empty_success() {
    let router = emulator();
    let case = "idempotent deletes";
    // A well-formed `ResourceId` nothing ever minted, so the answer comes from state and
    // not from the boundary validator.
    const UNMINTED: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

    // One live resource of each class, so each row can be deleted and then re-deleted.
    let user = create_user(&router, STORE, "idempotent-delete", case).await;
    let group = create_group(&router, STORE, "idempotent-delete", case).await;
    let membership = string(
        &ok(
            &router,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            case,
        )
        .await,
        "MembershipId",
        case,
    );

    // (the delete, its id member, the ARN resource segment, the live id, the reader that
    // reports the resource, its id member)
    let classes: [(&str, &str, &str, String, &str); 3] = [
        (
            "DeleteGroupMembership",
            "MembershipId",
            "membership",
            membership,
            "DescribeGroupMembership",
        ),
        ("DeleteUser", "UserId", "user", user, "DescribeUser"),
        ("DeleteGroup", "GroupId", "group", group, "DescribeGroup"),
    ];

    for (operation, member, segment, live, reader) in classes {
        // Rows differ along WHICH id is deleted and in which of the forms the modelled
        // `ResourceId` admits: one nothing ever minted (bare, as the resource's ARN and
        // in upper case), then the live resource — whose first delete removes it and
        // whose second and third are the no-op this rule is about.
        let rows: [(&str, String); 6] = [
            ("an id nothing minted", UNMINTED.to_string()),
            (
                "an id nothing minted, by ARN",
                format!("arn:aws:identitystore:::{segment}/{UNMINTED}"),
            ),
            (
                "an id nothing minted, in upper case",
                UNMINTED.to_ascii_uppercase(),
            ),
            ("the live resource", live.clone()),
            ("the same resource again", live.clone()),
            (
                "the same resource again, by ARN",
                format!("arn:aws:identitystore:::{segment}/{live}"),
            ),
        ];
        for (row, identifier) in rows {
            let row_case = format!("{operation}: {row}");
            let answered = ok(
                &router,
                operation,
                json!({ "IdentityStoreId": STORE, member: identifier }),
                &row_case,
            )
            .await;
            assert_eq!(
                answered,
                json!({}),
                "case {row_case:?}: expected the empty success of an idempotent delete, \
                 got {answered}"
            );
        }

        // The repeats changed nothing: the resource is still absent, and its reader
        // still reports it as not found.
        let body = refused(
            &router,
            reader,
            json!({ "IdentityStoreId": STORE, member: live.clone() }),
            NOT_FOUND,
            "ResourceNotFoundException",
            &format!("{operation}: the deleted resource stays absent"),
        )
        .await;
        assert!(
            body.get("ResourceId").unwrap_or(&Value::Null).is_null(),
            "case {operation:?}: expected no ResourceId on the reader's refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_membership_removes_it_from_the_group_listing() {
    let router = emulator();
    // Rows differ along WHICH of the group's memberships is deleted.
    let rows = [("the first", 0usize), ("a middle one", 1), ("the last", 2)];

    for (case, victim) in rows {
        let group = create_group(&router, STORE, &format!("group-{case}"), case).await;
        let mut memberships: Vec<(String, String)> = Vec::new();
        for index in 0..3 {
            let user = create_user(&router, STORE, &user_name(case, index), case).await;
            let created = ok(
                &router,
                "CreateGroupMembership",
                json!({
                    "IdentityStoreId": STORE,
                    "GroupId": group,
                    "MemberId": { "UserId": user.clone() },
                }),
                case,
            )
            .await;
            memberships.push((string(&created, "MembershipId", case), user));
        }

        let (removed_id, removed_user) = memberships[victim].clone();
        let deleted = ok(
            &router,
            "DeleteGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": removed_id.clone() }),
            case,
        )
        .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: expected DeleteGroupMembership to answer an empty body"
        );

        let remaining = walk_memberships(&router, STORE, &group, Some(MAX_RESULTS_MIN), case).await;
        let expected: Vec<String> = memberships
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != victim)
            .map(|(_, (id, _))| id.clone())
            .collect();
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the surviving memberships in creation order, got {remaining:?}"
        );

        // Deleting it again removes nothing and answers the same empty success:
        // `DeleteGroupMembership` is `@idempotent`.
        let repeated = ok(
            &router,
            "DeleteGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": removed_id.clone() }),
            case,
        )
        .await;
        assert_eq!(
            repeated,
            json!({}),
            "case {case:?}: expected a repeated DeleteGroupMembership to answer an empty body"
        );
        let after_repeat =
            walk_memberships(&router, STORE, &group, Some(MAX_RESULTS_MIN), case).await;
        assert_eq!(
            after_repeat, expected,
            "case {case:?}: expected the repeated delete to change nothing, got {after_repeat:?}"
        );

        // And the pair is free again: the same user can rejoin the group.
        ok(
            &router,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group,
                "MemberId": { "UserId": removed_user },
            }),
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_membership_needs_both_of_its_ends_and_cannot_be_created_twice() {
    let router = emulator();
    let case = "membership preconditions";
    let group = create_group(&router, STORE, "engineering", case).await;
    let user = create_user(&router, STORE, "alice", case).await;
    // A well-formed id of a resource nothing created: the modelled patterns admit it,
    // so the refusal comes from state, not from the boundary validator.
    let missing = "0123abcd-4567-89ab-cdef-0123456789ab";

    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "unknown group",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": missing,
                "MemberId": { "UserId": user.clone() },
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "unknown member",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": missing },
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "unknown identity store",
            json!({
                "IdentityStoreId": MISSING_STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "unknown group on a list",
            json!({ "IdentityStoreId": STORE, "GroupId": missing }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
    ];
    let operations = [
        "CreateGroupMembership",
        "CreateGroupMembership",
        "CreateGroupMembership",
        "ListGroupMemberships",
    ];
    for ((row_case, request, status, error), operation) in rows.into_iter().zip(operations) {
        refused(&router, operation, request, status, error, row_case).await;
    }

    // A delete is `@idempotent`: a membership id the store does not hold is removed
    // nowhere and answered with the empty success rather than refused.
    let deleted = ok(
        &router,
        "DeleteGroupMembership",
        json!({ "IdentityStoreId": STORE, "MembershipId": missing }),
        "unknown membership on a delete",
    )
    .await;
    assert_eq!(
        deleted,
        json!({}),
        "case \"unknown membership on a delete\": expected the empty success, got {deleted}"
    );

    // The pair is unique: a repeat of a membership that exists conflicts.
    let created = ok(
        &router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "MemberId": { "UserId": user.clone() },
        }),
        case,
    )
    .await;
    let membership_id = string(&created, "MembershipId", case);
    refused(
        &router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "MemberId": { "UserId": user.clone() },
        }),
        409,
        "ConflictException",
        case,
    )
    .await;
    let listed = walk_memberships(&router, STORE, &group, None, case).await;
    assert_eq!(
        listed,
        vec![membership_id],
        "case {case:?}: expected the refused repeat to have created nothing"
    );
}

#[tokio::test]
async fn an_id_member_also_accepts_the_resources_arn() {
    let router = emulator();
    let case = "identifier forms";
    let user = create_user(&router, STORE, "alice", case).await;
    let group = create_group(&router, STORE, "engineering", case).await;

    // Rows differ along the FORM of the identifier: the bare id, the resource's ARN,
    // and the id in upper case — the modelled `ResourceId` pattern admits all three
    // (`^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?[A-Fa-f0-9]{8}-…$`)
    // and they all name the same resource.
    let user_forms = [
        ("bare id", user.clone()),
        ("resource ARN", format!("arn:aws:identitystore:::user/{user}")),
        ("upper-case id", user.to_ascii_uppercase()),
    ];
    for (form, identifier) in &user_forms {
        let described = ok(
            &router,
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": identifier }),
            form,
        )
        .await;
        assert_eq!(
            string(&described, "UserId", form),
            user,
            "case {form:?}: expected {identifier} to name the same user"
        );
    }

    let group_forms = [
        ("bare id", group.clone()),
        (
            "resource ARN",
            format!("arn:aws:identitystore:::group/{group}"),
        ),
        ("upper-case id", group.to_ascii_uppercase()),
    ];
    for (form, identifier) in &group_forms {
        let described = ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": identifier }),
            form,
        )
        .await;
        assert_eq!(
            string(&described, "GroupId", form),
            group,
            "case {form:?}: expected {identifier} to name the same group"
        );
    }

    // The same holds for the identity store, whose modelled pattern admits its ARN,
    // and for a `MemberId.UserId` naming the user by ARN.
    let store_arn = format!("arn:aws:identitystore::000000000000:identitystore/{STORE}");
    let described = ok(
        &router,
        "DescribeUser",
        json!({ "IdentityStoreId": store_arn, "UserId": user.clone() }),
        "identity store ARN",
    )
    .await;
    assert_eq!(
        string(&described, "IdentityStoreId", "identity store ARN"),
        STORE,
        "case \"identity store ARN\": expected the store ARN to name store {STORE}"
    );

    let membership = ok(
        &router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": format!("arn:aws:identitystore:::group/{group}"),
            "MemberId": { "UserId": format!("arn:aws:identitystore:::user/{user}") },
        }),
        "member by ARN",
    )
    .await;
    let membership_id = string(&membership, "MembershipId", "member by ARN");
    let listed = walk_memberships(&router, STORE, &group, None, "member by ARN").await;
    assert_eq!(
        listed,
        vec![membership_id.clone()],
        "case \"member by ARN\": expected the membership created by ARN to be listed"
    );
    let item = ok(
        &router,
        "ListGroupMemberships",
        json!({ "IdentityStoreId": STORE, "GroupId": group }),
        "member by ARN",
    )
    .await;
    assert_eq!(
        item["GroupMemberships"][0]["MemberId"],
        json!({ "UserId": user }),
        "case \"member by ARN\": expected the stored member to be reported by its id"
    );

    // Deleting by ARN removes the same membership.
    ok(
        &router,
        "DeleteGroupMembership",
        json!({
            "IdentityStoreId": STORE,
            "MembershipId": format!("arn:aws:identitystore:::membership/{membership_id}"),
        }),
        "membership by ARN",
    )
    .await;
    let listed = walk_memberships(&router, STORE, &group, None, "membership by ARN").await;
    assert!(
        listed.is_empty(),
        "case \"membership by ARN\": expected the group to hold no memberships, got {listed:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let case = "cursor handling";
    let group = create_group(&router, STORE, "engineering", case).await;
    let other_group = create_group(&router, STORE, "security", case).await;
    for index in 0..3 {
        let user = create_user(&router, STORE, &format!("user-{index}"), case).await;
        ok(
            &router,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group,
                "MemberId": { "UserId": user },
            }),
            case,
        )
        .await;
    }

    let first_page = ok(
        &router,
        "ListGroupMemberships",
        json!({ "IdentityStoreId": STORE, "GroupId": group, "MaxResults": MAX_RESULTS_MIN }),
        case,
    )
    .await;
    let token = string(&first_page, "NextToken", case);
    assert!(
        token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-+=/:_".contains(&b)),
        "case {case:?}: expected a NextToken matching the modelled \
         ^[-a-zA-Z0-9+=/:_]*$ pattern, got {token:?}"
    );

    // Rows differ along HOW the token is wrong.
    let rows = [
        ("not issued by this service", "bogus-token".to_string()),
        ("issued for another group", token.clone()),
    ];
    for (row_case, bad_token) in rows {
        let group_id = if row_case == "issued for another group" {
            other_group.clone()
        } else {
            group.clone()
        };
        refused(
            &router,
            "ListGroupMemberships",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group_id,
                "NextToken": bad_token,
            }),
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }

    // The token this service DID issue walks the rest of the list.
    let resumed = ok(
        &router,
        "ListGroupMemberships",
        json!({ "IdentityStoreId": STORE, "GroupId": group, "NextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        resumed["GroupMemberships"].as_array().map(Vec::len),
        Some(2),
        "case {case:?}: expected the remaining two memberships, got {resumed}"
    );
}

#[tokio::test]
async fn a_max_results_past_the_modelled_bound_is_refused() {
    let router = emulator();
    let case = "MaxResults past the bound";
    let group = create_group(&router, STORE, "engineering", case).await;
    for (row_case, size) in [
        ("below the lower bound", MAX_RESULTS_MIN - 1),
        ("above the upper bound", MAX_RESULTS_MAX + 1),
    ] {
        refused(
            &router,
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": group, "MaxResults": size }),
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }
}

#[tokio::test]
async fn caller_driven_growth_is_refused_at_the_capacity_bound() {
    // Rows are the four bounds a caller can push against. Each asserts the LAST create
    // that fits still succeeds, the next one is refused with the modelled
    // `ServiceQuotaExceededException` (HTTP 402), and the refusal stored nothing — the
    // held count is unchanged, so the store cannot be grown past the bound.
    //
    // The bounds themselves come from the crate, so a row tracks the constant rather
    // than restating a number.
    let router = emulator();

    // Users per identity store.
    let case = "users per identity store";
    for index in 0..identitystore::state::MAX_USERS_PER_STORE {
        create_user(&router, STORE, &format!("user-{index}"), case).await;
    }
    refused(
        &router,
        "CreateUser",
        json!({ "IdentityStoreId": STORE, "UserName": "one-too-many" }),
        402,
        "ServiceQuotaExceededException",
        case,
    )
    .await;

    // Groups per identity store, in the same (now user-full) store.
    let case = "groups per identity store";
    for index in 0..identitystore::state::MAX_GROUPS_PER_STORE {
        create_group(&router, STORE, &format!("group-{index}"), case).await;
    }
    refused(
        &router,
        "CreateGroup",
        json!({ "IdentityStoreId": STORE, "DisplayName": "one-too-many" }),
        402,
        "ServiceQuotaExceededException",
        case,
    )
    .await;

    // Members per group, which also bounds the list scanned under the shared lock. A
    // fresh emulator, so the user bound this row's members would otherwise sit under is
    // not the one being tested here.
    let case = "members per group";
    let router = emulator();
    let store = STORE;
    let group = create_group(&router, store, "crowded", case).await;
    let mut last_user = String::new();
    for index in 0..=identitystore::state::MAX_MEMBERSHIPS_PER_GROUP {
        last_user = create_user(&router, store, &format!("member-{index}"), case).await;
        if index == identitystore::state::MAX_MEMBERSHIPS_PER_GROUP {
            break;
        }
        ok(
            &router,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": store,
                "GroupId": group.clone(),
                "MemberId": { "UserId": last_user.clone() },
            }),
            case,
        )
        .await;
    }
    refused(
        &router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": store,
            "GroupId": group.clone(),
            "MemberId": { "UserId": last_user },
        }),
        402,
        "ServiceQuotaExceededException",
        case,
    )
    .await;
    let listed = walk_memberships(&router, store, &group, Some(MAX_RESULTS_MAX), case).await;
    assert_eq!(
        listed.len(),
        identitystore::state::MAX_MEMBERSHIPS_PER_GROUP,
        "case {case:?}: expected the refused membership to have been stored nowhere"
    );
}

#[tokio::test]
async fn an_unknown_user_or_group_is_reported_as_not_found() {
    let router = emulator();
    // Rows differ along which read is made and whether the resource that is missing is
    // the user, the group or the identity store itself.
    let missing = "0123abcd-4567-89ab-cdef-0123456789ab";
    let known_user = create_user(&router, STORE, "alice", "seed").await;
    let rows: [(&str, &str, Value, &str); 4] = [
        (
            "unknown user in the store",
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": missing }),
            "USER",
        ),
        (
            "unknown group in the store",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": missing }),
            "GROUP",
        ),
        (
            "a known user, read through a store that does not exist",
            "DescribeUser",
            json!({ "IdentityStoreId": MISSING_STORE, "UserId": known_user }),
            "IDENTITY_STORE",
        ),
        (
            "a store that does not exist",
            "DescribeGroup",
            json!({ "IdentityStoreId": MISSING_STORE, "GroupId": missing }),
            "IDENTITY_STORE",
        ),
    ];
    for (case, operation, request, expected_type) in rows {
        let body = refused(
            &router,
            operation,
            request,
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        let resource_type = body["ResourceType"].as_str().unwrap_or_default();
        assert_eq!(
            resource_type, expected_type,
            "case {case:?}: expected ResourceType {expected_type}, got {body}"
        );
    }
}

/// AWS's fixed `ResourceNotFoundException` message for a `MembershipId` an identity
/// store does not hold: one constant sentence naming the identity store ("pool") and
/// the request member, independent of which id was probed (recorded behaviour, the same
/// text on every recorded rejection of that class).
const MEMBERSHIP_NOT_FOUND_MESSAGE: &str =
    "Group membership not found for given pool and membershipId.";

/// The members a `DescribeGroupMembership` response and a `ListGroupMemberships` entry
/// both carry, from the two modelled shapes
/// (`smithy_query(query="operation",operation="DescribeGroupMembership")` and
/// `smithy_query(query="shape",shape="GroupMembership")`). Both report the same stored
/// record, so they must agree on every one of them.
const SHARED_MEMBERSHIP_MEMBERS: &[&str] = &[
    "IdentityStoreId",
    "MembershipId",
    "MembershipArn",
    "GroupId",
    "MemberId",
    "CreatedAt",
    "UpdatedAt",
    "CreatedBy",
    "UpdatedBy",
];

/// Create a membership and return its `CreateGroupMembership` response.
async fn create_membership(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    user: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": store,
            "GroupId": group,
            "MemberId": { "UserId": user },
        }),
        case,
    )
    .await
}

/// `id` written in the identifier `form` a row asks for: the modelled `ResourceId`
/// pattern admits the bare id, the resource's ARN and either letter case, and all three
/// name the same membership.
fn membership_identifier(form: &str, id: &str) -> String {
    match form {
        "bare id" => id.to_string(),
        "membership ARN" => format!("arn:aws:identitystore:::membership/{id}"),
        "upper-case id" => id.to_ascii_uppercase(),
        other => panic!("unknown identifier form {other:?}"),
    }
}

#[tokio::test]
async fn a_created_membership_is_described_exactly_as_it_was_stored() {
    let router = emulator();
    // Rows differ along how many members the group holds, WHICH of them is described,
    // the form of the `MembershipId` the describe names, and whether the identity store
    // is named by id or by ARN. The single-membership create-then-describe of the
    // recorded reproduction is the first row.
    let rows: [(&str, usize, usize, &str, bool); 6] = [
        ("one member, bare id", 1, 0, "bare id", false),
        ("three members, the first by ARN", 3, 0, "membership ARN", false),
        (
            "three members, a middle one upper-cased",
            3,
            1,
            "upper-case id",
            false,
        ),
        ("three members, the last by id", 3, 2, "bare id", false),
        ("two members, the store by ARN", 2, 1, "bare id", true),
        (
            "five members, membership and store by ARN",
            5,
            4,
            "membership ARN",
            true,
        ),
    ];
    let store_arn = format!("arn:aws:identitystore::000000000000:identitystore/{STORE}");

    for (case, members, described, form, store_by_arn) in rows {
        let group = create_group(&router, STORE, &format!("group-{case}"), case).await;
        let mut created: Vec<(String, String, String)> = Vec::new();
        for index in 0..members {
            let user = create_user(&router, STORE, &user_name(case, index), case).await;
            let response = create_membership(&router, STORE, &group, &user, case).await;
            created.push((
                string(&response, "MembershipId", case),
                string(&response, "MembershipArn", case),
                user,
            ));
        }

        let (membership_id, membership_arn, member) = created[described].clone();
        let store = if store_by_arn {
            store_arn.clone()
        } else {
            STORE.to_string()
        };
        let request = json!({
            "IdentityStoreId": store,
            "MembershipId": membership_identifier(form, &membership_id),
        });
        let body = ok(&router, "DescribeGroupMembership", request.clone(), case).await;

        // Every member comes from the stored record: the ids are the canonical ones the
        // create minted, whatever form the request named them in.
        let reported_id = string(&body, "MembershipId", case);
        assert_eq!(
            reported_id, membership_id,
            "case {case:?}: expected the membership minted as {membership_id}, got {reported_id:?}"
        );
        assert_uuid(&reported_id, "MembershipId", case);
        let reported_arn = string(&body, "MembershipArn", case);
        assert_eq!(
            reported_arn, membership_arn,
            "case {case:?}: expected the ARN minted with the membership ({membership_arn}), \
             got {reported_arn:?}"
        );
        assert_resource_arn(&reported_arn, "membership", &membership_id, case);
        let reported_store = string(&body, "IdentityStoreId", case);
        assert_eq!(
            reported_store, STORE,
            "case {case:?}: expected the describe to report store {STORE}, got {reported_store:?}"
        );
        let reported_group = string(&body, "GroupId", case);
        assert_eq!(
            reported_group, group,
            "case {case:?}: expected the membership's group {group}, got {reported_group:?}"
        );
        assert_eq!(
            body["MemberId"],
            json!({ "UserId": member }),
            "case {case:?}: expected the stored member {member}, got {}",
            body["MemberId"]
        );

        // The timestamps are the record's own: present, numeric, and equal on a
        // membership nothing has updated since it was created.
        for member_name in ["CreatedAt", "UpdatedAt"] {
            let value = &body[member_name];
            assert!(
                value.as_f64().is_some_and(|seconds| seconds > 0.0),
                "case {case:?}: expected a positive epoch {member_name}, got {value}"
            );
        }
        assert_eq!(
            body["CreatedAt"], body["UpdatedAt"],
            "case {case:?}: expected UpdatedAt to equal CreatedAt on a membership \
             nothing updated, got {} and {}",
            body["CreatedAt"], body["UpdatedAt"]
        );
        // The membership records the principal that created it, so the describe reports
        // a populated `CreatedBy`; `UpdatedBy` reports the LAST write, which for a
        // membership (the API models no membership update) is that same create — just as
        // its `UpdatedAt` is its `CreatedAt`.
        assert!(
            body.get("CreatedBy")
                .and_then(Value::as_str)
                .is_some_and(|principal| !principal.is_empty()),
            "case {case:?}: expected CreatedBy populated on a membership the API \
             created, got {}",
            body.get("CreatedBy").unwrap_or(&Value::Null)
        );
        assert_eq!(
            body.get("UpdatedBy"),
            body.get("CreatedBy"),
            "case {case:?}: expected UpdatedBy to report the create as the last write, \
             got {}",
            body["UpdatedBy"]
        );

        // The describe and the listing report ONE record, so they agree member by
        // member.
        let page = ok(
            &router,
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": group }),
            case,
        )
        .await;
        let listed = page["GroupMemberships"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["MembershipId"] == json!(membership_id))
            })
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {membership_id} in the listing, got {page}")
            });
        for member_name in SHARED_MEMBERSHIP_MEMBERS {
            assert_eq!(
                body[*member_name], listed[*member_name],
                "case {case:?}: expected the describe and the listing to agree on \
                 {member_name}, got {} and {}",
                body[*member_name], listed[*member_name]
            );
        }

        // A read changes nothing: the same request answers the same body again.
        let again = ok(&router, "DescribeGroupMembership", request, case).await;
        assert_eq!(
            again, body,
            "case {case:?}: expected a repeated describe to answer the same body, got {again}"
        );
    }
}

#[tokio::test]
async fn a_membership_the_store_does_not_hold_is_reported_as_not_found() {
    let router = emulator();
    let case = "seed";
    let group = create_group(&router, STORE, "engineering", case).await;
    let alice = create_user(&router, STORE, "alice", case).await;
    let live = string(
        &create_membership(&router, STORE, &group, &alice, case).await,
        "MembershipId",
        case,
    );
    let bob = create_user(&router, STORE, "bob", case).await;
    let deleted = string(
        &create_membership(&router, STORE, &group, &bob, case).await,
        "MembershipId",
        case,
    );
    ok(
        &router,
        "DeleteGroupMembership",
        json!({ "IdentityStoreId": STORE, "MembershipId": deleted.clone() }),
        case,
    )
    .await;
    // A well-formed id of a membership nothing created: the modelled pattern admits it,
    // so the refusal comes from state rather than from the boundary validator.
    let missing = "0123abcd-4567-89ab-cdef-0123456789ab";

    // Rows differ along WHY the read finds no membership: the id names none, it names
    // one that was deleted, or the identity store the request is addressed to does not
    // exist at all — which is a different missing resource.
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "a well-formed id nothing created",
            STORE,
            missing,
            "GROUP_MEMBERSHIP",
        ),
        (
            "a membership that was deleted",
            STORE,
            deleted.as_str(),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a live membership, through a store that does not exist",
            MISSING_STORE,
            live.as_str(),
            "IDENTITY_STORE",
        ),
        (
            "a store that does not exist",
            MISSING_STORE,
            missing,
            "IDENTITY_STORE",
        ),
    ];
    for (row, store, membership, expected_type) in rows {
        let body = refused(
            &router,
            "DescribeGroupMembership",
            json!({ "IdentityStoreId": store, "MembershipId": membership }),
            NOT_FOUND,
            "ResourceNotFoundException",
            row,
        )
        .await;
        let resource_type = body["ResourceType"].as_str().unwrap_or_default();
        assert_eq!(
            resource_type, expected_type,
            "case {row:?}: expected the modelled ResourceType {expected_type}, got {body}"
        );
        let message = error_message(&body);
        if expected_type == "GROUP_MEMBERSHIP" {
            assert_eq!(
                message, MEMBERSHIP_NOT_FOUND_MESSAGE,
                "case {row:?}: expected the service's fixed not-found message, got {message:?}"
            );
        } else {
            // The store-not-found message is the recorded fixed text followed by the
            // `IdentityStoreId` the request sent, which `crate::state` renders once.
            assert_eq!(
                message,
                identitystore::state::identity_store_not_found_message(store),
                "case {row:?}: expected the identity store's not-found message, \
                 got {message:?}"
            );
        }
    }

    // A refused read left the store as it was: the live membership still describes.
    let still_there = ok(
        &router,
        "DescribeGroupMembership",
        json!({ "IdentityStoreId": STORE, "MembershipId": live.clone() }),
        case,
    )
    .await;
    assert_eq!(
        string(&still_there, "MembershipId", case),
        live,
        "case {case:?}: expected the live membership to survive the refused reads, \
         got {still_there}"
    );
}

/// The names the model reserves, so neither a user nor a group may be named one.
///
/// Restated here, rather than read off the crate, so this suite pins the catalog the
/// service applies: the two members that NAME an identity —
/// `CreateGroupRequest$DisplayName` and `CreateUserRequest$UserName`
/// (`smithy_query(query="doc",shape="CreateGroupRequest",member="DisplayName")`) — both
/// state that `Administrator` and `AWSAdministrators` are reserved names and can't be
/// used for users or groups.
const RESERVED_NAMES: [&str; 2] = ["Administrator", "AWSAdministrators"];

/// The rule a refusal states, as the model words it.
const RESERVED_NAME_MESSAGE: &str =
    "Administrator and AWSAdministrators are reserved names and can't be used for \
     users or groups";

/// The status the modelled `ValidationException` carries
/// (`smithy_query(query="shape",shape="ValidationException")`: `@httpError(400)`).
const INVALID_REQUEST: u16 = 400;

/// A `UniqueAttribute` alternate identifier naming `path` with `value`, the form
/// `GetGroupId` / `GetUserId` resolve a name with.
fn by_unique_attribute(path: &str, value: &str) -> Value {
    json!({ "UniqueAttribute": { "AttributePath": path, "AttributeValue": value } })
}

/// Every spelling of `name` that names the same identity: an identity name is not
/// case-exact, so a reserved name is reserved however it is cased.
fn spellings(name: &str) -> [(&'static str, String); 3] {
    [
        ("as the model writes it", name.to_string()),
        ("lower case", name.to_lowercase()),
        ("upper case", name.to_uppercase()),
    ]
}

/// What `DescribeGroup` reports for `group`.
async fn describe_group(router: &MultiServiceRouter, group: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeGroup",
        json!({ "IdentityStoreId": STORE, "GroupId": group }),
        case,
    )
    .await
}

/// What `DescribeUser` reports for `user`.
async fn describe_user(router: &MultiServiceRouter, user: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeUser",
        json!({ "IdentityStoreId": STORE, "UserId": user }),
        case,
    )
    .await
}

#[tokio::test]
async fn a_reserved_name_cannot_name_a_user_or_a_group() {
    let router = emulator();
    let seed = "the resources that keep their names";
    // A group and a user that already exist, so the rows that would RENAME an identity
    // have something to rename and every refusal can be shown to change nothing.
    let group = create_group(&router, STORE, "the group that keeps its name", seed).await;
    let user = create_user(&router, STORE, "the-user-that-keeps-its-name", seed).await;
    let group_before = describe_group(&router, &group, seed).await;
    let user_before = describe_user(&router, &user, seed).await;

    for reserved in RESERVED_NAMES {
        for (spelling, name) in spellings(reserved) {
            let case = format!("{reserved} written in {spelling}");
            let case = case.as_str();
            // Every request that would give an identity this name: the two creates that
            // mint one under it and the two updates that would rename one to it.
            let rows = [
                (
                    "CreateGroup",
                    json!({ "IdentityStoreId": STORE, "DisplayName": name }),
                ),
                (
                    "CreateUser",
                    json!({ "IdentityStoreId": STORE, "UserName": name }),
                ),
                (
                    "UpdateGroup",
                    json!({
                        "IdentityStoreId": STORE,
                        "GroupId": group,
                        "Operations": [
                            { "AttributePath": "displayName", "AttributeValue": name },
                        ],
                    }),
                ),
                (
                    "UpdateUser",
                    json!({
                        "IdentityStoreId": STORE,
                        "UserId": user,
                        "Operations": [
                            { "AttributePath": "userName", "AttributeValue": name },
                        ],
                    }),
                ),
            ];
            for (operation, request) in rows {
                let body = refused(
                    &router,
                    operation,
                    request,
                    INVALID_REQUEST,
                    "ValidationException",
                    case,
                )
                .await;
                let message = error_message(&body);
                assert_eq!(
                    message, RESERVED_NAME_MESSAGE,
                    "case {case:?}: expected {operation} to state the reserved-name rule, \
                     got {message:?}"
                );
            }

            // Nothing was stored under the refused name: it resolves to no group and to
            // no user.
            refused(
                &router,
                "GetGroupId",
                json!({
                    "IdentityStoreId": STORE,
                    "AlternateIdentifier": by_unique_attribute("displayName", &name),
                }),
                NOT_FOUND,
                "ResourceNotFoundException",
                case,
            )
            .await;
            refused(
                &router,
                "GetUserId",
                json!({
                    "IdentityStoreId": STORE,
                    "AlternateIdentifier": by_unique_attribute("userName", &name),
                }),
                NOT_FOUND,
                "ResourceNotFoundException",
                case,
            )
            .await;
        }
    }

    // The refused updates applied nothing: both resources are reported exactly as they
    // were, revision included.
    let group_after = describe_group(&router, &group, seed).await;
    assert_eq!(
        group_after, group_before,
        "case {seed:?}: expected the refused renames to leave the group untouched, \
         got {group_after}"
    );
    let user_after = describe_user(&router, &user, seed).await;
    assert_eq!(
        user_after, user_before,
        "case {seed:?}: expected the refused renames to leave the user untouched, \
         got {user_after}"
    );
}

#[tokio::test]
async fn a_name_the_model_does_not_reserve_names_a_user_or_a_group() {
    let router = emulator();
    // Rows are names a RESERVED name is a prefix, a suffix or a substring of, so the
    // rule above cannot be read as a substring match, plus one name sharing nothing
    // with either. None carries a space, because the modelled `UserName` pattern admits
    // none (`smithy_query(query="shape",shape="UserName")`).
    let rows = [
        ("the plural of one", "Administrators"),
        ("the singular of the other", "AWSAdministrator"),
        ("one with a suffix", "Administrator-2"),
        ("one with a prefix", "Deputy.Administrator"),
        ("a prefix of one", "Admin"),
        ("neither", "engineering"),
    ];

    for (case, name) in rows {
        let group = create_group(&router, STORE, name, case).await;
        let described = describe_group(&router, &group, case).await;
        assert_eq!(
            described["DisplayName"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: expected the group to be stored under {name:?}, got {described}"
        );

        let user = create_user(&router, STORE, name, case).await;
        let described = describe_user(&router, &user, case).await;
        assert_eq!(
            described["UserName"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: expected the user to be stored under {name:?}, got {described}"
        );
    }

    // And a rename to a name the model does not reserve still applies, so the rule
    // refuses the reserved names and nothing else.
    let case = "renamed to a name the model does not reserve";
    let group = create_group(&router, STORE, "the group before its rename", case).await;
    ok(
        &router,
        "UpdateGroup",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group,
            "Operations": [
                { "AttributePath": "displayName", "AttributeValue": "Administrators Guild" },
            ],
        }),
        case,
    )
    .await;
    let described = describe_group(&router, &group, case).await;
    assert_eq!(
        described["DisplayName"].as_str().unwrap_or_default(),
        "Administrators Guild",
        "case {case:?}: expected the rename to be applied, got {described}"
    );

    let user = create_user(&router, STORE, "the-user-before-its-rename", case).await;
    ok(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user,
            "Operations": [
                { "AttributePath": "userName", "AttributeValue": "AWSAdministrators.deputy" },
            ],
        }),
        case,
    )
    .await;
    let described = describe_user(&router, &user, case).await;
    assert_eq!(
        described["UserName"].as_str().unwrap_or_default(),
        "AWSAdministrators.deputy",
        "case {case:?}: expected the rename to be applied, got {described}"
    );
}

// ---------------------------------------------------------------------------
// The ONE paged walk every listing of this service answers from.
//
// `ListUsers`, `ListGroups`, `ListGroupMemberships`, `ListGroupMembershipsForMember` and
// `ListIdentityStores` are all `@paginated` over the same three members — `MaxResults`
// (range 1..100), an opaque `NextToken` in and the same `NextToken` out — so they answer
// one rule, and the cases below drive that one walk through EACH of them.
// ---------------------------------------------------------------------------

/// One listing, as the walk below drives it: the operation, the output member its items
/// arrive in, the member that identifies one item, the request that addresses it, the
/// items it must report, and the sentence a `NextToken` it cannot resume at is refused
/// with — the only request-visible difference between the listings.
struct Listing {
    case: &'static str,
    operation: &'static str,
    items_member: &'static str,
    id_member: &'static str,
    request: Value,
    expected: BTreeSet<String>,
    unresumable_token_message: &'static str,
}

/// Every item `listing` reports, in page order, walked at `max_results` items per page.
///
/// Each page is judged as it arrives: it carries no more items than it was asked for, a
/// `NextToken` only ever follows a FULL page and satisfies the modelled `NextToken`
/// pattern (`^[-a-zA-Z0-9+=/:_]*$`), nothing is reported twice, and the walk terminates.
async fn walk_listing(
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<i64>,
    case: &str,
) -> Vec<String> {
    let page_size = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;
    let mut walked: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    // A page reports at least one item, so a walk needs no more pages than the listing
    // holds items, plus the page that carries no cursor.
    let page_budget = listing.expected.len() + 1;
    for page_number in 1..=page_budget {
        let row = format!("{case}, page {page_number}");
        let mut request = listing.request.clone();
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, listing.operation, request, &row).await;
        let items = page[listing.items_member]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {row:?}: expected the required {} list, got {page}",
                    listing.items_member
                )
            });
        assert!(
            items.len() <= page_size,
            "case {row:?}: expected at most {page_size} items on a page, got {}",
            items.len()
        );
        for item in &items {
            let id = string(item, listing.id_member, &row);
            assert!(
                seen.insert(id.clone()),
                "case {row:?}: expected every item exactly once, saw {id} twice"
            );
            walked.push(id);
        }
        match page.get("NextToken").and_then(Value::as_str) {
            None => return walked,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    page_size,
                    "case {row:?}: expected a NextToken only after a full page of \
                     {page_size}, got {} items beside {next:?}",
                    items.len()
                );
                assert!(
                    next.bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-+=/:_".contains(&byte)),
                    "case {row:?}: expected a NextToken matching the modelled \
                     ^[-a-zA-Z0-9+=/:_]*$ pattern, got {next:?}"
                );
                token = Some(next.to_string());
            }
        }
    }
    panic!("case {case:?}: the paged walk did not finish within {page_budget} pages: {walked:?}")
}

/// Every listing of this service reports its whole set once, in an order its page size
/// does not change, and refuses a `NextToken` it cannot resume at with its own sentence.
///
/// The rows are the five listings — one for each handler that used to carry its own copy
/// of this walk — and the inner rows are page sizes, including both published `MaxResults`
/// bounds and an absent one.
#[tokio::test]
async fn every_listing_pages_through_the_one_shared_walk() {
    let router = emulator();
    let case = "shared walk";

    // Users, groups and memberships enough that every listing holds several items: the
    // member of the first group is also a member of every other group, so the member's
    // own listing and the group's listing report different sets.
    let mut users: Vec<String> = Vec::new();
    for index in 0..5 {
        users.push(create_user(&router, STORE, &user_name(case, index), case).await);
    }
    let mut groups: Vec<String> = Vec::new();
    for index in 0..3 {
        groups.push(create_group(&router, STORE, &format!("{case}-group-{index}"), case).await);
    }
    let mut group_memberships: Vec<String> = Vec::new();
    for user in &users {
        let created = create_membership(&router, STORE, &groups[0], user, case).await;
        group_memberships.push(string(&created, "MembershipId", case));
    }
    // The first user's membership of the first group is in BOTH membership listings; the
    // other users' memberships of that group are in the group's listing only, and this
    // user's memberships of the other groups are in the member's listing only.
    let mut member_memberships: Vec<String> = vec![group_memberships[0].clone()];
    for group in &groups[1..] {
        let created = create_membership(&router, STORE, group, &users[0], case).await;
        member_memberships.push(string(&created, "MembershipId", case));
    }
    // `ListIdentityStores` reports the stores this emulator holds a record of, so two
    // more stores are written to, leaving three for its walk.
    let mut stores: BTreeSet<String> = BTreeSet::new();
    stores.insert(STORE.to_string());
    for index in 0..2u32 {
        let store = format!("d-{:010x}", 0xd0 + index);
        create_group(&router, &store, &format!("{case}-marker-{index}"), case).await;
        stores.insert(store);
    }

    let listings = [
        Listing {
            case: "ListUsers",
            operation: "ListUsers",
            items_member: "Users",
            id_member: "UserId",
            request: json!({ "IdentityStoreId": STORE }),
            expected: users.iter().cloned().collect(),
            unresumable_token_message:
                "NextToken is not a cursor this service issued for this listing",
        },
        Listing {
            case: "ListGroups",
            operation: "ListGroups",
            items_member: "Groups",
            id_member: "GroupId",
            request: json!({ "IdentityStoreId": STORE }),
            expected: groups.iter().cloned().collect(),
            unresumable_token_message:
                "NextToken is not a cursor this service issued for this listing",
        },
        Listing {
            case: "ListGroupMemberships",
            operation: "ListGroupMemberships",
            items_member: "GroupMemberships",
            id_member: "MembershipId",
            request: json!({ "IdentityStoreId": STORE, "GroupId": groups[0] }),
            expected: group_memberships.iter().cloned().collect(),
            unresumable_token_message:
                "NextToken is not a cursor this service issued for this group",
        },
        Listing {
            case: "ListGroupMembershipsForMember",
            operation: "ListGroupMembershipsForMember",
            items_member: "GroupMemberships",
            id_member: "MembershipId",
            request: json!({ "IdentityStoreId": STORE, "MemberId": { "UserId": users[0] } }),
            expected: member_memberships.iter().cloned().collect(),
            unresumable_token_message:
                "NextToken is not a cursor this service issued for this member",
        },
        Listing {
            case: "ListIdentityStores",
            operation: "ListIdentityStores",
            items_member: "IdentityStores",
            id_member: "IdentityStoreId",
            request: json!({}),
            expected: stores.clone(),
            unresumable_token_message:
                "NextToken is not a cursor this service issued for this listing",
        },
    ];

    for listing in &listings {
        // One unpaged walk is the reference: the whole set, once.
        let whole = walk_listing(&router, listing, None, listing.case).await;
        assert_eq!(
            whole.iter().cloned().collect::<BTreeSet<String>>(),
            listing.expected,
            "case {:?}: expected the listing to report exactly what the store holds, got {whole:?}",
            listing.case
        );

        // Rows differ along the page size asked for: both published bounds of the
        // modelled range, and the sizes that split the set across several pages.
        for size in [MAX_RESULTS_MIN, 2, 3, MAX_RESULTS_MAX] {
            let row = format!("{} at {size} per page", listing.case);
            let paged = walk_listing(&router, listing, Some(size), &row).await;
            assert_eq!(
                paged, whole,
                "case {row:?}: expected the paged walk to report the same items in the \
                 same order as the unpaged one, got {paged:?} against {whole:?}"
            );
        }

        // A `NextToken` this listing cannot resume at is refused with this listing's own
        // sentence. Rows differ along HOW the token is wrong; each still satisfies the
        // modelled `NextToken` pattern, so the refusal is the handler's own.
        let foreign = hex::encode(b"0123abcd-4567-89ab-cdef-0123456789ab");
        for (row_case, token) in [
            ("a token this service did not mint", "not-a-cursor".to_string()),
            ("a well-formed token naming nothing", "deadbeef".to_string()),
            ("a cursor naming a resource outside this listing", foreign),
        ] {
            let row = format!("{}, {row_case}", listing.case);
            let mut request = listing.request.clone();
            request["NextToken"] = json!(token);
            let body = refused(
                &router,
                listing.operation,
                request,
                INVALID_REQUEST,
                "ValidationException",
                &row,
            )
            .await;
            let message = error_message(&body);
            assert_eq!(
                message, listing.unresumable_token_message,
                "case {row:?}: expected the listing's own cursor refusal, got {message:?}"
            );
        }

        // And one `MaxResults` past either published bound is refused, for every listing:
        // the modelled range is 1..100 and a value outside it is not coerced into it.
        for (row_case, size) in [
            ("a MaxResults below the modelled minimum", MAX_RESULTS_MIN - 1),
            ("a MaxResults above the modelled maximum", MAX_RESULTS_MAX + 1),
        ] {
            let row = format!("{}, {row_case}", listing.case);
            let mut request = listing.request.clone();
            request["MaxResults"] = json!(size);
            refused(
                &router,
                listing.operation,
                request,
                INVALID_REQUEST,
                "ValidationException",
                &row,
            )
            .await;
        }
    }
}
