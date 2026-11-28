// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch UI application surface: `CreateApplication`
//! and the `GetApplication` read it round-trips through.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the tagging paths from [`super::resource_tags`];
//! this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a create mints ONE application from the request and every read path reports that
//!   same stored record, over a table of eight creates differing in how many data
//!   sources and configuration settings they name (none, one, several, an explicitly
//!   empty list), in which configuration keys they carry, in whether they ask for IAM
//!   Identity Center, a customer-managed key or tags, and in the length and shape of
//!   the name (both published bounds included);
//! * the id a create mints is unique per application, and so is the endpoint derived
//!   from it;
//! * the application is minted in a transient status and the clock owes it the status
//!   provisioning leaves it in: `CREATING` the instant the call returns and `ACTIVE`
//!   once the span has passed, without any call having asked for the move;
//! * a name this account already holds an application under is refused with the
//!   modelled `ConflictException` and nothing is created;
//! * `clientToken` makes a create idempotent: the same token twice answers with the
//!   one application the first call minted, while the same token for a different name
//!   is the modelled conflict;
//! * an id that names no application of this account is refused with the modelled
//!   `ResourceNotFoundException`;
//! * an application's tags are the set the create named, reported and changed through
//!   the same tagging paths a domain's are, and a create naming more than the
//!   documented number of tags is refused and creates nothing;
//! * the URL an application publishes for a caller to FOLLOW is built over the endpoint
//!   this emulator is reached at, with the application's own id in the path, and this
//!   emulator answers there out of the stored application — asserted over the forms a
//!   caller can address this emulator by and over ids no application is held under.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::APPLICATION_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    APPLICATION_DOES_NOT_EXIST, APPLICATION_QUOTA, APPLICATION_STATUS_ACTIVE,
    APPLICATION_STATUS_CREATING, TAG_QUOTA, TAG_QUOTA_EXCEEDED,
};

use super::application_deletion::delete_application;
use super::application_updates::read_list;
use super::domain_lifecycle::{
    error_code, poll_until, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
    SETTLE_WAIT,
};
use super::resource_tags::{add_tags, list_tags, reported_tags};

/// The Region every request of this suite is signed for, so the ARN a create reports is
/// built for it.
const REGION: &str = "us-east-1";

/// The path a published application endpoint carries, ahead of the application's own id:
/// the reserved namespace segment every data plane of this emulator is addressed under
/// (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`) and the segment that
/// names this service's application plane (`state::APPLICATION_UI_PLANE`).
///
/// Written out here rather than read back off the implementation, so a case states the
/// address a client is handed.
const ENDPOINT_PATH_PREFIX: &str = "/_aws/opensearch-application";

/// The `host[:port]` an emulator served on NO port at all publishes its addresses on: the
/// base `smithy_server_runtime` reports for an unserved emulator, without its scheme.
///
/// This suite drives the router in process, so no port is open and this is the authority
/// every address it publishes carries. It is a host reserved so that nothing resolves it —
/// never one of AWS's.
fn unserved_authority() -> String {
    smithy_server_runtime::UNSERVED_URL_BASE
        .trim_start_matches("https://")
        .to_string()
}

/// A customer-managed key ARN satisfying `KmsKeyArn`'s own pattern.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// `CreateApplication`: `POST /2021-01-01/opensearch/application`.
async fn create_application(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(body),
        )
        .await
}

/// `GetApplication`: `GET /2021-01-01/opensearch/application/{id}`.
async fn get_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await
}

