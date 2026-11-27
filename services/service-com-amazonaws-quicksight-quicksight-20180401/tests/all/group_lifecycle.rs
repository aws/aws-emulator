// SPDX-License-Identifier: CC0-1.0

//! Groups: the create and the read that reports it, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a QuickSight group is identified by
//! `(AwsAccountId, Namespace, GroupName)` and belongs to a namespace the account holds;
//! `CreateGroup` persists it, answers HTTP 201 for the group it created and reports that
//! group — its ARN, its name, the description it was given and the `PrincipalId` that
//! names it inside its namespace — and `DescribeGroup` answers from that same record. A
//! name the namespace already holds is refused as an existing resource, and a namespace
//! the account does not hold is not found. `DeleteGroup` removes that record together
//! with what the group held, so every later read of the name reports the miss instead.
//! So the rows below vary what the rule generalises over (the group name's form and
//! length, whether a description is submitted and how long it is, the namespace, and the
//! account) rather than re-running one recorded call.
//!
//! The update cases further down are the same rule read through a write that is not the
//! create: an accepted `UpdateGroup` writes the `Description` it submitted to that
//! stored record, so the next `DescribeGroup` reports it.
//!
//! The membership cases at the end of this suite are about a group's MEMBERS — the
//! users it holds — which are reached through the group they belong to, so they are
//! driven here beside the group they hang on; the listing cases after them read those
//! same members back as the group's membership list, a page at a time. The
//! role-membership cases after them are
//! the other way round: a ROLE's members are groups, so they too are driven beside the
//! group they name.

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::namespace_lifecycle::{create_namespace_in, settle_creation};
use super::user_lifecycle::{register_user, sign_up_for_quicksight_identities};
use super::user_not_found_message::{
    deleted_message as deleted_user_message, delete_namespace, delete_user,
    locator as user_locator,
};

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the harness signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";
/// The namespace every account already has, and the one a caller with no custom
/// namespace addresses.
/// Source: smithy_query(query="operation", operation="CreateGroup") — `Namespace` is a
/// required label.
const DEFAULT_NAMESPACE: &str = "default";
/// A namespace this suite creates, so a row can show the namespace is part of a group's
/// identity rather than a filter applied afterwards.
const CUSTOM_NAMESPACE: &str = "sales-ns";

/// The bytes a group name is percent-encoded over when it goes into the URI path label
/// `DescribeGroup` addresses. `GroupName` admits every character in `\u{20}..\u{FF}`
/// (pattern `^[\u0020-\u00FF]+$`), so a name can hold a space or a non-ASCII letter.
/// Source: smithy_query(query="shape", shape="GroupName").
const GROUP_NAME_LABEL: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'#')
    .add(b'+');

/// The group name as the URI path label carries it.
///
/// `pub(crate)` because every suite that addresses a group by name needs it: the label
/// is percent-encoded over the bytes the pattern admits but a URI does not.
pub(crate) fn group_label(group_name: &str) -> String {
    utf8_percent_encode(group_name, GROUP_NAME_LABEL).to_string()
}

/// The ARN a group created under `(account, namespace, group_name)` in [`REGION`]
/// carries.
fn group_arn(account: &str, namespace: &str, group_name: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:group/{namespace}/{group_name}")
}

/// `CreateGroup`: `POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups`.
/// Source: smithy_query(query="operation", operation="CreateGroup").
async fn create_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/groups"),
            Some(body),
        )
        .await
}

/// `DescribeGroup`:
/// `GET /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}`.
/// Source: smithy_query(query="operation", operation="DescribeGroup").
async fn describe_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!(
                "/accounts/{account}/namespaces/{namespace}/groups/{}",
                group_label(group_name)
            ),
            None,
        )
        .await
}

/// `UpdateGroup`:
/// `PUT /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}`.
/// Source: smithy_query(query="operation", operation="UpdateGroup").
async fn update_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!(
                "/accounts/{account}/namespaces/{namespace}/groups/{}",
                group_label(group_name)
            ),
            Some(body),
        )
        .await
}

/// Create the namespace the rows that do not use the default one put their groups in.
async fn create_custom_namespace(harness: &Harness, account: &str) {
    let (status, body) = create_namespace_in(
        harness,
        REGION,
        account,
        json!({ "Namespace": CUSTOM_NAMESPACE, "IdentityStore": "QUICKSIGHT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the namespace the group rows need was not created: {status} with {body}"
    );
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    /// The `GroupName` the create names; the model admits len 1.. over the pattern
    /// `^[\u0020-\u00FF]+$`.
    /// Source: smithy_query(query="operation", operation="CreateGroup").
    group_name: String,
    /// The `Description` the create submits, if any; the model admits len 1..512.
    description: Option<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "plain-name-with-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            description: Some("Sales team".to_string()),
        },
        Row {
            // The pattern admits `\u0020`, so a name can hold spaces.
            case: "name-with-spaces-and-no-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales Management East".to_string(),
            description: None,
        },
        Row {
            case: "punctuated-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "sales-eu_west.2:readers".to_string(),
            description: Some("Readers in eu-west-2".to_string()),
        },
        Row {
            // The pattern reaches `\u00FF`, so a name can hold a Latin-1 letter.
            case: "non-ascii-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Ventes-Café".to_string(),
            description: Some("Équipe des ventes".to_string()),
        },
        Row {
            // The shortest name and the shortest description the model admits.
            case: "single-character-name-and-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g".to_string(),
            description: Some("d".to_string()),
        },
        Row {
            // The longest description the model admits: `GroupDescription` is len 1..512.
            case: "long-name-and-longest-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g".repeat(128),
            description: Some("d".repeat(512)),
        },
        Row {
            // Same name as the first row, in another namespace of the same account.
            case: "custom-namespace",
            account: ACCOUNT,
            namespace: CUSTOM_NAMESPACE,
            group_name: "Sales".to_string(),
            description: Some("Sales team of the sales namespace".to_string()),
        },
        Row {
            // Same name again, in another account's default namespace.
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            description: None,
        },
    ]
}

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({ "GroupName": self.group_name });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        body
    }

    /// The `Description` a read of this group reports: the one submitted, or none at
    /// all when the create named none.
    fn reported_description(&self) -> Value {
        match &self.description {
            Some(description) => json!(description),
            None => Value::Null,
        }
    }
}

#[tokio::test]
async fn create_group_persists_it_and_describe_reports_it() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    for row in rows() {
        let case = row.case;
        let group_name = row.group_name.as_str();
        let namespace = row.namespace;
        let expected_arn = group_arn(row.account, namespace, group_name);

        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with body {body}"
        );
        let created = &body["Group"];
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert_eq!(
            created["GroupName"],
            json!(group_name),
            "case {case:?}: create reported the wrong GroupName: {body}"
        );
        assert_eq!(
            created["Description"],
            row.reported_description(),
            "case {case:?}: create reported the wrong Description: {body}"
        );
        let principal_id = created["PrincipalId"].as_str().unwrap_or_default().to_string();
        assert!(
            !principal_id.is_empty(),
            "case {case:?}: create reported no PrincipalId: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );

        // The create wrote the group to the shared store, keyed by the account, the
        // namespace and the name, so the reads have a record to answer from.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .group(row.account, namespace, group_name)
                .unwrap_or_else(|| panic!("case {case:?}: the created group is not stored"));
            assert_eq!(
                record.arn, expected_arn,
                "case {case:?}: the stored group carries the wrong Arn"
            );
            assert_eq!(
                record.description, row.description,
                "case {case:?}: the submitted Description is not stored as given"
            );
            assert_eq!(
                record.principal_id, principal_id,
                "case {case:?}: the stored PrincipalId is not the one the create reported"
            );
        }

        // Two reads of the same group report the same thing: a read answers from the
        // stored record and changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{attempt}: describe expected 200, got {status} with {body}"
            );
            let reported = &body["Group"];
            assert_eq!(
                reported["Arn"],
                json!(expected_arn),
                "case {case:?}/{attempt}: describe reported the wrong Arn: {body}"
            );
            assert_eq!(
                reported["GroupName"],
                json!(group_name),
                "case {case:?}/{attempt}: describe reported the wrong GroupName: {body}"
            );
            assert_eq!(
                reported["Description"],
                row.reported_description(),
                "case {case:?}/{attempt}: describe reported the wrong Description: {body}"
            );
            assert_eq!(
                reported["PrincipalId"],
                json!(principal_id),
                "case {case:?}/{attempt}: the PrincipalId must not change between \
                 reads: {body}"
            );
        }

        // A second create of the same name in the same namespace is refused: the name
        // belongs to the group holding it.
        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same group expected 409, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceExistsException"),
            "case {case:?}: a name already in use names an existing resource: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Group {group_name} already exists. AWS account id: {}, groupName: \
                 {group_name}. ",
                row.account
            )),
            "case {case:?}: the refusal must name the group and its account: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("GROUP"),
            "case {case:?}: the refusal is about a group: {body}"
        );

        // And the group the first create stored is untouched by the refusal.
        let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused create must not disturb the group: {status} with {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(principal_id),
            "case {case:?}: the stored group is still the one the first create minted: {body}"
        );
    }
}

