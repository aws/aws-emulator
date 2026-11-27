// SPDX-License-Identifier: CC0-1.0

//! `PutThirdPartyServiceLinkedConfigurationRecorder`, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — and asserts
//! what the service PERSISTED, by reading the recorder back through
//! `DescribeConfigurationRecorders` and `DescribeConfigurationRecorderStatus`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded case:
//!
//! 1. a service principal that names no Amazon Web Services service, and one that
//!    names a service Config records no third-party cloud service provider for, are
//!    both refused with `ValidationException` and persist nothing, however well formed
//!    they are;
//! 2. a supported service principal creates the recorder the linked service manages:
//!    the name Config derives from that service, the recording configuration the
//!    linked service sets, and the scope of third-party resources the request asked
//!    for, all read back out of state;
//! 3. the scope configuration must satisfy its own published constraints;
//! 4. the connector the recorder records through must exist, and an ARN that names
//!    none resolves no recorder for the principal, so Config words that refusal about
//!    the service principal;
//! 5. a repeat call for the same principal and connector updates the scope, while a
//!    different connector for the same principal conflicts;
//! 6. a service-linked recorder is always recording and is not this account's
//!    customer managed recorder.
//!
//! The connector the recorder links to is SEEDED in the shared engine
//! (`crate::config_connectors::seed_connector`), because `PutConnector` creates none:
//! it verifies a provider credential with the provider before materializing anything,
//! which is an outbound connection this emulator does not make (see
//! `connector_credential_verification.rs`). Every other step goes over the wire.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state;

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const PUT_THIRD_PARTY: &str = "PutThirdPartyServiceLinkedConfigurationRecorder";

/// The sentence Config answers a `ServicePrincipal` that names no service it links a
/// third-party configuration recorder to with, spelled out here rather than read from
/// the handler's own constant so this suite pins the text instead of restating it.
const UNRECOGNIZED_PRINCIPAL_MESSAGE: &str =
    "The service principal of the configuration recorder you entered cannot be found. \
     Verify the configuration recorder service principal, and try again.";

/// The connector a third-party service-linked recorder links to, placed in the store
/// and answered by its ARN.
///
/// `PutConnector` creates no connector in this emulator — it refuses a provider
/// credential it cannot verify with the provider, which is every one — so the
/// connector this suite needs is seeded the way the other suites of this crate seed
/// state (`crate::config_connectors::seed_connector`).
fn seed_connector(emulator: &Emulator, tenant: &str) -> String {
    crate::config_connectors::seed_connector(emulator, tenant, &format!("client-{tenant}"))
}

/// The request body for the operation under test.
fn put_request(service_principal: &str, connector_arn: &str, scope: &Value) -> Value {
    json!({
        "ServicePrincipal": service_principal,
        "ConnectorArn": connector_arn,
        "ScopeConfiguration": scope,
    })
}

/// The `message` an error body carries, under either spelling the wire may use.
fn error_message(body: &Value) -> String {
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    format!("<no message in {body}>")
}

/// Call `operation` and require the modelled refusal `expected_code` with HTTP 400,
/// returning the message it carried.
async fn refused_with(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    expected_code: &str,
) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} with body {parsed}"
    );
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected {expected_code} from {operation}, got {code:?} with body {parsed}"
    );
    error_message(&parsed)
}

