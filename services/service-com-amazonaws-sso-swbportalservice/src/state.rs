// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sso-swbportalservice service.
//!
//! The IAM Identity Center portal has no create API of its own: a sign-in session is
//! minted elsewhere (`CreateToken` in the OIDC service) and the account/role
//! assignments are made by an administrator. So nothing a portal operation reports is
//! synthesized by a handler — it is RESOLVED, from the records of whoever made it:
//!
//! * the sign-in a presented access token names, and the accounts and roles assigned to
//!   it, come from the emulator's own records of those two events, read where they live
//!   ([`sign_in`], [`federated_sign_in`]);
//! * a sign-in and an assignment handed to this store directly
//!   ([`ComAmazonawsSsoSwbportalserviceState::put_session`] /
//!   [`ComAmazonawsSsoSwbportalserviceState::put_account`]) are reported the same way, so
//!   whatever stands an emulator up can seed one without an OIDC flow;
//! * what THIS store owns is what only the portal knows: that a `Logout` ended a sign-in,
//!   and the role credentials this portal minted.
//!
//! [`ComAmazonawsSsoSwbportalserviceState::signed_in`] is the one place those sources are
//! combined, so no handler decides for itself what a token names.
//!
//! This module also owns the rules every handler shares, so none is written twice:
//! wording the refusal when a token resolves to no sign-in ([`unauthorized_session`], and
//! [`unauthorized_response`] for the gate in [`crate::service`] that runs before any
//! handler), wording the refusal when this store cannot be reached at all
//! ([`store_unavailable`], and [`store_unavailable_response`] for those same seams),
//! stamping an expiry without risking an overflow panic ([`expiry_after`]),
//! ending the minted role credentials the clock has made spent
//! ([`end_expired_credentials`], reached through [`sweep_expired_credentials`] by the
//! seams in [`crate::service`] that answer a request without a handler),
//! cutting a sorted listing into a page with an opaque
//! cursor ([`page_of`]), and wording the refusal when the `nextToken` presented to such a
//! listing is not one it issued ([`invalid_cursor`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Resolving a presented access token against the records the REST of the emulator holds:
/// the sign-in its own `CreateToken` minted and the IAM Identity Center assignments an
/// administrator made. Re-exported, so `crate::state::federated_sign_in` and
/// `crate::state::SignedIn` stay the paths handlers and tests use.
mod sign_in;
pub use sign_in::*;

/// One IAM Identity Center sign-in session, keyed in [`ComAmazonawsSsoSwbportalserviceState::sessions`]
/// by the access token `CreateToken` issued for it.
///
/// `assignments` holds BOTH dimensions of the IAM Identity Center assignments the
/// session's user has — the accounts and, in each, the roles — as a `BTreeMap` of
/// `BTreeSet`s, so each listing walks a deterministic (account-id, then role-name) order
/// and a page cursor over that order stays stable between calls.
#[derive(Debug, Clone)]
pub struct Session {
    /// Who the session signed in as. Reported by no operation; it is what the
    /// assignments belong to.
    pub subject: String,
    /// The accounts this session may see, each with the role names assigned to
    /// [`Self::subject`] in that account, ascending.
    ///
    /// The roles are held HERE and not on the account, because an assignment binds a
    /// permission set to a principal in an account: a second user signed in to the same
    /// account reports the roles its OWN assignment names, not this one's.
    pub assignments: BTreeMap<String, BTreeSet<String>>,
    /// Cleared by `Logout`, which invalidates the server-side sign-in session.
    pub active: bool,
    /// When the sign-in session stops being usable, whether or not anyone logs out.
    pub expires_at: SystemTime,
    /// Whether this record is a COPY of a sign-in minted elsewhere in the emulator
    /// ([`ComAmazonawsSsoSwbportalserviceState::remember_sign_in`]) rather than a sign-in
    /// this store was given ([`ComAmazonawsSsoSwbportalserviceState::put_session`]).
    ///
    /// A copy is kept for the two things only this store knows — that a `Logout` ended the
    /// sign-in, and which identity a credential minted for it speaks for — and it never
    /// ANSWERS as a sign-in of its own: the mint's own record is what says whether such a
    /// token still names one, so a token the mint no longer holds (a refresh supersedes the
    /// access token it rotates) stops being served at that instant instead of being served
    /// from this copy. See
    /// [`ComAmazonawsSsoSwbportalserviceState::signed_in`].
    pub federated: bool,
}

impl Session {
    /// Whether a request presenting this session's token is served at `now`: a
    /// logged-out or expired session is not, and every operation answers
    /// `UnauthorizedException` for it.
    pub fn is_usable_at(&self, now: SystemTime) -> bool {
        self.active && self.expires_at > now
    }
}

