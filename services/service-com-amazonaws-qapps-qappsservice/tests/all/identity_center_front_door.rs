// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's two authorization front doors.
//!
//! The RULES these tests assert:
//!
//! * Every operation whose model declares `UnauthorizedException` is served only to a
//!   caller whose credentials carry an IAM Identity Center identity; one that carries
//!   none is refused 401 `UnauthorizedException` AHEAD of everything else — whether the
//!   resource it named exists, whether its payload satisfies the model, and whether the
//!   operation is implemented at all. The rows of each table below differ along exactly
//!   those axes, so no row can be satisfied by the answer another row earns.
//! * Which operations that is comes from the MODEL, not from a list: the three
//!   resource-tagging operations are the only ones declaring no 401, and they are
//!   authorized through the caller's IAM policy against the ARN they name instead —
//!   403 `AccessDeniedException` for an ARN that account's policies cannot grant,
//!   and the resource's own tags for one they can.
//! * A caller federated through the Identity Center portal is served, and the tags of a
//!   resource survive the round trip through `TagResource` / `ListTagsForResource` /
//!   `UntagResource`.
//!
//! Every test drives the service the way a client does — through the router, over
//! restJson1, with the `instance-id` header and a SigV4 `Authorization` header whose
//! access key id is the credential presented.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_qapps_qappsservice::service::identity_center::{
    IAM_AUTHORIZED_OPERATIONS, UNAUTHORIZED_MESSAGE,
};
use service_com_amazonaws_qapps_qappsservice::state::MAX_TAGS_PER_RESOURCE;
use service_com_amazonaws_qapps_qappsservice::EmulatedService;
use service_com_amazonaws_sso_swbportalservice::state::{self as portal, RoleCredentialRecord};
use service_com_amazonaws_sso_swbportalservice::EmulatedService as PortalService;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_center::{access_token_for, federate, ACCOUNT, ROLE, SESSION_TTL};

/// The Amazon Q Business application environment instance every request names.
const INSTANCE: &str = "11111111-2222-4333-8444-555555555555";

/// A card id, shaped as the model's `UUID`
/// (`^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`).
const CARD: &str = "aaaaaaaa-1111-4111-8111-111111111111";

/// `UUID`-shaped ids nothing was ever minted with.
const UNKNOWN_APP: &str = "deadbeef-0000-4000-8000-000000000000";
const UNKNOWN_SESSION: &str = "deadbeef-1111-4111-8111-111111111111";
const UNKNOWN_ITEM: &str = "deadbeef-2222-4222-9222-222222222222";

/// The access key id of a caller the Identity Center portal signed in.
const FEDERATED: &str = "AKIDFEDERATED0001";

/// A plain long-term IAM access key: a credential the portal never issued.
const LONG_TERM: &str = "AKIDEXAMPLE";

/// The host a client addresses this service by, so a request presenting NO credential
/// at all still names the service (the routing key then comes from the host).
const QAPPS_HOST: &str = "qapps.us-east-1.amazonaws.com";

/// The IAM Identity Center portal endpoint, which its own operations are addressed by.
const PORTAL_HOST: &str = "portal.sso.us-east-1.amazonaws.com";

/// The model this service is generated from, read by the test that pins which
/// operations each front door covers against the model's own error lists.
const MODEL: &str = include_str!("../../smithy-model/com-amazonaws-qapps-qappsservice.json");

