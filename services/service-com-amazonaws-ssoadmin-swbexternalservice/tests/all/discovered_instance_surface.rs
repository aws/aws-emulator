// SPDX-License-Identifier: CC0-1.0

//! The instance a client DISCOVERS, and the surface it serves — end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test: the IAM Identity Center instance an account already holds — the
//! one `ListInstances` hands out with no create first — is an ACCOUNT instance, which
//! serves no multi-account permissions. So `DescribeInstance` reports
//! `PermissionSetsEnabled: false` for it, and every operation of the permission-set,
//! account-assignment and ABAC attribute-configuration surface is refused on that ARN with
//! the modelled `ValidationException` (HTTP 400) BEFORE any resource lookup: a read of the
//! surface included, so neither an empty configuration nor a `ResourceNotFoundException`
//! stands in for the refusal, and a refused write persists nothing.
//! `UpdateInstance {PermissionSetsEnabled: true}` — "Enables permission sets for this
//! Identity Center instance" — is the one documented call that opens the surface, after
//! which the whole lifecycle runs against that same discovered ARN.
//!
//! The tables span what the rule generalises over: the REGION the discovery and the calls
//! that follow it are signed for, EVERY implemented operation of the withdrawn surface
//! (reads as well as writes) against inputs that vary the permission set, the account, the
//! principal, the page size, the request id and the attribute key — the table
//! `account_instance_restriction.rs` owns, since the restriction is one rule over both the
//! instance a caller creates and the instance an account discovers — and the SHAPE of the
//! permission set the lifecycle is driven with once permission sets are enabled.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, and nothing here writes to the store: the instance under test is the one
//! the API itself hands out. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_instance_restriction::{instance_id_of, surface_calls, VARIANTS};
use crate::ssoadmin_permission_sets::{call, call_in_region, error_code, router};

/// The Regions a discovery is signed for. SOURCE: `RegionName`'s modelled pattern
/// `^([a-z]+-){2,3}\d$` — rows span both the two-group and the three-group form it admits,
/// and none of them is the Region the harness signs for by default.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-2", "ap-southeast-2", "us-gov-east-1"];

/// One shape of permission set the lifecycle is driven with once permission sets are
/// enabled on the discovered instance: the `CreatePermissionSet` members beside
/// `InstanceArn`.
struct FlowCase {
    label: &'static str,
    /// `PermissionSetName`: `length(min: 1, max: 32)`, `^[\w+=,.@-]+$`.
    name: String,
    /// `PermissionSetDescription`: `length(min: 1, max: 700)`.
    description: Option<String>,
    /// `Duration`: an ISO-8601 duration, `length(min: 1, max: 100)`.
    session_duration: Option<&'static str>,
    /// The principal the assignment of the flow is granted to.
    principal_type: &'static str,
    principal_id: &'static str,
    /// `TargetId`: `^\d{12}$`.
    account_id: &'static str,
}

/// The rows: they differ along the members the create carries, the published bounds of the
/// name's length, both `PrincipalType` values, both forms the `PrincipalId` pattern admits
/// and the bounds of a 12-digit account id.
fn flow_cases() -> Vec<FlowCase> {
    vec![
        FlowCase {
            label: "a bare create",
            name: "DemoSet".to_string(),
            description: None,
            session_duration: None,
            principal_type: "GROUP",
            principal_id: "f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
            account_id: "123456789012",
        },
        FlowCase {
            // `PermissionSetName` is `length(min: 1, max: 32)`: one character is its
            // published minimum.
            label: "a name of the published minimum length, with a description",
            name: "A".to_string(),
            description: Some("read-only access for the audit team".to_string()),
            session_duration: Some("PT1H"),
            principal_type: "USER",
            principal_id: "9067f3a1c2-f81d4fae-7dec-11d0-a765-00a0c91e6bf7",
            account_id: "000000000000",
        },
        FlowCase {
            label: "a name of the published maximum length, with every member set",
            name: "n".repeat(32),
            description: Some("d".repeat(700)),
            session_duration: Some("PT12H"),
            principal_type: "USER",
            principal_id: "1b2d3c4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f",
            account_id: "999999999999",
        },
        FlowCase {
            label: "a name spanning the charset the pattern admits",
            name: "ops+team=eu.west-1@x_1".to_string(),
            description: Some("x".to_string()),
            session_duration: Some("PT90M"),
            principal_type: "GROUP",
            principal_id: "11111111-2222-4333-8444-555555555555",
            account_id: "123456789012",
        },
    ]
}

impl FlowCase {
    /// The `CreatePermissionSet` body this row sends under `instance_arn`.
    fn create_body(&self, instance_arn: &str) -> Value {
        let mut body = json!({"InstanceArn": instance_arn, "Name": self.name});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(session_duration) = self.session_duration {
            body["SessionDuration"] = json!(session_duration);
        }
        body
    }

