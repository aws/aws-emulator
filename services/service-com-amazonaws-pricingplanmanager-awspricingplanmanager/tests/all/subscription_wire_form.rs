// SPDX-License-Identifier: CC0-1.0

//! The wire form of a subscription's `@timestampFormat("date-time")` members and of its
//! concurrency token, on every path that publishes them.
//!
//! Two rules are pinned here.
//!
//! 1. A structure member the model marks `@timestampFormat("date-time")` travels on this
//!    service's restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING — never as the
//!    epoch-seconds number the protocol default would use. The members are
//!    `createdAt`/`updatedAt` on `Subscription` and on `SubscriptionSummary`, and
//!    `effectiveDate` on `ScheduledChange`
//!    (`smithy_query(query="shape", shape="Subscription,SubscriptionSummary,ScheduledChange")`).
//!    The INSTANT is per run; the wire TYPE and format are the requirement, so the
//!    assertions below are about the form and about the three read paths agreeing.
//!
//! 2. The entity tag is a monotonic per-subscription REVISION rendered in decimal: minting
//!    the record is its first write and activating it a second, every mutation hands out
//!    the next integer, a read never moves it, and the same token appears in the `ETag`
//!    response header and in the `eTag` body member of a list summary.
//!
//! Both rules are asserted over TABLES whose rows differ along what the rules generalise
//! over — tier, usage level, resource count, approval mode, whether a change is pending,
//! and the sequence of mutations a subscription has taken — plus one test pinning the
//! protocol seam the first rule's rendering comes from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::ops::types::{
    ScheduledChange, Subscription, SubscriptionSummary,
};
use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::state::{
    parse_date_time, FIRST_REVISION,
};
use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::EmulatedService;

/// A SigV4 `Authorization` header scoped to this service, as the router routes on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/pricingplanmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "us-west-2";

/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One HTTP exchange: status, parsed body and the `ETag` response header.
struct Answer {
    status: u16,
    body: Value,
    etag: Option<String>,
}

impl Answer {
    /// The body member `name` as a string, naming the whole body when it is absent.
    fn string(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("member {name} missing from body {:?}", self.body))
            .to_string()
    }

    /// The `ETag` response header; panics when the operation did not send one.
    fn require_etag(&self, case: &str) -> String {
        self.etag
            .clone()
            .unwrap_or_else(|| panic!("case {case}: no ETag response header on {:?}", self.body))
    }
}

/// A router with this one service mounted on a shared engine, request authentication off.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// POST `/v1/<operation>` with `body`, optionally under `If-Match`.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    if_match: Option<&str>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://pricingplanmanager.{REGION}.amazonaws.com/v1/{operation}"
        ))
        .header("host", format!("pricingplanmanager.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    if let Some(tag) = if_match {
        builder = builder.header("if-match", tag);
    }
    let request = builder
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Answer { status, body, etag }
}

/// A minimal valid `CreateSubscription` request.
fn create_body(
    family: &str,
    tier: &str,
    resources: usize,
    usage_level: Option<&str>,
    approval: Option<&str>,
) -> Value {
    let resource_arns: Vec<String> = (0..resources)
        .map(|index| format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E{index}"))
        .collect();
    let mut body = json!({
        "planFamily": family,
        "planTier": tier,
        "resourceArns": resource_arns,
    });
    if let Some(level) = usage_level {
        body["usageLevel"] = json!(level);
    }
    if let Some(mode) = approval {
        body["approvalMode"] = json!(mode);
    }
    body
}

/// Create one subscription, asserting only that it succeeded.
async fn create(router: &MultiServiceRouter, body: Value) -> Answer {
    let answer = call(router, "CreateSubscription", body.clone(), None).await;
    assert_eq!(
        answer.status, 200,
        "CreateSubscription {body:?} must succeed, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// The summary `ListSubscriptions` reports for `arn`, following `nextToken` to the end so
/// a store larger than one page is still searched.
async fn summary_for(router: &MultiServiceRouter, arn: &str) -> Value {
    let mut next: Option<String> = None;
    loop {
        let body = match next {
            None => json!({}),
            Some(ref token) => json!({"nextToken": token}),
        };
        let listed = call(router, "ListSubscriptions", body, None).await;
        assert_eq!(
            listed.status, 200,
            "ListSubscriptions must succeed, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = listed.body["subscriptionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("subscriptionSummaries missing from {:?}", listed.body))
            .clone();
        if let Some(found) = summaries
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(arn))
        {
            return found.clone();
        }
        match listed.body["nextToken"].as_str() {
            Some(token) => next = Some(token.to_string()),
            None => panic!("no summary for {arn} in the listing"),
        }
    }
}

/// Assert `value` is the RFC 3339 date-time STRING a `@timestampFormat("date-time")`
/// member travels as, and return the instant it names in epoch milliseconds.
///
/// A JSON number is exactly the protocol-default encoding this rule forbids, so it is
/// rejected by name rather than parsed.
fn date_time_millis(case: &str, member: &str, value: &Value) -> i64 {
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             an RFC 3339 string, got {value}"
        )
    });
    // Smithy's `date-time` is RFC 3339 at the `Z` offset: the date/time separator and the
    // zone designator are part of the form, not of the instant.
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case}: {member} must be an RFC 3339 date-time at the Z offset, got {text:?}"
    );
    let parsed = parse_date_time(text)
        .unwrap_or_else(|| panic!("case {case}: {member} = {text:?} must parse as a date-time"));
    parsed.to_millis().unwrap_or_else(|error| {
        panic!("case {case}: {member} = {text:?} must name a representable instant: {error}")
    })
}

