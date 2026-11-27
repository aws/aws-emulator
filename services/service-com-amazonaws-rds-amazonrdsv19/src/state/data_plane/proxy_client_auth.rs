// SPDX-License-Identifier: CC0-1.0

//! What a DB PROXY decides about the CLIENT's OWN credential, where its `Auth` says that
//! client authenticates with IAM.
//!
//! A proxy has two credential hops, and they are not the same credential: "the type of
//! authentication that the proxy uses for connections from the proxy to the underlying
//! database" (`UserAuthConfigInfo$AuthScheme`, the secret the proxy logs in with) and
//! "whether to require or disallow Amazon Web Services Identity and Access Management (IAM)
//! authentication for connections to the proxy" (`UserAuthConfigInfo$IAMAuth`) — the CLIENT's
//! hop, which is this module's subject. With `REQUIRED`, a client does not present the
//! database's password at all: it presents an authentication token for the proxy's own
//! endpoint ([`super::iam_auth_token`]), and whether that token admits anybody is an IAM
//! question about the principal that SIGNED it.
//!
//! That question is `rds-db:connect` on the DB USER the token names
//! ([`ProxyClientAuth::dbuser_arn`]): the resource an IAM policy grants database access over
//! is `arn:<partition>:rds-db:<region>:<account>:dbuser:<proxy id>/<database user>`, the proxy
//! being named by the id its own `DBProxyArn` carries — which is where a client derives the
//! resource it grants on from. So two principals connecting to one proxy as one database user
//! get different answers, which is the whole point of the setting.
//!
//! Three pieces, each held in exactly one place, as this crate's own control-plane gate
//! ([`crate::service::authorization`]) and its engine's S3 role reading
//! ([`super::engine_s3_role`]) hold them:
//!
//! 1. **who** — the identity the token's access key id speaks for: the session that key was
//!    minted for, or the IAM user that owns it ([`ConnectAuthorizer::identity_arn`]);
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    identity, read through IAM's OWN accessor, so a document `iam:PutRolePolicy` wrote is in
//!    force on the next connection;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's one evaluation of
//!    those documents against `(action, resource)`.
//!
//! Nothing is refused on evidence this emulator does not have, which is the same deliberate
//! non-refusal every other role reading in this crate takes: an emulator with no IAM or no
//! token service mounted beside this one, a credential neither store accounts for (the
//! account's own published credential), a principal the IAM store holds no policy record of,
//! and one carrying an attachment whose document this account does not hold are all
//! principals whose permissions cannot be READ, so the connection is admitted. A principal
//! whose readable documents simply do not allow the action is refused — that is IAM's default
//! deny, and it is what a missing grant looks like — and an explicit `Deny` in a document that
//! CAN be read refuses whatever else is beside it.
//!
//! The decision WRITES NOTHING: both stores are read, and a connection that is about to be
//! refused leaves nothing behind in either.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::EmbeddedEmulator;

use crate::ops::types::UserAuthConfigInfo;

/// The field a database credential secret holds the database user in.
///
/// A secret a proxy authenticates to its database with is the credential document Amazon RDS
/// documents for one — a `username` and a `password` — so the user an `Auth` entry naming none
/// is in force for is this field of the secret that entry points at.
const SECRET_USERNAME_FIELD: &str = "username";

/// `IAMAuthMode` — "REQUIRED", the mode under which a client of the proxy authenticates with
/// IAM and with nothing else, so the credential it presents in place of a password is an
/// authentication token.
pub const IAM_AUTH_REQUIRED: &str = "REQUIRED";

/// `IAMAuthMode` — "ENABLED", under which IAM authentication is allowed rather than demanded:
/// a client may present a token, and one that presents the database's own password is
/// admitted on that instead.
pub const IAM_AUTH_ENABLED: &str = "ENABLED";

/// The IAM action a client performs by connecting to a database — or to a proxy in front of
/// one — with IAM authentication: `rds-db:connect`, the action an IAM policy for IAM database
/// authentication grants on a `dbuser` resource (Amazon RDS User Guide, "Creating and using an
/// IAM policy for IAM database access").
pub const CONNECT_ACTION: &str = "rds-db:connect";

/// The ARN namespace that `dbuser` resource lives under, which is a namespace of its own and
/// not this service's `rds` (same source).
const DBUSER_ARN_NAMESPACE: &str = "rds-db";

/// The ARN resource type of one database user of one proxy (same source).
const DBUSER_RESOURCE_TYPE: &str = "dbuser";

