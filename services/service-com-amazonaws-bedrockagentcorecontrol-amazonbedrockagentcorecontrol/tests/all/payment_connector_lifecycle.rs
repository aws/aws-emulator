// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the payment connector lifecycle
//! (`CreatePaymentConnector`, `GetPaymentConnector`).
//!
//! The rules under test:
//!
//! * a create PERSISTS the connector on the payment manager its path names, and every read
//!   of that connector reports what was persisted — the payment provider it integrates
//!   with, the credential provider configuration it authenticates through, the description
//!   and how it was provisioned;
//! * `provisionMode` decides what the create leaves behind (`smithy_query doc
//!   shape=CreatePaymentConnectorRequest member=provisionMode`): `MANUAL` — the default for
//!   a request that names no mode — carries the credential provider the caller named and is
//!   `CREATING` with `READY` owed by the clock, while `QUICK_CREATE` waits for an end user's
//!   OAuth consent and so reports `PENDING_AUTHENTICATION` together with the
//!   `authorizationUrl` that consent is completed at, which this emulator publishes on its
//!   own endpoint and serves;
//! * a connector authenticates through a credential provider of this account's token
//!   vault, so a configuration naming no provider this account holds is refused and
//!   nothing is written;
//! * `clientToken` is idempotent per manager: a replayed create answers with the connector
//!   that token first produced.
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

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// How long a wait for a clock-owed transition may take before the test gives up. Far
/// longer than the span the service gives the transition, so a loaded host cannot fail this
/// suite; the test waits for the STATUS, never for a span to pass.
const READY_DEADLINE: Duration = Duration::from_secs(60);

/// How often that wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `PaymentConnectorStatus` members this suite expects (`smithy_query
/// shape=PaymentConnectorStatus`).
const STATUS_CREATING: &str = "CREATING";
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

/// The longest `PaymentConnectorName` the model admits: the pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` is one leading letter and up to 47 more characters
/// (`smithy_query operation=CreatePaymentConnector`).
const MAX_NAME_LEN: usize = 48;

/// `PaymentsDescription`'s published bounds, `len 1..4096` (`smithy_query
/// operation=CreatePaymentConnector`).
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

