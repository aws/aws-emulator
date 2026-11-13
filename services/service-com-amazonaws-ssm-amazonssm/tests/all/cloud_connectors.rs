// SPDX-License-Identifier: CC0-1.0

//! Cloud connectors end to end: `CreateCloudConnector` mints one,
//! `UpdateCloudConnector` changes it, `GetCloudConnector` reports what is held and
//! `ListCloudConnectors` reports the ones the Region holds.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service
//! router. Nothing reaches into the state struct, so a response that is not computed
//! from persisted state cannot pass: every read follows a write.
//!
//! The suites are tables. The rule under test is that an update replaces the members it
//! CARRIES and leaves every member it omits as it was, and that an update of a
//! `CloudConnectorId` the account holds no connector for at the endpoint's Region is
//! refused with `ResourceNotFoundException` and writes nothing. Each row is one input
//! along the axis that generalises over — which members the update carries, how many,
//! their size at the modelled bounds, the form and the subscription count of a
//! configuration, the Region the connector is held at, and how many callers ask at once
//! — rather than the single update a recorded unit happened to send.
//!
//! The listing suites pin a second rule: a listing reports the connectors the ENDPOINT's
//! Region holds, every member of each summary read out of what the writes persisted,
//! narrowed to the connectors meeting every filter the request carries and bounded by the
//! page the request asked for — with the leftover connector handed back as the token the
//! next page resumes at. Their rows differ along how many connectors are held, which Region
//! holds them, which filter keys and values narrow them, how large a page is asked for and
//! where the asked-for page sits against the published bounds.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{
    call, call_in_region, error_code, error_message, router, ACCOUNT, REGION,
};

/// The sentence `ResourceNotFoundException` carries for a `CloudConnectorId` the account
/// holds no cloud connector for
/// (`crate::state::cloud_connectors::CLOUD_CONNECTOR_NOT_FOUND`): fixed text, naming
/// the resource class and interpolating nothing from the request.
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_ssm_amazonssm::state::cloud_connectors::CLOUD_CONNECTOR_NOT_FOUND;

/// How many cloud connectors the store holds at once
/// (`crate::state::cloud_connectors::MAX_CLOUD_CONNECTORS`): the emulator's own ceiling
/// on the one map a create grows, not a quota AWS publishes.
const MAX_CLOUD_CONNECTORS: usize =
    service_com_amazonaws_ssm_amazonssm::state::cloud_connectors::MAX_CLOUD_CONNECTORS;

/// The published ceiling of `DisplayName`
/// (`smithy_query(query="shape", shape="DisplayName")`: `length 1..256`).
const DISPLAY_NAME_MAX: usize = 256;

/// The published floor of the same member.
const DISPLAY_NAME_MIN: usize = 1;

/// The published ceiling of `CloudConnectorDescription`
/// (`smithy_query(query="shape", shape="CloudConnectorDescription")`: `length 0..1024`).
const DESCRIPTION_MAX: usize = 1024;

/// An Azure tenant id: `AzureConfiguration$TenantId` is a hex UUID
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const TENANT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// See [`TENANT_ID`]: `AzureConfiguration$ApplicationId` has the same shape.
const APPLICATION_ID: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";

/// The tenant an update's replacement configuration names instead.
const OTHER_TENANT_ID: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// See [`OTHER_TENANT_ID`].
const OTHER_APPLICATION_ID: &str = "01234567-89ab-cdef-0123-456789abcdef";

/// A third tenant, which no connector of the filter population targets: same UUID shape.
const THIRD_TENANT_ID: &str = "22222222-3333-4444-5555-666666666666";

/// The three of [`SUBSCRIPTION_IDS`], named one by one so a filter table can name the
/// subscription a row searches for in a constant.
const SUBSCRIPTION_A: &str = SUBSCRIPTION_IDS[0];

/// See [`SUBSCRIPTION_A`].
const SUBSCRIPTION_B: &str = SUBSCRIPTION_IDS[1];

/// See [`SUBSCRIPTION_A`].
const SUBSCRIPTION_C: &str = SUBSCRIPTION_IDS[2];

/// `AzureSubscription$Id` values, same UUID shape again.
const SUBSCRIPTION_IDS: &[&str] = &[
    "aaaaaaa1-0000-0000-0000-000000000001",
    "aaaaaaa2-0000-0000-0000-000000000002",
    "aaaaaaa3-0000-0000-0000-000000000003",
];

/// A `CloudConnectorConfiguration`: the `AzureConfiguration` member of the modelled
/// union, targeting `subscriptions` of them (none at all when the list is empty, which
/// is a configuration that carries no `Targets` member).
fn azure_configuration(tenant: &str, application: &str, subscriptions: &[&str]) -> Value {
    let mut azure = json!({
        "TenantId": tenant,
        "TenantDisplayName": "Contoso",
        "ApplicationId": application,
        "ApplicationDisplayName": "Systems Manager connector",
    });
    if !subscriptions.is_empty() {
        azure["Targets"] = json!({
            "Subscriptions": subscriptions
                .iter()
                .map(|id| json!({ "Id": id, "DisplayName": format!("subscription {id}") }))
                .collect::<Vec<Value>>(),
        });
    }
    json!({ "AzureConfiguration": azure })
}

/// The IAM role a connector "uses to communicate with the third-party cloud
/// environment", in the form `CloudConnectorIamRoleArn` models.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The Config connector ARN a create associates the cloud connector with, in the form
/// `ConfigConnectorArn` models.
fn config_connector_arn(region: &str, name: &str) -> String {
    format!("arn:aws:config:{region}:{ACCOUNT}:connector/{name}")
}

