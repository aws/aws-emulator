// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteScheduledAudit`: what removing a DEVICE DEFENDER
//! scheduled audit does to the store every other scheduled-audit operation reads.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `DELETE /audit/scheduledaudits/{scheduledAuditName}` request signed for the `iot`
//! signing name — so what is asserted is what an AWS SDK client would see, not an internal
//! function's return value.
//!
//! The rule under test: a single-resource delete of this control plane REMOVES the record
//! the requested name identifies, so every later reader and writer of that store stops
//! reporting it and the name is free for a later create, while every other record is left
//! exactly as it was; a name the account holds no such record under is refused the
//! operation's modelled not-found error — in the same sentence the family's reader refuses
//! a miss with — rather than a hollow success. Here that means `DeleteScheduledAudit` drops
//! the schedule `CreateScheduledAudit` recorded under `scheduledAuditName`, after which
//! `DescribeScheduledAudit` of that name answers 404 `ResourceNotFoundException`,
//! `ListScheduledAudits` no longer reports it, the tag plane holds nothing for its ARN, and
//! a delete of a name the account holds no schedule under is itself refused 404.
//!
//! The table spans what that rule generalises over rather than one recorded request: every
//! modelled `AuditFrequency` (read through the generated constant, so a frequency the model
//! gains is a row this suite demands), both published bounds of `DayOfMonth`'s pattern
//! `^([1-9]|[12][0-9]|3[01])$|^LAST$` and its `LAST` form, days of the week, how many
//! checks a schedule targets, whether the create carried tags, the character classes and
//! both length bounds of `ScheduledAuditName` (`length 1..128, pattern ^[a-zA-Z0-9_-]+$`),
//! and the region the call is signed for — a schedule is addressed by name, so the delete
//! must reach it whatever region scoped its ARN.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::AUDIT_FREQUENCY_VALUES;
use service_com_amazonaws_iot_awsiotservice::state::AUDIT_CHECK_NAMES;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The role the account's audit configuration names (`RoleArn`, len 20..2048).
const AUDIT_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/device-defender-audit";

