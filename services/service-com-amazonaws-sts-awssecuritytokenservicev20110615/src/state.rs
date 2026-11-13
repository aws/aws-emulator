// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sts-awssecuritytokenservicev20110615 service.
//!
//! STS is a credential vendor: every operation either mints a session (`AssumeRole`,
//! `GetSessionToken`, `GetFederationToken`) or reports on one (`GetCallerIdentity`,
//! `GetAccessKeyInfo`). This module holds the registries those operations share —
//! the sessions keyed by the access key id each was minted under, plus the reading of
//! the role records the IAM store in the same engine holds — and the pure rules the
//! handlers apply to them, so a handler file contains only its own operation's decisions.
//!
//! Beside the sessions it keeps the encoded authorization messages the account has
//! issued: STS is also the service that decodes the opaque blob an authorization
//! failure hands a caller (`DecodeAuthorizationMessage`), and the authorization
//! detail behind such a blob is privileged, so the blob carries only the token that
//! names the record here.
//!
//! Secrets are deliberately NOT stored here. The access key id, secret access key
//! and session token are minted by the emulator's credential store
//! ([`mint_session`]), which keeps the secret material; this registry keeps only the
//! public facts a later call reports (who the credential speaks for, when it
//! expires, what was packed into it).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emulator_server::CredentialStore;
use sha2::{Digest, Sha256};

use crate::ops::types::{Credentials, Tag};

/// Which web identity a presented token stands for: the verification
/// `AssumeRoleWithWebIdentity` makes of it, and the class a token it cannot federate is
/// refused as. One concern, one file — it is the only thing in this service that reads a
/// token against the key set its issuer publishes.
pub mod web_identity;

/// Which SAML 2.0 identity a presented authentication response stands for: the verification
/// `AssumeRoleWithSAML` makes of it against the keys the registered provider publishes, and
/// the class a response it cannot federate is refused as. One concern, one file — it is the
/// only thing in this service that reads an XML signature.
pub mod saml;

/// `AssumeRole`'s default session length: "By default, the value is set to `3600`
/// seconds." (`AssumeRoleRequest$DurationSeconds`, this service's model).
pub const ASSUME_ROLE_DEFAULT_DURATION_SECONDS: i32 = 3600;

/// `GetSessionToken`'s and `GetFederationToken`'s default session length:
/// "Acceptable durations ... range from 900 seconds (15 minutes) to 129,600 seconds
/// (36 hours), with 43,200 seconds (12 hours) as the default."
/// (`GetSessionTokenRequest$DurationSeconds` / `GetFederationTokenRequest$DurationSeconds`,
/// this service's model).
pub const VENDED_SESSION_DEFAULT_DURATION_SECONDS: i32 = 43_200;

/// The size, in bytes, of the largest serialized session token STS will issue — the
/// budget the session policies and session tags of ONE mint are packed into, from the
/// model's `minimumSessionTokenSizeType` range (`0..4096`).
///
/// It is the limit BOTH halves of the packed-policy rule are stated against: a mint
/// whose packed token passes it is refused ([`packed_policy_too_large_message`]), and
/// `PackedPolicySize` — "a percentage value that indicates the packed size of the
/// session policies and session tags combined passed in the request" whose "request
/// fails if the packed size is greater than 100 percent" (`AssumeRole`'s
/// documentation, this service's model) — is that same size as a percentage of it
/// ([`packed_policy_size`]). One budget for the two is what makes them agree: over 100
/// percent is exactly over the byte limit.
pub const SESSION_TOKEN_SIZE_LIMIT: usize = 4096;

/// The name of the ambient IAM user: the identity a request speaks for when its
/// access key id names no session this registry minted (the long-term key the
/// emulator publishes at start, or an unsigned request).
///
/// Stated once by [`::service_common::caller_principal`], because IAM holds that user
/// among the account's own users: the service that HOLDS the principal and this one,
/// which REPORTS it as the caller, must not name it differently.
pub const AMBIENT_USER_NAME: &str = ::service_common::caller_principal::AMBIENT_USER_NAME;

/// The most managed session policies one mint may name: "You can provide up to 10
/// managed policy ARNs" (`AssumeRoleRequest$PolicyArns`, this service's model). The
/// model declares no length on `policyDescriptorListType`, so the ceiling is enforced
/// here.
pub const MAX_MANAGED_SESSION_POLICIES: usize = 10;

/// A role's session length when IAM holds no `MaxSessionDuration` for it: "The maximum
/// session duration setting can have a value from 1 hour to 12 hours"
/// (`AssumeRoleRequest$DurationSeconds`), and IAM's own default is the low end of that
/// range, which is also this operation's default duration.
pub const ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS: i32 = ASSUME_ROLE_DEFAULT_DURATION_SECONDS;

/// The ceiling on a ROLE-CHAINED session: "Role chaining limits your Amazon Web
/// Services CLI or Amazon Web Services API role session to a maximum of one hour"
/// (`AssumeRoleRequest$DurationSeconds`, this service's model).
pub const ROLE_CHAINING_MAX_DURATION_SECONDS: i32 = 3600;

/// The IAM action a role's trust policy must allow for a principal to assume it.
pub const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// The IAM action a role's trust policy must allow for a mint to carry session tags.
pub const TAG_SESSION_ACTION: &str = "sts:TagSession";

/// The IAM action a role's trust policy must allow for a mint to set a source identity.
pub const SET_SOURCE_IDENTITY_ACTION: &str = "sts:SetSourceIdentity";

/// The IAM action a role's trust policy must allow for a WEB IDENTITY to assume it:
/// "The trust policies of these roles must accept the cognito-identity.amazonaws.com
/// service principal" (`AssumeRoleWithWebIdentity$RoleArn`, this service's model), which is
/// a grant of this action to that principal.
pub const ASSUME_ROLE_WITH_WEB_IDENTITY_ACTION: &str = "sts:AssumeRoleWithWebIdentity";

/// The IAM action a role's trust policy must allow for a SAML 2.0 identity to assume it:
/// the trust policy of such a role names the IAM SAML provider as its `Federated` principal
/// and grants it this action (`AssumeRoleWithSAML`, this service's model).
pub const ASSUME_ROLE_WITH_SAML_ACTION: &str = "sts:AssumeRoleWithSAML";

/// The condition key a trust policy tests a request's `ExternalId` through.
pub const EXTERNAL_ID_CONDITION_KEY: &str = "sts:ExternalId";

/// The `Principal` spelling that names a whole account: the account's root user.
fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// Who a credential speaks for, as `GetCallerIdentity` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user_id: String,
    pub account: String,
    pub arn: String,
}

/// Which operation minted a session. Kept because the three kinds are not
/// interchangeable: only an `AssumeRole` session carries a role and a source
/// identity, and only a `GetFederationToken` session carries a federated user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    AssumedRole,
    SessionToken,
    FederationToken,
}

/// The session policies ONE mint passed, which scope the session's permissions down, and
/// the resolution of what they SAY at the instant of a request.
///
/// "Passing policies to this operation returns new temporary credentials. The resulting
/// session's permissions are the intersection of the role's identity-based policies and the
/// session policies" (`AssumeRoleRequest$Policy`, this service's model): what a request
/// signed with the credential may do is what the identity's own policies allow AND what
/// these documents allow, and an explicit `Deny` in any of them refuses whatever a grant
/// says.
///
/// Both the record and its resolution live in the IAM store's module, because the documents
/// a managed session policy stands for are IAM's and the gate that decides a call against
/// them is IAM's too ([`IamState`]) — one definition, so this service, IAM's own gate and
/// every other service handed the same credential read one scope. They are re-exported here
/// because a session is this service's: [`Session::policies`] is what a mint records, and
/// [`Caller::session_policies`] is what the credential presented by a later request carries.
pub use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    session_scoping_documents, ScopingPolicies, SessionPolicies,
};

/// One minted session, keyed in [`ComAmazonawsStsAwssecuritytokenservicev20110615State::sessions`]
/// by the access key id it was minted under.
#[derive(Debug, Clone)]
pub struct Session {
    /// The registry key: the access key id the credential store minted.
    pub access_key_id: String,
    pub created_at: SystemTime,
    /// `created_at + duration_seconds`: the wall-clock instant the credential stops
    /// being valid.
    pub expires_at: SystemTime,
    pub duration_seconds: i32,
    /// What `GetCallerIdentity` answers for a request signed with this credential.
    pub identity: Identity,
    pub kind: SessionKind,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    pub packed_policy_size: Option<i32>,
    /// The session policies the mint passed, which scope what this session may do; see
    /// [`SessionPolicies`].
    pub policies: SessionPolicies,
    pub external_id: Option<String>,
    /// The ARN of the role an `AssumeRole` session was minted for, exactly as IAM holds
    /// it — the spelling a trust policy names when this session chains into another
    /// role. `None` for every other kind of session.
    pub role_arn: Option<String>,
    /// Set when the mint of this session SATISFIED an MFA challenge — a `SerialNumber`
    /// with the `TokenCode` the device answers ([`mfa_challenge_failure`]); see
    /// [`Caller::multi_factor_auth_present`], which is what a policy condition over the
    /// request's MFA key is decided against.
    pub mfa_authenticated: bool,
    /// The condition keys the WEB IDENTITY this session was minted by exchanging publishes,
    /// as `(key, value)` pairs already named after the provider that issued the token; see
    /// [`federated_claim_keys`]. Empty for a session no web-identity exchange minted.
    ///
    /// Bounded by the token the exchange presented, whose own modelled length
    /// (`AssumeRoleWithWebIdentityRequest$WebIdentityToken` is `clientTokenType`, length
    /// 4..20000) caps the claims it can carry, and by [`MAX_SESSIONS`] records in all.
    pub federated_claims: Vec<(String, String)>,
    /// Set once the credential has been deleted from the credential store because
    /// the session expired, so the deletion happens exactly once.
    pub revoked: bool,
}

impl Session {
    /// Whether the clock has passed this session's expiry.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }
}

/// What a caller asks for when it mints a session: everything about the session
/// that does not come from the credential store.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub duration_seconds: i32,
    pub identity: Identity,
    pub kind: SessionKind,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    pub packed_policy_size: Option<i32>,
    /// See [`Session::policies`].
    pub policies: SessionPolicies,
    pub external_id: Option<String>,
    /// See [`Session::role_arn`].
    pub role_arn: Option<String>,
    /// See [`Session::mfa_authenticated`].
    pub mfa_authenticated: bool,
    /// See [`Session::federated_claims`].
    pub federated_claims: Vec<(String, String)>,
}

impl NewSession {
    /// A session of `kind` for `identity`, lasting `duration_seconds`, carrying no
    /// policy, tags or source identity. Fill the rest with struct-update syntax.
    pub fn new(identity: Identity, kind: SessionKind, duration_seconds: i32) -> Self {
        Self {
            duration_seconds,
            identity,
            kind,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: SessionPolicies::default(),
            external_id: None,
            role_arn: None,
            mfa_authenticated: false,
            federated_claims: Vec::new(),
        }
    }

    /// The registry record of this session, as minted under `access_key_id` at `now`.
    ///
    /// The ONE place a [`Session`] record is built out of what a mint asked for, so every
    /// recorded session carries the same derived facts however it was recorded — the one
    /// [`mint_session`] vends a fresh credential for, and the one a caller that already
    /// holds the access key id records for it. The expiry is `now + duration_seconds`,
    /// the instant the credential stops being valid, and the record starts un-revoked.
    ///
    /// This is a constructor and nothing else: recording the session in the registry, and
    /// in the account's audit view of its sessions, stays with the caller that does it.
    ///
    /// `duration_seconds` is the duration a mint already held to the modelled range
    /// (`AssumeRole$DurationSeconds` is `range 900..43200`), so a value below zero is not
    /// reachable from a served request; one reaching here anyway lasts no time at all,
    /// which is the shortest session this registry can hold rather than a longer one than
    /// was asked for.
    #[must_use]
    pub fn into_session(self, access_key_id: impl Into<String>, now: SystemTime) -> Session {
        let lasts = Duration::from_secs(u64::try_from(self.duration_seconds).unwrap_or(0));
        Session {
            access_key_id: access_key_id.into(),
            created_at: now,
            expires_at: now + lasts,
            duration_seconds: self.duration_seconds,
            identity: self.identity,
            kind: self.kind,
            source_identity: self.source_identity,
            tags: self.tags,
            transitive_tag_keys: self.transitive_tag_keys,
            packed_policy_size: self.packed_policy_size,
            policies: self.policies,
            external_id: self.external_id,
            role_arn: self.role_arn,
            mfa_authenticated: self.mfa_authenticated,
            federated_claims: self.federated_claims,
            revoked: false,
        }
    }
}

/// A minted session: the credential to hand back to the caller, and the record this
/// registry keeps of it.
#[derive(Debug, Clone)]
pub struct MintedSession {
    pub credentials: Credentials,
    pub session: Session,
}

