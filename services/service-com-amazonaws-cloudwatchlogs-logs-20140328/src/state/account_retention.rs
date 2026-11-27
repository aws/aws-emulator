// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-LEVEL RETENTION POLICIES this account holds: what one `RETENTION_POLICY`
//! document states, which log groups each one applies to, the retention a log group ends up
//! with once they have been resolved against its own setting, and the impact a dry run of a
//! change to them reports.
//!
//! `PutAccountPolicy` states the rule this module serves: "An account-level retention policy
//! overrides the retention setting of the log group", and "when more than one retention policy
//! applies to a log group, the policy with the longest retention period is used". So a log
//! group's retention is DERIVED here, from the policies the account holds and the group's own
//! setting, rather than written onto the group by a put: every reader of a retention — the
//! listing that reports it and the ingestion window that expires events against it — resolves
//! it through [`effective_retention`] and so cannot disagree with another.

use super::*;

/// What one `RETENTION_POLICY` document states: "A retention policy must include the
/// following attributes in a JSON block: `retentionInDays` [...] `enforcementMode`"
/// (`PutAccountPolicy$policyDocument`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRetention {
    /// `retentionInDays` — "The number of days to retain log events", or `None` for the
    /// documented `NEVER_EXPIRE`: "To retain log events indefinitely, specify the string
    /// NEVER_EXPIRE."
    pub retention_in_days: Option<i32>,
    /// `enforcementMode` — "Specifies which log groups the policy applies to", one of
    /// [`ENFORCEMENT_MODE_VALUES`].
    pub enforcement_mode: String,
}

impl AccountRetention {
    /// This retention as a `RetentionValue` — what `LogGroupImpact$currentRetention` and
    /// `$proposedRetention` report, "in days" or [`ACCOUNT_RETENTION_NEVER_EXPIRE`].
    pub fn reported(retention_in_days: Option<i32>) -> String {
        match retention_in_days {
            Some(days) => days.to_string(),
            None => ACCOUNT_RETENTION_NEVER_EXPIRE.to_string(),
        }
    }

    /// Whether this retention reaches a log group the policy stating it selects.
    ///
    /// `enforcementMode` is what narrows it: `RETROACTIVE` "applies the policy to every log
    /// group that matches the selection criteria, including log groups that existed before
    /// you created the policy", while `NEW_ONLY` "applies the policy only to log groups that
    /// are created after you create the policy. Log groups that already existed when you
    /// created the policy keep their own retention period." A group whose creation the clock
    /// cannot separate from the policy's is one that already existed, which is the reading
    /// that leaves a pre-existing group's own retention alone.
    fn reaches(&self, group: &LogGroupRecord, policy_created_time_ms: i64) -> bool {
        self.enforcement_mode != ENFORCEMENT_MODE_NEW_ONLY
            || group.creation_time_ms > policy_created_time_ms
    }
}

/// How long a retention is, as the policies applying to one log group are compared: "the
/// policy with the longest retention period is used. A policy that specifies NEVER_EXPIRE is
/// treated as the longest retention period."
fn retention_length(retention_in_days: Option<i32>) -> (u8, i32) {
    match retention_in_days {
        None => (1, 0),
        Some(days) => (0, days),
    }
}

/// The retention one account-level retention policy puts on the log groups it selects, and
/// WHEN that retention takes effect.
///
/// A put does not change what a policy does straight away: "A retention policy takes effect
/// 24 hours after you create it. If you update a policy, the new retention period takes
/// effect 24 hours after the update, and the previous retention period stays in effect until
/// then." So the policy carries both retentions — the one its current document states and
/// the one that is still in effect — and the instant the clock owes the switch
/// ([`ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS`]). Nothing has to be materialised: which of
/// the two applies is a question about the instant it is asked at.
#[derive(Debug, Clone)]
pub struct RetentionEffect {
    /// The retention this policy's current `policyDocument` states, in effect from
    /// [`RetentionEffect::effective_at_ms`].
    pub stated: AccountRetention,
    /// The instant [`RetentionEffect::stated`] takes effect.
    pub effective_at_ms: i64,
    /// The retention still in effect until then — the one the policy had in effect when the
    /// put replaced it. `None` for a policy nothing had in effect yet, which is every
    /// newly created one.
    pub superseded: Option<AccountRetention>,
}