/// What the PROXY's own record says about the client hop of a session on its endpoint: which
/// database users authenticate with IAM, and the resource a grant for each of them is written
/// over.
///
/// Pure data read off the proxy record under the state lock, so the facts a session is
/// admitted against carry it ([`super::wire_session::InstanceFacts::proxy_client_auth`]) and
/// no second read of the store can disagree with them. WHO a token speaks for is the other
/// half, and lives in [`ConnectAuthorizer`], because that answer comes out of two other
/// services' stores.
#[derive(Debug, Clone)]
pub struct ProxyClientAuth {
    /// The Region the proxy was created in: the Region a token for its endpoint has to be
    /// signed in, and the Region the `dbuser` resource is named in.
    region: String,
    /// The id the proxy's own `DBProxyArn` names it by, which is the id the `dbuser` resource
    /// carries.
    proxy_resource_id: String,
    /// The proxy's `Auth` list, as it reports it: one entry per database user the proxy logs
    /// in as, each stating that user's own `IAMAuth` mode.
    auth: Vec<UserAuthConfigInfo>,
}

impl ProxyClientAuth {
    /// What the proxy `proxy_resource_id` in `region` says about its clients' credentials.
    #[must_use]
    pub fn new(region: String, proxy_resource_id: String, auth: Vec<UserAuthConfigInfo>) -> Self {
        Self {
            region,
            proxy_resource_id,
            auth,
        }
    }

    /// The Region a token presented to this proxy has to be signed in.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The `IAMAuth` mode in force for the database user `user`: the mode of the first `Auth`
    /// entry naming that user, read however the request spelled the mode.
    ///
    /// `None` for a user the list names no entry for, which is a user the proxy was not
    /// configured to log in as at all, and for an entry stating no mode — the member is
    /// optional, and `DISABLED` is its own value.
    fn mode(&self, user: &str) -> Option<String> {
        self.auth
            .iter()
            .find(|entry| entry.user_name.as_deref() == Some(user))
            .and_then(|entry| entry.iam_auth.as_deref())
            .map(str::to_ascii_uppercase)
    }

    /// Whether a client logging in as `user` authenticates with IAM and with nothing else, so
    /// the credential this proxy asks it for is an authentication token.
    #[must_use]
    pub fn requires_a_token(&self, user: &str) -> bool {
        self.mode(user).as_deref() == Some(IAM_AUTH_REQUIRED)
    }

    /// Whether a token is a credential this proxy admits a login as `user` on at all — the
    /// mode that demands one and the mode that allows one.
    #[must_use]
    pub fn admits_a_token(&self, user: &str) -> bool {
        matches!(
            self.mode(user).as_deref(),
            Some(IAM_AUTH_REQUIRED | IAM_AUTH_ENABLED)
        )
    }

    /// The resource an IAM policy grants [`CONNECT_ACTION`] over for a login as `user`
    /// through this proxy: `dbuser:<proxy id>/<database user>` under the `rds-db` namespace,
    /// in the proxy's own Region and this account.
    #[must_use]
    pub fn dbuser_arn(&self, user: &str) -> String {
        ::emulator_server::emulator_arn(
            DBUSER_ARN_NAMESPACE,
            &self.region,
            &format!(
                "{DBUSER_RESOURCE_TYPE}:{}/{user}",
                self.proxy_resource_id
            ),
        )
    }

