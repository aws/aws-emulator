// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdatePaymentConnector` (`PATCH
//! /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`), and what a
//! later `GetPaymentConnector` reports of what it wrote.
//!
//! The rules under test:
//!
//! * PATCH semantics — "you only need to specify the fields you want to change"
//!   (`smithy_query doc shape=UpdatePaymentConnector`): a member the request carries
//!   replaces the stored one, a member it omits leaves the stored one alone, and a member
//!   the update's own response does not declare (the description, the provision mode, the
//!   instant of the create) is reported by the read;
//! * the pair a connector authenticates with has to stay consistent — a configuration is
//!   carried in the union member that names the connector's own `type`, and the credential
//!   provider it names has to be one this account's token vault holds, read against the
//!   pair the PATCH would leave behind rather than against the request alone;
//! * one write at a time — a connector whose own create or update has not taken effect is
//!   still being written, so a second writer is refused the modelled `ConflictException`
//!   (HTTP 409) and changes nothing;
//! * an update is asynchronous — it answers 202 with the connector `UPDATING`, and the
//!   move back to the status it was in is owed by the clock rather than by the call, so
//!   every wait below POLLS for the status instead of sleeping a fixed span. The status it
//!   returns to is the connector's own: one whose credentials the service provisions after
//!   OAuth consent goes back to waiting for that consent, which no update grants.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. Secrets Manager is mounted
//! beside this service on ONE engine, the shape `core/router/src/lib.rs` mounts them in, so
//! every credential provider a connector names is one this account really holds.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// How long a wait for a clock-owed transition may take before the test gives up. Far
/// longer than the span the service gives the transition, so a loaded host cannot fail this
/// suite; the test waits for the STATUS, never for a span to pass.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often that wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `PaymentConnectorStatus` members this suite expects (`smithy_query
/// shape=PaymentConnectorStatus`).
const STATUS_CREATING: &str = "CREATING";
const STATUS_UPDATING: &str = "UPDATING";
const STATUS_READY: &str = "READY";
const STATUS_PENDING_AUTHENTICATION: &str = "PENDING_AUTHENTICATION";

/// `PaymentConnectorProvisionMode`'s full value list, `MANUAL | QUICK_CREATE`
/// (`smithy_query shape=PaymentConnectorProvisionMode`).
const MODE_MANUAL: &str = "MANUAL";
const MODE_QUICK_CREATE: &str = "QUICK_CREATE";

/// `PaymentConnectorType`'s full value list, `CoinbaseCDP | StripePrivy` (`smithy_query
/// shape=PaymentConnectorType`), and the `CredentialsProviderConfiguration` union member
/// each type's configuration is carried in (`smithy_query
/// shape=CredentialsProviderConfiguration`).
const TYPE_COINBASE_CDP: &str = "CoinbaseCDP";
const TYPE_STRIPE_PRIVY: &str = "StripePrivy";
const MEMBER_COINBASE_CDP: &str = "coinbaseCDP";
const MEMBER_STRIPE_PRIVY: &str = "stripePrivy";

/// `PaymentsDescription`'s published bounds, `len 1..4096` (`smithy_query
/// operation=UpdatePaymentConnector`).
const MIN_DESCRIPTION_LEN: usize = 1;
const MAX_DESCRIPTION_LEN: usize = 4096;

/// A Coinbase CDP `apiKeySecret` in the key format that vendor issues one in: the base64 of
/// an Ed25519 private key, which `CreatePaymentCredentialProvider` refuses a managed value
/// outside of.
const CDP_API_KEY_SECRET: &str =
    "YWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjZA==";

/// An IAM role ARN inside the modelled `RoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$` (`smithy_query
/// operation=CreatePaymentManager`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/PaymentManagerRole";

/// A `PaymentCredentialProviderArn` of the modelled form that no create ever minted, so no
/// provider of this account's token vault reports it.
const GHOST_PROVIDER_ARN: &str = "arn:aws:bedrock-agentcore:us-east-1:000000000000:\
     token-vault/default/paymentcredentialprovider/ghost";

/// The sentence this service refuses a credential provider the account does not hold with.
const PROVIDER_NOT_FOUND: &str = "PaymentCredentialProvider not found";

/// The sentence a request that would leave a connector with no credential provider
/// configuration is refused with.
const CONFIGURATIONS_NOT_DEFINED: &str = "Credential provider configurations is not defined";

