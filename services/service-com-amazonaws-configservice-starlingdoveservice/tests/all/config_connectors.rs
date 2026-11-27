// SPDX-License-Identifier: CC0-1.0

//! The connector slice of AWS Config, end to end over the wire.
//!
//! Five rules are pinned here, each over a TABLE of inputs rather than one recorded
//! case:
//!
//! 1. `ListConnectors` reports exactly the connectors in state, each with the
//!    attributes the service derived at creation time (the ARN, the derived name, the
//!    provider and the tenant its configuration names), and reports nothing once a
//!    connector is deleted;
//! 2. one `ListConnectors` page carries at most `MaxResults` connectors and the
//!    `NextToken` it returns resumes the listing, so paging reports every connector
//!    exactly once at every page size;
//! 3. a filter selects the connectors whose named attribute equals one of the values
//!    the filter carries, and a filter naming anything the model does not publish is
//!    refused;
//! 4. an `Arn` that does not resolve to a connector of this account in the Region the
//!    request is scoped to is refused with ONE class and ONE text — whether the value
//!    is malformed, names a connector that was never created, or names one a previous
//!    delete removed — the same for every operation whose `Arn` names the connector it
//!    acts on;
//! 5. every member of a summary beside the connector's own identity is an identifier of
//!    ONE provider's own, so a summary reports it for exactly the connectors connected
//!    to that provider and reports none of the other provider's members.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`. The connectors these rules are pinned over are SEEDED in the
//! shared engine ([`seed_connector`]), the way the other suites of this crate seed
//! state, because `PutConnector` creates none: what it answers, and why, is pinned by
//! `connector_credential_verification.rs`.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::ops::types::{
    AzureConnectorConfiguration, ConnectorConfiguration, GcpConnectorConfiguration,
};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    self, service_state, ConnectorEntry,
};

use crate::config_setup_and_rule_loop::Emulator;

/// Every published `Provider` value, from
/// `smithy_query(query="shape", shape="Provider")`: `AZURE` and `GCP`.
const AZURE: &str = "AZURE";

/// See [`AZURE`]. The connectors the rules below are pinned over connect to Azure
/// unless a case says otherwise, so this is also the provider a filter selects nothing
/// for in those cases.
const GCP: &str = "GCP";

/// A `GcpServiceAccountEmail`, in the one form the member's `@pattern` allows
/// (`^[a-z][a-z0-9\-]*@[a-z][a-z0-9\-]*[a-z0-9]\.iam\.gserviceaccount\.com$`, from
/// `smithy_query(query="shape", shape="GcpServiceAccountEmail")`): "the format is
/// `SERVICE_ACCOUNT_NAME@PROJECT_ID.iam.gserviceaccount.com`".
pub(crate) const SERVICE_ACCOUNT: &str =
    "config-recorder@aws-config-project.iam.gserviceaccount.com";

/// A `GcpWorkloadIdentityPoolProvider`, in the one form its `@pattern` allows
/// (`^projects/[0-9]+/locations/global/workloadIdentityPools/[^/]+/providers/[^/]+$`):
/// "the format is
/// `projects/PROJECT_NUMBER/locations/global/workloadIdentityPools/POOL_ID/providers/PROVIDER_ID`".
pub(crate) const POOL_PROVIDER: &str =
    "projects/123456789012/locations/global/workloadIdentityPools/aws-config/providers/config";

/// A `GcpOrganizationId` — "the ID of the Google Cloud organization that contains the
/// connector's management project" — within the modelled length (1..20, from
/// `smithy_query(query="shape", shape="GcpOrganizationId")`).
pub(crate) const ORGANIZATION_ID: &str = "728351472933";

/// Every published `ConnectorFilterName` value, from
/// `smithy_query(query="shape", shape="ConnectorFilterName")`: `provider`.
const PROVIDER_FILTER: &str = "provider";

/// The `AzureTenantIdentifier` / `AzureClientIdentifier` length maximum, from
/// `smithy_query(query="shape", shape="AzureTenantIdentifier")`: 1..128.
pub(crate) const AZURE_IDENTIFIER_MAX_LEN: usize = 128;

/// The emulator's account id, the one every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

/// The ONE text an operation whose `Arn` names the connector answers every value that
/// does not resolve to one with, written out here rather than read from the handlers'
/// own constant so this suite PINS it instead of restating it.
const CONNECTOR_ARN_NOT_VALID_TEXT: &str = "The connector ARN provided is not valid.";

