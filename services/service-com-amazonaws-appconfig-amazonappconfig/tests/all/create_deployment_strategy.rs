// SPDX-License-Identifier: CC0-1.0

//! `CreateDeploymentStrategy` and `GetDeploymentStrategy`: a strategy is minted from the
//! rollout criteria the request carries, stored, and reported back out of the store.
//!
//! The suites drive real HTTP requests through the router, so every assertion is about
//! the wire behaviour. Each created strategy is read back through
//! `GetDeploymentStrategy`, and the criteria it persisted are read a third time through
//! `StartDeployment`, which paces a deployment by them — so a create that answered a
//! value it never persisted fails here.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, PREDEFINED_DEPLOYMENT_STRATEGIES,
};

use super::appconfig_workflow::{
    create_application, create_environment, create_profile, host_version, is_modelled_id, json_call,
    router, router_on, Answer,
};
use super::resource_tags::{arn, filler as padding, list_tags};

/// Where every deployment strategy lives: the collection the model binds
/// `CreateDeploymentStrategy` to (`POST /deploymentstrategies`).
const STRATEGIES: &str = "/deploymentstrategies";

/// One row of the create-then-read-back table.
struct Row {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The `CreateDeploymentStrategy` request body.
    request: Value,
    /// The members the created strategy must report, after the service has decided what
    /// each ABSENT optional member means.
    expected: Value,
    /// The members the created strategy must not report at all.
    absent: &'static [&'static str],
}

/// A string of `len` ASCII characters, for a row that sits on a `@length` bound. The
/// sibling tagging suite owns the generator; this wrapper just fixes its seed so a row
/// reads as one argument.
fn filler(len: usize) -> String {
    padding(len, 0)
}

