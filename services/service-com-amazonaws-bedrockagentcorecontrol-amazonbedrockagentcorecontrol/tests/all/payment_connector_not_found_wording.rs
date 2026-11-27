// SPDX-License-Identifier: CC0-1.0

//! How the payment-connector surface words a resource it cannot find, end to end over HTTP.
//!
//! ONE RULE: a `ResourceNotFoundException` (HTTP 404) on this surface names the resource
//! the request's ROUTE addresses, with that resource's identifier taken verbatim from the
//! path:
//!
//! * a route that addresses ONE connector —
//!   `/payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}` — reports
//!   "Payment connector not found: <paymentConnectorId>", whether the gap is the connector
//!   or the manager that would hold it, because the connector is what the caller asked the
//!   operation to act on;
//! * a route that addresses a manager's connector COLLECTION —
//!   `/payments/managers/{paymentManagerId}/connectors` — carries no connector id, so it
//!   reports its parent manager, under the `managerId=` label this surface names that
//!   parent by: "Payment manager not found: managerId=<paymentManagerId>";
//! * a route that addresses the manager RESOURCE itself — `/payments/managers/{id}` —
//!   keeps its own bare sentence, so neither wording spreads to the other.
//!
//! Sources: `smithy_query(query="operation",operation="GetPaymentConnector,\
//! UpdatePaymentConnector,CreatePaymentConnector,GetPaymentManager")` for the four routes,
//! their `@httpLabel` `@required` `paymentManagerId` / `paymentConnectorId` and the
//! modelled `ResourceNotFoundException` (httpError 404), and for the id constraints the
//! rows span: `PaymentConnectorId` is `length 12..211` with pattern
//! `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$` and `PaymentManagerId` `length 12..211` with
//! `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`. The model states no message text, so the
//! wording is the recorded one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// An IAM role ARN inside the modelled `RoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/PaymentManagerRole";

/// `PaymentConnectorType`'s value `CoinbaseCDP` and the
/// `CredentialsProviderConfiguration` union member its configuration is carried in
/// (`smithy_query shape=CredentialsProviderConfiguration`).
const TYPE_COINBASE_CDP: &str = "CoinbaseCDP";
const MEMBER_COINBASE_CDP: &str = "coinbaseCDP";

/// A Coinbase CDP `apiKeySecret` in the key format that vendor issues one in: the base64
/// of an Ed25519 private key, which `CreatePaymentCredentialProvider` refuses a managed
/// value outside of.
const CDP_API_KEY_SECRET: &str =
    "YWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjZA==";

/// One answer: the HTTP status, the error code `restJson1` puts in `__type`, and the parsed
/// body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

impl Answer {
    /// The `message` the body carries, or the empty string when it carries none.
    fn message(&self) -> String {
        text(&self.body, "message")
    }
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// This service and Secrets Manager over one shared engine, the shape
/// `core/router/src/lib.rs` mounts them in, so a credential provider a connector names is
/// one this account really holds.
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
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
                     aws4_request, SignedHeaders=host, Signature=abc"
                ),
            )
            .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer {
            status,
            code: body
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or("<no __type>")
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_string(),
            body,
        }
    }

    /// `CreatePaymentManager`: `POST /payments/managers`, and the id it minted.
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
            "setup: the payment manager {name:?} must be created, got {} ({})",
            created.status, created.body
        );
        text(&created.body, "paymentManagerId")
    }

    /// `CreatePaymentCredentialProvider` for Coinbase CDP, and the ARN the created provider
    /// reports — the only place a configuration's `credentialProviderArn` comes from.
    async fn create_credential_provider(&self, name: &str) -> String {
        let created = self
            .call(
                "POST",
                "/identities/CreatePaymentCredentialProvider",
                Some(&json!({
                    "name": name,
                    "credentialProviderVendor": TYPE_COINBASE_CDP,
                    "providerConfigurationInput": {"coinbaseCdpConfiguration": {
                        "apiKeyId": format!("{name}-api-key"),
                        "apiKeySecret": CDP_API_KEY_SECRET,
                        "walletSecret": "d2FsbGV0LXNlY3JldA==",
                    }},
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "setup: the credential provider {name:?} must be created, got {} ({})",
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

    /// `UpdatePaymentConnector`: `PATCH
    /// /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`, carrying the
    /// smallest body the operation accepts and no `clientToken`, so nothing but the path
    /// decides the answer.
    async fn update_connector(
        &self,
        payment_manager_id: &str,
        payment_connector_id: &str,
    ) -> Answer {
        self.call(
            "PATCH",
            &format!("/payments/managers/{payment_manager_id}/connectors/{payment_connector_id}"),
            Some(&json!({"description": "never applied"})),
        )
        .await
    }

    /// `GetPaymentManager`: `GET /payments/managers/{paymentManagerId}`.
    async fn get_manager(&self, payment_manager_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/payments/managers/{payment_manager_id}"),
            None,
        )
        .await
    }
}

/// A well-formed `CreatePaymentConnector` body for a connector of the Coinbase CDP type.
fn connector_create_body(name: &str, credential_provider_arn: &str) -> Value {
    json!({
        "name": name,
        "type": TYPE_COINBASE_CDP,
        "credentialProviderConfigurations": [
            {MEMBER_COINBASE_CDP: {"credentialProviderArn": credential_provider_arn}}
        ],
    })
}

/// The sentence a route that addresses ONE connector owes for a connector nothing resolves.
fn connector_not_found(payment_connector_id: &str) -> String {
    format!("Payment connector not found: {payment_connector_id}")
}

/// The sentence the connector COLLECTION's route owes for a parent manager this account
/// holds none under: the identifier under the label this surface names it by.
fn connector_parent_manager_not_found(payment_manager_id: &str) -> String {
    format!("Payment manager not found: managerId={payment_manager_id}")
}

/// The sentence a route that addresses the manager RESOURCE owes for the same id.
fn manager_not_found(payment_manager_id: &str) -> String {
    format!("Payment manager not found: {payment_manager_id}")
}

/// Assert one refusal: the modelled status, error code and message, naming the case and the
/// values actually answered.
fn assert_refusal(case: &str, operation: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?} ({operation}): a resource that is not there is the modelled 404 \
         ResourceNotFoundException, got {} {:?} with {}",
        answer.status,
        answer.code,
        answer.body
    );
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?} ({operation}): expected {expected:?}, got {:?}",
        answer.message()
    );
}

