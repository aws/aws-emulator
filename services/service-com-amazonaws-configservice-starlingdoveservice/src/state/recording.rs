// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING configuration recorder does between calls: it observes the
//! resources of the types it records, writes a configuration item for each one into
//! the inventory, and re-evaluates the Config rules whose scope covers them —
//! recording the verdict each rule places a resource at, and reporting a verdict that
//! CHANGED as a compliance change, and running the remediation a rule's own
//! configuration standing-orders for a resource that change finds non-compliant.
//!
//! Neither half needs a call from the caller: a recorder that is recording keeps
//! recording, and "when you create a rule, Config evaluates your resources against
//! the rule automatically" (`PutConfigRule`). So a pass is OWED at an instant
//! ([`RECORDING_PASS_DUE_AFTER`] after the call that made it due) and is run by the
//! in-process pump [`drive_recorder_clock`] starts — which is what lets a client that
//! consumes only the resulting compliance-change events make no further Config call
//! at all. A caller reading before that instant still sees the state from before the
//! pass, and the pass writes nothing a read could have written.
//!
//! What Config OBSERVES, it observes through the owning service's own API, reached
//! over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) exactly as the event
//! publication is, so this crate names no other service crate. A resource type no
//! service of this emulator reports is simply not recorded ([`OBSERVABLE_RESOURCE_TYPES`]),
//! and a rule whose implementation this emulator does not have records no verdict at
//! all rather than a synthesised one ([`MANAGED_RULES`]).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::{
    compliance_change_detail, publish_service_event_via, run_remediation_actions, service_state,
    stream_recorded_item_changes, ComAmazonawsConfigserviceStarlingdoveserviceState,
    DiscoveredResourceEntry, EvaluationResultEntry, RecordedItemChange,
    COMPLIANCE_CHANGE_DETAIL_TYPE, COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_NON_COMPLIANT,
    CONFIG_RULE_STATE_ACTIVE, EVALUATION_MODE_DETECTIVE, OWNER_AWS,
};
use crate::ops::types::{Scope, Source};

/// The resource type Config records an Amazon S3 general purpose bucket under, a
/// `com.amazonaws.configservice#ResourceType` value.
pub const S3_BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The resource type Config records an Amazon SQS queue under, a
/// `com.amazonaws.configservice#ResourceType` value.
pub const SQS_QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// The resource types this emulator's configuration recorder can OBSERVE: the ones
/// whose service it serves and whose configuration that service's own API reports.
/// A recorder records the intersection of these and the resource types its recording
/// group names, so a recording group of `allSupported` records exactly these — less the
/// GLOBAL types, which such a group covers only when it asks for them
/// ([`super::observed_iam::is_global_resource_type`]).
pub const OBSERVABLE_RESOURCE_TYPES: [&str; 4] = [
    S3_BUCKET_RESOURCE_TYPE,
    SQS_QUEUE_RESOURCE_TYPE,
    super::observed_iam::IAM_ROLE_RESOURCE_TYPE,
    super::observed_iam::IAM_POLICY_RESOURCE_TYPE,
];

/// The queue attributes a configuration item records as an Amazon SQS queue's
/// CONFIGURATION, in the spelling Amazon SQS names them by.
///
/// Source: the attributes `GetQueueAttributes` reports
/// (`com.amazonaws.sqs#QueueAttributeName`), less the ones that are not how the queue is
/// configured: the three `ApproximateNumberOfMessages*` values count the messages the
/// queue HOLDS, and `CreatedTimestamp` and `LastModifiedTimestamp` say when it was created
/// and last changed — none of which is part of how the queue is configured, so recording
/// them would name an instant among the properties a configuration change altered.
///
/// The document is rendered in THIS order, which is the order the names sort in: it is a
/// catalogue rather than the order a client's map happened to hand the attributes over, so
/// two observations of an unchanged queue render the same document.
const RECORDED_QUEUE_ATTRIBUTES: [&str; 16] = [
    "ContentBasedDeduplication",
    "DeduplicationScope",
    "DelaySeconds",
    "FifoQueue",
    "FifoThroughputLimit",
    "KmsDataKeyReusePeriodSeconds",
    "KmsMasterKeyId",
    "MaximumMessageSize",
    "MessageRetentionPeriod",
    "Policy",
    "QueueArn",
    "ReceiveMessageWaitTimeSeconds",
    "RedriveAllowPolicy",
    "RedrivePolicy",
    "SqsManagedSseEnabled",
    "VisibilityTimeout",
];

/// The attribute Amazon SQS reports a queue's own ARN under
/// (`com.amazonaws.sqs#QueueAttributeName`), which is what the queue's configuration item
/// carries as its `arn`. It is one of [`RECORDED_QUEUE_ATTRIBUTES`] as well, because it is
/// part of how the queue is configured.
const QUEUE_ARN_ATTRIBUTE: &str = "QueueArn";

/// How many pages of queues one pass reads before it stops asking for more.
///
/// A HOST bound, like the one on the pages of roles a pass reads: the inventory a pass
/// writes into is bounded ([`super::DISCOVERED_RESOURCES_KEPT`]), so one pass reads at
/// most the pages that can fill it rather than following a token without end.
const MAX_QUEUE_PAGES: usize = 20;

/// How many pages of buckets one pass reads before it stops asking for more; see
/// [`MAX_QUEUE_PAGES`].
const MAX_BUCKET_PAGES: usize = 20;

/// How long after the call that makes one due Config runs the next
/// recording-and-evaluation pass.
///
/// No sentence of the model or of the service's documentation states how long Config
/// takes to record a resource change or to evaluate a rule against it, so this span is
/// CHOSEN: it sits inside the 250–750 ms window this project uses for a transition the
/// clock owes, which keeps the state from before the pass observable to a caller that
/// reads straight after the call that made the pass due.
pub const RECORDING_PASS_DUE_AFTER: Duration = Duration::from_millis(500);

/// The shortest a pump waits before a pass, so a due instant already past still hands
/// the runtime back rather than spinning.
const MIN_PASS_WAIT: Duration = Duration::from_millis(1);

/// How many passes one pump runs before it releases its claim.
///
/// Continuous recording owes a pass for as long as a recorder is recording, so the
/// loop has no natural end; this cap guarantees termination, and the next operation
/// that makes a pass due starts a fresh pump.
const MAX_RECORDING_PASSES: usize = 10_000;

/// The most verdict CHANGES one pass records and publishes.
///
/// A pass re-evaluates every rule over every recorded resource in its scope, so the
/// work it can produce is the product of two quotas; this bounds what one pass does,
/// and because a pass that stops early leaves the remaining pairs UNevaluated, the
/// next pass continues where this one stopped rather than losing a verdict.
const MAX_CHANGES_PER_PASS: usize = 200;