/// The create-then-read-back table. Rows differ along what the rule generalises over:
/// which optional members the request states, each growth type the model's `GrowthType`
/// enum lists, each `ReplicateTo` value, and the published bounds of
/// `MinutesBetween0And24Hours` (range 0..1440), `GrowthFactor` (range 1..100) and `Name`
/// (length 1..64) — every bound taken from `smithy_query(query="operation",
/// operation="CreateDeploymentStrategy")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every member stated, linear growth",
            request: json!({
                "Name": "linear-stated",
                "Description": "a flat rollout",
                "DeploymentDurationInMinutes": 30,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 20,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            expected: json!({
                "Name": "linear-stated",
                "Description": "a flat rollout",
                "DeploymentDurationInMinutes": 30,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 20,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &[],
        },
        Row {
            case: "exponential growth",
            request: json!({
                "Name": "exponential",
                "DeploymentDurationInMinutes": 20,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 10,
                "GrowthType": "EXPONENTIAL",
            }),
            expected: json!({
                "Name": "exponential",
                "DeploymentDurationInMinutes": 20,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 10,
                "GrowthType": "EXPONENTIAL",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "every optional member absent",
            request: json!({
                "Name": "bare",
                "DeploymentDurationInMinutes": 15,
                "GrowthFactor": 25,
            }),
            // `FinalBakeTimeInMinutes` carries the model's own default of 0; the growth
            // type and the replication target are what an unstated one means.
            expected: json!({
                "Name": "bare",
                "DeploymentDurationInMinutes": 15,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 25,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "minutes at the published lower bound",
            request: json!({
                "Name": "all-at-once",
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 100,
            }),
            expected: json!({
                "Name": "all-at-once",
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "minutes at the published upper bound",
            request: json!({
                "Name": "a full day",
                "DeploymentDurationInMinutes": 1440,
                "FinalBakeTimeInMinutes": 1440,
                "GrowthFactor": 5,
            }),
            expected: json!({
                "Name": "a full day",
                "DeploymentDurationInMinutes": 1440,
                "FinalBakeTimeInMinutes": 1440,
                "GrowthFactor": 5,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "growth factor at the published lower bound",
            request: json!({
                "Name": "one percent",
                "DeploymentDurationInMinutes": 100,
                "GrowthFactor": 1,
            }),
            expected: json!({
                "Name": "one percent",
                "DeploymentDurationInMinutes": 100,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 1,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "growth factor at the published upper bound",
            request: json!({
                "Name": "one hundred percent",
                "DeploymentDurationInMinutes": 1,
                "GrowthFactor": 100,
            }),
            expected: json!({
                "Name": "one hundred percent",
                "DeploymentDurationInMinutes": 1,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "a fractional growth factor",
            request: json!({
                "Name": "a third at a time",
                "DeploymentDurationInMinutes": 9,
                "GrowthFactor": 33.3,
            }),
            expected: json!({
                "Name": "a third at a time",
                "DeploymentDurationInMinutes": 9,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 33.3,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "replicated to a Systems Manager document",
            request: json!({
                "Name": "replicated",
                "DeploymentDurationInMinutes": 6,
                "GrowthFactor": 20,
                "ReplicateTo": "SSM_DOCUMENT",
            }),
            expected: json!({
                "Name": "replicated",
                "DeploymentDurationInMinutes": 6,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 20,
                "GrowthType": "LINEAR",
                "ReplicateTo": "SSM_DOCUMENT",
            }),
            absent: &["Description"],
        },
        Row {
            case: "a name at the published lower length bound",
            request: json!({
                "Name": "x",
                "DeploymentDurationInMinutes": 2,
                "GrowthFactor": 50,
            }),
            expected: json!({
                "Name": "x",
                "DeploymentDurationInMinutes": 2,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 50,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &["Description"],
        },
        Row {
            case: "a name and a description at their published upper length bounds",
            request: json!({
                "Name": filler(64),
                "Description": filler(1024),
                "DeploymentDurationInMinutes": 3,
                "GrowthFactor": 10,
            }),
            expected: json!({
                "Name": filler(64),
                "Description": filler(1024),
                "DeploymentDurationInMinutes": 3,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 10,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &[],
        },
        Row {
            case: "an empty description",
            request: json!({
                "Name": "empty description",
                "Description": "",
                "DeploymentDurationInMinutes": 4,
                "GrowthFactor": 10,
            }),
            expected: json!({
                "Name": "empty description",
                "Description": "",
                "DeploymentDurationInMinutes": 4,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 10,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
            absent: &[],
        },
    ]
}

/// Assert `answer` reports every member `expected` names. Numbers are compared
/// numerically, so an integral float is not read as a different value from the integer
/// it renders as, and a 32-bit growth factor is not failed by its own precision.
fn assert_members(case: &str, reader: &str, answer: &Answer, expected: &Value) {
    let body = answer.json();
    for (member, want) in expected
        .as_object()
        .expect("every expectation is a JSON object")
    {
        let got = body.get(member);
        match (want.as_f64(), got.and_then(Value::as_f64)) {
            (Some(wanted), Some(actual)) => assert!(
                (wanted - actual).abs() < 1e-4,
                "case {case:?}: {reader} expected {member} {wanted}, got {actual}"
            ),
            _ => assert_eq!(
                got,
                Some(want),
                "case {case:?}: {reader} expected {member} {want}, got {got:?}"
            ),
        }
    }
}

/// `GET` one deployment strategy.
async fn get_strategy(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(router, "GET", &format!("{STRATEGIES}/{id}"), &Value::Null).await
}

/// Every criterion a request states is persisted and answered, every optional member it
/// omits gets the value an unstated one means, and the record a create answers is the
/// record `GetDeploymentStrategy` reports.
#[tokio::test]
async fn created_strategies_are_persisted_and_read_back() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut minted: HashSet<String> = HashSet::new();

    for row in rows() {
        let created = json_call(&router, "POST", STRATEGIES, &row.request).await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: CreateDeploymentStrategy expected 201, got {} / {}",
            row.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {:?}: no Id in {}", row.case, created.json()));
        assert!(
            is_modelled_id(&id),
            "case {:?}: Id must satisfy the model's `Id` shape ^[a-z0-9]{{4,7}}$, got {id:?}",
            row.case
        );
        assert!(
            minted.insert(id.clone()),
            "case {:?}: id {id:?} was already minted for another strategy",
            row.case
        );

        assert_members(row.case, "the create response", &created, &row.expected);
        for member in row.absent {
            assert!(
                created.json().get(member).is_none(),
                "case {:?}: the create response must not report {member}, got {:?}",
                row.case,
                created.json().get(member)
            );
        }

        // The round trip: the create is only real if a later read sees it.
        let read = get_strategy(&router, &id).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: GetDeploymentStrategy expected 200, got {} / {}",
            row.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.string("Id").as_deref(),
            Some(id.as_str()),
            "case {:?}: GetDeploymentStrategy must report the id it was asked for",
            row.case
        );
        assert_members(row.case, "GetDeploymentStrategy", &read, &row.expected);
        for member in row.absent {
            assert!(
                read.json().get(member).is_none(),
                "case {:?}: GetDeploymentStrategy must not report {member}, got {:?}",
                row.case,
                read.json().get(member)
            );
        }
    }

    // Every row above was stored, not just answered.
    let held = state::service_state(&*engine)
        .expect("the test engine must carry this service's state")
        .lock()
        .created_strategies()
        .map(|strategy| strategy.id.clone())
        .collect::<HashSet<String>>();
    assert_eq!(
        held, minted,
        "case \"the store after every row\": expected the minted ids {minted:?}, got {held:?}"
    );
}

/// The tags a create request assigns are persisted with the strategy and reported by
/// `ListTagsForResource` for the strategy's ARN. `DeploymentStrategy` carries no `Tags`
/// member, so the create response must not report one.
#[tokio::test]
async fn assigned_tags_are_persisted_with_the_strategy() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Rows differ along the number of tags assigned, including none at all.
    let cases: [(&str, Value, HashMap<String, String>); 3] = [
        ("no tags", json!({}), HashMap::new()),
        (
            "one tag",
            json!({ "team": "release" }),
            HashMap::from([("team".to_string(), "release".to_string())]),
        ),
        (
            "two tags, one with an empty value",
            json!({ "team": "release", "owner": "" }),
            HashMap::from([
                ("team".to_string(), "release".to_string()),
                ("owner".to_string(), String::new()),
            ]),
        ),
    ];

    for (case, tags, expected) in cases {
        let created = json_call(
            &router,
            "POST",
            STRATEGIES,
            &json!({
                "Name": format!("tagged-{case}"),
                "DeploymentDurationInMinutes": 5,
                "GrowthFactor": 25,
                "Tags": tags,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateDeploymentStrategy expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert!(
            created.json().get("Tags").is_none(),
            "case {case:?}: `DeploymentStrategy` carries no Tags member, got {:?}",
            created.json().get("Tags")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        let stored = state::service_state(&*engine)
            .expect("the test engine must carry this service's state")
            .lock()
            .created_strategies()
            .find(|strategy| strategy.id == id)
            .map(|strategy| strategy.tags.clone())
            .unwrap_or_else(|| panic!("case {case:?}: strategy {id} was not stored"));
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored tags {expected:?}, got {stored:?}"
        );

        // The same map, through the tag reader: a strategy's tags live on the strategy
        // record, so `ListTagsForResource` reports exactly what the create persisted.
        let strategy_arn = arn(&format!("deploymentstrategy/{id}"));
        let reported = list_tags(&router, &strategy_arn).await;
        assert_eq!(
            reported.status,
            200,
            "case {case:?}: ListTagsForResource expected 200 for {strategy_arn}, got {} / {}",
            reported.status,
            String::from_utf8_lossy(&reported.body)
        );
        let reported: HashMap<String, String> = reported
            .json()
            .get("Tags")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: ListTagsForResource expected {expected:?}, got {reported:?}"
        );
    }

    // A predefined strategy is not a record this service stores, so it carries no tag
    // map and its ARN addresses nothing.
    let predefined = arn("deploymentstrategy/AppConfig.AllAtOnce");
    let answer = list_tags(&router, &predefined).await;
    assert_eq!(
        answer.status,
        404,
        "case \"a predefined strategy's ARN\": expected 404, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// A tag key this service will not store is refused with the modelled 400
/// `BadRequestException` — through the same rule `TagResource` refuses through — and the
/// strategy itself is not created.
#[tokio::test]
async fn a_create_naming_an_unacceptable_tag_key_is_refused() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Rows differ along WHY the key is unacceptable: empty, reserved, and one character
    // past `TagKey`'s published 128-character maximum (`state::MAX_TAG_KEY_LENGTH`).
    let one_tag = |key: String, value: &str| -> Value {
        Value::Object(serde_json::Map::from_iter([(key, Value::from(value))]))
    };
    let cases = [
        ("an empty key", one_tag(String::new(), "orphan")),
        (
            "a key under the reserved aws: prefix",
            one_tag(
                format!("{}billing", state::RESERVED_TAG_KEY_PREFIX),
                "stack",
            ),
        ),
        (
            "a key one character past the published maximum",
            one_tag(filler(state::MAX_TAG_KEY_LENGTH + 1), "too long"),
        ),
    ];

    for (case, tags) in cases {
        let refused = json_call(
            &router,
            "POST",
            STRATEGIES,
            &json!({
                "Name": format!("rejected-{case}"),
                "DeploymentDurationInMinutes": 5,
                "GrowthFactor": 25,
                "Tags": tags,
            }),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            state::service_state(&*engine)
                .expect("the test engine must carry this service's state")
                .lock()
                .created_strategies()
                .count(),
            0,
            "case {case:?}: a refused create must persist nothing"
        );
    }
}

/// A criterion outside a bound the model publishes is refused with HTTP 400 before any
/// handler runs, and nothing is persisted.
#[tokio::test]
async fn criteria_outside_the_published_bounds_are_refused() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let held = || {
        state::service_state(&*engine)
            .expect("the test engine must carry this service's state")
            .lock()
            .created_strategies()
            .count()
    };

    // One row for each published bound, taken one step past it, plus one for each
    // required member left out and one for a value outside each enum's value set.
    let cases = [
        (
            "a deployment duration past the upper bound",
            json!({ "Name": "too long", "DeploymentDurationInMinutes": 1441, "GrowthFactor": 10 }),
        ),
        (
            "a negative deployment duration",
            json!({ "Name": "negative", "DeploymentDurationInMinutes": -1, "GrowthFactor": 10 }),
        ),
        (
            "a final bake time past the upper bound",
            json!({
                "Name": "bake too long",
                "DeploymentDurationInMinutes": 10,
                "FinalBakeTimeInMinutes": 1441,
                "GrowthFactor": 10,
            }),
        ),
        (
            "a growth factor below the lower bound",
            json!({ "Name": "no growth", "DeploymentDurationInMinutes": 10, "GrowthFactor": 0 }),
        ),
        (
            "a growth factor past the upper bound",
            json!({ "Name": "too much growth", "DeploymentDurationInMinutes": 10, "GrowthFactor": 101 }),
        ),
        (
            "an empty name",
            json!({ "Name": "", "DeploymentDurationInMinutes": 10, "GrowthFactor": 10 }),
        ),
        (
            "a name past the upper length bound",
            json!({ "Name": filler(65), "DeploymentDurationInMinutes": 10, "GrowthFactor": 10 }),
        ),
        (
            "a description past the upper length bound",
            json!({
                "Name": "long description",
                "Description": filler(1025),
                "DeploymentDurationInMinutes": 10,
                "GrowthFactor": 10,
            }),
        ),
        (
            "a growth type outside the enum",
            json!({
                "Name": "lowercase growth type",
                "DeploymentDurationInMinutes": 10,
                "GrowthFactor": 10,
                "GrowthType": "linear",
            }),
        ),
        (
            "a replication target outside the enum",
            json!({
                "Name": "unknown replication",
                "DeploymentDurationInMinutes": 10,
                "GrowthFactor": 10,
                "ReplicateTo": "SSM",
            }),
        ),
        (
            "no name",
            json!({ "DeploymentDurationInMinutes": 10, "GrowthFactor": 10 }),
        ),
        (
            "no growth factor",
            json!({ "Name": "no factor", "DeploymentDurationInMinutes": 10 }),
        ),
        (
            "no deployment duration",
            json!({ "Name": "no duration", "GrowthFactor": 10 }),
        ),
    ];

    for (case, request) in cases {
        let before = held();
        let refused = json_call(&router, "POST", STRATEGIES, &request).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: a bound the model declares is enforced by the schema walk, \
             which refuses it with the modelled BadRequestException; got {:?}",
            refused.error_code()
        );
        assert_eq!(
            held(),
            before,
            "case {case:?}: a refused create must persist nothing"
        );
    }
}

/// AppConfig's predefined `AppConfig.*` strategies are deployable without a create call,
/// so `GetDeploymentStrategy` reports each one, and an id naming no strategy at all is
/// `ResourceNotFoundException`.
#[tokio::test]
async fn predefined_strategies_are_readable_and_unknown_ids_are_not_found() {
    let router = router();

    // The catalog is `state::PREDEFINED_DEPLOYMENT_STRATEGIES`, from the AppConfig User
    // Guide's "Predefined deployment strategies" table; every row is asserted, including
    // the exponential canary one.
    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let read = get_strategy(&router, predefined.id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeploymentStrategy expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_members(
            case,
            "GetDeploymentStrategy",
            &read,
            &json!({
                // A predefined strategy's name is its id.
                "Id": predefined.id,
                "Name": predefined.id,
                "DeploymentDurationInMinutes": predefined.deployment_duration_in_minutes,
                "FinalBakeTimeInMinutes": predefined.final_bake_time_in_minutes,
                "GrowthFactor": predefined.growth_factor,
                "GrowthType": predefined.growth_type,
                "ReplicateTo": "NONE",
            }),
        );
    }

    // Rows differ along the shape of the id: one the model's `Id` pattern allows but
    // nothing minted, and one shaped like a predefined strategy that is not in the
    // catalog. Both name no strategy, so both are 404.
    for case in ["zzzz999", "AppConfig.NotAStrategyAtAll"] {
        let missing = get_strategy(&router, case).await;
        assert_eq!(
            missing.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        assert!(
            missing.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            missing.error_code()
        );
    }

    // An id outside the member's own pattern is refused before any lookup.
    let malformed = get_strategy(&router, "NOT-AN-ID").await;
    assert_eq!(
        malformed.status,
        400,
        "case \"an id outside the member's pattern\": expected 400, got {} / {}",
        malformed.status,
        String::from_utf8_lossy(&malformed.body)
    );
}

/// The criteria a created strategy persists are the criteria a deployment started with it
/// is paced by: a third reader of the same stored record.
#[tokio::test]
async fn a_created_strategy_paces_a_deployment() {
    let router = router();
    let application = create_application(&router, "paced").await;
    let profile = create_profile(&router, &application, "config").await;
    // The version each row deploys: a deployment resolves its `ConfigurationVersion`
    // against the versions the profile holds.
    host_version(
        &router,
        &application,
        &profile,
        "paced",
        br#"{"enabled":true}"#,
        None,
    )
    .await;

    // Rows differ along the growth algorithm and the length of the rollout.
    let cases = [
        ("all at once, linear", "LINEAR", 100, 0, 0),
        ("exponential canary", "EXPONENTIAL", 10, 20, 10),
    ];

    for (case, growth_type, growth_factor, duration, bake) in cases {
        let created = json_call(
            &router,
            "POST",
            STRATEGIES,
            &json!({
                "Name": format!("paced-{case}"),
                "DeploymentDurationInMinutes": duration,
                "FinalBakeTimeInMinutes": bake,
                "GrowthFactor": growth_factor,
                "GrowthType": growth_type,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateDeploymentStrategy expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let strategy = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        // One environment per row, so neither deployment waits on the other.
        let environment = create_environment(&router, &application, case).await;
        let started = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            &json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment expected 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_eq!(
            started.string("DeploymentStrategyId").as_deref(),
            Some(strategy.as_str()),
            "case {case:?}: the deployment must name the strategy it was started with"
        );
        assert_members(
            case,
            "StartDeployment",
            &started,
            &json!({
                "DeploymentDurationInMinutes": duration,
                "FinalBakeTimeInMinutes": bake,
                "GrowthFactor": growth_factor,
                "GrowthType": growth_type,
            }),
        );
    }
}