/// The session credential a request presented has expired. Every operation refuses
/// such a request rather than answering it as some other identity.
#[derive(Debug, Clone)]
pub struct ExpiredSession {
    pub access_key_id: String,
}

impl ExpiredSession {
    /// The wording AWS uses for an expired session credential.
    pub fn message(&self) -> String {
        "The security token included in the request is expired".to_string()
    }
}

/// The most sessions this registry holds at once.
///
/// A mint is a request-driven insert, so the registry needs a declared ceiling: at
/// the cap a mint makes room by giving up sessions the clock has already expired,
/// and is refused outright rather than dropping a LIVE session — a credential this
/// service handed out keeps working for as long as it was promised. The cap also
/// bounds the per-request expiry sweep, which is therefore O(cap), not O(calls made
/// so far). The value is a chosen emulator ceiling, far above any test or demo
/// workload; AWS publishes no such limit.
pub const MAX_SESSIONS: usize = 4096;

/// How long an expired session's record is kept after its expiry.
///
/// Keeping it briefly is what lets a caller still presenting the credential be told
/// the token EXPIRED instead of being quietly answered as some other identity; past
/// this window the record is pruned, so an idle emulator's registry shrinks back to
/// its live sessions. A chosen retention window, not a duration AWS documents.
pub const EXPIRED_SESSION_RETENTION: Duration = Duration::from_secs(3600);

/// Why a session could not be minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintError {
    /// The registry is full of sessions that have not expired yet.
    AtCapacity,
    /// The credential store minted a credential without its secrets, which the kind
    /// asked for always carries.
    NoSecrets,
}

impl MintError {
    /// Why the request was refused, for the caller to act on.
    pub fn message(&self) -> String {
        match self {
            Self::AtCapacity => format!(
                "this emulator holds at most {MAX_SESSIONS} sessions that have not expired yet; \
                 let some expire before minting another"
            ),
            Self::NoSecrets => {
                "the credential store minted a session without its secrets; no session was created"
                    .to_string()
            }
        }
    }
}

/// Per-service state for com-amazonaws-sts-awssecuritytokenservicev20110615. A handler reaches it through [`service_state`].
///
/// One store, bounded by [`MAX_SESSIONS`]: the sessions this service minted. There is
/// deliberately no map for role ids — a role's id belongs to the role, so it is read
/// from the IAM store in the same engine ([`resolve_role`]), the service that holds the
/// role; remembering one here would add a request-driven store without making anything
/// more stable.
#[derive(Debug, Default)]
pub struct ComAmazonawsStsAwssecuritytokenservicev20110615State {
    /// Every session this service minted and has not pruned, keyed by its access key
    /// id. At most [`MAX_SESSIONS`] entries; each entry's tags and transitive tag keys
    /// are bounded by [`MAX_SESSION_TAGS`] — the model's own `tagListType` /
    /// `tagKeyListType` maximum, which the mint path checks over the EFFECTIVE set (a
    /// chained mint's own tags plus the ones it carries forward), so a role chain
    /// cannot grow one record past it — and each key and value by
    /// [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`].
    pub sessions: HashMap<String, Session>,
    /// Every encoded authorization message the account has issued and not yet let
    /// expire, keyed by the token the blob carries. At most
    /// [`MAX_AUTHORIZATION_MESSAGES`] entries.
    pub authorization_messages: HashMap<String, AuthorizationMessage>,
    /// How many encoded authorization messages this account has ever issued. It only
    /// ever counts up — unlike the registry's size, which shrinks as messages expire
    /// or give way at the cap — which is what makes every token distinct.
    pub authorization_messages_issued: u64,
    /// Every trade-in token the account holds and has not pruned. At most
    /// [`MAX_TRADE_IN_TOKENS`] entries. See [`TradeInTokens`].
    pub trade_in_tokens: TradeInTokens,
}

/// Shorthand for this service's state, for use inside the crate.
pub type StsState = ComAmazonawsStsAwssecuritytokenservicev20110615State;

impl StsState {
    /// The session an access key id names, expired or not.
    pub fn session(&self, access_key_id: &str) -> Option<&Session> {
        self.sessions.get(access_key_id)
    }

    /// Record a minted session under its access key id.
    ///
    /// Refuses once the registry holds [`MAX_SESSIONS`], so NO path can push it past
    /// the cap; [`mint_session`] calls [`StsState::make_room`] first, which is what
    /// normally keeps that from happening.
    pub fn insert(&mut self, session: Session) -> Result<(), MintError> {
        let replacing = self.sessions.contains_key(&session.access_key_id);
        if !replacing && self.sessions.len() >= MAX_SESSIONS {
            return Err(MintError::AtCapacity);
        }
        self.sessions.insert(session.access_key_id.clone(), session);
        Ok(())
    }

    /// Make room for one more session: materialise due expiries (which prunes the
    /// records whose retention window has run out), then give up expired sessions,
    /// earliest expiry first, until the registry is under the cap. A session that has
    /// NOT expired is never dropped to make room; when only such sessions remain the
    /// mint is refused.
    pub fn make_room(&mut self, now: SystemTime, store: &CredentialStore) -> Result<(), MintError> {
        self.revoke_expired(now, store);
        while self.sessions.len() >= MAX_SESSIONS {
            let giveable = self
                .sessions
                .values()
                .filter(|session| session.is_expired(now))
                .min_by_key(|session| session.expires_at)
                .map(|session| session.access_key_id.clone());
            match giveable {
                Some(access_key_id) => self.discard(&access_key_id, store),
                None => return Err(MintError::AtCapacity),
            }
        }
        Ok(())
    }

    /// Drop a session and the credential it was minted with — for a mint this
    /// service decided, after minting, that it cannot honour.
    pub fn discard(&mut self, access_key_id: &str, store: &CredentialStore) {
        self.sessions.remove(access_key_id);
        let _ = store.delete(access_key_id);
    }

    /// Materialise every expiry the clock has already made due: the credential of an
    /// expired session is deleted from the credential store, so a request signed
    /// with it no longer verifies, and a record whose [`EXPIRED_SESSION_RETENTION`]
    /// window has run out is dropped altogether. Both are owed at the same instant
    /// whether or not anybody called, which is why a reporting operation may do
    /// them, and the pruning is what keeps the registry's size tied to the sessions
    /// still in play rather than to every session ever minted.
    ///
    /// Inside the retention window the session RECORD is kept, so a caller still
    /// presenting the credential is told the token expired instead of being quietly
    /// answered as the ambient identity.
    pub fn revoke_expired(&mut self, now: SystemTime, store: &CredentialStore) {
        for session in self.sessions.values_mut() {
            if !session.revoked && session.is_expired(now) {
                let _ = store.delete(&session.access_key_id);
                session.revoked = true;
            }
        }
        self.sessions
            .retain(|_, session| now < session.expires_at + EXPIRED_SESSION_RETENTION);
    }
}

// ---------------------------------------------------------------------------
// Encoded authorization messages
// ---------------------------------------------------------------------------

/// The envelope every encoded authorization message this account issues starts with,
/// before it is base64-encoded. It is what separates a blob this service minted from
/// any other run of bytes a caller may hand `DecodeAuthorizationMessage`, and it
/// carries a version so a later envelope can be told from this one.
const ENCODED_MESSAGE_ENVELOPE: &[u8] = b"STSEncodedAuthorizationMessage/1:";

/// How long an encoded authorization message stays decodable.
///
/// A CHOSEN window, not a duration AWS documents — the model says only that the
/// message may have "expired". It is long enough for a caller that was handed a blob
/// by an authorization failure to decode it, and short enough that the registry
/// shrinks back on an idle emulator.
pub const ENCODED_MESSAGE_VALIDITY: Duration = Duration::from_secs(3600);

/// The most encoded authorization messages this registry holds at once.
///
/// Issuing one is a request-driven insert, so the registry needs a declared ceiling.
/// At the cap the oldest record gives way: unlike a credential, an encoded message is
/// a diagnostic the caller may decode once, not a promise this service made for a
/// stated duration. A chosen emulator ceiling; AWS publishes no such limit.
pub const MAX_AUTHORIZATION_MESSAGES: usize = 1024;

/// How many digest bytes name one record: 16, rendered as 32 hex characters — far
/// more than enough to keep the [`MAX_AUTHORIZATION_MESSAGES`] records held at once
/// apart.
const AUTHORIZATION_MESSAGE_TOKEN_BYTES: usize = 16;

/// What an authorization failure decided, as the decoded message reports it: whether
/// the request was allowed, whether a `Deny` statement said so explicitly, and the
/// context of the request it was decided for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    /// Whether the refusal came from an explicit `Deny` rather than from the absence
    /// of an `Allow`.
    pub explicit_deny: bool,
    /// The principal that made the request: its unique id and its ARN.
    pub principal_id: String,
    pub principal_arn: String,
    /// The requested action and resource.
    pub action: String,
    pub resource: String,
    /// The condition keys in the context of the request, each with its values.
    pub conditions: Vec<(String, Vec<String>)>,
    /// The policy statements the decision matched, each by its statement id and its
    /// effect. Empty when the request matched none, which is what an implicit deny
    /// looks like.
    pub matched_statements: Vec<(String, String)>,
}

/// One encoded authorization message the account has issued, keyed in
/// [`StsState::authorization_messages`] by the token its blob carries.
#[derive(Debug, Clone)]
pub struct AuthorizationMessage {
    /// The registry key: the token the encoded blob names this record by.
    pub token: String,
    pub issued_at: SystemTime,
    /// `issued_at + ENCODED_MESSAGE_VALIDITY`: past this instant the message is no
    /// longer decodable.
    pub expires_at: SystemTime,
    /// The document `DecodeAuthorizationMessage` answers with.
    pub decoded_message: String,
}

impl StsState {
    /// The record a token names, expired or not.
    pub fn authorization_message(&self, token: &str) -> Option<&AuthorizationMessage> {
        self.authorization_messages.get(token)
    }

    /// Materialise every encoded-message expiry the clock has already made due: a
    /// message past its validity window cannot be decoded any more, whether or not
    /// anybody called, so its record is dropped. That is what keeps the registry's
    /// size tied to the messages still decodable rather than to every one ever
    /// issued.
    pub fn expire_authorization_messages(&mut self, now: SystemTime) {
        self.authorization_messages
            .retain(|_, message| now < message.expires_at);
    }

    /// Record an issued encoded authorization message, bounded by
    /// [`MAX_AUTHORIZATION_MESSAGES`]: expiries the clock has made due are
    /// materialised first, and at the cap the earliest-issued record gives way, so NO
    /// path can push the registry past the cap.
    pub fn record_authorization_message(&mut self, message: AuthorizationMessage, now: SystemTime) {
        self.expire_authorization_messages(now);
        while self.authorization_messages.len() >= MAX_AUTHORIZATION_MESSAGES {
            let oldest = self
                .authorization_messages
                .values()
                .min_by(|one, other| {
                    (one.issued_at, one.token.as_str()).cmp(&(other.issued_at, other.token.as_str()))
                })
                .map(|held| held.token.clone());
            match oldest {
                Some(token) => {
                    self.authorization_messages.remove(&token);
                }
                None => break,
            }
        }
        self.authorization_messages
            .insert(message.token.clone(), message);
    }
}

/// The decoded message for an authorization decision: the document AWS's
/// `DecodeAuthorizationMessage` answers with, which states whether the request was
/// allowed and whether a deny was explicit, the statements the decision matched, and
/// the context it was decided in — the principal, the action, the resource and the
/// condition keys of the request.
pub fn authorization_decision_document(decision: &AuthorizationDecision) -> String {
    let matched: Vec<serde_json::Value> = decision
        .matched_statements
        .iter()
        .map(|(statement_id, effect)| {
            serde_json::json!({ "statementId": statement_id, "effect": effect })
        })
        .collect();
    let conditions: Vec<serde_json::Value> = decision
        .conditions
        .iter()
        .map(|(key, values)| {
            let values: Vec<serde_json::Value> = values
                .iter()
                .map(|value| serde_json::json!({ "value": value }))
                .collect();
            serde_json::json!({ "key": key, "values": { "items": values } })
        })
        .collect();
    serde_json::json!({
        "allowed": decision.allowed,
        "explicitDeny": decision.explicit_deny,
        "matchedStatements": { "items": matched },
        // Nothing here evaluates a policy it could not read, so there is never a
        // failure to report — the member is still part of the document's shape.
        "failures": { "items": [] },
        "context": {
            "principal": { "id": decision.principal_id, "arn": decision.principal_arn },
            "action": decision.action,
            "resource": decision.resource,
            "conditions": { "items": conditions },
        },
    })
    .to_string()
}

