// SPDX-License-Identifier: CC0-1.0

//! The CREDENTIAL a running handler's own AWS SDK calls are made with: a SESSION of the
//! function's execution role.
//!
//! "The execution role grants the function permission to use Amazon Web Services services"
//! (`FunctionConfiguration$Role`), and a function uses them by calling them with temporary
//! credentials for that role — the credentials a managed runtime hands the SDK on the
//! function's import path. So a call a handler makes is a call made BY THAT ROLE, and the
//! service serving it judges it against the policies in force on the role and against its
//! own resource policy, exactly as it judges any other signed request. That is the whole
//! point of an execution role: two functions whose roles carry different grants are refused
//! different things.
//!
//! This module is where that credential comes from:
//!
//! * The role is resolved against the IAM store of the SAME shared engine, which is where
//!   the account's roles live, so the session speaks for the very record `iam:CreateRole`
//!   wrote.
//! * The session is minted by the TOKEN SERVICE's own mint
//!   ([`sts_store::mint_role_session`]), so `sts:GetCallerIdentity` under the credential
//!   reports the session, every service that authorizes a request resolves it the same way
//!   — the mint records the session wherever a temporary credential's principal is read
//!   from, so the call is judged on the policies in force on the role whichever service
//!   serves it — and the credential is one the emulator's own request verifier accepts.
//!   Nothing here constructs an access key id or a secret.
//! * Its identity is the assumed-role session AWS reports for a function's calls:
//!   `arn:<partition>:sts::<account>:assumed-role/<role name>/<function name>` — the session
//!   is named for the FUNCTION it was opened for, which is how a refusal says which
//!   function was refused.
//! * It belongs to the EXECUTION ENVIRONMENT rather than to one invocation: Lambda's
//!   credentials are held by the environment and refreshed, so one session is held per
//!   function here and re-minted once the clock has brought it close enough to its expiry
//!   that a run could outlast it ([`REFRESH_MARGIN`]). The one it replaces is given up in the
//!   credential store, so a request presenting it no longer verifies, and in the token
//!   service's registry, so it names no identity either; the record the mint left where a
//!   principal is read from speaks for a credential nothing can present any more, and is
//!   pruned at its own expiry.
//!
//! Where nothing can be SHOWN about the role — this emulator holds no IAM record of it (an
//! execution role is normally provisioned outside the client that creates the function,
//! which is why `crate::state::execution_role_is_unassumable` serves such a create), or
//! there is no token service to mint a session with — no session is minted and the call is
//! signed with the credential the framework hands this emulator's handlers, which is what
//! every other in-process call between two emulated services presents. Refusing on a role
//! this emulator knows nothing about would deny calls AWS permits.
//!
//! The credential never reaches the guest: it is held by host code, put on the request's
//! `Authorization` and session-token headers ([`super::aws_api`]), and in no response, log
//! or environment variable of the run.

use ::std::collections::BTreeMap;
use ::std::time::{Duration, SystemTime};

use ::emulator_server::CredentialStore;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

use super::aws_api::CallerIdentity;
use crate::state::event_source_authorization::iam_state;

/// How long a minted session lasts.
///
/// `sts:AssumeRole` documents 3600 seconds as the default duration of a role session
/// ("`DurationSeconds` … Default: 3600 seconds"), and a session is never minted for longer
/// than the role itself admits (`Role$MaxSessionDuration`), so a role configured for a
/// shorter maximum gets a shorter session.
const SESSION_SECONDS: i32 = 3600;

/// The range `Role$MaxSessionDuration` may carry: "the maximum session duration (in
/// seconds) … Minimum value of 3600. Maximum value of 43200" (`UpdateRole`'s own
/// documentation of the member). A stored value outside it is not a maximum AWS could have
/// recorded, so it caps nothing here and the default duration stands.
const MAX_SESSION_DURATION_RANGE: ::std::ops::RangeInclusive<i32> = 3600..=43_200;

/// How much of a held session's life has to be left for it to be used again.
///
/// The longest `Timeout` a function may carry is 900 seconds, so a session with at least
/// that much left cannot expire in the middle of a run that starts now; one with less is
/// re-minted first. Stated as a span and not as a count of invocations, because what makes
/// a credential unusable is the CLOCK.
const REFRESH_MARGIN: Duration = Duration::from_secs(900);

/// The sessions this emulator holds for its functions' execution environments, keyed by the
/// role and the function the session was opened for.
///
/// Engine-scoped rather than a field of this service's own store, so minting one takes the
/// token service's lock and this small one and never this service's state guard. LOCK
/// ORDER: this store first, then the token service's registry and the credential store, and
/// last the IAM store the mint records the session in — the order [`execution_role_identity`]
/// takes them in, and the only order anything may.
#[derive(Debug, Default)]
pub struct ExecutionRoleSessions {
    /// One held session per `(role ARN, function name)` — a structured key, so no spelling
    /// of either part can collide with another pair's.
    held: BTreeMap<(String, String), Held>,
}

