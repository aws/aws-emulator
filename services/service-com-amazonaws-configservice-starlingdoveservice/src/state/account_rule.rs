// SPDX-License-Identifier: CC0-1.0

//! The Config Managed rules whose subject is the ACCOUNT rather than a recorded
//! resource, and how each of them places the account at a verdict.
//!
//! Some managed rules check a resource the configuration recorder captured — those are
//! evaluated from that resource's configuration item (`super::recording`) — and some
//! check the ACCOUNT's own configuration, which no recorder discovers as a resource.
//! Config evaluates the second kind against its account pseudo-resource type
//! ([`ACCOUNT_RESOURCE_TYPE`]) with the account id as the resource id, which is the same
//! subject an externally delivered evaluation names, and the rule's own
//! `InputParameters` are the requirements it checks that configuration against.
//!
//! What the account's configuration IS comes from the owning service's own API — one
//! observation per service, composed here ([`observed_account`]) — so a rule judges what
//! another service reported and nothing else: the part of the configuration that could not
//! be read leaves the rules about it unevaluated rather than placed at a made-up verdict.

use std::time::SystemTime;

use super::{
    managed_rule_identifier, ComplianceChange, ConfigRuleEntry, EvaluationResultEntry,
    ObservedIamAccount, ObservedTrail, EVALUATION_MODE_DETECTIVE,
    EXTERNAL_EVALUATION_RESOURCE_TYPE,
};
use crate::ops::types::Source;

/// The resource type Config evaluates an ACCOUNT-wide rule against: the account
/// pseudo-resource type, the same one an externally delivered evaluation may name
/// ([`EXTERNAL_EVALUATION_RESOURCE_TYPE`]) — "Config accepts `AWS::::Account` resource
/// type".
pub const ACCOUNT_RESOURCE_TYPE: &str = EXTERNAL_EVALUATION_RESOURCE_TYPE;

/// One AWS managed Config rule whose subject is the ACCOUNT.
///
/// `identifier` is the `SourceIdentifier` Config knows the managed rule by
/// (`Source$SourceIdentifier`: "For Config Managed rules, a predefined identifier from a
/// list"), and `verdict` the `ComplianceType` it places the account at — computed from
/// the account's OBSERVED configuration and the rule's own input parameters, never from a
/// request.
///
/// A verdict answers `None` where the part of the account's configuration this rule checks
/// could not be OBSERVED: Config then records nothing for the rule, so the account keeps
/// reporting `INSUFFICIENT_DATA` rather than a verdict nothing computed.
pub struct AccountManagedRule {
    /// See [`AccountManagedRule`].
    pub identifier: &'static str,
    /// See [`AccountManagedRule`].
    pub verdict: fn(&ObservedAccount, &RuleParameters) -> Option<&'static str>,
}

/// The account-wide AWS managed rules this emulator evaluates.
///
/// Source: the Config managed-rule list (the `SourceIdentifier` values
/// `Source$SourceIdentifier` refers to), which is also where each rule's own description
/// and parameter names come from; the per-entry function below quotes it. A managed rule
/// NOT listed here and not in [`super::recording::MANAGED_RULES`] is left unevaluated —
/// Config records no verdict for it rather than a made-up one.
pub const ACCOUNT_MANAGED_RULES: [AccountManagedRule; 2] = [
    AccountManagedRule {
        identifier: "IAM_PASSWORD_POLICY",
        verdict: iam_password_policy_verdict,
    },
    AccountManagedRule {
        identifier: "CLOUD_TRAIL_ENABLED",
        verdict: cloud_trail_enabled_verdict,
    },
];

/// The ACCOUNT's own configuration, as the account-wide managed rules are evaluated
/// against it: ONE observation per service that owns part of it.
///
/// A member is `None` where that service could not be read at all, which is not the same as
/// a service that reported nothing — an account with no password policy and an account with
/// no trail are both configurations a rule judges.
#[derive(Debug, Clone, Default)]
pub struct ObservedAccount {
    /// What IAM reports about this account ([`super::observed_iam::observed_iam_account`]).
    pub iam: Option<ObservedIamAccount>,
    /// The trails CloudTrail reports for this account
    /// ([`super::observed_cloudtrail::observed_trails`]).
    pub trails: Option<Vec<ObservedTrail>>,
}