/// The ARN the service must mint for the connector `id` names in `region`
/// (`CloudConnectorArn`).
fn cloud_connector_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:cloud-connector/{id}")
}

/// The create body every case starts from, at `region`'s endpoint.
fn create_body(region: &str) -> Value {
    json!({
        "DisplayName": "Azure production tenant",
        "Description": "the connector every case starts from",
        "RoleArn": role_arn("SsmCloudConnectorRole"),
        "ConfigConnectorArn": config_connector_arn(region, "production"),
        "Configuration": azure_configuration(TENANT_ID, APPLICATION_ID, &SUBSCRIPTION_IDS[..1]),
        "Tags": [{ "Key": "Team", "Value": "platform" }],
    })
}

/// `CreateCloudConnector` at `region`'s endpoint, asserting it succeeded, and returning
/// the `CloudConnectorId` it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, region: &str, body: &Value) -> String {
    let (status, response) = call_in_region(router, region, "CreateCloudConnector", body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateCloudConnector must succeed, got {status} {response}"
    );
    let id = response["CloudConnectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a CloudConnectorId, got {response}"))
        .to_string();
    assert_eq!(
        id.len(),
        36,
        "case {case}: a CloudConnectorId is 36 characters, got {id:?}"
    );
    id
}

/// `UpdateCloudConnector` with `body`, returning `(status, body)`.
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "UpdateCloudConnector", body).await
}

/// `GetCloudConnector` of `id` at `region`'s endpoint, returning `(status, body)`.
async fn get_in_region(router: &MultiServiceRouter, region: &str, id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetCloudConnector",
        &json!({ "CloudConnectorId": id }),
    )
    .await
}

/// `GetCloudConnector` of `id`, asserting the read succeeded, and returning its body.
async fn get_ok(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, response) = get_in_region(router, REGION, id).await;
    assert_eq!(
        status, 200,
        "case {case}: GetCloudConnector {id} must succeed, got {status} {response}"
    );
    response
}

/// Assert `response` is the refusal an absent cloud connector carries: HTTP 400,
/// `ResourceNotFoundException`, and the one sentence this service words for it.
fn assert_not_found(case: &str, status: u16, response: &Value) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case}: expected 400 ResourceNotFoundException, got {status} {response}"
    );
    assert_eq!(
        response["Message"].as_str(),
        Some(NOT_FOUND_MESSAGE),
        "case {case}: expected the message {NOT_FOUND_MESSAGE:?}, got {response}"
    );
}

/// One row of the update table: which members the update carries.
#[derive(Debug)]
struct UpdateCase {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The `DisplayName` the update carries, or `None` when it omits it.
    display_name: Option<String>,
    /// The `Description` the update carries, or `None` when it omits it.
    description: Option<String>,
    /// The `Configuration` the update carries, or `None` when it omits it.
    configuration: Option<Value>,
}

impl UpdateCase {
    /// The update body for this row, carrying the id and only the members it names.
    fn body(&self, id: &str) -> Value {
        let mut body = json!({ "CloudConnectorId": id });
        if let Some(display_name) = &self.display_name {
            body["DisplayName"] = json!(display_name);
        }
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(configuration) = &self.configuration {
            body["Configuration"] = configuration.clone();
        }
        body
    }
}

/// Every row of the update table: the members an update may carry, in every combination
/// and at the published bounds of their sizes, with configurations that differ in
/// tenant, application and subscription count.
fn update_cases() -> Vec<UpdateCase> {
    let replacement = azure_configuration(
        OTHER_TENANT_ID,
        OTHER_APPLICATION_ID,
        &SUBSCRIPTION_IDS[..3],
    );
    vec![
        UpdateCase {
            case: "the display name alone",
            display_name: Some("Azure staging tenant".to_string()),
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the description alone",
            display_name: None,
            description: Some("now pointed at the staging tenant".to_string()),
            configuration: None,
        },
        UpdateCase {
            case: "the configuration alone, three subscriptions",
            display_name: None,
            description: None,
            configuration: Some(replacement.clone()),
        },
        UpdateCase {
            case: "the configuration alone, no targets at all",
            display_name: None,
            description: None,
            configuration: Some(azure_configuration(
                OTHER_TENANT_ID,
                OTHER_APPLICATION_ID,
                &[],
            )),
        },
        UpdateCase {
            case: "the display name and the description",
            display_name: Some("Azure staging tenant".to_string()),
            description: Some("renamed and re-described".to_string()),
            configuration: None,
        },
        UpdateCase {
            case: "every updatable member at once",
            display_name: Some("Azure staging tenant".to_string()),
            description: Some("renamed, re-described, re-pointed".to_string()),
            configuration: Some(replacement),
        },
        UpdateCase {
            case: "no updatable member at all",
            display_name: None,
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the description emptied",
            display_name: None,
            description: Some(String::new()),
            configuration: None,
        },
        UpdateCase {
            case: "the description at its published ceiling",
            display_name: None,
            description: Some("d".repeat(DESCRIPTION_MAX)),
            configuration: None,
        },
        UpdateCase {
            case: "the display name at its published floor",
            display_name: Some("x".repeat(DISPLAY_NAME_MIN)),
            description: None,
            configuration: None,
        },
        UpdateCase {
            case: "the display name at its published ceiling",
            display_name: Some("n".repeat(DISPLAY_NAME_MAX)),
            description: None,
            configuration: None,
        },
    ]
}

