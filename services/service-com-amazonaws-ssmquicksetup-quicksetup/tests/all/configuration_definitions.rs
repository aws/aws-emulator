// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateConfigurationDefinition`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an update replaces, in the ONE stored configuration definition
//! the `{ManagerArn}/{Id}` pair names, exactly the members the request carries — and,
//! inside `Parameters`, exactly the entries it names — leaves
//! every member and every parameter entry it omits, and every other definition, as they
//! were stored, and carries
//! the new values into the configuration the service deploys from that definition — so
//! a later `GetConfigurationManager` and `ListConfigurations` report them. A pair that
//! names no stored definition is the modelled 404, and an input the model or the
//! service cannot accept is the modelled 400 with nothing stored for it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    MAX_PARAMETERS, QUICK_SETUP_TYPE_CATALOG, STATUS_INITIALIZING, STATUS_SUCCEEDED,
    STATUS_TYPE_DEPLOYMENT, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create ever
/// minted, for the not-found cases.
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/\
     00000000-0000-4000-8000-000000000000";

/// Long enough that the clock-owed deployment transition has come due.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

async fn update_definition(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(body),
    )
    .await
}

/// Update a definition that must succeed.
async fn update_definition_ok(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
    id: &str,
    body: &Value,
) {
    let (status, answer) = update_definition(router, manager_arn, id, body).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateConfigurationDefinition must succeed, got {status} / {answer:?}"
    );
}

/// Create a manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = call(router, "POST", "/configurationManager", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// Read the manager back, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

async fn list_configurations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurations", Some(body)).await
}

/// The Quick Setup type the definition every `TypeVersion` case below updates
/// deploys: its published version history is what those versions are resolved
/// against.
const BASELINE_TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The organizational unit every definition below deploys to. A parameter set that
/// names neither the accounts nor the organizational units to deploy to is refused, and
/// no case here is about that rule, so every accepted parameter set names this one.
const TARGET_OU: &str = "ou-1";

/// The Region every definition below deploys into. A parameter set that names no
/// Regions to deploy into is refused, and no case here is about that rule, so every
/// accepted parameter set names this one.
const TARGET_REGIONS: &str = "us-east-1";

/// The newest configuration version the catalog publishes for `type_` — what the
/// modelled `LATEST` alias resolves to.
fn latest_version_of(type_: &str) -> &'static str {
    QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == type_)
        .unwrap_or_else(|| panic!("{type_:?} must be a published Quick Setup type"))
        .latest_version()
}

/// The parameter map the FIRST definition of [`baseline`] is created with: the map every
/// update below writes its entries into.
fn baseline_parameters() -> Value {
    json!({
        "TargetOrganizationalUnits": TARGET_OU,
        "TargetRegions": TARGET_REGIONS,
        "TargetType": "*",
        "UpdateSSMAgent": "true",
    })
}

/// The `Parameters` the updated definition HOLDS once an update whose `Parameters`
/// carries `entries` has been served: an update writes the entries it names into the
/// stored map and leaves every other stored entry alone.
fn parameters_after(entries: &Value) -> Value {
    merged(&baseline_parameters(), entries)
}

/// One row of the table below for an update carrying nothing but `entries` in its
/// `Parameters`: the body to send, and the members the named definition must then
/// report.
fn parameter_case(case: &'static str, entries: Value) -> (&'static str, Value, Value) {
    let held = parameters_after(&entries);
    (
        case,
        json!({ "Parameters": entries }),
        json!({ "Parameters": held }),
    )
}