/// Read the account's own configuration from the services that OWN it, as of this pass.
///
/// Each read stands alone, so a service that could not be read leaves the rules that judge
/// ITS part of the configuration unevaluated and the rest evaluated as usual. Made only
/// where this account holds a rule whose subject is the account
/// ([`super::ComAmazonawsConfigserviceStarlingdoveserviceState::evaluates_account_rules`]):
/// a rule nothing asks about costs no read.
pub async fn observed_account(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ObservedAccount {
    ObservedAccount {
        iam: super::observed_iam::observed_iam_account(callback, region).await,
        trails: super::observed_cloudtrail::observed_trails(callback, region).await,
    }
}

/// The account-wide managed rule `source` names, when this emulator evaluates it.
///
/// Which managed rule a source names at all is [`managed_rule_identifier`]'s, so a
/// Custom Lambda or Custom Policy rule — which carries the caller's own implementation —
/// names none.
pub fn account_managed_rule_of(source: &Source) -> Option<&'static AccountManagedRule> {
    let identifier = managed_rule_identifier(source)?;
    ACCOUNT_MANAGED_RULES
        .iter()
        .find(|managed| managed.identifier == identifier)
}

/// The requirements ONE rule checks, as its `InputParameters` state them
/// (`ConfigRule$InputParameters`: "A string, in JSON format, that is passed to the Config
/// rule Lambda function").
///
/// Config takes a rule's parameters as a JSON object whose values are written as text,
/// and a client may equally send them as JSON numbers and booleans, so each value is read
/// through BOTH spellings here. A parameter the rule does not name is not a requirement:
/// the rule checks the requirements it was given, so an absent one is not checked at all.
#[derive(Debug, Clone, Default)]
pub struct RuleParameters {
    parameters: ::serde_json::Map<String, ::serde_json::Value>,
}

impl RuleParameters {
    /// The parameters `input_parameters` states, or none at all when the rule carries
    /// none or carries something that is no JSON object.
    pub fn of(input_parameters: Option<&str>) -> Self {
        let parameters = input_parameters
            .map(str::trim)
            .filter(|parameters| !parameters.is_empty())
            .and_then(|parameters| ::serde_json::from_str::<::serde_json::Value>(parameters).ok())
            .and_then(|parsed| match parsed {
                ::serde_json::Value::Object(parameters) => Some(parameters),
                _ => None,
            })
            .unwrap_or_default();
        Self { parameters }
    }

