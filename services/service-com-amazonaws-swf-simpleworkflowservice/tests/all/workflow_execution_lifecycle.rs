// SPDX-License-Identifier: CC0-1.0

//! The SWF domain -> workflow-type -> workflow-execution lifecycle, end to end over
//! the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<OperationName>`), dispatched through the same
//! `MultiServiceRouter` the emulator mounts in production. Every assertion therefore
//! checks the HTTP behaviour a client sees, not an internal call into a handler.
//!
//! The rule every case here asserts: each operation answers from the ONE stored
//! record it addresses — Register*/Start/Terminate write it, Describe*/GetHistory read
//! it back — so no response is an echo of the request that asked for it, and a value
//! the request never carried (the minted `runId`, the resolved configuration, the
//! event log, the open counts) comes from state alone. Each table row varies what the
//! rule generalises over (retention value, tag count, name shape, which defaults are
//! registered, which of them the request overrides, child policy, tag-list size, page
//! size, ordering, whether `runId` is passed); no row is privileged.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::{
    CHILD_POLICY_VALUES, CLOSE_STATUS_VALUES, EVENT_TYPE_VALUES, EXECUTION_STATUS_VALUES,
    REGISTRATION_STATUS_VALUES,
};
use service_com_amazonaws_swf_simpleworkflowservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The members this service answers with that its published model snapshot does not
/// declare, pinned on the wire through this harness and against the overlay model they are
/// generated from, which is why it is a child of this suite rather than a second suite
/// beside it: every row it drives is one of these lifecycle operations. The `#[path]` is
/// what keeps it one — this crate's `tests/all.rs` declares the suites directly under
/// `tests/all/`, and a module of a `#[path]`-declared file resolves next to that file
/// unless it says otherwise.
#[path = "workflow_execution_lifecycle/local_model_additions.rs"]
mod local_model_additions;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SimpleWorkflowService";

/// The region the `Authorization` credential scope below signs for; every ARN this
/// service mints for these requests carries it.
pub(crate) const REGION: &str = "us-east-1";

/// The account segment every emulated ARN carries.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: a fresh engine behind a router that claims `swf` requests.
///
/// `pub(crate)` so the sibling suites under `tests/all/` drive the service through the
/// same router rather than each standing up a harness of its own.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(status, body)`. The `Authorization` header's
    /// credential scope is what routes the request to this service.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/swf/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error}: {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one operation, requiring a 200, and return the body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// Call one operation, requiring a 400 with `expected_code`.
    pub(crate) async fn refused(
        &self,
        case: &str,
        operation: &str,
        body: &Value,
        expected_code: &str,
    ) {
        let (status, parsed) = self.call(operation, body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected {operation} to answer 400 {expected_code}, \
             got {status} {code} with {parsed:?}"
        );
    }
}

/// The `__type` discriminator of an awsJson error body, reduced to the shape name.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

pub(crate) fn string_at(body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {pointer} of {body:?}"))
        .to_string()
}

pub(crate) fn i64_at(body: &Value, pointer: &str) -> i64 {
    body.pointer(pointer)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an integer at {pointer} of {body:?}"))
}

pub(crate) fn register_domain_body(name: &str, retention: &str) -> Value {
    json!({ "name": name, "workflowExecutionRetentionPeriodInDays": retention })
}

/// Register a domain, a workflow type carrying `defaults`, and nothing else.
pub(crate) async fn seeded(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_type: (&str, &str),
    defaults: Value,
) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
    let mut body = json!({
        "domain": domain,
        "name": workflow_type.0,
        "version": workflow_type.1,
    });
    merge(&mut body, defaults);
    emulator.ok(case, "RegisterWorkflowType", &body).await;
}

