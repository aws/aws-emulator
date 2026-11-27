// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the `@range` class of this service's constraint envelope:
//! what a clause about a NUMERIC bound states, and that a request it refuses records
//! nothing.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: a clause about a member's `@range` names the member and the bound it
//! broke and states NO value — `Value at '<member>' failed to satisfy constraint: Member must
//! have value {less,greater} than or equal to <bound>` — because the bound already states the
//! interval the value had to lie in, so the number that fell outside it would say the same
//! thing twice. It is the same decision the `@length` and `@pattern` classes take, and the
//! opposite of the enumerated class, whose requirement is a SET the rejected value is stated
//! beside.
//!
//! The envelope itself is the parent suite's: 400, `InvalidRequestException` qualified with
//! the service's API doc namespace in `x-amzn-errortype`, `N validation error(s) detected: `
//! over one clause per broken constraint joined with `; `.
//!
//! The table spans what the rule generalises over rather than one request: both ends of a
//! bound (a value past a maximum and one short of a minimum), the published bound itself and
//! the first value past it, bounds of three different widths, members bound to the body and
//! members bound to the query string, six operations across four resource families, and a
//! range clause alone and beside a clause of another class. A refused create is then read
//! back through the family's own reader, which must still answer its not-found refusal: a
//! request the validation layer turns away reaches no handler and so persists nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. No refusal here is regional, so
/// no assertion depends on it.
const REGION: &str = "us-east-2";

/// `InvalidRequestException` as AWS IoT names it in `x-amzn-errortype`: the shape name, a
/// `:`, and the service's API doc namespace.
const QUALIFIED_INVALID_REQUEST: &str =
    "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// The bounds of `CredentialDurationSeconds`, which both `CreateRoleAlias` and
/// `UpdateRoleAlias` carry (`range 900..43200`,
/// `smithy_query(query="shape",shape="CredentialDurationSeconds")`).
const MIN_CREDENTIAL_DURATION_SECONDS: i64 = 900;
const MAX_CREDENTIAL_DURATION_SECONDS: i64 = 43_200;

/// The bounds of the three page-size types this suite drives — `RegistryMaxResults`,
/// `PageSize` and `MaxResults` — every one of them `range 1..250`
/// (`smithy_query(query="shape",shape="RegistryMaxResults,PageSize,MaxResults")`).
const MIN_PAGE_SIZE: i64 = 1;
const MAX_PAGE_SIZE: i64 = 250;

/// The bounds of `TopicRuleDestinationMaxResults`, the page size of
/// `ListTopicRuleDestinations` (`range 1..1000`,
/// `smithy_query(query="shape",shape="TopicRuleDestinationMaxResults")`) — a third bound
/// width, so no row's expectation can be read off one number.
const MIN_DESTINATION_PAGE_SIZE: i64 = 1;
const MAX_DESTINATION_PAGE_SIZE: i64 = 1_000;

/// A role ARN inside `RoleArn`'s `length 20..2048`, so a row about the credential duration
/// draws no clause about the role.
const WELL_FORMED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/device";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name: the
/// scope is how the router picks the claiming service. The signature itself is never
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
    /// The `x-amzn-errortype` header, verbatim — including any namespace the shape name is
    /// qualified with.
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

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
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

/// The clause a refusal states for a member past its `@range` maximum: the member and the
/// bound, with no value at all.
fn past_the_maximum(member: &str, maximum: i64) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must have value less than \
         or equal to {maximum}"
    )
}

/// The clause a refusal states for a member short of its `@range` minimum: the member and
/// the bound, with no value at all.
fn short_of_the_minimum(member: &str, minimum: i64) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must have value greater \
         than or equal to {minimum}"
    )
}

/// One request this service's validation layer will not accept, and the clauses the refusal
/// must state for it, in the order it states them.
struct RefusedRequest {
    /// What this row is, for the assertion messages.
    case: String,
    method: &'static str,
    path: String,
    /// A body for the rows that carry one.
    body: Option<Value>,
    clauses: Vec<String>,
    /// What the request sent for the member the range clause is about: no clause may quote
    /// it.
    rejected: i64,
}

/// A `CreateRoleAlias` / `UpdateRoleAlias` body asking for `credential_duration_seconds`,
/// with the `roleArn` both operations accept so the refusal is about that member alone.
fn role_alias_body(credential_duration_seconds: i64) -> Value {
    json!({
        "roleArn": WELL_FORMED_ROLE_ARN,
        "credentialDurationSeconds": credential_duration_seconds,
    })
}

