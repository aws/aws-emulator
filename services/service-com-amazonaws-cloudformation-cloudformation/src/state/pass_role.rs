// SPDX-License-Identifier: CC0-1.0

//! Handing an IAM ROLE to the service that owns a resource: the `iam:PassRole` the calling
//! principal needs for it, and the refusal a resource whose role it may not pass fails with.
//!
//! ## The rule this module implements
//!
//! "To pass a role (and its permissions) to an AWS service, a user must have permissions to
//! pass the role to the service" (IAM, "Granting a user permissions to pass a role to an AWS
//! service"): the permission is `iam:PassRole` on the role being handed over, and the service
//! the role is passed to refuses the create otherwise. A template's resources are created by
//! CloudFormation on the caller's behalf — a stack with no `RoleARN` makes its calls with the
//! caller's own credentials ([`super::calling_principal`]) — so a declaration that names a
//! role the calling principal may not pass is a create that service refuses: the resource
//! reaches `CREATE_FAILED` carrying that refusal, and the stack rolls back like any other
//! failed resource.
//!
//! The permission is about the ROLE, not about the resource: it is decided on the role's own
//! ARN, which is what a `Resource` element of an `iam:PassRole` statement names, so a grant
//! scoped to one role covers the resources that pass THAT role and no other.
//!
//! ## Which property hands a role over
//!
//! A role is passed by a property that NAMES one — `Role`, `RoleArn`, `ServiceRole`,
//! `ExecutionRoleArn`, a target's `RoleArn` nested inside a list — and names it BY ARN, which
//! is how the owning service is given a role rather than merely told about one. Both halves
//! are required here ([`passed_roles`]): a role ARN appearing somewhere that is not such a
//! property is not a role being handed to anybody (the `Resource` of a policy statement the
//! template writes, a trust policy's `Principal`), and a property naming a role by NAME
//! addresses IAM's own API rather than passing the role to another service (an
//! `AWS::IAM::Policy`'s `Roles`, which attaches a document to a role).
//!
//! ## How a decision reaches a deployment
//!
//! The principal is resolved by the OPERATION that drives the deployment, from the request's
//! own credentials and with no store of this service locked, and put in scope for the whole of
//! that deployment ([`as_principal`]) — the same shape [`super::service_role`] puts a stack's
//! assumed role in scope with, and for the same reason: the walk that creates the resources is
//! several frames below the handler that knows who called. A deployment with no principal in
//! scope — the rollback walk a later read drives, a nested stack's own in-process create — is
//! decided by nothing here.
//!
//! ## Bounds
//!
//! One decision per role ARN a resource's own properties name, over a properties document that
//! is already bounded by the template reader that produced it, and the walk that finds them
//! stops at [`MAX_PROPERTY_DEPTH`] levels — the depth every walk over a caller-supplied
//! document in this crate is held to.

use std::sync::Arc;

use ::serde_json::Value;

use super::calling_principal::Caller;

/// The IAM action handing a role to a service asks for.
const PASS_ROLE: &str = "iam:PassRole";

/// How deep into a resource's properties a role-naming property is looked for: a target's
/// `RoleArn` sits inside a list of targets, and a document a request can nest arbitrarily
/// deep is one no native recursion may follow to its bottom.
const MAX_PROPERTY_DEPTH: usize = 8;

tokio::task_local! {
    /// The principal the deployment running in this task was requested by, set by
    /// [`as_principal`] around the whole of that deployment.
    static PRINCIPAL: Arc<Caller>;
}