#[tokio::test]
async fn group_identity_is_the_account_the_namespace_and_the_name() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    let shared = "Shared Group";

    // The same name in three places: two namespaces of one account, and another
    // account's default namespace.
    let places = [
        ("same-account-default-namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("same-account-custom-namespace", ACCOUNT, CUSTOM_NAMESPACE),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];

    let mut principal_ids = Vec::new();
    for (case, account, namespace) in places {
        let (status, body) = create_group(
            &harness,
            account,
            namespace,
            json!({ "GroupName": shared, "Description": case }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: one group of that name per namespace, so this create \
             expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Arn"],
            json!(group_arn(account, namespace, shared)),
            "case {case:?}: the ARN must carry the owning account and namespace: {body}"
        );
        principal_ids.push(
            body["Group"]["PrincipalId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (index, (case, account, namespace)) in places.iter().enumerate() {
        let (status, body) = describe_group(&harness, account, namespace, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Description"],
            json!(case),
            "case {case:?}: describe must report that namespace's own group: {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(principal_ids[index]),
            "case {case:?}: each group has its own PrincipalId: {body}"
        );
    }

    // Each of the three is a group of its own, and a principal id names the group
    // inside its NAMESPACE: the two in different namespaces are named differently, and
    // the two addressing the same namespace of different accounts are named the same —
    // what carries the account is the ARN, asserted above.
    assert_ne!(
        principal_ids[0], principal_ids[1],
        "case {:?}/{:?}: two namespaces name their groups differently",
        places[0].0, places[1].0
    );
    assert_ne!(
        principal_ids[1], principal_ids[2],
        "case {:?}/{:?}: two namespaces name their groups differently",
        places[1].0, places[2].0
    );
    assert_eq!(
        principal_ids[0], principal_ids[2],
        "case {:?}/{:?}: one name in one namespace is one principal id, whichever \
         account holds it",
        places[0].0, places[2].0
    );

    // A name no group in that namespace is held under is not found.
    for (case, account, namespace, group_name) in [
        (
            "unused-name",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such Group".to_string(),
        ),
        (
            "name-used-in-another-namespace-only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Sales".to_string(),
        ),
        (
            "account-that-created-nothing",
            "123456789012",
            DEFAULT_NAMESPACE,
            shared.to_string(),
        ),
    ] {
        let (status, body) = describe_group(&harness, account, namespace, &group_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a group that was never created cannot be described, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a name no group is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(group_miss_message(account, namespace, &group_name)),
            "case {case:?}: the refusal must name what was not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("GROUP"),
            "case {case:?}: the refusal is about a group: {body}"
        );
    }
}

#[tokio::test]
async fn a_group_needs_a_namespace_the_account_holds() {
    let harness = Harness::new();

    for (case, account, namespace) in [
        ("never-created", ACCOUNT, "no-such-namespace"),
        // A namespace created under one account is no namespace of another.
        ("created-under-another-account", OTHER_ACCOUNT, CUSTOM_NAMESPACE),
    ] {
        if case == "created-under-another-account" {
            create_custom_namespace(&harness, ACCOUNT).await;
        }
        let (status, body) =
            create_group(&harness, account, namespace, json!({ "GroupName": "Sales" })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a group needs a namespace to live in, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a namespace the account does not hold is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Namespace {namespace} not found")),
            "case {case:?}: the refusal must name the namespace: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );

        // Nothing was stored for the refused create.
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        assert!(
            store.group(account, namespace, "Sales").is_none(),
            "case {case:?}: a refused create must persist nothing"
        );
    }
}

// --- A group's members ------------------------------------------------------------
//
// The rule under test: a group membership joins a group to a user of the SAME namespace
// and is identified by `(AwsAccountId, Namespace, GroupName, MemberName)`;
// `CreateGroupMembership` persists it and reports the member — the member's name and the
// ARN of the user it names — `DescribeGroupMembership` answers from that same record, and
// neither a group nor a user the namespace does not hold can be joined. The rows vary
// what the rule generalises over (the member name's form and length, the group, the
// namespace and the account) rather than re-running one recorded call.

/// The address of one membership, as the model binds both the create and the read:
/// `.../namespaces/{Namespace}/groups/{GroupName}/members/{MemberName}`. `GroupName` and
/// `GroupMemberName` carry the same pattern, so both labels are percent-encoded over the
/// same bytes ([`group_label`]).
/// Source: smithy_query(query="operation",
/// operation="CreateGroupMembership,DescribeGroupMembership").
fn membership_path(account: &str, namespace: &str, group_name: &str, member_name: &str) -> String {
    format!(
        "/accounts/{account}/namespaces/{namespace}/groups/{}/members/{}",
        group_label(group_name),
        group_label(member_name)
    )
}

/// `CreateGroupMembership`: `PUT` on the membership's own address.
async fn add_group_member(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
    member_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &membership_path(account, namespace, group_name, member_name),
            None,
        )
        .await
}

/// `DescribeGroupMembership`: `GET` on that same address.
async fn describe_group_member(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
    member_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &membership_path(account, namespace, group_name, member_name),
            None,
        )
        .await
}

/// The ARN a member reports: the one the user it names carries, since a group's members
/// are users.
/// Source: smithy_query(query="shape", shape="GroupMember") — `Arn` is "the Amazon
/// Resource Name (ARN) for the group member (user)".
fn member_arn(account: &str, namespace: &str, member_name: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:user/{namespace}/{member_name}")
}

/// What a membership read reports for a member the group does not hold: the identity
/// path's locator over the four path labels the call carried, ending in a period and a
/// space.
fn membership_locator(
    account: &str,
    namespace: &str,
    group_name: &str,
    member_name: &str,
) -> String {
    format!(
        "AWS account id: {account}, Namespace: {namespace}, groupName: {group_name}, \
         memberName: {member_name}. "
    )
}

/// What a group operation reports for a `GroupName` the namespace holds no group under:
/// the sentence stating that no group of that name exists at the address the call named,
/// closed by the identity path's labelled fields (ending in a period and a space).
fn group_miss_message(account: &str, namespace: &str, group_name: &str) -> String {
    format!(
        "Group {group_name} does not exist in the namespace {namespace} of account \
         {account}. AWS account id: {account}, groupName: {group_name}. "
    )
}

/// Create [`CUSTOM_NAMESPACE`] under `account` and bring its creation transition forward,
/// so it is a place a user can be registered in as well as a place a group can live.
async fn create_settled_custom_namespace(harness: &Harness, account: &str) {
    create_custom_namespace(harness, account).await;
    settle_creation(harness, account, CUSTOM_NAMESPACE);
}

/// Create the group a membership case joins its member to.
async fn given_group(harness: &Harness, case: &str, account: &str, namespace: &str, name: &str) {
    let (status, body) = create_group(harness, account, namespace, json!({ "GroupName": name })).await;
    assert_eq!(
        status, 201,
        "case {case:?}: the fixture group {name:?} expected 201, got {status} with {body}"
    );
}

/// Register the user a membership case names as its member.
async fn given_user(harness: &Harness, case: &str, account: &str, namespace: &str, name: &str) {
    let (status, body) = register_user(
        harness,
        account,
        namespace,
        json!({
            "IdentityType": "QUICKSIGHT",
            "Email": "member@example.com",
            "UserRole": "READER",
            "UserName": name,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: the fixture user {name:?} expected 201, got {status} with {body}"
    );
}

/// How many memberships the store holds.
fn held_memberships(harness: &Harness) -> usize {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store.group_members.len()
}

/// Fill the store up to the total allowance this emulator holds memberships under, with
/// memberships of groups no case addresses, so the next NEW membership meets that
/// ceiling.
///
/// The ceiling bounds what the PROCESS holds, so it is reached by filling the store
/// rather than by driving a hundred thousand calls that would say nothing more; the
/// refusal itself is then asserted over the wire.
fn fill_memberships_to_the_ceiling(harness: &Harness) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let mut filler = 0usize;
    while store.group_members.len() < qs_state::MAX_GROUP_MEMBERS {
        store.group_members.insert(
            (
                ACCOUNT.to_string(),
                DEFAULT_NAMESPACE.to_string(),
                format!("filler-group-{filler}"),
                "filler-member".to_string(),
            ),
            qs_state::GroupMemberRecord {
                member_name: "filler-member".to_string(),
                arn: member_arn(ACCOUNT, DEFAULT_NAMESPACE, "filler-member"),
            },
        );
        filler += 1;
    }
}

/// One row of the add-then-read table.
struct MemberRow {
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    /// The group the member is added to; `GroupName` is len 1.. over the pattern
    /// `^[\u0020-\u00FF]+$`.
    group_name: String,
    /// The user added to it; `GroupMemberName` carries the same pattern, bounded at 256.
    /// Source: smithy_query(query="shape", shape="GroupMemberName").
    member_name: String,
}

fn member_rows() -> Vec<MemberRow> {
    vec![
        MemberRow {
            case: "plain-names",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            member_name: "analyst-1".to_string(),
        },
        MemberRow {
            // The pattern admits `\u0020`, so both names can hold spaces.
            case: "names-with-spaces",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales Management East".to_string(),
            member_name: "Jane Doe".to_string(),
        },
        MemberRow {
            // The pattern reaches `\u00FF`, so both can hold a Latin-1 letter.
            case: "non-ascii-names",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Ventes-Café".to_string(),
            member_name: "Émile Zola".to_string(),
        },
        MemberRow {
            // The shortest names the model admits.
            case: "single-character-names",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g".to_string(),
            member_name: "u".to_string(),
        },
        MemberRow {
            // The longest member name the model admits: `GroupMemberName` is len 1..256,
            // and a `UserName` of that length satisfies its own len 1.. bound.
            case: "longest-member-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Bounded".to_string(),
            member_name: "m".repeat(256),
        },
        MemberRow {
            // The same two names again, in another namespace of the same account.
            case: "custom-namespace",
            account: ACCOUNT,
            namespace: CUSTOM_NAMESPACE,
            group_name: "Sales".to_string(),
            member_name: "analyst-1".to_string(),
        },
        MemberRow {
            // And once more in another account's default namespace.
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            member_name: "analyst-1".to_string(),
        },
    ]
}

#[tokio::test]
async fn adding_a_member_persists_the_membership_and_describe_reports_it() {
    let harness = Harness::new();
    // A user QuickSight manages itself needs an account with an identity store of its
    // own, so both accounts are signed up for one.
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    for row in member_rows() {
        let case = row.case;
        let account = row.account;
        let namespace = row.namespace;
        let group_name = row.group_name.as_str();
        let member_name = row.member_name.as_str();
        let expected_arn = member_arn(account, namespace, member_name);

        given_group(&harness, case, account, namespace, group_name).await;
        given_user(&harness, case, account, namespace, member_name).await;

        let (status, body) =
            add_group_member(&harness, account, namespace, group_name, member_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: adding the member expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["GroupMember"]["MemberName"],
            json!(member_name),
            "case {case:?}: the add reported the wrong MemberName: {body}"
        );
        assert_eq!(
            body["GroupMember"]["Arn"],
            json!(expected_arn),
            "case {case:?}: a member's Arn is the ARN of the user it names: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the add reported no RequestId: {body}"
        );

        // The add wrote the membership to the shared store, keyed by the account, the
        // namespace, the group and the member, so the read has a record to answer from.
        let after_first_add = {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .group_member(account, namespace, group_name, member_name)
                .unwrap_or_else(|| panic!("case {case:?}: the membership is not stored"));
            assert_eq!(
                record.arn, expected_arn,
                "case {case:?}: the stored membership carries the wrong Arn"
            );
            assert_eq!(
                record.member_name, member_name,
                "case {case:?}: the stored membership carries the wrong MemberName"
            );
            store.group_members.len()
        };

        // Two reads of the same membership report the same thing: a read answers from
        // the stored record and changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) =
                describe_group_member(&harness, account, namespace, group_name, member_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{attempt}: describe expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["GroupMember"]["MemberName"],
                json!(member_name),
                "case {case:?}/{attempt}: describe reported the wrong MemberName: {body}"
            );
            assert_eq!(
                body["GroupMember"]["Arn"],
                json!(expected_arn),
                "case {case:?}/{attempt}: describe reported the wrong Arn: {body}"
            );
        }

        // Adding a member the group already holds reports that same membership: one user
        // is a member of one group once.
        let (status, body) =
            add_group_member(&harness, account, namespace, group_name, member_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-adding a member the group holds expected 200, got {status} \
             with {body}"
        );
        assert_eq!(
            body["GroupMember"]["Arn"],
            json!(expected_arn),
            "case {case:?}: the repeated add reported the wrong Arn: {body}"
        );
        assert_eq!(
            held_memberships(&harness),
            after_first_add,
            "case {case:?}: re-adding a member must not store a second membership"
        );
    }
}

#[tokio::test]
async fn a_membership_needs_the_group_and_the_user_it_joins() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    // A group that holds no member yet, a user that is in no group yet, and a user this
    // service deleted — so the rows are about what the call names rather than about an
    // empty store.
    given_group(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "Members").await;
    given_user(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "member-1").await;
    given_user(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "gone").await;
    let (status, body) = delete_user(&harness, ACCOUNT, DEFAULT_NAMESPACE, "gone").await;
    assert_eq!(
        status, 200,
        "the fixture deletion expected 200, got {status} with {body}"
    );

    // Rows: which half of the join is missing, and in what way — the group name, the
    // namespace and the account the group is looked for in, and the member name's form,
    // including one whose user this service deleted.
    let rows: Vec<(&str, &str, &str, String, String, &str, String)> = vec![
        (
            "a group name nothing created",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such Group".to_string(),
            "member-1".to_string(),
            "GROUP",
            group_miss_message(ACCOUNT, DEFAULT_NAMESPACE, "No Such Group"),
        ),
        (
            "a group held in another namespace only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Members".to_string(),
            "member-1".to_string(),
            "GROUP",
            group_miss_message(ACCOUNT, CUSTOM_NAMESPACE, "Members"),
        ),
        (
            "a group held in another account only",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members".to_string(),
            "member-1".to_string(),
            "GROUP",
            group_miss_message(OTHER_ACCOUNT, DEFAULT_NAMESPACE, "Members"),
        ),
        (
            "a member name nothing registered",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members".to_string(),
            "no-such-user".to_string(),
            "USER",
            user_locator(ACCOUNT, DEFAULT_NAMESPACE, "no-such-user"),
        ),
        (
            "a member name holding a space",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members".to_string(),
            "No Such Member".to_string(),
            "USER",
            user_locator(ACCOUNT, DEFAULT_NAMESPACE, "No Such Member"),
        ),
        (
            "a Latin-1 member name the pattern admits",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members".to_string(),
            "Café Lecteur".to_string(),
            "USER",
            user_locator(ACCOUNT, DEFAULT_NAMESPACE, "Café Lecteur"),
        ),
        (
            "a member whose user this service deleted",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members".to_string(),
            "gone".to_string(),
            "USER",
            deleted_user_message(ACCOUNT, DEFAULT_NAMESPACE, "gone"),
        ),
        (
            "a namespace the account does not hold",
            ACCOUNT,
            "no-such-ns",
            "Members".to_string(),
            "member-1".to_string(),
            "NAMESPACE",
            "Namespace no-such-ns not found".to_string(),
        ),
    ];

    for (case, account, namespace, group_name, member_name, resource_type, message) in rows {
        let (status, body) =
            add_group_member(&harness, account, namespace, &group_name, &member_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a membership joins two resources that exist, so this add \
             expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a resource that is not there is not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(resource_type),
            "case {case:?}: the refusal must name the kind of resource it is about: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(message),
            "case {case:?}: expected Message {message:?}, got {body}"
        );
    }

    // Not one of the refused adds stored anything.
    assert_eq!(
        held_memberships(&harness),
        0,
        "a refused add must persist nothing"
    );
}

#[tokio::test]
async fn a_membership_is_reported_only_where_it_was_created() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    let group = "Shared Group";
    let member = "Shared Member";
    // The same group name and the same member name in three places: two namespaces of
    // one account, and another account's default namespace.
    let places = [
        ("same-account-default-namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("same-account-custom-namespace", ACCOUNT, CUSTOM_NAMESPACE),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];
    for (case, account, namespace) in places {
        given_group(&harness, case, account, namespace, group).await;
        given_user(&harness, case, account, namespace, member).await;
    }
    // A second group of the first place, whose member nothing added.
    given_group(&harness, "other-group", ACCOUNT, DEFAULT_NAMESPACE, "Other Group").await;

    // The member is added in the FIRST place only.
    let (status, body) = add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, group, member).await;
    assert_eq!(
        status, 200,
        "the one add this case makes expected 200, got {status} with {body}"
    );

    // Every other address — another namespace, another account, another group of the
    // same namespace — reports a member that group does not hold, and says so with the
    // locator for the address the call named.
    let misses = [
        ("another-namespace", ACCOUNT, CUSTOM_NAMESPACE, group),
        ("another-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE, group),
        (
            "another-group-of-the-same-namespace",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Other Group",
        ),
    ];
    for (case, account, namespace, group_name) in misses {
        let (status, body) =
            describe_group_member(&harness, account, namespace, group_name, member).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a membership is reported only where it was created, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a member the group does not hold is not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("USER"),
            "case {case:?}: a group's members are users: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(membership_locator(account, namespace, group_name, member)),
            "case {case:?}: the refusal must report the address the call named: {body}"
        );
    }

    // And the one membership that was created is still readable.
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, group, member).await;
    assert_eq!(
        status, 200,
        "the created membership must stay readable, got {status} with {body}"
    );
    assert_eq!(
        body["GroupMember"]["Arn"],
        json!(member_arn(ACCOUNT, DEFAULT_NAMESPACE, member)),
        "the membership must report the user of its own namespace: {body}"
    );
    assert_eq!(
        held_memberships(&harness),
        1,
        "exactly one membership was created"
    );
}

#[tokio::test]
async fn a_membership_goes_with_the_user_it_names_and_the_namespace_it_is_in() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    let group = "Doomed Group";
    // Two members, so each removal is shown to take the memberships it owes and no
    // others; the names' forms differ.
    let members = ["doomed", "Doomed Café"];
    given_group(&harness, "doomed", ACCOUNT, CUSTOM_NAMESPACE, group).await;
    for member in members {
        given_user(&harness, member, ACCOUNT, CUSTOM_NAMESPACE, member).await;
        let (status, body) =
            add_group_member(&harness, ACCOUNT, CUSTOM_NAMESPACE, group, member).await;
        assert_eq!(
            status, 200,
            "case {member:?}: the fixture add expected 200, got {status} with {body}"
        );
    }

    // Deleting the user it names takes the membership with it: a group's members are
    // users, so a user that is gone is a member of nothing.
    let (status, body) = delete_user(&harness, ACCOUNT, CUSTOM_NAMESPACE, members[0]).await;
    assert_eq!(
        status, 200,
        "deleting the member's user expected 200, got {status} with {body}"
    );
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, CUSTOM_NAMESPACE, group, members[0]).await;
    assert_eq!(
        status, 404,
        "the membership of a deleted user must be gone, got {status} with {body}"
    );
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, CUSTOM_NAMESPACE, group, members[1]).await;
    assert_eq!(
        status, 200,
        "the other member's membership must be untouched, got {status} with {body}"
    );

    // And deleting the namespace takes the groups it holds, so their memberships go too.
    let (status, body) = delete_namespace(&harness, ACCOUNT, CUSTOM_NAMESPACE).await;
    assert_eq!(
        status, 200,
        "deleting the namespace expected 200, got {status} with {body}"
    );
    for member in members {
        let (status, body) =
            describe_group_member(&harness, ACCOUNT, CUSTOM_NAMESPACE, group, member).await;
        assert_eq!(
            status, 404,
            "case {member:?}: the memberships of a deleted namespace go with it, got \
             {status} with {body}"
        );
    }
    assert_eq!(
        held_memberships(&harness),
        0,
        "the deleted namespace still holds memberships"
    );
}

