// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeletePaymentConnector` (`DELETE
//! /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`), and what a
//! later `GetPaymentConnector` reports of the deletion it accepted.
//!
//! The rules under test:
//!
//! * a deletion is ACCEPTED, not carried out — the response's `status` is "set to
//!   DELETING when deletion is initiated" (`smithy_query doc
//!   shape=DeletePaymentConnectorResponse member=status`) and the call answers 202, so the
//!   connector stays readable reporting `DELETING` until the removal the clock owes it
//!   comes due, and only then is every read of it the modelled 404;
//! * a connector going away takes no further write: an update of it is refused the
//!   modelled `ConflictException` (HTTP 409);
//! * `@idempotent` — a repeat while the deletion is under way reports the same connector
//!   going away rather than pushing its removal further out;
//! * only the connector the path addresses is affected: its siblings on the same manager,
//!   and the manager itself, are left exactly as they were — and once the connector is
//!   really gone, the manager it was holding back can be deleted;
//! * a 404 of this route names the CONNECTOR the caller asked the operation to act on,
//!   whether the gap is the connector or the manager that would hold it, in the one
//!   sentence this plane reports a missing connector in.
//!
//! Every wait below polls for the CONDITION it is about (the status, or the connector
//! being gone) until it holds or a generous deadline passes, so nothing here depends on
//! how fast the host runs.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. Secrets Manager is mounted
//! beside this service on ONE engine, the shape `core/router/src/lib.rs` mounts them in,
//! so every credential provider a `MANUAL` connector names is one this account really
//! holds.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// How long a wait for a clock-owed transition or removal may take before the test gives
/// up. Far longer than the span the service gives either, so a loaded host cannot fail a
/// row; every wait below waits for the CONDITION, never for a span to pass.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How often such a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `PaymentConnectorStatus` members this suite expects (`smithy_query
/// shape=PaymentConnectorStatus`).
const STATUS_CREATING: &str = "CREATING";
const STATUS_UPDATING: &str = "UPDATING";
const STATUS_DELETING: &str = "DELETING";
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

/// A Coinbase CDP `apiKeySecret` in the key format that vendor issues one in: the base64
/// of an Ed25519 private key, which `CreatePaymentCredentialProvider` refuses a managed
/// value outside of.
const CDP_API_KEY_SECRET: &str =
    "YWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjYWJjZA==";

