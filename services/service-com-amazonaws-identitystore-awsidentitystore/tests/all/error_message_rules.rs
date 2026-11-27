// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, driven end to end over HTTP.
//!
//! Three rules are under test, each asserted over a TABLE whose rows differ along what
//! the rule generalises over (the member, the operation, the constraint class, the
//! value's form and size, the number of offending elements) — the cases a recording
//! pins are one row each:
//!
//! 1. a `ValidationException` reports the member path, the constraint it failed and —
//!    for a `@pattern` — the expression the service APPLIES: unanchored, with the ARN
//!    partition grammar in place of the model's wildcard, and with the offending value
//!    left out when the model marks that member `@sensitive`;
//! 2. violations on the scalar elements of a list member are reported ONCE, at the list
//!    member, with the whole constraint set its element shape declares — while a
//!    violation inside a STRUCT element keeps its own deeper path;
//! 3. a `ResourceNotFoundException` and a `ConflictException` carry the fixed message of
//!    the resource class / uniqueness claim, with nothing of the request and no id of a
//!    resource the caller did not name in it — and a not-found by resource id echoes no
//!    identifier in its typed members either, while the identity-store class names the
//!    `IdentityStoreId` the request sent, as AWS words that one.
//!
//! The harness is the lifecycle suite's: every case goes through `MultiServiceRouter`
//! as a client's `POST /` with `X-Amz-Target: AWSIdentityStore.<Operation>`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, ok, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{create_group, create_user};

/// A `ResourceId` the modelled pattern admits but that no create ever minted.
const UNKNOWN_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A second well-formed-but-unused `ResourceId`, so a case can name two.
const OTHER_UNKNOWN_ID: &str = "22222222-3333-4444-5555-666666666666";

// The expressions the service reports for the model's constrained string shapes: each
// is that shape's `@pattern` with the `^`/`$` anchors dropped (Smithy pattern semantics
// are a find, so the anchors are not part of the expression applied) and, for the two
// that admit an ARN, the partition grammar `aws(-[a-z]{1,5}){0,3}` in place of the
// model's `aws[a-z-]*` wildcard. Source:
// `smithy_query(query="shape",shape="ResourceId,IdentityStoreId,AttributePath,UserName,ExtensionName")`
// and `smithy_query(query="shape",shape="ListUsersRequest")` for `NextToken`.
const RESOURCE_ID_PATTERN: &str = r"(arn:aws(-[a-z]{1,5}){0,3}:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}";
const IDENTITY_STORE_ID_PATTERN: &str = r"(arn:aws(-[a-z]{1,5}){0,3}:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})";
const ATTRIBUTE_PATH_PATTERN: &str =
    r"(?:\p{L}+:\p{L}+:\p{L}+(?:\.\p{L}+){0,3}|\p{L}+(?:\.\p{L}+){0,2})";
const USER_NAME_PATTERN: &str = r"[\p{L}\p{M}\p{S}\p{N}\p{P}]+";
const EXTENSION_NAME_PATTERN: &str = r"aws:identitystore:[a-z]{1,20}";
const NEXT_TOKEN_PATTERN: &str = r"[-a-zA-Z0-9+=/:_]*";

/// The same for the two interchangeable sensitive text shapes, `GroupDisplayName` and
/// `SensitiveStringType`, which declare one `@pattern`
/// (`smithy_query(query="shape",shape="GroupDisplayName,SensitiveStringType")`). The model's
/// text spells the three space characters it admits LITERALLY — U+0020, U+00A0 (no-break
/// space) and U+3000 (ideographic space) — so they are written here as escapes the compiler
/// turns back into those characters, and the class names keep the single backslash the model
/// gives them.
const SENSITIVE_TEXT_PATTERN: &str = "[\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}\\t\\n\\r \u{a0}\u{3000}]+";

/// The modelled `UserName` length maximum (1..128) and the `GroupDisplayName` /
/// `SensitiveStringType` maximum (1..1024).
const USER_NAME_MAX_LEN: usize = 128;
const DISPLAY_NAME_MAX_LEN: usize = 1024;

/// The length MINIMUM every constrained text shape of this model declares: 1, so the empty
/// string is a value none of them admits.
const TEXT_MIN_LEN: usize = 1;

/// The separator the validation envelope puts between a clause's two halves. A test
/// value spells it to prove the renderer never parses its own rendered sentence.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// This service's Smithy model, so a test can derive a model fact the renderer restates
/// (its `@sensitive` member set, its operation count) instead of trusting the copy.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-identitystore-awsidentitystore.json"
));

/// An extension name that fails BOTH element constraints the list declares: past the
/// 50-character maximum and outside the pattern.
fn over_long_extension() -> String {
    "not-an-extension-".repeat(4)
}

