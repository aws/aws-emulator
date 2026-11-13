// SPDX-License-Identifier: CC0-1.0

//! The primary identifier of a type whose schema points `primaryIdentifier` at a READ-ONLY
//! ARN — one the service that owns the resource assigns.
//!
//! The rule every case here holds the service to: **`ProgressEvent$Identifier` is "The
//! primary identifier for the resource" — the property the resource type's own registry
//! schema points `primaryIdentifier` at — so for a type whose schema points it at an ARN the
//! owning service assigns, every answer reports that ARN, built from the partition, the
//! account, the Region the owning service's ARNs carry one in, and the parts of the resource
//! the desired state names; the resource model carries it under that property, that ARN is
//! what `GetResource`, `ListResources`, `UpdateResource` and `DeleteResource` accept, and it
//! is the handle the OWNING service's own API resolves the resource by.**
//!
//! Two types cover the rule from both sides of the Region question: `AWS::IAM::ManagedPolicy`
//! (`/properties/PolicyArn`), whose owning service is global so the ARN carries no Region but
//! does carry the policy's path, and `AWS::StepFunctions::StateMachine`
//! (`/properties/Arn`), whose ARN carries the Region the call was signed for. Within each,
//! the rows differ along the parts of the ARN the caller chooses — the name, and for a policy
//! the path — so what is asserted is the ARN FORM rather than any one recorded value.
//!
//! `arn_identified_resources` holds the same rule for `AWS::SNS::Topic` and
//! `resource_identity` for the other kinds of primary identifier, so what the suites assert
//! together is where a resource's identity COMES FROM rather than anything about one type.

use serde_json::{json, Value};

use super::emulator::{element, progress, text, Emulator, SIGNING_REGION, STATUS_FAILED};

/// The type whose primary identifier is the ARN IAM assigns a customer managed policy.
const MANAGED_POLICY: &str = "AWS::IAM::ManagedPolicy";

/// The property that type's schema points `primaryIdentifier` at.
const POLICY_ARN: &str = "PolicyArn";

/// The type whose primary identifier is the ARN Step Functions assigns a state machine.
const STATE_MACHINE: &str = "AWS::StepFunctions::StateMachine";

/// The property that type's schema points `primaryIdentifier` at.
const ARN: &str = "Arn";

/// One IAM JSON policy, as a `PolicyDocument` carries it: the elements an identity-based
/// policy's statements are required to name.
fn policy_document(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": action,
            "Resource": "arn:aws:s3:::example/*",
        }],
    })
}

/// The ARN IAM assigns the policy `name` filed under `path`: the fixed text `arn:`, `:iam::`
/// and `:policy`, with the partition and the account from this emulator's own configuration
/// and the path and the name from the request.
fn expected_policy_arn(path: &str, name: &str) -> String {
    engine::emulator_arn("iam", "", &format!("policy{path}{name}"))
}

/// The ARN Step Functions assigns the state machine `name` in `region`.
fn expected_state_machine_arn(region: &str, name: &str) -> String {
    engine::emulator_arn("states", region, &format!("stateMachine:{name}"))
}

/// One row of the managed-policy table.
struct PolicyCase {
    /// The name the create chooses, which the ARN is built from.
    name: String,
    /// The path the create files the policy under, or `None` where it names none and the
    /// policy is filed at the account root.
    path: Option<&'static str>,
    /// The description the create carries, where it carries one.
    description: Option<&'static str>,
}

impl PolicyCase {
    /// The path the policy is actually filed under: the one the model names, or the account
    /// root a model naming none leaves it at.
    fn path(&self) -> &str {
        self.path.unwrap_or("/")
    }

    /// The desired state this row creates the policy with.
    fn desired(&self) -> Value {
        let mut desired = json!({
            "ManagedPolicyName": self.name,
            "PolicyDocument": policy_document("s3:GetObject"),
        });
        if let Some(path) = self.path {
            desired["Path"] = json!(path);
        }
        if let Some(description) = self.description {
            desired["Description"] = json!(description);
        }
        desired
    }
}