/// An IAM role ARN inside the modelled `RoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$` (`smithy_query
/// operation=CreatePaymentManager`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/PaymentManagerRole";

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256, pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

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

    /// `DeletePaymentConnector`: `DELETE
    /// /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`, with the
    /// `clientToken` the model carries in the query string.
    async fn delete_connector(
        &self,
        payment_manager_id: &str,
        payment_connector_id: &str,
        client_token: Option<&str>,
    ) -> Answer {
        let path =
            format!("/payments/managers/{payment_manager_id}/connectors/{payment_connector_id}");
        let uri = match client_token {
            Some(token) => format!("{path}?clientToken={token}"),
            None => path,
        };
        self.call("DELETE", &uri, None).await
    }

    /// `GetPaymentManager`: `GET /payments/managers/{paymentManagerId}`.
    async fn get_manager(&self, payment_manager_id: &str) -> Answer {
        self.call("GET", &format!("/payments/managers/{payment_manager_id}"), None)
            .await
    }

    /// `DeletePaymentManager`: `DELETE /payments/managers/{paymentManagerId}`.
    async fn delete_manager(&self, payment_manager_id: &str) -> Answer {
        self.call("DELETE", &format!("/payments/managers/{payment_manager_id}"), None)
            .await
    }

    /// Read the connector again until it reports `wanted`, or the generous
    /// [`POLL_DEADLINE`] passes. The transition is owed by the clock, so this waits for
    /// the CONDITION rather than for a span.
    async fn poll_until_status(
        &self,
        case: &str,
        payment_manager_id: &str,
        payment_connector_id: &str,
        wanted: &str,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
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

    /// Read the connector again until it is GONE — the modelled 404 — or the generous
    /// [`POLL_DEADLINE`] passes. The removal is owed by the clock, so this waits for the
    /// CONDITION rather than for a span, and every read before it reports the connector
    /// still going away.
    async fn poll_until_gone(
        &self,
        case: &str,
        payment_manager_id: &str,
        payment_connector_id: &str,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
        loop {
            let read = self
                .get_connector(payment_manager_id, payment_connector_id)
                .await;
            if read.status == 404 {
                return read.body;
            }
            assert_eq!(
                (read.status, text(&read.body, "status").as_str()),
                (200, STATUS_DELETING),
                "case {case:?}: a connector whose deletion is under way reads back 200 \
                 {STATUS_DELETING} until it is gone, got {} ({})",
                read.status,
                read.body
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: the connector {payment_connector_id:?} was never removed"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what
/// the handlers read the ARN Region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
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

/// Assert the modelled not-found refusal for a payment connector: the resource the
/// request's route addresses, with the `paymentConnectorId` that path carried, which is
/// the one sentence every operation of this route reports a missing connector in.
fn assert_connector_not_found(case: &str, id: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected the modelled 404 for the connector {id:?}, got {status} ({body})"
    );
    assert_eq!(
        text(body, "message"),
        format!("Payment connector not found: {id}"),
        "case {case:?}: unexpected not-found message, got {body}"
    );
}

/// What a connector the delete is asked of looks like, and the status the delete finds it
/// in.
struct Row {
    case: &'static str,
    name: &'static str,
    connector_type: &'static str,
    /// The union member the configuration is sent in, or `None` for a `QUICK_CREATE`
    /// connector, whose credential provider the service provisions after consent.
    member: Option<&'static str>,
    /// The mode the request spells out, or `None` to leave it to its documented default
    /// (`MANUAL`).
    provision_mode: Option<&'static str>,
    /// The status the connector is waited into before the delete, or `None` to delete it
    /// while its own create is still in flight.
    settled_into: Option<&'static str>,
    /// A `PATCH` applied straight before the delete, so the delete arrives while a write
    /// of the connector's own is still in flight.
    updated: Option<Value>,
    /// The `clientToken` the delete carries, or `None` for a delete that sends none.
    client_token: Option<String>,
}

/// The rows of the acceptance table, spanning what the rule generalises over: how the
/// connector was provisioned (`MANUAL`, the documented default for a request that names
/// no mode, and `QUICK_CREATE`), which vendor it integrates with, the status the delete
/// finds it in (`READY`, `CREATING` while its create is still in flight, `UPDATING` while
/// an update is, `PENDING_AUTHENTICATION` while it waits for consent), the length of its
/// name, and whether the delete carries an idempotency token.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a settled Coinbase CDP connector provisioned manually",
            name: "checkoutConnector",
            connector_type: TYPE_COINBASE_CDP,
            member: Some(MEMBER_COINBASE_CDP),
            provision_mode: Some(MODE_MANUAL),
            settled_into: Some(STATUS_READY),
            updated: None,
            client_token: None,
        },
        Row {
            case: "a settled Stripe Privy connector in the documented default mode",
            name: "payoutConnector",
            connector_type: TYPE_STRIPE_PRIVY,
            member: Some(MEMBER_STRIPE_PRIVY),
            provision_mode: None,
            settled_into: Some(STATUS_READY),
            updated: Some(json!({"description": "the updated description"})),
            client_token: Some(client_token("payoutDelete")),
        },
        Row {
            case: "a connector whose own create has not settled yet",
            name: "creatingConnector",
            connector_type: TYPE_COINBASE_CDP,
            member: Some(MEMBER_COINBASE_CDP),
            provision_mode: Some(MODE_MANUAL),
            settled_into: None,
            updated: None,
            client_token: Some(client_token("creatingDelete")),
        },
        Row {
            case: "a connector waiting for an end user's OAuth consent",
            name: "consentConnector",
            connector_type: TYPE_STRIPE_PRIVY,
            member: None,
            provision_mode: Some(MODE_QUICK_CREATE),
            settled_into: Some(STATUS_PENDING_AUTHENTICATION),
            updated: None,
            client_token: None,
        },
        Row {
            case: "a one-letter name",
            name: "c",
            connector_type: TYPE_COINBASE_CDP,
            member: Some(MEMBER_COINBASE_CDP),
            provision_mode: Some(MODE_MANUAL),
            settled_into: Some(STATUS_READY),
            updated: None,
            client_token: Some(client_token("shortDelete")),
        },
    ]
}