/// Issue an encoded authorization message for an authorization decision: record the
/// decoded document and hand back the opaque blob the denying operation returns to its
/// caller.
///
/// The blob is the envelope and the token that names the record, and nothing
/// else: the authorization detail "can contain privileged information that the user
/// who requested the operation should not see" (this operation's documentation), so it
/// stays here and never travels with the blob. It is rendered in the URL-safe base64
/// alphabet with the padding stripped, the form an encoded authorization message
/// travels in.
pub fn encode_authorization_message(
    state: &mut StsState,
    now: SystemTime,
    decision: &AuthorizationDecision,
) -> String {
    let decoded_message = authorization_decision_document(decision);
    state.authorization_messages_issued = state.authorization_messages_issued.wrapping_add(1);
    let token = authorization_message_token(
        &decoded_message,
        now,
        state.authorization_messages_issued,
    );
    state.record_authorization_message(
        AuthorizationMessage {
            token: token.clone(),
            issued_at: now,
            expires_at: now + ENCODED_MESSAGE_VALIDITY,
            decoded_message,
        },
        now,
    );
    let mut payload = ENCODED_MESSAGE_ENVELOPE.to_vec();
    payload.extend_from_slice(token.as_bytes());
    ::aws_smithy_types::base64::encode(payload)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// The decoded message an encoded authorization message carries, or the wording to
/// refuse it with.
///
/// A message decodes only when it is base64 of this account's envelope AND names a
/// record the registry still holds: bytes that are not base64 at all, base64 of
/// something this account never issued, and a message whose validity window has run
/// out are all messages this service cannot decode — exactly the cases
/// `InvalidAuthorizationMessageException` reports. A read, so nothing recorded here
/// changes: the same message decodes again to the same document for as long as it is
/// valid.
pub fn decode_authorization_message(
    state: &StsState,
    encoded: &str,
    now: SystemTime,
) -> Result<String, String> {
    let Some(bytes) = encoded_message_bytes(encoded) else {
        return Err(invalid_authorization_message(encoded.as_bytes()));
    };
    bytes
        .strip_prefix(ENCODED_MESSAGE_ENVELOPE)
        .and_then(|token| std::str::from_utf8(token).ok())
        .and_then(|token| state.authorization_message(token))
        .filter(|record| now < record.expires_at)
        .map(|record| record.decoded_message.clone())
        .ok_or_else(|| invalid_authorization_message(&bytes))
}

/// The codec an encoded authorization message is READ with.
///
/// A blob that reached this service was written by something else, so it is decoded
/// with the latitude the service that decodes one has rather than with the strictness
/// an ENCODER is held to: the padding is optional, because it carries no data, and the
/// bits left over in a final group that is not a whole number of bytes are discarded
/// rather than being required to be zero. A message whose last group carries such bits
/// is base64 all the same — it names the same bytes under either reading — so refusing
/// to read it would report a blob's own characters as the bytes it carried.
const ENCODED_MESSAGE_CODEC: ::base64::engine::GeneralPurpose =
    ::base64::engine::GeneralPurpose::new(
        &::base64::alphabet::STANDARD,
        ::base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(::base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );

/// The bytes an encoded message carries, or `None` when it is not base64 at all.
///
/// Read in BOTH base64 alphabets — the standard one (`+`, `/`) and the URL-safe one
/// (`-`, `_`), which is the one AWS's own encoded authorization messages travel in —
/// with the latitude [`ENCODED_MESSAGE_CODEC`] states. Whitespace and anything outside
/// those alphabets — "invalid characters, such as line breaks", as the modelled error
/// puts it — is not base64 here, so such a message carries no bytes at all, and
/// neither does one whose final group is a single character, which names no byte.
fn encoded_message_bytes(encoded: &str) -> Option<Vec<u8>> {
    use ::base64::Engine as _;

    // Only a message in the URL-safe alphabet needs a copy; one already in the
    // standard alphabet is decoded where it lies.
    let standard: std::borrow::Cow<'_, str> = if encoded.contains(['-', '_']) {
        std::borrow::Cow::Owned(
            encoded
                .chars()
                .map(|character| match character {
                    '-' => '+',
                    '_' => '/',
                    other => other,
                })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(encoded)
    };
    ENCODED_MESSAGE_CODEC.decode(standard.as_ref()).ok()
}

/// The wording a message this service cannot decode is refused with: the sentence
/// AWS answers, followed by the bytes of the message rendered as signed 8-bit values.
///
/// Those bytes are all the service can report of a blob it did not issue — nothing
/// else about it is known to it — and they are the decoded bytes, which is what the
/// message actually carried. A blob that is not base64 at all has none, so its own
/// bytes are what gets reported. The rendering is bounded by the request: the model
/// caps `EncodedMessage` at 10,240 characters, and the boundary validator refuses a
/// longer one before any handler runs.
fn invalid_authorization_message(bytes: &[u8]) -> String {
    let mut rendered = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(&byte.cast_signed().to_string());
    }
    format!("Message is not valid: [{rendered}]")
}

/// The token that names one recorded decision: hex of a SHA-256 digest over the
/// decoded document, the instant the message was issued and how many messages this
/// account has issued so far — so a token is a function of the message it names, and
/// two messages for the same decision, even at the same instant, get different tokens.
///
/// It is an identifier, not a secret: what it names is a decision this emulator itself
/// recorded, and the caller it was handed to is the one the decision is about.
fn authorization_message_token(document: &str, issued_at: SystemTime, issued: u64) -> String {
    let nanos = issued_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut digest = Sha256::new();
    digest.update(document.as_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(issued.to_be_bytes());
    digest
        .finalize()
        .iter()
        .take(AUTHORIZATION_MESSAGE_TOKEN_BYTES)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// Trade-in tokens: the delegations a trade-in token can be exchanged for
// ---------------------------------------------------------------------------

/// The most trade-in tokens this registry holds at once.
///
/// A declared ceiling, like the one on every other store here: at the cap the
/// earliest-issued record gives way, so no path can grow the registry without bound. A
/// chosen emulator ceiling; AWS publishes no such limit.
pub const MAX_TRADE_IN_TOKENS: usize = 1024;

/// How long a trade-in token's record is kept after it expires.
///
/// Keeping it briefly is what lets a caller still presenting the token be told it
/// EXPIRED — the one failure the operation models for a token it knows — instead of being
/// told the token is not one the account holds; past this window the record is pruned, so
/// an idle emulator's registry shrinks back to the tokens still exchangeable. A chosen
/// retention window, not a duration AWS documents, and the same one expired sessions get
/// ([`EXPIRED_SESSION_RETENTION`]).
pub const EXPIRED_TRADE_IN_TOKEN_RETENTION: Duration = EXPIRED_SESSION_RETENTION;

/// The session length credentials exchanged for a trade-in token last when the record
/// states none: STS's own default for a session of an assumed principal
/// ([`ASSUME_ROLE_DEFAULT_DURATION_SECONDS`]), because the credentials this exchange
/// vends are credentials of an assumed principal and `GetDelegatedAccessToken` takes no
/// duration of its own.
pub const DELEGATED_ACCESS_DEFAULT_DURATION_SECONDS: i32 = ASSUME_ROLE_DEFAULT_DURATION_SECONDS;

/// One trade-in token the account holds, keyed in [`TradeInTokens`] by the token value a
/// request presents.
///
/// A trade-in token stands for a DELEGATION established outside this service: no
/// operation of this model issues one (`smithy_query(query="digest")` binds eleven
/// operations and none returns a trade-in token), so a record here is what the account
/// holds about a token some delegation flow handed a caller — which principal the token
/// speaks for, until when it may be exchanged, and how long the credentials it is
/// exchanged for last.
///
/// No secret lives in the record itself: a trade-in token IS its own bearer credential,
/// so a registry keyed BY the token holds nothing about it its holder does not already
/// have, and the record carries nothing that would let a token be derived rather than
/// presented.
#[derive(Clone)]
pub struct TradeInToken {
    /// The registry key: the token value a request presents.
    pub token: String,
    pub issued_at: SystemTime,
    /// The instant the token stops being exchangeable: "this token must be valid and
    /// unexpired at the time of the request"
    /// (`GetDelegatedAccessTokenRequest$TradeInToken`, this service's model).
    pub expires_at: SystemTime,
    /// The principal the token stands for — "the principal that was assumed when
    /// obtaining the delegated access token" — whose ARN the exchange reports as
    /// `AssumedPrincipal` and whose identity the vended credentials speak for.
    pub assumed_principal: Identity,
    /// The ARN of the ROLE the assumed principal is a session of, when it is one: the
    /// spelling a trust policy names if these credentials chain into another role. See
    /// [`Session::role_arn`].
    pub role_arn: Option<String>,
    /// How long the credentials exchanged for this token last, or `None` for
    /// [`DELEGATED_ACCESS_DEFAULT_DURATION_SECONDS`].
    pub duration_seconds: Option<i32>,
}

/// Renders the delegation and the window, and NOT the token: the value is a bearer of the
/// principal it names, so a derived rendering would put it into whatever formatted the
/// record, a log line included.
impl ::std::fmt::Debug for TradeInToken {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("TradeInToken")
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("assumed_principal", &self.assumed_principal)
            .field("role_arn", &self.role_arn)
            .field("duration_seconds", &self.duration_seconds)
            .finish_non_exhaustive()
    }
}

impl TradeInToken {
    /// Whether the clock has passed this token's expiry.
    #[must_use]
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }

    /// The session length credentials exchanged for this token last.
    #[must_use]
    pub fn session_duration_seconds(&self) -> i32 {
        self.duration_seconds
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DELEGATED_ACCESS_DEFAULT_DURATION_SECONDS)
    }
}

/// Every trade-in token the account holds and this registry has not pruned, keyed by the
/// token value a request presents.
#[derive(Default)]
pub struct TradeInTokens {
    held: HashMap<String, TradeInToken>,
}

/// Reports how many records are held and nothing else, for the reason
/// [`TradeInToken`]'s own rendering withholds the token: the KEYS of this registry are
/// the tokens themselves, so a derived rendering would put every outstanding one into
/// whatever formatted the store. A record is still readable through
/// [`TradeInTokens::held`] by a caller that already holds the token.
impl ::std::fmt::Debug for TradeInTokens {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("TradeInTokens")
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl TradeInTokens {
    /// The record a presented token names, EXPIRED OR NOT: a token the account holds and
    /// that has run out is a different answer from one it holds no record of, so the
    /// expiry is the caller's decision to take ([`TradeInToken::is_expired`]).
    ///
    /// A READ: nothing is dropped here, so two callers presenting one token are answered
    /// alike.
    #[must_use]
    pub fn held(&self, token: &str) -> Option<&TradeInToken> {
        self.held.get(token)
    }

    /// How many records the registry holds, expired ones included until they are pruned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether the registry holds no trade-in token at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Materialise every expiry the clock has already made due: a record whose
    /// [`EXPIRED_TRADE_IN_TOKEN_RETENTION`] window has run out is dropped, which is owed
    /// at the same instant whether or not anybody called. Inside the window the record is
    /// kept, so a caller presenting an expired token is told it EXPIRED rather than that
    /// the account holds no such token.
    pub fn prune(&mut self, now: SystemTime) {
        self.held
            .retain(|_, token| now < token.expires_at + EXPIRED_TRADE_IN_TOKEN_RETENTION);
    }

    /// Record a trade-in token the account holds, bounded by [`MAX_TRADE_IN_TOKENS`]:
    /// expiries the clock has made due are materialised first, and at the cap the
    /// earliest-issued record gives way, so NO path can push the registry past the cap.
    pub fn record(&mut self, token: TradeInToken, now: SystemTime) {
        self.prune(now);
        while self.held.len() >= MAX_TRADE_IN_TOKENS {
            let oldest = self
                .held
                .iter()
                .min_by(|(one_key, one), (other_key, other)| {
                    (one.issued_at, one_key.as_str()).cmp(&(other.issued_at, other_key.as_str()))
                })
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.held.remove(&key);
                }
                None => break,
            }
        }
        self.held.insert(token.token.clone(), token);
    }
}

impl StsState {
    /// The trade-in token record a presented token names, expired or not. See
    /// [`TradeInTokens::held`].
    pub fn trade_in_token(&self, token: &str) -> Option<&TradeInToken> {
        self.trade_in_tokens.held(token)
    }

    /// Materialise every trade-in-token expiry the clock has already made due. See
    /// [`TradeInTokens::prune`].
    pub fn expire_trade_in_tokens(&mut self, now: SystemTime) {
        self.trade_in_tokens.prune(now);
    }

    /// Record a trade-in token the account holds. See [`TradeInTokens::record`].
    pub fn record_trade_in_token(&mut self, token: TradeInToken, now: SystemTime) {
        self.trade_in_tokens.record(token, now);
    }
}

/// How AWS words the refusal of a trade-in token this account holds no record of.
///
/// The operation models no error for a token it cannot resolve at all — only for one it
/// knows and that has EXPIRED — so the refusal is the service's own `ValidationError`
/// (HTTP 400) carrying this sentence. It is fixed text: the token is `@sensitive` in this
/// service's model, so the value presented is not echoed back, and nothing else of the
/// request appears in it either.
pub const INVALID_TRADE_IN_TOKEN_MESSAGE: &str = "Invalid trade-in token";