/// The principal a deployment's calls are made as, which is whose `iam:PassRole` they are
/// decided against.
///
/// `role_arn` is the SERVICE ROLE in force on the stack. A stack that names one does not make
/// its calls with the caller's credentials at all — "CloudFormation uses the role's
/// credentials to make calls on your behalf" — so the caller's own permissions decide nothing
/// about them, and what that role may do is the gate [`super::service_role`] owns. A stack
/// that names none is carried out as the CALLER, whose permissions are read here.
pub fn principal_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: Option<&str>,
) -> Arc<Caller> {
    match role_arn {
        // No principal of the request to judge: a `Caller` that names nobody refuses nothing
        // ([`Caller::unauthorized_message`]).
        Some(_) => Arc::new(Caller::default()),
        None => Arc::new(Caller::of(engine, ctx)),
    }
}

/// Run `deployment` with `principal` as the caller whose `iam:PassRole` its creates are
/// decided against.
pub async fn as_principal<F: ::std::future::Future>(
    principal: Arc<Caller>,
    deployment: F,
) -> F::Output {
    PRINCIPAL.scope(principal, deployment).await
}

/// The principal the deployment running in this task is carried out AS, where one is in scope.
///
/// A stack that names no `RoleARN` makes its calls with the CALLER's own credentials, so a call
/// CloudFormation itself makes about a resource the template names — one whose action is the
/// documented action of the service being emulated and whose resource is the ARN the template
/// supplies — is decided against that principal exactly as the owning service would decide it
/// ([`Caller::unauthorized_message`]). That is the same position [`refusal`] puts `iam:PassRole`
/// in; this hands the principal itself to the arm that knows which other call its resource's
/// create depends on.
///
/// The `Caller` a deployment acting as a SERVICE ROLE carries names nobody and refuses nothing
/// ([`principal_of`]) — what that role may do is [`super::service_role`]'s gate — and a
/// deployment with no principal in scope answers `None`.
#[must_use]
pub fn deploying_principal() -> Option<Arc<Caller>> {
    PRINCIPAL.try_with(Arc::clone).ok()
}

/// The refusal the create of a resource carrying `properties` is answered with, or `None`
/// where nothing refuses it.
///
/// The refusal is the one the service the role would be passed to answers — the call was never
/// made, so what is reported is that service's own class of refusal and the sentence AWS words
/// it with ([`Caller::unauthorized_message`]). A deployment with no principal in scope, a
/// resource whose properties pass no role, and a role the principal may pass are all `None`.
pub fn refusal(properties: &Value) -> Option<super::provisioning::Refusal> {
    let principal = PRINCIPAL.try_with(Arc::clone).ok()?;
    passed_roles(properties)
        .into_iter()
        .find_map(|role| principal.unauthorized_message(PASS_ROLE, &role))
        .map(|message| super::provisioning::Refusal {
            // The class an unauthorized call is reported under, which is the one the stack's
            // own service role's refusals carry: the ONE spelling of it in this crate
            // ([`super::service_role::ACCESS_DENIED`]).
            code: super::service_role::ACCESS_DENIED.to_string(),
            message,
        })
}

/// The ARNs of the roles `properties` hands to the service that owns the resource, in the
/// order they are found and without repetition.
///
/// A role is named by a property whose name says it is a role ([`names_a_role`]) and whose
/// value IS a role ARN (`::service_common::arn::IAM_ROLE`, the workspace's one reading of
/// that form), so a property whose value is still an unresolved intrinsic — one reading a
/// resource this deployment has not created yet — names no role for this decision.
pub fn passed_roles(properties: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_roles(properties, 0, &mut found);
    found
}

/// [`passed_roles`]'s walk, which stops at [`MAX_PROPERTY_DEPTH`].
fn collect_roles(value: &Value, depth: usize, found: &mut Vec<String>) {
    if depth > MAX_PROPERTY_DEPTH {
        return;
    }
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                if let Some(role) = member.as_str() {
                    if names_a_role(name)
                        && ::service_common::arn::IAM_ROLE.admits(role)
                        && !found.iter().any(|held| held == role)
                    {
                        found.push(role.to_string());
                    }
                }
                collect_roles(member, depth + 1, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_roles(item, depth + 1, found);
            }
        }
        _ => {}
    }
}