/// An accepted deletion answers 202 with the connector `DELETING`, the connector reads
/// back `DELETING` while the removal is still owed, and it is gone once the clock owes
/// that removal.
#[tokio::test]
async fn an_accepted_deletion_reports_deleting_and_then_the_connector_is_gone() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("deletionManager").await;
    let cdp_arn = emulator
        .create_credential_provider("deletionCdp", TYPE_COINBASE_CDP)
        .await;
    let privy_arn = emulator
        .create_credential_provider("deletionPrivy", TYPE_STRIPE_PRIVY)
        .await;

    // Every row's connector is created and deleted before any row waits for its removal,
    // so the windows the clock owes run together.
    let mut deleted = Vec::new();
    for row in rows() {
        let mut request = json!({"name": row.name, "type": row.connector_type});
        request["credentialProviderConfigurations"] = match row.member {
            Some(MEMBER_STRIPE_PRIVY) => configuration(MEMBER_STRIPE_PRIVY, &privy_arn),
            Some(member) => configuration(member, &cdp_arn),
            None => json!([]),
        };
        if let Some(mode) = row.provision_mode {
            request["provisionMode"] = json!(mode);
        }
        let created = emulator.create_connector(&manager_id, &request).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: expected HTTP 202 from CreatePaymentConnector, got {} ({})",
            row.case, created.status, created.body
        );
        let connector_id = text(&created.body, "paymentConnectorId");

        if let Some(wanted) = row.settled_into {
            emulator
                .poll_until_status(row.case, &manager_id, &connector_id, wanted)
                .await;
        }
        if let Some(patch) = &row.updated {
            let updated = emulator
                .update_connector(&manager_id, &connector_id, patch)
                .await;
            assert_eq!(
                (updated.status, text(&updated.body, "status").as_str()),
                (202, STATUS_UPDATING),
                "case {:?}: the update must be accepted with the connector {STATUS_UPDATING}, got \
                 {} ({})",
                row.case,
                updated.status,
                updated.body
            );
        }

        let accepted = emulator
            .delete_connector(&manager_id, &connector_id, row.client_token.as_deref())
            .await;
        assert_eq!(
            accepted.status, 202,
            "case {:?}: expected HTTP 202 from DeletePaymentConnector, got {} ({})",
            row.case, accepted.status, accepted.body
        );
        assert_eq!(
            text(&accepted.body, "status"),
            STATUS_DELETING,
            "case {:?}: an accepted deletion reports {STATUS_DELETING}, got {}",
            row.case,
            accepted.body
        );
        assert_eq!(
            text(&accepted.body, "paymentConnectorId"),
            connector_id,
            "case {:?}: the response names the connector the path addressed, got {}",
            row.case,
            accepted.body
        );

        // A caller acting straight after the delete still sees the connector going away:
        // the removal is the clock's, and the span is never zero.
        let read = emulator.get_connector(&manager_id, &connector_id).await;
        assert_eq!(
            (read.status, text(&read.body, "status").as_str()),
            (200, STATUS_DELETING),
            "case {:?}: a read straight after the delete reports the connector \
             {STATUS_DELETING}, got {} ({})",
            row.case,
            read.status,
            read.body
        );
        deleted.push((row.case, connector_id));
    }

    for (case, connector_id) in deleted {
        let gone = emulator
            .poll_until_gone(case, &manager_id, &connector_id)
            .await;
        assert_connector_not_found(case, &connector_id, 404, &gone);

        // Past the removal the connector is gone, so a further delete of it is the
        // modelled 404 rather than a second acceptance.
        let refused = emulator
            .delete_connector(&manager_id, &connector_id, None)
            .await;
        assert_connector_not_found(case, &connector_id, refused.status, &refused.body);
    }
}

