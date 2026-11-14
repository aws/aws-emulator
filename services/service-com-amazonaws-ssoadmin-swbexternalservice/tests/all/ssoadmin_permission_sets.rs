// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the permission-set and account-assignment slice, driven over
//! the wire the service actually speaks.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] — the one path a socket-
//! served request takes — so the modeled constraints, the `awsJson1_1` envelope and
//! the `X-Amz-Target` routing are all exercised, not bypassed.
//!
//! The rules under test are stated once each, and each test asserts its rule over a
//! TABLE of inputs rather than over the single example the bootstrap issue cited:
//!
//! 1. a create stores a record under a server-minted ARN, and every read of that
//!    resource answers from the stored record until a delete removes it;
//! 2. a paginated read enumerates exactly the stored set, once per item, for ANY page
//!    size the model admits;
//! 3. an operation whose target resource does not exist answers
//!    `ResourceNotFoundException` — never a 200;
//! 4. an assignment's identity is its (instance, account, permission set, principal
//!    type, principal id) tuple, so creating it twice grants it once.
//!
//! The permission-set / account-assignment surface is served on an ORGANIZATION instance
//! of IAM Identity Center, which is enabled through AWS Organizations rather than through
//! this API. The tests of those rules run against an organization instance held under an
//! id of its own ([`organization_router`]), so that "under the instance the request names"
//! is not read off the only instance that exists; the account-instance restriction that
//! withdraws the same surface is `account_instance_restriction.rs`, and the kind of
//! instance an account DISCOVERS is `discovered_instance_surface.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The seeds a PROPERTY-based suite of this crate draws its populations from: the fixed
/// set the suite owns, so the case count is bounded and every run draws the same
/// populations, or the single seed `override_var` names for an exploratory run.
///
/// Shared by the drawn suites (`listed_instance_state_generated_accounts.rs`,
/// `application_assignment_repeat_grant.rs`), so one reading of a seed override serves
/// every one of them.
pub(crate) fn drawn_seeds(override_var: &str, fixed: &[u64]) -> Vec<u64> {
    match ::std::env::var(override_var) {
        Ok(named) => vec![named.trim().parse::<u64>().unwrap_or_else(|error| {
            panic!("{override_var}={named:?} is not a u64 seed: {error}")
        })],
        Err(_) => fixed.to_vec(),
    }
}

/// A pattern-valid `InstanceArn` for an instance that was never created, used to pin
/// the not-found rule. Shaped by the model's `InstanceArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
pub(crate) const UNKNOWN_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-0000000000000000";

/// A pattern-valid `PermissionSetArn` that was never minted. Shaped by the model's
/// `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
pub(crate) const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000";

/// The 12-digit `TargetId` most rows use (`^\d{12}$`).
const ACCOUNT_A: &str = "123456789012";

/// A one-service router over a fresh engine: the same assembly the production router
/// performs, minus the other services.
pub(crate) fn router() -> MultiServiceRouter {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// Read or write this service's store the way a handler does.
///
/// For a test that has to move a stored DUE INSTANT — the clock's business, not the wire's
/// — or set up account state no operation of this API can reach; everything a test then
/// asserts goes over the wire.
pub(crate) fn with_store<R>(
    engine: &Arc<engine::InMemoryEngine>,
    change: impl FnOnce(
        &mut service_com_amazonaws_ssoadmin_swbexternalservice::state::ComAmazonawsSsoadminSwbexternalserviceState,
    ) -> R,
) -> R {
    let service_state =
        service_com_amazonaws_ssoadmin_swbexternalservice::state::service_state(&**engine)
            .expect("the SSO Admin state store");
    let mut store = service_state.lock();
    change(&mut store)
}

/// Move the stored deletion instant of every instance whose removal a `DeleteInstance`
/// owes to `due_at`, so both sides of that instant can be observed without the test
/// sleeping.
///
/// The transition itself is still applied by the service's own `materialize_due`, exactly
/// as in production: this moves only the CLOCK's instant, which is the clock's business
/// and not the wire's.
pub(crate) fn hold_deletion(engine: &Arc<engine::InMemoryEngine>, due_at: std::time::SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            if instance.deleted_due.is_some() {
                instance.deleted_due = Some(due_at);
            }
        }
    });
}

/// Put the activation of one Region of `instance_arn` on `due_at` — and back into the
/// `ADDING` its `AddRegion` left it in, through the state's own transition, so only the due
/// INSTANT is the test's doing.
///
/// The shared set-up of the suites whose subject is WHICH INSTANT a call decides and stamps
/// from: an instance carrying a transition the clock owes it is what makes that instant
/// observable.
pub(crate) fn hold_region_activation(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
    due_at: std::time::SystemTime,
) {
    with_store(engine, |store| {
        let region = store
            .instance_mut(instance_arn)
            .expect("the instance")
            .region_mut(region_name)
            .expect("the added Region");
        region.start_addition();
        region.active_due = Some(due_at);
    });
}

/// Whether the call that just ran found that Region's activation already owed: the workflow
/// has been applied, so the record is `ACTIVE` with nothing left owing.
pub(crate) fn region_activated(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
) -> bool {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state::STATUS_ACTIVE;

    with_store(engine, |store| {
        let region = store
            .instance(instance_arn)
            .expect("the instance")
            .region(region_name)
            .expect("the added Region");
        region.status == STATUS_ACTIVE && region.active_due.is_none()
    })
}

