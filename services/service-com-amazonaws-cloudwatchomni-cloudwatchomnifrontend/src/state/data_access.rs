// SPDX-License-Identifier: CC0-1.0

//! The temporary credentials a caller works with a space's data under, as issued and as
//! held.
//!
//! `GetSpaceDataAccess`'s model doc reads "Returns temporary credentials for working with
//! data in a space, limited to the actions you request", and its `durationSeconds` member
//! "The lifetime of the credentials, in seconds. Defaults to 3600". Three things follow,
//! and all three are facts about the SPACE and the credentials issued against it rather
//! than about one request, which is why they live beside the store:
//!
//! * a credential set is RECORDED against the space it was issued for
//!   ([`StoredDataAccessSession`]), with the instant it falls due: what the response
//!   reports is read back out of that record, and the clock ends it there and in the
//!   emulator's credential store together ([`end_expired_data_access_sessions`]), so the
//!   emulator stops accepting it for signing at exactly the `expiration` it advertised;
//! * "limited to the actions you request" is a SCOPE, so the requested actions are
//!   rendered as the policy the credentials carry ([`data_access_scope_policy`]) and that
//!   document is held with the record — the credentials are scoped by something the store
//!   holds rather than by a claim in a response;
//! * a scope whose policy is larger than a session policy may be is what the modelled
//!   `PolicyTooLargeException` is for ([`MAX_SESSION_POLICY_CHARACTERS`]), so the size of
//!   that same document decides the refusal.
//!
//! BOUNDED twice over: every issue first drops the records the clock has made spent, and
//! the longest lifetime a request can ask for is the 3600 seconds `durationSeconds`'
//! `@range(min: 900, max: 3600)` admits, so the map holds only the credentials issued
//! within one such span; above that, [`DATA_ACCESS_SESSION_QUOTA`] caps how many live
//! records it holds at once. Each record is itself bounded — its policy by
//! [`MAX_SESSION_POLICY_CHARACTERS`] and its audit context by `DataAccessAuditContext`'s
//! own `@length(max: 10)` with its 256-character keys and values.

use std::collections::HashMap;

use crate::ops::types::DataAccessTarget;

/// The lifetime credentials get when the request names no `durationSeconds`.
///
/// The member's own model doc: "The lifetime of the credentials, in seconds. Defaults to
/// 3600."
pub const DEFAULT_DATA_ACCESS_DURATION_SECONDS: i32 = 3600;

/// The shortest lifetime a request may ask for — the `min` of `durationSeconds`'
/// `@range(min: 900, max: 3600)`
/// (`smithy_query(query="operation",operation="GetSpaceDataAccess")`).
pub const MIN_DATA_ACCESS_DURATION_SECONDS: i32 = 900;

/// The longest lifetime a request may ask for — the `max` of the same range, which is also
/// the default.
pub const MAX_DATA_ACCESS_DURATION_SECONDS: i32 = DEFAULT_DATA_ACCESS_DURATION_SECONDS;

/// The language version every IAM policy document declares, which the policy a credential
/// set is scoped by carries.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The largest policy, in characters, a credential set can carry its scope in.
///
/// AWS states it for the policies passed with temporary credentials: the plaintext of a
/// session policy cannot exceed 2,048 characters
/// (<https://docs.aws.amazon.com/IAM/latest/UserGuide/access_policies.html#reference_iam-quotas>).
/// A scope whose rendered document is longer than that cannot be conferred, which is what
/// `PolicyTooLargeException` — "The requested actions produce a credential that exceeds the
/// maximum size. Request fewer actions." — is declared for.
pub const MAX_SESSION_POLICY_CHARACTERS: usize = 2048;

/// The message a request whose scope does not fit a session policy is refused with.
///
/// Wholly fixed text, as `PolicyTooLargeException` itself words the condition: the refusal
/// is about the size of the scope asked for, and naming the actions back would repeat the
/// request rather than tell the caller anything the sentence does not.
pub const POLICY_TOO_LARGE_MESSAGE: &str =
    "The requested actions produce a credential that exceeds the maximum size. Request fewer \
     actions.";