/// Every row of the range table.
fn refused_requests() -> Vec<RefusedRequest> {
    let mut rows = Vec::new();

    // `CreateRoleAlias$credentialDurationSeconds` and `UpdateRoleAlias$credentialDurationSeconds`
    // — BODY members of two operations sharing one bound — at each end of it: the first value
    // past the published maximum, a value well past it, the first value short of the published
    // minimum, and the lowest value a client can send.
    let duration = "credentialDurationSeconds";
    for (method, operation) in [("POST", "CreateRoleAlias"), ("PUT", "UpdateRoleAlias")] {
        for (label, value, clause) in [
            (
                "the first second past the maximum",
                MAX_CREDENTIAL_DURATION_SECONDS + 1,
                past_the_maximum(duration, MAX_CREDENTIAL_DURATION_SECONDS),
            ),
            (
                "a duration well past the maximum",
                46_800,
                past_the_maximum(duration, MAX_CREDENTIAL_DURATION_SECONDS),
            ),
            (
                "the last second short of the minimum",
                MIN_CREDENTIAL_DURATION_SECONDS - 1,
                short_of_the_minimum(duration, MIN_CREDENTIAL_DURATION_SECONDS),
            ),
            (
                "a duration of no seconds at all",
                0,
                short_of_the_minimum(duration, MIN_CREDENTIAL_DURATION_SECONDS),
            ),
        ] {
            rows.push(RefusedRequest {
                case: format!("{operation} asking for {label}"),
                method,
                path: "/role-aliases/range_probe".to_string(),
                body: Some(role_alias_body(value)),
                clauses: vec![clause],
                rejected: value,
            });
        }
    }

    // The same class on a QUERY member, over three paginated readers of three resource
    // families: `ListThings$maxResults` (`RegistryMaxResults`) and the `pageSize` of
    // `ListPolicies` and `ListRoleAliases` (`PageSize`), each `range 1..250`.
    for (operation, path, member) in [
        ("ListThings", "/things", "maxResults"),
        ("ListPolicies", "/policies", "pageSize"),
        ("ListRoleAliases", "/role-aliases", "pageSize"),
    ] {
        for (label, value, clause) in [
            (
                "one page past the maximum",
                MAX_PAGE_SIZE + 1,
                past_the_maximum(member, MAX_PAGE_SIZE),
            ),
            (
                "a page of none",
                MIN_PAGE_SIZE - 1,
                short_of_the_minimum(member, MIN_PAGE_SIZE),
            ),
        ] {
            rows.push(RefusedRequest {
                case: format!("{operation} asking for {label}"),
                method: "GET",
                path: format!("{path}?{member}={value}"),
                body: None,
                clauses: vec![clause],
                rejected: value,
            });
        }
    }

    // A bound of a THIRD width, on a fourth resource family:
    // `ListTopicRuleDestinations$maxResults` (`TopicRuleDestinationMaxResults`,
    // `range 1..1000`), at each end of it.
    for (label, value, clause) in [
        (
            "one page past the maximum",
            MAX_DESTINATION_PAGE_SIZE + 1,
            past_the_maximum("maxResults", MAX_DESTINATION_PAGE_SIZE),
        ),
        (
            "a page of none",
            MIN_DESTINATION_PAGE_SIZE - 1,
            short_of_the_minimum("maxResults", MIN_DESTINATION_PAGE_SIZE),
        ),
    ] {
        rows.push(RefusedRequest {
            case: format!("ListTopicRuleDestinations asking for {label}"),
            method: "GET",
            path: format!("/destinations?maxResults={value}"),
            body: None,
            clauses: vec![clause],
            rejected: value,
        });
    }

    rows
}

/// RULE: a clause about a `@range` bound names the member and the bound and states NO value,
/// at either end of the bound, for a member bound anywhere in the request, on every
/// operation — and the envelope counts the clauses as it does for any other class.
#[tokio::test]
async fn a_range_clause_states_the_bound_and_withholds_the_value_that_broke_it() {
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
        let envelope = match case.clauses.len() {
            1 => "1 validation error detected: ".to_string(),
            n => format!("{n} validation errors detected: "),
        };
        let expected = format!("{envelope}{}", case.clauses.join("; "));
        assert_eq!(
            reply.message(),
            expected,
            "case {:?}: expected message {expected:?}, got {:?}",
            case.case,
            reply.message()
        );
        let quoted = format!("Value '{}'", case.rejected);
        assert!(
            !reply.message().contains(&quoted),
            "case {:?}: expected the clause NOT to quote {quoted:?}, got {:?}",
            case.case,
            reply.message()
        );
    }
}

