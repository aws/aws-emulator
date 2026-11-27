// SPDX-License-Identifier: CC0-1.0

//! What Config does for a rule it does NOT evaluate itself: it invokes the evaluator the
//! account pointed the rule at, and that evaluator delivers the verdict back.
//!
//! A Config Custom Lambda rule names a Lambda function as its `SourceIdentifier`, and
//! that function "is required in every Lambda function that is invoked by an Config rule"
//! to report what it found with `PutEvaluations` (`PutEvaluations`). So the invocation is
//! the half Config owes: it hands the function the event describing what to evaluate and
//! the `ResultToken` the delivery comes back on
//! ([`super::ConfigRuleEntry::mint_evaluation_result_token`]), and the verdict itself is
//! the function's own — nothing here computes one.
//!
//! An invocation is OWED for every configuration item Config has to hand the rule's
//! evaluator: the items in the rule's scope a write of the rule asks for ("when you create
//! a rule, Config evaluates your resources against the rule automatically",
//! `PutConfigRule`) and the ones `StartConfigRulesEvaluation` asks for again ("re-runs an
//! evaluation against the last known state of your resources"), and after that ONE per
//! configuration item recorded for a resource in that scope. Each stays owed until its
//! invocation is MADE: the items handed over are the ones before the rule's own watermark
//! ([`super::RuleEvaluationEntry::pushed_through`]), which moves only forward in the order
//! the items were RECORDED, so a pump that runs every few hundred milliseconds neither
//! invokes the function twice for one item nor forgets an item past the bound one pass
//! hands over ([`MAX_INVOCATIONS_PER_PASS`]). A newly recorded item sorts after everything
//! already handed over, so it costs ONE invocation rather than a re-push of the whole
//! scope, and a call that did not reach the function at all leaves its item owed for a
//! later pass.
//!
//! WHICH event the function is handed is the rule's own trigger
//! (`SourceDetail$MessageType`): a periodic rule is invoked with the scheduled
//! notification, and a change-triggered one once per configuration item in its scope with
//! that item's own change notification — the same notification body a delivery channel
//! streams ([`super::change_notification`]), so there is one statement of what a
//! configuration-item event looks like.
//!
//! The function is invoked over the in-process callback, exactly as every other
//! cross-service call of this crate is made, so this crate names no other service crate.
//! It is the function's own run that writes anything: an invocation that could not be made
//! is logged and leaves the rule unevaluated — and owed again — rather than recording a
//! verdict nothing produced. What the evaluation status reports about a rule of this kind
//! is written HERE and by the delivery itself (`PutEvaluations`), from the invocations that
//! were actually made: nothing claims a successful invocation or evaluation of a rule whose
//! evaluator Config never reached.

use std::time::SystemTime;

use super::{
    service_state, ComAmazonawsConfigserviceStarlingdoveserviceState, ConfigRuleEntry,
    PushedThrough, CONFIG_RULE_STATE_ACTIVE,
};

/// The `messageType` of the event Config hands a PERIODIC rule's function (the AWS Config
/// Developer Guide's *Example event for periodic rules*), which is also the
/// `SourceDetail$MessageType` such a rule declares.
pub const SCHEDULED_NOTIFICATION_MESSAGE_TYPE: &str = "ScheduledNotification";

/// The `version` of the invocation event Config hands a rule's function (the same guide's
/// example events).
const INVOCATION_EVENT_VERSION: &str = "1.0";

/// The `recordVersion` the scheduled notification inside that event carries.
const SCHEDULED_NOTIFICATION_RECORD_VERSION: &str = "1.0";

/// The most invocations ONE pass makes for a single change-triggered rule.
///
/// A HOST bound: a change-triggered rule is invoked per configuration item in its scope,
/// and the inventory is bounded, so one owed pass cannot turn into an unbounded number of
/// in-process invocations. What a pass has no room for is LEFT OWED rather than dropped —
/// the rule keeps owing its evaluator an invocation and the next pass carries on after the
/// item this one stopped at ([`super::RuleEvaluationEntry::pushed_through`]) —
/// exactly as a pass that reaches its budget of recorded verdicts leaves the rest for the
/// next one.
pub const MAX_INVOCATIONS_PER_PASS: usize = 50;

/// The most passes that try, and fail, to MAKE one pass's evaluator invocation before
/// Config stops trying.
///
/// A HOST bound: an invocation the call could not deliver at all leaves the pass owed so a
/// later pass makes it
/// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::owe_failed_evaluator_dispatch`]),
/// which is what keeps a momentarily unavailable function from costing a rule its
/// evaluation for good. This many tries — about a minute of passes — covers a function
/// that is briefly away, one still being created or one whose concurrency is momentarily
/// full, many passes over; past them a function that can never be invoked stops costing an
/// attempt per pass.
pub const MAX_FAILED_DISPATCHES: u32 = 120;

/// How many of a pass's owed invocations Config makes AT ONCE.
///
/// A change-triggered rule is invoked once per configuration item, and those invocations
/// are independent of one another — each carries one item and the verdict it delivers is
/// about that item's resource — so a pass makes them concurrently rather than waiting for
/// one function's run before the next call. A HOST bound: this many at once keeps the
/// fan-out one pass can cost bounded, so a rule whose scope covers a long backlog of
/// recorded items does not turn one pass into as many simultaneous in-process runs as it
/// has items.
const CONCURRENT_INVOCATIONS: usize = 8;

impl ConfigRuleEntry {
    /// Whether Config still owes the invocation of THIS rule's own evaluator: the rule
    /// delivers its own verdicts ([`ConfigRuleEntry::delivers_own_evaluations`]), Config
    /// has invoked an evaluation pass for it (so the pass has a `ResultToken`), and that
    /// pass's evaluator has not been invoked yet.
    pub fn owes_evaluator_invocation(&self) -> bool {
        self.delivers_own_evaluations()
            && self.evaluation.result_token.is_some()
            && self.evaluation.result_token != self.evaluation.dispatched_token
    }

