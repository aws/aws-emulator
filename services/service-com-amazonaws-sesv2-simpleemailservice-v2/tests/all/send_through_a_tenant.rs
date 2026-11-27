// SPDX-License-Identifier: CC0-1.0

//! Sending THROUGH a tenant of the account, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`], so
//! routing, deserialization, the handler and serialization are all exercised.
//!
//! The rules these cases pin:
//!
//! * `TenantName` NAMES a tenant of the caller's account ("the name of the tenant through
//!   which this email will be sent"), so a send carrying one is resolved against the
//!   account's tenant table, and a value naming no tenant of it is refused
//!   `NotFoundException` (HTTP 404) with a sentence naming the requested name and the
//!   caller's account id. That resolution is part of reading the request's own resources,
//!   so it happens BEFORE the message is weighed against the account's sending identities:
//!   the same send from an UNVERIFIED sender is refused for the tenant while the tenant is
//!   not there, and `MessageRejected` (HTTP 400) for the unverified sender once it is —
//!   identically to a send that names no tenant at all.
//! * A `TenantName` outside the documented form is the caller's mistake
//!   (`BadRequestException`, HTTP 400) with the one sentence every operation over the
//!   member refuses it with, settled before the tenant table is read at all.
//! * A send a tenant resolved for is ACCEPTED through that tenant, and the accepted
//!   message is recorded as having been sent through it: the `TENANT_NAME` metric
//!   dimension ("the name of the tenant used when sending messages (if one was used)")
//!   attributes exactly those messages to it, and none to another tenant of the account.
//! * Both send paths read the member the same way, so `SendBulkEmail` resolves it with the
//!   same sentences as `SendEmail`.
//!
//! The tables span what the rules generalise over: the form of the name that resolves to
//! nothing (one character, the longest the member admits, hyphens, digits, underscores,
//! another casing of a name the account does hold), the form the message is carried in
//! (simple content and templated content), each way a name can be outside the documented
//! character-and-length rule, and both send operations.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

use crate::configuration_sets::{Reply, Ses};

/// The tenant the account holds in every case below.
const LIVE_TENANT: &str = "marketing-eu";

/// A second tenant of the account, created but never sent through: what a dimension
/// naming it must therefore account for is nothing.
const OTHER_TENANT: &str = "transactional";

/// The sender no case verifies, which is what makes the second leg of the ordering table
/// reach the verification check.
const UNVERIFIED_SENDER: &str = "stranger@example.com";

/// The verified sender the accepted sends below are made from.
const VERIFIED_SENDER: &str = "sender@example.com";

/// The recipient the accepted sends address: ONE mailbox outside the SES mailbox
/// simulator, which is what the `SEND` metric counts ("this excludes emails sent to the
/// mailbox simulator" and "emails addressed to more than one recipient").
const RECIPIENT: &str = "reader@recipient.example";

/// The Region every case is signed for, and therefore the one a failed verification check
/// is reported in.
const REGION: &str = "us-east-1";

/// The template a templated row names. It is never created: a templated send is refused
/// for the tenant before the template it names is looked up.
const TEMPLATE: &str = "receipt";

/// `BatchGetMetricData`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="BatchGetMetricData")`).
const METRICS_PATH: &str = "/v2/email/metrics/batch";

/// The only namespace `MetricNamespace` publishes.
const VDM: &str = "VDM";

/// `Metric`'s value for the messages the account sent.
const SEND: &str = "SEND";

/// Seconds in a day: the span one data point of the series covers.
const DAY: i64 = 24 * 60 * 60;

/// The tenant, suppression and sending calls these cases make.
trait TenantSends {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn send(&self, body: Value) -> Reply;
    async fn send_bulk(&self, body: Value) -> Reply;
    async fn create_template(&self, name: &str) -> Reply;
    async fn enable_vdm(&self) -> Reply;
    async fn metric_series(&self, dimensions: Value) -> Vec<i64>;
    async fn charged_recipients(&self) -> f64;
}

impl TenantSends for Ses {
    async fn create_tenant(&self, name: &str) -> Reply {
        self.call("POST", "/v2/email/tenants", Some(json!({"TenantName": name})))
            .await
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    async fn send_bulk(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-bulk-emails", Some(body))
            .await
    }

    async fn create_template(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/templates",
            Some(json!({
                "TemplateName": name,
                "TemplateContent": {"Subject": "Hello {{name}}", "Text": "Hi {{name}}"},
            })),
        )
        .await
    }

