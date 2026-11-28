// SPDX-License-Identifier: CC0-1.0

//! What `UpdateProtectConfiguration` leaves on a stored protect configuration, end to end
//! over the wire.
//!
//! Every request goes through [`MultiServiceRouter::dispatch`] as `awsJson1_0` — `POST /`
//! with `X-Amz-Target: PinpointSMSVoiceV2.<Op>` — so these tests exercise routing,
//! deserialization, constraint validation, the handler and serialization, exactly as a real
//! SDK client would reach them.
//!
//! The rule under test is the one the sibling suite `event_destination_updates` pins for an
//! event destination, here for the other resource this service lets a caller update: the
//! update writes the member the request carries onto the stored record and leaves a member
//! it omits as it was, so the resource's state is what the last request naming each member
//! set — which is what every later read reports AND what the delete weighs its documented
//! precondition ("must have deletion protection disabled") against.
//!
//! Its harness is this suite's own: the sibling suites' helpers are private to their
//! modules, and a test target shares nothing it does not export.

use std::sync::Arc;

use service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2 as sms_voice;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for, so the ARNs under assertion are not the default one.
const REGION: &str = "us-east-2";

/// The account id every emulated ARN carries.
const ACCOUNT_ID: &str = emulator_server::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus a router with only this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(sms_voice::EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and answer `(http status, parsed JSON body)`.
    async fn call(&self, operation: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sms-voice/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require HTTP 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: serde_json::Value) -> serde_json::Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create one protect configuration and answer the record the create reports.
    async fn create(&self, deletion_protection_enabled: bool) -> serde_json::Value {
        self.ok(
            "CreateProtectConfiguration",
            serde_json::json!({"DeletionProtectionEnabled": deletion_protection_enabled}),
        )
        .await
    }

    /// The record one stored protect configuration reports, as the selecting describe
    /// answers it.
    async fn described(&self, id: &str) -> serde_json::Value {
        let described = self
            .ok(
                "DescribeProtectConfigurations",
                serde_json::json!({"ProtectConfigurationIds": [id]}),
            )
            .await;
        described
            .get("ProtectConfigurations")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .cloned()
            .unwrap_or_else(|| panic!("expected a stored protect configuration in {described}"))
    }
}

/// A string member of a response body.
fn member(body: &serde_json::Value, name: &str) -> String {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {name} in {body}"))
        .to_string()
}

/// A boolean member of a response body.
fn flag(body: &serde_json::Value, name: &str) -> bool {
    body.get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| panic!("expected a boolean member {name} in {body}"))
}

/// The wire error code of a failed call (`__type`, with any namespace prefix dropped).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The ARN one stored protect configuration carries, as the create builds it from the
/// signing region and the emulator's account id.
fn protect_configuration_arn(id: &str) -> String {
    format!("arn:aws:sms-voice:{REGION}:{ACCOUNT_ID}:protect-configuration/{id}")
}

/// One row of the update table: the setting the request carries (or none at all), and the
/// deletion protection the record must hold afterwards.
struct Row {
    case: &'static str,
    /// The `DeletionProtectionEnabled` the request names, or `None` for a request that
    /// names no setting.
    requested: Option<bool>,
    expected: bool,
}

