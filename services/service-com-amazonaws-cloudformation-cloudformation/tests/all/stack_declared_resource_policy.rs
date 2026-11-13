// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a stack declares for a secret, end to end over the wire.
//!
//! RULE: **a stack that declares `AWS::SecretsManager::ResourcePolicy` really attaches that
//! document to the secret its `SecretId` names**, so once the stack reports `CREATE_COMPLETE`
//! AWS Secrets Manager answers `GetResourcePolicy` on that secret with the declared document
//! and AUTHORIZES every later call against it — a `Deny` in it refuses exactly the action it
//! names and leaves the others alone — exactly as the same document attached with
//! `PutResourcePolicy` does. Removing the resource from the template takes the document off
//! again, which is what `DeleteResourcePolicy` does, so the calls the policy refused are
//! served once more.
//!
//! A stack that reported the resource created while nothing attached the document would leave
//! the secret with no policy at all: `GetResourcePolicy` would report none and every call the
//! policy was written to refuse would be served, so an infrastructure test could not tell a
//! deployed guardrail from one that did nothing.
//!
//! The rows differ along what the rule generalises over: the form the declaration states the
//! document in (the JSON object it is, and the JSON string), the form it addresses the secret
//! by (the ARN a `Ref` to the secret resolves to, and the secret's own name), whether the
//! declaration asks for public policies to be blocked, the effect the document states and the
//! action it scopes that effect to.

use serde_json::{json, Value};

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, output_value, router, xml_text,
};
use crate::stack_owned_secret_teardown::{error_code, poll_to_terminal, secretsmanager};

/// The logical ids the template declares the two resources under.
const SECRET_LOGICAL_ID: &str = "Credential";
const POLICY_LOGICAL_ID: &str = "CredentialPolicy";

/// The output keys the stack publishes the two resources' `Ref` values under.
const SECRET_OUTPUT: &str = "SecretArn";
const POLICY_OUTPUT: &str = "PolicyId";

/// The value every row's secret holds, so a SERVED read is asserted on the material Secrets
/// Manager hands out rather than on its status alone.
const SECRET_VALUE: &str = "a-stack-declared-credential";

/// The error code an authorization refusal carries, as the AWS "Common Errors" table names
/// it, and the status this awsJson1_1 service answers it with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// How a row's declaration addresses the secret the policy is attached to.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// `{"Ref": "<the secret>"}`, which resolves to the ARN Secrets Manager minted — the
    /// form a template written beside the secret uses.
    RefToTheSecret,
    /// The secret's own `Name`, which is the other form a `SecretId` takes.
    ByName,
}

/// One declared policy: how it is written, and which call it must refuse.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The name the row's secret is declared under, which also names its stack.
    secret_name: &'static str,
    addressed: Addressed,
    /// Whether the document is declared as a JSON string rather than as the object it is.
    declared_as_text: bool,
    /// Whether the declaration asks for policies granting public access to be blocked.
    blocks_public_policies: bool,
    /// The action the document's `Deny` names, or `""` for a document that denies nothing.
    denied_action: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: the document as an object, the secret by `Ref`, and a deny of
        // the value read.
        case: "a deny of the value read, declared as an object",
        secret_name: "stack-policy-denied-read",
        addressed: Addressed::RefToTheSecret,
        declared_as_text: false,
        blocks_public_policies: false,
        denied_action: "secretsmanager:GetSecretValue",
    },
    Row {
        // The document as a STRING, the secret by NAME, and a deny of the METADATA read:
        // the policy scopes only the action it names.
        case: "a deny of the metadata read, declared as a string and addressed by name",
        secret_name: "stack-policy-denied-describe",
        addressed: Addressed::ByName,
        declared_as_text: true,
        blocks_public_policies: false,
        denied_action: "secretsmanager:DescribeSecret",
    },
    Row {
        // A document that denies NOTHING, attached under the flag that blocks public
        // policies: the attach is accepted and no call is refused.
        case: "a policy that denies nothing, attached with public policies blocked",
        secret_name: "stack-policy-allow-only",
        addressed: Addressed::RefToTheSecret,
        declared_as_text: false,
        blocks_public_policies: true,
        denied_action: "",
    },
];

/// The document a row declares, as the JSON it is.
fn document(row: &Row) -> Value {
    let account = crate::stack_lifecycle::ACCOUNT_ID;
    if row.denied_action.is_empty() {
        return json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "ReadableByThisAccount",
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "secretsmanager:GetSecretValue",
                "Resource": "*"
            }]
        });
    }
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "DeniedToEveryPrincipal",
            "Effect": "Deny",
            "Principal": "*",
            "Action": row.denied_action,
            "Resource": "*"
        }]
    })
}

