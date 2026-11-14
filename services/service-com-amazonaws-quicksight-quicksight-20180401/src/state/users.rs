// SPDX-License-Identifier: CC0-1.0

//! The users QuickSight's identity path holds: what a registration persists, and what
//! every later user read answers from.
//!
//! A user belongs to one namespace of one account and its `UserName` identifies it
//! there, so the record is keyed by `(AwsAccountId, Namespace, UserName)` the way a
//! group's is — and, like a group, a user goes with the namespace it lives in when that
//! namespace is deleted.

use super::*;

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// The user `(aws_account_id, namespace)` holds under `user_name`, or `None` when no
    /// registration stored one — the record every user read answers from.
    pub fn user(
        &self,
        aws_account_id: &str,
        namespace: &str,
        user_name: &str,
    ) -> Option<&UserRecord> {
        self.users.get(&(
            aws_account_id.to_string(),
            namespace.to_string(),
            user_name.to_string(),
        ))
    }

    /// How many users `(aws_account_id, namespace)` holds — what
    /// [`MAX_USERS_PER_NAMESPACE`] is checked against.
    pub fn user_count(&self, aws_account_id: &str, namespace: &str) -> usize {
        self.users
            .keys()
            .filter(|(account, user_namespace, _)| {
                account == aws_account_id && user_namespace == namespace
            })
            .count()
    }

    /// Removes the users `(aws_account_id, namespace)` holds, answering how many went:
    /// deleting a namespace deletes the users and groups associated with it, so they go
    /// with the namespace rather than outliving it.
    pub fn remove_users_in(&mut self, aws_account_id: &str, namespace: &str) -> usize {
        let before = self.users.len();
        self.users.retain(|(account, user_namespace, _), _| {
            account != aws_account_id || user_namespace != namespace
        });
        before - self.users.len()
    }
}

/// A stored user: what `RegisterUser` persisted, which is what every later user read
/// reports.
///
/// The record carries no timestamps: the modelled `User` structure declares none, so
/// nothing is stored that no read can report.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub arn: String,
    /// The name that identifies the user in its namespace: the one the request named,
    /// or — for an identity type that federates through IAM — the one derived from the
    /// IAM principal the request named.
    pub user_name: String,
    pub email: String,
    /// The `UserRole` the registration assigned, a member of the model's `UserRole`
    /// enum.
    pub role: String,
    /// The `IdentityType` the registration used, a member of the model's `IdentityType`
    /// enum.
    pub identity_type: String,
    /// Whether the user is already active. A user QuickSight manages itself is inactive
    /// until it signs in and provides a password; one backed by an outside identity is
    /// active as soon as it is registered.
    pub active: bool,
    /// The service's own handle on this user, minted when it was registered and stable
    /// for as long as the user exists.
    pub principal_id: String,
    /// The custom permissions profile the registration assigned, if any.
    pub custom_permissions_name: Option<String>,
    /// The external login provider the user federates through, if any.
    pub external_login_federation_provider_type: Option<String>,
    /// That provider's URL: the one the provider type fixes, or the custom one the
    /// request carried.
    pub external_login_federation_provider_url: Option<String>,
    /// The user's identity id at that provider, if any.
    pub external_login_id: Option<String>,
    /// The tags the registration associated with the user.
    pub tags: Vec<Tag>,
}

impl UserRecord {
    /// This user as the modelled `User` structure every user read reports.
    pub fn to_user(&self) -> User {
        User {
            arn: Some(self.arn.clone()),
            user_name: Some(self.user_name.clone()),
            email: Some(self.email.clone()),
            role: Some(self.role.clone()),
            identity_type: Some(self.identity_type.clone()),
            active: self.active,
            principal_id: Some(self.principal_id.clone()),
            custom_permissions_name: self.custom_permissions_name.clone(),
            external_login_federation_provider_type: self
                .external_login_federation_provider_type
                .clone(),
            external_login_federation_provider_url: self
                .external_login_federation_provider_url
                .clone(),
            external_login_id: self.external_login_id.clone(),
        }
    }
}

/// The ARN of a user: `.../user/<Namespace>/<UserName>` — the resource QuickSight's
/// identity path reports for a registered user, scoped by the namespace the user lives
/// in the way a group's ARN is.
pub fn user_arn(region: &str, aws_account_id: &str, namespace: &str, user_name: &str) -> String {
    quicksight_arn(
        region,
        aws_account_id,
        &format!("user/{namespace}/{user_name}"),
    )
}

/// The `PrincipalId` of a newly registered user: the service's own handle on the user,
/// which the request does not name, so the registration mints it.
pub fn next_user_principal_id() -> String {
    uuid_shaped(next_minted_id())
}

/// The `IAM` member of the model's `IdentityType` enum.
/// Source: `smithy_query(query="shape", shape="IdentityType")`.
pub const IDENTITY_TYPE_IAM: &str = "IAM";

/// The `QUICKSIGHT` member of the model's `IdentityType` enum.
pub const IDENTITY_TYPE_QUICKSIGHT: &str = "QUICKSIGHT";