/// `DeletePaymentConnector` is `@idempotent`: a repeat while the deletion is under way
/// reports the same connector going away, and does not push its removal further out.
#[tokio::test]
async fn a_repeated_delete_reports_the_same_deletion_and_does_not_re_arm_the_clock() {
    let emulator = Emulator::new();
    let case = "the repeated delete";
    let manager_id = emulator.create_manager("repeatedManager").await;
    let arn = emulator
        .create_credential_provider("repeatedCdp", TYPE_COINBASE_CDP)
        .await;

    let created = emulator
        .create_connector(
            &manager_id,
            &json!({
                "name": "repeatedConnector",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations": configuration(MEMBER_COINBASE_CDP, &arn),
            }),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the connector must be created, got {} ({})",
        created.status, created.body
    );
    let connector_id = text(&created.body, "paymentConnectorId");
    emulator
        .poll_until_status(case, &manager_id, &connector_id, STATUS_READY)
        .await;

    // The same token twice, then a different token, then none at all: every repeat is the
    // same accepted deletion.
    let tokens = [
        Some(client_token("repeatedDelete")),
        Some(client_token("repeatedDelete")),
        Some(client_token("anotherDelete")),
        None,
    ];
    for (attempt, token) in tokens.into_iter().enumerate() {
        let accepted = emulator
            .delete_connector(&manager_id, &connector_id, token.as_deref())
            .await;
        assert_eq!(
            (accepted.status, text(&accepted.body, "status").as_str()),
            (202, STATUS_DELETING),
            "case {case:?}: delete attempt {attempt} must report the same accepted deletion, got \
             {} ({})",
            accepted.status,
            accepted.body
        );
        assert_eq!(
            text(&accepted.body, "paymentConnectorId"),
            connector_id,
            "case {case:?}: delete attempt {attempt} must name the same connector, got {}",
            accepted.body
        );
    }

    // The repeats did not re-arm the removal: it still comes due, within the deadline.
    let gone = emulator
        .poll_until_gone(case, &manager_id, &connector_id)
        .await;
    assert_connector_not_found(case, &connector_id, 404, &gone);
}

/// A connector whose deletion is already under way is going away, so it takes no further
/// write: an update of it is refused the modelled 409, in the sentence that states what
/// the connector is doing.
#[tokio::test]
async fn a_connector_being_deleted_accepts_no_further_write() {
    let emulator = Emulator::new();
    let case = "a connector already going away";
    let manager_id = emulator.create_manager("doomedManager").await;
    let arn = emulator
        .create_credential_provider("doomedCdp", TYPE_COINBASE_CDP)
        .await;

    let created = emulator
        .create_connector(
            &manager_id,
            &json!({
                "name": "doomedConnector",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations": configuration(MEMBER_COINBASE_CDP, &arn),
            }),
        )
        .await;
    let connector_id = text(&created.body, "paymentConnectorId");
    emulator
        .poll_until_status(case, &manager_id, &connector_id, STATUS_READY)
        .await;
    let accepted = emulator
        .delete_connector(&manager_id, &connector_id, None)
        .await;
    assert_eq!(
        (accepted.status, text(&accepted.body, "status").as_str()),
        (202, STATUS_DELETING),
        "case {case:?}: the deletion must be accepted, got {} ({})",
        accepted.status,
        accepted.body
    );

    let refused = emulator
        .update_connector(
            &manager_id,
            &connector_id,
            &json!({"description": "too late"}),
        )
        .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (409, "ConflictException"),
        "case {case:?}: a connector being deleted cannot be updated, got {} ({})",
        refused.status,
        refused.body
    );
    assert_eq!(
        text(&refused.body, "message"),
        format!("Payment connector {connector_id} is in {STATUS_DELETING} status and cannot be \
                 modified."),
        "case {case:?}: unexpected conflict message, got {}",
        refused.body
    );

    // The refused update changed nothing: the connector is still going away, and the
    // removal still comes due.
    let read = emulator.get_connector(&manager_id, &connector_id).await;
    assert_eq!(
        (read.status, text(&read.body, "status").as_str()),
        (200, STATUS_DELETING),
        "case {case:?}: the refused update must leave the connector going away, got {} ({})",
        read.status,
        read.body
    );
    let gone = emulator
        .poll_until_gone(case, &manager_id, &connector_id)
        .await;
    assert_connector_not_found(case, &connector_id, 404, &gone);
}