/// Every row is one update request; the state it must leave is the previous state with the
/// setting the request named replaced, and a request naming none leaves it alone.
#[tokio::test]
async fn an_update_writes_the_setting_it_names_and_leaves_the_record_alone_otherwise() {
    // Both starting points: a configuration created with deletion protection on, and one
    // created with it off.
    for created_with in [false, true] {
        let emulator = Emulator::new();
        let created = emulator.create(created_with).await;
        let id = member(&created, "ProtectConfigurationId");
        assert_eq!(
            flag(&created, "DeletionProtectionEnabled"),
            created_with,
            "case created_with={created_with}: the create reports the setting it stored, got \
             {created}"
        );

        let rows = [
            Row {
                case: "deletion protection turned off",
                requested: Some(false),
                expected: false,
            },
            Row {
                case: "deletion protection turned on",
                requested: Some(true),
                expected: true,
            },
            Row {
                case: "the same setting written again",
                requested: Some(true),
                expected: true,
            },
            Row {
                case: "no setting named at all",
                requested: None,
                expected: true,
            },
            Row {
                case: "deletion protection turned off again",
                requested: Some(false),
                expected: false,
            },
            Row {
                case: "no setting named after it was turned off",
                requested: None,
                expected: false,
            },
        ];

        for row in &rows {
            let case = row.case;
            // The id form the request names the resource by is the one the member's target
            // declares (`ProtectConfigurationIdOrArn`), so every other row names it by ARN.
            let named = if row.requested == Some(true) {
                protect_configuration_arn(&id)
            } else {
                id.clone()
            };
            let mut request = serde_json::json!({"ProtectConfigurationId": named});
            if let Some(requested) = row.requested {
                request
                    .as_object_mut()
                    .expect("the request is an object")
                    .insert(
                        "DeletionProtectionEnabled".to_string(),
                        serde_json::Value::Bool(requested),
                    );
            }
            let answered = emulator.ok("UpdateProtectConfiguration", request).await;

            assert_eq!(
                (
                    member(&answered, "ProtectConfigurationId"),
                    member(&answered, "ProtectConfigurationArn"),
                    flag(&answered, "DeletionProtectionEnabled"),
                    flag(&answered, "AccountDefault"),
                ),
                (
                    id.clone(),
                    protect_configuration_arn(&id),
                    row.expected,
                    false,
                ),
                "case created_with={created_with}/{case:?}: expected the record the update \
                 left, got {answered}"
            );
            assert_eq!(
                answered.get("CreatedTimestamp"),
                created.get("CreatedTimestamp"),
                "case created_with={created_with}/{case:?}: an update may not move the \
                 creation time, got {answered}"
            );

            // The round trip: a later read reports exactly what the update persisted.
            let described = emulator.described(&id).await;
            assert_eq!(
                (
                    member(&described, "ProtectConfigurationId"),
                    flag(&described, "DeletionProtectionEnabled"),
                ),
                (id.clone(), row.expected),
                "case created_with={created_with}/{case:?}: a later read must report the \
                 persisted setting, got {described}"
            );
        }
    }
}