    /// Switch the account's Virtual Deliverability Manager on, which is the configuration
    /// the metric namespace is served under.
    async fn enable_vdm(&self) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/vdm",
            Some(json!({"VdmAttributes": {"VdmEnabled": "ENABLED"}})),
        )
        .await
    }

    /// The `SEND` values the account's records yield for `dimensions` over a window that
    /// brackets now by a day on either side, so a case's own sends fall inside it whenever
    /// it runs.
    async fn metric_series(&self, dimensions: Value) -> Vec<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("now is after the epoch")
            .as_secs() as i64;
        let mut query = json!({
            "Id": "q",
            "Namespace": VDM,
            "Metric": SEND,
            "StartDate": now - DAY,
            "EndDate": now + DAY,
        });
        if let Value::Object(members) = &dimensions {
            if !members.is_empty() {
                query["Dimensions"] = dimensions.clone();
            }
        }
        let answered = self
            .call("POST", METRICS_PATH, Some(json!({"Queries": [query]})))
            .await;
        assert_eq!(
            answered.status,
            200,
            "case {dimensions:?}: expected HTTP 200 from the metric batch, got {} {:?}",
            answered.status,
            answered.message()
        );
        answered
            .pointer("/Results/0/Values")
            .and_then(|values| values.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|value| value.as_f64().unwrap_or_default() as i64)
            .collect()
    }

    /// What the account's 24-hour counter has been charged, which a refused send must
    /// leave at nothing.
    async fn charged_recipients(&self) -> f64 {
        let account = self.call("GET", "/v2/email/account", None).await;
        account
            .pointer("/SendQuota/SentLast24Hours")
            .and_then(|value| value.as_f64())
            .unwrap_or_default()
    }
}

/// The sentence the send paths refuse a tenant the account does not hold with: the
/// requested name and the caller's account id are its only variable parts.
fn tenant_not_found(name: &str) -> String {
    format!(
        "Tenant {name} for AwsAccountId {} not found.",
        emulator_server::emulator_account_id()
    )
}

/// The sentence a send whose own sender failed the verification check is refused with:
/// the Region of the call, upper-cased, and every identity the message involved that
/// failed the check, in the order the request states them.
fn unverified(identities: &[&str]) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in \
         region {}: {}",
        REGION.to_uppercase(),
        identities.join(", ")
    )
}

/// `SendEmail`'s body, from `sender`, through `tenant` when one is named, carrying
/// `content`.
fn send_body(sender: &str, tenant: Option<&str>, content: Value) -> Value {
    let mut body = json!({
        "FromEmailAddress": sender,
        "Destination": {"ToAddresses": [RECIPIENT]},
        "Content": content,
    });
    if let Some(tenant) = tenant {
        body["TenantName"] = json!(tenant);
    }
    body
}

/// A simple message's content: the one form a request carries its own subject and body in.
fn simple_content() -> Value {
    json!({"Simple": {
        "Subject": {"Data": "hello"},
        "Body": {"Text": {"Data": "a message"}},
    }})
}

/// Templated content naming a template of the account, with values for its variables.
fn templated_content() -> Value {
    json!({"Template": {
        "TemplateName": TEMPLATE,
        "TemplateData": "{\"name\":\"Ada\"}",
    }})
}

/// `SendBulkEmail`'s body, from `sender`, through `tenant` when one is named.
fn bulk_body(sender: &str, tenant: Option<&str>) -> Value {
    let mut body = json!({
        "FromEmailAddress": sender,
        "DefaultContent": {"Template": {
            "TemplateName": TEMPLATE,
            "TemplateData": "{\"name\":\"Ada\"}",
        }},
        "BulkEmailEntries": [{"Destination": {"ToAddresses": [RECIPIENT]}}],
    });
    if let Some(tenant) = tenant {
        body["TenantName"] = json!(tenant);
    }
    body
}