/// The same router, over an engine the caller already holds (so a test can seed state
/// through [`service_com_amazonaws_ssoadmin_swbexternalservice::state`] the way the
/// service's own `Default` does).
pub(crate) fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's business, not this service's; this test drives the
    // wire format, so authentication is off and the request carries an unsigned scope
    // purely to name the service it routes to.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssoadmin_swbexternalservice::EmulatedService::new(
        engine,
    ));
    router
}

/// A router over an account that holds NO instance — the account state `CreateInstance`
/// is documented for, and the only state in which an instance can be enabled with tags.
///
/// That is the account state a completed `DeleteInstance` leaves behind: the account's
/// ambient instance has been enabled once and is now gone, so no later request enables
/// another. The delete path itself is driven over the wire in `instance_delete.rs`; this
/// helper sets the same two fields directly so a create can be tested without waiting a
/// deletion out, and everything a test then asserts goes over the wire.
pub(crate) fn empty_account_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
    });
    router_over(engine)
}

/// The instance id the organization instance below is held under: `ssoins-` + 16 of
/// `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const ORGANIZATION_INSTANCE_ID: &str = "ssoins-0a1b2c3d4e5f6071";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const ORGANIZATION_IDENTITY_STORE_ID: &str = "d-1a2b3c4d5e";

/// A router whose account holds an ORGANIZATION instance under an id of its own, and that
/// instance's ARN.
///
/// The account's ambient instance is an ACCOUNT instance, so this fixture is how a test
/// reaches the surface an organization instance serves — under an id of its own, so a rule
/// asserted against it cannot be satisfied by reading the one id the emulator seeds. An
/// organization instance is enabled through AWS Organizations rather than through this API
/// (`CreateInstance` creates the account instance of a standalone account), so the test
/// seeds it through the SAME `state::Instance` constructor the handlers use, and the calls
/// below all go over the wire.
pub(crate) fn organization_router() -> (MultiServiceRouter, String) {
    let (engine, instance_arn) = organization_engine();
    (router_over(engine), instance_arn)
}

/// The ENGINE behind [`organization_router`], for a test that has to read or write the
/// store directly (to move a stored due instant, say) as well as call over the wire.
pub(crate) fn organization_engine() -> (Arc<engine::InMemoryEngine>, String) {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state;

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let instance_arn = {
        let service_state = state::service_state(&*engine).expect("the SSO Admin state store");
        let mut store = service_state.lock();
        let instance = state::Instance::active(
            ORGANIZATION_INSTANCE_ID,
            ORGANIZATION_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("Organization".to_string()),
            aws_smithy_types::DateTime::from(std::time::SystemTime::now()),
            // Enabled in the Region every call of this harness signs for, so the
            // organization instance reports the Region it was enabled in like any other.
            TEST_REGION,
        );
        let instance_arn = instance.instance_arn.clone();
        // One instance per account: this organization instance is the account's instance,
        // so inserting it here is what the account holds — and because the store already
        // holds an instance, no request enables the ambient instance beside it.
        store.instances.clear();
        store.instances.insert(instance_arn.clone(), instance);
        store.ambient_instance_enabled = true;
        instance_arn
    };
    (engine, instance_arn)
}

/// Make the instance `instance_arn` names the ORGANIZATION instance of an AWS
/// Organizations organization, for a test whose subject is a Region WRITE.
///
/// Replicating an IAM Identity Center instance across Regions is a feature of the
/// organization instance, so `AddRegion` and `RemoveRegion` are withdrawn on an account
/// instance — the refusal `account_instance_region_writes.rs` pins. An organization
/// instance is enabled through AWS Organizations rather than through this API, so a test
/// that drives a Region write reaches one by seeding the instance's KIND, exactly as
/// [`organization_engine`] seeds a whole organization instance.
///
/// Only the kind and the permission-set enablement that comes with it change: the Regions
/// the instance holds, its status, its primary Region and its tags are whatever the calls
/// before this left, and everything the test then asserts goes over the wire.
pub(crate) fn as_organization_instance(engine: &Arc<engine::InMemoryEngine>, instance_arn: &str) {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state;

    with_store(engine, |store| {
        let instance = store
            .instance_mut(instance_arn)
            .unwrap_or_else(|| panic!("no instance is held under {instance_arn}"));
        instance.instance_type = state::InstanceType::Organization;
        instance.permission_sets_enabled = true;
    });
}

/// A router whose account holds an ACCOUNT instance — the instance of a single
/// standalone account — and that instance's ARN.
///
/// Built entirely over the WIRE: a fresh store has decided nothing about the account's
/// IAM Identity Center enablement, and `CreateInstance` is the call that decides it, as
/// "an instance of IAM Identity Center for a standalone Amazon Web Services account".
/// Its activation instant is then moved into the past through the activation helper
/// `instance_create.rs` owns — the clock's business, not the wire's, exactly as
/// [`hold_deletion`] moves a deletion instant — so the instance the test calls against is
/// the ACTIVE instance a caller sees once creation has finished.
pub(crate) async fn account_instance_router() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let (status, body) = call(
        &router,
        "CreateInstance",
        &json!({"Name": "StandaloneAccount"}),
    )
    .await;
    assert_eq!(status, 200, "CreateInstance expected 200, got {status} {body}");
    let instance_arn = body["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInstance minted no InstanceArn: {body}"))
        .to_string();
    crate::instance_create::hold_activation(
        &engine,
        std::time::SystemTime::now() - std::time::Duration::from_secs(60),
    );
    (router, instance_arn)
}

