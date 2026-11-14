// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for EVERY operation of this service that ADDRESSES a trust
//! anchor by `trustAnchorId`.
//!
//! The rule under test: an operation that addresses a trust anchor resolves the id
//! against this service's stored state — a stored id is served from that record, and an
//! id no stored anchor carries is the operation's modelled `ResourceNotFoundException`
//! (HTTP 404) carrying the fixed wording this service answers an addressed miss with. No
//! such operation may answer a resolvable request with the scaffold's HTTP 501
//! `NotImplemented`, which is in no operation's modelled error set.
//!
//! The sibling suites each pin ONE operation; this table's rows differ along the
//! dimension the rule generalises over that none of them covers: WHICH operation
//! addresses the anchor. The seven rows below are every operation whose input carries
//! `trustAnchorId` — `GetTrustAnchor`, `DeleteTrustAnchor`, `EnableTrustAnchor`,
//! `DisableTrustAnchor`, `UpdateTrustAnchor` (the five that bind it as an `@httpLabel`)
//! and `PutNotificationSettings`, `ResetNotificationSettings` (which carry it in the
//! body), each modelling `ResourceNotFoundException` at 404 and a required `trustAnchor`
//! in its output
//! (`smithy_query(query="operation", operation="GetTrustAnchor,DeleteTrustAnchor,EnableTrustAnchor,DisableTrustAnchor,UpdateTrustAnchor,PutNotificationSettings,ResetNotificationSettings")`).
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle. `SourceData$x509CertificateData` is `@length(1, 204800)`
/// with no `@pattern`, so any non-empty string is a legal certificate bundle on the wire
/// (`smithy_query(query="shape", shape="SourceData")`).
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nYWRkcmVzc2VkLW1pc3Mtc3VpdGUtY2VydA==\n-----END CERTIFICATE-----\n";

/// The Region the suite signs for.
const REGION: &str = "ap-southeast-2";

/// The wording a refusal carries when the trust anchor a request ADDRESSES by
/// `trustAnchorId` is not stored: wholly fixed text, the same for every operation that
/// addresses one.
const TRUST_ANCHOR_NOT_FOUND_MESSAGE: &str = "TrustAnchor not found";

/// One operation that addresses a trust anchor by `trustAnchorId`.
#[derive(Clone, Copy, Debug)]
struct Addressed {
    /// The operation's name, used as the case label.
    name: &'static str,
    /// The HTTP method the model binds it to.
    method: &'static str,
    /// The URI the model binds it to, with `{id}` standing in for the `@httpLabel`.
    uri: &'static str,
    /// The request body, with `{id}` standing in for an id the body carries; `None` for
    /// an operation whose input is only the label.
    body: Option<&'static str>,
    /// The `enabled` flag the operation WRITES, when writing it is the operation's whole
    /// effect; `None` for an operation that leaves the flag as the record held it.
    writes_enabled: Option<bool>,
}

/// Every operation whose input carries `trustAnchorId`, with the binding and the smallest
/// legal body the model accepts: `UpdateTrustAnchor` takes two optional members and so is
/// driven with `name` alone; the notification operations each require their list member
/// (`notificationSettings` of `NotificationSetting`, which requires `enabled` and `event`;
/// `notificationSettingKeys` of `NotificationSettingKey`, which requires `event`), and
/// `CA_CERTIFICATE_EXPIRY` is the first value the modelled `NotificationEvent` enum lists
/// (`smithy_query(query="shape", shape="NotificationSetting,NotificationSettingKey,NotificationEvent")`).
const ADDRESSED_OPERATIONS: [Addressed; 7] = [
    Addressed {
        name: "GetTrustAnchor",
        method: "GET",
        uri: "/trustanchor/{id}",
        body: None,
        writes_enabled: None,
    },
    Addressed {
        name: "EnableTrustAnchor",
        method: "POST",
        uri: "/trustanchor/{id}/enable",
        body: None,
        writes_enabled: Some(true),
    },
    Addressed {
        name: "DisableTrustAnchor",
        method: "POST",
        uri: "/trustanchor/{id}/disable",
        body: None,
        writes_enabled: Some(false),
    },
    Addressed {
        name: "UpdateTrustAnchor",
        method: "PATCH",
        uri: "/trustanchor/{id}",
        body: Some(r#"{"name": "renamed by the addressed-miss suite"}"#),
        writes_enabled: None,
    },
    Addressed {
        name: "PutNotificationSettings",
        method: "PATCH",
        uri: "/put-notifications-settings",
        body: Some(
            r#"{"trustAnchorId": "{id}",
                "notificationSettings": [
                  {"enabled": true, "event": "CA_CERTIFICATE_EXPIRY", "threshold": 10}
                ]}"#,
        ),
        writes_enabled: None,
    },
    Addressed {
        name: "ResetNotificationSettings",
        method: "PATCH",
        uri: "/reset-notifications-settings",
        body: Some(
            r#"{"trustAnchorId": "{id}",
                "notificationSettingKeys": [{"event": "CA_CERTIFICATE_EXPIRY"}]}"#,
        ),
        writes_enabled: None,
    },
    // Last in the table only for readability: each case runs against its own anchor, so
    // the delete never takes an anchor another row needs.
    Addressed {
        name: "DeleteTrustAnchor",
        method: "DELETE",
        uri: "/trustanchor/{id}",
        body: None,
        writes_enabled: None,
    },
];

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn create_trust_anchor(&self, name: &str, enabled: bool) -> Answer {
        self.send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
                "enabled": enabled,
            })),
        )
        .await
    }

    async fn get_trust_anchor(&self, id: &str) -> Answer {
        self.send("GET", &format!("/trustanchor/{id}"), None).await
    }

    async fn delete_trust_anchor(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/trustanchor/{id}"), None)
            .await
    }

    /// Drive one row of the table against one id.
    async fn call(&self, operation: &Addressed, id: &str) -> Answer {
        let uri = operation.uri.replace("{id}", id);
        let body = operation.body.map(|template| {
            serde_json::from_str::<Value>(&template.replace("{id}", id)).expect("body template")
        });
        self.send(operation.method, &uri, body).await
    }
}