#[tokio::test]
async fn a_new_membership_past_the_allowance_is_refused_and_a_held_one_still_answers() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    given_group(&harness, "allowance", ACCOUNT, DEFAULT_NAMESPACE, "Members").await;
    given_user(&harness, "allowance", ACCOUNT, DEFAULT_NAMESPACE, "member-1").await;
    given_user(&harness, "allowance", ACCOUNT, DEFAULT_NAMESPACE, "member-2").await;

    let (status, body) =
        add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "the membership held before the allowance runs out expected 200, got {status} \
         with {body}"
    );
    fill_memberships_to_the_ceiling(&harness);

    // Adding a member the group already holds asks for no further room, so it still
    // answers from the store.
    let (status, body) =
        add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "a membership the group already holds must not be refused by the allowance, got \
         {status} with {body}"
    );

    // A membership that is new is refused, and the refusal states the allowance that ran
    // out.
    let (status, body) =
        add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-2").await;
    assert_eq!(
        status, 503,
        "one past the allowance expected 503, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceUnavailableException"),
        "an allowance this emulator holds memberships under is reported as a resource \
         that is unavailable: {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("GROUP"),
        "the exhausted resource is the group the members hang on: {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!(
            "This QuickSight emulator holds the maximum of {} group memberships.",
            qs_state::MAX_GROUP_MEMBERS
        )),
        "the refusal must state the allowance that ran out: {body}"
    );

    // The refused add stored nothing, and the membership that was held is untouched.
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-2").await;
    assert_eq!(
        status, 404,
        "the refused membership must not exist, got {status} with {body}"
    );
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "the held membership must survive the refusal, got {status} with {body}"
    );
    assert_eq!(
        held_memberships(&harness),
        qs_state::MAX_GROUP_MEMBERS,
        "the store must hold exactly the allowance's worth of memberships"
    );
}

// --- A group's membership list ------------------------------------------------------
//
// The rule under test: a group's membership list reports exactly the memberships that
// group holds — each as the name of the user it names and that user's own ARN — ordered
// by member name and handed out a page of at most `MaxResults` at a time, with a
// `NextToken` that resumes after the page just reported; it is a report on stored state,
// so two reads of one group answer the same thing and neither changes what the store
// holds. A name no group of the namespace is held under, and a namespace the account
// does not hold, are not found rather than reported as a group holding no members.
// So the rows below vary what the rule generalises over (how many members the group
// holds, the form of their names and the order they were added in, the group's own name,
// the namespace, the account, and the page size) rather than re-running one recorded
// call.

/// The address of one group's membership list:
/// `.../namespaces/{Namespace}/groups/{GroupName}/members`.
/// Source: smithy_query(query="operation", operation="ListGroupMemberships").
fn group_members_path(account: &str, namespace: &str, group_name: &str) -> String {
    format!(
        "/accounts/{account}/namespaces/{namespace}/groups/{}/members",
        group_label(group_name)
    )
}

/// `ListGroupMemberships`: `GET` on that list, with `query` carrying the pagination
/// parameters the model binds to the query string (`max-results`, `next-token`).
async fn list_group_members(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
    query: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!(
                "{}{query}",
                group_members_path(account, namespace, group_name)
            ),
            None,
        )
        .await
}

