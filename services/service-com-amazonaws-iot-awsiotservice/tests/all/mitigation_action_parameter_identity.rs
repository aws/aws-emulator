// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHICH second `CreateMitigationAction` under a name the
//! account already holds is the same action, when both creates ask for the same TYPE of
//! change: the one whose per-type parameters carry the same VALUES, member for member.
//!
//! The collision this family states is an existing resource "with different attributes", so
//! what a repeat create ASKS for decides it. `MitigationActionParams` is a structure with
//! one optional member per type of change, and each of those members is a structure of its
//! own (`smithy_query(query="shape",shape="MitigationActionParams")`) — so two creates can
//! name the same type and still ask for different actions, by carrying different values
//! inside that type's parameters. A repeat create is the same action only when every member
//! of the stored parameters is the value the request carries; anything else is the conflict,
//! `ResourceAlreadyExistsException` (409) carrying the family's clause and the ARN of the
//! action that exists, with the stored record untouched.
//!
//! The table spans what that rule generalises over — the KINDS of value a type's parameters
//! hold, read off the model's own declarations
//! (`smithy_query(query="shape",shape="AddThingsToThingGroupParams")` and the other five):
//! an enumerated string (`EnableIoTLoggingParams$logLevel`), a free-form ARN
//! (`EnableIoTLoggingParams$roleArnForLogging`, `PublishFindingToSnsParams$topicArn`), a
//! list by its elements and by how many it holds (`AddThingsToThingGroupParams$thingGroupNames`),
//! and an optional boolean by its value and by its presence
//! (`AddThingsToThingGroupParams$overrideDynamicGroups`). The three types whose parameters
//! the model admits exactly one value for — `DeviceCertificateUpdateAction`,
//! `CACertificateUpdateAction` and `PolicyTemplateName` each declare a single member — are
//! rows that can only be restated identically, and are here so every modelled type is
//! covered rather than only the ones with room to differ.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK client
//! would see.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The region every request in this suite is signed for; the ARN assertions read it back.
const REGION: &str = "us-west-2";

/// A well-formed role ARN: the role the actions in this suite are applied with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/device-defender-mitigation";

/// The fixed text of the clause this family states a taken name with; the ARN of the
/// resource that already exists follows it.
const ALREADY_EXISTS_CLAUSE: &str =
    "Resource already exists for account with different attributes, ";

/// One row: the action a first create stored, the action a second create under the same name
/// asks for, and whether the two are the same action.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The `actionParams` the stored action was created with.
    stored: Value,
    /// The `actionParams` the second create carries — the same TYPE of change as `stored`.
    requested: Value,
    /// Whether the second create asks for the action the record holds.
    same_action: bool,
}

/// Every row of the table: one group per modelled `MitigationActionType`, differing in the
/// kind of value the type's own parameters hold.
fn rows() -> Vec<Row> {
    let logging = |role: &str, level: &str| {
        json!({
            "enableIoTLoggingParams": { "roleArnForLogging": role, "logLevel": level },
        })
    };
    let publish = |topic: &str| json!({ "publishFindingToSnsParams": { "topicArn": topic } });
    let add_things = |groups: Value, override_dynamic: Option<bool>| {
        let mut params = json!({ "thingGroupNames": groups });
        if let Some(override_dynamic) = override_dynamic {
            params
                .as_object_mut()
                .expect("the parameters are a JSON object")
                .insert("overrideDynamicGroups".to_string(), json!(override_dynamic));
        }
        json!({ "addThingsToThingGroupParams": params })
    };

    let logging_role = "arn:aws:iam::000000000000:role/iot-logging";
    let other_logging_role = "arn:aws:iam::000000000000:role/iot-logging-verbose";
    let topic = "arn:aws:sns:us-west-2:000000000000:defender-findings";
    let other_topic = "arn:aws:sns:us-west-2:000000000000:defender-escalations";

    vec![
        // An enumerated member: `LogLevel` declares DEBUG, INFO, ERROR, WARN and DISABLED
        // (`smithy_query(query="shape",shape="LogLevel")`), so two of its values are two
        // different actions.
        Row {
            case: "the same logging parameters restated",
            stored: logging(logging_role, "WARN"),
            requested: logging(logging_role, "WARN"),
            same_action: true,
        },
        Row {
            case: "logging at another level",
            stored: logging(logging_role, "WARN"),
            requested: logging(logging_role, "ERROR"),
            same_action: false,
        },
        // A free-form ARN member inside the parameters, which is not the action's own
        // `roleArn`: the action logs under another role, so it is another action.
        Row {
            case: "logging under another role",
            stored: logging(logging_role, "WARN"),
            requested: logging(other_logging_role, "WARN"),
            same_action: false,
        },
        Row {
            case: "the same SNS topic restated",
            stored: publish(topic),
            requested: publish(topic),
            same_action: true,
        },
        Row {
            case: "findings published to another topic",
            stored: publish(topic),
            requested: publish(other_topic),
            same_action: false,
        },
        // A list member, by its elements and by how many it holds.
        Row {
            case: "the same thing groups restated",
            stored: add_things(json!(["quarantine"]), Some(true)),
            requested: add_things(json!(["quarantine"]), Some(true)),
            same_action: true,
        },
        Row {
            case: "another thing group",
            stored: add_things(json!(["quarantine"]), Some(true)),
            requested: add_things(json!(["review"]), Some(true)),
            same_action: false,
        },
        Row {
            case: "one more thing group than the record holds",
            stored: add_things(json!(["quarantine"]), Some(true)),
            requested: add_things(json!(["quarantine", "review"]), Some(true)),
            same_action: false,
        },
        // An optional boolean member, by its value and by whether it is carried at all.
        Row {
            case: "the dynamic-group override turned off",
            stored: add_things(json!(["quarantine"]), Some(true)),
            requested: add_things(json!(["quarantine"]), Some(false)),
            same_action: false,
        },
        Row {
            case: "the dynamic-group override left out",
            stored: add_things(json!(["quarantine"]), Some(true)),
            requested: add_things(json!(["quarantine"]), None),
            same_action: false,
        },
        Row {
            case: "the dynamic-group override carried by neither create",
            stored: add_things(json!(["quarantine"]), None),
            requested: add_things(json!(["quarantine"]), None),
            same_action: true,
        },
        // The three types whose parameters hold a single admissible value: restating them is
        // all a second create can do.
        Row {
            case: "the device-certificate action restated",
            stored: json!({ "updateDeviceCertificateParams": { "action": "DEACTIVATE" } }),
            requested: json!({ "updateDeviceCertificateParams": { "action": "DEACTIVATE" } }),
            same_action: true,
        },
        Row {
            case: "the CA-certificate action restated",
            stored: json!({ "updateCACertificateParams": { "action": "DEACTIVATE" } }),
            requested: json!({ "updateCACertificateParams": { "action": "DEACTIVATE" } }),
            same_action: true,
        },
        Row {
            case: "the policy-replacement action restated",
            stored: json!({ "replaceDefaultPolicyVersionParams": { "templateName": "BLANK_POLICY" } }),
            requested: json!({ "replaceDefaultPolicyVersionParams": { "templateName": "BLANK_POLICY" } }),
            same_action: true,
        },
    ]
}