/// One held session: what a call is signed with, and when it stops being valid.
#[derive(Debug, Clone)]
struct Held {
    /// The credential itself. Its debug form carries no field.
    identity: CallerIdentity,
    /// The instant the token service minted it to last until.
    expires_at: SystemTime,
}

/// The sessions store in the SAME shared engine, or `None` where the engine carries none.
fn sessions_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ExecutionRoleSessions>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ExecutionRoleSessions>)
}

/// The SDK seam one invocation of `function_name` gets: the callback it reaches this
/// emulator through, signing every call as the session of the function's execution role
/// `role_arn`.
///
/// This is what every operation that RUNS a handler builds the seam with, so a handler's
/// own call is made by the role the function runs as whichever operation started it. Where
/// no session can be minted — nothing is known about the role, or there is no token service
/// to mint one with (see the module docs) — the seam signs with the credential the framework
/// hands this emulator's handlers, which is what every other in-process call presents.
/// `None` where there is no seam at all, which is a run with no SDK.
///
/// It also carries the records of WHICH service answers at an address this emulator
/// published for a resource's own data plane, out of the same shared engine the roles above
/// live in: a handler that read such an address out of a control-plane response and dials it
/// is reaching the service that published it
/// ([`::service_common::published_data_planes`]).
#[must_use]
pub fn function_sdk_access<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    function_name: &str,
) -> Option<super::AwsAccess> {
    let identity = execution_role_identity(
        engine,
        ctx.credential_store(),
        role_arn,
        function_name,
        SystemTime::now(),
    );
    Some(
        super::AwsAccess::new(ctx.callback().clone(), region)?
            .signing_as(identity)
            .resolving_published_planes(
                ::service_common::published_data_planes::PublishedPlanes::of(engine),
            ),
    )
}

/// The credential the calls of an invocation of `function_name` running as `role_arn` are
/// signed with, minting the session if this emulator holds none that is still good for a
/// whole run.
#[must_use]
pub fn execution_role_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role_arn: &str,
    function_name: &str,
    now: SystemTime,
) -> Option<CallerIdentity> {
    let role = held_role(engine, role_arn)?;
    let sessions = sessions_state(engine)?;
    let key = (role_arn.to_string(), function_name.to_string());
    let mut sessions = sessions.lock();
    // A session the clock has already expired is held by nobody: it is given up here, so a
    // credential no execution environment presents does not stay in either store for the
    // life of the process.
    let expired: Vec<(String, String)> = sessions
        .held
        .iter()
        .filter(|(_, held)| held.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect();
    for stale in expired {
        if let Some(held) = sessions.held.remove(&stale) {
            forget(engine, store, &held.identity.access_key_id);
        }
    }
    // A held session is used again only while it has a whole run's worth of life left.
    let still_good = sessions
        .held
        .get(&key)
        .filter(|held| held.expires_at > now + REFRESH_MARGIN);
    if let Some(held) = still_good {
        return Some(held.identity.clone());
    }
    let minted = mint(engine, store, &role, function_name, now)?;
    let identity = minted.identity.clone();
    // The session this one replaces is forgotten where it was held, so a credential this
    // environment no longer presents stops being valid.
    if let Some(replaced) = sessions.held.insert(key, minted) {
        forget(engine, store, &replaced.identity.access_key_id);
    }
    Some(identity)
}

/// What the IAM store holds about one execution role, as a session of it is minted from.
struct ResolvedRole {
    /// The role's ARN, exactly as IAM holds it — the spelling a policy names the role by.
    arn: String,
    /// Its name, which the session's own ARN carries.
    name: String,
    /// Its unique id, which the session's `AssumedRoleId` is built from.
    id: String,
    /// The account whose IAM holds it, read off its ARN.
    account: String,
    /// The partition that ARN names, so a session of a role of another partition is named
    /// in that partition.
    partition: String,
    /// "The maximum session duration (in seconds) that you want to set for the specified
    /// role" (`Role$MaxSessionDuration`), which caps the session minted here — the default
    /// duration for a role whose record carries no value in the range AWS admits for it
    /// ([`MAX_SESSION_DURATION_RANGE`]).
    max_session_duration: i32,
}

/// The role `role_arn` names, when the IAM store of this engine holds one under it.
///
/// `None` when nothing is known about the role: no IAM store on this engine, a value that
/// names no IAM role, or a name this account's IAM holds no role under.
fn held_role<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> Option<ResolvedRole> {
    let parsed = ::service_common::arn::parse(role_arn)?;
    let iam = iam_state(engine)?;
    let held = iam.lock();
    let role = held.roles.values().find(|role| role.arn == role_arn)?;
    let resolved = ResolvedRole {
        arn: role.arn.clone(),
        name: role.role_name.clone(),
        id: role.role_id.clone(),
        account: parsed.account.to_string(),
        partition: parsed.partition.to_string(),
        max_session_duration: role
            .max_session_duration
            .filter(|duration| MAX_SESSION_DURATION_RANGE.contains(duration))
            .unwrap_or(SESSION_SECONDS),
    };
    Some(resolved)
}

/// Mint one session of `role` for `function_name`, through the token service's own mint.
fn mint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role: &ResolvedRole,
    function_name: &str,
    now: SystemTime,
) -> Option<Held> {
    let registry = sts_store::service_state(engine)?;
    let identity = sts_store::Identity {
        user_id: format!("{}:{function_name}", role.id),
        account: role.account.clone(),
        arn: format!(
            "arn:{}:sts::{}:assumed-role/{}/{function_name}",
            role.partition, role.account, role.name
        ),
    };
    let duration_seconds = SESSION_SECONDS.min(role.max_session_duration);
    // Minted through the token service's shared role-session mint, which also records the
    // session where a service resolves a temporary credential's principal: a call this
    // function's handler makes is then judged on the policies in force on the execution
    // role, whichever service serves it.
    let minted = sts_store::mint_role_session(
        engine,
        store,
        &registry,
        now,
        sts_store::NewSession {
            role_arn: Some(role.arn.clone()),
            ..sts_store::NewSession::new(
                identity,
                sts_store::SessionKind::AssumedRole,
                duration_seconds,
            )
        },
    )
    .ok()?;
    Some(Held {
        identity: CallerIdentity {
            access_key_id: minted.credentials.access_key_id,
            secret_access_key: minted.credentials.secret_access_key,
            session_token: minted.credentials.session_token,
        },
        expires_at: minted.session.expires_at,
    })
}