/// A `ConnectorConfiguration` naming the Azure provider.
pub(crate) fn azure_configuration(tenant: &str, client: &str) -> Value {
    json!({ "azure": { "tenantIdentifier": tenant, "clientIdentifier": client } })
}

/// A `ConnectorConfiguration` naming the Google Cloud provider, carrying the
/// organization id where the case has one: it is "required if the management project
/// belongs to an organization" and omitted otherwise, in which case "the connector can
/// record only the management project".
pub(crate) fn gcp_configuration(service_account: &str, organization: Option<&str>) -> Value {
    let mut configuration = json!({
        "serviceAccountEmail": service_account,
        "workloadIdentityPoolProvider": POOL_PROVIDER,
    });
    if let Some(organization) = organization {
        configuration["gcpOrganizationId"] = json!(organization);
    }
    json!({ "gcp": configuration })
}

/// The connector name the service derives for an Azure tenant, and the ARN it mints
/// for it in `region`: the provider it connects to and the tenant within it.
fn expected_name_and_arn(region: &str, tenant: &str) -> (String, String) {
    let name = format!("{}-{tenant}", AZURE.to_lowercase());
    (
        name.clone(),
        format!("arn:aws:config:{region}:{ACCOUNT}:connector/{name}"),
    )
}

/// Place a connector for the Azure `tenant`/`client` pair in the store, reaching it
/// the way a handler does, and answer the ARN it is keyed by.
///
/// `PutConnector` creates NO connector in this emulator: a connector IS the credential
/// Config connects to the third-party cloud service provider with, and verifying that
/// credential with the provider is an outbound connection this emulator does not make,
/// so every configuration is refused (`connector_credential_verification.rs` pins
/// that). The connectors the read, listing and delete paths are pinned over are
/// therefore seeded in the shared engine the router hands the handlers, the way the
/// other suites of this crate seed state.
pub(crate) fn seed_connector(emulator: &Emulator, tenant: &str, client: &str) -> String {
    seed(
        emulator,
        state::PROVIDER_AZURE,
        tenant,
        ConnectorConfiguration {
            azure: Some(AzureConnectorConfiguration {
                tenant_identifier: tenant.to_string(),
                client_identifier: client.to_string(),
            }),
            // The provider configurations are exclusive: this connector connects to
            // Azure, so it carries no Google Cloud configuration.
            gcp: None,
        },
    )
}

/// Place a connector for the Google Cloud `service_account` in the store, the same way
/// and for the same reason as [`seed_connector`], and answer the ARN it is keyed by.
///
/// The connector is named for the identifier Google Cloud knows the recorded scope by:
/// the organization the management project belongs to, or the project itself where the
/// configuration names no organization.
pub(crate) fn seed_gcp_connector(
    emulator: &Emulator,
    identifier: &str,
    service_account: &str,
    organization: Option<&str>,
) -> String {
    seed(
        emulator,
        state::PROVIDER_GCP,
        identifier,
        ConnectorConfiguration {
            azure: None,
            gcp: Some(GcpConnectorConfiguration {
                gcp_organization_id: organization.map(str::to_string),
                service_account_email: service_account.to_string(),
                workload_identity_pool_provider: POOL_PROVIDER.to_string(),
            }),
        },
    )
}

/// Place a connector for `configuration` in the store, reaching it the way a handler
/// does. The name and the ARN are the ones the service derives for the configuration, so
/// a seeded connector is keyed exactly as a created one would be.
fn seed(
    emulator: &Emulator,
    provider: &str,
    identifier: &str,
    configuration: ConnectorConfiguration,
) -> String {
    let name = state::connector_name(provider, identifier);
    let arn = state::connector_arn(&emulator.region, &name);
    let handle = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    handle.lock().connectors.insert(
        arn.clone(),
        ConnectorEntry {
            name,
            arn: arn.clone(),
            region: emulator.region.clone(),
            provider: provider.to_string(),
            connector_configuration: configuration,
            created_at: SystemTime::now(),
            tags: state::TagSet::new(),
        },
    );
    arn
}