/// The message the portal's authentication gate rejects a presented bearer token with.
///
/// The gate is one gate: it answers the same `UnauthorizedException` whatever operation
/// was addressed, whatever the token looked like and whatever else the request carried,
/// so the text is a constant of the service and never derived from the request. The
/// Smithy model types `UnauthorizedException$message` as an optional `ErrorDescription`
/// without stating a text; this is the one the Portal API is recorded answering, on all
/// four bound operations and every class of unusable token.
pub const INVALID_SESSION_MESSAGE: &str = "Session token not found or invalid";

/// The portal's one authentication refusal, for a presented access token that names no
/// usable sign-in session (unknown, logged out or expired alike).
///
/// Every handler returns THIS value — wrapped in its own operation's error enum — rather
/// than a message of its own, so the gate cannot drift per operation and nothing about
/// the request or the emulator's wiring leaks into it.
pub fn unauthorized_session() -> crate::ops::types::UnauthorizedException {
    crate::ops::types::UnauthorizedException {
        message: Some(INVALID_SESSION_MESSAGE.to_string()),
    }
}

/// The SAME refusal as [`unauthorized_session`], already rendered as a wire response in
/// this service's own protocol envelope.
///
/// Two classes of request never reach a handler's own gate, and this is what answers
/// both. A request presenting no bearer credential at all matches no route — the REST
/// router selects one only when every `@required` http-bound member is on the request —
/// and a request carrying a value outside a modelled constraint is refused by the
/// framework's boundary validation, which runs before any handler. `crate::service`
/// answers each through here, so the authentication refusal a caller sees is the same one
/// whether its credential was absent, named no usable session, or came alongside a value
/// this service would not have read.
///
/// The status and the `__type` come from the model, not from a literal here:
/// `UnauthorizedException` carries its own `@httpError` and is a common error of all four
/// bound operations, so one modeled value states the refusal for every route and
/// [`smithy_server_runtime::OperationError`] reads both off that shape.
///
/// `None` only if the protocol cannot serialize the error, which it cannot fail to do for
/// a shape of this service's own model; the caller then leaves the framework's answer in
/// place rather than inventing one.
pub fn unauthorized_response(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
) -> Option<aws_smithy_runtime_api::http::Response> {
    use smithy_server_runtime::OperationError;

    let refusal = crate::ops::list_accounts::ListAccountsErrorModeled::UnauthorizedException(
        unauthorized_session(),
    );
    protocol
        .serialize_error(
            refusal.as_serializable(),
            refusal.variant_schema(),
            refusal.shape_name(),
            refusal.http_status(),
        )
        .ok()
}

/// The message this service answers a request with when its own store cannot be reached.
///
/// One constant for the whole service, for the same reason [`INVALID_SESSION_MESSAGE`] is
/// one: every operation and the gate in [`crate::service`] say the same thing about the
/// same condition, and nothing about the request reaches it.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the SSO portal store is unavailable";

/// This service's refusal when [`service_state`] answers `None` — the service was built
/// on something that is not the shared `InMemoryEngine`, so there is no store to resolve
/// anything against.
///
/// Of the errors every bound operation models, the 429 is the only one that does not blame
/// the caller: the model declares no server-fault error, a 4xx naming a parameter would
/// state that the request was read and judged, and a 501 would claim the operation is
/// unwritten. Every handler returns THIS value — wrapped in its own operation's error enum
/// — so the answer cannot drift per operation.
pub fn store_unavailable() -> crate::ops::types::TooManyRequestsException {
    crate::ops::types::TooManyRequestsException {
        message: Some(STORE_UNAVAILABLE_MESSAGE.to_string()),
    }
}

/// The SAME refusal as [`store_unavailable`], already rendered as a wire response in this
/// service's own protocol envelope, for the pre-handler seams in [`crate::service`].
///
/// A request that reaches no handler is answered through here, so an unreachable store
/// answers one thing whether the request named an operation or was refused before one was
/// named. The status and the `__type` come from the model, exactly as in
/// [`unauthorized_response`]: `TooManyRequestsException` carries its own `@httpError` and
/// is a common error of all four bound operations.
///
/// `None` only if the protocol cannot serialize the error, which it cannot fail to do for
/// a shape of this service's own model; the caller then leaves the framework's answer in
/// place rather than inventing one.
///
/// `ListAccountsErrorModeled` is an arbitrary carrier, as in [`unauthorized_response`]: the
/// variant is only how one modelled SHAPE is reached, and every value on the wire — the
/// status, the `__type` and the message — comes off that shape, so the response is the same
/// whichever of the four routes the request addressed and nothing about it is
/// operation-specific.
pub fn store_unavailable_response(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
) -> Option<aws_smithy_runtime_api::http::Response> {
    use smithy_server_runtime::OperationError;

    let refusal = crate::ops::list_accounts::ListAccountsErrorModeled::TooManyRequestsException(
        store_unavailable(),
    );
    protocol
        .serialize_error(
            refusal.as_serializable(),
            refusal.variant_schema(),
            refusal.shape_name(),
            refusal.http_status(),
        )
        .ok()
}

