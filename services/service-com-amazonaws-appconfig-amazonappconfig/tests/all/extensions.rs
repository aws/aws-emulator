// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppConfig's extension slice: `CreateExtension` and the
//! `GetExtension` reader that proves what it persisted.
//!
//! Every case drives a real HTTP request through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The rules
//! under test:
//!
//! * `CreateExtension` stores a NAMED, VERSIONED extension built from the request — the
//!   action list bound to each action point, the parameters, the description and the tags
//!   — mints the extension's id and the ARN of that version, and answers the stored
//!   record, which `GetExtension` then reports unchanged by the extension's id, by its
//!   name or by that ARN;
//! * `LatestVersionNumber` is an OPTIMISTIC LOCK on the name's current latest version
//!   (zero for a name no extension carries): a request naming that number appends version
//!   `number + 1`, and any other number — including omitting it for a name that already
//!   exists — is refused with HTTP 409 `ConflictException` and stores nothing;
//! * each version keeps the definition the call that wrote it gave, and a reader naming
//!   no version gets the highest one;
//! * a parameter marked both `Dynamic` and `Required` is refused with HTTP 400 and
//!   stores nothing, because "Dynamic parameters can't be marked `Required`".
//!
//! The tables span the class each rule covers rather than one extension: every action
//! point the model's `ActionPoint` enum declares, one action point and the five the
//! `ActionsMap` bound allows, no parameters / one / the ten `ParameterMap` allows, each
//! combination of a parameter's two flags, a description that is absent, empty and the
//! documented 1024-character maximum, and names spanning the `ExtensionOrParameterName`
//! pattern.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The action-point catalog under test is the model's own `ActionPoint` enum, re-exported
// by the service crate as `ACTION_POINT_VALUES`
// (`smithy_query(query="shape",shape="ActionPoint")`), so the table below cannot drift
// from the model by being hand-copied.
use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;

use crate::appconfig_workflow::{json_call, router, send, Answer};
use crate::resource_tags::{arn, filler, list_tags, pct};

/// The documented maximum length of `Description` (`Description, len 0..1024`).
const MAX_DESCRIPTION: usize = 1024;

/// The most action points one extension binds (`ActionsMap, len 1..5`).
const MAX_ACTION_POINTS: usize = 5;

/// The most parameters one extension declares (`ParameterMap, len 1..10`).
const MAX_PARAMETERS: usize = 10;

/// One `Action`: the model gives it a name, a URI and an assume-role ARN, and exactly
/// one action is bound per point (`ActionList, len 1..1`).
fn action(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": format!("the {name} action"),
        "Uri": arn(&format!("function:{name}")),
        "RoleArn": engine::emulator_arn("iam", "", &format!("role/{name}-invoker")),
    })
}

/// An `ActionsMap` binding one action to each point in `points`. `pub(crate)` because
/// every extension a suite creates needs at least one action bound.
pub(crate) fn actions(points: &[&str]) -> Value {
    Value::Object(
        points
            .iter()
            .map(|point| {
                (
                    (*point).to_string(),
                    Value::Array(vec![action(&point.to_lowercase())]),
                )
            })
            .collect::<Map<String, Value>>(),
    )
}

/// A `ParameterMap` of `count` parameters, the `index`-th carrying the flags `flags`
/// gives for it.
fn parameters(count: usize, flags: impl Fn(usize) -> (bool, bool)) -> Value {
    Value::Object(
        (0..count)
            .map(|index| {
                let (required, dynamic) = flags(index);
                (
                    format!("parameter-{index}"),
                    json!({
                        "Description": format!("parameter {index}"),
                        "Required": required,
                        "Dynamic": dynamic,
                    }),
                )
            })
            .collect::<Map<String, Value>>(),
    )
}

/// `POST /extensions`. `LatestVersionNumber` is bound to `@httpHeader`, so when the case
/// supplies one it travels as the `Latest-Version-Number` header rather than in the body.
/// `pub(crate)` because the `ListExtensions` suite creates its fixtures through the same
/// front door.
pub(crate) async fn create_extension(
    router: &MultiServiceRouter,
    body: &Value,
    latest_version_number: Option<i32>,
) -> Answer {
    let headers: Vec<(&str, String)> = latest_version_number
        .map(|number| vec![("Latest-Version-Number", number.to_string())])
        .unwrap_or_default();
    send(
        router,
        "POST",
        "/extensions",
        "application/json",
        serde_json::to_vec(body).expect("the test request body must serialize"),
        &headers,
    )
    .await
}

