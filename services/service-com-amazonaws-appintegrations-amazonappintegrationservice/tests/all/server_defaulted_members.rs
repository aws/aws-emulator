// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members this service fills in itself.
//!
//! The rule under test: a read reports every member the SERVICE assigns a value to,
//! whether or not the create named it — a collection the caller left unset reads back
//! EMPTY (`Publications`, `Subscriptions`, `Permissions`, `Tags`), an unset
//! `IframeConfig` reads back as the empty structure, and `ApplicationType` reads back as
//! the type the service assigned the application. A member the service assigns no
//! default to (`Description`, `InitializationTimeout`, `ApplicationConfig`,
//! `AuthConfig`) stays absent until a caller sets it, and a member the model does not
//! declare on a response is never reported at all.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), so a member that is not computed from
//! persisted state cannot pass. The suites are tables whose rows differ along what the
//! rule generalises over — which members the create names, the count and ORDER of each
//! collection's entries, the tag count, and whether the application is a service — and
//! the single recorded create behind the issue is only one row.

use serde_json::{json, Map, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The `GetApplication` members this service fills in, each with the value a read must
/// report when the create named none of them. `ApplicationType` is handled on its own
/// below because its default depends on the application (see [`assigned_type`]).
const APPLICATION_DEFAULTS: &[(&str, &str)] = &[
    ("Subscriptions", "[]"),
    ("Publications", "[]"),
    ("Permissions", "[]"),
    ("Tags", "{}"),
    ("IframeConfig", "{}"),
];

/// The `GetApplication` members the service assigns NO default to, so a read reports
/// them only when a caller set them.
const APPLICATION_UNSET: &[&str] = &[
    "Description",
    "InitializationTimeout",
    "ApplicationConfig",
    "AuthConfig",
];

/// The `ApplicationType` a read must report for an application whose create named none:
/// the model deprecates `IsService` "in favor of ApplicationType", so a service reads
/// back as `SERVICE` and every other application as `STANDARD`. Both are values of the
/// modelled enum (`smithy_query(query="shape", shape="ApplicationType")`:
/// STANDARD | SERVICE | MCP_SERVER | A2A_SERVER).
fn assigned_type(is_service: bool) -> &'static str {
    if is_service {
        "SERVICE"
    } else {
        "STANDARD"
    }
}

async fn create_application(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/applications", REGION, Some(body))
        .await
}

/// `GET /applications/{Arn}`: the identifier travels in a `@httpLabel`, so its `:` and
/// `/` are percent-encoded.
async fn get_application(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/applications/{}", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

async fn update_application(emulator: &Emulator, identifier: &str, body: Value) -> Reply {
    emulator
        .send(
            "PATCH",
            &format!("/applications/{}", encode_label(identifier)),
            REGION,
            Some(body),
        )
        .await
}

async fn create_data_integration(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/dataIntegrations", REGION, Some(body))
        .await
}

async fn get_data_integration(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/dataIntegrations/{}", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

async fn create_event_integration(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/eventIntegrations", REGION, Some(body))
        .await
}

async fn get_event_integration(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/eventIntegrations/{name}"), REGION, None)
        .await
}

/// The rows of the application table: each is a `CreateApplication` body, and they
/// differ in WHICH server-defaulted members they name (none, one, some, all), in the
/// count and order of each collection's entries, in tag count, and in whether the
/// application is a service.
fn application_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the recorded create: required members only",
            json!({
                "Name": "defaults-none",
                "Namespace": "defaults-none-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
                },
            }),
        ),
        (
            "a service application naming no type",
            json!({
                "Name": "defaults-service",
                "Namespace": "defaults-service-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/service" }
                },
                "IsService": true,
            }),
        ),
        (
            "every collection sent empty on purpose",
            json!({
                "Name": "defaults-empty",
                "Namespace": "defaults-empty-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/empty" }
                },
                "Subscriptions": [],
                "Publications": [],
                "Permissions": [],
            }),
        ),
        (
            "one permission, one tag, no lists and no iframe config",
            json!({
                "Name": "defaults-partial",
                "Namespace": "defaults-partial-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/partial" }
                },
                "Permissions": ["User.Details.View"],
                "Tags": { "owner": "team-a" },
            }),
        ),
        (
            "three permissions in an order of their own, an empty iframe config sent",
            json!({
                "Name": "defaults-ordered",
                "Namespace": "defaults-ordered-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/ordered" }
                },
                "Permissions": [
                    "Contact.CustomerName.View",
                    "User.Details.View",
                    "Contact.Details.View",
                ],
                "IframeConfig": {},
            }),
        ),
        (
            "every defaulted member named, type chosen by the caller",
            json!({
                "Name": "defaults-all",
                "Namespace": "defaults-all-ns",
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/all" }
                },
                "Subscriptions": [{ "Event": "Contact::Created" }],
                "Publications": [{ "Event": "App::Ready", "Schema": "{}" }],
                "Permissions": ["User.Details.View", "Contact.Details.View"],
                "Tags": { "a": "1", "b": "2", "c": "" },
                "IframeConfig": { "Allow": ["microphone"], "Sandbox": ["allow-forms"] },
                "ApplicationType": "MCP_SERVER",
                "IsService": true,
            }),
        ),
    ]
}