/// The error message of a refusal, whichever casing the envelope carries: a framework
/// refusal renders `message`, a modelled error the model's own `Message` member.
fn message_of(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// Call `operation` expecting `status` and an error code naming `error`, and return the
/// message it was refused with.
async fn refusal_message(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> String {
    let (actual_status, code, parsed) = call(router, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected {operation} to answer {status} {error}, \
         got {actual_status} {code:?} with {parsed}"
    );
    assert!(
        code.contains(error),
        "case {case:?}: expected {operation} to answer {error}, got {code:?} with {parsed}"
    );
    message_of(&parsed, case)
}

/// The envelope a single-violation refusal carries.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a value the service shows, at `path`, that failed `requirement`.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause for a value the service does NOT show — a `@sensitive` member's.
fn redacted_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement that reports a `@pattern`.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The composite requirement a list member carries: everything its element shape
/// declares, then that an element must not be null.
fn element_requirement(max_len: usize, min_len: usize, pattern: &str) -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to {max_len}, \
         Member must have length greater than or equal to {min_len}, \
         Member must satisfy regular expression pattern: {pattern}, Member must not be null]"
    )
}

#[tokio::test]
async fn a_constraint_violation_reports_the_expression_the_service_applies() {
    let router = emulator();
    let long_user_name = "u".repeat(USER_NAME_MAX_LEN + 1);
    let long_display_name = "g".repeat(DISPLAY_NAME_MAX_LEN + 1);

    // (case, operation, request, expected message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a resource id at the operation's own member",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": "not-a-valid-resource-id!!" }),
            one_error(&clause(
                "not-a-valid-resource-id!!",
                "groupId",
                &pattern_requirement(RESOURCE_ID_PATTERN),
            )),
        ),
        (
            "the same shape through another operation's member",
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": "nope!" }),
            one_error(&clause(
                "nope!",
                "userId",
                &pattern_requirement(RESOURCE_ID_PATTERN),
            )),
        ),
        (
            "an identity store id on a reporting operation",
            "DescribeGroup",
            json!({ "IdentityStoreId": "not-a-valid-identity-store-id", "GroupId": UNKNOWN_ID }),
            one_error(&clause(
                "not-a-valid-identity-store-id",
                "identityStoreId",
                &pattern_requirement(IDENTITY_STORE_ID_PATTERN),
            )),
        ),
        (
            "an identity store id on a mutating operation",
            "CreateUser",
            json!({ "IdentityStoreId": "d-NOTAVALIDSTORE", "UserName": "alice" }),
            one_error(&clause(
                "d-NOTAVALIDSTORE",
                "identityStoreId",
                &pattern_requirement(IDENTITY_STORE_ID_PATTERN),
            )),
        ),
        (
            "an attribute path inside a union inside a structure",
            "GetUserId",
            json!({
                "IdentityStoreId": STORE,
                "AlternateIdentifier": {
                    "UniqueAttribute": { "AttributePath": "userName123", "AttributeValue": "x" },
                },
            }),
            one_error(&clause(
                "userName123",
                "alternateIdentifier.uniqueAttribute.attributePath",
                &pattern_requirement(ATTRIBUTE_PATH_PATTERN),
            )),
        ),
        (
            "an attribute path inside a structure element of a list",
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": UNKNOWN_ID,
                "Operations": [{ "AttributePath": "1bad", "AttributeValue": "x" }],
            }),
            one_error(&clause(
                "1bad",
                "operations.1.member.attributePath",
                &pattern_requirement(ATTRIBUTE_PATH_PATTERN),
            )),
        ),
        (
            "a pagination token, whose pattern carries no ARN",
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "NextToken": "not a token!" }),
            one_error(&clause(
                "not a token!",
                "nextToken",
                &pattern_requirement(NEXT_TOKEN_PATTERN),
            )),
        ),
        (
            "a sensitive member's pattern: the value is not echoed",
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": "alice example" }),
            one_error(&redacted_clause(
                "userName",
                &pattern_requirement(USER_NAME_PATTERN),
            )),
        ),
        (
            "a sensitive member's length: the value is not echoed either",
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": long_user_name.clone() }),
            one_error(&redacted_clause(
                "userName",
                &format!("Member must have length less than or equal to {USER_NAME_MAX_LEN}"),
            )),
        ),
        (
            "a sensitive member of another operation",
            "CreateGroup",
            json!({ "IdentityStoreId": STORE, "DisplayName": long_display_name.clone() }),
            one_error(&redacted_clause(
                "displayName",
                &format!("Member must have length less than or equal to {DISPLAY_NAME_MAX_LEN}"),
            )),
        ),
        (
            "a numeric range, which quotes no pattern at all",
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": UNKNOWN_ID, "MaxResults": 0 }),
            one_error(&clause(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        ),
        (
            "an absent required member still states its absence",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE }),
            one_error("Value null at 'groupId' failed to satisfy constraint: Member must not be null"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let message =
            refusal_message(&router, operation, request, 400, "ValidationException", case).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        // The two rules the whole table shares: no anchor from the model trait, and no
        // model-only partition wildcard, reaches a caller.
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: expected the reported pattern to be unanchored, got {message:?}"
        );
        assert!(
            !message.contains("aws[a-z-]*"),
            "case {case:?}: expected the runtime partition form, got {message:?}"
        );
    }

    // And no sensitive value the table sent appears in any answer.
    for (case, operation, request, value) in [
        (
            "a rejected sensitive user name",
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": "alice example" }),
            "alice example".to_string(),
        ),
        (
            "a rejected oversized sensitive user name",
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": long_user_name.clone() }),
            long_user_name.clone(),
        ),
        (
            "a rejected oversized sensitive display name",
            "CreateGroup",
            json!({ "IdentityStoreId": STORE, "DisplayName": long_display_name.clone() }),
            long_display_name.clone(),
        ),
    ] {
        let message =
            refusal_message(&router, operation, request, 400, "ValidationException", case).await;
        assert!(
            !message.contains(&value),
            "case {case:?}: expected the sensitive value to stay out of the message, got {message:?}"
        );
    }
}

