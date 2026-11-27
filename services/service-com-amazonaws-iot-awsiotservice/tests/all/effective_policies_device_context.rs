// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the two things `GetEffectivePolicies` resolves BESIDE the
//! principal: the DEVICE the question is about, and the Amazon Cognito identity POOL an
//! identity is federated into.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see. Amazon Cognito Identity is mounted on the same engine, because an
//! identity pool is that service's resource.
//!
//! The rules under test:
//!
//! * `thingName` is CONTEXT that extends the answer to the policies attached to the thing
//!   GROUPS the named thing belongs to, with the groups above them — a policy attached to a
//!   group governs the devices in it, and one attached to a parent governs the groups
//!   beneath — so the same principal answers differently with and without the device named,
//!   and the union is reported once per policy;
//! * a `cognitoIdentityPoolId` is resolved against the HOLDER of identity pools: a pool
//!   Amazon Cognito Identity has is answered about, and one it does not have is refused HTTP
//!   404 `ResourceNotFoundException` in the holder's own sentence.
//!
//! The tables span what the rules generalise over rather than one recorded request: a thing
//! in one group, in a nested group with a policy above it, in two unrelated groups and in no
//! group at all; a policy attached both to a group and to the principal; and pool ids of
//! several forms, held and unheld, beside the malformed value that is no pool id at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService as CognitoIdentityService;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for.
const REGION: &str = "ap-south-1";