/// The members one `ListGroupMemberships` response reported, as `(MemberName, Arn)` pairs
/// in the order it reported them.
fn reported_group_members(case: &str, body: &Value) -> Vec<(String, String)> {
    body["GroupMemberList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the response holds no GroupMemberList: {body}"))
        .iter()
        .map(|member| {
            (
                member["MemberName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                member["Arn"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The members `(account, namespace, members)` are expected to be reported as: one pair
/// per member, each carrying the ARN of the user it names, in member-name order.
fn expected_group_members(
    account: &str,
    namespace: &str,
    members: &[&str],
) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = members
        .iter()
        .map(|member| {
            (
                (*member).to_string(),
                member_arn(account, namespace, member),
            )
        })
        .collect();
    expected.sort();
    expected
}

#[tokio::test]
async fn a_group_reports_exactly_the_members_it_holds() {
    let harness = Harness::new();
    // A user QuickSight manages itself needs an account with an identity store of its
    // own, so both accounts are signed up for one.
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    // Rows: a group holding no members, one, and several added out of name order; names
    // whose form differs (digits, a space, non-ASCII letters); a custom namespace; and
    // the same group name in another account.
    let rows: [(&str, &str, &str, &str, &[&str]); 5] = [
        ("no-members", ACCOUNT, DEFAULT_NAMESPACE, "Empty Group", &[]),
        (
            "one-member",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Readers",
            &["reader-1"],
        ),
        (
            "several-added-out-of-name-order",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Analysts",
            &["zoe", "adam", "Mikael"],
        ),
        (
            "custom-namespace-and-names-of-other-forms",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Groupe Ventes",
            &["café au lait", "Ärger", "bob jones"],
        ),
        (
            "same-group-name-in-another-account",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "Analysts",
            &["other-analyst"],
        ),
    ];

    for (case, account, namespace, group_name, members) in rows {
        given_group(&harness, case, account, namespace, group_name).await;
        for member in members {
            given_user(&harness, case, account, namespace, member).await;
            let (status, body) =
                add_group_member(&harness, account, namespace, group_name, member).await;
            assert_eq!(
                status, 200,
                "case {case:?}: adding {member:?} expected 200, got {status} with {body}"
            );
        }
    }

    let held_before = held_memberships(&harness);

    for (case, account, namespace, group_name, members) in rows {
        // Two reads of one group report the same list: a read answers from the stored
        // memberships and changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) =
                list_group_members(&harness, account, namespace, group_name, "").await;
            assert_eq!(
                status, 200,
                "case {case:?}/{attempt}: listing the members expected 200, got {status} \
                 with {body}"
            );
            assert_eq!(
                reported_group_members(case, &body),
                expected_group_members(account, namespace, members),
                "case {case:?}/{attempt}: the group must report exactly the members added \
                 to it, each with the ARN of the user it names, in member-name order: \
                 {body}"
            );
            assert_eq!(
                body["NextToken"],
                Value::Null,
                "case {case:?}/{attempt}: a list that fits one page hands out no \
                 NextToken: {body}"
            );
            assert!(
                body["RequestId"]
                    .as_str()
                    .is_some_and(|request_id| !request_id.is_empty()),
                "case {case:?}/{attempt}: the response must report a RequestId: {body}"
            );
        }
    }

    assert_eq!(
        held_memberships(&harness),
        held_before,
        "reading a group's members must not change what the store holds"
    );
}

#[tokio::test]
async fn group_membership_pages_walk_every_member_exactly_once() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;

    let group = "Paged Group";
    // Seven members in one group, so a page size smaller than the list leaves a
    // remainder.
    let members: Vec<String> = (0..7).map(|n| format!("page-member-{n:02}")).collect();
    given_group(&harness, "paging", ACCOUNT, DEFAULT_NAMESPACE, group).await;
    for member in &members {
        given_user(&harness, "paging", ACCOUNT, DEFAULT_NAMESPACE, member).await;
        let (status, body) =
            add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, group, member).await;
        assert_eq!(
            status, 200,
            "the fixture membership of {member:?} expected 200, got {status} with {body}"
        );
    }

    // Rows: the unpaginated request, both published bounds of `MaxResults` (1..100) and
    // sizes in between. Each must walk the seven members exactly once, in name order.
    // Source: smithy_query(query="operation", operation="ListGroupMemberships").
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let case = format!("max-results={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match (max_results, token.as_deref()) {
                (None, None) => String::new(),
                (Some(max), None) => format!("?max-results={max}"),
                (None, Some(next)) => format!("?next-token={next}"),
                (Some(max), Some(next)) => format!("?max-results={max}&next-token={next}"),
            };
            let (status, body) =
                list_group_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, group, &query).await;
            assert_eq!(
                status, 200,
                "case {case}: listing expected 200, got {status} with {body}"
            );
            let page = reported_group_members(&case, &body);
            if let Some(max) = max_results {
                assert!(
                    page.len() <= usize::try_from(max).unwrap_or_default(),
                    "case {case}: a page held {} members, more than MaxResults: {body}",
                    page.len()
                );
            }
            for (member_name, arn) in &page {
                assert_eq!(
                    *arn,
                    member_arn(ACCOUNT, DEFAULT_NAMESPACE, member_name),
                    "case {case}: every page reports the ARN of the user the member names: \
                     {body}"
                );
                seen.push(member_name.clone());
            }
            pages += 1;
            assert!(
                pages <= members.len() + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, members,
            "case {case}: paging must yield every member exactly once, in name order"
        );
    }

    // A token this service did not issue names no position, so it is refused rather than
    // read as "from the beginning": an opaque-looking token, one with this service's
    // prefix but a body that is not hex, and an empty one.
    for token in ["not-a-token", "qs1-zzzz", "qs1-abc", ""] {
        let (status, body) = list_group_members(
            &harness,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            group,
            &format!("?next-token={token}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: an unreadable NextToken expected 400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidNextTokenException"),
            "case {token:?}: expected InvalidNextTokenException, got {body}"
        );
    }

    // `MaxResults` is bounded at 1..100, so a page size one past either bound is refused
    // before the handler runs.
    // Source: smithy_query(query="operation", operation="ListGroupMemberships").
    for max_results in ["0", "101"] {
        let (status, body) = list_group_members(
            &harness,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            group,
            &format!("?max-results={max_results}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {max_results:?}: a page size outside the bounds expected 400, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {max_results:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_membership_list_needs_the_group_and_the_namespace_it_is_held_in() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    // One group that does hold a member, so every row below is about the address the call
    // named rather than about an empty store.
    given_group(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "Members").await;
    given_user(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "member-1").await;
    let (status, body) =
        add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "the fixture membership expected 200, got {status} with {body}"
    );

    let rows = [
        (
            "a name no group of the namespace is held under",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such Group",
            "GROUP",
            group_miss_message(ACCOUNT, DEFAULT_NAMESPACE, "No Such Group"),
        ),
        (
            "a group held in another namespace only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Members",
            "GROUP",
            group_miss_message(ACCOUNT, CUSTOM_NAMESPACE, "Members"),
        ),
        (
            "a group held in another account only",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members",
            "GROUP",
            group_miss_message(OTHER_ACCOUNT, DEFAULT_NAMESPACE, "Members"),
        ),
        (
            "a namespace the account does not hold",
            ACCOUNT,
            "no-such-namespace",
            "Members",
            "NAMESPACE",
            "Namespace no-such-namespace not found".to_string(),
        ),
    ];

    for (case, account, namespace, group_name, resource_type, message) in rows {
        let (status, body) = list_group_members(&harness, account, namespace, group_name, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a membership list is reported only where the group is held, so \
             this expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(resource_type),
            "case {case:?}: the refusal must name the resource that is missing: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(message),
            "case {case:?}: the refusal must report the address the call named: {body}"
        );
    }

    // And the group that does hold a member still reports it.
    let (status, body) =
        list_group_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "").await;
    assert_eq!(
        status, 200,
        "the group that holds a member must stay readable, got {status} with {body}"
    );
    assert_eq!(
        reported_group_members("after-the-misses", &body),
        expected_group_members(ACCOUNT, DEFAULT_NAMESPACE, &["member-1"]),
        "the held membership must survive the refusals: {body}"
    );
}

// --- Taking a member out of a group -----------------------------------------------
//
// The rule under test: a membership is identified by
// `(AwsAccountId, Namespace, GroupName, MemberName)`, and the delete addressed to it
// removes exactly that stored membership — so the membership's own read reports the miss
// again, the group's membership listing reports every OTHER member it holds, and the two
// resources the membership joined (the group and the user) are untouched and can be
// joined again. A membership the group does not hold is nothing to remove, and so are a
// group name the namespace holds nothing under and a namespace the account does not
// hold. The rows vary what the rule generalises over (the member's name form and length,
// how many members the group holds, which of them is removed, the namespace and the
// account) rather than re-running one recorded call.

/// `DeleteGroupMembership`: `DELETE` on the membership's own address.
/// Source: smithy_query(query="operation", operation="DeleteGroupMembership").
async fn remove_group_member(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
    member_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &membership_path(account, namespace, group_name, member_name),
            None,
        )
        .await
}

/// One row of the removal table: the group a case fills, the members it is given, and
/// the one of them the case takes out again.
struct RemovalRow {
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    group_name: &'static str,
    members: Vec<String>,
    removed: String,
}

fn removal_rows() -> Vec<RemovalRow> {
    vec![
        RemovalRow {
            // The only member a group holds.
            case: "the-one-member-the-group-holds",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Readers",
            members: vec!["reader-1".to_string()],
            removed: "reader-1".to_string(),
        },
        RemovalRow {
            // One of several, so the rest of the group has to survive it.
            case: "one-of-several-members",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Analysts",
            members: vec![
                "zoe".to_string(),
                "adam".to_string(),
                "Mikael".to_string(),
            ],
            removed: "adam".to_string(),
        },
        RemovalRow {
            // `GroupMemberName` admits `\u0020`, so a member's name can hold spaces.
            // Source: smithy_query(query="shape", shape="GroupMemberName").
            case: "a-name-holding-spaces",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales Management East",
            members: vec!["Jane Doe".to_string(), "John Roe".to_string()],
            removed: "Jane Doe".to_string(),
        },
        RemovalRow {
            // The pattern reaches `\u00FF`, so a name can hold a Latin-1 letter.
            case: "a-non-ascii-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Ventes-Café",
            members: vec!["Émile Zola".to_string(), "café au lait".to_string()],
            removed: "Émile Zola".to_string(),
        },
        RemovalRow {
            // The shortest name the model admits (len 1..256).
            case: "the-shortest-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g",
            members: vec!["u".to_string()],
            removed: "u".to_string(),
        },
        RemovalRow {
            // And the longest: `GroupMemberName` is len 1..256.
            case: "the-longest-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Bounded",
            members: vec!["m".repeat(256), "m".to_string()],
            removed: "m".repeat(256),
        },
        RemovalRow {
            // The same names again, in another namespace of the same account.
            case: "custom-namespace",
            account: ACCOUNT,
            namespace: CUSTOM_NAMESPACE,
            group_name: "Readers",
            members: vec!["reader-1".to_string(), "reader-2".to_string()],
            removed: "reader-2".to_string(),
        },
        RemovalRow {
            // And once more in another account's default namespace.
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Readers",
            members: vec!["reader-1".to_string()],
            removed: "reader-1".to_string(),
        },
    ]
}