/// How this service words the refusal of a trade-in token the account HOLDS but whose
/// validity window has run out: `ExpiredTradeInTokenException`'s runtime message, in the
/// same sentence shape STS states an expired credential with
/// ([`ExpiredSession::message`]) and likewise carrying nothing of the request.
pub const EXPIRED_TRADE_IN_TOKEN_MESSAGE: &str =
    "The trade-in token included in the request is expired";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsStsAwssecuritytokenservicev20110615State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsStsAwssecuritytokenservicev20110615State>)
}

/// The identity a request speaks for, resolved from the access key id it signed
/// with: the identity of the session that key was minted for, or the ambient
/// identity when the key names no session here.
///
/// Expiries the clock has made due are materialised first, so an expired session is
/// refused rather than answered.
pub fn resolve_caller(
    state: &mut StsState,
    access_key_id: &str,
    now: SystemTime,
    store: &CredentialStore,
) -> Result<Identity, ExpiredSession> {
    resolve_request_caller(state, access_key_id, now, store, ambient_identity())
        .map(|caller| caller.identity)
}

/// Who a request speaks for, and everything about its credential a later decision needs:
/// which operation minted it (`None` for a long-term access key), and — for a session —
/// the source identity and session tags it carries forward.
#[derive(Debug, Clone)]
pub struct Caller {
    pub identity: Identity,
    /// Which STS operation minted the credential, or `None` when the key names no
    /// session here: a long-term IAM access key, or the key this emulator publishes.
    pub kind: Option<SessionKind>,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    /// The role ARN of an assumed-role credential; see [`Session::role_arn`].
    pub role_arn: Option<String>,
    /// The condition keys the web identity this credential was minted by exchanging
    /// publishes; see [`Session::federated_claims`]. Empty for every other credential.
    pub federated_claims: Vec<(String, String)>,
    /// The session policies the mint of this credential passed, which scope what a
    /// request signed with it may do; see [`SessionPolicies`]. Empty for a long-term
    /// access key, which no mint scoped.
    pub session_policies: SessionPolicies,
    /// Set when the mint of this credential satisfied an MFA challenge; see
    /// [`Session::mfa_authenticated`] and [`Caller::multi_factor_auth_present`].
    pub mfa_authenticated: bool,
}

impl Caller {
    /// A caller presenting no session credential: the long-term identity `identity`.
    fn long_term(identity: Identity) -> Self {
        Self {
            identity,
            kind: None,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            role_arn: None,
            federated_claims: Vec::new(),
            session_policies: SessionPolicies::default(),
            mfa_authenticated: false,
        }
    }

    /// What the global condition key naming whether the request was signed with
    /// MULTI-FACTOR-AUTHENTICATED credentials answers for this caller: `Some(true)` for a
    /// session whose mint satisfied an MFA challenge, `Some(false)` for every other
    /// session, and `None` for a long-term access key, which carries no value for the key
    /// at all.
    ///
    /// "This key is present in the request context only when the principal uses temporary
    /// credentials" (AWS General Reference, *Global condition context keys*,
    /// `aws:MultiFactorAuthPresent`), which is why a long-term key answers nothing rather
    /// than answering `false`: the difference is what an `…IfExists` clause over the key is
    /// written about. ONE reading, so every service that authorizes a request signed with a
    /// credential this registry minted decides such a clause alike.
    #[must_use]
    pub fn multi_factor_auth_present(&self) -> Option<bool> {
        self.kind.map(|_| self.mfa_authenticated)
    }

    /// Every spelling of this caller a role's trust policy may name in its `Principal`
    /// element: its own ARN, the account's root user (which delegates to the whole
    /// account) and the bare account id AWS accepts in that root's place — plus, for an
    /// assumed-role session, the ARN of the ROLE it is a session of, because that is the
    /// principal a trust policy names to admit a chained assume.
    #[must_use]
    pub fn trust_policy_principals(&self) -> Vec<String> {
        let mut named = vec![
            self.identity.arn.clone(),
            account_root_arn(&self.identity.account),
            self.identity.account.clone(),
        ];
        if let Some(role_arn) = &self.role_arn {
            named.push(role_arn.clone());
        }
        named
    }
}

/// The caller a request's access key id names, with everything its credential carries.
///
/// `long_term` is the identity the key speaks for when it names no session this registry
/// minted — the IAM user that holds the access key ([`long_term_identity`]), or the
/// ambient identity when nothing holds it. Expiries the clock has made due are
/// materialised first, so an expired session is refused rather than answered.
pub fn resolve_request_caller(
    state: &mut StsState,
    access_key_id: &str,
    now: SystemTime,
    store: &CredentialStore,
    long_term: Identity,
) -> Result<Caller, ExpiredSession> {
    state.revoke_expired(now, store);
    match state.session(access_key_id) {
        Some(session) if session.is_expired(now) => Err(ExpiredSession {
            access_key_id: access_key_id.to_string(),
        }),
        Some(session) => Ok(Caller {
            identity: session.identity.clone(),
            kind: Some(session.kind),
            source_identity: session.source_identity.clone(),
            tags: session.tags.clone(),
            transitive_tag_keys: session.transitive_tag_keys.clone(),
            role_arn: session.role_arn.clone(),
            federated_claims: session.federated_claims.clone(),
            session_policies: session.policies.clone(),
            mfa_authenticated: session.mfa_authenticated,
        }),
        None => Ok(Caller::long_term(long_term)),
    }
}

/// This service's view of the IAM store in the SAME shared engine: the account's users,
/// their access keys and their roles.
///
/// The IAM service's OWN accessor is called rather than copied, so STS resolves a role
/// and an access key against the very records `iam:CreateRole` and `iam:CreateAccessKey`
/// wrote. `None` means the engine carries no IAM store — a router that mounts STS
/// without IAM, where the account can hold no role and no user at all.
pub type IamState = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// See [`IamState`].
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// Record, in the IAM store of the same engine, the role session the credential `minted`
/// was vended for, so a request signed with it is judged on that role's policies.
///
/// Which principal an access key id speaks for is a question about the account's
/// identities, and IAM answers it: its own authorizer resolves a long-term key through the
/// registry `iam:CreateAccessKey` wrote, and a TEMPORARY credential of a role session
/// through this record ([`IamState::record_role_session`]) — the permissions in force on
/// such a session are the role's ("the temporary security credentials … have the
/// permissions of the role", `AssumeRole`), capped by the session policies the mint passed.
/// The session itself stays this service's, which is why only the three facts an IAM
/// decision needs travel: the principal the session is, the role it is of, and the scope it
/// was minted with.
///
/// Nothing is recorded for a session that is not a role's — `GetSessionToken` and
/// `GetFederationToken` vend no `role_arn` — and nothing at all when the engine carries no
/// IAM store. Call it with the session registry's lock already GIVEN UP, so no handler
/// holds the IAM lock and the session lock at once.
pub fn publish_role_session(
    iam: Option<&::engine::ServiceState<IamState>>,
    minted: &MintedSession,
    now: SystemTime,
) {
    let (Some(iam), Some(role_arn)) = (iam, minted.session.role_arn.as_ref()) else {
        return;
    };
    iam.lock().record_role_session(
        &minted.session.access_key_id,
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::RoleSessionRecord {
            session_arn: minted.session.identity.arn.clone(),
            role_arn: role_arn.clone(),
            expires: ::aws_smithy_types::DateTime::from_secs(unix_seconds(minted.session.expires_at)),
            // The scope the mint passed travels with the session, because a session policy
            // caps what the role's own policies grant and the gate that reads those
            // policies is the one that has to read this too (see [`SessionPolicies`]).
            scope: minted.session.policies.clone(),
        },
        ::aws_smithy_types::DateTime::from_secs(unix_seconds(now)),
    );
}

/// The identity a LONG-TERM access key speaks for: the IAM user that holds it.
///
/// "Returns details about the IAM user or role whose credentials are used to call the
/// operation" (`GetCallerIdentity`, this service's model), so a request signed with an
/// IAM user's own access key speaks for THAT user — its `UserId` and its ARN as IAM
/// holds them, never a fixed one. A key IAM holds no record of is the ambient identity
/// ([`ambient_identity`]): the long-term key this emulator publishes at start.
///
/// The key's `AccessKey$Status` is deliberately NOT consulted. Whether a credential
/// verifies at all is the emulator's AUTHENTICATION layer's decision, taken from the
/// credential store before any handler runs — this service never checks a signature —
/// and that store, not IAM's record, is where a deactivated key would have to stop
/// verifying. Reading the status here would instead report an Inactive key as some OTHER
/// identity, which is the very substitution this function exists to remove.
pub fn long_term_identity(iam: Option<&IamState>, access_key_id: &str) -> Identity {
    let user = iam.and_then(|iam| {
        let record = iam.access_keys.get(access_key_id)?;
        iam.users.get(&record.user_key)
    });
    match user {
        Some(user) => Identity {
            user_id: user.user_id.clone(),
            account: ::engine::emulator_account_id().to_string(),
            arn: user.arn.clone(),
        },
        None => ambient_identity(),
    }
}

/// A role as IAM holds it, reduced to the facts an `AssumeRole` decision needs.
#[derive(Debug, Clone)]
pub struct ResolvedRole {
    /// `Role$RoleId`, exactly the value `iam:CreateRole`/`iam:GetRole` reported, which
    /// is what `AssumedRoleUser$AssumedRoleId` is built from.
    pub role_id: String,
    /// The role name in the case IAM holds it.
    pub role_name: String,
    /// `Role$Arn`, the resource a trust-policy decision is made about.
    pub arn: String,
    /// The account the role belongs to.
    pub account: String,
    /// `Role$AssumeRolePolicyDocument`: the trust policy, as IAM stored it.
    pub trust_policy: Option<String>,
    /// `Role$MaxSessionDuration`, defaulted to
    /// [`ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS`] when IAM holds none.
    pub max_session_duration: i32,
}

/// The role an `AssumeRole` request's `RoleArn` names, as IAM holds it.
///
/// `None` when the account holds no such role — which is also the answer for a role ARN
/// in ANOTHER account, because this emulator holds exactly one account's IAM records. The
/// stored role's own ARN must match the one the request named, so a role of the same name
/// under a different path is not resolved either.
pub fn resolve_role(iam: &IamState, role_arn: &str) -> Option<ResolvedRole> {
    let named = parse_role_arn(role_arn)?;
    let key = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key(
        &named.name,
    );
    let role = iam.roles.get(&key).filter(|role| role.arn == role_arn)?;
    Some(ResolvedRole {
        role_id: role.role_id.clone(),
        role_name: role.role_name.clone(),
        arn: role.arn.clone(),
        account: named.account,
        trust_policy: role.assume_role_policy_document.clone(),
        max_session_duration: role
            .max_session_duration
            .unwrap_or(ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS),
    })
}

impl ResolvedRole {
    /// Whether this role's trust policy lets `principals` perform `action` on it, given
    /// the condition keys `context` carries and the keys `published` says the request
    /// ANSWERS for (see [`assume_role_published_condition_keys`] and
    /// [`web_identity_published_condition_keys`] — a web identity's keys are named after
    /// the provider that issued its token, so they cannot be a constant).
    ///
    /// The DECISION is [`::service_common::iam_authorization`], the tree's one evaluation
    /// of IAM policy documents against a request, read as the RESOURCE-based policy it is
    /// — a trust policy is attached to the role, so its statements name the principals
    /// they grant to and a statement naming no resource covers the role itself. A role
    /// whose trust policy this cannot read as a policy, or which carries none at all,
    /// grants nothing: nothing is assumed on evidence the account does not hold.
    #[must_use]
    pub fn trust_policy_admits(
        &self,
        action: &str,
        principals: &[String],
        context: &[(String, String)],
        published: &[&str],
    ) -> bool {
        let Some(document) = self.trust_policy.as_deref() else {
            return false;
        };
        let request = ::service_common::iam_authorization::Request::new(action, &self.arn, principals)
            .with_context(context)
            .with_published_keys(published);
        ::service_common::iam_authorization::resource_decision(document, &request)
            == ::service_common::iam_authorization::Decision::Allowed
    }
}

/// How AWS words an authorization refusal: the principal, the action and the resource.
///
/// STS does not disclose whether the resource EXISTS, so a role the account does not
/// hold and a role whose trust policy withholds the grant are worded alike.
pub fn not_authorized_message(caller_arn: &str, action: &str, resource: &str) -> String {
    format!("User: {caller_arn} is not authorized to perform: {action} on resource: {resource}")
}