/// What a read must report for every server-defaulted member of `sent`: the value the
/// create named, or the default the service assigns when it named none.
fn expected_application_members(sent: &Map<String, Value>) -> Vec<(String, Value)> {
    let is_service = sent
        .get("IsService")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut expected: Vec<(String, Value)> = APPLICATION_DEFAULTS
        .iter()
        .map(|(member, default)| {
            let default: Value =
                serde_json::from_str(default).expect("the defaults table holds JSON");
            (
                (*member).to_string(),
                sent.get(*member).cloned().unwrap_or(default),
            )
        })
        .collect();
    expected.push((
        "ApplicationType".to_string(),
        sent.get("ApplicationType")
            .cloned()
            .unwrap_or_else(|| json!(assigned_type(is_service))),
    ));
    expected
}

fn assert_application_defaults(label: &str, reply: &Reply, sent: &Value) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    let sent = sent.as_object().expect("the create body is an object");
    for (member, expected) in expected_application_members(sent) {
        assert_eq!(
            reply.field(&member),
            Some(&expected),
            "case {label:?}: expected {member} to be reported as {expected}, got {:?}",
            reply.field(&member)
        );
    }
    for member in APPLICATION_UNSET {
        assert_eq!(
            reply.field(member),
            sent.get(*member),
            "case {label:?}: expected {member} to be reported only when the create sent \
             it ({:?}), got {:?}",
            sent.get(*member),
            reply.field(member)
        );
    }
}

#[tokio::test]
async fn get_application_reports_every_server_defaulted_member() {
    let emulator = Emulator::new();

    for (label, body) in application_cases() {
        let created = create_application(&emulator, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to answer HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {label:?}: expected an Id, got {}", created.body));

        let fetched = get_application(&emulator, &id).await;
        assert_application_defaults(label, &fetched, &body);

        // A read changes nothing: the second read reports the identical body, defaults
        // included.
        let again = get_application(&emulator, &id).await;
        assert_eq!(
            again.body, fetched.body,
            "case {label:?}: a second read reported {} after the first reported {}",
            again.body, fetched.body
        );
    }
}

#[tokio::test]
async fn an_update_replaces_a_defaulted_member_and_never_drops_it() {
    // Each step is the complete new value of the member it names, and a read must report
    // it — including the empty collection, which is a value and not an absence. The
    // members the step does not name keep whatever they hold, default or not.
    let steps: Vec<(&str, Value, &str, Value)> = vec![
        (
            "permissions emptied",
            json!({ "Permissions": [] }),
            "Permissions",
            json!([]),
        ),
        (
            "permissions set",
            json!({ "Permissions": ["User.Details.View", "Contact.Details.View"] }),
            "Permissions",
            json!(["User.Details.View", "Contact.Details.View"]),
        ),
        (
            "the same permissions in another order",
            json!({ "Permissions": ["Contact.Details.View", "User.Details.View"] }),
            "Permissions",
            json!(["Contact.Details.View", "User.Details.View"]),
        ),
        (
            "subscriptions set",
            json!({ "Subscriptions": [{ "Event": "Contact::Created" }] }),
            "Subscriptions",
            json!([{ "Event": "Contact::Created" }]),
        ),
        (
            "subscriptions emptied again",
            json!({ "Subscriptions": [] }),
            "Subscriptions",
            json!([]),
        ),
        (
            "the iframe config replaced",
            json!({ "IframeConfig": { "Allow": ["geolocation"] } }),
            "IframeConfig",
            json!({ "Allow": ["geolocation"] }),
        ),
        (
            "the type changed",
            json!({ "ApplicationType": "A2A_SERVER" }),
            "ApplicationType",
            json!("A2A_SERVER"),
        ),
        (
            "nothing updatable at all",
            json!({}),
            "Publications",
            json!([]),
        ),
    ];

    let emulator = Emulator::new();
    let created = create_application(
        &emulator,
        json!({
            "Name": "patched-defaults",
            "Namespace": "patched-defaults-ns",
            "ApplicationSourceConfig": {
                "ExternalUrlConfig": { "AccessUrl": "https://example.com/patched" }
            },
        }),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");

    for (label, patch, member, expected) in steps {
        let patched = update_application(&emulator, &id, patch).await;
        assert_eq!(
            patched.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateApplication, got {} / {}",
            patched.status, patched.body
        );
        let fetched = get_application(&emulator, &id).await;
        assert_eq!(
            fetched.field(member),
            Some(&expected),
            "case {label:?}: expected {member} to be reported as {expected}, got {:?}",
            fetched.field(member)
        );
        // Whatever the step touched, every defaulted member is still reported.
        for (defaulted, _) in APPLICATION_DEFAULTS {
            assert!(
                fetched.field(defaulted).is_some(),
                "case {label:?}: expected {defaulted} to still be reported, got body {}",
                fetched.body
            );
        }
        assert!(
            fetched.field("ApplicationType").is_some(),
            "case {label:?}: expected ApplicationType to still be reported, got body {}",
            fetched.body
        );
    }
}

/// The rows of the data-integration table: they differ in tag count, in which optional
/// members the create names, and in the source the integration pulls from.
fn data_integration_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the recorded create: no tags, no source",
            json!({ "Name": "typed.minimal" }),
        ),
        (
            "one tag and an s3 source",
            json!({
                "Name": "typed.one-tag",
                "SourceURI": "s3://feed-bucket/accounts",
                "Tags": { "team": "crm" },
            }),
        ),
        (
            "three tags, a description and a schedule",
            json!({
                "Name": "typed.three-tags",
                "Description": "pulls the account feed",
                "SourceURI": "https://example.com/feed",
                "ScheduleConfig": {
                    "ScheduleExpression": "rate(1 hour)",
                    "FirstExecutionFrom": "1685400000",
                    "Object": "Account",
                },
                "Tags": { "a": "1", "b": "2", "c": "" },
            }),
        ),
        (
            "an object configuration and no tags",
            json!({
                "Name": "typed.object-config",
                "SourceURI": "salesforce://my.instance/data",
                "ObjectConfiguration": { "Customer": { "Account": ["Id", "Name"] } },
            }),
        ),
    ]
}