/// Call `operation` with `body` and answer the HTTP status, the error code and the
/// message of the refusal it must be. Shared with the suites that pin the other
/// connector operations' refusals.
pub(crate) async fn refusal(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    let message = ["message", "Message"]
        .iter()
        .find_map(|key| parsed.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// Call `operation` with `arn` and answer the HTTP status, the error code and the
/// message of the refusal it must be.
async fn refused_arn(emulator: &Emulator, operation: &str, arn: &str) -> (u16, String, String) {
    refusal(emulator, operation, &json!({ "Arn": arn })).await
}

/// Every connector ARN `ListConnectors` reports, paged at `max_results` (0 asks for
/// no particular page size), together with the size of each page it served.
async fn list_connector_arns(emulator: &Emulator, max_results: i64) -> (Vec<String>, Vec<usize>) {
    let mut arns = Vec::new();
    let mut page_sizes = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": max_results });
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("ListConnectors", &request).await;
        let summaries = page["ConnectorSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("ListConnectors must answer ConnectorSummaries, got {page}"))
            .clone();
        page_sizes.push(summaries.len());
        for summary in &summaries {
            arns.push(
                summary["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a ConnectorSummary must carry an arn, got {summary}"))
                    .to_string(),
            );
        }
        match page["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    (arns, page_sizes)
}

/// The summaries one unpaginated `ListConnectors` call reports, with `filters`.
pub(crate) async fn list_with_filters(emulator: &Emulator, filters: &Value) -> Vec<Value> {
    let listed = emulator
        .ok("ListConnectors", &json!({ "Filters": filters }))
        .await;
    listed["ConnectorSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListConnectors must answer ConnectorSummaries, got {listed}"))
        .clone()
}

#[tokio::test]
async fn a_connector_in_state_is_listed_and_read_back_exactly_as_it_is_stored() {
    let emulator = Emulator::new();
    // Rows differ along the form and the length of the identifiers the configuration
    // carries: the GUID form Microsoft Entra ID uses, a one-character identifier (the
    // modelled length minimum), a domain-form tenant, and the modelled length
    // maximum. None of them is the identifier the issue's recording used.
    let max_length_tenant = "t".repeat(AZURE_IDENTIFIER_MAX_LEN);
    let max_length_client = "c".repeat(AZURE_IDENTIFIER_MAX_LEN);
    let cases: [(&str, &str, &str); 4] = [
        (
            "guid identifiers",
            "72f988bf-86f1-41af-91ab-2d7cd011db47",
            "04b07795-8ddb-461a-bbee-02f9e1bf7b46",
        ),
        ("shortest identifiers", "t", "c"),
        (
            "domain-form tenant",
            "contoso.onmicrosoft.com",
            "config.connector.app",
        ),
        (
            "longest identifiers",
            max_length_tenant.as_str(),
            max_length_client.as_str(),
        ),
    ];

    for (case, tenant, client) in cases {
        let (expected_name, expected_arn) = expected_name_and_arn(&emulator.region, tenant);
        let arn = seed_connector(&emulator, tenant, client);
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: a connector must be keyed by the ARN naming its derived \
             connector name, got {arn:?}"
        );

        let read = emulator.ok("GetConnector", &json!({ "Arn": arn })).await;
        let connector = &read["Connector"];
        assert_eq!(
            connector["name"].as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: GetConnector must report the derived name, got {connector}"
        );
        assert_eq!(
            connector["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: GetConnector must report the ARN it was created with, got \
             {connector}"
        );
        assert_eq!(
            connector["connectorConfiguration"],
            azure_configuration(tenant, client),
            "case {case:?}: GetConnector must report the configuration it was created \
             with, got {connector}"
        );
        let created_time = connector["createdTime"].as_f64().unwrap_or_default();
        assert!(
            created_time > 0.0,
            "case {case:?}: GetConnector must report when the connector was created, \
             got {connector}"
        );

        let summaries = list_with_filters(&emulator, &json!([])).await;
        let listed = summaries
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(expected_arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListConnectors must report the connector in state, got {summaries:?}")
            });
        assert_eq!(
            (
                listed["name"].as_str(),
                listed["provider"].as_str(),
                listed["tenantIdentifier"].as_str(),
                listed["createdTime"].as_f64(),
            ),
            (
                Some(expected_name.as_str()),
                Some(AZURE),
                Some(tenant),
                Some(created_time),
            ),
            "case {case:?}: a summary must report the attributes state holds for the \
             connector, got {listed}"
        );
    }

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        cases.len(),
        "every connector in state must be listed exactly once, got {arns:?}"
    );
}