/// Whether a credential of `kind` may call the STS operation `operation`.
///
/// Each kind of temporary credential this service vends is documented with the STS
/// operations it may NOT call, and they differ, so the rule is the credential's class
/// rather than one blanket ban:
///
/// * an `AssumeRole` session "cannot call the Amazon Web Services STS
///   `GetFederationToken` or `GetSessionToken` API operations" (`AssumeRole`);
/// * a `GetSessionToken` session "cannot call any STS API except `AssumeRole` or
///   `GetCallerIdentity`" (`GetSessionToken`);
/// * a `GetFederationToken` session "cannot call any STS operations except
///   `GetCallerIdentity`" (`GetFederationToken`).
///
/// A long-term access key (`None`) may call every one of them: `GetSessionToken` and
/// `GetFederationToken` "must be called by using the long-term ... credentials of an IAM
/// user" (both operations' documentation, this service's model).
#[must_use]
pub fn caller_may_call(kind: Option<SessionKind>, operation: &str) -> bool {
    match kind {
        None => true,
        Some(SessionKind::AssumedRole) => {
            !matches!(operation, "GetFederationToken" | "GetSessionToken")
        }
        Some(SessionKind::SessionToken) => {
            matches!(operation, "AssumeRole" | "GetCallerIdentity")
        }
        Some(SessionKind::FederationToken) => operation == "GetCallerIdentity",
    }
}

/// The condition keys an `AssumeRole` mint MODELS — the keys whose absence from
/// [`assume_role_condition_context`] means the request carried no value for them, rather
/// than this emulator not knowing the key. Only these get IAM's empty-set reading under a
/// `ForAllValues:` / `ForAnyValue:` qualifier; a clause over any other key stays
/// undecided and withholds the grant.
pub const ASSUME_ROLE_PUBLISHED_CONDITION_KEYS: &[&str] = &[
    "sts:RoleSessionName",
    "sts:ExternalId",
    "sts:SourceIdentity",
    "sts:TransitiveTagKeys",
    "aws:PrincipalArn",
    "aws:PrincipalAccount",
    "aws:PrincipalType",
    "aws:userid",
    "aws:TagKeys",
];

/// The `aws:RequestTag/` family: every session tag THIS mint sets, which is exactly the
/// set the request named plus the ones the calling session carries forward. A mint knows
/// all of them, so the family is published and a clause over a tag key the mint does not
/// set is decided — as absent — rather than left undecided.
const REQUEST_TAG_CONDITION_KEY_FAMILY: &str = "aws:RequestTag/";

/// The `aws:PrincipalTag/` family: the session tags the CALLING principal carries.
const PRINCIPAL_TAG_CONDITION_KEY_FAMILY: &str = "aws:PrincipalTag/";

/// The condition keys an `AssumeRole` mint by `caller` publishes
/// ([`ASSUME_ROLE_PUBLISHED_CONDITION_KEYS`]), plus the tag key FAMILIES whose values are
/// known in full for this caller.
///
/// A family is published only by a caller that can answer for EVERY key in it, including
/// answering that it carries none ([`::service_common::iam_authorization`]). The tags one
/// mint sets are whatever this request determines, so `aws:RequestTag/` is always
/// answerable; the tags the CALLING principal carries are answerable only for a principal
/// whose tags this registry holds, which is a session it minted — an IAM user's own tags
/// live in IAM and are not read here, so a long-term caller publishes no principal-tag
/// family and a clause over one stays undecided rather than claiming the caller is
/// untagged.
///
/// Publishing them is what makes the multi-tenancy pattern decide: a clause that tests
/// whether a session carries a tag at all (`Null`), or that every value satisfies it
/// (`ForAllValues:`), or that a value differs from one (`StringNotEquals`), needs the
/// request to answer for an ABSENT key, which an unpublished key cannot.
#[must_use]
pub fn assume_role_published_condition_keys(caller: &Caller) -> Vec<&'static str> {
    let mut published = ASSUME_ROLE_PUBLISHED_CONDITION_KEYS.to_vec();
    published.push(REQUEST_TAG_CONDITION_KEY_FAMILY);
    if caller.kind.is_some() {
        published.push(PRINCIPAL_TAG_CONDITION_KEY_FAMILY);
    }
    published
}

/// The condition keys a credential's federated claims ANSWER for — each distinct key in
/// [`Session::federated_claims`], once.
///
/// A session minted by exchanging a web identity token stands for the identity that token
/// established, and the facts of that identity travel with the session: a request the
/// credential signs is decided against the very `<provider>:<claim>` keys the exchange was
/// decided under, which is what lets a policy in force on the role scope the session to its
/// own federated subject. Publishing them is what makes a clause over one of them DECIDE a
/// request rather than stay undecided, and what lets a `${<provider>:<claim>}` policy
/// variable be substituted at all. A credential no web-identity exchange minted answers for
/// none of them, so a clause over one stays undecided and withholds the grant.
#[must_use]
pub fn federated_claim_keys(claims: &[(String, String)]) -> Vec<&str> {
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in claims {
        if !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    keys
}

/// The condition keys STS publishes about one role-session mint, for a trust policy's
/// `Condition` to be decided against.
///
/// Each is a key AWS documents for an `sts:AssumeRole` request: the `aws:` keys that
/// describe the CALLING principal, the `sts:` keys the operation's own members supply,
/// the `aws:RequestTag/<key>` of each session tag the mint sets, and the multi-valued
/// `aws:TagKeys` / `sts:TransitiveTagKeys` — passed as one pair per value, which is how
/// [`::service_common::iam_authorization`] reads a multi-valued key. Nothing is invented:
/// a key the request does not determine is left out, so a clause over it stays undecided
/// and withholds the grant.
pub fn assume_role_condition_context(
    caller: &Caller,
    role_session_name: &str,
    external_id: Option<&str>,
    source_identity: Option<&str>,
    tags: &[(String, String)],
    transitive_tag_keys: &[String],
) -> Vec<(String, String)> {
    let mut context = vec![
        (
            "sts:RoleSessionName".to_string(),
            role_session_name.to_string(),
        ),
        // For a role session the principal AWS names here is the ROLE, not the session:
        // the session's own spelling is `aws:userid`.
        (
            "aws:PrincipalArn".to_string(),
            caller
                .role_arn
                .clone()
                .unwrap_or_else(|| caller.identity.arn.clone()),
        ),
        (
            "aws:PrincipalAccount".to_string(),
            caller.identity.account.clone(),
        ),
        (
            "aws:PrincipalType".to_string(),
            caller_principal_type(caller.kind).to_string(),
        ),
        ("aws:userid".to_string(), caller.identity.user_id.clone()),
    ];
    if let Some(external_id) = external_id {
        context.push((
            EXTERNAL_ID_CONDITION_KEY.to_string(),
            external_id.to_string(),
        ));
    }
    if let Some(source_identity) = source_identity {
        context.push((
            "sts:SourceIdentity".to_string(),
            source_identity.to_string(),
        ));
    }
    // The tags the CALLING principal already carries, which is what a target role's trust
    // policy tests when it gates a chained assume on the session it is being made from.
    for (key, value) in &caller.tags {
        context.push((format!("aws:PrincipalTag/{key}"), value.clone()));
    }
    for (key, value) in tags {
        context.push((format!("aws:RequestTag/{key}"), value.clone()));
        context.push(("aws:TagKeys".to_string(), key.clone()));
    }
    for key in transitive_tag_keys {
        context.push(("sts:TransitiveTagKeys".to_string(), key.clone()));
    }
    context
}

/// How `aws:PrincipalType` names the calling principal's kind.
fn caller_principal_type(kind: Option<SessionKind>) -> &'static str {
    match kind {
        Some(SessionKind::AssumedRole) => "AssumedRole",
        Some(SessionKind::FederationToken) => "FederatedUser",
        Some(SessionKind::SessionToken) | None => "User",
    }
}

/// The registry of web identity tokens THIS emulator issued, in the same shared engine:
/// one store, written by the service that issues such a token and read here, by the
/// service that redeems it (`AssumeRoleWithWebIdentity`).
///
/// `None` means the engine carries no such registry, which is an engine on which no token
/// was ever issued — so no token can be redeemed against it either.
pub fn issued_web_identity_tokens<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::web_identity_tokens::IssuedWebIdentityTokens>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::web_identity_tokens::IssuedWebIdentityTokens>)
}

/// Every spelling of a web identity's ISSUER that a role's trust policy may name in its
/// `Principal` element.
///
/// A trust policy admits a web identity by naming its issuer as a FEDERATED principal, and
/// AWS accepts two spellings of one issuer: the service principal or host name an identity
/// provider is known by (`cognito-identity.amazonaws.com`, `accounts.google.com`) and the
/// ARN of the IAM OpenID Connect provider the account registered for it
/// (`arn:aws:iam::<account>:oidc-provider/<host>`). The `iss` claim itself carries the
/// issuer as a URL, so the scheme is dropped to get the name the other two are built from
/// — the form a policy and a condition key both spell it in.
#[must_use]
pub fn web_identity_provider_name(issuer: &str) -> &str {
    issuer
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
}

/// The identity provider every Amazon Cognito identity pool's tokens are issued by, as a
/// trust policy and a condition key name it: "The trust policies of these roles must accept
/// the cognito-identity.amazonaws.com service principal"
/// (`AssumeRoleWithWebIdentity$RoleArn`, this service's model).
///
/// It is the one issuer an account can federate without registering an OpenID Connect
/// provider for it, because the pools it issues for are the account's own.
pub const COGNITO_IDENTITY_PROVIDER_NAME: &str = "cognito-identity.amazonaws.com";

// The condition keys an identity provider publishes about ONE of its tokens, and the
// principals a trust-policy decision about it is made for, live in
// `web_identity::condition_context` / `web_identity::principals`, beside the verification
// that reads the token: a key is named after the provider that issued the token and carries
// a CLAIM of it, so publishing them is part of reading the token rather than a rule of its
// own.

/// The modelled character class a session tag's key and value must satisfy
/// (`tagKeyType` / `tagValueType`, this service's model): letters, separators, numbers
/// and `_.:/=+-@`. The two shapes declare the same class, differing only in whether the
/// empty string is admitted, which the length bounds below state.
const TAG_TEXT_PATTERN: &str = r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";

/// The most session tags one session may hold: "You can pass up to 50 session tags"
/// (`AssumeRoleRequest$Tags`), which is also the model's own `tagListType` and
/// `tagKeyListType` length maximum.
///
/// The boundary walk bounds only the list a REQUEST carries. A chained mint also carries
/// the calling session's transitive tags forward, so the EFFECTIVE set is bounded by this
/// same maximum before a session is recorded — otherwise one stored session record would
/// grow by a request's worth of tags at every step of a role chain.
pub const MAX_SESSION_TAGS: usize = 50;

/// The longest session tag KEY the model admits (`tagKeyType`, length 1..128), counted
/// the way the operation's own documentation counts it: "The plaintext session tag keys
/// can't exceed 128 characters" (`AssumeRoleRequest$Tags`).
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest session tag VALUE the model admits (`tagValueType`, length 0..256): "the
/// values can't exceed 256 characters" (`AssumeRoleRequest$Tags`).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The compiled [`TAG_TEXT_PATTERN`], compiled once for the process.
fn tag_text_regex() -> &'static ::regex::Regex {
    static COMPILED: ::std::sync::OnceLock<::regex::Regex> = ::std::sync::OnceLock::new();
    COMPILED.get_or_init(|| {
        ::regex::Regex::new(TAG_TEXT_PATTERN).expect("the modelled tag pattern compiles")
    })
}

/// The first session tag whose key or value falls outside what the model admits, worded
/// the way AWS words a constraint violation: the member path, the value it rejected and
/// the requirement it broke.
///
/// The model constrains a session tag's key and value (`tagKeyType` length 1..128 and
/// `tagValueType` length 0..256, both over one character class), but those constraints sit
/// on members NESTED inside `tagListType`, which the boundary walk does not descend into
/// — so they are checked here, before anything is minted. Lengths are counted in
/// CHARACTERS, as the operation's documentation states the same limits.
pub fn invalid_session_tag(tags: Option<&[Tag]>) -> Option<String> {
    let violation = |position: usize, member: &str, value: &str, requirement: String| {
        Some(format!(
            "1 validation error detected: Value '{value}' at 'tags.{position}.member.{member}' \
             failed to satisfy constraint: {requirement}"
        ))
    };
    for (index, tag) in tags.unwrap_or(&[]).iter().enumerate() {
        let position = index + 1;
        let key_length = tag.key.chars().count();
        if key_length == 0 || key_length > MAX_TAG_KEY_CHARS {
            return violation(
                position,
                "key",
                &tag.key,
                format!(
                    "Member must have length less than or equal to {MAX_TAG_KEY_CHARS} and \
                     greater than or equal to 1"
                ),
            );
        }
        if tag.value.chars().count() > MAX_TAG_VALUE_CHARS {
            return violation(
                position,
                "value",
                &tag.value,
                format!("Member must have length less than or equal to {MAX_TAG_VALUE_CHARS}"),
            );
        }
        for (member, value) in [("key", &tag.key), ("value", &tag.value)] {
            if !tag_text_regex().is_match(value) {
                return violation(
                    position,
                    member,
                    value,
                    format!(
                        "Member must satisfy regular expression pattern: {}",
                        ::service_common::validation::unanchored(TAG_TEXT_PATTERN)
                    ),
                );
            }
        }
    }
    None
}