/// The versioning status an S3 bucket carries while versioning is ON
/// (`com.amazonaws.s3#BucketVersioningStatus`, whose values are `Enabled` and
/// `Suspended`).
const VERSIONING_STATUS_ENABLED: &str = "Enabled";

/// One AWS managed Config rule this emulator evaluates.
///
/// `identifier` is the `SourceIdentifier` Config knows the managed rule by
/// (`Source$SourceIdentifier`: "For Config Managed rules, a predefined identifier from
/// a list"), `resource_type` the type of resource it evaluates, and `verdict` the
/// `ComplianceType` it places a resource at — computed from the CONFIGURATION ITEM the
/// recorder wrote for that resource, never from a request.
pub struct ManagedRule {
    /// See [`ManagedRule`].
    pub identifier: &'static str,
    /// See [`ManagedRule`].
    pub resource_type: &'static str,
    /// See [`ManagedRule`].
    pub verdict: fn(&RecordedConfiguration) -> &'static str,
}

/// The AWS managed rules this emulator evaluates, each one paired with the member of
/// the recorded configuration item its verdict reads.
///
/// Source: the Config managed-rule list (the `SourceIdentifier` values
/// `Source$SourceIdentifier` refers to), which is also where each rule's own
/// description comes from; the per-entry functions below quote it. A managed rule NOT
/// listed here is left unevaluated — Config records no verdict for it rather than a
/// made-up one — which is why this table is the single statement of what this emulator
/// can judge.
pub const MANAGED_RULES: [ManagedRule; 2] = [
    ManagedRule {
        identifier: "S3_BUCKET_VERSIONING_ENABLED",
        resource_type: S3_BUCKET_RESOURCE_TYPE,
        verdict: bucket_versioning_verdict,
    },
    ManagedRule {
        identifier: "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
        resource_type: S3_BUCKET_RESOURCE_TYPE,
        verdict: bucket_encryption_verdict,
    },
];

/// The managed rule `source` names, when this emulator evaluates it.
///
/// Only a rule OWNED by Amazon Web Services is a managed rule; a Custom Lambda or
/// Custom Policy rule carries the caller's own implementation, which this emulator
/// does not run.
pub fn managed_rule_of(source: &Source) -> Option<&'static ManagedRule> {
    if source.owner != OWNER_AWS {
        return None;
    }
    let identifier = source.source_identifier.as_deref().map(str::trim)?;
    MANAGED_RULES
        .iter()
        .find(|managed| managed.identifier == identifier)
}

/// The `ManagedRule` whose verdict reads a configuration item: its `configuration`
/// document and its `supplementaryConfiguration` blocks, each parsed.
///
/// Config "returns [`supplementaryConfiguration`] for certain resource types to
/// supplement the information returned for the `configuration` field"
/// (`ConfigurationItem$supplementaryConfiguration`), and for an S3 bucket it is where
/// the per-feature configuration a rule checks lives — so a verdict reads the item as
/// a whole rather than one member of it.
pub struct RecordedConfiguration {
    /// The item's `Configuration` document.
    pub configuration: ::serde_json::Value,
    /// The item's `supplementaryConfiguration` blocks, keyed as Config keys them.
    pub supplementary: BTreeMap<String, ::serde_json::Value>,
}

impl RecordedConfiguration {
    /// The supplementary block `name`, or `Null` when the item carries none.
    fn block(&self, name: &str) -> &::serde_json::Value {
        self.supplementary.get(name).unwrap_or(&::serde_json::Value::Null)
    }
}

/// The `supplementaryConfiguration` block Config records an S3 bucket's versioning
/// configuration under, and the member of it that carries the status — the
/// `GetBucketVersioning` answer, recorded as the configuration item carries it.
const BUCKET_VERSIONING_BLOCK: &str = "BucketVersioningConfiguration";
/// The block Config records an S3 bucket's default encryption configuration under — the
/// `GetBucketEncryption` answer.
const BUCKET_ENCRYPTION_BLOCK: &str = "ServerSideEncryptionConfiguration";

/// `S3_BUCKET_VERSIONING_ENABLED` — "Checks if versioning is enabled for your S3
/// buckets": COMPLIANT exactly while the bucket's versioning configuration is
/// `Enabled`, which the recorder records in the [`BUCKET_VERSIONING_BLOCK`] block from
/// `GetBucketVersioning`. A bucket whose versioning has never been set carries no
/// status at all, and one whose versioning was suspended carries `Suspended`; neither
/// has versioning enabled.
fn bucket_versioning_verdict(recorded: &RecordedConfiguration) -> &'static str {
    verdict(
        recorded.block(BUCKET_VERSIONING_BLOCK)["status"].as_str()
            == Some(VERSIONING_STATUS_ENABLED),
    )
}

/// `S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED` — "Checks whether the S3 buckets have
/// default encryption enabled": COMPLIANT exactly while a default server-side
/// encryption algorithm is in force for the bucket, which the recorder records in the
/// [`BUCKET_ENCRYPTION_BLOCK`] block from `GetBucketEncryption`.
fn bucket_encryption_verdict(recorded: &RecordedConfiguration) -> &'static str {
    verdict(
        recorded.block(BUCKET_ENCRYPTION_BLOCK)["rules"]
            .as_array()
            .is_some_and(|rules| {
                rules.iter().any(|rule| {
                    rule["applyServerSideEncryptionByDefault"]["sseAlgorithm"]
                        .as_str()
                        .is_some_and(|algorithm| !algorithm.is_empty())
                })
            }),
    )
}

/// The `ComplianceType` a managed rule's check places a resource at: a check the
/// resource satisfies is `COMPLIANT`, one it does not is `NON_COMPLIANT`.
///
/// Visible to the whole state module because the rules whose subject is the ACCOUNT
/// (`super::account_rule`) place it the same way: one statement of what a satisfied check
/// amounts to.
pub(super) fn verdict(satisfied: bool) -> &'static str {
    if satisfied {
        COMPLIANCE_TYPE_COMPLIANT
    } else {
        COMPLIANCE_TYPE_NON_COMPLIANT
    }
}

/// One verdict change a pass produced: the rule whose verdict moved, the result that
/// now holds and the one it replaced (absent when this is the rule's first verdict for
/// the resource).
#[derive(Debug, Clone)]
pub struct ComplianceChange {
    /// The rule whose verdict changed.
    pub rule_name: String,
    /// That rule's ARN, which the notification carries.
    pub rule_arn: String,
    /// The verdict that now holds.
    pub new_result: EvaluationResultEntry,
    /// The verdict it replaced, where there was one.
    pub old_result: Option<EvaluationResultEntry>,
}