/// Every configuration recorder this account holds, read back over the wire.
async fn described_recorders(emulator: &Emulator) -> Vec<Value> {
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    described["ConfigurationRecorders"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_service_principal_that_owns_no_third_party_recorder_is_refused() {
    // Every row satisfies the modelled `ServicePrincipal` constraints (length 1..128,
    // pattern `^[\w+=,.@-]+$`, from
    // smithy_query(query="operation", operation="PutThirdPartyServiceLinkedConfigurationRecorder")),
    // so the handler is what has to refuse them. The rows differ in FORM: another
    // Amazon Web Services service, the supported service mis-cased, it with an extra
    // label, a bare service label, a principal built only from the pattern's other
    // character classes, and the longest principal the model allows.
    let longest = format!("{}.amazonaws.com", "x".repeat(114));
    let cases: Vec<(&str, String)> = vec![
        ("another AWS service", "s3.amazonaws.com".to_string()),
        (
            "the supported service mis-cased",
            "SecurityHub.amazonaws.com".to_string(),
        ),
        (
            "the supported service with an extra label",
            "securityhub.amazonaws.com.example".to_string(),
        ),
        ("a bare service label", "securityhub".to_string()),
        (
            "the pattern's other character classes",
            "a+b=c,d.e@f-g".to_string(),
        ),
        ("the longest principal the model allows", longest),
    ];

    for (case, principal) in cases {
        assert_eq!(
            principal.len().clamp(1, 128),
            principal.len(),
            "case {case:?}: the row must satisfy the modelled length, got {} characters",
            principal.len()
        );
        assert!(
            !state::supports_third_party_service_linked_recording(&principal),
            "case {case:?}: the row must name no service Config records a third-party \
             cloud service provider for, got {principal:?}"
        );

        let emulator = Emulator::new();
        let connector = seed_connector(&emulator, "tenant-unrecognized");
        let message = refused_with(
            &emulator,
            case,
            PUT_THIRD_PARTY,
            &put_request(
                &principal,
                &connector,
                &json!({ "scopeType": "tenant", "allRegions": true }),
            ),
            "ValidationException",
        )
        .await;
        // The refusal is the one request-independent sentence Config answers this
        // condition with, whichever of the two reasons refused the value.
        assert_eq!(
            message, UNRECOGNIZED_PRINCIPAL_MESSAGE,
            "case {case:?}: expected the refusal Config answers an unrecognized recorder \
             service principal with, got {message:?}"
        );

        let recorders = described_recorders(&emulator).await;
        assert!(
            recorders.is_empty(),
            "case {case:?}: a refused request must persist no recorder, got {recorders:?}"
        );
    }
}

#[tokio::test]
async fn a_supported_service_principal_creates_the_recorder_the_linked_service_manages() {
    // Rows differ along the scope of third-party resources recorded: every published
    // `scopeType` value, one pair per provider (`ScopeConfiguration$scopeType`: "tenant
    // – Applies to all resources in an Azure tenant", "subscription", "organization –
    // Applies to all resources in a Google Cloud organization", "project"), how many
    // scope values are named, and whether every region of the provider is recorded or an
    // explicit `includedRegions` list is.
    let cases: [(&str, Value); 9] = [
        (
            "tenant scope, every region",
            json!({ "scopeType": "tenant", "allRegions": true }),
        ),
        (
            "tenant scope naming one management group",
            json!({ "scopeType": "tenant", "scopeValues": ["mg-root"], "allRegions": true }),
        ),
        (
            "subscription scope naming two subscriptions",
            json!({
                "scopeType": "subscription",
                "scopeValues": ["sub-1111", "sub-2222"],
                "allRegions": true,
            }),
        ),
        (
            "subscription scope with one included region",
            json!({
                "scopeType": "subscription",
                "scopeValues": ["sub-1111"],
                "allRegions": false,
                "includedRegions": ["eastus"],
            }),
        ),
        (
            "tenant scope with several included regions",
            json!({
                "scopeType": "tenant",
                "allRegions": false,
                "includedRegions": ["eastus", "westeurope", "australiaeast"],
            }),
        ),
        (
            // `includedRegions` is "used when allRegions is set to false", so a request
            // that records every region AND lists regions is stored as sent rather
            // than refused: the list simply does not narrow anything.
            "every region recorded beside an included-region list",
            json!({
                "scopeType": "subscription",
                "allRegions": true,
                "includedRegions": ["eastus"],
            }),
        ),
        (
            "organization scope, every region",
            json!({ "scopeType": "organization", "allRegions": true }),
        ),
        (
            "project scope naming two projects",
            json!({
                "scopeType": "project",
                "scopeValues": ["analytics-project", "platform-project"],
                "allRegions": true,
            }),
        ),
        (
            "project scope with one included region",
            json!({
                "scopeType": "project",
                "scopeValues": ["analytics-project"],
                "allRegions": false,
                "includedRegions": ["us-central1"],
            }),
        ),
    ];

    for (case, scope) in cases {
        // Every service principal Config records a third-party cloud service provider
        // for, from `state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS`.
        for principal in state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS {
            let emulator = Emulator::in_region("us-west-2");
            let connector = seed_connector(&emulator, "tenant-create");

            let created = emulator
                .ok(PUT_THIRD_PARTY, &put_request(principal, &connector, &scope))
                .await;
            // Config assigns the name itself, from the service principal and the
            // reserved prefix — the same derivation every service-linked recorder of
            // this account is named by.
            let expected_name = state::service_linked_recorder_name(principal)
                .unwrap_or_else(|| panic!("{principal} must name a service-linked recorder"));
            assert!(
                expected_name.starts_with(state::SERVICE_LINKED_RECORDER_NAME_PREFIX),
                "case {case:?}/{principal}: the assigned name carries the reserved prefix, \
                 got {expected_name:?}"
            );
            assert_eq!(
                created["Name"], expected_name,
                "case {case:?}/{principal}: Config names the recorder after the linked \
                 service, got {created}"
            );
            let arn = created["Arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.contains(&format!(":configuration-recorder/{expected_name}/"))
                    && arn.contains(&format!(":{}:", emulator.region)),
                "case {case:?}/{principal}: the recorder ARN names the resource type, the \
                 assigned name and the Region, got {arn:?}"
            );

            // The read back: the recorder the linked service now owns.
            let described = emulator
                .ok(
                    "DescribeConfigurationRecorders",
                    &json!({ "ServicePrincipal": principal }),
                )
                .await;
            let recorders = described["ConfigurationRecorders"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                recorders.len(),
                1,
                "case {case:?}/{principal}: exactly the created recorder is linked to that \
                 service, got {described}"
            );
            let recorder = &recorders[0];

            for (member, actual, expected) in [
                ("arn", recorder["arn"].clone(), json!(arn)),
                ("name", recorder["name"].clone(), json!(expected_name)),
                (
                    "servicePrincipal",
                    recorder["servicePrincipal"].clone(),
                    json!(principal),
                ),
                (
                    "connectorArn",
                    recorder["connectorArn"].clone(),
                    json!(connector),
                ),
                (
                    "recordingScope",
                    recorder["recordingScope"].clone(),
                    json!(state::RECORDING_SCOPE_INTERNAL),
                ),
                (
                    "scopeConfiguration",
                    recorder["scopeConfiguration"].clone(),
                    scope.clone(),
                ),
            ] {
                assert_eq!(
                    actual, expected,
                    "case {case:?}/{principal}: expected {member} {expected}, got {actual}"
                );
            }
            assert_eq!(
                recorder["roleARN"],
                json!(state::config_service_linked_role_arn()),
                "case {case:?}/{principal}: a service-linked recorder records with Config's \
                 own service-linked role, not one the caller named, got {recorder}"
            );
            // The linked service sets the recording configuration, so it is reported
            // whole rather than left for the caller to infer.
            assert_eq!(
                recorder["recordingGroup"]["recordingStrategy"]["useOnly"],
                "ALL_SUPPORTED_RESOURCE_TYPES",
                "case {case:?}/{principal}: the linked service records every supported \
                 resource type, got {recorder}"
            );
            assert_eq!(
                recorder["recordingMode"]["recordingFrequency"], "CONTINUOUS",
                "case {case:?}/{principal}: the linked service records continuously, got \
                 {recorder}"
            );

            // A service-linked recorder is recording from the moment it is created.
            let status = emulator
                .ok(
                    "DescribeConfigurationRecorderStatus",
                    &json!({ "ServicePrincipal": principal }),
                )
                .await;
            let statuses = status["ConfigurationRecordersStatus"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                statuses.len(),
                1,
                "case {case:?}/{principal}: the recorder has one status record, got {status}"
            );
            assert_eq!(
                statuses[0]["recording"],
                json!(true),
                "case {case:?}/{principal}: a service-linked recorder is always recording, \
                 got {status}"
            );
            assert_eq!(
                statuses[0]["servicePrincipal"], principal,
                "case {case:?}/{principal}: the status names the linked service, got {status}"
            );

            // The listing reports the provider the recorder's connector connects to.
            let listed = emulator
                .ok("ListConfigurationRecorders", &json!({}))
                .await;
            let summaries = listed["ConfigurationRecorderSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let summary = summaries
                .iter()
                .find(|summary| summary["name"] == json!(expected_name))
                .cloned()
                .unwrap_or(Value::Null);
            for (member, expected) in [
                ("servicePrincipal", json!(principal)),
                ("recordingScope", json!(state::RECORDING_SCOPE_INTERNAL)),
                ("provider", json!(state::PROVIDER_AZURE)),
            ] {
                assert_eq!(
                    summary[member], expected,
                    "case {case:?}/{principal}: expected the summary's {member} to be \
                     {expected}, got {summary}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_first_recording_event_settles_the_way_the_clock_owes_it() {
    let emulator = Emulator::new();
    let connector = seed_connector(&emulator, "tenant-settle");
    let principal = state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0];
    emulator
        .ok(
            PUT_THIRD_PARTY,
            &put_request(
                principal,
                &connector,
                &json!({ "scopeType": "tenant", "allRegions": true }),
            ),
        )
        .await;

    let first = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    assert_eq!(
        first["ConfigurationRecordersStatus"][0]["lastStatus"], "Pending",
        "case \"just created\": the first recording event is still being processed, got {first}"
    );

    // The move to Success is owed by the clock, not by the describe.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let settled = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    assert_eq!(
        settled["ConfigurationRecordersStatus"][0]["lastStatus"], "Success",
        "case \"settled\": the recording event has been processed, got {settled}"
    );
    assert_eq!(
        settled["ConfigurationRecordersStatus"][0]["recording"],
        json!(true),
        "case \"settled\": the recorder is still recording, got {settled}"
    );
}

#[tokio::test]
async fn a_scope_configuration_outside_its_published_constraints_is_refused() {
    // Rows differ along which published value the scope type misses (every published
    // `scopeType`, from `smithy_query(query="shape", shape="ScopeConfiguration")`:
    // `tenant`, `subscription`, `organization`, `project`), including values that only
    // differ in case and one that names another provider's resource container.
    let cases: [(&str, Value); 5] = [
        (
            "a scope type outside the published values",
            json!({ "scopeType": "resourceGroup", "allRegions": true }),
        ),
        (
            "a published scope type mis-cased",
            json!({ "scopeType": "Tenant", "allRegions": true }),
        ),
        (
            "a published scope type upper-cased",
            json!({ "scopeType": "SUBSCRIPTION", "allRegions": true }),
        ),
        (
            "another container of the Google Cloud resource hierarchy",
            json!({ "scopeType": "folder", "allRegions": true }),
        ),
        (
            "a published scope type in the plural",
            json!({ "scopeType": "projects", "allRegions": true }),
        ),
    ];

    for (case, scope) in cases {
        let emulator = Emulator::new();
        let connector = seed_connector(&emulator, "tenant-scope");
        let principal = state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0];
        let message = refused_with(
            &emulator,
            case,
            PUT_THIRD_PARTY,
            &put_request(principal, &connector, &scope),
            "ValidationException",
        )
        .await;
        assert!(
            message.contains("scopeConfiguration"),
            "case {case:?}: the refusal names the member it was built from, got {message:?}"
        );
        for published in state::SCOPE_TYPES {
            assert!(
                message.contains(published),
                "case {case:?}: the refusal names every value the member admits, \
                 {published:?} missing from {message:?}"
            );
        }

        let recorders = described_recorders(&emulator).await;
        assert!(
            recorders.is_empty(),
            "case {case:?}: a refused request must persist no recorder, got {recorders:?}"
        );
    }
}

#[tokio::test]
async fn the_connector_the_recorder_is_linked_to_must_exist() {
    let emulator = Emulator::new();
    let principal = state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0];
    let scope = json!({ "scopeType": "subscription", "allRegions": true });

    // Rows differ in the FORM of a connector ARN this account does not hold: a
    // well-formed connector ARN of another account, one of another Region, one of this
    // account that was never created, and a string that is no ARN at all. A well-formed
    // ARN that names no connector resolves no third-party recorder for the principal at
    // all, so the refusal is the service-principal sentence, quoting nothing of the
    // request; a value that is no connector ARN is the member's own form, which Config
    // states against the value sent.
    let absent = [
        (
            "a connector of another account",
            "arn:aws:config:us-east-1:999999999999:connector/0123456789abcdef".to_string(),
            UNRECOGNIZED_PRINCIPAL_MESSAGE.to_string(),
        ),
        (
            "a connector of another Region",
            state::connector_arn("eu-west-1", "azure-tenant-elsewhere"),
            UNRECOGNIZED_PRINCIPAL_MESSAGE.to_string(),
        ),
        (
            "a connector of this account that was never created",
            state::connector_arn(&emulator.region, "azure-tenant-never-created"),
            UNRECOGNIZED_PRINCIPAL_MESSAGE.to_string(),
        ),
        (
            "no ARN at all",
            "connector-1".to_string(),
            state::invalid_connector_arn_message("connector-1"),
        ),
    ];

    for (case, connector_arn, expected) in &absent {
        let message = refused_with(
            &emulator,
            case,
            PUT_THIRD_PARTY,
            &put_request(principal, connector_arn, &scope),
            "ValidationException",
        )
        .await;
        assert_eq!(
            &message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        let recorders = described_recorders(&emulator).await;
        assert!(
            recorders.is_empty(),
            "case {case:?}: a refused request must persist no recorder, got {recorders:?}"
        );
    }

    // The same request succeeds once that connector exists, so the refusals above are
    // about existence and not about the shape of the request.
    let connector = seed_connector(&emulator, "tenant-now-created");
    let created = emulator
        .ok(PUT_THIRD_PARTY, &put_request(principal, &connector, &scope))
        .await;
    assert_eq!(
        created["Name"],
        json!(state::service_linked_recorder_name(principal)
            .expect("the supported principal names a service-linked recorder")),
        "case \"the connector exists\": expected the recorder to be created, got {created}"
    );
}

#[tokio::test]
async fn a_repeat_call_updates_the_scope_and_another_connector_conflicts() {
    let emulator = Emulator::new();
    let principal = state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0];
    let first_connector = seed_connector(&emulator, "tenant-first");
    let second_connector = seed_connector(&emulator, "tenant-second");

    let mut request = put_request(
        principal,
        &first_connector,
        &json!({ "scopeType": "tenant", "allRegions": true }),
    );
    request["Tags"] = json!([{ "Key": "owner", "Value": "security" }]);
    let created = emulator.ok(PUT_THIRD_PARTY, &request).await;

    // Rows differ along what the repeated call asks to record.
    let updates: [(&str, Value); 3] = [
        (
            "narrowed to one subscription",
            json!({ "scopeType": "subscription", "scopeValues": ["sub-1111"], "allRegions": true }),
        ),
        (
            "narrowed to two regions",
            json!({
                "scopeType": "subscription",
                "scopeValues": ["sub-1111"],
                "allRegions": false,
                "includedRegions": ["eastus", "westus"],
            }),
        ),
        (
            "widened back to the whole tenant",
            json!({ "scopeType": "tenant", "allRegions": true }),
        ),
    ];

    for (case, scope) in updates {
        let mut repeated = put_request(principal, &first_connector, &scope);
        // Tags are added at creation and are not updated by this operation, so a
        // repeat carrying other tags must not change the ones the recorder was
        // created with.
        repeated["Tags"] = json!([{ "Key": "owner", "Value": "someone-else" }]);
        let again = emulator.ok(PUT_THIRD_PARTY, &repeated).await;
        assert_eq!(
            (again["Arn"].clone(), again["Name"].clone()),
            (created["Arn"].clone(), created["Name"].clone()),
            "case {case:?}: a repeat call updates the one recorder instead of creating a \
             second, got {again}"
        );

        let described = emulator
            .ok(
                "DescribeConfigurationRecorders",
                &json!({ "ServicePrincipal": principal }),
            )
            .await;
        let recorder = described["ConfigurationRecorders"][0].clone();
        assert_eq!(
            recorder["scopeConfiguration"], scope,
            "case {case:?}: the repeat call updates the recorded scope, got {described}"
        );
        assert_eq!(
            recorder["connectorArn"], json!(first_connector),
            "case {case:?}: the connector the recorder is linked to does not change, got \
             {described}"
        );

        // Tags are added at creation and are not updated here, read back through
        // `ListTagsForResource`.
        let listed = emulator
            .ok(
                "ListTagsForResource",
                &json!({ "ResourceArn": created["Arn"].clone() }),
            )
            .await;
        let tags = listed["Tags"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            tags.len(),
            1,
            "case {case:?}: the recorder keeps exactly the tags it was created with, got \
             {listed}"
        );
        assert_eq!(
            (tags[0]["Key"].clone(), tags[0]["Value"].clone()),
            (json!("owner"), json!("security")),
            "case {case:?}: tags are added at creation and are not updated here, got {listed}"
        );
    }

    // A second connector for the same service cannot have a recorder of its own.
    refused_with(
        &emulator,
        "a different connector for the same service",
        PUT_THIRD_PARTY,
        &put_request(
            principal,
            &second_connector,
            &json!({ "scopeType": "tenant", "allRegions": true }),
        ),
        "ConflictException",
    )
    .await;
    let recorders = described_recorders(&emulator).await;
    assert_eq!(
        recorders.len(),
        1,
        "case \"a different connector for the same service\": the conflict leaves the one \
         recorder in place, got {recorders:?}"
    );
    assert_eq!(
        recorders[0]["connectorArn"], json!(first_connector),
        "case \"a different connector for the same service\": the recorder stays linked to \
         its connector, got {recorders:?}"
    );
}

#[tokio::test]
async fn a_service_linked_recorder_is_not_this_account_s_customer_managed_recorder() {
    let emulator = Emulator::new();
    let principal = state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0];
    let connector = seed_connector(&emulator, "tenant-separation");
    emulator
        .ok(
            PUT_THIRD_PARTY,
            &put_request(
                principal,
                &connector,
                &json!({ "scopeType": "tenant", "allRegions": true }),
            ),
        )
        .await;
    let linked_name = state::service_linked_recorder_name(principal)
        .expect("the supported principal names a service-linked recorder");

    // A delivery channel delivers for a CUSTOMER MANAGED recorder, and this account
    // has none yet: the service-linked recorder does not stand in for one.
    refused_with(
        &emulator,
        "a delivery channel with only a service-linked recorder",
        "PutDeliveryChannel",
        &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
        "NoAvailableConfigurationRecorderException",
    )
    .await;

    // Nor does it occupy this account's single customer managed recorder slot.
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;

    // Neither start nor stop affects a service-linked recorder, which is always
    // recording, and it is deleted only through its own operation.
    for (case, operation, code) in [
        (
            "starting a service-linked recorder",
            "StartConfigurationRecorder",
            "UnmodifiableEntityException",
        ),
        (
            "stopping a service-linked recorder",
            "StopConfigurationRecorder",
            "UnmodifiableEntityException",
        ),
        (
            "deleting a service-linked recorder",
            "DeleteConfigurationRecorder",
            "UnmodifiableEntityException",
        ),
    ] {
        refused_with(
            &emulator,
            case,
            operation,
            &json!({ "ConfigurationRecorderName": linked_name }),
            code,
        )
        .await;
    }

    let status = emulator
        .ok(
            "DescribeConfigurationRecorderStatus",
            &json!({ "ServicePrincipal": principal }),
        )
        .await;
    assert_eq!(
        status["ConfigurationRecordersStatus"][0]["recording"],
        json!(true),
        "case \"after the refused start and stop\": the service-linked recorder is still \
         recording, got {status}"
    );

    let recorders = described_recorders(&emulator).await;
    assert_eq!(
        recorders.len(),
        2,
        "case \"both recorder classes\": the account holds its customer managed recorder \
         beside the service-linked one, got {recorders:?}"
    );
}
