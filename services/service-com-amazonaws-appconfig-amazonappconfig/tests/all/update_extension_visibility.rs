// SPDX-License-Identifier: CC0-1.0

//! `UpdateExtension` as the WRITER the rest of AppConfig's extension slice answers from:
//! what an update stores is what every reader of the extension reports, and an update
//! touches the addressed version's definition only — never the set of versions the
//! extension holds.
//!
//! Every case drives a real HTTP request through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The rules under
//! test:
//!
//! * the definition `UpdateExtension` stored is what EVERY reader of that version reports
//!   — `GetExtension` by the extension's id, by its name and by the ARN of the version
//!   (which pins it), and `ListExtensions`, whose summary is built from the extension's
//!   latest version — so no reader disagrees with the update's own answer;
//! * an update changes a version's definition and nothing about the version set: no
//!   version is minted or removed, so the name's latest version number is what
//!   `CreateExtension`'s `LatestVersionNumber` lock still demands, the version that lock
//!   appends carries its OWN request's definition rather than the edit, and
//!   `DeleteExtension` of an edited version removes exactly that version, leaving the
//!   others readable and editable.
//!
//! The tables span what those rules generalise over rather than one extension: how many
//! versions the extension holds (one, two, three), which of them the edit addresses (the
//! only one, the lowest, the middle, the latest), which identifier form addresses it (id,
//! name, version ARN — the last pinning the version by itself), and which members the edit
//! carries (description, actions, parameters, all three).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The action-point catalog is the model's own `ActionPoint` enum, re-exported by the
// service crate (`smithy_query(query="shape",shape="ActionPoint")`), so a version's
// definition below cannot drift from the model by being hand-copied.
use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;

use crate::appconfig_workflow::router;
use crate::extensions::{actions, create_extension, extension_facts, get_extension, minted_id};
use crate::list_extensions::{items, list};
use crate::resource_deletes::delete_extension;
use crate::update_extension::update_extension;

/// The `ExtensionSummary` members `ListExtensions` reports for one extension, as one
/// comparable value. The summary carries no `Actions` or `Parameters`
/// (`smithy_query(query="shape",shape="ExtensionSummary")`), so a listing is compared
/// against a reader on exactly the members it does report.
fn summary_facts(body: &Value) -> BTreeMap<&'static str, Value> {
    ["Id", "Name", "VersionNumber", "Arn", "Description"]
        .into_iter()
        .map(|member| (member, body.get(member).cloned().unwrap_or(Value::Null)))
        .collect()
}

/// A `ParameterMap` holding one parameter. Never both flags at once: "Dynamic parameters
/// can't be marked `Required`" (`smithy_query(query="doc",shape="Parameter",member="Dynamic")`).
fn parameter(name: &str, required: bool, dynamic: bool) -> Value {
    let mut map = Map::new();
    map.insert(
        name.to_string(),
        json!({
            "Description": format!("parameter {name}"),
            "Required": required,
            "Dynamic": dynamic,
        }),
    );
    Value::Object(map)
}

/// Create an extension named `name` carrying `versions` versions, each written by its own
/// `CreateExtension` call and each with a definition of its own, and answer the id the
/// first call minted. `LatestVersionNumber` is the lock on the name's current latest
/// version, so version `n` is appended by naming `n - 1` and the first version by naming
/// nothing.
async fn versioned_extension(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    versions: i32,
) -> String {
    let mut id = String::new();
    for version in 1..=versions {
        let point = ACTION_POINT_VALUES[(version as usize - 1) % ACTION_POINT_VALUES.len()];
        let body = json!({
            "Name": name,
            "Description": format!("version {version}"),
            "Actions": actions(&[point]),
            "Parameters": parameter(&format!("P{version}"), version % 2 == 0, false),
        });
        let created = create_extension(router, &body, (version > 1).then_some(version - 1)).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExtension of version {version} expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let answered = created.json();
        assert_eq!(
            answered.get("VersionNumber").and_then(Value::as_i64),
            Some(i64::from(version)),
            "case {case:?}: CreateExtension minted version {:?} rather than {version}",
            answered.get("VersionNumber")
        );
        id = minted_id(case, &answered);
    }
    id
}