/// Only the connector the path addresses is deleted: its sibling on the same manager, and
/// the manager itself, are left exactly as they were. And a connector is gone from the
/// manager for real — once its removal comes due, the manager it was holding back can be
/// deleted, which "All payment connectors associated with the payment manager must be
/// deleted before the payment manager can be deleted" refuses while it is still there.
#[tokio::test]
async fn deleting_one_connector_leaves_its_siblings_and_frees_the_manager() {
    let emulator = Emulator::new();
    let case = "one of two connectors deleted";
    let manager_id = emulator.create_manager("siblingManager").await;
    let cdp_arn = emulator
        .create_credential_provider("siblingCdp", TYPE_COINBASE_CDP)
        .await;
    let privy_arn = emulator
        .create_credential_provider("siblingPrivy", TYPE_STRIPE_PRIVY)
        .await;

    let mut connectors = Vec::new();
    for (name, connector_type, member, arn) in [
        (
            "deletedConnector",
            TYPE_COINBASE_CDP,
            MEMBER_COINBASE_CDP,
            &cdp_arn,
        ),
        (
            "keptConnector",
            TYPE_STRIPE_PRIVY,
            MEMBER_STRIPE_PRIVY,
            &privy_arn,
        ),
    ] {
        let created = emulator
            .create_connector(
                &manager_id,
                &json!({
                    "name": name,
                    "type": connector_type,
                    "credentialProviderConfigurations": configuration(member, arn),
                }),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the connector {name:?} must be created, got {} ({})",
            created.status, created.body
        );
        connectors.push(text(&created.body, "paymentConnectorId"));
    }
    let (deleted_id, kept_id) = (connectors[0].clone(), connectors[1].clone());
    emulator
        .poll_until_status(case, &manager_id, &kept_id, STATUS_READY)
        .await;

    let accepted = emulator
        .delete_connector(&manager_id, &deleted_id, None)
        .await;
    assert_eq!(
        (accepted.status, text(&accepted.body, "status").as_str()),
        (202, STATUS_DELETING),
        "case {case:?}: the deletion must be accepted, got {} ({})",
        accepted.status,
        accepted.body
    );

    // The sibling is untouched: still readable, still reporting what it was created with.
    let kept = emulator.get_connector(&manager_id, &kept_id).await;
    assert_eq!(
        (kept.status, text(&kept.body, "status").as_str()),
        (200, STATUS_READY),
        "case {case:?}: the sibling connector must be untouched, got {} ({})",
        kept.status,
        kept.body
    );
    assert_eq!(
        text(&kept.body, "name"),
        "keptConnector",
        "case {case:?}: the sibling connector must still report its own name, got {}",
        kept.body
    );

    // A connector still held — even one going away — still holds its manager's deletion
    // back, so the manager delete is refused while it is there.
    let refused = emulator.delete_manager(&manager_id).await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (409, "ConflictException"),
        "case {case:?}: a manager still holding connectors cannot be deleted, got {} ({})",
        refused.status,
        refused.body
    );

    // Once the deleted connector's removal is due, only the sibling is left: the manager
    // reports the one connector still on it, and after that one is gone too the manager's
    // own deletion is accepted.
    let gone = emulator
        .poll_until_gone(case, &manager_id, &deleted_id)
        .await;
    assert_connector_not_found(case, &deleted_id, 404, &gone);
    let refused = emulator.delete_manager(&manager_id).await;
    assert_eq!(
        text(&refused.body, "message"),
        "Payment manager still contains 1 payment connector and cannot be deleted",
        "case {case:?}: the refusal must count only the connectors still held, got {}",
        refused.body
    );

    let accepted = emulator.delete_connector(&manager_id, &kept_id, None).await;
    assert_eq!(
        accepted.status, 202,
        "case {case:?}: the sibling's deletion must be accepted, got {} ({})",
        accepted.status, accepted.body
    );
    emulator
        .poll_until_gone(case, &manager_id, &kept_id)
        .await;
    let accepted = emulator.delete_manager(&manager_id).await;
    assert_eq!(
        accepted.status, 202,
        "case {case:?}: a manager holding no connector any more can be deleted, got {} ({})",
        accepted.status, accepted.body
    );

    // The manager is still readable while its own removal is owed, and it reports the
    // deletion it accepted.
    let read = emulator.get_manager(&manager_id).await;
    assert_eq!(
        (read.status, text(&read.body, "status").as_str()),
        (200, STATUS_DELETING),
        "case {case:?}: the manager must report its accepted deletion, got {} ({})",
        read.status,
        read.body
    );
}