/// The two definitions every case below starts from: both carry a `TypeVersion` the
/// catalog publishes for their type and a parameter set naming somewhere to deploy to
/// and the Region to deploy into (which every accepted definition must), the first also
/// both local-deployment role
/// members, so an update that omits a member is observably leaving a STORED value alone
/// rather than leaving an absent one absent.
fn baseline() -> Value {
    json!({
        "Name": "definition-updates",
        "ConfigurationDefinitions": [
            {
                "Type": BASELINE_TYPE,
                "Parameters": baseline_parameters(),
                "TypeVersion": "1.0",
                "LocalDeploymentExecutionRoleName": "QuickSetupExecution",
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::000000000000:role/QuickSetupAdministration",
            },
            {
                "Type": "AWSQuickSetupType-Scheduler",
                "Parameters": {
                    "TargetOrganizationalUnits": TARGET_OU,
                    "TargetRegions": TARGET_REGIONS,
                    "TargetTagKey": "Env",
                    "TargetTagValue": "production",
                    "ICalendarString": "BEGIN:VCALENDAR",
                },
                "TypeVersion": "2.0",
            },
        ],
    })
}

/// The definition ids a Get reports, in order.
fn definition_ids(case: &str, manager: &Value) -> Vec<String> {
    manager
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"))
        .iter()
        .map(|definition| {
            definition
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected an Id, got {definition:?}"))
                .to_string()
        })
        .collect()
}

/// The one definition a Get reports under `id`.
fn definition_by_id(case: &str, manager: &Value, id: &str) -> Value {
    manager
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"))
        .iter()
        .find(|definition| definition.get("Id").and_then(Value::as_str) == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: no definition {id:?} in {manager:?}"))
}

/// `base` with every member `overrides` names replaced: the definition an update
/// carrying `overrides` must leave behind.
fn merged(base: &Value, overrides: &Value) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    for (member, value) in overrides.as_object().cloned().unwrap_or_default() {
        out.insert(member, value);
    }
    Value::Object(out)
}

/// The `Deployment` summary's `Status`, which is what the clock settles.
fn deployment_status(case: &str, summaries: &Value) -> String {
    summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summaries:?}"))
        .iter()
        .find(|summary| {
            summary.get("StatusType").and_then(Value::as_str) == Some(STATUS_TYPE_DEPLOYMENT)
        })
        .and_then(|summary| summary.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {summaries:?}")
        })
        .to_string()
}

/// A modelled timestamp member as seconds since the epoch, so two stamps can be
/// ordered. The member carries `@timestampFormat("date-time")`, so only the RFC 3339
/// string is accepted; `tests/all/date_time_timestamps.rs` pins that wire form in its own
/// right, and reading it here keeps this helper about the ORDERING of the stamps.
fn epoch_seconds(case: &str, value: &Value, member: &str) -> f64 {
    let stamp = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case}: expected {member}, got {value:?}"));
    match stamp {
        Value::String(text) => {
            aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_else(|e| panic!("case {case}: {member} {text:?} is not a date-time: {e}"))
                .as_secs_f64()
        }
        other => panic!(
            "case {case}: {member} must be an RFC 3339 date-time string, got {other:?}"
        ),
    }
}

/// A parameter map of `count` entries, TWO of them the deployment target and the
/// Regions every accepted definition must name, and the rest keys inside the modelled
/// `ConfigurationParametersMap` key pattern `^[A-Za-z0-9+=@_\/\s-]+$`.
fn parameters(count: usize) -> Value {
    assert!(
        count >= 2,
        "a parameter set names somewhere to deploy to and the Regions to deploy into, so \
         it holds at least two entries"
    );
    let mut map: Map<String, Value> = (0..count - 2)
        .map(|index| (format!("Key{index}"), json!(index.to_string())))
        .collect();
    map.insert("TargetOrganizationalUnits".to_string(), json!(TARGET_OU));
    map.insert("TargetRegions".to_string(), json!(TARGET_REGIONS));
    Value::Object(map)
}