#[tokio::test]
async fn paging_reports_every_connector_exactly_once_at_every_page_size() {
    let emulator = Emulator::new();
    const CONNECTORS: usize = 7;
    let mut created = Vec::new();
    for index in 0..CONNECTORS {
        created.push(seed_connector(&emulator, &format!("tenant-{index}"), "client"));
    }
    created.sort();

    // `MaxResults` is 0..100 with a default of 0 ("no particular page size"), from
    // `smithy_query(query="shape", shape="ListConnectorsMaxResults")`. The rows span
    // that range: both published bounds, one past the maximum, a page smaller than
    // the store, a page the size of the store and a page larger than it.
    let cases: [(&str, i64); 6] = [
        ("unspecified page size", 0),
        ("one per page", 1),
        ("smaller than the store", 3),
        ("exactly the store", CONNECTORS as i64),
        ("larger than the store", CONNECTORS as i64 + 1),
        ("the published maximum", 100),
    ];

    for (case, max_results) in cases {
        let (mut arns, page_sizes) = list_connector_arns(&emulator, max_results).await;
        arns.sort();
        assert_eq!(
            arns, created,
            "case {case:?}: paging must report every connector exactly once, got {arns:?}"
        );
        if max_results > 0 {
            let oversized: Vec<usize> = page_sizes
                .iter()
                .copied()
                .filter(|size| *size > max_results as usize)
                .collect();
            assert!(
                oversized.is_empty(),
                "case {case:?}: no page may carry more than MaxResults connectors, got \
                 page sizes {page_sizes:?}"
            );
        }
    }

    // One past the published maximum is refused by the modelled range, not served.
    let (status, code) = emulator
        .refused("ListConnectors", &json!({ "MaxResults": 101 }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"past the published maximum\": expected HTTP 400 ValidationException, \
         got {status} {code}"
    );

    // A token this operation did not issue is malformed, not an empty page. Rows
    // differ along HOW the value fails to be a token this operation handed out: one
    // that is not even base64, and one that is well-formed base64 of something that
    // is not a key this operation pages on.
    for (case, token) in [
        ("not base64 at all", "not a token!!"),
        ("base64 of a value that is no page key", "YWJj"),
    ] {
        let (status, code) = emulator
            .refused("ListConnectors", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_filter_selects_the_connectors_whose_attribute_it_matches() {
    let emulator = Emulator::new();
    const CONNECTORS: usize = 3;
    for index in 0..CONNECTORS {
        seed_connector(&emulator, &format!("tenant-{index}"), "client");
    }

    // Every row is a filter list and the number of the three stored connectors it
    // selects. Each stored connector's provider is `AZURE`; `GCP`, the other published
    // one, is a provider this store holds no connector for.
    let cases: [(&str, Value, usize); 5] = [
        ("no filters", json!([]), CONNECTORS),
        (
            "the stored provider",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [AZURE] }]),
            CONNECTORS,
        ),
        (
            "a provider nothing is stored for",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [GCP] }]),
            0,
        ),
        (
            "either provider",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [GCP, AZURE] }]),
            CONNECTORS,
        ),
        (
            "no value to match",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [] }]),
            0,
        ),
    ];

    for (case, filters, expected) in cases {
        let summaries = list_with_filters(&emulator, &filters).await;
        assert_eq!(
            summaries.len(),
            expected,
            "case {case:?}: expected {expected} connector(s), got {summaries:?}"
        );
        for summary in &summaries {
            assert_eq!(
                summary["provider"].as_str(),
                Some(AZURE),
                "case {case:?}: a selected connector must satisfy the filter, got {summary}"
            );
        }
    }

    // A filter this service cannot apply is refused: answering it with the connectors
    // it would have excluded is not an answer to the request.
    let unsupported: [(&str, Value); 3] = [
        (
            "a filter name the model does not publish",
            json!([{ "filterName": "name", "filterValues": ["azure-tenant-0"] }]),
        ),
        (
            "a filter naming nothing",
            json!([{ "filterValues": [AZURE] }]),
        ),
        (
            "a supported filter beside an unsupported one",
            json!([
                { "filterName": PROVIDER_FILTER, "filterValues": [AZURE] },
                { "filterName": "tenantIdentifier", "filterValues": ["tenant-0"] },
            ]),
        ),
    ];
    for (case, filters) in unsupported {
        let (status, code) = emulator
            .refused("ListConnectors", &json!({ "Filters": filters }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_deleted_connector_is_reported_by_neither_a_read_nor_a_listing() {
    let emulator = Emulator::new();
    let kept = seed_connector(&emulator, "tenant-kept", "client");
    let deleted = seed_connector(&emulator, "tenant-deleted", "client");

    emulator
        .ok("DeleteConnector", &json!({ "Arn": deleted }))
        .await;

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns,
        vec![kept.clone()],
        "a listing must report the connectors still in state, got {arns:?}"
    );

    for operation in ["GetConnector", "DeleteConnector"] {
        let (status, code, message) = refused_arn(&emulator, operation, &deleted).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", CONNECTOR_ARN_NOT_VALID_TEXT),
            "case {operation:?}: a deleted connector's ARN must no longer resolve, got \
             {status} {code} with message {message:?}"
        );
    }

    // The kept connector is untouched by the delete and by the refusals above.
    let read = emulator
        .ok("GetConnector", &json!({ "Arn": kept }))
        .await;
    assert_eq!(
        read["Connector"]["arn"].as_str(),
        Some(kept.as_str()),
        "the connector that was not deleted must still be readable, got {read}"
    );
}

#[tokio::test]
async fn an_arn_that_does_not_resolve_is_refused_the_same_way_however_it_fails() {
    let emulator = Emulator::new();
    seed_connector(&emulator, "tenant-0", "client");

    // One table, two kinds of failure to resolve, ONE refusal. The first rows differ
    // along WHY the value is no connector ARN of this service: not an ARN at all, no
    // resource portion, no connector name, another service's ARN, and another resource
    // type of this service. The last rows are WELL-FORMED connector ARNs that name no
    // connector this account holds in the Region the request is scoped to — a name
    // never created, and this account's own connector in another Region. AWS collapses
    // both kinds into one class and one text, so a caller cannot tell from a refusal
    // whether a connector by that ARN exists.
    let cases: [(&str, &str); 8] = [
        ("not an ARN", "tenant-0"),
        ("too few ARN segments", "arn:aws:config:us-east-1:000000000000"),
        (
            "no connector name",
            "arn:aws:config:us-east-1:000000000000:connector",
        ),
        (
            "an empty connector name",
            "arn:aws:config:us-east-1:000000000000:connector/",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:us-east-1:000000000000:connector/azure-tenant-0",
        ),
        (
            "another resource type of this service",
            "arn:aws:config:us-east-1:000000000000:configuration-recorder/default/abc",
        ),
        (
            "a connector this account never created",
            "arn:aws:config:us-east-1:000000000000:connector/azure-absent",
        ),
        (
            "a connector of another Region",
            "arn:aws:config:us-west-2:000000000000:connector/azure-tenant-0",
        ),
    ];

    for operation in ["GetConnector", "DeleteConnector"] {
        for (case, arn) in cases {
            let (status, code, message) = refused_arn(&emulator, operation, arn).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, "ValidationException", CONNECTOR_ARN_NOT_VALID_TEXT),
                "case {operation:?} / {case:?}: expected HTTP 400 ValidationException \
                 with the one connector-ARN text, got {status} {code} with message \
                 {message:?}"
            );
        }
    }

    // None of the refusals above changed the store.
    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        1,
        "a refused request must not change what a listing reports, got {arns:?}"
    );
}