/// An update replaces the members it carries and leaves every member it omits, and
/// every member that is not an update member at all, exactly as the create persisted
/// them — which a read after the write reports.
#[tokio::test]
async fn an_update_replaces_what_it_carries_and_leaves_the_rest() {
    let router = router();
    for case in update_cases() {
        let id = create_ok(&router, case.case, REGION, &create_body(REGION)).await;
        let before = get_ok(&router, case.case, &id).await;

        let (status, response) = update(&router, &case.body(&id)).await;
        assert_eq!(
            status, 200,
            "case {}: UpdateCloudConnector must succeed, got {status} {response}",
            case.case
        );
        assert_eq!(
            response["CloudConnectorId"].as_str(),
            Some(id.as_str()),
            "case {}: an update reports the id it updated, got {response}",
            case.case
        );

        let after = get_ok(&router, case.case, &id).await;
        let expected_display_name = case
            .display_name
            .clone()
            .map_or_else(|| before["DisplayName"].clone(), |name| json!(name));
        assert_eq!(
            after["DisplayName"], expected_display_name,
            "case {}: expected DisplayName {expected_display_name}, got {after}",
            case.case
        );
        let expected_description = case
            .description
            .clone()
            .map_or_else(|| before["Description"].clone(), |text| json!(text));
        assert_eq!(
            after["Description"], expected_description,
            "case {}: expected Description {expected_description}, got {after}",
            case.case
        );
        let expected_configuration = case
            .configuration
            .clone()
            .unwrap_or_else(|| before["Configuration"].clone());
        assert_eq!(
            after["Configuration"], expected_configuration,
            "case {}: expected Configuration {expected_configuration}, got {after}",
            case.case
        );

        // Nothing an update cannot carry moves: the ARN, the Region it carries, the IAM
        // role, the Config connector ARN and the creation instant are what the create
        // persisted.
        assert_eq!(
            after["CloudConnectorArn"].as_str(),
            Some(cloud_connector_arn(REGION, &id).as_str()),
            "case {}: the ARN is the connector's own, got {after}",
            case.case
        );
        for member in ["CloudConnectorArn", "RoleArn", "ConfigConnectorArn", "CreatedAt"] {
            assert_eq!(
                after[member], before[member],
                "case {}: {member} is unchanged by an update, got {after}",
                case.case
            );
        }
        let created = after["CreatedAt"].as_f64().unwrap_or_default();
        let updated = after["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated >= created,
            "case {}: UpdatedAt {updated} is at or after CreatedAt {created}, got {after}",
            case.case
        );
    }
}

/// `UpdatedAt` is when the last write to the connector ran, not when it was created:
/// after a wait longer than the timestamp's own resolution, an update moves it while
/// `CreatedAt` stays put.
#[tokio::test]
async fn an_update_stamps_when_it_ran() {
    let router = router();
    let id = create_ok(&router, "the stamped connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "before the update", &id).await;
    assert_eq!(
        before["UpdatedAt"], before["CreatedAt"],
        "a connector nothing has updated was last written when it was created, got {before}"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (status, response) = update(
        &router,
        &json!({ "CloudConnectorId": id, "DisplayName": "Stamped" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {response}"
    );

    let after = get_ok(&router, "after the update", &id).await;
    assert_eq!(
        after["CreatedAt"], before["CreatedAt"],
        "case the stamped connector: CreatedAt is unchanged by an update, got {after}"
    );
    let created = after["CreatedAt"].as_f64().unwrap_or_default();
    let updated = after["UpdatedAt"].as_f64().unwrap_or_default();
    assert!(
        updated > created,
        "case the stamped connector: UpdatedAt {updated} moved past CreatedAt {created}, \
         got {after}"
    );
}

/// An update naming a well-formed `CloudConnectorId` the account holds no connector for
/// at the endpoint's Region is refused with `ResourceNotFoundException` and the one
/// sentence this service words for it, writes nothing, and leaves the connector that IS
/// held exactly as it was.
#[tokio::test]
async fn an_update_of_an_unheld_id_is_refused_and_writes_nothing() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "the held connector", &held).await;
    // A connector of ANOTHER Region: Systems Manager is regional, so its id names no
    // connector at this endpoint.
    let elsewhere = create_ok(
        &router,
        "a connector of another Region",
        "eu-central-1",
        &create_body("eu-central-1"),
    )
    .await;
    // One nibble of the held id changed, so the id is as well formed as the held one and
    // names nothing.
    let mut near_miss: Vec<char> = held.chars().collect();
    let last = near_miss.len() - 1;
    near_miss[last] = if near_miss[last] == 'f' { '0' } else { 'f' };
    let near_miss: String = near_miss.into_iter().collect();

    let cases: Vec<(&str, String)> = vec![
        ("an all-zero id", "00000000-0000-0000-0000-000000000000".to_string()),
        ("an id nothing minted", "7f3b2a19-4c5d-4e6f-8a9b-0c1d2e3f4a5b".to_string()),
        ("the held id with one nibble changed", near_miss),
        ("a connector held at another Region", elsewhere),
    ];
    for (case, id) in cases {
        let (status, response) = update(
            &router,
            &json!({ "CloudConnectorId": id, "DisplayName": "Renamed by a stray update" }),
        )
        .await;
        assert_not_found(case, status, &response);

        // The refusal persisted nothing: the id still names no connector...
        let (read_status, read) = get_in_region(&router, REGION, &id).await;
        assert_not_found(case, read_status, &read);
        // ...and the connector that is held is untouched.
        let after = get_ok(&router, case, &held).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update leaves the held connector as it was, got {after}"
        );
    }
}

/// Five callers updating one unheld id at once all get the same refusal: the connector
/// is resolved before anything else, so a concurrent request is not reported as a
/// conflict and none of them creates anything.
#[tokio::test]
async fn concurrent_updates_of_an_unheld_id_all_get_the_same_refusal() {
    let router = router();
    let id = "4d5e6f70-1a2b-3c4d-5e6f-708192a3b4c5";
    let racer = |nth: usize| {
        let router = &router;
        let body = json!({
            "CloudConnectorId": id,
            "DisplayName": format!("racer {nth}"),
        });
        async move { update(router, &body).await }
    };
    let answers = tokio::join!(racer(0), racer(1), racer(2), racer(3), racer(4));
    for (nth, (status, response)) in [answers.0, answers.1, answers.2, answers.3, answers.4]
        .into_iter()
        .enumerate()
    {
        assert_not_found(&format!("racer {nth} of 5"), status, &response);
    }
    let (status, response) = get_in_region(&router, REGION, id).await;
    assert_not_found("the id after five racing updates", status, &response);
}

/// An update outside a MODELLED constraint — an id that is not a `CloudConnectorId`, a
/// display name or a description past its published ceiling — is refused as a
/// `ValidationException` rather than resolved, and changes nothing.
#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;
    let before = get_ok(&router, "the held connector", &held).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "an id one character short",
            json!({ "CloudConnectorId": held[..35].to_string() }),
        ),
        (
            "an id one character long",
            json!({ "CloudConnectorId": format!("{held}a") }),
        ),
        (
            "an id in upper-case hex",
            json!({ "CloudConnectorId": held.to_uppercase() }),
        ),
        (
            "an id with a non-hex character",
            json!({ "CloudConnectorId": format!("g{}", &held[1..]) }),
        ),
        (
            "an id without its hyphens",
            json!({ "CloudConnectorId": held.replace('-', "0") }),
        ),
        (
            "a display name one character past its ceiling",
            json!({
                "CloudConnectorId": held,
                "DisplayName": "n".repeat(DISPLAY_NAME_MAX + 1),
            }),
        ),
        (
            "an empty display name, one character below its floor",
            json!({ "CloudConnectorId": held, "DisplayName": "" }),
        ),
        (
            "a description one character past its ceiling",
            json!({
                "CloudConnectorId": held,
                "Description": "d".repeat(DESCRIPTION_MAX + 1),
            }),
        ),
        ("no id at all", json!({ "DisplayName": "Renamed" })),
    ];
    for (case, body) in cases {
        let (status, response) = update(&router, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {response}"
        );
        let after = get_ok(&router, case, &held).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update leaves the connector as it was, got {after}"
        );
    }
}

