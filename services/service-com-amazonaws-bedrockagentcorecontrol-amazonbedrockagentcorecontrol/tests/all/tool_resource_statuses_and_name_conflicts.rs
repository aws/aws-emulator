// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two rules this control plane's sandbox tool resources
//! (custom browsers, custom code interpreters) and its browser profiles share:
//!
//! * a sandbox tool resource (a custom browser, a custom code interpreter) is provisioned
//!   inside the call that creates it and torn down inside
//!   the call that deletes it, so the only statuses a caller observes are the terminal
//!   ones — `READY` for as long as it exists, `DELETED` from the delete that removed it,
//!   and the modelled 404 for every read after that;
//! * a create whose `name` the account already holds is refused 409 `ConflictException`
//!   with the fixed sentence "<resource> with name '<name>' already exists in this
//!   account", whose only variable part is the `name` the request carried — no identifier
//!   the service generated for the resource already holding the name appears in it.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
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

/// One resource family under test: how its create is addressed and what it carries beyond
/// the `name`, which output member holds the minted id, and the words this control plane
/// spells the family with in a duplicate-name refusal.
///
/// Sources: `smithy_query
/// operation=CreateBrowser,CreateCodeInterpreter,CreateBrowserProfile` for the routes, the
/// required members and the id members.
struct Family {
    /// What the assertions call this family.
    case: &'static str,
    /// The create route, which is also the collection the delete and the read hang off.
    route: &'static str,
    /// The output member carrying the id the create minted.
    id_member: &'static str,
    /// The members the create carries beyond `name`.
    extra: fn() -> Value,
    /// The HTTP status the create answers with (`http` on the operation).
    create_status: u16,
    /// How the duplicate-name sentence names the family.
    conflict_label: &'static str,
}

/// The three families, each with the smallest body its create accepts.
fn families() -> [Family; 3] {
    [
        Family {
            case: "custom browser",
            route: "/browsers",
            id_member: "browserId",
            extra: || json!({"networkConfiguration": {"networkMode": "PUBLIC"}}),
            create_status: 202,
            conflict_label: "Browser",
        },
        Family {
            case: "custom code interpreter",
            route: "/code-interpreters",
            id_member: "codeInterpreterId",
            extra: || json!({"networkConfiguration": {"networkMode": "SANDBOX"}}),
            create_status: 202,
            conflict_label: "CodeInterpreter",
        },
        Family {
            case: "browser profile",
            route: "/browser-profiles",
            id_member: "profileId",
            extra: || json!({}),
            create_status: 200,
            conflict_label: "Browser profile",
        },
    ]
}

/// The request body for a create of `family` under `name`.
fn create_body(family: &Family, name: &str) -> Value {
    let mut body = json!({"name": name});
    for (member, value) in (family.extra)()
        .as_object()
        .expect("the family's extra members are an object")
    {
        body[member] = value.clone();
    }
    body
}

/// A sandbox tool resource is READY from the create that minted it until the delete that
/// removed it, which reports DELETED and leaves every later read the modelled 404.
///
/// The rows differ along the resource family and along the configuration its create
/// carries, which is what the provisioning would depend on: the network mode of a
/// browser and of a code interpreter (`CodeInterpreterNetworkMode` is PUBLIC | SANDBOX |
/// VPC, `smithy_query shape=CodeInterpreterNetworkMode`), with and without a caller
/// supplied VPC.
#[tokio::test]
async fn a_created_tool_resource_is_ready_and_a_deleted_one_is_deleted() {
    let router = router();

    // (case, the create route, the id member, the create body, the create's HTTP status)
    let vpc = json!({
        "networkMode": "VPC",
        "vpcConfig": {
            "securityGroups": ["sg-0123456789abcdef0"],
            "subnets": ["subnet-0123456789abcdef0"]
        }
    });
    let rows: [(&str, &str, &str, Value, u16); 5] = [
        (
            "a public browser",
            "/browsers",
            "browserId",
            json!({"name": "publicBrowser", "networkConfiguration": {"networkMode": "PUBLIC"}}),
            202,
        ),
        (
            "a browser on a caller-supplied VPC",
            "/browsers",
            "browserId",
            json!({"name": "vpcBrowser", "networkConfiguration": vpc.clone()}),
            202,
        ),
        (
            "a sandbox code interpreter",
            "/code-interpreters",
            "codeInterpreterId",
            json!({
                "name": "sandboxInterpreter",
                "networkConfiguration": {"networkMode": "SANDBOX"}
            }),
            202,
        ),
        (
            "a public code interpreter with a description",
            "/code-interpreters",
            "codeInterpreterId",
            json!({
                "name": "publicInterpreter",
                "description": "runs the reports",
                "networkConfiguration": {"networkMode": "PUBLIC"}
            }),
            202,
        ),
        (
            "a code interpreter on a caller-supplied VPC",
            "/code-interpreters",
            "codeInterpreterId",
            json!({"name": "vpcInterpreter", "networkConfiguration": vpc}),
            202,
        ),
    ];

    for (case, route, id_member, body, create_status) in rows {
        let (status, created) = send(&router, "PUT", route, Some(&body)).await;
        assert_eq!(
            status, create_status,
            "case {case:?}: expected HTTP {create_status} from the create, got {status} ({created})"
        );
        assert_eq!(
            text(&created, "status"),
            "READY",
            "case {case:?}: a created tool resource is usable at once, so the create reports \
             READY, got {}",
            text(&created, "status")
        );

        let id = text(&created, id_member);
        let path = format!("{route}/{id}");

        let (status, fetched) = send(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "status"),
            "READY",
            "case {case:?}: a read of the created resource reports READY, got {}",
            text(&fetched, "status")
        );

        let (status, deleted) = send(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from the delete, got {status} ({deleted})"
        );
        assert_eq!(
            text(&deleted, "status"),
            "DELETED",
            "case {case:?}: a deletion completes in the call, so it reports DELETED, got {}",
            text(&deleted, "status")
        );

        let (status, missing) = send(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted tool resource is the modelled 404, got {status} ({missing})"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {} ({missing})",
            error_code(&missing)
        );
    }
}