#[tokio::test]
async fn get_data_integration_reports_an_empty_tag_map_and_only_modelled_members() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    for (label, mut body) in data_integration_cases() {
        body["KmsKey"] = json!(kms_key);
        let created = create_data_integration(&emulator, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to answer HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {label:?}: expected an Id, got {}", created.body));
        let arn = created.string("Arn").expect("the create reported an ARN");

        // Both identifier forms address the integration, and both report the same thing.
        for (form, identifier) in [("by id", id.clone()), ("by arn", arn.clone())] {
            let fetched = get_data_integration(&emulator, &identifier).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?} {form}: expected HTTP 200, got {} with body {}",
                fetched.status, fetched.body
            );
            // Only the members `GetDataIntegrationResponse` declares are reported:
            // this service assigns and persists a kind for the integration, and the
            // model declares no member for it (`smithy_query(query="operation",
            // operation="GetDataIntegration")`), so none travels on the wire.
            assert_eq!(
                fetched.field("IntegrationType"),
                None,
                "case {label:?} {form}: GetDataIntegrationResponse models no \
                 IntegrationType, got {:?}",
                fetched.field("IntegrationType")
            );
            let expected_tags = body.get("Tags").cloned().unwrap_or_else(|| json!({}));
            assert_eq!(
                fetched.field("Tags"),
                Some(&expected_tags),
                "case {label:?} {form}: expected Tags {expected_tags}, got {:?}",
                fetched.field("Tags")
            );
        }
    }
}

#[tokio::test]
async fn get_event_integration_reports_an_empty_tag_map_for_an_untagged_integration() {
    let emulator = Emulator::new();

    // Rows differ in tag count: none, one, several — the axis the empty-map rule
    // generalises over.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("no tags", "untagged.integration", json!(null)),
        (
            "one tag",
            "one-tag.integration",
            json!({ "owner": "team-a" }),
        ),
        (
            "three tags, one with an empty value",
            "three-tags.integration",
            json!({ "a": "1", "b": "2", "c": "" }),
        ),
    ];

    for (label, name, tags) in rows {
        let mut body = json!({
            "Name": name,
            "EventBridgeBus": "default",
            "EventFilter": { "Source": "aws.partner/demo" },
        });
        if !tags.is_null() {
            body["Tags"] = tags.clone();
        }
        let created = create_event_integration(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to answer HTTP 200, got {} with body {}",
            created.status, created.body
        );

        let fetched = get_event_integration(&emulator, name).await;
        let expected_tags = if tags.is_null() { json!({}) } else { tags };
        assert_eq!(
            fetched.field("Tags"),
            Some(&expected_tags),
            "case {label:?}: expected Tags {expected_tags}, got {:?}",
            fetched.field("Tags")
        );
    }
}