/// This service and Secrets Manager over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// One `restJson1` request of this service, signed for [`REGION`].
    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization())
            .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: if response.body().is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(response.body()).unwrap_or(Value::Null)
            },
        }
    }

    /// `CreatePaymentManager`: `POST /payments/managers`.
    async fn create_manager(&self, name: &str) -> String {
        let created = self
            .call(
                "POST",
                "/payments/managers",
                Some(&json!({"name": name, "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN})),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "the payment manager {name:?} must be created, got {} ({})",
            created.status, created.body
        );
        text(&created.body, "paymentManagerId")
    }

    /// `CreatePaymentCredentialProvider` for `vendor`, and the ARN the created provider
    /// reports — the only place a configuration's `credentialProviderArn` comes from.
    async fn create_credential_provider(&self, name: &str, vendor: &str) -> String {
        let configuration = if vendor == TYPE_COINBASE_CDP {
            json!({"coinbaseCdpConfiguration": {
                "apiKeyId": format!("{name}-api-key"),
                "apiKeySecret": CDP_API_KEY_SECRET,
                "walletSecret": "d2FsbGV0LXNlY3JldA==",
            }})
        } else {
            json!({"stripePrivyConfiguration": {
                "appId": format!("{name}-app"),
                "authorizationId": format!("{name}-authorization"),
                "appSecret": "cHJpdnktYXBwLXNlY3JldA==",
                "authorizationPrivateKey": "wallet-auth:cHJpdmF0ZS1rZXk=",
            }})
        };
        let created = self
            .call(
                "POST",
                "/identities/CreatePaymentCredentialProvider",
                Some(&json!({
                    "name": name,
                    "credentialProviderVendor": vendor,
                    "providerConfigurationInput": configuration,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "the {vendor} credential provider {name:?} must be created, got {} ({})",
            created.status, created.body
        );
        text(&created.body, "credentialProviderArn")
    }

    /// `CreatePaymentConnector`: `POST /payments/managers/{paymentManagerId}/connectors`.
    async fn create_connector(&self, payment_manager_id: &str, body: &Value) -> Answer {
        self.call(
            "POST",
            &format!("/payments/managers/{payment_manager_id}/connectors"),
            Some(body),
        )
        .await
    }

    /// `UpdatePaymentConnector`: `PATCH
    /// /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`.
    async fn update_connector(
        &self,
        payment_manager_id: &str,
        payment_connector_id: &str,
        body: &Value,
    ) -> Answer {
        self.call(
            "PATCH",
            &format!("/payments/managers/{payment_manager_id}/connectors/{payment_connector_id}"),
            Some(body),
        )
        .await
    }

    /// `GetPaymentConnector`: `GET
    /// /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`.
    async fn get_connector(&self, payment_manager_id: &str, payment_connector_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/payments/managers/{payment_manager_id}/connectors/{payment_connector_id}"),
            None,
        )
        .await
    }

    /// Create one connector of `connector_type` and wait for it to settle, so a row updates
    /// a connector that is not already being written. Reports its `paymentConnectorId`.
    async fn settled_connector(
        &self,
        case: &str,
        payment_manager_id: &str,
        name: &str,
        body: Value,
    ) -> String {
        let created = self.create_connector(payment_manager_id, &body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreatePaymentConnector, got {} ({})",
            created.status, created.body
        );
        let id = text(&created.body, "paymentConnectorId");
        assert!(
            !id.is_empty(),
            "case {case:?}: the create of {name:?} reported no paymentConnectorId, got {}",
            created.body
        );
        let settled = if text(&created.body, "status") == STATUS_PENDING_AUTHENTICATION {
            STATUS_PENDING_AUTHENTICATION
        } else {
            STATUS_READY
        };
        self.poll_until_status(case, payment_manager_id, &id, settled)
            .await;
        id
    }

    /// Read the connector again until it reports `wanted`, or the generous
    /// [`SETTLE_DEADLINE`] passes. The transition is owed by the clock, so this waits for
    /// the CONDITION rather than for a span.
    async fn poll_until_status(
        &self,
        case: &str,
        payment_manager_id: &str,
        payment_connector_id: &str,
        wanted: &str,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let read = self
                .get_connector(payment_manager_id, payment_connector_id)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetPaymentConnector for \
                 {payment_connector_id:?}, got {} ({})",
                read.status, read.body
            );
            if text(&read.body, "status") == wanted {
                return read.body;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: the connector {payment_connector_id:?} never reached {wanted}, \
                 last status {:?}",
                text(&read.body, "status")
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what the
/// handlers read the ARN Region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One configuration of the `CredentialsProviderConfigurations` list: the union member the
/// vendor's configuration is carried in, holding the ARN of a credential provider.
fn configuration(member: &str, credential_provider_arn: &str) -> Value {
    json!([{ member: {"credentialProviderArn": credential_provider_arn} }])
}

/// The create body of a `MANUAL` connector of `connector_type` authenticating through
/// `credential_provider_arn`.
fn manual_create_body(
    name: &str,
    connector_type: &str,
    member: &str,
    credential_provider_arn: &str,
    description: &str,
) -> Value {
    json!({
        "name": name,
        "type": connector_type,
        "provisionMode": MODE_MANUAL,
        "description": description,
        "credentialProviderConfigurations": configuration(member, credential_provider_arn),
    })
}

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256, pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC 3339
/// form, naming a real instant. `lastUpdatedAt` targets `DateTimestamp`, which carries
/// `@timestampFormat("date-time")` (`smithy_query operation=UpdatePaymentConnector`), so an
/// epoch-seconds number is not the modelled type.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// An update changes exactly the members its request carried, and the read reports the
/// result; everything else the connector holds is left as it was.
///
/// The rows differ along which members the PATCH carries and what they hold: the description
/// alone, the description at each published bound of `PaymentsDescription` (`len 1..4096`),
/// the credential provider configuration alone, the payment provider together with the
/// configuration that matches it — in both directions across `PaymentConnectorType`'s whole
/// value list — two members at once, and a request that carries no changeable member at all.
/// The expectation is kept as this test's OWN model of the PATCH rule, applied row by row,
/// so the read is compared against the accumulated effect rather than against one row's
/// request.
#[tokio::test]
async fn an_update_changes_only_the_members_it_carries_and_the_read_reports_them() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("storefront").await;
    let coinbase_arn = emulator
        .create_credential_provider("coinbase-first", TYPE_COINBASE_CDP)
        .await;
    let other_coinbase_arn = emulator
        .create_credential_provider("coinbase-second", TYPE_COINBASE_CDP)
        .await;
    let privy_arn = emulator
        .create_credential_provider("privy-first", TYPE_STRIPE_PRIVY)
        .await;

    let connector_id = emulator
        .settled_connector(
            "the connector the rows patch",
            &manager_id,
            "patchedConnector",
            manual_create_body(
                "patchedConnector",
                TYPE_COINBASE_CDP,
                MEMBER_COINBASE_CDP,
                &coinbase_arn,
                "the created description",
            ),
        )
        .await;

    // This test's own model of the stored connector, against which every read is checked.
    let mut expected_description = json!("the created description");
    let mut expected_type = json!(TYPE_COINBASE_CDP);
    let mut expected_configurations = configuration(MEMBER_COINBASE_CDP, &coinbase_arn);

    let longest_description = "d".repeat(MAX_DESCRIPTION_LEN);
    let shortest_description = "s".repeat(MIN_DESCRIPTION_LEN);

    // (case, the members the PATCH carries)
    let rows: [(&str, Value); 7] = [
        (
            "the description alone",
            json!({"description": "the second description"}),
        ),
        (
            "the longest description the model admits",
            json!({"description": longest_description}),
        ),
        (
            "the shortest description the model admits",
            json!({"description": shortest_description}),
        ),
        (
            "another credential provider of the same vendor, alone",
            json!({
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &other_coinbase_arn)
            }),
        ),
        (
            "the other payment provider with the configuration that matches it",
            json!({
                "type": TYPE_STRIPE_PRIVY,
                "credentialProviderConfigurations":
                    configuration(MEMBER_STRIPE_PRIVY, &privy_arn),
            }),
        ),
        (
            "back to the first payment provider, with its configuration",
            json!({
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &coinbase_arn),
            }),
        ),
        ("no changeable member at all", json!({})),
    ];

    for (case, patch) in rows {
        let before = emulator.get_connector(&manager_id, &connector_id).await.body;
        let updated = emulator
            .update_connector(&manager_id, &connector_id, &patch)
            .await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202 from UpdatePaymentConnector, got {} ({})",
            updated.status, updated.body
        );
        assert_eq!(
            text(&updated.body, "status"),
            STATUS_UPDATING,
            "case {case:?}: an accepted update reports the connector UPDATING, got {}",
            updated.body
        );
        assert_date_time(case, "lastUpdatedAt", &updated.body["lastUpdatedAt"]);

        if let Some(description) = patch.get("description") {
            expected_description = description.clone();
        }
        if let Some(connector_type) = patch.get("type") {
            expected_type = connector_type.clone();
        }
        if let Some(configurations) = patch.get("credentialProviderConfigurations") {
            expected_configurations = configurations.clone();
        }

        // `UpdatePaymentConnectorResponse` declares the identity, the type, the
        // configurations, the instant of this write and the status — and no description,
        // provision mode or `createdAt`, which the read reports.
        assert_eq!(
            (
                updated.body["paymentConnectorId"].clone(),
                updated.body["paymentManagerId"].clone(),
                updated.body["name"].clone(),
                updated.body["type"].clone(),
                updated.body["credentialProviderConfigurations"].clone(),
            ),
            (
                json!(connector_id),
                json!(manager_id),
                before["name"].clone(),
                expected_type.clone(),
                expected_configurations.clone(),
            ),
            "case {case:?}: the update reports the patched connector, got {}",
            updated.body
        );

        let settled = emulator
            .poll_until_status(case, &manager_id, &connector_id, STATUS_READY)
            .await;
        assert_eq!(
            (
                settled["description"].clone(),
                settled["type"].clone(),
                settled["credentialProviderConfigurations"].clone(),
            ),
            (
                expected_description.clone(),
                expected_type.clone(),
                expected_configurations.clone(),
            ),
            "case {case:?}: the read must report the patched connector, got {settled}"
        );
        // Members no update carried are the create's still, including the ones the
        // update's own response does not declare.
        assert_eq!(
            (
                settled["name"].clone(),
                settled["createdAt"].clone(),
                settled["provisionMode"].clone(),
            ),
            (
                before["name"].clone(),
                before["createdAt"].clone(),
                json!(MODE_MANUAL),
            ),
            "case {case:?}: the update changed a member it did not carry, got {settled}"
        );
        assert_eq!(
            settled["lastUpdatedAt"],
            updated.body["lastUpdatedAt"],
            "case {case:?}: the read must report the instant of the write the update \
             reported, got {settled}"
        );
    }
}

/// A connector whose credentials the service provisions after OAuth consent goes back to
/// waiting for that consent once the update takes effect, and keeps publishing the address
/// the consent is completed at: no update grants a consent that is an end user's act.
///
/// The rows differ along what the PATCH carries: the description alone, and a body that
/// re-states the empty configuration list such a connector carries — accepted for it, where
/// the same body is refused for a connector carrying the caller's own credentials.
#[tokio::test]
async fn an_update_of_a_connector_awaiting_consent_leaves_it_awaiting_consent() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("quickstore").await;
    let connector_id = emulator
        .settled_connector(
            "the connector awaiting consent",
            &manager_id,
            "quickConnector",
            json!({
                "name": "quickConnector",
                "type": TYPE_COINBASE_CDP,
                "provisionMode": MODE_QUICK_CREATE,
                "credentialProviderConfigurations": [],
            }),
        )
        .await;
    let before = emulator.get_connector(&manager_id, &connector_id).await.body;
    let published = text(&before, "authorizationUrl");
    assert!(
        published.starts_with("https://"),
        "a connector awaiting consent publishes the address it is completed at, got {before}"
    );

    // (case, the members the PATCH carries, the description the read must report)
    let rows: [(&str, Value, Value); 2] = [
        (
            "the description alone",
            json!({"description": "waiting for consent"}),
            json!("waiting for consent"),
        ),
        (
            "the empty configuration list such a connector carries",
            json!({"credentialProviderConfigurations": []}),
            json!("waiting for consent"),
        ),
    ];

    for (case, patch, description) in rows {
        let updated = emulator
            .update_connector(&manager_id, &connector_id, &patch)
            .await;
        assert_eq!(
            (updated.status, text(&updated.body, "status")),
            (202, STATUS_UPDATING.to_string()),
            "case {case:?}: expected HTTP 202 with the connector UPDATING, got {} ({})",
            updated.status,
            updated.body
        );

        let settled = emulator
            .poll_until_status(
                case,
                &manager_id,
                &connector_id,
                STATUS_PENDING_AUTHENTICATION,
            )
            .await;
        assert_eq!(
            (
                settled["description"].clone(),
                settled["authorizationUrl"].clone(),
                settled["credentialProviderConfigurations"].clone(),
                settled["credentialsUpdatedAt"].clone(),
            ),
            (description, json!(published), json!([]), Value::Null),
            "case {case:?}: the connector must go back to waiting for the consent it \
             published, carrying no provisioned credentials, got {settled}"
        );
    }
}

/// Every update this operation refuses leaves the connector exactly as it was: a manager
/// this account holds none under, a connector that manager does not hold, a connector whose
/// own create has not taken effect, a PATCH that would leave a connector carrying the
/// caller's own credentials with none, one that would leave its type and its configuration
/// configuring different vendors, and one naming a credential provider this account's token
/// vault does not hold.
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("refusals").await;
    let coinbase_arn = emulator
        .create_credential_provider("coinbase-refusals", TYPE_COINBASE_CDP)
        .await;
    let connector_id = emulator
        .settled_connector(
            "the connector the refusals address",
            &manager_id,
            "refusedConnector",
            manual_create_body(
                "refusedConnector",
                TYPE_COINBASE_CDP,
                MEMBER_COINBASE_CDP,
                &coinbase_arn,
                "the created description",
            ),
        )
        .await;
    let before = emulator.get_connector(&manager_id, &connector_id).await.body;

    // Ids of the modelled forms (`^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` and its sibling)
    // that this account holds no manager and this manager no connector under.
    let absent_manager = "nosuchmanager-0123456789";
    let absent_connector = "nosuchconnector-0123456789";

    // (case, the manager, the connector, the PATCH, the status, the error, the message)
    let rows: [(&str, &str, &str, Value, u16, &str, String); 6] = [
        (
            "a manager this account holds none under",
            absent_manager,
            &connector_id,
            json!({"description": "never applied"}),
            404,
            "ResourceNotFoundException",
            format!("Payment connector not found: {connector_id}"),
        ),
        (
            "a connector this manager does not hold",
            &manager_id,
            absent_connector,
            json!({"description": "never applied"}),
            404,
            "ResourceNotFoundException",
            format!("Payment connector not found: {absent_connector}"),
        ),
        (
            "a PATCH that would leave the connector with no credential provider",
            &manager_id,
            &connector_id,
            json!({"credentialProviderConfigurations": []}),
            400,
            "ValidationException",
            CONFIGURATIONS_NOT_DEFINED.to_string(),
        ),
        (
            "the other payment provider, with the configuration of this one still stored",
            &manager_id,
            &connector_id,
            json!({"type": TYPE_STRIPE_PRIVY}),
            400,
            "ValidationException",
            format!(
                "credentialProviderConfigurations: configures {TYPE_COINBASE_CDP}, which is not \
                 this payment connector's type {TYPE_STRIPE_PRIVY}"
            ),
        ),
        (
            "a configuration carried in the member of the other payment provider",
            &manager_id,
            &connector_id,
            json!({
                "credentialProviderConfigurations":
                    configuration(MEMBER_STRIPE_PRIVY, &coinbase_arn)
            }),
            400,
            "ValidationException",
            format!(
                "credentialProviderConfigurations: configures {TYPE_STRIPE_PRIVY}, which is not \
                 this payment connector's type {TYPE_COINBASE_CDP}"
            ),
        ),
        (
            "a credential provider this account's token vault does not hold",
            &manager_id,
            &connector_id,
            json!({
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, GHOST_PROVIDER_ARN)
            }),
            404,
            "ResourceNotFoundException",
            PROVIDER_NOT_FOUND.to_string(),
        ),
    ];

    for (case, manager, connector, patch, status, error, message) in rows {
        let refused = emulator.update_connector(manager, connector, &patch).await;
        assert_eq!(
            (refused.status, error_code(&refused.body)),
            (status, error.to_string()),
            "case {case:?}: expected HTTP {status} {error}, got {} ({})",
            refused.status,
            refused.body
        );
        assert_eq!(
            text(&refused.body, "message"),
            message,
            "case {case:?}: unexpected refusal message, got {}",
            refused.body
        );
        let read = emulator.get_connector(&manager_id, &connector_id).await;
        assert_eq!(
            (read.status, read.body.clone()),
            (200, before.clone()),
            "case {case:?}: a refused update changed the connector, got {} ({})",
            read.status,
            read.body
        );
    }

    // A connector whose create has not taken effect yet is still being written, so a
    // second writer collides with it.
    let created = emulator
        .create_connector(
            &manager_id,
            &manual_create_body(
                "unsettledConnector",
                TYPE_COINBASE_CDP,
                MEMBER_COINBASE_CDP,
                &coinbase_arn,
                "the created description",
            ),
        )
        .await;
    assert_eq!(
        (created.status, text(&created.body, "status")),
        (202, STATUS_CREATING.to_string()),
        "the unsettled connector must be created CREATING, got {} ({})",
        created.status,
        created.body
    );
    let unsettled_id = text(&created.body, "paymentConnectorId");
    let refused = emulator
        .update_connector(
            &manager_id,
            &unsettled_id,
            &json!({"description": "while still creating"}),
        )
        .await;
    assert_eq!(
        (refused.status, error_code(&refused.body)),
        (409, "ConflictException".to_string()),
        "an update of a connector that is still being created is the modelled 409, got {} ({})",
        refused.status,
        refused.body
    );
    assert_eq!(
        text(&refused.body, "message"),
        format!(
            "Payment connector {unsettled_id} is in {STATUS_CREATING} status and cannot be \
             modified. Retry the request once its status is {STATUS_READY}."
        ),
        "unexpected conflict message for the unsettled connector, got {}",
        refused.body
    );
    let settled = emulator
        .poll_until_status(
            "the unsettled connector",
            &manager_id,
            &unsettled_id,
            STATUS_READY,
        )
        .await;
    assert_eq!(
        settled["description"],
        json!("the created description"),
        "the refused update changed the connector, got {settled}"
    );
}