/// THE RULE for a route that addresses ONE connector: the connector is the resource
/// reported missing, by the `paymentConnectorId` the path carried, whichever operation asks
/// and whether the gap is the connector, its parent manager, or both.
///
/// The rows differ along WHICH id resolves to nothing (the connector, the manager, both),
/// the shape of the connector id (the shortest `PaymentConnectorId` the model admits, an
/// underscore stem — which `PaymentConnectorId`'s pattern admits where
/// `PaymentManagerId`'s does not — interior hyphens, a stored connector's stem with a
/// different suffix, and the longest stem the pattern's `{1,100}` repetition admits), and
/// the operation addressing it (`GetPaymentConnector`, `UpdatePaymentConnector`), which
/// must answer identically.
#[tokio::test]
async fn an_operation_addressing_one_connector_reports_the_connector_id_it_was_given() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("storedManager").await;
    let provider_arn = emulator.create_credential_provider("coinbase-wording").await;

    // A connector that IS held, so a row can address it through a manager that is not
    // there, and the suite can prove no refusal touched it.
    let created = emulator
        .create_connector(
            &manager_id,
            &connector_create_body("storedConnector", &provider_arn),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "setup: the connector the rows address must be created, got {} ({})",
        created.status, created.body
    );
    let stored_connector = text(&created.body, "paymentConnectorId");
    let (head, last) = stored_connector.split_at(stored_connector.len() - 1);
    let neighbour_connector = format!("{head}{}", if last == "a" { "b" } else { "a" });

    // Ids inside the modelled patterns that nothing created: `PaymentConnectorId` is
    // `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$` with `length 12..211`, so the shortest
    // admissible one is a single stem character, a hyphen and ten more, and the longest
    // stem the repetition admits is 100 characters; `PaymentManagerId` is the same shape
    // without the underscore.
    let shortest_connector = "a-0123456789".to_string();
    let longest_stem_connector = format!("{}-0123456789", "c".repeat(100));
    let absent_manager = "nosuchmanager-0123456789";

    // (case, the manager the path carries, the connector the path carries)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the shortest connector id the model admits, on a manager that is there",
            manager_id.clone(),
            shortest_connector,
        ),
        (
            "a connector id whose stem carries an underscore",
            manager_id.clone(),
            "my_connector-0123456789".to_string(),
        ),
        (
            "a connector id carrying interior hyphens",
            manager_id.clone(),
            "ab-cd-ef-0123456789".to_string(),
        ),
        (
            "the stored connector's stem with a different suffix",
            manager_id.clone(),
            neighbour_connector,
        ),
        (
            "the longest connector stem the pattern admits",
            manager_id.clone(),
            longest_stem_connector.clone(),
        ),
        (
            "a connector this account DOES hold, addressed through a manager it does not",
            absent_manager.to_string(),
            stored_connector.clone(),
        ),
        (
            "neither the manager nor the connector is there",
            absent_manager.to_string(),
            longest_stem_connector,
        ),
    ];

    for (case, manager, connector) in rows {
        let expected = connector_not_found(&connector);
        let read = emulator.get_connector(&manager, &connector).await;
        assert_refusal(case, "GetPaymentConnector", &read, &expected);
        let updated = emulator.update_connector(&manager, &connector).await;
        assert_refusal(case, "UpdatePaymentConnector", &updated, &expected);
        assert_eq!(
            updated.message(),
            read.message(),
            "case {case:?}: UpdatePaymentConnector must answer exactly what GetPaymentConnector \
             answers, so the asking operation cannot appear in the sentence, got {:?} against \
             {:?}",
            updated.message(),
            read.message()
        );
    }

    // None of the refusals above wrote or removed anything: the connector this manager
    // holds still reads back, with the description its create left it carrying.
    let held = emulator.get_connector(&manager_id, &stored_connector).await;
    assert_eq!(
        (
            held.status,
            text(&held.body, "paymentConnectorId").as_str()
        ),
        (200, stored_connector.as_str()),
        "case \"the connector this manager holds\": the refusals above changed nothing, got {} \
         with {}",
        held.status,
        held.body
    );
}

