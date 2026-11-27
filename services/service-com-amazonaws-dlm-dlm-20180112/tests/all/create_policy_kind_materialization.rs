// SPDX-License-Identifier: CC0-1.0

//! Which materialization `CreateLifecyclePolicy` stores for a new policy, end to
//! end over HTTP.
//!
//! The rule under test: the KIND of policy a create makes is decided by the
//! request — a DEFAULT policy when the request names the default-policy type at
//! the body level or states the `SIMPLIFIED` language in the configuration
//! structure, a CUSTOM policy otherwise — and the configuration the create stores
//! is materialized for THAT kind, never for the kind the submitted structure's own
//! `PolicyLanguage` happens to name. So a default policy reports the language its
//! kind names (`PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default
//! policy", "`STANDARD` To create a custom policy") and none of the members
//! documented `[Custom snapshot and AMI policies only]`, and every way of asking
//! for the same kind of policy reads back ONE configuration.
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the only path on which a
//! create answered 200 can be seen to have stored what a read reports.
//!
//! The rows span what the rule generalises over: both members of
//! `DefaultPolicyTypeValues` as the kind of default policy; all the ways a create
//! may state that one kind (the body member alone, the body member beside a
//! structure stating no language, beside a structure restating `SIMPLIFIED`,
//! beside a structure stating the CUSTOM language, and the structure alone); and
//! each kind of custom policy (a snapshot policy stating no language, the same one
//! restating `STANDARD`, an AMI policy and an event-based policy).
//!
//! The optional parameters a custom snapshot or AMI policy carries, which the last
//! three tests below pin: `PolicyDetails$Parameters` and each of its members are
//! optional and non-defaulted, so the stored configuration carries exactly the
//! optional parameters the request stated — the structure absent when the request
//! stated none, and only the members it stated when it stated one — and a read
//! round-trips into an update unchanged. Its rows differ along the policy type, the
//! target resource type, whether the request stated the parameters structure at
//! all, which members it stated, and the value of each.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// A destination Region for the event-based policy's copy action, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "eu-west-1";

/// The language a DEFAULT policy's configuration names.
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The language a CUSTOM policy's configuration names.
const CUSTOM_POLICY_LANGUAGE: &str = "STANDARD";

/// Every `DefaultPolicyTypeValues` member, from
/// `smithy_query(query="shape", shape="DefaultPolicyTypeValues")`: the kind of
/// default policy each one names is `CreateLifecyclePolicyRequest$DefaultPolicy`'s
/// documentation — `VOLUME` creates snapshots of volumes, `INSTANCE` creates
/// EBS-backed AMIs from instances.
const DEFAULT_POLICY_TYPE_VALUES: &[&str] = &["INSTANCE", "VOLUME"];

/// The `PolicyTypeValues` member that manages EBS snapshots, which
/// `PolicyDetails$PolicyType` documents as the default ("The default is
/// `EBS_SNAPSHOT_MANAGEMENT`").
const SNAPSHOT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The `PolicyTypeValues` member that "manages the lifecycle of EBS-backed AMIs".
const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `PolicyTypeValues` member of a policy that "performs specific actions when
/// a defined event occurs".
const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// The in-Region `ResourceLocationValues` member a custom snapshot or AMI policy's
/// configuration carries when the request states no `PolicyDetails$ResourceLocations`
/// ("If the source resources are located in a Region, specify `CLOUD`").
const CLOUD_LOCATION: &str = "CLOUD";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let answer = call(router, "POST", "/policies", Some(request)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

async fn get(router: &MultiServiceRouter, case: &str, id: &str) -> Answer {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The `LifecyclePolicySummary` `GetLifecyclePolicies` reports for `id`.
async fn summary(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = call(router, "GET", "/policies", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Policies"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|policy| policy["PolicyId"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetLifecyclePolicies must report {id}"))
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// One create request, and what the kind of policy it asks for requires of the
/// stored configuration a read reports.
struct Case {
    /// What is particular about this row.
    case: String,
    /// The create request.
    request: Value,
    /// The kind of policy it asks for: every row sharing a group reads back ONE
    /// configuration, whichever way it asked.
    group: String,
    /// True when the request asks for a DEFAULT policy, as both read projections
    /// must report.
    default_policy: bool,
    /// The `PolicyLanguage` the kind of policy names.
    language: &'static str,
    /// The `PolicyType` the policy reports.
    policy_type: &'static str,
    /// `PolicyDetails$ResourceLocations` the read must report, and `Value::Null`
    /// for a policy that has none: the member is documented "[Custom snapshot and
    /// AMI policies only]" (`smithy_query(query="doc", shape="PolicyDetails",
    /// member="ResourceLocations")`).
    resource_locations: Value,
}

fn create_request(description: &str, members: Value) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
    });
    for (name, value) in members.as_object().expect("the members are an object") {
        request[name.as_str()] = value.clone();
    }
    request
}

