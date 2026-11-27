// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a request that named a resource it does not hold, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is the
//! status, the `x-amzn-errortype` header a client resolves the error shape from, and the
//! `message` an SDK surfaces, not an internal function's return value.
//!
//! The rule under test: AWS IoT has no single not-found sentence. Each resource FAMILY
//! refuses a lookup miss with its own fixed text, whose variable parts come from the
//! request and from this account's state:
//!
//! * the REGISTRY families qualify the name with the account the call was made for —
//!   `<Family> <accountId>:<name> not found.` — with the family's own noun (`Thing`,
//!   `ThingGroup`, `Thing Type`, `BillingGroup`);
//! * the JOBS and COMMANDS planes word it `<Family> <id> cannot be found.`;
//! * the AUTHORIZER family words it `Authorizer <name> not found`;
//! * the BULK THING REGISTRATION task family words it with a sentence carrying NO part
//!   of the request at all;
//! * a `DescribeJobExecution` whose JOB does not exist names that missing job and not
//!   the thing, while one whose job exists but has no execution on the thing names the
//!   pair, because the two conditions are distinguishable;
//! * and an `indexName` that is no fleet index of this service is not a miss at all but
//!   a request the fleet-indexing plane will not accept: `InvalidRequestException`
//!   (400), whose shape name that plane's validation layer qualifies with the service's
//!   API doc namespace in `x-amzn-errortype`.
//!
//! Every operation that addresses ONE resource of a family refuses with that family's
//! sentence, so the table below spans what the rule generalises over — the family, the
//! operation (a read, a delete and an update of the same family), the method, and the
//! form and length of the identifier — rather than one recorded request.
//!
//! The second rule this suite pins is the OTHER refusal every operation of this service
//! shares, the one its validation layer answers before any handler runs: a request that
//! breaks a constraint the model declares is refused 400, named `InvalidRequestException`
//! qualified with the service's API doc namespace in `x-amzn-errortype` — the name belongs
//! to the SHAPE, so it is the same for every operation — and worded in AWS's constraint
//! envelope, one clause per broken constraint, where
//!
//! * a member the request did not carry has no value to quote, and the clause states none;
//! * a member outside the `@pattern` of a THING-REGISTRY NAME is stated WITH that name,
//!   single-quoted, and a member whose expression this service does not publish with the
//!   value that missed it with no value at all; the expression is quoted without the
//!   whole-value anchors the model wraps it in, unless what they enclose carries an anchor of
//!   its own. The child suite [`clause_value_classes`] pins that split over the name types,
//!   the bindings and the operations that rule covers, and the suite
//!   `tests/all/pattern_clause_expression.rs` the expression that keeps its anchors;
//! * a member outside either end of its `@length` is stated with no value either — the
//!   clause names the bound it broke, not the text that broke it — which the child suite
//!   [`length_constraint_clauses`] pins over the operations and bounds that rule covers,
//!   while a COLLECTION outside its `@length` is stated WITH the collection, which the
//!   child suite [`modelled_refusal_shape`] pins beside the error SHAPE each plane's
//!   operations model;
//! * a member outside either end of its `@range` is stated WITH the number that broke it,
//!   quoted beside the numeric bound — a bound states the interval a value had to lie in
//!   rather than the value — which the child suite [`range_constraint_clauses`] pins over
//!   the operations, the bindings and both ends of the bounds that rule covers;
//! * a value outside an enumerated set is quoted beside that set, whose members are listed
//!   in the order of the hash table the front door holds them in rather than in the model's
//!   declaration order.
//!
//! What a clause states for a `@pattern` on a member of another plane is pinned by the child
//! suite as well. The error SHAPE, which is what this suite's second table asserts, is the
//! same for all of them.
//!
//! Its tables span what the rule generalises over: five operations, four constraint classes
//! (an absent `@required` member, a `@pattern` on a URI label on two operations, a
//! `@pattern` on a body member and an enumerated set), members bound to the URI, to the
//! query string and to the body, and several rejected values per row.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    FLEET_INDEX_NAMES, THING_GROUP_INDEX_NAME, THING_INDEX_NAME,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `@length` class of that same constraint envelope: which half of a clause a bound
/// states, over the operations, the bindings and both ends of a bound that the rule covers.
/// The `#[path]` is what keeps it a child of THIS suite rather than a second suite beside
/// it: this crate's `tests/all.rs` declares the suites directly under `tests/all/`, and a
/// module of a `#[path]`-declared file resolves next to that file unless it says otherwise.
#[path = "not_found_wording/length_constraint_clauses.rs"]
mod length_constraint_clauses;