    /// Whether this rule is triggered by a CONFIGURATION CHANGE rather than periodically:
    /// one of its `SourceDetails` asks for the configuration-item notification
    /// (`SourceDetail$MessageType`: "ConfigurationItemChangeNotification - Triggers an
    /// evaluation when Config delivers a configuration item as a result of a resource
    /// change").
    ///
    /// A rule that declares no source details at all is PERIODIC here: this emulator has
    /// no configuration-item delivery to trigger off for it, so it is invoked with the
    /// scheduled notification rather than not at all.
    pub(super) fn is_change_triggered(&self) -> bool {
        self.source
            .source_details
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|detail| {
                detail.message_type.as_deref()
                    == Some(super::CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE)
            })
    }
}

/// One invocation of one rule's evaluator that Config owes.
#[derive(Debug, Clone)]
pub struct EvaluatorInvocation {
    /// The function the rule names as its `SourceIdentifier`, which is what the
    /// invocation addresses.
    pub function: String,
    /// The rule the invocation is about, for the log of an invocation that failed.
    pub config_rule_name: String,
    /// The `(resourceType, resourceId)` of the recorded resource this invocation carries
    /// the configuration item of, for a CHANGE-TRIGGERED rule; `None` for a periodic rule,
    /// whose invocation is about no one resource.
    ///
    /// It names which resource an invocation that could not be made was about, which is
    /// what the log of such a failure carries.
    pub subject: Option<(String, String)>,
    /// The item this invocation FOLLOWS in its pass — the one handed over before it, or
    /// the item the pass itself resumed after for the first of them.
    ///
    /// It is the watermark a pass goes back to when THIS invocation could not be made, so a
    /// retry resumes at this invocation and keeps what the passes before it handed over.
    pub follows: Option<PushedThrough>,
    /// The `ResultToken` of the pass this invocation belongs to, which is what says WHICH
    /// pass an invocation that could not be made leaves owed
    /// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::owe_failed_evaluator_dispatch`]).
    pub result_token: String,
    /// The event the function is handed, as the function's own handler reads it.
    pub event: ::serde_json::Value,
}

/// One recorded configuration item a change-triggered rule may be invoked for: what
/// identifies the resource, WHEN its item was recorded, the tags it was recorded with
/// (which the rule's scope can narrow by) and the configuration-item change notification
/// its evaluator is handed.
struct RecordedSubject {
    resource_type: String,
    resource_id: String,
    recorded_at: SystemTime,
    tags: ::std::collections::BTreeMap<String, String>,
    notification: ::serde_json::Value,
}

impl RecordedSubject {
    /// Where this item stands in the order Config hands items over
    /// ([`PushedThrough`]).
    fn pushed_through(&self) -> PushedThrough {
        (
            self.recorded_at,
            self.resource_type.clone(),
            self.resource_id.clone(),
        )
    }
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Whether any rule that owes its evaluator an invocation is a CHANGE-TRIGGERED one,
    /// i.e. whether the recorded inventory has to be read to build those invocations at
    /// all.
    fn owes_change_triggered_invocation(&self) -> bool {
        self.config_rules.values().any(|rule| {
            rule.state == CONFIG_RULE_STATE_ACTIVE
                && rule.owes_evaluator_invocation()
                && rule.is_change_triggered()
        })
    }

    /// Note that the configuration items `recorded` TRIGGER the rules they are in the
    /// scope of, so Config owes those rules' evaluators an invocation.
    ///
    /// A change-triggered rule is evaluated "when Config delivers a configuration item as
    /// a result of a resource change" (`SourceDetail$MessageType`), so what invokes such a
    /// rule's evaluator is the recorded item, not a pass of the clock: a rule written
    /// before the account held anything in its scope is invoked when the first item in
    /// that scope is recorded, and a rule nothing in the pass touched is not invoked again.
    pub fn owe_change_triggered_invocations(
        &mut self,
        recorded: &[super::RecordedItemChange],
        now: SystemTime,
    ) {
        if recorded.is_empty() {
            return;
        }
        // What identifies each recorded resource, and the tags its item was recorded
        // with, read out of the inventory before the rules are walked mutably.
        let subjects: Vec<(String, String, ::std::collections::BTreeMap<String, String>)> = recorded
            .iter()
            .filter_map(|change| {
                let entry = self
                    .discovered_resources
                    .get(&(change.resource_type.clone(), change.resource_id.clone()))?;
                Some((
                    entry.resource_type.clone(),
                    entry.resource_id.clone(),
                    entry.tags.clone(),
                ))
            })
            .collect();
        for rule in self.config_rules.values_mut() {
            if rule.state != CONFIG_RULE_STATE_ACTIVE
                || !rule.delivers_own_evaluations()
                || !rule.is_change_triggered()
            {
                continue;
            }
            let triggered = subjects.iter().any(|(resource_type, resource_id, tags)| {
                super::scope_covers(rule.scope.as_ref(), resource_type, resource_id, tags)
            });
            if triggered {
                // The invocation this recorded item owes: the item itself is what Config
                // hands the evaluator, and the token it mints is the one that item's
                // delivery comes back on. The items already handed over STAY handed over —
                // this owes one invocation, not a re-push of the rule's whole scope.
                rule.evaluation.invoke_for_recorded_item(now);
                rule.mint_evaluation_result_token(now);
            }
        }
    }