    /// Fill in the DATABASE USER of every `Auth` entry that names none, out of the secret that
    /// entry points at.
    ///
    /// `UserName` — "the name of the database user to which the proxy connects" — is an
    /// OPTIONAL member, and the credential the proxy logs in as that user with is the one in the
    /// secret the same entry names (`SecretArn`, "the secret that the proxy uses to authenticate
    /// to the RDS DB instance or Aurora DB cluster"). So an entry that spells no user out is in
    /// force for the user THAT SECRET holds, read as the field a database credential secret
    /// carries it in ([`SECRET_USERNAME_FIELD`]): a proxy created with an `Auth` entry stating
    /// the scheme, the secret and the IAM mode and no user authenticates the client logging in
    /// as the secret's own user.
    ///
    /// The secret is read through Secrets Manager's OWN operation over the in-process callback —
    /// with this crate's one in-process client for it
    /// ([`crate::state::managed_master_secret::secrets_manager_client`]) — because what a secret
    /// holds is that service's to answer rather than another store's to read: only the user is
    /// kept, and the credential beside it is not looked at. An entry whose secret cannot be read,
    /// or whose secret holds no such field, names no user still and decides nothing about any
    /// client; an entry naming one is left exactly as the proxy reports it.
    pub async fn resolve_users_from_secrets(&mut self, callback: &EmbeddedEmulator) {
        if self.auth.iter().all(|entry| entry.user_name.is_some()) {
            return;
        }
        let Some(client) =
            crate::state::managed_master_secret::secrets_manager_client(callback, &self.region)
        else {
            return;
        };
        for entry in &mut self.auth {
            if entry.user_name.is_some() {
                continue;
            }
            let Some(secret_arn) = entry
                .secret_arn
                .as_deref()
                .filter(|arn| !arn.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            entry.user_name = user_in_secret(&client, &secret_arn).await;
        }
    }
}

/// The database user the secret `secret_arn` holds, or `None` when Secrets Manager answered no
/// value, the value is not a document stating fields at all, or it states no user.
///
/// The document is read for its `username` field and nothing else: every other field a
/// credential secret carries — the engine, the host, the port, the database name — is left
/// alone, whatever type it holds, and the credential beside the user is never touched, logged or
/// carried out of this function.
async fn user_in_secret(
    client: &::aws_sdk_secretsmanager::Client,
    secret_arn: &str,
) -> Option<String> {
    let answered = client
        .get_secret_value()
        .secret_id(secret_arn)
        .send()
        .await
        .ok()?;
    let document = answered.secret_string()?;
    let fields: ::serde_json::Value = ::serde_json::from_str(document).ok()?;
    fields
        .get(SECRET_USERNAME_FIELD)
        .and_then(::serde_json::Value::as_str)
        .filter(|user| !user.is_empty())
        .map(str::to_string)
}

/// WHO an authentication token speaks for, and what that principal may do: the two stores the
/// decision is read out of — IAM's, which holds the documents, and the token service's, which
/// says which identity a session credential speaks for.
///
/// Held as the state handles themselves, so a policy written after the endpoint was opened is
/// in force on the next connection: the endpoint outlives every request that changes one.
#[derive(Clone, Default)]
pub struct ConnectAuthorizer {
    /// IAM's store, or `None` on an engine carrying none.
    iam: Option<::engine::ServiceState<iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State>>,
    /// The token service's store, or `None` on an engine carrying none.
    sts: Option<
        ::engine::ServiceState<sts_store::ComAmazonawsStsAwssecuritytokenservicev20110615State>,
    >,
}

/// Written by hand rather than derived: the state handles carry no `Debug` of their own, and
/// what is diagnosable about the authorizer is whether each store is there to ask.
impl std::fmt::Debug for ConnectAuthorizer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectAuthorizer")
            .field("iam", &self.iam.is_some())
            .field("sts", &self.sts.is_some())
            .finish()
    }
}