/// A custom snapshot policy's configuration: `Schedules`' creation interval stays
/// inside the supported set (`interval_validation.rs` owns that rule).
fn custom_snapshot_details(resource_type: &str) -> Value {
    json!({
        "ResourceTypes": [resource_type],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom AMI policy's configuration.
fn custom_image_details() -> Value {
    let mut details = custom_snapshot_details("INSTANCE");
    details["PolicyType"] = json!(IMAGE_POLICY_TYPE);
    details
}

/// An event-based policy's configuration, which configures no target and no
/// schedule, so it has no resource locations either.
fn event_based_details() -> Value {
    json!({
        "PolicyType": EVENT_BASED_POLICY_TYPE,
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": "^.*$",
            },
        },
        "Actions": [{
            "Name": "copy",
            "CrossRegionCopy": [{
                "Target": OTHER_REGION,
                "EncryptionConfiguration": {"Encrypted": false},
                "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
            }],
        }],
    })
}

/// The table: for each member of `DefaultPolicyTypeValues`, every way a create may
/// ask for that one kind of default policy, plus each kind of custom policy.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for default_type in DEFAULT_POLICY_TYPE_VALUES {
        let policy_type = if *default_type == "INSTANCE" {
            IMAGE_POLICY_TYPE
        } else {
            SNAPSHOT_POLICY_TYPE
        };
        let group = format!("a default {default_type} policy");
        // The five ways to ask for this one kind of default policy. The last of
        // them states the kind in the structure alone; the one before it states
        // the CUSTOM language beside the body member that creates a default
        // policy, so the kind can only come from the request's kind-naming
        // member.
        let askings: Vec<(String, Value)> = vec![
            (
                "naming DefaultPolicy alone".to_string(),
                json!({"DefaultPolicy": default_type}),
            ),
            (
                "naming DefaultPolicy beside a structure stating no language".to_string(),
                json!({"DefaultPolicy": default_type, "PolicyDetails": {}}),
            ),
            (
                "naming DefaultPolicy beside a structure restating the default language"
                    .to_string(),
                json!({
                    "DefaultPolicy": default_type,
                    "PolicyDetails": {"PolicyLanguage": DEFAULT_POLICY_LANGUAGE},
                }),
            ),
            (
                "naming DefaultPolicy beside a structure stating the custom language".to_string(),
                json!({
                    "DefaultPolicy": default_type,
                    "PolicyDetails": {"PolicyLanguage": CUSTOM_POLICY_LANGUAGE},
                }),
            ),
            (
                "stating the default language and the resource type in the structure alone"
                    .to_string(),
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        "ResourceType": default_type,
                    },
                }),
            ),
        ];
        for (asking, members) in askings {
            cases.push(Case {
                case: format!("{group}, {asking}"),
                request: create_request(&format!("default {default_type} policy"), members),
                group: group.clone(),
                default_policy: true,
                language: DEFAULT_POLICY_LANGUAGE,
                policy_type,
                resource_locations: Value::Null,
            });
        }
    }

    for (asking, details) in [
        ("stating no language", custom_snapshot_details("VOLUME")),
        ("restating the custom language", {
            let mut details = custom_snapshot_details("VOLUME");
            details["PolicyLanguage"] = json!(CUSTOM_POLICY_LANGUAGE);
            details
        }),
    ] {
        cases.push(Case {
            case: format!("a custom snapshot policy {asking}"),
            request: create_request(
                "custom snapshot policy",
                json!({"PolicyDetails": details}),
            ),
            group: "a custom snapshot policy".to_string(),
            default_policy: false,
            language: CUSTOM_POLICY_LANGUAGE,
            policy_type: SNAPSHOT_POLICY_TYPE,
            resource_locations: json!([CLOUD_LOCATION]),
        });
    }

    cases.push(Case {
        case: "a custom AMI policy".to_string(),
        request: create_request(
            "custom ami policy",
            json!({"PolicyDetails": custom_image_details()}),
        ),
        group: "a custom AMI policy".to_string(),
        default_policy: false,
        language: CUSTOM_POLICY_LANGUAGE,
        policy_type: IMAGE_POLICY_TYPE,
        resource_locations: json!([CLOUD_LOCATION]),
    });

    cases.push(Case {
        case: "a custom event-based policy".to_string(),
        request: create_request(
            "event based copies",
            json!({"PolicyDetails": event_based_details()}),
        ),
        group: "a custom event-based policy".to_string(),
        default_policy: false,
        language: CUSTOM_POLICY_LANGUAGE,
        policy_type: EVENT_BASED_POLICY_TYPE,
        resource_locations: Value::Null,
    });

    cases
}

