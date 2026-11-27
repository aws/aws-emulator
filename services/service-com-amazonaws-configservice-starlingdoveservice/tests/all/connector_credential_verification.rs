// SPDX-License-Identifier: CC0-1.0

//! What `PutConnector` answers a provider configuration, end to end over the wire.
//!
//! Four rules are pinned here, each over a TABLE of configurations rather than one
//! recorded case:
//!
//! 1. a `ConnectorConfiguration` that names no provider is refused with the modelled
//!    `ValidationException` and the one fixed structural text, whatever way it fails
//!    to name one, and so is one that names SEVERAL: "you must specify exactly one
//!    provider configuration";
//! 2. a provider identifier whose FORM is not the provider's own — an Azure tenant or
//!    client identifier is a GUID in its canonical hyphenated form — is refused with
//!    `ValidationException` naming the identifier that is not one, before any
//!    credential is used;
//! 3. a well-formed provider configuration is refused because its credentials cannot
//!    be verified WITH the provider: a connector IS the credential Config connects
//!    with, so it is verified before anything is materialized, and verifying it means
//!    calling the provider's identity service — an outbound connection this emulator
//!    does not make. The refusal names the provider whose credentials could not be
//!    verified, because each provider's credential is its own. Nothing is created by any
//!    of the legs: no ARN comes back, the connector is readable nowhere and no listing
//!    reports it, and the refusal happens BEFORE the conflict and quota legs this
//!    operation also models;
//! 4. the same holds for either published provider, so a Google Cloud configuration
//!    reaches the Google Cloud credential boundary and is refused there.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` builds; every case
//! drives the emulator the way a client does, through the `MultiServiceRouter`. The
//! connectors a case needs in state are seeded through `service_state`
//! (`config_connectors::seed_connector`), the way the other suites of this crate seed
//! state, because `PutConnector` creates none.

use serde_json::{json, Value};

use crate::config_connectors::{
    azure_configuration, gcp_configuration, refusal, seed_connector, AZURE_IDENTIFIER_MAX_LEN,
    ORGANIZATION_ID, POOL_PROVIDER, SERVICE_ACCOUNT,
};
use crate::config_setup_and_rule_loop::Emulator;
use service_com_amazonaws_configservice_starlingdoveservice::state;

/// The operation under test.
const PUT_CONNECTOR: &str = "PutConnector";

/// The texts AWS answers each refusal leg of this operation with, written out here
/// rather than read from the handler's own constants, so this suite PINS them instead
/// of restating them. The three legs share one error code, so the text is the only
/// thing that identifies the leg.
const NO_PROVIDER_TEXT: &str = "ConnectorConfiguration must contain at least one provider";
/// See [`NO_PROVIDER_TEXT`].
const SEVERAL_PROVIDERS_TEXT: &str = "ConnectorConfiguration must contain exactly one provider";
/// See [`NO_PROVIDER_TEXT`].
const INVALID_TENANT_TEXT: &str = "Invalid tenant identifier";
/// See [`NO_PROVIDER_TEXT`].
const INVALID_CLIENT_TEXT: &str = "Invalid client identifier";
/// See [`NO_PROVIDER_TEXT`].
const UNVERIFIABLE_TEXT: &str =
    "Unable to verify Azure credentials for the provided tenant configuration.";
/// See [`NO_PROVIDER_TEXT`]. The Google Cloud leg of [`UNVERIFIABLE_TEXT`].
const UNVERIFIABLE_GCP_TEXT: &str =
    "Unable to verify Google Cloud credentials for the provided service account configuration.";

/// A GUID in the canonical hyphenated form Microsoft Entra ID (Azure AD) publishes a
/// directory (tenant) identifier in. The model constrains these identifiers only by
/// length (`config_connectors::AZURE_IDENTIFIER_MAX_LEN`, from
/// `smithy_query(query="shape", shape="AzureTenantIdentifier")`: 1..128, no
/// `@pattern`), which is why the FORM is the handler's to check.
const TENANT_GUID: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

/// A second one, for the application (client) identifier.
const CLIENT_GUID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";

