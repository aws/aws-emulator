// SPDX-License-Identifier: CC0-1.0

//! The provider credential a Systems Manager cloud connector acts with, across two
//! services: Systems Manager mints the connector, and the credential it would act with is
//! verified by the Amazon Web Services Config CONNECTOR the cloud connector names.
//!
//! This suite needs more than one service by construction: the question Systems Manager
//! resolves is put to the Config crate, whose own store answers it, so a single-service
//! test cannot see the hand-off at all.
//!
//! The rule under test: a cloud connector acts in the third-party cloud as ONE application
//! registration of ONE tenant, and the credential it acts under is created for that pair —
//! which Systems Manager can do only where this account already holds a credential verified
//! for them. That credential is the Config connector the required `ConfigConnectorArn` names
//! ("The ARN of the Amazon Web Services Config connector associated with this cloud
//! connector"), whose `AzureConnectorConfiguration` carries the tenant and the client it was
//! created for. So a create, and an update that REPLACES the configuration, is served only
//! where Config answers with a connector held for exactly that tenant and that application,
//! and is otherwise refused HTTP 400 `ValidationException` with the one sentence for a
//! provider credential that could not be created, writing nothing; a validation of a
//! connector reports the same answer as the modelled `ProviderCredentialCreationFailed`
//! finding of the step that failed.
//!
//! The rows span what the rule generalises over: which tenant and which application Config
//! holds a connector for, whether it holds one under the named ARN at all, which Region
//! holds it, which provider its configuration names, how the configuration under test
//! differs from the verified pair, and which members an update carries.
//!
//! Every Config connector here is PLANTED in that service's own store and then read back
//! through its own `GetConnector`, because `PutConnector` verifies a provider configuration's
//! credentials with the provider itself — an outbound call this emulator does not make — and
//! so creates nothing.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_configservice_starlingdoveservice::ops::types::{
    AzureConnectorConfiguration, ConnectorConfiguration,
};
use service_com_amazonaws_configservice_starlingdoveservice::state as config_state;

/// The Region every request below is signed for unless a case says otherwise.
const REGION: &str = "us-west-2";

/// A second Region, so a case can name a connector Config holds somewhere else.
const OTHER_REGION: &str = "eu-west-1";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An Azure tenant id: `AzureConfiguration$TenantId` is a hyphenated hex UUID
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`), the
/// form Config's `AzureTenantIdentifier` carries too.
const TENANT: &str = "11111111-2222-3333-4444-555555555555";

/// See [`TENANT`]: `AzureConfiguration$ApplicationId` has the same shape, and is the
/// application Config's `AzureClientIdentifier` names.
const APPLICATION: &str = "66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A second tenant, which the account holds no verified credential for.
const OTHER_TENANT: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// A second application registration, in the same shape.
const OTHER_APPLICATION: &str = "01234567-89ab-cdef-0123-456789abcdef";

/// `AzureSubscription$Id` values a configuration may target, same UUID shape again.
const SUBSCRIPTION_A: &str = "aaaaaaa1-0000-0000-0000-000000000001";

/// See [`SUBSCRIPTION_A`].
const SUBSCRIPTION_B: &str = "aaaaaaa2-0000-0000-0000-000000000002";

/// The sentence a request whose tenant and application no verified credential of this
/// account stands behind is refused with: entirely fixed text, interpolating nothing from
/// the request.
const PROVIDER_CREDENTIAL_UNOBTAINABLE: &str =
    "Unable to create Azure credentials. Please verify the Azure tenant ID, application ID, \
     and federated identity credential configuration are correct.";

/// The `ValidationFindingCode` a validation reports that failure under, from the modelled
/// enum (`smithy_query(query="shape", shape="ValidationFindingCode")`:
/// `… WebIdentityTokenFailed | OutboundWebIdentityFederationDisabled |
/// ProviderCredentialCreationFailed | TenantSummary | SubscriptionAccessible`).
const PROVIDER_CREDENTIAL_FINDING_CODE: &str = "ProviderCredentialCreationFailed";

/// The `ValidationFindingType` of a finding reporting a check that failed (same model,
/// `ValidationFindingType`: `INFO | WARN | ERROR`).
const FINDING_TYPE_ERROR: &str = "ERROR";