impl RetentionEffect {
    /// What a put of `stated` at `now_ms` leaves on a policy that held `held` before it: the
    /// stated retention falls due a span from now, and whatever was in effect at the instant
    /// of the put stays in effect until then.
    pub fn put(stated: AccountRetention, now_ms: i64, held: Option<&RetentionEffect>) -> Self {
        Self {
            stated,
            effective_at_ms: now_ms.saturating_add(ACCOUNT_RETENTION_POLICY_EFFECT_DELAY_MS),
            superseded: held
                .and_then(|held| held.in_effect(now_ms))
                .cloned(),
        }
    }

    /// The retention this policy puts on the groups it selects AT `now_ms`, or `None` while
    /// a newly created policy has yet to take effect.
    pub fn in_effect(&self, now_ms: i64) -> Option<&AccountRetention> {
        if now_ms >= self.effective_at_ms {
            Some(&self.stated)
        } else {
            self.superseded.as_ref()
        }
    }
}

/// Judge a `RETENTION_POLICY` document, answering the message to refuse it with or the
/// retention it states.
///
/// The attributes are the two `PutAccountPolicy$policyDocument` lists for one, and both are
/// required ("A retention policy must include the following attributes in a JSON block"):
/// `retentionInDays`, which is "the number of days to retain log events" out of the
/// documented catalogue ([`account_retention_in_days_values`]) or the string `NEVER_EXPIRE`,
/// and `enforcementMode`, one of [`ENFORCEMENT_MODE_VALUES`]. A document whose FORM the
/// service cannot read is refused with its fixed text for that
/// ([`MALFORMED_POLICY_DOCUMENT_MESSAGE`]); a readable document stating a value outside a
/// catalogue is refused by that catalogue's own sentence.
pub fn check_retention_policy_document(document: &str) -> Result<AccountRetention, String> {
    let malformed = || MALFORMED_POLICY_DOCUMENT_MESSAGE.to_string();
    let members = policy_object(document).map_err(|_| malformed())?;
    let stated = members
        .get(ACCOUNT_RETENTION_DAYS_MEMBER)
        .ok_or_else(malformed)?;
    let valid_days = account_retention_in_days_values();
    // The one member of the document that is written as EITHER a number or a string, which
    // is how a retention that never expires is stated.
    let retention_in_days = match stated {
        serde_json::Value::String(never) if never == ACCOUNT_RETENTION_NEVER_EXPIRE => None,
        serde_json::Value::Number(days) => {
            let days = days
                .as_i64()
                .and_then(|days| i32::try_from(days).ok())
                .filter(|days| valid_days.contains(days))
                .ok_or_else(|| invalid_retention_message(&valid_days))?;
            Some(days)
        }
        _ => return Err(invalid_retention_message(&valid_days)),
    };
    let enforcement_mode = members
        .get(ACCOUNT_RETENTION_ENFORCEMENT_MODE_MEMBER)
        .ok_or_else(malformed)?
        .as_str()
        .filter(|mode| ENFORCEMENT_MODE_VALUES.contains(mode))
        .ok_or_else(|| {
            invalid_value_message(
                ACCOUNT_RETENTION_ENFORCEMENT_MODE_MEMBER,
                ENFORCEMENT_MODE_VALUES,
            )
        })?
        .to_string();
    Ok(AccountRetention {
        retention_in_days,
        enforcement_mode,
    })
}

/// How a `retentionInDays` outside what a retention policy document may state is refused:
/// the service's one sentence for a value outside a documented catalogue
/// ([`invalid_value_message`]), listing the retentions this document admits and the string
/// that stands for one that never expires.
fn invalid_retention_message(valid_days: &[i32]) -> String {
    let admitted: Vec<String> = valid_days
        .iter()
        .map(i32::to_string)
        .chain([ACCOUNT_RETENTION_NEVER_EXPIRE.to_string()])
        .collect();
    invalid_value_message(ACCOUNT_RETENTION_DAYS_MEMBER, admitted)
}

/// One account-level retention policy as the retention computation reads it: the stored
/// policy, and the retention it has in effect at the instant being asked about.
#[derive(Debug, Clone)]
pub struct RetentionRule<'a> {
    /// The stored policy — which log groups it selects is its `selectionCriteria`
    /// ([`account_policy_selects_log_group`]), and when it was created is what its
    /// `NEW_ONLY` enforcement mode is measured against.
    pub policy: &'a AccountPolicyRecord,
    /// The retention it puts on the groups it selects.
    pub retention: AccountRetention,
}

impl RetentionRule<'_> {
    /// Whether this rule applies to one log group: its policy selects the group and its
    /// enforcement mode reaches it.
    fn applies_to(&self, group: &LogGroupRecord) -> bool {
        account_policy_selects_log_group(self.policy, group)
            && self.retention.reaches(group, self.policy.created_time_ms)
    }
}