/// An AWS account ONE sign-in may report, with the roles assigned to that sign-in's user
/// in it.
///
/// A role is a property of the (user, account) assignment rather than of the account: IAM
/// Identity Center assigns a permission set to a principal IN an account, and the portal
/// reports the role that is assigned to the user. So this is the per-sign-in view a
/// listing is built from ([`SignedIn::accounts`]) and never a record the store holds
/// account-wide — what the store holds about the account ITSELF is [`AccountRecord`].
///
/// `roles` is a `BTreeSet`: role names are unique within an assignment and a listing of
/// them must be deterministically ordered for its page cursor to resume correctly.
#[derive(Debug, Clone)]
pub struct Account {
    pub account_id: String,
    /// The account's display name, as the emulator's records of the ACCOUNT hold it, and
    /// `None` when it holds no record of this account at all.
    ///
    /// `AccountInfo$accountName` is an optional member, and the portal is not where an
    /// account's name lives: it reports the name the organization's own records carry (see
    /// [`sign_in`]). An account nothing in the emulator has a record of has no name to
    /// report, and reporting none is what that is — never a name derived from the id.
    pub account_name: Option<String>,
    /// The account's email address, on the same footing as [`Self::account_name`]
    /// (`AccountInfo$emailAddress` is optional too).
    pub email_address: Option<String>,
    /// The role names assigned to this sign-in's user in this account, ascending.
    pub roles: BTreeSet<String>,
}

/// What this store records about an AWS ACCOUNT itself, keyed in
/// [`ComAmazonawsSsoSwbportalserviceState::accounts`] by account id: the display name and
/// the email address the portal reports for it.
///
/// No role names live here. A role is the permission set an assignment grants a principal
/// in the account, so it belongs to the (user, account) pair and is held on the sign-in's
/// own assignment ([`Session::assignments`]); a set held on the account could only report
/// one role list to every user assigned to it.
#[derive(Debug, Clone, Default)]
pub struct AccountRecord {
    /// The account's display name, reported as [`Account::account_name`].
    pub account_name: Option<String>,
    /// The account's email address, reported as [`Account::email_address`].
    pub email_address: Option<String>,
}

/// A role credential `GetRoleCredentials` minted, stored so the same session asking
/// again for the same role sees the same credential until it expires.
///
/// The id and both secrets come from the emulator's credential store, so the
/// credential this service hands back is one the emulator itself will accept.
#[derive(Debug, Clone)]
pub struct RoleCredentialRecord {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// Epoch milliseconds, the unit `RoleCredentials$expiration` is modeled in.
    pub expiration_millis: i64,
}

impl RoleCredentialRecord {
    /// Whether this credential is still usable at `now`.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        self.expiration_millis > epoch_millis(now)
    }
}

/// How an issued role credential is keyed: the access token that asked for it, the
/// account, and the role. A second session asking for the same role gets its own
/// credential, exactly as a second sign-in would.
pub type IssuedCredentialKey = (String, String, String);

/// How long a minted role credential lasts when nothing CONFIGURES its length: one hour.
///
/// IAM Identity Center returns the credentials of an IAM role session whose length is
/// "the duration configured in the permission set" (the `Logout` model documentation
/// says so in as many words), so the configured duration is what a minted credential
/// runs for ([`role_session_duration`]) — nothing in this API lets a CALLER ask for
/// another one. This is the length left for a role session no permission set states one
/// for: the IAM Identity Center User Guide's "Set session duration" page gives that
/// permission-set default as 1 hour, and `SessionDuration` is an optional member a
/// permission set may carry none of.
pub const ROLE_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);

/// The largest page any listing returns, which is also the largest `maxResults` the
/// model's `MaxResultType` range (1..100) admits — so it doubles as the page size for
/// a request that asks for no particular one.
pub const MAX_PAGE_SIZE: usize = 100;

/// Where an unrepresentable session expiry saturates: far enough out that no request
/// sees it as expired, and inside the range every platform's clock type can hold.
const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// [`CENTURY`] past the epoch, or the epoch itself on a platform that cannot hold it.
fn furthest_representable_expiry() -> SystemTime {
    UNIX_EPOCH.checked_add(CENTURY).unwrap_or(UNIX_EPOCH)
}

/// `at` advanced by `duration` — the ONE way this service stamps an expiry.
///
/// `SystemTime + Duration` PANICS on a sum the platform's clock type cannot represent,
/// so every expiry is computed here instead: the sign-in session's (whose ttl is the
/// seeding caller's value) and the minted role credential's alike. An unrepresentable
/// sum saturates at the furthest instant the clock can hold, which is "not expired" for
/// every request anyway, and never earlier than `at` itself.
pub fn expiry_after(at: SystemTime, duration: Duration) -> SystemTime {
    at.checked_add(duration)
        .unwrap_or_else(|| furthest_representable_expiry().max(at))
}