/// What each version of `id` reads as, keyed by version number.
async fn stored_versions(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    versions: i32,
) -> BTreeMap<i32, BTreeMap<&'static str, Value>> {
    let mut stored = BTreeMap::new();
    for version in 1..=versions {
        let read = get_extension(router, id, Some(version)).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtension at version {version} expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        stored.insert(version, extension_facts(&read.json()));
    }
    stored
}

/// How a row addresses the extension version it edits.
enum Address {
    /// The extension's minted id, with the version named in the request body.
    Id,
    /// The extension's name, with the version named in the request body.
    Name,
    /// The ARN of the version itself, which pins the version with no `VersionNumber`
    /// member at all.
    VersionArn,
}

/// One row of the reader-agreement table.
struct Row {
    case: &'static str,
    /// How many versions the extension holds when the edit lands.
    versions: i32,
    /// The version the edit addresses.
    target: i32,
    address: Address,
    description: Option<&'static str>,
    actions: Option<Value>,
    parameters: Option<Value>,
}

/// RULE: the definition an `UpdateExtension` call stores is what every reader of that
/// extension version reports — `GetExtension` by the extension's id, by its name and by
/// the version's own ARN, and the `ListExtensions` summary, which is built from the
/// extension's latest version — and the update mints no version, so a version number past
/// the highest one written is still missing afterwards.
#[tokio::test]
async fn every_reader_reports_the_definition_an_update_stored() {
    let table = vec![
        Row {
            case: "one version, the only one edited by id",
            versions: 1,
            target: 1,
            address: Address::Id,
            description: Some("edited by id"),
            actions: None,
            parameters: None,
        },
        Row {
            case: "two versions, the lowest edited by name with new actions",
            versions: 2,
            target: 1,
            address: Address::Name,
            description: None,
            actions: Some(actions(&["PRE_START_DEPLOYMENT", "ON_DEPLOYMENT_STEP"])),
            parameters: None,
        },
        Row {
            case: "three versions, the middle one edited by its ARN with new parameters",
            versions: 3,
            target: 2,
            address: Address::VersionArn,
            description: None,
            actions: None,
            parameters: Some(parameter("edited", false, true)),
        },
        Row {
            case: "three versions, the latest edited by id with every member at once",
            versions: 3,
            target: 3,
            address: Address::Id,
            description: Some("edited latest"),
            actions: Some(actions(&["AT_DEPLOYMENT_TICK"])),
            parameters: Some(parameter("edited", true, false)),
        },
        Row {
            case: "two versions, the latest edited by name with an emptied description",
            versions: 2,
            target: 2,
            address: Address::Name,
            description: Some(""),
            actions: None,
            parameters: None,
        },
    ];

    for row in &table {
        let case = row.case;
        let router = router();
        let name = "reader-agreement";
        let id = versioned_extension(&router, case, name, row.versions).await;
        let before = stored_versions(&router, case, &id, row.versions).await;
        let target_arn = before
            .get(&row.target)
            .and_then(|facts| facts.get("Arn"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the target version carries no ARN"))
            .to_string();

        let mut body = Map::new();
        if let Some(description) = row.description {
            body.insert("Description".to_string(), Value::from(description));
        }
        if let Some(actions) = &row.actions {
            body.insert("Actions".to_string(), actions.clone());
        }
        if let Some(parameters) = &row.parameters {
            body.insert("Parameters".to_string(), parameters.clone());
        }
        let identifier = match row.address {
            Address::Id => {
                body.insert("VersionNumber".to_string(), Value::from(row.target));
                id.clone()
            }
            Address::Name => {
                body.insert("VersionNumber".to_string(), Value::from(row.target));
                name.to_string()
            }
            Address::VersionArn => target_arn.clone(),
        };

        let answer = update_extension(&router, &identifier, &Value::Object(body)).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: UpdateExtension expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // The edited version: the members the request carried, over the ones it left as
        // they were.
        let mut expected = before
            .get(&row.target)
            .expect("the target version was read before the edit")
            .clone();
        if let Some(description) = row.description {
            expected.insert("Description", Value::from(description));
        }
        if let Some(actions) = &row.actions {
            expected.insert("Actions", actions.clone());
        }
        if let Some(parameters) = &row.parameters {
            expected.insert("Parameters", parameters.clone());
        }
        assert_eq!(
            extension_facts(&answer.json()),
            expected,
            "case {case:?}: UpdateExtension answered something other than the edited version"
        );

        // Every reader of that version, by each identifier form the model's `Identifier`
        // accepts, reports exactly it.
        let readers = [
            (
                "GetExtension by id at the edited version",
                get_extension(&router, &id, Some(row.target)).await,
            ),
            (
                "GetExtension by name at the edited version",
                get_extension(&router, name, Some(row.target)).await,
            ),
            (
                "GetExtension by the version's own ARN",
                get_extension(&router, &target_arn, None).await,
            ),
        ];
        for (reader, read) in &readers {
            assert_eq!(
                read.status,
                200,
                "case {case:?}: {reader} expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                extension_facts(&read.json()),
                expected,
                "case {case:?}: {reader} does not report the edit"
            );
        }

        // A reader naming no version gets the extension's HIGHEST version, which is the
        // edited one only when the edit addressed it.
        let latest = get_extension(&router, &id, None).await;
        assert_eq!(
            latest.status,
            200,
            "case {case:?}: GetExtension naming no version expected 200, got {} / {}",
            latest.status,
            String::from_utf8_lossy(&latest.body)
        );
        let wanted_latest = if row.target == row.versions {
            expected.clone()
        } else {
            before
                .get(&row.versions)
                .expect("the highest version was read before the edit")
                .clone()
        };
        assert_eq!(
            extension_facts(&latest.json()),
            wanted_latest,
            "case {case:?}: the highest version does not read as the edit left it"
        );

        // `ListExtensions` summarises each extension by that same latest version, both
        // narrowed to this name and over the whole catalogue.
        let wanted_summary = summary_facts(&latest.json());
        for (listing, query) in [
            ("narrowed by Name", format!("name={name}")),
            ("over the whole catalogue", String::new()),
        ] {
            let answer = list(&router, &query).await;
            let listed: Vec<Value> = items(case, &answer)
                .into_iter()
                .filter(|item| item.get("Id").and_then(Value::as_str) == Some(id.as_str()))
                .collect();
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: the listing {listing} reports the edited extension {} time(s)",
                listed.len()
            );
            assert_eq!(
                summary_facts(&listed[0]),
                wanted_summary,
                "case {case:?}: the listing {listing} disagrees with the reader about the edited extension"
            );
        }

        // No version was minted: the number past the highest one written is still missing.
        let forked = get_extension(&router, &id, Some(row.versions + 1)).await;
        assert_eq!(
            forked.status,
            404,
            "case {case:?}: an update may mint no version, got {} / {}",
            forked.status,
            String::from_utf8_lossy(&forked.body)
        );
    }
}

/// RULE: an update changes a version's definition and nothing about the version set, so
/// after an edit `CreateExtension`'s `LatestVersionNumber` lock still demands the name's
/// unchanged latest version number (any other number is refused HTTP 409
/// `ConflictException` and appends nothing), the version it appends carries its own
/// request's definition rather than the edit, and `DeleteExtension` of the edited version
/// removes exactly that version while the others stay readable and editable.
///
/// The rows span which version the edit addressed — the lowest or the latest — because the
/// lock and the delete both key on the version set the edit must have left alone.
#[tokio::test]
async fn an_edit_leaves_the_version_set_the_lock_and_the_delete_alone() {
    for target in [1, 2] {
        let case = format!("two versions, version {target} edited");
        let case = case.as_str();
        let router = router();
        let name = "edited-then-appended";
        let id = versioned_extension(&router, case, name, 2).await;

        let edited = update_extension(
            &router,
            &id,
            &json!({ "VersionNumber": target, "Description": "edited" }),
        )
        .await;
        assert_eq!(
            edited.status,
            200,
            "case {case:?}: UpdateExtension expected 200, got {} / {}",
            edited.status,
            String::from_utf8_lossy(&edited.body)
        );

        // The lock is still on version 2: the edit neither advanced nor reset it. An
        // omitted lock on a name that exists is the missing-lock bad request; a lock behind
        // the latest version is the stale-lock conflict.
        for (lock, locked, status, code) in [
            ("omitted", None, 400, "BadRequestException"),
            (
                "a number behind the latest",
                Some(1),
                409,
                "ConflictException",
            ),
        ] {
            let refused = create_extension(
                &router,
                &json!({
                    "Name": name,
                    "Actions": actions(&[ACTION_POINT_VALUES[0]]),
                }),
                locked,
            )
            .await;
            assert_eq!(
                refused.status, status,
                "case {case:?}: CreateExtension with {lock} expected HTTP {status}, got {} / {}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: CreateExtension with {lock} expected a {code}, got {}",
                String::from_utf8_lossy(&refused.body)
            );
            let appended = get_extension(&router, &id, Some(3)).await;
            assert_eq!(
                appended.status,
                404,
                "case {case:?}: a refused CreateExtension with {lock} appended a version: {} / {}",
                appended.status,
                String::from_utf8_lossy(&appended.body)
            );
        }

        // Naming that unchanged latest version appends the next one, built from its own
        // request: the edit is not carried into it.
        let appended = create_extension(
            &router,
            &json!({
                "Name": name,
                "Description": "version three",
                "Actions": actions(&[ACTION_POINT_VALUES[0]]),
            }),
            Some(2),
        )
        .await;
        assert_eq!(
            appended.status,
            201,
            "case {case:?}: CreateExtension naming the latest version expected 201, got {} / {}",
            appended.status,
            String::from_utf8_lossy(&appended.body)
        );
        assert_eq!(
            appended.json().get("VersionNumber").and_then(Value::as_i64),
            Some(3),
            "case {case:?}: the appended version is numbered {:?} rather than 3",
            appended.json().get("VersionNumber")
        );
        assert_eq!(
            appended.json().get("Description").and_then(Value::as_str),
            Some("version three"),
            "case {case:?}: the appended version carries {:?} rather than its own description",
            appended.json().get("Description")
        );
        let read_target = get_extension(&router, &id, Some(target)).await;
        assert_eq!(
            read_target.json().get("Description").and_then(Value::as_str),
            Some("edited"),
            "case {case:?}: version {target} lost the edit when a version was appended: {}",
            read_target.json()
        );

        // The delete takes exactly the version it names, edited or not.
        let deleted = delete_extension(&router, &id, Some(target)).await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: DeleteExtension of version {target} expected 204, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        let gone = get_extension(&router, &id, Some(target)).await;
        assert_eq!(
            gone.status,
            404,
            "case {case:?}: version {target} is still readable after its delete, got {} / {}",
            gone.status,
            String::from_utf8_lossy(&gone.body)
        );
        for version in [1, 2, 3].into_iter().filter(|version| *version != target) {
            let read = get_extension(&router, &id, Some(version)).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: version {version} must survive the delete of version {target}, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
        }

        // What survives is still editable, and an independent read reflects the edit.
        let after = update_extension(
            &router,
            &id,
            &json!({ "VersionNumber": 3, "Description": "after the delete" }),
        )
        .await;
        assert_eq!(
            after.status,
            200,
            "case {case:?}: an update after the delete expected 200, got {} / {}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
        let read = get_extension(&router, &id, None).await;
        assert_eq!(
            summary_facts(&read.json()).get("Description"),
            Some(&Value::from("after the delete")),
            "case {case:?}: the update after the delete is not visible to a reader: {}",
            read.json()
        );
    }
}
