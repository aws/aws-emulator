// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateApplication` (`PATCH /applications/{Arn}`), read back
//! through `GetApplication`.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The suites are tables. The rule under test is that a member the request carries
//! REPLACES the stored one whole — a list arrives whole and is stored whole, never
//! merged with the list already there — and a member the request omits is left alone, so
//! the rows differ along what that generalises over: which members the PATCH carries,
//! the size of each list (empty, shorter, longer, at the published bound), the ORDER of
//! a list's entries, the form and length of each scalar, and which identifier form
//! addressed the application.

use serde_json::{json, Map, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// `Description` is `len 0..1000` (model: `UpdateApplication` input).
const DESCRIPTION_MAX: usize = 1000;
/// `PermissionList` is `len 0..150`, `SubscriptionList` and `PublicationList` `len 0..50`
/// (model: `smithy_query(query="operation", operation="UpdateApplication")`).
const PERMISSIONS_MAX: usize = 150;
const SUBSCRIPTIONS_MAX: usize = 50;
/// `InitializationTimeout` is `range 1..600000` (same model entry).
const INITIALIZATION_TIMEOUT_MIN: i64 = 1;

/// The members a read reports that no `UpdateApplication` input can touch: the identity
/// the create minted, its namespace, its creation instant and its tags (`TagResource`
/// owns those).
const UNTOUCHABLE: &[&str] = &["Id", "Arn", "Namespace", "CreatedTime", "Tags"];

/// Every member `UpdateApplication` accepts beyond the identifier, so a row's
/// expectations can be checked over the whole updatable surface rather than only the
/// members it happened to send. `IsService` is absent here because the model gives it a
/// default (false), which a response need not spell out: it is compared as a boolean.
const UPDATABLE: &[&str] = &[
    "Name",
    "Description",
    "ApplicationSourceConfig",
    "Subscriptions",
    "Publications",
    "Permissions",
    "InitializationTimeout",
    "ApplicationConfig",
    "IframeConfig",
    "ApplicationType",
    "AuthConfig",
];

async fn create(emulator: &Emulator, body: Value) -> Reply {
    create_in(emulator, REGION, body).await
}

/// `POST /applications` signed for `region`, which is the region the call is made in.
async fn create_in(emulator: &Emulator, region: &str, body: Value) -> Reply {
    emulator
        .send("POST", "/applications", region, Some(body))
        .await
}

/// `GET /applications/{Arn}`: the identifier travels in a `@httpLabel`, so its `:` and
/// `/` are percent-encoded.
async fn get(emulator: &Emulator, identifier: &str) -> Reply {
    get_in(emulator, REGION, identifier).await
}

/// The same read, made in `region`.
async fn get_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/applications/{}", encode_label(identifier)),
            region,
            None,
        )
        .await
}

/// `PATCH /applications/{Arn}` with `body`.
async fn update(emulator: &Emulator, identifier: &str, body: Value) -> Reply {
    update_in(emulator, REGION, identifier, body).await
}

/// The same update, made in `region`.
async fn update_in(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "PATCH",
            &format!("/applications/{}", encode_label(identifier)),
            region,
            Some(body),
        )
        .await
}

/// The application every row patches: every updatable member is already set to a
/// non-empty value, so a row that replaces one can be told from a row that merges into
/// it or leaves it behind.
fn populated_application(name: &str, namespace: &str) -> Value {
    json!({
        "Name": name,
        "Namespace": namespace,
        "Description": "the description the create stored",
        "ApplicationSourceConfig": {
            "ExternalUrlConfig": {
                "AccessUrl": "https://example.com/created",
                "ApprovedOrigins": ["https://a.example.com", "https://b.example.com"],
            }
        },
        "Subscriptions": [
            { "Event": "Contact::Created", "Description": "new contacts" },
            { "Event": "Contact::Updated*" },
        ],
        "Publications": [
            { "Event": "App::Ready", "Schema": "{\"type\":\"object\"}", "Description": "ready" },
        ],
        "Permissions": ["User.Details.View", "Contact.Details.View", "Contact.CustomerName.View"],
        "Tags": { "owner": "team-a" },
        "IsService": true,
        "InitializationTimeout": 30_000,
        "ApplicationConfig": { "ContactHandling": { "Scope": "CROSS_CONTACTS" } },
        "IframeConfig": {
            "Allow": ["microphone", "camera"],
            "Sandbox": ["allow-forms", "allow-scripts"],
        },
        "ApplicationType": "STANDARD",
        "AuthConfig": {
            "AuthType": "CONNECT_JWT",
            "AudienceOverride": "https://audience.example.com",
        },
    })
}