    /// The `CreateAccountAssignment` / `DeleteAccountAssignment` body this row sends for
    /// `permission_set_arn`.
    fn assignment_body(&self, instance_arn: &str, permission_set_arn: &str) -> Value {
        json!({
            "InstanceArn": instance_arn,
            "TargetId": self.account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": self.principal_type,
            "PrincipalId": self.principal_id,
        })
    }
}

/// The `InstanceArn` a client discovers, read the way a client reads it: `ListInstances`
/// signed for `region`, with no prior call having enabled anything.
async fn discovered_instance_arn(router: &MultiServiceRouter, region: &str) -> String {
    let (status, body) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {region:?}: ListInstances expected 200, got {status} {body}"
    );
    body["Instances"][0]["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region:?}: ListInstances handed out no instance: {body}"))
        .to_string()
}

/// `DescribeInstance` for `instance_arn`, which must be served whatever the instance's
/// kind: the restriction belongs to the surface built on permission sets, not to reading
/// the instance.
async fn describe_instance(router: &MultiServiceRouter, case: &str, instance_arn: &str) -> Value {
    let (status, body) =
        call(router, "DescribeInstance", &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// Assert one call of the withdrawn surface was refused as the account-instance
/// restriction: HTTP 400, error code `ValidationException`, and the refusal says why.
fn assert_withdrawn(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: the surface is withdrawn on the discovered instance, so this call \
         must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: a permanent instance-kind restriction is a ValidationException, \
         never a not-found, got {body}"
    );
    let message = body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains("account instances"),
        "case {case:?}: the refusal must say the operation is unsupported for account \
         instances, got {message:?}"
    );
}

/// One call that must be SERVED: 200, and so never the account-instance refusal. Answers
/// the response body for the caller to read on.
async fn served(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must be served once permission sets are enabled, got \
         {status} {answer}"
    );
    answer
}

