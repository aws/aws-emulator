// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the configuration-manager slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the persisted
//! configuration-manager and configuration records, so whatever a create or an update
//! stored is what a later Get or List reports — including the ids the service assigned
//! — and anything never stored (or since deleted) is refused with the modelled 404,
//! whether the ARN naming it came in the request body or as a route's path label
//! (`tests/all/target_authorization.rs`).
//!
//! This file no longer carries a stub census: every operation it used to assert a 501
//! for is implemented and covered by its own suite — `ListTagsForResource` in
//! `tests/all/list_tags_for_resource.rs`, `TagResource` and `UntagResource` in
//! `tests/all/resource_tags.rs`, and the service settings and type catalog in
//! `tests/all/service_settings_and_types.rs`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::ops::types::{
    STATUS_TYPE_VALUES, STATUS_VALUES,
};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    COMMON_PARAMETER_KEYS, MAX_FILTERS, MAX_FILTER_VALUES, MAX_ITEMS_LIMIT,
    QUICK_SETUP_TYPE_CATALOG, STATUS_INITIALIZING, STATUS_SUCCEEDED, STATUS_TYPE_DEPLOYMENT,
    TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The common parameters of a stored parameter map: the entries a summary reports as
/// its `FirstClassParameters`, taken from the set the crate publishes so this file
/// writes no second copy of it. The subset rule itself is covered by
/// `tests/all/first_class_parameters.rs`.
fn common_parameters(parameters: &Value) -> Value {
    let mut common = Map::new();
    for key in COMMON_PARAMETER_KEYS {
        if let Some(value) = parameters.get(*key) {
            common.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(common)
}

/// The region every request below signs for; every ARN must carry exactly this one.
const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create
/// ever minted, for the not-found cases.
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

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes
/// on and the handlers read the region from.
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

async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/configurationManager", Some(body)).await
}

async fn get(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn update(router: &MultiServiceRouter, manager_arn: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/configurationManager/{}", label(manager_arn)),
        Some(body),
    )
    .await
}

async fn delete(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

async fn list_managers(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurationManagers", Some(body)).await
}

async fn list_configurations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listConfigurations", Some(body)).await
}

/// The token to resume a listing at, or `None` once it is exhausted.
///
/// Every page of a listing carries `NextToken`: the empty string is what says there is
/// no further page, so a paging loop stops on it rather than on a missing member. The
/// wire form of the member is pinned in its own right by
/// `tests/all/pagination_tokens.rs`.
fn next_page_token(case: &str, answer: &Value) -> Option<String> {
    let token = answer
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a NextToken, got {answer:?}"));
    (!token.is_empty()).then(|| token.to_string())
}

/// Create a manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string();
    assert_manager_arn(case, &arn);
    arn
}

/// The minimum a create needs: one definition whose `Type` satisfies the modelled
/// `^[a-zA-Z0-9_\-.:/]{3,200}$` pattern and whose `Parameters` provide what a
/// definition of that type must.
///
/// A definition that names neither the accounts nor the organizational units to deploy
/// to is refused, and so is one that names no Regions to deploy into, or one that leaves
/// out a parameter its own type's documented table marks `(Required)`; no case here is
/// about any of those rules, so the organizational unit, [`REGION`] — the Region every
/// request below signs for — and each of that type's own required parameters are added
/// wherever the row's own `parameters` leave them out, which leaves the accounts and the
/// Regions the definition fans out to exactly what those parameters name.
fn definition(type_: &str, parameters: Value) -> Value {
    let mut parameters = parameters.as_object().cloned().unwrap_or_default();
    parameters
        .entry("TargetOrganizationalUnits".to_string())
        .or_insert_with(|| json!("ou-1"));
    parameters
        .entry("TargetRegions".to_string())
        .or_insert_with(|| json!(REGION));
    let parameters = super::type_parameter_fixtures::with_required_parameters(
        type_,
        Value::Object(parameters),
    );
    json!({ "Type": type_, "Parameters": parameters })
}

/// The `TypeVersion` the stored definition (and every configuration deployed from it)
/// must report for `submitted`: the version the request named, or — when it named none
/// — the newest version the catalog publishes for that type, which is the version the
/// service deploys and therefore the one a caller reads back.
fn expected_type_version(case: &str, submitted: &Value) -> Value {
    if let Some(named) = submitted.get("TypeVersion") {
        return named.clone();
    }
    let type_ = submitted
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: a submitted definition carries its Type, got {submitted:?}")
        });
    let entry = QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == type_)
        .unwrap_or_else(|| panic!("case {case}: {type_:?} must be a published type"));
    json!(entry.latest_version())
}

/// Read back the manager, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = get(router, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

/// Every `ManagerArn` this service mints matches the modelled pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<8-4-4-4-12 hex>$`
/// — the one `smithy_query(query="operation",operation="GetConfigurationManager")`
/// reports on its `ManagerArn` label — in the signing region and the emulator account.
fn assert_manager_arn(case: &str, arn: &str) {
    let prefix = format!("arn:aws:ssm-quicksetup:{REGION}:{ACCOUNT}:configuration-manager/");
    let uuid = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an ARN under {prefix}, got {arn:?}"));
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.len(),
        5,
        "case {case}: expected a 5-group uuid resource segment, got {uuid:?}"
    );
    for (group, width) in groups.iter().zip([8, 4, 4, 4, 12]) {
        assert!(
            group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit()),
            "case {case}: expected a {width}-digit hex group, got {group:?} in {uuid:?}"
        );
    }
    // The service mints version-4 uuids: the third group starts with the version
    // nibble and the fourth with the variant nibble.
    assert!(
        groups[2].starts_with('4') && groups[3].starts_with('8'),
        "case {case}: expected a version-4 uuid, got {uuid:?}"
    );
}

/// A configuration-definition id satisfies the modelled `^[a-z0-9-]{1,20}$` pattern
/// that `UpdateConfigurationDefinition` binds on its `{Id}` label.
fn assert_definition_id(case: &str, id: &str) {
    assert!(
        (1..=20).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "case {case}: expected an id matching ^[a-z0-9-]{{1,20}}$, got {id:?}"
    );
}

/// Every status summary reports values from the modelled `StatusType` / `Status`
/// enums (the generated `STATUS_TYPE_VALUES` / `STATUS_VALUES` catalogs) and carries
/// the `@required` `LastUpdatedAt`.
fn assert_status_summaries(case: &str, summaries: &Value) {
    let summaries = summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summaries:?}"));
    assert!(
        !summaries.is_empty(),
        "case {case}: expected at least one status summary"
    );
    for summary in summaries {
        let status_type = summary
            .get("StatusType")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: StatusType is required, got {summary:?}"));
        assert!(
            STATUS_TYPE_VALUES.contains(&status_type),
            "case {case}: StatusType {status_type:?} is not in the modelled catalog {STATUS_TYPE_VALUES:?}"
        );
        let status = summary
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a Status, got {summary:?}"));
        assert!(
            STATUS_VALUES.contains(&status),
            "case {case}: Status {status:?} is not in the modelled catalog {STATUS_VALUES:?}"
        );
        assert!(
            summary.get("LastUpdatedAt").is_some(),
            "case {case}: LastUpdatedAt is required, got {summary:?}"
        );
    }
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
        .unwrap_or_else(|| panic!("case {case}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {summaries:?}"))
        .to_string()
}