/// `GET /extensions/{ExtensionIdentifier}[?version_number=..]`. The identifier is bound
/// to a NON-greedy `@httpLabel`, so an ARN travels percent-encoded. `pub(crate)` because
/// the `ListExtensions` suite reads its fixtures back through the same reader.
pub(crate) async fn get_extension(
    router: &MultiServiceRouter,
    identifier: &str,
    version_number: Option<i32>,
) -> Answer {
    let uri = match version_number {
        Some(number) => format!("/extensions/{}?version_number={number}", pct(identifier)),
        None => format!("/extensions/{}", pct(identifier)),
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Extension` members a reader must report, as one comparable value: absent and
/// present are both represented, so two answers are compared whole rather than member by
/// member. `pub(crate)` because every suite that reads an extension back compares the
/// same members.
pub(crate) fn extension_facts(body: &Value) -> BTreeMap<&'static str, Value> {
    [
        "Id",
        "Name",
        "VersionNumber",
        "Arn",
        "Description",
        "Actions",
        "Parameters",
    ]
    .into_iter()
    .map(|member| (member, body.get(member).cloned().unwrap_or(Value::Null)))
    .collect()
}

/// The `Id` of a 201 answer, asserted to be present and to match the model's `Id` shape
/// (`^[a-z0-9]{4,7}$`). `pub(crate)` because every suite that creates an extension needs
/// the same check.
pub(crate) fn minted_id(case: &str, body: &Value) -> String {
    let id = body
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateExtension answered no Id: {body}"))
        .to_string();
    assert!(
        (4..=7).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() && c.is_alphanumeric() || c.is_ascii_digit()),
        "case {case:?}: expected a minted id matching ^[a-z0-9]{{4,7}}$, got {id:?}"
    );
    id
}

/// The latest version number `GetExtension` reports for `identifier`, or `None` when it
/// reports the extension missing.
async fn latest_version(router: &MultiServiceRouter, identifier: &str) -> Option<i64> {
    let answer = get_extension(router, identifier, None).await;
    if answer.status == 404 {
        return None;
    }
    assert_eq!(
        answer.status,
        200,
        "GetExtension({identifier:?}) expected 200 or 404, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json().get("VersionNumber").and_then(Value::as_i64)
}

/// One row of the create table: a definition `CreateExtension` must store and answer.
struct Created {
    case: &'static str,
    name: String,
    description: Option<String>,
    actions: Value,
    parameters: Option<Value>,
    tags: Option<Value>,
}

/// The request body for a row.
fn body_of(row: &Created) -> Value {
    let mut body = Map::new();
    body.insert("Name".to_string(), Value::from(row.name.clone()));
    body.insert("Actions".to_string(), row.actions.clone());
    if let Some(description) = &row.description {
        body.insert("Description".to_string(), Value::from(description.clone()));
    }
    if let Some(parameters) = &row.parameters {
        body.insert("Parameters".to_string(), parameters.clone());
    }
    if let Some(tags) = &row.tags {
        body.insert("Tags".to_string(), tags.clone());
    }
    Value::Object(body)
}

/// `CreateExtension` answers the record it stored, and `GetExtension` reports that same
/// record back by id, by name and by the ARN the create minted.
///
/// The table spans what the rule generalises over: how many action points are bound and
/// which, how many parameters are declared and with which flags, whether a description
/// is absent / empty / at its documented maximum, and the shape of the name.
#[tokio::test]
async fn create_extension_persists_the_definition_and_get_reports_it() {
    let table = vec![
        Created {
            case: "one action point, nothing else",
            name: "minimal".to_string(),
            description: None,
            actions: actions(&["PRE_START_DEPLOYMENT"]),
            parameters: None,
            tags: None,
        },
        Created {
            case: "the five action points the ActionsMap bound allows",
            name: "five-points".to_string(),
            description: Some("binds every point the map holds".to_string()),
            actions: actions(&ACTION_POINT_VALUES[..MAX_ACTION_POINTS]),
            parameters: None,
            tags: None,
        },
        Created {
            case: "one parameter, neither required nor dynamic",
            name: "one-parameter".to_string(),
            description: Some(String::new()),
            actions: actions(&["ON_DEPLOYMENT_START"]),
            parameters: Some(parameters(1, |_| (false, false))),
            tags: None,
        },
        Created {
            case: "the ten parameters the ParameterMap bound allows, flags alternating",
            name: "ten-parameters".to_string(),
            description: Some(filler(MAX_DESCRIPTION, 3)),
            actions: actions(&["ON_DEPLOYMENT_COMPLETE", "ON_DEPLOYMENT_BAKING"]),
            parameters: Some(parameters(MAX_PARAMETERS, |index| {
                (index % 2 == 0, index % 3 == 0 && index % 2 != 0)
            })),
            tags: Some(json!({ "team": "platform", "cost-centre": "" })),
        },
        Created {
            case: "a required parameter and a dynamic one",
            name: "flagged-parameters".to_string(),
            description: Some("one required, one dynamic".to_string()),
            actions: actions(&["AT_DEPLOYMENT_TICK"]),
            parameters: Some(parameters(2, |index| (index == 0, index == 1))),
            tags: None,
        },
        Created {
            // `ExtensionOrParameterName` is `^[^\/#:\n]{1,64}$` — anything but `/`, `#`,
            // `:` and a newline, so a name may carry spaces and non-ASCII.
            case: "a name of spaces and non-ASCII characters",
            name: "naïve extension (ünïcode)".to_string(),
            description: None,
            actions: actions(&["ON_DEPLOYMENT_ROLLED_BACK"]),
            parameters: Some(parameters(1, |_| (true, false))),
            tags: None,
        },
        Created {
            case: "a name at the 64-character maximum the pattern allows",
            name: filler(64, 11),
            description: None,
            actions: actions(&["ON_DEPLOYMENT_STEP"]),
            parameters: None,
            tags: Some(json!({ "only": "tag" })),
        },
    ];

    let router = router();
    for row in &table {
        let case = row.case;
        let created = create_extension(&router, &body_of(row), None).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExtension expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_body = created.json();
        let id = minted_id(case, &created_body);

        // The create answers the stored record: the version is the first one, the ARN
        // names this emulator's account and that version, and every member it was given
        // comes back. `Parameters` is absent rather than an empty map when none was
        // given, because `ParameterMap` is bounded `len 1..10`.
        let expected_arn = arn(&format!("extension/{id}/1"));
        assert_eq!(
            created_body.get("Name").and_then(Value::as_str),
            Some(row.name.as_str()),
            "case {case:?}: expected Name {:?}, got {created_body}",
            row.name
        );
        assert_eq!(
            created_body.get("VersionNumber").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: expected VersionNumber 1, got {created_body}"
        );
        assert_eq!(
            created_body.get("Arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected Arn {expected_arn:?}, got {created_body}"
        );
        assert_eq!(
            created_body.get("Description").cloned(),
            row.description.clone().map(Value::from),
            "case {case:?}: expected Description {:?}, got {created_body}",
            row.description
        );
        assert_eq!(
            created_body.get("Actions").cloned(),
            Some(row.actions.clone()),
            "case {case:?}: expected Actions {}, got {created_body}",
            row.actions
        );
        assert_eq!(
            created_body.get("Parameters").cloned(),
            row.parameters.clone(),
            "case {case:?}: expected Parameters {:?}, got {created_body}",
            row.parameters
        );

        // ... and `GetExtension` reports that same record, by each of the three forms
        // `ExtensionIdentifier` accepts.
        for identifier in [id.as_str(), row.name.as_str(), expected_arn.as_str()] {
            let read = get_extension(&router, identifier, None).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: GetExtension({identifier:?}) expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                extension_facts(&read.json()),
                extension_facts(&created_body),
                "case {case:?}: GetExtension({identifier:?}) reported something other than \
                 what CreateExtension stored"
            );
        }

        // Naming the version explicitly reports the same version.
        let pinned = get_extension(&router, &id, Some(1)).await;
        assert_eq!(
            pinned.status,
            200,
            "case {case:?}: GetExtension at version 1 expected 200, got {} / {}",
            pinned.status,
            String::from_utf8_lossy(&pinned.body)
        );
        assert_eq!(
            extension_facts(&pinned.json()),
            extension_facts(&created_body),
            "case {case:?}: GetExtension at version 1 reported something else"
        );

        // The tags the create was asked to assign are the ones the tagging reader
        // reports for the ARN it answered — an extension that was given none carries
        // none.
        let expected_tags: BTreeMap<String, String> = row
            .tags
            .as_ref()
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let listed = list_tags(&router, &expected_arn).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: ListTagsForResource({expected_arn:?}) expected 200, got {} / {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let reported: BTreeMap<String, String> = listed
            .json()
            .get("Tags")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
            .collect();
        assert_eq!(
            reported, expected_tags,
            "case {case:?}: expected the tags the create assigned, got {reported:?}"
        );
    }
}

/// Every action point the model's `ActionPoint` enum declares is storable and is reported
/// back unchanged — including the ones no issue cited.
#[tokio::test]
async fn every_modelled_action_point_round_trips() {
    let router = router();
    for point in ACTION_POINT_VALUES {
        let case = *point;
        let body = json!({ "Name": format!("at-{point}"), "Actions": actions(&[point]) });
        let created = create_extension(&router, &body, None).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExtension expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let id = minted_id(case, &created.json());

        let read = get_extension(&router, &id, None).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtension expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json().get("Actions").cloned(),
            Some(actions(&[point])),
            "case {case:?}: expected the action bound to {point}, got {}",
            read.json()
        );
    }
}

