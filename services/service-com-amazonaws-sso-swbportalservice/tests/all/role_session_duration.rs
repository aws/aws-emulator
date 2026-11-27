// SPDX-License-Identifier: CC0-1.0

//! The length of the role session the portal mints is the `SessionDuration` the
//! administrator configured on the permission set the role is the name of.
//!
//! RULE: a role session `GetRoleCredentials` creates through an IAM Identity Center
//! permission set runs for the duration configured in that permission set ("any existing
//! IAM role sessions that were created by using IAM Identity Center permission sets
//! continue based on the duration configured in the permission set", this service's own
//! `Logout` documentation), so the credential's advertised `expiration` is the instant it
//! was minted plus that configured span, whatever the span is; a permission set carrying no
//! `SessionDuration` — the member is optional — leaves the role session the permission-set
//! default of one hour ([`ROLE_SESSION_DURATION`]).
//!
//! The configured duration is another service's record, so this suite drives the whole
//! chain the way a running emulator does: IAM Identity Center's `CreatePermissionSet` /
//! `UpdatePermissionSet` configure it, `CreateAccountAssignment` grants the role, the OIDC
//! service's `CreateToken` mints the sign-in that spends it, and the portal answers. Every
//! call goes through `MultiServiceRouter::dispatch`, and nothing is written into the
//! portal's own store by the test.
//!
//! The rows span what the rule generalises over — the configured span itself: both ends of
//! the window IAM Identity Center admits, spans stated in hours, in minutes, in seconds and
//! in a combination of fields, and a permission set that configures none at all. The
//! administrator's LATER change is asserted too, because the rule is about the record and
//! not about the create that first wrote it, and a span past the published window is
//! refused where it would be stored, so no role session is ever configured outside it.
//!
//! Two set-up steps are not calls, because the emulator serves no API for them: the user's
//! APPROVAL of the device authorization, which IAM Identity Center takes at the
//! access-portal page, and the completion INSTANT of an assignment's asynchronous grant,
//! moved through IAM Identity Center's own state so the suite never sleeps — exactly as
//! `minted_sign_in.rs` does.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_identitystore_awsidentitystore as identity_store;
use service_com_amazonaws_sso_swbportalservice::state::ROLE_SESSION_DURATION;
use service_com_amazonaws_sso_swbportalservice::EmulatedService as PortalService;
use service_com_amazonaws_ssoadmin_swbexternalservice as identity_center;
use service_com_amazonaws_ssooidc_awsssooidcservice as oidc;

/// The portal endpoint an SDK addresses. Every portal operation carries an empty `@auth`,
/// so the request presents no SigV4 scope and the service claims it by this host.
const PORTAL_HOST: &str = "portal.sso.us-east-1.amazonaws.com";

/// The Region every call of this suite is scoped to.
const REGION: &str = "us-east-1";

/// The principal every assignment of this suite is made to: a `PrincipalId` of a form the
/// modelled pattern admits, written for this suite.
const PRINCIPAL_ID: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf7";

/// How far the advertised expiration may sit from the instant the call was made plus the
/// configured span. The spans the rows configure differ by half an hour at the closest, so
/// this is generous for a loaded host and still tells every row apart.
const EXPIRATION_SLACK: Duration = Duration::from_secs(5 * 60);

/// One permission set an administrator configures, and the role session its role must run
/// for.
struct Configured {
    /// What the row is about.
    case: &'static str,
    /// The permission set's `Name`, which is the role the portal reports (modelled
    /// `^[\w+=,.@-]+$`, length 1..32).
    role_name: &'static str,
    /// `SessionDuration`, "the length of time that the application user sessions are valid
    /// in the ISO-8601 standard" (`CreatePermissionSetRequest$SessionDuration`), or `None`
    /// for a permission set configured with none.
    session_duration: Option<&'static str>,
    /// The span the minted credential must advertise.
    expected: Duration,
}

/// The rows: both published ends of the window IAM Identity Center admits for a permission
/// set's session duration (1 hour to 12 hours), spans stated in each of the fields the
/// modelled `Duration` grammar carries, and a permission set that configures none — whose
/// role session is left the permission-set default this service holds as its one constant.
const CONFIGURED: &[Configured] = &[
    Configured {
        case: "the shortest session a permission set can hold",
        role_name: "OneHour",
        session_duration: Some("PT1H"),
        expected: Duration::from_secs(60 * 60),
    },
    Configured {
        case: "the longest session a permission set can hold",
        role_name: "TwelveHour",
        session_duration: Some("PT12H"),
        expected: Duration::from_secs(12 * 60 * 60),
    },
    Configured {
        case: "a span stated in minutes",
        role_name: "Ninety.Minutes",
        session_duration: Some("PT90M"),
        expected: Duration::from_secs(90 * 60),
    },
    Configured {
        case: "a span stated in hours and minutes",
        role_name: "Two-Half.Hours",
        session_duration: Some("PT2H30M"),
        expected: Duration::from_secs(150 * 60),
    },
    Configured {
        case: "a span stated in seconds",
        role_name: "FourHours",
        session_duration: Some("PT14400S"),
        expected: Duration::from_secs(4 * 60 * 60),
    },
    Configured {
        case: "a permission set that configures no session duration",
        role_name: "Unconfigured",
        session_duration: None,
        expected: ROLE_SESSION_DURATION,
    },
];