#[tokio::test]
async fn removing_a_member_takes_the_membership_out_of_every_later_read() {
    let harness = Harness::new();
    // A user QuickSight manages itself needs an account with an identity store of its
    // own, so both accounts are signed up for one.
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    for row in removal_rows() {
        let RemovalRow {
            case,
            account,
            namespace,
            group_name,
            members,
            removed,
        } = row;
        let removed = removed.as_str();

        given_group(&harness, case, account, namespace, group_name).await;
        for member in &members {
            given_user(&harness, case, account, namespace, member).await;
            let (status, body) =
                add_group_member(&harness, account, namespace, group_name, member).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture membership for {member:?} expected 200, got \
                 {status} with {body}"
            );
        }
        let held_before = held_memberships(&harness);
        let kept: Vec<&str> = members
            .iter()
            .map(String::as_str)
            .filter(|member| *member != removed)
            .collect();

        let (status, body) =
            remove_group_member(&harness, account, namespace, group_name, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing a member the group holds expected 200, got {status} \
             with {body}"
        );
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {case:?}: Status is the HTTP status, so it is not a body member: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the removal reported no RequestId: {body}"
        );

        // Exactly that membership went from the shared store: the ones the group still
        // holds are still there, and nothing else was removed with it.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            assert!(
                store
                    .group_member(account, namespace, group_name, removed)
                    .is_none(),
                "case {case:?}: the removed membership is still stored"
            );
            for member in &kept {
                assert!(
                    store
                        .group_member(account, namespace, group_name, member)
                        .is_some(),
                    "case {case:?}: the membership of {member:?} must survive the removal"
                );
            }
            assert_eq!(
                store.group_members.len(),
                held_before - 1,
                "case {case:?}: exactly one membership goes with the removal"
            );
        }

        // The membership's own read reports the miss again, as it did before the member
        // was ever added.
        let (status, body) =
            describe_group_member(&harness, account, namespace, group_name, removed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the removed membership expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("USER"),
            "case {case:?}: a group's members are users: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(membership_locator(account, namespace, group_name, removed)),
            "case {case:?}: the miss must report the address the call named: {body}"
        );

        // And the group's membership listing reports exactly the members it still holds.
        let (status, body) = list_group_members(&harness, account, namespace, group_name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing expected 200, got {status} with {body}"
        );
        assert_eq!(
            reported_group_members(case, &body),
            expected_group_members(account, namespace, &kept),
            "case {case:?}: the listing must report exactly the members left: {body}"
        );

        // A membership that is already gone is nothing to remove, so the repeat is a
        // miss rather than a second deletion.
        let (status, body) =
            remove_group_member(&harness, account, namespace, group_name, removed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: removing a membership the group does not hold expected 404, \
             got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(membership_locator(account, namespace, group_name, removed)),
            "case {case:?}: the repeated removal must report the address it named: {body}"
        );
        assert_eq!(
            held_memberships(&harness),
            held_before - 1,
            "case {case:?}: a refused removal must not remove a second membership"
        );

        // Only the membership went: the group that held it still reads, and the user it
        // named is still a user of the namespace — which is why the two can be joined
        // again, and the rejoined membership reads like any other.
        let (status, body) = describe_group(&harness, account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group must survive the removal of a member, got {status} \
             with {body}"
        );
        let (status, body) =
            add_group_member(&harness, account, namespace, group_name, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-adding the removed member expected 200, got {status} with \
             {body}"
        );
        assert_eq!(
            body["GroupMember"]["Arn"],
            json!(member_arn(account, namespace, removed)),
            "case {case:?}: the re-added member carries the ARN of the user it names: {body}"
        );
        let (status, body) =
            describe_group_member(&harness, account, namespace, group_name, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rejoined membership expected 200, got {status} with {body}"
        );
        assert_eq!(
            held_memberships(&harness),
            held_before,
            "case {case:?}: the rejoined membership is one membership again"
        );
    }
}

#[tokio::test]
async fn removing_a_member_needs_the_namespace_the_group_and_the_membership() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    // One group that does hold a member, so every row below is about the address the
    // call named rather than about an empty store.
    given_group(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "Members").await;
    given_user(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "member-1").await;
    let (status, body) =
        add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "the fixture membership expected 200, got {status} with {body}"
    );
    // A user of that namespace the group was never given, so a row can show that a
    // membership is removable only where it was created.
    given_user(&harness, "fixture", ACCOUNT, DEFAULT_NAMESPACE, "outsider").await;

    let rows = [
        (
            "a registered user the group does not hold",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members",
            "outsider",
            "USER",
            membership_locator(ACCOUNT, DEFAULT_NAMESPACE, "Members", "outsider"),
        ),
        (
            "a member name no user is known by",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members",
            "nobody at all",
            "USER",
            membership_locator(ACCOUNT, DEFAULT_NAMESPACE, "Members", "nobody at all"),
        ),
        (
            "a name no group of the namespace is held under",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such Group",
            "member-1",
            "GROUP",
            group_miss_message(ACCOUNT, DEFAULT_NAMESPACE, "No Such Group"),
        ),
        (
            "a group held in another namespace only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Members",
            "member-1",
            "GROUP",
            group_miss_message(ACCOUNT, CUSTOM_NAMESPACE, "Members"),
        ),
        (
            "a group held in another account only",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "Members",
            "member-1",
            "GROUP",
            group_miss_message(OTHER_ACCOUNT, DEFAULT_NAMESPACE, "Members"),
        ),
        (
            "a namespace the account does not hold",
            ACCOUNT,
            "no-such-namespace",
            "Members",
            "member-1",
            "NAMESPACE",
            "Namespace no-such-namespace not found".to_string(),
        ),
    ];

    for (case, account, namespace, group_name, member_name, resource_type, message) in rows {
        let (status, body) =
            remove_group_member(&harness, account, namespace, group_name, member_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a membership is removed only where it is held, so this \
             expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(resource_type),
            "case {case:?}: the refusal must name the resource that is missing: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(message),
            "case {case:?}: the refusal must report the address the call named: {body}"
        );
    }

    // And the membership the group does hold survived every refusal above.
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "member-1").await;
    assert_eq!(
        status, 200,
        "the held membership must survive the refusals, got {status} with {body}"
    );
    let (status, body) =
        list_group_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Members", "").await;
    assert_eq!(
        status, 200,
        "the group that holds a member must stay readable, got {status} with {body}"
    );
    assert_eq!(
        reported_group_members("after-the-misses", &body),
        expected_group_members(ACCOUNT, DEFAULT_NAMESPACE, &["member-1"]),
        "the held membership must survive the refusals: {body}"
    );
}

// --- A role's members -------------------------------------------------------------
//
// The rule under test: a role membership joins one of the service's roles to a GROUP of
// the same namespace and is identified by `(AwsAccountId, Namespace, Role, MemberName)`;
// `CreateRoleMembership` persists it, `ListRoleMemberships` reports the groups one role
// holds by name — in name order, a page at a time — and `DeleteRoleMembership` takes one
// out again. A role is held by a group the account manages itself, so an account whose
// authentication method gives it no QuickSight identity store of its own is denied its
// roles altogether. The rows vary what the rule generalises over (the role, the number
// and form of the group names, the page size, the namespace and the account) rather than
// re-running one recorded call.

/// A third 12-digit account id, for the row whose account takes its identities from a
/// directory.
const DIRECTORY_ACCOUNT: &str = "123456789012";

/// Every member of the model's `Role` enum: the roles a group can be added to, and the
/// only values the `Role` label admits.
/// Source: smithy_query(query="shape", shape="Role").
const ROLES: [&str; 6] = [
    "ADMIN",
    "ADMIN_PRO",
    "AUTHOR",
    "AUTHOR_PRO",
    "READER",
    "READER_PRO",
];

/// The address of one role's membership list:
/// `.../namespaces/{Namespace}/roles/{Role}/members`.
/// Source: smithy_query(query="operation", operation="ListRoleMemberships").
fn role_members_path(account: &str, namespace: &str, role: &str) -> String {
    format!("/accounts/{account}/namespaces/{namespace}/roles/{role}/members")
}

/// The address of one role membership: that list with the group's name under it.
/// `MemberName` targets `GroupName`, so the label is percent-encoded over the same bytes
/// a group's own name is ([`group_label`]).
/// Source: smithy_query(query="operation",
/// operation="CreateRoleMembership,DeleteRoleMembership").
fn role_member_path(account: &str, namespace: &str, role: &str, member_name: &str) -> String {
    format!(
        "{}/{}",
        role_members_path(account, namespace, role),
        group_label(member_name)
    )
}

/// `ListRoleMemberships`: `GET` on a role's membership list, with `query` carrying the
/// pagination parameters the model binds to the query string (`max-results`,
/// `next-token`).
async fn list_role_members(
    harness: &Harness,
    account: &str,
    namespace: &str,
    role: &str,
    query: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("{}{query}", role_members_path(account, namespace, role)),
            None,
        )
        .await
}

/// `CreateRoleMembership`: `POST` on the membership's own address.
async fn add_role_member(
    harness: &Harness,
    account: &str,
    namespace: &str,
    role: &str,
    member_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &role_member_path(account, namespace, role, member_name),
            None,
        )
        .await
}

/// `DeleteRoleMembership`: `DELETE` on that same address.
async fn remove_role_member(
    harness: &Harness,
    account: &str,
    namespace: &str,
    role: &str,
    member_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &role_member_path(account, namespace, role, member_name),
            None,
        )
        .await
}

/// Sign `account` up for QuickSight with `subscription`, the sign-up body that chooses
/// where the account's identities come from.
/// Source: smithy_query(query="operation", operation="CreateAccountSubscription").
async fn sign_up(harness: &Harness, case: &str, account: &str, subscription: Value) {
    let (status, body) = harness
        .call("POST", &format!("/account/{account}"), Some(subscription))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the sign-up this case needs failed: {status} with {body}"
    );
}

/// How many role memberships the store holds.
fn held_role_memberships(harness: &Harness) -> usize {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store.role_memberships.len()
}

