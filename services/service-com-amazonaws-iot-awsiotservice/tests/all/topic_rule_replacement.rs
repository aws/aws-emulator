// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ReplaceTopicRule`, proved through the `GetTopicRule`
//! and `ListTopicRules` readers.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `ReplaceTopicRule` (`PATCH /rules/{ruleName}`) records the
//! request's `TopicRulePayload` as the rule's WHOLE payload, so a member the new
//! payload omits is absent from the rule afterwards instead of being kept from the
//! rule it replaced, and the new payload's optional members are resolved exactly as a
//! create resolves them (an absent `ruleDisabled` is an enabled rule, an absent
//! `awsIotSqlVersion` is the SQL reference's default version); what the payload does
//! not carry — the rule's name, its ARN and its `createdAt` — is unchanged. The output
//! shape is `Unit`, so a success is an empty 200, and a rule name the store does not
//! hold is refused with the operation's modelled `UnauthorizedException` (401,
//! `smithy_query(query="operation",operation="ReplaceTopicRule")` lists no not-found
//! error), whose message is AWS's topic-rule access refusal — `Access to topic rule
//! '<ruleName>' was denied` — while writing nothing.
//!
//! The table spans what that rule generalises over — which optional payload members
//! the rule starts with and which the replacement carries (so each member is both
//! added and dropped), the number and types of actions on either side, the SQL
//! statement and therefore the reported `topicPattern`, the `awsIotSqlVersion`, the
//! form and length of the rule name, the region the call is signed for, and a rule
//! replaced repeatedly — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The `awsIotSqlVersion` a rule whose payload omits one must report.
///
/// From `state::DEFAULT_AWS_IOT_SQL_VERSION`: the AWS IoT SQL reference's "SQL
/// versions" section lists `2015-10-08`, `2016-03-23` and `beta`, and `2015-10-08` is
/// the version used when a rule names none.
const DEFAULT_SQL_VERSION: &str = "2015-10-08";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
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
    message: Option<String>,
    body: Value,
}

impl Reply {
    /// The `rule` member of a `GetTopicRule` response.
    fn rule(&self) -> Option<&Value> {
        self.body.get("rule")
    }

    /// A member of the reported `TopicRule`, or `None` when the rule or the member is
    /// absent.
    fn rule_member(&self, name: &str) -> Option<&Value> {
        self.rule().and_then(|rule| rule.get(name))
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string);
        Reply {
            status,
            error_code,
            message,
            body,
        }
    }

    async fn create_rule_in(&self, rule_name: &str, payload: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}"), Some(payload), region)
            .await
    }

    async fn replace_rule_in(&self, rule_name: &str, payload: &Value, region: &str) -> Reply {
        self.call(
            "PATCH",
            &format!("/rules/{rule_name}"),
            Some(payload),
            region,
        )
        .await
    }

    async fn get_rule_in(&self, rule_name: &str, region: &str) -> Reply {
        self.call("GET", &format!("/rules/{rule_name}"), None, region)
            .await
    }

    async fn disable_rule_in(&self, rule_name: &str, region: &str) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}/disable"), None, region)
            .await
    }

    async fn list_rules_in(&self, region: &str) -> Reply {
        self.call("GET", "/rules", None, region).await
    }
}

/// What the stored rule must report for one member after a payload was recorded:
/// `None` means the member must be absent from the reported `TopicRule`.
fn expected_member(payload: &Value, member: &str) -> Option<Value> {
    match member {
        // `ruleDisabled` and `awsIotSqlVersion` are optional in the payload and
        // resolved on the write: an absent `ruleDisabled` is an enabled rule, an
        // absent `awsIotSqlVersion` is the default SQL version.
        "ruleDisabled" => Some(payload.get(member).cloned().unwrap_or(Value::Bool(false))),
        "awsIotSqlVersion" => Some(
            payload
                .get(member)
                .cloned()
                .unwrap_or_else(|| Value::String(DEFAULT_SQL_VERSION.to_string())),
        ),
        // `sql` and `actions` are `@required`, so the payload always carries them.
        "sql" | "actions" => Some(payload[member].clone()),
        // `description` and `errorAction` are optional and carry no default: a
        // payload without one leaves the member absent.
        _ => payload.get(member).cloned(),
    }
}

/// Every member of `TopicRule` a `TopicRulePayload` carries
/// (`smithy_query(query="shape",shape="TopicRulePayload,TopicRule")`): the members a
/// replace is responsible for. `ruleName` and `createdAt` are the rule's identity and
/// are not payload members.
const PAYLOAD_MEMBERS: [&str; 6] = [
    "sql",
    "description",
    "actions",
    "ruleDisabled",
    "awsIotSqlVersion",
    "errorAction",
];