/// The emulator under test: one shared engine with the IoT service and the holder of
/// identity pools mounted on it, exactly as the assembled emulator mounts them.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(CognitoIdentityService::new(engine));
        Self { router }
    }

    /// One IoT request, signed for the `iot` scope that routes it to this service.
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        Reply::of(self.router.dispatch(request).await)
    }

    /// `CreatePolicy`: `POST /policies/{policyName}`.
    async fn given_policy(&self, policy_name: &str) -> String {
        let document = policy_document(policy_name);
        let created = self
            .call(
                "POST",
                &format!("/policies/{policy_name}"),
                Some(&json!({ "policyDocument": document })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {policy_name:?}: CreatePolicy must answer 200, got {created:?}"
        );
        document
    }

    /// `CreateThingGroup`: `POST /thing-groups/{thingGroupName}`, answering its ARN.
    async fn given_thing_group(&self, name: &str, parent: Option<&str>) -> String {
        let body = match parent {
            Some(parent) => json!({ "parentGroupName": parent }),
            None => json!({}),
        };
        let created = self
            .call("POST", &format!("/thing-groups/{name}"), Some(&body))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateThingGroup must answer 200, got {created:?}"
        );
        created
            .text("thingGroupArn")
            .unwrap_or_else(|| panic!("case {name:?}: no thingGroupArn in {created:?}"))
    }

    /// `CreateThing`: `POST /things/{thingName}`.
    async fn given_thing(&self, thing_name: &str) {
        let created = self
            .call("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await;
        assert_eq!(
            created.status, 200,
            "case {thing_name:?}: CreateThing must answer 200, got {created:?}"
        );
    }

    /// `AddThingToThingGroup`: `PUT /thing-groups/addThingToThingGroup`.
    async fn given_membership(&self, thing_name: &str, thing_group_name: &str) {
        let added = self
            .call(
                "PUT",
                "/thing-groups/addThingToThingGroup",
                Some(&json!({
                    "thingName": thing_name,
                    "thingGroupName": thing_group_name,
                })),
            )
            .await;
        assert_eq!(
            added.status, 200,
            "case {thing_name:?} in {thing_group_name:?}: AddThingToThingGroup must answer 200, \
             got {added:?}"
        );
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, answering the ARN of the
    /// certificate it issued.
    async fn given_certificate(&self) -> String {
        let issued = self
            .call("POST", "/keys-and-certificate?setAsActive=true", None)
            .await;
        assert_eq!(
            issued.status, 200,
            "CreateKeysAndCertificate must answer 200, got {issued:?}"
        );
        issued
            .text("certificateArn")
            .unwrap_or_else(|| panic!("no certificateArn in {issued:?}"))
    }

    /// `AttachThingPrincipal`: `PUT /things/{thingName}/principals`, the principal in the
    /// header the model binds it to.
    async fn given_attached_principal(&self, thing_name: &str, principal: &str) {
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!(
                "https://iot.{REGION}.amazonaws.com/things/{thing_name}/principals"
            ))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0000"
                ),
            )
            .header("x-amzn-principal", principal)
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let attached = Reply::of(self.router.dispatch(request).await);
        assert_eq!(
            attached.status, 200,
            "case {thing_name:?}<-{principal:?}: AttachThingPrincipal must answer 200, got \
             {attached:?}"
        );
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}`.
    async fn given_attachment(&self, policy_name: &str, target: &str) {
        let attached = self
            .call(
                "PUT",
                &format!("/target-policies/{policy_name}"),
                Some(&json!({ "target": target })),
            )
            .await;
        assert_eq!(
            attached.status, 200,
            "case {policy_name:?}->{target:?}: AttachPolicy must answer 200, got {attached:?}"
        );
    }

    /// `GetEffectivePolicies`: `POST /effective-policies`, whose identities travel in the
    /// body and whose `thingName` is `@httpQuery`.
    async fn get_effective_policies(
        &self,
        principal: Option<&str>,
        pool_id: Option<&str>,
        thing_name: Option<&str>,
    ) -> Reply {
        let mut request = serde_json::Map::new();
        if let Some(principal) = principal {
            request.insert("principal".to_string(), json!(principal));
        }
        if let Some(pool_id) = pool_id {
            request.insert("cognitoIdentityPoolId".to_string(), json!(pool_id));
        }
        let path = match thing_name {
            Some(thing_name) => format!("/effective-policies?thingName={thing_name}"),
            None => "/effective-policies".to_string(),
        };
        self.call("POST", &path, Some(&Value::Object(request))).await
    }

    /// Amazon Cognito Identity's `CreateIdentityPool`, through the one caller of that API this
    /// suite tree has (`crate::effective_policies::created_identity_pool`): the pool a case
    /// names has to exist where identity pools live.
    async fn given_identity_pool(&self, identity_pool_name: &str) -> String {
        crate::effective_policies::created_identity_pool(&self.router, REGION, identity_pool_name)
            .await
    }
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn of(response: http::Response<bytes::Bytes>) -> Self {
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|code| {
                code.split(':')
                    .next()
                    .unwrap_or(code)
                    .to_string()
            });
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Self {
            status,
            error_code,
            body,
        }
    }

    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body, or the `__type` for a body that carries none.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// The `policyName` of every reported effective policy, in the order reported.
    fn policy_names(&self) -> Vec<String> {
        self.body
            .get("effectivePolicies")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .get("policyName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `policyDocument` reported for `policy_name`, if any.
    fn document_of(&self, policy_name: &str) -> Option<String> {
        self.body
            .get("effectivePolicies")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.get("policyName").and_then(Value::as_str) == Some(policy_name))
                    .and_then(|entry| entry.get("policyDocument").and_then(Value::as_str))
                    .map(str::to_string)
            })
    }
}

/// An IoT policy document naming the policy it belongs to, so a document reported for the
/// wrong policy would fail the assertion.
fn policy_document(policy_name: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "iot:Connect",
            "Resource": format!("arn:aws:iot:{REGION}:000000000000:client/{policy_name}"),
        }],
    })
    .to_string()
}

/// One row of the device-context table: how the thing's groups are arranged, and which
/// policies the principal-only and the thing-qualified reads must report.
struct DeviceCase {
    /// What the case is called in an assertion.
    label: &'static str,
    /// The thing the question is about.
    thing_name: &'static str,
    /// `(groupName, parentGroupName)` of every group the case creates, parents first.
    groups: &'static [(&'static str, Option<&'static str>)],
    /// The groups the thing is added to.
    member_of: &'static [&'static str],
    /// `(policyName, groupName)` of every policy attached to one of those groups.
    group_policies: &'static [(&'static str, &'static str)],
    /// The policies the thing-qualified read must report, beside the principal's own.
    expected_added: &'static [&'static str],
}

/// A `thingName` extends the answer to the policies of the thing's GROUPS and of the groups
/// above them; the same principal asked without the device reports only its own.
#[tokio::test]
async fn a_named_device_adds_the_policies_of_the_groups_it_belongs_to() {
    // The rows span what the rule generalises over: no group at all, one group, a nested
    // group whose policy sits above it, two unrelated groups, and a group whose policy is
    // also attached to the principal (so it has one effect and is reported once).
    let cases: &[DeviceCase] = &[
        DeviceCase {
            label: "a thing in no group",
            thing_name: "lonely-sensor",
            groups: &[],
            member_of: &[],
            group_policies: &[],
            expected_added: &[],
        },
        DeviceCase {
            label: "a thing in one group with one policy",
            thing_name: "floor-sensor",
            groups: &[("floor", None)],
            member_of: &["floor"],
            group_policies: &[("floor-policy", "floor")],
            expected_added: &["floor-policy"],
        },
        DeviceCase {
            label: "a thing in a nested group, the policy two levels above it",
            thing_name: "cell-sensor",
            groups: &[
                ("plant", None),
                ("line", Some("plant")),
                ("cell", Some("line")),
            ],
            member_of: &["cell"],
            group_policies: &[("plant-policy", "plant"), ("cell-policy", "cell")],
            expected_added: &["cell-policy", "plant-policy"],
        },
        DeviceCase {
            label: "a thing in two unrelated groups",
            thing_name: "shared-sensor",
            groups: &[("east", None), ("maintenance", None)],
            member_of: &["east", "maintenance"],
            group_policies: &[("east-policy", "east"), ("maintenance-policy", "maintenance")],
            expected_added: &["east-policy", "maintenance-policy"],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let certificate = emulator.given_certificate().await;
        // The principal's OWN policy, so every row proves the answer is a UNION and not a
        // replacement.
        let own_document = emulator.given_policy("device-policy").await;
        emulator
            .given_attachment("device-policy", &certificate)
            .await;
        emulator.given_thing(case.thing_name).await;
        emulator
            .given_attached_principal(case.thing_name, &certificate)
            .await;
        for (group, parent) in case.groups {
            emulator.given_thing_group(group, *parent).await;
        }
        for group in case.member_of {
            emulator.given_membership(case.thing_name, group).await;
        }
        let mut group_documents = Vec::new();
        for (policy_name, group) in case.group_policies {
            let document = emulator.given_policy(policy_name).await;
            let group_arn = format!("arn:aws:iot:{REGION}:000000000000:thinggroup/{group}");
            emulator.given_attachment(policy_name, &group_arn).await;
            group_documents.push((*policy_name, document));
        }

        // The principal alone reaches no group: a certificate is not a member of anything.
        let principal_only = emulator
            .get_effective_policies(Some(&certificate), None, None)
            .await;
        assert_eq!(
            principal_only.status, 200,
            "case {}: the principal-only read must answer 200, got {principal_only:?}",
            case.label
        );
        assert_eq!(
            principal_only.policy_names(),
            vec!["device-policy".to_string()],
            "case {}: expected the principal's own policy alone, got {:?}",
            case.label,
            principal_only.policy_names()
        );

        // Named beside the device, the same principal answers with the groups' policies too.
        let qualified = emulator
            .get_effective_policies(Some(&certificate), None, Some(case.thing_name))
            .await;
        assert_eq!(
            qualified.status, 200,
            "case {}: the thing-qualified read must answer 200, got {qualified:?}",
            case.label
        );
        let mut expected: Vec<String> = case
            .expected_added
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        expected.push("device-policy".to_string());
        expected.sort();
        let reported = qualified.policy_names();
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case {}: expected {expected:?}, got {reported:?}",
            case.label
        );
        // A policy that governs through two paths has ONE effect, so it is reported once.
        let deduped: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            reported.len(),
            deduped.len(),
            "case {}: expected each policy once, got {reported:?}",
            case.label
        );
        // Each entry carries the document of the policy it names.
        assert_eq!(
            qualified.document_of("device-policy"),
            Some(own_document.clone()),
            "case {}: expected the principal's own document, got {:?}",
            case.label,
            qualified.body
        );
        for (policy_name, document) in &group_documents {
            if !case.expected_added.contains(policy_name) {
                continue;
            }
            assert_eq!(
                qualified.document_of(policy_name).as_ref(),
                Some(document),
                "case {}/{policy_name}: expected the group policy's document, got {:?}",
                case.label,
                qualified.body
            );
        }
    }
}

/// The device context is READ: asking with the device named changes neither the membership
/// relation nor what a later read reports.
#[tokio::test]
async fn a_thing_qualified_read_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let certificate = emulator.given_certificate().await;
    emulator.given_policy("group-policy").await;
    emulator.given_thing_group("fleet", None).await;
    emulator.given_thing("robot-1").await;
    emulator.given_membership("robot-1", "fleet").await;
    emulator
        .given_attached_principal("robot-1", &certificate)
        .await;
    emulator
        .given_attachment(
            "group-policy",
            &format!("arn:aws:iot:{REGION}:000000000000:thinggroup/fleet"),
        )
        .await;

    let first = emulator
        .get_effective_policies(Some(&certificate), None, Some("robot-1"))
        .await;
    let again = emulator
        .get_effective_policies(Some(&certificate), None, Some("robot-1"))
        .await;
    assert_eq!(
        first.policy_names(),
        vec!["group-policy".to_string()],
        "case the first read: expected the group's policy, got {first:?}"
    );
    assert_eq!(
        again.policy_names(),
        first.policy_names(),
        "case a repeated read: the answer must not move, got {again:?}"
    );

    // Leaving the group takes the policy with it: the answer is computed from the relation
    // on every call.
    let removed = emulator
        .call(
            "PUT",
            "/thing-groups/removeThingFromThingGroup",
            Some(&json!({ "thingName": "robot-1", "thingGroupName": "fleet" })),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case the removal: RemoveThingFromThingGroup must answer 200, got {removed:?}"
    );
    let after = emulator
        .get_effective_policies(Some(&certificate), None, Some("robot-1"))
        .await;
    assert_eq!(
        after.policy_names(),
        Vec::<String>::new(),
        "case after the removal: the group's policy must go, got {after:?}"
    );
}

/// An identity pool the HOLDER does not have is refused as missing, whichever well-formed
/// id names it; a pool it has is answered about.
#[tokio::test]
async fn an_identity_pool_is_resolved_against_the_service_that_holds_pools() {
    let emulator = Emulator::new();
    let held = emulator.given_identity_pool("governed").await;
    emulator.given_policy("pool-policy").await;
    emulator.given_attachment("pool-policy", &held).await;

    let answered = emulator.get_effective_policies(None, Some(&held), None).await;
    assert_eq!(
        answered.status, 200,
        "case a pool the holder has: expected 200, got {answered:?}"
    );
    assert_eq!(
        answered.policy_names(),
        vec!["pool-policy".to_string()],
        "case a pool the holder has: expected its attachment, got {answered:?}"
    );

    // The rows span the well-formed `<region>:<id>` ids a pool is named by that no service
    // holds: the all-zero id of the request's own region, one of another region, a shorter
    // id, and the id of a pool that was created and then deleted.
    let deleted = emulator.given_identity_pool("retired").await;
    let delete = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            "AWSCognitoIdentityService.DeleteIdentityPool",
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTESTIOT/20260101/{REGION}/cognito-identity/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            json!({ "IdentityPoolId": deleted }).to_string(),
        ))
        .expect("a well-formed request");
    let deleted_reply = Reply::of(emulator.router.dispatch(delete).await);
    assert_eq!(
        deleted_reply.status, 200,
        "case the deleted pool: DeleteIdentityPool must answer 200, got {deleted_reply:?}"
    );

    let unheld = [
        format!("{REGION}:00000000-0000-0000-0000-000000000000"),
        "eu-west-1:99998888-7777-6666-5555-444433332222".to_string(),
        format!("{REGION}:pool1"),
        deleted,
    ];
    for pool_id in &unheld {
        let refused = emulator
            .get_effective_policies(None, Some(pool_id), None)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {pool_id:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {pool_id:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            format!("IdentityPool '{pool_id}' not found."),
            "case {pool_id:?}: unexpected refusal, got {refused:?}"
        );
    }

    // A value that is no pool id at all is still the bad-request refusal: the FORM is read
    // before the holder is asked.
    let malformed = emulator
        .get_effective_policies(None, Some("not-a-pool"), None)
        .await;
    assert_eq!(
        malformed.status, 400,
        "case a value that is no pool id: expected HTTP 400, got {malformed:?}"
    );
    assert_eq!(
        malformed.error_code.as_deref(),
        Some("InvalidRequestException"),
        "case a value that is no pool id: expected InvalidRequestException, got {malformed:?}"
    );
}