/// Fold `extra`'s members into `body`.
pub(crate) fn merge(body: &mut Value, extra: Value) {
    let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) else {
        panic!("merge expects two JSON objects, got {body:?} and {extra:?}");
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

/// Every default a workflow type can carry, so a start needs no overrides at all.
pub(crate) fn full_defaults(task_list: &str, child_policy: &str) -> Value {
    json!({
        "defaultTaskList": { "name": task_list },
        "defaultTaskStartToCloseTimeout": "60",
        "defaultExecutionStartToCloseTimeout": "300",
        "defaultChildPolicy": child_policy,
    })
}

// ---------------------------------------------------------------------------
// RegisterDomain / DescribeDomain: the stored domain record
// ---------------------------------------------------------------------------

/// One domain registration, varying the value, the optional description and the tag
/// count — what the "answer from the stored record" rule generalises over here.
struct DomainCase {
    name: &'static str,
    retention: &'static str,
    description: Option<&'static str>,
    tags: usize,
}

#[tokio::test]
async fn register_domain_is_read_back_from_the_stored_record() {
    // `workflowExecutionRetentionPeriodInDays` accepts "NONE" or 0..=90 days (the
    // member's own doc: "If you pass the value NONE or 0 (zero) ... The maximum
    // workflow execution retention period is 90 days"), so the rows below span both
    // published bounds and the sentinel, none of which the issue cited beyond "1".
    let long_name: String = "d".repeat(256);
    let cases = [
        DomainCase {
            name: "demo-domain",
            retention: "1",
            description: None,
            tags: 0,
        },
        DomainCase {
            name: "retention-sentinel",
            retention: "NONE",
            description: Some("history is not retained at all"),
            tags: 1,
        },
        DomainCase {
            name: "retention-low-bound",
            retention: "0",
            description: None,
            tags: 7,
        },
        DomainCase {
            name: "retention-high-bound",
            retention: "90",
            description: Some("ninety days"),
            tags: 50,
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let mut body = register_domain_body(case.name, case.retention);
        if let Some(description) = case.description {
            merge(&mut body, json!({ "description": description }));
        }
        let tags: Vec<Value> = (0..case.tags)
            .map(|index| json!({ "key": format!("k{index}"), "value": format!("v{index}") }))
            .collect();
        if !tags.is_empty() {
            merge(&mut body, json!({ "tags": tags }));
        }
        let registered = emulator.ok(case.name, "RegisterDomain", &body).await;
        assert_eq!(
            registered,
            json!({}),
            "case {:?}: RegisterDomain's output shape is Unit, so the body must be empty, got {registered:?}",
            case.name
        );

        let described = emulator
            .ok(
                case.name,
                "DescribeDomain",
                &json!({ "name": case.name }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/domainInfo/name"),
            case.name,
            "case {:?}: DescribeDomain must report the stored name, got {described:?}",
            case.name
        );
        assert_eq!(
            string_at(&described, "/domainInfo/status"),
            "REGISTERED",
            "case {:?}: a freshly registered domain is REGISTERED, got {described:?}",
            case.name
        );
        assert_eq!(
            string_at(&described, "/domainInfo/arn"),
            format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/{}", case.name),
            "case {:?}: the stored ARN must carry this request's region and the emulator \
             account, got {described:?}",
            case.name
        );
        assert_eq!(
            described
                .pointer("/domainInfo/description")
                .and_then(Value::as_str),
            case.description,
            "case {:?}: DescribeDomain must report the stored description, got {described:?}",
            case.name
        );
        assert_eq!(
            string_at(
                &described,
                "/configuration/workflowExecutionRetentionPeriodInDays"
            ),
            case.retention,
            "case {:?}: DescribeDomain must report the stored retention period, got {described:?}",
            case.name
        );
    }

    // A long name is still read back by name, so nothing about the key is truncated.
    emulator
        .ok(
            "256-char name",
            "RegisterDomain",
            &register_domain_body(&long_name, "7"),
        )
        .await;
    let described = emulator
        .ok(
            "256-char name",
            "DescribeDomain",
            &json!({ "name": long_name }),
        )
        .await;
    assert_eq!(
        string_at(&described, "/domainInfo/name"),
        long_name,
        "case \"256-char name\": the stored name must round-trip whole, got {described:?}"
    );

    // Re-registering any of them is a conflict, never a silent overwrite.
    for case in &cases {
        emulator
            .refused(
                case.name,
                "RegisterDomain",
                &register_domain_body(case.name, "7"),
                "DomainAlreadyExistsFault",
            )
            .await;
    }
}

#[tokio::test]
async fn retention_period_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();
    // Each published bound is accepted above; these are the values past/outside it.
    // `+5` is in range numerically but is not the plain decimal form SWF documents
    // ("an integer greater than or equal to 0") and stores verbatim.
    for (index, retention) in ["91", "365", "-1", "none", "", "7d", "+5"]
        .iter()
        .enumerate()
    {
        let name = format!("bad-retention-{index}");
        emulator
            .refused(
                retention,
                "RegisterDomain",
                &register_domain_body(&name, retention),
                // "" is shorter than the modelled DurationInDays minimum of 1, so the
                // boundary validator refuses it with the same code the handler uses.
                "ValidationException",
            )
            .await;
        emulator
            .refused(
                retention,
                "DescribeDomain",
                &json!({ "name": name }),
                "UnknownResourceFault",
            )
            .await;
    }
}

#[tokio::test]
async fn a_duration_that_is_not_the_documented_form_is_refused() {
    // The same plain-decimal rule on every duration member: task start-to-close also
    // accepts the documented "NONE", execution start-to-close does not and is capped
    // at one year (31536000 seconds).
    let emulator = Emulator::new();
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &register_domain_body("durations", "7"),
        )
        .await;

    let cases: [(&str, &str, Value); 6] = [
        ("task", "defaultTaskStartToCloseTimeout", json!("+60")),
        ("task", "defaultTaskStartToCloseTimeout", json!("none")),
        ("task", "defaultTaskStartToCloseTimeout", json!("60s")),
        (
            "execution",
            "defaultExecutionStartToCloseTimeout",
            json!("+300"),
        ),
        // "NONE" is explicitly not accepted for the execution timeout.
        (
            "execution",
            "defaultExecutionStartToCloseTimeout",
            json!("NONE"),
        ),
        // One past the documented one-year maximum.
        (
            "execution",
            "defaultExecutionStartToCloseTimeout",
            json!("31536001"),
        ),
    ];

    for (index, (kind, member, value)) in cases.iter().enumerate() {
        let case = format!("{kind} {value}");
        let mut body = json!({
            "domain": "durations",
            "name": format!("wf-{index}"),
            "version": "1.0",
        });
        merge(&mut body, json!({ (*member): value }));
        emulator
            .refused(&case, "RegisterWorkflowType", &body, "ValidationException")
            .await;
    }

    // The documented forms on each member are accepted, including the published
    // maximum, the task timeout's "NONE" and a zero-padded integer.
    for (index, (task, execution)) in [("NONE", "0"), ("0", "31536000"), ("0060", "0300")]
        .iter()
        .enumerate()
    {
        let case = format!("accepted {task}/{execution}");
        emulator
            .ok(
                &case,
                "RegisterWorkflowType",
                &json!({
                    "domain": "durations",
                    "name": format!("ok-{index}"),
                    "version": "1.0",
                    "defaultTaskStartToCloseTimeout": task,
                    "defaultExecutionStartToCloseTimeout": execution,
                }),
            )
            .await;
    }

    // And the same rule holds on the start request's own override members.
    emulator
        .ok(
            "start seed",
            "RegisterWorkflowType",
            &json!({
                "domain": "durations",
                "name": "start-wf",
                "version": "1.0",
                "defaultTaskList": { "name": "tasks" },
                "defaultChildPolicy": "TERMINATE",
            }),
        )
        .await;
    for (member, value) in [
        ("taskStartToCloseTimeout", "+60"),
        ("executionStartToCloseTimeout", "300s"),
        ("executionStartToCloseTimeout", "NONE"),
    ] {
        let case = format!("start {member}={value}");
        let mut body = json!({
            "domain": "durations",
            "workflowId": "wf-1",
            "workflowType": { "name": "start-wf", "version": "1.0" },
            "taskStartToCloseTimeout": "60",
            "executionStartToCloseTimeout": "300",
        });
        merge(&mut body, json!({ member: value }));
        emulator
            .refused(
                &case,
                "StartWorkflowExecution",
                &body,
                "ValidationException",
            )
            .await;
    }
}

#[tokio::test]
async fn more_tags_than_a_domain_allows_is_refused() {
    let emulator = Emulator::new();
    let tags: Vec<Value> = (0..51)
        .map(|index| json!({ "key": format!("k{index}") }))
        .collect();
    let mut body = register_domain_body("too-many-tags", "7");
    merge(&mut body, json!({ "tags": tags }));
    emulator
        .refused("51 tags", "RegisterDomain", &body, "TooManyTagsFault")
        .await;
    emulator
        .refused(
            "51 tags",
            "DescribeDomain",
            &json!({ "name": "too-many-tags" }),
            "UnknownResourceFault",
        )
        .await;
}

#[tokio::test]
async fn a_tag_outside_the_documented_character_set_is_refused() {
    // Source: the `ResourceTag` shape doc — "Tags may only contain unicode letters,
    // digits, whitespace, or these symbols: _ . : / = + - @". Each documented symbol
    // is exercised, and a character outside the set is refused on BOTH sides of the
    // pair, since the rule is stated for the tag and not for one member of it.
    let emulator = Emulator::new();

    let accepted = json!([
        { "key": "team", "value": "platform engineering" },
        { "key": "cost_center.1", "value": "a:b/c=d+e-f@g" },
        { "key": "Ünïcødé", "value": "Ünïcødé 42" },
    ]);
    let mut body = register_domain_body("tag-charset", "7");
    merge(&mut body, json!({ "tags": accepted }));
    emulator
        .ok("documented symbols", "RegisterDomain", &body)
        .await;
    emulator
        .ok(
            "documented symbols",
            "DescribeDomain",
            &json!({ "name": "tag-charset" }),
        )
        .await;

    for (index, (key, value)) in [
        ("bad!key", None),
        ("bad#key", None),
        ("bad,key", None),
        ("ok", Some("bad!value")),
        ("ok", Some("bad\u{1}value")),
    ]
    .iter()
    .enumerate()
    {
        let case = format!("{key:?}/{value:?}");
        let name = format!("bad-tag-{index}");
        let mut tag = json!({ "key": key });
        if let Some(value) = value {
            merge(&mut tag, json!({ "value": value }));
        }
        let mut body = register_domain_body(&name, "7");
        merge(&mut body, json!({ "tags": [tag] }));
        emulator
            .refused(&case, "RegisterDomain", &body, "ValidationException")
            .await;
        // The refusal persisted nothing.
        emulator
            .refused(
                &case,
                "DescribeDomain",
                &json!({ "name": name }),
                "UnknownResourceFault",
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// The SWF naming rule, wherever a caller supplies a name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn names_outside_the_swf_naming_rule_are_refused_on_every_member() {
    // Source: the `name` member doc shared by RegisterDomain / RegisterWorkflowType /
    // StartWorkflowExecution — "The specified string must not start or end with
    // whitespace. It must not contain a : (colon), / (slash), | (vertical bar), or any
    // control characters (\u0000-\u001f | \u007f-\u009f). Also, it must not be the
    // literal string arn." The issue cited none of these; the rows below include the
    // vertical bar and both control-character bands.
    let rejected = [
        "colon:name",
        "slash/name",
        "bar|name",
        "tab\tname",
        "del\u{7f}name",
        "c1\u{9f}name",
        " leading",
        "trailing ",
        "arn",
    ];
    // Accepted: the rule bans those characters and that one literal, nothing more.
    let accepted = ["plain.name_1-2", "ARN", "arn-prefixed", "has space inside"];

    let emulator = Emulator::new();
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &register_domain_body("naming-rule", "7"),
        )
        .await;

    for name in rejected {
        emulator
            .refused(
                name,
                "RegisterDomain",
                &register_domain_body(name, "7"),
                "ValidationException",
            )
            .await;
        emulator
            .refused(
                name,
                "RegisterWorkflowType",
                &json!({ "domain": "naming-rule", "name": name, "version": "1.0" }),
                "ValidationException",
            )
            .await;
        emulator
            .refused(
                name,
                "RegisterWorkflowType",
                &json!({ "domain": "naming-rule", "name": "wf", "version": name }),
                "ValidationException",
            )
            .await;
        emulator
            .refused(
                name,
                "StartWorkflowExecution",
                &json!({
                    "domain": "naming-rule",
                    "workflowId": name,
                    "workflowType": { "name": "wf", "version": "1.0" },
                }),
                "ValidationException",
            )
            .await;
        emulator
            .refused(
                name,
                "TerminateWorkflowExecution",
                &json!({ "domain": "naming-rule", "workflowId": name }),
                "ValidationException",
            )
            .await;
    }

    for name in accepted {
        emulator
            .ok("accepted", "RegisterDomain", &register_domain_body(name, "7"))
            .await;
        emulator
            .ok(
                "accepted",
                "RegisterWorkflowType",
                &json!({ "domain": "naming-rule", "name": name, "version": name }),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// RegisterWorkflowType / DescribeWorkflowType: the stored type record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_workflow_type_is_read_back_from_the_stored_record() {
    let emulator = Emulator::new();
    emulator
        .ok("seed", "RegisterDomain", &register_domain_body("types", "7"))
        .await;

    // Rows vary WHICH registration-time defaults are stored: none, one, all — and the
    // child policy walks the generated `ChildPolicy` catalogue, so every published
    // value is exercised, not only the TERMINATE the issue cited.
    let mut cases: Vec<(String, Value)> = vec![
        ("no-defaults".to_string(), json!({})),
        (
            "task-list-only".to_string(),
            json!({ "defaultTaskList": { "name": "only-tasks" } }),
        ),
        (
            "every-default".to_string(),
            json!({
                "description": "all defaults",
                "defaultTaskList": { "name": "all-tasks" },
                "defaultTaskStartToCloseTimeout": "NONE",
                "defaultExecutionStartToCloseTimeout": "31536000",
                "defaultTaskPriority": "10",
                "defaultChildPolicy": "ABANDON",
                "defaultLambdaRole": format!("arn:aws:iam::{ACCOUNT}:role/swf-lambda"),
            }),
        ),
    ];
    for policy in CHILD_POLICY_VALUES {
        cases.push((
            format!("policy-{policy}"),
            json!({
                "defaultTaskList": { "name": "policy-tasks" },
                "defaultChildPolicy": policy,
            }),
        ));
    }

    for (name, defaults) in &cases {
        let mut body = json!({ "domain": "types", "name": name, "version": "1.0" });
        merge(&mut body, defaults.clone());
        let registered = emulator.ok(name, "RegisterWorkflowType", &body).await;
        assert_eq!(
            registered,
            json!({}),
            "case {name:?}: RegisterWorkflowType's output shape is Unit, got {registered:?}"
        );

        let described = emulator
            .ok(
                name,
                "DescribeWorkflowType",
                &json!({
                    "domain": "types",
                    "workflowType": { "name": name, "version": "1.0" },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/typeInfo/workflowType/name"),
            name.as_str(),
            "case {name:?}: DescribeWorkflowType must report the stored identity, got {described:?}"
        );
        assert_eq!(
            string_at(&described, "/typeInfo/status"),
            "REGISTERED",
            "case {name:?}: a freshly registered type is REGISTERED, got {described:?}"
        );
        assert!(
            described
                .pointer("/typeInfo/creationDate")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {name:?}: the stored creationDate must be reported, got {described:?}"
        );
        // Exactly the defaults that were registered come back, and no others.
        for member in [
            "defaultTaskList",
            "defaultTaskStartToCloseTimeout",
            "defaultExecutionStartToCloseTimeout",
            "defaultTaskPriority",
            "defaultChildPolicy",
            "defaultLambdaRole",
        ] {
            let expected = defaults.get(member);
            let actual = described.pointer(&format!("/configuration/{member}"));
            assert_eq!(
                actual, expected,
                "case {name:?}: configuration.{member} must mirror what registration stored, \
                 got {actual:?}"
            );
        }

        // A second registration of the same (name, version) is a conflict.
        emulator
            .refused(name, "RegisterWorkflowType", &body, "TypeAlreadyExistsFault")
            .await;
    }

    // The same name at a DIFFERENT version is a different type, not a conflict.
    emulator
        .ok(
            "second version",
            "RegisterWorkflowType",
            &json!({ "domain": "types", "name": "no-defaults", "version": "2.0" }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// StartWorkflowExecution / DescribeWorkflowExecution: the resolved configuration
// ---------------------------------------------------------------------------

/// One start, varying which values the TYPE registers and which the REQUEST overrides.
struct StartCase {
    name: &'static str,
    defaults: Value,
    overrides: Value,
    expected_config: Value,
}

#[tokio::test]
async fn start_resolves_its_configuration_once_and_describe_reports_the_stored_run() {
    let cases = [
        StartCase {
            name: "all from the type defaults",
            defaults: json!({
                "defaultTaskList": { "name": "demo-tasks" },
                "defaultTaskStartToCloseTimeout": "60",
                "defaultExecutionStartToCloseTimeout": "300",
                "defaultChildPolicy": "TERMINATE",
            }),
            overrides: json!({}),
            expected_config: json!({
                "taskList": { "name": "demo-tasks" },
                "taskStartToCloseTimeout": "60",
                "executionStartToCloseTimeout": "300",
                "childPolicy": "TERMINATE",
            }),
        },
        StartCase {
            name: "every default overridden by the request",
            defaults: json!({
                "defaultTaskList": { "name": "demo-tasks" },
                "defaultTaskStartToCloseTimeout": "60",
                "defaultExecutionStartToCloseTimeout": "300",
                "defaultChildPolicy": "TERMINATE",
                "defaultTaskPriority": "1",
                "defaultLambdaRole": format!("arn:aws:iam::{ACCOUNT}:role/from-type"),
            }),
            overrides: json!({
                "taskList": { "name": "request-tasks" },
                "taskStartToCloseTimeout": "NONE",
                "executionStartToCloseTimeout": "1200",
                "childPolicy": "ABANDON",
                "taskPriority": "-5",
                "lambdaRole": format!("arn:aws:iam::{ACCOUNT}:role/from-request"),
            }),
            expected_config: json!({
                "taskList": { "name": "request-tasks" },
                "taskStartToCloseTimeout": "NONE",
                "executionStartToCloseTimeout": "1200",
                "childPolicy": "ABANDON",
                "taskPriority": "-5",
                "lambdaRole": format!("arn:aws:iam::{ACCOUNT}:role/from-request"),
            }),
        },
        StartCase {
            name: "a mix of request and type",
            defaults: json!({
                "defaultTaskList": { "name": "demo-tasks" },
                "defaultTaskStartToCloseTimeout": "60",
                "defaultExecutionStartToCloseTimeout": "300",
                "defaultChildPolicy": "TERMINATE",
                "defaultTaskPriority": "3",
            }),
            overrides: json!({
                "executionStartToCloseTimeout": "900",
                "childPolicy": "REQUEST_CANCEL",
            }),
            expected_config: json!({
                "taskList": { "name": "demo-tasks" },
                "taskStartToCloseTimeout": "60",
                "executionStartToCloseTimeout": "900",
                "childPolicy": "REQUEST_CANCEL",
                "taskPriority": "3",
            }),
        },
        StartCase {
            name: "nothing registered, everything on the request",
            defaults: json!({}),
            overrides: json!({
                "taskList": { "name": "only-request-tasks" },
                "taskStartToCloseTimeout": "0",
                "executionStartToCloseTimeout": "0",
                "childPolicy": "TERMINATE",
            }),
            expected_config: json!({
                "taskList": { "name": "only-request-tasks" },
                "taskStartToCloseTimeout": "0",
                "executionStartToCloseTimeout": "0",
                "childPolicy": "TERMINATE",
            }),
        },
    ];

    // Tag lists of different sizes, inside the modelled TagList length of 0..5.
    let tag_lists = [
        Vec::<&str>::new(),
        vec!["t1"],
        vec!["a", "b", "c", "d", "e"],
    ];

    let mut run_ids: BTreeSet<String> = BTreeSet::new();
    for (index, case) in cases.iter().enumerate() {
        for (tag_index, tags) in tag_lists.iter().enumerate() {
            let emulator = Emulator::new();
            let domain = format!("start-{index}-{tag_index}");
            seeded(
                &emulator,
                case.name,
                &domain,
                ("demo-wf", "1.0"),
                case.defaults.clone(),
            )
            .await;

            let payload = format!("{{\"n\":{index}}}");
            let mut body = json!({
                "domain": domain,
                "workflowId": "wf-1",
                "workflowType": { "name": "demo-wf", "version": "1.0" },
                "input": payload,
            });
            merge(&mut body, case.overrides.clone());
            if !tags.is_empty() {
                merge(&mut body, json!({ "tagList": tags }));
            }

            let started = emulator.ok(case.name, "StartWorkflowExecution", &body).await;
            let run_id = string_at(&started, "/runId");
            assert!(
                !run_id.is_empty() && run_id.len() <= 64,
                "case {:?}: runId must be a non-empty id inside WorkflowRunId's 1..64 length, \
                 got {run_id:?}",
                case.name
            );
            assert!(
                run_ids.insert(run_id.clone()),
                "case {:?}: every start must mint a fresh runId, but {run_id:?} repeated",
                case.name
            );

            let described = emulator
                .ok(
                    case.name,
                    "DescribeWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "execution": { "workflowId": "wf-1", "runId": run_id },
                    }),
                )
                .await;

            assert_eq!(
                string_at(&described, "/executionInfo/execution/runId"),
                run_id,
                "case {:?}: the minted runId must have been STORED, got {described:?}",
                case.name
            );
            assert_eq!(
                string_at(&described, "/executionInfo/executionStatus"),
                "OPEN",
                "case {:?}: a freshly started run is OPEN, got {described:?}",
                case.name
            );
            assert_eq!(
                described.pointer("/executionInfo/closeStatus"),
                None,
                "case {:?}: an OPEN run has no closeStatus, got {described:?}",
                case.name
            );
            assert_eq!(
                string_at(&described, "/executionInfo/workflowType/name"),
                "demo-wf",
                "case {:?}: the stored workflow type must be reported, got {described:?}",
                case.name
            );
            assert!(
                described
                    .pointer("/executionInfo/startTimestamp")
                    .and_then(Value::as_f64)
                    .is_some_and(|at| at > 0.0),
                "case {:?}: the stored startTimestamp must be reported, got {described:?}",
                case.name
            );
            let expected_tags = (!tags.is_empty()).then(|| json!(tags));
            assert_eq!(
                described.pointer("/executionInfo/tagList").cloned(),
                expected_tags,
                "case {:?}: the stored tagList must be reported, got {described:?}",
                case.name
            );
            assert_eq!(
                i64_at(&described, "/openCounts/openDecisionTasks"),
                1,
                "case {:?}: the DecisionTaskScheduled event leaves exactly one open \
                 decision task, got {described:?}",
                case.name
            );
            for counter in [
                "openActivityTasks",
                "openTimers",
                "openChildWorkflowExecutions",
                "openLambdaFunctions",
            ] {
                assert_eq!(
                    i64_at(&described, &format!("/openCounts/{counter}")),
                    0,
                    "case {:?}: a run that only started has no {counter}, got {described:?}",
                    case.name
                );
            }

            let configuration = described
                .pointer("/executionConfiguration")
                .unwrap_or_else(|| panic!("case {:?}: no executionConfiguration", case.name));
            assert_eq!(
                configuration,
                &case.expected_config,
                "case {:?}: executionConfiguration must be the configuration resolved at \
                 start (request value winning over the type default), got {configuration:?}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_default_defined_neither_on_the_request_nor_the_type_is_refused() {
    // One row per member that must be defined somewhere: the type registers every
    // default EXCEPT the named one, and the request supplies none of them.
    let members = [
        ("taskList", "defaultTaskList"),
        ("taskStartToCloseTimeout", "defaultTaskStartToCloseTimeout"),
        (
            "executionStartToCloseTimeout",
            "defaultExecutionStartToCloseTimeout",
        ),
        ("childPolicy", "defaultChildPolicy"),
    ];
    let every_default = json!({
        "defaultTaskList": { "name": "demo-tasks" },
        "defaultTaskStartToCloseTimeout": "60",
        "defaultExecutionStartToCloseTimeout": "300",
        "defaultChildPolicy": "TERMINATE",
    });
    let request_values = json!({
        "taskList": { "name": "request-tasks" },
        "taskStartToCloseTimeout": "60",
        "executionStartToCloseTimeout": "300",
        "childPolicy": "TERMINATE",
    });

    for (member, default_member) in members {
        let emulator = Emulator::new();
        let mut defaults = every_default.clone();
        defaults
            .as_object_mut()
            .expect("an object")
            .remove(default_member);
        seeded(&emulator, member, "undefined", ("demo-wf", "1.0"), defaults).await;

        let start = json!({
            "domain": "undefined",
            "workflowId": "wf-1",
            "workflowType": { "name": "demo-wf", "version": "1.0" },
        });
        emulator
            .refused(
                member,
                "StartWorkflowExecution",
                &start,
                "DefaultUndefinedFault",
            )
            .await;

        // Supplying it on the request is what makes the same start succeed.
        let mut with_value = start.clone();
        merge(
            &mut with_value,
            json!({ member: request_values.get(member).expect("a request value") }),
        );
        let started = emulator
            .ok(member, "StartWorkflowExecution", &with_value)
            .await;
        assert!(
            !string_at(&started, "/runId").is_empty(),
            "case {member:?}: the start must succeed once the value is on the request, \
             got {started:?}"
        );
    }
}

#[tokio::test]
async fn one_open_run_per_workflow_id_and_a_new_run_after_it_closes() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "already started",
        "reuse",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let start = json!({
        "domain": "reuse",
        "workflowId": "wf-1",
        "workflowType": { "name": "demo-wf", "version": "1.0" },
    });

    let first = string_at(
        &emulator
            .ok("already started", "StartWorkflowExecution", &start)
            .await,
        "/runId",
    );
    emulator
        .refused(
            "already started",
            "StartWorkflowExecution",
            &start,
            "WorkflowExecutionAlreadyStartedFault",
        )
        .await;

    emulator
        .ok(
            "already started",
            "TerminateWorkflowExecution",
            &json!({ "domain": "reuse", "workflowId": "wf-1" }),
        )
        .await;

    let second = string_at(
        &emulator
            .ok("already started", "StartWorkflowExecution", &start)
            .await,
        "/runId",
    );
    assert_ne!(
        first, second,
        "case \"already started\": re-starting a closed workflowId must mint a new runId, \
         got {second:?} twice"
    );

    // Both runs are still addressable, each with its own status.
    for (run_id, expected_status) in [(&first, "CLOSED"), (&second, "OPEN")] {
        let described = emulator
            .ok(
                "already started",
                "DescribeWorkflowExecution",
                &json!({
                    "domain": "reuse",
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            expected_status,
            "case {run_id:?}: each stored run keeps its own status, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetWorkflowExecutionHistory: the persisted event log
// ---------------------------------------------------------------------------

/// Read a whole history in one call.
pub(crate) async fn whole_history(
    emulator: &Emulator,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> Vec<Value> {
    let body = emulator
        .ok(
            "whole history",
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await;
    assert_eq!(
        body.pointer("/nextPageToken"),
        None,
        "case \"whole history\": an unpaged read returns no continuation token, got {body:?}"
    );
    body.pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case \"whole history\": no events in {body:?}"))
}

#[tokio::test]
async fn history_records_the_seeded_and_appended_events() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "history",
        "history",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "history",
                "StartWorkflowExecution",
                &json!({
                    "domain": "history",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                    "input": "{\"n\":1}",
                    "tagList": ["t1"],
                }),
            )
            .await,
        "/runId",
    );

    let events = whole_history(&emulator, "history", "wf-1", &run_id).await;
    assert_eq!(
        events.len(),
        2,
        "case \"history\": a start seeds exactly WorkflowExecutionStarted and \
         DecisionTaskScheduled, got {events:?}"
    );
    assert_eq!(
        (
            string_at(&events[0], "/eventType"),
            i64_at(&events[0], "/eventId")
        ),
        ("WorkflowExecutionStarted".to_string(), 1),
        "case \"history\": the first event is WorkflowExecutionStarted with eventId 1, \
         got {:?}",
        events[0]
    );
    let started = events[0]
        .pointer("/workflowExecutionStartedEventAttributes")
        .unwrap_or_else(|| panic!("case \"history\": no start attributes in {:?}", events[0]));
    assert_eq!(
        string_at(started, "/input"),
        "{\"n\":1}",
        "case \"history\": the start event carries the stored input, got {started:?}"
    );
    assert_eq!(
        string_at(started, "/taskList/name"),
        "demo-tasks",
        "case \"history\": the start event carries the resolved task list, got {started:?}"
    );
    assert_eq!(
        string_at(started, "/workflowType/version"),
        "1.0",
        "case \"history\": the start event carries the workflow type, got {started:?}"
    );
    assert_eq!(
        started.pointer("/tagList").cloned(),
        Some(json!(["t1"])),
        "case \"history\": the start event carries the stored tag list, got {started:?}"
    );
    assert_eq!(
        (
            string_at(&events[1], "/eventType"),
            i64_at(&events[1], "/eventId")
        ),
        ("DecisionTaskScheduled".to_string(), 2),
        "case \"history\": the second event is DecisionTaskScheduled with eventId 2, got {:?}",
        events[1]
    );
    assert_eq!(
        string_at(&events[1], "/decisionTaskScheduledEventAttributes/taskList/name"),
        "demo-tasks",
        "case \"history\": the scheduled event carries the resolved task list, got {:?}",
        events[1]
    );
    // One event carries exactly ONE attributes member.
    for event in &events {
        let attribute_members = event
            .as_object()
            .expect("an object")
            .keys()
            .filter(|key| key.ends_with("EventAttributes"))
            .count();
        assert_eq!(
            attribute_members, 1,
            "case \"history\": an event carries exactly one *EventAttributes member, \
             got {event:?}"
        );
    }

    emulator
        .ok(
            "history",
            "TerminateWorkflowExecution",
            &json!({
                "domain": "history",
                "workflowId": "wf-1",
                "runId": run_id,
                "reason": "done",
                "details": "bye",
            }),
        )
        .await;
    let events = whole_history(&emulator, "history", "wf-1", &run_id).await;
    assert_eq!(
        (
            events.len(),
            string_at(&events[2], "/eventType"),
            i64_at(&events[2], "/eventId")
        ),
        (3, "WorkflowExecutionTerminated".to_string(), 3),
        "case \"history\": Terminate appends WorkflowExecutionTerminated as eventId 3, \
         got {events:?}"
    );
    let terminated = events[2]
        .pointer("/workflowExecutionTerminatedEventAttributes")
        .unwrap_or_else(|| panic!("case \"history\": no terminate attributes in {:?}", events[2]));
    assert_eq!(
        (
            string_at(terminated, "/reason"),
            string_at(terminated, "/details"),
            string_at(terminated, "/childPolicy")
        ),
        (
            "done".to_string(),
            "bye".to_string(),
            "TERMINATE".to_string()
        ),
        "case \"history\": the terminate event carries reason, details and the effective \
         child policy, got {terminated:?}"
    );
    assert_eq!(
        terminated.pointer("/cause").and_then(Value::as_str),
        Some("OPERATOR_INITIATED"),
        "case \"history\": `cause` names what terminated the run, which for an operator's \
         own TerminateWorkflowExecution call is OPERATOR_INITIATED, got {terminated:?}"
    );
}

#[tokio::test]
async fn paging_a_history_visits_every_event_exactly_once_in_either_order() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "paging",
        "paging",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "paging",
                "StartWorkflowExecution",
                &json!({
                    "domain": "paging",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );
    emulator
        .ok(
            "paging",
            "TerminateWorkflowExecution",
            &json!({ "domain": "paging", "workflowId": "wf-1", "runId": run_id }),
        )
        .await;

    let forward: Vec<i64> = whole_history(&emulator, "paging", "wf-1", &run_id)
        .await
        .iter()
        .map(|event| i64_at(event, "/eventId"))
        .collect();
    assert_eq!(
        forward,
        vec![1, 2, 3],
        "case \"paging\": the stored log reads oldest-first with strictly increasing \
         eventIds, got {forward:?}"
    );

    // The paging invariant: for EVERY page size and both orderings, the concatenated
    // pages are exactly the full ordering — no repeats, no loss. The input space here
    // (page size x ordering) is small enough to enumerate exhaustively, which is
    // stronger than sampling it.
    for reverse_order in [false, true] {
        let expected: Vec<i64> = if reverse_order {
            forward.iter().rev().copied().collect()
        } else {
            forward.clone()
        };
        for page_size in 1..=(forward.len() + 1) {
            let case = format!("reverseOrder={reverse_order}, maximumPageSize={page_size}");
            let mut seen: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = json!({
                    "domain": "paging",
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                    "maximumPageSize": page_size,
                    "reverseOrder": reverse_order,
                });
                if let Some(token) = &token {
                    merge(&mut body, json!({ "nextPageToken": token }));
                }
                let answer = emulator
                    .ok(&case, "GetWorkflowExecutionHistory", &body)
                    .await;
                let events = answer
                    .pointer("/events")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("case {case:?}: no events in {answer:?}"));
                assert!(
                    events.len() <= page_size,
                    "case {case:?}: a page must not exceed maximumPageSize, got {answer:?}"
                );
                seen.extend(events.iter().map(|event| i64_at(event, "/eventId")));
                token = answer
                    .pointer("/nextPageToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                pages += 1;
                assert!(
                    pages <= forward.len() + 2,
                    "case {case:?}: paging must terminate, but it produced {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: the pages must concatenate to the whole ordering, got {seen:?}"
            );
        }
    }

    // A token minted for one ordering is refused against the other, rather than
    // silently addressing a different event.
    let first_page = emulator
        .ok(
            "cross-order token",
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": "paging",
                "execution": { "workflowId": "wf-1", "runId": run_id },
                "maximumPageSize": 1,
            }),
        )
        .await;
    let token = string_at(&first_page, "/nextPageToken");
    emulator
        .refused(
            "cross-order token",
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": "paging",
                "execution": { "workflowId": "wf-1", "runId": run_id },
                "maximumPageSize": 1,
                "reverseOrder": true,
                "nextPageToken": token,
            }),
            "ValidationException",
        )
        .await;

    // `maximumPageSize` is modelled with range 0..1000: both bounds read the whole
    // history (0 means "unspecified"), and one past the maximum is refused.
    for page_size in [0, 1000] {
        let case = format!("maximumPageSize={page_size}");
        let answer = emulator
            .ok(
                &case,
                "GetWorkflowExecutionHistory",
                &json!({
                    "domain": "paging",
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                    "maximumPageSize": page_size,
                }),
            )
            .await;
        let events = answer
            .pointer("/events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no events in {answer:?}"));
        assert_eq!(
            events.len(),
            forward.len(),
            "case {case:?}: a page size at a published bound reads the whole history, \
             got {answer:?}"
        );
    }
    let (status, body) = emulator
        .call(
            "GetWorkflowExecutionHistory",
            &json!({
                "domain": "paging",
                "execution": { "workflowId": "wf-1", "runId": run_id },
                "maximumPageSize": 1001,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"maximumPageSize=1001\": one past the modelled maximum is refused, \
         got {status} with {body:?}"
    );
}

#[tokio::test]
async fn a_page_token_names_an_event_so_an_append_between_pages_repeats_nothing() {
    // The token is a cursor on the event log, not an index into it, so a history that
    // GROWS between two pages — the other input this rule generalises over, alongside
    // page size and ordering — still never hands the same event back twice. Reverse
    // order is the demanding case: an appended event lands at the FRONT of it.
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "append mid-paging",
        "append",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "append mid-paging",
                "StartWorkflowExecution",
                &json!({
                    "domain": "append",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );

    for reverse_order in [false, true] {
        let case = format!("append mid-paging, reverseOrder={reverse_order}");
        let first = emulator
            .ok(
                &case,
                "GetWorkflowExecutionHistory",
                &json!({
                    "domain": "append",
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                    "maximumPageSize": 1,
                    "reverseOrder": reverse_order,
                }),
            )
            .await;
        let seen: Vec<i64> = first
            .pointer("/events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no events in {first:?}"))
            .iter()
            .map(|event| i64_at(event, "/eventId"))
            .collect();
        let token = string_at(&first, "/nextPageToken");

        // Terminate (on the first pass only) appends event 3 while the cursor is held.
        if !reverse_order {
            emulator
                .ok(
                    &case,
                    "TerminateWorkflowExecution",
                    &json!({ "domain": "append", "workflowId": "wf-1", "runId": run_id }),
                )
                .await;
        }

        let rest = emulator
            .ok(
                &case,
                "GetWorkflowExecutionHistory",
                &json!({
                    "domain": "append",
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                    "reverseOrder": reverse_order,
                    "nextPageToken": token,
                }),
            )
            .await;
        let resumed: Vec<i64> = rest
            .pointer("/events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no events in {rest:?}"))
            .iter()
            .map(|event| i64_at(event, "/eventId"))
            .collect();

        assert!(
            resumed.iter().all(|event_id| !seen.contains(event_id)),
            "case {case:?}: resuming must not repeat an event already paged \
             (first page {seen:?}, resumed {resumed:?})"
        );
        let monotone = resumed.windows(2).all(|pair| {
            if reverse_order {
                pair[0] > pair[1]
            } else {
                pair[0] < pair[1]
            }
        });
        assert!(
            monotone,
            "case {case:?}: the resumed page must follow the requested ordering, \
             got {resumed:?}"
        );
        assert!(
            !resumed.is_empty(),
            "case {case:?}: a token the service issued must address at least one \
             remaining event, got {resumed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// TerminateWorkflowExecution: closing the stored run
// ---------------------------------------------------------------------------
/// One terminate, varying the optional members and whether `runId` is passed.
struct TerminateCase {
    name: &'static str,
    pass_run_id: bool,
    extra: Value,
    expected_child_policy: &'static str,
}

#[tokio::test]
async fn terminate_closes_the_stored_run_and_is_not_repeatable() {
    let cases = [
        TerminateCase {
            name: "explicit runId, reason and details",
            pass_run_id: true,
            extra: json!({ "reason": "done", "details": "bye" }),
            expected_child_policy: "TERMINATE",
        },
        TerminateCase {
            name: "runId omitted, resolved from the open run",
            pass_run_id: false,
            extra: json!({}),
            expected_child_policy: "TERMINATE",
        },
        TerminateCase {
            name: "child policy overridden on the call",
            pass_run_id: true,
            extra: json!({ "childPolicy": "ABANDON", "reason": "operator" }),
            expected_child_policy: "ABANDON",
        },
        TerminateCase {
            name: "the third child policy",
            pass_run_id: false,
            extra: json!({ "childPolicy": "REQUEST_CANCEL" }),
            expected_child_policy: "REQUEST_CANCEL",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("terminate-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            ("demo-wf", "1.0"),
            full_defaults("demo-tasks", "TERMINATE"),
        )
        .await;
        let run_id = string_at(
            &emulator
                .ok(
                    case.name,
                    "StartWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": "wf-1",
                        "workflowType": { "name": "demo-wf", "version": "1.0" },
                    }),
                )
                .await,
            "/runId",
        );

        let mut body = json!({ "domain": domain, "workflowId": "wf-1" });
        if case.pass_run_id {
            merge(&mut body, json!({ "runId": run_id }));
        }
        merge(&mut body, case.extra.clone());
        let terminated = emulator
            .ok(case.name, "TerminateWorkflowExecution", &body)
            .await;
        assert_eq!(
            terminated,
            json!({}),
            "case {:?}: TerminateWorkflowExecution's output shape is Unit, got {terminated:?}",
            case.name
        );

        let described = emulator
            .ok(
                case.name,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": "wf-1", "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            (
                string_at(&described, "/executionInfo/executionStatus"),
                string_at(&described, "/executionInfo/closeStatus")
            ),
            ("CLOSED".to_string(), "TERMINATED".to_string()),
            "case {:?}: a terminated run is CLOSED/TERMINATED, got {described:?}",
            case.name
        );
        assert!(
            described
                .pointer("/executionInfo/closeTimestamp")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {:?}: a terminated run carries a closeTimestamp, got {described:?}",
            case.name
        );
        assert_eq!(
            i64_at(&described, "/openCounts/openDecisionTasks"),
            1,
            "case {:?}: terminating does not answer the decision task the run was started \
             with, so that task is still outstanding and still counted, got {described:?}",
            case.name
        );

        let events = whole_history(&emulator, &domain, "wf-1", &run_id).await;
        let last = events
            .last()
            .unwrap_or_else(|| panic!("case {:?}: an empty history", case.name));
        assert_eq!(
            string_at(
                last,
                "/workflowExecutionTerminatedEventAttributes/childPolicy"
            ),
            case.expected_child_policy,
            "case {:?}: the terminate event records the effective child policy \
             (the call's value, else the one resolved at start), got {last:?}",
            case.name
        );

        // A closed run can no longer be terminated, by either addressing form.
        for repeat in [
            json!({ "domain": domain, "workflowId": "wf-1", "runId": run_id }),
            json!({ "domain": domain, "workflowId": "wf-1" }),
        ] {
            emulator
                .refused(
                    case.name,
                    "TerminateWorkflowExecution",
                    &repeat,
                    "UnknownResourceFault",
                )
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Unknown resources, on every reader and writer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_domain_type_or_run_is_an_unknown_resource() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "unknown",
        "known",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "unknown",
                "StartWorkflowExecution",
                &json!({
                    "domain": "known",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );

    let unknown_execution = json!({ "workflowId": "wf-1", "runId": "00000000-0000-4000-8000-000000000000" });
    let cases: Vec<(&str, &str, Value)> = vec![
        ("unknown domain", "DescribeDomain", json!({ "name": "nope" })),
        (
            "unknown domain",
            "RegisterWorkflowType",
            json!({ "domain": "nope", "name": "demo-wf", "version": "1.0" }),
        ),
        (
            "unknown domain",
            "DescribeWorkflowType",
            json!({ "domain": "nope", "workflowType": { "name": "demo-wf", "version": "1.0" } }),
        ),
        (
            "unknown type",
            "DescribeWorkflowType",
            json!({ "domain": "known", "workflowType": { "name": "demo-wf", "version": "9.9" } }),
        ),
        (
            "unknown domain",
            "StartWorkflowExecution",
            json!({
                "domain": "nope",
                "workflowId": "wf-2",
                "workflowType": { "name": "demo-wf", "version": "1.0" },
            }),
        ),
        (
            "unregistered type",
            "StartWorkflowExecution",
            json!({
                "domain": "known",
                "workflowId": "wf-2",
                "workflowType": { "name": "demo-wf", "version": "9.9" },
            }),
        ),
        (
            "unknown runId",
            "DescribeWorkflowExecution",
            json!({ "domain": "known", "execution": unknown_execution }),
        ),
        (
            "unknown workflowId",
            "DescribeWorkflowExecution",
            json!({ "domain": "known", "execution": { "workflowId": "wf-9", "runId": run_id } }),
        ),
        (
            "unknown domain",
            "DescribeWorkflowExecution",
            json!({ "domain": "nope", "execution": { "workflowId": "wf-1", "runId": run_id } }),
        ),
        (
            "unknown runId",
            "GetWorkflowExecutionHistory",
            json!({ "domain": "known", "execution": unknown_execution }),
        ),
        (
            "unknown domain",
            "GetWorkflowExecutionHistory",
            json!({ "domain": "nope", "execution": { "workflowId": "wf-1", "runId": run_id } }),
        ),
        (
            "unknown workflowId",
            "TerminateWorkflowExecution",
            json!({ "domain": "known", "workflowId": "wf-9" }),
        ),
        (
            "unknown runId",
            "TerminateWorkflowExecution",
            json!({
                "domain": "known",
                "workflowId": "wf-1",
                "runId": "00000000-0000-4000-8000-000000000000",
            }),
        ),
        (
            "unknown domain",
            "TerminateWorkflowExecution",
            json!({ "domain": "nope", "workflowId": "wf-1" }),
        ),
    ];

    for (case, operation, body) in cases {
        emulator
            .refused(case, operation, &body, "UnknownResourceFault")
            .await;
    }

    // The known run is untouched by every refusal above: a read never mutates.
    let described = emulator
        .ok(
            "unknown",
            "DescribeWorkflowExecution",
            &json!({
                "domain": "known",
                "execution": { "workflowId": "wf-1", "runId": run_id },
            }),
        )
        .await;
    assert_eq!(
        (
            string_at(&described, "/executionInfo/executionStatus"),
            i64_at(&described, "/openCounts/openDecisionTasks")
        ),
        ("OPEN".to_string(), 1),
        "case \"unknown\": refused calls must not have changed the stored run, \
         got {described:?}"
    );
}

#[tokio::test]
async fn reading_a_run_repeatedly_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "repeat reads",
        "reads",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "repeat reads",
                "StartWorkflowExecution",
                &json!({
                    "domain": "reads",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );

    let describe = json!({
        "domain": "reads",
        "execution": { "workflowId": "wf-1", "runId": run_id },
    });
    let first = emulator
        .ok("repeat reads", "DescribeWorkflowExecution", &describe)
        .await;
    for round in 1..4 {
        let again = emulator
            .ok("repeat reads", "DescribeWorkflowExecution", &describe)
            .await;
        assert_eq!(
            again, first,
            "case \"repeat reads\" round {round}: Describe must not move the run's status, \
             consume its decision task or bump a counter, got {again:?}"
        );
        let events = whole_history(&emulator, "reads", "wf-1", &run_id).await;
        assert_eq!(
            events.len(),
            2,
            "case \"repeat reads\" round {round}: reading the history must not append to it, \
             got {events:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The enum values this service emits all come from the model's own catalogues
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_emitted_enum_value_is_in_the_models_catalogue() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        "catalogue",
        "catalogue",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;
    let run_id = string_at(
        &emulator
            .ok(
                "catalogue",
                "StartWorkflowExecution",
                &json!({
                    "domain": "catalogue",
                    "workflowId": "wf-1",
                    "workflowType": { "name": "demo-wf", "version": "1.0" },
                }),
            )
            .await,
        "/runId",
    );
    emulator
        .ok(
            "catalogue",
            "TerminateWorkflowExecution",
            &json!({ "domain": "catalogue", "workflowId": "wf-1", "runId": run_id }),
        )
        .await;

    let domain = emulator
        .ok(
            "catalogue",
            "DescribeDomain",
            &json!({ "name": "catalogue" }),
        )
        .await;
    let workflow_type = emulator
        .ok(
            "catalogue",
            "DescribeWorkflowType",
            &json!({
                "domain": "catalogue",
                "workflowType": { "name": "demo-wf", "version": "1.0" },
            }),
        )
        .await;
    let described = emulator
        .ok(
            "catalogue",
            "DescribeWorkflowExecution",
            &json!({
                "domain": "catalogue",
                "execution": { "workflowId": "wf-1", "runId": run_id },
            }),
        )
        .await;

    for (member, value, catalogue) in [
        (
            "domainInfo.status",
            string_at(&domain, "/domainInfo/status"),
            REGISTRATION_STATUS_VALUES,
        ),
        (
            "typeInfo.status",
            string_at(&workflow_type, "/typeInfo/status"),
            REGISTRATION_STATUS_VALUES,
        ),
        (
            "executionInfo.executionStatus",
            string_at(&described, "/executionInfo/executionStatus"),
            EXECUTION_STATUS_VALUES,
        ),
        (
            "executionInfo.closeStatus",
            string_at(&described, "/executionInfo/closeStatus"),
            CLOSE_STATUS_VALUES,
        ),
        (
            "executionConfiguration.childPolicy",
            string_at(&described, "/executionConfiguration/childPolicy"),
            CHILD_POLICY_VALUES,
        ),
    ] {
        assert!(
            catalogue.contains(&value.as_str()),
            "case {member:?}: the emitted value must be one of {catalogue:?}, got {value:?}"
        );
    }

    for event in whole_history(&emulator, "catalogue", "wf-1", &run_id).await {
        let event_type = string_at(&event, "/eventType");
        assert!(
            EVENT_TYPE_VALUES.contains(&event_type.as_str()),
            "case \"eventType\": every recorded eventType must be in the model's EventType \
             catalogue, got {event_type:?}"
        );
    }
}