/// Assert that the rule read back reports exactly what `payload` says, member for
/// member, plus the identity members it must have kept.
fn assert_rule_is(read: &Reply, payload: &Value, case: &str, rule_name: &str, region: &str) {
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetTopicRule must find the rule, got {read:?}"
    );
    assert_eq!(
        read.body.get("ruleArn").and_then(Value::as_str),
        Some(expected_arn(region, rule_name).as_str()),
        "case {case:?}: expected the rule's ARN to be unchanged, got {read:?}"
    );
    assert_eq!(
        read.rule_member("ruleName").and_then(Value::as_str),
        Some(rule_name),
        "case {case:?}: expected ruleName {rule_name:?}, got {read:?}"
    );

    for member in PAYLOAD_MEMBERS {
        let expected = expected_member(payload, member);
        let actual = read.rule_member(member).cloned();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {member} to be {expected:?}, got {actual:?} in {read:?}"
        );
    }
}

/// The ARN a rule created in `region` must carry.
fn expected_arn(region: &str, rule_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:rule/{rule_name}")
}

/// A payload with every optional member set, plus several action types.
fn rich_payload() -> Value {
    json!({
        "sql": "SELECT temperature FROM 'sensors/#' WHERE temperature > 30",
        "description": "fan out hot readings",
        "ruleDisabled": true,
        "awsIotSqlVersion": "2016-03-23",
        "actions": [
            {
                "republish": {
                    "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                    "topic": "alerts/hot",
                    "qos": 1,
                },
            },
            { "lambda": { "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:on-event" } },
            {
                "sqs": {
                    "roleArn": "arn:aws:iam::000000000000:role/iot-sqs",
                    "queueUrl": "https://sqs.us-east-1.amazonaws.com/000000000000/events",
                    "useBase64": false,
                },
            },
        ],
        "errorAction": {
            "republish": {
                "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                "topic": "errors",
            },
        },
    })
}

/// A payload carrying only the two `@required` members, and no actions at all.
fn bare_payload() -> Value {
    json!({ "sql": "SELECT * FROM 'bare/#'", "actions": [] })
}

/// One rule the table creates, and the payloads it is then replaced with in order.
struct Case {
    label: &'static str,
    rule_name: String,
    region: &'static str,
    created: Value,
    replacements: Vec<Value>,
}

/// The table every case below is built from. It spans what the rule generalises over:
///
/// * which optional payload members the rule starts with and which the replacement
///   carries — every one of `description`, `ruleDisabled`, `awsIotSqlVersion` and
///   `errorAction` is both ADDED by a replacement and DROPPED by one;
/// * the number and types of actions on either side (`ActionList` is modelled
///   `length 0..10`), including a replacement that empties the list and one that
///   fills it;
/// * the SQL statement, so the `topicPattern` a listing reports is re-read from the
///   new statement;
/// * the `awsIotSqlVersion` — the SQL reference's `2015-10-08`, `2016-03-23` and
///   `beta`, plus absent (the default);
/// * the form and length of `ruleName` (`RuleName` is modelled
///   `length 1..128, pattern ^[a-zA-Z0-9_]+$`), so the rows include the shortest
///   legal name, the longest, and one using every character class the pattern allows;
/// * the region the call is signed for;
/// * how many replacements are applied in a row, including one that re-records the
///   payload the rule already has.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a rule with every optional member replaced by a bare payload",
            rule_name: "replace_rich_with_bare".to_string(),
            region: REGION,
            created: rich_payload(),
            replacements: vec![bare_payload()],
        },
        Case {
            label: "a bare rule replaced by a payload with every optional member",
            rule_name: "replace_bare_with_rich".to_string(),
            region: REGION,
            created: bare_payload(),
            replacements: vec![rich_payload()],
        },
        Case {
            label: "alternating rich and bare payloads on one rule",
            rule_name: "Rule_01_aZ".to_string(),
            region: REGION,
            created: bare_payload(),
            replacements: vec![
                rich_payload(),
                bare_payload(),
                rich_payload(),
                rich_payload(),
            ],
        },
        Case {
            label: "a disabled rule replaced by a payload that omits ruleDisabled",
            rule_name: "replace_drops_disabled".to_string(),
            region: REGION,
            created: json!({
                "sql": "SELECT * FROM 'devices/+/events'",
                "ruleDisabled": true,
                "actions": [],
            }),
            replacements: vec![json!({
                "sql": "SELECT * FROM 'devices/+/events'",
                "actions": [],
            })],
        },
        Case {
            label: "a replacement that only changes the SQL statement",
            rule_name: "replace_sql_only".to_string(),
            region: REGION,
            created: json!({
                "sql": "SELECT * FROM 'before/#'",
                "description": "stable description",
                "actions": [{
                    "kinesis": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-kinesis",
                        "streamName": "shadow-updates",
                        "partitionKey": "${topic()}",
                    },
                }],
            }),
            replacements: vec![json!({
                "sql": "SELECT state FROM 'after/+/shadow' WHERE state = 'on'",
                "description": "stable description",
                "actions": [{
                    "kinesis": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-kinesis",
                        "streamName": "shadow-updates",
                        "partitionKey": "${topic()}",
                    },
                }],
            })],
        },
        Case {
            label: "a replacement that empties a full action list, shortest legal name",
            rule_name: "a".to_string(),
            region: "eu-west-1",
            created: json!({
                "sql": "SELECT * FROM 'eu/#'",
                "awsIotSqlVersion": "beta",
                "actions": (0..10)
                    .map(|index| json!({
                        "lambda": {
                            "functionArn": format!(
                                "arn:aws:lambda:eu-west-1:000000000000:function:fan{index}"
                            ),
                        },
                    }))
                    .collect::<Vec<Value>>(),
            }),
            replacements: vec![json!({ "sql": "SELECT * FROM 'eu/quiet'", "actions": [] })],
        },
        Case {
            label: "the longest legal rule name, replaced in another region",
            rule_name: "r".repeat(128),
            region: "ap-southeast-2",
            created: bare_payload(),
            replacements: vec![json!({
                "sql": "SELECT * FROM 'long/+/path'",
                "description": "",
                "ruleDisabled": false,
                "awsIotSqlVersion": "2015-10-08",
                "actions": [{
                    "sns": {
                        "targetArn": "arn:aws:sns:ap-southeast-2:000000000000:events",
                        "roleArn": "arn:aws:iam::000000000000:role/iot-sns",
                        "messageFormat": "JSON",
                    },
                }],
            })],
        },
    ]
}

