// SPDX-License-Identifier: CC0-1.0

//! The documented teardown path of an AWS IoT custom authorizer, end to end over HTTP:
//! `UpdateAuthorizer` takes the authorizer out of service, and `DeleteAuthorizer`
//! removes it once it is.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see: the status, the `x-amzn-errortype` header it resolves the
//! error shape from, the `message` it surfaces, and the response members.
//!
//! The rule under test: a delete of a resource this control plane keeps a STATUS on is
//! admissible only while that status says the resource is out of service, and the
//! operation that carries the `status` member is how a client gets it there. For a
//! custom authorizer that means
//!
//! * `UpdateAuthorizer` against an authorizer the account HOLDS answers 200 with the
//!   authorizer's `authorizerName` and the `authorizerArn` its create minted — whatever
//!   members the request states, and whichever status it moves the authorizer to — and
//!   the next `DescribeAuthorizer` reads the change back;
//! * `ResourceNotFoundException` (404) is reserved for a name the account holds NO
//!   authorizer under, worded as the authorizer family words a miss;
//! * `DeleteAuthorizer` of an authorizer whose persisted status is not `INACTIVE` is
//!   refused `InvalidRequestException` (400), naming the status the record carries and
//!   the status to move it to, and leaves the record exactly as a client read it;
//! * the same delete succeeds, with an empty body, once the authorizer is `INACTIVE` —
//!   and is refused again if it is re-activated, because the precondition is read from
//!   the record and not from the create request.
//!
//! The table spans what that rule generalises over: which status the create asks for
//! (absent, `ACTIVE`, `INACTIVE` — the whole `AuthorizerStatus` enum, from
//! `smithy_query(query="shape",shape="AuthorizerStatus")`), the form and length of the
//! `authorizerName` within its modelled `@pattern` `^[\w=,@-]+$` and `@length 1..128`,
//! how many members the deactivating update states beside the status, and whether the
//! authorizer is deactivated, re-activated or deactivated twice before the delete.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    DEFAULT_AUTHORIZER_STATUS, INACTIVE_AUTHORIZER_STATUS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// `InvalidRequestException` as this service names it in `x-amzn-errortype`: the shape
/// name qualified with the service's API doc namespace.
const INVALID_REQUEST: &str = "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";
/// The wire error shape of a resource this service does not hold.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and the region: the scope is how the router picks the claiming service and how the
/// service scopes the ARNs it mints. The signature itself is never checked here — the
/// harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
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
    /// The `x-amzn-errortype` header, verbatim.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<&str> {
        self.body.get(name).and_then(Value::as_str)
    }

    /// The `message` the body carries, or `""` when it carries none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The `authorizerDescription` a `DescribeAuthorizer` reports.
    fn description(&self) -> Option<&Value> {
        self.body.get("authorizerDescription")
    }

    /// A member of the reported `AuthorizerDescription`.
    fn member(&self, name: &str) -> Option<&Value> {
        self.description().and_then(|value| value.get(name))
    }

    /// A string member of the reported `AuthorizerDescription`.
    fn member_str(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
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

    async fn create(&self, name: &str, body: &Value) -> Reply {
        self.call("POST", &format!("/authorizer/{name}"), Some(body))
            .await
    }

    async fn describe(&self, name: &str) -> Reply {
        self.call("GET", &format!("/authorizer/{name}"), None).await
    }

    async fn update(&self, name: &str, body: &Value) -> Reply {
        self.call("PUT", &format!("/authorizer/{name}"), Some(body))
            .await
    }

    async fn delete(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/authorizer/{name}"), None)
            .await
    }
}

/// The ARN an authorizer created in this suite's region must carry.
fn expected_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:authorizer/{name}")
}

/// How this service words the refusal of a delete that comes while the authorizer is
/// still in service.
fn undeletable(name: &str, status: &str) -> String {
    format!(
        "Cannot delete authorizer {name} in {status} status. Update status to \
         {INACTIVE_AUTHORIZER_STATUS} and delete."
    )
}