/// The services the production router mounts, on ONE shared engine: the portal, the token
/// mint, IAM Identity Center and its identity store.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator's business, not a service's; the signed calls below
        // carry a credential scope purely to name the service they route to.
        router.set_request_authentication(false);
        router.register_service(PortalService::new(Arc::clone(&engine)));
        router.register_service(oidc::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(identity_center::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(identity_store::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one IAM Identity Center (`awsJson1_1`) operation.
    async fn admin(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SWBExternalService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sso/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test built a well-formed request");
        parsed(self.router.dispatch(request).await)
    }

    /// Call one IAM Identity Center OIDC (`restJson1`) operation by its modelled path.
    async fn oidc(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("https://oidc.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/sso-oauth/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test built a well-formed request");
        parsed(self.router.dispatch(request).await)
    }

    /// Send one portal request, presenting `token` as the bearer credential the model binds
    /// the access token to.
    async fn portal(&self, method: Method, uri: &str, token: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("https://{PORTAL_HOST}{uri}"))
            .header("host", PORTAL_HOST)
            .header("x-amz-sso_bearer_token", token)
            .body(Bytes::new())
            .expect("the test built a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status();
        let (_, body) = parsed(response);
        (status, body)
    }

    /// The account's IAM Identity Center instance, read the way a client reads it, with
    /// permission sets enabled on it.
    async fn instance(&self) -> (String, String) {
        let (status, body) = self.admin("ListInstances", &json!({})).await;
        assert_eq!(status, 200, "setup: ListInstances answered {status} {body}");
        let instance = &body["Instances"][0];
        let instance_arn = text(instance, "InstanceArn");
        let identity_store_id = text(instance, "IdentityStoreId");
        let (status, body) = self
            .admin(
                "UpdateInstance",
                &json!({"InstanceArn": instance_arn, "PermissionSetsEnabled": true}),
            )
            .await;
        assert_eq!(status, 200, "setup: UpdateInstance answered {status} {body}");
        (instance_arn, identity_store_id)
    }

    /// Create one permission set, with `session_duration` when the row configures one, and
    /// answer the ARN the create minted.
    async fn permission_set(
        &self,
        instance_arn: &str,
        name: &str,
        session_duration: Option<&str>,
    ) -> String {
        let mut request = json!({"InstanceArn": instance_arn, "Name": name});
        if let Some(session_duration) = session_duration {
            request["SessionDuration"] = json!(session_duration);
        }
        let (status, body) = self.admin("CreatePermissionSet", &request).await;
        assert_eq!(
            status, 200,
            "setup: CreatePermissionSet({name}) answered {status} {body}"
        );
        let created = &body["PermissionSet"];
        assert_eq!(
            created["SessionDuration"].as_str(),
            session_duration,
            "setup: the permission set {name} must report the session duration it was \
             created with, got {created}"
        );
        text(created, "PermissionSetArn")
    }

    /// Grant one account assignment, the way an administrator does, and complete its grant.
    async fn grant(&self, instance_arn: &str, account_id: &str, permission_set_arn: &str) {
        let (status, body) = self
            .admin(
                "CreateAccountAssignment",
                &json!({
                    "InstanceArn": instance_arn,
                    "TargetId": account_id,
                    "TargetType": "AWS_ACCOUNT",
                    "PermissionSetArn": permission_set_arn,
                    "PrincipalType": "USER",
                    "PrincipalId": PRINCIPAL_ID,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: CreateAccountAssignment answered {status} {body}"
        );
        self.grants_complete();
    }

    /// Put the completion of every assignment grant made so far in the past.
    ///
    /// Granting an assignment is asynchronous: the call is accepted `IN_PROGRESS` and the
    /// grant completes when the clock makes it due. Moving that INSTANT is the clock's
    /// business, not the wire's, so the suite observes the completed grant without
    /// sleeping; the status is still derived by IAM Identity Center from the instant it
    /// holds.
    fn grants_complete(&self) {
        let state = identity_center::state::service_state(&*self.engine)
            .expect("the IAM Identity Center store is reachable");
        let mut state = state.lock();
        let due_at = SystemTime::now() - Duration::from_secs(1);
        for operation in state.assignment_operations.values_mut() {
            operation.succeeded_due = due_at;
        }
    }

    /// An access token the emulator's OWN OIDC service issued, through the device flow a
    /// client (`aws sso login`) walks: register, start the authorization, have it approved
    /// at the access portal, then exchange the device code.
    async fn minted_token(&self, identity_store_id: &str) -> String {
        let (status, body) = self
            .oidc(
                "/client/register",
                &json!({"clientName": "emulator-test", "clientType": "public"}),
            )
            .await;
        assert_eq!(status, 200, "setup: RegisterClient answered {status} {body}");
        let client_id = text(&body, "clientId");
        let client_secret = text(&body, "clientSecret");

        let (status, body) = self
            .oidc(
                "/device_authorization",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "startUrl": oidc::state::access_portal_url(identity_store_id),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: StartDeviceAuthorization answered {status} {body}"
        );
        let device_code = text(&body, "deviceCode");
        let user_code = text(&body, "userCode");

        // The user's approval at the verification page, which is no operation of the OIDC
        // API: recorded through that service's own state, as its own suites do.
        let approved = oidc::state::service_state(&*self.engine)
            .expect("the OIDC store is reachable")
            .lock()
            .approve_user_code(&user_code, SystemTime::now());
        assert!(approved, "setup: the device authorization must be approvable");

        let (status, body) = self
            .oidc(
                "/token",
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "grantType": oidc::state::GRANT_TYPE_DEVICE_CODE,
                    "deviceCode": device_code,
                }),
            )
            .await;
        assert_eq!(status, 200, "setup: CreateToken answered {status} {body}");
        text(&body, "accessToken")
    }

    /// The roles the portal reports for `account_id`, in the order it reported them.
    async fn reported_roles(&self, token: &str, account_id: &str) -> Vec<String> {
        let (status, body) = self
            .portal(
                Method::GET,
                &format!("/assignment/roles?account_id={account_id}"),
                token,
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "ListAccountRoles answered {status} {body}"
        );
        body["roleList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|role| role["roleName"].as_str().map(str::to_string))
            .collect()
    }

    /// `GetRoleCredentials` for one (account, role), with the instants the call was made
    /// between — what the advertised expiration is judged against.
    async fn role_credentials(
        &self,
        token: &str,
        account_id: &str,
        role_name: &str,
    ) -> (StatusCode, Value, SystemTime, SystemTime) {
        let before = SystemTime::now();
        let (status, body) = self
            .portal(
                Method::GET,
                &format!("/federation/credentials?account_id={account_id}&role_name={role_name}"),
                token,
            )
            .await;
        let after = SystemTime::now();
        (status, body, before, after)
    }
}

/// The status and parsed body of a response; a body that is not JSON fails the case rather
/// than being swallowed.
fn parsed(response: Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "the response body must be JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, body)
}

/// One required string member of a response.
fn text(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is missing from {body}"))
        .to_string()
}

/// `at` as the epoch milliseconds `RoleCredentials$expiration` is modelled in.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .expect("an instant at or after the epoch")
            .as_millis(),
    )
    .expect("an instant a long can hold")
}

/// Assert that `body` advertises a credential expiring `expected` after the call that was
/// made between `before` and `after`.
fn assert_expires_after(
    case: &str,
    body: &Value,
    expected: Duration,
    before: SystemTime,
    after: SystemTime,
) {
    let credentials = &body["roleCredentials"];
    let expiration = credentials["expiration"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a modelled expiration, got {credentials}"));
    let earliest = epoch_millis(before + expected - EXPIRATION_SLACK);
    let latest = epoch_millis(after + expected + EXPIRATION_SLACK);
    assert!(
        (earliest..=latest).contains(&expiration),
        "case {case:?}: expected an expiration {expected:?} after the call, so in \
         {earliest}..={latest} epoch ms, got {expiration}"
    );
    for member in ["accessKeyId", "secretAccessKey", "sessionToken"] {
        assert!(
            credentials[member]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected a minted {member}, got {credentials}"
        );
    }
}

/// Every configured span reaches the credential the portal mints, and a permission set that
/// configures none leaves the role session the permission-set default.
#[tokio::test]
async fn a_minted_role_credential_runs_for_the_duration_its_permission_set_configures() {
    let emulator = Emulator::new();
    let (instance_arn, identity_store_id) = emulator.instance().await;
    let account_id = engine::emulator_account_id().to_string();

    for row in CONFIGURED {
        let permission_set_arn = emulator
            .permission_set(&instance_arn, row.role_name, row.session_duration)
            .await;
        emulator
            .grant(&instance_arn, &account_id, &permission_set_arn)
            .await;
    }

    let token = emulator.minted_token(&identity_store_id).await;

    // The roles the portal offers are those permission sets' names: the records the
    // durations below are read off are the records the listing is built from.
    let mut reported = emulator.reported_roles(&token, &account_id).await;
    reported.sort();
    let mut expected_roles: Vec<String> = CONFIGURED
        .iter()
        .map(|row| row.role_name.to_string())
        .collect();
    expected_roles.sort();
    assert_eq!(
        reported, expected_roles,
        "case \"the assigned roles\": expected {expected_roles:?}, got {reported:?}"
    );

    for row in CONFIGURED {
        let (status, body, before, after) = emulator
            .role_credentials(&token, &account_id, row.role_name)
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: GetRoleCredentials answered {status} {body}",
            row.case
        );
        assert_expires_after(row.case, &body, row.expected, before, after);
    }
}

/// The rule is about the permission set's record, not about the create that first wrote it:
/// a duration an administrator configures LATER is the duration the next role session of
/// that role runs for.
#[tokio::test]
async fn a_later_configured_duration_reaches_the_next_role_session() {
    /// What the administrator changes the session duration to, and the span that change
    /// must give the next role session.
    const CHANGES: &[(&str, &str, Duration)] = &[
        (
            "configuring a duration on a permission set that had none",
            "PT8H",
            Duration::from_secs(8 * 60 * 60),
        ),
        (
            "shortening it again",
            "PT1H30M",
            Duration::from_secs(90 * 60),
        ),
    ];

    let emulator = Emulator::new();
    let (instance_arn, identity_store_id) = emulator.instance().await;
    let account_id = engine::emulator_account_id().to_string();
    let role_name = "Reconfigured";
    let permission_set_arn = emulator
        .permission_set(&instance_arn, role_name, None)
        .await;
    emulator
        .grant(&instance_arn, &account_id, &permission_set_arn)
        .await;

    // Before any change, the role session is the default an unconfigured permission set
    // leaves.
    let first_token = emulator.minted_token(&identity_store_id).await;
    let (status, body, before, after) = emulator
        .role_credentials(&first_token, &account_id, role_name)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "case \"before any change\": GetRoleCredentials answered {status} {body}"
    );
    assert_expires_after(
        "before any change",
        &body,
        ROLE_SESSION_DURATION,
        before,
        after,
    );

    for (case, configured, expected) in CHANGES {
        let (status, body) = emulator
            .admin(
                "UpdatePermissionSet",
                &json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "SessionDuration": configured,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdatePermissionSet answered {status} {body}"
        );

        // A sign-in of its own, because a credential already minted for a sign-in is the
        // one that sign-in keeps being handed until it runs out.
        let token = emulator.minted_token(&identity_store_id).await;
        let (status, body, before, after) = emulator
            .role_credentials(&token, &account_id, role_name)
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: GetRoleCredentials answered {status} {body}"
        );
        assert_expires_after(case, &body, *expected, before, after);
    }
}