/// A modelled timestamp member as seconds since the epoch, so two stamps can be
/// ordered.
///
/// The member carries `@timestampFormat("date-time")`, so the only accepted rendering is
/// the RFC 3339 string; `tests/all/date_time_timestamps.rs` pins that wire form in its own
/// right, and reading it here keeps this helper about the ORDERING of the stamps.
fn epoch_seconds(case: &str, value: &Value, member: &str) -> f64 {
    let stamp = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case}: expected {member}, got {value:?}"));
    match stamp {
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| panic!("case {case}: {member} {text:?} is not a date-time: {e}"))
        .as_secs_f64(),
        other => panic!(
            "case {case}: {member} must be an RFC 3339 date-time string, got {other:?}"
        ),
    }
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
                .unwrap_or_else(|| panic!("case {case}: a stored definition must carry its service-assigned Id, got {definition:?}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The round trip: a create is readable back exactly as it was stored.
// ---------------------------------------------------------------------------

/// A create followed by a Get reports everything that was stored, whatever the
/// request's shape: one definition or several, with or without a name, a description,
/// tags, a type version or the local-deployment role members. Each row differs along
/// what the rule generalises over — the number of definitions, which optional members
/// are present, and the size of the parameter map.
#[tokio::test]
async fn a_created_manager_is_read_back_exactly_as_it_was_stored() {
    let router = emulator();
    // Every definition id the service assigns must be new, across managers too.
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one definition, named, described and tagged",
            json!({
                "Name": "demo",
                "Description": "bootstrap",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({
                        "DelegatedAccountId": "111122223333",
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": "us-east-1",
                    }),
                )],
                "Tags": { "env": "dev" },
            }),
        ),
        (
            "three definitions, no name, no description, no tags",
            json!({
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                    definition("AWSQuickSetupType-PatchPolicy", json!({ "RateControlErrorThreshold": "10%" })),
                    definition("AWSQuickSetupType-DHMC", json!({ "TargetType": "*" })),
                ],
            }),
        ),
        (
            "a definition carrying every optional member",
            json!({
                "Name": "local.deployment-1",
                "Description": "",
                "ConfigurationDefinitions": [{
                    "Type": "AWSQuickSetupType-ResourceExplorer",
                    "Parameters": {
                        "SelectedAggregatorRegion": "us-west-2",
                        "ReplaceExistingAggregator": "false",
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": "us-west-2",
                    },
                    "TypeVersion": "1.1",
                    "LocalDeploymentExecutionRoleName": "QuickSetupExecRole",
                    "LocalDeploymentAdministrationRoleArn":
                        "arn:aws:iam::000000000000:role/QuickSetupAdminRole",
                }],
                "Tags": { "env": "prod", "owner": "platform" },
            }),
        ),
        (
            "a definition with no parameter beyond its deployment target",
            json!({
                "Name": "empty-params",
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
    ];

    for (case, request) in cases {
        let manager_arn = create_ok(&router, case, &request).await;
        let manager = get_ok(&router, case, &manager_arn).await;

        assert_eq!(
            manager.get("ManagerArn").and_then(Value::as_str),
            Some(manager_arn.as_str()),
            "case {case}: the Get must report the ARN it was asked for, got {manager:?}"
        );
        for member in ["Name", "Description"] {
            assert_eq!(
                manager.get(member),
                request.get(member),
                "case {case}: {member} must come back as it was stored, got {manager:?}"
            );
        }
        assert_eq!(
            manager.get("Tags").cloned().unwrap_or_else(|| json!({})),
            request.get("Tags").cloned().unwrap_or_else(|| json!({})),
            "case {case}: Tags must come back as they were stored, got {manager:?}"
        );

        let created = epoch_seconds(case, &manager, "CreatedAt");
        let modified = epoch_seconds(case, &manager, "LastModifiedAt");
        assert!(
            modified >= created,
            "case {case}: LastModifiedAt {modified} must not precede CreatedAt {created}"
        );
        assert_status_summaries(
            case,
            manager
                .get("StatusSummaries")
                .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {manager:?}")),
        );

        let wanted = request
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .expect("the request carries definitions");
        let stored = manager
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"));
        assert_eq!(
            stored.len(),
            wanted.len(),
            "case {case}: expected one stored definition per submitted one, got {stored:?}"
        );
        for (index, (want, got)) in wanted.iter().zip(stored).enumerate() {
            for member in [
                "Type",
                "Parameters",
                "LocalDeploymentExecutionRoleName",
                "LocalDeploymentAdministrationRoleArn",
            ] {
                assert_eq!(
                    got.get(member),
                    want.get(member),
                    "case {case}: definition {index}'s {member} must come back as stored, got {got:?}"
                );
            }
            let version = expected_type_version(case, want);
            assert_eq!(
                got.get("TypeVersion"),
                Some(&version),
                "case {case}: definition {index} must report the published version it was \
                 deployed at ({version}), got {got:?}"
            );
            let id = got
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: definition {index} must carry a service-assigned Id, got {got:?}"));
            assert_definition_id(case, id);
            assert!(
                seen_ids.insert(id.to_string()),
                "case {case}: definition id {id:?} was assigned twice"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListConfigurationManagers
// ---------------------------------------------------------------------------

/// Paging visits every stored manager exactly once, for any store size and any page
/// bound: each page is at most `MaxItems` long, `NextToken` names a resume point only
/// while more remain (and is the empty string otherwise), and the union of the pages is
/// exactly the set of created ARNs.
#[tokio::test]
async fn paging_the_manager_list_visits_each_manager_exactly_once() {
    // (managers in the store, MaxItems asked for) — the page bound includes both
    // published ends of the modelled `@range(1, 100)`.
    let cases: [(usize, Option<i64>); 6] = [
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (3, None),
        (4, Some(i64::from(MAX_ITEMS_LIMIT))),
        (0, Some(1)),
    ];

    for (manager_count, max_items) in cases {
        let case = format!("{manager_count} managers, MaxItems={max_items:?}");
        let router = emulator();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..manager_count {
            created.insert(
                create_ok(
                    &router,
                    &case,
                    &json!({
                        "Name": format!("manager-{index}"),
                        "ConfigurationDefinitions": [definition(
                            "AWSQuickSetupType-SSMOpsCenter",
                            json!({ "TargetRegions": "us-east-1" }),
                        )],
                    }),
                )
                .await,
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = Map::new();
            if let Some(max_items) = max_items {
                body.insert("MaxItems".to_string(), json!(max_items));
            }
            if let Some(token) = &token {
                body.insert("StartingToken".to_string(), json!(token));
            }
            let (status, answer) = list_managers(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
            );
            let page = answer
                .get("ConfigurationManagersList")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"));
            if let Some(max_items) = max_items {
                assert!(
                    page.len() <= max_items as usize,
                    "case {case}: a page may not exceed MaxItems, got {} entries",
                    page.len()
                );
            }
            for summary in page {
                let arn = summary
                    .get("ManagerArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: ManagerArn is required, got {summary:?}"));
                assert!(
                    summary.get("ConfigurationDefinitionSummaries").is_some(),
                    "case {case}: expected ConfigurationDefinitionSummaries, got {summary:?}"
                );
                assert_status_summaries(
                    &case,
                    summary
                        .get("StatusSummaries")
                        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {summary:?}")),
                );
                seen.push(arn.to_string());
            }
            pages += 1;
            assert!(
                pages <= manager_count + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
            match next_page_token(&case, &answer) {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(
            seen.len(),
            manager_count,
            "case {case}: expected each manager exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case}: the pages must cover exactly the created managers, got {seen:?}"
        );
    }
}

/// A `MaxItems` outside the modelled `@range(1, 100)` is refused; both published
/// bounds are accepted.
#[tokio::test]
async fn the_page_bound_is_accepted_at_its_published_ends_and_refused_outside_them() {
    let router = emulator();
    create_ok(
        &router,
        "range",
        &json!({
            "Name": "range",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;

    for (max_items, want_status) in [
        (0, 400),
        (1, 200),
        (i64::from(MAX_ITEMS_LIMIT), 200),
        (i64::from(MAX_ITEMS_LIMIT) + 1, 400),
    ] {
        let (status, answer) = list_managers(&router, &json!({ "MaxItems": max_items })).await;
        assert_eq!(
            status, want_status,
            "case MaxItems={max_items}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case MaxItems={max_items}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

/// A `StartingToken` this service never issued is refused rather than silently read
/// as "start from the beginning" — including a token the OTHER listing issued, whose
/// key names a record of a different kind.
#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    for (case, uri) in [
        ("managers", "/listConfigurationManagers"),
        ("configurations", "/listConfigurations"),
    ] {
        for token in ["not-a-token", "6e6f742d612d746f6b656e"] {
            let (status, answer) =
                call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
            assert_eq!(
                status, 400,
                "case {case} / {token}: a forged StartingToken must be refused, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case} / {token}: expected ValidationException, got {answer:?}"
            );
        }
    }

    // Two managers, each with two definitions, so both listings hand out a token.
    for name in ["alpha", "beta"] {
        create_ok(
            &router,
            "cross-listing token",
            &json!({
                "Name": name,
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                ],
            }),
        )
        .await;
    }

    let (status, answer) = list_managers(&router, &json!({ "MaxItems": 1 })).await;
    assert_eq!(status, 200, "got {answer:?}");
    let manager_token = answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a manager page token, got {answer:?}"))
        .to_string();
    let (status, answer) = list_configurations(&router, &json!({ "MaxItems": 1 })).await;
    assert_eq!(status, 200, "got {answer:?}");
    let configuration_token = answer["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a configuration page token, got {answer:?}"))
        .to_string();

    // (case, the listing to hand the token to, the token the other listing issued)
    for (case, uri, token) in [
        (
            "a manager token on the configuration listing",
            "/listConfigurations",
            manager_token.clone(),
        ),
        (
            "a configuration token on the manager listing",
            "/listConfigurationManagers",
            configuration_token.clone(),
        ),
    ] {
        let (status, answer) =
            call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
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

    // And each listing still accepts its OWN token.
    for (case, uri, token) in [
        (
            "the manager listing's own token",
            "/listConfigurationManagers",
            manager_token,
        ),
        (
            "the configuration listing's own token",
            "/listConfigurations",
            configuration_token,
        ),
    ] {
        let (status, answer) =
            call(&router, "POST", uri, Some(&json!({ "StartingToken": token }))).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} / {answer:?}"
        );
    }
}

/// A `Filters` key that names no field of the listed shape is refused, and a key that
/// does narrows the page to the records whose value is one of the filter's `Values`.
#[tokio::test]
async fn a_filter_narrows_the_listing_and_an_unknown_filter_key_is_refused() {
    let router = emulator();
    let first = create_ok(
        &router,
        "filters",
        &json!({
            "Name": "alpha",
            "Description": "first",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let second = create_ok(
        &router,
        "filters",
        &json!({
            "Name": "beta",
            "Description": "second",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-Scheduler",
                json!({ "TargetTagKey": "Env" }),
            )],
        }),
    )
    .await;

    // (filter, the ARNs the listing must report)
    let cases: Vec<(Value, Vec<String>)> = vec![
        (
            json!([{ "Key": "Name", "Values": ["alpha"] }]),
            vec![first.clone()],
        ),
        (
            json!([{ "Key": "Name", "Values": ["beta", "gamma"] }]),
            vec![second.clone()],
        ),
        (
            json!([{ "Key": "Description", "Values": ["second"] }]),
            vec![second.clone()],
        ),
        (
            json!([{ "Key": "Name", "Values": ["alpha"] }, { "Key": "Description", "Values": ["second"] }]),
            Vec::new(),
        ),
        (json!([{ "Key": "Name", "Values": ["nobody"] }]), Vec::new()),
    ];
    for (filters, want) in cases {
        let (status, answer) = list_managers(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {filters:?}: the listing must succeed, got {status} / {answer:?}"
        );
        let got: Vec<String> = answer["ConfigurationManagersList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {filters:?}: expected a list, got {answer:?}"))
            .iter()
            .map(|summary| summary["ManagerArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got,
            want.clone(),
            "case {filters:?}: expected {want:?}, got {got:?}"
        );
    }

    // `ManagerArn` is not a filter key on either listing — a filter value cannot even
    // carry one, since the modelled `FilterValues` pattern refuses `:`.
    for (case, uri) in [
        ("managers", "/listConfigurationManagers"),
        ("configurations", "/listConfigurations"),
    ] {
        let (status, answer) = call(
            &router,
            "POST",
            uri,
            Some(&json!({ "Filters": [{ "Key": "ManagerArn", "Values": ["alpha"] }] })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an unknown filter key must be refused, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    // The filter list and each filter's values are caller-sized and the model gives
    // them no length, so the service's own bounds are accepted at their ends and
    // refused one past them.
    let at_most = |count: usize| {
        Value::Array(
            (0..count)
                .map(|_| json!({ "Key": "Name", "Values": ["alpha"] }))
                .collect(),
        )
    };
    let many_values = |count: usize| {
        json!([{
            "Key": "Name",
            "Values": Value::Array((0..count).map(|index| json!(format!("alpha-{index}"))).collect()),
        }])
    };
    for (case, filters, want_status) in [
        ("filters at the cap", at_most(MAX_FILTERS), 200),
        ("one filter too many", at_most(MAX_FILTERS + 1), 400),
        ("values at the cap", many_values(MAX_FILTER_VALUES), 200),
        ("one value too many", many_values(MAX_FILTER_VALUES + 1), 400),
    ] {
        let (status, answer) = list_managers(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            status, want_status,
            "case {case}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListConfigurations
// ---------------------------------------------------------------------------

/// Each manager's create produced one configuration record per account and Region the
/// definition targets, and `ListConfigurations` reports exactly those — narrowed by
/// the `ManagerArn` and `ConfigurationDefinitionId` the request names, and by a
/// `Region` filter that sees the deployment Regions, not the signing one.
#[tokio::test]
async fn the_configuration_listing_reports_one_record_per_deployed_target() {
    let router = emulator();
    let first = create_ok(
        &router,
        "configurations",
        &json!({
            "Name": "two-definitions",
            "ConfigurationDefinitions": [
                definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                {
                    "Type": "AWSQuickSetupType-Scheduler",
                    "Parameters": {
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": REGION,
                        "TargetTagKey": "Env",
                        "TargetTagValue": "production",
                        "ICalendarString": "BEGIN:VCALENDAR",
                    },
                    "TypeVersion": "2.0",
                },
            ],
        }),
    )
    .await;
    let second = create_ok(
        &router,
        "configurations",
        &json!({
            "Name": "one-definition",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-DHMC",
                json!({ "TargetType": "*" }),
            )],
        }),
    )
    .await;

    let first_ids = definition_ids(
        "configurations",
        &get_ok(&router, "configurations", &first).await,
    );
    let second_ids = definition_ids(
        "configurations",
        &get_ok(&router, "configurations", &second).await,
    );
    assert_eq!(first_ids.len(), 2, "got {first_ids:?}");

    // (case, request, the (ManagerArn, ConfigurationDefinitionId, Region) rows it must
    // report) — the first definition targets us-east-1, the other two target the Region
    // the requests are signed for.
    type ListingCase = (&'static str, Value, Vec<(String, String, String)>);
    let targeted = "us-east-1".to_string();
    let local = REGION.to_string();
    let cases: Vec<ListingCase> = vec![
        (
            "every configuration",
            json!({}),
            vec![
                (first.clone(), first_ids[0].clone(), targeted.clone()),
                (first.clone(), first_ids[1].clone(), local.clone()),
                (second.clone(), second_ids[0].clone(), local.clone()),
            ],
        ),
        (
            "one manager",
            json!({ "ManagerArn": first }),
            vec![
                (first.clone(), first_ids[0].clone(), targeted.clone()),
                (first.clone(), first_ids[1].clone(), local.clone()),
            ],
        ),
        (
            "one definition of one manager",
            json!({ "ManagerArn": first, "ConfigurationDefinitionId": first_ids[1] }),
            vec![(first.clone(), first_ids[1].clone(), local.clone())],
        ),
        (
            "a filter on the configuration type",
            json!({ "Filters": [{ "Key": "Type", "Values": ["AWSQuickSetupType-DHMC"] }] }),
            vec![(second.clone(), second_ids[0].clone(), local.clone())],
        ),
        (
            "a filter on the targeted deployment region",
            json!({ "Filters": [{ "Key": "Region", "Values": [targeted] }] }),
            vec![(first.clone(), first_ids[0].clone(), "us-east-1".to_string())],
        ),
        (
            "a filter on the manager's own region",
            json!({ "Filters": [{ "Key": "Region", "Values": [REGION] }] }),
            vec![
                (first.clone(), first_ids[1].clone(), local.clone()),
                (second.clone(), second_ids[0].clone(), local.clone()),
            ],
        ),
    ];

    for (case, request, want) in cases {
        let (status, answer) = list_configurations(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
        );
        let rows = answer
            .get("ConfigurationsList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"));
        let mut got: Vec<(String, String, String)> = rows
            .iter()
            .map(|row| {
                (
                    row["ManagerArn"].as_str().unwrap_or_default().to_string(),
                    row["ConfigurationDefinitionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    row["Region"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        got.sort();
        let mut want_sorted = want.clone();
        want_sorted.sort();
        assert_eq!(
            got, want_sorted,
            "case {case}: expected {want_sorted:?}, got {got:?}"
        );

        for row in rows {
            assert_eq!(
                row.get("Account").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case}: a definition naming no target account is deployed in the \
                 account this emulator serves, got {row:?}"
            );
            let id = row
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a configuration Id, got {row:?}"));
            assert_ne!(
                Some(id),
                row.get("ConfigurationDefinitionId").and_then(Value::as_str),
                "case {case}: a configuration's own id is not its definition id, got {row:?}"
            );
            assert!(
                row.get("CreatedAt").is_some(),
                "case {case}: expected CreatedAt, got {row:?}"
            );
            assert_status_summaries(
                case,
                row.get("StatusSummaries")
                    .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {row:?}")),
            );
        }
    }

    // A resource this endpoint does not hold is absent, not an empty page — for either
    // identifier this listing accepts, including a definition id held by a manager other
    // than the one the same request names.
    for (case, request) in [
        (
            "a manager that was never created",
            json!({ "ManagerArn": UNKNOWN_MANAGER_ARN }),
        ),
        (
            "a definition id that manager does not own",
            json!({ "ManagerArn": second, "ConfigurationDefinitionId": first_ids[0] }),
        ),
    ] {
        let (status, answer) = list_configurations(&router, &request).await;
        assert_eq!(
            status, 404,
            "case {case}: {request:?} must answer 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}

/// The configuration listing pages the same way the manager listing does.
#[tokio::test]
async fn paging_the_configuration_list_visits_each_configuration_exactly_once() {
    let router = emulator();
    create_ok(
        &router,
        "paging configurations",
        &json!({
            "Name": "five-definitions",
            "ConfigurationDefinitions": (0..5)
                .map(|index| definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": format!("us-east-{}", index + 1) }),
                ))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    for max_items in [1, 2, 5] {
        let case = format!("MaxItems={max_items}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = Map::new();
            body.insert("MaxItems".to_string(), json!(max_items));
            if let Some(token) = &token {
                body.insert("StartingToken".to_string(), json!(token));
            }
            let (status, answer) = list_configurations(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
            );
            let page = answer["ConfigurationsList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {answer:?}"))
                .clone();
            assert!(
                page.len() <= max_items,
                "case {case}: a page may not exceed MaxItems, got {} entries",
                page.len()
            );
            for row in &page {
                seen.push(row["Id"].as_str().unwrap_or_default().to_string());
            }
            match next_page_token(&case, &answer) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            5,
            "case {case}: expected every configuration exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>().len(),
            5,
            "case {case}: a configuration was served twice, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateConfigurationManager
// ---------------------------------------------------------------------------

/// An update mutates the stored record: the members it carries replace the stored
/// ones, the members it omits are left as they were, and `LastModifiedAt` moves
/// forward when the update stored a new value — all of which the next Get reports.
#[tokio::test]
async fn an_update_mutates_the_stored_record_and_bumps_the_modified_stamp() {
    // (case, the update body, the Name and Description the Get must then report)
    let cases: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "name only",
            json!({ "Name": "demo-renamed" }),
            Some("demo-renamed"),
            Some("first"),
        ),
        (
            "description only",
            json!({ "Description": "rewritten" }),
            Some("demo"),
            Some("rewritten"),
        ),
        (
            "both members",
            json!({ "Name": "demo-2", "Description": "second" }),
            Some("demo-2"),
            Some("second"),
        ),
        ("neither member", json!({}), Some("demo"), Some("first")),
        (
            // Both members carry the values the create stored, so this update asks for
            // no change at all.
            "the stored values again",
            json!({ "Name": "demo", "Description": "first" }),
            Some("demo"),
            Some("first"),
        ),
    ];

    for (case, body, want_name, want_description) in cases {
        let router = emulator();
        let manager_arn = create_ok(
            &router,
            case,
            &json!({
                "Name": "demo",
                "Description": "first",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        )
        .await;
        let before = get_ok(&router, case, &manager_arn).await;
        let created = epoch_seconds(case, &before, "CreatedAt");
        let modified_before = epoch_seconds(case, &before, "LastModifiedAt");
        let definitions_before = before.get("ConfigurationDefinitions").cloned();

        // So the bumped stamp is observably later than the one the create wrote.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, answer) = update(&router, &manager_arn, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateConfigurationManager must succeed, got {status} / {answer:?}"
        );

        let after = get_ok(&router, case, &manager_arn).await;
        assert_eq!(
            after.get("Name").and_then(Value::as_str),
            want_name,
            "case {case}: expected Name {want_name:?}, got {after:?}"
        );
        assert_eq!(
            after.get("Description").and_then(Value::as_str),
            want_description,
            "case {case}: expected Description {want_description:?}, got {after:?}"
        );
        assert_eq!(
            after.get("ConfigurationDefinitions").cloned(),
            definitions_before,
            "case {case}: an update must not disturb the configuration definitions, got {after:?}"
        );
        assert_eq!(
            epoch_seconds(case, &after, "CreatedAt"),
            created,
            "case {case}: an update must not move CreatedAt, got {after:?}"
        );
        let modified_after = epoch_seconds(case, &after, "LastModifiedAt");
        // The stamp reports when the record was last CHANGED: it moves for a row whose
        // update stored a new value, and stays put for one that stored none — the
        // `@idempotent` trait's own case.
        let stored_a_new_value = want_name != before.get("Name").and_then(Value::as_str)
            || want_description != before.get("Description").and_then(Value::as_str);
        if stored_a_new_value {
            assert!(
                modified_after > modified_before,
                "case {case}: expected LastModifiedAt to move past {modified_before}, got {modified_after}"
            );
        } else {
            assert_eq!(
                modified_after, modified_before,
                "case {case}: an update that stored no new value must not move \
                 LastModifiedAt, expected {modified_before}, got {modified_after}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DeleteConfigurationManager
// ---------------------------------------------------------------------------

/// Once the teardown a delete starts has come due, the manager and the configurations
/// it owned are gone: the Get is a 404, neither listing reports it, and a second delete
/// is a 404 too. The manager that was not deleted is untouched. The phases of the
/// teardown itself are in `tests/all/configuration_manager_teardown.rs`.
#[tokio::test]
async fn a_delete_removes_the_manager_and_the_configurations_it_owned() {
    let router = emulator();
    let doomed = create_ok(
        &router,
        "delete",
        &json!({
            "Name": "doomed",
            "ConfigurationDefinitions": [
                definition("AWSQuickSetupType-SSMOpsCenter", json!({ "TargetRegions": "us-east-1" })),
                definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
            ],
        }),
    )
    .await;
    let survivor = create_ok(
        &router,
        "delete",
        &json!({
            "Name": "survivor",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-DHMC",
                json!({ "TargetType": "*" }),
            )],
        }),
    )
    .await;

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        answer["ConfigurationsList"].as_array().map(Vec::len),
        Some(3),
        "three configurations were created, got {answer:?}"
    );

    let (status, answer) = delete(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );

    // Removing the deployed resources is work the service does after it has answered, so
    // the records wind down first and are removed when that teardown comes due: wait for
    // that condition, not for a duration (`tests/all/target_authorization.rs` owns the one
    // wait).
    super::target_authorization::poll_until_absent(&router, "a deleted manager", &doomed).await;

    let (status, answer) = get(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "a deleted manager names no resource this endpoint serves, got {status} / {answer:?}"
    );
    assert_eq!(error_code(&answer), "ResourceNotFoundException", "{answer:?}");

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    let listed: Vec<String> = answer["ConfigurationManagersList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list, got {answer:?}"))
        .iter()
        .map(|summary| summary["ManagerArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "only the surviving manager may be listed, got {listed:?}"
    );

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    let owners: Vec<String> = answer["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list, got {answer:?}"))
        .iter()
        .map(|row| row["ManagerArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        owners,
        vec![survivor.clone()],
        "a deleted manager's configurations go with it, got {owners:?}"
    );

    // Deleting it again names a manager the store no longer holds, so the route's own
    // label names no resource at all: the modelled not-found.
    let (status, answer) = delete(&router, &doomed).await;
    assert_eq!(
        status, 404,
        "a repeated delete must answer 404, got {status} / {answer:?}"
    );
    assert_eq!(error_code(&answer), "ResourceNotFoundException", "{answer:?}");

    // And the survivor is still readable, unchanged.
    let survivor_record = get_ok(&router, "delete", &survivor).await;
    assert_eq!(
        survivor_record.get("Name").and_then(Value::as_str),
        Some("survivor"),
        "got {survivor_record:?}"
    );
}

/// An ARN no create ever minted is the modelled 404 on every operation whose route
/// carries it as a path label: each handler resolves the ARN it was given
/// (`tests/all/target_authorization.rs`).
#[tokio::test]
async fn an_unknown_manager_arn_is_a_404_on_every_labelled_operation() {
    let router = emulator();
    let uri = format!("/configurationManager/{}", label(UNKNOWN_MANAGER_ARN));
    let cases: [(&str, &str, Option<Value>); 3] = [
        ("GET", uri.as_str(), None),
        ("PUT", uri.as_str(), Some(json!({ "Name": "renamed" }))),
        ("DELETE", uri.as_str(), None),
    ];
    let expected =
        format!("Quick setup manager with arn: {UNKNOWN_MANAGER_ARN} could not be found.");
    for (method, uri, body) in cases {
        let (status, answer) = call(&router, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 404,
            "case {method}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {method}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.get("Message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {method}: expected {expected:?}, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A create the model or the service cannot accept is refused with a 400
/// `ValidationException`, and nothing is stored for it.
#[tokio::test]
async fn a_create_the_service_cannot_accept_is_refused_and_stores_nothing() {
    let router = emulator();
    let cases: Vec<(&str, Value)> = vec![
        (
            "no ConfigurationDefinitions member at all",
            json!({ "Name": "demo" }),
        ),
        (
            "an empty ConfigurationDefinitions list",
            json!({ "Name": "demo", "ConfigurationDefinitions": [] }),
        ),
        (
            "a Type shorter than the modelled 3 characters",
            json!({ "ConfigurationDefinitions": [definition("ab", json!({ "K": "v" }))] }),
        ),
        (
            "a Type carrying a character the modelled pattern refuses",
            json!({ "ConfigurationDefinitions": [definition("bad type", json!({ "K": "v" }))] }),
        ),
        (
            "a definition with no Parameters member",
            json!({ "ConfigurationDefinitions": [{ "Type": "AWSQuickSetupType-SSM" }] }),
        ),
        (
            "a Name longer than the modelled 120 characters",
            json!({
                "Name": "n".repeat(121),
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer) = create(&router, &body).await;
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

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        answer["ConfigurationManagersList"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed deployment transition
// ---------------------------------------------------------------------------

/// `CreateConfigurationManager` starts a deployment, so the manager and its
/// configurations are reported in the transient `INITIALIZING` status first and settle
/// into `SUCCEEDED` once the span the clock owes has passed — with no further call,
/// and the same answer from the Get and from both listings.
#[tokio::test]
async fn the_deployment_status_settles_once_the_clock_owes_it() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "settle",
        &json!({
            "Name": "settling",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;

    let fresh = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        deployment_status("fresh", &fresh["StatusSummaries"]),
        STATUS_INITIALIZING,
        "a caller reading straight after the create must still see the transient status, got {fresh:?}"
    );

    tokio::time::sleep(past_transition()).await;

    let settled = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        deployment_status("settled", &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "the deployment must settle without any other call, got {settled:?}"
    );

    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        deployment_status(
            "listed manager",
            &answer["ConfigurationManagersList"][0]["StatusSummaries"]
        ),
        STATUS_SUCCEEDED,
        "the listing must report the settled status, got {answer:?}"
    );

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {answer:?}");
    assert_eq!(
        deployment_status(
            "listed configuration",
            &answer["ConfigurationsList"][0]["StatusSummaries"]
        ),
        STATUS_SUCCEEDED,
        "the configuration listing must report the settled status, got {answer:?}"
    );

    // A read reports the settled status; it does not move it anywhere else.
    let again = get_ok(&router, "settle", &manager_arn).await;
    assert_eq!(
        again["StatusSummaries"], settled["StatusSummaries"],
        "a second read must report the same summaries, got {again:?}"
    );
}

// This file used to carry a stub census — a test asserting that the operations this
// slice left for their own issues still answered the scaffold's 501. Every operation of
// this service is now implemented (no `//! SCAFFOLDED, NOT IMPLEMENTED:` marker is left
// under `src/ops/`), so the census has nothing to assert and is gone: the last entry it
// held, `PUT /tags/{ResourceArn}` (`TagResource`), is implemented and covered end to end
// by `tests/all/resource_tags.rs`, including the refusal an ARN naming no resource earns.

// ---------------------------------------------------------------------------
// GetConfiguration
// ---------------------------------------------------------------------------

/// `GET /getConfiguration/{ConfigurationId}`, the id carried as one `@httpLabel`.
async fn get_configuration(router: &MultiServiceRouter, configuration_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(configuration_id)),
        None,
    )
    .await
}

/// Every `ConfigurationSummary` the listing reports for one manager, in listing order.
async fn configurations_of(
    router: &MultiServiceRouter,
    case: &str,
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

/// The rule: `GetConfiguration` reports the stored configuration its `ConfigurationId`
/// names — the record the create minted for one configuration definition — so every
/// member comes back as it was stored, and as the configuration listing reports it.
///
/// Each row differs along what the rule generalises over: how many definitions the
/// create carried, how large each parameter map is (including an empty one), whether a
/// `TypeVersion` was given, and which Quick Setup type was deployed.
#[tokio::test]
async fn a_stored_configuration_is_read_back_by_its_service_generated_id() {
    let router = emulator();
    // Every configuration id the service hands out must be new, across managers too.
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one definition, one parameter",
            json!({
                "Name": "single",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMOpsCenter",
                    json!({ "TargetRegions": "us-east-1" }),
                )],
            }),
        ),
        (
            "three definitions, differing types, one with a TypeVersion",
            json!({
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-Scheduler", json!({ "TargetTagKey": "Env" })),
                    {
                        "Type": "AWSQuickSetupType-PatchPolicy",
                        "Parameters": {
                            "RateControlErrorThreshold": "10%",
                            "PatchPolicyName": "patch-policy",
                            "SelectedPatchBaselines": "[{\"baselineId\":\"AWS-DefaultPatchBaseline\"}]",
                            "TargetOrganizationalUnits": "ou-1",
                            "TargetRegions": "us-east-1",
                            "TargetType": "*",
                        },
                        "TypeVersion": "2.0",
                    },
                    definition("AWSQuickSetupType-DHMC", json!({ "TargetType": "*" })),
                ],
            }),
        ),
        (
            "a definition with no parameter beyond its deployment target",
            json!({
                "Name": "empty-params",
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
        (
            "a definition carrying every optional member",
            json!({
                "Name": "fully-specified",
                "ConfigurationDefinitions": [{
                    "Type": "AWSQuickSetupType-ResourceExplorer",
                    "Parameters": {
                        "SelectedAggregatorRegion": "us-west-2",
                        "ReplaceExistingAggregator": "false",
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": "us-west-2",
                    },
                    "TypeVersion": "1.1",
                    "LocalDeploymentExecutionRoleName": "QuickSetupExecRole",
                    "LocalDeploymentAdministrationRoleArn":
                        "arn:aws:iam::000000000000:role/QuickSetupAdminRole",
                }],
                "Tags": { "env": "prod" },
            }),
        ),
    ];

    for (case, request) in cases {
        let manager_arn = create_ok(&router, case, &request).await;
        let submitted = request
            .get("ConfigurationDefinitions")
            .and_then(Value::as_array)
            .expect("the request carries definitions");
        // The definition ids the service assigned, in the order the create submitted
        // them, so each configuration can be matched to the definition it deploys.
        let ids = definition_ids(case, &get_ok(&router, case, &manager_arn).await);
        let listed = configurations_of(&router, case, &manager_arn).await;
        assert_eq!(
            listed.len(),
            submitted.len(),
            "case {case}: every definition here targets exactly one account and Region, so \
             one configuration each was deployed, got {listed:?}"
        );

        for summary in &listed {
            let id = summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected an Id, got {summary:?}"));
            let definition_id = summary
                .get("ConfigurationDefinitionId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a ConfigurationDefinitionId, got {summary:?}")
                });
            let index = ids
                .iter()
                .position(|assigned| assigned == definition_id)
                .unwrap_or_else(|| {
                    panic!("case {case}: {definition_id:?} is not one of this manager's definition ids {ids:?}")
                });
            let want = &submitted[index];

            let (status, got) = get_configuration(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case}: GetConfiguration({id}) must succeed, got {status} / {got:?}"
            );
            assert_eq!(
                got.get("Id").and_then(Value::as_str),
                Some(id),
                "case {case}: the Get must report the id it was asked for, got {got:?}"
            );
            assert_eq!(
                got.get("ManagerArn").and_then(Value::as_str),
                Some(manager_arn.as_str()),
                "case {case}: the configuration must name the manager that owns it, got {got:?}"
            );
            assert_eq!(
                got.get("ConfigurationDefinitionId").and_then(Value::as_str),
                Some(definition_id),
                "case {case}: the Get must name the definition it deploys, got {got:?}"
            );
            assert_eq!(
                got.get("Type"),
                want.get("Type"),
                "case {case}: Type must come back as it was stored, got {got:?}"
            );
            let version = expected_type_version(case, want);
            assert_eq!(
                got.get("TypeVersion"),
                Some(&version),
                "case {case}: the deployed configuration must report the published version it \
                 was deployed at ({version}), got {got:?}"
            );
            // The create supplied the parameters; `GetConfiguration$Parameters` is that
            // whole map, while the listing's `ConfigurationSummary$FirstClassParameters`
            // is only its common parameters.
            let stored_parameters = got.get("Parameters").cloned().unwrap_or_else(|| json!({}));
            assert_eq!(
                stored_parameters,
                want.get("Parameters").cloned().unwrap_or_else(|| json!({})),
                "case {case}: Parameters must come back as they were stored, got {got:?}"
            );
            let common = common_parameters(&stored_parameters);
            assert_eq!(
                summary.get("FirstClassParameters"),
                Some(&common),
                "case {case}: the listing must report the common parameters {common:?} of the \
                 map the Get reports, got {summary:?}"
            );
            assert_eq!(
                got.get("Account").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case}: a definition naming no target account is deployed in the account \
                 this emulator serves, got {got:?}"
            );
            // Where the definition targets a Region, that is the Region the deployed
            // configuration reports; where it targets none, the manager's own.
            let targeted = want
                .get("Parameters")
                .and_then(|parameters| parameters.get("TargetRegions"))
                .and_then(Value::as_str)
                .unwrap_or(REGION);
            assert_eq!(
                got.get("Region").and_then(Value::as_str),
                Some(targeted),
                "case {case}: expected the deployment Region {targeted}, got {got:?}"
            );
            let created = epoch_seconds(case, &got, "CreatedAt");
            let modified = epoch_seconds(case, &got, "LastModifiedAt");
            assert!(
                modified >= created,
                "case {case}: LastModifiedAt {modified} must not precede CreatedAt {created}"
            );
            assert_eq!(
                epoch_seconds(case, summary, "CreatedAt"),
                created,
                "case {case}: the Get and the listing must report one CreatedAt, got {got:?}"
            );
            assert_status_summaries(
                case,
                got.get("StatusSummaries")
                    .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {got:?}")),
            );
            assert!(
                seen_ids.insert(id.to_string()),
                "case {case}: configuration id {id:?} was handed out twice"
            );
        }
    }
}

/// A configuration the store does not hold is the modelled 404 when the id asked for is
/// one this service could have handed out — the form
/// `crate::state::is_configuration_id` describes: one reclaimed with the manager that
/// owned it, and ids of that form no create minted. The rows differ along the digits and
/// their case, which is everything that form leaves free. An id OUTSIDE that form never
/// reaches the lookup (`tests/all/configuration_id_format.rs` pins the 400 it earns),
/// and one past the `ConfigurationId` pattern `^[a-zA-Z0-9-_/:]{1,100}$` (from
/// `smithy_query(query="operation",operation="GetConfiguration")`) is refused at the
/// modelled boundary.
#[tokio::test]
async fn a_configuration_the_store_does_not_hold_is_the_modelled_404() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "404",
        &json!({
            "Name": "to-be-deleted",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let listed = configurations_of(&router, "404", &manager_arn).await;
    let reclaimed = listed[0]["Id"]
        .as_str()
        .expect("a stored configuration carries its id")
        .to_string();

    // While its manager is stored, the configuration is readable.
    let (status, answer) = get_configuration(&router, &reclaimed).await;
    assert_eq!(
        status, 200,
        "a stored configuration must be readable, got {status} / {answer:?}"
    );

    let (status, answer) = delete(&router, &manager_arn).await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );

    // The configuration is reclaimed when the teardown its manager's delete started
    // comes due, not while it is still in flight.
    tokio::time::sleep(past_transition()).await;

    let cases: Vec<(&str, String)> = vec![
        ("reclaimed with its manager", reclaimed),
        ("digits no create minted", "0".repeat(32)),
        (
            "every hex digit",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            // Hex is case-insensitive, so this is the form too.
            "the same digits in upper case",
            "0123456789ABCDEF0123456789ABCDEF".to_string(),
        ),
    ];

    for (case, configuration_id) in cases {
        let (status, answer) = get_configuration(&router, &configuration_id).await;
        assert_eq!(
            status, 404,
            "case {case}: an unstored configuration must answer 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // One past the pattern's published maximum is a bad request, not a 404: the
    // modelled constraint is refused before the handler runs.
    let (status, answer) = get_configuration(&router, &"c".repeat(101)).await;
    assert_eq!(
        status, 400,
        "an id past the pattern's 100-character maximum must be refused, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "expected ValidationException, got {answer:?}"
    );
}

/// Reading a configuration reports the deployment status the clock owes it — the
/// transient one straight after the create, the settled one once the span has passed —
/// and reports it without moving anything: a second read answers exactly the same.
#[tokio::test]
async fn reading_a_configuration_reports_the_status_the_clock_owes_and_moves_nothing() {
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        "clock",
        &json!({
            "Name": "settling-configuration",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let listed = configurations_of(&router, "clock", &manager_arn).await;
    let id = listed[0]["Id"]
        .as_str()
        .expect("a stored configuration carries its id")
        .to_string();

    let (status, fresh) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {fresh:?}");
    assert_eq!(
        deployment_status("fresh", &fresh["StatusSummaries"]),
        STATUS_INITIALIZING,
        "a caller reading straight after the create must still see the transient status, got {fresh:?}"
    );

    tokio::time::sleep(past_transition()).await;

    let (status, settled) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {settled:?}");
    assert_eq!(
        deployment_status("settled", &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "the deployment must settle without any other call, got {settled:?}"
    );

    let (status, again) = get_configuration(&router, &id).await;
    assert_eq!(status, 200, "got {status} / {again:?}");
    assert_eq!(
        again, settled,
        "a second read must report the same record, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// A configuration definition may only name a configuration type this service offers
// ---------------------------------------------------------------------------

/// Whether the published catalog (`crate::state::QUICK_SETUP_TYPE_CATALOG`, what
/// `ListQuickSetupTypes` answers) offers `type_`.
fn is_published_type(type_: &str) -> bool {
    QUICK_SETUP_TYPE_CATALOG
        .iter()
        .any(|entry| entry.type_ == type_)
}

/// The rule: a configuration definition's `Type` selects the service-owned template
/// Quick Setup deploys, so only a type the service publishes can be named — a
/// pattern-valid value the catalog does not carry is refused with the modelled 400
/// `ValidationException` and nothing is stored for it.
///
/// Each row is a value that satisfies the modelled
/// `ConfigurationDefinitionInput$Type` pattern `^[a-zA-Z0-9_\-.:/]{3,200}$` (so the
/// boundary walk admits it and the handler is what refuses it) while naming no
/// published type; the rows differ along what the rule generalises over — the value's
/// form (a plausible Quick Setup name, a published one with a suffix, truncated,
/// re-separated, re-cased, differently prefixed) and its size, including each bound
/// the pattern publishes. Each row is sent twice: alone, and beside a published type,
/// because one unacceptable definition refuses the whole create.
#[tokio::test]
async fn a_definition_naming_an_unpublished_configuration_type_is_refused_and_stores_nothing() {
    let router = emulator();

    // A manager created with a published type, so each refusal can be shown to leave
    // the store exactly as it already was.
    let kept_type = "AWSQuickSetupType-SSMOpsCenter";
    let kept = create_ok(
        &router,
        "kept",
        &json!({
            "Name": "published-type",
            "ConfigurationDefinitions": [definition(
                kept_type,
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "a plausible but unpublished Quick Setup name",
            "AWSQuickSetupType-NotAThing".to_string(),
        ),
        (
            "the shortest value the modelled pattern admits",
            "abc".to_string(),
        ),
        (
            "the longest value the modelled pattern admits",
            "T".repeat(200),
        ),
        (
            "a published type differing only in case",
            "awsquicksetuptype-dhmc".to_string(),
        ),
        (
            "a published type carrying a suffix",
            "AWSQuickSetupType-DHMC-v2".to_string(),
        ),
        (
            "a published type truncated by one character",
            "AWSQuickSetupType-DHM".to_string(),
        ),
        (
            "a published type with its separator changed",
            "AWSQuickSetupType/DHMC".to_string(),
        ),
        (
            "a published type with surrounding punctuation",
            ".AWSQuickSetupType-SSM.".to_string(),
        ),
        (
            "a type named for another product family",
            "CustomType-Patching".to_string(),
        ),
        (
            "a type naming only the catalog's common prefix",
            "AWSQuickSetupType-".to_string(),
        ),
    ];

    for (case, type_) in &cases {
        assert!(
            !is_published_type(type_),
            "case {case}: this row must name a type outside the published catalog, got {type_:?}"
        );
        let requests: [(&str, Value); 2] = [
            (
                "alone",
                json!({
                    "Name": "unsupported",
                    "ConfigurationDefinitions": [definition(type_, json!({ "TargetRegions": "us-east-1" }))],
                }),
            ),
            (
                "beside a published type",
                json!({
                    "ConfigurationDefinitions": [
                        definition(kept_type, json!({ "TargetRegions": "us-east-1" })),
                        definition(type_, json!({})),
                    ],
                }),
            ),
        ];
        for (position, body) in requests {
            let (status, answer) = create(&router, &body).await;
            assert_eq!(
                status, 400,
                "case {case} ({position}): {type_:?} names no published type and must be refused \
                 with 400, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case} ({position}): expected ValidationException, got {answer:?}"
            );
        }
    }

    // Nothing a refused create named was stored: the only manager is the one created
    // with a published type, and every stored configuration names a published type.
    let (status, answer) = list_managers(&router, &json!({})).await;
    assert_eq!(status, 200, "got {status} / {answer:?}");
    let managers = answer["ConfigurationManagersList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ConfigurationManagersList, got {answer:?}"));
    assert_eq!(
        managers.len(),
        1,
        "a refused create must store no manager, got {answer:?}"
    );
    assert_eq!(
        managers[0]["ManagerArn"].as_str(),
        Some(kept.as_str()),
        "the surviving manager must be the one the accepted create minted, got {answer:?}"
    );

    let (status, answer) = list_configurations(&router, &json!({})).await;
    assert_eq!(status, 200, "got {status} / {answer:?}");
    let configurations = answer["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ConfigurationsList, got {answer:?}"));
    assert_eq!(
        configurations.len(),
        1,
        "a refused create must store no configuration, got {answer:?}"
    );
    for configuration in configurations {
        let type_ = configuration["Type"]
            .as_str()
            .unwrap_or_else(|| panic!("a stored configuration carries its Type, got {answer:?}"));
        assert!(
            is_published_type(type_),
            "the store must hold no configuration of an unpublished type, got {type_:?}"
        );
    }
}

/// The other half of the same rule: EVERY type the catalog publishes is accepted and
/// stored, so the accepted set is exactly the published one — not a subset of it. Each
/// row is one catalog entry, read back through `GetConfigurationManager`, the
/// configuration listing and `GetConfiguration`.
#[tokio::test]
async fn every_published_configuration_type_is_accepted_and_read_back() {
    let router = emulator();
    assert!(
        !QUICK_SETUP_TYPE_CATALOG.is_empty(),
        "the published catalog must not be empty"
    );

    for entry in QUICK_SETUP_TYPE_CATALOG {
        let case = entry.type_;
        let manager_arn = create_ok(
            &router,
            case,
            &json!({
                "Name": "published",
                "ConfigurationDefinitions": [{
                    "Type": entry.type_,
                    "TypeVersion": entry.latest_version(),
                    // Somewhere to deploy to, the Region to deploy into and every
                    // parameter this entry's own documented table marks `(Required)`:
                    // this case is about the TYPE being accepted, not about those
                    // requirements, which `tests/all/required_type_parameters.rs` covers.
                    "Parameters": super::type_parameter_fixtures::with_required_parameters(
                        entry.type_,
                        json!({
                            "TargetOrganizationalUnits": "ou-1",
                            "TargetRegions": REGION,
                        }),
                    ),
                }],
            }),
        )
        .await;

        let manager = get_ok(&router, case, &manager_arn).await;
        assert_eq!(
            manager["ConfigurationDefinitions"][0]["Type"].as_str(),
            Some(entry.type_),
            "case {case}: the stored definition must carry the type the create named, got {manager:?}"
        );

        let listed = configurations_of(&router, case, &manager_arn).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case}: expected one configuration per definition, got {listed:?}"
        );
        assert_eq!(
            listed[0]["Type"].as_str(),
            Some(entry.type_),
            "case {case}: the listed configuration must carry that type, got {:?}",
            listed[0]
        );

        let id = listed[0]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a stored configuration carries its id"));
        let (status, got) = get_configuration(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case}: GetConfiguration must succeed, got {status} / {got:?}"
        );
        assert_eq!(
            got["Type"].as_str(),
            Some(entry.type_),
            "case {case}: the read configuration must carry that type, got {got:?}"
        );
        assert_eq!(
            got["TypeVersion"].as_str(),
            Some(entry.latest_version()),
            "case {case}: the published LatestVersion must be accepted as the TypeVersion, got {got:?}"
        );
    }
}