/// How many live credential sets the store holds before a further issue is refused.
///
/// The emulator's OWN bound rather than an AWS quota, for the same reason as
/// [`super::SPACE_QUOTA`]: a credential set is issued per call, so the map needs a declared
/// cap above the lifetime that already bounds it. It REFUSES a new issue rather than ending
/// a live one, so credentials already handed out keep working for the lifetime they were
/// advertised with.
pub const DATA_ACCESS_SESSION_QUOTA: usize = 1_000;

/// The message an issue refused by [`DATA_ACCESS_SESSION_QUOTA`] carries.
///
/// The condition is a rate this emulator serves credential issues at, which is what the
/// modelled `ThrottlingException` states, so it carries the sentence AWS words that class
/// with. Fixed text: nothing in the request decides it.
pub const DATA_ACCESS_THROTTLED_MESSAGE: &str = "Rate exceeded";

/// Nothing was recorded because the store already holds as many live credential sets as
/// [`DATA_ACCESS_SESSION_QUOTA`] admits — what
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::try_record_data_access_session`]
/// answers instead of writing.
///
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::try_record_data_access_session`]: super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::try_record_data_access_session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtDataAccessSessionQuota;

/// One set of temporary data-access credentials as persisted.
///
/// Holds the secrets the response reported, because they are the credentials the emulator
/// itself accepts for signing until they expire: the record is what [`super::now`] measures
/// an expiry against and what ends the credential in the emulator's credential store.
#[derive(Debug, Clone)]
pub struct StoredDataAccessSession {
    /// The id of the space the credentials were issued for, and the only space they are
    /// scoped to.
    pub space_id: String,
    /// The access key id the emulator's credential store minted — the key this record is
    /// held under.
    pub access_key_id: String,
    /// The secret that key signs with, as reported once to the caller.
    pub secret_access_key: String,
    /// The session token that key presents, as reported once to the caller.
    pub session_token: String,
    /// The policy document the credentials are scoped by
    /// ([`data_access_scope_policy`]) — at most [`MAX_SESSION_POLICY_CHARACTERS`]
    /// characters, since a longer scope is refused instead of issued.
    pub scope_policy: String,
    /// The audit context the request carried, which `auditContext`'s model doc says is
    /// "recorded for auditing" and does "not affect authorization or the permissions of the
    /// credentials" — so it is held here and reaches neither the scope nor the decision.
    pub audit_context: HashMap<String, String>,
    /// When the credentials were issued.
    pub issued_at: ::aws_smithy_types::DateTime,
    /// When they stop being usable — the `expiration` the response reports.
    pub expires_at: ::aws_smithy_types::DateTime,
}

/// The policy a credential set issued for `space_arn` with `target_scopes` is scoped by.
///
/// One statement per requested scope, in the order the request listed them:
/// `DataAccessTarget`'s model doc is "One requested scope: an explicit list of actions for
/// one service … The same service may appear in more than one entry", so a group of actions
/// stays one statement rather than being merged with another group's. Every statement allows
/// its own actions on the space the credentials are for, which is the only resource they
/// confer anything over.
///
/// Rendered by the JSON serializer rather than assembled as text, so an action a caller
/// sent is a JSON string value rather than something spliced into a document.
pub fn data_access_scope_policy(space_arn: &str, target_scopes: &[DataAccessTarget]) -> String {
    let statements: Vec<::serde_json::Value> = target_scopes
        .iter()
        .map(|scope| {
            ::serde_json::json!({
                "Effect": "Allow",
                "Action": scope.actions,
                "Resource": space_arn,
            })
        })
        .collect();
    ::serde_json::json!({
        "Version": POLICY_LANGUAGE_VERSION,
        "Statement": statements,
    })
    .to_string()
}

/// Whether `policy` is larger than a credential set can carry its scope in — the condition
/// `PolicyTooLargeException` is declared for.
///
/// Measured in CHARACTERS, the unit the published limit is stated in.
#[must_use]
pub fn policy_too_large(policy: &str) -> bool {
    policy.chars().count() > MAX_SESSION_POLICY_CHARACTERS
}