#[tokio::test]
async fn a_connector_is_served_only_to_the_region_it_belongs_to() {
    // One account's store reached from two Regions: a connector is a regional
    // resource, so each Region sees only its own.
    let east = Emulator::in_region("us-east-1");
    let west = east.in_other_region("us-west-2");

    let east_arn = seed_connector(&east, "tenant-0", "client");
    // The same tenant in another Region is another connector: the Region is part of
    // the connector's identity and of its ARN.
    let west_arn = seed_connector(&west, "tenant-0", "client");
    assert_ne!(
        east_arn, west_arn,
        "each Region's connector must carry its own ARN, got {east_arn:?} and \
         {west_arn:?}"
    );

    for (case, emulator, own, other) in [
        ("us-east-1 caller", &east, &east_arn, &west_arn),
        ("us-west-2 caller", &west, &west_arn, &east_arn),
    ] {
        let (arns, _) = list_connector_arns(emulator, 0).await;
        assert_eq!(
            arns,
            vec![own.clone()],
            "case {case:?}: a listing must report only this Region's connectors, got \
             {arns:?}"
        );
        let read = emulator
            .ok("GetConnector", &json!({ "Arn": own }))
            .await;
        assert_eq!(
            read["Connector"]["arn"].as_str(),
            Some(own.as_str()),
            "case {case:?}: this Region's connector must be readable, got {read}"
        );
        for operation in ["GetConnector", "DeleteConnector"] {
            let (status, code, message) = refused_arn(emulator, operation, other).await;
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, "ValidationException", CONNECTOR_ARN_NOT_VALID_TEXT),
                "case {case:?} / {operation:?}: another Region's connector ARN must not \
                 resolve here, got {status} {code} with message {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_the_members_of_the_provider_the_connector_connects_to() {
    let emulator = Emulator::new();

    // (case, the ARN the seeded connector is keyed by, the provider it connects to, the
    //  provider-specific members its summary must report)
    //
    // Rows differ along WHICH provider the connector connects to and, within Google
    // Cloud, along whether the configuration carries the optional organization id — the
    // one provider-specific member a connector of that provider may have none of.
    // `tenantIdentifier` is "returned only for Azure connectors", `gcpOrganizationId`
    // and `serviceAccountEmail` "only for Google Cloud connectors"
    // (`smithy_query(query="shape", shape="ConnectorSummary")`).
    let other_service_account = "config-reader@analytics-project.iam.gserviceaccount.com";
    let cases: [(&str, String, &str, Value); 4] = [
        (
            "an Azure connector",
            seed_connector(&emulator, "72f988bf-86f1-41af-91ab-2d7cd011db47", "client"),
            AZURE,
            json!({ "tenantIdentifier": "72f988bf-86f1-41af-91ab-2d7cd011db47" }),
        ),
        (
            "a Google Cloud connector recording an organization",
            seed_gcp_connector(&emulator, ORGANIZATION_ID, SERVICE_ACCOUNT, Some(ORGANIZATION_ID)),
            GCP,
            json!({
                "gcpOrganizationId": ORGANIZATION_ID,
                "serviceAccountEmail": SERVICE_ACCOUNT,
            }),
        ),
        (
            "a Google Cloud connector recording only its management project",
            seed_gcp_connector(&emulator, "management-project", other_service_account, None),
            GCP,
            json!({ "serviceAccountEmail": other_service_account }),
        ),
        (
            "a second Azure connector, for a domain-form tenant",
            seed_connector(&emulator, "contoso.onmicrosoft.com", "client"),
            AZURE,
            json!({ "tenantIdentifier": "contoso.onmicrosoft.com" }),
        ),
    ];

    // Every provider-specific member any summary may carry, so each row is checked for
    // the ones it must NOT report as well as the ones it must.
    const PROVIDER_MEMBERS: [&str; 3] =
        ["tenantIdentifier", "gcpOrganizationId", "serviceAccountEmail"];

    let summaries = list_with_filters(&emulator, &json!([])).await;
    for (case, arn, provider, expected) in &cases {
        let summary = summaries
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: ListConnectors must report the connector {arn:?}, got \
                     {summaries:?}"
                )
            });
        assert_eq!(
            summary["provider"].as_str(),
            Some(*provider),
            "case {case:?}: a summary must report the provider the connector connects \
             to, got {summary}"
        );
        for member in PROVIDER_MEMBERS {
            let reported = summary.get(member).cloned().unwrap_or(Value::Null);
            let wanted = expected.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected {member} to be {wanted}, got {reported} in \
                 {summary}"
            );
        }

        // The configuration the summary's members are read off round-trips through the
        // connector read, under the provider member it was created with.
        let read = emulator.ok("GetConnector", &json!({ "Arn": arn })).await;
        let stored = &read["Connector"]["connectorConfiguration"];
        let provider_member = if *provider == GCP { "gcp" } else { "azure" };
        let other_member = if *provider == GCP { "azure" } else { "gcp" };
        assert!(
            stored.get(provider_member).is_some() && stored.get(other_member).is_none(),
            "case {case:?}: the stored configuration must carry the {provider_member} \
             configuration and no other provider's, got {stored}"
        );
    }

    // A filter on the provider selects exactly the connectors of that provider, which is
    // the same split the summaries' members follow.
    for (case, provider) in [("the Azure connectors", AZURE), ("the Google Cloud ones", GCP)] {
        let filtered = list_with_filters(
            &emulator,
            &json!([{ "filterName": PROVIDER_FILTER, "filterValues": [provider] }]),
        )
        .await;
        let expected = cases
            .iter()
            .filter(|(_, _, seeded, _)| *seeded == provider)
            .count();
        assert_eq!(
            filtered.len(),
            expected,
            "case {case:?}: expected {expected} connector(s), got {filtered:?}"
        );
        for summary in &filtered {
            assert_eq!(
                summary["provider"].as_str(),
                Some(provider),
                "case {case:?}: a selected connector must satisfy the filter, got {summary}"
            );
        }
    }
}