    /// Take the evaluator invocations this account owes as of `now`, marking each rule's
    /// pass as dispatched so one pass is invoked once.
    ///
    /// Read under the store's guard and performed with it released, because the function
    /// an invocation runs calls back into this same emulator — which is also why the
    /// marking happens HERE rather than after the call: a second pump that takes the
    /// invocations finds nothing left to make.
    ///
    /// A change-triggered rule is invoked once per recorded configuration item in its
    /// scope, and one pass makes at most [`MAX_INVOCATIONS_PER_PASS`] of them. The items it
    /// had no room for are LEFT OWED: the pass is not marked dispatched, how far it got is
    /// noted ([`super::RuleEvaluationEntry::pushed_through`]) and the next pass carries on
    /// after that item. The items are ordered by WHEN THEY WERE RECORDED, so that watermark
    /// only moves forward: an item recorded while a pass is still behind is handed over
    /// after the ones before it rather than rewinding them, and every recorded item in the
    /// rule's scope reaches the evaluator exactly once however many resources that scope
    /// covers.
    pub fn take_due_evaluator_invocations(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<EvaluatorInvocation> {
        // The configuration items a change-triggered rule is invoked for, copied out so
        // the rules below can be walked mutably, in the order they were RECORDED — which is
        // the order Config hands them over. A resource whose deletion is recorded is
        // no longer there to evaluate. Rendering one item's notification is the cost of
        // one invocation, so the inventory is read only when a rule actually owes a
        // change-triggered invocation: a pass that owes none — which is every pass of an
        // account holding no such rule — reads nothing at all.
        let mut recorded: Vec<RecordedSubject> = if self.owes_change_triggered_invocation() {
            self.discovered_resources
                .values()
                .filter(|entry| entry.deleted_at.is_none())
                .map(|entry| {
                    let item = entry.to_configuration_item(
                        region,
                        self.recording_frequency_of(&entry.resource_type),
                    );
                    RecordedSubject {
                        resource_type: entry.resource_type.clone(),
                        resource_id: entry.resource_id.clone(),
                        // When the item this invocation carries was captured, which is what
                        // places it in the order the items are handed over.
                        recorded_at: entry
                            .last_recorded_at
                            .or(entry.first_recorded_at)
                            .unwrap_or(SystemTime::UNIX_EPOCH),
                        tags: entry.tags.clone(),
                        // The event a change-triggered rule is handed IS the notification
                        // the recorded item produced, built through the one place such a
                        // notification is rendered. The item's own status says which change
                        // it records, so the diff names the change the item is rather than
                        // one chosen here; which PROPERTIES changed is reported by the
                        // delivery channel's stream, and Config's invocation carries the
                        // item.
                        notification: super::configuration_item_change_notification(
                            &super::delivered_configuration_item(&item),
                            recorded_change_type(entry.configuration_item_status()),
                            ::serde_json::Map::new(),
                            now,
                        ),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        recorded.sort_by_key(RecordedSubject::pushed_through);

        let account_id = ::emulator_server::emulator_account_id();
        // The role Config ASSUMED to make this invocation, which is the role the account's
        // customer managed configuration recorder runs as — the one a rule's evaluator is
        // handed as its `executionRoleArn`. An account holding no such recorder names no
        // role, and the member is then left out rather than carried empty.
        let execution_role_arn = self.customer_managed_recorder_role_arn();
        let mut invocations = Vec::new();
        for rule in self.config_rules.values_mut() {
            if rule.state != CONFIG_RULE_STATE_ACTIVE || !rule.owes_evaluator_invocation() {
                continue;
            }
            // The function the rule points at: a Custom Lambda rule's `SourceIdentifier`
            // is "the Amazon Resource Name (ARN) of the Lambda function". A rule naming
            // none names no evaluator, so there is nothing to invoke.
            let Some(function) = rule
                .source
                .source_identifier
                .as_deref()
                .map(str::trim)
                .filter(|identifier| !identifier.is_empty())
            else {
                continue;
            };
            let Some(result_token) = rule.evaluation.result_token.clone() else {
                continue;
            };
            // How far Config has already got through the items in the rule's scope, and
            // what is left for this pass to hand over.
            let reached = rule
                .is_change_triggered()
                .then(|| rule.evaluation.pushed_through.clone())
                .flatten();
            let mut handed_through = None;
            let mut left_owed = false;
            // Each event with the recorded resource it is about and the item it FOLLOWS
            // in this pass, so an invocation that could not be made says exactly where the
            // retry resumes: at itself, with everything before it already handed over.
            type Handed = (
                Option<(String, String)>,
                Option<PushedThrough>,
                ::serde_json::Value,
            );
            let events: Vec<Handed> = if rule.is_change_triggered() {
                let owed: Vec<&RecordedSubject> = recorded
                    .iter()
                    .filter(|subject| {
                        // The items are in the order they were recorded, so "after the item
                        // the last pass stopped at" is the rest of that order — which is
                        // where an item recorded since then is, too.
                        reached
                            .as_ref()
                            .is_none_or(|reached| &subject.pushed_through() > reached)
                    })
                    .filter(|subject| {
                        super::scope_covers(
                            rule.scope.as_ref(),
                            &subject.resource_type,
                            &subject.resource_id,
                            &subject.tags,
                        )
                    })
                    .collect();
                left_owed = owed.len() > MAX_INVOCATIONS_PER_PASS;
                let handing: Vec<&RecordedSubject> =
                    owed.into_iter().take(MAX_INVOCATIONS_PER_PASS).collect();
                handed_through = handing.last().map(|subject| subject.pushed_through());
                let mut follows = reached.clone();
                let mut handed = Vec::with_capacity(handing.len());
                for subject in handing {
                    handed.push((
                        Some((subject.resource_type.clone(), subject.resource_id.clone())),
                        follows.clone(),
                        subject.notification.clone(),
                    ));
                    follows = Some(subject.pushed_through());
                }
                handed
            } else {
                vec![(None, None, scheduled_notification(account_id, now))]
            };
            for (subject, follows, invoking_event) in events {
                invocations.push(EvaluatorInvocation {
                    function: function.to_string(),
                    config_rule_name: rule.name.clone(),
                    result_token: result_token.clone(),
                    subject,
                    follows,
                    event: invocation_event(
                        rule,
                        &invoking_event,
                        &result_token,
                        account_id,
                        execution_role_arn.as_deref(),
                    ),
                });
            }
            if let Some(handed_through) = handed_through {
                rule.evaluation.pushed_through = Some(handed_through);
            }
            if left_owed {
                // The rule still owes the items this pass had no room for, so it is NOT
                // marked dispatched: the next pass carries on after the item above
                // rather than dropping them.
                continue;
            }
            // The pass's evaluator has now been invoked, whatever the function answers:
            // the verdict is the function's to deliver, and Config does not invoke it
            // twice for one pass.
            rule.evaluation.dispatched_token = Some(result_token);
        }
        invocations
    }

    /// Note that the invocation of the pass `result_token` names could NOT BE MADE for the
    /// rule `config_rule_name`, so Config owes it again.
    ///
    /// A call that did not reach the function ran nothing and delivered no verdict, so the
    /// rule is left exactly as unevaluated as it was before the pass — which is why the
    /// pass goes back to OWING its invocation and a later pass makes it. That is the
    /// difference between a function Config could not reach and a function that RAN and
    /// reported an error: the second has answered the invocation, and Config does not
    /// invoke it twice for one pass.
    ///
    /// The retry RESUMES rather than restarts: `delivered_through` is the item the
    /// failing invocation FOLLOWED in its pass ([`EvaluatorInvocation::follows`]), so the
    /// next pass carries on at the one that failed and neither this pass's earlier items
    /// nor the ones an earlier pass handed over are handed over again. `None` — a periodic
    /// rule, or a pass whose very first item failed with none handed over before it —
    /// starts the rule's items from the beginning. Bounded by [`MAX_FAILED_DISPATCHES`]
    /// tries, after which the pass is marked invoked and the rule left unevaluated rather
    /// than retried without end. The failure itself is what the rule's evaluation status
    /// reports as its last failed invocation, with the error Config met. Answers whether the
    /// pass was owed again, which is `false` for a pass another invocation has since
    /// replaced, for a rule the account no longer holds, and past that bound.
    pub fn owe_failed_evaluator_dispatch(
        &mut self,
        config_rule_name: &str,
        result_token: &str,
        delivered_through: Option<PushedThrough>,
    ) -> bool {
        let Some(rule) = self.config_rules.get_mut(config_rule_name) else {
            return false;
        };
        // A pass Config has since invoked again has its own token and its own invocation
        // to make, so the failure of the previous one owes nothing.
        if rule.evaluation.result_token.as_deref() != Some(result_token) {
            return false;
        }
        if rule.evaluation.failed_dispatches >= MAX_FAILED_DISPATCHES {
            ::tracing::warn!(
                config_rule_name = %config_rule_name,
                tries = rule.evaluation.failed_dispatches,
                "the evaluator of a Config rule could not be invoked by any pass"
            );
            // Config stops trying, so the pass is marked invoked however little of it was
            // made: the rule then owes no further invocation, which is what lets the pump
            // making them end rather than re-attempting a function nothing can reach for
            // as long as the account holds the rule.
            rule.evaluation.dispatched_token = rule.evaluation.result_token.clone();
            return false;
        }
        rule.evaluation.failed_dispatches += 1;
        rule.evaluation.dispatched_token = None;
        rule.evaluation.pushed_through = delivered_through;
        true
    }
}

/// The `configurationItemDiff$changeType` of a recorded item, read off the
/// `configurationItemStatus` the item itself reports: the resource's discovery, its
/// deletion, or a configuration that has been updated.
fn recorded_change_type(configuration_item_status: &str) -> &'static str {
    match configuration_item_status {
        super::CONFIGURATION_ITEM_STATUS_RESOURCE_DISCOVERED => super::CHANGE_TYPE_CREATE,
        super::CONFIGURATION_ITEM_STATUS_RESOURCE_DELETED => super::CHANGE_TYPE_DELETE,
        _ => super::CHANGE_TYPE_UPDATE,
    }
}

/// The `invokingEvent` a PERIODIC rule's function is handed: "the notification that
/// triggered the evaluation", which for such a rule is the scheduled one.
fn scheduled_notification(account_id: &str, now: SystemTime) -> ::serde_json::Value {
    ::serde_json::json!({
        "awsAccountId": account_id,
        "notificationCreationTime": super::event_instant(now),
        "messageType": SCHEDULED_NOTIFICATION_MESSAGE_TYPE,
        "recordVersion": SCHEDULED_NOTIFICATION_RECORD_VERSION,
    })
}

/// The event Config hands a rule's function, with the members and the spellings the AWS
/// Config Developer Guide's example events carry.
///
/// `invokingEvent` and `ruleParameters` travel as JSON-encoded STRINGS, which is why a
/// function's handler parses them (`json.loads(event["invokingEvent"])`), and
/// `resultToken` is the token the delivery comes back on. `eventLeftScope` is "whether the
/// Amazon Web Services resource to be evaluated has been removed from the rule's scope",
/// which is false for every invocation Config makes here: the items an invocation carries
/// are the ones the inventory holds. `executionRoleArn` is the role Config assumed to make
/// the invocation.
///
/// A member Config has NO value for is LEFT OUT of the event rather than carried as a JSON
/// null: `ruleParameters` is "if present, the parameters the rule was defined with", so a
/// rule declaring none hands its evaluator an event with no such member — which is what a
/// handler reading it as the string it is (`json.loads(event.get("ruleParameters", "{}"))`)
/// depends on, since a null is neither that string nor absent. The same holds for the
/// execution role of an account whose recorder names none.
fn invocation_event(
    rule: &ConfigRuleEntry,
    invoking_event: &::serde_json::Value,
    result_token: &str,
    account_id: &str,
    execution_role_arn: Option<&str>,
) -> ::serde_json::Value {
    let mut event = ::serde_json::json!({
        "invokingEvent": invoking_event.to_string(),
        "resultToken": result_token,
        "eventLeftScope": false,
        "configRuleArn": rule.arn.clone(),
        "configRuleName": rule.name.clone(),
        "configRuleId": rule.id.clone(),
        "accountId": account_id,
        "version": INVOCATION_EVENT_VERSION,
    });
    let Some(members) = event.as_object_mut() else {
        return event;
    };
    // The parameters the rule was defined with, as the JSON-encoded string they travel as;
    // a rule that declares none carries no such member.
    if let Some(parameters) = rule
        .input_parameters
        .as_deref()
        .filter(|parameters| !parameters.is_empty())
    {
        members.insert(
            "ruleParameters".to_string(),
            ::serde_json::Value::String(parameters.to_string()),
        );
    }
    if let Some(role) = execution_role_arn.filter(|role| !role.is_empty()) {
        members.insert(
            "executionRoleArn".to_string(),
            ::serde_json::Value::String(role.to_string()),
        );
    }
    event
}

/// Invoke the evaluators this account owes an invocation of, as of now, and record what
/// each invocation came to.
///
/// Takes the callback rather than an `OperationContext`, because the pump that calls it
/// outlives the request that started it ([`super::drive_recorder_clock`]). No lock is held
/// across an await: the invocations are decided under the store's guard and made with it
/// released, because the function they run calls back into this same emulator — including
/// into this service, which is how its verdict is delivered.
///
/// What the rule's evaluation status reports comes from HERE, because only the invocation
/// knows: a call the function answered is the successful invocation
/// (`ConfigRuleEvaluationStatus$LastSuccessfulInvocationTime`), a call that did not reach
/// it is the failed one, and a function that ran and reported an error is a failed
/// EVALUATION. The successful evaluation is the function's own delivery, recorded by
/// `PutEvaluations`.
pub async fn invoke_due_rule_evaluators<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let invocations = state
        .lock()
        .take_due_evaluator_invocations(region, SystemTime::now());
    if invocations.is_empty() {
        return;
    }
    let client = lambda_client(callback, region);
    // The rules whose pass this run has already owed again: one pass's failure costs one
    // try, and the invocations of that pass still queued are not made — the retry hands
    // them over, so making them now would hand an item over twice.
    let mut owed_again: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    // The calls are made CONCURRENTLY, a bounded number at a time
    // ([`CONCURRENT_INVOCATIONS`]): one item's evaluation does not wait on another's, so a
    // rule whose scope covers a backlog of recorded items is caught up in passes rather
    // than one function run at a time. Their outcomes are then read IN ORDER, so a failure
    // owes the pass again from the earliest item whose call did not reach the function.
    for chunk in invocations.chunks(CONCURRENT_INVOCATIONS) {
        let answers = ::futures_util::future::join_all(chunk.iter().map(|invocation| {
            let client = client.clone();
            async move {
                // "RequestResponse": Config waits for the function, because the verdict is
                // delivered by the function's own run.
                client
                    .invoke()
                    .function_name(&invocation.function)
                    .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
                    .payload(::aws_sdk_lambda::primitives::Blob::new(
                        invocation.event.to_string().into_bytes(),
                    ))
                    .send()
                    .await
            }
        }))
        .await;
        for (invocation, answered) in chunk.iter().zip(answers) {
            if owed_again.contains(&invocation.config_rule_name) {
                // The pass is owed again from an earlier item of this very chunk, and that
                // retry hands this one over too: whatever this call came to, it is the
                // retry that decides what the rule was told.
                continue;
            }
            let at = SystemTime::now();
            match answered {
                Ok(answer) => {
                    // The call reached the function and it answered, which is the invocation
                    // Config made: the verdict is the function's own to deliver.
                    let function_error = answer.function_error().map(str::to_string);
                    let reported = function_error.as_deref().map(|function_error| {
                        (function_error.to_string(), answered_payload(&answer))
                    });
                    let mut store = state.lock();
                    let Some(rule) = store.config_rules.get_mut(&invocation.config_rule_name)
                    else {
                        continue;
                    };
                    rule.evaluation.note_evaluator_invoked(at);
                    // "FunctionError: if present, indicates that an error occurred during
                    // function execution": the evaluator ran and failed, so it delivered no
                    // verdict and Config records none of its own — what failed is the
                    // EVALUATION, with the function's own answer as the error.
                    if let Some((code, message)) = reported {
                        rule.evaluation
                            .note_evaluation_failed(at, Some(&code), &message);
                        drop(store);
                        ::tracing::warn!(
                            config_rule_name = %invocation.config_rule_name,
                            function_error = %code,
                            "the evaluator of a Config rule failed"
                        );
                    }
                }
                Err(error) => {
                    let fault =
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string();
                    let code = ::aws_sdk_lambda::error::ProvideErrorMetadata::code(&error)
                        .map(str::to_string);
                    ::tracing::warn!(
                        config_rule_name = %invocation.config_rule_name,
                        subject = ?invocation.subject,
                        error = %fault,
                        "the evaluator of a Config rule could not be invoked"
                    );
                    // Nothing ran, so nothing delivered a verdict: the pass goes back to
                    // owing its invocation, resuming at THIS item, and a later pass makes
                    // it. A pass that is NOT owed again — one another invocation has
                    // replaced, or one past the bound on failed tries — is the end of
                    // Config's trying, so it is logged as such.
                    owed_again.insert(invocation.config_rule_name.clone());
                    let mut store = state.lock();
                    if let Some(rule) = store.config_rules.get_mut(&invocation.config_rule_name) {
                        rule.evaluation
                            .note_evaluator_invocation_failed(at, code.as_deref(), &fault);
                    }
                    let owed = store.owe_failed_evaluator_dispatch(
                        &invocation.config_rule_name,
                        &invocation.result_token,
                        invocation.follows.clone(),
                    );
                    drop(store);
                    if !owed {
                        ::tracing::warn!(
                            config_rule_name = %invocation.config_rule_name,
                            "Config is no longer owed the invocation of this rule's evaluator"
                        );
                    }
                }
            }
        }
    }
}

/// What the function ANSWERED, as text: the error document a failed run produced, which is
/// what says why that run failed.
///
/// A payload that is not text is reported as nothing rather than lossily decoded, because
/// an error message is what a reader of the status needs and bytes are not one.
fn answered_payload(answer: &::aws_sdk_lambda::operation::invoke::InvokeOutput) -> String {
    answer
        .payload()
        .map(|payload| String::from_utf8_lossy(payload.as_ref()).to_string())
        .unwrap_or_default()
}

/// An AWS Lambda client over the in-process callback, built the way this crate's other
/// cross-service clients are: the emulator supplies the transport, so there is nothing to
/// retry, time out, cache an identity for or watch for a stalled stream.
///
/// The ONE place this crate reaches AWS Lambda, so the invocation Config makes and the
/// reading of whether it MAY make it ([`super::evaluator_authorization`]) address the same
/// Lambda the same way.
pub(super) fn lambda_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{Source, SourceDetail};
    use crate::state::SQS_QUEUE_RESOURCE_TYPE;

    /// The Region the subjects below are recorded in.
    const REGION: &str = "us-west-2";

    /// The rule every row writes.
    const RULE: &str = "queue-rule";

    /// A change-triggered Custom Lambda rule Config has just invoked a pass for, pointed
    /// at `function`.
    fn change_triggered_rule(name: &str, function: &str, at: SystemTime) -> ConfigRuleEntry {
        let mut rule = ConfigRuleEntry {
            name: name.to_string(),
            arn: format!("arn:aws:config:{REGION}:000000000000:config-rule/{name}"),
            id: name.to_string(),
            description: None,
            scope: None,
            source: Source {
                owner: super::super::OWNER_CUSTOM_LAMBDA.to_string(),
                source_identifier: Some(function.to_string()),
                source_details: Some(vec![SourceDetail {
                    event_source: Some("aws.config".to_string()),
                    message_type: Some(
                        super::super::CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE.to_string(),
                    ),
                    maximum_execution_frequency: None,
                }]),
                custom_policy_details: None,
            },
            input_parameters: None,
            maximum_execution_frequency: None,
            state: CONFIG_RULE_STATE_ACTIVE.to_string(),
            evaluation_modes: super::super::default_evaluation_modes(),
            rule_evaluation_visibility: None,
            created_by: None,
            created_at: at,
            tags: super::super::TagSet::new(),
            evaluation_results: ::std::collections::BTreeMap::new(),
            evaluation: super::super::RuleEvaluationEntry::invoked_at(at),
        };
        rule.mint_evaluation_result_token(at);
        rule
    }

    /// A recorded queue, as the recorder's own pass captures one: an id, a configuration
    /// document and the instant its item was captured at.
    fn recorded_queue(index: usize, at: SystemTime) -> super::super::DiscoveredResourceEntry {
        let name = format!("queue-{index:04}");
        super::super::DiscoveredResourceEntry {
            resource_type: SQS_QUEUE_RESOURCE_TYPE.to_string(),
            resource_id: format!("https://sqs.{REGION}.amazonaws.com/000000000000/{name}"),
            resource_name: Some(name.clone()),
            configuration: Some(
                ::serde_json::json!({ "QueueName": name, "VisibilityTimeout": "30" }).to_string(),
            ),
            first_recorded_at: Some(at),
            last_recorded_at: Some(at),
            ..super::super::DiscoveredResourceEntry::default()
        }
    }

    /// A store holding `subjects` recorded queues and one change-triggered rule over them,
    /// with the ids of those queues in the order the inventory holds them.
    fn store_with(
        subjects: usize,
        at: SystemTime,
    ) -> (
        ComAmazonawsConfigserviceStarlingdoveserviceState,
        Vec<String>,
    ) {
        let mut store = ComAmazonawsConfigserviceStarlingdoveserviceState::default();
        let mut ids = Vec::new();
        for index in 0..subjects {
            let entry = recorded_queue(index, at);
            ids.push(entry.resource_id.clone());
            store.record_discovered_resource(entry);
        }
        store.config_rules.insert(
            RULE.to_string(),
            change_triggered_rule(RULE, "the-evaluator", at),
        );
        (store, ids)
    }

    /// The `resourceId` each invocation of `invocations` carries the configuration item of.
    fn subjects_of(invocations: &[EvaluatorInvocation]) -> Vec<String> {
        invocations
            .iter()
            .map(|invocation| {
                let invoking: ::serde_json::Value = ::serde_json::from_str(
                    invocation.event["invokingEvent"]
                        .as_str()
                        .unwrap_or_default(),
                )
                .expect("the invoking event is a JSON document");
                invoking["configurationItem"]["resourceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// Whether the rule still owes its evaluator an invocation.
    fn still_owed(store: &ComAmazonawsConfigserviceStarlingdoveserviceState) -> bool {
        store
            .config_rules
            .get(RULE)
            .expect("the rule is held")
            .owes_evaluator_invocation()
    }

    /// Every recorded resource in a change-triggered rule's scope is handed to its
    /// evaluator, across as many passes as the per-pass bound needs: a pass that has no
    /// room for the rest leaves them OWED, and no subject is handed over twice.
    ///
    /// The rows differ along how many recorded resources the rule's scope covers, against
    /// the bound one pass makes: one, well under it, exactly it, one past it, and past two
    /// passes' worth of it.
    #[test]
    fn a_change_triggered_rules_evaluator_is_handed_every_recorded_subject() {
        let cases: [(&str, usize); 5] = [
            ("one recorded resource", 1),
            ("well under one pass", 7),
            ("exactly one pass", MAX_INVOCATIONS_PER_PASS),
            ("one past one pass", MAX_INVOCATIONS_PER_PASS + 1),
            ("past two passes", MAX_INVOCATIONS_PER_PASS * 2 + 3),
        ];

        for (case, subjects) in cases {
            let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
            let (mut store, ids) = store_with(subjects, at);
            let mut handed: Vec<String> = Vec::new();
            // One pass per wake-up, for as many as the subjects could need plus a spare:
            // the loop stops as soon as the rule owes no further invocation.
            let passes = subjects / MAX_INVOCATIONS_PER_PASS + 2;
            for _ in 0..passes {
                let invocations = store.take_due_evaluator_invocations(REGION, at);
                assert!(
                    invocations.len() <= MAX_INVOCATIONS_PER_PASS,
                    "case {case:?}: one pass makes at most {MAX_INVOCATIONS_PER_PASS} \
                     invocations, got {}",
                    invocations.len()
                );
                handed.extend(subjects_of(&invocations));
                if !still_owed(&store) {
                    break;
                }
            }

            assert_eq!(
                handed, ids,
                "case {case:?}: expected the evaluator to be handed every recorded \
                 resource exactly once, got {} of {subjects}",
                handed.len()
            );
            // And the rule now owes nothing: a pass that handed everything over is
            // dispatched, so no further pass invokes the function again for it.
            let invocations = store.take_due_evaluator_invocations(REGION, at);
            assert!(
                invocations.is_empty(),
                "case {case:?}: expected no further invocation once every subject was \
                 handed over, got {:?}",
                subjects_of(&invocations)
            );
        }
    }

    /// An invocation that could NOT BE MADE leaves the pass owed, so a later pass makes
    /// it — bounded, so a function that can never be invoked is not retried without end,
    /// and a pass Config invokes afterwards owes its own invocation whatever became of the
    /// one before.
    ///
    /// The rows differ along how many passes fail to reach the function: one, a handful,
    /// and more than Config goes on trying for.
    #[test]
    fn an_invocation_that_could_not_be_made_is_owed_again() {
        let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        let cases: [(&str, u32); 3] = [
            ("one call that did not reach the function", 1),
            ("a handful of them", 5),
            (
                "more than Config goes on trying for",
                MAX_FAILED_DISPATCHES + 3,
            ),
        ];

        for (case, failures) in cases {
            let (mut store, ids) = store_with(1, at);
            let mut made = 0_u32;
            for _ in 0..failures {
                let invocations = store.take_due_evaluator_invocations(REGION, at);
                if invocations.is_empty() {
                    break;
                }
                made += 1;
                assert_eq!(
                    subjects_of(&invocations),
                    ids,
                    "case {case:?}: every try hands the recorded resource over"
                );
                // The call did not reach the function, so nothing ran — and this pass had
                // made no earlier invocation, so the retry starts at its first subject.
                let token = invocations[0].result_token.clone();
                store.owe_failed_evaluator_dispatch(RULE, &token, None);
            }
            let tries = failures.min(MAX_FAILED_DISPATCHES + 1);
            assert_eq!(
                made, tries,
                "case {case:?}: expected {tries} tries at the invocation, got {made}"
            );

            // Past the bound the pass is left dispatched: the rule owes no further
            // invocation, so a pump does not go on trying for good.
            if failures > MAX_FAILED_DISPATCHES {
                assert!(
                    !still_owed(&store),
                    "case {case:?}: expected Config to stop trying past the bound"
                );
            }

            // A pass Config invokes AFTER that — a write of the rule, or an on-demand
            // call — owes its own invocation again, whatever became of the one before.
            let later = at + ::std::time::Duration::from_secs(1);
            let rule = store
                .config_rules
                .get_mut(RULE)
                .expect("the rule is held");
            rule.evaluation.invoke(later);
            rule.mint_evaluation_result_token(later);
            assert_eq!(
                subjects_of(&store.take_due_evaluator_invocations(REGION, later)),
                ids,
                "case {case:?}: expected the next pass to hand the subject over again"
            );
        }
    }

    /// A pass whose invocations were part made and part un-makeable RESUMES at the subject
    /// the call did not reach: the subjects already handed over are not handed over again,
    /// and every subject of the pass still reaches the evaluator.
    ///
    /// The rows differ along how far into the pass's subjects the first un-makeable call
    /// falls: the very first one, one in the middle, and the last of them.
    #[test]
    fn a_retry_resumes_at_the_subject_the_call_did_not_reach() {
        let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        // Fewer subjects than one pass hands over, so what a row shows is the resumption
        // and not the per-pass bound.
        let subjects = 6;
        let cases: [(&str, usize); 3] = [
            ("the first invocation of the pass", 0),
            ("one in the middle of the pass", 3),
            ("the last of the pass", subjects - 1),
        ];

        for (case, failed_at) in cases {
            let (mut store, ids) = store_with(subjects, at);
            let first = store.take_due_evaluator_invocations(REGION, at);
            assert_eq!(
                subjects_of(&first),
                ids,
                "case {case:?}: the pass hands every subject over"
            );
            // The invocations up to `failed_at` were made; that one's call did not reach
            // the function, so the pass is owed again from the subject it FOLLOWS.
            assert!(
                store.owe_failed_evaluator_dispatch(
                    RULE,
                    &first[failed_at].result_token.clone(),
                    first[failed_at].follows.clone(),
                ),
                "case {case:?}: expected the pass to be owed again"
            );

            let retried = store.take_due_evaluator_invocations(REGION, at);
            assert_eq!(
                subjects_of(&retried),
                ids[failed_at..].to_vec(),
                "case {case:?}: expected the retry to resume at the subject the call did \
                 not reach"
            );
            assert!(
                !still_owed(&store),
                "case {case:?}: expected the resumed pass to owe nothing further"
            );
        }
    }

    /// A pass whose subjects need more than one wake-up keeps what the earlier ones handed
    /// over when a later chunk's FIRST call cannot be made: the retry resumes at that
    /// chunk, not at the first subject of the rule.
    #[test]
    fn a_retry_of_a_later_chunk_keeps_what_earlier_passes_handed_over() {
        let case = "the first call of the second chunk";
        let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        // More subjects than one wake-up hands over, so the rule's subjects take two.
        let subjects = MAX_INVOCATIONS_PER_PASS + 3;
        let (mut store, ids) = store_with(subjects, at);

        let chunk = store.take_due_evaluator_invocations(REGION, at);
        assert_eq!(
            subjects_of(&chunk),
            ids[..MAX_INVOCATIONS_PER_PASS].to_vec(),
            "case {case:?}: the first wake-up hands over what it has room for"
        );
        assert!(
            still_owed(&store),
            "case {case:?}: the rest of the subjects are still owed"
        );

        // The next wake-up's FIRST call does not reach the function.
        let next = store.take_due_evaluator_invocations(REGION, at);
        assert_eq!(
            subjects_of(&next),
            ids[MAX_INVOCATIONS_PER_PASS..].to_vec(),
            "case {case:?}: the second wake-up carries on after the first"
        );
        assert!(
            store.owe_failed_evaluator_dispatch(
                RULE,
                &next[0].result_token.clone(),
                next[0].follows.clone(),
            ),
            "case {case:?}: expected the pass to be owed again"
        );

        // What the first wake-up handed over is not handed over again: the retry carries
        // the remaining subjects alone.
        let retried = store.take_due_evaluator_invocations(REGION, at);
        assert_eq!(
            subjects_of(&retried),
            ids[MAX_INVOCATIONS_PER_PASS..].to_vec(),
            "case {case:?}: expected the retry to resume at the chunk that failed"
        );
    }

    /// Past the bound on failed tries the pass is marked INVOKED, so a rule whose evaluator
    /// no call can reach stops owing an invocation — which is what lets the pump making
    /// them end — including a pass that had subjects left to hand over.
    #[test]
    fn a_pass_past_the_bound_on_failed_tries_owes_nothing_further() {
        let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        // The rows differ along whether the pass had handed every subject over: one that
        // fits a single wake-up, and one whose subjects need more than one.
        let cases: [(&str, usize); 2] = [
            ("a pass that fits one wake-up", 2),
            ("a pass with subjects left over", MAX_INVOCATIONS_PER_PASS + 2),
        ];

        for (case, subjects) in cases {
            let (mut store, _) = store_with(subjects, at);
            for _ in 0..=MAX_FAILED_DISPATCHES {
                let invocations = store.take_due_evaluator_invocations(REGION, at);
                if invocations.is_empty() {
                    break;
                }
                let token = invocations[0].result_token.clone();
                let follows = invocations[0].follows.clone();
                store.owe_failed_evaluator_dispatch(RULE, &token, follows);
            }
            assert!(
                !still_owed(&store),
                "case {case:?}: expected the rule to owe no further invocation past the \
                 bound of {MAX_FAILED_DISPATCHES} failed tries"
            );
            assert!(
                store
                    .take_due_evaluator_invocations(REGION, at)
                    .is_empty(),
                "case {case:?}: expected no further invocation to be taken"
            );
        }
    }

    /// A configuration item RECORDED while a rule's push is still behind does not rewind
    /// what Config has already handed over: the items pushed before it are not pushed
    /// again, the ones a bounded pass had no room for are still pushed, and the new item is
    /// pushed too — each exactly once.
    ///
    /// The rows differ along WHEN the new item is recorded relative to the rule's push: in
    /// the middle of it, with items still owed; at the point where everything in scope has
    /// been handed over; and in the middle of it twice over, which is the account whose
    /// resources keep changing while a rule with more of them than one pass hands over is
    /// still catching up.
    #[test]
    fn an_item_recorded_mid_push_does_not_rewind_what_was_already_pushed() {
        let at = SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        // (case, how many items the rule's scope already covers, how many items are
        // recorded after the FIRST pass of the push)
        let cases: [(&str, usize, usize); 3] = [
            ("an item recorded while items are still owed", MAX_INVOCATIONS_PER_PASS + 3, 1),
            ("an item recorded once every item was pushed", 2, 1),
            ("items recorded again while catching up", MAX_INVOCATIONS_PER_PASS * 2, 3),
        ];

        for (case, already, arriving) in cases {
            let (mut store, mut expected) = store_with(already, at);
            // The first wake-up of the push, which hands over what it has room for.
            let mut handed = subjects_of(&store.take_due_evaluator_invocations(REGION, at));

            // Each item recorded after it: a queue of its own, captured later than every
            // item already in the inventory.
            for index in 0..arriving {
                let later = at + ::std::time::Duration::from_secs(1 + index as u64);
                let entry = recorded_queue(already + index, later);
                expected.push(entry.resource_id.clone());
                let change = store
                    .record_discovered_resource(entry)
                    .expect("a queue the inventory did not hold records its first item");
                store.owe_change_triggered_invocations(&[change], later);
            }

            // Every wake-up after that, until the rule owes nothing further.
            let at_last = at + ::std::time::Duration::from_secs(1 + arriving as u64);
            for _ in 0..(already / MAX_INVOCATIONS_PER_PASS + arriving + 2) {
                if !still_owed(&store) {
                    break;
                }
                handed.extend(subjects_of(
                    &store.take_due_evaluator_invocations(REGION, at_last),
                ));
            }

            assert_eq!(
                handed, expected,
                "case {case:?}: expected every recorded item to be pushed exactly once, in \
                 the order they were recorded, got {} pushes of {} items",
                handed.len(),
                expected.len()
            );
            assert!(
                store
                    .take_due_evaluator_invocations(REGION, at_last)
                    .is_empty(),
                "case {case:?}: expected no further invocation once every item was pushed"
            );
        }
    }
}