/// How AWS words the refusal of an STS operation a temporary credential may not call:
/// the operation, and the fact that the request presented session credentials.
#[must_use]
pub fn session_credentials_message(operation: &str) -> String {
    format!("Cannot call {operation} with session credentials")
}

/// The role id STS reports for a role ARN: a pure function of the ARN, so the same
/// role keeps one id across every session and two roles never share one — without
/// anything being remembered per request.
pub fn role_id(role_arn: &str) -> String {
    unique_id("AROA", role_arn)
}

/// The identity of the ambient caller: the IAM user whose long-term key this
/// emulator publishes. Every account and ARN comes from the emulator's own account
/// contract, and the user id is derived from the ARN, so it is the same on every
/// call and across restarts.
///
/// The ARN and the id are the ones
/// [`::service_common::caller_principal`] states for that user, which is also the record
/// IAM holds for it — so `GetCallerIdentity` and `iam:GetUser` report one principal.
pub fn ambient_identity() -> Identity {
    Identity {
        user_id: ::service_common::caller_principal::ambient_user_id(),
        account: ::engine::emulator_account_id().to_string(),
        arn: ::service_common::caller_principal::ambient_user_arn(),
    }
}

/// An AWS unique id: `prefix` plus 17 characters derived from `seed`.
///
/// The derivation is [`::service_common::caller_principal::aws_unique_id`], the tree's one
/// mapping of a seed onto an AWS unique id's alphabet, so the id a role gets here and the
/// id the ambient user carries in IAM are built by one rule: the same seed always yields
/// the same id, two different ones practically never collide, and nothing about the id
/// depends on when it was asked for.
pub fn unique_id(prefix: &str, seed: &str) -> String {
    ::service_common::caller_principal::aws_unique_id(prefix, seed)
}

/// An ARN in STS's own namespace. STS ARNs are global, so the region segment is
/// empty, and the account is the one the session belongs to: the account of the role
/// for an assumed role, the caller's for a federated user.
///
/// `engine::emulator_arn` fixes the account segment to the emulator's own account,
/// which is right for a resource the emulator owns; the account here is instead
/// whichever account the request named, exactly as AWS reports it.
pub fn sts_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:sts::{account}:{resource}")
}

/// The account that owns a role, and the role's name, from a role ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRef {
    pub account: String,
    pub name: String,
}

/// Split an IAM role ARN into the account that owns the role and the role's name.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; a role's
/// resource is `role/<path…>/<name>`, where the path is optional and only the last
/// segment is the role's name — which is why the assumed-role ARN STS reports
/// carries the name without the path. `None` when the value is not an IAM role ARN
/// of an account.
pub fn parse_role_arn(arn: &str) -> Option<RoleRef> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if partition.is_empty() || !is_account_id(account) {
        return None;
    }
    let name = resource.strip_prefix("role/")?.split('/').next_back()?;
    if name.is_empty() {
        return None;
    }
    Some(RoleRef {
        account: account.to_string(),
        name: name.to_string(),
    })
}

/// An AWS account id is 12 digits.
pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// How many characters an access key id has: "an access key ID (for example,
/// AKIAIOSFODNN7EXAMPLE)" (`GetAccessKeyInfo`, this service's model) — twenty, which is
/// also the length of every key id the emulator's own credential store mints.
///
/// The MODEL's `accessKeyIdType` is wider (`@length` 16..128, `@pattern ^[\w]*$`), because
/// that type bounds what may be SENT; a value inside those bounds that is not an access
/// key id at all is refused by this service rather than looked up
/// ([`is_access_key_id`]).
pub const ACCESS_KEY_ID_CHARS: usize = 20;

/// How many characters of an access key id name the KIND of credential it is: four —
/// "access key IDs beginning with AKIA are long-term credentials for an IAM user or the
/// Amazon Web Services account root user. Access key IDs beginning with ASIA are temporary
/// credentials that are created using STS operations" (`GetAccessKeyInfo`, this service's
/// model).
pub const ACCESS_KEY_ID_PREFIX_CHARS: usize = 4;

/// The four-character prefixes an ACCESS KEY id carries: the two this operation's own
/// documentation names — `AKIA` for a long-term key and `ASIA` for a temporary one — which
/// are the only two kinds of credential an access key id names. Every other AWS unique-id
/// prefix belongs to something that is not an access key (a user, a role, a group), so a
/// value carrying one names no access key.
pub const ACCESS_KEY_ID_PREFIXES: &[&str] = &["AKIA", "ASIA"];

/// The prefix the emulator's own credential store mints its key ids under.
///
/// Those ids are the long-term key this emulator publishes and every session credential it
/// vends — real credentials of this account, each resolvable to the identity it speaks for
/// — so they are access key ids here whatever body they carry, which an AWS-issued id
/// writes in one fixed alphabet ([`is_access_key_id`]).
pub const EMULATOR_ACCESS_KEY_ID_PREFIX: &str = "EMULATOR";

/// The alphabet the body of an AWS-issued access key id is written in: RFC 4648 base32
/// (`A`–`Z` and `2`–`7`), the encoding AWS carries an id's owning account in — so the
/// digits `0`, `1`, `8` and `9` never appear in one.
const ACCESS_KEY_ID_BODY_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Whether `value` is an access key id, and so names a key whose owning account is a
/// question with an answer.
///
/// One is [`ACCESS_KEY_ID_CHARS`] characters of upper-case letters and digits, made of a
/// four-character prefix naming the kind of credential ([`ACCESS_KEY_ID_PREFIXES`]) and a
/// body in that base32 alphabet — or a key id this emulator itself issued
/// ([`EMULATOR_ACCESS_KEY_ID_PREFIX`]). WHICH account an admitted id belongs to is a
/// separate question, answered from this service's own session registry rather than from
/// the id's bytes.
///
/// It is narrower than the modelled type, which admits lower case, underscores and any
/// length from 16 to 128: a value the model admits but this rule rejects names no access
/// key AWS could have issued, so there is no account to report for it.
pub fn is_access_key_id(value: &str) -> bool {
    if value.len() != ACCESS_KEY_ID_CHARS
        || !value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return false;
    }
    if value.starts_with(EMULATOR_ACCESS_KEY_ID_PREFIX) {
        return true;
    }
    let (prefix, body) = value.split_at(ACCESS_KEY_ID_PREFIX_CHARS);
    ACCESS_KEY_ID_PREFIXES.contains(&prefix)
        && body
            .bytes()
            .all(|b| ACCESS_KEY_ID_BODY_ALPHABET.contains(&b))
}

/// How AWS words the refusal of a value that is not an access key id: fixed text, with
/// nothing of the request in it — the rejected value is never echoed.
pub const INVALID_ACCESS_KEY_ID_MESSAGE: &str = "Access key ID is not valid.";

/// An MFA device the account holds, reduced to what an MFA challenge is decided by.
///
/// Read from the IAM store in the same engine ([`held_mfa_device`]), so a challenge is
/// decided against the devices the account actually holds rather than against anything
/// the request asserts.
#[derive(Debug, Clone)]
pub struct HeldMfaDevice {
    /// The serial number the device is addressed by — the value a request's
    /// `SerialNumber` names it as.
    pub serial_number: String,
    /// [`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key`] of
    /// the IAM user the device is enabled for: the challenge is the CALLER's, so a device
    /// enabled for another user answers none of the caller's codes.
    pub user_key: String,
    /// What one-time pass codes this device answers, which is what a presented `TokenCode`
    /// is authenticated against.
    pub pass_codes: PassCodes,
}

impl HeldMfaDevice {
    /// Whether this device emits `token_code` at `now` — the proof a request presenting an
    /// MFA challenge has to carry.
    #[must_use]
    pub fn answers(&self, token_code: &str, now: SystemTime) -> bool {
        match &self.pass_codes {
            PassCodes::None => false,
            PassCodes::TimeBased(base32_string_seed) => {
                ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::virtual_mfa_device_answers(
                    base32_string_seed,
                    token_code,
                    now,
                )
            }
        }
    }
}

/// Whether an MFA device answers a one-time pass code at all, and what the codes it answers
/// are computed from.
///
/// `Debug` is written out rather than derived, so no formatter of a resolved device can print
/// the seed — the same treatment the IAM store's own device record gives it.
#[derive(Clone)]
pub enum PassCodes {
    /// A device that answers no one-time pass code: a FIDO security key authenticates by
    /// challenge and response, so no six-digit code presented against one is a code it emits.
    None,
    /// A virtual MFA device, which emits the time-based pass codes of the seed it was
    /// provisioned with ("the base32 seed", `VirtualMFADevice$Base32StringSeed`).
    TimeBased(String),
}

impl ::std::fmt::Debug for PassCodes {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::TimeBased(_) => f.write_str("TimeBased(..)"),
        }
    }
}

/// The MFA device the account holds under `serial_number`, or `None` when it holds none.
///
/// A READ of the IAM store, which holds the two kinds of device a serial number can name: a
/// FIDO security key of its own device registry, and a VIRTUAL MFA device `iam:EnableMFADevice`
/// enabled for a user — addressed, as the model says, by the device's ARN
/// (`VirtualMFADevice$SerialNumber`). A virtual device no user holds is enabled for nobody, so
/// it answers no caller's challenge.
pub fn held_mfa_device(iam: &IamState, serial_number: &str) -> Option<HeldMfaDevice> {
    if let Some(device) = iam.fido_security_key(serial_number) {
        return Some(HeldMfaDevice {
            serial_number: device.serial_number.clone(),
            user_key: device.user_key.clone(),
            pass_codes: PassCodes::None,
        });
    }
    let device = iam.virtual_mfa_device(serial_number)?;
    Some(HeldMfaDevice {
        serial_number: serial_number.to_string(),
        user_key: device.user_key.clone()?,
        pass_codes: PassCodes::TimeBased(device.base32_string_seed.clone()),
    })
}

/// The key the IAM store holds the user named by `arn` under, or `None` when `arn` is not
/// an IAM user's ARN — which an assumed-role or federated-user ARN is not, and no MFA
/// device is enabled for one.
pub fn iam_user_key(arn: &str) -> Option<String> {
    let named = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::parse_entity_arn(
        arn,
    )?;
    matches!(
        named.kind,
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::EntityKind::User
    )
    .then(|| {
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key(&named.name)
    })
}

/// Why an MFA challenge failed, and how AWS words each case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaChallengeFailure {
    /// The request named no device the code could be validated against: no serial number,
    /// or a serial number naming no MFA device enabled for the calling identity.
    SerialNotAssociated,
    /// A device of the calling identity answers that serial number, but not the code the
    /// request presented.
    InvalidPassCode,
}

impl MfaChallengeFailure {
    /// The sentence AWS refuses this case with — fixed text, carrying neither the serial
    /// number nor the code the request presented.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::SerialNotAssociated => {
                "MultiFactorAuthentication failed, unable to validate MFA code.  Please verify \
                 your MFA serial number is valid and associated with this user."
            }
            Self::InvalidPassCode => {
                "MultiFactorAuthentication failed with invalid MFA one time pass code."
            }
        }
    }
}

/// Decide the MFA challenge a request presents, against the device the account holds.
///
/// "MFA-enabled IAM users must call `GetSessionToken` and submit an MFA code that is
/// associated with their MFA device … an incorrect MFA code causes the API to return an
/// access denied error" (`GetSessionToken`, this service's model), and "if the role being
/// assumed requires MFA and if the `TokenCode` value is missing or expired, the
/// `AssumeRole` call returns an 'access denied' error"
/// (`AssumeRoleRequest$TokenCode`) — so a request that presents an MFA challenge is
/// authenticated against the device enabled for the CALLING identity, and `Some(..)` is
/// the refusal the caller gets instead of a credential.
///
/// `None` when the request presents no challenge at all (neither member), which is every
/// request of an identity no policy requires MFA of, and when it presents one the device
/// ANSWERS — the case a session is MFA-authenticated by. `device` is the record
/// [`held_mfa_device`] read for the serial the request named, and `now` the instant the
/// presented code is checked at.
///
/// Which codes a device answers is the device's own property ([`HeldMfaDevice::answers`]): a
/// virtual MFA device emits the time-based pass codes of the seed it was provisioned with,
/// while a FIDO security key authenticates by challenge and response and emits none, so no
/// code presented against one is a code it would answer.
pub fn mfa_challenge_failure(
    device: Option<&HeldMfaDevice>,
    identity: &Identity,
    serial_number: Option<&str>,
    token_code: Option<&str>,
    now: SystemTime,
) -> Option<MfaChallengeFailure> {
    let (serial_number, token_code) = match (serial_number, token_code) {
        // No challenge: nothing to validate, and nothing MFA-gated is claimed either.
        (None, None) => return None,
        (Some(serial_number), Some(presented)) => (serial_number, presented),
        // Half a challenge cannot be validated: a code with no device named to validate it
        // against, or a device named with no code to validate.
        _ => return Some(MfaChallengeFailure::SerialNotAssociated),
    };
    let enabled = device.filter(|device| {
        device.serial_number == serial_number
            && iam_user_key(&identity.arn).is_some_and(|user| user == device.user_key)
    });
    match enabled {
        // The account holds no device of this identity under that serial, which is what
        // the refusal tells the caller to check.
        None => Some(MfaChallengeFailure::SerialNotAssociated),
        // The serial IS this identity's, so what is validated is the CODE: the challenge is
        // satisfied exactly when the device emits the one presented.
        Some(device) => (!device.answers(token_code, now)).then_some(MfaChallengeFailure::InvalidPassCode),
    }
}