/// A created policy reports the configuration materialized for the kind of policy
/// the request asked for: that kind's `PolicyLanguage`, its `PolicyType`, and the
/// `[Custom snapshot and AMI policies only]` locations only where that kind has
/// them.
#[tokio::test]
async fn a_created_policy_reports_the_materialization_of_its_kind() {
    let router = emulator();

    for expected in cases() {
        let case = expected.case.as_str();
        let id = create(&router, case, expected.request.clone()).await;
        let read = get(&router, case, &id).await;

        assert_eq!(
            at(read.policy(), "/DefaultPolicy"),
            json!(expected.default_policy),
            "case {case:?}: expected DefaultPolicy {}, got {:?}",
            expected.default_policy,
            at(read.policy(), "/DefaultPolicy")
        );
        assert_eq!(
            at(read.policy(), "/PolicyDetails/PolicyLanguage"),
            json!(expected.language),
            "case {case:?}: expected PolicyLanguage {:?}, got {:?}",
            expected.language,
            at(read.policy(), "/PolicyDetails/PolicyLanguage")
        );
        assert_eq!(
            at(read.policy(), "/PolicyDetails/PolicyType"),
            json!(expected.policy_type),
            "case {case:?}: expected PolicyType {:?}, got {:?}",
            expected.policy_type,
            at(read.policy(), "/PolicyDetails/PolicyType")
        );
        assert_eq!(
            at(read.policy(), "/PolicyDetails/ResourceLocations"),
            expected.resource_locations,
            "case {case:?}: expected ResourceLocations {:?}, got {:?}",
            expected.resource_locations,
            at(read.policy(), "/PolicyDetails/ResourceLocations")
        );

        // The two read projections of one policy agree about its kind and type.
        let listed = summary(&router, case, &id).await;
        assert_eq!(
            at(&listed, "/DefaultPolicy"),
            json!(expected.default_policy),
            "case {case:?}: the summary must report DefaultPolicy {}, got {:?}",
            expected.default_policy,
            at(&listed, "/DefaultPolicy")
        );
        assert_eq!(
            at(&listed, "/PolicyType"),
            json!(expected.policy_type),
            "case {case:?}: the summary must report PolicyType {:?}, got {:?}",
            expected.policy_type,
            at(&listed, "/PolicyType")
        );
    }
}