/// The Region every call of this harness signs for unless a test names another one: the
/// service reads it off the request's SigV4 credential scope
/// (`state::request_region`), so it is the Region the account's instance is enabled in.
pub(crate) const TEST_REGION: &str = "us-east-1";

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, TEST_REGION, operation, body).await
}

/// The same call, signed for `region` — the Region an instance this call enables or reads
/// is enabled in.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_raw(
        router,
        region,
        operation,
        &serde_json::to_string(body).expect("the request body serializes"),
    )
    .await
}

/// The same call, signed for `region` and asserted SERVED, as the parsed 200 body.
///
/// The shared form of the "make this call and require the 200" step every suite that scripts
/// a sequence of calls needs, so a new suite names the region it drives rather than carrying
/// its own copy of the assertion.
pub(crate) async fn served_in_region(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) = call_in_region(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} {answer}"
    );
    answer
}

/// The same call, over a body given as the exact BYTES to send.
///
/// For a test whose subject is something a `serde_json::Value` cannot express, because a
/// parsed `Value` holds its members in a sorted map: the ORDER a request carries its
/// members in, or a member whose type the model does not declare.
pub(crate) async fn call_raw(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("SWBExternalService.{operation}"))
        // The SigV4 credential scope is what routes a request to its service
        // (`resolve_route_key`); `sso` is this service's signing name, and the Region
        // segment is the one the service reads the request's Region from.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/sso/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response: `__type` on the awsJson protocols, with
/// any `namespace#` prefix stripped.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert one call was REFUSED: HTTP 400 carrying `expected` as its wire error code.
///
/// The shared refusal assertion of this crate's suites: `case` names the table row,
/// `what` the call that was refused, so a failure reads as the row and the actual answer
/// rather than as a bare `400 != 200`.
pub(crate) fn assert_refused(case: &str, what: &str, expected: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: {what} must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        expected,
        "case {case:?}: {what} expected {expected}, got {body}"
    );
}

