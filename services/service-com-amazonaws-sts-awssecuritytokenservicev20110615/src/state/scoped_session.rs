// SPDX-License-Identifier: CC0-1.0

//! What a SCOPE-DOWN of the calling session is decided by: the rules
//! `GetScopedCredentials` applies to the session its caller presented, and the sentences it
//! refuses one with.
//!
//! "Returns a set of temporary security credentials that are scoped down from the caller's
//! existing temporary security credentials. `GetScopedCredentials` preserves the caller's
//! identity while narrowing the permissions and optionally shortening the session lifetime"
//! (this operation's documentation, this service's model). So the call does not mint a
//! session of its own: it vends another credential for the session the caller already is,
//! with one more scope-down document capping what that credential may do and a lifetime
//! that cannot reach past the calling session's. Everything here is a rule about THAT
//! session — one concern, one file — so the handler holds only the order the rules are
//! applied in.

/// The most scope-down policies one session may carry: "a maximum of 3 scope down policy
/// layers are allowed per session. If a session already has 3 `GetScopedCredentials`
/// scope-down policies, a subsequent `GetScopedCredentials` call that supplies a `Policy`
/// (attempting to append a fourth scope-down policy) is denied"
/// (`GetScopedCredentialsResponse$ScopedPoliciesCount`, this service's model).
pub const MAX_SCOPE_DOWN_POLICIES: usize = 3;

/// How AWS states the rule a request carrying neither member breaks: "a request must provide
/// at least one of `Policy` or `DurationSeconds`. Both parameters are optional individually,
/// but you must specify at least one of them" (`GetScopedCredentialsRequest$Policy`, this
/// service's model). Fixed text: the rule is about which members are there, so the sentence
/// carries nothing of the request.
pub const NEITHER_POLICY_NOR_DURATION_MESSAGE: &str =
    "A request must provide at least one of Policy or DurationSeconds.";

/// How this service refuses a call whose credential is no session at all: "you can only call
/// `GetScopedCredentials` with temporary security credentials, for example, credentials from
/// an assumed role or a federated session. You cannot call it with IAM user credentials or
/// root credentials" (this operation's documentation). There is no session to scope down, so
/// the sentence names the credential class the call needs and nothing of the caller.
pub const NOT_A_SESSION_MESSAGE: &str =
    "GetScopedCredentials can only be called with temporary security credentials, not with \
     IAM user credentials or root credentials.";

/// How AWS refuses a duration past what is left of the calling session: "the request was
/// rejected because the requested `DurationSeconds` is greater than the remaining lifetime of
/// the calling session. `GetScopedCredentials` cannot extend a session beyond its original
/// expiration" (`InsufficientSessionLifetimeException`, this service's model) — stated the
/// way this service words every other duration refusal, as the bound that was passed rather
/// than as the two numbers it was measured from.
pub const DURATION_PAST_SESSION_LIFETIME_MESSAGE: &str =
    "The requested DurationSeconds exceeds the remaining lifetime of the calling session.";

/// How a call that would append one scope-down policy too many is denied, stating the limit
/// the session is already at ([`MAX_SCOPE_DOWN_POLICIES`], the model's own sentence).
#[must_use]
pub fn scope_down_limit_message() -> String {
    format!("A maximum of {MAX_SCOPE_DOWN_POLICIES} scope down policy layers are allowed per session.")
}

/// How many scope-down policies the session this call vends credentials for carries:
/// "the caller's existing `GetScopedCredentials` scope-down count plus the one this request
/// appends when it supplies a `Policy` … a request that supplies only a `DurationSeconds` (no
/// `Policy`) adds no scope-down policy, so this value matches the caller's existing count"
/// (`scopedPoliciesCountType`, this service's model).
#[must_use]
pub fn scoped_policies_count(carried: usize, appends_a_policy: bool) -> i32 {
    let count = carried.saturating_add(usize::from(appends_a_policy));
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Whether appending one more scope-down policy would take the session past
/// [`MAX_SCOPE_DOWN_POLICIES`] layers — asked only of a request that supplies a `Policy`,
/// because one that does not appends no layer.
#[must_use]
pub fn scope_down_limit_reached(carried: usize, appends_a_policy: bool) -> bool {
    appends_a_policy && carried >= MAX_SCOPE_DOWN_POLICIES
}

/// How long the scoped credentials last, or [`Err`] when the request asked for longer than
/// the calling session has left.
///
/// "The duration, in seconds, of the resulting scoped down credentials… If you do not specify
/// this value, the scoped down credentials will inherit the calling session's expiration
/// time" (`GetScopedCredentialsRequest$DurationSeconds`), and the call "cannot extend a
/// session beyond its original expiration" — so a request that names no duration gets exactly
/// what is left of the calling session, and one that names more than that is refused.
///
/// `remaining` is what is left of the calling session at the instant of the call, in seconds.
pub fn scoped_duration_seconds(asked_for: Option<i32>, remaining: i32) -> Result<i32, TooLong> {
    match asked_for {
        Some(asked_for) if asked_for > remaining => Err(TooLong),
        Some(asked_for) => Ok(asked_for),
        // A session with less than a second left inherits an instant, not a count of
        // seconds: the expiry the scoped session is recorded with is the calling session's
        // own ([`crate::state::NewSession::expires_at`]), so what this value states is the
        // DURATION on the record, which is held inside the modelled range (`1..129600`)
        // rather than written as the zero the remainder floors to.
        None => Ok(remaining.max(1)),
    }
}

/// The request asked for a longer session than the calling one has left
/// ([`scoped_duration_seconds`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLong;

/// How many whole seconds of `session` are left at `now`, as the duration a scoped session
/// may inherit: zero for a session the clock has already reached, which no scope-down can
/// extend.
#[must_use]
pub fn remaining_lifetime_seconds(expires_at: ::std::time::SystemTime, now: ::std::time::SystemTime) -> i32 {
    let left = expires_at
        .duration_since(now)
        .unwrap_or_default()
        .as_secs();
    i32::try_from(left).unwrap_or(i32::MAX)
}