#[tokio::test]
async fn a_send_naming_no_tenant_of_the_account_is_refused_before_the_sender_is_weighed() {
    /// One row: a name the account holds no tenant under, and the form the message is
    /// carried in.
    struct Row {
        case: &'static str,
        ghost: String,
        content: Value,
    }

    // The names span the documented `TenantName` form — "up to 64 alphanumeric characters,
    // including letters, numbers, hyphens (-) and underscores (_) only"
    // (`smithy_query(query="shape",shape="TenantName")`) — so a name the member ADMITS is
    // what each row fails to resolve, and the member's own bound is the longest row. The
    // last row is the live tenant's name in another casing: a tenant is held under the name
    // it was created with, so that spelling names no tenant of the account.
    let rows = vec![
        Row {
            case: "a one-character name",
            ghost: "g".to_string(),
            content: simple_content(),
        },
        Row {
            case: "the longest name the member admits",
            ghost: "g".repeat(state::MAX_TENANT_NAME_CHARS),
            content: simple_content(),
        },
        Row {
            case: "a name carrying hyphens and digits",
            ghost: "ghost-tenant-2026".to_string(),
            content: simple_content(),
        },
        Row {
            case: "a name carrying underscores",
            ghost: "ghost_tenant".to_string(),
            content: simple_content(),
        },
        Row {
            case: "the live tenant's name in another casing",
            ghost: LIVE_TENANT.to_uppercase(),
            content: simple_content(),
        },
        Row {
            case: "a templated message, whose template the account does not hold either",
            ghost: "ghost-tenant".to_string(),
            content: templated_content(),
        },
    ];

    for row in rows {
        let case = row.case;
        let ses = Ses::new();
        let created = ses.create_tenant(LIVE_TENANT).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
            created.status,
            created.message()
        );

        // The tenant the account does NOT hold: refused for the tenant, from a sender the
        // account has not verified either.
        let refused = ses
            .send(send_body(
                UNVERIFIED_SENDER,
                Some(&row.ghost),
                row.content.clone(),
            ))
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "NotFoundException".to_string()),
            "case {case:?}: expected HTTP 404 NotFoundException for the tenant, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            tenant_not_found(&row.ghost),
            "case {case:?}: expected the refusal to name the tenant and the account, got {:?}",
            refused.message()
        );

        // The SAME send through the tenant the account DOES hold reaches the verification
        // check, and so does the same send naming no tenant at all: both report the
        // unverified sender, identically.
        let through_live = ses
            .send(send_body(
                UNVERIFIED_SENDER,
                Some(LIVE_TENANT),
                row.content.clone(),
            ))
            .await;
        let without_tenant = ses
            .send(send_body(UNVERIFIED_SENDER, None, row.content.clone()))
            .await;
        for (leg, reply) in [
            ("the tenant the account holds", &through_live),
            ("no tenant at all", &without_tenant),
        ] {
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "MessageRejected".to_string()),
                "case {case:?} through {leg}: expected HTTP 400 MessageRejected, got {} {:?}",
                reply.status,
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                unverified(&[UNVERIFIED_SENDER, RECIPIENT]),
                "case {case:?} through {leg}: expected the unverified-sender sentence, got {:?}",
                reply.message()
            );
        }
        assert_eq!(
            through_live.message(),
            without_tenant.message(),
            "case {case:?}: a resolved tenant and no tenant must report the identical \
             sentence, got {:?} and {:?}",
            through_live.message(),
            without_tenant.message()
        );

        // The BULK send reads the member the same way: the same two sentences, in the same
        // order against the verification check.
        let bulk_refused = ses
            .send_bulk(bulk_body(UNVERIFIED_SENDER, Some(&row.ghost)))
            .await;
        assert_eq!(
            (bulk_refused.status, bulk_refused.error_code()),
            (404, "NotFoundException".to_string()),
            "case {case:?} on SendBulkEmail: expected HTTP 404 NotFoundException for the \
             tenant, got {} {:?}",
            bulk_refused.status,
            bulk_refused.error_code()
        );
        assert_eq!(
            bulk_refused.message(),
            tenant_not_found(&row.ghost),
            "case {case:?} on SendBulkEmail: expected the refusal to name the tenant and the \
             account, got {:?}",
            bulk_refused.message()
        );
        let bulk_through_live = ses
            .send_bulk(bulk_body(UNVERIFIED_SENDER, Some(LIVE_TENANT)))
            .await;
        assert_eq!(
            (bulk_through_live.status, bulk_through_live.error_code()),
            (400, "MessageRejected".to_string()),
            "case {case:?} on SendBulkEmail through the tenant the account holds: expected \
             HTTP 400 MessageRejected, got {} {:?}",
            bulk_through_live.status,
            bulk_through_live.error_code()
        );
        assert_eq!(
            bulk_through_live.message(),
            // A bulk send states ONE sending identity for the whole call, and the check
            // weighs the identities of the whole call: that sending identity and every
            // mailbox any entry addresses, so the sentence names both the unverified
            // sender and the recipient the entry addresses
            // (`send_sandbox_destination_gate.rs`).
            unverified(&[UNVERIFIED_SENDER, RECIPIENT]),
            "case {case:?} on SendBulkEmail through the tenant the account holds: expected \
             the unverified-identity sentence, got {:?}",
            bulk_through_live.message()
        );

        // None of the five was accepted, so nothing was charged against the account.
        let charged = ses.charged_recipients().await;
        assert_eq!(
            charged, 0.0,
            "case {case:?}: a refused send must charge nothing, got {charged}"
        );
    }
}

