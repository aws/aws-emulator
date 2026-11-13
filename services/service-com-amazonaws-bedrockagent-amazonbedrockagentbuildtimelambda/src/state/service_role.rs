// SPDX-License-Identifier: CC0-1.0

//! The SERVICE ROLE a knowledge base acts as, resolved against the account's own IAM
//! records.
//!
//! `CreateKnowledgeBase$roleArn` is "the Amazon Resource Name (ARN) of the IAM role with
//! permissions to invoke API operations on the knowledge base"
//! (`smithy_query(query="doc",shape="CreateKnowledgeBaseRequest",member="roleArn")`), and
//! Amazon Bedrock acts as that role: it assumes it to read the data sources and to write
//! into the vector store. So whether a knowledge base can be created under the named role at
//! all is a question about a role in IAM — the store those roles live in ([`iam_state`]),
//! the principal the role has to trust ([`SERVICE_PRINCIPAL`]) and the reading of its trust
//! policy ([`unassumable_role_message`]) — and it is resolved BEFORE anything else the
//! request names, because a role the service cannot assume is a role nothing else can be
//! done with.
//!
//! The rule and its wording are not this service's invention: the trust document is
//! evaluated through the ONE reading of one every service in this tree shares
//! ([`::service_common::iam_trust_policy::admits_service_principal`]), and the refusal is
//! the sentence Amazon Bedrock answers for a role it could not assume.
//!
//! LOCK ORDER: the function below takes the IAM store's lock and holds no other while it
//! does, so it must be called with NO guard of this service's store held — the same
//! discipline [`crate::state::vector_store`] states.

/// The IAM store a knowledge base resolves its `roleArn` against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so this service reads the very records `iam:CreateRole` wrote
/// rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The service principal Amazon Bedrock assumes a knowledge base's role as.
///
/// A role a knowledge base is created under has to trust this principal for
/// `sts:AssumeRole`; the Amazon Bedrock User Guide's knowledge-base prerequisites ("Create a
/// service role for Amazon Bedrock Knowledge Bases") give exactly that trust relationship.
pub const SERVICE_PRINCIPAL: &str = "bedrock.amazonaws.com";

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied. `None` has the same
/// meaning as for [`crate::state::service_state`]: the engine carries no such store.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// `None` when this emulator has no reason to think Amazon Bedrock could not assume
/// `role_arn`; otherwise the `ValidationException` message that refuses the call.
///
/// The role is resolved against the account's IAM records by its ARN, and the call is
/// refused when that role's trust policy does not let [`SERVICE_PRINCIPAL`] assume it. An
/// ARN the IAM store holds no role for is NOT evidence of a missing trust — the role may
/// belong to another account, or to a client that never told this emulator about it — so the
/// operation goes ahead; only a role this emulator holds and whose trust policy withholds
/// the grant is refused. A document this emulator cannot read as a policy at all is no
/// evidence either way.
pub fn unassumable_role_message<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<String> {
    let iam = iam_state(engine)?;
    let store = iam.lock();
    let role = store.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    // The rule itself is IAM's, not this service's, so it is read through the ONE evaluator
    // every service that resolves a caller-supplied service role shares: the statement must
    // name the principal and `sts:AssumeRole`, and an explicit `Deny` wins over any `Allow`.
    match ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL)
    {
        None | Some(true) => None,
        Some(false) => Some(UNASSUMABLE_ROLE.to_string()),
    }
}

/// The `ValidationException` message Amazon Bedrock answers for a knowledge-base role it
/// could not assume.
///
/// Fixed text, with no part taken from the request: the recorded refusal names neither the
/// role nor the knowledge base, and tells the caller to fix the role's permissions and retry.
pub const UNASSUMABLE_ROLE: &str = "Bedrock Knowledge Base was unable to assume the given role. \
                                    Provide the proper permissions and retry the request.";