/// Every way of asking for the SAME kind of policy reads back ONE configuration:
/// how the request stated the kind cannot change which members the stored
/// configuration carries.
#[tokio::test]
async fn every_way_of_asking_for_one_kind_of_policy_reads_back_one_configuration() {
    let router = emulator();
    // group -> (the first row of the group, its reported configuration).
    let mut reported: BTreeMap<String, (String, Value)> = BTreeMap::new();

    for expected in cases() {
        let case = expected.case.clone();
        let id = create(&router, &case, expected.request.clone()).await;
        let details = at(get(&router, &case, &id).await.policy(), "/PolicyDetails");

        match reported.get(&expected.group) {
            Some((first, first_details)) => assert_eq!(
                &details, first_details,
                "case {case:?}: {} must read back the configuration {first:?} reads back \
                 ({first_details:?}), got {details:?}",
                expected.group
            ),
            None => {
                reported.insert(expected.group.clone(), (case, details));
            }
        }
    }

    assert!(
        reported.len() >= DEFAULT_POLICY_TYPE_VALUES.len(),
        "the table must cover a kind of policy per DefaultPolicyTypeValues member, covered {:?}",
        reported.keys().collect::<Vec<_>>()
    );
}

/// One policy and the `PolicyDetails$Parameters` every read of it reports.
struct ParameterCase {
    /// What is particular about this row.
    case: &'static str,
    /// The configuration, without its optional parameters.
    details: Value,
    /// The parameters the request states, `None` for a request that states no
    /// `Parameters` structure at all.
    stated: Option<Value>,
}

impl ParameterCase {
    /// The whole `Parameters` structure a read must report: the structure the
    /// request stated, and nothing — `Value::Null` — for a request that stated
    /// none. Every member of `Parameters` is optional and non-defaulted
    /// (`smithy_query(query="shape", shape="Parameters")`), so a read reports the
    /// stated structure member for member.
    fn reported(&self) -> Value {
        self.stated.clone().unwrap_or(Value::Null)
    }
}

/// The configuration a row submits: its details with the parameters it states.
fn details_with_parameters(row: &ParameterCase) -> Value {
    let mut details = row.details.clone();
    if let Some(stated) = row.stated.clone() {
        details["Parameters"] = stated;
    }
    details
}

/// The table: each combination of policy type and target resource type the
/// documented parameters are admitted for, plus one that admits none, each stating
/// no structure, an empty structure, and each admitted member at both of its
/// values.
fn parameter_cases() -> Vec<ParameterCase> {
    let instance_snapshot_details = || custom_snapshot_details("INSTANCE");
    vec![
        ParameterCase {
            case: "a custom AMI policy stating no Parameters",
            details: custom_image_details(),
            stated: None,
        },
        ParameterCase {
            case: "a custom AMI policy stating an empty Parameters structure",
            details: custom_image_details(),
            stated: Some(json!({})),
        },
        ParameterCase {
            case: "a custom AMI policy that asks for its instances to be rebooted",
            details: custom_image_details(),
            stated: Some(json!({"NoReboot": false})),
        },
        ParameterCase {
            case: "a custom AMI policy stating the NoReboot its documentation calls the default",
            details: custom_image_details(),
            stated: Some(json!({"NoReboot": true})),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, stating no Parameters",
            details: instance_snapshot_details(),
            stated: None,
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, stating an empty Parameters \
                   structure",
            details: instance_snapshot_details(),
            stated: Some(json!({})),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, stating only the data volume tags",
            details: instance_snapshot_details(),
            stated: Some(json!({"ExcludeDataVolumeTags": [{"Key": "data", "Value": "keep"}]})),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, excluding the boot volume",
            details: instance_snapshot_details(),
            stated: Some(json!({"ExcludeBootVolume": true})),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, stating the ExcludeBootVolume \
                   its documentation calls the default",
            details: instance_snapshot_details(),
            stated: Some(json!({"ExcludeBootVolume": false})),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting instances, stating both members it admits",
            details: instance_snapshot_details(),
            stated: Some(json!({
                "ExcludeBootVolume": true,
                "ExcludeDataVolumeTags": [{"Key": "data", "Value": "keep"}],
            })),
        },
        ParameterCase {
            case: "a custom snapshot policy targeting volumes, stating no Parameters",
            details: custom_snapshot_details("VOLUME"),
            stated: None,
        },
        ParameterCase {
            case: "a custom snapshot policy targeting volumes, stating an empty Parameters \
                   structure",
            details: custom_snapshot_details("VOLUME"),
            stated: Some(json!({})),
        },
        ParameterCase {
            case: "a custom event-based policy",
            details: event_based_details(),
            stated: None,
            // `Parameters` is "[Custom snapshot and AMI policies only]", so an
            // event-based policy states none either way.
        },
    ]
}