/// The group names one `ListRoleMemberships` response reported, in the order it reported
/// them.
fn reported_members(case: &str, body: &Value) -> Vec<String> {
    body["MembersList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no MembersList in {body}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// What a role-membership call on an account whose identities QuickSight does not manage
/// reports: one fixed sentence, which is the whole message — it carries nothing from the
/// request and nothing from the account's stored configuration.
const ROLE_DENIAL_MESSAGE: &str = "Operation is disabled for identities managed by QuickSight. ";

/// What a role-membership delete reports for a group the role does not hold: the locator
/// alone, because nothing is known to have existed at that address.
fn role_membership_locator(
    account: &str,
    namespace: &str,
    role: &str,
    member_name: &str,
) -> String {
    format!(
        "AWS account id: {account}, Namespace: {namespace}, Role: {role}, \
         memberName: {member_name}. "
    )
}

#[tokio::test]
async fn a_role_reports_exactly_the_groups_added_to_it() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    sign_up_for_quicksight_identities(&harness, OTHER_ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    // Rows: one per member of the model's `Role` catalog, differing in how many groups
    // the role holds (none, one, several), the form and length of those group names, the
    // namespace the role belongs to and the account it is in.
    let rows: [(&str, &str, &str, &str, &[&str]); 6] = [
        ("one-group", ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", &["Admins"]),
        (
            "three-groups",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "AUTHOR",
            &["Analysts", "Report Authors", "Zürich Authoren"],
        ),
        ("no-group", ACCOUNT, DEFAULT_NAMESPACE, "READER", &[]),
        (
            "custom-namespace",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "ADMIN_PRO",
            &["Admins"],
        ),
        (
            "other-account",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "AUTHOR_PRO",
            &["Admins"],
        ),
        (
            "digits-in-the-names",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "READER_PRO",
            &["team-42", "team-7"],
        ),
    ];

    // Every group a row names has to exist where the row puts it: a role takes an
    // EXISTING group of its own namespace.
    let mut created: Vec<(&str, &str, &str)> = Vec::new();
    for (case, account, namespace, _, members) in rows {
        for member in members {
            if created.contains(&(account, namespace, member)) {
                continue;
            }
            given_group(&harness, case, account, namespace, member).await;
            created.push((account, namespace, member));
        }
    }

    let mut added = 0usize;
    for (case, account, namespace, role, members) in rows {
        for member in members {
            let (status, body) = add_role_member(&harness, account, namespace, role, member).await;
            assert_eq!(
                status, 200,
                "case {case:?}: adding {member:?} to {role} expected 200, got {status} \
                 with {body}"
            );
            assert!(
                body["RequestId"].is_string(),
                "case {case:?}: the add must report a RequestId: {body}"
            );
            added += 1;
        }
    }

    for (case, account, namespace, role, members) in rows {
        let (status, body) = list_role_members(&harness, account, namespace, role, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing {role} expected 200, got {status} with {body}"
        );
        let mut expected: Vec<String> = members.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            reported_members(case, &body),
            expected,
            "case {case:?}: {role} must report exactly the groups added to it, in name \
             order: {body}"
        );
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {case:?}: a complete page needs no NextToken: {body}"
        );
        assert!(
            body["RequestId"].is_string(),
            "case {case:?}: the listing must report a RequestId: {body}"
        );
    }

    // A membership is held under ONE role of ONE namespace: every other role of the
    // custom namespace reports nothing, though a group of the same name holds a role in
    // the account's default namespace.
    for role in ROLES {
        let (status, body) = list_role_members(&harness, ACCOUNT, CUSTOM_NAMESPACE, role, "").await;
        assert_eq!(
            status, 200,
            "case {role:?}: listing the custom namespace's role expected 200, got \
             {status} with {body}"
        );
        let expected: Vec<String> = if role == "ADMIN_PRO" {
            vec!["Admins".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_members(role, &body),
            expected,
            "case {role:?}: a membership must be reported by its own role only: {body}"
        );
    }

    assert_eq!(
        held_role_memberships(&harness),
        added,
        "the store must hold exactly the memberships the adds created"
    );
}

#[tokio::test]
async fn role_membership_pages_walk_every_member_exactly_once() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;

    // Seven groups in one role, so a page size smaller than the list leaves a remainder.
    let members: Vec<String> = (0..7).map(|n| format!("page-group-{n:02}")).collect();
    for member in &members {
        given_group(&harness, "paging", ACCOUNT, DEFAULT_NAMESPACE, member).await;
        let (status, body) =
            add_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "READER", member).await;
        assert_eq!(
            status, 200,
            "fixture {member:?}: the add expected 200, got {status} with {body}"
        );
    }

    // Rows: the unpaginated request, both published bounds of `MaxResults` (1..100) and
    // sizes in between. Each must walk the seven members exactly once.
    // Source: smithy_query(query="operation", operation="ListRoleMemberships").
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let case = format!("max-results={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match (&token, max_results) {
                (Some(token), Some(max)) => format!("?max-results={max}&next-token={token}"),
                (Some(token), None) => format!("?next-token={token}"),
                (None, Some(max)) => format!("?max-results={max}"),
                (None, None) => String::new(),
            };
            let (status, body) =
                list_role_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "READER", &query).await;
            assert_eq!(
                status, 200,
                "case {case}: listing expected 200, got {status} with {body}"
            );
            let page = reported_members(&case, &body);
            if let Some(max) = max_results {
                assert!(
                    page.len() <= usize::try_from(max).unwrap_or_default(),
                    "case {case}: a page held {} members, more than MaxResults: {body}",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= members.len() + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, members,
            "case {case}: paging must yield every member exactly once, in name order"
        );
    }

    // A token this service did not issue names no position, so it is refused rather than
    // read as "from the beginning": an opaque-looking token, one with this service's
    // prefix but a body that is not hex, and an empty one.
    for token in ["not-a-token", "qs1-zzzz", "qs1-abc", ""] {
        let (status, body) = list_role_members(
            &harness,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "READER",
            &format!("?next-token={token}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: an unreadable NextToken expected 400, got {status} with \
             {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidNextTokenException"),
            "case {token:?}: expected InvalidNextTokenException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_role_is_reached_only_in_an_account_that_holds_a_quicksight_identity_store() {
    let harness = Harness::new();
    // Rows: the ways an account ends up with no QuickSight identity store of its own —
    // it never signed up, so it authenticates Amazon Web Services identities alone (the
    // service default); it signed up choosing exactly that; or it signed up against a
    // directory, which is where its identities then come from. However the account got
    // there, the refusal reads the same sentence.
    // Source: smithy_query(query="shape", shape="AuthenticationMethodOption").
    let rows = [
        ("never-signed-up", ACCOUNT, Value::Null),
        (
            "signed-up-without-one",
            OTHER_ACCOUNT,
            json!({
                "AccountName": format!("roles-{OTHER_ACCOUNT}"),
                "AuthenticationMethod": "IAM_ONLY",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            }),
        ),
        (
            "signed-up-against-a-directory",
            DIRECTORY_ACCOUNT,
            json!({
                "AccountName": format!("roles-{DIRECTORY_ACCOUNT}"),
                "AuthenticationMethod": "ACTIVE_DIRECTORY",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
                "ActiveDirectoryName": "corp.example.com",
                "Realm": "CORP.EXAMPLE.COM",
                "AdminGroup": ["Domain Admins"],
            }),
        ),
    ];
    for (case, account, subscription) in rows {
        if !subscription.is_null() {
            sign_up(&harness, case, account, subscription).await;
        }
        // The group itself CAN be created there, so what the refusals below are about is
        // the role rather than a group the namespace is missing.
        given_group(&harness, case, account, DEFAULT_NAMESPACE, "Admins").await;
        for role in ROLES {
            let calls = [
                (
                    "list",
                    list_role_members(&harness, account, DEFAULT_NAMESPACE, role, "").await,
                ),
                (
                    "create",
                    add_role_member(&harness, account, DEFAULT_NAMESPACE, role, "Admins").await,
                ),
                (
                    "delete",
                    remove_role_member(&harness, account, DEFAULT_NAMESPACE, role, "Admins").await,
                ),
            ];
            for (call, (status, body)) in calls {
                assert_eq!(
                    status, 401,
                    "case {case:?} {call} {role}: an account with no QuickSight identity \
                     store must be denied its roles, got {status} with {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("AccessDeniedException"),
                    "case {case:?} {call} {role}: expected AccessDeniedException, got \
                     {body}"
                );
                assert_eq!(
                    body["Message"],
                    json!(ROLE_DENIAL_MESSAGE),
                    "case {case:?} {call} {role}: expected the fixed denial \
                     {ROLE_DENIAL_MESSAGE:?}, got {body}"
                );
                assert!(
                    body["RequestId"]
                        .as_str()
                        .is_some_and(|request_id| !request_id.is_empty()),
                    "case {case:?} {call} {role}: a refusal of a route scoped to a \
                     namespace states the id of the call in its document as well as in the \
                     x-amzn-RequestId header: {body}"
                );
            }
        }
    }
    assert_eq!(
        held_role_memberships(&harness),
        0,
        "a denied call must persist nothing"
    );
}

#[tokio::test]
async fn the_role_membership_denial_reads_the_same_whatever_the_call_named() {
    let harness = Harness::new();
    // One account signed up against a directory, so a row reads the refusal on an account
    // whose stored identity store is not the service default: the sentence does not vary
    // with what the account has either.
    sign_up(
        &harness,
        "directory",
        DIRECTORY_ACCOUNT,
        json!({
            "AccountName": format!("roles-{DIRECTORY_ACCOUNT}"),
            "AuthenticationMethod": "ACTIVE_DIRECTORY",
            "NotificationEmail": "ops@example.com",
            "Edition": "ENTERPRISE",
            "ActiveDirectoryName": "corp.example.com",
            "Realm": "CORP.EXAMPLE.COM",
            "AdminGroup": ["Domain Admins"],
        }),
    )
    .await;

    // Rows: the address a refused call named — the account (one that never signed up, so
    // it holds the service default, one signed up against a directory, and a third
    // unrelated one), the namespace label (the one every account has, a name no namespace
    // is held under, the punctuation the pattern admits, the shortest name and the
    // longest), the role, and the form and length of the member name. The denial settles
    // on the ACCOUNT before any address inside it is resolved, so neither the namespace
    // nor the group a row names has to exist for the call to be refused.
    // Source: smithy_query(query="operation",
    // operation="ListRoleMemberships,CreateRoleMembership,DeleteRoleMembership") —
    // `Namespace` is len 0..64 over `^[a-zA-Z0-9._-]*$`, `Role` is the `Role` enum and
    // `MemberName` targets `GroupName`, len 1.. over `^[\u0020-\u00FF]+$`.
    let rows: Vec<(&str, &str, String, &str, String)> = vec![
        (
            "the default namespace",
            ACCOUNT,
            DEFAULT_NAMESPACE.to_string(),
            "ADMIN",
            "Admins".to_string(),
        ),
        (
            "a namespace the account holds no record of",
            ACCOUNT,
            "absent-ns".to_string(),
            "READER",
            "Readers".to_string(),
        ),
        (
            "a namespace name holding the punctuation the pattern admits",
            OTHER_ACCOUNT,
            "sales.archive_2026-Q1".to_string(),
            "AUTHOR_PRO",
            "Report Authors".to_string(),
        ),
        (
            "the shortest namespace name the model admits",
            OTHER_ACCOUNT,
            "n".to_string(),
            "READER_PRO",
            "Zürich Authoren".to_string(),
        ),
        (
            "the longest namespace name the model admits",
            DIRECTORY_ACCOUNT,
            "n".repeat(64),
            "ADMIN_PRO",
            "g".repeat(128),
        ),
        (
            "an account whose identities come from a directory",
            DIRECTORY_ACCOUNT,
            DEFAULT_NAMESPACE.to_string(),
            "AUTHOR",
            "Domain Admins".to_string(),
        ),
    ];

    for (case, account, namespace, role, member) in rows {
        let calls = [
            (
                "list",
                list_role_members(&harness, account, &namespace, role, "").await,
            ),
            (
                "create",
                add_role_member(&harness, account, &namespace, role, &member).await,
            ),
            (
                "delete",
                remove_role_member(&harness, account, &namespace, role, &member).await,
            ),
        ];
        for (call, (status, body)) in calls {
            assert_eq!(
                status, 401,
                "case {case:?} {call}: an account whose identities QuickSight does not \
                 manage must be denied its roles, got {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("AccessDeniedException"),
                "case {case:?} {call}: expected AccessDeniedException, got {body}"
            );
            assert_eq!(
                body["Message"],
                json!(ROLE_DENIAL_MESSAGE),
                "case {case:?} {call}: expected the fixed denial {ROLE_DENIAL_MESSAGE:?} \
                 whatever the call named, got {body}"
            );
        }
    }
    assert_eq!(
        held_role_memberships(&harness),
        0,
        "a denied call must persist nothing"
    );
}

#[tokio::test]
async fn removing_a_group_from_a_role_takes_it_out_of_the_listing() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;
    create_settled_custom_namespace(&harness, ACCOUNT).await;

    let doomed = "Doomed Admins";
    let kept = "Kept Admins";
    for member in [doomed, kept] {
        given_group(&harness, "removal", ACCOUNT, DEFAULT_NAMESPACE, member).await;
        let (status, body) =
            add_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", member).await;
        assert_eq!(
            status, 200,
            "fixture {member:?}: the add expected 200, got {status} with {body}"
        );
    }

    let (status, body) = remove_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", doomed)
        .await;
    assert_eq!(
        status, 200,
        "removing a group the role holds expected 200, got {status} with {body}"
    );
    assert!(
        body["RequestId"].is_string(),
        "the removal must report a RequestId: {body}"
    );

    let (status, body) = list_role_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", "").await;
    assert_eq!(
        status, 200,
        "the listing after the removal expected 200, got {status} with {body}"
    );
    assert_eq!(
        reported_members("after-removal", &body),
        vec![kept.to_string()],
        "the removed group must be out of the listing and the other one still in it: \
         {body}"
    );
    assert_eq!(
        held_role_memberships(&harness),
        1,
        "the removal must take exactly the membership it named"
    );

    // Removing it again finds no membership at that address, and says so with the
    // locator for the address the call named.
    let (status, body) = remove_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", doomed)
        .await;
    assert_eq!(
        status, 404,
        "a membership the role does not hold expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "a membership the role does not hold is not found: {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("GROUP"),
        "a role's members are groups: {body}"
    );
    assert_eq!(
        body["Message"],
        json!(role_membership_locator(
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "ADMIN",
            doomed
        )),
        "the refusal must report the address the call named: {body}"
    );

    // A role takes an EXISTING group of its namespace: a name no group there is known by
    // names no member. The group the row names exists in ANOTHER namespace of the same
    // account, so what is missing is the group in this one.
    given_group(&harness, "elsewhere", ACCOUNT, CUSTOM_NAMESPACE, "Sales").await;
    let (status, body) =
        add_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "AUTHOR", "Sales").await;
    assert_eq!(
        status, 404,
        "a role takes an existing group, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("GROUP"),
        "the refusal is about the group the call named: {body}"
    );
    assert_eq!(
        body["Message"],
        json!(group_miss_message(ACCOUNT, DEFAULT_NAMESPACE, "Sales")),
        "the refusal must name the group the namespace does not hold: {body}"
    );

    // A role is a role OF a namespace, so every call on one the account does not hold
    // reports that namespace.
    let missing = "no-such-namespace";
    let calls = [
        (
            "list",
            list_role_members(&harness, ACCOUNT, missing, "ADMIN", "").await,
        ),
        (
            "create",
            add_role_member(&harness, ACCOUNT, missing, "ADMIN", kept).await,
        ),
        (
            "delete",
            remove_role_member(&harness, ACCOUNT, missing, "ADMIN", kept).await,
        ),
    ];
    for (call, (status, body)) in calls {
        assert_eq!(
            status, 404,
            "case {call}: a namespace the account does not hold expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {call}: the refusal is about a namespace: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Namespace {missing} not found")),
            "case {call}: the refusal must name the namespace: {body}"
        );
    }

    // And a membership goes with the group it names: the groups of a namespace go when
    // that namespace is deleted, so the roles they held go with them.
    let (status, body) =
        add_role_member(&harness, ACCOUNT, CUSTOM_NAMESPACE, "AUTHOR", "Sales").await;
    assert_eq!(
        status, 200,
        "the membership of the doomed namespace expected 200, got {status} with {body}"
    );
    let (status, body) = delete_namespace(&harness, ACCOUNT, CUSTOM_NAMESPACE).await;
    assert_eq!(
        status, 200,
        "deleting the namespace expected 200, got {status} with {body}"
    );
    assert_eq!(
        held_role_memberships(&harness),
        1,
        "the memberships of a deleted namespace go with it, and only those"
    );
    let (status, body) = list_role_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", "").await;
    assert_eq!(
        status, 200,
        "the untouched namespace's role must still answer, got {status} with {body}"
    );
    assert_eq!(
        reported_members("survivor", &body),
        vec![kept.to_string()],
        "the membership of another namespace must survive the deletion: {body}"
    );
}