#[tokio::test]
async fn the_identifier_is_the_policy_arn_whatever_name_and_path_the_create_chose() {
    let emulator = Emulator::with_iam();
    // The name and the path are the only parts of the ARN the caller chooses; the partition
    // and the account come from this emulator's own configuration, and IAM is global, so the
    // Region segment is empty. The rows span a plain name, a name carrying the punctuation
    // `policyNameType` admits, the published maximum name length and the shortest name — and
    // across them the account root, a one-segment path and a nested one.
    let rows = [
        PolicyCase {
            name: "orders-reader".to_string(),
            path: None,
            description: None,
        },
        PolicyCase {
            name: "Orders.Reader+1@eu=west,2".to_string(),
            path: Some("/app/"),
            description: Some("reads the orders bucket"),
        },
        PolicyCase {
            name: "p".repeat(128),
            path: None,
            description: None,
        },
        PolicyCase {
            name: "x".to_string(),
            path: Some("/team/platform/"),
            description: None,
        },
    ];

    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for case in &rows {
        let event = emulator.create_ok(MANAGED_POLICY, &case.desired(), None).await;
        accepted.push(text(&event, "RequestToken"));
    }

    for (case, request_token) in rows.iter().zip(&accepted) {
        let expected = expected_policy_arn(case.path(), &case.name);
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        assert_eq!(
            identifier, expected,
            "name {:?} at path {:?}: expected the policy ARN {expected:?}, got {identifier:?}",
            case.name,
            case.path()
        );

        // The model reports it under the property the schema names, so the handle a caller
        // was handed is findable as a property of the resource it names.
        let stored = emulator.properties(MANAGED_POLICY, &identifier).await;
        assert_eq!(
            stored.get(POLICY_ARN),
            Some(&json!(expected)),
            "name {:?}: the model must carry the ARN under {POLICY_ARN}, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("ManagedPolicyName"),
            Some(&json!(case.name)),
            "name {:?}: the model must carry the name the create chose, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("Path"),
            Some(&json!(case.path())),
            "name {:?}: the model must carry the path the ARN is built from, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("DefaultVersionId"),
            Some(&json!("v1")),
            "name {:?}: a created policy is in force at its first version, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("PolicyDocument"),
            Some(&policy_document("s3:GetObject")),
            "name {:?}: the model must carry the document the create sent, got {stored:?}",
            case.name
        );

        // And the identifier is the handle the OWNING service resolves the policy by, which
        // is what makes it usable by a client that was given nothing else.
        let (status, answered) = emulator
            .iam("GetPolicy", &[("PolicyArn", &identifier)])
            .await;
        assert_eq!(
            status, 200,
            "name {:?}: IAM must resolve the reported identifier, got {status} with body \
             {answered:?}",
            case.name
        );
        assert_eq!(
            element(&answered, "PolicyName"),
            case.name,
            "name {:?}: IAM must report that policy, got {answered:?}",
            case.name
        );
        assert_eq!(
            element(&answered, "DefaultVersionId"),
            "v1",
            "name {:?}: IAM must report the first version in force, got {answered:?}",
            case.name
        );
    }

    // Every policy is listed under its ARN, and nothing else is.
    let mut listed = emulator.walk(MANAGED_POLICY, None).await;
    listed.sort();
    let mut expected: Vec<String> = rows
        .iter()
        .map(|case| expected_policy_arn(case.path(), &case.name))
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing must report every policy under its ARN, expected {expected:?}, got \
         {listed:?}"
    );

    // The documented flow is create, read the Identifier off the ProgressEvent, delete by
    // it — so each ARN is one the delete accepts, and the policy is gone from the owning
    // service too.
    for case in &rows {
        let arn = expected_policy_arn(case.path(), &case.name);
        let deleted = emulator.delete_and_settle(MANAGED_POLICY, &arn).await;
        assert_eq!(
            text(&deleted, "Identifier"),
            arn,
            "name {:?}: the delete must name the policy ARN, got {deleted:?}",
            case.name
        );
        let (status, body) = emulator.get(MANAGED_POLICY, &arn).await;
        assert_eq!(
            status, 404,
            "name {:?}: GetResource after the delete: expected HTTP 404, got {status} with \
             body {body:?}",
            case.name
        );
        let (status, answered) = emulator.iam("GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_ne!(
            status, 200,
            "name {:?}: IAM must hold no policy under that ARN, got {answered:?}",
            case.name
        );
        assert!(
            answered.contains("NoSuchEntity"),
            "name {:?}: IAM must report no such entity, got {answered:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_patched_policy_document_becomes_the_version_in_force_under_the_same_arn() {
    let emulator = Emulator::with_iam();
    let desired = json!({
        "ManagedPolicyName": "orders-writer",
        "PolicyDocument": policy_document("s3:GetObject"),
    });
    let identifier = emulator.provision(MANAGED_POLICY, &desired).await;
    let expected = expected_policy_arn("/", "orders-writer");
    assert_eq!(
        identifier, expected,
        "expected the policy ARN {expected:?}, got {identifier:?}"
    );

    // The identifier is stable across the update, and the patched document becomes the
    // version in force — which is how IAM changes a managed policy's content.
    let patched = emulator
        .update_and_settle(
            MANAGED_POLICY,
            &identifier,
            &json!([{
                "op": "replace",
                "path": "/PolicyDocument/Statement/0/Action",
                "value": "s3:PutObject",
            }]),
        )
        .await;
    assert_eq!(
        text(&patched, "Identifier"),
        identifier,
        "the update must report the same primary identifier, got {patched:?}"
    );

    let stored = emulator.properties(MANAGED_POLICY, &identifier).await;
    assert_eq!(
        stored.get("PolicyDocument"),
        Some(&policy_document("s3:PutObject")),
        "the patched document must be the one in force, got {stored:?}"
    );
    assert_eq!(
        stored.get("DefaultVersionId"),
        Some(&json!("v2")),
        "the patched document must be a second version in force, got {stored:?}"
    );
    let (status, answered) = emulator
        .iam("GetPolicy", &[("PolicyArn", &identifier)])
        .await;
    assert_eq!(
        status, 200,
        "IAM must still resolve the identifier, got {status} with body {answered:?}"
    );
    assert_eq!(
        element(&answered, "DefaultVersionId"),
        "v2",
        "IAM must report the new version in force, got {answered:?}"
    );
}

#[tokio::test]
async fn a_create_only_property_may_be_stated_back_but_never_changed_by_an_update() {
    let emulator = Emulator::with_iam();
    // `Description` is create-only in the type's schema and IAM publishes no call that
    // changes it, so the rows span what an update may say about such a property: state back
    // the value the policy was created with (accepted), name a different one, and name one
    // for a policy that carries none — each of the last two asking for work no handler
    // performs, which is refused rather than accepted and dropped.
    let described = emulator
        .provision(
            MANAGED_POLICY,
            &json!({
                "ManagedPolicyName": "orders-described",
                "Description": "reads the orders bucket",
                "PolicyDocument": policy_document("s3:GetObject"),
            }),
        )
        .await;
    let bare = emulator
        .provision(
            MANAGED_POLICY,
            &json!({
                "ManagedPolicyName": "orders-bare",
                "PolicyDocument": policy_document("s3:GetObject"),
            }),
        )
        .await;

    // Stating the description back unchanged asks for nothing, so the update that also
    // patches the document succeeds.
    let settled = emulator
        .update_and_settle(
            MANAGED_POLICY,
            &described,
            &json!([{
                "op": "replace",
                "path": "/PolicyDocument/Statement/0/Action",
                "value": "s3:PutObject",
            }]),
        )
        .await;
    assert_eq!(
        text(&settled, "Identifier"),
        described,
        "the update must report the same primary identifier, got {settled:?}"
    );
    let stored = emulator.properties(MANAGED_POLICY, &described).await;
    assert_eq!(
        stored.get("Description"),
        Some(&json!("reads the orders bucket")),
        "the description the policy was created with must survive the update, got {stored:?}"
    );

    // Naming any other description — and naming one at all where the policy carries none —
    // is refused by the owning handler, so the request reaches a terminal FAILED and the
    // policy is left exactly as it was.
    let rows = [
        (
            "a different description",
            described.as_str(),
            json!({ "op": "replace", "path": "/Description", "value": "something else" }),
        ),
        (
            "a description on a policy that carries none",
            bare.as_str(),
            json!({ "op": "add", "path": "/Description", "value": "added later" }),
        ),
    ];
    for (case, identifier, operation) in rows {
        let (status, body) = emulator
            .update(MANAGED_POLICY, identifier, &json!([operation]), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update is accepted and then failed by the handler, got \
             {status} with body {body:?}"
        );
        let accepted = progress(&body);
        let settled = emulator.terminal(&text(&accepted, "RequestToken")).await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: expected the handler to refuse the update, got {settled:?}"
        );
        assert_eq!(
            text(&settled, "ErrorCode"),
            "InvalidRequest",
            "case {case:?}: expected the handler's invalid-request verdict, got {settled:?}"
        );
        assert!(
            text(&settled, "StatusMessage").contains("Description"),
            "case {case:?}: the refusal must name the property it is about, got {settled:?}"
        );
    }

    // Neither refusal changed the policy it was about.
    let stored = emulator.properties(MANAGED_POLICY, &described).await;
    assert_eq!(
        stored.get("Description"),
        Some(&json!("reads the orders bucket")),
        "the refused update must leave the description in force, got {stored:?}"
    );
    let stored = emulator.properties(MANAGED_POLICY, &bare).await;
    assert_eq!(
        stored.get("Description"),
        None,
        "the refused update must leave the policy carrying no description, got {stored:?}"
    );
}