/// Per-service state for com-amazonaws-sso-swbportalservice. A handler reaches it through [`service_state`].
///
/// What is held here is what only the portal knows: that a `Logout` ended a sign-in, the
/// role credentials this portal minted, and whatever sign-in or assignment was handed to
/// this store directly. The sign-in and the assignments the EMULATOR records elsewhere
/// are read where they live instead of being copied in ([`federated_sign_in`]), so an
/// assignment an administrator makes or withdraws is in force on the next request without
/// anything having to propagate it.
#[derive(Debug, Default)]
pub struct ComAmazonawsSsoSwbportalserviceState {
    /// Sign-in sessions by access token: one seeded here, and one this store was made to
    /// REMEMBER because something only it knows happened to it — a `Logout` that ended it
    /// ([`Self::invalidate_session`]) or a role credential minted for it
    /// ([`Self::remember_sign_in`]).
    pub sessions: HashMap<String, Session>,
    /// Account records by account id, ascending (a `BTreeMap`, so listings and their
    /// cursors are deterministic). What a SEEDED session reports for the accounts its
    /// assignments name; a sign-in the emulator minted carries the accounts its own
    /// assignments name, with the organization's records of each.
    pub accounts: BTreeMap<String, AccountRecord>,
    /// Role credentials already minted, by [`IssuedCredentialKey`].
    pub issued_credentials: HashMap<IssuedCredentialKey, RoleCredentialRecord>,
}

impl ComAmazonawsSsoSwbportalserviceState {
    /// Seed (or replace) the record of an account the portal can report.
    ///
    /// The portal models no create API, so this is one of the two ways the records an
    /// administrator would have made reach the emulator: a test, or whatever stands an
    /// emulator up, calls it through [`service_state`]. The other is the administrator's
    /// own IAM Identity Center records, which are read where they live
    /// ([`federated_sign_in`]) rather than copied in here.
    ///
    /// Only the account's OWN data is seeded here. Who is assigned which role in it comes
    /// with the sign-in ([`Self::put_session`]), because that is whose assignment it is.
    pub fn put_account(
        &mut self,
        account_id: impl Into<String>,
        account_name: impl Into<String>,
        email_address: impl Into<String>,
    ) {
        self.accounts.insert(
            account_id.into(),
            AccountRecord {
                account_name: Some(account_name.into()),
                email_address: Some(email_address.into()),
            },
        );
    }