/// The discovered instance reports permission sets DISABLED and refuses a create of one,
/// whatever Region the client discovered it from.
#[tokio::test]
async fn the_discovered_instance_reports_permission_sets_disabled_in_every_region() {
    for region in REGIONS {
        // A fresh store per row: the instance under test is the one the account hands out
        // when nothing has yet decided its Identity Center enablement.
        let router = router();
        let instance_arn = discovered_instance_arn(&router, region).await;

        let described = describe_instance(&router, region, &instance_arn).await;
        assert_eq!(
            described["PermissionSetsEnabled"],
            Value::Bool(false),
            "case {region:?}: the discovered instance is an account instance, so it must \
             report permission sets disabled, got {:?}",
            described["PermissionSetsEnabled"]
        );
        assert_eq!(
            described["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {region:?}: the describe must answer about the discovered instance, got \
             {described}"
        );

        let (status, created) = call_in_region(
            &router,
            region,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": "DiscoveredSet"}),
        )
        .await;
        assert_withdrawn(region, status, &created);
    }
}

/// THE RULE, over the whole table: on the instance a client discovers, every operation of
/// the permission-set / account-assignment / ABAC surface answers 400 `ValidationException`
/// — whatever permission set, account, principal, page size, request id or attribute key
/// the request names, since no resource of that surface can exist on it.
#[tokio::test]
async fn every_operation_of_the_withdrawn_surface_is_refused_on_the_discovered_instance() {
    let router = router();
    let instance_arn = discovered_instance_arn(&router, "us-east-1").await;

    for variant in VARIANTS {
        for surface_call in surface_calls(&instance_arn, variant) {
            let (status, body) = call(&router, surface_call.operation, &surface_call.body).await;
            assert_withdrawn(&surface_call.label, status, &body);
        }
    }
}

/// A refused call of the surface WRITES NOTHING: the ABAC create the recorded client sends
/// is refused rather than persisted, so the read that follows it is the same refusal — not
/// a configuration, and not a not-found that would tell the client to create one — and the
/// instance a later caller discovers is byte-identical to the one before the refusals.
#[tokio::test]
async fn a_refused_call_persists_nothing_on_the_discovered_instance() {
    let router = router();
    let instance_arn = discovered_instance_arn(&router, "us-east-1").await;
    let before = describe_instance(&router, "before the refusals", &instance_arn).await;

    for variant in VARIANTS {
        let configuration = json!({
            "InstanceArn": instance_arn,
            "InstanceAccessControlAttributeConfiguration": {
                "AccessControlAttributes": [{
                    "Key": variant.attribute_key,
                    "Value": {"Source": ["${path:enterprise.department}"]},
                }],
            },
        });
        let (status, body) = call(
            &router,
            "CreateInstanceAccessControlAttributeConfiguration",
            &configuration,
        )
        .await;
        assert_withdrawn(variant.label, status, &body);

        let (status, read_back) = call(
            &router,
            "DescribeInstanceAccessControlAttributeConfiguration",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_withdrawn(variant.label, status, &read_back);
        assert_eq!(
            read_back["InstanceAccessControlAttributeConfiguration"],
            Value::Null,
            "case {:?}: a refused create persists no configuration to report, got \
             {read_back}",
            variant.label
        );
    }

    let after = describe_instance(&router, "after the refusals", &instance_arn).await;
    assert_eq!(
        before, after,
        "a refused call must change nothing a later caller sees, got {before} then {after}"
    );
}

/// `UpdateInstance {PermissionSetsEnabled: true}` is the one call that opens the surface on
/// the discovered instance: after it the whole lifecycle runs against that same ARN, each
/// step answered from the state the step before wrote, over every row of the table.
#[tokio::test]
async fn enabling_permission_sets_opens_the_lifecycle_on_the_discovered_instance() {
    for flow in flow_cases() {
        let case = flow.label;
        let router = router();
        let instance_arn = discovered_instance_arn(&router, "us-east-1").await;

        served(
            &router,
            case,
            "UpdateInstance",
            &json!({"InstanceArn": instance_arn, "PermissionSetsEnabled": true}),
        )
        .await;
        let described = describe_instance(&router, case, &instance_arn).await;
        assert_eq!(
            described["PermissionSetsEnabled"],
            Value::Bool(true),
            "case {case:?}: the enablement must be stored on the instance, got {:?}",
            described["PermissionSetsEnabled"]
        );

        let created = served(
            &router,
            case,
            "CreatePermissionSet",
            &flow.create_body(&instance_arn),
        )
        .await;
        let permission_set_arn = created["PermissionSet"]["PermissionSetArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no minted PermissionSetArn: {created}"))
            .to_string();
        let instance_id = instance_id_of(&instance_arn);
        assert!(
            permission_set_arn
                .starts_with(&format!("arn:aws:sso:::permissionSet/{instance_id}/ps-")),
            "case {case:?}: the minted ARN must sit under the discovered instance, got \
             {permission_set_arn:?}"
        );

        // ROUND TRIP: the describe answers from the stored record, not from the request.
        let described = served(
            &router,
            case,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            described["PermissionSet"], created["PermissionSet"],
            "case {case:?}: the describe must report what the create stored, got {:?} vs {:?}",
            described["PermissionSet"], created["PermissionSet"]
        );
        assert_eq!(
            described["PermissionSet"]["Name"].as_str(),
            Some(flow.name.as_str()),
            "case {case:?}: expected the created Name, got {:?}",
            described["PermissionSet"]["Name"]
        );
        assert_eq!(
            described["PermissionSet"]["Description"].as_str(),
            flow.description.as_deref(),
            "case {case:?}: expected the created Description, got {:?}",
            described["PermissionSet"]["Description"]
        );
        assert_eq!(
            described["PermissionSet"]["SessionDuration"].as_str(),
            flow.session_duration,
            "case {case:?}: expected the created SessionDuration, got {:?}",
            described["PermissionSet"]["SessionDuration"]
        );

        let listed = served(
            &router,
            case,
            "ListPermissionSets",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        let arns: Vec<&str> = listed["PermissionSets"]
            .as_array()
            .map(|rows| rows.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            arns,
            vec![permission_set_arn.as_str()],
            "case {case:?}: the enumeration must report exactly the stored set, got {listed}"
        );

        let assignment = flow.assignment_body(&instance_arn, &permission_set_arn);
        let accepted = served(&router, case, "CreateAccountAssignment", &assignment).await;
        let request_id = accepted["AccountAssignmentCreationStatus"]["RequestId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RequestId to poll by: {accepted}"))
            .to_string();

        let polled = served(
            &router,
            case,
            "DescribeAccountAssignmentCreationStatus",
            &json!({
                "InstanceArn": instance_arn,
                "AccountAssignmentCreationRequestId": request_id,
            }),
        )
        .await;
        assert_eq!(
            polled["AccountAssignmentCreationStatus"]["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {case:?}: the poll must report the operation it names, got {polled}"
        );

        let assignments = served(
            &router,
            case,
            "ListAccountAssignments",
            &json!({
                "InstanceArn": instance_arn,
                "AccountId": flow.account_id,
                "PermissionSetArn": permission_set_arn,
            }),
        )
        .await;
        let principals: Vec<&str> = assignments["AccountAssignments"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["PrincipalId"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            principals,
            vec![flow.principal_id],
            "case {case:?}: the enumeration must report the granted assignment, got \
             {assignments}"
        );

        served(&router, case, "DeleteAccountAssignment", &assignment).await;
        served(
            &router,
            case,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;

        // The delete removed the record, so the read that answered from it is now a
        // missing resource — never the account-instance refusal, since this instance now
        // serves the surface.
        let (status, gone) = call(
            &router,
            "DescribePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted permission set is a missing resource, got {status} {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {gone}"
        );
    }
}