/// An update replaces exactly the members it carries, in exactly the definition it
/// names. Each row differs along what the rule generalises over — WHICH members the
/// request carries, and the form and the size of their values.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries_in_the_definition_it_names() {
    let latest = latest_version_of(BASELINE_TYPE);
    // (case, the update body, the members the named definition must then report)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no member at all",
            json!({}),
            json!({}),
        ),
        (
            // A version the catalog publishes for this definition's type.
            "a published type version",
            json!({ "TypeVersion": "1.1" }),
            json!({ "TypeVersion": "1.1" }),
        ),
        (
            // `^\d{1,3}(\.\d{1,3})?$|^LATEST$` — the modelled `TypeVersion` pattern of
            // this operation's input — admits the `LATEST` alias, which names whichever
            // version the catalog publishes as this type's newest, so that version is
            // what the definition is moved to and reports.
            "the LATEST alias",
            json!({ "TypeVersion": "LATEST" }),
            json!({ "TypeVersion": latest }),
        ),
        (
            // That pattern admits a major with no minor, which numbers the same version
            // as `<major>.0`: the spelling the catalog publishes is what is stored.
            "a published type version spelled without its minor",
            json!({ "TypeVersion": "2" }),
            json!({ "TypeVersion": "2.0" }),
        ),
        parameter_case(
            // A parameter the update names is written over the stored one; a parameter
            // it does not name keeps the value the definition holds for it.
            "one parameter written over the stored map",
            json!({
                "TargetOrganizationalUnits": TARGET_OU,
                "TargetRegions": TARGET_REGIONS,
                "TargetType": "instance",
            }),
        ),
        parameter_case(
            // Every entry it names already holds that value, so this update stores
            // nothing at all.
            "a parameter set naming only entries the definition already holds",
            parameters(2),
        ),
        parameter_case(
            "a parameter set larger than the stored one",
            parameters(5),
        ),
        parameter_case(
            // The stored map holds two entries this one does not name
            // (`baseline_parameters`), which it leaves in place, so a set of
            // `MAX_PARAMETERS - 2` entries leaves the definition holding exactly the
            // bound.
            "a parameter set leaving the definition at the bound it may hold",
            parameters(MAX_PARAMETERS - 2),
        ),
        (
            "the local deployment execution role only",
            json!({ "LocalDeploymentExecutionRoleName": "OtherExecutionRole" }),
            json!({ "LocalDeploymentExecutionRoleName": "OtherExecutionRole" }),
        ),
        (
            // `^[\w+=,.@-]{1,64}$` is the modelled pattern; 64 characters is its
            // published upper bound, so it must be accepted.
            "an execution role name at the pattern's upper bound",
            json!({ "LocalDeploymentExecutionRoleName": "r".repeat(64) }),
            json!({ "LocalDeploymentExecutionRoleName": "r".repeat(64) }),
        ),
        (
            "the local deployment administration role only",
            json!({
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::123456789012:role/OtherAdministration",
            }),
            json!({
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::123456789012:role/OtherAdministration",
            }),
        ),
        (
            "every member at once",
            json!({
                "TypeVersion": "LATEST",
                "Parameters": {
                    "TargetOrganizationalUnits": TARGET_OU,
                    "TargetRegions": TARGET_REGIONS,
                    "TargetType": "tag-key",
                    "TargetTagKey": "Env",
                },
                "LocalDeploymentExecutionRoleName": "AllAtOnceExecution",
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::000000000000:role/AllAtOnceAdministration",
            }),
            json!({
                "TypeVersion": latest,
                "Parameters": parameters_after(&json!({
                    "TargetOrganizationalUnits": TARGET_OU,
                    "TargetRegions": TARGET_REGIONS,
                    "TargetType": "tag-key",
                    "TargetTagKey": "Env",
                })),
                "LocalDeploymentExecutionRoleName": "AllAtOnceExecution",
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::000000000000:role/AllAtOnceAdministration",
            }),
        ),
    ];

    for (case, body, overrides) in cases {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let manager_arn = create_ok(&router, case, &baseline()).await;
        let before = get_ok(&router, case, &manager_arn).await;
        let ids = definition_ids(case, &before);
        assert_eq!(ids.len(), 2, "case {case}: expected two definitions, got {ids:?}");
        let (target, sibling) = (ids[0].clone(), ids[1].clone());
        let target_before = definition_by_id(case, &before, &target);
        let sibling_before = definition_by_id(case, &before, &sibling);
        let created = epoch_seconds(case, &before, "CreatedAt");
        let modified_before = epoch_seconds(case, &before, "LastModifiedAt");

        // So the bumped stamp is observably later than the one the create wrote.
        tokio::time::sleep(Duration::from_millis(20)).await;
        update_definition_ok(&router, case, &manager_arn, &target, &body).await;

        let after = get_ok(&router, case, &manager_arn).await;
        let want = merged(&target_before, &overrides);
        assert_eq!(
            definition_by_id(case, &after, &target),
            want,
            "case {case}: expected the definition {want:?}, got {after:?}"
        );
        assert_eq!(
            definition_by_id(case, &after, &sibling),
            sibling_before,
            "case {case}: the definition the update did not name must be untouched, got {after:?}"
        );
        assert_eq!(
            definition_ids(case, &after),
            ids,
            "case {case}: the stored definitions must keep their ids and their order, got {after:?}"
        );
        assert_eq!(
            epoch_seconds(case, &after, "CreatedAt"),
            created,
            "case {case}: CreatedAt must not move, got {after:?}"
        );
        // The stamp reports when the record was last CHANGED, so it moves for a row
        // whose update stored a new value and stays put for one that asked for no
        // change at all (`@idempotent`).
        let modified_after = epoch_seconds(case, &after, "LastModifiedAt");
        if want == target_before {
            assert_eq!(
                modified_after, modified_before,
                "case {case}: an update that stored no new value must not move \
                 LastModifiedAt, expected {modified_before}, got {modified_after}"
            );
        } else {
            assert!(
                modified_after > modified_before,
                "case {case}: LastModifiedAt must move forward, expected past \
                 {modified_before}, got {modified_after}"
            );
        }

        // `@idempotent`: replaying exactly the same request leaves exactly the same
        // definition, and stores no new value, so the stamp stays where the first call
        // left it.
        update_definition_ok(&router, case, &manager_arn, &target, &body).await;
        let replayed = get_ok(&router, case, &manager_arn).await;
        assert_eq!(
            definition_by_id(case, &replayed, &target),
            want,
            "case {case}: a replayed update must leave the same definition, got {replayed:?}"
        );
        let modified_replayed = epoch_seconds(case, &replayed, "LastModifiedAt");
        assert_eq!(
            modified_replayed, modified_after,
            "case {case}: a replayed update must not move LastModifiedAt, expected \
             {modified_after}, got {modified_replayed}"
        );
    }
}