/// The error SHAPE that same envelope is named with, per OPERATION, and the COLLECTION half
/// of a clause: a child of THIS suite for the reason the one above is, and declared the same
/// way.
#[path = "not_found_wording/modelled_refusal_shape.rs"]
mod modelled_refusal_shape;

/// The `@pattern` class of that same envelope: which half of a clause it states — the
/// rejected name for a registry name, the expression alone for every other member — over the
/// name types, the bindings and the operations that rule covers. A child of THIS suite for
/// the same reason as the one above.
#[path = "not_found_wording/clause_value_classes.rs"]
mod clause_value_classes;

/// The `@range` class of that same envelope: a clause about a NUMERIC bound states the
/// bound and the number that broke it, over the operations, the bindings and both ends
/// of the bounds the rule covers. Declared the same way, and for the same reason, as the
/// length suite above.
#[path = "not_found_wording/range_constraint_clauses.rs"]
mod range_constraint_clauses;

/// The account id this emulator serves, which the registry families qualify a missing
/// name with (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// The region every request in this suite is signed for. None of these refusals is
/// regional, so no assertion depends on it.
const REGION: &str = "eu-west-3";

/// The wire error shape of a resource this service does not hold.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// `InvalidRequestException` as AWS IoT names it in `x-amzn-errortype`: the shape name, a
/// `:`, and the service's API doc namespace. The name belongs to the SHAPE, so a refusal
/// its validation layer words and one a handler answers both carry it.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The `x-amzn-errortype` header, verbatim — including any namespace a plane
    /// qualifies the shape name with.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The `message` the body carries, or `""` when it carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
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
            error_type,
            body,
        }
    }

}

/// One request that names a resource the store does not hold, and the refusal it must
/// answer.
#[derive(Debug)]
struct Case {
    /// What this row is, for the assertion messages.
    case: String,
    method: &'static str,
    path: String,
    /// A body for the methods that carry one; `None` for a read or a delete.
    body: Option<Value>,
    status: u16,
    error_type: &'static str,
    message: String,
}

/// Identifiers of the form `^[a-zA-Z0-9_-]+$` within 1..64 — the jobs and commands
/// planes' `JobId` / `CommandId` constraints — differing in length, case and separator.
fn plane_ids() -> Vec<String> {
    vec![
        "x".to_string(),
        "missing-job-id".to_string(),
        "Missing_Id_42".to_string(),
        "n".repeat(64),
    ]
}

/// Names of the form `^[a-zA-Z0-9:_-]+$` within 1..128 — the registry families'
/// `ThingName` / `ThingGroupName` / `ThingTypeName` constraints.
fn registry_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "missing-resource".to_string(),
        "Missing:Scoped_Name-9".to_string(),
        "r".repeat(128),
    ]
}

/// Names of the form `^[\w=,@-]+$` within 1..128 — the `AuthorizerName` constraint,
/// which admits punctuation the registry names do not.
fn authorizer_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "missing-authorizer".to_string(),
        "Missing_Authorizer=1,2@host".to_string(),
        "z".repeat(128),
    ]
}