// ---------------------------------------------------------------------------
// Rule 1 — the date-time members
// ---------------------------------------------------------------------------

/// Every `date-time` member of a subscription is a date-time string on all three read
/// paths (`CreateSubscription`, `GetSubscription`, `ListSubscriptions`), and the three
/// paths name the same instant.
#[tokio::test]
async fn date_time_members_travel_as_iso_8601_strings_on_every_read_path() {
    struct Case {
        label: &'static str,
        family: &'static str,
        tier: &'static str,
        resources: usize,
        usage_level: Option<&'static str>,
        approval: Option<&'static str>,
        /// A tier to move to after creating, or `None` to leave the record as created.
        then_update_to: Option<&'static str>,
        /// Whether that move is a DOWNGRADE, which leaves a `scheduledChange` behind —
        /// its `effectiveDate` is the third `date-time` member in the model.
        expects_pending_change: bool,
    }

    // The rows differ along plan family, tier, usage level, resource count across the
    // modelled 1..10 bound, approval mode, and whether the record has taken a mutation
    // and carries a pending change — i.e. along everything the rule generalises over.
    let cases = [
        Case {
            label: "free tier, as created",
            family: "CloudFront",
            tier: "FREE",
            resources: 1,
            usage_level: None,
            approval: None,
            then_update_to: None,
            expects_pending_change: false,
        },
        Case {
            label: "paid tier held for approval",
            family: "CloudFront",
            tier: "PREMIUM",
            resources: 10,
            usage_level: Some("LEVEL_2"),
            approval: Some("MANUAL"),
            then_update_to: None,
            expects_pending_change: false,
        },
        Case {
            label: "upgraded immediately",
            family: "Route53",
            tier: "PRO",
            resources: 3,
            usage_level: Some("DEFAULT"),
            approval: Some("IMMEDIATE"),
            then_update_to: Some("BUSINESS"),
            expects_pending_change: false,
        },
        Case {
            label: "downgrade pending, so effectiveDate is populated",
            family: "CloudFront",
            tier: "PREMIUM",
            resources: 2,
            usage_level: None,
            approval: Some("IMMEDIATE"),
            then_update_to: Some("FREE"),
            expects_pending_change: true,
        },
    ];

    let router = router();
    for case in &cases {
        let label = case.label;
        let created = create(
            &router,
            create_body(
                case.family,
                case.tier,
                case.resources,
                case.usage_level,
                case.approval,
            ),
        )
        .await;
        let arn = created.string("arn");
        let mut current = created;

        if let Some(target) = case.then_update_to {
            let tag = current.require_etag(label);
            let updated = call(
                &router,
                "UpdateSubscription",
                json!({"arn": arn.clone(), "planTier": target}),
                Some(&tag),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {label}: UpdateSubscription must succeed, got {} {:?}",
                updated.status, updated.body
            );
            current = updated;
        }

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {label}: GetSubscription must succeed, got {} {:?}",
            fetched.status, fetched.body
        );
        let summary = summary_for(&router, &arn).await;

        // The two required members, on the three paths that publish them.
        for member in ["createdAt", "updatedAt"] {
            let mutated = date_time_millis(
                &format!("{label}/mutating response"),
                member,
                &current.body[member],
            );
            let read = date_time_millis(&format!("{label}/GetSubscription"), member, &fetched.body[member]);
            let listed =
                date_time_millis(&format!("{label}/ListSubscriptions"), member, &summary[member]);
            assert_eq!(
                read, listed,
                "case {label}: the summary's {member} must name the instant the read reports"
            );
            assert_eq!(
                read, mutated,
                "case {label}: {member} must not change between the write and the read"
            );
            assert!(
                read > 1_600_000_000_000,
                "case {label}: {member} must be a real instant, got {read} ms"
            );
        }

        // `effectiveDate` is the one optional `date-time` member; it is present exactly
        // when a change is pending, and then it too is a string on both paths.
        let pending_change = case.expects_pending_change;
        assert_eq!(
            fetched.body.get("scheduledChange").is_some(),
            pending_change,
            "case {label}: a pending change is reported exactly when a downgrade was \
             scheduled, got {:?}",
            fetched.body
        );
        if pending_change {
            let read = date_time_millis(
                &format!("{label}/GetSubscription"),
                "scheduledChange.effectiveDate",
                &fetched.body["scheduledChange"]["effectiveDate"],
            );
            let listed = date_time_millis(
                &format!("{label}/ListSubscriptions"),
                "scheduledChange.effectiveDate",
                &summary["scheduledChange"]["effectiveDate"],
            );
            assert_eq!(
                read, listed,
                "case {label}: the summary's effectiveDate must name the instant the read \
                 reports"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — the concurrency token
// ---------------------------------------------------------------------------

/// The token form the rule requires: a decimal integer, with no sign, no padding and no
/// other character. Returns the revision it names.
fn revision_of(case: &str, token: &str) -> u64 {
    assert!(
        !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case}: the entity tag is a decimal revision, got {token:?}"
    );
    assert!(
        token == "0" || !token.starts_with('0'),
        "case {case}: a decimal revision carries no leading zero, got {token:?}"
    );
    token
        .parse()
        .unwrap_or_else(|error| panic!("case {case}: {token:?} must parse as a revision: {error}"))
}

/// The entity tag is the record's revision in decimal: the mint after the create, one
/// more when the create also activated the subscription, the next integer after each
/// mutation, unchanged by any read, and identical on the `ETag` response header and the
/// `eTag` body member of the list summary.
#[tokio::test]
async fn the_entity_tag_is_the_decimal_revision_and_advances_once_per_mutation() {
    /// One mutation in a row's sequence: the operation and the body to send it, built
    /// from the subscription's ARN.
    struct Mutation {
        operation: &'static str,
        body: fn(&str) -> Value,
    }

    struct Case {
        label: &'static str,
        tier: &'static str,
        approval: Option<&'static str>,
        /// Whether the create also activates the subscription — a write of its own, so
        /// such a record is one revision past the mint when the create answers, while a
        /// paid tier held for `ApprovePaidSubscription` is still at the mint.
        activated_by_the_create: bool,
        mutations: Vec<Mutation>,
    }

    // The rows differ in the tier they start at, the approval mode, and the number and
    // kind of mutations they take — so what is asserted is the counter, not one path.
    let cases = vec![
        Case {
            label: "no mutation at all",
            tier: "FREE",
            approval: None,
            activated_by_the_create: true,
            mutations: Vec::new(),
        },
        Case {
            label: "approval then two upgrades",
            tier: "PRO",
            approval: Some("MANUAL"),
            activated_by_the_create: false,
            mutations: vec![
                Mutation {
                    operation: "ApprovePaidSubscription",
                    body: |arn| json!({"arn": arn}),
                },
                Mutation {
                    operation: "UpdateSubscription",
                    body: |arn| json!({"arn": arn, "planTier": "BUSINESS"}),
                },
                Mutation {
                    operation: "UpdateSubscription",
                    body: |arn| json!({"arn": arn, "planTier": "PREMIUM"}),
                },
            ],
        },
        Case {
            label: "one upgrade",
            tier: "FREE",
            approval: Some("IMMEDIATE"),
            activated_by_the_create: true,
            mutations: vec![Mutation {
                operation: "UpdateSubscription",
                body: |arn| json!({"arn": arn, "planTier": "BUSINESS"}),
            }],
        },
        Case {
            label: "a scheduled downgrade, then its cancellation",
            tier: "PREMIUM",
            approval: Some("IMMEDIATE"),
            activated_by_the_create: true,
            mutations: vec![
                Mutation {
                    operation: "UpdateSubscription",
                    body: |arn| json!({"arn": arn, "planTier": "PRO"}),
                },
                Mutation {
                    operation: "CancelSubscription",
                    body: |arn| json!({"arn": arn}),
                },
            ],
        },
    ];

    let router = router();
    for case in &cases {
        let label = case.label;
        let created = create(
            &router,
            create_body("CloudFront", case.tier, 1, None, case.approval),
        )
        .await;
        let arn = created.string("arn");
        let mut token = created.require_etag(label);
        // The mint is the record's first write; activating it as part of the create is a
        // second one, which a tier held for approval has not taken yet.
        let first = if case.activated_by_the_create {
            FIRST_REVISION + 1
        } else {
            FIRST_REVISION
        };
        assert_eq!(
            revision_of(label, &token),
            first,
            "case {label}: the create's own writes put the first token at {first}, got \
             {token:?}"
        );

        for (step, mutation) in case.mutations.iter().enumerate() {
            let before = revision_of(label, &token);

            // A read reports the current token and does not move it.
            let read = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
            assert_eq!(
                read.require_etag(label),
                token,
                "case {label}/step {step}: a read must report the current tag"
            );
            let summary = summary_for(&router, &arn).await;
            assert_eq!(
                summary["eTag"].as_str(),
                Some(token.as_str()),
                "case {label}/step {step}: the summary's eTag must be the current tag, got \
                 {summary:?}"
            );

            let answer = call(
                &router,
                mutation.operation,
                (mutation.body)(&arn),
                Some(&token),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {label}/step {step}: {} under the current If-Match must succeed, got {} {:?}",
                mutation.operation, answer.status, answer.body
            );
            token = answer.require_etag(label);
            assert_eq!(
                revision_of(label, &token),
                before + 1,
                "case {label}/step {step}: {} must advance the revision by one, got {token:?}",
                mutation.operation
            );
        }

        // The record survives every row's sequence here (a cancellation of an ACTIVE
        // subscription schedules the termination rather than deleting the record), so the
        // final token is still readable on both paths.
        let read = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            read.status, 200,
            "case {label}: the subscription must still be readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.require_etag(label),
            token,
            "case {label}: the last tag a mutation minted is the one a read reports"
        );
        let summary = summary_for(&router, &arn).await;
        assert_eq!(
            summary["eTag"].as_str(),
            Some(token.as_str()),
            "case {label}: the summary's eTag must equal the record's current tag, got \
             {summary:?}"
        );
        assert_eq!(
            revision_of(label, &token),
            first + case.mutations.len() as u64,
            "case {label}: the revision counts the create's writes plus every mutation"
        );
    }
}

// ---------------------------------------------------------------------------
// Where the date-time form comes from
// ---------------------------------------------------------------------------

/// The `date-time` form reaches the wire through the protocol seam — the shared
/// `restJson1` codec this service returns from `Service::override_protocol` — because the
/// member schemas built from the model carry no timestamp format of their own. This pins
/// both halves of that: the seam is taken, and these are the members that depend on it.
///
/// The member lists are from
/// `smithy_query(query="shape", shape="Subscription,SubscriptionSummary,ScheduledChange")`;
/// they are the model's only timestamp members, which is what makes one protocol-wide
/// correction the right place for the rule.
#[test]
fn the_service_takes_the_shared_date_time_protocol() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    assert!(
        Service::override_protocol(&service).is_some(),
        "case \"protocol seam\": every timestamp member of this model is date-time, so the \
         service must answer override_protocol with the shared date-time codec"
    );

    struct Shape {
        name: &'static str,
        schema: &'static aws_smithy_schema::Schema,
        /// The members the model marks `@timestampFormat("date-time")`.
        date_time_members: &'static [&'static str],
    }

    let shapes = [
        Shape {
            name: "Subscription",
            schema: &Subscription::SCHEMA,
            date_time_members: &["createdAt", "updatedAt"],
        },
        Shape {
            name: "SubscriptionSummary",
            schema: &SubscriptionSummary::SCHEMA,
            date_time_members: &["createdAt", "updatedAt"],
        },
        Shape {
            name: "ScheduledChange",
            schema: &ScheduledChange::SCHEMA,
            date_time_members: &["effectiveDate"],
        },
    ];

    for shape in &shapes {
        let case = shape.name;
        for wanted in shape.date_time_members {
            let member = shape
                .schema
                .members()
                .iter()
                .find(|member| member.member_name() == Some(*wanted))
                .unwrap_or_else(|| {
                    panic!("case {case}: the model declares a {wanted} member, so the schema \
                            must carry one")
                });
            assert_eq!(
                member.shape_type(),
                aws_smithy_schema::ShapeType::Timestamp,
                "case {case}/{wanted}: the model targets this member at Timestamp, got {:?}",
                member.shape_type()
            );
        }
    }
}