/// A replayed `clientToken` is ignored rather than applied a second time: "If this token
/// matches a previous request, the service ignores the request, but doesn't return an
/// error."
///
/// The rows differ along what the replay carries: the identical body, and a body asking for
/// a different description — which the service must NOT apply.
#[tokio::test]
async fn a_replayed_client_token_applies_the_update_once() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("replays").await;
    let coinbase_arn = emulator
        .create_credential_provider("coinbase-replays", TYPE_COINBASE_CDP)
        .await;
    let connector_id = emulator
        .settled_connector(
            "the connector the replay addresses",
            &manager_id,
            "replayedConnector",
            manual_create_body(
                "replayedConnector",
                TYPE_COINBASE_CDP,
                MEMBER_COINBASE_CDP,
                &coinbase_arn,
                "the created description",
            ),
        )
        .await;

    let token = client_token("replayedConnectorUpdate");
    let first = json!({"description": "the applied description", "clientToken": token});
    let applied = emulator
        .update_connector(&manager_id, &connector_id, &first)
        .await;
    assert_eq!(
        applied.status, 202,
        "the first update must apply, got {} ({})",
        applied.status, applied.body
    );

    // (case, the body the replay carries)
    let rows: [(&str, Value); 2] = [
        ("the identical body", first.clone()),
        (
            "a body asking for another description",
            json!({"description": "never applied", "clientToken": token}),
        ),
    ];
    for (case, body) in rows {
        let replayed = emulator
            .update_connector(&manager_id, &connector_id, &body)
            .await;
        assert_eq!(
            replayed.status, 202,
            "case {case:?}: a replayed token answers rather than failing, got {} ({})",
            replayed.status, replayed.body
        );
        assert_eq!(
            (
                replayed.body["paymentConnectorId"].clone(),
                replayed.body["lastUpdatedAt"].clone(),
            ),
            (
                applied.body["paymentConnectorId"].clone(),
                applied.body["lastUpdatedAt"].clone(),
            ),
            "case {case:?}: the replay reports the connector the first update left, got {}",
            replayed.body
        );
    }

    let settled = emulator
        .poll_until_status(
            "the replayed connector",
            &manager_id,
            &connector_id,
            STATUS_READY,
        )
        .await;
    assert_eq!(
        settled["description"],
        json!("the applied description"),
        "the replays must not have applied a change of their own, got {settled}"
    );
}