/// The `IAM_IDENTITY_CENTER` member of the model's `IdentityType` enum.
pub const IDENTITY_TYPE_IAM_IDENTITY_CENTER: &str = "IAM_IDENTITY_CENTER";

/// Amazon Cognito, one of the two external login providers `RegisterUser`'s own
/// documentation names for `ExternalLoginFederationProviderType` — a member the model
/// types as a plain string rather than as an enum.
pub const EXTERNAL_LOGIN_PROVIDER_COGNITO: &str = "COGNITO";

/// A custom OpenID Connect provider, the other provider type that documentation names.
pub const EXTERNAL_LOGIN_PROVIDER_CUSTOM_OIDC: &str = "CUSTOM_OIDC";

/// The provider URL Amazon Cognito federation uses, which the request does not carry:
/// `RegisterUser` documents it as the provider URL of the `COGNITO` type.
pub const COGNITO_FEDERATION_PROVIDER_URL: &str = "cognito-identity.amazonaws.com";

/// The `UserRole` members a registration cannot assign: the model documents each of
/// these two as "This role isn't currently available for use", so QuickSight registers
/// no user with one even though the enum declares it.
/// Source: `smithy_query(query="doc", shape="User", member="Role")`.
pub const UNAVAILABLE_USER_ROLES: &[&str] = &["RESTRICTED_READER", "RESTRICTED_AUTHOR"];

/// How many users one namespace may hold before `RegisterUser` refuses with the modelled
/// `LimitExceededException`: the ceiling QuickSight states for the users of a namespace,
/// the same allowance its groups have ([`MAX_GROUPS_PER_NAMESPACE`]).
pub const MAX_USERS_PER_NAMESPACE: usize = 10_000;

/// The TOTAL number of users this emulator holds, across every account and namespace.
///
/// This is the bound that holds for the process, for the same reason as [`MAX_GROUPS`]:
/// every part of the store key comes from the request — the account id and the namespace
/// are both `@httpLabel`s the caller chooses — so the per-namespace ceiling alone would
/// be reset by varying either.
pub const MAX_USERS: usize = 100_000;

/// The `ExceptionResourceType` a user's refusals report (model enum
/// `com.amazonaws.quicksight#ExceptionResourceType`).
pub const EXCEPTION_RESOURCE_TYPE_USER: &str = "USER";

/// The address a QuickSight-managed user visits to complete its registration and
/// provide a password — `RegisterUser`'s `UserInvitationUrl`.
///
/// The user FOLLOWS this address, so it has to name a host this emulator answers on: it
/// is reached on its own endpoint alone, and no production AWS hostname resolves to it.
/// So the URL is presigned against this emulator's own base URL, and the user it invites
/// travels in the path — the namespace it was registered in and its name, the two labels
/// that identify it.
pub fn user_invitation_url(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    namespace: &str,
    user_name: &str,
) -> String {
    let path = format!("/invitations/{namespace}/{user_name}");
    let request = ::smithy_server_runtime::SignedUrlRequest::new(
        ::smithy_server_runtime::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    );
    match ctx.url_signer().sign(request, USER_INVITATION_URL_EXPIRY) {
        Ok(signed) => signed.as_str().to_string(),
        // Nothing about a stored name or this emulator's own endpoint makes a request
        // unsignable, so this is unreachable in practice; the invitation still addresses
        // that user, so the address is what is reported rather than none at all. The
        // endpoint is `scheme://host[:port]` with no path of its own, and a trailing
        // separator is dropped so the invitation's path is the only one the URL carries.
        Err(_) => format!("{}{path}", ctx.endpoint().as_str().trim_end_matches('/')),
    }
}

/// How long an invitation this service hands out stays valid.
///
/// QuickSight documents no span for the registration URL `RegisterUser` generates, so
/// this is one the emulator chooses: the longest a URL signed against this emulator may
/// be valid for, which is a week.
pub const USER_INVITATION_URL_EXPIRY: Duration =
    ::smithy_server_runtime::MAX_SIGNED_URL_EXPIRY;

/// The name the user registered for an IAM principal is known by in QuickSight.
///
/// QuickSight names such a user after the IAM identity it federates through rather than
/// after a name the caller chose: the principal's own name, and — when the registration
/// assumes a role under a session name — that session's name under it, which is what
/// lets one role register several users.
pub fn iam_user_name(iam_arn: &str, session_name: Option<&str>) -> String {
    let principal = iam_principal_name(iam_arn);
    match session_name {
        Some(session) if !session.is_empty() => format!("{principal}/{session}"),
        _ => principal.to_string(),
    }
}

/// The principal an IAM ARN names: the last segment of its resource field, so a role or
/// user held under a path is named by its own name rather than by the path.
///
/// The ARN is read by the tree's shared reading of one
/// ([`::service_common::arn::resource`]), and a value that is no well-formed ARN is
/// named by itself — the name a user is known by is derived from the request either way,
/// and whether the principal exists is IAM's question, not this service's.
fn iam_principal_name(iam_arn: &str) -> &str {
    let resource = ::service_common::arn::resource(iam_arn).unwrap_or(iam_arn);
    resource
        .rsplit_once('/')
        .map_or(resource, |(_, name)| name)
}