/// The emulator under test: the production router with this service and the Identity
/// Center portal mounted on ONE shared engine, which is how the assembled emulator
/// mounts them.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; the credential scope in the
        // header below still carries the access key id the front door resolves.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(PortalService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// One qapps request signed with `akid` (or presenting no credential at all when
    /// it is `None`), answered as `(status, body)`.
    async fn call(
        &self,
        method: &str,
        uri: &str,
        akid: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request"))
        });
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", QAPPS_HOST)
            .header("instance-id", INSTANCE)
            .header("content-type", "application/json");
        if let Some(akid) = akid {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/qapps/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            );
        }
        let response = self
            .router
            .dispatch(request.body(payload).expect("build the request"))
            .await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// The portal's store: the seam a sign-in and an administrator's assignments arrive
    /// through, since IAM Identity Center models no sign-in API.
    fn portal(&self) -> engine::ServiceState<portal::ComAmazonawsSsoSwbportalserviceState> {
        portal::service_state(&*self.engine).expect("the portal store is reachable")
    }

    /// `sso:GetRoleCredentials` over HTTP: the call an Identity Center user makes to
    /// get the role credentials it then signs AWS requests with.
    async fn role_credentials(&self, access_token: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!(
                "https://{PORTAL_HOST}/federation/credentials?account_id={ACCOUNT}&role_name={ROLE}"
            ))
            .header("host", PORTAL_HOST)
            .header("x-amz-sso_bearer_token", access_token)
            .body(bytes::Bytes::new())
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Create one Q App as the federated caller and answer `(appId, appArn)`.
    async fn create_app(&self, title: &str, tags: Value) -> (String, String) {
        let (status, body) = self
            .call(
                "POST",
                "/apps.create",
                Some(FEDERATED),
                Some(json!({
                    "title": title,
                    "appDefinition": { "cards": [text_card(CARD, "Topic")] },
                    "tags": tags,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {title:?}: expected CreateQApp to serve a federated caller, got \
             {status} {body:?}"
        );
        (
            text(&body, "appId").expect("CreateQApp reports the app id"),
            text(&body, "appArn").expect("CreateQApp reports the app ARN"),
        )
    }
}

/// A text-input card. `type` is the model's `CardType` value for the variant.
fn text_card(id: &str, title: &str) -> Value {
    json!({ "textInput": { "id": id, "title": title, "type": "text-input" } })
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> Option<String> {
    body.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The wire error code (`__type`) of a failure response.
fn error_code(body: &Value) -> String {
    text(body, "__type").unwrap_or_default()
}

/// The `message` an error envelope carries.
fn error_message(body: &Value) -> String {
    text(body, "message").unwrap_or_default()
}

/// An ARN inside one path segment, as a client puts it in the `@httpLabel` of
/// `/tags/{resourceARN}`: RFC 3986 percent-encoding over everything that is not
/// unreserved, so the ARN's own `:` and `/` cannot be read as path structure.
fn in_path(resource_arn: &str) -> String {
    utf8_percent_encode(resource_arn, NON_ALPHANUMERIC).to_string()
}

/// One request to the tagging surface for `resource_arn`.
fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", in_path(resource_arn))
}

/// Every operation this service's model BINDS, with whether it declares
/// `UnauthorizedException`.
///
/// Read out of the service's own Smithy model rather than listed here, so the split
/// below is the model's and cannot drift from it.
fn modelled_operations() -> Vec<(String, bool)> {
    let model: Value = serde_json::from_str(MODEL).expect("the Smithy model parses");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model carries shapes");
    let service = shapes
        .values()
        .find(|shape| shape["type"] == "service")
        .expect("the model carries its service shape");
    let bound: Vec<String> = service["operations"]
        .as_array()
        .expect("the service binds operations")
        .iter()
        .filter_map(|entry| entry["target"].as_str().map(str::to_string))
        .collect();
    bound
        .iter()
        .map(|shape_id| {
            let declares_401 = shapes[shape_id]["errors"]
                .as_array()
                .is_some_and(|errors| {
                    errors.iter().any(|error| {
                        error["target"] == "com.amazonaws.qapps#UnauthorizedException"
                    })
                });
            let name = shape_id
                .rsplit('#')
                .next()
                .unwrap_or(shape_id)
                .to_string();
            (name, declares_401)
        })
        .collect()
}

/// RULE: the operations behind the Identity Center door are exactly those whose model
/// declares `UnauthorizedException`; the rest — the resource-tagging surface — are the
/// ones the IAM door covers.
#[test]
fn the_two_doors_cover_what_the_model_says_they_cover() {
    let operations = modelled_operations();
    assert!(
        operations.len() > 30,
        "case \"the model's operations\": expected this service's whole surface, got {}",
        operations.len()
    );
    for (name, declares_401) in &operations {
        let iam_door = IAM_AUTHORIZED_OPERATIONS.contains(&name.as_str());
        assert_eq!(
            iam_door, !declares_401,
            "case {name:?}: an operation is behind the IAM door exactly when it declares \
             no UnauthorizedException, but iam_door={iam_door} and declares_401={declares_401}"
        );
    }
    for named in IAM_AUTHORIZED_OPERATIONS {
        assert!(
            operations.iter().any(|(name, _)| name == named),
            "case {named:?}: the IAM door names an operation the model does not bind"
        );
    }
}

/// The table of requests behind the Identity Center door: one row per way a request can
/// differ in what it would have earned had it been authorized — an unknown resource, a
/// resource that EXISTS, a payload the model refuses, an operation that is implemented,
/// and one that is still a scaffold.
///
/// `(case, method, uri, body)`, with `{app}` standing for the id of an app that exists.
fn door_covered_requests(app_id: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let over_the_grant_limit: Vec<Value> = (0..101)
        .map(|index| json!({ "action": "read", "principal": format!("user-{index}") }))
        .collect();
    vec![
        (
            "GetQApp on an app that does not exist",
            "GET",
            format!("/apps.get?appId={UNKNOWN_APP}"),
            None,
        ),
        (
            "GetQApp on an app that DOES exist",
            "GET",
            format!("/apps.get?appId={app_id}"),
            None,
        ),
        (
            "DeleteQApp on an app that DOES exist",
            "POST",
            "/apps.delete".to_string(),
            Some(json!({ "appId": app_id })),
        ),
        (
            "UpdateQApp on an app that DOES exist",
            "POST",
            "/apps.update".to_string(),
            Some(json!({ "appId": app_id, "title": "a new title" })),
        ),
        (
            "StartQAppSession on an app that DOES exist",
            "POST",
            "/runtime.startQAppSession".to_string(),
            Some(json!({ "appId": app_id, "appVersion": 1 })),
        ),
        (
            "ListQApps, which names no resource",
            "GET",
            "/apps.list".to_string(),
            None,
        ),
        (
            "CreateQApp with a payload the model accepts",
            "POST",
            "/apps.create".to_string(),
            Some(json!({
                "title": "a well-formed title",
                "appDefinition": { "cards": [text_card(CARD, "Topic")] },
            })),
        ),
        (
            "CreateQApp with a title outside the model's pattern",
            "POST",
            "/apps.create".to_string(),
            Some(json!({
                "title": "sim2real-{invalid}-title",
                "appDefinition": { "cards": [text_card(CARD, "Topic")] },
            })),
        ),
        (
            "CreateQApp with no title at all",
            "POST",
            "/apps.create".to_string(),
            Some(json!({ "appDefinition": { "cards": [text_card(CARD, "Topic")] } })),
        ),
        (
            "UpdateQAppPermissions past the grant limit",
            "POST",
            "/apps.updateQAppPermissions".to_string(),
            Some(json!({ "appId": app_id, "grantPermissions": over_the_grant_limit })),
        ),
        (
            "DescribeQAppPermissions, a scaffold",
            "GET",
            format!("/apps.describeQAppPermissions?appId={app_id}"),
            None,
        ),
        (
            "PredictQApp with no options, a scaffold",
            "POST",
            "/apps.predictQApp".to_string(),
            Some(json!({})),
        ),
        (
            "ListCategories, a scaffold",
            "GET",
            "/catalog.listCategories".to_string(),
            None,
        ),
        (
            "BatchCreateCategory, a scaffold",
            "POST",
            "/catalog.createCategories".to_string(),
            Some(json!({ "categories": [{ "title": "Finance" }] })),
        ),
        (
            "AssociateQAppWithUser, a scaffold",
            "POST",
            "/apps.install".to_string(),
            Some(json!({ "appId": app_id })),
        ),
        (
            "DisassociateQAppFromUser, a scaffold",
            "POST",
            "/apps.uninstall".to_string(),
            Some(json!({ "appId": app_id })),
        ),
        (
            "UpdateQAppSession on a session that does not exist, a scaffold",
            "POST",
            "/runtime.updateQAppSession".to_string(),
            Some(json!({ "sessionId": UNKNOWN_SESSION, "values": [] })),
        ),
        (
            "UpdateQAppSessionMetadata, a scaffold",
            "POST",
            "/runtime.updateQAppSessionMetadata".to_string(),
            Some(json!({
                "sessionId": UNKNOWN_SESSION,
                "sharingConfiguration": { "enabled": true },
            })),
        ),
        (
            "GetQAppSessionMetadata, a scaffold",
            "GET",
            format!("/runtime.getQAppSessionMetadata?sessionId={UNKNOWN_SESSION}"),
            None,
        ),
        (
            "CreateLibraryItem",
            "POST",
            "/catalog.createItem".to_string(),
            Some(json!({ "appId": app_id, "appVersion": 1, "categories": [] })),
        ),
        (
            "GetLibraryItem on an item that does not exist",
            "GET",
            format!("/catalog.getItem?libraryItemId={UNKNOWN_ITEM}"),
            None,
        ),
    ]
}

/// RULE: a caller with no Identity Center identity is refused 401
/// `UnauthorizedException` on every operation behind that door — before the store is
/// read, before the payload is validated and before a scaffold answers.
#[tokio::test]
async fn an_unfederated_caller_is_refused_before_anything_else_happens() {
    let emulator = Emulator::new();
    federate(&emulator.engine, FEDERATED);
    let (app_id, _) = emulator.create_app("the app that exists", json!({})).await;

    for (case, method, uri, body) in door_covered_requests(&app_id) {
        let (status, answer) = emulator
            .call(method, &uri, Some(LONG_TERM), body.clone())
            .await;
        assert_eq!(
            status, 401,
            "case {case:?}: expected the front door's 401, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "UnauthorizedException",
            "case {case:?}: expected UnauthorizedException, got {answer:?}"
        );
        // The recorded wording, written out here rather than read back from the crate's
        // own constant, so the sentence is pinned independently of the implementation.
        assert_eq!(
            error_message(&answer),
            "Unauthorized",
            "case {case:?}: expected the one gate message, got {answer:?}"
        );
        assert_eq!(
            UNAUTHORIZED_MESSAGE, "Unauthorized",
            "case {case:?}: expected the crate's gate message to be the recorded one"
        );
    }

    // Nothing behind the door ran for any of those requests: the app is untouched and
    // no second app was created.
    let (status, body) = emulator
        .call("GET", "/apps.list", Some(FEDERATED), None)
        .await;
    let apps = body["apps"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        (status, apps.len()),
        (200, 1),
        "case \"after every refusal\": expected the one app created to be the only one, \
         got {status} {body:?}"
    );
    assert_eq!(
        apps.first().and_then(|app| app["title"].as_str()),
        Some("the app that exists"),
        "case \"after every refusal\": expected the stored app unchanged, got {body:?}"
    );
}

/// RULE: what makes a caller federated is an Identity Center credential that is still
/// usable — not the shape of the access key id, and not any header the caller controls.
/// One row per class of credential a request can present.
#[tokio::test]
async fn only_an_identity_center_credential_is_served() {
    let emulator = Emulator::new();

    // A credential the portal issued, and one it issued whose own duration has run out.
    federate(&emulator.engine, FEDERATED);
    let lapsed = "AKIDLAPSED0001";
    let logged_out = "AKIDLOGGEDOUT0001";
    federate(&emulator.engine, logged_out);
    {
        let state = emulator.portal();
        let mut store = state.lock();
        store.put_account(ACCOUNT, "Emulator", "qapps@example.com");
        let access_token = access_token_for(lapsed);
        store.put_session(
            &access_token,
            "lapsed@example.com",
            [(ACCOUNT, [ROLE])],
            SESSION_TTL,
        );
        store.issued_credentials.insert(
            (access_token, ACCOUNT.to_string(), ROLE.to_string()),
            RoleCredentialRecord {
                access_key_id: lapsed.to_string(),
                secret_access_key: "secret".to_string(),
                session_token: "session-token".to_string(),
                // Dated before now, so the credential is already past its duration.
                expiration_millis: portal::epoch_millis(
                    SystemTime::now()
                        .checked_sub(Duration::from_secs(60))
                        .expect("a representable instant"),
                ),
            },
        );
        // `Logout` ends the sign-in but leaves the credentials it minted working for
        // the rest of their own duration.
        store.invalidate_session(&access_token_for(logged_out), SystemTime::now());
    }

    // (case, the credential presented, served)
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("a credential the Identity Center portal issued", Some(FEDERATED), true),
        (
            "a portal credential whose sign-in was logged out",
            Some(logged_out),
            true,
        ),
        ("a portal credential past its own duration", Some(lapsed), false),
        ("a long-term IAM access key", Some(LONG_TERM), false),
        ("an access key id the portal never issued", Some("AKIDSTRANGER0001"), false),
        ("an access key id shaped like the federated one", Some("AKIDFEDERATED0002"), false),
        ("no credential at all", None, false),
    ];

    for (case, credential, served) in rows {
        let (status, body) = emulator.call("GET", "/apps.list", *credential, None).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListQApps to be served, got {status} {body:?}"
            );
            assert!(
                body.get("apps").is_some(),
                "case {case:?}: expected the modelled ListQApps output, got {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (401, "UnauthorizedException".to_string()),
                "case {case:?}: expected the front door's 401, got {status} {body:?}"
            );
        }
    }
}