    /// The boolean the parameter `name` requires, or `None` when the rule does not name
    /// it (or names it as something that is no boolean).
    pub fn flag(&self, name: &str) -> Option<bool> {
        match self.parameters.get(name)? {
            ::serde_json::Value::Bool(flag) => Some(*flag),
            ::serde_json::Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// The number the parameter `name` requires, read the same two ways as
    /// [`Self::flag`].
    pub fn number(&self, name: &str) -> Option<i64> {
        match self.parameters.get(name)? {
            ::serde_json::Value::Number(number) => number.as_i64(),
            ::serde_json::Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// The text the parameter `name` requires, or `None` when the rule does not name it
    /// (or names it as something with no text of its own, a list or an object).
    ///
    /// A value a client sent as a JSON number or boolean reads as the text it was written
    /// as, for the same reason [`Self::flag`] reads both spellings: Config states a rule's
    /// parameters as text, and a client may equally send them as JSON scalars.
    pub fn text(&self, name: &str) -> Option<String> {
        match self.parameters.get(name)? {
            ::serde_json::Value::String(text) => Some(text.clone()),
            ::serde_json::Value::Number(number) => Some(number.to_string()),
            ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }
}

/// The parameter names `IAM_PASSWORD_POLICY` takes, as the managed rule's own
/// documentation spells them.
const REQUIRE_UPPERCASE_CHARACTERS: &str = "RequireUppercaseCharacters";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const REQUIRE_LOWERCASE_CHARACTERS: &str = "RequireLowercaseCharacters";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const REQUIRE_SYMBOLS: &str = "RequireSymbols";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const REQUIRE_NUMBERS: &str = "RequireNumbers";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const MINIMUM_PASSWORD_LENGTH: &str = "MinimumPasswordLength";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const PASSWORD_REUSE_PREVENTION: &str = "PasswordReusePrevention";
/// See [`REQUIRE_UPPERCASE_CHARACTERS`].
const MAX_PASSWORD_AGE: &str = "MaxPasswordAge";

/// `IAM_PASSWORD_POLICY` — "Checks if the account password policy for users meets the
/// specified requirements indicated in the parameters": COMPLIANT exactly while the
/// account HAS a password policy and that policy meets every requirement the rule's
/// parameters state.
///
/// An account with no password policy meets no requirement at all, so it is
/// `NON_COMPLIANT` whatever the rule asks for. A requirement the rule does not state is
/// not checked: the rule checks "the specified requirements". A requirement that asks for
/// a character class demands the policy require it; one that asks for a minimum length or
/// a reuse count demands at least that much; and `MaxPasswordAge` demands an expiry no
/// longer than the one it names, which a policy whose passwords never expire does not
/// meet. An account whose IAM configuration could not be READ is judged by nothing at all.
fn iam_password_policy_verdict(
    account: &ObservedAccount,
    parameters: &RuleParameters,
) -> Option<&'static str> {
    let Some(policy) = account.iam.as_ref()?.password_policy.as_ref() else {
        return Some(super::COMPLIANCE_TYPE_NON_COMPLIANT);
    };
    // A requirement asking for a character class is met when the policy requires it; one
    // explicitly asking for `false` demands nothing.
    let requires = |name: &str, required_by_policy: bool| {
        parameters
            .flag(name)
            .is_none_or(|asked| !asked || required_by_policy)
    };
    let at_least = |name: &str, held: Option<i32>| {
        parameters
            .number(name)
            .is_none_or(|asked| held.is_some_and(|held| i64::from(held) >= asked))
    };
    let at_most = |name: &str, held: Option<i32>| {
        parameters
            .number(name)
            .is_none_or(|asked| held.is_some_and(|held| i64::from(held) <= asked))
    };
    Some(super::recording::verdict(
        requires(
            REQUIRE_UPPERCASE_CHARACTERS,
            policy.require_uppercase_characters,
        )
            && requires(
                REQUIRE_LOWERCASE_CHARACTERS,
                policy.require_lowercase_characters,
            )
            && requires(REQUIRE_SYMBOLS, policy.require_symbols)
            && requires(REQUIRE_NUMBERS, policy.require_numbers)
            && at_least(MINIMUM_PASSWORD_LENGTH, policy.minimum_password_length)
            && at_least(PASSWORD_REUSE_PREVENTION, policy.password_reuse_prevention)
            && at_most(MAX_PASSWORD_AGE, policy.max_password_age),
    ))
}

/// The parameter names `CLOUD_TRAIL_ENABLED` takes, as the managed rule's own
/// documentation spells them: the delivery destinations it can be asked to check the
/// account's trail uses.
const S3_BUCKET_NAME: &str = "s3BucketName";
/// See [`S3_BUCKET_NAME`].
const SNS_TOPIC_ARN: &str = "snsTopicArn";
/// See [`S3_BUCKET_NAME`].
const CLOUD_WATCH_LOGS_LOG_GROUP_ARN: &str = "cloudWatchLogsLogGroupArn";

/// `CLOUD_TRAIL_ENABLED` — "Checks if an CloudTrail trail is enabled in your Amazon Web
/// Services account": COMPLIANT exactly while the account holds a trail that is LOGGING,
/// and that delivers to each destination the rule's parameters name.
///
/// "The rule is NON_COMPLIANT if a trail is not enabled", so an account with no trail at
/// all and an account whose every trail has stopped logging are both non-compliant. A
/// destination parameter the rule does not state is not checked — the rule "optionally
/// checks" them — and one it does state has to be the destination of the SAME trail that is
/// logging, because that is the trail whose being enabled the rule is about.
fn cloud_trail_enabled_verdict(
    account: &ObservedAccount,
    parameters: &RuleParameters,
) -> Option<&'static str> {
    let trails = account.trails.as_deref()?;
    // A destination the rule names has to be the one the trail delivers to; one it does not
    // name demands nothing, and neither does one named as nothing at all.
    let delivers_to = |name: &str, held: Option<&str>| {
        parameters
            .text(name)
            .map(|asked| asked.trim().to_string())
            .filter(|asked| !asked.is_empty())
            .is_none_or(|asked| held.is_some_and(|held| held == asked))
    };
    Some(super::recording::verdict(trails.iter().any(|trail| {
        trail.is_logging
            && delivers_to(S3_BUCKET_NAME, trail.s3_bucket_name.as_deref())
            && delivers_to(SNS_TOPIC_ARN, trail.sns_topic_arn.as_deref())
            && delivers_to(
                CLOUD_WATCH_LOGS_LOG_GROUP_ARN,
                trail.cloud_watch_logs_log_group_arn.as_deref(),
            )
    })))
}

impl ConfigRuleEntry {
    /// Evaluate this rule against the ACCOUNT `account` as of `now`, recording the verdict
    /// it places the account at and answering the compliance CHANGE that verdict is, where
    /// it changed.
    ///
    /// Only a rule this emulator has an account-wide implementation for
    /// ([`account_managed_rule_of`]) is evaluated here; anything else records nothing, so a
    /// rule whose logic is not here keeps reporting `INSUFFICIENT_DATA` rather than a
    /// synthesised verdict. So does a rule whose part of the account's configuration this
    /// pass could not OBSERVE — its verdict answers `None` — and such a rule is left still
    /// owing its pass, so the next pass evaluates it once that read answers. The result is
    /// recorded through the rule's own writer ([`ConfigRuleEntry::record_verdict`]), which is
    /// what keeps a rule's results bounded and what decides whether the verdict moved, and
    /// the pass the rule was owed is settled once it has run.
    pub(super) fn evaluate_against_account(
        &mut self,
        account: &ObservedAccount,
        now: SystemTime,
    ) -> Option<ComplianceChange> {
        let managed = account_managed_rule_of(&self.source)?;
        let parameters = RuleParameters::of(self.input_parameters.as_deref());
        let verdict = (managed.verdict)(account, &parameters)?;
        // The instant Config invoked the pass this rule is owed, which the result reports
        // as the rule's invocation time; a pass no write asked for is invoked at the
        // instant it runs.
        let invoked_at = self.evaluation.invoked_at.unwrap_or(now);
        let result = EvaluationResultEntry {
            config_rule_invoked_time: invoked_at,
            result_recorded_time: now,
            ..EvaluationResultEntry::new(
                ACCOUNT_RESOURCE_TYPE,
                ::emulator_server::emulator_account_id(),
                EVALUATION_MODE_DETECTIVE,
                verdict,
                // The event that triggered the evaluation is the invocation itself: an
                // account-wide rule is evaluated periodically rather than on a recorded
                // configuration change.
                invoked_at,
            )
        };
        let change = self.record_verdict(result);
        self.evaluation.settle(now);
        change
    }
}

impl super::ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Whether this account holds an ACTIVE rule whose subject is the ACCOUNT, so that a
    /// pass has to read the account's own configuration to evaluate it.
    ///
    /// Asked before that read is made: an account holding no such rule is not read at all,
    /// which is what keeps a pass from putting a question nothing would use the answer to.
    pub fn evaluates_account_rules(&self) -> bool {
        self.config_rules.values().any(|rule| {
            rule.state == super::CONFIG_RULE_STATE_ACTIVE
                && account_managed_rule_of(&rule.source).is_some()
        })
    }
}