/// The configuration the service deploys from a definition reports what the update
/// wrote, and only for the definition the update named.
#[tokio::test]
async fn the_deployed_configuration_reports_what_the_update_wrote() {
    let case = "deployed configuration";
    let router = emulator();
    let manager_arn = create_ok(&router, case, &baseline()).await;
    let before = get_ok(&router, case, &manager_arn).await;
    let ids = definition_ids(case, &before);
    let (target, sibling) = (ids[0].clone(), ids[1].clone());

    let rows_before = configuration_rows(case, &router, &manager_arn).await;
    let target_type = rows_before
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&target))
        .and_then(|row| row.get("Type"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a row for {target}, got {rows_before:?}"));
    let sibling_before = rows_before
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&sibling))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a row for {sibling}, got {rows_before:?}"));

    update_definition_ok(
        &router,
        case,
        &manager_arn,
        &target,
        &json!({
            "TypeVersion": "1.1",
            "Parameters": {
                "TargetOrganizationalUnits": TARGET_OU,
                "TargetRegions": TARGET_REGIONS,
                "TargetType": "instance",
            },
        }),
    )
    .await;

    let rows_after = configuration_rows(case, &router, &manager_arn).await;
    assert_eq!(
        rows_after.len(),
        rows_before.len(),
        "case {case}: an update deploys no new configuration, got {rows_after:?}"
    );
    let target_after = rows_after
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&target))
        .unwrap_or_else(|| panic!("case {case}: expected a row for {target}, got {rows_after:?}"));
    assert_eq!(
        target_after.get("TypeVersion"),
        Some(&json!("1.1")),
        "case {case}: the deployed configuration must carry the updated TypeVersion, got {target_after:?}"
    );
    assert_eq!(
        target_after.get("FirstClassParameters"),
        Some(&json!({
            "TargetOrganizationalUnits": TARGET_OU,
            "TargetRegions": TARGET_REGIONS,
        })),
        "case {case}: the deployed configuration must carry the common parameters the update \
         wrote, got {target_after:?}"
    );
    // The listing reports only a definition's common parameters, so the whole map the
    // update wrote — the type-specific option included — is read back through
    // `GetConfiguration`.
    let target_id = target_after["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an Id, got {target_after:?}"));
    let (status, deployed) = call(
        &router,
        "GET",
        &format!("/getConfiguration/{target_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfiguration must succeed, got {status} / {deployed:?}"
    );
    assert_eq!(
        deployed.get("Parameters"),
        Some(&parameters_after(&json!({
            "TargetOrganizationalUnits": TARGET_OU,
            "TargetRegions": TARGET_REGIONS,
            "TargetType": "instance",
        }))),
        "case {case}: the deployed configuration must carry every parameter the definition \
         now holds, got {deployed:?}"
    );
    assert_eq!(
        target_after.get("Type"),
        Some(&target_type),
        "case {case}: Type is not an input of this operation, got {target_after:?}"
    );
    let sibling_after = rows_after
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&sibling))
        .unwrap_or_else(|| panic!("case {case}: expected a row for {sibling}, got {rows_after:?}"));
    // The sibling's own clock-owed deployment transition may have settled between the
    // two listings, so its summaries are not compared — everything the update could
    // have written is.
    for member in [
        "Id",
        "ManagerArn",
        "ConfigurationDefinitionId",
        "Type",
        "TypeVersion",
        "Account",
        "Region",
        "CreatedAt",
        "FirstClassParameters",
    ] {
        assert_eq!(
            sibling_after.get(member),
            sibling_before.get(member),
            "case {case}: {member} of the configuration the update did not name must be untouched, \
             got {sibling_after:?}"
        );
    }
}