/// Every row of the table: each family, over each operation of this service that
/// addresses one resource of it.
fn cases() -> Vec<Case> {
    let mut rows = Vec::new();

    for name in authorizer_names() {
        let message = format!("Authorizer {name} not found");
        // DescribeAuthorizer, DeleteAuthorizer and UpdateAuthorizer all address the one
        // record by name, so all three word the miss the same way.
        for (operation, method, body) in [
            ("DescribeAuthorizer", "GET", None),
            ("DeleteAuthorizer", "DELETE", None),
            ("UpdateAuthorizer", "PUT", Some(json!({}))),
        ] {
            rows.push(Case {
                case: format!("{operation} on authorizer {name:?}"),
                method,
                path: format!("/authorizer/{name}"),
                body,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }
    }

    for id in plane_ids() {
        let message = format!("Command {id} cannot be found.");
        for (operation, method, path, body) in [
            ("GetCommand", "GET", format!("/commands/{id}"), None),
            (
                "UpdateCommand",
                "PATCH",
                format!("/commands/{id}"),
                Some(json!({})),
            ),
        ] {
            rows.push(Case {
                case: format!("{operation} on command {id:?}"),
                method,
                path,
                body,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }

        let message = format!("Job {id} cannot be found.");
        for (operation, method, path) in [
            ("DescribeJob", "GET", format!("/jobs/{id}")),
            ("DeleteJob", "DELETE", format!("/jobs/{id}")),
            ("GetJobDocument", "GET", format!("/jobs/{id}/job-document")),
        ] {
            rows.push(Case {
                case: format!("{operation} on job {id:?}"),
                method,
                path,
                body: None,
                status: 404,
                error_type: NOT_FOUND,
                message: message.clone(),
            });
        }

        // A job execution of a job nobody created: the refusal is about the missing JOB,
        // and the thing in the URI is not part of it.
        rows.push(Case {
            case: format!("DescribeJobExecution on the missing job {id:?}"),
            method: "GET",
            path: format!("/things/some-device/jobs/{id}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: format!("Job execution cannot be found as the Job {id} does not exist."),
        });
    }

    for name in registry_names() {
        // The registry families, each with its own noun and all account-qualified.
        for (operation, method, path, noun) in [
            (
                "DescribeThing",
                "GET",
                format!("/things/{name}"),
                "Thing",
            ),
            (
                "DescribeThingGroup",
                "GET",
                format!("/thing-groups/{name}"),
                "ThingGroup",
            ),
            (
                "DescribeThingType",
                "GET",
                format!("/thing-types/{name}"),
                "Thing Type",
            ),
            (
                "DeprecateThingType",
                "POST",
                format!("/thing-types/{name}/deprecate"),
                "Thing Type",
            ),
            (
                "DescribeBillingGroup",
                "GET",
                format!("/billing-groups/{name}"),
                "BillingGroup",
            ),
        ] {
            rows.push(Case {
                case: format!("{operation} on {noun} {name:?}"),
                method,
                path,
                body: (method == "POST").then(|| json!({})),
                status: 404,
                error_type: NOT_FOUND,
                message: format!("{noun} {ACCOUNT_ID}:{name} not found."),
            });
        }

        // The billing-group family's WRITE states the same sentence as its read.
        // `UpdateBillingGroup$billingGroupProperties` is `@required`
        // (`smithy_query(query="operation",operation="UpdateBillingGroup")`), so this row
        // carries one — a miss is decided by the name, not by what the body asks for.
        rows.push(Case {
            case: format!("UpdateBillingGroup on BillingGroup {name:?}"),
            method: "PATCH",
            path: format!("/billing-groups/{name}"),
            body: Some(json!({ "billingGroupProperties": {} })),
            status: 404,
            error_type: NOT_FOUND,
            message: format!("BillingGroup {ACCOUNT_ID}:{name} not found."),
        });
    }

    // The bulk-registration task family: one fixed sentence, whatever was asked for.
    // `TaskId` is length 0..40 with no pattern, so these differ in form as well as
    // length.
    for task_id in [
        "t",
        "00000000-0000-0000-0000-000000000000",
        "task-nobody-started",
    ] {
        rows.push(Case {
            case: format!("DescribeThingRegistrationTask on task {task_id:?}"),
            method: "GET",
            path: format!("/thing-registration-tasks/{task_id}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: "Task doesn't exist".to_string(),
        });
    }

    // A name that is no fleet index: the fleet-indexing plane will not accept the
    // request, so this is the 400 and not a miss.
    for index_name in ["AWS_Thing", "aws_things", "my-index", "AWS_Things:1"] {
        rows.push(Case {
            case: format!("DescribeIndex on the unrecognised index {index_name:?}"),
            method: "GET",
            path: format!("/indices/{index_name}"),
            body: None,
            status: 400,
            error_type: QUALIFIED_INVALID_REQUEST,
            message: format!("Unrecognized indexName {index_name}"),
        });
    }

    // A name that IS one of the recognised indexes, while the section that drives it is
    // off: the account has no such index, which is the miss.
    for index_name in FLEET_INDEX_NAMES {
        rows.push(Case {
            case: format!("DescribeIndex on the disabled index {index_name:?}"),
            method: "GET",
            path: format!("/indices/{index_name}"),
            body: None,
            status: 404,
            error_type: NOT_FOUND,
            message: format!("Index {index_name} does not exist"),
        });
    }

    rows
}

/// Every family's refusal: the status, the error shape and the sentence, over every
/// operation of this service that addresses one resource of that family.
#[tokio::test]
async fn each_resource_family_words_a_miss_its_own_way() {
    for case in cases() {
        // A fresh emulator per row: a refusal must not depend on what another row left
        // behind, and nothing here creates the resource it asks about.
        let emulator = Emulator::new();
        let reply = emulator
            .call(case.method, &case.path, case.body.as_ref())
            .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected status {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(case.error_type),
            "case {:?}: expected x-amzn-errortype {:?}, got {:?}",
            case.case,
            case.error_type,
            reply.error_type
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {:?}: expected message {:?}, got {:?}",
            case.case,
            case.message,
            reply.message()
        );
    }
}

/// The bulk-registration family's refusal carries NO part of the request: the requested
/// task id is nowhere in the answer.
#[tokio::test]
async fn a_missing_registration_task_echoes_nothing_of_the_request() {
    let emulator = Emulator::new();
    for task_id in ["abc", "0123456789", "task-nobody-started"] {
        let reply = emulator
            .call("GET", &format!("/thing-registration-tasks/{task_id}"), None)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {task_id:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.message().contains(task_id),
            "case {task_id:?}: this family's refusal must echo no part of the request, \
             got {:?}",
            reply.message()
        );
    }
}

/// A `DescribeJobExecution` whose JOB exists but has no execution on the thing is the
/// OTHER condition, and is worded about the pair — so the two are distinguishable, which
/// is the whole point of resolving the job first.
#[tokio::test]
async fn a_missing_job_and_a_missing_execution_are_distinguishable() {
    let emulator = Emulator::new();
    let registered = emulator
        .call("POST", "/things/targeted", Some(&json!({})))
        .await;
    assert_eq!(
        registered.status, 200,
        "registering the targeted thing must answer 200, got {} {:?}",
        registered.status, registered.body
    );
    let created = emulator
        .call(
            "PUT",
            "/jobs/known-job",
            Some(&json!({
                "targets": [format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/targeted")],
                "document": "{}",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the job must answer 200, got {} {:?}",
        created.status, created.body
    );

    // (case, the job in the URI, the job exists)
    for (case, job_id, exists) in [
        ("a job nobody created", "no-such-job", false),
        ("a job that exists", "known-job", true),
    ] {
        let reply = emulator
            .call("GET", &format!("/things/untargeted/jobs/{job_id}"), None)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        let message = reply.message();
        if exists {
            assert_eq!(
                message,
                format!(
                    "Job execution for thing untargeted cannot be found within job {job_id}."
                ),
                "case {case:?}: a missing EXECUTION of an existing job names the pair, \
                 got {message:?}"
            );
        } else {
            assert_eq!(
                message,
                format!("Job execution cannot be found as the Job {job_id} does not exist."),
                "case {case:?}: a missing JOB is reported as the missing job, got \
                 {message:?}"
            );
        }
    }
}

/// An index this service DOES recognise, once the section that drives it is enabled, is
/// described rather than refused — so the 400 the table pins is about the NAME and not
/// about the fleet-indexing plane refusing everything.
#[tokio::test]
async fn a_recognised_index_is_served_once_it_is_enabled() {
    let emulator = Emulator::new();
    // Enabled through the configuration endpoint, so the index this reads back is one a
    // request created rather than a record the test seeded.
    for body in [
        json!({ "thingIndexingConfiguration": { "thingIndexingMode": "REGISTRY" } }),
        json!({ "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": "ON" } }),
    ] {
        let updated = emulator.call("POST", "/indexing/config", Some(&body)).await;
        assert_eq!(
            updated.status, 200,
            "enabling an index must answer 200, got {} {:?}",
            updated.status, updated.body
        );
    }

    for index_name in [THING_INDEX_NAME, THING_GROUP_INDEX_NAME] {
        let reply = emulator
            .call("GET", &format!("/indices/{index_name}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index_name:?}: an enabled index must be described, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body.get("indexName").and_then(Value::as_str),
            Some(index_name),
            "case {index_name:?}: the description must name the index, got {:?}",
            reply.body
        );
    }
}

/// The `LogTargetType` members as a refusal LISTS them: the order of the hash table the
/// front door holds an enumerated type's admissible values in, which for this type's five
/// declared members (`smithy_query(query="shape",shape="LogTargetType")`: DEFAULT,
/// THING_GROUP, CLIENT_ID, SOURCE_IP, PRINCIPAL_ID) is neither their declaration order nor
/// the alphabetical one. Written out here so the order is pinned by this suite rather than
/// by the code under test.
const LOG_TARGET_TYPES: &str = "DEFAULT, SOURCE_IP, PRINCIPAL_ID, THING_GROUP, CLIENT_ID";

/// One request this service's validation layer will not accept, and the clause the
/// refusal must state for it.
#[derive(Debug)]
struct RefusedRequest {
    /// What this row is, for the assertion messages.
    case: String,
    method: &'static str,
    path: String,
    /// A body for the methods that carry one; `None` for a read or a delete.
    body: Option<Value>,
    /// What AWS states after the envelope's `1 validation error detected: `.
    clause: String,
    /// What the request sent for the member the clause is about.
    rejected: String,
    /// Whether this constraint class quotes the submitted value: an absent member and a
    /// `@pattern` are stated with none, an enumerated set with the value it rejected.
    quotes_the_value: bool,
}

/// A `CreateScheduledAudit` body carrying the two members the model requires of one —
/// `frequency` and `targetCheckNames` — plus the `dayOfMonth` under test, so the refusal
/// is about that member alone.
fn scheduled_audit_body(day_of_month: &str) -> Value {
    json!({
        "frequency": "MONTHLY",
        "targetCheckNames": ["LOGGING_DISABLED_CHECK"],
        "dayOfMonth": day_of_month,
    })
}

/// Every row of the validation table: each operation, each constraint class, and several
/// rejected values per row.
fn refused_requests() -> Vec<RefusedRequest> {
    let mut rows = Vec::new();

    // A value outside an enumerated set, bound to the QUERY string: the clause quotes the
    // value and the set, and the set is listed in the order the front door's own hash table
    // is walked ([`LOG_TARGET_TYPES`]).
    for value in ["NOT_A_REAL_TYPE", "default", "THING-GROUP"] {
        rows.push(RefusedRequest {
            case: format!("DeleteV2LoggingLevel with the targetType {value:?}"),
            method: "DELETE",
            path: format!("/v2LoggingLevel?targetType={value}&targetName=probe"),
            body: None,
            clause: format!(
                "Value '{value}' at 'targetType' failed to satisfy constraint: Member must \
                 satisfy enum value set: [{LOG_TARGET_TYPES}]"
            ),
            rejected: value.to_string(),
            quotes_the_value: true,
        });
    }

    // A URI LABEL outside its `@pattern` (`DomainConfigurationName`: `^[\w.-]+$`): the
    // clause names the expression without the model's anchors and states no value.
    for value in ["IoT:reserved", "edge+1", "config!"] {
        rows.push(RefusedRequest {
            case: format!("CreateDomainConfiguration named {value:?}"),
            method: "POST",
            path: format!("/domainConfigurations/{value}"),
            body: Some(json!({})),
            clause: format!(
                "Value at 'domainConfigurationName' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: {}",
                r"[\w.-]+"
            ),
            rejected: value.to_string(),
            quotes_the_value: false,
        });
    }

    // A second URI label under the same class, on another operation and another
    // expression (`PolicyVersionId`: `^[0-9]+$`).
    for value in ["v2", "1.0", "latest"] {
        rows.push(RefusedRequest {
            case: format!("SetDefaultPolicyVersion on the version {value:?}"),
            method: "PATCH",
            path: format!("/policies/probe-policy/version/{value}"),
            body: None,
            clause: "Value at 'policyVersionId' failed to satisfy constraint: Member must \
                     satisfy regular expression pattern: [0-9]+"
                .to_string(),
            rejected: value.to_string(),
            quotes_the_value: false,
        });
    }

    // A `@required` member the request did not carry at all: no value is rendered between
    // `Value` and `at`, however the rest of the body is shaped.
    for (label, body) in [
        ("an empty body", json!({})),
        ("a body carrying only tags", json!({ "tags": [] })),
    ] {
        rows.push(RefusedRequest {
            case: format!("CreatePolicy with {label}"),
            method: "POST",
            path: "/policies/probe-policy".to_string(),
            body: Some(body),
            clause: "Value at 'policyDocument' failed to satisfy constraint: Member must not \
                     be null"
                .to_string(),
            rejected: "null".to_string(),
            quotes_the_value: false,
        });
    }

    rows
}

/// Every row of the validation table: the status, the error shape as this service NAMES
/// it, and the one clause AWS states for the constraint the request broke.
#[tokio::test]
async fn a_request_the_validation_layer_will_not_accept_is_one_refusal_everywhere() {
    for case in refused_requests() {
        // A fresh emulator per row: a validation refusal is decided by the REQUEST, so it
        // must not depend on what another row left behind.
        let emulator = Emulator::new();
        let reply = emulator
            .call(case.method, &case.path, case.body.as_ref())
            .await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected status 400, got {} {:?}",
            case.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(QUALIFIED_INVALID_REQUEST),
            "case {:?}: expected x-amzn-errortype {QUALIFIED_INVALID_REQUEST:?}, got {:?}",
            case.case,
            reply.error_type
        );
        let expected = format!("1 validation error detected: {}", case.clause);
        assert_eq!(
            reply.message(),
            expected,
            "case {:?}: expected message {expected:?}, got {:?}",
            case.case,
            reply.message()
        );
        let quoted = format!("Value '{}'", case.rejected);
        assert_eq!(
            reply.message().contains(&quoted),
            case.quotes_the_value,
            "case {:?}: expected the clause to quote the submitted value: \
             {}, got {:?}",
            case.case,
            case.quotes_the_value,
            reply.message()
        );
    }
}

/// RULE: the error SHAPE a request-validation refusal names is this service's, qualified
/// with its API doc namespace, for EVERY operation — the name belongs to the shape, so no
/// operation has to be known for it to be named that way.
///
/// The CLAUSE is not asserted here: it is the table above's for the rows it covers, and for
/// `CreateScheduledAudit`'s `dayOfMonth` the recorded wording (the value quoted, the
/// `@pattern` verbatim) is a divergence of the shared clause builder that this suite does
/// not pin either way.
#[tokio::test]
async fn every_operations_validation_refusal_names_this_services_shape() {
    // (case, method, path, body) — one broken constraint per operation, over the three
    // places a member is bound: the URI path, the query string and the body.
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "DeleteV2LoggingLevel on a targetType outside the value set",
            "DELETE",
            "/v2LoggingLevel?targetType=NOT_A_REAL_TYPE&targetName=probe".to_string(),
            None,
        ),
        (
            "CreateDomainConfiguration on a name outside its pattern",
            "POST",
            "/domainConfigurations/IoT:reserved".to_string(),
            Some(json!({})),
        ),
        (
            "SetDefaultPolicyVersion on a version id outside its pattern",
            "PATCH",
            "/policies/probe-policy/version/v2".to_string(),
            None,
        ),
        (
            "CreatePolicy with the required document absent",
            "POST",
            "/policies/probe-policy".to_string(),
            Some(json!({})),
        ),
        (
            "CreateScheduledAudit on a day of month outside its pattern",
            "POST",
            "/audit/scheduledaudits/probe_audit".to_string(),
            Some(scheduled_audit_body("99")),
        ),
        (
            "CreateScheduledAudit with a frequency outside the value set",
            "POST",
            "/audit/scheduledaudits/probe_audit".to_string(),
            Some(json!({
                "frequency": "HOURLY",
                "targetCheckNames": ["LOGGING_DISABLED_CHECK"],
            })),
        ),
    ];

    for (case, method, path, body) in rows {
        let emulator = Emulator::new();
        let reply = emulator.call(method, &path, body.as_ref()).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected status 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(QUALIFIED_INVALID_REQUEST),
            "case {case:?}: expected x-amzn-errortype {QUALIFIED_INVALID_REQUEST:?}, got {:?}",
            reply.error_type
        );
        assert!(
            reply.message().starts_with("1 validation error detected: "),
            "case {case:?}: expected AWS's constraint envelope, got {:?}",
            reply.message()
        );
    }
}

/// The other side of the same decision: a request that satisfies the constraint is NOT
/// answered in that envelope, so the refusals above are about the value the request sent
/// and not about the operation.
#[tokio::test]
async fn a_request_that_satisfies_the_constraint_is_not_refused_that_way() {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "iot:Connect", "Resource": "*" }],
    })
    .to_string();

    // (case, method, path, body, whether the request must be SERVED)
    let rows: Vec<(&str, &str, String, Option<Value>, bool)> = vec![
        (
            "CreatePolicy carrying the required document",
            "POST",
            "/policies/probe-policy".to_string(),
            Some(json!({ "policyDocument": document })),
            true,
        ),
        (
            "CreateDomainConfiguration named within its pattern",
            "POST",
            "/domainConfigurations/probe.config-1".to_string(),
            Some(json!({})),
            true,
        ),
        (
            // The audit create is still scaffolded, so what it answers past the
            // validation layer is not asserted here; what matters is that a day of month
            // within the pattern draws no constraint clause.
            "CreateScheduledAudit on a day of month within its pattern",
            "POST",
            "/audit/scheduledaudits/probe_audit".to_string(),
            Some(scheduled_audit_body("28")),
            false,
        ),
        (
            // A member inside the model's value set: whatever this operation answers, it
            // is not the validation layer's envelope.
            "DeleteV2LoggingLevel with a targetType the model declares",
            "DELETE",
            "/v2LoggingLevel?targetType=CLIENT_ID&targetName=probe".to_string(),
            None,
            false,
        ),
        (
            // A version id inside the model's pattern: the refusal that follows is the
            // operation's own, not a constraint clause.
            "SetDefaultPolicyVersion on a numeric version id",
            "PATCH",
            "/policies/probe-policy/version/2".to_string(),
            None,
            false,
        ),
    ];

    for (case, method, path, body, served) in rows {
        let emulator = Emulator::new();
        let reply = emulator.call(method, &path, body.as_ref()).await;
        if served {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the request to be served, got {} {:?}",
                reply.status, reply.body
            );
        }
        assert!(
            !reply.message().starts_with("1 validation error detected"),
            "case {case:?}: expected no constraint clause, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}