/// The `trustAnchor` a successful call answered with.
fn anchor_of(case: &str, expected_status: u16, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
    answer
        .body
        .get("trustAnchor")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no trustAnchor in {answer:?}"))
}

/// The id of a trust anchor a call answered with.
fn id_of(case: &str, anchor: &Value) -> String {
    anchor
        .get("trustAnchorId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no string trustAnchorId in {anchor:?}"))
        .to_string()
}

/// The `message` an answer carries.
fn message_of(answer: &Answer) -> String {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every operation that addresses a trust anchor refuses an id no stored anchor carries
/// with the modelled `ResourceNotFoundException` (404) and the fixed wording — never the
/// scaffold's 501 and never a success.
///
/// The rows differ along what the rule generalises over: which operation addresses the
/// anchor, and each way a well-formed id can name no stored anchor (never created at all,
/// and the id of an anchor that was created and then deleted).
#[tokio::test]
async fn every_operation_addressing_a_trust_anchor_refuses_an_unknown_id_as_not_found() {
    let client = Client::new();

    // A well-formed `Uuid` (`@length(36, 36)`, pattern
    // `[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}`) that no create minted.
    let never_created = "7f1e2d3c-aaaa-bbbb-cccc-ddddeeeeffff".to_string();

    let created = anchor_of(
        "setup",
        201,
        &client.create_trust_anchor("anchor to delete", true).await,
    );
    let deleted_id = id_of("setup", &created);
    let deleted = client.delete_trust_anchor(&deleted_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"setup\": expected DeleteTrustAnchor 200, got {deleted:?}"
    );

    for operation in ADDRESSED_OPERATIONS {
        for (miss, id) in [
            ("never-created", never_created.clone()),
            ("already-deleted", deleted_id.clone()),
        ] {
            let case = format!("{}-{miss}", operation.name);
            let case = case.as_str();
            let answer = client.call(&operation, &id).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an id no stored anchor carries must be refused with HTTP 404, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: the refusal must be the operation's modelled error, got {answer:?}"
            );
            assert_eq!(
                message_of(&answer),
                TRUST_ANCHOR_NOT_FOUND_MESSAGE,
                "case {case:?}: the refusal must carry the wording this service answers an \
                 addressed miss with, got {answer:?}"
            );

            // The refusal landed nowhere: the id still resolves to nothing.
            let read_back = client.get_trust_anchor(&id).await;
            assert_eq!(
                read_back.status, 404,
                "case {case:?}: the refused call must not have created anything, got {read_back:?}"
            );
        }
    }
}

/// Every operation that addresses a trust anchor serves a STORED id from that record: the
/// answer is HTTP 200 carrying the addressed anchor, so none of them is left answering the
/// scaffold's 501.
///
/// The rows differ along what the rule generalises over: which operation addresses the
/// anchor, and the flag the stored anchor held when the call arrived.
#[tokio::test]
async fn every_operation_addressing_a_trust_anchor_serves_a_stored_id() {
    let client = Client::new();
    for operation in ADDRESSED_OPERATIONS {
        for created_enabled in [false, true] {
            let case = format!("{}-created-enabled-{created_enabled}", operation.name);
            let case = case.as_str();

            // One anchor per case, so a row that deletes or writes the anchor cannot
            // disturb another row.
            let created = anchor_of(
                case,
                201,
                &client
                    .create_trust_anchor(&format!("{} subject", operation.name), created_enabled)
                    .await,
            );
            let id = id_of(case, &created);

            let answer = client.call(&operation, &id).await;
            let served = anchor_of(case, 200, &answer);
            assert_eq!(
                id_of(case, &served),
                id,
                "case {case:?}: the answer must carry the addressed anchor, got {served:?}"
            );
            assert_eq!(
                served.get("trustAnchorArn"),
                created.get("trustAnchorArn"),
                "case {case:?}: the answer must come from the stored record ({:?}), got {:?}",
                created.get("trustAnchorArn"),
                served.get("trustAnchorArn")
            );

            match operation.writes_enabled {
                // The operation's whole effect is the flag: the answer reports what it
                // wrote, whatever the record held before.
                Some(written) => assert_eq!(
                    served.get("enabled"),
                    Some(&Value::Bool(written)),
                    "case {case:?}: the answer must report enabled {written}, got {served:?}"
                ),
                // Every other operation leaves the flag as the record held it.
                None => assert_eq!(
                    served.get("enabled"),
                    Some(&Value::Bool(created_enabled)),
                    "case {case:?}: the flag must stay as the record held it \
                     ({created_enabled}), got {served:?}"
                ),
            }
        }
    }
}