/// One row of the teardown table: a case name, the authorizer name, the create body, the
/// status that create resolves to, and the body of the update that deactivates it.
struct Row {
    case: &'static str,
    name: String,
    create: Value,
    /// What `DescribeAuthorizer` must report before the deactivating update: the create
    /// body's `status`, or [`DEFAULT_AUTHORIZER_STATUS`] when it stated none.
    created_status: &'static str,
    /// The `UpdateAuthorizer` body that takes the authorizer out of service. Rows differ
    /// in how many members they state beside the status.
    deactivate: Value,
}

/// A Lambda function ARN for an authorizer to call, varied per row only so no two rows
/// share a value.
fn function_arn(suffix: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:authorize-{suffix}")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create that states no status, deactivated by the status alone",
            name: "default_status".to_string(),
            create: json!({ "authorizerFunctionArn": function_arn("default") }),
            created_status: DEFAULT_AUTHORIZER_STATUS,
            deactivate: json!({ "status": INACTIVE_AUTHORIZER_STATUS }),
        },
        Row {
            case: "an explicitly ACTIVE authorizer, deactivated alongside a new function ARN",
            name: "explicitly_active".to_string(),
            create: json!({
                "authorizerFunctionArn": function_arn("active"),
                "status": "ACTIVE",
                "tokenKeyName": "X-Amz-Custom-Token",
            }),
            created_status: "ACTIVE",
            deactivate: json!({
                "status": INACTIVE_AUTHORIZER_STATUS,
                "authorizerFunctionArn": function_arn("active-replacement"),
            }),
        },
        Row {
            case: "an authorizer deactivated alongside every other member the update takes",
            name: "Teardown_Authorizer=42,@host-1".to_string(),
            create: json!({
                "authorizerFunctionArn": function_arn("every-member"),
                "tokenKeyName": "token",
                "tokenSigningPublicKeys": {"FIRST_KEY": "-----BEGIN PUBLIC KEY-----\nAAAA"},
                "enableCachingForHttp": false,
            }),
            created_status: DEFAULT_AUTHORIZER_STATUS,
            deactivate: json!({
                "status": INACTIVE_AUTHORIZER_STATUS,
                "authorizerFunctionArn": function_arn("every-member-replacement"),
                "tokenKeyName": "other-token",
                "tokenSigningPublicKeys": {"SECOND_KEY": "-----BEGIN PUBLIC KEY-----\nBBBB"},
                "enableCachingForHttp": true,
            }),
        },
        Row {
            case: "the shortest legal authorizer name",
            name: "a".to_string(),
            create: json!({
                "authorizerFunctionArn": function_arn("short"),
                "signingDisabled": true,
            }),
            created_status: DEFAULT_AUTHORIZER_STATUS,
            deactivate: json!({ "status": INACTIVE_AUTHORIZER_STATUS }),
        },
        Row {
            case: "the longest legal authorizer name",
            name: "z".repeat(128),
            create: json!({
                "authorizerFunctionArn": function_arn("long"),
                "status": "ACTIVE",
                "enableCachingForHttp": true,
            }),
            created_status: "ACTIVE",
            deactivate: json!({ "status": INACTIVE_AUTHORIZER_STATUS }),
        },
        Row {
            case: "an authorizer created INACTIVE, which the update states again",
            name: "created-inactive".to_string(),
            create: json!({
                "authorizerFunctionArn": function_arn("inactive"),
                "status": INACTIVE_AUTHORIZER_STATUS,
            }),
            created_status: INACTIVE_AUTHORIZER_STATUS,
            deactivate: json!({ "status": INACTIVE_AUTHORIZER_STATUS }),
        },
    ]
}