    /// Seed (or replace) an active sign-in session for `access_token`, holding the
    /// `assignments` its user has — `(account id, the roles assigned to that user in that
    /// account)` — and usable for `ttl` from now.
    ///
    /// Both dimensions are the sign-in's own, so two sign-ins seeded on one account can
    /// carry different roles there, which is what an administrator assigning two users
    /// different permission sets in one account produces.
    ///
    /// `ttl` is the caller's: the sign-in session length is a directory setting in IAM
    /// Identity Center, not something this API fixes.
    pub fn put_session<A, I, R, S>(
        &mut self,
        access_token: impl Into<String>,
        subject: impl Into<String>,
        assignments: A,
        ttl: Duration,
    ) where
        A: IntoIterator<Item = (I, R)>,
        I: Into<String>,
        R: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let session = Session {
            subject: subject.into(),
            assignments: assignments
                .into_iter()
                .map(|(account_id, roles)| {
                    (
                        account_id.into(),
                        roles.into_iter().map(Into::into).collect(),
                    )
                })
                .collect(),
            active: true,
            expires_at: expiry_after(SystemTime::now(), ttl),
            // A sign-in this store was GIVEN: no mint elsewhere in the emulator holds a
            // record of it, so this record is the whole of it.
            federated: false,
        };
        self.sessions.insert(access_token.into(), session);
    }

    /// Invalidate the sign-in session `access_token` names, as `Logout` does.
    /// Answers whether a usable session was invalidated; `false` is the unauthorized
    /// case.
    ///
    /// This is the half of that invalidation THIS store keeps, keyed by the token the
    /// request presented. A sign-in session is what a whole chain of tokens belongs to
    /// rather than any one of them, so the other half is the mint's
    /// ([`end_minted_sign_in`]): without it, the refresh grant beside the presented token
    /// would rotate into a pair this store knows nothing about and the ended sign-in would
    /// be served again.
    ///
    /// Only a session this store holds a record of can be ended here, which is why
    /// `Logout` records the sign-in it resolved ([`Self::remember_sign_in`]) before it
    /// calls this: a sign-in minted by the emulator's own `CreateToken` is otherwise
    /// resolved afresh on every request, and ending it has to be remembered somewhere.
    ///
    /// The record is kept (deactivated) rather than dropped: the credentials already
    /// minted for it are keyed by this token and, per the `Logout` documentation, they
    /// go on working for the rest of their own duration.
    pub fn invalidate_session(&mut self, access_token: &str, now: SystemTime) -> bool {
        match self.sessions.get_mut(access_token) {
            Some(session) if session.is_usable_at(now) => {
                session.active = false;
                // The sign-in session ended: the one state change a caller cannot see
                // directly. Recorded without the token or the subject.
                tracing::info!(target: "sso.portal", "a sign-in session was invalidated by Logout");
                true
            }
            _ => false,
        }
    }

    /// Forget every role credential whose advertised expiration has passed at `now`, and
    /// answer the access key id of each, so the caller can end it in the emulator's
    /// credential store too (that is what [`end_expired_credentials`] does).
    ///
    /// A credential's expiration is the instant the CLOCK makes it spent, so ending it is
    /// owed then — whether or not anyone ever asks for that role again, and whether or
    /// not its sign-in session is still alive. Nothing about the shape of the request
    /// that happens to materialise it enters into which records end: every record the
    /// clock has made due does.
    pub fn expire_credentials(&mut self, now: SystemTime) -> Vec<String> {
        let mut ended = Vec::new();
        self.issued_credentials.retain(|_, record| {
            let valid = record.is_valid_at(now);
            if !valid {
                ended.push(record.access_key_id.clone());
            }
            valid
        });
        ended
    }

    /// Whether a role credential this store still holds was issued to `access_token` and is
    /// itself still inside its own duration at `now`.
    ///
    /// That is the one thing that keeps a sign-in record alive past the record's own expiry:
    /// such a credential goes on working for the rest of its duration and this record is the
    /// identity it speaks for (see [`Self::signed_in_subject`]).
    fn speaks_for_a_live_credential(&self, access_token: &str, now: SystemTime) -> bool {
        self.issued_credentials
            .iter()
            .any(|((issued_to, _, _), record)| {
                issued_to == access_token && record.is_valid_at(now)
            })
    }

    /// Whether the record held under `access_token` is one the CLOCK has made spent at
    /// `now`, by [`sign_in_record_is_spent`] — the question asked of a record this store is
    /// still holding, so the credential that may be keeping it alive is looked up here.
    fn recorded_sign_in_is_spent(
        &self,
        access_token: &str,
        session: &Session,
        now: SystemTime,
    ) -> bool {
        sign_in_record_is_spent(
            session.expires_at,
            self.speaks_for_a_live_credential(access_token, now),
            now,
        )
    }

    /// The record `access_token` names that the clock has NOT made spent at `now` — the one
    /// [`Self::expire_sessions`] leaves behind. A spent record reads as absent, because it
    /// is a record this store owes nothing but its reclaim.
    pub(crate) fn live_sign_in_record(
        &self,
        access_token: &str,
        now: SystemTime,
    ) -> Option<&Session> {
        self.sessions
            .get(access_token)
            .filter(|session| !self.recorded_sign_in_is_spent(access_token, session, now))
    }

    /// Forget every sign-in record the clock has made spent at `now`: one whose own expiry
    /// has passed and that no role credential this store still holds was issued to.
    ///
    /// A sign-in's expiry is the instant the CLOCK makes the record spent — past it the
    /// record answers nothing, because resolving a token reads a spent record as absent
    /// ([`Self::live_sign_in_record`]) — so dropping it is owed then and no response changes
    /// because this ran. A record a still-valid credential was issued to is KEPT whatever
    /// its expiry, because that credential goes on working for the rest of its own duration
    /// and this record is the identity it speaks for (see [`Self::signed_in_subject`]); the
    /// sweep is therefore run AFTER [`Self::expire_credentials`], when every credential left
    /// is one still inside its duration — and it drops exactly the records that resolution
    /// already disregards, whichever order the two run in.
    ///
    /// This is what bounds the record count: a request presenting a token the emulator's
    /// own mint issued has one recorded for it ([`Self::remember_sign_in`]), and the records
    /// of tokens whose lifetimes have run out are reclaimed here rather than held for the
    /// life of the process.
    pub fn expire_sessions(&mut self, now: SystemTime) {
        // The tokens a still-usable credential speaks through, read once: the sweep is run
        // on every request, so it walks the credentials once rather than once per record.
        // A credential whose own duration has run out holds nothing back — it is itself a
        // record `expire_credentials` is owed to end — so this set says the same thing
        // `speaks_for_a_live_credential` does, whether or not that sweep has run yet.
        let issued_to: HashSet<&str> = self
            .issued_credentials
            .iter()
            .filter(|(_, record)| record.is_valid_at(now))
            .map(|((access_token, _, _), _)| access_token.as_str())
            .collect();
        self.sessions.retain(|access_token, session| {
            !sign_in_record_is_spent(
                session.expires_at,
                issued_to.contains(access_token.as_str()),
                now,
            )
        });
    }

    /// Who the role credential `access_key_id` speaks for: the subject of the sign-in
    /// session this portal issued that credential to, while the credential itself is
    /// still usable at `now`.
    ///
    /// This is the IAM Identity Center identity a request signed with that credential
    /// carries, which is what a service requiring a federated caller resolves a request
    /// through. `None` for every credential this portal did not issue (a long-term IAM
    /// access key, a session another service minted) and for one whose own duration has
    /// run out: neither carries an Identity Center identity.
    ///
    /// A credential whose sign-in session `Logout` ENDED still speaks for the same
    /// subject until its own expiration, which is why the session's usability is not
    /// required here: the credentials already minted for a session go on working for
    /// the rest of their own duration (see [`Self::invalidate_session`]), so the
    /// identity they carry cannot stop existing earlier than they do.
    pub fn signed_in_subject(&self, access_key_id: &str, now: SystemTime) -> Option<&str> {
        self.issued_credentials
            .iter()
            .find(|(_, record)| record.access_key_id == access_key_id && record.is_valid_at(now))
            .and_then(|((access_token, _, _), _)| self.sessions.get(access_token))
            .map(|session| session.subject.as_str())
    }
}