/// A created policy reports the optional parameters its request STATED and no
/// others: the structure member for member when the request carried one, and no
/// `Parameters` member at all when it carried none — whichever combination of
/// policy type and target resource type the policy is, since every member of
/// `Parameters` is optional and non-defaulted.
#[tokio::test]
async fn a_created_policy_reports_the_parameters_its_request_stated() {
    let router = emulator();

    for row in parameter_cases() {
        let case = row.case;
        let request = create_request(
            "optional parameters as stated",
            json!({"PolicyDetails": details_with_parameters(&row)}),
        );
        let id = create(&router, case, request).await;
        let read = get(&router, case, &id).await;
        let reported = at(read.policy(), "/PolicyDetails/Parameters");
        assert_eq!(
            reported,
            row.reported(),
            "case {case:?}: expected Parameters {:?}, got {reported:?}",
            row.reported()
        );

        // Reading twice is reading one record: what the create stored, not
        // something synthesized per read.
        let again = at(get(&router, case, &id).await.policy(), "/PolicyDetails/Parameters");
        assert_eq!(
            again, reported,
            "case {case:?}: two reads of one policy must report one Parameters, got {again:?} \
             after {reported:?}"
        );
    }
}

/// And an UPDATE that replaces the configuration stores the same thing: the
/// replacement is "the configuration of the lifecycle policy", so a read after it
/// reports the optional parameters the replacement stated, exactly as a read after
/// the create does.
#[tokio::test]
async fn a_replaced_configuration_reports_the_parameters_it_stated() {
    let router = emulator();

    for row in parameter_cases() {
        let case = row.case;
        let configuration = details_with_parameters(&row);
        let request = create_request(
            "optional parameters as stated",
            json!({"PolicyDetails": configuration.clone()}),
        );
        let id = create(&router, case, request).await;

        let patched = call(
            &router,
            "PATCH",
            &format!("/policies/{id}"),
            Some(json!({"PolicyDetails": configuration})),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
            patched.status, patched.body
        );

        let reported = at(get(&router, case, &id).await.policy(), "/PolicyDetails/Parameters");
        assert_eq!(
            reported,
            row.reported(),
            "case {case:?}: after an update expected Parameters {:?}, got {reported:?}",
            row.reported()
        );
    }
}

/// The invariant that follows: the configuration a read reports is one an update
/// accepts back unchanged, so feeding a read of a policy straight into
/// `UpdateLifecyclePolicy` leaves the configuration exactly as it was — no optional
/// parameter appears, disappears or changes value on the round trip.
#[tokio::test]
async fn a_read_configuration_round_trips_through_an_update() {
    let router = emulator();

    for row in parameter_cases() {
        let case = row.case;
        let request = create_request(
            "optional parameters as stated",
            json!({"PolicyDetails": details_with_parameters(&row)}),
        );
        let id = create(&router, case, request).await;
        let stored = at(get(&router, case, &id).await.policy(), "/PolicyDetails");

        let patched = call(
            &router,
            "PATCH",
            &format!("/policies/{id}"),
            Some(json!({"PolicyDetails": stored.clone()})),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: a read configuration must be accepted back, got {} with {:?}",
            patched.status, patched.body
        );

        let after = at(get(&router, case, &id).await.policy(), "/PolicyDetails");
        assert_eq!(
            after, stored,
            "case {case:?}: expected the read configuration {stored:?} back unchanged, got \
             {after:?}"
        );
    }
}
