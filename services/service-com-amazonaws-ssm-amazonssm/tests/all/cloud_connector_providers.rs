// SPDX-License-Identifier: CC0-1.0

//! Which third-party cloud a connector connects to, and what a connector created without an
//! Amazon Web Services Config connector reports.
//!
//! `CloudConnectorConfiguration` is a union over the providers Systems Manager connects to —
//! `AzureConfiguration` and `GcpConfiguration`
//! (`smithy_query(query="shape", shape="CloudConnectorConfiguration")`) — and
//! `ConfigConnectorArn` is optional on the create, "returned only if the cloud connector was
//! created with an Amazon Web Services Config connector"
//! (`CloudConnectorSummary$ConfigConnectorArn`). The rules under test:
//!
//! 1. A create takes a configuration of EITHER provider and a read reports it back exactly
//!    as it was written, whatever the provider.
//! 2. A summary reports the `Provider` of the configuration the connector holds — `Azure`
//!    for an Azure configuration and `GCP` for a GCP one, the two values the modelled
//!    `Provider` enum declares — and the `ConfigConnectorArn` the create associated, which
//!    is absent from both reads for a connector created without one.
//! 3. A listing narrows by a property of the configuration: `TenantId` and `SubscriptionId`
//!    for Azure (with `NONE` asking for the tenant-level connectors) and `OrganizationId`
//!    and `ProjectId` for GCP — a key of one provider's family selecting no connector of the
//!    other, because one connector's configuration is of one provider.
//! 4. "You can't combine Azure filter keys (`SubscriptionId` and `TenantId`) with GCP filter
//!    keys (`ProjectId` and `OrganizationId`) in the same request"
//!    (`CloudConnectorFilter$FilterValues`): such a request is refused 400
//!    `ValidationException` and nothing is listed.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`call_in_region`], [`router`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router — so every
//! assertion is about the wire, and every read follows the write it must report.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, router, ACCOUNT, REGION};

/// An Azure tenant id: `AzureConfiguration$TenantId` is a hex UUID
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const TENANT_ID: &str = "4a4f1b31-6c2d-4f0e-9a7b-2c3d4e5f6a7b";

/// See [`TENANT_ID`]: `AzureConfiguration$ApplicationId` has the same shape.
const APPLICATION_ID: &str = "5b5f2c42-7d3e-4a1f-8b6c-3d4e5f6a7b8c";

/// `AzureSubscription$Id`, the same shape again.
const SUBSCRIPTION_ID: &str = "6c6f3d53-8e4f-4b2a-9c7d-4e5f6a7b8c9d";

/// A GCP organization id: `GcpConfiguration$OrganizationId` is a decimal id
/// (`^[1-9][0-9]{0,18}$`).
const ORGANIZATION_ID: &str = "123456789012";

/// A second one, so a filter row can name an organization no connector of the population
/// targets.
const OTHER_ORGANIZATION_ID: &str = "987654321098";

/// A GCP project id: `GcpProject$ProjectId` is `^[a-z][a-z0-9-]{4,28}[a-z0-9]$`.
const PROJECT_ID: &str = "systems-manager-project";

/// A second project, targeted by the second GCP connector.
const OTHER_PROJECT_ID: &str = "another-managed-project";

/// `GcpConfiguration$WorkloadIdentityPoolProvider`, whose pattern names the pool provider's
/// full resource name
/// (`^projects/[1-9][0-9]{0,18}/locations/global/workloadIdentityPools/[a-z0-9-]{4,32}/providers/[a-z0-9-]{4,32}$`).
const POOL_PROVIDER: &str =
    "projects/123456789012/locations/global/workloadIdentityPools/ssm-pool/providers/ssm-provider";

/// `GcpConfiguration$ServiceAccountEmail`
/// (`^[a-z][a-z0-9\-]*[a-z0-9]@[a-z][a-z0-9\-]*[a-z0-9]\.iam\.gserviceaccount\.com$`).
const SERVICE_ACCOUNT: &str = "ssm-connector@systems-manager-project.iam.gserviceaccount.com";

/// The `Provider` a connector of an Azure configuration reports
/// (`smithy_query(query="shape", shape="Provider")`: `Azure | GCP`).
const PROVIDER_AZURE: &str = "Azure";

/// The `Provider` a connector of a GCP configuration reports. The other value of that enum.
const PROVIDER_GCP: &str = "GCP";