/// Call `PutConnector` with `configuration` and answer the HTTP status, the error code
/// and the message of the refusal it must be.
async fn refused(emulator: &Emulator, configuration: &Value) -> (u16, String, String) {
    refusal(
        emulator,
        PUT_CONNECTOR,
        &json!({ "ConnectorConfiguration": configuration }),
    )
    .await
}

/// Every connector ARN `ListConnectors` reports for this emulator's Region.
async fn listed_arns(emulator: &Emulator) -> Vec<String> {
    let listed = emulator.ok("ListConnectors", &json!({})).await;
    listed["ConnectorSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListConnectors must answer ConnectorSummaries, got {listed}"))
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("a ConnectorSummary must carry an arn, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The ARN a connector for `tenant` would be keyed by in this emulator's Region, used
/// to check that a refused `PutConnector` left nothing readable behind.
fn would_be_arn(emulator: &Emulator, tenant: &str) -> String {
    state::connector_arn(
        &emulator.region,
        &state::connector_name(state::PROVIDER_AZURE, tenant),
    )
}

#[tokio::test]
async fn a_configuration_naming_no_provider_is_refused_with_the_structural_text() {
    let emulator = Emulator::new();

    // `ConnectorConfiguration`: "You must specify exactly one provider configuration."
    // Rows differ along HOW the configuration fails to name one: it carries no member
    // at all, it carries a member for a provider the model does not publish
    // (`Provider` publishes `AZURE` and `GCP`), and it carries a published provider's
    // member under a name the model does not declare — both of the last two are members
    // the wire drops, so what reaches the handler names no provider.
    let cases: [(&str, Value); 3] = [
        ("an empty configuration", json!({})),
        (
            "a provider the model does not publish",
            json!({ "oracle": { "tenantIdentifier": TENANT_GUID } }),
        ),
        (
            "the provider member under a name the model does not declare",
            json!({ "Azure": { "tenantIdentifier": TENANT_GUID, "clientIdentifier": CLIENT_GUID } }),
        ),
    ];

    for (case, configuration) in cases {
        let (status, code, message) = refused(&emulator, &configuration).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, NO_PROVIDER_TEXT,
            "case {case:?}: expected the structural text, got {message:?}"
        );
    }

    let arns = listed_arns(&emulator).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}