/// Forget a session this module minted, in both stores that held it: the credential store
/// (so a request presenting it no longer verifies) and the token service's own registry (so
/// it names no identity either).
fn forget<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    access_key_id: &str,
) {
    sts_store::forget_audited_session(engine, access_key_id);
    match sts_store::service_state(engine) {
        Some(registry) => registry.lock().discard(access_key_id, store),
        None => {
            let _ = store.delete(access_key_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use ::std::time::Duration;

    use super::{execution_role_identity, sessions_state, REFRESH_MARGIN, SESSION_SECONDS};
    use ::emulator_server::CredentialStore;
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    /// The role the rows below run as, held where IAM holds the account's roles: the record
    /// `iam:CreateRole` writes, planted here through IAM's OWN state type and key rule
    /// rather than through a second copy of it.
    fn held_role(engine: &::engine::InMemoryEngine, role_name: &str) -> String {
        use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

        let arn = format!(
            "arn:aws:iam::{}:role/{role_name}",
            ::engine::emulator_account_id()
        );
        let role = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        };
        let iam = iam_store::service_state(engine).expect("the engine carries an IAM store");
        iam.lock()
            .roles
            .insert(iam_store::name_key(role_name), role);
        arn
    }

    /// RULE: the credential belongs to the execution environment — it is used again while it
    /// has a whole run's worth of life left, and re-minted once the CLOCK has brought it
    /// nearer than that to its expiry, with the one it replaces left valid nowhere.
    ///
    /// The rows differ along WHEN the next call is made: at the same instant, inside the
    /// session's usable span, and past it.
    #[test]
    fn a_held_session_is_used_again_until_the_clock_brings_it_near_its_expiry() {
        let engine = ::engine::InMemoryEngine::new();
        let store = CredentialStore::default();
        let role_arn = held_role(&engine, "lambda-session-probe");
        let minted_at = ::std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let identity = |now| {
            execution_role_identity(&engine, &store, &role_arn, "probe", now)
                .map(|held| held.access_key_id)
        };
        let first = identity(minted_at).expect("a session is minted for a held role");
        let usable =
            Duration::from_secs(u64::try_from(SESSION_SECONDS).expect("a positive duration"))
                - REFRESH_MARGIN;
        let rows: [(&str, Duration, bool); 3] = [
            ("the same instant", Duration::ZERO, true),
            ("inside the usable span", usable - Duration::from_secs(1), true),
            ("past it", usable + Duration::from_secs(1), false),
        ];
        for (case, elapsed, same) in rows {
            let later = identity(minted_at + elapsed).expect("a session is still minted");
            assert_eq!(
                later == first,
                same,
                "case {case:?}: expected the held credential to be reused={same}, got \
                 {later:?} against {first:?}"
            );
            if same {
                continue;
            }
            // The replaced session is valid nowhere: the token service's registry no longer
            // names it, so a request presenting it speaks for nobody.
            let registry =
                sts_store::service_state(&engine).expect("the engine carries a session registry");
            assert!(
                registry.lock().session(&first).is_none(),
                "case {case:?}: expected the replaced session {first:?} to be forgotten"
            );
        }
        // One session per (role, function), however many invocations ask for it.
        let sessions = sessions_state(&engine).expect("the engine carries the session store");
        let held = sessions.lock().held.len();
        assert_eq!(
            held, 1,
            "expected one held session for the one function, got {held}"
        );
    }
}