/// An action bound to something the `ActionPoint` enum does not declare is refused with
/// HTTP 400 and stores nothing.
#[tokio::test]
async fn an_unmodelled_action_point_is_refused() {
    let router = router();
    for point in ["", "PRE_START", "pre_start_deployment", "ON_DEPLOYMENT_DONE"] {
        let body = json!({
            "Name": "unmodelled-point",
            "Actions": { point: [action("handler")] },
        });
        let answer = create_extension(&router, &body, None).await;
        assert_eq!(
            answer.status,
            400,
            "case {point:?}: expected 400 for an action point the model does not declare, \
             got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            latest_version(&router, "unmodelled-point").await,
            None,
            "case {point:?}: the refused create stored an extension anyway"
        );
    }
}

/// One row of the optimistic-lock table.
struct Lock {
    case: &'static str,
    /// How many versions the name already carries when the request arrives.
    existing_versions: i32,
    /// The `LatestVersionNumber` the request names, or `None` when it names none.
    claimed: Option<i32>,
    /// The status the request must answer, and the version the name is left at.
    expected_status: u16,
    expected_latest: i64,
}

/// `LatestVersionNumber` is an optimistic lock on the name's current latest version:
/// "When you create a new version, specify the most recent current version number."
///
/// The table spans the class: a name nobody has created (current version zero) and names
/// already at one, two and three versions, each against a claim that is omitted, correct,
/// stale, ahead, and negative.
///
/// The two ways the lock can fail are two different refusals: a lock the request did not
/// SUPPLY for a name that already exists is HTTP 400 `BadRequestException` — the caller has
/// to name the version it means to add to — while a lock it did supply and that is not the
/// name's current version is HTTP 409 `ConflictException`, the conditional write losing.
/// Each states the version numbers in play and nothing else about the extension.
#[tokio::test]
async fn latest_version_number_locks_the_name_to_its_current_version() {
    let table = [
        Lock {
            case: "a new name, naming no version, opens the history at 1",
            existing_versions: 0,
            claimed: None,
            expected_status: 201,
            expected_latest: 1,
        },
        Lock {
            case: "a new name naming version 0 is the same request",
            existing_versions: 0,
            claimed: Some(0),
            expected_status: 201,
            expected_latest: 1,
        },
        Lock {
            case: "a new name claiming version 1 conflicts",
            existing_versions: 0,
            claimed: Some(1),
            expected_status: 409,
            expected_latest: 0,
        },
        Lock {
            case: "a new name claiming a version far ahead conflicts",
            existing_versions: 0,
            claimed: Some(97),
            expected_status: 409,
            expected_latest: 0,
        },
        Lock {
            case: "a name already at 1, naming no version, is the missing-lock bad request",
            existing_versions: 1,
            claimed: None,
            expected_status: 400,
            expected_latest: 1,
        },
        Lock {
            case: "a name already at 3, naming no version, is the same bad request",
            existing_versions: 3,
            claimed: None,
            expected_status: 400,
            expected_latest: 3,
        },
        Lock {
            case: "a name already at 1, claiming 1, appends version 2",
            existing_versions: 1,
            claimed: Some(1),
            expected_status: 201,
            expected_latest: 2,
        },
        Lock {
            case: "a name already at 1, claiming 0, conflicts",
            existing_versions: 1,
            claimed: Some(0),
            expected_status: 409,
            expected_latest: 1,
        },
        Lock {
            case: "a name already at 1, claiming 2, conflicts",
            existing_versions: 1,
            claimed: Some(2),
            expected_status: 409,
            expected_latest: 1,
        },
        Lock {
            case: "a name already at 2, claiming 2, appends version 3",
            existing_versions: 2,
            claimed: Some(2),
            expected_status: 201,
            expected_latest: 3,
        },
        Lock {
            case: "a name already at 2, claiming 1, conflicts",
            existing_versions: 2,
            claimed: Some(1),
            expected_status: 409,
            expected_latest: 2,
        },
        Lock {
            case: "a name already at 3, claiming 3, appends version 4",
            existing_versions: 3,
            claimed: Some(3),
            expected_status: 201,
            expected_latest: 4,
        },
        Lock {
            case: "a name already at 3, claiming a negative version, conflicts",
            existing_versions: 3,
            claimed: Some(-1),
            expected_status: 409,
            expected_latest: 3,
        },
    ];

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        // A fresh engine per row, so each row's history is exactly what it declares.
        let router = router();
        let name = format!("locked-{index}");
        for version in 0..row.existing_versions {
            let body = json!({ "Name": name, "Actions": actions(&["PRE_START_DEPLOYMENT"]) });
            let seed = create_extension(&router, &body, Some(version)).await;
            assert_eq!(
                seed.status,
                201,
                "case {case:?}: seeding version {} must answer 201, got {} / {}",
                version + 1,
                seed.status,
                String::from_utf8_lossy(&seed.body)
            );
        }

        let body = json!({ "Name": name, "Actions": actions(&["ON_DEPLOYMENT_START"]) });
        let answer = create_extension(&router, &body, row.claimed).await;
        assert_eq!(
            answer.status,
            row.expected_status,
            "case {case:?}: expected HTTP {}, got {} / {}",
            row.expected_status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if row.expected_status != 201 {
            // Which class, and which sentence, follows from WHICH way the lock failed.
            let (code, message) = match row.claimed {
                None => (
                    "BadRequestException",
                    format!(
                        "LatestVersionNumber not populated and an extension with name {name} \
                         already exists."
                    ),
                ),
                Some(claimed) => (
                    "ConflictException",
                    format!(
                        "LatestVersionNumber supplied in request {claimed} does not match latest \
                         version {}.",
                        row.existing_versions
                    ),
                ),
            };
            assert_eq!(
                answer.error_code().split('#').next_back().unwrap_or_default(),
                code,
                "case {case:?}: expected {code}, got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.string("Message").as_deref(),
                Some(message.as_str()),
                "case {case:?}: expected the message {message:?}, got {:?}",
                answer.string("Message")
            );
        }

        let left_at = latest_version(&router, &name).await.unwrap_or(0);
        assert_eq!(
            left_at, row.expected_latest,
            "case {case:?}: expected the name left at version {}, got {left_at}",
            row.expected_latest
        );
    }
}