/// The longest legal `scheduledAuditName`: the member is modelled `length 1..128`
/// (`smithy_query(query="operation",operation="DeleteScheduledAudit")`).
const MAX_SCHEDULED_AUDIT_NAME_LEN: usize = 128;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// `region`: the scope is how the router picks the claiming service, and how the service
/// picks the region its ARNs are scoped to. The signature itself is never checked here —
/// the harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Whether the body reports no members at all — `null` on an empty payload, or an
    /// object with no keys.
    fn is_empty_payload(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator whose account has enabled every audit check in the catalog, which is the
    /// state a scheduled audit may target checks in ("Checks must be enabled for your
    /// account").
    async fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        let emulator = Self { router };

        let mut checks = Map::new();
        for check_name in AUDIT_CHECK_NAMES {
            checks.insert((*check_name).to_string(), json!({ "enabled": true }));
        }
        let configuration = json!({
            "roleArn": AUDIT_ROLE_ARN,
            "auditCheckConfigurations": Value::Object(checks),
        });
        let reply = emulator
            .call("PATCH", "/audit/configuration", Some(&configuration), REGION)
            .await;
        assert_eq!(
            reply.status, 200,
            "case setup: UpdateAccountAuditConfiguration must succeed, got {reply:?}"
        );
        emulator
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreateScheduledAudit`, bound to `POST /audit/scheduledaudits/{scheduledAuditName}`
    /// by this service's own model.
    async fn create_in(&self, name: &str, body: &Value, region: &str) -> Reply {
        self.call(
            "POST",
            &format!("/audit/scheduledaudits/{name}"),
            Some(body),
            region,
        )
        .await
    }

    /// `DescribeScheduledAudit`, bound to
    /// `GET /audit/scheduledaudits/{scheduledAuditName}`.
    async fn describe(&self, name: &str) -> Reply {
        self.call("GET", &format!("/audit/scheduledaudits/{name}"), None, REGION)
            .await
    }

    /// `DeleteScheduledAudit`, bound to
    /// `DELETE /audit/scheduledaudits/{scheduledAuditName}`.
    async fn delete(&self, name: &str) -> Reply {
        self.delete_in(name, REGION).await
    }

    async fn delete_in(&self, name: &str, region: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/audit/scheduledaudits/{name}"),
            None,
            region,
        )
        .await
    }

    /// The names `ListScheduledAudits` reports, in the order it reports them.
    async fn listed_names(&self) -> Vec<String> {
        let reply = self
            .call("GET", "/audit/scheduledaudits?maxResults=250", None, REGION)
            .await;
        assert_eq!(
            reply.status, 200,
            "ListScheduledAudits must succeed, got {reply:?}"
        );
        reply.body["scheduledAudits"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a scheduledAudits list, got {reply:?}"))
            .iter()
            .map(|entry| {
                entry["scheduledAuditName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The tags the tag plane holds for `resource_arn`, as `Tag$Key` -> `Tag$Value`.
    async fn tags_of(&self, resource_arn: &str) -> Vec<(String, Option<String>)> {
        let query = ::form_urlencoded::Serializer::new(String::new())
            .append_pair("resourceArn", resource_arn)
            .finish();
        let reply = self
            .call("GET", &format!("/tags?{query}"), None, REGION)
            .await;
        assert_eq!(
            reply.status, 200,
            "ListTagsForResource must succeed, got {reply:?}"
        );
        reply.body["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a tags list, got {reply:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    /// `TagResource` on `resource_arn`: a WRITE of the tag plane, which refuses an ARN the
    /// account holds no resource for.
    async fn tag(&self, resource_arn: &str) -> Reply {
        let body = json!({
            "resourceArn": resource_arn,
            "tags": [{ "Key": "owner", "Value": "platform" }],
        });
        self.call("POST", "/tags", Some(&body), REGION).await
    }
}

/// The sentence every reader and writer of one scheduled audit refuses a name the account
/// holds no schedule under with, as the DEVICE DEFENDER configuration families word one:
/// the fixed word `Resource`, the input member the name was carried at, `=`, that name, and
/// the fixed clause ` was not found`.
fn missing_message(name: &str) -> String {
    format!("Resource scheduledAuditName={name} was not found")
}

/// The ARN this plane mints for `name` in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:scheduledaudit/{name}")
}

/// One scheduled audit to create and then delete: a label, the name, the region the create
/// is signed for, and the create's body.
struct Case {
    label: &'static str,
    name: String,
    region: &'static str,
    body: Value,
}

/// The table every case list below is built from. It spans what the rule generalises over:
///
/// * every modelled `AuditFrequency` and the day member each one demands — `dayOfMonth` for
///   `MONTHLY`, `dayOfWeek` for `WEEKLY` and `BIWEEKLY`, neither for `DAILY` — including
///   both published bounds of `DayOfMonth`'s pattern `^([1-9]|[12][0-9]|3[01])$|^LAST$` and
///   its `LAST` form;
/// * how many checks a schedule targets, read out of the account's own catalog;
/// * whether the create carried tags, since what a delete removes must not depend on it;
/// * the form and length of `scheduledAuditName` — modelled `length 1..128, pattern
///   `^[a-zA-Z0-9_-]+$`` — so the rows include the shortest legal name, the longest, one
///   using every character class the pattern allows, and one that is another row's prefix;
/// * the region the create and the delete are signed for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a daily schedule naming no day at all",
            name: "nightly".to_string(),
            region: REGION,
            body: json!({
                "frequency": "DAILY",
                "targetCheckNames": [AUDIT_CHECK_NAMES[0]],
            }),
        },
        Case {
            label: "a weekly schedule with tags",
            name: "nightly-weekly".to_string(),
            region: REGION,
            body: json!({
                "frequency": "WEEKLY",
                "dayOfWeek": "MON",
                "targetCheckNames": [AUDIT_CHECK_NAMES[0], AUDIT_CHECK_NAMES[1]],
                "tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
        },
        Case {
            label: "a biweekly schedule signed for another region",
            name: "Bi_Weekly-99".to_string(),
            region: "eu-west-3",
            body: json!({
                "frequency": "BIWEEKLY",
                "dayOfWeek": "SAT",
                "targetCheckNames": [AUDIT_CHECK_NAMES[1]],
            }),
        },
        Case {
            label: "a monthly schedule on the first day its pattern allows",
            name: "a".to_string(),
            region: REGION,
            body: json!({
                "frequency": "MONTHLY",
                "dayOfMonth": "1",
                "targetCheckNames": [AUDIT_CHECK_NAMES[2]],
            }),
        },
        Case {
            label: "a monthly schedule on the last day its pattern allows",
            name: "monthly-31".to_string(),
            region: "ap-southeast-2",
            body: json!({
                "frequency": "MONTHLY",
                "dayOfMonth": "31",
                "targetCheckNames": [AUDIT_CHECK_NAMES[0], AUDIT_CHECK_NAMES[2]],
            }),
        },
        Case {
            label: "the longest legal name, on the LAST day of the month",
            name: "z".repeat(MAX_SCHEDULED_AUDIT_NAME_LEN),
            region: REGION,
            body: json!({
                "frequency": "MONTHLY",
                "dayOfMonth": "LAST",
                "targetCheckNames": [AUDIT_CHECK_NAMES[1]],
                "tags": [{ "Key": "retention", "Value": "none" }],
            }),
        },
    ]
}