/// The ARN this plane mints for `action_name`.
fn expected_arn(action_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:mitigationaction/{action_name}")
}

/// One emulator: the shared engine plus a router with this service mounted, reached the way
/// the production router reaches it.
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

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's `aws.auth#sigv4`
            // trait declares, which is what routes the request, and the region the served
            // ARNs are built for.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the mitigation-action request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    /// `CreateMitigationAction`, bound to `POST /mitigationactions/actions/{actionName}`.
    async fn create(&self, action_name: &str, params: &Value) -> Reply {
        self.dispatch(
            "POST",
            &format!("/mitigationactions/actions/{action_name}"),
            Some(&json!({ "roleArn": ROLE_ARN, "actionParams": params })),
        )
        .await
    }

    /// `DescribeMitigationAction`, bound to `GET /mitigationactions/actions/{actionName}`.
    async fn describe(&self, action_name: &str) -> Reply {
        self.dispatch(
            "GET",
            &format!("/mitigationactions/actions/{action_name}"),
            None,
        )
        .await
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries in its body.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    /// The `message` the body carries.
    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }

    /// The member of a 200 body named `member`, as text.
    fn member(&self, member: &str) -> String {
        self.json()[member]
            .as_str()
            .unwrap_or_else(|| panic!("no {member} in {}", self.text()))
            .to_string()
    }
}

/// RULE: a second create under a taken name asking for the same type of change is the same
/// action — answered from the stored record, writing nothing — only when the type's
/// parameters carry the values the record holds, member for member; otherwise it is the
/// conflict and the record stands.
#[tokio::test]
async fn a_repeat_create_is_the_same_action_only_when_its_parameters_values_match() {
    for Row {
        case,
        stored,
        requested,
        same_action,
    } in rows()
    {
        // A fresh emulator per row: what the row proves must depend on this row alone.
        let emulator = Emulator::new();
        let action_name = "parameter-values-under-test";

        let first = emulator.create(action_name, &stored).await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: expected 200 from the first create, got {} body {}",
            first.status,
            first.text()
        );
        let minted_arn = first.member("actionArn");
        let minted_id = first.member("actionId");
        assert_eq!(
            minted_arn,
            expected_arn(action_name),
            "case {case:?}: expected the ARN this plane mints, got {minted_arn:?}"
        );

        // The read of the stored action reports the parameters it was created with, so the
        // comparison below is against what a caller can see.
        let record = emulator.describe(action_name).await;
        assert_eq!(
            record.status,
            200,
            "case {case:?}: expected the created action to read back, got {} body {}",
            record.status,
            record.text()
        );
        assert_eq!(
            record.json()["actionParams"],
            stored,
            "case {case:?}: expected the stored parameters to read back, got {}",
            record.text()
        );
        let before = record.json();

        let repeat = emulator.create(action_name, &requested).await;
        if same_action {
            assert_eq!(
                repeat.status,
                200,
                "case {case:?}: expected 200 for the same action, got {} body {}",
                repeat.status,
                repeat.text()
            );
            assert_eq!(
                (repeat.member("actionArn"), repeat.member("actionId")),
                (minted_arn.clone(), minted_id.clone()),
                "case {case:?}: expected the stored record's identity, got {}",
                repeat.text()
            );
        } else {
            assert_eq!(
                repeat.status,
                409,
                "case {case:?}: expected 409 for a different action, got {} body {}",
                repeat.status,
                repeat.text()
            );
            assert_eq!(
                repeat.error_code(),
                "ResourceAlreadyExistsException",
                "case {case:?}: got {}",
                repeat.text()
            );
            assert_eq!(
                repeat.error_message(),
                format!("{ALREADY_EXISTS_CLAUSE}{}", expected_arn(action_name)),
                "case {case:?}: expected the family's clause and the existing action's ARN, \
                 got {:?}",
                repeat.error_message()
            );
        }

        let after = emulator.describe(action_name).await;
        assert_eq!(
            after.json(),
            before,
            "case {case:?}: expected the second create to write nothing, got {}",
            after.text()
        );
    }
}