/// The `ValidationFindingScopeType` a tenant-scoped finding carries
/// (`ValidationFindingScopeType`: `azure:tenant | azure:subscription`).
const FINDING_SCOPE_TENANT: &str = "azure:tenant";

/// The two services this hand-off needs, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what this suite is about; the router still reads the SigV4
    // credential scope below, which is where each request's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One `awsJson1_1` RPC call signed for `signing_name` in `region`, as its status and
/// parsed body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A Systems Manager call at `REGION`'s endpoint (`AmazonSSM.<Operation>`).
async fn ssm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "ssm", "AmazonSSM", REGION, operation, body).await
}

/// The same call, required to be served, answering its body.
async fn ssm_ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    let (status, parsed) = ssm(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served, got {status} with {parsed}"
    );
    parsed
}

/// The code a refusal names itself by (`__type`), its shape name alone.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a refusal states, in whichever spelling of the member it carries.
fn error_message(body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(message) = body[member].as_str() {
            return message.to_string();
        }
    }
    String::new()
}

/// A `CloudConnectorConfiguration` naming `tenant` and `application`, targeting
/// `subscriptions` (none at all when the list is empty, which is a tenant-level connector).
fn azure_configuration(tenant: &str, application: &str, subscriptions: &[&str]) -> Value {
    let mut azure = json!({
        "TenantId": tenant,
        "ApplicationId": application,
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

/// A `CreateCloudConnector` body naming `config_connector_arn` and `configuration`.
fn create_body(display_name: &str, config_connector_arn: &str, configuration: Value) -> Value {
    json!({
        "DisplayName": display_name,
        // No IAM is mounted here, so nothing answers about the role: this suite is about the
        // step AFTER the role, and the role questions resolve to nothing either way.
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SsmCloudConnectorRole"),
        "ConfigConnectorArn": config_connector_arn,
        "Configuration": configuration,
    })
}

/// The ARN a Config connector for `tenant` in `region` is named by, whether or not that
/// service holds one: the ARN Config itself mints
/// (`arn:aws:config:<region>:<account>:connector/<provider>-<tenant>`).
fn config_connector_arn(region: &str, tenant: &str) -> String {
    config_state::connector_arn(
        region,
        &config_state::connector_name(config_state::PROVIDER_AZURE, tenant),
    )
}

/// Plant a Config connector verified for `configuration` in `region`, and read it back
/// through Config's OWN `GetConnector` so the credential this suite builds on is one that
/// service reports from its own store.
///
/// It is planted rather than created because `PutConnector` verifies a provider
/// configuration WITH the provider — an outbound call this emulator does not make — and so
/// creates no connector at all.
async fn config_connector(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    tenant: &str,
    configuration: ConnectorConfiguration,
    case: &str,
) -> String {
    let name = config_state::connector_name(config_state::PROVIDER_AZURE, tenant);
    let arn = config_state::connector_arn(region, &name);
    {
        let state = engine.state::<config_state::ComAmazonawsConfigserviceStarlingdoveserviceState>();
        let mut store = state.lock();
        store.connectors.insert(
            arn.clone(),
            config_state::ConnectorEntry {
                name,
                arn: arn.clone(),
                region: region.to_string(),
                provider: config_state::PROVIDER_AZURE.to_string(),
                connector_configuration: configuration,
                created_at: SystemTime::now(),
                tags: config_state::TagSet::default(),
            },
        );
    }
    let read = call(
        router,
        "config",
        "StarlingDoveService",
        region,
        "GetConnector",
        &json!({ "Arn": arn }),
    )
    .await;
    assert_eq!(
        read.0, 200,
        "case {case:?}: Config must report the connector it holds, got {} with {}",
        read.0, read.1
    );
    arn
}

/// The `ConnectorConfiguration` of a connector verified for `tenant` and `client`.
fn verified_for(tenant: &str, client: &str) -> ConnectorConfiguration {
    ConnectorConfiguration {
        azure: Some(AzureConnectorConfiguration {
            tenant_identifier: tenant.to_string(),
            client_identifier: client.to_string(),
        }),
        // The provider configurations are exclusive — "you must specify exactly one
        // provider configuration" — and this connector connects to Azure.
        gcp: None,
    }
}

/// The cloud connector ids Systems Manager reports at `REGION`'s endpoint.
async fn listed(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = ssm_ok(router, "ListCloudConnectors", &json!({}), case).await;
    body["CloudConnectors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["CloudConnectorId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: a cloud connector is minted only where Config answers with a connector it holds
/// for exactly the tenant and the application the configuration names; every other answer
/// refuses the create with the provider-credential sentence and mints nothing.
#[tokio::test]
async fn a_cloud_connector_is_minted_only_for_a_tenant_config_verifies() {
    /// One row: what Config holds, and the configuration the create asks for.
    struct Row {
        case: &'static str,
        /// The connector to plant in Config: its Region, the tenant it is named for and
        /// the configuration it was verified with — or `None` to plant none.
        held: Option<(&'static str, &'static str, ConnectorConfiguration)>,
        /// The `ConfigConnectorArn` the create names: the Region and tenant it is built
        /// from.
        names: (&'static str, &'static str),
        /// The tenant and the application the create's `Configuration` names, and the
        /// subscriptions it targets.
        configuration: (&'static str, &'static str, &'static [&'static str]),
        served: bool,
    }

    let rows = vec![
        Row {
            case: "a Config connector verified for that tenant and application",
            held: Some((REGION, TENANT, verified_for(TENANT, APPLICATION))),
            names: (REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: true,
        },
        Row {
            case: "the same verified pair, targeting two subscriptions",
            held: Some((REGION, TENANT, verified_for(TENANT, APPLICATION))),
            names: (REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[SUBSCRIPTION_A, SUBSCRIPTION_B]),
            served: true,
        },
        Row {
            case: "another tenant, verified for the application the create names",
            held: Some((REGION, OTHER_TENANT, verified_for(OTHER_TENANT, APPLICATION))),
            names: (REGION, OTHER_TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: false,
        },
        Row {
            case: "that tenant, verified for another application",
            held: Some((REGION, TENANT, verified_for(TENANT, OTHER_APPLICATION))),
            names: (REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: false,
        },
        Row {
            case: "an ARN Config holds no connector under",
            held: None,
            names: (REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: false,
        },
        Row {
            case: "a connector Config holds in another Region",
            held: Some((OTHER_REGION, TENANT, verified_for(TENANT, APPLICATION))),
            names: (OTHER_REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: false,
        },
        Row {
            case: "a connector whose configuration names no provider identifiers",
            held: Some((REGION, TENANT, ConnectorConfiguration::default())),
            names: (REGION, TENANT),
            configuration: (TENANT, APPLICATION, &[]),
            served: false,
        },
    ];

    for row in rows {
        let case = row.case;
        // A router per row, so each row's Config store holds only its own connector.
        let (router, engine) = router();
        if let Some((region, tenant, configuration)) = row.held {
            config_connector(&router, &engine, region, tenant, configuration, case).await;
        }
        let (tenant, application, subscriptions) = row.configuration;
        let (named_region, named_tenant) = row.names;
        let (status, body) = ssm(
            &router,
            "CreateCloudConnector",
            &create_body(
                case,
                &config_connector_arn(named_region, named_tenant),
                azure_configuration(tenant, application, subscriptions),
            ),
        )
        .await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: the create must be served, got {status} with {body}"
            );
            let id = body["CloudConnectorId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a served create reports an id, got {body}")
            });
            assert_eq!(
                listed(&router, case).await,
                vec![id.to_string()],
                "case {case:?}: the minted connector is the one reported"
            );
            // The round trip: what the create persisted is what a read reports.
            let read = ssm_ok(
                &router,
                "GetCloudConnector",
                &json!({ "CloudConnectorId": id }),
                case,
            )
            .await;
            let azure = &read["Configuration"]["AzureConfiguration"];
            assert_eq!(
                azure["TenantId"].as_str(),
                Some(tenant),
                "case {case:?}: expected the tenant {tenant:?}, got {read}"
            );
            assert_eq!(
                azure["ApplicationId"].as_str(),
                Some(application),
                "case {case:?}: expected the application {application:?}, got {read}"
            );
            let targeted: Vec<String> = azure["Targets"]["Subscriptions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|subscription| subscription["Id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                targeted,
                subscriptions
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected the subscriptions the create named, got {targeted:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the create must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                PROVIDER_CREDENTIAL_UNOBTAINABLE,
                "case {case:?}: expected {PROVIDER_CREDENTIAL_UNOBTAINABLE:?}, got {:?}",
                error_message(&body)
            );
            assert!(
                listed(&router, case).await.is_empty(),
                "case {case:?}: a refused create mints nothing"
            );
        }
    }
}

/// RULE: an update that REPLACES the configuration resolves the replacement the same way,
/// against the Config connector the connector was created with — so a replacement naming a
/// pair that connector does not verify is refused and the stored configuration survives,
/// while an update of anything else is served.
#[tokio::test]
async fn an_update_replacing_the_configuration_is_resolved_against_config() {
    /// One row: the update's members, and whether it is served.
    struct Row {
        case: &'static str,
        display_name: Option<&'static str>,
        /// The tenant, application and subscriptions a replacement configuration names,
        /// or `None` for an update carrying no configuration at all.
        configuration: Option<(&'static str, &'static str, &'static [&'static str])>,
        served: bool,
    }

    let rows = vec![
        Row {
            case: "an update carrying no configuration",
            display_name: Some("renamed"),
            configuration: None,
            served: true,
        },
        Row {
            case: "the verified pair again, targeting a subscription",
            display_name: None,
            configuration: Some((TENANT, APPLICATION, &[SUBSCRIPTION_A])),
            served: true,
        },
        Row {
            case: "a replacement naming another tenant",
            display_name: None,
            configuration: Some((OTHER_TENANT, APPLICATION, &[])),
            served: false,
        },
        Row {
            case: "a replacement naming another application in that tenant",
            display_name: Some("also renamed"),
            configuration: Some((TENANT, OTHER_APPLICATION, &[])),
            served: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let (router, engine) = router();
        // Config holds a connector for each tenant, so a refusal rests on which pair the
        // connector the cloud connector NAMES verifies rather than on what Config holds at
        // all.
        let arn = config_connector(
            &router,
            &engine,
            REGION,
            TENANT,
            verified_for(TENANT, APPLICATION),
            case,
        )
        .await;
        config_connector(
            &router,
            &engine,
            REGION,
            OTHER_TENANT,
            verified_for(OTHER_TENANT, OTHER_APPLICATION),
            case,
        )
        .await;

        let created = ssm_ok(
            &router,
            "CreateCloudConnector",
            &create_body(
                "the connector under update",
                &arn,
                azure_configuration(TENANT, APPLICATION, &[]),
            ),
            case,
        )
        .await;
        let id = created["CloudConnectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a served create reports an id, got {created}"))
            .to_string();

        let mut update = json!({ "CloudConnectorId": id });
        if let Some(display_name) = row.display_name {
            update["DisplayName"] = json!(display_name);
        }
        if let Some((tenant, application, subscriptions)) = row.configuration {
            update["Configuration"] = azure_configuration(tenant, application, subscriptions);
        }
        let (status, body) = ssm(&router, "UpdateCloudConnector", &update).await;
        let read = ssm_ok(
            &router,
            "GetCloudConnector",
            &json!({ "CloudConnectorId": id }),
            case,
        )
        .await;
        let azure = &read["Configuration"]["AzureConfiguration"];

        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: the update must be served, got {status} with {body}"
            );
            let (tenant, application, subscriptions) =
                row.configuration.unwrap_or((TENANT, APPLICATION, &[]));
            assert_eq!(
                (
                    azure["TenantId"].as_str(),
                    azure["ApplicationId"].as_str(),
                    azure["Targets"]["Subscriptions"]
                        .as_array()
                        .map(|list| list.len())
                        .unwrap_or_default(),
                ),
                (Some(tenant), Some(application), subscriptions.len()),
                "case {case:?}: expected the configuration the update carried, got {read}"
            );
            if let Some(display_name) = row.display_name {
                assert_eq!(
                    read["DisplayName"].as_str(),
                    Some(display_name),
                    "case {case:?}: expected the name the update carried, got {read}"
                );
            }
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the update must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                PROVIDER_CREDENTIAL_UNOBTAINABLE,
                "case {case:?}: expected {PROVIDER_CREDENTIAL_UNOBTAINABLE:?}, got {:?}",
                error_message(&body)
            );
            // A refused update wrote nothing: the stored configuration is the created one,
            // and the display name the refused update also carried did not land either.
            assert_eq!(
                (
                    azure["TenantId"].as_str(),
                    azure["ApplicationId"].as_str(),
                    read["DisplayName"].as_str(),
                ),
                (
                    Some(TENANT),
                    Some(APPLICATION),
                    Some("the connector under update"),
                ),
                "case {case:?}: expected the stored connector untouched, got {read}"
            );
        }
    }
}

/// RULE: a validation reports the SAME answer as a finding of the step it belongs to — so a
/// connector whose verified credential Config no longer holds validates with an `ERROR`
/// finding coded `ProviderCredentialCreationFailed` carrying that sentence, scoped to the
/// tenant it targets, and a connector whose credential is still held reports no such
/// finding. The validation changes nothing either way.
#[tokio::test]
async fn a_validation_reports_the_provider_credential_step_from_config() {
    let case = "a connector whose Config connector is given up after it was minted";
    let (router, engine) = router();
    let arn = config_connector(
        &router,
        &engine,
        REGION,
        TENANT,
        verified_for(TENANT, APPLICATION),
        case,
    )
    .await;
    let created = ssm_ok(
        &router,
        "CreateCloudConnector",
        &create_body(
            "the validated connector",
            &arn,
            azure_configuration(TENANT, APPLICATION, &[SUBSCRIPTION_A]),
        ),
        case,
    )
    .await;
    let id = created["CloudConnectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served create reports an id, got {created}"))
        .to_string();

    /// The findings a validation reports, as (type, code, message, scope type, scope id).
    fn reported(body: &Value) -> Vec<(String, String, String, String, String)> {
        body["ValidationFindings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|finding| {
                (
                    finding["Type"].as_str().unwrap_or_default().to_string(),
                    finding["Code"].as_str().unwrap_or_default().to_string(),
                    finding["Message"].as_str().unwrap_or_default().to_string(),
                    finding["Scope"]["Type"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    finding["Scope"]["Id"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    let validated = ssm_ok(
        &router,
        "ValidateCloudConnector",
        &json!({ "CloudConnectorId": id }),
        case,
    )
    .await;
    let findings = reported(&validated);
    assert!(
        !findings
            .iter()
            .any(|(_, code, _, _, _)| code == PROVIDER_CREDENTIAL_FINDING_CODE),
        "case {case:?}: a connector whose credential Config still holds reports no \
         {PROVIDER_CREDENTIAL_FINDING_CODE} finding, got {findings:?}"
    );

    // Config gives the connector up, so nothing verifies the credential any more.
    {
        let state =
            engine.state::<config_state::ComAmazonawsConfigserviceStarlingdoveserviceState>();
        let mut store = state.lock();
        store.connectors.remove(&arn);
    }

    let validated = ssm_ok(
        &router,
        "ValidateCloudConnector",
        &json!({ "CloudConnectorId": id }),
        case,
    )
    .await;
    let findings = reported(&validated);
    assert!(
        findings.contains(&(
            FINDING_TYPE_ERROR.to_string(),
            PROVIDER_CREDENTIAL_FINDING_CODE.to_string(),
            PROVIDER_CREDENTIAL_UNOBTAINABLE.to_string(),
            FINDING_SCOPE_TENANT.to_string(),
            TENANT.to_string(),
        )),
        "case {case:?}: expected an {FINDING_TYPE_ERROR} {PROVIDER_CREDENTIAL_FINDING_CODE} \
         finding for the tenant, got {findings:?}"
    );

    // Two validations of one unchanged connector report the same findings, and the
    // connector is still the one the create persisted.
    let again = reported(
        &ssm_ok(
            &router,
            "ValidateCloudConnector",
            &json!({ "CloudConnectorId": id }),
            case,
        )
        .await,
    );
    assert_eq!(
        again, findings,
        "case {case:?}: expected the same findings twice, got {again:?} then {findings:?}"
    );
    let read = ssm_ok(
        &router,
        "GetCloudConnector",
        &json!({ "CloudConnectorId": id }),
        case,
    )
    .await;
    assert_eq!(
        read["Configuration"]["AzureConfiguration"]["TenantId"].as_str(),
        Some(TENANT),
        "case {case:?}: a validation changes nothing the connector holds, got {read}"
    );
}