impl ConnectAuthorizer {
    /// The two stores as the engine this service is mounted on holds them.
    #[must_use]
    pub fn of_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self {
            iam: super::engine_s3_role::iam_store(engine),
            sts: sts_store::service_state(engine),
        }
    }

    /// Whether the principal behind `access_key_id` may perform `action` on `resource`, as of
    /// `now`.
    ///
    /// `true` for every principal whose permissions cannot be read — the module docs list them
    /// — because a connection is refused only on what IAM SAID about the principal.
    #[must_use]
    pub fn allows(
        &self,
        access_key_id: &str,
        action: &str,
        resource: &str,
        now: SystemTime,
    ) -> bool {
        let Some(iam_state) = self.iam.as_ref() else {
            return true;
        };
        let Some(identity_arn) = self.identity_arn(access_key_id, now) else {
            return true;
        };
        let store = iam_state.lock();
        // A principal the IAM store holds no record of, and one carrying an attachment it
        // resolves no document for (an entry of AWS's own managed-policy catalog), are
        // principals whose permissions cannot be read in full.
        let Some(policies) = store.principal_policies(&identity_arn) else {
            return true;
        };
        if !policies.uncarried.is_empty() {
            return true;
        }
        let request = iam::Request::new(action, resource, &[]);
        // An identity-based document states no `Principal` element, so the decision rests on
        // the action and the resource; a readable permission set that states nothing about
        // them is IAM's default deny.
        if iam::identity_decision(&policies.documents, &request) != Decision::Allowed {
            return false;
        }
        // "A permissions boundary sets the maximum permissions that an identity-based policy
        // can grant": a grant the boundary does not also allow is not a permission this
        // principal has.
        store
            .permissions_boundary_document(&identity_arn)
            .is_none_or(|boundary| {
                iam::identity_decision(&[boundary], &request) == Decision::Allowed
            })
    }

    /// The ARN of the identity whose identity-based policies are in force on a credential: for
    /// a session, the role it was minted for (or the user behind it); for a long-term key, the
    /// IAM user that owns it.
    ///
    /// `None` for a credential neither store accounts for — the account's own published
    /// credential — and for a session the clock has already expired, which speaks for nobody.
    fn identity_arn(&self, access_key_id: &str, now: SystemTime) -> Option<String> {
        if let Some(sts) = self.sts.as_ref() {
            let store = sts.lock();
            if let Some(session) = store.session(access_key_id) {
                if session.expires_at <= now {
                    return None;
                }
                return session
                    .role_arn
                    .clone()
                    .or_else(|| session.federating_user_arn.clone())
                    .or_else(|| Some(session.identity.arn.clone()));
            }
        }
        let iam_state = self.iam.as_ref()?;
        let store = iam_state.lock();
        // IAM's store maps an access key id to the USER that owns it, and a token signed with
        // one speaks for that user and is judged on the user's policies.
        store
            .access_keys
            .get(access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
            .map(|user| user.arn.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `Auth` entry naming `user` with the `IAMAuth` mode `mode`.
    fn entry(user: &str, mode: Option<&str>) -> UserAuthConfigInfo {
        UserAuthConfigInfo {
            user_name: Some(user.to_string()),
            iam_auth: mode.map(str::to_string),
            ..UserAuthConfigInfo::default()
        }
    }

    /// The proxy the rows below ask about: one database user per `IAMAuth` mode.
    fn proxy() -> ProxyClientAuth {
        ProxyClientAuth::new(
            "us-west-2".to_string(),
            "emu-proxy".to_string(),
            vec![
                entry("required_user", Some(IAM_AUTH_REQUIRED)),
                entry("enabled_user", Some(IAM_AUTH_ENABLED)),
                entry("disabled_user", Some("DISABLED")),
                entry("unstated_user", None),
                // The mode is read however the request spelled it, as every other value of
                // this service's enums is.
                entry("lowercase_user", Some("required")),
            ],
        )
    }

    /// RULE: the credential a proxy asks a client for follows that database user's own
    /// `IAMAuth` mode — a token and nothing else where it is required, either credential where
    /// it is enabled, and the database's own password where it is disabled, unstated, or the
    /// user is not one the proxy logs in as at all.
    ///
    /// The rows are the values `IAMAuthMode` takes
    /// (`smithy_query(query="shape", shape="IAMAuthMode")`: `DISABLED | REQUIRED | ENABLED`),
    /// an entry stating none, a user no entry names, and a mode written in another case.
    #[test]
    fn the_credential_a_proxy_asks_for_follows_the_users_iam_auth_mode() {
        let proxy = proxy();
        // (case, user, requires a token, admits a token)
        let rows: [(&str, &str, bool, bool); 6] = [
            ("IAM authentication required", "required_user", true, true),
            ("IAM authentication enabled", "enabled_user", false, true),
            ("IAM authentication disabled", "disabled_user", false, false),
            ("an entry stating no mode", "unstated_user", false, false),
            ("a user no entry names", "nobody", false, false),
            ("a mode written in another case", "lowercase_user", true, true),
        ];
        for (case, user, requires, admits) in rows {
            let asked = proxy.requires_a_token(user);
            assert_eq!(
                asked, requires,
                "case {case:?}: expected requires_a_token={requires}, got {asked}"
            );
            let admitted = proxy.admits_a_token(user);
            assert_eq!(
                admitted, admits,
                "case {case:?}: expected admits_a_token={admits}, got {admitted}"
            );
        }
    }

    /// RULE: the resource a grant for a login through a proxy is written over names the
    /// DATABASE USER of that proxy, under the `rds-db` namespace, in the proxy's own Region and
    /// this account — so a grant for one user of one proxy is a grant for nothing else.
    #[test]
    fn a_grant_is_written_over_the_database_user_of_the_proxy() {
        let proxy = proxy();
        let arn = proxy.dbuser_arn("required_user");
        let expected = format!(
            "arn:aws:rds-db:us-west-2:{}:dbuser:emu-proxy/required_user",
            ::engine::emulator_account_id()
        );
        assert_eq!(
            arn, expected,
            "case \"the dbuser resource\": expected {expected}, got {arn}"
        );
        let other = proxy.dbuser_arn("enabled_user");
        assert_ne!(
            arn, other,
            "case \"another database user\": expected a resource of its own, got {other}"
        );
    }
}