/// THE RULE for the route that addresses a manager's connector COLLECTION: it carries no
/// connector id, so the parent manager is the resource reported missing — under the
/// `managerId=` label this surface names it by — whatever the body carries.
///
/// The rows differ along the shape of the `paymentManagerId` the path carries (the shortest
/// the model admits, an all-digit stem, interior hyphens, a stored manager's stem with a
/// different suffix, the longest stem the pattern admits) and along the BODY: a create this
/// service would serve on a manager that is there, and one its own rule would refuse (an
/// empty `credentialProviderConfigurations`, which the model's `len 0..1` admits), so the
/// path's resource is reported whatever the body would have produced.
#[tokio::test]
async fn creating_a_connector_on_a_manager_nothing_holds_names_the_manager_under_its_label() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("collectionManager").await;
    let provider_arn = emulator.create_credential_provider("coinbase-collection").await;
    let (head, last) = manager_id.split_at(manager_id.len() - 1);
    let neighbour_manager = format!("{head}{}", if last == "a" { "b" } else { "a" });

    // `PaymentManagerId` is `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` with `length 12..211`.
    let shortest_manager = "a-0123456789".to_string();
    let longest_stem_manager = format!("{}-0123456789", "m".repeat(100));

    let bodies = [
        (
            "a body this service would serve",
            connector_create_body("servedBody", &provider_arn),
        ),
        (
            "a body this service's own rule would refuse",
            json!({
                "name": "refusedBody",
                "type": TYPE_COINBASE_CDP,
                "provisionMode": "MANUAL",
                "credentialProviderConfigurations": [],
            }),
        ),
    ];

    let managers: Vec<(&str, String)> = vec![
        (
            "the shortest manager id the model admits",
            shortest_manager,
        ),
        (
            "a manager id whose stem is all digits",
            "90210manager-0123456789".to_string(),
        ),
        (
            "a manager id carrying interior hyphens",
            "ab-cd-ef-0123456789".to_string(),
        ),
        (
            "the stored manager's stem with a different suffix",
            neighbour_manager,
        ),
        (
            "the longest manager stem the pattern admits",
            longest_stem_manager,
        ),
    ];

    for (manager_case, manager) in &managers {
        let expected = connector_parent_manager_not_found(manager);
        for (body_case, body) in &bodies {
            let case = format!("{manager_case}, with {body_case}");
            let refused = emulator.create_connector(manager, body).await;
            assert_refusal(&case, "CreatePaymentConnector", &refused, &expected);
        }

        // The labelled form belongs to the connector collection's route: the route that
        // addresses the manager RESOURCE answers the same id in its own bare sentence, so a
        // client branching on one wording is never answered the other.
        let read = emulator.get_manager(manager).await;
        assert_eq!(
            (read.status, read.code.as_str(), read.message().as_str()),
            (
                404,
                "ResourceNotFoundException",
                manager_not_found(manager).as_str()
            ),
            "case {manager_case:?} (GetPaymentManager): the manager's own route keeps its bare \
             sentence, got {} {:?} with {}",
            read.status,
            read.code,
            read.body
        );
    }

    // None of the refusals above wrote anything: the manager this account holds still takes
    // a well-formed create, and the connector it mints reads back.
    let created = emulator
        .create_connector(
            &manager_id,
            &connector_create_body("afterRefusals", &provider_arn),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case \"a well-formed create after the refusals\": expected HTTP 202, got {} ({})",
        created.status, created.body
    );
    let connector_id = text(&created.body, "paymentConnectorId");
    let read = emulator.get_connector(&manager_id, &connector_id).await;
    assert_eq!(
        (read.status, text(&read.body, "name").as_str()),
        (200, "afterRefusals"),
        "case \"the connector the refusals did not stop\": it must read back, got {} ({})",
        read.status,
        read.body
    );
}