/// One row of the state-machine table.
struct MachineCase {
    /// The name the create chooses, which the ARN is built from.
    name: String,
    /// Whether the definition travels as the schema's write-only `Definition` object rather
    /// than as `DefinitionString`.
    definition_as_object: bool,
    /// The workflow type the create names, or `None` where it names none and the owning
    /// service's own default applies.
    workflow_type: Option<&'static str>,
}

impl MachineCase {
    /// The Amazon States Language definition this row creates the state machine with.
    fn definition(&self) -> Value {
        json!({
            "Comment": self.name,
            "StartAt": "P",
            "States": { "P": { "Type": "Pass", "Result": { "version": 1 }, "End": true } },
        })
    }

    /// The workflow type the state machine ends up with: the one the model names, or
    /// `STANDARD` — "the default is STANDARD" — for a model naming none.
    fn workflow_type(&self) -> &str {
        self.workflow_type.unwrap_or("STANDARD")
    }

    /// The desired state this row creates the state machine with.
    fn desired(&self, role_arn: &str) -> Value {
        let mut desired = json!({
            "StateMachineName": self.name,
            "RoleArn": role_arn,
        });
        if self.definition_as_object {
            desired["Definition"] = self.definition();
        } else {
            desired["DefinitionString"] = json!(self.definition().to_string());
        }
        if let Some(workflow_type) = self.workflow_type {
            desired["StateMachineType"] = json!(workflow_type);
        }
        desired
    }
}