/// RULE: a user signed in to IAM Identity Center is served, through the credentials the
/// portal itself issues — and what it writes is readable back through `GetQApp`.
#[tokio::test]
async fn a_user_signed_in_through_the_portal_is_served() {
    let emulator = Emulator::new();
    // The sign-in and the administrator's assignment, which IAM Identity Center models
    // no API for.
    {
        let state = emulator.portal();
        let mut store = state.lock();
        store.put_account(ACCOUNT, "Emulator", "qapps@example.com");
        store.put_session(
            "portal-token",
            "analyst@example.com",
            [(ACCOUNT, [ROLE])],
            SESSION_TTL,
        );
    }

    let (status, credentials) = emulator.role_credentials("portal-token").await;
    assert_eq!(
        status, 200,
        "case \"GetRoleCredentials\": expected the portal to issue role credentials, \
         got {status} {credentials:?}"
    );
    let access_key_id = credentials["roleCredentials"]["accessKeyId"]
        .as_str()
        .expect("the portal reports an access key id")
        .to_string();

    let (status, created) = emulator
        .call(
            "POST",
            "/apps.create",
            Some(&access_key_id),
            Some(json!({
                "title": "signed in through the portal",
                "appDefinition": { "cards": [text_card(CARD, "Topic")] },
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"CreateQApp as the signed-in user\": expected it to be served, got \
         {status} {created:?}"
    );
    let app_id = text(&created, "appId").expect("CreateQApp reports the app id");

    let (status, read) = emulator
        .call(
            "GET",
            &format!("/apps.get?appId={app_id}"),
            Some(&access_key_id),
            None,
        )
        .await;
    assert_eq!(
        (status, text(&read, "title")),
        (200, Some("signed in through the portal".to_string())),
        "case \"GetQApp as the signed-in user\": expected the app just created, got \
         {status} {read:?}"
    );
}

/// RULE: the resource-tagging surface authorizes through the caller's IAM policy against
/// the ARN the request names — an ARN that account's identity-based policies cannot
/// grant (another account's, or a value that names no Q Apps resource) is refused 403
/// `AccessDeniedException` naming the caller and nothing about the resource, while an
/// ARN of this account's own resource is served. The refusal is NOT this service's 401:
/// these three operations declare none.
#[tokio::test]
async fn the_tagging_surface_authorizes_through_iam_against_the_arn() {
    let emulator = Emulator::new();
    federate(&emulator.engine, FEDERATED);
    let (app_id, app_arn) = emulator
        .create_app("tagged app", json!({ "owner": "analytics" }))
        .await;
    let (status, session) = emulator
        .call(
            "POST",
            "/runtime.startQAppSession",
            Some(FEDERATED),
            Some(json!({
                "appId": app_id,
                "appVersion": 1,
                "tags": { "run": "nightly" },
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"StartQAppSession\": expected a session, got {status} {session:?}"
    );
    let session_arn = text(&session, "sessionArn").expect("the session reports its ARN");
    let unknown_app_arn = app_arn.replace(&app_id, UNKNOWN_APP);

    // (case, the ARN the request names, the status, the error code, the tags)
    let rows: Vec<(&str, String, u16, &str, Value)> = vec![
        (
            "this account's own Q App",
            app_arn.clone(),
            200,
            "",
            json!({ "owner": "analytics" }),
        ),
        (
            "this account's own Q App session",
            session_arn.clone(),
            200,
            "",
            json!({ "run": "nightly" }),
        ),
        (
            "a Q App ARN of ANOTHER account",
            app_arn.replace(ACCOUNT, "111122223333"),
            403,
            "AccessDeniedException",
            Value::Null,
        ),
        (
            "a Q App session ARN of ANOTHER account",
            session_arn.replace(ACCOUNT, "111122223333"),
            403,
            "AccessDeniedException",
            Value::Null,
        ),
        (
            "another service's ARN in this account",
            format!("arn:aws:s3:::a-bucket/{app_id}"),
            403,
            "AccessDeniedException",
            Value::Null,
        ),
        (
            "a Q Apps ARN whose resource is of no modelled form",
            format!("arn:aws:qapps:us-east-1:{ACCOUNT}:widget/{app_id}"),
            403,
            "AccessDeniedException",
            Value::Null,
        ),
        (
            "a value that is no ARN at all",
            format!("application/{INSTANCE}/qapp/{app_id}"),
            403,
            "AccessDeniedException",
            Value::Null,
        ),
        (
            "this account's Q App that does not exist",
            unknown_app_arn,
            404,
            "ResourceNotFoundException",
            Value::Null,
        ),
        (
            "a session id read under the wrong Q App",
            session_arn.replace(&app_id, UNKNOWN_APP),
            404,
            "ResourceNotFoundException",
            Value::Null,
        ),
    ];

    for (case, resource_arn, expected_status, expected_code, expected_tags) in rows {
        let uri = tags_uri(&resource_arn);
        for credential in [Some(FEDERATED), Some(LONG_TERM), None] {
            // The SAME answer whatever Identity Center identity the request carries:
            // this surface is not behind that door, which is why none of these three
            // operations declares its 401. The caller named in a refusal is the
            // principal the presented credential speaks for.
            let (status, body) = emulator.call("GET", &uri, credential, None).await;
            assert_eq!(
                status, expected_status,
                "case {case:?} as {credential:?}: expected {expected_status}, got \
                 {status} {body:?}"
            );
            if expected_status == 200 {
                assert_eq!(
                    body["tags"], expected_tags,
                    "case {case:?} as {credential:?}: expected the tags the resource \
                     holds, got {body:?}"
                );
                continue;
            }
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} as {credential:?}: expected {expected_code}, got {body:?}"
            );
            if expected_code == "AccessDeniedException" {
                let message = error_message(&body);
                assert!(
                    message.starts_with("User: ")
                        && message.ends_with("is not authorized to access this resource"),
                    "case {case:?} as {credential:?}: expected the caller-named \
                     access-denied wording, got {message:?}"
                );
                assert!(
                    !message.contains(&resource_arn),
                    "case {case:?} as {credential:?}: expected the refusal to name no \
                     resource, got {message:?}"
                );
            }
        }
    }
}

/// RULE: the tags of a Q Apps resource are what `TagResource` left on it —
/// `ListTagsForResource` reports exactly that, `UntagResource` removes only the keys it
/// names, and both are idempotent. One row per way a tagging request can differ: how
/// many keys it carries, whether they are new or already held, and whether the keys
/// removed are held at all.
#[tokio::test]
async fn tags_survive_the_round_trip_through_the_tagging_surface() {
    let emulator = Emulator::new();
    federate(&emulator.engine, FEDERATED);
    let (_, app_arn) = emulator
        .create_app("round trip", json!({ "created-with": "yes" }))
        .await;
    let uri = tags_uri(&app_arn);

    // (case, the tags submitted, the keys removed, the tags held afterwards)
    let rows: Vec<(&str, Value, Vec<&str>, Value)> = vec![
        (
            "no tags submitted and none removed",
            json!({}),
            vec![],
            json!({ "created-with": "yes" }),
        ),
        (
            "one new key",
            json!({ "team": "analytics" }),
            vec![],
            json!({ "created-with": "yes", "team": "analytics" }),
        ),
        (
            "several new keys at once",
            json!({ "env": "staging", "cost-centre": "42" }),
            vec![],
            json!({
                "created-with": "yes",
                "team": "analytics",
                "env": "staging",
                "cost-centre": "42",
            }),
        ),
        (
            "a key the resource already holds, revised",
            json!({ "env": "production" }),
            vec![],
            json!({
                "created-with": "yes",
                "team": "analytics",
                "env": "production",
                "cost-centre": "42",
            }),
        ),
        (
            "a key removed",
            json!({}),
            vec!["cost-centre"],
            json!({ "created-with": "yes", "team": "analytics", "env": "production" }),
        ),
        (
            "the same key removed again, and one never held",
            json!({}),
            vec!["cost-centre", "never-held"],
            json!({ "created-with": "yes", "team": "analytics", "env": "production" }),
        ),
        (
            "a value the model's TagValue admits as empty",
            json!({ "note": "" }),
            vec![],
            json!({
                "created-with": "yes",
                "team": "analytics",
                "env": "production",
                "note": "",
            }),
        ),
        (
            "every key removed",
            json!({}),
            vec!["created-with", "team", "env", "note"],
            json!({}),
        ),
    ];

    for (case, submitted, removed, expected) in rows {
        if submitted.as_object().is_some_and(|tags| !tags.is_empty()) {
            let (status, body) = emulator
                .call("POST", &uri, Some(FEDERATED), Some(json!({ "tags": submitted })))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected TagResource to be served, got {status} {body:?}"
            );
        }
        if !removed.is_empty() {
            let keys = removed
                .iter()
                .map(|key| format!("tagKeys={key}"))
                .collect::<Vec<String>>()
                .join("&");
            let (status, body) = emulator
                .call("DELETE", &format!("{uri}?{keys}"), Some(FEDERATED), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UntagResource to be served, got {status} {body:?}"
            );
        }
        let (status, body) = emulator.call("GET", &uri, Some(FEDERATED), None).await;
        assert_eq!(
            (status, body["tags"].clone()),
            (200, expected.clone()),
            "case {case:?}: expected the tags left on the resource, got {status} {body:?}"
        );
    }
}

/// RULE: a resource holds at most `MAX_TAGS_PER_RESOURCE` tags, and a request that
/// would leave it with more is refused and changes nothing — while one that only
/// revises keys it already holds is not growth and is served.
#[tokio::test]
async fn a_resource_holds_no_more_tags_than_the_bound() {
    let emulator = Emulator::new();
    federate(&emulator.engine, FEDERATED);
    let (_, app_arn) = emulator.create_app("bounded tags", json!({})).await;
    let uri = tags_uri(&app_arn);
    let fill: Value = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), Value::from(format!("value-{index}"))))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let (status, body) = emulator
        .call("POST", &uri, Some(FEDERATED), Some(json!({ "tags": fill })))
        .await;
    assert_eq!(
        status, 200,
        "case \"the bound exactly\": expected {MAX_TAGS_PER_RESOURCE} tags to be \
         accepted, got {status} {body:?}"
    );

    // (case, the tags submitted, the status)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "one key past the bound",
            json!({ "one-too-many": "x" }),
            400,
        ),
        (
            "several keys past the bound",
            json!({ "a": "1", "b": "2" }),
            400,
        ),
        (
            "a key already held, revised",
            json!({ "key-0": "revised" }),
            200,
        ),
    ];
    for (case, submitted, expected_status) in rows {
        let (status, body) = emulator
            .call("POST", &uri, Some(FEDERATED), Some(json!({ "tags": submitted })))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            // AWS's own clause for a map over a length bound: the member, the bound and
            // the envelope, and no value (a map has no scalar rendering to quote).
            assert_eq!(
                error_message(&body),
                format!(
                    "1 validation error detected: Value at 'tags' failed to satisfy \
                     constraint: Member must have length less than or equal to \
                     {MAX_TAGS_PER_RESOURCE}"
                ),
                "case {case:?}: expected the constraint clause, got {body:?}"
            );
        }
    }

    let (status, body) = emulator.call("GET", &uri, Some(FEDERATED), None).await;
    let held = body["tags"].as_object().cloned().unwrap_or_default();
    assert_eq!(
        (status, held.len()),
        (200, MAX_TAGS_PER_RESOURCE),
        "case \"after the refusals\": expected the resource still at the bound, got \
         {status} {body:?}"
    );
    assert_eq!(
        held.get("key-0").and_then(Value::as_str),
        Some("revised"),
        "case \"after the refusals\": expected the revised value, got {body:?}"
    );
}