/// A span outside the window IAM Identity Center publishes for a permission set (one hour
/// to twelve hours) is refused where it would be stored, so no role session is ever
/// configured outside it: one step past either published bound earns a refusal (HTTP 400)
/// and mints no permission set.
#[tokio::test]
async fn a_session_duration_past_the_published_window_is_never_stored() {
    /// One value past a published bound, and the name a create would have minted.
    const OUTSIDE: &[(&str, &str)] = &[
        ("PT59M", "JustUnderTheMinimum"),
        ("PT12H1M", "JustOverTheMaximum"),
        ("PT13H", "WellOverTheMaximum"),
    ];

    let emulator = Emulator::new();
    let (instance_arn, _) = emulator.instance().await;

    for (session_duration, name) in OUTSIDE {
        let (status, body) = emulator
            .admin(
                "CreatePermissionSet",
                &json!({
                    "InstanceArn": instance_arn,
                    "Name": name,
                    "SessionDuration": session_duration,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {session_duration:?}: expected CreatePermissionSet to be refused 400, \
             got {status} {body}"
        );
        let (_, listed) = emulator
            .admin("ListPermissionSets", &json!({"InstanceArn": instance_arn}))
            .await;
        let names = listed["PermissionSets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            names.is_empty(),
            "case {session_duration:?}: a refused create must mint no permission set, \
             got {names:?}"
        );
    }
}
