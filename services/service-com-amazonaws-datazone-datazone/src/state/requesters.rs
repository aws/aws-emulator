// SPDX-License-Identifier: CC0-1.0

//! WHO is asking: the DataZone user a request speaks for, and which projects that user
//! OWNS.
//!
//! Most of what this service reports about a caller is the single identity it authenticates
//! every call as ([`super::caller_user_id`]). One question needs more than that: whether the
//! principal presenting the credential already OWNS a project, because what a caller owns is
//! what it needs no permission — and no review — to reach.
//!
//! So the principal is resolved the way every other service in this emulator resolves one:
//! from the access key id the request presented, through the token service's own resolution
//! of it in the SAME shared engine. A credential that registry minted speaks for the session
//! it was minted for; a long-term access key speaks for the IAM user the account holds it
//! for; and a key no store holds a record of — the position the credential this emulator
//! publishes at start is in — speaks for the account's own principal. A caller reading its
//! own identity through `sts:GetCallerIdentity` and this service deciding what that caller
//! owns therefore see ONE principal.
//!
//! That principal is then matched against the DataZone users the domain holds, because
//! ownership is recorded about a DataZone user: every domain holds a profile for the
//! account's own principal from the instant it exists ([`super::caller_user_profile`]), and
//! `CreateUserProfile` adds one for any other identity. A principal the domain holds no
//! profile for is no DataZone user of that domain, so it owns nothing in it.
//!
//! Nothing here opens a socket: both stores live in the one engine this service was
//! constructed from.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use super::{Domain, MemberKind, Project, DESIGNATION_PROJECT_OWNER};

/// The identity a request speaks for, in the spellings a stored record names a user by.
///
/// Resolved ONCE per request ([`requester`]) and passed to whatever asks what the caller
/// owns, so two rules about the same request cannot disagree about who made it.
#[derive(Debug, Clone)]
pub struct Requester {
    /// The ARN of the IAM principal the request's credential belongs to
    /// ([`caller_principal_arn`]).
    pub principal_arn: String,
    /// The `UserProfileId` of the DataZone user the domain holds for that principal, or
    /// `None` when the domain holds no profile for it — in which case the principal is no
    /// DataZone user of that domain.
    pub user_profile_id: Option<String>,
}

/// The ARN of the IAM principal the request's credential belongs to.
///
/// One resolution of that key id — the one `sts:GetCallerIdentity` answers the same request
/// with — so the principal this service decides ownership for is the principal the caller
/// reads as itself. A session whose expiry the clock has passed resolves to the long-term
/// identity instead: the same call materialises that expiry, which is a write the clock
/// already owed.
///
/// An engine carrying no session registry at all (a router that mounts this service without
/// the token service) can hold no session, so the only identity a request can present there
/// is the one its long-term key resolves to. Call it with NO store guard of this service
/// held: it takes the locks of the stores that own the answer.
pub fn caller_principal_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
    let long_term = {
        let iam = sts_state::iam_state(engine);
        let guard = iam.as_ref().map(|iam| iam.lock());
        sts_state::long_term_identity(guard.as_deref(), &access_key_id)
    };
    let Some(registry) = sts_state::service_state(engine) else {
        return long_term.arn;
    };
    let resolved = sts_state::resolve_request_caller(
        &mut registry.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term.clone(),
    )
    .map(|caller| caller.identity);
    resolved.unwrap_or(long_term).arn
}

/// The identity `principal_arn` speaks for IN `domain`: the principal itself, plus the
/// DataZone user the domain holds a profile for it under, when it holds one.
///
/// The profile is looked up through the domain's own accessor
/// ([`Domain::user_profile`]), so the user this resolves to is the very user
/// `GetUserProfile` answers for that identity.
#[must_use]
pub fn requester(domain: &Domain, principal_arn: &str) -> Requester {
    Requester {
        principal_arn: principal_arn.to_string(),
        user_profile_id: domain
            .user_profile(principal_arn, None, None)
            .map(|profile| profile.id.clone()),
    }
}

impl Project {
    /// Whether `requester` OWNS this project.
    ///
    /// Two ways a DataZone user owns one, which is why this is asked in one place: it
    /// CREATED the project — a project records "the Amazon DataZone user who created the
    /// project" (smithy_query operation="CreateProject"), and that user is the project's
    /// owner — or it holds the owner designation among the project's members
    /// ([`DESIGNATION_PROJECT_OWNER`]), the designation `CreateProjectMembership` grants
    /// ownership with.
    ///
    /// A membership names its user by whatever `UserIdentifier` the request carried — a
    /// `UserProfileId` or the IAM principal's own ARN (smithy_query shape="UserIdentifier")
    /// — so both spellings of the requester are compared against it, and one member is
    /// recognised however it was added.
    #[must_use]
    pub fn is_owned_by(&self, requester: &Requester) -> bool {
        if requester
            .user_profile_id
            .as_ref()
            .is_some_and(|id| *id == self.created_by)
        {
            return true;
        }
        self.memberships.values().any(|membership| {
            membership.kind == MemberKind::User
                && membership.designation == DESIGNATION_PROJECT_OWNER
                && (membership.identifier == requester.principal_arn
                    || requester
                        .user_profile_id
                        .as_ref()
                        .is_some_and(|id| *id == membership.identifier))
        })
    }
}