#[tokio::test]
async fn an_authorizer_is_updated_out_of_service_and_then_deleted() {
    let emulator = Emulator::new();

    for Row {
        case,
        name,
        create,
        created_status,
        deactivate,
    } in rows()
    {
        let created = emulator.create(&name, &create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAuthorizer must succeed, got {created:?}"
        );
        let arn = expected_arn(&name);

        // While the authorizer is in service the delete is refused, and the record a
        // client read a moment earlier is left exactly as it was.
        let before = emulator.describe(&name).await;
        assert_eq!(
            before.member_str("status"),
            Some(created_status),
            "case {case:?}: expected the created status {created_status:?}, got {before:?}"
        );
        if created_status != INACTIVE_AUTHORIZER_STATUS {
            let refused = emulator.delete(&name).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: deleting an authorizer in {created_status} status must be \
                 refused 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some(INVALID_REQUEST),
                "case {case:?}: expected {INVALID_REQUEST}, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                undeletable(&name, created_status),
                "case {case:?}: expected the refusal to name the status and the one to move \
                 to, got {refused:?}"
            );
            let after = emulator.describe(&name).await;
            assert_eq!(
                after.body, before.body,
                "case {case:?}: the refused delete must leave the authorizer untouched, got \
                 {after:?}"
            );
        }

        // The update answers 200 with the authorizer's identity — the name it was
        // created under and the ARN that create minted — never a not-found for a
        // resource the account holds.
        let updated = emulator.update(&name, &deactivate).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateAuthorizer of an authorizer the account holds must answer \
             200, got {updated:?}"
        );
        assert_eq!(
            updated.text("authorizerName"),
            Some(name.as_str()),
            "case {case:?}: expected the update to report the authorizer's name, got {updated:?}"
        );
        assert_eq!(
            updated.text("authorizerArn"),
            Some(arn.as_str()),
            "case {case:?}: expected the update to report the minted ARN {arn}, got {updated:?}"
        );

        // The change is readable, member for member, from the reader of one authorizer.
        let read = emulator.describe(&name).await;
        assert_eq!(
            read.member_str("status"),
            Some(INACTIVE_AUTHORIZER_STATUS),
            "case {case:?}: expected the deactivated status to be read back, got {read:?}"
        );
        for (member, want) in deactivate.as_object().expect("the update body is an object") {
            if member == "status" {
                continue;
            }
            assert_eq!(
                read.member(member),
                Some(want),
                "case {case:?}: expected {member} to read back as {want}, got {read:?}"
            );
        }

        // Out of service, the delete succeeds: `DeleteAuthorizer` models no output
        // members, so it answers an empty body, and the authorizer is gone.
        let deleted = emulator.delete(&name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting a deactivated authorizer must succeed, got {deleted:?}"
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {case:?}: DeleteAuthorizer models no output members, got {deleted:?}"
        );
        let gone = emulator.describe(&name).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted authorizer must no longer be readable, got {gone:?}"
        );
    }
}