/// The requirement a `@length` maximum states.
fn max_length_requirement(max_len: usize) -> String {
    format!("Member must have length less than or equal to {max_len}")
}

/// The requirement a `@length` minimum states.
fn min_length_requirement(min_len: usize) -> String {
    format!("Member must have length greater than or equal to {min_len}")
}

/// The envelope for the clauses one refused value produced, counting the CLAUSES: a value
/// that fails two of its member's constraints is two errors.
fn errors(clauses: &[String]) -> String {
    match clauses.len() {
        1 => one_error(&clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// Rule 1 and rule 2 hold for a violation whichever CODE PATH noticed it.
///
/// An `AttributeOperation`'s `AttributeValue` is modelled as a `document`
/// (`smithy_query(query="shape",shape="AttributeOperation")`), so the request boundary
/// applies no modelled constraint to it: the `@length` and the `@pattern` of the member an
/// operation WRITES are applied where the value is decoded, inside the operation. That
/// refusal is still this service's `ValidationException`, so it is worded by the service's
/// one renderer — the clause names the member the value was written to and the constraint
/// class it failed, reports a `@pattern` as the expression the service APPLIES (unanchored),
/// and states no value fragment at all for a member the model marks `@sensitive`.
///
/// The rows differ along the operation, the attribute written, the modelled shape it
/// targets, the constraint class the value fails, and the value's form and size.
#[tokio::test]
async fn an_attribute_value_is_refused_in_the_same_words_as_the_member_it_writes() {
    let router = emulator();
    let case = "the resources an attribute-value case needs";
    let group = create_group(&router, STORE, "attribute value wording", case).await;
    let user = create_user(&router, STORE, "attribute-value-wording", case).await;

    // U+0007 BELL is a control character neither class admits; U+2000 EN QUAD is a space
    // character, but not one of the three the sensitive text class lists.
    let bell = format!("bell{}value", '\u{7}');
    let en_quad = format!("en{}quad", '\u{2000}');
    let long_text = "d".repeat(DISPLAY_NAME_MAX_LEN + 1);
    let long_user_name = "u".repeat(USER_NAME_MAX_LEN + 1);

    // (case, operation, attribute path, value, the requirements the value fails). The
    // empty string fails two of its member's constraints — the length minimum and the
    // pattern, which admits no empty value — so those rows carry two.
    let rows: Vec<(&str, &str, &str, String, Vec<String>)> = vec![
        (
            "a group description outside the sensitive text class",
            "UpdateGroup",
            "description",
            bell.clone(),
            vec![pattern_requirement(SENSITIVE_TEXT_PATTERN)],
        ),
        (
            "a group display name carrying a space the class omits",
            "UpdateGroup",
            "displayName",
            en_quad,
            vec![pattern_requirement(SENSITIVE_TEXT_PATTERN)],
        ),
        (
            "a group description past the modelled maximum length",
            "UpdateGroup",
            "description",
            long_text.clone(),
            vec![max_length_requirement(DISPLAY_NAME_MAX_LEN)],
        ),
        (
            "a group display name below the modelled minimum length",
            "UpdateGroup",
            "displayName",
            String::new(),
            vec![
                min_length_requirement(TEXT_MIN_LEN),
                pattern_requirement(SENSITIVE_TEXT_PATTERN),
            ],
        ),
        (
            "a user name carrying a space, which its own class excludes",
            "UpdateUser",
            "userName",
            "ada lovelace".to_string(),
            vec![pattern_requirement(USER_NAME_PATTERN)],
        ),
        (
            "a user name past its own, shorter maximum length",
            "UpdateUser",
            "userName",
            long_user_name,
            vec![max_length_requirement(USER_NAME_MAX_LEN)],
        ),
        (
            "a user locale outside the sensitive text class",
            "UpdateUser",
            "locale",
            bell.clone(),
            vec![pattern_requirement(SENSITIVE_TEXT_PATTERN)],
        ),
        (
            "a sub-attribute of a user's name past the modelled maximum length",
            "UpdateUser",
            "name.familyName",
            long_text.clone(),
            vec![max_length_requirement(DISPLAY_NAME_MAX_LEN)],
        ),
        (
            "a user title below the modelled minimum length",
            "UpdateUser",
            "title",
            String::new(),
            vec![
                min_length_requirement(TEXT_MIN_LEN),
                pattern_requirement(SENSITIVE_TEXT_PATTERN),
            ],
        ),
    ];

    for (case, operation, path, value, requirements) in rows {
        let mut request = json!({
            "IdentityStoreId": STORE,
            "Operations": [{ "AttributePath": path, "AttributeValue": value }],
        });
        if operation == "UpdateGroup" {
            request["GroupId"] = json!(group);
        } else {
            request["UserId"] = json!(user);
        }
        let message =
            refusal_message(&router, operation, request, 400, "ValidationException", case).await;
        let clauses: Vec<String> = requirements
            .iter()
            .map(|requirement| redacted_clause(path, requirement))
            .collect();
        let expected = errors(&clauses);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: expected the reported pattern to be unanchored, got {message:?}"
        );
        // Every attribute either update writes targets a `@sensitive` shape, so no clause
        // of theirs opens with a value fragment — neither the value nor a stand-in for it.
        assert!(
            !message.contains("Value '"),
            "case {case:?}: expected no value fragment for a sensitive member, got {message:?}"
        );
        assert!(
            value.is_empty() || !message.contains(value.as_str()),
            "case {case:?}: expected the rejected value to stay out of the message, \
             got {message:?}"
        );
    }

    // The same wording from the other side: a value an attribute operation is refused for
    // is refused in the SAME words when a create carries it in that member's modelled
    // string, which is the path the boundary validator walks.
    for (index, (case, member, path, value)) in [
        (
            "a user name outside its class",
            "UserName",
            "userName",
            "ada lovelace".to_string(),
        ),
        ("a locale outside its class", "Locale", "locale", bell),
        (
            "a title past the modelled maximum length",
            "Title",
            "title",
            long_text,
        ),
        (
            "a nick name below the modelled minimum length",
            "NickName",
            "nickName",
            String::new(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut created = json!({
            "IdentityStoreId": STORE,
            "UserName": format!("wording-parity-{index}"),
        });
        created[member] = json!(value);
        let on_create =
            refusal_message(&router, "CreateUser", created, 400, "ValidationException", case).await;
        let on_update = refusal_message(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user,
                "Operations": [{ "AttributePath": path, "AttributeValue": value }],
            }),
            400,
            "ValidationException",
            case,
        )
        .await;
        assert_eq!(
            on_update, on_create,
            "case {case:?}: expected the update to be refused in the create's words, \
             {on_create:?}, got {on_update:?}"
        );
    }
}

#[tokio::test]
async fn list_element_violations_are_reported_once_at_the_list_member() {
    let router = emulator();
    let case = "a user to describe";
    let user = create_user(&router, STORE, "list-element-rules", case).await;

    // (case, operation, request, expected message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "one bad extension name",
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "Extensions": ["not:a:real:extension"] }),
            one_error(&clause(
                "[not:a:real:extension]",
                "extensions",
                &element_requirement(50, 1, EXTENSION_NAME_PATTERN),
            )),
        ),
        (
            "two bad extension names fold into ONE error",
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "Extensions": ["bad:one", "bad:two"] }),
            one_error(&clause(
                "[bad:one, bad:two]",
                "extensions",
                &element_requirement(50, 1, EXTENSION_NAME_PATTERN),
            )),
        ),
        (
            // One element failing TWO of the element constraints (over the 50-character
            // bound AND outside the pattern) is still one element: the clause lists it
            // once.
            "one element that fails two element constraints",
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "Extensions": [over_long_extension()] }),
            one_error(&clause(
                &format!("[{}]", over_long_extension()),
                "extensions",
                &element_requirement(50, 1, EXTENSION_NAME_PATTERN),
            )),
        ),
        (
            "the same list member on another operation",
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": user, "Extensions": ["nope"] }),
            one_error(&clause(
                "[nope]",
                "extensions",
                &element_requirement(50, 1, EXTENSION_NAME_PATTERN),
            )),
        ),
        (
            "a list of ids, whose element shape declares other bounds",
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": UNKNOWN_ID },
                "GroupIds": ["not-a-uuid"],
            }),
            one_error(&clause(
                "[not-a-uuid]",
                "groupIds",
                &element_requirement(100, 1, RESOURCE_ID_PATTERN),
            )),
        ),
        (
            "a violation inside a STRUCTURE element keeps its own path",
            "ListUsers",
            json!({
                "IdentityStoreId": STORE,
                "Filters": [{ "AttributePath": "1bad", "AttributeValue": "x" }],
            }),
            one_error(&clause(
                "1bad",
                "filters.1.member.attributePath",
                &pattern_requirement(ATTRIBUTE_PATH_PATTERN),
            )),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let message =
            refusal_message(&router, operation, request, 400, "ValidationException", case).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_that_is_not_there_is_reported_with_its_classs_fixed_message() {
    let router = emulator();
    let case = "the resources a not-found case needs";
    let user = create_user(&router, STORE, "not-found-rules", case).await;
    let group = create_group(&router, STORE, "not-found-rules", case).await;

    // (case, operation, request, expected message, the ids the request named)
    let rows: Vec<(&str, &str, Value, &str, Vec<String>)> = vec![
        (
            "a group id no create minted, in a store that exists",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": UNKNOWN_ID }),
            "Group not found",
            vec![UNKNOWN_ID.to_string()],
        ),
        (
            "a group named by its ARN",
            "DescribeGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": format!("arn:aws:identitystore:::group/{UNKNOWN_ID}"),
            }),
            "Group not found",
            vec![UNKNOWN_ID.to_string()],
        ),
        (
            "a user id no create minted",
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": UNKNOWN_ID }),
            "User not found",
            vec![UNKNOWN_ID.to_string()],
        ),
        (
            "a membership's member end is missing",
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": UNKNOWN_ID },
            }),
            "User with provided member ID not found",
            vec![UNKNOWN_ID.to_string(), group.clone()],
        ),
        (
            "a membership's group end is missing",
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": UNKNOWN_ID,
                "MemberId": { "UserId": user.clone() },
            }),
            "Group not found",
            vec![UNKNOWN_ID.to_string(), user.clone()],
        ),
        (
            // The membership class's constant is the recorded sentence that names the
            // identity store ("pool") and the request member, not the id probed.
            "a membership id no create minted",
            "DescribeGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": UNKNOWN_ID }),
            identitystore::state::MEMBERSHIP_NOT_FOUND_MESSAGE,
            vec![UNKNOWN_ID.to_string()],
        ),
        (
            "a listing of a group that is not there",
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": UNKNOWN_ID }),
            "Group not found",
            vec![UNKNOWN_ID.to_string()],
        ),
        (
            // An operation that RESOLVES an identifier states the class's resolve
            // constant — the modelled `ResourceType` token — not the sentence a read by
            // resource id answers, and nothing of the identifier it could not resolve.
            "a display name no group holds",
            "GetGroupId",
            json!({
                "IdentityStoreId": STORE,
                "AlternateIdentifier": {
                    "UniqueAttribute": {
                        "AttributePath": "displayName",
                        "AttributeValue": "no-group-holds-this",
                    },
                },
            }),
            "GROUP not found.",
            vec!["no-group-holds-this".to_string()],
        ),
        (
            "an external id no group carries",
            "GetGroupId",
            json!({
                "IdentityStoreId": STORE,
                "AlternateIdentifier": {
                    "ExternalId": { "Issuer": "scim.example", "Id": "nobody" },
                },
            }),
            "GROUP not found.",
            vec!["scim.example".to_string(), "nobody".to_string()],
        ),
        (
            "a (group, member) pair no membership joins",
            "GetGroupMembershipId",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            "GROUP_MEMBERSHIP not found.",
            vec![group.clone(), user.clone()],
        ),
        (
            // Named through `MemberId` rather than through a user id of its own, so it
            // is the member end's constant, as `CreateGroupMembership` answers.
            "a member id no user carries, on a membership query",
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": UNKNOWN_ID },
                "GroupIds": [group.clone()],
            }),
            "User with provided member ID not found",
            vec![UNKNOWN_ID.to_string(), group.clone()],
        ),
        (
            // Named through `MemberId` on a listing too: one member end, one constant,
            // whichever operation asks.
            "a member id no user carries, on a membership listing",
            "ListGroupMembershipsForMember",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": UNKNOWN_ID },
            }),
            "User with provided member ID not found",
            vec![UNKNOWN_ID.to_string()],
        ),
    ];

    for (case, operation, request, expected, named) in rows {
        let message = refusal_message(
            &router,
            operation,
            request,
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        for id in named {
            assert!(
                !message.contains(&id),
                "case {case:?}: expected the probed id to stay out of the message, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_not_found_by_resource_id_carries_only_its_class_and_no_probed_identifier() {
    let router = emulator();
    let case = "the resources a typed-member case needs";
    let user = create_user(&router, STORE, "typed-members", case).await;
    let group = create_group(&router, STORE, "typed-members", case).await;

    // Rows are the reads BY RESOURCE ID, each named in every form the modelled
    // `ResourceId` admits (the bare UUID, the resource's ARN, the id in upper case —
    // `smithy_query(query="shape",shape="ResourceId")`), plus a live id read through the
    // wrong resource class. Whatever the row, the refusal reports the resource CLASS and
    // its fixed message and echoes no identifier: `ResourceId` and `Reason` are absent.
    type IdForm = fn(&str, &str) -> String;
    let forms: [(&str, IdForm); 3] = [
        ("the bare id", |_kind, id| id.to_string()),
        ("the resource ARN", |kind, id| {
            format!("arn:aws:identitystore:::{kind}/{id}")
        }),
        ("the id in upper case", |_kind, id| {
            id.to_ascii_uppercase()
        }),
    ];
    // (operation, the id member, the resource segment of the ARN form, the
    // `ResourceType` the class reports, its fixed message, the ids to probe)
    type NotFoundClass = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        [&'static str; 2],
    );
    let classes: [NotFoundClass; 3] = [
        (
            "DescribeUser",
            "UserId",
            "user",
            "USER",
            "User not found",
            [UNKNOWN_ID, OTHER_UNKNOWN_ID],
        ),
        (
            "DescribeGroup",
            "GroupId",
            "group",
            "GROUP",
            "Group not found",
            [UNKNOWN_ID, OTHER_UNKNOWN_ID],
        ),
        (
            "DescribeGroupMembership",
            "MembershipId",
            "membership",
            "GROUP_MEMBERSHIP",
            identitystore::state::MEMBERSHIP_NOT_FOUND_MESSAGE,
            [UNKNOWN_ID, OTHER_UNKNOWN_ID],
        ),
    ];

    for (operation, member, segment, resource_type, message, unknown_ids) in classes {
        // A live id of ANOTHER class is a row too: the resource that class does not hold
        // is still reported without the identifier that was probed.
        let live_of_another_class = if operation == "DescribeUser" {
            group.clone()
        } else {
            user.clone()
        };
        let probes: Vec<String> = unknown_ids
            .iter()
            .map(|id| (*id).to_string())
            .chain(std::iter::once(live_of_another_class))
            .collect();
        for probe in probes {
            for (form_case, form) in forms {
                let case = format!("{operation} by {form_case} ({probe})");
                let (status, code, body) = call(
                    &router,
                    operation,
                    json!({ "IdentityStoreId": STORE, member: form(segment, &probe) }),
                )
                .await;
                assert_eq!(
                    (status, code.ends_with("ResourceNotFoundException")),
                    (NOT_FOUND, true),
                    "case {case:?}: expected {NOT_FOUND} ResourceNotFoundException, \
                     got {status} {code} with {body}"
                );
                assert_eq!(
                    body["ResourceType"].as_str().unwrap_or_default(),
                    resource_type,
                    "case {case:?}: expected ResourceType {resource_type:?}, got {body}"
                );
                assert_eq!(
                    crate::harness::error_message(&body),
                    message,
                    "case {case:?}: expected the class's fixed message {message:?}, \
                     got {body}"
                );
                assert!(
                    body.get("ResourceId").unwrap_or(&Value::Null).is_null(),
                    "case {case:?}: expected no ResourceId on a not-found by resource \
                     id, got {body}"
                );
                assert!(
                    body.get("Reason").unwrap_or(&Value::Null).is_null(),
                    "case {case:?}: expected no Reason outside the KMS case, got {body}"
                );
            }
        }
    }
}

/// An identity store the account does not have is named in its own refusal: the recorded
/// fixed text followed by the `IdentityStoreId` the request sent, verbatim.
#[tokio::test]
async fn an_identity_store_the_account_does_not_have_is_named_in_its_refusal() {
    let router = emulator();

    // The FIXED text of this class's recorded message, written out here rather than read
    // from the implementation, so a change in the wording fails this suite. The variable
    // part is the `IdentityStoreId` of the request each row makes.
    const RECORDED_PREFIX: &str = "IdentityStore not present for IdentityStoreId: ";

    // Rows differ along the FORM of an identifier that names a store outside this
    // account — the partition spelling, the owning account, and whether the id is the
    // `d-` or the UUID form the modelled pattern admits — and along which operation is
    // addressed to it.
    let identifiers: Vec<(&str, String)> = vec![
        ("another account's d- store", MISSING_STORE.to_string()),
        (
            "another account's store in UUID form",
            "arn:aws:identitystore::210987654321:identitystore/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "another account's store in another partition",
            format!("arn:aws-cn:identitystore::555555555555:identitystore/{STORE}"),
        ),
    ];
    let requests: [(&str, &str); 4] = [
        ("ListUsers", "{}"),
        ("ListGroups", "{}"),
        ("DescribeUser", UNKNOWN_ID),
        ("CreateGroup", "a group never stored"),
    ];

    for (identifier_case, identifier) in &identifiers {
        for (operation, argument) in requests {
            let case = format!("{identifier_case} / {operation}");
            let mut request = json!({ "IdentityStoreId": identifier });
            match operation {
                "DescribeUser" => request["UserId"] = json!(argument),
                "CreateGroup" => request["DisplayName"] = json!(argument),
                _ => {}
            }
            let (status, code, body) = call(&router, operation, request).await;
            assert_eq!(
                (status, code.ends_with("ResourceNotFoundException")),
                (NOT_FOUND, true),
                "case {case:?}: expected {NOT_FOUND} ResourceNotFoundException, \
                 got {status} {code} with {body}"
            );
            assert_eq!(
                body["ResourceType"].as_str().unwrap_or_default(),
                "IDENTITY_STORE",
                "case {case:?}: expected the identity store to be the missing resource, \
                 got {body}"
            );
            assert_eq!(
                crate::harness::error_message(&body),
                format!("{RECORDED_PREFIX}{identifier}"),
                "case {case:?}: expected the store class's recorded message to name the \
                 requested IdentityStoreId, got {body}"
            );
            assert_eq!(
                crate::harness::error_message(&body),
                identitystore::state::identity_store_not_found_message(identifier),
                "case {case:?}: expected the handler and the crate's one renderer of that \
                 message to agree, got {body}"
            );
            assert!(
                body.get("ResourceId").unwrap_or(&Value::Null).is_null(),
                "case {case:?}: expected no ResourceId: an IdentityStoreId is not a \
                 modelled ResourceId, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_create_is_refused_with_its_claims_fixed_message() {
    let router = emulator();

    // Rows differ along the form and size of the value whose uniqueness collides: the
    // message is the same constant for every one of them.
    let display_names: Vec<(&str, String)> = vec![
        ("an ascii name", "engineering".to_string()),
        ("a unicode name", "équipe-日本-🙂".to_string()),
        ("a single-character name", "g".to_string()),
        (
            "a name at the modelled maximum",
            "n".repeat(DISPLAY_NAME_MAX_LEN),
        ),
    ];

    for (case, display_name) in &display_names {
        let group = create_group(&router, STORE, display_name, case).await;
        let message = refusal_message(
            &router,
            "CreateGroup",
            json!({ "IdentityStoreId": STORE, "DisplayName": display_name }),
            409,
            "ConflictException",
            case,
        )
        .await;
        // The message EQUALS the claim's constant, which is itself the proof that
        // nothing of the request — the `@sensitive` name included — reached it; the
        // assertion below adds that the winning record's id did not either.
        assert_eq!(
            message, "Duplicate GroupDisplayName",
            "case {case:?}: expected the claim's fixed message, got {message:?}"
        );
        assert!(
            !message.contains(&group),
            "case {case:?}: expected no id of the group that already holds the name in \
             the message, got {message:?}"
        );
    }

    // `UserName` is bounded at 128 rather than 1024, so its rows carry their own
    // maximum; every row is whitespace-free, which its pattern demands.
    let user_names: Vec<(&str, String)> = vec![
        ("an ascii name", "alice".to_string()),
        ("a unicode name", "élodie-日本-🙂".to_string()),
        ("a single-character name", "u".to_string()),
        (
            "a name at the modelled maximum",
            "n".repeat(USER_NAME_MAX_LEN),
        ),
    ];

    for (case, user_name) in &user_names {
        let user = create_user(&router, STORE, user_name, case).await;
        let message = refusal_message(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": user_name }),
            409,
            "ConflictException",
            case,
        )
        .await;
        assert_eq!(
            message, "Duplicate UserName",
            "case {case:?}: expected the claim's fixed message, got {message:?}"
        );
        assert!(
            !message.contains(&user),
            "case {case:?}: expected no id of the user that already holds the name in \
             the message, got {message:?}"
        );
    }

    // The claim is the NAME's, not the create's: an update that renames a user onto a
    // name another user holds violates the same claim and states the same constant.
    let case = "an update onto a taken user name";
    let taken = "rename-target";
    let holder = create_user(&router, STORE, taken, case).await;
    let renamed = create_user(&router, STORE, "rename-source", case).await;
    let message = refusal_message(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": renamed,
            "Operations": [{ "AttributePath": "userName", "AttributeValue": taken }],
        }),
        409,
        "ConflictException",
        case,
    )
    .await;
    assert_eq!(
        message, "Duplicate UserName",
        "case {case:?}: expected the claim's fixed message, got {message:?}"
    );
    assert!(
        !message.contains(&holder) && !message.contains(taken),
        "case {case:?}: expected neither the taken name nor its holder's id in the \
         message, got {message:?}"
    );

    // A membership is a claim on the (group, member) PAIR, so the rows differ along the
    // pair: a second member of one group, and a second group of one member.
    let case = "a repeated membership";
    let group = create_group(&router, STORE, "membership-claim", case).await;
    let other_group = create_group(&router, STORE, "membership-claim-other", case).await;
    let user = create_user(&router, STORE, "membership-claim", case).await;
    let other_user = create_user(&router, STORE, "membership-claim-other", case).await;

    for (case, group_id, user_id) in [
        ("one pair", group.clone(), user.clone()),
        ("another member of the same group", group.clone(), other_user),
        ("another group for the same member", other_group, user),
    ] {
        let request = json!({
            "IdentityStoreId": STORE,
            "GroupId": group_id,
            "MemberId": { "UserId": user_id },
        });
        let created = ok(&router, "CreateGroupMembership", request.clone(), case).await;
        let membership = string(&created, "MembershipId", case);
        let message = refusal_message(
            &router,
            "CreateGroupMembership",
            request,
            409,
            "ConflictException",
            case,
        )
        .await;
        assert_eq!(
            message, "Member and Group relationship already exists",
            "case {case:?}: expected the claim's fixed message, got {message:?}"
        );
        assert!(
            !message.contains(&membership) && !message.contains(&group_id),
            "case {case:?}: expected no id of the existing membership in the message, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_rejected_value_cannot_word_the_clause_it_is_rejected_by() {
    let router = emulator();
    let case = "a sensitive value spelling the envelope's own separator";
    // Both halves of a clause are the service's own words: the member path and the
    // requirement of the constraint CLASS that failed. A value that spells the
    // separator between them would, if the renderer read back its own rendered
    // sentence, have its tail handed to the caller as the requirement — and on a
    // `@sensitive` member that is the very value the clause withholds. This value
    // carries the separator and is over its length bound, so it fails BOTH the length
    // and the pattern (whitespace is outside `UserName`'s pattern): two clauses, one
    // envelope, nothing of the value in either.
    let crafted = format!(
        "a' at 'x'{CONSTRAINT_SEPARATOR}zzz{}",
        "z".repeat(USER_NAME_MAX_LEN)
    );
    let message = refusal_message(
        &router,
        "CreateUser",
        json!({ "IdentityStoreId": STORE, "UserName": crafted.clone() }),
        400,
        "ValidationException",
        case,
    )
    .await;

    let expected = format!(
        "2 validation errors detected: {}; {}",
        redacted_clause(
            "userName",
            &format!("Member must have length less than or equal to {USER_NAME_MAX_LEN}")
        ),
        redacted_clause("userName", &pattern_requirement(USER_NAME_PATTERN)),
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains("zzz") && !message.contains("at 'x'"),
        "case {case:?}: expected no fragment of the rejected value in the message, \
         got {message:?}"
    );
}

#[test]
fn the_renderer_reads_every_operations_input_schema() {
    // The element constraint set rule 3 composes is read off the operation input
    // schemas, which the renderer enumerates by hand because a refusal does not say
    // which operation it came from. An operation missing from that enumeration loses the
    // rule silently, so the model's own operation count pins it.
    let model: Value = serde_json::from_str(MODEL).expect("the service's model parses");
    let operations = model["shapes"]
        .as_object()
        .expect("the model carries a shape map")
        .values()
        .filter(|shape| shape["type"] == "operation")
        .count();
    let enumerated = identitystore::service::validation::input_schemas().len();
    assert_eq!(
        enumerated, operations,
        "expected the renderer to read every operation's input schema: it enumerates \
         {enumerated} of the model's {operations} operations"
    );
}

#[test]
fn the_redacted_member_set_is_the_models_own_sensitive_set() {
    // `@sensitive` is a model fact the generated runtime schema does not carry, so the
    // renderer restates it as a list of member names. This derives the same set from the
    // model itself — every member whose target shape carries `smithy.api#sensitive` —
    // so the restatement cannot drift from the model without a failing test.
    let model: Value = serde_json::from_str(MODEL).expect("the service's model parses");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model carries a shape map");

    let sensitive_shapes: BTreeSet<&str> = shapes
        .iter()
        .filter(|(_, shape)| shape["traits"].get("smithy.api#sensitive").is_some())
        .map(|(id, _)| id.as_str())
        .collect();
    assert!(
        !sensitive_shapes.is_empty(),
        "expected the model to mark some shape @sensitive"
    );

    let mut from_model: BTreeSet<String> = BTreeSet::new();
    for shape in shapes.values() {
        let Some(members) = shape["members"].as_object() else {
            continue;
        };
        for (name, member) in members {
            let target = member["target"].as_str().unwrap_or_default();
            // A member is sensitive when its target shape is, or when the member itself
            // carries the trait.
            if sensitive_shapes.contains(target)
                || member["traits"].get("smithy.api#sensitive").is_some()
            {
                let mut chars = name.chars();
                let lower_camel = match chars.next() {
                    Some(first) => first.to_lowercase().chain(chars).collect(),
                    None => String::new(),
                };
                from_model.insert(lower_camel);
            }
        }
    }

    let restated: BTreeSet<String> = identitystore::service::validation::SENSITIVE_MEMBERS
        .iter()
        .map(|member| (*member).to_string())
        .collect();
    assert_eq!(
        restated, from_model,
        "expected the redacted member set to be the model's own @sensitive set: \
         missing {:?}, extra {:?}",
        from_model.difference(&restated).collect::<Vec<_>>(),
        restated.difference(&from_model).collect::<Vec<_>>()
    );
}