/// The `ConfigurationsList` rows `ListConfigurations` reports for one manager.
async fn configuration_rows(
    case: &str,
    router: &MultiServiceRouter,
    manager_arn: &str,
) -> Vec<Value> {
    let (status, answer) = list_configurations(router, &json!({ "ManagerArn": manager_arn })).await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .clone()
}

/// A `{ManagerArn}/{Id}` pair that names no stored definition is the modelled 404, and
/// nothing is written for it. (A pair whose MANAGER ARN names no stored manager is not
/// one of these: the route's target is authorized ahead of the handler, so that request
/// never reaches the lookup — `tests/all/target_authorization.rs`.)
#[tokio::test]
async fn a_pair_that_names_no_stored_definition_is_a_404() {
    let router = emulator();
    let manager_arn = create_ok(&router, "not found", &baseline()).await;
    let before = get_ok(&router, "not found", &manager_arn).await;
    let ids = definition_ids("not found", &before);

    // A second manager, so a definition id one manager owns is still not found under
    // the OTHER manager's ARN.
    let other_arn = create_ok(
        &router,
        "not found",
        &json!({
            "Name": "other",
            "ConfigurationDefinitions": [
                {
                    "Type": "AWSQuickSetupType-DHMC",
                    "Parameters": {
                        "TargetOrganizationalUnits": TARGET_OU,
                        "TargetRegions": TARGET_REGIONS,
                        "TargetType": "*",
                    },
                },
            ],
        }),
    )
    .await;

    let cases: Vec<(&str, String, String)> = vec![
        (
            "a definition id this manager does not own",
            manager_arn.clone(),
            "zzzzzzzz".to_string(),
        ),
        (
            "a definition id of the OTHER manager",
            other_arn.clone(),
            ids[0].clone(),
        ),
    ];

    for (case, arn, id) in cases {
        let (status, answer) =
            update_definition(&router, &arn, &id, &json!({ "TypeVersion": "9" })).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    let after = get_ok(&router, "not found", &manager_arn).await;
    assert_eq!(
        after.get("ConfigurationDefinitions"),
        before.get("ConfigurationDefinitions"),
        "a refused update must write nothing, got {after:?}"
    );

    // A pair whose MANAGER ARN names no stored manager is the handler's own modelled
    // not-found: the manager is resolved first, so the definition id it carries is never
    // looked at.
    let (status, answer) = update_definition(
        &router,
        UNKNOWN_MANAGER_ARN,
        &ids[0],
        &json!({ "TypeVersion": "9" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a manager ARN no create minted\": expected 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case \"a manager ARN no create minted\": expected ResourceNotFoundException, got \
         {answer:?}"
    );
    let expected = format!("Quick setup manager with arn: {UNKNOWN_MANAGER_ARN} could not be found.");
    assert_eq!(
        answer.get("Message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case \"a manager ARN no create minted\": expected {expected:?}, got {answer:?}"
    );
}

/// An update the model or the service cannot accept is refused with a 400
/// `ValidationException`, and the stored definition survives it unchanged.
#[tokio::test]
async fn an_update_the_service_cannot_accept_is_refused_and_writes_nothing() {
    let router = emulator();
    let manager_arn = create_ok(&router, "refusals", &baseline()).await;
    let before = get_ok(&router, "refusals", &manager_arn).await;
    let ids = definition_ids("refusals", &before);
    let target = ids[0].clone();

    // (case, the id to address, the update body)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            // `ConfigurationParametersMap` bounds each key and value but not the map's
            // size, so the service's own bound is what refuses a set that would leave
            // the definition past it — the two stored entries the update does not name
            // included.
            "a parameter set leaving the definition one entry past the bound",
            target.clone(),
            json!({ "Parameters": parameters(MAX_PARAMETERS - 1) }),
        ),
        (
            // A definition is deployed where its parameters say, so a parameter set
            // naming neither the accounts nor the organizational units to deploy to
            // names nowhere — including when the update is STRIPPING the target the
            // stored definition has by writing its key with no value.
            "a parameter set stripping the stored deployment target",
            target.clone(),
            json!({ "Parameters": { "TargetOrganizationalUnits": "" } }),
        ),
        (
            "a parameter set whose only target keys are written empty",
            target.clone(),
            json!({ "Parameters": {
                "TargetAccounts": "",
                "TargetOrganizationalUnits": " ",
                "TargetRegions": "us-east-1",
            } }),
        ),
        (
            // `IAMRoleArn` carries `@arnReference(AWS::IAM::Role)` and no pattern, so
            // the service refuses a value that is not an IAM role ARN.
            "an administration role that is not an ARN",
            target.clone(),
            json!({ "LocalDeploymentAdministrationRoleArn": "QuickSetupAdministration" }),
        ),
        (
            "an administration role ARN of another service",
            target.clone(),
            json!({
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:ssm-quicksetup:us-west-2:000000000000:role/Admin",
            }),
        ),
        (
            // Outside the modelled `^\d{1,3}(\.\d{1,3})?$|^LATEST$` pattern.
            "a type version with four digits",
            target.clone(),
            json!({ "TypeVersion": "1234" }),
        ),
        (
            "a type version with three components",
            target.clone(),
            json!({ "TypeVersion": "1.2.3" }),
        ),
        (
            "a type version that is not a number",
            target.clone(),
            json!({ "TypeVersion": "latest" }),
        ),
        (
            // Inside that pattern, but no version the catalog publishes for this
            // definition's type: it names no revision of that type's template.
            "a type version the definition's type does not publish",
            target.clone(),
            json!({ "TypeVersion": "99.99" }),
        ),
        (
            // The widest form the pattern admits is still only a form.
            "a type version at the pattern's widest form",
            target.clone(),
            json!({ "TypeVersion": "999.999" }),
        ),
        (
            // Published for `AWSQuickSetupType-CFGRecording`, not for this type: each
            // type's template is versioned independently.
            "a type version another type publishes",
            target.clone(),
            json!({ "TypeVersion": "1.2" }),
        ),
        (
            // Outside the modelled `^[\w+=,.@-]{1,64}$` pattern: one past its upper
            // bound, and a character it does not admit.
            "an execution role name one past the pattern's upper bound",
            target.clone(),
            json!({ "LocalDeploymentExecutionRoleName": "r".repeat(65) }),
        ),
        (
            "an execution role name with a space",
            target.clone(),
            json!({ "LocalDeploymentExecutionRoleName": "execution role" }),
        ),
        (
            // Outside the modelled `^[a-z0-9-]{1,20}$` id pattern.
            "an id in upper case",
            "ABC".to_string(),
            json!({ "TypeVersion": "9" }),
        ),
        (
            "an id one past the pattern's upper bound",
            "a".repeat(21),
            json!({ "TypeVersion": "9" }),
        ),
    ];

    for (case, id, body) in cases {
        let (status, answer) = update_definition(&router, &manager_arn, &id, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    let after = get_ok(&router, "refusals", &manager_arn).await;
    assert_eq!(
        after.get("ConfigurationDefinitions"),
        before.get("ConfigurationDefinitions"),
        "a refused update must write nothing, got {after:?}"
    );
}

/// An update submits the definition for deployment again, so the manager and the
/// configuration it changed report the transient `INITIALIZING` status first and settle
/// into `SUCCEEDED` once the span the clock owes has passed — with no further call.
#[tokio::test]
async fn an_update_redeploys_the_definition_and_the_deployment_settles() {
    let case = "redeploy";
    let router = emulator();
    let manager_arn = create_ok(&router, case, &baseline()).await;
    let before = get_ok(&router, case, &manager_arn).await;
    let target = definition_ids(case, &before)[0].clone();

    // Let the create's own deployment settle first, so what the update does is
    // observable.
    tokio::time::sleep(past_transition()).await;
    let settled = get_ok(&router, case, &manager_arn).await;
    assert_eq!(
        deployment_status(case, &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "case {case}: the create's deployment must settle first, got {settled:?}"
    );

    update_definition_ok(
        &router,
        case,
        &manager_arn,
        &target,
        &json!({ "Parameters": {
            "TargetOrganizationalUnits": TARGET_OU,
            "TargetRegions": TARGET_REGIONS,
            "TargetType": "instance",
        } }),
    )
    .await;

    let fresh = get_ok(&router, case, &manager_arn).await;
    assert_eq!(
        deployment_status(case, &fresh["StatusSummaries"]),
        STATUS_INITIALIZING,
        "case {case}: a caller reading straight after the update must see the transient status, \
         got {fresh:?}"
    );
    let fresh_rows = configuration_rows(case, &router, &manager_arn).await;
    let fresh_row = fresh_rows
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&target))
        .unwrap_or_else(|| panic!("case {case}: expected a row for {target}, got {fresh_rows:?}"));
    assert_eq!(
        deployment_status(case, &fresh_row["StatusSummaries"]),
        STATUS_INITIALIZING,
        "case {case}: the re-deployed configuration must report the transient status, got {fresh_row:?}"
    );

    tokio::time::sleep(past_transition()).await;

    let resettled = get_ok(&router, case, &manager_arn).await;
    assert_eq!(
        deployment_status(case, &resettled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "case {case}: the re-deployment must settle without any other call, got {resettled:?}"
    );
    let resettled_rows = configuration_rows(case, &router, &manager_arn).await;
    let resettled_row = resettled_rows
        .iter()
        .find(|row| row.get("ConfigurationDefinitionId").and_then(Value::as_str) == Some(&target))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a row for {target}, got {resettled_rows:?}")
        });
    assert_eq!(
        deployment_status(case, &resettled_row["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "case {case}: the configuration's re-deployment must settle too, got {resettled_row:?}"
    );
}