/// A list of `count` distinct `Permission` values (member pattern
/// `^[a-zA-Z0-9\/\._\-\*]+$`, `len 1..255`).
fn permissions(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("Permission.{index}.View")).collect()
}

/// A list of `count` distinct `Subscription` entries.
fn subscriptions(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({ "Event": format!("Contact::Event{index}") }))
        .collect()
}

/// One row: the label that names it in an assertion and the PATCH body it sends.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    patch: Value,
}

fn case(label: &'static str, patch: Value) -> Case {
    Case { label, patch }
}

/// The table every replacement assertion runs over.
fn patch_cases() -> Vec<Case> {
    vec![
        // What the recorded calls in the issue carry: one row of many.
        case(
            "the recorded pair: a description and a permission list",
            json!({
                "Description": "the description the update stored",
                "Permissions": ["User.Details.View"],
            }),
        ),
        case(
            "permissions replaced by a shorter, disjoint list",
            json!({ "Permissions": ["Contact.Attributes.View"] }),
        ),
        case(
            "permissions replaced by an empty list",
            json!({ "Permissions": [] }),
        ),
        case(
            "the same permissions in another order",
            json!({
                "Permissions": [
                    "Contact.CustomerName.View",
                    "User.Details.View",
                    "Contact.Details.View",
                ]
            }),
        ),
        case(
            "permissions at the published upper bound",
            json!({ "Permissions": permissions(PERMISSIONS_MAX) }),
        ),
        case("only a rename", json!({ "Name": "renamed application" })),
        case(
            "a description cleared to the empty string",
            json!({ "Description": "" }),
        ),
        case(
            "a description at its published upper bound",
            json!({ "Description": "d".repeat(DESCRIPTION_MAX) }),
        ),
        case(
            "the deprecated lists replaced wholesale",
            json!({
                "Subscriptions": subscriptions(SUBSCRIPTIONS_MAX),
                "Publications": [
                    { "Event": "App::Closed", "Schema": "{}" },
                ],
            }),
        ),
        case(
            "both deprecated lists emptied",
            json!({ "Subscriptions": [], "Publications": [] }),
        ),
        case(
            "the source config replaced, dropping its approved origins",
            json!({
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/updated" }
                }
            }),
        ),
        case(
            "the iframe config replaced with fewer entries",
            json!({ "IframeConfig": { "Allow": ["geolocation"] } }),
        ),
        case(
            "the auth config and application config replaced",
            json!({
                "AuthConfig": { "AuthType": "CONNECT_JWT" },
                "ApplicationConfig": { "ContactHandling": { "Scope": "PER_CONTACT" } },
            }),
        ),
        case(
            "the type changed, the service flag cleared and the timeout at its lower bound",
            json!({
                "ApplicationType": "MCP_SERVER",
                "IsService": false,
                "InitializationTimeout": INITIALIZATION_TIMEOUT_MIN,
            }),
        ),
        case(
            "every updatable member at once",
            json!({
                "Name": "fully.updated-app",
                "Description": "everything replaced",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": {
                        "AccessUrl": "https://example.com/everything",
                        "ApprovedOrigins": ["https://only.example.com"],
                    }
                },
                "Subscriptions": [{ "Event": "Contact::Deleted", "Description": "gone" }],
                "Publications": [{ "Event": "App::Done", "Schema": "{}", "Description": "done" }],
                "Permissions": permissions(7),
                "IsService": false,
                "InitializationTimeout": 600_000,
                "ApplicationConfig": { "ContactHandling": { "Scope": "PER_CONTACT" } },
                "IframeConfig": { "Sandbox": ["allow-popups"] },
                "ApplicationType": "A2A_SERVER",
                "AuthConfig": { "AuthType": "CONNECT_JWT", "AudienceOverride": "https://b.example" },
            }),
        ),
        case("no updatable member at all", json!({})),
    ]
}

/// The member values a read must report after `patch` was applied to `stored`:
/// everything `stored` carries, with each member `patch` carries REPLACED by the value
/// it carries.
fn expected_members(stored: &Value, patch: &Value) -> Map<String, Value> {
    let mut expected = stored
        .as_object()
        .expect("the create body is an object")
        .clone();
    for (member, value) in patch.as_object().expect("the patch body is an object") {
        expected.insert(member.clone(), value.clone());
    }
    expected
}