#[tokio::test]
async fn a_tenant_name_outside_the_documented_form_is_the_callers_mistake() {
    let over_long = "n".repeat(state::MAX_TENANT_NAME_CHARS + 1);
    // Each row is one way a value can fall outside the documented form: a character the
    // member does not admit (a dot, a space, a punctuation mark) and one character past
    // the documented length bound. A name AT the bound is admitted, which the ordering
    // table above covers.
    for (case, name) in [
        ("a name carrying a dot", "live.tenant".to_string()),
        ("a name carrying a space", "live tenant".to_string()),
        ("a name carrying punctuation", "live!tenant".to_string()),
        ("one character past the length bound", over_long),
    ] {
        let ses = Ses::new();
        let created = ses.create_tenant(LIVE_TENANT).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
            created.status,
            created.message()
        );

        for (operation, reply) in [
            (
                "SendEmail",
                ses.send(send_body(VERIFIED_SENDER, Some(&name), simple_content()))
                    .await,
            ),
            (
                "SendBulkEmail",
                ses.send_bulk(bulk_body(VERIFIED_SENDER, Some(&name))).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException".to_string()),
                "case {case:?} on {operation}: expected HTTP 400 BadRequestException, got {} {:?}",
                reply.status,
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                state::invalid_tenant_name(&name),
                "case {case:?} on {operation}: expected the invalid-name sentence, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_send_is_recorded_as_having_gone_through_the_tenant_it_named() {
    let ses = Ses::new();
    // About the tenant a send is recorded as having gone through, not about the sandbox's
    // destination gate: the account is given production access, so the send may address
    // the mailbox this row names (`send_sandbox_destination_gate.rs`).
    ses.grant_production_access();
    let vdm = ses.enable_vdm().await;
    assert_eq!(
        vdm.status,
        200,
        "case \"setup\": expected HTTP 200 from the VDM put, got {} {:?}",
        vdm.status,
        vdm.message()
    );
    for tenant in [LIVE_TENANT, OTHER_TENANT] {
        let created = ses.create_tenant(tenant).await;
        assert_eq!(
            created.status,
            200,
            "case \"setup\": expected HTTP 200 creating {tenant}, got {} {:?}",
            created.status,
            created.message()
        );
    }
    let template = ses.create_template(TEMPLATE).await;
    assert_eq!(
        template.status,
        200,
        "case \"setup\": expected HTTP 200 creating the template, got {} {:?}",
        template.status,
        template.message()
    );
    ses.verify_identity(VERIFIED_SENDER).await;

    // Three accepted messages: one single send through the tenant, one bulk send through
    // the same tenant, and one single send of the account itself.
    let through_tenant = ses
        .send(send_body(
            VERIFIED_SENDER,
            Some(LIVE_TENANT),
            simple_content(),
        ))
        .await;
    assert_eq!(
        through_tenant.status,
        200,
        "case \"through the tenant\": expected HTTP 200 from the send, got {} {:?}",
        through_tenant.status,
        through_tenant.message()
    );
    assert!(
        through_tenant
            .member("MessageId")
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "case \"through the tenant\": expected the accepted send to answer a MessageId, got {:?}",
        through_tenant.member("MessageId")
    );

    let bulk = ses
        .send_bulk(bulk_body(VERIFIED_SENDER, Some(LIVE_TENANT)))
        .await;
    assert_eq!(
        bulk.status,
        200,
        "case \"bulk through the tenant\": expected HTTP 200 from the bulk send, got {} {:?}",
        bulk.status,
        bulk.message()
    );
    assert_eq!(
        bulk.pointer("/BulkEmailEntryResults/0/Status")
            .and_then(|status| status.as_str().map(str::to_string)),
        Some("SUCCESS".to_string()),
        "case \"bulk through the tenant\": expected the entry to be accepted, got {:?}",
        bulk.pointer("/BulkEmailEntryResults")
    );

    let without_tenant = ses
        .send(send_body(VERIFIED_SENDER, None, simple_content()))
        .await;
    assert_eq!(
        without_tenant.status,
        200,
        "case \"no tenant\": expected HTTP 200 from the send, got {} {:?}",
        without_tenant.status,
        without_tenant.message()
    );

    // What each dimension value accounts for. Summed over the series rather than read
    // point by point, because a run that straddles a UTC midnight splits the same
    // messages over two days.
    let total = |series: Vec<i64>| series.iter().sum::<i64>();
    let all = total(ses.metric_series(json!({})).await);
    assert_eq!(
        all, 3,
        "case \"every message\": expected the three accepted messages to be counted, got {all}"
    );
    let tenants = total(
        ses.metric_series(json!({"TENANT_NAME": LIVE_TENANT}))
            .await,
    );
    assert_eq!(
        tenants, 2,
        "case \"the tenant sent through\": expected the two messages sent through \
         {LIVE_TENANT} to be attributed to it, got {tenants}"
    );
    let other = total(
        ses.metric_series(json!({"TENANT_NAME": OTHER_TENANT}))
            .await,
    );
    assert_eq!(
        other, 0,
        "case \"a tenant nothing was sent through\": expected no message to be attributed \
         to {OTHER_TENANT}, got {other}"
    );
}