/// The PACKED SESSION TOKEN one mint's session policies and session tags serialize into:
/// the artifact whose length the token budget applies to.
///
/// "An Amazon Web Services conversion compresses the session policy document, session
/// policy ARNs, and session tags into a packed binary format that has a separate limit"
/// (`PackedPolicyTooLargeException`, this service's model), so the packed state is exactly
/// those three — the inline policy document, the managed policy ARNs the request names, and
/// each session tag's key and value — serialized as the members that carry them, and then
/// encoded the way a token is carried, which is the form whose length is measured.
///
/// Measuring the TOKEN rather than adding up the field contents is what makes the figure
/// track the request: a serialization carries its own framing per element, so ten tags and
/// one tag of the same total content do not pack into the same token, and a byte added to a
/// tag value moves the measured size. The emulator compresses nothing, so no caller's
/// session state is ever admitted because it happened to compress well.
///
/// A request that carries no policy, no policy ARN and no session tag packs NO token at all
/// — there is no session state to serialize — so its size is zero rather than the length of
/// an empty document.
#[must_use]
pub fn packed_session_token(policy: Option<&str>, policy_arns: &[String], tags: &[Tag]) -> String {
    let mut packed = serde_json::Map::new();
    if let Some(policy) = policy {
        packed.insert(
            "Policy".to_string(),
            serde_json::Value::String(policy.to_string()),
        );
    }
    if !policy_arns.is_empty() {
        packed.insert(
            "PolicyArns".to_string(),
            serde_json::Value::Array(
                policy_arns
                    .iter()
                    .map(|arn| serde_json::json!({ "arn": arn }))
                    .collect(),
            ),
        );
    }
    if !tags.is_empty() {
        packed.insert(
            "Tags".to_string(),
            serde_json::Value::Array(
                tags.iter()
                    .map(|tag| serde_json::json!({ "Key": tag.key, "Value": tag.value }))
                    .collect(),
            ),
        );
    }
    if packed.is_empty() {
        return String::new();
    }
    let serialized = serde_json::Value::Object(packed).to_string();
    ::base64::Engine::encode(&PACKED_TOKEN_CODEC, serialized)
}

/// The coder a packed session token is carried in: base64 as RFC 4648 §4 defines it,
/// padded, which is the one coding every token this service hands a client travels in.
const PACKED_TOKEN_CODEC: ::base64::engine::GeneralPurpose =
    ::base64::engine::general_purpose::STANDARD;

/// The SIZE, in bytes, of the packed session token one mint's session policies and session
/// tags pack into ([`packed_session_token`]) — the figure the refusal reports and the figure
/// [`packed_policy_size`] is the percentage of.
#[must_use]
pub fn packed_policy_bytes(policy: Option<&str>, policy_arns: &[String], tags: &[Tag]) -> usize {
    packed_session_token(policy, policy_arns, tags).len()
}

/// The percentage of [`SESSION_TOKEN_SIZE_LIMIT`] a request's session policies and
/// session tags take up — "a percentage value that indicates the packed size of the
/// session policies and session tags combined passed in the request" (`PackedPolicySize`,
/// this service's model).
///
/// It is a measurement of the mint, so EVERY mint has one: a request that carries no
/// session policy and no session tag packs nothing, which is zero percent of the budget
/// and inside the member's modelled `0..` range — not an absent measurement. Every
/// operation whose response models the member therefore reports it, so two mints AWS
/// answers alike cannot differ in the shape of their response.
///
/// The percentage is rounded up, so anything packed at all reports at least one percent,
/// and it passes 100 exactly when the packed size passes the byte limit — the one
/// condition `PackedPolicyTooLargeException` is answered for.
pub fn packed_policy_size(policy: Option<&str>, policy_arns: &[String], tags: &[Tag]) -> i32 {
    let packed = packed_policy_bytes(policy, policy_arns, tags);
    let percent = packed.saturating_mul(100).div_ceil(SESSION_TOKEN_SIZE_LIMIT);
    i32::try_from(percent).unwrap_or(i32::MAX)
}

/// The most characters an inline session policy's PLAINTEXT may carry: "The plaintext
/// that you use for both inline and managed session policies can't exceed 2,048
/// characters" (`AssumeRoleRequest$Policy`, this service's model).
///
/// The model states that maximum as the `@length` of `sessionPolicyDocumentType`, the
/// shape every OTHER operation's `Policy` member targets — so for those the framework
/// refuses an over-long document before a handler runs. `AssumeRole`'s member targets
/// `unrestrictedSessionPolicyDocumentType`, which carries the same documented limit and
/// no `@length` maximum, so the same constraint is checked by the handler instead
/// ([`session_policy_over_plaintext_limit`]).
pub const MAX_SESSION_POLICY_PLAINTEXT_CHARS: usize = 2048;

/// How a request whose inline session policy passes
/// [`MAX_SESSION_POLICY_PLAINTEXT_CHARS`] is refused, and `None` for a document within
/// it.
///
/// The limit is on the policy's PLAINTEXT, so it is measured in characters of the document
/// as submitted, and the refusal is stated as the constraint class it is — the framework's
/// own clause for a `@length` maximum, at `member_path`, quoting the document — rather than
/// as a sentence of this service's own: a caller that reads the message to find the
/// offending input sees the same wording whichever operation refused it, and the plaintext
/// limit is answered BEFORE the request's session state is packed, so "your policy text is
/// too long" and "your session state does not fit" stay two distinct refusals.
///
/// The quoting goes through [`::service_common::validation::quoted_echo`], the ceiling
/// every clause this service states carries: the member this check exists for declares no
/// `@length` maximum of its own, so without it one message would be sized by the request.
/// The CHECK is made against the document as sent, never against that rendering.
pub fn session_policy_over_plaintext_limit(member_path: &str, policy: &str) -> Option<String> {
    if policy.chars().count() <= MAX_SESSION_POLICY_PLAINTEXT_CHARS {
        return None;
    }
    let violation = ::aws_smithy_schema_server::Violation {
        path: member_path.to_string(),
        kind: ::aws_smithy_schema_server::ViolationKind::MaxLength(
            i64::try_from(MAX_SESSION_POLICY_PLAINTEXT_CHARS).unwrap_or(i64::MAX),
        ),
        value: Some(
            ::service_common::validation::quoted_echo(policy).into_owned(),
        ),
        sensitive: false,
    };
    ::service_common::validation_message::aws_validation_refusal(
        &::smithy_server_runtime::Refusal::Invalid(vec![violation]),
    )
    .map(|stated| stated.message)
}

/// How AWS words `PackedPolicyTooLarge`: the size the request's session policies and
/// session tags were measured at, and the limit that size passed.
///
/// The message names the constraint the request violated rather than a percentage,
/// because the caller trimming a session policy or a session tag to fit needs both
/// figures to know how much to remove. Both come from this mint — `packed` is measured
/// over the request ([`packed_policy_bytes`]) and the limit is the service's own
/// ([`SESSION_TOKEN_SIZE_LIMIT`]).
pub fn packed_policy_too_large_message(packed: usize) -> String {
    format!(
        "Serialized token too large for session. Token size of {packed} bytes exceeds the \
         maximum allowed limit of {SESSION_TOKEN_SIZE_LIMIT} bytes."
    )
}

/// How STS words a request that names more managed session policies than one mint may
/// carry ([`MAX_MANAGED_SESSION_POLICIES`]).
///
/// The cap is the SERVICE's own rule — `policyDescriptorListType` carries no `@length`,
/// so the framework's member-constraint renderer has nothing to state here — and STS
/// states it as its own sentence: fixed text naming the parameter and the limit, and
/// nothing of the request, so a refusal never echoes the ARNs the caller sent. One
/// wording for every operation that carries `PolicyArns`, so two of them cannot word the
/// same refusal differently.
pub fn policy_arns_over_limit_message() -> String {
    format!(
        "The limit of ARNs for the PolicyArns parameter was exceeded. \
         {MAX_MANAGED_SESSION_POLICIES} ARNs can be requested at most in an API request."
    )
}

/// How AWS words an attempt to replace the source identity a role session already
/// carries.
///
/// "The source identity value persists across chained role sessions"
/// (`AssumeRoleRequest$SourceIdentity`, this service's model), so a chained mint made
/// from a session that already carries one cannot name a different value: the value is
/// already set on the session this mint continues. Fixed text — it names no request
/// value, because the conflict is about the session rather than about any one of the two
/// values.
pub const SOURCE_IDENTITY_ALREADY_SET_MESSAGE: &str =
    "The source identity is already set for this assume role session";

/// The ARNs of the managed session policies a request named.
pub fn policy_arns(policy_arns: Option<&[crate::ops::types::PolicyDescriptorType]>) -> Vec<String> {
    policy_arns
        .unwrap_or(&[])
        .iter()
        .filter_map(|descriptor| descriptor.arn.clone())
        .collect()
}

/// The session tags a request named, as key/value pairs.
pub fn tag_pairs(tags: Option<&[Tag]>) -> Vec<(String, String)> {
    tags.unwrap_or(&[])
        .iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// The first session tag key that repeats an earlier one, ignoring case. AWS refuses
/// a request that passes the same session tag key twice.
pub fn duplicate_tag_key(tags: Option<&[Tag]>) -> Option<&str> {
    let tags = tags.unwrap_or(&[]);
    tags.iter().enumerate().find_map(|(index, tag)| {
        tags[..index]
            .iter()
            .any(|earlier| earlier.key.eq_ignore_ascii_case(&tag.key))
            .then_some(tag.key.as_str())
    })
}

/// The transitive session tag key the calling session carries forward that the request
/// passes AGAIN — `None` when it passes none of them.
///
/// A key the calling session carries forward is already set on the session this mint
/// continues, so a request may not set it a second time: "if you used temporary
/// credentials to perform this operation, the new session inherits any transitive session
/// tags from the calling session. If you pass a session tag with the same key as an
/// inherited tag, the operation fails" (`AssumeRoleRequest$Tags`, this service's model).
/// Keys are compared without case, as a repeat within one request is
/// ([`duplicate_tag_key`]).
pub fn inherited_tag_key_collision<'a>(
    caller: &'a Caller,
    requested: &[(String, String)],
) -> Option<&'a str> {
    caller
        .tags
        .iter()
        .map(|(key, _)| key.as_str())
        .find(|key| {
            caller
                .transitive_tag_keys
                .iter()
                .any(|transitive| transitive.eq_ignore_ascii_case(key))
                && requested
                    .iter()
                    .any(|(named, _)| named.eq_ignore_ascii_case(key))
        })
}

/// The transitive session tags a chained mint carries FORWARD from the calling session:
/// "If you set a tag key as transitive, the corresponding key and value passes to
/// subsequent sessions in a role chain" (`AssumeRoleRequest$TransitiveTagKeys`, this
/// service's model).
///
/// A tag the new request names itself is not inherited — a request that names one is
/// refused before this is reached ([`inherited_tag_key_collision`]) — and tag keys are
/// compared without case because they "are not case sensitive, but case is preserved"
/// (`GetFederationToken`, this service's model).
pub fn inherited_transitive_tags(
    caller: &Caller,
    requested: &[(String, String)],
) -> Vec<(String, String)> {
    caller
        .tags
        .iter()
        .filter(|(key, _)| {
            caller
                .transitive_tag_keys
                .iter()
                .any(|transitive| transitive.eq_ignore_ascii_case(key))
                && !requested
                    .iter()
                    .any(|(named, _)| named.eq_ignore_ascii_case(key))
        })
        .cloned()
        .collect()
}

/// The whole message `GetFederationToken` answers a malformed session policy with.
///
/// Fixed text: every part of it, so nothing of the submitted document reaches it. A
/// document that stops while a value, object, array or string is still open has no
/// offending character to point at — the parse ran out of input rather than meeting
/// something it could not accept — so there is no specific error to describe and this
/// one sentence is the whole refusal.
pub const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";