/// What the evaluation step needs about one recorded resource, copied out of the
/// inventory so the rules can be walked mutably while it is read.
struct RecordedResource {
    resource_type: String,
    resource_id: String,
    /// The configuration item a rule's check reads.
    recorded: RecordedConfiguration,
    /// When the configuration item was captured, which is the instant the evaluation a
    /// rule runs over it is ABOUT (`EvaluationResultIdentifier$OrderingTimestamp`: "the
    /// time of the event that triggered the evaluation").
    captured_at: SystemTime,
    /// The tags recorded with the configuration item, which a rule's scope can narrow
    /// by.
    tags: BTreeMap<String, String>,
}

/// Whether a rule with `scope` covers the RECORDED resource `resource`: the rules
/// that can place THIS resource at a verdict.
fn scope_covers_recorded(scope: Option<&Scope>, resource: &RecordedResource) -> bool {
    scope_covers(
        scope,
        &resource.resource_type,
        &resource.resource_id,
        &resource.tags,
    )
}

/// Whether a rule with `scope` covers the recorded resource of type `resource_type` with
/// id `resource_id`, whose configuration item was recorded with `tags`.
///
/// A rule with no scope covers every recorded resource; a scope narrows by resource
/// type, by resource id and by the TAGS the configuration item was recorded with
/// (`Scope$TagKey`: "The tag key that is applied to only those Amazon Web Services
/// resources that you want to trigger an evaluation for the rule", with `TagValue`
/// narrowing further and only "if you also specify a tag key"). `ServicePrincipals`
/// narrows nothing: it is "populated only if the service-linked rule is created by a
/// service" and names the services the rule is FOR, not the resources it evaluates.
///
/// Visible to the whole state module because WHICH resources a rule covers is one
/// question: the evaluation Config performs itself asks it, and so does the invocation of
/// a change-triggered rule's own evaluator (`super::custom_lambda_rule`).
pub(super) fn scope_covers(
    scope: Option<&Scope>,
    resource_type: &str,
    resource_id: &str,
    tags: &BTreeMap<String, String>,
) -> bool {
    let Some(scope) = scope else {
        return true;
    };
    let covers_type = scope
        .compliance_resource_types
        .as_deref()
        .filter(|types| !types.is_empty())
        .is_none_or(|types| types.iter().any(|scoped| scoped == resource_type));
    let covers_id = scope
        .compliance_resource_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .is_none_or(|id| id == resource_id);
    let covers_tag = scope
        .tag_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .is_none_or(|key| {
            tags.get(key).is_some_and(|held| {
                scope
                    .tag_value
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .is_none_or(|value| value == held)
            })
        });
    covers_type && covers_id && covers_tag
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Note the recording-and-evaluation pass this account owes next, due
    /// [`RECORDING_PASS_DUE_AFTER`] from `now`.
    ///
    /// A pass is owed while a recorder is RECORDING — Config observes nothing while every
    /// recorder is stopped — and while any rule is still owed the evaluation pass a write
    /// or an on-demand call invoked, because "when you create a rule, Config evaluates
    /// your resources against the rule automatically" (`PutConfigRule`) whether or not a
    /// recorder is running. An account that owes neither owes no pass, which is also what
    /// ends the pump.
    ///
    /// A pass ALREADY owed keeps the instant it falls due at: it is the pass the account
    /// owes, and pushing it out on every call that notes one would let a caller that keeps
    /// calling hold recording off for as long as it kept calling.
    pub fn owe_recording_pass(&mut self, now: SystemTime) {
        let owed = self.recording_recorder().is_some() || self.owes_rule_evaluation(now);
        if !owed {
            self.recording_pass_due = None;
            return;
        }
        if self.recording_pass_due.is_none() {
            self.recording_pass_due = Some(now + RECORDING_PASS_DUE_AFTER);
        }
    }

    /// Whether any rule of this account is still owed an evaluation pass — one a write or
    /// an on-demand call invoked that has not fallen due yet — or still owes the
    /// invocation of the evaluator that delivers its verdicts
    /// ([`super::custom_lambda_rule`]).
    ///
    /// A pass that is PAST its due instant is one the next pass runs, and that pass is
    /// already due; a rule this emulator has no implementation for records nothing when it
    /// runs, so nothing is owed for it after that. That is what ends the pump: an account
    /// whose recorders are stopped and whose rules have all had their pass owes no further
    /// work.
    fn owes_rule_evaluation(&self, now: SystemTime) -> bool {
        self.config_rules.values().any(|rule| {
            rule.state == CONFIG_RULE_STATE_ACTIVE
                && (rule.evaluation.in_progress(now) || rule.owes_evaluator_invocation())
        })
    }

    /// Give up the lease the pump `id` holds on this store's passes.
    ///
    /// The lease is TAKEN through [`super::PumpClaim`], which gives it up again however
    /// the pump that holds it ends — so this is the one way it is released, and there is
    /// no second way to take it that would skip that guard.
    pub fn release_recording_pump(&mut self, id: u64) {
        self.recording_pump.release(id);
    }

    /// The instant this store's next pass falls due, for the pump `id`: the lease is
    /// renewed in the same critical section, and given up when the account owes no
    /// further pass — so no pass can be owed while no pump is running.
    ///
    /// A pump ends when the account OWES nothing — which is what stopping every recorder
    /// does — and not merely because the pass it was going to run has already been taken:
    /// a recorder that is recording goes on owing the next pass, so the instant it falls
    /// due at is noted again here ([`Self::owe_recording_pass`]).
    ///
    /// `None` ends that pump's loop, which is also what it answers when another pump has
    /// taken the lease over.
    pub fn next_recording_pass_due_or_release_pump(
        &mut self,
        id: u64,
        now: SystemTime,
    ) -> Option<SystemTime> {
        if self.recording_pass_due.is_none() {
            self.owe_recording_pass(now);
        }
        let Some(due) = self.recording_pass_due else {
            self.release_recording_pump(id);
            return None;
        };
        self.recording_pump.renew(id, now).then_some(due)
    }

    /// Take the pass the clock has made due as of `now`, as the resource types the
    /// recording recorder records and this emulator can observe; `None` when no pass is
    /// due yet.
    ///
    /// Taking it clears the due instant, so one due pass runs once however many pumps
    /// ask for it. The list is EMPTY while no recorder is recording: nothing is observed
    /// then, and the pass is still the one that evaluates the rules the account owes a
    /// verdict for.
    ///
    /// A GLOBAL resource type is observed only where the recording group asks for the
    /// global resource types (`RecordingGroup$includeGlobalResourceTypes`) or names that
    /// type itself, which is what a group of `allSupported` alone does not cover
    /// (`PutConfigurationRecorder`: "Default is to record all supported resource types,
    /// excluding the global IAM resource types").
    pub fn take_due_recording_pass(&mut self, now: SystemTime) -> Option<Vec<&'static str>> {
        if self.recording_pass_due.is_none_or(|due| now < due) {
            return None;
        }
        self.recording_pass_due = None;
        Some(
            OBSERVABLE_RESOURCE_TYPES
                .into_iter()
                .filter(|resource_type| self.records_resource_type(resource_type))
                .collect(),
        )
    }

    /// Whether this account is RECORDING `resource_type` at this instant: a recorder of
    /// its own is recording and the recording group that recorder records by covers that
    /// type.
    ///
    /// The ONE statement of what the account records, so the pass that OBSERVES the
    /// resources of a type ([`Self::take_due_recording_pass`]) and the write that RECORDS
    /// what it observed ([`Self::record_observed`]) cannot disagree about it.
    ///
    /// A GLOBAL resource type is recorded only where the recording group asks for the
    /// global resource types (`RecordingGroup$includeGlobalResourceTypes`) or names that
    /// type itself, which is what a group of `allSupported` alone does not cover
    /// (`PutConfigurationRecorder`: "Default is to record all supported resource types,
    /// excluding the global IAM resource types").
    pub fn records_resource_type(&self, resource_type: &str) -> bool {
        let Some(recorder) = self.recording_recorder() else {
            return false;
        };
        match recorder.recording_group.as_ref() {
            // A recorder stored without a recording group records every supported
            // resource type, as a group of `allSupported` does.
            None => !super::observed_iam::is_global_resource_type(resource_type),
            Some(group) => {
                super::group_records_resource_type(group, resource_type)
                    && super::observed_iam::group_records_global_resource_type(
                        group,
                        resource_type,
                    )
            }
        }
    }

    /// Record what a pass OBSERVED for one resource type: a configuration item per
    /// resource the owning service reported, and the DELETION of a resource of that
    /// type the inventory holds and the pass did not see.
    ///
    /// Every item goes in through [`Self::record_discovered_resource`], so a resource
    /// keeps the instant Config FIRST recorded it and the states it has already left,
    /// and a deleted resource keeps its entry carrying the instant it went away — a
    /// discovered-resource read can be asked to include deleted resources.
    ///
    /// Answers the configuration items the pass RECORDED, which are the ones the delivery
    /// channel owes a change notification for; an observation of a resource that is still
    /// in the state its held item records contributes none.
    ///
    /// Nothing is recorded for a type the account is no longer RECORDING
    /// ([`Self::records_resource_type`]). A pass reads the resources from their own
    /// services with this store's guard RELEASED, so the recorder can be stopped — or its
    /// recording group narrowed — while it is reading: Config records nothing while it is
    /// not recording, so what such a pass read is dropped rather than written as though
    /// the recorder had still been on. The decision is taken here, under the guard that
    /// writes, which is the only instant at which it holds.
    pub fn record_observed(
        &mut self,
        resource_type: &str,
        observed: Vec<DiscoveredResourceEntry>,
        now: SystemTime,
    ) -> Vec<RecordedItemChange> {
        if !self.records_resource_type(resource_type) {
            return Vec::new();
        }
        let mut recorded = Vec::new();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        for entry in observed {
            seen.insert((entry.resource_type.clone(), entry.resource_id.clone()));
            recorded.extend(self.record_discovered_resource(entry));
        }
        let gone: Vec<DiscoveredResourceEntry> = self
            .discovered_resources
            .iter()
            .filter(|(key, entry)| {
                key.0 == resource_type && !seen.contains(*key) && entry.deleted_at.is_none()
            })
            .map(|(_, entry)| DiscoveredResourceEntry {
                deleted_at: Some(now),
                last_recorded_at: Some(now),
                ..entry.clone()
            })
            .collect();
        for entry in gone {
            recorded.extend(self.record_discovered_resource(entry));
        }
        recorded
    }

    /// Evaluate the account's rules as of `now` — the managed rules over the recorded
    /// resources in their scope, and the account-wide managed rules over the account's own
    /// configuration `account` where the pass observed it — recording one result per rule
    /// and subject, and answer the verdicts that CHANGED.
    ///
    /// A rule is evaluated when it is `ACTIVE`, when the pass it is owed has fallen due
    /// ([`super::RuleEvaluationEntry::in_progress`]) and when this emulator has its
    /// implementation ([`MANAGED_RULES`], [`super::account_rule::ACCOUNT_MANAGED_RULES`]);
    /// anything else records nothing, so a rule whose logic is not here keeps reporting
    /// `INSUFFICIENT_DATA` rather than a synthesised verdict.
    pub fn evaluate_rules(
        &mut self,
        account: Option<&super::ObservedAccount>,
        now: SystemTime,
    ) -> Vec<ComplianceChange> {
        // The configuration items the recorder has captured, copied out so the rules
        // below can be walked mutably. A resource whose deletion is recorded is no
        // longer there to evaluate, and one no configuration item has been recorded for
        // has nothing to evaluate against.
        let recorded: Vec<RecordedResource> = self
            .discovered_resources
            .values()
            .filter(|entry| entry.deleted_at.is_none())
            .filter_map(|entry| {
                Some(RecordedResource {
                    resource_type: entry.resource_type.clone(),
                    resource_id: entry.resource_id.clone(),
                    recorded: RecordedConfiguration {
                        configuration: ::serde_json::from_str(entry.configuration.as_deref()?)
                            .ok()?,
                        supplementary: entry
                            .supplementary_configuration
                            .iter()
                            .filter_map(|(block, value)| {
                                Some((block.clone(), ::serde_json::from_str(value).ok()?))
                            })
                            .collect(),
                    },
                    captured_at: entry.last_recorded_at?,
                    tags: entry.tags.clone(),
                })
            })
            .collect();

        let mut changes: Vec<ComplianceChange> = Vec::new();
        for rule in self.config_rules.values_mut() {
            if rule.state != CONFIG_RULE_STATE_ACTIVE || rule.evaluation.in_progress(now) {
                continue;
            }
            let Some(managed) = managed_rule_of(&rule.source) else {
                // A rule whose subject is the ACCOUNT is placed from the account's own
                // configuration instead, which this pass has only where that
                // configuration could be read (`super::account_rule`).
                if super::account_managed_rule_of(&rule.source).is_some() {
                    if let Some(account) = account {
                        changes.extend(rule.evaluate_against_account(account, now));
                    }
                }
                continue;
            };
            // The instant Config invoked the pass this rule is owed, which the result
            // reports as the rule's invocation time; a pass no write asked for is
            // invoked at the instant it runs.
            let invoked_at = rule.evaluation.invoked_at.unwrap_or(now);
            for resource in &recorded {
                if resource.resource_type != managed.resource_type
                    || !scope_covers_recorded(rule.scope.as_ref(), resource)
                {
                    continue;
                }
                if changes.len() >= MAX_CHANGES_PER_PASS {
                    // What is left stays UNevaluated, so the next pass evaluates it.
                    return changes;
                }
                let result = EvaluationResultEntry {
                    config_rule_invoked_time: invoked_at,
                    result_recorded_time: now,
                    ..EvaluationResultEntry::new(
                        &resource.resource_type,
                        &resource.resource_id,
                        EVALUATION_MODE_DETECTIVE,
                        (managed.verdict)(&resource.recorded),
                        resource.captured_at,
                    )
                };
                // The rule's own writer records it, which is what keeps a rule's results
                // bounded and decides whether the verdict MOVED — the verdict it replaces
                // is what a notification reports the resource changed from.
                changes.extend(rule.record_verdict(result));
            }
            // The pass this rule was owed has now evaluated the resources in its scope.
            rule.evaluation.settle(now);
        }
        changes
    }
}