/// Every account-level retention policy that has a retention IN EFFECT at `now_ms`.
///
/// Read once per request and handed to [`effective_retention_of`] for each log group, so a
/// listing resolves every group's retention against one reading of the account's policies.
pub fn retention_rules_in_effect(
    account_policies: &BTreeMap<(String, String), AccountPolicyRecord>,
    now_ms: i64,
) -> Vec<RetentionRule<'_>> {
    account_policies
        .values()
        .filter(|policy| policy.policy_type == POLICY_TYPE_RETENTION)
        .filter_map(|policy| {
            let retention = policy.retention.as_ref()?.in_effect(now_ms)?.clone();
            Some(RetentionRule { policy, retention })
        })
        .collect()
}

/// The retention one log group's log events are kept for, and the account-level policy that
/// decided it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveRetention {
    /// `LogGroup$retentionInDays`, which an account-level policy reports "the retention
    /// period from that policy instead of the retention period set on the log group".
    /// `None` when the log events never expire — which is a group with no retention of its
    /// own that no policy applies to, and equally one whose policy states `NEVER_EXPIRE`:
    /// "If the policy specifies that log events never expire, retentionInDays is not
    /// returned."
    pub retention_in_days: Option<i32>,
    /// `LogGroup$retentionPolicyName` — "The name of the account-level retention policy that
    /// determines the retention period of this log group. This field is returned only when
    /// an account-level retention policy applies to the log group."
    pub policy_name: Option<String>,
}

/// The retention of one log group, resolved against the account's retention policies at
/// `now_ms`. See [`effective_retention_of`] for the rule.
pub fn effective_retention(
    account_policies: &BTreeMap<(String, String), AccountPolicyRecord>,
    group: &LogGroupRecord,
    now_ms: i64,
) -> EffectiveRetention {
    effective_retention_of(&retention_rules_in_effect(account_policies, now_ms), group)
}