/// A create whose `name` the account already holds is refused 409 `ConflictException`
/// with this control plane's fixed sentence around the requested name, and the resource
/// already holding the name is not disclosed.
///
/// The rows differ along the name the collision is on — its length (the one-character
/// minimum and the 48-character maximum `SandboxName` / `BrowserProfileName`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` admits), its case and the digits and underscores it
/// carries — and the table spans every resource family that refuses a duplicate name.
#[tokio::test]
async fn a_duplicate_name_is_refused_with_the_control_planes_conflict_sentence() {
    // (case, the name both creates carry)
    let names: [(&str, &str); 4] = [
        ("the shortest name the pattern admits", "a"),
        (
            "the longest name the pattern admits (48 characters)",
            "n23456789_123456789_123456789_123456789_12345678",
        ),
        ("a mixed-case name with digits", "Report_Tool_42"),
        ("a name of underscores and letters", "team_shared_tool"),
    ];

    for family in families() {
        for (name_case, name) in names {
            // One router per row, so the name is free in the account each row starts from.
            let router = router();
            let body = create_body(&family, name);

            let (status, created) = send(&router, "PUT", family.route, Some(&body)).await;
            assert_eq!(
                status, family.create_status,
                "case {:?} / {name_case:?}: the first create of a name succeeds, got {status} \
                 ({created})",
                family.case
            );
            let held_id = text(&created, family.id_member);

            let (status, refused) = send(&router, "PUT", family.route, Some(&body)).await;
            assert_eq!(
                status, 409,
                "case {:?} / {name_case:?}: a name the account already holds is refused 409, \
                 got {status} ({refused})",
                family.case
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {:?} / {name_case:?}: expected ConflictException, got {} ({refused})",
                family.case,
                error_code(&refused)
            );
            assert_eq!(
                text(&refused, "message"),
                format!(
                    "{} with name '{name}' already exists in this account",
                    family.conflict_label
                ),
                "case {:?} / {name_case:?}: the refusal is the control plane's sentence around \
                 the requested name, got {}",
                family.case,
                text(&refused, "message")
            );
            assert!(
                !text(&refused, "message").contains(&held_id),
                "case {:?} / {name_case:?}: the refusal discloses the resource already holding \
                 the name, in {}",
                family.case,
                text(&refused, "message")
            );
        }
    }
}

/// A name that differs from a held one only in case, or only in a trailing character, is
/// its own resource: the refusal is keyed to the name the request carried, not to a
/// normalised form of it.
#[tokio::test]
async fn a_name_differing_from_a_held_one_is_its_own_resource() {
    // (case, the first name, the second name)
    let pairs: [(&str, &str, &str); 3] = [
        ("differing in the leading character's case", "toolOne", "ToolOne"),
        ("differing in a trailing digit", "toolTwo2", "toolTwo3"),
        ("differing by a trailing underscore", "toolThree", "toolThree_"),
    ];

    for family in families() {
        for (case, first, second) in pairs {
            let router = router();

            let (status, _) = send(&router, "PUT", family.route, Some(&create_body(&family, first)))
                .await;
            assert_eq!(
                status, family.create_status,
                "case {:?} / {case:?}: the first create succeeds, got {status}",
                family.case
            );

            let (status, answered) =
                send(&router, "PUT", family.route, Some(&create_body(&family, second))).await;
            assert_eq!(
                status, family.create_status,
                "case {:?} / {case:?}: a name that differs is its own resource, got {status} \
                 ({answered})",
                family.case
            );
            assert_eq!(
                text(&answered, "status"),
                "READY",
                "case {:?} / {case:?}: the resource the second create mints is READY, got {}",
                family.case,
                text(&answered, "status")
            );
        }
    }
}