/// The connector store is bounded: once it holds as many connectors as the emulator
/// keeps, a further create is refused with `ServiceQuotaExceededException` and writes
/// nothing, while the connectors already held still read and update.
#[tokio::test]
async fn a_create_past_the_stores_ceiling_is_refused() {
    let router = router();
    let mut ids = Vec::new();
    for nth in 0..MAX_CLOUD_CONNECTORS {
        // Spread the connectors over two Regions, so the ceiling is shown to be on the
        // store as a whole rather than on one Region's share of it.
        let region = if nth % 2 == 0 { REGION } else { "eu-central-1" };
        ids.push((
            region,
            create_ok(&router, "filling the store", region, &create_body(region)).await,
        ));
    }

    let (status, response) =
        call_in_region(&router, REGION, "CreateCloudConnector", &create_body(REGION)).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ServiceQuotaExceededException"),
        "case one past the ceiling: expected 400 ServiceQuotaExceededException, got \
         {status} {response}"
    );
    assert!(
        response["QuotaCode"].as_str().is_some(),
        "case one past the ceiling: the refusal names the quota it reports, got {response}"
    );

    // The refusal wrote nothing, and what the store does hold is untouched: the last
    // connector created still reads, and still takes an update.
    let (region, id) = ids.last().expect("the store was filled").clone();
    let (read_status, read) = get_in_region(&router, region, &id).await;
    assert_eq!(
        read_status, 200,
        "case one past the ceiling: the connectors already held still read, got \
         {read_status} {read}"
    );
    let (update_status, updated) = call_in_region(
        &router,
        region,
        "UpdateCloudConnector",
        &json!({ "CloudConnectorId": id, "DisplayName": "Still updatable" }),
    )
    .await;
    assert_eq!(
        update_status, 200,
        "case one past the ceiling: a held connector still updates, got {update_status} \
         {updated}"
    );
}

// ---------------------------------------------------------------------------
// `ListCloudConnectors`
// ---------------------------------------------------------------------------

/// The published ceiling of `CloudConnectorMaxResults`
/// (`smithy_query(query="operation", operation="ListCloudConnectors")`: `range 0..10`).
const MAX_RESULTS_CEILING: i64 = 10;

/// The published floor of the same member, which is also the value
/// `CloudConnectorMaxResults` defaults to.
const MAX_RESULTS_FLOOR: i64 = 0;

/// The `SubscriptionId` filter value that asks for the connectors targeting no subscription
/// at all: "To return only tenant-level connectors, specify NONE"
/// (`smithy_query(query="shape", shape="CloudConnectorFilter")`, member `FilterValues`).
const TENANT_LEVEL: &str = "NONE";