/// Each version keeps the definition the call that wrote it gave, the extension's id and
/// name are shared by all of them, and a reader naming no version gets the highest one.
#[tokio::test]
async fn each_version_keeps_its_own_definition() {
    let router = router();
    let name = "versioned";
    // Row `n` is version `n + 1`: a different action point, a different description and a
    // different parameter count per version, so a reader cannot pass by reporting any
    // other version's definition.
    let definitions = [
        (
            actions(&["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"]),
            Some("the first cut".to_string()),
            0,
        ),
        (actions(&["PRE_START_DEPLOYMENT", "AT_DEPLOYMENT_TICK"]), None, 1),
        (
            actions(&ACTION_POINT_VALUES[..3]),
            Some(filler(MAX_DESCRIPTION, 7)),
            MAX_PARAMETERS,
        ),
        (actions(&["ON_DEPLOYMENT_ROLLED_BACK"]), Some(String::new()), 4),
    ];

    let mut extension_id = String::new();
    for (index, (points, description, parameter_count)) in definitions.iter().enumerate() {
        let case = format!("version {}", index + 1);
        let mut body = Map::new();
        body.insert("Name".to_string(), Value::from(name));
        body.insert("Actions".to_string(), points.clone());
        if let Some(description) = description {
            body.insert("Description".to_string(), Value::from(description.clone()));
        }
        if *parameter_count > 0 {
            body.insert(
                "Parameters".to_string(),
                parameters(*parameter_count, |slot| (slot == 0, slot == 1)),
            );
        }
        let created = create_extension(
            &router,
            &Value::Object(body),
            Some(i32::try_from(index).expect("the table is small")),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExtension expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_body = created.json();
        let id = minted_id(&case, &created_body);
        if extension_id.is_empty() {
            extension_id = id.clone();
        }
        assert_eq!(
            id, extension_id,
            "case {case:?}: every version of one extension shares its id"
        );
        assert_eq!(
            created_body.get("VersionNumber").and_then(Value::as_i64),
            Some(index as i64 + 1),
            "case {case:?}: expected version {}, got {created_body}",
            index + 1
        );
    }

    // Each version reads back with its own definition...
    for (index, (points, description, parameter_count)) in definitions.iter().enumerate() {
        let version = i32::try_from(index + 1).expect("the table is small");
        let case = format!("version {version}");
        let read = get_extension(&router, name, Some(version)).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtension expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let body = read.json();
        assert_eq!(
            body.get("Actions").cloned(),
            Some(points.clone()),
            "case {case:?}: expected this version's own Actions, got {body}"
        );
        assert_eq!(
            body.get("Description").cloned(),
            description.clone().map(Value::from),
            "case {case:?}: expected this version's own Description, got {body}"
        );
        assert_eq!(
            body.get("Parameters")
                .and_then(Value::as_object)
                .map_or(0, Map::len),
            *parameter_count,
            "case {case:?}: expected {parameter_count} parameters, got {body}"
        );
        assert_eq!(
            body.get("Arn").and_then(Value::as_str),
            Some(arn(&format!("extension/{extension_id}/{version}")).as_str()),
            "case {case:?}: expected this version's own Arn, got {body}"
        );
    }

    // ... and a reader naming no version gets the highest one.
    let highest = i32::try_from(definitions.len()).expect("the table is small");
    let latest = get_extension(&router, name, None).await;
    assert_eq!(latest.status, 200, "GetExtension must answer the latest version");
    assert_eq!(
        latest.json().get("VersionNumber").and_then(Value::as_i64),
        Some(i64::from(highest)),
        "expected the highest version {highest}, got {}",
        latest.json()
    );

    // A version past the highest exists nowhere.
    let missing = get_extension(&router, name, Some(highest + 1)).await;
    assert_eq!(
        missing.status,
        404,
        "expected 404 for a version that was never written, got {} / {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
}

/// "Dynamic parameters can't be marked `Required`" — the one combination of a
/// parameter's two flags that cannot hold together is refused with HTTP 400 and stores
/// nothing; the other three are stored.
#[tokio::test]
async fn a_dynamic_parameter_cannot_also_be_required() {
    for (index, (required, dynamic)) in [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .enumerate()
    {
        let router = router();
        let case = format!("Required={required}, Dynamic={dynamic}");
        let name = format!("flagged-{index}");
        let body = json!({
            "Name": name,
            "Actions": actions(&["PRE_START_DEPLOYMENT"]),
            "Parameters": { "knob": { "Required": required, "Dynamic": dynamic } },
        });
        let answer = create_extension(&router, &body, None).await;
        let expected = if required && dynamic { 400 } else { 201 };
        assert_eq!(
            answer.status,
            expected,
            "case {case:?}: expected HTTP {expected}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            latest_version(&router, &name).await,
            (expected == 201).then_some(1),
            "case {case:?}: the stored state does not match the answer"
        );
    }
}

/// An identifier that addresses no extension this service holds is HTTP 404
/// `ResourceNotFoundException` — never a 501 and never a success.
#[tokio::test]
async fn an_identifier_that_names_no_extension_is_not_found() {
    let router = router();
    // One extension exists, so a miss is a miss on the lookup and not on an empty store.
    let created = create_extension(
        &router,
        &json!({ "Name": "present", "Actions": actions(&["PRE_START_DEPLOYMENT"]) }),
        None,
    )
    .await;
    assert_eq!(created.status, 201, "the fixture extension must be created");
    let present = minted_id("present", &created.json());

    let misses = [
        ("an id nobody minted", "zzzz999".to_string()),
        ("a name nobody used", "absent".to_string()),
        (
            "an extension ARN whose id does not exist",
            arn("extension/zzzz999/1"),
        ),
        (
            "an extension ARN whose version does not exist",
            arn(&format!("extension/{present}/2")),
        ),
        ("an ARN of another resource type", arn("application/zzzz999")),
        ("an ARN of another service", engine::emulator_arn("lambda", "us-east-1", "function:f")),
    ];
    for (case, identifier) in misses {
        let answer = get_extension(&router, &identifier, None).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404 for {identifier:?}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code().split('#').next_back().unwrap_or_default(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }
}