/// Run the recording-and-evaluation pass the clock has made due as of `now`, and
/// publish every compliance change it produced.
///
/// Takes the callback rather than an `OperationContext`, because the pump that calls it
/// outlives the request that started it; a handler reaches it through
/// [`drive_recorder_clock`]. No lock is held across an await: the pass takes the store's
/// guard to decide what is due, releases it to read the resources from their own
/// services, takes it again to record and evaluate, and releases it before publishing.
pub async fn run_due_recording_pass<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let Some(resource_types) = state.lock().take_due_recording_pass(now) else {
        return;
    };

    let mut observed: Vec<(&'static str, Vec<DiscoveredResourceEntry>)> = Vec::new();
    for resource_type in resource_types {
        // Only a type whose resources were actually READ is recorded: a read that
        // failed says nothing about what the account holds, and recording it as empty
        // would record every resource of that type as deleted.
        if resource_type == S3_BUCKET_RESOURCE_TYPE {
            if let Some(buckets) = observed_s3_buckets(callback, region, now).await {
                observed.push((resource_type, buckets));
            }
        }
        if resource_type == SQS_QUEUE_RESOURCE_TYPE {
            if let Some(queues) = observed_sqs_queues(callback, region, now).await {
                observed.push((resource_type, queues));
            }
        }
        if resource_type == super::observed_iam::IAM_ROLE_RESOURCE_TYPE {
            if let Some(roles) = super::observed_iam::observed_iam_roles(callback, region, now).await
            {
                observed.push((resource_type, roles));
            }
        }
        if resource_type == super::observed_iam::IAM_POLICY_RESOURCE_TYPE {
            if let Some(policies) =
                super::observed_iam::observed_iam_policies(callback, region, now).await
            {
                observed.push((resource_type, policies));
            }
        }
    }

    // The ACCOUNT's own configuration, read only while this account holds a rule whose
    // subject is the account: a rule nothing asks about costs no read, and an account
    // whose configuration could not be read leaves those rules unevaluated rather than
    // placed at a verdict nothing computed.
    let account = if state.lock().evaluates_account_rules() {
        super::observed_iam::observed_account(callback, region).await
    } else {
        None
    };

    let (recorded, changes) = {
        let mut store = state.lock();
        let mut recorded = Vec::new();
        for (resource_type, entries) in observed {
            recorded.extend(store.record_observed(resource_type, entries, now));
        }
        // A configuration item this pass RECORDED triggers the rules whose scope covers
        // the resource it is about, which is what owes their evaluators an invocation
        // (`super::custom_lambda_rule`).
        store.owe_change_triggered_invocations(&recorded, now);
        (recorded, store.evaluate_rules(account.as_ref(), now))
    };

    // The evaluators Config has to INVOKE for the rules it does not evaluate itself: a
    // Custom Lambda rule is evaluated by the function the account points it at, which
    // delivers its verdict back with `PutEvaluations` (`super::custom_lambda_rule`). The
    // invocations happen with the store's guard released, because the function they run
    // calls back into this same emulator.
    super::invoke_due_rule_evaluators(callback, engine, region).await;

    // Each configuration item the pass recorded is streamed to the delivery channel's
    // Amazon SNS topic, which is what a consumer subscribed to that topic receives.
    stream_recorded_item_changes(callback, engine, region, &recorded, now).await;

    // What the account has standing instructions to remediate: the resources a verdict
    // just placed at NON_COMPLIANT under a rule whose remediation configuration is
    // AUTOMATIC. The executions are recorded under the same guard that decided them,
    // and the runs happen with the guard released, as a started execution's do.
    let actions = state.lock().automatic_remediations(&changes, now);

    for change in changes {
        let detail = compliance_change_detail(
            &change.rule_name,
            &change.rule_arn,
            region,
            &change.new_result,
            change.old_result.as_ref(),
            now,
        );
        publish_service_event_via(callback, region, COMPLIANCE_CHANGE_DETAIL_TYPE, &detail).await;
    }

    run_remediation_actions(callback, engine, region, &actions).await;
}