/// The ARN of the one seeded instance, read the way a client reads it.
pub(crate) async fn seeded_instance_arn(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances: expected 200, got {status} {body}");
    body["Instances"][0]["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("ListInstances returned no seeded instance: {body}"))
        .to_string()
}

/// Grant ONE direct application assignment, the way a client does.
///
/// Shared by every suite of the application-assignment slice, so they all send the same
/// request for the (application, principal type, principal id) tuple that identifies an
/// assignment.
pub(crate) async fn create_application_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "CreateApplicationAssignment",
        &json!({
            "ApplicationArn": application_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await
}

/// Describe one direct application assignment, signed for `region`.
///
/// An `ApplicationArn` carries no Region segment, so the Region is a property of the CALL
/// rather than of the assignment: a suite that asserts the report is the same from
/// anywhere names the Region here.
pub(crate) async fn describe_application_assignment_in_region(
    router: &MultiServiceRouter,
    region: &str,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DescribeApplicationAssignment",
        &json!({
            "ApplicationArn": application_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await
}

/// The same describe, signed for [`TEST_REGION`] like every other call of this harness.
pub(crate) async fn describe_application_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    describe_application_assignment_in_region(
        router,
        TEST_REGION,
        application_arn,
        principal_type,
        principal_id,
    )
    .await
}

/// Whether `arn` has the shape the model's `InstanceArn` pattern requires:
/// `arn:aws:sso:::instance/` then `ins-` or `ssoins-` then 16 of `[a-zA-Z0-9-.]`.
pub(crate) fn is_instance_arn(arn: &str) -> bool {
    let Some(id) = arn.strip_prefix("arn:aws:sso:::instance/") else {
        return false;
    };
    let id = id.strip_prefix("sso").unwrap_or(id);
    let Some(suffix) = id.strip_prefix("ins-") else {
        return false;
    };
    suffix.len() == 16
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Whether `arn` has the shape the model's `PermissionSetArn` pattern requires, under
/// the instance `instance_arn` names: `ps-` then 16 of `[a-zA-Z0-9-./]`.
fn is_permission_set_arn_of(arn: &str, instance_arn: &str) -> bool {
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default();
    let Some(suffix) =
        arn.strip_prefix(&format!("arn:aws:sso:::permissionSet/{instance_id}/ps-"))
    else {
        return false;
    };
    suffix.len() == 16
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '/')
}

/// Whether `id` is a hyphenated lowercase UUID, which is what the model's `UUId` shape
/// (`length 36..36`, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
/// requires of a `RequestId`.
pub(crate) fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// One row of the permission-set table: a `CreatePermissionSet` input and a label.
struct PermissionSetCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    session_duration: Option<&'static str>,
    relay_state: Option<&'static str>,
    tags: Option<Value>,
}

/// The table the create/read/delete rule is asserted over: the rows differ along every
/// axis the rule generalises over — which optional members are present, the length of
/// `Name` at both published bounds (1..32), its character class (`^[\w+=,.@-]+$`), the
/// length of `Description` at its upper bound (1..700), and the ISO-8601 forms the
/// `Duration` pattern admits INSIDE the session-duration window the service serves
/// (`permission_set_session_duration.rs` owns that window and the refusal outside it).
fn permission_set_cases() -> Vec<PermissionSetCase> {
    vec![
        PermissionSetCase {
            label: "only the required members",
            name: "MinimalSet".to_string(),
            description: None,
            session_duration: None,
            relay_state: None,
            tags: None,
        },
        PermissionSetCase {
            label: "every optional member set",
            name: "FullSet".to_string(),
            description: Some("a demo permission set".to_string()),
            session_duration: Some("PT1H"),
            relay_state: Some("https://example.com/start"),
            tags: Some(json!([{"Key": "Owner", "Value": "platform"}])),
        },
        PermissionSetCase {
            // Name at its lower length bound.
            label: "Name of one character",
            name: "a".to_string(),
            description: Some("x".to_string()),
            session_duration: Some("PT3600S"),
            relay_state: None,
            tags: None,
        },
        PermissionSetCase {
            // Name at its upper length bound, using the full `^[\w+=,.@-]+$` class.
            label: "Name of 32 characters, full charset",
            name: format!("{}+=,.@-_", "n".repeat(25)),
            description: Some("d".repeat(700)),
            session_duration: Some("PT4H5M6S"),
            relay_state: Some("s3://bucket/key?x=1"),
            tags: Some(json!([
                {"Key": "Owner", "Value": "platform"},
                {"Key": "Tier", "Value": "test"}
            ])),
        },
        PermissionSetCase {
            label: "a session duration carrying a day component",
            name: "DailySet".to_string(),
            description: None,
            session_duration: Some("P0DT12H"),
            relay_state: None,
            tags: None,
        },
    ]
}

impl PermissionSetCase {
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({"InstanceArn": instance_arn, "Name": self.name});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(duration) = self.session_duration {
            body["SessionDuration"] = json!(duration);
        }
        if let Some(relay_state) = self.relay_state {
            body["RelayState"] = json!(relay_state);
        }
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        body
    }
}

/// RULE 1 — a create stores the record under a server-minted ARN and every later read
/// answers from that record, for every input shape the model admits.
#[tokio::test]
async fn create_stores_a_permission_set_that_every_reader_answers_from() {
    let (router, instance_arn) = organization_router();

    for case in permission_set_cases() {
        let (status, created) =
            call(&router, "CreatePermissionSet", &case.request(&instance_arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreatePermissionSet expected 200, got {status} {created}",
            case.label
        );
        let permission_set = &created["PermissionSet"];
        let arn = permission_set["PermissionSetArn"].as_str().unwrap_or_default();
        assert!(
            is_permission_set_arn_of(arn, &instance_arn),
            "case {:?}: expected a minted PermissionSetArn under {instance_arn}, got {arn:?}",
            case.label
        );
        assert!(
            permission_set["CreatedDate"].is_number(),
            "case {:?}: expected a server-set CreatedDate, got {:?}",
            case.label,
            permission_set["CreatedDate"]
        );
        assert_eq!(
            permission_set["Name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: expected Name {:?}, got {:?}",
            case.label,
            case.name,
            permission_set["Name"]
        );
        assert_eq!(
            permission_set["Description"].as_str(),
            case.description.as_deref(),
            "case {:?}: expected Description {:?}, got {:?}",
            case.label,
            case.description,
            permission_set["Description"]
        );
        assert_eq!(
            permission_set["SessionDuration"].as_str(),
            case.session_duration,
            "case {:?}: expected SessionDuration {:?}, got {:?}",
            case.label,
            case.session_duration,
            permission_set["SessionDuration"]
        );
        assert_eq!(
            permission_set["RelayState"].as_str(),
            case.relay_state,
            "case {:?}: expected RelayState {:?}, got {:?}",
            case.label,
            case.relay_state,
            permission_set["RelayState"]
        );

        // The round trip: the describe answers from the stored record, so it matches
        // the create field for field — including the two members the server generated.
        let (status, described) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePermissionSet expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["PermissionSet"], *permission_set,
            "case {:?}: describe must return the stored record, got {:?}",
            case.label, described["PermissionSet"]
        );
    }
}

/// RULE 1, continued — a minted ARN is unique per create, so two permission sets with
/// different names never share one.
#[tokio::test]
async fn each_create_mints_its_own_permission_set_arn() {
    let (router, instance_arn) = organization_router();

    let mut arns = Vec::new();
    for case in permission_set_cases() {
        let (_, created) = call(&router, "CreatePermissionSet", &case.request(&instance_arn)).await;
        arns.push(
            created["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let mut unique = arns.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        arns.len(),
        "expected one distinct ARN per create, got {arns:?}"
    );
}

/// RULE 1 — a duplicate `Name` within one instance conflicts, whatever the rest of the
/// request looks like.
#[tokio::test]
async fn a_second_permission_set_with_the_same_name_conflicts() {
    let (router, instance_arn) = organization_router();

    for case in permission_set_cases() {
        let (status, _) = call(&router, "CreatePermissionSet", &case.request(&instance_arn)).await;
        assert_eq!(status, 200, "case {:?}: the first create must succeed", case.label);

        // Same name, deliberately different optional members: the name alone decides.
        let retry = json!({"InstanceArn": instance_arn, "Name": case.name, "SessionDuration": "PT2H"});
        let (status, body) = call(&router, "CreatePermissionSet", &retry).await;
        assert_eq!(
            status, 400,
            "case {:?}: a duplicate name expected 400, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {:?}: expected ConflictException, got {body}",
            case.label
        );
    }
}

/// RULE 2 — `ListPermissionSets` enumerates exactly the stored ARNs, once each, for
/// EVERY page size the model admits.
///
/// The axis the rule generalises over is the page size, and its whole admissible range
/// here is small enough to enumerate exhaustively (1..=count, plus the absent and
/// maximum cases), which is stronger than sampling it.
#[tokio::test]
async fn list_permission_sets_pages_the_whole_stored_set_for_any_page_size() {
    let (router, instance_arn) = organization_router();

    let mut expected = Vec::new();
    for index in 0..7 {
        let (_, created) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": format!("Paged{index}")}),
        )
        .await;
        expected.push(
            created["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    expected.sort();

    // `MaxResults` is modeled `range(min: 1, max: 100)`; `None` means "the service's
    // own page size".
    let page_sizes: Vec<Option<i64>> = (1..=7)
        .map(Some)
        .chain([Some(8), Some(100), None])
        .collect();
    for page_size in page_sizes {
        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"InstanceArn": instance_arn});
            if let Some(size) = page_size {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListPermissionSets", &request).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: ListPermissionSets expected 200, got {status} {body}"
            );
            let page: Vec<String> = body["PermissionSets"]
                .as_array()
                .unwrap_or_else(|| panic!("case {page_size:?}: no PermissionSets in {body}"))
                .iter()
                .map(|arn| arn.as_str().unwrap_or_default().to_string())
                .collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {page_size:?}: a page may not exceed MaxResults, got {} items",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {page_size:?}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {page_size:?}: expected every stored ARN exactly once, got {seen:?}"
        );
    }
}

/// RULE 2 — a `NextToken` this service never issued is a refused request, not a
/// silently empty page, for every paginated read in this slice.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let (router, instance_arn) = organization_router();
    let (_, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "TokenSet"}),
    )
    .await;
    let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second permission set, so `ListPermissionSets` really issues a cursor below and
    // the account-assignment filter has a sibling to be refused against.
    let (_, second) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "TokenSetTwo"}),
    )
    .await;
    let other_permission_set_arn = second["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // A cursor this service DID issue — for a different enumeration. It is as unusable
    // on another read as a forged one: resuming an instance listing from a
    // permission-set key would skip or repeat rows silently.
    let (_, page) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn, "MaxResults": 1}),
    )
    .await;
    let foreign_cursor = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListPermissionSets issued no cursor: {page}"))
        .to_string();

    // `Token` is modeled `^[-a-zA-Z0-9+=/_]*$`, so every one of these reaches the
    // handler. The rows differ along what a forged token can look like: not hex at all,
    // hex of a plain word, hex of a stored key with no cursor tag, hex of a cursor of
    // the wrong kind, and a cursor of another enumeration verbatim.
    let hex_of_a_word = hex::encode("aa");
    let hex_of_a_key = hex::encode(&permission_set_arn);
    let hex_of_another_kind = hex::encode(format!("ssoadmin:v1:bananas:{permission_set_arn}"));
    for token in [
        "notatoken",
        "zz",
        "0",
        hex_of_a_word.as_str(),
        hex_of_a_key.as_str(),
        hex_of_another_kind.as_str(),
        foreign_cursor.as_str(),
    ] {
        let cases: Vec<(&str, Value)> = vec![
            ("ListInstances", json!({"NextToken": token})),
            (
                "ListPermissionSets",
                json!({"InstanceArn": instance_arn, "NextToken": token}),
            ),
            (
                "ListAccountAssignments",
                json!({
                    "InstanceArn": instance_arn,
                    "AccountId": ACCOUNT_A,
                    "PermissionSetArn": permission_set_arn,
                    "NextToken": token,
                }),
            ),
        ];
        for (operation, request) in cases {
            // The one pairing that is NOT a forgery: the enumeration that issued the
            // cursor accepts its own.
            if token == foreign_cursor && operation == "ListPermissionSets" {
                continue;
            }
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation}/{token:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation}/{token:?}: expected ValidationException, got {body}"
            );
        }
    }

    // A cursor is scoped to the FILTER that produced it as well as to the operation: the
    // page key only distinguishes rows within one filter, so resuming a different filter
    // from it would skip or repeat rows silently. Two principals on one (account,
    // permission set) filter, so that enumeration really issues a cursor.
    for principal_id in [
        "11111111-2222-3333-4444-555555555555",
        "66666666-7777-8888-9999-000000000000",
    ] {
        let (status, body) = call(
            &router,
            "CreateAccountAssignment",
            &json!({
                "InstanceArn": instance_arn,
                "TargetId": ACCOUNT_A,
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": permission_set_arn,
                "PrincipalType": "USER",
                "PrincipalId": principal_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal_id}: CreateAccountAssignment expected 200, got {status} {body}"
        );
    }
    let own_filter = json!({
        "InstanceArn": instance_arn,
        "AccountId": ACCOUNT_A,
        "PermissionSetArn": permission_set_arn,
        "MaxResults": 1,
    });
    let (_, page) = call(&router, "ListAccountAssignments", &own_filter).await;
    let filter_cursor = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListAccountAssignments issued no cursor: {page}"))
        .to_string();

    // Its own filter resumes with it; every other filter refuses it.
    let (status, body) = call(
        &router,
        "ListAccountAssignments",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": ACCOUNT_A,
            "PermissionSetArn": permission_set_arn,
            "NextToken": filter_cursor,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case own filter: the enumeration that issued the cursor must accept it, got {status} {body}"
    );
    let other_filters: Vec<(&str, Value)> = vec![
        (
            "another account id",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": "000000000000",
                "PermissionSetArn": permission_set_arn,
                "NextToken": filter_cursor,
            }),
        ),
        (
            "another permission set",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": ACCOUNT_A,
                "PermissionSetArn": other_permission_set_arn,
                "NextToken": filter_cursor,
            }),
        ),
    ];
    for (label, request) in other_filters {
        let (status, body) = call(&router, "ListAccountAssignments", &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a cursor of another filter expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 3 — every operation in this slice that names a resource answers
/// `ResourceNotFoundException` when that resource does not exist, at the HTTP 400 this
/// awsJson1_1 service answers every refusal of a request with.
#[tokio::test]
async fn an_unknown_resource_is_not_found_for_every_operation_that_names_one() {
    let (router, instance_arn) = organization_router();
    let (_, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "RealSet"}),
    )
    .await;
    let real_permission_set = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let unknown_instance: Vec<(&str, Value)> = vec![
        (
            "CreatePermissionSet",
            json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "Name": "Orphan"}),
        ),
        (
            "DescribePermissionSet",
            json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN}),
        ),
        ("ListPermissionSets", json!({"InstanceArn": UNKNOWN_INSTANCE_ARN})),
        (
            "DeletePermissionSet",
            json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN}),
        ),
        (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "TargetId": ACCOUNT_A,
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "PrincipalType": "USER",
                "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            }),
        ),
        (
            "ListAccountAssignments",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "AccountId": ACCOUNT_A,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
            }),
        ),
    ];
    // The same operations, this time with the REAL instance and an unknown permission
    // set: the resource that is missing changes, the answer does not.
    let unknown_permission_set: Vec<(&str, Value)> = vec![
        (
            "DescribePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN}),
        ),
        (
            "DeletePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN}),
        ),
        (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": ACCOUNT_A,
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "PrincipalType": "GROUP",
                "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            }),
        ),
        (
            "ListAccountAssignments",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": ACCOUNT_A,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
            }),
        ),
    ];

    for (operation, request) in unknown_instance.into_iter().chain(unknown_permission_set) {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation}: an unknown resource expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {body}"
        );
    }

    // And the control: the same operations against resources that DO exist are 200, so
    // the not-founds above are about the resource and not about the request's shape.
    let (status, body) = call(
        &router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": real_permission_set}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an existing permission set must describe: got {status} {body}"
    );
}

