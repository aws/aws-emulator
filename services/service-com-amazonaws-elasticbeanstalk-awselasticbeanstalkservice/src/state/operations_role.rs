// SPDX-License-Identifier: CC0-1.0

//! The OPERATIONS ROLE an environment runs its downstream calls under: which
//! values name one, and what a role Elastic Beanstalk cannot act under means for
//! the calls that follow.
//!
//! "The Amazon Resource Name (ARN) of an EXISTING IAM role to be used as the
//! environment's operations role"
//! (`AssociateEnvironmentOperationsRoleMessage$OperationsRole`), after which
//! "Elastic Beanstalk uses the associated operations role for permissions to
//! downstream services during subsequent calls acting on this environment". Two
//! separate rules follow from that one sentence, and they are answered at
//! different moments:
//!
//! 1. The ASSOCIATION requires an EXISTING role: a value IAM holds no role under
//!    names nothing that can be associated, so the request is refused and the
//!    environment keeps whatever role it carried.
//! 2. A SUBSEQUENT call acting on that environment is made WITH the role's
//!    permissions, which Elastic Beanstalk can only take on if the role's trust
//!    policy admits it — so a call acting on an environment whose operations role
//!    it cannot assume cannot be carried out at all, and is refused with the
//!    privileges error those operations model. Associating a role is therefore
//!    accepted on its existence alone: what a role nobody may assume costs is
//!    paid by the calls that follow, which is where it is reported.
//!
//! Both questions are the tree's shared reading of a caller-supplied execution
//! role ([`::service_common::iam_role`]): which role of this account the ARN
//! names, the in-process `GetRole`, and the trust-policy reading. So this module
//! states only WHICH principal has to be admitted and HOW this service words each
//! refusal, and a refusal is always grounded in what IAM ANSWERED — nothing is
//! refused for an answer this emulator could not read (an emulator with no IAM
//! mounted, a value naming another account's role).

/// The service principal an environment's operations role has to trust: Elastic
/// Beanstalk is what assumes it, in the global spelling a trust policy names a
/// service by (the principal IAM's own published service-linked-role catalog
/// carries for this service).
pub const OPERATIONS_ROLE_PRINCIPAL: &str = "elasticbeanstalk.amazonaws.com";

/// How a request naming a role this account does not hold is refused:
/// `Operations role ARN '<arn>' isn't valid. Specify an existing IAM role.`
///
/// The only variable part is the ARN the request supplied, echoed back so the
/// caller sees which value was rejected; the rest is the service's fixed
/// sentence, which names what to supply instead.
pub fn invalid_operations_role_message(operations_role: &str) -> String {
    format!("Operations role ARN '{operations_role}' isn't valid. Specify an existing IAM role.")
}

/// The refusal the operations role `operations_role` earns from rule 1, or `None`
/// when IAM holds a role under the name it carries (and for every value nothing
/// could be read about — see the module docs).
pub async fn operations_role_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operations_role: &str,
) -> Option<String> {
    ::service_common::iam_role::role_is_absent(ctx, region, operations_role)
        .await
        .then(|| invalid_operations_role_message(operations_role))
}

/// How a call acting on an environment whose operations role Elastic Beanstalk
/// cannot act under is refused (rule 2): the service states that it is unable to
/// assume the role, names it, and names the grant its trust policy is missing —
/// which is what the account has to put right before any call on that environment
/// can be carried out. The same two-sentence form the tree's other services word
/// an unassumable caller-supplied role with (`service-com-amazonaws-pipes-pipes`'s
/// `state::execution_role`,
/// `service-com-amazonaws-s3control-awss3controlservicev20180820`'s
/// `state::batch_job_role`).
pub fn unusable_operations_role_message(operations_role: &str) -> String {
    format!(
        "Elastic Beanstalk is unable to assume the operations role '{operations_role}' of this \
         environment. Its trust policy must allow {OPERATIONS_ROLE_PRINCIPAL} to perform \
         sts:AssumeRole."
    )
}

/// The refusal a call ACTING ON the environment `environment_id` earns from rule
/// 2, or `None` when the environment carries no operations role or carries one
/// this service can act under.
///
/// Takes and releases this service's store itself to read the role off the
/// environment, so a handler calls it BEFORE locking the store for its own work —
/// which it must anyway, since the question is put to IAM over the in-process hop.
pub async fn unusable_operations_role_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> Option<String> {
    let operations_role = super::service_state(engine).and_then(|state| {
        let store = state.lock();
        store
            .resolve_environment_id(environment_id, environment_name)
            .and_then(|id| store.environments.get(&id))
            .and_then(|environment| environment.operations_role.clone())
    })?;
    // Absent or untrusted alike: either way this service has no role to make the
    // environment's downstream calls under.
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        &operations_role,
        OPERATIONS_ROLE_PRINCIPAL,
    )
    .await
    .then(|| unusable_operations_role_message(&operations_role))
}