/// The two `CloudConnectorFilterKey` values the checked-in model declares
/// (`smithy_query(query="shape", shape="CloudConnectorFilterKey")`).
const FILTER_KEY_TENANT_ID: &str = "TenantId";

/// See [`FILTER_KEY_TENANT_ID`].
const FILTER_KEY_SUBSCRIPTION_ID: &str = "SubscriptionId";

/// The sentence a `NextToken` this service did not issue is refused with
/// (`crate::state::INVALID_NEXT_TOKEN`).
const INVALID_NEXT_TOKEN: &str = service_com_amazonaws_ssm_amazonssm::state::INVALID_NEXT_TOKEN;

/// A create body naming `display_name` and targeting `tenant` with `subscriptions`, at
/// `region`'s endpoint. `description` is omitted when it is `None`, so a row can hold a
/// connector that carries no description at all.
fn create_body_for(
    region: &str,
    display_name: &str,
    description: Option<&str>,
    tenant: &str,
    subscriptions: &[&str],
) -> Value {
    let mut body = json!({
        "DisplayName": display_name,
        "RoleArn": role_arn("SsmCloudConnectorRole"),
        "ConfigConnectorArn": config_connector_arn(region, "production"),
        "Configuration": azure_configuration(tenant, APPLICATION_ID, subscriptions),
    });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    body
}

/// `ListCloudConnectors` with `body` at `region`'s endpoint, returning `(status, body)`.
async fn list_in_region(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "ListCloudConnectors", body).await
}

/// `ListCloudConnectors` with `body` at `region`'s endpoint, asserting it succeeded.
async fn list_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> Value {
    let (status, response) = list_in_region(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListCloudConnectors {body} must succeed, got {status} {response}"
    );
    response
}

/// The `CloudConnectorId`s a listing reports, in the order it reports them.
fn listed_ids(case: &str, response: &Value) -> Vec<String> {
    response["CloudConnectors"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a listing reports a CloudConnectors list, got {response}")
        })
        .iter()
        .map(|summary| {
            summary["CloudConnectorId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every summary carries its id, got {summary}")
                })
                .to_string()
        })
        .collect()
}

/// The summaries a listing reports.
fn listed_summaries(case: &str, response: &Value) -> Vec<Value> {
    response["CloudConnectors"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a listing reports a CloudConnectors list, got {response}")
        })
        .clone()
}

/// A listing reports the connectors the ENDPOINT's Region holds — every one of them and
/// nothing else — with every member of each summary read out of what the create persisted,
/// which is what the single-connector read reports for the same connector.
///
/// The rows differ along how many connectors the Region holds, from none at all up to more
/// than one page, while another Region holds connectors of its own throughout.
#[tokio::test]
async fn a_listing_reports_the_connectors_of_its_own_region() {
    /// The members a `CloudConnectorSummary` declares beside the id
    /// (`smithy_query(query="shape", shape="CloudConnectorSummary")`).
    const SUMMARY_MEMBERS: &[&str] =
        &["DisplayName", "Description", "RoleArn", "CreatedAt", "UpdatedAt"];

    /// The members the single-connector read declares that a summary does NOT
    /// (`CloudConnectorSummary` against `GetCloudConnector`'s output).
    const MEMBERS_A_SUMMARY_OMITS: &[&str] =
        &["CloudConnectorArn", "ConfigConnectorArn", "Configuration"];

    // How many connectors the Region holds: none, one, a handful, and more than the page a
    // request asking for no bound is served.
    for held in [0usize, 1, 2, 5, usize::try_from(MAX_RESULTS_CEILING).unwrap() + 2] {
        let case = format!("{held} connectors held");
        let router = router();

        let mut created = Vec::new();
        for nth in 0..held {
            // Each connector differs from its neighbours: its name, whether it carries a
            // description at all, and how many subscriptions it targets.
            let description = (nth % 2 == 0).then(|| format!("connector {nth} of {held}"));
            let body = create_body_for(
                REGION,
                &format!("connector {nth}"),
                description.as_deref(),
                TENANT_ID,
                &SUBSCRIPTION_IDS[..nth % (SUBSCRIPTION_IDS.len() + 1)],
            );
            created.push(create_ok(&router, &case, REGION, &body).await);
        }
        // Another Region holds two connectors of its own, which this endpoint's listing
        // must not report: Systems Manager is regional.
        let mut elsewhere = Vec::new();
        for nth in 0..2 {
            let body = create_body_for(
                "eu-central-1",
                &format!("frankfurt connector {nth}"),
                Some("held at another Region"),
                OTHER_TENANT_ID,
                &SUBSCRIPTION_IDS[..1],
            );
            elsewhere.push(create_ok(&router, &case, "eu-central-1", &body).await);
        }

        // The whole result, read a page at a time so a population past one page is covered.
        let listed = every_page(&router, &case, REGION, &json!({})).await;
        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: the listing reports every connector of this Region, in ascending \
             id order, got {listed:?}"
        );

        // Every member of every summary is what the create persisted — which is what the
        // single-connector read reports — and a summary carries nothing the shape does not
        // declare.
        let first_page = list_ok(&router, &case, REGION, &json!({})).await;
        for summary in listed_summaries(&case, &first_page) {
            let id = summary["CloudConnectorId"]
                .as_str()
                .expect("every summary carries its id");
            let read = get_ok(&router, &case, id).await;
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    summary[member], read[member],
                    "case {case}: summary {member} is what the read reports, got {summary}"
                );
            }
            for member in MEMBERS_A_SUMMARY_OMITS {
                assert!(
                    summary.get(member).is_none(),
                    "case {case}: a summary declares no {member}, got {summary}"
                );
            }
        }

        // The other Region's endpoint reports its own two, and nothing of this one's.
        let there = every_page(&router, &case, "eu-central-1", &json!({})).await;
        let mut expected_there = elsewhere.clone();
        expected_there.sort();
        assert_eq!(
            there, expected_there,
            "case {case}: another Region's listing reports its own connectors, got {there:?}"
        );

        // A listing is a read: what a later caller sees is unchanged by it.
        let again = every_page(&router, &case, REGION, &json!({})).await;
        assert_eq!(
            again, listed,
            "case {case}: a second listing reports the same connectors, got {again:?}"
        );
    }
}