/// RULE: the class that states a SET of admissible values quotes the value it rejected,
/// where the range class states none — the envelope states one clause per broken constraint
/// and each clause its own value half — including in a message that states a range clause
/// beside it.
#[tokio::test]
async fn a_class_that_quotes_the_submitted_value_still_quotes_it() {
    // `ListAuditTasks` binds both `maxResults` (`range 1..250`) and `taskStatus` (the
    // `AuditTaskStatus` enumerated type) to the query string
    // (`smithy_query(query="operation",operation="ListAuditTasks")`), so one request can
    // break a numeric bound and a value set at once.
    let rejected_status = "NOT_A_STATUS";
    let rejected_max_results = MAX_PAGE_SIZE + 1;
    let rows = [
        ("an unmodelled task status alone", None, 1),
        (
            "an unmodelled task status beside a page size past its maximum",
            Some(past_the_maximum("maxResults", MAX_PAGE_SIZE)),
            2,
        ),
    ];

    for (case, range_clause, clauses) in rows {
        let emulator = Emulator::new();
        let mut query = format!(
            "startTime=2026-01-01T00%3A00%3A00Z&endTime=2026-01-02T00%3A00%3A00Z\
             &taskStatus={rejected_status}"
        );
        if range_clause.is_some() {
            query.push_str(&format!("&maxResults={rejected_max_results}"));
        }
        let reply = emulator
            .call("GET", &format!("/audit/tasks?{query}"), None)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected status 400, got {} {:?}",
            reply.status, reply.body
        );
        let envelope = match clauses {
            1 => "1 validation error detected: ".to_string(),
            n => format!("{n} validation errors detected: "),
        };
        assert!(
            reply.message().starts_with(&envelope),
            "case {case:?}: expected the envelope {envelope:?}, got {:?}",
            reply.message()
        );
        let quoted = format!(
            "Value '{rejected_status}' at 'taskStatus' failed to satisfy constraint: Member \
             must satisfy enum value set: ["
        );
        assert!(
            reply.message().contains(&quoted),
            "case {case:?}: expected the enum clause to quote the submitted value, got {:?}",
            reply.message()
        );
        if let Some(range_clause) = range_clause {
            assert!(
                reply.message().contains(&range_clause),
                "case {case:?}: expected the clause {range_clause:?}, got {:?}",
                reply.message()
            );
        }
    }
}

/// The other side of the same decision: a value AT either published bound, and one between
/// them, draws no constraint envelope at all — so the refusals above are about the values
/// the requests sent and not about the operations.
#[tokio::test]
async fn a_request_within_every_bound_is_not_refused_that_way() {
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateRoleAlias at the credential duration minimum",
            "POST",
            "/role-aliases/at_the_minimum".to_string(),
            Some(role_alias_body(MIN_CREDENTIAL_DURATION_SECONDS)),
        ),
        (
            "CreateRoleAlias at the credential duration maximum",
            "POST",
            "/role-aliases/at_the_maximum".to_string(),
            Some(role_alias_body(MAX_CREDENTIAL_DURATION_SECONDS)),
        ),
        (
            "CreateRoleAlias between the credential duration bounds",
            "POST",
            "/role-aliases/between_the_bounds".to_string(),
            Some(role_alias_body(3_600)),
        ),
        (
            "ListThings at the page-size maximum",
            "GET",
            format!("/things?maxResults={MAX_PAGE_SIZE}"),
            None,
        ),
        (
            "ListRoleAliases at the page-size minimum",
            "GET",
            format!("/role-aliases?pageSize={MIN_PAGE_SIZE}"),
            None,
        ),
        (
            "ListTopicRuleDestinations at its own page-size maximum",
            "GET",
            format!("/destinations?maxResults={MAX_DESTINATION_PAGE_SIZE}"),
            None,
        ),
    ];

    for (case, method, path, body) in rows {
        let emulator = Emulator::new();
        let reply = emulator.call(method, &path, body.as_ref()).await;
        assert!(
            !reply.message().contains(" validation error"),
            "case {case:?}: expected no constraint clause, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// RULE: a request the validation layer turns away reaches no handler, so it records
/// nothing — the family's own single-resource reader still refuses the name it named with
/// its not-found sentence, whose only variable part is that name.
#[tokio::test]
async fn a_role_alias_a_range_refusal_turned_away_was_never_created() {
    let emulator = Emulator::new();

    for (case, name, duration) in [
        (
            "a create past the credential duration maximum",
            "above_the_maximum",
            MAX_CREDENTIAL_DURATION_SECONDS + 1,
        ),
        (
            "a create short of the credential duration minimum",
            "below_the_minimum",
            MIN_CREDENTIAL_DURATION_SECONDS - 1,
        ),
    ] {
        let refused = emulator
            .call(
                "POST",
                &format!("/role-aliases/{name}"),
                Some(&role_alias_body(duration)),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the create to be refused 400, got {refused:?}"
        );

        let read = emulator
            .call("GET", &format!("/role-aliases/{name}"), None)
            .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected DescribeRoleAlias to answer 404, got {read:?}"
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {read:?}"
        );
        assert_eq!(
            read.message(),
            format!("Role alias {name} not found"),
            "case {case:?}: unexpected not-found message, got {read:?}"
        );
    }
}