/// End every minted role credential the clock has made spent at `now`: drop its record
/// here and forget the credential it names in the emulator's credential store, so the
/// emulator stops accepting it for signing at exactly the `expiration` the response that
/// minted it advertised; then reclaim every sign-in record the clock has made spent too
/// ([`ComAmazonawsSsoSwbportalserviceState::expire_sessions`]), which is what keeps this
/// store's record count bounded.
///
/// Every handler calls this before it serves, because the expiration is due at its
/// instant and not at the next request for the same role: a credential nobody asks about
/// again has still run out. It materialises only what the clock already owed, so no
/// response changes because this ran, and a credential still inside its own duration is
/// untouched (the sign-in session having ended does not shorten it \u2014 see
/// [`ComAmazonawsSsoSwbportalserviceState::invalidate_session`]).
///
/// A store that no longer holds the credential is already in the state this asks for, so
/// the delete's error is nothing to report.
pub fn end_expired_credentials(
    state: &mut ComAmazonawsSsoSwbportalserviceState,
    credential_store: &::emulator_server::CredentialStore,
    now: SystemTime,
) {
    for access_key_id in state.expire_credentials(now) {
        let _ = credential_store.delete(&access_key_id);
        // The role session ended on its own duration: a state change no response
        // carries. The access key id is a credential id, so it is not recorded.
        tracing::debug!(
            target: "sso.portal",
            "a minted role credential reached its expiration and was ended"
        );
    }
    // After the credentials, so a record still speaking for a usable credential is kept
    // and every other expired record is reclaimed.
    state.expire_sessions(now);
}

/// [`end_expired_credentials`] for a caller holding no store: reach this service's store in
/// `engine`, hold it for exactly this sweep, and end what the clock had already made spent
/// at `now`.
///
/// A handler already holds the store for its own work and calls
/// [`end_expired_credentials`] directly. The seams of [`crate::service`] that answer a
/// request WITHOUT reaching a handler — the one that refuses a request presenting no usable
/// session, and the one that refuses a request whose wire value no modelled value can be
/// read from — hold it for nothing else, and the reclaim is owed at the instant the clock
/// made it due whatever request happens to arrive next, so they run it through here: one
/// reading of that sweep for every such seam, beside the rule it materialises, rather than a
/// copy of it per seam. A seam the framework hands no
/// [`OperationContext`](smithy_server_runtime::OperationContext) has no credential store to
/// end a credential in, so it runs no sweep at all.
///
/// A store that cannot be reached holds no credential to end, and it materialises only what
/// the clock already owed, so no response changes because this ran.
pub fn sweep_expired_credentials<Eng: Send + Sync + 'static>(
    engine: &Eng,
    credential_store: &::emulator_server::CredentialStore,
    now: SystemTime,
) {
    if let Some(state) = service_state(engine) {
        let mut state = state.lock();
        end_expired_credentials(&mut state, credential_store, now);
    }
}

/// Whether the CLOCK has made a sign-in record spent at `now`: its own expiry has passed
/// and no role credential still inside its own duration was issued to it
/// (`kept_by_a_credential`).
///
/// The ONE expression of "spent", evaluated by both readings of that question — the
/// reclaim's own ([`ComAmazonawsSsoSwbportalserviceState::expire_sessions`], which drops
/// every record it holds of) and resolution's
/// ([`ComAmazonawsSsoSwbportalserviceState::live_sign_in_record`], which reads such a record
/// as absent). Written once because the two must agree: were they stated separately,
/// resolving a token would answer one thing while the reclaim was still outstanding and
/// another once it had run, and the reclaim is owed at its instant rather than at whichever
/// request happens to materialise it.
fn sign_in_record_is_spent(
    expires_at: SystemTime,
    kept_by_a_credential: bool,
    now: SystemTime,
) -> bool {
    expires_at <= now && !kept_by_a_credential
}

/// `at` as epoch milliseconds, the unit `RoleCredentials$expiration` is modeled in.
pub fn epoch_millis(at: SystemTime) -> i64 {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// One page of a listing: the items to report, and the cursor to continue from when
/// the listing was cut short.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page — a client stops when the token stops coming.
    pub next_token: Option<String>,
}