/// `DeleteGroup`:
/// `DELETE /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}`.
/// Source: smithy_query(query="operation", operation="DeleteGroup").
async fn delete_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &format!(
                "/accounts/{account}/namespaces/{namespace}/groups/{}",
                group_label(group_name)
            ),
            None,
        )
        .await
}

/// Whether the store still holds the group at `(account, namespace, group_name)`.
fn holds_group(harness: &Harness, account: &str, namespace: &str, group_name: &str) -> bool {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store.group(account, namespace, group_name).is_some()
}

#[tokio::test]
async fn deleting_a_group_removes_it_and_every_later_read_reports_the_miss() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    // Rows vary what the rule generalises over: the form and length of the name
    // (`GroupName` is len 1.. over the pattern `^[\u0020-\u00FF]+$`, so a space and a
    // non-ASCII letter are both admitted), the namespace the group lives in, and the
    // account that holds it.
    // Source: smithy_query(query="shape", shape="GroupName").
    let long_name = "L".repeat(120);
    let rows: Vec<(&str, &str, &str, String)> = vec![
        ("plain-name", ACCOUNT, DEFAULT_NAMESPACE, "Sales".to_string()),
        (
            "name-with-a-space",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "Data Science".to_string(),
        ),
        (
            "non-ascii-name",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Groupe Générée".to_string(),
        ),
        (
            "same-name-another-account",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            "Sales".to_string(),
        ),
        ("long-name", ACCOUNT, DEFAULT_NAMESPACE, long_name),
    ];

    for (case, account, namespace, group_name) in &rows {
        let group_name = group_name.as_str();
        given_group(&harness, case, account, namespace, group_name).await;

        let (status, body) = delete_group(&harness, account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the group expected 200, got {status} with {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the delete reported no RequestId: {body}"
        );
        assert!(
            !holds_group(&harness, account, namespace, group_name),
            "case {case:?}: the deleted group is still in the store"
        );

        // Reading the deleted group is the miss every group read reports, and reading it
        // twice reports it twice: the refusal changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) = describe_group(&harness, account, namespace, group_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{attempt}: describing a deleted group expected 404, got \
                 {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ResourceNotFoundException"),
                "case {case:?}/{attempt}: a deleted group is not found: {body}"
            );
            assert_eq!(
                body["Message"],
                json!(group_miss_message(account, namespace, group_name)),
                "case {case:?}/{attempt}: the refusal must name what was not found: {body}"
            );
            assert_eq!(
                body["ResourceType"],
                json!("GROUP"),
                "case {case:?}/{attempt}: the refusal is about a group: {body}"
            );
        }

        // A second delete has nothing to delete, so it is the same miss.
        let (status, body) = delete_group(&harness, account, namespace, group_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting a group twice expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(group_miss_message(account, namespace, group_name)),
            "case {case:?}: the repeated delete must name what was not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("GROUP"),
            "case {case:?}: the repeated delete is about a group: {body}"
        );

        // The name is free again: the create that collided before the delete succeeds
        // after it.
        let (status, body) =
            create_group(&harness, account, namespace, json!({ "GroupName": group_name })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: re-creating the deleted group expected 201, got {status} with \
             {body}"
        );
        assert_eq!(
            body["Group"]["Arn"],
            json!(group_arn(account, namespace, group_name)),
            "case {case:?}: the re-created group carries the same ARN: {body}"
        );
    }

    // A name no group is held under is nothing to delete, whichever part of the address
    // is the one that holds no group.
    let rows: [(&str, &str, &str, &str); 3] = [
        ("never-created", ACCOUNT, DEFAULT_NAMESPACE, "No Such Group"),
        (
            "held-in-another-namespace-only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Sales",
        ),
        ("account-that-created-nothing", "123456789012", DEFAULT_NAMESPACE, "Sales"),
    ];
    for (case, account, namespace, group_name) in rows {
        let (status, body) = delete_group(&harness, account, namespace, group_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting a group that does not exist expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(group_miss_message(account, namespace, group_name)),
            "case {case:?}: the refusal must name what was not found: {body}"
        );
    }

    // Deleting one group leaves every other group where it was: the group the first row
    // re-created is still readable.
    let (status, body) = describe_group(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Sales").await;
    assert_eq!(
        status, 200,
        "an untouched group must still answer, got {status} with {body}"
    );
    assert_eq!(
        body["Group"]["GroupName"],
        json!("Sales"),
        "the untouched group must report its own name: {body}"
    );
}