#[tokio::test]
async fn an_identifier_that_is_not_the_providers_own_form_is_refused_naming_it() {
    let emulator = Emulator::new();

    // Every row is a value that is NOT a GUID in its canonical hyphenated form, so it
    // names no tenant and no client at the provider. They differ along the form of the
    // deviation: not an identifier at all, a GUID with a group missing, one with an
    // extra group, a non-hexadecimal digit, the spellings the hyphenated form is not
    // (simple, braced, URN), padding around an otherwise canonical one, and a value at
    // the model's length maximum.
    let not_guids: [(&str, String); 10] = [
        ("not an identifier at all", "not-a-guid".to_string()),
        (
            "a GUID with a group missing",
            "9188040d-6c67-4c5b-b112".to_string(),
        ),
        (
            "a GUID with an extra group",
            format!("{TENANT_GUID}-36a304b66dad"),
        ),
        (
            "a non-hexadecimal digit",
            "9188040d-6c67-4c5b-b112-36a304b66dag".to_string(),
        ),
        (
            "the simple (unhyphenated) spelling",
            TENANT_GUID.replace('-', ""),
        ),
        ("the braced spelling", format!("{{{TENANT_GUID}}}")),
        ("the URN spelling", format!("urn:uuid:{TENANT_GUID}")),
        ("a leading space", format!(" {TENANT_GUID}")),
        ("a trailing newline", format!("{TENANT_GUID}\n")),
        (
            "the model's length maximum",
            "a".repeat(AZURE_IDENTIFIER_MAX_LEN),
        ),
    ];

    for (case, value) in &not_guids {
        // The same deviation on each identifier: the refusal names the identifier that
        // is not one, and the tenant — what the credential belongs to — is checked
        // first, so a request where both deviate is refused for the tenant.
        for (member, configuration, expected) in [
            (
                "tenantIdentifier",
                azure_configuration(value, CLIENT_GUID),
                INVALID_TENANT_TEXT,
            ),
            (
                "clientIdentifier",
                azure_configuration(TENANT_GUID, value),
                INVALID_CLIENT_TEXT,
            ),
            (
                "both identifiers",
                azure_configuration(value, value),
                INVALID_TENANT_TEXT,
            ),
        ] {
            let (status, code, message) = refused(&emulator, &configuration).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?} at {member:?}: expected HTTP 400 ValidationException, \
                 got {status} {code} with message {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} at {member:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    let arns = listed_arns(&emulator).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}

#[tokio::test]
async fn a_well_formed_configuration_is_refused_because_its_credentials_are_unverifiable() {
    let emulator = Emulator::new();

    // Every row is a configuration whose identifiers ARE the provider's own form, so
    // each is past the structural and form legs and reaches the credential
    // verification. They differ along the identifiers themselves: distinct GUIDs, the
    // same GUID for both members, the nil GUID, the maximum GUID, and an uppercase
    // spelling of the canonical form (which is the same identifier, hexadecimal being
    // case-insensitive).
    let configurations: [(&str, String, String); 5] = [
        (
            "a tenant and a client",
            TENANT_GUID.to_string(),
            CLIENT_GUID.to_string(),
        ),
        (
            "the same GUID for both",
            TENANT_GUID.to_string(),
            TENANT_GUID.to_string(),
        ),
        (
            "the nil GUID",
            "00000000-0000-0000-0000-000000000000".to_string(),
            CLIENT_GUID.to_string(),
        ),
        (
            "the maximum GUID",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            CLIENT_GUID.to_string(),
        ),
        (
            "an uppercase canonical GUID",
            TENANT_GUID.to_uppercase(),
            CLIENT_GUID.to_uppercase(),
        ),
    ];

    for (case, tenant, client) in &configurations {
        let (status, code, message) = refused(&emulator, &azure_configuration(tenant, client)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, UNVERIFIABLE_TEXT,
            "case {case:?}: expected the unverifiable-credential text, got {message:?}"
        );

        // Nothing was materialized: the connector the configuration would have been
        // connected by is readable nowhere.
        let arn = would_be_arn(&emulator, tenant);
        for operation in ["GetConnector", "DeleteConnector"] {
            let (status, code) = emulator.refused(operation, &json!({ "Arn": arn })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?} / {operation:?}: the refused configuration must have \
                 created no connector, got {status} {code}"
            );
        }
    }

    let arns = listed_arns(&emulator).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}

#[tokio::test]
async fn the_credential_boundary_is_reached_before_the_conflict_and_quota_legs() {
    let emulator = Emulator::new();

    // AWS validates the configuration and verifies its credentials BEFORE it
    // materializes anything, so the legs that look at what is already stored — the one
    // connector a configuration can have, and the account's connector quota — are past
    // that boundary. Rows differ along what the store holds when the request arrives:
    // a connector for the very configuration the request carries, and a store filled
    // to the quota.
    let seeded = seed_connector(&emulator, TENANT_GUID, CLIENT_GUID);
    let (status, code, message) =
        refused(&emulator, &azure_configuration(TENANT_GUID, CLIENT_GUID)).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"a connector already exists for the configuration\": the credential \
         boundary is reached first, got {status} {code} with message {message:?}"
    );
    assert_eq!(
        message, UNVERIFIABLE_TEXT,
        "case \"a connector already exists for the configuration\": expected the \
         unverifiable-credential text, got {message:?}"
    );

    for index in 1..state::CONNECTORS_QUOTA {
        seed_connector(&emulator, &format!("{TENANT_GUID}-{index:03}"), CLIENT_GUID);
    }
    let arns = listed_arns(&emulator).await;
    assert_eq!(
        arns.len(),
        state::CONNECTORS_QUOTA,
        "the store must hold the quota's worth of connectors before the next case, got \
         {} of them",
        arns.len()
    );

    for (case, configuration, expected) in [
        (
            "a store at the quota",
            azure_configuration(CLIENT_GUID, TENANT_GUID),
            UNVERIFIABLE_TEXT,
        ),
        (
            "a store at the quota and a configuration naming no provider",
            json!({}),
            NO_PROVIDER_TEXT,
        ),
        (
            "a store at the quota and an identifier of the wrong form",
            azure_configuration("not-a-guid", CLIENT_GUID),
            INVALID_TENANT_TEXT,
        ),
    ] {
        let (status, code, message) = refused(&emulator, &configuration).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Every refusal left the stored connectors alone, the seeded one included.
    let after = listed_arns(&emulator).await;
    assert_eq!(
        after.len(),
        state::CONNECTORS_QUOTA,
        "a refused PutConnector must not change what a listing reports, got {} \
         connector(s)",
        after.len()
    );
    assert!(
        after.contains(&seeded),
        "the connector in state must survive every refusal, got {after:?}"
    );
}

#[tokio::test]
async fn exactly_one_provider_configuration_is_what_reaches_a_credential_boundary() {
    let emulator = Emulator::new();

    // (case, the configuration the request carries, the text it must be refused with)
    //
    // Rows differ along how many providers the configuration names — none, both, one —
    // and, for the rows naming one, along which provider's credential boundary they
    // reach and along what that provider's configuration carries: each provider's
    // credential is its own, so the refusal names it.
    let other_service_account = "config-reader@analytics-project.iam.gserviceaccount.com";
    let longest_organization = "9".repeat(20);
    let cases: [(&str, Value, &str); 7] = [
        ("no provider at all", json!({}), NO_PROVIDER_TEXT),
        (
            "both providers at once",
            json!({
                "azure": { "tenantIdentifier": TENANT_GUID, "clientIdentifier": CLIENT_GUID },
                "gcp": {
                    "serviceAccountEmail": SERVICE_ACCOUNT,
                    "workloadIdentityPoolProvider": POOL_PROVIDER,
                },
            }),
            SEVERAL_PROVIDERS_TEXT,
        ),
        (
            "the Azure provider alone",
            azure_configuration(TENANT_GUID, CLIENT_GUID),
            UNVERIFIABLE_TEXT,
        ),
        (
            "the Google Cloud provider with an organization",
            gcp_configuration(SERVICE_ACCOUNT, Some(ORGANIZATION_ID)),
            UNVERIFIABLE_GCP_TEXT,
        ),
        (
            "the Google Cloud provider without an organization",
            gcp_configuration(SERVICE_ACCOUNT, None),
            UNVERIFIABLE_GCP_TEXT,
        ),
        (
            "the Google Cloud provider at the organization id length maximum",
            gcp_configuration(other_service_account, Some(&longest_organization)),
            UNVERIFIABLE_GCP_TEXT,
        ),
        (
            "the Google Cloud provider at the organization id length minimum",
            gcp_configuration(other_service_account, Some("7")),
            UNVERIFIABLE_GCP_TEXT,
        ),
    ];

    for (case, configuration, expected) in cases {
        let (status, code, message) = refused(&emulator, &configuration).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A Google Cloud identifier outside the form its member's `@pattern` publishes is
    // refused by the modelled constraint, before any credential is used. Rows differ
    // along which member deviates and how.
    for (case, configuration) in [
        (
            "a service account that is not an IAM service account address",
            gcp_configuration("config-recorder@example.com", None),
        ),
        (
            "a workload identity pool provider that is not a provider resource name",
            json!({
                "gcp": {
                    "serviceAccountEmail": SERVICE_ACCOUNT,
                    "workloadIdentityPoolProvider": "projects/123456789012/providers/config",
                },
            }),
        ),
        (
            "an organization id past the modelled length maximum",
            gcp_configuration(SERVICE_ACCOUNT, Some(&"9".repeat(21))),
        ),
    ] {
        let (status, code, message) = refused(&emulator, &configuration).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code} \
             with message {message:?}"
        );
    }

    // None of the legs created anything: no connector of either provider is readable.
    let arns = listed_arns(&emulator).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}