/// The documented teardown: deletion protection blocks the delete while it is on, the
/// update turns it off, and the delete then removes the record so no later read reports it.
#[tokio::test]
async fn turning_deletion_protection_off_makes_the_configuration_deletable() {
    // One row per way the protection can stand when the delete is attempted, so the delete
    // is driven from both sides of the setting rather than from one recorded order.
    let rows: Vec<(&str, bool, Vec<bool>)> = vec![
        ("created protected, then unprotected", true, vec![false]),
        (
            "created protected, unprotected and protected again before the final release",
            true,
            vec![false, true, false],
        ),
        ("created unprotected, then protected", false, vec![true, false]),
        ("created unprotected and left alone", false, vec![]),
    ];

    for (case, created_with, updates) in rows {
        let emulator = Emulator::new();
        let created = emulator.create(created_with).await;
        let id = member(&created, "ProtectConfigurationId");

        let mut protected = created_with;
        let mut attempt = 0;
        for requested in updates.iter().copied() {
            attempt += 1;
            // While the protection is on, the delete is refused and the record stays as it
            // is; the probe is only made in that state, since a delete that is allowed
            // would remove the record this row is still driving.
            if protected {
                assert_delete_refused(&emulator, case, attempt, &id).await;
            }
            let answered = emulator
                .ok(
                    "UpdateProtectConfiguration",
                    serde_json::json!({
                        "ProtectConfigurationId": id,
                        "DeletionProtectionEnabled": requested,
                    }),
                )
                .await;
            assert_eq!(
                flag(&answered, "DeletionProtectionEnabled"),
                requested,
                "case {case:?} attempt {attempt}: the update reports the setting it wrote, got \
                 {answered}"
            );
            protected = requested;
        }

        assert!(
            !protected,
            "case {case:?}: the rows end with the protection off, so the delete is allowed"
        );
        let deleted = emulator
            .ok(
                "DeleteProtectConfiguration",
                serde_json::json!({"ProtectConfigurationId": id}),
            )
            .await;
        assert_eq!(
            (
                member(&deleted, "ProtectConfigurationId"),
                flag(&deleted, "DeletionProtectionEnabled"),
                flag(&deleted, "AccountDefault"),
            ),
            (id.clone(), false, false),
            "case {case:?}: the delete reports the record it removed, got {deleted}"
        );

        // The record is gone, so the selecting read is a not-found rather than a 200.
        let (status, body) = emulator
            .call(
                "DescribeProtectConfigurations",
                serde_json::json!({"ProtectConfigurationIds": [id.clone()]}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a deleted configuration is not found, got {status} with {body}"
        );
        assert_eq!(
            (
                member(&body, "Message"),
                member(&body, "ResourceType"),
                member(&body, "ResourceId"),
            ),
            (
                format!(
                    "Resource Not Found - ResourceType=\"protect-configuration\" \
                     ResourceId=\"{id}\""
                ),
                "protect-configuration".to_string(),
                id.clone(),
            ),
            "case {case:?}: the refusal names the resource it could not find, got {body}"
        );

        // The update of a record that is gone is a not-found too, for the same reason.
        let (status, body) = emulator
            .call(
                "UpdateProtectConfiguration",
                serde_json::json!({
                    "ProtectConfigurationId": id,
                    "DeletionProtectionEnabled": true,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a deleted configuration cannot be updated, got {status} with {body}"
        );
    }
}

/// A protected configuration refuses its delete with the modelled conflict, and the refused
/// delete leaves the record as it was.
async fn assert_delete_refused(emulator: &Emulator, case: &str, attempt: u32, id: &str) {
    let (status, body) = emulator
        .call(
            "DeleteProtectConfiguration",
            serde_json::json!({"ProtectConfigurationId": id}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ConflictException"),
        "case {case:?} attempt {attempt}: a protected configuration refuses its delete, got \
         {status} with {body}"
    );
    assert_eq!(
        (
            member(&body, "Message"),
            member(&body, "Reason"),
            member(&body, "ResourceType"),
            member(&body, "ResourceId"),
        ),
        (
            format!(
                "Conflict Occurred - Reason=\"DELETION_PROTECTION_ENABLED\" \
                 ResourceType=\"protect-configuration\" ResourceId=\"{id}\""
            ),
            "DELETION_PROTECTION_ENABLED".to_string(),
            "protect-configuration".to_string(),
            id.to_string(),
        ),
        "case {case:?} attempt {attempt}: the conflict restates its own members, got {body}"
    );
    let described = emulator.described(id).await;
    assert!(
        flag(&described, "DeletionProtectionEnabled"),
        "case {case:?} attempt {attempt}: a refused delete leaves the record as it was, got \
         {described}"
    );
}

/// An update of a configuration the account does not hold is a not-found, named in both id
/// forms the input member accepts.
#[tokio::test]
async fn an_update_of_a_configuration_that_is_not_there_is_refused() {
    let emulator = Emulator::new();
    // A stored configuration exists throughout, so the refusals below are about the id the
    // request names rather than about an empty store.
    let created = emulator.create(false).await;
    let stored_id = member(&created, "ProtectConfigurationId");

    let absent = "protect-configuration-never-created";
    let rows: Vec<(&str, String, &str)> = vec![
        ("a bare id no configuration carries", absent.to_string(), absent),
        (
            "the same id in its ARN form",
            protect_configuration_arn(absent),
            absent,
        ),
    ];
    for (case, named, resource_id) in rows {
        let (status, body) = emulator
            .call(
                "UpdateProtectConfiguration",
                serde_json::json!({
                    "ProtectConfigurationId": named,
                    "DeletionProtectionEnabled": true,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} with {body}"
        );
        assert_eq!(
            (
                member(&body, "Message"),
                member(&body, "ResourceType"),
                member(&body, "ResourceId"),
            ),
            (
                format!(
                    "Resource Not Found - ResourceType=\"protect-configuration\" \
                     ResourceId=\"{resource_id}\""
                ),
                "protect-configuration".to_string(),
                resource_id.to_string(),
            ),
            "case {case:?}: the refusal names the resource it could not find, got {body}"
        );
        let described = emulator.described(&stored_id).await;
        assert!(
            !flag(&described, "DeletionProtectionEnabled"),
            "case {case:?}: a refused update changes no other record, got {described}"
        );
    }
}