#[tokio::test]
async fn deleting_a_group_takes_its_memberships_and_its_roles_with_it() {
    let harness = Harness::new();
    sign_up_for_quicksight_identities(&harness, ACCOUNT).await;

    let doomed = "Doomed Group";
    let kept = "Kept Group";
    let member = "analyst";
    given_user(&harness, "cascade", ACCOUNT, DEFAULT_NAMESPACE, member).await;
    for group_name in [doomed, kept] {
        given_group(&harness, "cascade", ACCOUNT, DEFAULT_NAMESPACE, group_name).await;
        let (status, body) =
            add_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, group_name, member).await;
        assert_eq!(
            status, 200,
            "fixture {group_name:?}: the membership expected 200, got {status} with {body}"
        );
        let (status, body) =
            add_role_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", group_name).await;
        assert_eq!(
            status, 200,
            "fixture {group_name:?}: the role membership expected 200, got {status} with \
             {body}"
        );
    }

    let (status, body) = delete_group(&harness, ACCOUNT, DEFAULT_NAMESPACE, doomed).await;
    assert_eq!(
        status, 200,
        "deleting the group expected 200, got {status} with {body}"
    );

    // What the group held is gone with it: the membership it carried is no longer
    // readable, and the read reports the group it was reached through as missing.
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, doomed, member).await;
    assert_eq!(
        status, 404,
        "a membership of a deleted group expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(group_miss_message(ACCOUNT, DEFAULT_NAMESPACE, doomed)),
        "the refusal must name the group that is gone: {body}"
    );

    // And the role no longer holds it, while the other group's membership survives.
    let (status, body) = list_role_members(&harness, ACCOUNT, DEFAULT_NAMESPACE, "ADMIN", "").await;
    assert_eq!(
        status, 200,
        "listing the role's members expected 200, got {status} with {body}"
    );
    assert_eq!(
        reported_members("after-delete", &body),
        vec![kept.to_string()],
        "a deleted group holds no role, and only that group's membership goes: {body}"
    );
    assert_eq!(
        held_memberships(&harness),
        1,
        "the memberships of a deleted group go with it, and only those"
    );
    assert_eq!(
        held_role_memberships(&harness),
        1,
        "the role memberships naming a deleted group go with it, and only those"
    );
    let (status, body) =
        describe_group_member(&harness, ACCOUNT, DEFAULT_NAMESPACE, kept, member).await;
    assert_eq!(
        status, 200,
        "the other group's membership must survive the deletion, got {status} with {body}"
    );
    assert_eq!(
        body["GroupMember"]["MemberName"],
        json!(member),
        "the surviving membership must report its member: {body}"
    );
}

/// The descriptions the update rows submit, one per row of [`rows`], so the table varies
/// what the read-your-write rule generalises over: the shortest and the longest
/// `GroupDescription` the model admits (len 1..512), a non-ASCII one, one holding
/// punctuation and separators, and a plain re-description.
/// Source: smithy_query(query="operation", operation="UpdateGroup") — `Description`
/// targets `GroupDescription`, len 1..512.
fn updated_descriptions() -> Vec<(&'static str, String)> {
    vec![
        ("plain", "Sales team, reorganised".to_string()),
        ("shortest", "d".to_string()),
        ("longest", "u".repeat(512)),
        ("non-ascii", "Équipe des ventes — Café".to_string()),
        ("punctuated", "sales/eu-west-2: readers (2026)".to_string()),
    ]
}

#[tokio::test]
async fn update_group_persists_the_new_description_and_describe_reports_it() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    let descriptions = updated_descriptions();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let group_name = row.group_name.as_str();
        let namespace = row.namespace;

        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with body {body}"
        );
        let arn = body["Group"]["Arn"].clone();
        let principal_id = body["Group"]["PrincipalId"].clone();

        // Two updates in a row, each submitting another description: whatever the
        // latest accepted update submitted is what the group carries.
        for step in ["first-update", "second-update"] {
            let (label, description) = &descriptions[if step == "first-update" {
                index % descriptions.len()
            } else {
                (index + 1) % descriptions.len()
            }];
            let (status, body) = update_group(
                &harness,
                row.account,
                namespace,
                group_name,
                json!({ "Description": description }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{step}/{label}: update expected 200, got {status} with {body}"
            );
            let updated = &body["Group"];
            assert_eq!(
                updated["Description"],
                json!(description),
                "case {case:?}/{step}/{label}: the update must report the description it \
                 was given: {body}"
            );
            assert_eq!(
                updated["GroupName"],
                json!(group_name),
                "case {case:?}/{step}/{label}: the update must report the group it \
                 addressed: {body}"
            );
            assert_eq!(
                updated["Arn"], arn,
                "case {case:?}/{step}/{label}: an update must not change the group's \
                 Arn: {body}"
            );
            assert_eq!(
                updated["PrincipalId"], principal_id,
                "case {case:?}/{step}/{label}: an update must not change the group's \
                 PrincipalId: {body}"
            );
            assert!(
                body["RequestId"]
                    .as_str()
                    .is_some_and(|request_id| !request_id.is_empty()),
                "case {case:?}/{step}/{label}: the update reported no RequestId: {body}"
            );

            // The read that follows answers from the record the update wrote, and twice
            // over: a read changes nothing.
            for attempt in ["first-read", "second-read"] {
                let (status, body) =
                    describe_group(&harness, row.account, namespace, group_name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}/{step}/{label}/{attempt}: describe expected 200, got \
                     {status} with {body}"
                );
                assert_eq!(
                    body["Group"]["Description"],
                    json!(description),
                    "case {case:?}/{step}/{label}/{attempt}: describe must report the \
                     Description the update submitted, not the one the create did: {body}"
                );
                assert_eq!(
                    body["Group"]["PrincipalId"], principal_id,
                    "case {case:?}/{step}/{label}/{attempt}: the PrincipalId must not \
                     change: {body}"
                );
            }

            // And the stored record is what those reads answered from.
            {
                let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
                let store = state.lock();
                let record = store
                    .group(row.account, namespace, group_name)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}/{step}/{label}: the group is not stored")
                    });
                assert_eq!(
                    record.description.as_deref(),
                    Some(description.as_str()),
                    "case {case:?}/{step}/{label}: the submitted Description did not \
                     reach the stored record"
                );
            }
        }

        // `Description` is optional, so an update that submits none says nothing about
        // the description: the one the last update wrote stays.
        let (_, kept) = &descriptions[(index + 1) % descriptions.len()];
        let (status, body) =
            update_group(&harness, row.account, namespace, group_name, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update submitting no Description expected 200, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Description"],
            json!(kept),
            "case {case:?}: an update that names no Description must leave the one the \
             group carries: {body}"
        );
        let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Description"],
            json!(kept),
            "case {case:?}: describe must still report the description the last update \
             wrote: {body}"
        );
    }
}

#[tokio::test]
async fn update_group_touches_only_the_group_it_addresses() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    let shared = "Shared Group";

    // The same name in three places, as the identity case above: two namespaces of one
    // account, and another account's default namespace.
    let places = [
        ("same-account-default-namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("same-account-custom-namespace", ACCOUNT, CUSTOM_NAMESPACE),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];
    for (case, account, namespace) in places {
        let (status, body) = create_group(
            &harness,
            account,
            namespace,
            json!({ "GroupName": shared, "Description": case }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with {body}"
        );
    }

    let (case, account, namespace) = places[1];
    let (status, body) = update_group(
        &harness,
        account,
        namespace,
        shared,
        json!({ "Description": "only this one" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the update expected 200, got {status} with {body}"
    );

    for (index, (case, account, namespace)) in places.iter().enumerate() {
        let expected = if index == 1 { "only this one" } else { *case };
        let (status, body) = describe_group(&harness, account, namespace, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Description"],
            json!(expected),
            "case {case:?}: an update addresses the group of one namespace of one \
             account: {body}"
        );
    }
}

#[tokio::test]
async fn updating_a_group_name_no_namespace_holds_is_not_found() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    let (status, body) = create_group(
        &harness,
        ACCOUNT,
        DEFAULT_NAMESPACE,
        json!({ "GroupName": "Sales", "Description": "Sales team" }),
    )
    .await;
    assert_eq!(status, 201, "the create expected 201, got {status} with {body}");

    // A name no group is held under, a name held only in another namespace, a name held
    // only in another account, and an account that holds nothing at all.
    for (case, account, namespace, group_name) in [
        ("unused-name", ACCOUNT, DEFAULT_NAMESPACE, "No Such Group"),
        (
            "name-used-in-another-namespace-only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Sales",
        ),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE, "Sales"),
        ("account-with-nothing", "123456789012", DEFAULT_NAMESPACE, "Sales"),
    ] {
        let (status, body) = update_group(
            &harness,
            account,
            namespace,
            group_name,
            json!({ "Description": "new description" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: updating a group that does not exist expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a name no group is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(group_miss_message(account, namespace, group_name)),
            "case {case:?}: the refusal must name the group, its namespace and its \
             account: {body}"
        );

        // A refused update creates nothing.
        let (status, body) = describe_group(&harness, account, namespace, group_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must persist nothing, got {status} with \
             {body}"
        );
    }

    // And the group the namespace does hold is untouched by the refusals.
    let (status, body) = describe_group(&harness, ACCOUNT, DEFAULT_NAMESPACE, "Sales").await;
    assert_eq!(status, 200, "describe expected 200, got {status} with {body}");
    assert_eq!(
        body["Group"]["Description"],
        json!("Sales team"),
        "a refused update must not change the group it did not reach: {body}"
    );
}

#[tokio::test]
async fn a_group_principal_id_is_the_namespace_and_the_name_it_is_known_by() {
    // The rule: a group's `PrincipalId` is not a handle the service mints but the group
    // itself — the namespace it belongs to and the name it is known by there, which is
    // the resource part of the group's own ARN — so the same value comes back from every
    // read of that group, and the namespace and the name can be recovered from it.
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    for row in rows() {
        let case = row.case;
        let group_name = row.group_name.as_str();
        let namespace = row.namespace;
        let expected = format!("group/{namespace}/{group_name}");
        let expected_arn = group_arn(row.account, namespace, group_name);

        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(expected),
            "case {case:?}: the create must report the group's own identity as its \
             PrincipalId: {body}"
        );
        assert_eq!(
            body["Group"]["Arn"],
            json!(expected_arn),
            "case {case:?}: and that identity is the resource part of the group's ARN: \
             {body}"
        );
        assert!(
            expected_arn.ends_with(&format!(":{expected}")),
            "case {case:?}: the ARN {expected_arn} must end in the principal id \
             {expected}"
        );

        // Every read of the group reports that same value, and so does a write.
        let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(expected),
            "case {case:?}: describe must report the same PrincipalId the create did: \
             {body}"
        );
        let (status, body) = update_group(
            &harness,
            row.account,
            namespace,
            group_name,
            json!({ "Description": "re-described" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(expected),
            "case {case:?}: an update must report the same PrincipalId too: {body}"
        );
    }
}