/// One row of the assignment table.
struct AssignmentCase {
    label: &'static str,
    account_id: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
}

/// The table RULE 4 is asserted over: rows differ along the account, the principal type
/// (the model's `PrincipalType` enum is exactly `USER | GROUP`) and both forms the
/// `PrincipalId` pattern admits — a bare GUID and the `[0-9a-f]{10}-` prefixed form at
/// its 47-character upper length bound.
fn assignment_cases() -> Vec<AssignmentCase> {
    vec![
        AssignmentCase {
            label: "a user, bare GUID",
            account_id: ACCOUNT_A,
            principal_type: "USER",
            principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        },
        AssignmentCase {
            label: "a group on the same account",
            account_id: ACCOUNT_A,
            principal_type: "GROUP",
            principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
        },
        AssignmentCase {
            label: "a user with the store-prefixed principal id",
            account_id: ACCOUNT_A,
            principal_type: "USER",
            principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
        },
        AssignmentCase {
            label: "another account entirely",
            account_id: "000000000000",
            principal_type: "USER",
            principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
        },
        AssignmentCase {
            label: "the highest 12-digit account",
            account_id: "999999999999",
            principal_type: "GROUP",
            principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
        },
    ]
}

/// RULE 4 — an assignment is created once per identity tuple, is reported back under
/// the (account, permission set) filter it belongs to, and never under another.
#[tokio::test]
async fn account_assignments_are_persisted_once_per_identity_and_read_back_by_filter() {
    let (router, instance_arn) = organization_router();
    let (_, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "AssignedSet"}),
    )
    .await;
    let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second permission set, so the filter has something to exclude.
    let (_, other) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "UnassignedSet"}),
    )
    .await;
    let other_permission_set_arn = other["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for case in assignment_cases() {
        let request = json!({
            "InstanceArn": instance_arn,
            "TargetId": case.account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": case.principal_type,
            "PrincipalId": case.principal_id,
        });
        let (status, body) = call(&router, "CreateAccountAssignment", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAccountAssignment expected 200, got {status} {body}",
            case.label
        );
        let created_status = &body["AccountAssignmentCreationStatus"];
        let request_id = created_status["RequestId"].as_str().unwrap_or_default();
        assert!(
            is_uuid(request_id),
            "case {:?}: expected a UUID RequestId, got {request_id:?}",
            case.label
        );
        assert_eq!(
            created_status["Status"].as_str(),
            Some("IN_PROGRESS"),
            "case {:?}: expected the accepted operation's in-flight status, got {:?}",
            case.label,
            created_status["Status"]
        );
        assert!(
            created_status["CreatedDate"].is_number(),
            "case {:?}: expected a server-set CreatedDate, got {:?}",
            case.label,
            created_status["CreatedDate"]
        );

        // Creating the same identity tuple again grants it once: the row count under
        // its filter does not move, and the call answers from the status record the
        // create that DID grant it stored — same RequestId, same CreatedDate — rather
        // than writing a second record for work that was already done.
        let request_id = request_id.to_string();
        let created_date = created_status["CreatedDate"].clone();
        let (status, repeat) = call(&router, "CreateAccountAssignment", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: re-creating an assignment must succeed, got {status} {repeat}",
            case.label
        );
        let repeat_status = &repeat["AccountAssignmentCreationStatus"];
        assert_eq!(
            repeat_status["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {:?}: expected the stored RequestId {request_id:?} again, got {:?}",
            case.label,
            repeat_status["RequestId"]
        );
        assert_eq!(
            repeat_status["CreatedDate"], created_date,
            "case {:?}: expected the stored CreatedDate {created_date:?} again, got {:?}",
            case.label,
            repeat_status["CreatedDate"]
        );
    }

    // Each (account, permission set) filter reports exactly the principals assigned to
    // it — once each, however many times they were created.
    for account_id in ["123456789012", "000000000000", "999999999999"] {
        let expected: Vec<(&str, &str)> = assignment_cases()
            .iter()
            .filter(|case| case.account_id == account_id)
            .map(|case| (case.principal_type, case.principal_id))
            .collect();
        let (status, body) = call(
            &router,
            "ListAccountAssignments",
            &json!({
                "InstanceArn": instance_arn,
                "AccountId": account_id,
                "PermissionSetArn": permission_set_arn,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account_id}: ListAccountAssignments expected 200, got {status} {body}"
        );
        let mut got: Vec<(String, String)> = body["AccountAssignments"]
            .as_array()
            .unwrap_or_else(|| panic!("case {account_id}: no AccountAssignments in {body}"))
            .iter()
            .map(|row| {
                assert_eq!(
                    row["AccountId"].as_str(),
                    Some(account_id),
                    "case {account_id}: a row escaped the account filter: {row}"
                );
                assert_eq!(
                    row["PermissionSetArn"].as_str(),
                    Some(permission_set_arn.as_str()),
                    "case {account_id}: a row escaped the permission-set filter: {row}"
                );
                (
                    row["PrincipalType"].as_str().unwrap_or_default().to_string(),
                    row["PrincipalId"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        got.sort();
        let mut want: Vec<(String, String)> = expected
            .iter()
            .map(|(principal_type, principal_id)| {
                ((*principal_type).to_string(), (*principal_id).to_string())
            })
            .collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {account_id}: expected exactly the principals assigned on that account, got {got:?}"
        );
    }

    // Nothing assigned from the other permission set: an empty list, not an error.
    let (status, body) = call(
        &router,
        "ListAccountAssignments",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": ACCOUNT_A,
            "PermissionSetArn": other_permission_set_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an unassigned permission set must list empty, got {status} {body}"
    );
    assert_eq!(
        body["AccountAssignments"].as_array().map(Vec::len),
        Some(0),
        "expected an empty AccountAssignments list, got {body}"
    );
}

/// RULE 1 — a delete removes the record, so every later reader sees it gone: the
/// describe is not found, the list drops the ARN, the assignments made from it are gone, and a
/// second delete has nothing left to remove.
#[tokio::test]
async fn deleting_a_permission_set_is_observable_to_every_later_reader() {
    let (router, instance_arn) = organization_router();

    // Several permission sets, so a delete has to remove exactly one of them.
    let mut arns = Vec::new();
    for case in permission_set_cases() {
        let (_, created) = call(&router, "CreatePermissionSet", &case.request(&instance_arn)).await;
        arns.push(
            created["PermissionSet"]["PermissionSetArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (index, arn) in arns.clone().into_iter().enumerate() {
        // An assignment from the set about to be deleted.
        let (status, _) = call(
            &router,
            "CreateAccountAssignment",
            &json!({
                "InstanceArn": instance_arn,
                "TargetId": ACCOUNT_A,
                "TargetType": "AWS_ACCOUNT",
                "PermissionSetArn": arn,
                "PrincipalType": "USER",
                "PrincipalId": "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {arn}: the assignment must be created");

        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn}: DeletePermissionSet expected 200, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {arn}: a deleted permission set must describe as not found, got {status} {body}"
        );

        // A second delete: nothing left to remove.
        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {arn}: a second delete expected not found, got {status} {body}"
        );

        // The assignment it granted went with it, so listing by it is not found on the set.
        let (status, body) = call(
            &router,
            "ListAccountAssignments",
            &json!({
                "InstanceArn": instance_arn,
                "AccountId": ACCOUNT_A,
                "PermissionSetArn": arn,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {arn}: assignments of a deleted permission set expected not found, got {status} {body}"
        );

        // The list reports exactly the sets that are left — the deleted one is gone and
        // the others are untouched.
        let (_, body) = call(
            &router,
            "ListPermissionSets",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        let listed: Vec<String> = body["PermissionSets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {arn}: no PermissionSets in {body}"))
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        let mut expected = arns[index + 1..].to_vec();
        expected.sort();
        let mut got = listed;
        got.sort();
        assert_eq!(
            got, expected,
            "case {arn}: expected only the undeleted ARNs to remain, got {got:?}"
        );
    }
}

/// The account's one instance is a real, ACTIVE record the other operations resolve
/// against, and reading it does not change it.
#[tokio::test]
async fn the_one_instance_is_active_and_stable_across_reads() {
    let (router, _) = organization_router();

    let (status, first) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {first}");
    let instances = first["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("no Instances in {first}"));
    assert_eq!(
        instances.len(),
        1,
        "expected the account's one instance, got {instances:?}"
    );
    let instance = &instances[0];
    let arn = instance["InstanceArn"].as_str().unwrap_or_default();
    assert!(
        is_instance_arn(arn),
        "expected a modeled InstanceArn, got {arn:?}"
    );
    assert_eq!(
        instance["Status"].as_str(),
        Some("ACTIVE"),
        "expected an ACTIVE instance, got {:?}",
        instance["Status"]
    );
    assert_eq!(
        instance["OwnerAccountId"].as_str(),
        Some(engine::EMULATOR_ACCOUNT_ID),
        "expected the emulator's account id, got {:?}",
        instance["OwnerAccountId"]
    );
    assert!(
        instance["IdentityStoreId"].as_str().is_some_and(|id| !id.is_empty()),
        "expected an IdentityStoreId, got {:?}",
        instance["IdentityStoreId"]
    );
    assert!(
        instance["CreatedDate"].is_number(),
        "expected a CreatedDate, got {:?}",
        instance["CreatedDate"]
    );
    // AWS's `ListInstances` carries a `NextToken` on every page that returned an
    // instance, including the account's only one, and following it ends the enumeration.
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the page that returned an instance carries a NextToken: {first}"))
        .to_string();
    let (status, last) = call(&router, "ListInstances", &json!({"NextToken": token})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {last}");
    assert_eq!(
        last["Instances"].as_array().map(Vec::len),
        Some(0),
        "following the token ends the enumeration, got {last}"
    );
    assert_eq!(
        last["NextToken"],
        Value::Null,
        "the empty page past the end carries no token, got {last}"
    );

    // A read is a read: the second call reports the identical record, and creating a
    // permission set under the instance does not disturb it either.
    let (_, _) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": arn, "Name": "StabilitySet"}),
    )
    .await;
    let (status, second) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {second}");
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees, got {second}"
    );
}

/// Modeled constraints are refused at the boundary, on both sides of a published bound:
/// `Name` is `length 1..32` with `^[\w+=,.@-]+$`, and an `InstanceArn` that does not
/// match its pattern is a bad request rather than a missing resource.
#[tokio::test]
async fn requests_outside_the_modeled_constraints_are_refused() {
    let (router, instance_arn) = organization_router();

    let cases: Vec<(&str, Value)> = vec![
        // One past the upper length bound of `Name`.
        (
            "Name of 33 characters",
            json!({"InstanceArn": instance_arn, "Name": "n".repeat(33)}),
        ),
        // One past its lower bound.
        (
            "empty Name",
            json!({"InstanceArn": instance_arn, "Name": ""}),
        ),
        // Outside its character class.
        (
            "Name with a space",
            json!({"InstanceArn": instance_arn, "Name": "not a name"}),
        ),
        // Outside the `InstanceArn` pattern.
        (
            "InstanceArn of the wrong shape",
            json!({"InstanceArn": "arn:aws:sso:::instance/nope", "Name": "Fine"}),
        ),
        // One past the upper length bound of `Description`.
        (
            "Description of 701 characters",
            json!({
                "InstanceArn": instance_arn,
                "Name": "Fine",
                "Description": "d".repeat(701),
            }),
        ),
    ];
    for (label, request) in cases {
        let (status, body) = call(&router, "CreatePermissionSet", &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a 400 refusal, got {status} {body}"
        );
    }

    // And the in-bounds neighbours of those bounds are accepted.
    for (label, name) in [("Name of 1 character", "n"), ("Name of 32 characters", &"n".repeat(32))]
    {
        let (status, body) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an in-bounds Name expected 200, got {status} {body}"
        );
    }
}

/// Each caller-driven store holds at most
/// [`service_com_amazonaws_ssoadmin_swbexternalservice::state::MAX_RECORDS_PER_STORE`]
/// records — the emulator's own memory ceiling, not an AWS quota — and a create that
/// would cross it is refused with the modeled `ServiceQuotaExceededException`
/// instead of growing the process. The ceiling is read from the crate, so the test
/// tracks the constant rather than restating it.
#[tokio::test]
async fn a_caller_cannot_grow_a_store_past_its_ceiling() {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state::MAX_RECORDS_PER_STORE;

    let (router, instance_arn) = organization_router();

    // Fill the permission-set store to the ceiling with distinct names, which is the
    // dimension a caller controls (the name-uniqueness check is not a count bound).
    for index in 0..MAX_RECORDS_PER_STORE {
        let (status, body) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": format!("Set{index}")}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index}: a create below the ceiling expected 200, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "OneTooMany"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {MAX_RECORDS_PER_STORE} permission sets: expected the modeled quota refusal, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case {MAX_RECORDS_PER_STORE} permission sets: expected ServiceQuotaExceededException, got {body}"
    );

    // The ceiling refuses; it does not corrupt. Deleting one record makes room for
    // exactly one more, so the bound is a ceiling rather than a one-way door.
    let (status, listed) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn, "MaxResults": 1}),
    )
    .await;
    assert_eq!(status, 200, "ListPermissionSets expected 200, got {status} {listed}");
    let victim = listed["PermissionSets"][0]
        .as_str()
        .unwrap_or_else(|| panic!("no stored permission set to delete: {listed}"))
        .to_string();
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": victim}),
    )
    .await;
    assert_eq!(status, 200, "DeletePermissionSet expected 200, got {status} {body}");
    let (status, body) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "OneTooMany"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case one record below the ceiling again: expected 200, got {status} {body}"
    );
}