/// Why a session policy is not a JSON policy document, as the message
/// `MalformedPolicyDocumentException` carries — `None` when it is one.
///
/// The refusal's message "describes the specific error"
/// (`MalformedPolicyDocumentException`, this service's model) WHERE the document gives
/// one to point at, so it is built from the submitted document rather than restating the
/// member's type: a document whose parse stopped ON a character is reported with that
/// character and its character code, what the parser expected in its place, and the line
/// and column the failure sits at; a document that parses but is not an object is
/// reported with the JSON type it carries instead. A document that merely RAN OUT of
/// input names nothing to describe, and is refused with the fixed sentence
/// [`SYNTAX_ERRORS_IN_POLICY`].
///
/// One reading for every operation that takes an inline session policy: the wording
/// follows the document's defect, not the operation it was passed to.
pub fn policy_document_error(policy: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(policy) {
        Ok(value) if value.is_object() => None,
        Ok(value) => Some(format!(
            "The policy document must be a JSON object, not {}",
            json_type_name(&value)
        )),
        // `Category::Eof` is the parser's own classification of "ran out of input"
        // (`serde_json::error::Category`), so the two cases are told apart by the parse
        // that actually ran rather than by re-reading the document.
        Err(parse) if parse.classify() == serde_json::error::Category::Eof => {
            Some(SYNTAX_ERRORS_IN_POLICY.to_string())
        }
        Err(parse) => Some(policy_parse_message(policy, &parse)),
    }
}

/// The JSON type a value carries, named the way the refusal names it.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a JSON boolean",
        serde_json::Value::Number(_) => "a JSON number",
        serde_json::Value::String(_) => "a JSON string",
        serde_json::Value::Array(_) => "a JSON array",
        serde_json::Value::Object(_) => "a JSON object",
    }
}

/// The parse failure of a policy document the parse STOPPED ON, as the refusal reports
/// it: the offending character and its code, what was expected there, and the
/// position of the failure in the submitted document.
///
/// Both readings are the tree's shared one
/// ([`::service_common::json_parse_fault`]), so a document this service and another
/// refuse cannot be reported as failing in two different places. A document that ran out
/// of input does not reach this: it has nothing to point at, and
/// [`SYNTAX_ERRORS_IN_POLICY`] is its whole refusal.
fn policy_parse_message(document: &str, parse: &serde_json::Error) -> String {
    format!(
        "Unexpected IOException: {}{}",
        ::service_common::json_parse_fault::unexpected_input(document, parse),
        ::service_common::json_parse_fault::source_location(parse.line(), parse.column())
    )
}

/// Mint a session credential and record the session it belongs to.
///
/// The access key id, the secret access key and the session token are minted by the
/// emulator's credential store, which is the only thing that creates credential
/// material: the returned credential is therefore one a later signed request
/// verifies against, and the id it carries is the registry key
/// `GetCallerIdentity` and `GetAccessKeyInfo` resolve.
///
/// Room is made in the bounded registry BEFORE any credential material is created,
/// so a refused mint leaves nothing behind. Fails only when the registry is full of
/// live sessions, or if the store minted a credential without the secrets the kind it
/// is asked for always carries — the caller refuses the request rather than answering
/// with a credential that is missing half of itself.
pub fn mint_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    state: &mut StsState,
    now: SystemTime,
    request: NewSession,
) -> Result<MintedSession, MintError> {
    use emulator_server::aws_credentials::{AWS_SESSION, SECRET_ACCESS_KEY, SESSION_TOKEN};

    state.make_room(now, store)?;

    let minted = store.create(&AWS_SESSION);
    let secrets = minted
        .secret(SECRET_ACCESS_KEY)
        .zip(minted.secret(SESSION_TOKEN));
    let Some((secret_access_key, session_token)) = secrets else {
        let _ = store.delete(&minted.id);
        return Err(MintError::NoSecrets);
    };
    let (secret_access_key, session_token) =
        (secret_access_key.to_string(), session_token.to_string());
    let session = request.into_session(minted.id.clone(), now);

    let credentials = Credentials {
        access_key_id: minted.id,
        secret_access_key,
        session_token,
        expiration: ::aws_smithy_types::DateTime::from_secs(unix_seconds(session.expires_at)),
    };
    if let Err(full) = state.insert(session.clone()) {
        let _ = store.delete(&session.access_key_id);
        return Err(full);
    }
    // The account's audit view of the session, so a call made with this credential is
    // recorded as the session that made it (see [`remember_audited_session`]). Written
    // once, here, because this is the one path every session of this emulator is minted
    // through.
    remember_audited_session(engine, &session, now);
    Ok(MintedSession {
        credentials,
        session,
    })
}

/// Mint a session of a ROLE for a service other than this one, and record it everywhere a
/// request signed with it is resolved from.
///
/// A credential that is a session of a role is judged on the policies in force on THAT role
/// — "the temporary security credentials … have the permissions of the role"
/// (`AssumeRole`) — whichever operation vended it, so the record a service resolves a
/// temporary credential's principal through ([`publish_role_session`]) belongs to every
/// mint, not only to this service's own operations. Those operations publish it themselves
/// because each holds the registry lock across its own later checks; everything else mints
/// through here, so a session minted outside this service speaks for its role exactly as an
/// assumed one does.
///
/// The registry lock is taken for the mint alone and GIVEN UP before the record is
/// published, so no caller holds the IAM lock and the session lock at once.
pub fn mint_role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    registry: &::engine::ServiceState<StsState>,
    now: SystemTime,
    request: NewSession,
) -> Result<MintedSession, MintError> {
    let minted = {
        let mut held = registry.lock();
        mint_session(engine, store, &mut held, now, request)?
    };
    publish_role_session(iam_state(engine).as_ref(), &minted, now);
    Ok(minted)
}

/// The account's SESSION directory in the shared engine: the audit view of the sessions
/// this emulator minted, keyed by the access key id each was minted under.
///
/// A SHARED type ([`::service_common::management_events::SessionAuditDirectory`]) and not a
/// field of this service's own store, because the service that MINTS a session is not the
/// service that SERVES the calls made with it: a record of such a call names the session,
/// and the recording service cannot read this service's state. The sessions themselves stay
/// here — [`StsState::sessions`] is what resolves an identity — so the directory carries
/// only what a record names.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::SessionAuditDirectory,
            >,
        )
}

/// Remember `session` in the account's session directory, so every service that records a
/// call made with its credential can name the session it was made by.
///
/// Best-effort, as every other audit write in this tree: an engine carrying no directory (a
/// unit test over a bare engine) still mints the session, because failing a caller because
/// the audit view is unreachable would be a worse answer than serving it unrecorded.
pub fn remember_audited_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    session: &Session,
    now: SystemTime,
) {
    let Some(directory) = session_audit_directory(engine) else {
        return;
    };
    directory
        .lock()
        .remember(session.access_key_id.clone(), audited_session(session), now);
}

/// Forget a session the account's session directory holds — for a mint this service gave
/// up again, so the directory never names a credential no caller was handed.
pub fn forget_audited_session<Eng: Send + Sync + 'static>(engine: &Eng, access_key_id: &str) {
    if let Some(directory) = session_audit_directory(engine) {
        directory.lock().forget(access_key_id);
    }
}

/// The audit view of one session: who a call made with its credential speaks for, which
/// entity ISSUED the session, and the source identity the session carries.
///
/// Every value is the session's own — nothing is read off a request — so the record of a
/// call made with the credential names the same session this service resolves an identity
/// from.
pub fn audited_session(session: &Session) -> ::service_common::management_events::RecordedSession {
    use ::service_common::management_events::{
        RecordedSession, RecordedSessionIssuer, PRINCIPAL_TYPE_ASSUMED_ROLE,
        PRINCIPAL_TYPE_FEDERATED_USER, PRINCIPAL_TYPE_IAM_USER, SESSION_ISSUER_TYPE_ROLE,
    };

    // Which `userIdentity.type` a call made with the credential carries follows from the
    // KIND of session: a role session is an assumed role, a federation token speaks for a
    // federated user, and a session token speaks for the IAM user that asked for it.
    let principal_type = match session.kind {
        SessionKind::AssumedRole => PRINCIPAL_TYPE_ASSUMED_ROLE,
        SessionKind::FederationToken => PRINCIPAL_TYPE_FEDERATED_USER,
        SessionKind::SessionToken => PRINCIPAL_TYPE_IAM_USER,
    };
    // The issuer is the entity the session was minted OF, as CloudTrail's `sessionIssuer`
    // reports "how the credentials were obtained":
    //
    // * a role session is minted of its ROLE, named by the ARN this service stored when the
    //   role was assumed, with the role id IAM issued for it; and
    // * a session token is minted of the IAM USER that asked for it — which is the identity
    //   the session itself speaks for ("the credentials have the same permissions as the IAM
    //   user", `GetSessionToken`), so the session's own identity IS its issuer.
    //
    // A FEDERATION token is the one kind with neither: it speaks for the federated user the
    // request named, while its issuer is the IAM user that called for it — a principal this
    // registry's record of the session does not hold ([`Session`] carries the identity the
    // credential speaks for), so the record names no issuer rather than one derived from
    // the federated user it is not.
    let issuer = match (session.role_arn.as_deref(), session.kind) {
        (Some(role_arn), _) => Some(RecordedSessionIssuer {
            principal_type: SESSION_ISSUER_TYPE_ROLE.to_string(),
            // `AssumedRoleId` is `<RoleId>:<RoleSessionName>`, so the role's own id is the
            // segment before the colon — the id IAM issued for that role.
            principal_id: session
                .identity
                .user_id
                .split_once(':')
                .map_or_else(|| session.identity.user_id.clone(), |(id, _)| id.to_string()),
            arn: role_arn.to_string(),
            account_id: session.identity.account.clone(),
            user_name: entity_name_of(role_arn).to_string(),
        }),
        (None, SessionKind::SessionToken) => Some(RecordedSessionIssuer {
            principal_type: PRINCIPAL_TYPE_IAM_USER.to_string(),
            principal_id: session.identity.user_id.clone(),
            arn: session.identity.arn.clone(),
            account_id: session.identity.account.clone(),
            user_name: entity_name_of(&session.identity.arn).to_string(),
        }),
        (None, _) => None,
    };
    RecordedSession {
        principal_type: principal_type.to_string(),
        principal_id: session.identity.user_id.clone(),
        arn: session.identity.arn.clone(),
        account_id: session.identity.account.clone(),
        // The name the session speaks under, which CloudTrail reports as an event's
        // `Username`: the last segment of the session's own ARN — the role session name of a
        // role session, the federated user's name, the IAM user's name.
        user_name: Some(entity_name_of(&session.identity.arn).to_string()),
        issuer,
        source_identity: session.source_identity.clone(),
        created_at: session.created_at,
        expires_at: session.expires_at,
    }
}

/// The friendly name an ARN names its entity by: the last `/`-separated segment
/// (`arn:aws:iam::<account>:role/path/demo` -> `demo`), which is the whole resource for an
/// ARN that carries no path at all.
fn entity_name_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// Honour a request's `MinimumSessionTokenSize`, or say why it cannot be.
///
/// "STS increases the session token to at least this size, regardless of its actual
/// content" (`MinimumSessionTokenSize`, this service's model). The emulator's
/// session token is minted by the credential store and cannot be padded without
/// invalidating it, so a request asking for a token larger than the store mints is
/// refused — with the session it already minted dropped again — rather than answered
/// with a token smaller than it asked for.
pub fn honour_minimum_token_size<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &mut StsState,
    store: &CredentialStore,
    minted: &MintedSession,
    minimum: Option<i32>,
) -> Result<(), String> {
    let size = minted.credentials.session_token.len();
    let minimum = minimum.unwrap_or(0).max(0);
    if usize::try_from(minimum).unwrap_or(0) <= size {
        return Ok(());
    }
    state.discard(&minted.credentials.access_key_id, store);
    // The session was given up, so the account's audit view gives it up too: no caller was
    // handed this credential, so no record may name it.
    forget_audited_session(engine, &minted.credentials.access_key_id);
    Err(format!(
        "MinimumSessionTokenSize {minimum} is larger than the {size}-byte session token this \
         emulator mints, and a minted session token cannot be padded (the limit is \
         {SESSION_TOKEN_SIZE_LIMIT} bytes)"
    ))
}

/// The percentage of the session-token size limit a minted token takes up, and its
/// size in bytes: `SessionTokenUtilization` and `SessionTokenSize`, both measured on
/// the token actually returned.
pub fn session_token_metrics(credentials: &Credentials) -> (i32, i32) {
    let size = credentials.session_token.len();
    let utilization = size.saturating_mul(100) / SESSION_TOKEN_SIZE_LIMIT;
    (
        i32::try_from(utilization).unwrap_or(i32::MAX),
        i32::try_from(size).unwrap_or(i32::MAX),
    )
}

/// Seconds since the Unix epoch, for a timestamp on the wire. A time before the
/// epoch cannot arise here (every instant is `now` or later), and reads as the epoch
/// if it ever did.
pub fn unix_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