/// Every connector a listing of `body` reports, walked page by page, as the ids in the
/// order the pages reported them. A page that hands back a token must be full, and the walk
/// must terminate.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> Vec<String> {
    // The page each request is served: what `MaxResults` asked for, or the published
    // ceiling when it asked for no bound (or for the zero the member defaults to).
    let page_size = usize::try_from(
        body["MaxResults"]
            .as_i64()
            .filter(|asked| *asked > 0)
            .unwrap_or(MAX_RESULTS_CEILING),
    )
    .expect("a page size is not negative");

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per connector the store can hold, plus the page that ends the walk: a walk
    // that has not finished by then is not paging at all.
    for _ in 0..=MAX_CLOUD_CONNECTORS {
        let mut asked = body.clone();
        if let Some(token) = &token {
            asked["NextToken"] = json!(token);
        }
        let response = list_ok(router, case, region, &asked).await;
        let page = listed_ids(case, &response);
        assert!(
            page.len() <= page_size,
            "case {case}: a page holds at most {page_size} connectors, got {page:?}"
        );
        seen.extend(page.iter().cloned());
        token = response["NextToken"].as_str().map(str::to_string);
        match &token {
            None => return seen,
            Some(_) => assert_eq!(
                page.len(),
                page_size,
                "case {case}: only a FULL page hands back a token, got {page:?} with \
                 {response}"
            ),
        }
    }
    panic!("case {case}: the page walk did not terminate after {seen:?}");
}

/// One row of the filter table: the filters a request carries and the connectors of the
/// population they select.
#[derive(Debug)]
struct FilterCase {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The filters the request carries, as `(key, value)`.
    filters: &'static [(&'static str, &'static str)],
    /// The connectors of the population the filters select, by the name the population
    /// holds them under.
    selects: &'static [&'static str],
}

/// A listing reports the connectors meeting EVERY filter it carries, each filter held
/// against the connector's own configuration: `TenantId` against the tenant it targets and
/// `SubscriptionId` against the subscriptions it targets, with `NONE` asking for the
/// connectors that target none.
#[tokio::test]
async fn a_listing_is_narrowed_by_every_filter_it_carries() {
    let router = router();

    // The population: (name, the tenant it targets, the subscriptions it targets). A
    // connector that targets no subscription is a tenant-level connector.
    let population: &[(&str, &str, &[&str])] = &[
        ("one subscription of the first tenant", TENANT_ID, &SUBSCRIPTION_IDS[..1]),
        ("two subscriptions of the first tenant", TENANT_ID, &SUBSCRIPTION_IDS[1..3]),
        ("tenant-level, first tenant", TENANT_ID, &[]),
        ("one subscription of the second tenant", OTHER_TENANT_ID, &SUBSCRIPTION_IDS[..1]),
        ("tenant-level, second tenant", OTHER_TENANT_ID, &[]),
    ];
    let mut held: Vec<(&str, String)> = Vec::new();
    for (name, tenant, subscriptions) in population {
        let body = create_body_for(REGION, name, Some(name), tenant, subscriptions);
        held.push((name, create_ok(&router, name, REGION, &body).await));
    }
    let id_of = |name: &str| -> String {
        held.iter()
            .find(|(held_name, _)| *held_name == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("the population holds {name}"))
    };

    let cases: &[FilterCase] = &[
        FilterCase {
            case: "no filter at all",
            filters: &[],
            selects: &[
                "one subscription of the first tenant",
                "two subscriptions of the first tenant",
                "tenant-level, first tenant",
                "one subscription of the second tenant",
                "tenant-level, second tenant",
            ],
        },
        FilterCase {
            case: "the first tenant",
            filters: &[(FILTER_KEY_TENANT_ID, TENANT_ID)],
            selects: &[
                "one subscription of the first tenant",
                "two subscriptions of the first tenant",
                "tenant-level, first tenant",
            ],
        },
        FilterCase {
            case: "the second tenant",
            filters: &[(FILTER_KEY_TENANT_ID, OTHER_TENANT_ID)],
            selects: &[
                "one subscription of the second tenant",
                "tenant-level, second tenant",
            ],
        },
        FilterCase {
            case: "a tenant nothing targets",
            filters: &[(FILTER_KEY_TENANT_ID, THIRD_TENANT_ID)],
            selects: &[],
        },
        FilterCase {
            case: "a subscription two connectors target",
            filters: &[(FILTER_KEY_SUBSCRIPTION_ID, SUBSCRIPTION_A)],
            selects: &[
                "one subscription of the first tenant",
                "one subscription of the second tenant",
            ],
        },
        FilterCase {
            case: "the first subscription of a two-subscription target",
            filters: &[(FILTER_KEY_SUBSCRIPTION_ID, SUBSCRIPTION_B)],
            selects: &["two subscriptions of the first tenant"],
        },
        FilterCase {
            case: "the second subscription of the same target",
            filters: &[(FILTER_KEY_SUBSCRIPTION_ID, SUBSCRIPTION_C)],
            selects: &["two subscriptions of the first tenant"],
        },
        FilterCase {
            case: "the tenant-level connectors",
            filters: &[(FILTER_KEY_SUBSCRIPTION_ID, TENANT_LEVEL)],
            selects: &["tenant-level, first tenant", "tenant-level, second tenant"],
        },
        FilterCase {
            case: "one tenant and one of its subscriptions",
            filters: &[
                (FILTER_KEY_TENANT_ID, TENANT_ID),
                (FILTER_KEY_SUBSCRIPTION_ID, SUBSCRIPTION_A),
            ],
            selects: &["one subscription of the first tenant"],
        },
        FilterCase {
            case: "one tenant and the tenant-level connectors",
            filters: &[
                (FILTER_KEY_TENANT_ID, OTHER_TENANT_ID),
                (FILTER_KEY_SUBSCRIPTION_ID, TENANT_LEVEL),
            ],
            selects: &["tenant-level, second tenant"],
        },
        FilterCase {
            case: "a tenant and a subscription no one connector holds together",
            filters: &[
                (FILTER_KEY_TENANT_ID, OTHER_TENANT_ID),
                (FILTER_KEY_SUBSCRIPTION_ID, SUBSCRIPTION_B),
            ],
            selects: &[],
        },
    ];

    for FilterCase { case, filters, selects } in cases {
        let mut body = json!({});
        if !filters.is_empty() {
            body["Filters"] = Value::Array(
                filters
                    .iter()
                    .map(|(key, value)| json!({ "FilterKey": key, "FilterValues": [value] }))
                    .collect(),
            );
        }
        let listed = every_page(&router, case, REGION, &body).await;
        let mut expected: Vec<String> = selects.iter().copied().map(&id_of).collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected the connectors {selects:?}, got {listed:?}"
        );
    }
}