/// The `SubscriptionId` filter value that asks for the connectors targeting no subscription:
/// "to return only tenant-level connectors, specify `NONE`"
/// (`CloudConnectorFilter$FilterValues`).
const TENANT_LEVEL: &str = "NONE";

/// The IAM role a connector "uses to communicate with the third-party cloud environment".
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The Config connector ARN a create may associate the connector with, in the form
/// `ConfigConnectorArn` models
/// (`^arn:aws(-cn|-us-gov)?:config:([^:]+):\d{12}:connector/.+$`).
fn config_connector_arn(name: &str) -> String {
    format!("arn:aws:config:{REGION}:{ACCOUNT}:connector/{name}")
}

/// An `AzureConfiguration`, targeting `subscriptions` of the tenant (none at all when the
/// list is empty, which is a tenant-level connector).
fn azure_configuration(tenant: &str, subscriptions: &[&str]) -> Value {
    let mut azure = json!({
        "TenantId": tenant,
        "ApplicationId": APPLICATION_ID,
    });
    if !subscriptions.is_empty() {
        azure["Targets"] = json!({
            "Subscriptions": subscriptions
                .iter()
                .map(|id| json!({ "Id": id }))
                .collect::<Vec<Value>>(),
        });
    }
    json!({ "AzureConfiguration": azure })
}

/// A `GcpConfiguration`, naming `organization` when one is given and targeting `projects`.
fn gcp_configuration(organization: Option<&str>, projects: &[&str]) -> Value {
    let mut gcp = json!({
        "WorkloadIdentityPoolProvider": POOL_PROVIDER,
        "ServiceAccountEmail": SERVICE_ACCOUNT,
    });
    if let Some(organization) = organization {
        gcp["OrganizationId"] = json!(organization);
    }
    if !projects.is_empty() {
        gcp["Targets"] = json!({
            "Projects": projects
                .iter()
                .map(|id| json!({ "ProjectId": id }))
                .collect::<Vec<Value>>(),
        });
    }
    json!({ "GcpConfiguration": gcp })
}

/// `CreateCloudConnector` with `body`, asserting it succeeded, and returning the
/// `CloudConnectorId` it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call_in_region(router, REGION, "CreateCloudConnector", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCloudConnector must succeed, got {status} {response}"
    );
    response["CloudConnectorId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a create reports a CloudConnectorId, got {response}")
        })
        .to_string()
}

/// `GetCloudConnector` of `id`, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, response) = call_in_region(
        router,
        REGION,
        "GetCloudConnector",
        &json!({ "CloudConnectorId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCloudConnector must succeed, got {status} {response}"
    );
    response
}

/// `ListCloudConnectors` with `body`, returning `(status, body)`.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, "ListCloudConnectors", body).await
}