/// The `IsService` a reply reports: the model defaults the member to false, so a reply
/// that spells out no value reports false.
fn is_service(reply: &Reply) -> bool {
    reply
        .field("IsService")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The epoch seconds a reported timestamp carries.
fn seconds(reply: &Reply, member: &str) -> f64 {
    reply
        .field(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric {member}, got body {}", reply.body))
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_the_others_alone() {
    let emulator = Emulator::new();

    for (index, Case { label, patch }) in patch_cases().into_iter().enumerate() {
        // Both identifier forms the model's `ArnOrUUID` allows address the application,
        // and a trailing qualifier does not change which one.
        for form in ["bare UUID", "full ARN", "UUID with a qualifier"] {
            let case_label = format!("{label} / {form}");
            let created = create(
                &emulator,
                populated_application(
                    &format!("patched-app-{index}"),
                    &format!("patched-ns-{index}-{}", form.replace(' ', "-")),
                ),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case_label:?}: expected the create to answer HTTP 200, got {} / {}",
                created.status, created.body
            );
            let id = created.string("Id").expect("the create reported an Id");
            let arn = created.string("Arn").expect("the create reported an ARN");
            let identifier = match form {
                "bare UUID" => id.clone(),
                "full ARN" => arn.clone(),
                _ => format!("{id}:1"),
            };

            let before = get(&emulator, &identifier).await;
            let stored = populated_application(
                &format!("patched-app-{index}"),
                &format!("patched-ns-{index}-{}", form.replace(' ', "-")),
            );
            let expected = expected_members(&stored, &patch);
            // Whether this row asks for a value different from the one already stored:
            // only then does the resource's own clock move.
            let changes_something =
                patch
                    .as_object()
                    .expect("the patch body is an object")
                    .iter()
                    .any(|(member, value)| {
                        if member == "IsService" {
                            value.as_bool() != Some(is_service(&before))
                        } else {
                            before.field(member) != Some(value)
                        }
                    });

            let patched = update(&emulator, &identifier, patch.clone()).await;
            assert_eq!(
                patched.status, 200,
                "case {case_label:?}: expected HTTP 200 from UpdateApplication, got {} / {}",
                patched.status, patched.body
            );

            let fetched = get(&emulator, &identifier).await;
            assert_eq!(
                fetched.status, 200,
                "case {case_label:?}: expected the application to still be readable, got {} / {}",
                fetched.status, fetched.body
            );
            for member in UPDATABLE {
                assert_eq!(
                    fetched.field(member),
                    expected.get(*member).filter(|value| !value.is_null()),
                    "case {case_label:?}: expected {member} to be {:?}, got {:?}",
                    expected.get(*member),
                    fetched.field(member)
                );
            }
            let expected_is_service = expected
                .get("IsService")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            assert_eq!(
                is_service(&fetched),
                expected_is_service,
                "case {case_label:?}: expected IsService {expected_is_service}, got {:?}",
                fetched.field("IsService")
            );
            // The identity the create minted, the namespace and the tags are not inputs
            // of this operation, so the update leaves every one of them as it was.
            for member in UNTOUCHABLE {
                assert_eq!(
                    fetched.field(member),
                    before.field(member),
                    "case {case_label:?}: an update must not change {member}, got {:?} after {:?}",
                    fetched.field(member),
                    before.field(member)
                );
            }

            let created_time = seconds(&fetched, "CreatedTime");
            let last_modified = seconds(&fetched, "LastModifiedTime");
            if changes_something {
                assert!(
                    last_modified > created_time,
                    "case {case_label:?}: expected LastModifiedTime past CreatedTime \
                     ({created_time}), got {last_modified}"
                );
            } else {
                assert_eq!(
                    last_modified, created_time,
                    "case {case_label:?}: an update that changed nothing must leave \
                     LastModifiedTime at {created_time}, got {last_modified}"
                );
            }

            // Reading twice reports the same thing: neither the update nor the read
            // changes what a later caller sees.
            let again = get(&emulator, &identifier).await;
            assert_eq!(
                again.body, fetched.body,
                "case {case_label:?}: a second read reported {} after the first reported {}",
                again.body, fetched.body
            );
        }
    }
}

#[tokio::test]
async fn a_second_update_replaces_the_first_one_rather_than_merging_into_it() {
    let emulator = Emulator::new();
    let created = create(
        &emulator,
        populated_application("sequenced application", "sequenced-namespace"),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");

    // Each step's list is the complete new value, so what a read reports is the LAST
    // list sent and never the union of the ones before it.
    let steps: Vec<(&str, Value)> = vec![
        ("three permissions", json!(permissions(3))),
        ("one permission", json!(["User.Details.View"])),
        ("no permissions", json!([])),
        ("the upper bound", json!(permissions(PERMISSIONS_MAX))),
        ("two permissions", json!(["A.View", "B.View"])),
    ];
    for (label, expected) in steps {
        let patched = update(&emulator, &id, json!({ "Permissions": expected })).await;
        assert_eq!(
            patched.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            patched.status, patched.body
        );
        let fetched = get(&emulator, &id).await;
        assert_eq!(
            fetched.field("Permissions"),
            Some(&expected),
            "case {label:?}: expected exactly the permissions the last update sent, got {:?}",
            fetched.field("Permissions")
        );
    }
}

#[tokio::test]
async fn an_update_of_an_identifier_no_application_carries_is_not_found() {
    let emulator = Emulator::new();
    // One live application, so the refusals below are about the identifier and not
    // about an empty store.
    let created = create(
        &emulator,
        populated_application("live application", "live-namespace"),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");
    let account = engine::EMULATOR_ACCOUNT_ID;

    let unknown_id = "00000000-0000-0000-0000-000000000000";
    for (label, identifier) in [
        ("a UUID no application carries", unknown_id.to_string()),
        (
            "the ARN of that UUID",
            format!("arn:aws:app-integrations:{REGION}:{account}:application/{unknown_id}"),
        ),
        (
            "a live id under another region",
            format!("arn:aws:app-integrations:eu-west-1:{account}:application/{id}"),
        ),
        (
            "a live id under another resource type",
            format!("arn:aws:app-integrations:{REGION}:{account}:data-integration/{id}"),
        ),
    ] {
        let reply = update(
            &emulator,
            &identifier,
            json!({ "Description": "never stored" }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.message(),
            Some(format!(
                "Could not find Application with identifier: {identifier}."
            )),
            "case {label:?}: expected the templated not-found message, got body {}",
            reply.body
        );
    }

    // The live application is untouched by every refusal above.
    let fetched = get(&emulator, &id).await;
    assert_eq!(
        fetched.string("Description").as_deref(),
        Some("the description the create stored"),
        "a refused update must change nothing, got {}",
        fetched.body
    );
    assert_eq!(
        seconds(&fetched, "LastModifiedTime"),
        seconds(&fetched, "CreatedTime"),
        "a refused update must leave LastModifiedTime at CreatedTime, got {}",
        fetched.body
    );
}

#[tokio::test]
async fn a_rename_onto_a_name_taken_in_the_same_namespace_is_refused() {
    // The (Namespace, Name) pair is an application's identity, so a rename cannot make
    // two applications share one — whatever the form of the name.
    for taken in ["neighbour application", "a", &"n".repeat(255)] {
        let emulator = Emulator::new();
        let neighbour = create(&emulator, populated_application(taken, "shared-namespace")).await;
        assert_eq!(
            neighbour.status, 200,
            "case {taken:?}: expected the neighbour create to succeed, got {} / {}",
            neighbour.status, neighbour.body
        );
        let subject = create(
            &emulator,
            populated_application("subject application", "shared-namespace"),
        )
        .await;
        let subject_id = subject.string("Id").expect("the create reported an Id");

        let refused = update(&emulator, &subject_id, json!({ "Name": taken })).await;
        assert_eq!(
            refused.status, 409,
            "case {taken:?}: expected HTTP 409 for a rename onto a taken name, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case {taken:?}: expected ConflictException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.message(),
            Some(format!(
                "An application named {taken} already exists in namespace shared-namespace."
            )),
            "case {taken:?}: expected the duplicate-name message, got body {}",
            refused.body
        );
        // The refused rename persisted nothing.
        let fetched = get(&emulator, &subject_id).await;
        assert_eq!(
            fetched.string("Name").as_deref(),
            Some("subject application"),
            "case {taken:?}: a refused rename must leave the stored Name, got {:?}",
            fetched.field("Name")
        );

        // The same name under ANOTHER namespace is free, and renaming an application to
        // the name it already carries is not a conflict with itself.
        let elsewhere = create(&emulator, populated_application(taken, "other-namespace")).await;
        let elsewhere_id = elsewhere.string("Id").expect("the create reported an Id");
        let accepted = update(&emulator, &elsewhere_id, json!({ "Name": taken })).await;
        assert_eq!(
            accepted.status, 200,
            "case {taken:?}: renaming an application to its own name must be accepted, got {} / {}",
            accepted.status, accepted.body
        );
        let moved = update(&emulator, &subject_id, json!({ "Name": "a free name" })).await;
        assert_eq!(
            moved.status, 200,
            "case {taken:?}: expected a free name to be accepted, got {} / {}",
            moved.status, moved.body
        );
    }
}

/// One row of the `LastModifiedRegion` table: the region the create is made in, the
/// region the changing update is made in, and the region the updates that change
/// nothing are made in.
///
/// Every value is a `RegionName` (model: `GetApplicationResponse.LastModifiedRegion`,
/// `len 1..25`, pattern `^[a-z]{2,4}(-[a-z0-9]+){1,3}$`), and the rows differ along what
/// the rule generalises over: how many segments the region name carries, and whether the
/// modifying call lands in the region the create did.
const REGION_ROWS: &[(&str, &str, &str)] = &[
    ("us-east-1", "us-east-1", "eu-west-1"),
    ("eu-west-2", "us-west-2", "ap-south-1"),
    ("ap-southeast-3", "eu-central-1", "us-east-2"),
    ("us-gov-west-1", "cn-north-1", "me-south-1"),
];

#[tokio::test]
async fn last_modified_region_is_the_region_of_the_call_that_last_changed_the_application() {
    for (index, (created_in, changed_in, elsewhere)) in REGION_ROWS.iter().enumerate() {
        let label = format!("created in {created_in}, changed in {changed_in}");
        // A fresh store per row, so the (Namespace, Name) pair below is free and no
        // other row's application is in the way.
        let emulator = Emulator::new();
        let created = create_in(
            &emulator,
            created_in,
            populated_application(
                &format!("regional-app-{index}"),
                &format!("regional-ns-{index}"),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        let id = created.string("Id").expect("the create reported an Id");

        // Nothing has modified the application yet, so it was last modified where it was
        // created — and reading it from another region reports the same thing, because
        // the member comes from the record and not from the request.
        for read_in in [created_in, elsewhere] {
            let fetched = get_in(&emulator, read_in, &id).await;
            assert_eq!(
                fetched.string("LastModifiedRegion").as_deref(),
                Some(*created_in),
                "case {label:?}: a read made in {read_in} must report the create's \
                 region, got {:?}",
                fetched.field("LastModifiedRegion")
            );
        }

        // An update that changes nothing (it re-sends the stored description) leaves the
        // region it was last modified in exactly where the timestamp stays.
        let no_op = update_in(
            &emulator,
            elsewhere,
            &id,
            json!({ "Description": "the description the create stored" }),
        )
        .await;
        assert_eq!(
            no_op.status, 200,
            "case {label:?}: expected HTTP 200 from an update that changes nothing, got {} / {}",
            no_op.status, no_op.body
        );
        let fetched = get_in(&emulator, created_in, &id).await;
        assert_eq!(
            fetched.string("LastModifiedRegion").as_deref(),
            Some(*created_in),
            "case {label:?}: an update that changed nothing must leave LastModifiedRegion \
             at {created_in}, got {:?}",
            fetched.field("LastModifiedRegion")
        );
        assert_eq!(
            seconds(&fetched, "LastModifiedTime"),
            seconds(&fetched, "CreatedTime"),
            "case {label:?}: an update that changed nothing must leave LastModifiedTime \
             at CreatedTime, got body {}",
            fetched.body
        );

        // An update that DOES change a member moves the region to the one that call was
        // made in, alongside the timestamp.
        let patched = update_in(
            &emulator,
            changed_in,
            &id,
            json!({ "Description": format!("changed from {changed_in}") }),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateApplication, got {} / {}",
            patched.status, patched.body
        );
        let fetched = get_in(&emulator, created_in, &id).await;
        assert_eq!(
            fetched.string("LastModifiedRegion").as_deref(),
            Some(*changed_in),
            "case {label:?}: expected LastModifiedRegion to be the updating call's \
             region, got {:?}",
            fetched.field("LastModifiedRegion")
        );
        assert!(
            seconds(&fetched, "LastModifiedTime") > seconds(&fetched, "CreatedTime"),
            "case {label:?}: expected LastModifiedTime past CreatedTime, got body {}",
            fetched.body
        );

        // And a later update that changes nothing, made from a third region, leaves the
        // region of the call that did change something.
        let no_op_again = update_in(
            &emulator,
            elsewhere,
            &id,
            json!({ "Description": format!("changed from {changed_in}") }),
        )
        .await;
        assert_eq!(
            no_op_again.status, 200,
            "case {label:?}: expected HTTP 200 from the second no-op update, got {} / {}",
            no_op_again.status, no_op_again.body
        );
        let fetched = get_in(&emulator, created_in, &id).await;
        assert_eq!(
            fetched.string("LastModifiedRegion").as_deref(),
            Some(*changed_in),
            "case {label:?}: an update that changed nothing must leave LastModifiedRegion \
             at {changed_in}, got {:?}",
            fetched.field("LastModifiedRegion")
        );
    }
}