/// The template a row deploys: the secret, the policy attached to it, and both resources'
/// `Ref` values as outputs.
fn template(row: &Row) -> String {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "SecretId".to_string(),
        match row.addressed {
            Addressed::RefToTheSecret => json!({"Ref": SECRET_LOGICAL_ID}),
            Addressed::ByName => Value::String(row.secret_name.to_string()),
        },
    );
    let declared = document(row);
    properties.insert(
        "ResourcePolicy".to_string(),
        if row.declared_as_text {
            Value::String(declared.to_string())
        } else {
            declared
        },
    );
    if row.blocks_public_policies {
        properties.insert("BlockPublicPolicy".to_string(), Value::Bool(true));
    }
    json!({
        "Resources": {
            SECRET_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": row.secret_name,
                    "SecretString": SECRET_VALUE,
                }
            },
            POLICY_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::ResourcePolicy",
                "Properties": Value::Object(properties),
            }
        },
        "Outputs": {
            SECRET_OUTPUT: {"Value": {"Ref": SECRET_LOGICAL_ID}},
            POLICY_OUTPUT: {"Value": {"Ref": POLICY_LOGICAL_ID}},
        }
    })
    .to_string()
}

/// The same template with the POLICY resource removed, which is what asks for the document
/// to be taken off the secret again.
fn template_without_the_policy(row: &Row) -> String {
    json!({
        "Resources": {
            SECRET_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": row.secret_name,
                    "SecretString": SECRET_VALUE,
                }
            }
        },
        "Outputs": {
            SECRET_OUTPUT: {"Value": {"Ref": SECRET_LOGICAL_ID}},
        }
    })
    .to_string()
}

/// The document `GetResourcePolicy` reports for `secret`, parsed — or `None` where the secret
/// carries no policy at all.
async fn attached_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    secret: &str,
) -> Option<Value> {
    let (status, answered) =
        secretsmanager(router, "GetResourcePolicy", &json!({"SecretId": secret})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetResourcePolicy on the stack's secret to answer 200, got \
         {status} {answered}"
    );
    let reported = answered.get("ResourcePolicy")?.as_str()?;
    Some(
        serde_json::from_str(reported).unwrap_or_else(|error| {
            panic!("case {case:?}: the reported policy is not JSON: {reported:?} ({error})")
        }),
    )
}

/// Whether `operation` on `secret` is SERVED, asserting the refusal is the authorization one
/// when it is not.
async fn is_served(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    secret: &str,
) -> bool {
    let (status, answered) =
        secretsmanager(router, operation, &json!({"SecretId": secret})).await;
    if status == 200 {
        if operation == "GetSecretValue" {
            assert_eq!(
                answered.get("SecretString").and_then(Value::as_str),
                Some(SECRET_VALUE),
                "case {case:?}: expected the value the stack declared, got {answered}"
            );
        }
        return true;
    }
    assert_eq!(
        (status, error_code(case, &answered).as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?}: expected {operation} to be refused with {ACCESS_DENIED}, got {status} \
         {answered}"
    );
    false
}

#[tokio::test]
async fn a_stacks_resource_policy_is_attached_to_the_secret_and_enforced() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(&router, row.secret_name, &template(row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must answer 200, got {status} {body}"
        );
        let described = poll_to_terminal(&router, case, row.secret_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {described}"
        );
        let arn = output_value(&described, SECRET_OUTPUT).unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no secret ARN in {described}")
        });
        // "Ref returns the ARN of the secret" for the policy too: the resource IS the policy
        // of that secret, so the handle the stack publishes for it is the secret's ARN.
        assert_eq!(
            output_value(&described, POLICY_OUTPUT).as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the policy resource to publish the secret's ARN, got \
             {described}"
        );

        // The document really is attached, and reads back as the document declared.
        let attached = attached_policy(&router, case, &arn).await;
        assert_eq!(
            attached.as_ref(),
            Some(&document(row)),
            "case {case:?}: expected the declared document attached to the stack's secret, got \
             {attached:?}"
        );

        // ...and it is ENFORCED: the action the document denies is refused, every other call
        // on the secret is served.
        for operation in ["GetSecretValue", "DescribeSecret"] {
            let denied = row.denied_action == format!("secretsmanager:{operation}");
            let served = is_served(&router, case, operation, &arn).await;
            assert_eq!(
                served, !denied,
                "case {case:?}: expected {operation} served={}, got {served}",
                !denied
            );
        }

        // Removing the resource from the template takes the document off the secret again, so
        // the call the policy refused is served once more.
        let (status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.secret_name),
                member("TemplateBody", &template_without_the_policy(row)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStack must answer 200, got {status} {body}"
        );
        let (status, described) = describe_stack(&router, row.secret_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks must answer 200 while the update runs, got {status} \
             {described}"
        );
        let described = poll_to_terminal(&router, case, row.secret_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to complete, got {described}"
        );
        let left = attached_policy(&router, case, &arn).await;
        assert_eq!(
            left, None,
            "case {case:?}: expected the secret to carry no policy once the resource is gone, \
             got {left:?}"
        );
        for operation in ["GetSecretValue", "DescribeSecret"] {
            assert!(
                is_served(&router, case, operation, &arn).await,
                "case {case:?}: expected {operation} to be served once the policy is gone"
            );
        }
    }
}