/// Whether the property `name` is one that names the role a resource is created WITH: the
/// property is spelled for the role it carries (`Role`, `ServiceRole`), with or without the
/// `Arn` suffix a property naming it by ARN carries (`RoleArn`, `ExecutionRoleARN`).
///
/// A plural (`AWS::IAM::Policy`'s `Roles`) is NOT one of them: such a property names the roles
/// a document is attached to, which is a call to IAM about those roles rather than a role
/// handed to this resource's own service.
fn names_a_role(name: &str) -> bool {
    let stem = name
        .strip_suffix("Arn")
        .or_else(|| name.strip_suffix("ARN"))
        .unwrap_or(name);
    stem.ends_with("Role")
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::*;

    /// RULE: a role is handed over by a property that names a role AND names it by ARN, at any
    /// depth of the document; nothing else in a declaration passes a role.
    ///
    /// The rows span the property spellings AWS uses, the nesting a property can sit at, and
    /// the ways a role ARN appears without being passed to anybody.
    #[test]
    fn only_a_role_naming_property_holding_a_role_arn_passes_a_role() {
        let role = "arn:aws:iam::000000000000:role/deployed";
        let other = "arn:aws:iam::000000000000:role/other";
        // (case, properties, the roles passed)
        let rows: &[(&str, Value, Vec<&str>)] = &[
            (
                "a function's Role",
                json!({"Role": role, "Runtime": "python3.12"}),
                vec![role],
            ),
            (
                "a state machine's RoleArn",
                json!({"RoleArn": role}),
                vec![role],
            ),
            (
                "a task definition's two role properties",
                json!({"TaskRoleArn": role, "ExecutionRoleArn": other}),
                vec![role, other],
            ),
            (
                "a rule target's RoleArn, nested in a list",
                json!({"Targets": [{"Id": "one", "RoleArn": role}]}),
                vec![role],
            ),
            (
                "the same role named by two properties",
                json!({"Role": role, "ServiceRoleARN": role}),
                vec![role],
            ),
            (
                "a role named by NAME rather than by ARN",
                json!({"Roles": ["deployed"], "PolicyName": "granted"}),
                vec![],
            ),
            (
                "a role ARN a policy statement is granted over",
                json!({"PolicyDocument": {"Statement": [
                    {"Effect": "Allow", "Action": "iam:PassRole", "Resource": role},
                ]}}),
                vec![],
            ),
            (
                "a trust policy naming a role as its principal",
                json!({"AssumeRolePolicyDocument": {"Statement": [
                    {"Effect": "Allow", "Principal": {"AWS": role}},
                ]}}),
                vec![],
            ),
            (
                "a property whose value is no ARN at all",
                json!({"Role": "deployed"}),
                vec![],
            ),
            (
                "a property whose ARN is of another vendor",
                json!({"Role": "arn:aws:lambda:us-east-1:000000000000:function:f"}),
                vec![],
            ),
            (
                "an unresolved intrinsic standing in for a role",
                json!({"Role": {"Fn::GetAtt": ["ExecRole", "Arn"]}}),
                vec![],
            ),
            ("no properties at all", Value::Null, vec![]),
        ];
        for (case, properties, expected) in rows {
            let actual = passed_roles(properties);
            let expected: Vec<String> = expected.iter().map(|role| role.to_string()).collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the roles passed to be {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the walk stops at a bounded depth, so a document nested deeper than any reading of
    /// a template is held to cannot drive it off the stack.
    #[test]
    fn the_walk_over_a_nested_document_is_bounded() {
        let role = "arn:aws:iam::000000000000:role/deep";
        let mut document = json!({"Role": role});
        for _ in 0..(MAX_PROPERTY_DEPTH * 4) {
            document = json!({"Nested": document});
        }
        let actual = passed_roles(&document);
        assert!(
            actual.is_empty(),
            "case \"past the depth bound\": expected no role to be read, got {actual:?}"
        );
    }
}