#[tokio::test]
async fn the_identifier_is_the_state_machine_arn_whatever_name_the_create_chose() {
    let emulator = Emulator::with_step_functions();
    let role_arn = engine::emulator_arn("iam", "", "role/states-execution");
    // The name is the only part of the ARN the caller chooses; the partition, the Region and
    // the account come from the call and this emulator's own configuration. The rows span a
    // plain name, one carrying the punctuation a state machine name admits, the published
    // maximum name length and the shortest name — and across them both forms the schema
    // carries the definition in, and both workflow types beside the default.
    let rows = [
        MachineCase {
            name: "orders".to_string(),
            definition_as_object: true,
            workflow_type: Some("STANDARD"),
        },
        MachineCase {
            name: "orders-2026_eu.west+1".to_string(),
            definition_as_object: false,
            workflow_type: Some("EXPRESS"),
        },
        MachineCase {
            name: "s".repeat(80),
            definition_as_object: true,
            workflow_type: None,
        },
        MachineCase {
            name: "x".to_string(),
            definition_as_object: false,
            workflow_type: None,
        },
    ];

    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for case in &rows {
        let event = emulator
            .create_ok(STATE_MACHINE, &case.desired(&role_arn), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }

    for (case, request_token) in rows.iter().zip(&accepted) {
        let expected = expected_state_machine_arn(SIGNING_REGION, &case.name);
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        assert_eq!(
            identifier, expected,
            "name {:?}: expected the state machine ARN {expected:?}, got {identifier:?}",
            case.name
        );

        let definition = case.definition().to_string();
        let stored = emulator.properties(STATE_MACHINE, &identifier).await;
        assert_eq!(
            stored.get(ARN),
            Some(&json!(expected)),
            "name {:?}: the model must carry the ARN under {ARN}, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("Name"),
            Some(&json!(case.name)),
            "name {:?}: the model must carry the name the ARN is built from, got {stored:?}",
            case.name
        );
        // `Definition` is write-only in the schema, so a read reports the definition as
        // `DefinitionString` whichever form the create sent it in.
        assert_eq!(
            stored.get("DefinitionString"),
            Some(&json!(definition)),
            "name {:?}: the model must report the definition as DefinitionString, got \
             {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("Definition"),
            None,
            "name {:?}: the write-only Definition must not be reported, got {stored:?}",
            case.name
        );
        assert_eq!(
            stored.get("StateMachineType"),
            Some(&json!(case.workflow_type())),
            "name {:?}: the model must carry the workflow type in force, got {stored:?}",
            case.name
        );

        // And the identifier is the handle the OWNING service resolves the state machine
        // by — the one `states:DescribeStateMachine` and `states:StartExecution` take.
        let described = emulator
            .sfn_ok(
                "DescribeStateMachine",
                json!({ "stateMachineArn": identifier }),
            )
            .await;
        assert_eq!(
            text(&described, "name"),
            case.name,
            "name {:?}: Step Functions must report that state machine, got {described:?}",
            case.name
        );
        assert_eq!(
            text(&described, "definition"),
            definition,
            "name {:?}: Step Functions must hold the definition the create sent, got \
             {described:?}",
            case.name
        );
        assert_eq!(
            text(&described, "roleArn"),
            role_arn,
            "name {:?}: Step Functions must hold the role the create named, got {described:?}",
            case.name
        );
        assert_eq!(
            text(&described, "type"),
            case.workflow_type(),
            "name {:?}: Step Functions must hold the workflow type in force, got \
             {described:?}",
            case.name
        );
    }

    // Every state machine is listed under its ARN, and nothing else is.
    let mut listed = emulator.walk(STATE_MACHINE, None).await;
    listed.sort();
    let mut expected: Vec<String> = rows
        .iter()
        .map(|case| expected_state_machine_arn(SIGNING_REGION, &case.name))
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing must report every state machine under its ARN, expected {expected:?}, \
         got {listed:?}"
    );

    // The documented flow is create, read the Identifier off the ProgressEvent, delete by
    // it — so each ARN is one the delete accepts, and the state machine is gone from the
    // owning service too.
    for case in &rows {
        let arn = expected_state_machine_arn(SIGNING_REGION, &case.name);
        let deleted = emulator.delete_and_settle(STATE_MACHINE, &arn).await;
        assert_eq!(
            text(&deleted, "Identifier"),
            arn,
            "name {:?}: the delete must name the state machine ARN, got {deleted:?}",
            case.name
        );
        let (status, body) = emulator.get(STATE_MACHINE, &arn).await;
        assert_eq!(
            status, 404,
            "name {:?}: GetResource after the delete: expected HTTP 404, got {status} with \
             body {body:?}",
            case.name
        );
        let (status, body) = emulator
            .sfn("DescribeStateMachine", json!({ "stateMachineArn": arn }))
            .await;
        assert_ne!(
            status, 200,
            "name {:?}: Step Functions must hold no state machine under that ARN, got \
             {body:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_patched_definition_is_what_the_owning_service_then_runs_under_the_same_arn() {
    let emulator = Emulator::with_step_functions();
    let role_arn = engine::emulator_arn("iam", "", "role/states-execution");
    let first = json!({
        "Comment": "v1",
        "StartAt": "P",
        "States": { "P": { "Type": "Pass", "Result": { "version": 1 }, "End": true } },
    });
    let identifier = emulator
        .provision(
            STATE_MACHINE,
            &json!({
                "StateMachineName": "orders-flow",
                "RoleArn": role_arn,
                "Definition": first,
                "StateMachineType": "STANDARD",
            }),
        )
        .await;
    let expected = expected_state_machine_arn(SIGNING_REGION, "orders-flow");
    assert_eq!(
        identifier, expected,
        "expected the state machine ARN {expected:?}, got {identifier:?}"
    );

    // The identifier is stable across the update, and the patched definition is the one the
    // owning service then holds — whichever of the two forms the schema carries a definition
    // in the patch writes. The rows differ along that form: the write-only `Definition`
    // added onto a model that reports `DefinitionString`, and `DefinitionString` itself.
    let rows = [
        ("v2", DEFINITION_AS_OBJECT),
        ("v3", DEFINITION_AS_TEXT),
    ];
    for (comment, as_object) in rows {
        let mut wanted = first.clone();
        wanted["Comment"] = json!(comment);
        let operation = if as_object {
            // `Definition` is write-only, so the model a patch is applied to reports none:
            // writing it onto that model is how a caller supplies a definition as the object
            // the schema declares.
            json!({ "op": "add", "path": "/Definition", "value": wanted })
        } else {
            json!({
                "op": "replace",
                "path": "/DefinitionString",
                "value": wanted.to_string(),
            })
        };
        let patched = emulator
            .update_and_settle(STATE_MACHINE, &identifier, &json!([operation]))
            .await;
        assert_eq!(
            text(&patched, "Identifier"),
            identifier,
            "case {comment:?}: the update must report the same primary identifier, got \
             {patched:?}"
        );

        // The model reports the definition back as `DefinitionString` and never as the
        // write-only `Definition`, whichever form the patch wrote.
        let stored = emulator.properties(STATE_MACHINE, &identifier).await;
        assert_eq!(
            stored.get("DefinitionString"),
            Some(&json!(wanted.to_string())),
            "case {comment:?}: the model must report the patched definition, got {stored:?}"
        );
        assert_eq!(
            stored.get("Definition"),
            None,
            "case {comment:?}: the write-only Definition must not be reported, got {stored:?}"
        );

        let described = emulator
            .sfn_ok(
                "DescribeStateMachine",
                json!({ "stateMachineArn": identifier }),
            )
            .await;
        assert_eq!(
            text(&described, "definition"),
            wanted.to_string(),
            "case {comment:?}: Step Functions must hold the patched definition, got \
             {described:?}"
        );
        assert!(
            !text(&described, "revisionId").is_empty(),
            "case {comment:?}: an updated state machine is at a revision the owning service \
             minted, got {described:?}"
        );
    }
}

/// A patch that writes the definition as the schema's write-only `Definition` object.
const DEFINITION_AS_OBJECT: bool = true;

/// A patch that writes it as `DefinitionString`, the form a read reports.
const DEFINITION_AS_TEXT: bool = false;