/// The summary a listing reports for `id`, asserting the listing succeeded and reported it.
async fn summary_of(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, response) = list(router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCloudConnectors must succeed, got {status} {response}"
    );
    response["CloudConnectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a listing reports a list, got {response}"))
        .iter()
        .find(|summary| summary["CloudConnectorId"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report {id}, got {response}"))
        .clone()
}

/// RULE 1 and RULE 2: a create takes a configuration of either provider — with or without a
/// Config connector — a read reports the configuration back as it was written, and a summary
/// reports that configuration's provider and the Config connector ARN the create associated.
#[tokio::test]
async fn a_connector_reports_the_provider_of_the_configuration_it_holds() {
    /// One row: the configuration a create was handed, the Config connector it named, and
    /// the provider a summary must report for it.
    struct Row {
        case: &'static str,
        configuration: Value,
        /// The `ConfigConnectorArn` the create carries, or `None` for a create that names
        /// none.
        config_connector_arn: Option<String>,
        provider: &'static str,
    }

    let rows = vec![
        Row {
            case: "an Azure tenant with one subscription and a Config connector",
            configuration: azure_configuration(TENANT_ID, &[SUBSCRIPTION_ID]),
            config_connector_arn: Some(config_connector_arn("azure-production")),
            provider: PROVIDER_AZURE,
        },
        Row {
            case: "a tenant-level Azure connector with no Config connector",
            configuration: azure_configuration(TENANT_ID, &[]),
            config_connector_arn: None,
            provider: PROVIDER_AZURE,
        },
        Row {
            case: "a GCP organization with one project and a Config connector",
            configuration: gcp_configuration(Some(ORGANIZATION_ID), &[PROJECT_ID]),
            config_connector_arn: Some(config_connector_arn("gcp-production")),
            provider: PROVIDER_GCP,
        },
        Row {
            case: "a GCP connector naming neither an organization nor a project",
            configuration: gcp_configuration(None, &[]),
            config_connector_arn: None,
            provider: PROVIDER_GCP,
        },
        Row {
            case: "a GCP connector targeting two projects",
            configuration: gcp_configuration(Some(ORGANIZATION_ID), &[PROJECT_ID, OTHER_PROJECT_ID]),
            config_connector_arn: None,
            provider: PROVIDER_GCP,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = router();
        let mut body = json!({
            "DisplayName": format!("connector for {case}"),
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": row.configuration,
        });
        if let Some(arn) = &row.config_connector_arn {
            body["ConfigConnectorArn"] = json!(arn);
        }
        let id = create_ok(&router, case, &body).await;

        // RULE 1: the read reports the configuration the create was handed, member for
        // member.
        let read = get_ok(&router, case, &id).await;
        assert_eq!(
            read["Configuration"], row.configuration,
            "case {case:?}: expected the configuration the create was handed, got {read}"
        );
        // RULE 2: the Config connector ARN is on both reads for a connector created with
        // one, and absent from both for a connector created without one.
        assert_eq!(
            read["ConfigConnectorArn"].as_str(),
            row.config_connector_arn.as_deref(),
            "case {case:?}: expected ConfigConnectorArn {:?}, got {read}",
            row.config_connector_arn
        );

        let summary = summary_of(&router, case, &id).await;
        assert_eq!(
            summary["Provider"].as_str(),
            Some(row.provider),
            "case {case:?}: expected the summary to report Provider {:?}, got {summary}",
            row.provider
        );
        assert_eq!(
            summary["ConfigConnectorArn"].as_str(),
            row.config_connector_arn.as_deref(),
            "case {case:?}: expected the summary's ConfigConnectorArn {:?}, got {summary}",
            row.config_connector_arn
        );
        // A summary carries no configuration of its own: the shape does not declare one.
        assert!(
            summary.get("Configuration").is_none(),
            "case {case:?}: a summary declares no Configuration, got {summary}"
        );
    }
}

/// RULE 3: a listing narrows by a property of the configuration, and a filter key of one
/// provider's family selects no connector of the other.
#[tokio::test]
async fn a_filter_key_selects_the_connectors_of_its_own_provider() {
    let router = router();
    // The population: two Azure connectors (one targeting a subscription, one tenant-level)
    // and two GCP ones (one targeting a project under an organization, one targeting another
    // project under no organization).
    let azure_subscription = create_ok(
        &router,
        "the Azure connector targeting a subscription",
        &json!({
            "DisplayName": "azure with a subscription",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": azure_configuration(TENANT_ID, &[SUBSCRIPTION_ID]),
        }),
    )
    .await;
    let azure_tenant_level = create_ok(
        &router,
        "the tenant-level Azure connector",
        &json!({
            "DisplayName": "azure tenant level",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": azure_configuration(TENANT_ID, &[]),
        }),
    )
    .await;
    let gcp_organization = create_ok(
        &router,
        "the GCP connector under an organization",
        &json!({
            "DisplayName": "gcp with an organization",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": gcp_configuration(Some(ORGANIZATION_ID), &[PROJECT_ID]),
        }),
    )
    .await;
    let gcp_project_only = create_ok(
        &router,
        "the GCP connector with no organization",
        &json!({
            "DisplayName": "gcp without an organization",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": gcp_configuration(None, &[OTHER_PROJECT_ID]),
        }),
    )
    .await;

    // (case, the filter key, the value it searches for, the connectors it must select)
    let rows: Vec<(&str, &str, String, Vec<&String>)> = vec![
        (
            "the tenant both Azure connectors target",
            "TenantId",
            TENANT_ID.to_string(),
            vec![&azure_subscription, &azure_tenant_level],
        ),
        (
            "a tenant no connector targets",
            "TenantId",
            "99999999-9999-9999-9999-999999999999".to_string(),
            vec![],
        ),
        (
            "the subscription one Azure connector targets",
            "SubscriptionId",
            SUBSCRIPTION_ID.to_string(),
            vec![&azure_subscription],
        ),
        (
            "the tenant-level connectors, which no GCP connector is",
            "SubscriptionId",
            TENANT_LEVEL.to_string(),
            vec![&azure_tenant_level],
        ),
        (
            "the organization one GCP connector targets",
            "OrganizationId",
            ORGANIZATION_ID.to_string(),
            vec![&gcp_organization],
        ),
        (
            "an organization no connector targets",
            "OrganizationId",
            OTHER_ORGANIZATION_ID.to_string(),
            vec![],
        ),
        (
            "the project one GCP connector targets",
            "ProjectId",
            PROJECT_ID.to_string(),
            vec![&gcp_organization],
        ),
        (
            "the project the other GCP connector targets",
            "ProjectId",
            OTHER_PROJECT_ID.to_string(),
            vec![&gcp_project_only],
        ),
    ];

    for (case, key, value, expected) in &rows {
        let (status, response) = list(
            &router,
            &json!({ "Filters": [{ "FilterKey": key, "FilterValues": [value] }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing must succeed, got {status} {response}"
        );
        let mut selected: Vec<String> = response["CloudConnectors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listing reports a list, got {response}"))
            .iter()
            .map(|summary| {
                summary["CloudConnectorId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        selected.sort();
        let mut wanted: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        wanted.sort();
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected the filter {key}={value} to select {wanted:?}, got \
             {selected:?}"
        );
    }
}

/// RULE 4: a listing that narrows by an Azure filter key and a GCP one at once is refused,
/// whichever pair of keys it combines, and a listing narrowing by two keys of ONE family is
/// served.
#[tokio::test]
async fn azure_and_gcp_filter_keys_cannot_be_combined() {
    let router = router();
    let azure = create_ok(
        &router,
        "the Azure connector",
        &json!({
            "DisplayName": "azure",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": azure_configuration(TENANT_ID, &[SUBSCRIPTION_ID]),
        }),
    )
    .await;
    let gcp = create_ok(
        &router,
        "the GCP connector",
        &json!({
            "DisplayName": "gcp",
            "RoleArn": role_arn("SsmCloudConnectorRole"),
            "Configuration": gcp_configuration(Some(ORGANIZATION_ID), &[PROJECT_ID]),
        }),
    )
    .await;

    /// The sentence such a request is refused with: fixed text naming the two families.
    const NOT_COMBINABLE: &str =
        "Azure filter keys (SubscriptionId, TenantId) can't be combined with GCP filter keys \
         (ProjectId, OrganizationId) in the same request";

    // (case, the two filters the request carries)
    let refused: Vec<(&str, Value)> = vec![
        (
            "a tenant and a project",
            json!([
                { "FilterKey": "TenantId", "FilterValues": [TENANT_ID] },
                { "FilterKey": "ProjectId", "FilterValues": [PROJECT_ID] },
            ]),
        ),
        (
            "a subscription and an organization",
            json!([
                { "FilterKey": "SubscriptionId", "FilterValues": [SUBSCRIPTION_ID] },
                { "FilterKey": "OrganizationId", "FilterValues": [ORGANIZATION_ID] },
            ]),
        ),
        (
            "a GCP key first",
            json!([
                { "FilterKey": "OrganizationId", "FilterValues": [ORGANIZATION_ID] },
                { "FilterKey": "TenantId", "FilterValues": [TENANT_ID] },
            ]),
        ),
    ];

    for (case, filters) in &refused {
        let (status, response) = list(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(NOT_COMBINABLE),
            "case {case:?}: expected the refusal to state {NOT_COMBINABLE:?}, got {response}"
        );
    }

    // Two keys of ONE family are served, and select the connector meeting both.
    for (case, filters, expected) in [
        (
            "two Azure keys",
            json!([
                { "FilterKey": "TenantId", "FilterValues": [TENANT_ID] },
                { "FilterKey": "SubscriptionId", "FilterValues": [SUBSCRIPTION_ID] },
            ]),
            &azure,
        ),
        (
            "two GCP keys",
            json!([
                { "FilterKey": "OrganizationId", "FilterValues": [ORGANIZATION_ID] },
                { "FilterKey": "ProjectId", "FilterValues": [PROJECT_ID] },
            ]),
            &gcp,
        ),
    ] {
        let (status, response) = list(&router, &json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: two keys of one family are served, got {status} {response}"
        );
        let selected: Vec<&str> = response["CloudConnectors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listing reports a list, got {response}"))
            .iter()
            .map(|summary| summary["CloudConnectorId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            selected,
            vec![expected.as_str()],
            "case {case:?}: expected the connector meeting both keys, got {response}"
        );
    }
}