/// A presented `nextToken` that cannot be a cursor of the listing it was presented to:
/// not a cursor at all, or one scoped to a different listing or a different session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;

/// The message this service refuses a `nextToken` no listing of its own issued with.
///
/// One constant for the whole service, for the same reason [`INVALID_SESSION_MESSAGE`] is
/// one: the cursor codec is one codec ([`encode_cursor`] / [`decode_cursor`]), so every
/// paginated operation says the same thing about a token that is not its output, and
/// nothing about the rejected token reaches the text. The Smithy model types
/// `InvalidRequestException$message` as an optional `ErrorDescription` without stating a
/// text, so the service states this one.
pub const INVALID_CURSOR_MESSAGE: &str =
    "nextToken is not a page token this operation issued for this listing";

/// The one refusal every paginated operation answers an [`InvalidCursor`] with: the
/// modelled 400 `InvalidRequestException`, worded by [`INVALID_CURSOR_MESSAGE`].
///
/// Both paginated handlers map the `InvalidCursor` that [`page_of`] hands back through
/// HERE — wrapping it in their own operation's error enum — rather than classifying and
/// wording it themselves, so neither the class nor the sentence can drift between the two
/// listings, exactly as [`unauthorized_session`] holds the authentication refusal to one
/// value.
pub fn invalid_cursor() -> crate::ops::types::InvalidRequestException {
    crate::ops::types::InvalidRequestException {
        message: Some(INVALID_CURSOR_MESSAGE.to_string()),
    }
}

/// The version stamped into a cursor, so a token minted by an older encoding is
/// refused rather than silently misread.
const CURSOR_VERSION: &str = "v1";

/// One field of a composite string, written so the reader takes back exactly the bytes
/// the writer put in: the field's length in bytes, a `:`, then the field itself.
///
/// A composite string is only as trustworthy as the boundaries between its fields. Joining
/// variable-length fields with a bare separator leaves the split to be GUESSED whenever a
/// field may carry that separator itself, and then two different field tuples spell the
/// same string: whoever reads it back cannot tell which tuple it is holding, and a check
/// that re-writes what it read agrees with itself either way. Fields here do carry
/// anything — a cursor's scope names the listing, which for a role listing embeds the
/// requested account id, and `AccountIdType` is an unconstrained string
/// (`smithy_query(query="shape", shape="AccountIdType")` reports no `@pattern` and no
/// `@length`) — so the length goes in front of the field, terminated by the `:`. The
/// reader then knows each field's extent before reading it and no byte inside a field can
/// be taken for a boundary, which makes the join INJECTIVE: one string, one tuple.
///
/// A composition whose only variable-length field comes last needs no framing (the
/// remainder of the string IS that field), which is why a listing name may still be built
/// by a handler as a constant prefix followed by the account id.
fn framed_field(field: &str) -> String {
    format!("{}:{field}", field.len())
}

/// The field [`framed_field`] wrote at the start of `framed`, and whatever follows it.
/// `None` when `framed` does not begin with a field this crate could have written.
///
/// The length is admitted only in the ONE spelling [`framed_field`] writes — plain ASCII
/// digits, no sign and no leading zero — so a framed field has a single encoding and the
/// framing stays injective in both directions.
fn take_framed_field(framed: &str) -> Option<(&str, &str)> {
    let (length, rest) = framed.split_once(':')?;
    if length.is_empty()
        || !length.bytes().all(|digit| digit.is_ascii_digit())
        || (length.len() > 1 && length.starts_with('0'))
    {
        return None;
    }
    let length: usize = length.parse().ok()?;
    if rest.len() < length || !rest.is_char_boundary(length) {
        return None;
    }
    Some(rest.split_at(length))
}

/// The scope a cursor is bound to: the listing it walks, and the SIGN-IN SESSION that
/// asked for it. A cursor presented to another listing or by another sign-in is refused.
///
/// The sign-in session is named by its own identifier ([`SignedIn::sign_in_session`]) and
/// never by the token that happened to present it, because a sign-in session is what a
/// whole chain of tokens belongs to rather than any one of them: the pair a refresh
/// rotation issues supersedes the access token it rotates while the sign-in itself
/// continues, so the token a client must present after a rotation continues the walk the
/// token before it began. Scoping to the presented token instead would strand the rest of
/// a listing at every rotation — the cursor refused for the one token that can still be
/// presented — while buying nothing: the items a continuation reports are built from the
/// sign-in's own assignments either way.
///
/// The identifier is digested rather than carried, and only its first half is kept: a
/// cursor travels back out in a response and into a client's next URL, so neither the
/// session identifier nor anything else the store holds may be recoverable from it.
///
/// The two parts are joined by [`framed_field`], so one scope string names one (listing,
/// sign-in) pair whatever the listing name carries — a scope is what a cursor is bound
/// to, and a binding two different pairs could spell is no binding.
pub fn cursor_scope(listing: &str, sign_in_session: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(sign_in_session.as_bytes());
    format!(
        "{}{}",
        framed_field(listing),
        framed_field(&hex::encode(&digest[..16]))
    )
}