/// Start the in-process pump that runs the passes this store owes, after noting that
/// one is owed.
///
/// Called by an operation that makes a pass due — one that starts a recorder, or one
/// that writes a rule Config then evaluates — AFTER it has released the store's lock.
/// It does not change WHEN a pass happens: the pass runs at the due instant the store
/// holds, so a caller reading before it still sees the state from before the pass. At
/// most one pump runs per store, it is bounded by [`MAX_RECORDING_PASSES`] passes, and
/// it ends as soon as the store owes nothing — which is what stopping every recorder
/// does. With no async runtime to spawn onto there is no pump, and the pass is simply
/// not run: nothing a caller is told depends on it.
///
/// A recorder that is recording keeps recording, so a pump that is no longer running
/// the passes cannot be what stops them: it holds the store's pump lease only while it
/// renews it ([`super::PumpClaim`]), and this call starts a fresh pump once that lease
/// has lapsed — which is what resumes recording after a pass panicked, was dropped with
/// the runtime that spawned it, or wedged in a call that never answered.
pub fn drive_recorder_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let owed = {
        let mut store = state.lock();
        store.owe_recording_pass(SystemTime::now());
        store.recording_pass_due.is_some()
    };
    if !owed {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(claim) = super::PumpClaim::take(
        &state,
        |store| &mut store.recording_pump,
        SystemTime::now(),
    ) else {
        return;
    };

    let callback = ctx.callback().clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    runtime.spawn(async move {
        // The claim is held for as long as this pump runs and given up however it ends,
        // so the next call that makes a pass due can start a pump whatever became of
        // this one.
        let pump = claim.id();
        for _ in 0..MAX_RECORDING_PASSES {
            let Some(due) = claim
                .store()
                .lock()
                .next_recording_pass_due_or_release_pump(pump, SystemTime::now())
            else {
                break;
            };
            // Every wake-up waits, even for an instant already past, so the cap above
            // can never cost a CPU-bound spin and the runtime is handed back between
            // passes.
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or(MIN_PASS_WAIT)
                .clamp(MIN_PASS_WAIT, RECORDING_PASS_DUE_AFTER);
            ::tokio::time::sleep(wait).await;
            run_due_recording_pass(&callback, &*engine, &region, SystemTime::now()).await;
            // Continuous recording owes the next pass as soon as this one is done; an
            // account whose recorders are all stopped owes none, which ends the pump.
            claim.store().lock().owe_recording_pass(SystemTime::now());
        }
    });
}