/// This service and Secrets Manager over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        Self::served_at(None)
    }

    /// The same emulator, reporting `base` as the endpoint it is served at — which is the
    /// address a published `authorizationUrl` has to name.
    fn served_at(base: Option<&str>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        if let Some(base) = base {
            router.set_url_base(
                smithy_server_runtime::EndpointUrl::parse(base)
                    .expect("a base URL the router accepts"),
            );
        }
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
            .header("authorization", authorization("bedrock-agentcore"))
            .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        answer(self.router.dispatch(request).await)
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

    /// `GetPaymentConnector`: `GET
    /// /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`.
    async fn get_connector(&self, payment_manager_id: &str, payment_connector_id: &str) -> Answer {
        self.call(
            "GET",
            &format!(
                "/payments/managers/{payment_manager_id}/connectors/{payment_connector_id}"
            ),
            None,
        )
        .await
    }

    /// Read the connector again until it reports `wanted`, or the generous
    /// [`READY_DEADLINE`] passes. The transition is owed by the clock, so this waits for
    /// the CONDITION rather than for a span.
    async fn poll_until_status(
        &self,
        case: &str,
        payment_manager_id: &str,
        payment_connector_id: &str,
        wanted: &str,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + READY_DEADLINE;
        loop {
            let read = self.get_connector(payment_manager_id, payment_connector_id).await;
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

    /// Fetch `url` the way a browser holding an `authorizationUrl` does: no credential, no
    /// target header, just the address the connector published.
    async fn fetch(&self, method: &str, url: &str) -> Page {
        let (host, path) = split_url(url);
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .body(Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Page {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            body: String::from_utf8_lossy(response.body()).into_owned(),
        }
    }
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

/// One page a client FETCHES: the status, the content type and the document.
struct Page {
    status: u16,
    content_type: String,
    body: String,
}

fn answer(response: http::Response<Bytes>) -> Answer {
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// A SigV4 `Authorization` header for `signing_name` in [`REGION`]. It is what the router
/// routes on, and what the handlers read the ARN Region from.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The host and the path of a published URL, as a browser splits one.
fn split_url(url: &str) -> (&str, &str) {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    match after_scheme.find('/') {
        Some(at) => (&after_scheme[..at], &after_scheme[at..]),
        None => (after_scheme, "/"),
    }
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

/// `PaymentConnectorId`'s pattern, `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$` (`smithy_query
/// shape=PaymentConnectorId`), checked by hand because this crate carries no regex engine:
/// the stem the id carries for `name` — which `PaymentConnectorName` allows upper case in
/// and the id's pattern does not — then a 10-character lower-case suffix.
fn is_modelled_connector_id(id: &str, name: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{}-", name.to_lowercase())) else {
        return false;
    };
    suffix.len() == 10
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC 3339
/// form, naming a real instant. `createdAt` and `lastUpdatedAt` target `DateTimestamp`,
/// which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetPaymentConnector`), so an epoch-seconds number is not the modelled type.
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

/// The address a connector's authorization page is published at, spelled out here rather
/// than taken from the service: the reserved namespace segment every emulator-served plane
/// is addressed under, the segment naming this service's payment-connector authorization
/// plane, and the connector addressed through its manager.
fn published_authorization_url(base: &str, manager_id: &str, connector_id: &str) -> String {
    format!(
        "{base}/_aws/bedrock-agentcore-payment-connector-authorization/{manager_id}/{connector_id}"
    )
}

/// One configuration of the `CredentialsProviderConfigurations` list: the union member the
/// vendor's configuration is carried in, holding the ARN of a credential provider.
fn configuration(member: &str, credential_provider_arn: &str) -> Value {
    json!([{ member: {"credentialProviderArn": credential_provider_arn} }])
}

/// One row of the create table: the request a connector is created from, and what the
/// create and the read must report of it.
struct Row {
    case: &'static str,
    name: String,
    /// The payment provider the connector integrates with, which is also the vendor its
    /// credential provider is created for: a configuration configures the provider of
    /// exactly that type.
    connector_type: &'static str,
    /// The union member the configuration is sent in, or `None` for a `QUICK_CREATE`
    /// connector, whose credential provider the service provisions after consent.
    member: Option<&'static str>,
    description: Option<String>,
    /// The mode the request spells out, or `None` to leave it to its documented default.
    provision_mode: Option<&'static str>,
    client_token: Option<String>,
    /// The status the create reports, and the one the connector settles into.
    created_status: &'static str,
    settled_status: &'static str,
}

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256, pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreatePaymentConnector`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Coinbase CDP connector provisioned manually, described and idempotent",
            name: "checkoutConnector".to_string(),
            connector_type: TYPE_COINBASE_CDP,
            member: Some(MEMBER_COINBASE_CDP),
            description: Some("the storefront's payment connector".to_string()),
            provision_mode: Some(MODE_MANUAL),
            client_token: Some(client_token("checkoutShape")),
            created_status: STATUS_CREATING,
            settled_status: STATUS_READY,
        },
        Row {
            case: "a Stripe Privy connector with no provisionMode, which defaults to MANUAL",
            name: "privyConnector".to_string(),
            connector_type: TYPE_STRIPE_PRIVY,
            member: Some(MEMBER_STRIPE_PRIVY),
            description: None,
            provision_mode: None,
            client_token: None,
            created_status: STATUS_CREATING,
            settled_status: STATUS_READY,
        },
        Row {
            case: "the longest name the model admits, carrying upper case and underscores",
            name: format!("n{}_{}", "A".repeat(MAX_NAME_LEN - 12), "b9".repeat(5)),
            connector_type: TYPE_COINBASE_CDP,
            member: Some(MEMBER_COINBASE_CDP),
            description: Some("x".repeat(MAX_DESCRIPTION_LEN)),
            provision_mode: Some(MODE_MANUAL),
            client_token: None,
            created_status: STATUS_CREATING,
            settled_status: STATUS_READY,
        },
        Row {
            case: "a one-letter name and the shortest description the model admits",
            name: "c".to_string(),
            connector_type: TYPE_STRIPE_PRIVY,
            member: Some(MEMBER_STRIPE_PRIVY),
            description: Some("y".repeat(MIN_DESCRIPTION_LEN)),
            provision_mode: Some(MODE_MANUAL),
            client_token: None,
            created_status: STATUS_CREATING,
            settled_status: STATUS_READY,
        },
        Row {
            case: "a Coinbase CDP connector the service provisions after OAuth consent",
            name: "quickCoinbase".to_string(),
            connector_type: TYPE_COINBASE_CDP,
            member: None,
            description: Some("provisioned by the service".to_string()),
            provision_mode: Some(MODE_QUICK_CREATE),
            client_token: Some(client_token("quickCoinbaseShape")),
            created_status: STATUS_PENDING_AUTHENTICATION,
            settled_status: STATUS_PENDING_AUTHENTICATION,
        },
        Row {
            case: "a Stripe Privy connector the service provisions after OAuth consent",
            name: "quickPrivy".to_string(),
            connector_type: TYPE_STRIPE_PRIVY,
            member: None,
            description: None,
            provision_mode: Some(MODE_QUICK_CREATE),
            client_token: None,
            created_status: STATUS_PENDING_AUTHENTICATION,
            settled_status: STATUS_PENDING_AUTHENTICATION,
        },
    ]
}

impl Row {
    /// The request body this row creates its connector from, naming `credential_provider_arn`
    /// when the row provides its own credentials.
    fn body(&self, credential_provider_arn: &str) -> Value {
        let configurations = match self.member {
            Some(member) => configuration(member, credential_provider_arn),
            // `credentialProviderConfigurations` is `@required` with `len 0..1`, so the
            // connector whose provider the service provisions names none by sending the
            // empty list the model admits.
            None => json!([]),
        };
        let mut body = json!({
            "name": self.name,
            "type": self.connector_type,
            "credentialProviderConfigurations": configurations,
        });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if let Some(mode) = self.provision_mode {
            body["provisionMode"] = json!(mode);
        }
        if let Some(token) = &self.client_token {
            body["clientToken"] = json!(token);
        }
        body
    }
}

/// A payment connector is created on its manager from the request and read back through
/// `GetPaymentConnector`, for every shape of the request `CreatePaymentConnector` accepts.
///
/// The rows differ along the payment provider (which decides the union member the
/// configuration is carried in), along the name's length and character classes, along the
/// description at each published bound of `PaymentsDescription` (`len 1..4096`), along
/// whether an idempotency token is sent, and along how the connector is provisioned — which
/// is what decides the status it reports and whether it publishes an authorization address.
#[tokio::test]
async fn a_payment_connector_is_created_and_read_back_for_every_request_shape() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("storefront").await;

    // Every connector is created first, so the clock-owed transitions of the whole table
    // run at once rather than one after another.
    let mut created_rows = Vec::new();
    for row in rows() {
        let provider_arn = match row.member {
            Some(_) => {
                emulator
                    .create_credential_provider(
                        &format!("provider-{}", created_rows.len()),
                        row.connector_type,
                    )
                    .await
            }
            None => String::new(),
        };
        let body = row.body(&provider_arn);
        let created = emulator.create_connector(&manager_id, &body).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: expected HTTP 202 from CreatePaymentConnector, got {} ({})",
            row.case, created.status, created.body
        );

        let id = text(&created.body, "paymentConnectorId");
        assert!(
            is_modelled_connector_id(&id, &row.name),
            "case {:?}: expected a paymentConnectorId of the form \
             <name lower-cased>-<10 lower-case alphanumerics>, got {id:?}",
            row.case
        );
        assert_eq!(
            text(&created.body, "paymentManagerId"),
            manager_id,
            "case {:?}: the create reports the manager it created the connector on, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            (text(&created.body, "name"), text(&created.body, "type")),
            (row.name.clone(), row.connector_type.to_string()),
            "case {:?}: the name and type read back as sent, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            created.body["credentialProviderConfigurations"],
            body["credentialProviderConfigurations"],
            "case {:?}: the credential provider configurations read back as sent, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            text(&created.body, "status"),
            row.created_status,
            "case {:?}: expected the created connector to report {}, got {}",
            row.case,
            row.created_status,
            created.body
        );
        assert_date_time(row.case, "createdAt", &created.body["createdAt"]);
        // "This field is only present when the payment connector status is
        // PENDING_AUTHENTICATION."
        let authorization_url = created.body.get("authorizationUrl").cloned();
        if row.created_status == STATUS_PENDING_AUTHENTICATION {
            assert_eq!(
                authorization_url,
                Some(json!(published_authorization_url(
                    smithy_server_runtime::UNSERVED_URL_BASE,
                    &manager_id,
                    &id
                ))),
                "case {:?}: a connector waiting for consent publishes the address it is \
                 completed at, got {}",
                row.case,
                created.body
            );
        } else {
            assert!(
                authorization_url.is_none_or(|url| url.is_null()),
                "case {:?}: a connector that waits for no consent publishes no \
                 authorizationUrl, got {}",
                row.case,
                created.body
            );
        }

        created_rows.push((row, id, body));
    }

    for (row, id, body) in &created_rows {
        // The read reports what the create persisted, including the two members the
        // create's own response does not declare: the description and the provision mode.
        let read = emulator
            .poll_until_status(row.case, &manager_id, id, row.settled_status)
            .await;
        assert_eq!(
            (
                text(&read, "paymentConnectorId"),
                text(&read, "name"),
                text(&read, "type")
            ),
            (id.clone(), row.name.clone(), row.connector_type.to_string()),
            "case {:?}: the read reports the connector the create minted, got {read}",
            row.case
        );
        assert_eq!(
            read["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {:?}: the description reads back as sent, got {read}",
            row.case
        );
        assert_eq!(
            text(&read, "provisionMode"),
            row.provision_mode.unwrap_or(MODE_MANUAL),
            "case {:?}: a request naming no provisionMode is provisioned MANUAL, got {read}",
            row.case
        );
        assert_eq!(
            read["credentialProviderConfigurations"],
            body["credentialProviderConfigurations"],
            "case {:?}: the read reports the configurations the create persisted, got {read}",
            row.case
        );
        assert_date_time(row.case, "createdAt", &read["createdAt"]);
        assert_date_time(row.case, "lastUpdatedAt", &read["lastUpdatedAt"]);
        // "It is first set when the credentials are provisioned … present only for payment
        // connectors with a provisionMode of QUICK_CREATE": nothing has provisioned a
        // service-managed credential for any of these connectors.
        assert!(
            read.get("credentialsUpdatedAt").is_none_or(Value::is_null),
            "case {:?}: no service-managed credentials have been provisioned, so none is \
             reported, got {read}",
            row.case
        );
        let still_waiting = row.settled_status == STATUS_PENDING_AUTHENTICATION;
        assert_eq!(
            read.get("authorizationUrl").is_some_and(|url| !url.is_null()),
            still_waiting,
            "case {:?}: the authorization address is reported exactly while the connector \
             waits at it, got {read}",
            row.case
        );
    }
}

/// RULE: a connector authenticates through a credential provider of this account's token
/// vault and is created only where the request can name one — and only on a manager this
/// account holds. Every row leaves nothing behind.
#[tokio::test]
async fn a_create_that_cannot_name_a_credential_provider_is_refused() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("refusals").await;
    let coinbase_arn = emulator
        .create_credential_provider("refusal-coinbase", TYPE_COINBASE_CDP)
        .await;
    let privy_arn = emulator
        .create_credential_provider("refusal-privy", TYPE_STRIPE_PRIVY)
        .await;

    /// A `PaymentManagerId` inside the modelled pattern
    /// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that this account holds no manager under.
    const ABSENT_MANAGER_ID: &str = "absentmanager-aaaaaaaaaa";
    // A `credentialProviderArn` of the modelled form naming no provider of this account.
    let absent_provider_arn = format!(
        "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:token-vault/default/\
         paymentcredentialprovider/absent-provider"
    );

    // (case, manager, body, status, error code, message)
    let rows: Vec<(&str, String, Value, u16, &str, String)> = vec![
        (
            "a MANUAL connector that names no credential provider at all",
            manager_id.clone(),
            json!({
                "name": "noProvider",
                "type": TYPE_COINBASE_CDP,
                "provisionMode": MODE_MANUAL,
                "credentialProviderConfigurations": [],
            }),
            400,
            "ValidationException",
            "Credential provider configurations is not defined".to_string(),
        ),
        (
            "a connector that names no mode and no credential provider either",
            manager_id.clone(),
            json!({
                "name": "defaultedNoProvider",
                "type": TYPE_STRIPE_PRIVY,
                "credentialProviderConfigurations": [],
            }),
            400,
            "ValidationException",
            "Credential provider configurations is not defined".to_string(),
        ),
        (
            "a Coinbase CDP connector configured with a Stripe Privy provider",
            manager_id.clone(),
            json!({
                "name": "mismatchedCoinbase",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations": configuration(MEMBER_STRIPE_PRIVY, &privy_arn),
            }),
            400,
            "ValidationException",
            format!(
                "credentialProviderConfigurations: configures {TYPE_STRIPE_PRIVY}, which is not \
                 this payment connector's type {TYPE_COINBASE_CDP}"
            ),
        ),
        (
            "a Stripe Privy connector configured with a Coinbase CDP provider",
            manager_id.clone(),
            json!({
                "name": "mismatchedPrivy",
                "type": TYPE_STRIPE_PRIVY,
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &coinbase_arn),
            }),
            400,
            "ValidationException",
            format!(
                "credentialProviderConfigurations: configures {TYPE_COINBASE_CDP}, which is not \
                 this payment connector's type {TYPE_STRIPE_PRIVY}"
            ),
        ),
        (
            "a connector naming a credential provider this account does not hold",
            manager_id.clone(),
            json!({
                "name": "absentProvider",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &absent_provider_arn),
            }),
            404,
            "ResourceNotFoundException",
            "PaymentCredentialProvider not found".to_string(),
        ),
        (
            "a connector on a payment manager this account does not hold",
            ABSENT_MANAGER_ID.to_string(),
            json!({
                "name": "absentManager",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &coinbase_arn),
            }),
            404,
            "ResourceNotFoundException",
            format!("Payment manager not found: managerId={ABSENT_MANAGER_ID}"),
        ),
    ];

    for (case, manager, body, status, code, message) in rows {
        let refused = emulator.create_connector(&manager, &body).await;
        assert_eq!(
            (
                refused.status,
                error_code(&refused.body),
                text(&refused.body, "message")
            ),
            (status, code.to_string(), message.clone()),
            "case {case:?}: expected HTTP {status} {code} {message:?}, got {} ({})",
            refused.status,
            refused.body
        );
    }

    // None of the refusals above wrote anything: the manager still takes a well-formed
    // create, and the connector that one mints is the only one it holds.
    let created = emulator
        .create_connector(
            &manager_id,
            &json!({
                "name": "afterRefusals",
                "type": TYPE_COINBASE_CDP,
                "credentialProviderConfigurations":
                    configuration(MEMBER_COINBASE_CDP, &coinbase_arn),
            }),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "a well-formed create after the refusals is served, got {} ({})",
        created.status, created.body
    );
    let read = emulator
        .get_connector(&manager_id, &text(&created.body, "paymentConnectorId"))
        .await;
    assert_eq!(
        read.status, 200,
        "the connector the refusals did not stop reads back, got {} ({})",
        read.status, read.body
    );
}

/// RULE: a read of one connector names the resource it was asked to read — the connector,
/// by the `paymentConnectorId` the path carried — whether the gap is the connector or the
/// manager that would hold it.
#[tokio::test]
async fn a_read_of_a_connector_nothing_holds_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let manager_id = emulator.create_manager("reads").await;

    // Two ids inside the modelled patterns that name nothing.
    let absent_manager = "absentmanager-bbbbbbbbbb";
    let absent_connector = "absent_connector-bbbbbbbbbb";

    let rows = [
        (
            "a connector id this manager holds nothing under",
            manager_id.as_str(),
            absent_connector,
            format!("Payment connector not found: {absent_connector}"),
        ),
        (
            "a manager id this account holds nothing under",
            absent_manager,
            absent_connector,
            format!("Payment connector not found: {absent_connector}"),
        ),
    ];

    for (case, manager, connector, message) in rows {
        let read = emulator.get_connector(manager, connector).await;
        assert_eq!(
            (
                read.status,
                error_code(&read.body),
                text(&read.body, "message")
            ),
            (404, "ResourceNotFoundException".to_string(), message.clone()),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException {message:?}, got {} ({})",
            read.status,
            read.body
        );
    }
}

/// RULE: `clientToken` is idempotent per manager — a replayed create answers with the
/// connector that token first produced, a different token produces a second connector, and
/// a token presented to ANOTHER manager has produced nothing there.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_connector_it_first_created() {
    let emulator = Emulator::new();
    let first_manager = emulator.create_manager("firstManager").await;
    let second_manager = emulator.create_manager("secondManager").await;
    let provider_arn = emulator
        .create_credential_provider("idempotent-provider", TYPE_COINBASE_CDP)
        .await;

    let token = client_token("replayedCreate");
    let body = json!({
        "name": "replayed",
        "type": TYPE_COINBASE_CDP,
        "credentialProviderConfigurations": configuration(MEMBER_COINBASE_CDP, &provider_arn),
        "clientToken": token,
    });

    let first = emulator.create_connector(&first_manager, &body).await;
    assert_eq!(
        first.status, 202,
        "the first create is served, got {} ({})",
        first.status, first.body
    );
    let replay = emulator.create_connector(&first_manager, &body).await;
    assert_eq!(
        (replay.status, text(&replay.body, "paymentConnectorId")),
        (202, text(&first.body, "paymentConnectorId")),
        "a replayed clientToken answers with the connector it first created, got {} ({})",
        replay.status,
        replay.body
    );

    let mut fresh = body.clone();
    fresh["clientToken"] = json!(client_token("anotherCreate"));
    let second = emulator.create_connector(&first_manager, &fresh).await;
    assert_ne!(
        text(&second.body, "paymentConnectorId"),
        text(&first.body, "paymentConnectorId"),
        "a different clientToken creates a second connector, got {}",
        second.body
    );

    // The same token on another manager has produced nothing there, so that create is
    // served as a fresh one — on that manager.
    let elsewhere = emulator.create_connector(&second_manager, &body).await;
    assert_eq!(
        (
            elsewhere.status,
            text(&elsewhere.body, "paymentManagerId"),
            text(&elsewhere.body, "paymentConnectorId")
                == text(&first.body, "paymentConnectorId")
        ),
        (202, second_manager.clone(), false),
        "a token presented to another manager creates a connector there, got {} ({})",
        elsewhere.status,
        elsewhere.body
    );
    let read = emulator
        .get_connector(
            &second_manager,
            &text(&elsewhere.body, "paymentConnectorId"),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "the connector created on the second manager reads back there, got {} ({})",
        read.status, read.body
    );
    // And it is not a connector of the FIRST manager: a connector is addressed only
    // through the manager it was created on.
    let crossed = emulator
        .get_connector(&first_manager, &text(&elsewhere.body, "paymentConnectorId"))
        .await;
    assert_eq!(
        crossed.status, 404,
        "a connector of one manager is not readable through another, got {} ({})",
        crossed.status, crossed.body
    );
}

/// The endpoint bases this emulator can report as its own: the forms a data port is
/// published at, plus the base of an emulator embedded in another process. One row each,
/// because the published authorization address has to follow whichever one this emulator is
/// reached at.
const ENDPOINT_BASES: [&str; 4] = [
    "https://127.0.0.1:9297",
    "https://localhost:39355",
    "https://[::1]:8443",
    smithy_server_runtime::UNSERVED_URL_BASE,
];

/// The DNS zones AWS publishes its own endpoints in: a published address must name none of
/// them, because this emulator answers at none of them.
const FOREIGN_HOST_MARKERS: [&str; 3] = [".on.aws", ".amazonaws.com", ".api.aws"];

/// RULE: `authorizationUrl` names the endpoint THIS emulator is served at, with the
/// connector's own path — never a host AWS publishes and this emulator does not answer on —
/// and a fetch of that address is served here, from the stored connector.
#[tokio::test]
async fn the_published_authorization_url_is_served_by_this_emulator() {
    for base in ENDPOINT_BASES {
        let emulator = Emulator::served_at(Some(base));
        let manager_id = emulator.create_manager("published").await;
        let created = emulator
            .create_connector(
                &manager_id,
                &json!({
                    "name": "quickConnector",
                    "type": TYPE_COINBASE_CDP,
                    "provisionMode": MODE_QUICK_CREATE,
                    "credentialProviderConfigurations": [],
                }),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {base:?}: the connector must be created, got {} ({})",
            created.status, created.body
        );
        let connector_id = text(&created.body, "paymentConnectorId");
        let url = published_authorization_url(base, &manager_id, &connector_id);
        assert_eq!(
            text(&created.body, "authorizationUrl"),
            url,
            "case {base:?}: expected the consent address published on this emulator's own \
             endpoint, got {}",
            created.body
        );
        for marker in FOREIGN_HOST_MARKERS {
            assert!(
                !url.contains(marker),
                "case {base:?}: a published authorization URL must name no AWS host, got {url:?}"
            );
        }

        let page = emulator.fetch("GET", &url).await;
        assert_eq!(
            page.status, 200,
            "case {base:?}: expected the published URL {url:?} to be served here, got {} ({})",
            page.status, page.body
        );
        assert!(
            page.content_type.starts_with("text/html"),
            "case {base:?}: expected a web page, got content type {:?}",
            page.content_type
        );
        for expected in [
            connector_id.as_str(),
            manager_id.as_str(),
            TYPE_COINBASE_CDP,
            STATUS_PENDING_AUTHENTICATION,
        ] {
            assert!(
                page.body.contains(expected),
                "case {base:?}: expected the page to name {expected:?}, got {:?}",
                page.body
            );
        }

        // What a client asks when it only wants to know the address is there.
        let head = emulator.fetch("HEAD", &url).await;
        assert_eq!(
            (head.status, head.body.is_empty()),
            (200, true),
            "case {base:?}: a HEAD of the published URL reports it with no body, got {} ({:?})",
            head.status,
            head.body
        );

        // An address of the same shape naming no connector of this emulator is answered,
        // and answered as nothing: no connector is invented for it.
        let absent = published_authorization_url(base, &manager_id, "absent_connector-cccccccccc");
        let missing = emulator.fetch("GET", &absent).await;
        assert_eq!(
            missing.status, 404,
            "case {base:?}: an address naming no connector is not served as one, got {} ({})",
            missing.status, missing.body
        );
    }
}