/// A page is bounded by the `MaxResults` the request asked for, and the connector left over
/// past it is handed back as the token the next page resumes at — so walking the pages
/// reports every connector exactly once, whatever page size the walk asked for.
#[tokio::test]
async fn a_page_is_bounded_and_the_leftover_is_resumed_by_the_token() {
    // (how many connectors the Region holds, the `MaxResults` the request names)
    let cases: &[(usize, Option<i64>)] = &[
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(1)),
        (3, Some(2)),
        (5, Some(5)),
        // At the published ceiling, under it and past it.
        (12, Some(MAX_RESULTS_CEILING)),
        (12, Some(MAX_RESULTS_CEILING - 7)),
        (12, None),
        // The floor, which is the value the member defaults to: no bound was asked for.
        (12, Some(MAX_RESULTS_FLOOR)),
        (10, Some(MAX_RESULTS_CEILING)),
    ];

    for (held, max_results) in cases {
        let case = format!("{held} connectors held, MaxResults {max_results:?}");
        let router = router();
        let mut created = Vec::new();
        for nth in 0..*held {
            let body = create_body_for(
                REGION,
                &format!("connector {nth}"),
                None,
                TENANT_ID,
                &SUBSCRIPTION_IDS[..1],
            );
            created.push(create_ok(&router, &case, REGION, &body).await);
        }
        created.sort();

        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        let listed = every_page(&router, &case, REGION, &body).await;
        assert_eq!(
            listed, created,
            "case {case}: the pages together report every connector once, got {listed:?}"
        );
    }
}

/// A `MaxResults` outside the published range is refused rather than served, while the
/// bounds themselves are served.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;

    for asked in [MAX_RESULTS_CEILING + 1, MAX_RESULTS_CEILING * 10, -1] {
        let (status, response) =
            list_in_region(&router, REGION, &json!({ "MaxResults": asked })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case MaxResults {asked}: expected 400 ValidationException, got {status} \
             {response}"
        );
    }
    for asked in [MAX_RESULTS_FLOOR, 1, MAX_RESULTS_CEILING] {
        let response = list_ok(
            &router,
            &format!("MaxResults {asked}"),
            REGION,
            &json!({ "MaxResults": asked }),
        )
        .await;
        assert_eq!(
            listed_ids(&format!("MaxResults {asked}"), &response).len(),
            1,
            "case MaxResults {asked}: the one connector held is served, got {response}"
        );
    }
}

/// A `NextToken` this listing did not issue is refused, with the one sentence this service
/// words for it, rather than answered with an arbitrary page.
#[tokio::test]
async fn a_token_this_listing_did_not_issue_is_refused() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;
    // A connector of ANOTHER Region: its id is as well formed as the held one and names no
    // connector of this endpoint's result.
    let elsewhere = create_ok(
        &router,
        "a connector of another Region",
        "eu-central-1",
        &create_body("eu-central-1"),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        ("a token no listing could have issued", "not-a-token".to_string()),
        ("an id-shaped token nothing minted", "7f3b2a19-4c5d-4e6f-8a9b-0c1d2e3f4a5b".to_string()),
        ("the held id in upper-case hex", held.to_uppercase()),
        ("the held id without its hyphens", held.replace('-', "")),
        ("a connector held at another Region", elsewhere),
    ];
    for (case, token) in cases {
        let (status, response) =
            list_in_region(&router, REGION, &json!({ "NextToken": token })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            INVALID_NEXT_TOKEN,
            "case {case}: expected the message {INVALID_NEXT_TOKEN:?}, got {response}"
        );
    }

    // The empty token is the absent one: a listing resumed from nothing starts at the
    // beginning rather than being refused.
    let response = list_ok(&router, "an empty token", REGION, &json!({ "NextToken": "" })).await;
    assert_eq!(
        listed_ids("an empty token", &response),
        vec![held],
        "case an empty token: the listing starts at the beginning, got {response}"
    );
}