/// The configuration items the recorder observes for this Region's Amazon S3 buckets,
/// or `None` when S3 could not be read in full — which is not the same as an account
/// with no buckets.
///
/// Every member of the recorded configuration document comes from an S3 read named on
/// it; nothing else is recorded, because nothing else is observed.
async fn observed_s3_buckets(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    now: SystemTime,
) -> Option<Vec<DiscoveredResourceEntry>> {
    let client = s3_client(callback, region);
    // "Limits the response to buckets that are located in the specified Amazon Web
    // Services Region" (`ListBucketsRequest$BucketRegion`): a configuration recorder
    // records the resources of ONE Region, the one its own account and Region stand for.
    //
    // The listing is PAGED (`ListBucketsOutput$ContinuationToken`: "a token that you can
    // use in a subsequent request to fetch the next page"), and a pass follows the token
    // so an account holding more buckets than one page is observed in full. A listing this
    // pass could not read to its end says nothing about what the account holds — every
    // bucket past the last page it read would look gone — so it is answered as an unread
    // listing rather than as a complete one.
    let mut buckets: Vec<::aws_sdk_s3::types::Bucket> = Vec::new();
    let mut continuation_token: Option<String> = None;
    let mut read_in_full = false;
    for _ in 0..MAX_BUCKET_PAGES {
        let page = match client
            .list_buckets()
            .bucket_region(region)
            .set_continuation_token(continuation_token.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read this account's S3 buckets"
                );
                return None;
            }
        };
        buckets.extend(page.buckets().iter().cloned());
        continuation_token = page.continuation_token;
        if continuation_token.is_none() {
            read_in_full = true;
            break;
        }
    }
    if !read_in_full {
        ::tracing::warn!(
            pages = MAX_BUCKET_PAGES,
            "the configuration recorder stopped short of this account's whole S3 bucket \
             listing"
        );
        return None;
    }

    let mut observed = Vec::new();
    for bucket in &buckets {
        let Some(name) = bucket.name() else {
            continue;
        };
        // `GetBucketVersioning`'s `Status`, absent for a bucket whose versioning has
        // never been set.
        let versioning = client
            .get_bucket_versioning()
            .bucket(name)
            .send()
            .await
            .ok()
            .and_then(|answer| answer.status().map(|status| status.as_str().to_string()));
        // `GetBucketEncryption`'s `ServerSideEncryptionConfiguration`, as the default
        // algorithm each of its rules applies.
        let encryption_algorithms: Vec<String> = client
            .get_bucket_encryption()
            .bucket(name)
            .send()
            .await
            .ok()
            .and_then(|answer| answer.server_side_encryption_configuration)
            .map(|configuration| {
                configuration
                    .rules
                    .iter()
                    .filter_map(|rule| {
                        rule.apply_server_side_encryption_by_default
                            .as_ref()
                            .map(|default| default.sse_algorithm.as_str().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        // `GetBucketTagging`'s `TagSet`: the tags a rule's scope can narrow by.
        let tags: BTreeMap<String, String> = client
            .get_bucket_tagging()
            .bucket(name)
            .send()
            .await
            .ok()
            .map(|answer| {
                answer
                    .tag_set
                    .into_iter()
                    .map(|tag| (tag.key, tag.value))
                    .collect()
            })
            .unwrap_or_default();

        // The configuration item Config records for a bucket: the bucket's own
        // identity in `configuration`, and each feature's configuration as a
        // `supplementaryConfiguration` block, which is "a string to string map ...
        // Config returns ... for certain resource types to supplement the information
        // returned for the configuration field", each value a JSON document.
        let configuration = ::serde_json::json!({
            "name": name,
            // "Date the bucket was created" (`Bucket$CreationDate`, from `ListBuckets`).
            "creationDate": bucket
                .creation_date()
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()),
        });
        let mut supplementary: BTreeMap<String, String> = BTreeMap::new();
        supplementary.insert(
            BUCKET_VERSIONING_BLOCK.to_string(),
            ::serde_json::json!({ "status": versioning }).to_string(),
        );
        supplementary.insert(
            BUCKET_ENCRYPTION_BLOCK.to_string(),
            ::serde_json::json!({
                "rules": encryption_algorithms
                    .iter()
                    .map(|algorithm| ::serde_json::json!({
                        "applyServerSideEncryptionByDefault": { "sseAlgorithm": algorithm },
                    }))
                    .collect::<Vec<::serde_json::Value>>(),
            })
            .to_string(),
        );

        observed.push(DiscoveredResourceEntry {
            resource_type: S3_BUCKET_RESOURCE_TYPE.to_string(),
            // Config identifies a bucket by its name, which is both the resource id and
            // the resource name of its configuration item.
            resource_id: name.to_string(),
            resource_name: Some(name.to_string()),
            // A bucket's ARN is the one its name derives (`recorded_resource_arn`), and
            // no S3 read above reports one, so none is stored for it.
            arn: None,
            deleted_at: None,
            // The schema version of a configuration item Config itself recorded is not
            // one a caller supplied, and this emulator has no catalogue of them, so none
            // is reported rather than an invented one.
            schema_version_id: None,
            configuration: Some(configuration.to_string()),
            supplementary_configuration: supplementary,
            tags,
            first_recorded_at: Some(now),
            last_recorded_at: Some(now),
            // No S3 read above reports another resource this bucket is linked to, so the
            // item records no relationship rather than one nothing observed.
            relationships: Vec::new(),
            // The states this bucket has already left are kept by the store's own write
            // path, which carries them over from the item this one replaces.
            history: Vec::new(),
        });
    }
    Some(observed)
}

/// The configuration items the recorder observes for this Region's Amazon SQS queues, or
/// `None` when Amazon SQS could not be read in full — which is not the same as an account
/// with no queues.
///
/// Every member of the recorded configuration document comes from an Amazon SQS read named
/// on it; nothing else is recorded, because nothing else is observed. A queue whose
/// attributes could not be read contributes no item rather than one recording an empty
/// configuration.
async fn observed_sqs_queues(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    now: SystemTime,
) -> Option<Vec<DiscoveredResourceEntry>> {
    let client = sqs_client(callback, region);
    // "Returns a list of your queues in the current region" (`ListQueues`): a
    // configuration recorder records the resources of ONE Region, the one its own account
    // and Region stand for, which is the Region this client is built for.
    //
    // The listing is PAGED (`ListQueuesResult$NextToken`: "Pagination token to include in
    // the next request"), and a pass follows the token so an account holding more queues
    // than one page is observed in full. A listing this pass could not read to its end
    // says nothing about what the account holds — every queue past the last page it read
    // would look gone — so it is answered as an unread listing rather than as a complete
    // one.
    let mut queue_urls: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut read_in_full = false;
    for _ in 0..MAX_QUEUE_PAGES {
        let page = match client
            .list_queues()
            .set_next_token(next_token.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read this account's Amazon SQS queues"
                );
                return None;
            }
        };
        queue_urls.extend(page.queue_urls().iter().cloned());
        next_token = page.next_token;
        if next_token.is_none() {
            read_in_full = true;
            break;
        }
    }
    if !read_in_full {
        ::tracing::warn!(
            pages = MAX_QUEUE_PAGES,
            "the configuration recorder stopped short of this account's whole Amazon SQS \
             queue listing"
        );
        return None;
    }

    let mut observed = Vec::new();
    for queue_url in &queue_urls {
        let queue_url = queue_url.as_str();
        // "The queue name" is the last segment of a queue's url
        // (`https://sqs.<region>.amazonaws.com/<account>/<name>`), and it is the queue's
        // custom name; the url itself is how Amazon SQS addresses the queue and is what
        // identifies it, so it is the id the configuration item records.
        let Some(name) = queue_url.rsplit('/').next().filter(|name| !name.is_empty()) else {
            continue;
        };
        // `GetQueueAttributes`' `Attributes`, asked for in full and recorded as the
        // queue's configuration where the attribute is one.
        //
        // Keyed in the attribute names' own order rather than in the order Amazon SQS'
        // client handed them over: that client answers an UNORDERED map, which hands the
        // same attributes over in a different order each time it is read, and the
        // document below is rendered in the order its members are written. A document
        // whose member order moved would record a state the resource never left.
        let attributes: BTreeMap<String, String> = match client
            .get_queue_attributes()
            .queue_url(queue_url)
            .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::All)
            .send()
            .await
        {
            Ok(answer) => answer
                .attributes
                .unwrap_or_default()
                .into_iter()
                .map(|(attribute, value)| (attribute.as_str().to_string(), value))
                .collect(),
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read an Amazon SQS queue's attributes"
                );
                continue;
            }
        };
        // The document's members are written in the catalogue's own order, so observing
        // an unchanged queue twice renders the SAME document — see
        // [`RECORDED_QUEUE_ATTRIBUTES`].
        let mut configuration = ::serde_json::Map::new();
        for attribute in RECORDED_QUEUE_ATTRIBUTES {
            if let Some(value) = attributes.get(attribute) {
                configuration.insert(
                    attribute.to_string(),
                    ::serde_json::Value::String(value.clone()),
                );
            }
        }
        // The `QueueArn` attribute Amazon SQS itself reports for the queue, which is the
        // ARN its configuration item carries: an ARN the owning service stated is the one
        // the resource has.
        let queue_arn = attributes.get(QUEUE_ARN_ATTRIBUTE).cloned();
        // `ListQueueTags`' `Tags`: the tags a rule's scope can narrow by.
        let tags: BTreeMap<String, String> = client
            .list_queue_tags()
            .queue_url(queue_url)
            .send()
            .await
            .ok()
            .and_then(|answer| answer.tags)
            .unwrap_or_default()
            .into_iter()
            .collect();

        observed.push(DiscoveredResourceEntry {
            resource_type: SQS_QUEUE_RESOURCE_TYPE.to_string(),
            resource_id: queue_url.to_string(),
            resource_name: Some(name.to_string()),
            // The ARN Amazon SQS reported for the queue.
            arn: queue_arn,
            deleted_at: None,
            // The schema version of a configuration item Config itself recorded is not one
            // a caller supplied, and this emulator has no catalogue of them, so none is
            // reported rather than an invented one.
            schema_version_id: None,
            configuration: Some(::serde_json::Value::Object(configuration).to_string()),
            // Amazon SQS reports a queue's whole configuration through one call, so there
            // is no further block to supplement it with.
            supplementary_configuration: BTreeMap::new(),
            tags,
            first_recorded_at: Some(now),
            last_recorded_at: Some(now),
            // No Amazon SQS read above reports another resource this queue is linked to,
            // so the item records no relationship rather than one nothing observed.
            relationships: Vec::new(),
            // The states this queue has already left are kept by the store's own write
            // path, which carries them over from the item this one replaces.
            history: Vec::new(),
        });
    }
    Some(observed)
}