/// The retention of one log group under `rules`.
///
/// "An account-level retention policy overrides the retention setting of the log group", so
/// the group's own `retentionInDays` is what it is kept for only while no policy applies to
/// it. When several do, "the policy with the longest retention period is used. A policy that
/// specifies NEVER_EXPIRE is treated as the longest retention period. [...] If two policies
/// that apply to the same log group specify the same retention period, the policy that was
/// created first is the one reported in retentionPolicyName."
pub fn effective_retention_of(
    rules: &[RetentionRule<'_>],
    group: &LogGroupRecord,
) -> EffectiveRetention {
    let deciding = rules
        .iter()
        .filter(|rule| rule.applies_to(group))
        // The longest retention first, then the policy created first, and between two
        // created at the same instant the one the account holds first by name — so one set
        // of policies decides one group's retention the same way on every read.
        .min_by_key(|rule| {
            (
                std::cmp::Reverse(retention_length(rule.retention.retention_in_days)),
                rule.policy.created_time_ms,
                rule.policy.policy_name.clone(),
            )
        });
    match deciding {
        Some(rule) => EffectiveRetention {
            retention_in_days: rule.retention.retention_in_days,
            policy_name: Some(rule.policy.policy_name.clone()),
        },
        None => EffectiveRetention {
            retention_in_days: group.retention_in_days,
            policy_name: None,
        },
    }
}

/// The change to the account's retention policies a dry run previews: "Before you create,
/// update, or delete a retention policy, you can specify true for dryRun to preview how many
/// log groups would have a longer, shorter, or unchanged retention period."
#[derive(Debug)]
pub enum RetentionPolicyChange<'a> {
    /// What `PutAccountPolicy` would store, as the put built it.
    Put(&'a AccountPolicyRecord),
    /// The stored policy `DeleteAccountPolicy` would remove.
    Delete(&'a AccountPolicyRecord),
}

impl<'a> RetentionPolicyChange<'a> {
    /// The policy the change is about, stored or proposed.
    fn policy(&self) -> &'a AccountPolicyRecord {
        match self {
            Self::Put(policy) | Self::Delete(policy) => policy,
        }
    }

    /// The retention whose enforcement mode decides which log groups are in the scope of the
    /// change: the one the put states, and for a delete the one the stored policy has in
    /// effect — or, while it has none in effect yet, the one it states.
    fn scope_retention(&self, now_ms: i64) -> Option<&AccountRetention> {
        let policy = self.policy().retention.as_ref()?;
        match self {
            Self::Put(_) => Some(&policy.stated),
            Self::Delete(_) => Some(policy.in_effect(now_ms).unwrap_or(&policy.stated)),
        }
    }
}

/// The estimated impact of `change` on the account's log groups, as `dryRunResult` reports
/// it.
///
/// `DryRunResult` states what the analysis is: "The analysis compares the effective
/// retention period of each log group, which accounts for all account-level retention
/// policies that apply to it, before and after the change. Log groups in the Delivery class
/// are not included." The change is the only thing supposed: every other policy counts for
/// what it has in effect, while the previewed one counts "as if the policy were already in
/// effect", which is what a dry run is for.
pub fn retention_dry_run_result(
    account_policies: &BTreeMap<(String, String), AccountPolicyRecord>,
    log_groups: &BTreeMap<String, LogGroupRecord>,
    change: &RetentionPolicyChange<'_>,
    now_ms: i64,
) -> crate::ops::types::DryRunResult {
    let changed = change.policy();
    let before = retention_rules_in_effect(account_policies, now_ms);
    // The same policies with the change applied: the previewed policy counted for what it
    // states, and nothing counted for the policy a delete would take away. A retention
    // policy is identified by its name among the account's policies of its type, which is
    // what every rule here is.
    let mut after: Vec<RetentionRule<'_>> = before
        .iter()
        .filter(|rule| rule.policy.policy_name != changed.policy_name)
        .cloned()
        .collect();
    if let RetentionPolicyChange::Put(policy) = change {
        if let Some(effect) = policy.retention.as_ref() {
            after.push(RetentionRule {
                policy,
                retention: effect.stated.clone(),
            });
        }
    }

    // "The number of log groups in the scope of the policy that were analyzed": the groups
    // the changed policy applies to, less the class the analysis leaves out.
    let scope_retention = change.scope_retention(now_ms);
    let in_scope = log_groups.values().filter(|group| {
        group.log_group_class != RETENTION_ANALYSIS_EXCLUDED_LOG_GROUP_CLASS
            && account_policy_selects_log_group(changed, group)
            && scope_retention
                .is_none_or(|retention| retention.reaches(group, changed.created_time_ms))
    });

    let mut increased = 0i32;
    let mut decreased = 0i32;
    let mut unchanged = 0i32;
    let mut impacted: Vec<crate::ops::types::LogGroupImpact> = Vec::new();
    for group in in_scope {
        let current = effective_retention_of(&before, group).retention_in_days;
        let proposed = effective_retention_of(&after, group).retention_in_days;
        match retention_length(proposed).cmp(&retention_length(current)) {
            std::cmp::Ordering::Greater => increased = increased.saturating_add(1),
            std::cmp::Ordering::Less => decreased = decreased.saturating_add(1),
            std::cmp::Ordering::Equal => unchanged = unchanged.saturating_add(1),
        }
        impacted.push(crate::ops::types::LogGroupImpact {
            log_group_name: Some(group.name.clone()),
            current_retention: Some(AccountRetention::reported(current)),
            proposed_retention: Some(AccountRetention::reported(proposed)),
            // "The number of bytes stored in the log group": the figure the group's
            // metering last measured, which is what every reader of a stored volume reports.
            stored_bytes: Some(group.metered_bytes),
        });
    }
    // "Up to 20 log groups in the scope of the policy, sorted by stored bytes in descending
    // order. This list can include log groups whose effective retention period would not
    // change." Between two groups storing the same volume the name order the account holds
    // them in decides, so one store reports one list.
    impacted.sort_by(|one, two| {
        two.stored_bytes
            .cmp(&one.stored_bytes)
            .then_with(|| one.log_group_name.cmp(&two.log_group_name))
    });
    impacted.truncate(MAX_DRY_RUN_IMPACTED_LOG_GROUPS);

    crate::ops::types::DryRunResult {
        // "This value is the sum of increasedRetentionLogGroupCount,
        // decreasedRetentionLogGroupCount, and unchangedRetentionLogGroupCount, and does not
        // include log groups that could not be analyzed."
        total_matched_log_groups: increased
            .saturating_add(decreased)
            .saturating_add(unchanged),
        increased_retention_log_group_count: increased,
        decreased_retention_log_group_count: decreased,
        unchanged_retention_log_group_count: unchanged,
        // "The number of log groups in the scope of the policy that could not be analyzed":
        // the analysis is the comparison above, which every group in scope is answered by,
        // so none of them fails it.
        failed_analysis_count: 0,
        top_log_groups_impacted: Some(impacted),
    }
}