/// Whether `id` is a value the model's `Id` allows: its pattern is
/// `^[a-z0-9]{3,30}$` (`smithy_query(query="shape",shape="GetApplicationResponse")`).
fn is_application_id(id: &str) -> bool {
    (3..=30).contains(&id.chars().count())
        && id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// What a response reports for a LIST member the request carried: the list itself
/// when it had entries, and nothing at all when the request named none or named an
/// empty one — a member the application holds nothing for is left out rather than
/// answered with an empty list.
fn expected_list(request: &Value, member: &str) -> Value {
    match request.get(member) {
        Some(Value::Array(items)) if !items.is_empty() => Value::Array(items.clone()),
        _ => Value::Null,
    }
}

/// A `tagList` member as (key, value) pairs sorted by key, so a case compares the SET
/// the application carries rather than the order one answer happened to use: a tag set
/// holds one value per key, so the service reports it keyed rather than as the request
/// listed it.
fn tag_pairs(list: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = list
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// The two values `AppConfigType` declares
/// (`smithy_query(query="shape",shape="AppConfigType")`), so the table below carries a
/// configuration key the issue did not cite as well as one it did.
const APP_CONFIG_USERS: &str = "opensearchDashboards.dashboardAdmin.users";
const APP_CONFIG_GROUPS: &str = "opensearchDashboards.dashboardAdmin.groups";

/// One `CreateApplication` request the round-trip rule is asserted over.
struct Row {
    case: &'static str,
    request: Value,
}

/// The table every row of [`a_created_application_round_trips_through_get`] drives:
/// one row per way a create can differ — the members it carries, the lists' sizes,
/// the configuration keys, and the name at each published length bound.
fn creates() -> Vec<Row> {
    vec![
        Row {
            case: "only the required name, at the 3-character lower bound",
            request: json!({ "name": "ui1" }),
        },
        Row {
            // `ApplicationName`'s upper bound is 30 characters; this name is exactly 30.
            case: "a 30-character name at the upper bound",
            request: json!({ "name": "abcdefghij-klmnopqrst-uvwxyz12" }),
        },
        Row {
            case: "one data source",
            request: json!({
                "name": "search-ui",
                "dataSources": [{
                    "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/logs",
                    "dataSourceDescription": "the logs domain",
                }],
            }),
        },
        Row {
            case: "three data sources, one with a cross-account role",
            request: json!({
                "name": "multi-source-ui",
                "dataSources": [
                    { "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/logs" },
                    {
                        "dataSourceArn": "arn:aws:s3:::my-catalog",
                        "dataSourceDescription": "an S3 Glue catalog",
                        "iamRoleForDataSourceArn":
                            "arn:aws:iam::123456789012:role/data-source-reader",
                    },
                    { "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/metrics" },
                ],
            }),
        },
        Row {
            case: "both configuration keys",
            request: json!({
                "name": "admin-ui",
                "appConfigs": [
                    { "key": APP_CONFIG_USERS, "value": "arn:aws:iam::123456789012:user/ana" },
                    { "key": APP_CONFIG_GROUPS, "value": "operators" },
                ],
            }),
        },
        Row {
            case: "explicitly empty lists",
            request: json!({
                "name": "empty-lists-ui",
                "dataSources": [],
                "appConfigs": [],
                "tagList": [],
            }),
        },
        Row {
            case: "IAM Identity Center and a customer-managed key",
            request: json!({
                "name": "federated-ui",
                "iamIdentityCenterOptions": {
                    "enabled": true,
                    "iamIdentityCenterInstanceArn":
                        "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                    "iamRoleForIdentityCenterApplicationArn":
                        "arn:aws:iam::123456789012:role/identity-center",
                },
                "kmsKeyArn": KMS_KEY,
            }),
        },
        Row {
            case: "tags, one configuration setting and a data source together",
            request: json!({
                "name": "everything-ui",
                "dataSources": [{ "dataSourceArn": "arn:aws:s3:::everything" }],
                "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "everyone" }],
                "tagList": [
                    { "Key": "team", "Value": "search" },
                    { "Key": "cost-center", "Value": "" },
                ],
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_application_round_trips_through_get() {
    let emulator = Emulator::new();
    let mut minted: Vec<(&str, String, String)> = Vec::new();

    for row in creates() {
        let case = row.case;
        let created = create_application(&emulator, row.request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );

        let id = string_member(&created.body, "id", case);
        assert!(
            is_application_id(&id),
            "case {case}: the minted id is an Id the model allows, got {id:?}"
        );
        let name = row.request["name"].as_str().unwrap_or_default();
        assert_eq!(
            created.body["name"],
            json!(name),
            "case {case}: the create reports the name it was given, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["arn"],
            json!(format!(
                "arn:aws:opensearch:{REGION}:{}:application/{id}",
                engine::emulator_account_id()
            )),
            "case {case}: the create reports the application's own ARN, got {:?}",
            created.body
        );
        assert!(
            created.body["createdAt"].is_number(),
            "case {case}: the create reports when the application was created, got {:?}",
            created.body
        );
        for member in ["dataSources", "appConfigs"] {
            assert_eq!(
                created.body[member],
                expected_list(&row.request, member),
                "case {case}: the create reports the {member} it was given, got {:?}",
                created.body
            );
        }
        assert_eq!(
            tag_pairs(&created.body["tagList"]),
            tag_pairs(&row.request["tagList"]),
            "case {case}: the create reports the tags it was given, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["tagList"].is_null(),
            expected_list(&row.request, "tagList").is_null(),
            "case {case}: a create that named no tags reports none, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["kmsKeyArn"],
            row.request
                .get("kmsKeyArn")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case}: the create reports the key it was given, got {:?}",
            created.body
        );
        match row.request.get("iamIdentityCenterOptions") {
            None => assert_eq!(
                created.body["iamIdentityCenterOptions"],
                Value::Null,
                "case {case}: a create that asked for no Identity Center reports none, got {:?}",
                created.body
            ),
            Some(asked) => {
                for member in [
                    "enabled",
                    "iamIdentityCenterInstanceArn",
                    "iamRoleForIdentityCenterApplicationArn",
                ] {
                    assert_eq!(
                        created.body["iamIdentityCenterOptions"][member], asked[member],
                        "case {case}: the create reports the {member} it was given, got {:?}",
                        created.body
                    );
                }
                assert_eq!(
                    created.body["iamIdentityCenterOptions"]["iamIdentityCenterApplicationArn"],
                    Value::Null,
                    "case {case}: the Identity Center application's own ARN is minted by that \
                     service, got {:?}",
                    created.body
                );
            }
        }

        // The read of the id reports the SAME record the create answered with, plus
        // the two members only a read carries.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        for member in ["id", "name", "arn", "dataSources", "kmsKeyArn"] {
            assert_eq!(
                read.body[member], created.body[member],
                "case {case}: the read reports the {member} the create persisted, got {:?}",
                read.body
            );
        }
        // The read reports the whole stored record, so the configuration settings come
        // back as the list the application holds — `[]` where it holds none.
        assert_eq!(
            read.body["appConfigs"],
            read_list(row.request.get("appConfigs")),
            "case {case}: the read reports the configuration settings the create persisted, got \
             {:?}",
            read.body
        );
        assert_eq!(
            read.body["createdAt"], created.body["createdAt"],
            "case {case}: the creation instant does not move between reads, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["lastUpdatedAt"], created.body["createdAt"],
            "case {case}: nothing has changed the application since it was created, got {:?}",
            read.body
        );
        // The URL the application is reached on: this emulator's own endpoint with the
        // application named in the path, never a host of AWS's — asserted over the hosts a
        // caller can reach this emulator at by
        // [`a_published_application_endpoint_is_served_by_this_emulator`] below.
        assert_eq!(
            read.body["endpoint"],
            json!(format!(
                "https://{}{ENDPOINT_PATH_PREFIX}/{id}",
                unserved_authority()
            )),
            "case {case}: the read reports the URL the application is reached on, got {:?}",
            read.body
        );
        // The status is one of the two the clock can have produced by now: the
        // provisioning span is short, so this row pins only that the status is a
        // lifecycle value of a just-created application — the strict
        // `CREATING` -> `ACTIVE` move is proved on its own below.
        let status = string_member(&read.body, "status", case);
        assert!(
            status == APPLICATION_STATUS_CREATING || status == APPLICATION_STATUS_ACTIVE,
            "case {case}: a new application is being provisioned or already up, got {status:?}"
        );

        // A read changes nothing a later caller sees: the second read answers exactly
        // what the first one did, bar the status move the clock may have owed between
        // them.
        let again = get_application(&emulator, &id).await;
        for member in [
            "id",
            "arn",
            "name",
            "endpoint",
            "dataSources",
            "appConfigs",
            "kmsKeyArn",
            "createdAt",
            "lastUpdatedAt",
        ] {
            assert_eq!(
                again.body[member], read.body[member],
                "case {case}: two reads of one application report the same {member}, got {:?}",
                again.body
            );
        }

        minted.push((case, id, string_member(&read.body, "endpoint", case)));
    }

    // Every application of the account is a record of its own: no two of them share
    // the id they are addressed by, and none shares the endpoint derived from it.
    for (index, (case, id, endpoint)) in minted.iter().enumerate() {
        for (other_case, other_id, other_endpoint) in minted.iter().skip(index + 1) {
            assert_ne!(
                id, other_id,
                "case {case} vs {other_case}: two applications never share an id, got {id:?}"
            );
            assert_ne!(
                endpoint, other_endpoint,
                "case {case} vs {other_case}: two applications never share an endpoint, got \
                 {endpoint:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_application_settles_from_creating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();

    // What differs between the rows is only what each application was created with:
    // the settling is owed to each of them alike.
    for row in creates() {
        let created = create_application(&emulator, row.request).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200 from CreateApplication, got {} with {:?}",
            row.case, created.status, created.body
        );
        let id = string_member(&created.body, "id", row.case);

        // The transient status is observable: the read that follows the create happens
        // well inside the provisioning span, so what it reports is the status the
        // application was minted in — a handler that minted one already up fails here.
        let minted = get_application(&emulator, &id).await;
        assert_eq!(
            minted.body["status"],
            json!(APPLICATION_STATUS_CREATING),
            "case {}: a just-created application is being provisioned, got {:?}",
            row.case,
            minted.body
        );
        ids.push((row.case, id));
    }

    // Nothing below asks for the move: the span each application was minted with falls
    // due on its own, and the next read materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, id) in &ids {
        let read = get_application(&emulator, id).await;
        assert_eq!(
            read.body["status"],
            json!(APPLICATION_STATUS_ACTIVE),
            "case {case}: the status the clock owed is in place once the span passed, got {:?}",
            read.body
        );
        assert!(
            APPLICATION_STATUS_VALUES.contains(&read.body["status"].as_str().unwrap_or_default()),
            "case {case}: the status is one the model declares, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_name_the_account_already_uses_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();

    // One row per shape of name, so the rule is asserted about names rather than about
    // one name: the lower bound, the upper bound, a hyphenated name and a name with
    // digits.
    for name in ["ui1", "abcdefghij-klmnopqrst-uvwxyz12", "search-ui", "logs9"] {
        let first = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            first.status, 200,
            "case {name}: expected 200 from the first CreateApplication, got {} with {:?}",
            first.status, first.body
        );
        let id = string_member(&first.body, "id", name);

        let again = create_application(
            &emulator,
            json!({ "name": name, "appConfigs": [{ "key": APP_CONFIG_USERS, "value": "x" }] }),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {name}: a name the account already uses is refused, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ConflictException",
            "case {name}: the refusal is the modelled conflict, got {:?}",
            again.body
        );
        assert_eq!(
            again.body["message"],
            json!(format!("Application name {name} already exists")),
            "case {name}: the refusal names the application that already exists, got {:?}",
            again.body
        );

        // The refused call created nothing and changed nothing: the application the
        // first create minted is still the one the name resolves to, unchanged.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["name"],
            json!(name),
            "case {name}: the refused create left the first application in place, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["appConfigs"],
            json!([]),
            "case {name}: the refused create changed nothing about it, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_client_token_makes_a_create_idempotent() {
    let emulator = Emulator::new();

    // One row per kind of request a client might retry, so what is asserted is the
    // token rule rather than one request: the bare create, one carrying lists, and one
    // carrying tags and a key.
    let rows = vec![
        ("bare", json!({ "name": "token-bare" })),
        (
            "with lists",
            json!({
                "name": "token-lists",
                "dataSources": [{ "dataSourceArn": "arn:aws:s3:::token-lists" }],
                "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "retries" }],
            }),
        ),
        (
            "with tags and a key",
            json!({
                "name": "token-tagged",
                "tagList": [{ "Key": "retry", "Value": "yes" }],
                "kmsKeyArn": KMS_KEY,
            }),
        ),
    ];

    for (index, (case, request)) in rows.into_iter().enumerate() {
        let token = format!("client-token-{index}");
        let mut body = request;
        body["clientToken"] = json!(token);

        let first = create_application(&emulator, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected 200 from the first CreateApplication, got {} with {:?}",
            first.status, first.body
        );
        let retry = create_application(&emulator, body.clone()).await;
        assert_eq!(
            retry.status, 200,
            "case {case}: a retry under the same token is served, got {} with {:?}",
            retry.status, retry.body
        );
        assert_eq!(
            retry.body, first.body,
            "case {case}: the retry answers with the application the first call minted, got {:?}",
            retry.body
        );

        // The same token for a DIFFERENT name is one token identifying two creates,
        // which is the modelled conflict rather than a second application.
        let mut other = body.clone();
        other["name"] = json!(format!("other-{index}"));
        let conflict = create_application(&emulator, other).await;
        assert_eq!(
            conflict.status, 409,
            "case {case}: one token cannot identify two creates, got {} with {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case}: the refusal is the modelled conflict, got {:?}",
            conflict.body
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_application_is_refused() {
    let emulator = Emulator::new();

    // The account holds one application, so what each row below exercises is the id
    // rather than an empty store.
    let created = create_application(&emulator, json!({ "name": "held-ui" })).await;
    let held = string_member(&created.body, "id", "held-ui");

    // One row per shape of well-formed id the account holds nothing under: the
    // shortest the pattern allows, the longest, a digits-only one, and the held id
    // with one character changed.
    let mut absent = vec![
        "abc".to_string(),
        "a".repeat(30),
        "0123456789".to_string(),
        format!("{}z", &held[..held.len() - 1]),
    ];
    absent.retain(|id| *id != held);

    for id in absent {
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {id}: an id naming no application is refused, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {id}: the refusal is the modelled not-found, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["message"],
            json!(APPLICATION_DOES_NOT_EXIST),
            "case {id}: the refusal states that the account holds no such application, got {:?}",
            read.body
        );
    }

    // The application the account does hold is untouched by every refusal above.
    let read = get_application(&emulator, &held).await;
    assert_eq!(
        read.status, 200,
        "case held-ui: the held application still resolves, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn an_applications_tags_are_the_set_its_create_named() {
    let emulator = Emulator::new();

    // One row per tag set a create can carry, differing in how many tags they name and
    // in the characters their keys and values use.
    let rows = vec![
        ("no tags", "tagless-ui", json!([])),
        (
            "one tag",
            "one-tag-ui",
            json!([{ "Key": "team", "Value": "search" }]),
        ),
        (
            "three tags, one with an empty value",
            "three-tag-ui",
            json!([
                { "Key": "team", "Value": "search" },
                { "Key": "cost-center", "Value": "" },
                { "Key": "env:stage", "Value": "pre/prod" },
            ]),
        ),
    ];

    for (case, name, tag_list) in rows {
        let created =
            create_application(&emulator, json!({ "name": name, "tagList": tag_list })).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_member(&created.body, "arn", case);

        // The tagging paths resolve the application's ARN exactly as they resolve a
        // domain's, so the set the create named is the set `ListTags` reports.
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from ListTags, got {} with {:?}",
            listed.status, listed.body
        );
        let mut expected: Vec<(String, String)> = tag_pairs(&tag_list);
        assert_eq!(
            reported_tags(&listed, case),
            expected,
            "case {case}: the listing reports the tags the create named"
        );

        // And a later write reaches that same set: the tag the create named keeps its
        // place beside the one attached now.
        let added = add_tags(
            &emulator,
            &arn,
            json!([{ "Key": "added", "Value": "later" }]),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case {case}: expected 200 from AddTags, got {} with {:?}",
            added.status, added.body
        );
        expected.push(("added".to_string(), "later".to_string()));
        expected.sort();
        assert_eq!(
            reported_tags(&list_tags(&emulator, &arn).await, case),
            expected,
            "case {case}: the application carries the create's tags and the added one"
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_so_a_caller_looping_cannot_grow_it_without_end() {
    let emulator = Emulator::new();

    // Fill the account to its bound with names that differ only in their index: what
    // the bound counts is records, so the shape of each request is beside the point.
    for index in 0..APPLICATION_QUOTA {
        let created = create_application(&emulator, json!({ "name": format!("app-{index}") })).await;
        assert_eq!(
            created.status, 200,
            "case application {index}: a create below the bound is served, got {} with {:?}",
            created.status, created.body
        );
    }

    // One more, under a name nothing holds, is refused: the bound is on the SET and
    // not on the name.
    for name in ["one-too-many", "another-one"] {
        let refused = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            refused.status, 400,
            "case {name}: a create past the bound is refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name}: the refusal is the modelled client-input error, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"],
            json!(format!(
                "This account already holds the maximum of {APPLICATION_QUOTA} applications."
            )),
            "case {name}: the refusal names the bound, got {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_create_naming_more_tags_than_a_resource_may_carry_is_refused() {
    let emulator = Emulator::new();

    // The bound itself, one past it, and well past it: the first is served and the
    // other two are refused.
    for count in [TAG_QUOTA, TAG_QUOTA + 1, TAG_QUOTA * 2] {
        let name = format!("tag-bound-{count}");
        let tag_list: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
            .collect();
        let created =
            create_application(&emulator, json!({ "name": name, "tagList": tag_list })).await;

        if count <= TAG_QUOTA {
            assert_eq!(
                created.status, 200,
                "case {count} tags: the documented bound is served, got {} with {:?}",
                created.status, created.body
            );
            continue;
        }

        assert_eq!(
            created.status, 400,
            "case {count} tags: more tags than a resource may carry is refused, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            error_code(&created),
            "ValidationException",
            "case {count} tags: the refusal is the modelled client-input error, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["message"],
            json!(TAG_QUOTA_EXCEEDED),
            "case {count} tags: the refusal is the service's fixed sentence for the bound, got \
             {:?}",
            created.body
        );

        // Nothing was created: the name the refused call used is still free, which the
        // create that takes it proves.
        let retry = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            retry.status, 200,
            "case {count} tags: the refused create left the name free, got {} with {:?}",
            retry.status, retry.body
        );
    }
}

/// One caller the endpoint rule is asserted over: the host it addressed this emulator at,
/// and the name it created its application under.
struct EndpointRow {
    case: &'static str,
    /// The `Host` the create and the read carry — the endpoint this caller reached.
    host: &'static str,
    /// The authority the published URL has to carry for that caller, or `None` where the
    /// caller addressed a host of AWS's: there is no own endpoint in the request to report,
    /// so the address this process is served at stands instead.
    authority: Option<&'static str>,
    name: &'static str,
}

/// The table [`a_published_application_endpoint_is_served_by_this_emulator`] drives.
///
/// The rows differ along what the rule generalises over: the FORM of the endpoint the
/// caller reached this emulator at — a loopback IPv4 address, a loopback name, a bracketed
/// IPv6 literal — and one caller that resolves AWS's own hosts, which addresses this
/// service's published control-plane host instead.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            case: "a client on the emulator's own loopback address",
            host: "127.0.0.1:9297",
            authority: Some("127.0.0.1:9297"),
            name: "endpoint-ui-one",
        },
        EndpointRow {
            case: "a client on a loopback name",
            host: "localhost:9297",
            authority: Some("localhost:9297"),
            name: "endpoint-ui-two",
        },
        EndpointRow {
            case: "a client on the emulator's IPv6 loopback literal",
            host: "[::1]:9297",
            authority: Some("[::1]:9297"),
            name: "endpoint-ui-three",
        },
        EndpointRow {
            case: "a client that resolves AWS's own hosts",
            host: "es.us-east-1.amazonaws.com",
            authority: None,
            name: "endpoint-ui-four",
        },
    ]
}

/// One request to this service addressed at `host`, so a case can read a resource from the
/// endpoint a client of this emulator reaches it at as well as from the service's AWS host.
async fn call_at(emulator: &Emulator, host: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/es/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// The authority and the request path of `url`, split the way a client that FOLLOWS the URL
/// splits it: whatever stands before the first `/` of the address is the host it dials and
/// signs, and the rest is the path it sends.
fn dialled(url: &str) -> (String, String) {
    let address = url
        .strip_prefix("https://")
        .expect("a published endpoint is an absolute https URL");
    match address.find('/') {
        Some(end) => (address[..end].to_string(), address[end..].to_string()),
        None => (address.to_string(), "/".to_string()),
    }
}

/// A fetch of `url` exactly as a client follows it: a `GET` of the path the URL carries, at
/// the host it names, PRESENTING an AWS credential — the credential this plane admits a fetch
/// on, since an application is reached through a sign-in and is read by no anonymous caller
/// ([`super::application_endpoint_sign_in`] states that rule and the refusal an
/// uncredentialed fetch earns).
async fn followed(emulator: &Emulator, url: &str) -> Answer {
    let (authority, path) = dialled(url);
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", authority)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/es/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::new())
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// The `content-type` an answer carries, which is what a client reads its body as.
fn media_type(answer: &Answer) -> String {
    answer
        .headers
        .get("content-type")
        .cloned()
        .unwrap_or_default()
}

/// RULE: a URL an operation reports for a caller to FOLLOW is built over the endpoint this
/// emulator is reached at — the one the request arrived on, else the port this process is
/// served at — with the resource's own id in the path, and this emulator SERVES it out of
/// the stored resource. It is never a host of AWS's, which resolves nowhere here.
///
/// Asserted over the forms a caller can address this emulator by, and by USING the address:
/// what it answers states the stored application, and the same address for an id this account
/// holds no application under answers that there is no such application.
#[tokio::test]
async fn a_published_application_endpoint_is_served_by_this_emulator() {
    let emulator = Emulator::new();
    let unserved = unserved_authority();

    for row in endpoint_rows() {
        let case = row.case;
        let created = call_at(
            &emulator,
            row.host,
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(json!({
                "name": row.name,
                "dataSources": [{
                    "dataSourceArn": format!("arn:aws:es:{REGION}:123456789012:domain/logs"),
                    "dataSourceDescription": "the logs domain",
                }],
            })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        let read = call_at(
            &emulator,
            row.host,
            "GET",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        let published = string_member(&read.body, "endpoint", case);

        // The address is this emulator's own, with the application named in the path.
        let authority = row.authority.unwrap_or(&unserved);
        assert_eq!(
            published,
            format!("https://{authority}{ENDPOINT_PATH_PREFIX}/{id}"),
            "case {case}: expected the endpoint on the address this emulator answers at, got \
             {published:?}"
        );
        // And never a host of AWS's: a client handed one leaves the emulator altogether.
        assert!(
            !published.contains("amazonaws.com"),
            "case {case}: expected no AWS host in the endpoint, got {published:?}"
        );

        // Following it reaches THIS application: what it answers states the record the
        // control plane reports, out of the one store both read.
        let followed = followed(&emulator, &published).await;
        assert_eq!(
            followed.status, 200,
            "case {case}: expected 200 from following {published:?}, got {} with {:?}",
            followed.status, followed.body
        );
        assert_eq!(
            media_type(&followed),
            "application/json",
            "case {case}: expected the answer to be served as JSON, got {:?}",
            media_type(&followed)
        );
        for member in ["id", "name", "status"] {
            assert_eq!(
                followed.body[member], read.body[member],
                "case {case}: expected {member:?} at {published:?} to be the one the control \
                 plane reports, got {:?}",
                followed.body
            );
        }
        assert_eq!(
            followed.body["dataSources"], read.body["dataSources"],
            "case {case}: expected the data sources at {published:?} to be the ones the control \
             plane reports, got {:?}",
            followed.body
        );

        // The address is the APPLICATION's, so it is minted once and every read reports the
        // same one — a caller that stored what one reader told it keeps an address that works.
        let again = call_at(
            &emulator,
            "es.us-east-1.amazonaws.com",
            "GET",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await;
        assert_eq!(
            string_member(&again.body, "endpoint", case),
            published,
            "case {case}: two reads of one application report the same endpoint, got {:?}",
            again.body
        );
    }

    // An address of that shape naming an application this account does not hold answers that
    // there is none, rather than an answer about an application nothing created: ids differing
    // in length and alphabet, and one whose application was taken away.
    let created = create_application(&emulator, json!({ "name": "endpoint-ui-gone" })).await;
    let gone = string_member(&created.body, "id", "an application taken away");
    let deleted = delete_application(&emulator, &gone).await;
    assert_eq!(
        deleted.status, 200,
        "case an application taken away: expected 200 from DeleteApplication, got {} with {:?}",
        deleted.status, deleted.body
    );
    // The removal is what the clock owes that application, so the case waits for the
    // CONDITION — the account no longer holding it — rather than for a stretch of time.
    let gone_url = format!("https://{unserved}{ENDPOINT_PATH_PREFIX}/{gone}");
    poll_until("an application taken away", || async {
        followed(&emulator, &gone_url).await.status == 404
    })
    .await;
    for (case, id) in [
        ("an id nothing was ever created under", "abc"),
        ("a 20-character id of the width this service mints", "ab3xk9qp17vn42zd0c8m"),
        ("an application taken away", gone.as_str()),
    ] {
        let url = format!("https://{unserved}{ENDPOINT_PATH_PREFIX}/{id}");
        let answered = followed(&emulator, &url).await;
        assert_eq!(
            answered.status, 404,
            "case {case}: expected 404 from following {url:?}, got {} with {:?}",
            answered.status, answered.body
        );
        // The same sentence the control plane states about an id naming no application of
        // this account, so both halves of the service say one thing about it.
        assert_eq!(
            answered.body["message"],
            json!(APPLICATION_DOES_NOT_EXIST),
            "case {case}: expected the answer to state that there is no such application, got \
             {:?}",
            answered.body
        );
    }
}