#[tokio::test]
async fn replace_records_the_new_payload_whole_and_keeps_the_rule_identity() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let name = case.rule_name.as_str();
        let region = case.region;

        let created = emulator.create_rule_in(name, &case.created, region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTopicRule must succeed, got {created:?}"
        );

        let after_create = emulator.get_rule_in(name, region).await;
        assert_rule_is(&after_create, &case.created, label, name, region);
        // `createdAt` belongs to the rule, not to its payload: every replacement
        // below must report this same instant.
        let created_at = after_create
            .rule_member("createdAt")
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the created rule reported no createdAt"));

        for (index, replacement) in case.replacements.iter().enumerate() {
            let replaced = emulator.replace_rule_in(name, replacement, region).await;
            assert_eq!(
                replaced.status, 200,
                "case {label:?} replacement {index}: must answer 200, got {replaced:?}"
            );
            // The output shape is `Unit`: a success carries no members.
            assert_eq!(
                replaced.body,
                Value::Null,
                "case {label:?} replacement {index}: the output shape is Unit, got {replaced:?}"
            );

            let read = emulator.get_rule_in(name, region).await;
            let case_label = format!("{label} (replacement {index})");
            assert_rule_is(&read, replacement, &case_label, name, region);
            assert_eq!(
                read.rule_member("createdAt").cloned(),
                Some(created_at.clone()),
                "case {case_label:?}: expected createdAt {created_at:?} to survive the replace, \
                 got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn replace_updates_the_topic_pattern_a_listing_reports() {
    let emulator = Emulator::new();

    // One row per SQL statement a replacement installs, with the topic filter the
    // statement's `FROM` clause names — what `TopicRuleListItem$topicPattern` reports.
    let statements = [
        ("SELECT * FROM 'sensors/+/telemetry'", Some("sensors/+/telemetry")),
        ("select temperature from 'rooms/#' where temperature > 10", Some("rooms/#")),
        ("SELECT * FROM 'single'", Some("single")),
        // A statement naming no topic filter: the pattern is unknown, so the member
        // is reported absent rather than invented.
        ("SELECT * WHERE 1 = 1", None),
    ];

    let name = "pattern_rule";
    let created = emulator
        .create_rule_in(name, &json!({ "sql": "SELECT * FROM 'initial/#'", "actions": [] }), REGION)
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateTopicRule must succeed, got {created:?}"
    );

    for (sql, pattern) in statements {
        let replaced = emulator
            .replace_rule_in(name, &json!({ "sql": sql, "actions": [] }), REGION)
            .await;
        assert_eq!(
            replaced.status, 200,
            "case {sql:?}: ReplaceTopicRule must succeed, got {replaced:?}"
        );

        let listed = emulator.list_rules_in(REGION).await;
        assert_eq!(
            listed.status, 200,
            "case {sql:?}: ListTopicRules must succeed, got {listed:?}"
        );
        let item = listed
            .body
            .get("rules")
            .and_then(Value::as_array)
            .and_then(|rules| {
                rules
                    .iter()
                    .find(|rule| rule.get("ruleName").and_then(Value::as_str) == Some(name))
            })
            .cloned()
            .unwrap_or_else(|| panic!("case {sql:?}: the rule is missing from {listed:?}"));
        assert_eq!(
            item.get("topicPattern").and_then(Value::as_str),
            pattern,
            "case {sql:?}: expected topicPattern {pattern:?}, got {item:?}"
        );
    }
}