/// An opaque continuation cursor for `scope`, resuming after `last_key`.
///
/// Hex rather than a bespoke encoding because the token only has to be opaque and
/// round-trip exactly; `hex` is already a workspace dependency. It is NOT
/// authenticated: a caller who reproduces the encoding can resume this listing at a
/// key of its choosing, which reaches nothing the same session's listing would not
/// already report (the items are built from that session's own assignments), and
/// authenticating it would need either a server secret or per-request state written by
/// a reporting operation.
///
/// Its fields are joined by [`framed_field`], which is what makes the mint injective over
/// (version, scope, key): a cursor minted for one scope is bytes no other scope's mint can
/// produce, so it cannot be read back under another scope at all, however the two scopes
/// or the resume key are spelt.
pub fn encode_cursor(scope: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{}{}{}",
        framed_field(CURSOR_VERSION),
        framed_field(scope),
        framed_field(last_key)
    ))
}

/// The key a cursor resumes after, when it is one [`encode_cursor`] produced for the
/// same `scope`. A cursor from another listing (a different account's roles, or another
/// session's accounts) is refused rather than applied to this one.
///
/// Admitted ONLY when the token is byte for byte the one [`encode_cursor`] mints for the
/// key it carries: the acceptance set is the mint's output set and nothing wider.
/// `hex::decode` reads spellings `hex::encode` never writes (upper- and mixed-case
/// digits), and such a spelling is a token no response of this service ever carried, so
/// re-minting the decoded key and comparing is what holds the two to exact inverses —
/// whatever the encoding is, and without a second grammar maintained beside it.
///
/// The version and the scope are matched as the FRAMED fields [`encode_cursor`] wrote, so
/// the key this answers is the one that was minted and never a tail some other scope's
/// cursor happens to leave behind: the re-mint check cannot catch a mis-split, because
/// re-minting the halves it split into spells the very same bytes.
pub fn decode_cursor(scope: &str, token: &str) -> Result<String, InvalidCursor> {
    let bytes = hex::decode(token).map_err(|_| InvalidCursor)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidCursor)?;
    let rest = decoded
        .strip_prefix(&framed_field(CURSOR_VERSION))
        .and_then(|rest| rest.strip_prefix(&framed_field(scope)))
        .ok_or(InvalidCursor)?;
    let (last_key, trailing) = take_framed_field(rest).ok_or(InvalidCursor)?;
    if !trailing.is_empty() {
        return Err(InvalidCursor);
    }
    if encode_cursor(scope, last_key) != token {
        return Err(InvalidCursor);
    }
    Ok(last_key.to_string())
}

/// Cut `sorted` — a listing of `(sort key, item)` pairs in ascending key order — into
/// the page a request asked for.
///
/// `next_token` resumes strictly after the key it carries, so a continuation never
/// repeats or skips an item; `max_results` bounds the page, defaulting to
/// [`MAX_PAGE_SIZE`] when the caller names none; and the token comes back only when
/// items were left over, so the last page carries none.
///
/// A `max_results` OUTSIDE the modeled `MaxResultType` range (1..100) never reaches
/// here from the wire: the boundary validator refuses it before the handler runs, and
/// this service words that refusal as `InvalidRequestException` (the 400 rows in
/// `tests/all/portal_session.rs` pin it). The clamp below is therefore only a guard for
/// a direct, non-HTTP caller, not a silent reshaping of a request a client sent: a bound
/// that caller NAMED is carried into the modeled range — floored at 1 below it, capped at
/// [`MAX_PAGE_SIZE`] above it — and never folded into the "named none" case, so only the
/// ABSENCE of a bound takes the default page.
pub fn page_of<T>(
    sorted: impl IntoIterator<Item = (String, T)>,
    scope: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, InvalidCursor> {
    let after = match next_token {
        Some(token) => Some(decode_cursor(scope, token)?),
        None => None,
    };
    let limit = match max_results {
        // Clamped while the value is still SIGNED: `usize::try_from` fails for every
        // negative bound, so converting first would discard the sign and leave a named
        // bound indistinguishable from `None` — the default page, the far end of the
        // range from the floor a too-small bound belongs at.
        Some(requested) => usize::try_from(requested.max(1))
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE),
        None => MAX_PAGE_SIZE,
    };

    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut truncated = false;
    for (key, item) in sorted {
        if after.as_deref().is_some_and(|after| key.as_str() <= after) {
            continue;
        }
        if items.len() >= limit {
            truncated = true;
            break;
        }
        last_key = key;
        items.push(item);
    }

    Ok(Page {
        items,
        next_token: truncated.then(|| encode_cursor(scope, &last_key)),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsoSwbportalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsoSwbportalserviceState>)
}