/// The table covers every modelled `AuditFrequency`, so a frequency the model gains is a
/// row this suite demands rather than a case nothing exercises.
#[test]
fn the_table_spans_every_modeled_audit_frequency() {
    let covered: std::collections::BTreeSet<String> = cases()
        .iter()
        .map(|case| case.body["frequency"].as_str().unwrap_or_default().to_string())
        .collect();
    let modeled: std::collections::BTreeSet<String> = AUDIT_FREQUENCY_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        covered, modeled,
        "expected one row per modelled AuditFrequency {AUDIT_FREQUENCY_VALUES:?}, got {covered:?}"
    );
}

#[tokio::test]
async fn deleting_a_scheduled_audit_removes_what_describe_reported() {
    for case in cases() {
        let label = case.label;
        // A fresh emulator per row, so what the delete removes is what this row created.
        let emulator = Emulator::new().await;

        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateScheduledAudit must succeed, got {created:?}"
        );
        let before = emulator.describe(&case.name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the created schedule must be readable before the delete, got \
             {before:?}"
        );

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteScheduledAudit must succeed, got {deleted:?}"
        );
        // `DeleteScheduledAuditResponse` models no members: the success reports nothing,
        // and that the schedule is gone is read back through the readers below.
        assert!(
            deleted.is_empty_payload(),
            "case {label:?}: a successful delete reports no members, got {deleted:?}"
        );

        let after = emulator.describe(&case.name).await;
        assert_eq!(
            after.status, 404,
            "case {label:?}: the deleted schedule must be gone for every later reader, got \
             {after:?}"
        );
        assert_eq!(
            after.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {after:?}"
        );
        assert_eq!(
            after.text("message"),
            Some(missing_message(&case.name)),
            "case {label:?}: unexpected not-found message, got {after:?}"
        );
        assert!(
            after.body.get("frequency").is_none(),
            "case {label:?}: a refusal reports no schedule, got {after:?}"
        );

        // And the listing of the account's schedules reports none at all: the one record
        // the delete removed is the only one this row held.
        let listed = emulator.listed_names().await;
        assert!(
            listed.is_empty(),
            "case {label:?}: the deleted schedule must be gone from the listing, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_one_scheduled_audit_leaves_every_other_in_place() {
    let emulator = Emulator::new().await;

    // Every row of the table lives in one store, so each delete below is a delete of one
    // record out of many rather than of the only record there is.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateScheduledAudit must succeed, got {created:?}"
        );
    }

    let mut remaining: Vec<Case> = cases();
    while !remaining.is_empty() {
        let case = remaining.remove(0);
        let label = case.label;
        // What every surviving schedule reports right before the delete, so the comparison
        // after it is against a value this test read rather than one it rebuilt.
        let mut survivors = Vec::new();
        for other in &remaining {
            survivors.push((other.label, emulator.describe(&other.name).await));
        }

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteScheduledAudit must succeed, got {deleted:?}"
        );

        for ((survivor_label, before), other) in survivors.into_iter().zip(&remaining) {
            let after = emulator.describe(&other.name).await;
            assert_eq!(
                after.status, 200,
                "case {label:?}: deleting it must leave {survivor_label:?} readable, got \
                 {after:?}"
            );
            assert_eq!(
                before.body, after.body,
                "case {label:?}: deleting it must leave {survivor_label:?} untouched, got \
                 {after:?}"
            );
        }

        // The listing reports exactly the records that are left, in the order it sorts
        // them, and never the one just removed.
        let mut expected: Vec<String> = remaining.iter().map(|other| other.name.clone()).collect();
        expected.sort();
        let listed = emulator.listed_names().await;
        assert_eq!(
            listed, expected,
            "case {label:?}: the listing must report exactly the surviving schedules"
        );
    }
}