#[tokio::test]
async fn replace_leaves_every_other_rule_alone() {
    let emulator = Emulator::new();

    for name in ["neighbour_one", "neighbour_two", "replace_target"] {
        let created = emulator
            .create_rule_in(
                name,
                &json!({ "sql": format!("SELECT * FROM '{name}/#'"), "actions": [] }),
                REGION,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateTopicRule must succeed, got {created:?}"
        );
    }

    let before_one = emulator.get_rule_in("neighbour_one", REGION).await;
    let before_two = emulator.get_rule_in("neighbour_two", REGION).await;

    for replacement in [rich_payload(), bare_payload()] {
        let replaced = emulator
            .replace_rule_in("replace_target", &replacement, REGION)
            .await;
        assert_eq!(
            replaced.status, 200,
            "the replace must succeed, got {replaced:?}"
        );

        let after_one = emulator.get_rule_in("neighbour_one", REGION).await;
        let after_two = emulator.get_rule_in("neighbour_two", REGION).await;
        assert_eq!(
            before_one.body, after_one.body,
            "an untargeted rule must be untouched, got {after_one:?}"
        );
        assert_eq!(
            before_two.body, after_two.body,
            "an untargeted rule must be untouched, got {after_two:?}"
        );
    }
}

#[tokio::test]
async fn a_replaced_rule_is_still_the_rule_the_enablement_pair_flips() {
    let emulator = Emulator::new();

    let name = "replace_then_disable";
    let created = emulator
        .create_rule_in(name, &rich_payload(), REGION)
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateTopicRule must succeed, got {created:?}"
    );

    // The replacement omits `ruleDisabled`, so the rule is enabled afterwards even
    // though the rule it replaced was disabled.
    let replacement = bare_payload();
    let replaced = emulator.replace_rule_in(name, &replacement, REGION).await;
    assert_eq!(
        replaced.status, 200,
        "ReplaceTopicRule must succeed, got {replaced:?}"
    );
    let read = emulator.get_rule_in(name, REGION).await;
    assert_rule_is(&read, &replacement, "replace then disable", name, REGION);

    // `DisableTopicRule` still finds the replaced rule and flips only that member.
    let disabled = emulator.disable_rule_in(name, REGION).await;
    assert_eq!(
        disabled.status, 200,
        "DisableTopicRule must succeed on the replaced rule, got {disabled:?}"
    );
    let mut expected = replacement.clone();
    expected["ruleDisabled"] = Value::Bool(true);
    let after_disable = emulator.get_rule_in(name, REGION).await;
    assert_rule_is(
        &after_disable,
        &expected,
        "replace then disable",
        name,
        REGION,
    );
}

#[tokio::test]
async fn a_rule_the_store_does_not_hold_cannot_be_replaced() {
    let emulator = Emulator::new();

    // One rule exists, so the refusals below are about the NAME not being in the
    // store rather than the store being empty.
    let created = emulator
        .create_rule_in("existing_rule", &rich_payload(), REGION)
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateTopicRule must succeed, got {created:?}"
    );
    let before = emulator.get_rule_in("existing_rule", REGION).await;

    // `ReplaceTopicRule` models no not-found error, so an unknown rule name is
    // refused with the modelled `UnauthorizedException` (401) — never an empty 200,
    // and never a create.
    for rule_name in [
        "a",
        "unknown_rule",
        "existing_rule_2",
        "EXISTING_RULE",
        &"r".repeat(128),
    ] {
        for payload in [bare_payload(), rich_payload()] {
            let refused = emulator.replace_rule_in(rule_name, &payload, REGION).await;
            assert_eq!(
                refused.status, 401,
                "case {rule_name:?}: an unknown rule must be refused 401, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("UnauthorizedException"),
                "case {rule_name:?}: expected UnauthorizedException, got {refused:?}"
            );
            assert_eq!(
                refused.message.as_deref(),
                Some(format!("Access to topic rule '{rule_name}' was denied").as_str()),
                "case {rule_name:?}: the refusal must name the rule the request asked for, \
                 got {refused:?}"
            );

            // The refusal recorded nothing under that name: the rule is still absent.
            let read = emulator.get_rule_in(rule_name, REGION).await;
            assert_eq!(
                read.status, 401,
                "case {rule_name:?}: the refusal must record no rule, got {read:?}"
            );
        }
    }

    // The rule that does exist is untouched by the refusals.
    let after = emulator.get_rule_in("existing_rule", REGION).await;
    assert_eq!(
        before.body, after.body,
        "the refusals must leave the existing rule alone, got {after:?}"
    );
}