/// An Amazon SQS client over the in-process callback, built the way [`s3_client`] is: the
/// emulator supplies the transport, so there is nothing to retry, time out, cache an
/// identity for or watch for a stalled stream.
fn sqs_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sqs::Client {
    let config = ::aws_sdk_sqs::Config::builder()
        .behavior_version(::aws_sdk_sqs::config::BehaviorVersion::latest())
        .region(::aws_sdk_sqs::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sqs.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sqs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sqs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sqs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sqs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sqs::Client::from_conf(config)
}

/// An S3 client over the in-process callback.
///
/// The callback is the framework's seam for calling another emulated service: it routes
/// in process and signs with the emulator's own credentials, so no network client is
/// configured here. Path-style addressing is what the emulated S3 service routes on
/// (its operations are bound under a `{Bucket}` path label), and the host is built from
/// the fixed service label and [`super::DEFAULT_REGION`], never from anything a caller
/// sent.
pub(crate) fn s3_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{}.amazonaws.com", super::DEFAULT_REGION))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::Scope;

    /// A recorded S3 bucket whose configuration item carries `versioning` as its
    /// versioning status and `algorithms` as the default encryption algorithms in force,
    /// each in the `supplementaryConfiguration` block Config records it in.
    fn bucket(
        name: &str,
        versioning: Option<&str>,
        algorithms: &[&str],
        tags: &[(&str, &str)],
    ) -> RecordedResource {
        let supplementary = BTreeMap::from([
            (
                BUCKET_VERSIONING_BLOCK.to_string(),
                ::serde_json::json!({ "status": versioning }),
            ),
            (
                BUCKET_ENCRYPTION_BLOCK.to_string(),
                ::serde_json::json!({
                    "rules": algorithms
                        .iter()
                        .map(|algorithm| ::serde_json::json!({
                            "applyServerSideEncryptionByDefault": {
                                "sseAlgorithm": algorithm,
                            },
                        }))
                        .collect::<Vec<::serde_json::Value>>(),
                }),
            ),
        ]);
        RecordedResource {
            resource_type: S3_BUCKET_RESOURCE_TYPE.to_string(),
            resource_id: name.to_string(),
            recorded: RecordedConfiguration {
                configuration: ::serde_json::json!({ "name": name }),
                supplementary,
            },
            captured_at: SystemTime::UNIX_EPOCH,
            tags: tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    /// One row of the verdict table: what it is called, the managed rule's identifier,
    /// the bucket's recorded versioning status, its default encryption algorithms and
    /// the verdict the rule must place it at.
    type VerdictCase = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static [&'static str],
        &'static str,
    );

    /// Each managed rule's verdict comes from the configuration item's own members, so
    /// the rows differ along what the recorder recorded: the versioning status (never
    /// set, suspended, enabled) and the default encryption algorithms (none, one).
    ///
    /// `S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED` is the entry this issue did not cite.
    #[test]
    fn a_managed_rule_places_a_recorded_resource_from_its_configuration_item() {
        let cases: [VerdictCase; 8] = [
            (
                "versioning never set",
                "S3_BUCKET_VERSIONING_ENABLED",
                None,
                &[],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                "versioning suspended",
                "S3_BUCKET_VERSIONING_ENABLED",
                Some("Suspended"),
                &[],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                "versioning enabled",
                "S3_BUCKET_VERSIONING_ENABLED",
                Some("Enabled"),
                &[],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "versioning enabled and the bucket encrypted",
                "S3_BUCKET_VERSIONING_ENABLED",
                Some("Enabled"),
                &["AES256"],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "no default encryption",
                "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
                None,
                &[],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                "SSE-S3 default encryption",
                "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
                None,
                &["AES256"],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "SSE-KMS default encryption",
                "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
                Some("Enabled"),
                &["aws:kms"],
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "an encryption rule naming no algorithm",
                "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
                None,
                &[""],
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
        ];

        for (case, identifier, versioning, algorithms, expected) in cases {
            let managed = MANAGED_RULES
                .iter()
                .find(|managed| managed.identifier == identifier)
                .unwrap_or_else(|| panic!("case {case:?}: {identifier} is not in the catalogue"));
            let recorded = bucket("a-bucket", versioning, algorithms, &[]);
            let placed = (managed.verdict)(&recorded.recorded);
            assert_eq!(
                placed, expected,
                "case {case:?}: expected {identifier} to place the bucket at {expected}, \
                 got {placed:?}"
            );
        }
    }

    /// Only a rule Amazon Web Services owns and whose implementation is in the
    /// catalogue is evaluated; the rows differ along the owner and the identifier.
    #[test]
    fn a_rule_this_emulator_cannot_evaluate_has_no_managed_implementation() {
        let cases: [(&str, &str, Option<&str>, bool); 5] = [
            ("a managed rule in the catalogue", OWNER_AWS, Some("S3_BUCKET_VERSIONING_ENABLED"), true),
            (
                "the same identifier, whitespace padded",
                OWNER_AWS,
                Some(" S3_BUCKET_VERSIONING_ENABLED "),
                true,
            ),
            (
                "a managed rule this emulator cannot evaluate",
                OWNER_AWS,
                Some("IAM_PASSWORD_POLICY"),
                false,
            ),
            (
                "a custom Lambda rule",
                "CUSTOM_LAMBDA",
                Some("arn:aws:lambda:us-east-1:000000000000:function:judge"),
                false,
            ),
            ("a rule with no source identifier", OWNER_AWS, None, false),
        ];

        for (case, owner, identifier, expected) in cases {
            let source = Source {
                owner: owner.to_string(),
                source_identifier: identifier.map(ToString::to_string),
                source_details: None,
                custom_policy_details: None,
            };
            let found = managed_rule_of(&source).is_some();
            assert_eq!(
                found, expected,
                "case {case:?}: expected an implementation to be found: {expected}, got {found}"
            );
        }
    }

    /// A rule's scope is what decides WHICH recorded resources it places; the rows
    /// differ along which member of the scope narrows and whether the resource matches.
    #[test]
    fn a_scope_covers_the_recorded_resources_it_narrows_to() {
        let recorded = bucket(
            "the-bucket",
            Some("Enabled"),
            &["AES256"],
            &[("stage", "prod")],
        );
        let scope = |types: Option<&[&str]>, id: Option<&str>, tag: Option<(&str, &str)>| Scope {
            compliance_resource_types: types
                .map(|types| types.iter().map(ToString::to_string).collect()),
            compliance_resource_id: id.map(ToString::to_string),
            tag_key: tag.map(|(key, _)| key.to_string()),
            tag_value: tag.map(|(_, value)| value.to_string()),
            // Narrows nothing: see [`scope_covers_recorded`].
            service_principals: None,
        };
        let cases: [(&str, Option<Scope>, bool); 8] = [
            ("no scope at all", None, true),
            (
                "the resource type it records",
                Some(scope(Some(&[S3_BUCKET_RESOURCE_TYPE]), None, None)),
                true,
            ),
            (
                "another resource type",
                Some(scope(Some(&["AWS::SQS::Queue"]), None, None)),
                false,
            ),
            (
                "the resource type and this resource's id",
                Some(scope(Some(&[S3_BUCKET_RESOURCE_TYPE]), Some("the-bucket"), None)),
                true,
            ),
            (
                "the resource type and another resource's id",
                Some(scope(
                    Some(&[S3_BUCKET_RESOURCE_TYPE]),
                    Some("another-bucket"),
                    None,
                )),
                false,
            ),
            (
                "a tag the resource carries, with its value",
                Some(scope(None, None, Some(("stage", "prod")))),
                true,
            ),
            (
                "a tag the resource carries, with another value",
                Some(scope(None, None, Some(("stage", "dev")))),
                false,
            ),
            (
                "a tag the resource does not carry",
                Some(scope(None, None, Some(("owner", "platform")))),
                false,
            ),
        ];

        for (case, scope, expected) in cases {
            let covers = scope_covers_recorded(scope.as_ref(), &recorded);
            assert_eq!(
                covers, expected,
                "case {case:?}: expected the scope to cover the recorded bucket: \
                 {expected}, got {covers}"
            );
        }
    }
}