#[tokio::test]
async fn a_scheduled_audit_the_store_does_not_hold_is_refused_not_found() {
    let emulator = Emulator::new().await;

    // One schedule exists and stays, so the refusals below are about the NAME not being in
    // the store rather than the store being empty.
    let kept = json!({
        "frequency": "DAILY",
        "targetCheckNames": [AUDIT_CHECK_NAMES[0]],
    });
    let created = emulator.create_in("existing-audit", &kept, REGION).await;
    assert_eq!(
        created.status, 200,
        "the setup CreateScheduledAudit must succeed, got {created:?}"
    );

    // A schedule created and then deleted: the SECOND delete of it is refused for exactly
    // the same reason a name nobody created is — the account holds no schedule under it, so
    // a repeated delete is not a hollow success.
    let created = emulator.create_in("deleted-once", &kept, REGION).await;
    assert_eq!(
        created.status, 200,
        "the setup CreateScheduledAudit must succeed, got {created:?}"
    );
    let deleted = emulator.delete("deleted-once").await;
    assert_eq!(
        deleted.status, 200,
        "the first DeleteScheduledAudit must succeed, got {deleted:?}"
    );

    // The names differ in form, length and case — a scheduled audit name is
    // case-sensitive, so a name that differs from an existing one only in case names no
    // schedule.
    for name in [
        "a",
        "deleted-once",
        "unknown-audit",
        "existing-audit-2",
        "EXISTING-AUDIT",
        "Missing_Audit-9",
        &"n".repeat(MAX_SCHEDULED_AUDIT_NAME_LEN),
    ] {
        let refused = emulator.delete(name).await;
        assert_eq!(
            refused.status, 404,
            "case {name:?}: DeleteScheduledAudit of a name the account holds no schedule \
             under must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        // The refusal is the family's sentence with the name taken from the request, and it
        // is the SAME sentence the family's reader refuses a miss with, so the two cannot
        // drift.
        assert_eq!(
            refused.text("message"),
            Some(missing_message(name)),
            "case {name:?}: unexpected not-found message, got {refused:?}"
        );
        assert_eq!(
            refused.text("message"),
            emulator.describe(name).await.text("message"),
            "case {name:?}: the delete and the read must word the miss the same way, got \
             {refused:?}"
        );
    }

    // The refused deletes changed nothing: the schedule that was there is still there, and
    // it is the only one listed.
    let survivor = emulator.describe("existing-audit").await;
    assert_eq!(
        survivor.status, 200,
        "a refused delete must leave the existing schedule in place, got {survivor:?}"
    );
    let listed = emulator.listed_names().await;
    assert_eq!(
        listed,
        vec!["existing-audit".to_string()],
        "a refused delete must leave the listing as it was, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_frees_the_name_for_a_later_create() {
    let emulator = Emulator::new().await;

    // The name is the schedule's identity, so while a schedule holds it a create is
    // refused; once the delete removes the record the name is free again, and what the
    // re-create records is the NEW schedule rather than anything the deleted one carried.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first CreateScheduledAudit must succeed, got {created:?}"
        );

        let refused = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: a create under a name the store holds must be refused 409, got \
             {refused:?}"
        );

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteScheduledAudit must succeed, got {deleted:?}"
        );

        // The re-create names a schedule no row above does, so what the reader reports
        // afterwards can only come from this request.
        let recreated_body = json!({
            "frequency": "WEEKLY",
            "dayOfWeek": "THU",
            "targetCheckNames": [AUDIT_CHECK_NAMES[2]],
        });
        let recreated = emulator
            .create_in(&case.name, &recreated_body, case.region)
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: the name must be free after the delete, got {recreated:?}"
        );

        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the re-created schedule must be readable, got {read:?}"
        );
        assert_eq!(
            read.body.get("frequency"),
            recreated_body.get("frequency"),
            "case {label:?}: the re-created schedule must report the new frequency, got \
             {read:?}"
        );
        assert_eq!(
            read.body.get("dayOfWeek"),
            recreated_body.get("dayOfWeek"),
            "case {label:?}: the re-created schedule must report the new day of the week, \
             got {read:?}"
        );
        assert!(
            read.body.get("dayOfMonth").is_none(),
            "case {label:?}: the re-created schedule names no day of the month, got {read:?}"
        );
        assert_eq!(
            read.body.get("targetCheckNames"),
            recreated_body.get("targetCheckNames"),
            "case {label:?}: the re-created schedule must report the new checks, got {read:?}"
        );
        assert_eq!(
            read.body.get("scheduledAuditArn").and_then(Value::as_str),
            Some(expected_arn(case.region, &case.name).as_str()),
            "case {label:?}: the re-created schedule must carry the ARN its region mints, \
             got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_schedule_holds_nothing_on_the_tag_plane() {
    let emulator = Emulator::new().await;

    // Only the rows whose create carried tags: the tags a schedule carries are part of the
    // record the delete removes, so after it the tag plane holds nothing for that ARN and
    // its write refuses the ARN outright.
    for case in cases() {
        let label = case.label;
        let Some(tags) = case.body.get("tags").and_then(Value::as_array) else {
            continue;
        };
        let expected: Vec<(String, Option<String>)> = {
            let mut pairs: Vec<(String, Option<String>)> = tags
                .iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().map(str::to_string),
                    )
                })
                .collect();
            pairs.sort();
            pairs
        };

        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateScheduledAudit must succeed, got {created:?}"
        );
        let arn = expected_arn(case.region, &case.name);

        let mut held = emulator.tags_of(&arn).await;
        held.sort();
        assert_eq!(
            held, expected,
            "case {label:?}: the tag plane must report the tags the create carried"
        );

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteScheduledAudit must succeed, got {deleted:?}"
        );

        let held = emulator.tags_of(&arn).await;
        assert!(
            held.is_empty(),
            "case {label:?}: the deleted schedule's tags must be gone, got {held:?}"
        );
        let refused = emulator.tag(&arn).await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: a tag write must refuse the ARN of a schedule the account no \
             longer holds, got {refused:?}"
        );
        assert_eq!(
            refused.text("message"),
            Some(missing_message(&case.name)),
            "case {label:?}: the tag write must word the miss the family's way, got \
             {refused:?}"
        );
    }
}