/// A filter that names no key, or a key with no value to search for, narrows nothing and is
/// refused — both members of `CloudConnectorFilter` are optional, so such an entry reaches
/// the handler — while a key outside the published set is refused at the boundary.
#[tokio::test]
async fn a_filter_that_cannot_narrow_the_listing_is_refused() {
    let router = router();
    let held = create_ok(&router, "the held connector", REGION, &create_body(REGION)).await;

    // (case, the Filters the request carries, the message the refusal states, if this
    // service words it itself)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "a filter carrying neither member",
            json!([{}]),
            Some("A Filters entry carries no filter key to search on".to_string()),
        ),
        (
            "a filter carrying only values",
            json!([{ "FilterValues": [TENANT_ID] }]),
            Some("A Filters entry carries no filter key to search on".to_string()),
        ),
        (
            "a filter carrying only a key",
            json!([{ "FilterKey": FILTER_KEY_TENANT_ID }]),
            Some(format!(
                "Filter key '{FILTER_KEY_TENANT_ID}' carries no value to search for"
            )),
        ),
        (
            "a second filter carrying only a key",
            json!([
                { "FilterKey": FILTER_KEY_TENANT_ID, "FilterValues": [TENANT_ID] },
                { "FilterKey": FILTER_KEY_SUBSCRIPTION_ID },
            ]),
            Some(format!(
                "Filter key '{FILTER_KEY_SUBSCRIPTION_ID}' carries no value to search for"
            )),
        ),
        // Refused at the boundary, which words its own clause: a key outside the published
        // set, and a value list outside the one-value length the model declares.
        (
            "a key outside the published set",
            json!([{ "FilterKey": "NotARealFilterKey", "FilterValues": [TENANT_ID] }]),
            None,
        ),
        (
            "a filter carrying no value in its list",
            json!([{ "FilterKey": FILTER_KEY_TENANT_ID, "FilterValues": [] }]),
            None,
        ),
        (
            "more filters than the list admits",
            json!([
                { "FilterKey": FILTER_KEY_TENANT_ID, "FilterValues": [TENANT_ID] },
                { "FilterKey": FILTER_KEY_SUBSCRIPTION_ID, "FilterValues": [SUBSCRIPTION_IDS[0]] },
                { "FilterKey": FILTER_KEY_TENANT_ID, "FilterValues": [OTHER_TENANT_ID] },
            ]),
            None,
        ),
    ];
    for (case, filters, message) in cases {
        let (status, response) =
            list_in_region(&router, REGION, &json!({ "Filters": filters })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {response}"
        );
        if let Some(message) = message {
            assert_eq!(
                error_message(&response),
                message,
                "case {case}: expected the message {message:?}, got {response}"
            );
        }
    }

    // Every refusal above left the store as it was: the connector held still lists.
    let response = list_ok(&router, "after the refusals", REGION, &json!({})).await;
    assert_eq!(
        listed_ids("after the refusals", &response),
        vec![held],
        "case after the refusals: the connector held still lists, got {response}"
    );
}

/// A listing reports what the WRITES persisted: a connector an update renamed and
/// re-described is reported with the members the update replaced, and one no update touched
/// is reported as the create left it.
#[tokio::test]
async fn a_listing_reports_what_an_update_replaced() {
    let router = router();
    let updated = create_ok(&router, "the updated connector", REGION, &create_body(REGION)).await;
    let untouched = create_ok(
        &router,
        "the untouched connector",
        REGION,
        &create_body_for(
            REGION,
            "left alone",
            Some("never updated"),
            TENANT_ID,
            &SUBSCRIPTION_IDS[..2],
        ),
    )
    .await;
    let before = list_ok(&router, "before the update", REGION, &json!({})).await;

    let (status, response) = update(
        &router,
        &json!({
            "CloudConnectorId": updated,
            "DisplayName": "Renamed before the listing",
            "Description": "re-described before the listing",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {response}"
    );

    let after = list_ok(&router, "after the update", REGION, &json!({})).await;
    for summary in listed_summaries("after the update", &after) {
        let id = summary["CloudConnectorId"]
            .as_str()
            .expect("every summary carries its id");
        let read = get_ok(&router, "after the update", id).await;
        if id == updated {
            assert_eq!(
                (summary["DisplayName"].as_str(), summary["Description"].as_str()),
                (
                    Some("Renamed before the listing"),
                    Some("re-described before the listing")
                ),
                "case the updated connector: the listing reports what the update \
                 replaced, got {summary}"
            );
        } else {
            assert_eq!(
                id, untouched,
                "case the untouched connector: the listing reports the two connectors \
                 held, got {after}"
            );
            let was = listed_summaries("before the update", &before)
                .into_iter()
                .find(|was| was["CloudConnectorId"].as_str() == Some(id))
                .unwrap_or_else(|| panic!("the connector {id} was listed before the update"));
            assert_eq!(
                summary, was,
                "case the untouched connector: a summary no update touched is unchanged, \
                 got {summary}"
            );
        }
        assert_eq!(
            summary["UpdatedAt"], read["UpdatedAt"],
            "case {id}: the summary reports the connector's own UpdatedAt, got {summary}"
        );
    }
}