/// A delete addressing a payment connector this account holds none of is the modelled
/// 404: a manager that was never created, a connector that manager never held, and a
/// connector created on ANOTHER manager — a connector is addressed only through the
/// manager that holds it.
#[tokio::test]
async fn a_delete_of_an_absent_connector_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("absentManager").await;
    let other_manager_id = emulator.create_manager("otherManager").await;
    let arn = emulator
        .create_credential_provider("absentCdp", TYPE_COINBASE_CDP)
        .await;
    let created = emulator
        .create_connector(
            &manager_id,
            &json!({
                "name": "elsewhereConnector",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations": configuration(MEMBER_COINBASE_CDP, &arn),
            }),
        )
        .await;
    let connector_id = text(&created.body, "paymentConnectorId");

    // Ids of the modelled forms `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` (the manager) and
    // `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$` (the connector) that this account holds no
    // resource under.
    let absent_connectors = [
        (
            "a connector id no create ever minted",
            "nosuchconnector-0123456789",
        ),
        ("the shortest id the model admits", "a-0123456789"),
        (
            "an id carrying the separators and underscore the pattern admits",
            "a-b_c-connector-9876543210",
        ),
    ];
    for (case, id) in absent_connectors {
        let refused = emulator.delete_connector(&manager_id, id, None).await;
        assert_connector_not_found(case, id, refused.status, &refused.body);
        // A read of the same connector answers exactly the same sentence: the operation
        // asking cannot appear in it.
        let read = emulator.get_connector(&manager_id, id).await;
        assert_connector_not_found(case, id, read.status, &read.body);
    }

    // A connector of ANOTHER manager is not this manager's to delete, and the connector it
    // names is left alone.
    let refused = emulator
        .delete_connector(&other_manager_id, &connector_id, None)
        .await;
    assert_connector_not_found(
        "a connector created on another manager",
        &connector_id,
        refused.status,
        &refused.body,
    );
    let read = emulator.get_connector(&manager_id, &connector_id).await;
    assert_eq!(
        read.status, 200,
        "the connector must survive a delete addressed to another manager, got {} ({})",
        read.status, read.body
    );
    assert_ne!(
        text(&read.body, "status"),
        STATUS_DELETING,
        "the connector must not be going away, got {}",
        read.body
    );
    assert!(
        [STATUS_CREATING, STATUS_READY].contains(&text(&read.body, "status").as_str()),
        "the connector must still be in a status of its own create, got {}",
        read.body
    );

    // A manager this account holds none of holds no connector under the id the path
    // carries either, and this route addresses ONE connector: the connector is the
    // resource the operation was asked to delete, so it is the one reported missing,
    // naming the pair the path carried.
    let absent_manager = "nosuchmanager-0123456789";
    let refused = emulator
        .delete_connector(absent_manager, &connector_id, None)
        .await;
    assert_connector_not_found(
        "a delete under a manager this account holds none of",
        &connector_id,
        refused.status,
        &refused.body,
    );
}