#[tokio::test]
async fn the_delete_precondition_is_read_from_the_record_and_not_from_the_create() {
    let emulator = Emulator::new();

    // Each row ends in the status the authorizer carries when the delete is attempted,
    // reached by a different route: created out of service, deactivated, deactivated
    // twice, or deactivated and then put back in service.
    struct Walk {
        case: &'static str,
        name: &'static str,
        create_status: Option<&'static str>,
        updates: Vec<&'static str>,
        deletable: bool,
    }

    let walks = vec![
        Walk {
            case: "created in service and never updated",
            name: "walk_untouched",
            create_status: None,
            updates: vec![],
            deletable: false,
        },
        Walk {
            case: "created in service and deactivated",
            name: "walk_deactivated",
            create_status: Some("ACTIVE"),
            updates: vec![INACTIVE_AUTHORIZER_STATUS],
            deletable: true,
        },
        Walk {
            case: "deactivated twice",
            name: "walk_deactivated_twice",
            create_status: Some("ACTIVE"),
            updates: vec![INACTIVE_AUTHORIZER_STATUS, INACTIVE_AUTHORIZER_STATUS],
            deletable: true,
        },
        Walk {
            case: "created out of service and put back in service",
            name: "walk_reactivated",
            create_status: Some(INACTIVE_AUTHORIZER_STATUS),
            updates: vec!["ACTIVE"],
            deletable: false,
        },
        Walk {
            case: "deactivated, re-activated, then deactivated again",
            name: "walk_round_trip",
            create_status: Some(INACTIVE_AUTHORIZER_STATUS),
            updates: vec!["ACTIVE", INACTIVE_AUTHORIZER_STATUS],
            deletable: true,
        },
    ];

    for walk in walks {
        let case = walk.case;
        let mut create = json!({ "authorizerFunctionArn": function_arn(walk.name) });
        if let Some(status) = walk.create_status {
            create["status"] = json!(status);
        }
        let created = emulator.create(walk.name, &create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAuthorizer must succeed, got {created:?}"
        );

        for status in &walk.updates {
            let updated = emulator.update(walk.name, &json!({ "status": status })).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: moving the authorizer to {status} must answer 200, got \
                 {updated:?}"
            );
        }

        let read = emulator.describe(walk.name).await;
        let status = read
            .member_str("status")
            .unwrap_or_else(|| panic!("case {case:?}: the authorizer must be readable: {read:?}"))
            .to_string();
        let deleted = emulator.delete(walk.name).await;
        if walk.deletable {
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: an authorizer in {status} status must be deletable, got \
                 {deleted:?}"
            );
        } else {
            assert_eq!(
                deleted.status, 400,
                "case {case:?}: an authorizer in {status} status must not be deletable, got \
                 {deleted:?}"
            );
            assert_eq!(
                deleted.message(),
                undeletable(walk.name, &status),
                "case {case:?}: expected the refusal to name the status the record carries, \
                 got {deleted:?}"
            );
            // The refused delete left the authorizer in place, so the documented
            // recovery path still works from here.
            let recovered = emulator
                .update(
                    walk.name,
                    &json!({ "status": INACTIVE_AUTHORIZER_STATUS }),
                )
                .await;
            assert_eq!(
                recovered.status, 200,
                "case {case:?}: the authorizer must still be updatable after the refusal, got \
                 {recovered:?}"
            );
            let deleted = emulator.delete(walk.name).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: the delete must succeed once the authorizer is out of service, \
                 got {deleted:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_the_account_holds_no_authorizer_under_is_the_only_not_found() {
    let emulator = Emulator::new();

    // One authorizer exists and is in service, so the refusals below are about the NAME
    // rather than about an empty store.
    let created = emulator
        .create(
            "held_authorizer",
            &json!({ "authorizerFunctionArn": function_arn("held") }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateAuthorizer must succeed, got {created:?}"
    );

    // The authorizer family words a miss `Authorizer <name> not found`, with the name
    // the request addressed as its only variable part.
    for name in [
        "a",
        "held_authorizer_2",
        "HELD_AUTHORIZER",
        "Missing=Authorizer,1@host-2",
        &"m".repeat(128),
    ] {
        for (operation, reply) in [
            ("UpdateAuthorizer", emulator.update(name, &json!({"status": "ACTIVE"})).await),
            ("DeleteAuthorizer", emulator.delete(name).await),
            ("DescribeAuthorizer", emulator.describe(name).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {operation} on {name:?}: expected 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some(NOT_FOUND),
                "case {operation} on {name:?}: expected {NOT_FOUND}, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                format!("Authorizer {name} not found"),
                "case {operation} on {name:?}: expected the authorizer family's sentence, got \
                 {reply:?}"
            );
        }
    }

    // None of those refusals recorded or removed anything: the authorizer the account
    // holds is still in service and still refuses its own delete for that reason.
    let read = emulator.describe("held_authorizer").await;
    assert_eq!(
        read.member_str("status"),
        Some(DEFAULT_AUTHORIZER_STATUS),
        "the held authorizer must be unchanged, got {read:?}"
    );
    let refused = emulator.delete("held_authorizer").await;
    assert_eq!(
        refused.status, 400,
        "the held authorizer is in service, so its delete must be refused 400, got {refused:?}"
    );
}

#[tokio::test]
async fn a_create_under_a_name_the_account_already_holds_is_refused_by_name() {
    let emulator = Emulator::new();

    // The name is the authorizer's identity: a second create under it is refused 409,
    // worded with the family's fixed text and the name the request addressed.
    for name in ["taken", "Taken_Name=1,@host-2", &"t".repeat(128)] {
        let first = emulator
            .create(name, &json!({ "authorizerFunctionArn": function_arn("first") }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first CreateAuthorizer must succeed, got {first:?}"
        );

        let second = emulator
            .create(name, &json!({ "authorizerFunctionArn": function_arn("second") }))
            .await;
        assert_eq!(
            second.status, 409,
            "case {name:?}: a repeat create must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.message(),
            format!("Authorizer {name} already exist for this account"),
            "case {name:?}: expected the family's already-exists sentence, got {second:?}"
        );
    }
}