/// When credentials issued at `issued_at` for `duration_seconds` expire.
///
/// Whole seconds, like every other instant this service reports ([`super::now`]). The
/// lifetime is bounded by `durationSeconds`' modelled `@range(min: 900, max: 3600)`, so the
/// sum cannot leave the range of the type; a value that somehow would leaves the expiry at
/// the instant of issue rather than wrapping.
pub fn data_access_expiry(
    issued_at: ::aws_smithy_types::DateTime,
    duration_seconds: i32,
) -> ::aws_smithy_types::DateTime {
    let seconds = issued_at
        .secs()
        .checked_add(i64::from(duration_seconds))
        .unwrap_or_else(|| issued_at.secs());
    ::aws_smithy_types::DateTime::from_secs(seconds)
}

impl super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState {
    /// Record `session` unless the store already holds as many live credential sets at `now`
    /// as [`DATA_ACCESS_SESSION_QUOTA`] admits.
    ///
    /// The bound is enforced HERE, under the same guard that writes, so a caller of this
    /// store cannot record past it and two concurrent issues cannot both read room that only
    /// one of them has. `Err(`[`AtDataAccessSessionQuota`]`)` means nothing was written: the
    /// cap refuses a new issue rather than ending a live one, so credentials already handed
    /// out keep working for the lifetime they were advertised with.
    pub fn try_record_data_access_session(
        &mut self,
        session: StoredDataAccessSession,
        now: ::aws_smithy_types::DateTime,
    ) -> Result<(), AtDataAccessSessionQuota> {
        if self.live_data_access_sessions(now) >= DATA_ACCESS_SESSION_QUOTA {
            return Err(AtDataAccessSessionQuota);
        }
        self.record_data_access_session(session);
        Ok(())
    }

    /// Hold `session` as a live credential set of the space it names, keyed by its access
    /// key id — which is what a credential presented later is recognised by and what the
    /// emulator's credential store holds it under.
    ///
    /// Private: every issue goes through [`Self::try_record_data_access_session`], so the
    /// bound above cannot be bypassed.
    fn record_data_access_session(&mut self, session: StoredDataAccessSession) {
        self.data_access_sessions
            .insert(session.access_key_id.clone(), session);
    }

    /// The credential set `access_key_id` names, while `now` has not passed its expiry.
    #[must_use]
    pub fn data_access_session(
        &self,
        access_key_id: &str,
        now: ::aws_smithy_types::DateTime,
    ) -> Option<&StoredDataAccessSession> {
        self.data_access_sessions
            .get(access_key_id)
            .filter(|stored| stored.expires_at.secs() > now.secs())
    }

    /// How many credential sets `now` has not yet made spent — what
    /// [`DATA_ACCESS_SESSION_QUOTA`] bounds.
    #[must_use]
    pub fn live_data_access_sessions(&self, now: ::aws_smithy_types::DateTime) -> usize {
        self.data_access_sessions
            .values()
            .filter(|stored| stored.expires_at.secs() > now.secs())
            .count()
    }

    /// Forget every credential set whose advertised expiry `now` has passed, and answer the
    /// access key id of each so the caller can end it in the emulator's credential store
    /// too ([`end_expired_data_access_sessions`]).
    ///
    /// The expiry is the instant the CLOCK makes a credential spent, so ending it is owed
    /// then — whether or not anybody asks for credentials again. Nothing about the request
    /// that happens to materialise it decides which records end: every record the clock has
    /// made due does.
    pub fn expire_data_access_sessions(
        &mut self,
        now: ::aws_smithy_types::DateTime,
    ) -> Vec<String> {
        let mut ended = Vec::new();
        self.data_access_sessions.retain(|access_key_id, stored| {
            let live = stored.expires_at.secs() > now.secs();
            if !live {
                ended.push(access_key_id.clone());
            }
            live
        });
        ended
    }

    /// Forget every credential set issued for the space `space_id`, as deleting that space
    /// does, and answer the access key id of each.
    ///
    /// Credentials confer access to the data of ONE space, so nothing the emulator accepts
    /// for signing may outlive the space it was scoped to.
    pub fn release_space_data_access_sessions(&mut self, space_id: &str) -> Vec<String> {
        let mut released = Vec::new();
        self.data_access_sessions.retain(|access_key_id, stored| {
            let kept = stored.space_id != space_id;
            if !kept {
                released.push(access_key_id.clone());
            }
            kept
        });
        released
    }
}

/// End every credential set the clock has made spent at `now`: drop its record here and
/// forget the credential it names in the emulator's credential store, so the emulator stops
/// accepting it for signing at exactly the `expiration` the response that issued it
/// advertised.
///
/// Called before an issue is served, because the expiry falls due at its own instant rather
/// than at the next call about the same space. It materialises only what the clock already
/// owed, so no response changes because it ran, and a credential still inside its lifetime
/// is untouched.
///
/// A credential store that no longer holds the credential is already in the state this asks
/// for, so a delete that fails is nothing to report.
pub fn end_expired_data_access_sessions(
    state: &mut super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState,
    credential_store: &::emulator_server::CredentialStore,
    now: ::aws_smithy_types::DateTime,
) {
    for access_key_id in state.expire_data_access_sessions(now) {
        let _ = credential_store.delete(&access_key_id);
    }
}

/// End every credential set issued for the space `space_id`, in the store and in the
/// emulator's credential store together — what deleting the space owes.
pub fn end_space_data_access_sessions(
    state: &mut super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState,
    credential_store: &::emulator_server::CredentialStore,
    space_id: &str,
) {
    for access_key_id in state.release_space_data_access_sessions(space_id) {
        let _ = credential_store.delete(&access_key_id);
    }
}

/// The map a state struct holds its live data-access credential sets in, keyed by access
/// key id.
pub type DataAccessSessions = HashMap<String, StoredDataAccessSession>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A space ARN standing for the one resource a credential set confers access over.
    const SPACE_ARN: &str = "arn:aws:cloudwatch:us-west-2:000000000000:space/\
                             3f6b1d20-7c84-4d1a-9f0e-2b5c8a71d4e3";

    /// A scope of the actions in `actions`, as a request carries one.
    fn scope(actions: &[&str]) -> DataAccessTarget {
        DataAccessTarget {
            actions: actions.iter().map(|action| (*action).to_string()).collect(),
        }
    }

    /// RULE: the policy is one statement per requested scope, in request order, each
    /// allowing its own actions on the space alone.
    #[test]
    fn the_scope_policy_is_one_statement_per_requested_group() {
        // Rows differ along the number of groups and the number of actions in each, which
        // is what the rule generalises over: one action, several in one group, and the
        // same service in two groups (`DataAccessTarget`'s doc: "The same service may
        // appear in more than one entry").
        for (case, scopes, expected_actions) in [
            (
                "one action",
                vec![scope(&["logs:StartQuery"])],
                vec![vec!["logs:StartQuery"]],
            ),
            (
                "several actions of one service",
                vec![scope(&["logs:StartQuery", "logs:GetQueryResults"])],
                vec![vec!["logs:StartQuery", "logs:GetQueryResults"]],
            ),
            (
                "the same service in two groups",
                vec![scope(&["logs:StartQuery"]), scope(&["logs:StopQuery"])],
                vec![vec!["logs:StartQuery"], vec!["logs:StopQuery"]],
            ),
            (
                "two services",
                vec![
                    scope(&["logs:StartQuery"]),
                    scope(&["cloudwatch:GetMetricData", "cloudwatch:ListMetrics"]),
                ],
                vec![
                    vec!["logs:StartQuery"],
                    vec!["cloudwatch:GetMetricData", "cloudwatch:ListMetrics"],
                ],
            ),
        ] {
            let policy = data_access_scope_policy(SPACE_ARN, &scopes);
            let document: ::serde_json::Value = ::serde_json::from_str(&policy)
                .unwrap_or_else(|error| panic!("case {case:?}: the policy is JSON: {error}"));
            assert_eq!(
                document["Version"].as_str(),
                Some(POLICY_LANGUAGE_VERSION),
                "case {case:?}: expected the policy language version, got {document}"
            );
            let statements = document["Statement"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the policy carries statements"));
            assert_eq!(
                statements.len(),
                expected_actions.len(),
                "case {case:?}: expected {} statements, got {document}",
                expected_actions.len()
            );
            for (index, actions) in expected_actions.iter().enumerate() {
                let statement = &statements[index];
                let rendered: Vec<&str> = statement["Action"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: statement {index} lists actions"))
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .collect();
                assert_eq!(
                    (
                        statement["Effect"].as_str(),
                        rendered,
                        statement["Resource"].as_str()
                    ),
                    (Some("Allow"), actions.clone(), Some(SPACE_ARN)),
                    "case {case:?}: statement {index} must allow its own actions on the space, \
                     got {statement}"
                );
            }
        }
    }

    /// RULE: a scope whose policy is longer than the published limit is too large, and one
    /// at or under it is not — checked at the bound and one character past it.
    #[test]
    fn a_policy_past_the_published_limit_is_too_large() {
        // Rows differ along the size of the rendered document: the smallest scope, one
        // exactly at the limit, one a character past it, and the largest scope the model
        // admits (`targetScopes`' `@length(min: 1, max: 10)` of `DataAccessActionList`'s
        // `@length(min: 1, max: 50)`).
        let smallest = data_access_scope_policy(SPACE_ARN, &[scope(&["logs:StartQuery"])]);
        assert!(
            !policy_too_large(&smallest),
            "case \"the smallest scope\": expected a policy that fits, got {} characters",
            smallest.chars().count()
        );

        let at_the_limit = "p".repeat(MAX_SESSION_POLICY_CHARACTERS);
        let past_the_limit = "p".repeat(MAX_SESSION_POLICY_CHARACTERS + 1);
        for (case, policy, expected) in [
            ("exactly at the limit", at_the_limit, false),
            ("one character past the limit", past_the_limit, true),
        ] {
            assert_eq!(
                policy_too_large(&policy),
                expected,
                "case {case:?}: expected too_large={expected} for {} characters",
                policy.chars().count()
            );
        }

        let actions: Vec<String> = (0..50)
            .map(|index| format!("logs:StartQueryVariant{index}"))
            .collect();
        let largest = data_access_scope_policy(SPACE_ARN, &vec![DataAccessTarget { actions }; 10]);
        assert!(
            policy_too_large(&largest),
            "case \"the largest scope the model admits\": expected a policy past the limit, got \
             {} characters",
            largest.chars().count()
        );
    }

    /// RULE: the expiry is the lifetime past the issue, at whole-second resolution, at both
    /// published bounds of `durationSeconds`' `@range(min: 900, max: 3600)` and at the
    /// default.
    #[test]
    fn the_expiry_is_the_lifetime_past_the_issue() {
        let issued_at = ::aws_smithy_types::DateTime::from_secs(1_700_000_000);
        for duration_seconds in [900, 1800, DEFAULT_DATA_ACCESS_DURATION_SECONDS] {
            let expires_at = data_access_expiry(issued_at, duration_seconds);
            let expected = issued_at.secs() + i64::from(duration_seconds);
            assert_eq!(
                (expires_at.secs(), expires_at.subsec_nanos()),
                (expected, 0),
                "case {duration_seconds}: expected {expected} whole seconds, got {expires_at:?}"
            );
        }
    }

    /// A credential set of `space_id` under `access_key_id`, live for `duration_seconds`
    /// from `issued_at`.
    fn session(
        access_key_id: &str,
        space_id: &str,
        issued_at: ::aws_smithy_types::DateTime,
        duration_seconds: i32,
    ) -> StoredDataAccessSession {
        StoredDataAccessSession {
            space_id: space_id.to_string(),
            access_key_id: access_key_id.to_string(),
            secret_access_key: "secret".to_string(),
            session_token: "token".to_string(),
            scope_policy: data_access_scope_policy(SPACE_ARN, &[scope(&["logs:StartQuery"])]),
            audit_context: HashMap::new(),
            issued_at,
            expires_at: data_access_expiry(issued_at, duration_seconds),
        }
    }

    /// RULE: a recorded credential set resolves by its access key id until its expiry, an
    /// issue drops what the clock has spent, and deleting the space releases what was issued
    /// for it.
    #[test]
    fn a_recorded_credential_set_lives_until_its_expiry_and_dies_with_its_space() {
        let mut state = super::super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::default();
        let issued_at = ::aws_smithy_types::DateTime::from_secs(1_700_000_000);
        // Rows differ along the lifetime asked for and the space issued for, so the two
        // records fall due at different instants and belong to different spaces.
        for (access_key_id, space_id, duration_seconds) in [
            ("EMULATORASSHORT", "space-one", 900),
            ("EMULATORASLONG", "space-two", 3600),
        ] {
            state
                .try_record_data_access_session(
                    session(access_key_id, space_id, issued_at, duration_seconds),
                    issued_at,
                )
                .unwrap_or_else(|refused| {
                    panic!(
                        "case {access_key_id:?}: expected the issue to be recorded, got \
                         {refused:?}"
                    )
                });
        }
        assert_eq!(
            state.live_data_access_sessions(issued_at),
            2,
            "case \"both issued\": expected two live records, got {}",
            state.live_data_access_sessions(issued_at)
        );

        let past_short = data_access_expiry(issued_at, 900);
        assert!(
            state
                .data_access_session("EMULATORASSHORT", past_short)
                .is_none(),
            "case \"the spent credential set\": expected no record, got {:?}",
            state.data_access_session("EMULATORASSHORT", past_short)
        );
        let ended = state.expire_data_access_sessions(past_short);
        assert_eq!(
            ended,
            vec!["EMULATORASSHORT".to_string()],
            "case \"an issue past one expiry\": expected the spent key alone, got {ended:?}"
        );
        let live = state
            .data_access_session("EMULATORASLONG", past_short)
            .map(|stored| stored.space_id.clone());
        assert_eq!(
            live,
            Some("space-two".to_string()),
            "case \"the credential set still inside its lifetime\": expected its record, got \
             {live:?}"
        );

        let released = state.release_space_data_access_sessions("space-two");
        assert_eq!(
            released,
            vec!["EMULATORASLONG".to_string()],
            "case \"the space is deleted\": expected its credential set released, got {released:?}"
        );
        assert_eq!(
            state.live_data_access_sessions(issued_at),
            0,
            "case \"the space is deleted\": expected no live record, got {}",
            state.live_data_access_sessions(issued_at)
        );
    }

    /// RULE: the store records no more live credential sets than its bound admits, refuses
    /// the NEW issue rather than ending a live one, and takes an issue again as soon as the
    /// clock has made room.
    #[test]
    fn the_bound_refuses_a_further_issue_and_keeps_every_live_one() {
        let mut state = super::super::ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::default();
        let issued_at = ::aws_smithy_types::DateTime::from_secs(1_700_000_000);
        // The first set falls due before the rest, so the clock makes room for exactly one
        // further issue later on.
        for index in 0..DATA_ACCESS_SESSION_QUOTA {
            let duration_seconds = if index == 0 {
                MIN_DATA_ACCESS_DURATION_SECONDS
            } else {
                MAX_DATA_ACCESS_DURATION_SECONDS
            };
            state
                .try_record_data_access_session(
                    session(
                        &format!("EMULATORAS{index}"),
                        "space-one",
                        issued_at,
                        duration_seconds,
                    ),
                    issued_at,
                )
                .unwrap_or_else(|refused| {
                    panic!("case \"issue {index}\": expected room in the store, got {refused:?}")
                });
        }
        assert_eq!(
            state.try_record_data_access_session(
                session(
                    "EMULATORASOVER",
                    "space-one",
                    issued_at,
                    MAX_DATA_ACCESS_DURATION_SECONDS
                ),
                issued_at,
            ),
            Err(AtDataAccessSessionQuota),
            "case \"one issue past the bound\": expected the new issue to be refused"
        );
        assert_eq!(
            state.live_data_access_sessions(issued_at),
            DATA_ACCESS_SESSION_QUOTA,
            "case \"one issue past the bound\": every live set must be kept, got {}",
            state.live_data_access_sessions(issued_at)
        );
        assert!(
            state.data_access_session("EMULATORAS0", issued_at).is_some(),
            "case \"one issue past the bound\": the first set must be untouched"
        );

        // Once the clock has spent the shortest-lived set, one further issue is admitted
        // again — and only one.
        let past_first = data_access_expiry(issued_at, MIN_DATA_ACCESS_DURATION_SECONDS);
        assert_eq!(
            state.expire_data_access_sessions(past_first),
            vec!["EMULATORAS0".to_string()],
            "case \"past the first expiry\": expected the spent set alone to end"
        );
        for (case, expected) in [
            ("the issue the clock made room for", Ok(())),
            (
                "one issue past the bound again",
                Err(AtDataAccessSessionQuota),
            ),
        ] {
            let actual = state.try_record_data_access_session(
                session(
                    &format!("EMULATORASNEW{case}"),
                    "space-one",
                    past_first,
                    MAX_DATA_ACCESS_DURATION_SECONDS,
                ),
                past_first,
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
