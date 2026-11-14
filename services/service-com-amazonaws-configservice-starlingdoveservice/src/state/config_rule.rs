// SPDX-License-Identifier: CC0-1.0

//! One stored Config rule: the rule itself, the evaluation pass it is owed, and the
//! evaluation results it holds — one verdict per resource and evaluation mode — with the
//! projections the compliance read paths report them through.

use super::*;

/// What identifies one evaluation result WITHIN a rule: the resource type, the
/// resource id and the evaluation mode of its `EvaluationResultQualifier`
/// (`EvaluationResultQualifier` carries `ConfigRuleName`, `ResourceType`, `ResourceId`
/// and `EvaluationMode`; the rule name is the map this key indexes). A rule holds ONE
/// result per key — the last one recorded for that resource in that mode.
///
/// Ordering this triple is what gives [`ConfigRuleEntry::evaluation_results`] a stable
/// total order, so a page of evaluation details can resume at a key.
pub type EvaluationResultKey = (String, String, String);

/// One evaluation result recorded against a Config rule.
#[derive(Debug, Clone)]
pub struct EvaluationResultEntry {
    /// The type of the evaluated resource (`EvaluationResultQualifier$ResourceType`).
    pub resource_type: String,
    /// The id of the evaluated resource (`EvaluationResultQualifier$ResourceId`).
    pub resource_id: String,
    /// The mode the rule was evaluated in
    /// (`EvaluationResultQualifier$EvaluationMode`, `DETECTIVE` or `PROACTIVE`).
    pub evaluation_mode: String,
    /// Whether the resource complies (`EvaluationResult$ComplianceType`).
    pub compliance_type: String,
    /// The supplementary information the evaluation carried
    /// (`EvaluationResult$Annotation`), absent when it carried none.
    pub annotation: Option<String>,
    /// The time of the event that triggered the evaluation
    /// (`EvaluationResultIdentifier$OrderingTimestamp`).
    pub ordering_timestamp: SystemTime,
    /// The time the result was recorded (`EvaluationResult$ResultRecordedTime`).
    pub result_recorded_time: SystemTime,
    /// The time the rule that produced the result was invoked
    /// (`EvaluationResult$ConfigRuleInvokedTime`).
    pub config_rule_invoked_time: SystemTime,
    /// The resource evaluation this result came from
    /// (`EvaluationResultIdentifier$ResourceEvaluationId`), set only for a result a
    /// `StartResourceEvaluation` run produced.
    pub resource_evaluation_id: Option<String>,
}

impl EvaluationResultEntry {
    /// The result of evaluating the resource `resource_type`/`resource_id` in
    /// `evaluation_mode` at `at`, carrying no annotation and belonging to no resource
    /// evaluation run.
    ///
    /// One evaluation is triggered, invoked and recorded at the same instant here, so
    /// `at` is all three timestamps; a caller that needs them apart sets the fields.
    pub fn new(
        resource_type: &str,
        resource_id: &str,
        evaluation_mode: &str,
        compliance_type: &str,
        at: SystemTime,
    ) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            evaluation_mode: evaluation_mode.to_string(),
            compliance_type: compliance_type.to_string(),
            annotation: None,
            ordering_timestamp: at,
            result_recorded_time: at,
            config_rule_invoked_time: at,
            resource_evaluation_id: None,
        }
    }

    /// The key this result is stored under, which is its identity within a rule.
    pub fn key(&self) -> EvaluationResultKey {
        (
            self.resource_type.clone(),
            self.resource_id.clone(),
            self.evaluation_mode.clone(),
        )
    }

    /// The modelled `EvaluationResult` for the rule named `config_rule_name`, read
    /// back out of state.
    ///
    /// `ResultToken` stays unset: the token associates an evaluation with the
    /// `PutEvaluations` call that delivered it, and the read path does not hand one
    /// back.
    pub fn to_shape(&self, config_rule_name: &str) -> crate::ops::types::EvaluationResult {
        crate::ops::types::EvaluationResult {
            evaluation_result_identifier: Some(crate::ops::types::EvaluationResultIdentifier {
                evaluation_result_qualifier: Some(crate::ops::types::EvaluationResultQualifier {
                    config_rule_name: Some(config_rule_name.to_string()),
                    resource_type: Some(self.resource_type.clone()),
                    resource_id: Some(self.resource_id.clone()),
                    evaluation_mode: Some(self.evaluation_mode.clone()),
                }),
                ordering_timestamp: Some(timestamp(self.ordering_timestamp)),
                resource_evaluation_id: self.resource_evaluation_id.clone(),
            }),
            compliance_type: Some(self.compliance_type.clone()),
            result_recorded_time: Some(timestamp(self.result_recorded_time)),
            config_rule_invoked_time: Some(timestamp(self.config_rule_invoked_time)),
            annotation: self.annotation.clone(),
            result_token: None,
        }
    }

    /// The modelled `AggregateEvaluationResult` an AGGREGATE read reports this result
    /// as: the same verdict, plus the source account and Region the data was aggregated
    /// FROM (`AggregateEvaluationResult$AccountId`, `$AwsRegion`), which is what tells
    /// one source's result from another's in an aggregator's answer.
    pub fn to_aggregate_shape(
        &self,
        config_rule_name: &str,
        account_id: &str,
        region: &str,
    ) -> crate::ops::types::AggregateEvaluationResult {
        let reported = self.to_shape(config_rule_name);
        crate::ops::types::AggregateEvaluationResult {
            evaluation_result_identifier: reported.evaluation_result_identifier,
            compliance_type: reported.compliance_type,
            result_recorded_time: reported.result_recorded_time,
            config_rule_invoked_time: reported.config_rule_invoked_time,
            annotation: reported.annotation,
            account_id: Some(account_id.to_string()),
            aws_region: Some(region.to_string()),
        }
    }

    /// The modelled `ConformancePackEvaluationResult` a CONFORMANCE PACK's compliance
    /// details report this result as: the same verdict, the same identifier and the same
    /// timestamps the rule's own evaluation-details read reports
    /// ([`Self::to_shape`]), under the name the pack's deployment gave the rule.
    ///
    /// `None` when the result's `ComplianceType` is not one a conformance pack's
    /// compliance holds (`ConformancePackEvaluationResult$ComplianceType` targets
    /// `ConformancePackComplianceType`, whose values are `COMPLIANT`, `NON_COMPLIANT`
    /// and `INSUFFICIENT_DATA`), so a result recorded with a value outside that set has
    /// nothing this report can carry and is left out rather than reported as another
    /// type.
    pub fn to_conformance_pack_shape(
        &self,
        config_rule_name: &str,
    ) -> Option<ConformancePackEvaluationResult> {
        if !crate::ops::types::CONFORMANCE_PACK_COMPLIANCE_TYPE_VALUES
            .contains(&self.compliance_type.as_str())
        {
            return None;
        }
        let reported = self.to_shape(config_rule_name);
        Some(ConformancePackEvaluationResult {
            compliance_type: self.compliance_type.clone(),
            evaluation_result_identifier: reported.evaluation_result_identifier?,
            config_rule_invoked_time: timestamp(self.config_rule_invoked_time),
            result_recorded_time: timestamp(self.result_recorded_time),
            annotation: self.annotation.clone(),
        })
    }
}

/// The invocation-and-evaluation history of one Config rule, as
/// `DescribeConfigRuleEvaluationStatus` reports it.
///
/// An evaluation pass visits the recorded configuration state of the resources in the
/// rule's scope and records one evaluation result per resource it placed
/// (`StartConfigRulesEvaluation`: it "re-runs an evaluation against the last known
/// state of your resources"). This emulator holds no recorded configuration items yet
/// — `PutResourceConfig` is still scaffolded — so a pass visits nothing, runs no rule
/// logic at all and records no evaluation result, which is why a rule's compliance
/// stays `INSUFFICIENT_DATA` ([`ConfigRuleEntry::compliance`]) while its pass
/// completes successfully.
#[derive(Debug, Clone, Default)]
pub struct RuleEvaluationEntry {
    /// When set, the instant Config invoked the evaluation pass this rule is owed.
    /// Recorded when the pass starts, so the instant the rule reports never depends
    /// on the span the pass takes.
    pub invoked_at: Option<SystemTime>,
    /// When set, the instant at which the evaluation pass this rule is owed
    /// completes. Materialised by [`Self::settle`].
    pub completes_at: Option<SystemTime>,
    /// Whether a pass has ever completed for this rule
    /// (`ConfigRuleEvaluationStatus$FirstEvaluationStarted`: "Config has evaluated
    /// your Amazon Web Services resources against the rule at least once").
    pub first_evaluation_started: bool,
    /// When Config last invoked the rule, i.e. when the last completed pass started.
    pub last_successful_invocation_at: Option<SystemTime>,
    /// When the last completed pass finished evaluating the resources in scope.
    pub last_successful_evaluation_at: Option<SystemTime>,
    /// Whether the pass in flight was asked for ON DEMAND — by a
    /// `StartConfigRulesEvaluation` call — rather than run by Config itself because
    /// the rule was written.
    ///
    /// The on-demand path states a precondition of its own about the pass IT started:
    /// "An existing StartConfigRulesEvaluation call for the specified rules must
    /// complete before you can call the API again" (`StartConfigRulesEvaluation`).
    /// That sentence is about an existing on-demand call, not about the pass Config
    /// runs by itself when a rule is created or updated ("When you create a rule,
    /// Config evaluates your resources against the rule automatically" — which the
    /// same page says needs no call from the caller), so which kind of pass is in
    /// flight has to be part of the record for the precondition to be checkable.
    /// Cleared when the pass completes, like every other in-flight member here.
    pub on_demand: bool,
    /// The `ResultToken` Config minted for the evaluation it last invoked, for a rule
    /// whose verdicts are DELIVERED to Config rather than computed by it
    /// ([`ConfigRuleEntry::delivers_own_evaluations`]).
    ///
    /// Config hands the token to whatever performs the evaluation and takes it back on
    /// `PutEvaluations` ("An encrypted token that associates an evaluation with an
    /// Config rule. Identifies the rule and the event that triggered the evaluation"),
    /// so it is the record of WHICH rule and which invocation a delivery belongs to.
    /// `None` for a rule Config evaluates itself, which delivers nothing back.
    pub result_token: Option<String>,
    /// The `ResultToken` of the pass whose EVALUATOR Config has already invoked
    /// ([`super::custom_lambda_rule`]).
    ///
    /// A rule whose verdicts are delivered back is evaluated by invoking the function the
    /// account pointed it at, and that invocation is owed ONCE per pass Config invoked:
    /// this is the record of which pass has had its invocation made, so the in-process
    /// pump does not invoke the function again for the same pass. It stays set when the
    /// pass settles, because the invocation it names has happened.
    pub dispatched_token: Option<String>,
}

impl RuleEvaluationEntry {
    /// The evaluation history of a rule Config has just invoked an evaluation pass
    /// for, which is every rule a write creates.
    pub fn invoked_at(at: SystemTime) -> Self {
        let mut evaluation = Self::default();
        evaluation.invoke(at);
        evaluation
    }

    /// Record that Config owes this rule an evaluation pass, invoked at `at`, because
    /// the rule was written.
    pub fn invoke(&mut self, at: SystemTime) {
        self.invoked_at = Some(at);
        self.completes_at = Some(at + RULE_EVALUATION_SETTLE_AFTER);
        self.on_demand = false;
    }

    /// Record that Config owes this rule an evaluation pass, invoked at `at`, because
    /// a caller asked for one on demand (`StartConfigRulesEvaluation`: it "re-runs an
    /// evaluation against the last known state of your resources").
    ///
    /// The pass itself is the same pass a write owes — same invocation instant, same
    /// span before it completes — so the two share [`Self::invoke`]; only
    /// [`Self::on_demand`] tells them apart.
    pub fn invoke_on_demand(&mut self, at: SystemTime) {
        self.invoke(at);
        self.on_demand = true;
    }

    /// Whether a pass a `StartConfigRulesEvaluation` call started has still not
    /// completed as of `now`. See [`Self::on_demand`].
    pub fn on_demand_in_progress(&self, now: SystemTime) -> bool {
        self.on_demand && self.in_progress(now)
    }

    /// Whether the pass this rule is owed has still not completed as of `now`.
    ///
    /// Reading the pending pass is what tells a caller the rule's reported instants
    /// are the previous pass's, not this one's.
    pub fn in_progress(&self, now: SystemTime) -> bool {
        self.completes_at.is_some_and(|due| now < due)
    }

    /// Materialise the completion the clock has already made due.
    ///
    /// This is the ONLY write a reporting operation performs on a rule: the pass was
    /// owed at `completes_at` whether or not anybody read the rule's evaluation
    /// status. A pass that is still running ([`Self::in_progress`]) is left alone, so
    /// the rule keeps reporting the previous pass's instants until this one is due.
    pub fn settle(&mut self, now: SystemTime) {
        if self.in_progress(now) {
            return;
        }
        if let Some(due) = self.completes_at {
            self.first_evaluation_started = true;
            // The invocation instant is the one recorded when the pass started; a
            // pass always has one, so an absent `invoked_at` can only mean the
            // completion instant itself.
            self.last_successful_invocation_at = Some(self.invoked_at.unwrap_or(due));
            self.last_successful_evaluation_at = Some(due);
            self.invoked_at = None;
            self.completes_at = None;
            self.on_demand = false;
        }
    }
}

/// One persisted Config rule.
#[derive(Debug, Clone)]
pub struct ConfigRuleEntry {
    pub name: String,
    pub arn: String,
    pub id: String,
    pub description: Option<String>,
    pub scope: Option<Scope>,
    pub source: Source,
    pub input_parameters: Option<String>,
    pub maximum_execution_frequency: Option<String>,
    pub state: String,
    pub evaluation_modes: Vec<EvaluationModeConfiguration>,
    pub rule_evaluation_visibility: Option<String>,
    /// The service principal of the Amazon Web Services service that created this rule
    /// (`ConfigRule$CreatedBy`: "Service principal name of the service that created the
    /// rule"), which is set for a rule a SERVICE deployed — a conformance pack deploys
    /// its rules through its own service-linked role
    /// ([`CONFORMANCE_PACK_SERVICE_PRINCIPAL`]) — and unset for a rule the account
    /// created through `PutConfigRule`.
    pub created_by: Option<String>,
    pub created_at: SystemTime,
    /// The rule's tags, taken at creation by `PutConfigRule` and changed after that
    /// by `TagResource` and `UntagResource` (`PutConfigRule`: "Use TagResource and
    /// UntagResource to update tags after creation").
    pub tags: TagSet,
    /// The last evaluation result recorded for each resource this rule evaluated,
    /// keyed by [`EvaluationResultKey`] so the map is the single record of both what
    /// `GetComplianceDetailsByConfigRule` reports and what the rule's own compliance
    /// rolls up from ([`Self::compliance`]).
    ///
    /// Three writers record into it. `PutExternalEvaluation` records one for a Config
    /// Process Check rule, whose evaluations are delivered from outside Config: its
    /// evaluated resource is the account this store stands for, so a process check rule
    /// holds at most ONE result and a delivery revises it in place. `PutEvaluations`
    /// records one per resource a Custom Lambda rule's own evaluation placed, identified
    /// by the `ResultToken` Config minted when it invoked that rule
    /// ([`ConfigRuleEntry::mint_evaluation_result_token`]). The configuration recorder's
    /// own pass (`state::recording`) records one per resource it observed for each rule
    /// in whose scope that resource falls. A verdict any writer CHANGES is what
    /// publishes the compliance-change notification. A rule nothing has evaluated
    /// carries no results — an empty page of evaluation details, and a roll-up of
    /// `INSUFFICIENT_DATA`.
    ///
    /// BOUNDED at [`EVALUATION_RESULTS_KEPT_PER_RULE`] entries: write it through
    /// [`ConfigRuleEntry::record_evaluation_result`].
    pub evaluation_results: BTreeMap<EvaluationResultKey, EvaluationResultEntry>,
    /// When Config invoked this rule and when those invocations finished evaluating
    /// the resources in scope, as `DescribeConfigRuleEvaluationStatus` reports it.
    pub evaluation: RuleEvaluationEntry,
}

impl ConfigRuleEntry {
    /// The modelled `ConfigRule` as a READ reports it.
    ///
    /// `Source.CustomPolicyDetails.PolicyText` is reported EMPTY: the describe path
    /// does not hand the stored Guard policy body back, which is why the model gives
    /// `PolicyText` a zero minimum length and why the body has an operation of its
    /// own (`GetCustomRulePolicy`). The stored body is untouched — only this
    /// projection of it is blank.
    pub fn to_describe_shape(&self) -> ConfigRule {
        let mut source = self.source.clone();
        if let Some(custom_policy) = source.custom_policy_details.as_mut() {
            custom_policy.policy_text = String::new();
        }
        ConfigRule {
            config_rule_name: Some(self.name.clone()),
            config_rule_arn: Some(self.arn.clone()),
            config_rule_id: Some(self.id.clone()),
            description: self.description.clone(),
            scope: self.scope.clone(),
            source,
            input_parameters: self.input_parameters.clone(),
            maximum_execution_frequency: self.maximum_execution_frequency.clone(),
            config_rule_state: Some(self.state.clone()),
            created_by: self.created_by.clone(),
            evaluation_modes: Some(self.evaluation_modes.clone()),
            rule_evaluation_visibility: self.rule_evaluation_visibility.clone(),
        }
    }

    /// Whether this rule is a Config Process Check rule, i.e. the Config Managed rule
    /// whose evaluations are delivered from OUTSIDE Config rather than computed by
    /// Config from recorded configuration state. A process check is identified by the
    /// managed rule it is an instance of ([`names_process_check`]), which is the ONE
    /// predicate for that question so this and the write path that refuses the
    /// identifier cannot disagree about a value's spelling.
    pub fn is_process_check(&self) -> bool {
        names_process_check(&self.source)
    }

    /// Whether this rule's verdicts are DELIVERED to Config with `PutEvaluations`
    /// rather than computed by Config itself.
    ///
    /// A Config Custom Lambda rule is evaluated by the function the account points the
    /// rule at, and that function "is required in every Lambda function that is invoked
    /// by an Config rule" to deliver what it found with `PutEvaluations`
    /// (`PutEvaluations`). Config's own managed and Custom Policy rules are evaluated
    /// inside Config, so nothing delivers verdicts for them.
    pub fn delivers_own_evaluations(&self) -> bool {
        self.source.owner == OWNER_CUSTOM_LAMBDA
    }

    /// Mint the `ResultToken` for the evaluation Config has just invoked on this rule,
    /// or clear the one it carried when this rule is not one that delivers its own
    /// verdicts ([`Self::delivers_own_evaluations`] — which a write can change, by
    /// pointing the rule at another source).
    ///
    /// The token belongs to ONE invocation, so it is replaced every time Config invokes
    /// the rule again, and the last one stays valid until then: Config has no further
    /// invocation to associate a delivery with.
    pub fn mint_evaluation_result_token(&mut self, invoked_at: SystemTime) {
        self.evaluation.result_token = self
            .delivers_own_evaluations()
            .then(|| mint_result_token(&self.name, invoked_at));
    }

    /// Record `result` as this rule's verdict on the resource it names, keeping the
    /// rule's results bounded.
    ///
    /// This is the ONE place [`Self::evaluation_results`] GROWS: past
    /// [`EVALUATION_RESULTS_KEPT_PER_RULE`] results it drops the one whose evaluation
    /// is OLDEST (by ordering timestamp, then by key so the choice is deterministic),
    /// so repeated deliveries cannot grow this process's state without end. A result
    /// for a resource the rule already holds one for REPLACES it: a rule reports its
    /// last verdict per resource and evaluation mode.
    pub fn record_evaluation_result(&mut self, result: EvaluationResultEntry) {
        self.evaluation_results.insert(result.key(), result);
        while self.evaluation_results.len() > EVALUATION_RESULTS_KEPT_PER_RULE {
            let oldest = self
                .evaluation_results
                .values()
                .min_by_key(|recorded| (recorded.ordering_timestamp, recorded.key()))
                .map(EvaluationResultEntry::key);
            let Some(key) = oldest else {
                break;
            };
            self.evaluation_results.remove(&key);
        }
    }

    /// Record `result` as this rule's verdict on the resource it names, and answer the
    /// compliance CHANGE it amounts to — the verdict that now holds and the one it
    /// replaced — or `None` when the verdict did not change.
    ///
    /// Every writer that EVALUATES a rule goes through here, so "the verdict changed" is
    /// one statement: the verdict it replaces is read before the write, because that is
    /// what a compliance-change notification reports the resource changed FROM, and a
    /// verdict that did not change notifies nobody while the evaluation itself is still
    /// recorded.
    pub fn record_verdict(&mut self, result: EvaluationResultEntry) -> Option<ComplianceChange> {
        let replaced = self.evaluation_results.get(&result.key()).cloned();
        self.record_evaluation_result(result.clone());
        let changed = replaced
            .as_ref()
            .is_none_or(|previous| previous.compliance_type != result.compliance_type);
        changed.then(|| ComplianceChange {
            rule_name: self.name.clone(),
            rule_arn: self.arn.clone(),
            new_result: result,
            old_result: replaced,
        })
    }

    /// Whether `DescribeConfigRuleEvaluationStatus` reports a status for this rule.
    ///
    /// Config reports one for its managed rules and for Custom Policy rules only
    /// (`ConfigRuleEvaluationStatus`: "Status information for your Config Managed
    /// rules and Config Custom Policy rules. … This operation does not return status
    /// information about Config Custom Lambda rules.").
    pub fn reports_evaluation_status(&self) -> bool {
        self.source.owner != OWNER_CUSTOM_LAMBDA
    }

    /// The modelled `ConfigRuleEvaluationStatus` as a READ reports it.
    ///
    /// `FirstActivatedTime` is when this rule was written, which is when Config
    /// activated it. The failure members (`LastFailedInvocationTime`,
    /// `LastFailedEvaluationTime`, `LastErrorCode`, `LastErrorMessage`) are reported
    /// ABSENT because no evaluation pass of this emulator fails: there is no rule
    /// logic to fail and no resource to fail on. The debug-log members
    /// (`LastDebugLogDeliveryStatus`, `LastDebugLogDeliveryStatusReason`,
    /// `LastDebugLogDeliveryTime`) are absent for the same reason — nothing has
    /// attempted to deliver a Custom Policy debug log, which is the delivery path
    /// `DeliverConfigSnapshot` owns and which is still scaffolded. `LastDeactivatedTime`
    /// is absent because no operation of this service turns a rule off: `DeleteConfigRule`
    /// removes it outright.
    pub fn to_evaluation_status_shape(&self) -> ConfigRuleEvaluationStatus {
        ConfigRuleEvaluationStatus {
            config_rule_name: Some(self.name.clone()),
            config_rule_arn: Some(self.arn.clone()),
            config_rule_id: Some(self.id.clone()),
            first_activated_time: Some(timestamp(self.created_at)),
            last_deactivated_time: None,
            first_evaluation_started: self.evaluation.first_evaluation_started,
            last_successful_invocation_time: self
                .evaluation
                .last_successful_invocation_at
                .map(timestamp),
            last_successful_evaluation_time: self
                .evaluation
                .last_successful_evaluation_at
                .map(timestamp),
            last_failed_invocation_time: None,
            last_failed_evaluation_time: None,
            last_error_code: None,
            last_error_message: None,
            last_debug_log_delivery_status: None,
            last_debug_log_delivery_status_reason: None,
            last_debug_log_delivery_time: None,
        }
    }

    /// This rule's `ComplianceType`: the roll-up of the evaluation results recorded
    /// for it — noncompliant if any evaluated resource is noncompliant, compliant if
    /// at least one is compliant and none is not, and `INSUFFICIENT_DATA` when no
    /// result places it either way (which includes a rule nothing has evaluated).
    pub fn compliance(&self) -> &'static str {
        // One roll-up rule for the whole service: see
        // [`resource_evaluation::roll_up_compliance`].
        resource_evaluation::roll_up_compliance(
            self.evaluation_results
                .values()
                .map(|recorded| recorded.compliance_type.as_str()),
        )
    }

    /// The `ComplianceType` Config has PLACED this rule at ([`Self::compliance`]), or
    /// `None` for a rule nothing has evaluated yet.
    ///
    /// The two are not interchangeable: a report of what Config FOUND carries a rule
    /// only once an evaluation has produced a result for it, while
    /// [`Self::compliance`] answers what the results recorded so far roll up to, which
    /// for a rule with no results at all is the `INSUFFICIENT_DATA` of a rule nothing
    /// has placed either way. This is the ONE place that distinction is drawn, so every
    /// read that must not invent a result for an unevaluated rule asks here.
    pub fn recorded_compliance(&self) -> Option<&'static str> {
        (!self.evaluation_results.is_empty()).then(|| self.compliance())
    }

    /// This rule's modelled `Compliance`: the `ComplianceType` it rolls up to
    /// ([`Self::compliance`]) and how many resources contribute that result.
    ///
    /// `Compliance$ComplianceContributorCount` is "the number of Amazon Web Services
    /// resources or Config rules that cause a result of NON_COMPLIANT", so it is
    /// reported only where such resources exist — a rule no evaluation result places
    /// as noncompliant carries no contributor count — and it is counted up to
    /// [`COMPLIANCE_RESOURCE_CAP`] because the contributors counted here are
    /// resources.
    pub fn compliance_shape(&self) -> Compliance {
        let contributors = self
            .evaluation_results
            .values()
            .filter(|recorded| recorded.compliance_type == COMPLIANCE_TYPE_NON_COMPLIANT)
            .count();
        Compliance {
            compliance_type: Some(self.compliance().to_string()),
            compliance_contributor_count: (contributors > 0).then(|| {
                capped_count(
                    i64::try_from(contributors).unwrap_or(i64::MAX),
                    COMPLIANCE_RESOURCE_CAP,
                )
            }),
        }
    }
}

/// The MANAGED rule `source` names, by its identifier, or `None` when it names none.
///
/// The OWNER decides whether the identifier is a managed rule's at all: a rule owned by
/// Amazon Web Services is an instance of one of Config's own managed rules, named by
/// `Source$SourceIdentifier` ("For Config Managed rules, a predefined identifier from a
/// list"), while a Config Custom Lambda rule's identifier is the ARN of the function
/// that evaluates it and a Custom Policy rule carries its policy instead. The identifier
/// is TRIMMED, because the wire carries it as the caller typed it, and this is the ONE
/// place that reading is taken — the managed-rule catalogues ([`managed_rule_of`],
/// [`account_managed_rule_of`]), the process check predicate
/// ([`names_process_check`]) and the withdrawn set below all look it up through here, so
/// no two of them can disagree about which managed rule a source names.
pub fn managed_rule_identifier(source: &Source) -> Option<&str> {
    if source.owner != OWNER_AWS {
        return None;
    }
    source
        .source_identifier
        .as_deref()
        .map(str::trim)
        .filter(|identifier| !identifier.is_empty())
}

/// Whether `source` names the managed rule a Config process check IS
/// ([`SOURCE_IDENTIFIER_PROCESS_CHECK`]).
pub fn names_process_check(source: &Source) -> bool {
    managed_rule_identifier(source) == Some(SOURCE_IDENTIFIER_PROCESS_CHECK)
}

/// Point `source` at the managed rule a Config process check IS.
///
/// A process check rule cannot be WRITTEN any more — Config has withdrawn that managed
/// rule ([`WITHDRAWN_MANAGED_RULE_IDENTIFIERS`]) — while `PutExternalEvaluation` still
/// applies to the rules an account created while Config did offer it. This is how such a
/// rule comes to exist in a store without `PutConfigRule` creating one, so a suite about
/// what happens to a rule's DELIVERED verdicts has the rule kind those verdicts belong
/// to, and reaches it through this one documented seam rather than by spelling the
/// identifier into the store itself.
pub fn point_at_process_check(source: &mut Source) {
    source.source_identifier = Some(SOURCE_IDENTIFIER_PROCESS_CHECK.to_string());
}

/// The `Source$SourceIdentifier`s of the MANAGED rules Config no longer offers.
///
/// A rule owned by Amazon Web Services is an instance of one of Config's own managed
/// rules, and the list `Source$SourceIdentifier` points at — *List of Config Managed
/// Rules* — is what Config offers. An identifier WITHDRAWN from that list names no
/// managed rule Config can evaluate, so nothing can be created or updated to be an
/// instance of it.
///
/// RECORDED behaviour names this entry: `PutConfigRule` for an Amazon Web
/// Services-owned rule whose identifier is [`SOURCE_IDENTIFIER_PROCESS_CHECK`] is
/// refused as an invalid source identifier, while `PutExternalEvaluation` still applies
/// to the process check rules an account created while Config did offer it.
///
/// The set is stated as what Config has WITHDRAWN rather than as what it offers because
/// *List of Config Managed Rules* holds hundreds of entries that this emulator does not
/// enumerate: the catalogues it does carry ([`MANAGED_RULES`], [`ACCOUNT_MANAGED_RULES`])
/// are the managed rules it EVALUATES, which is a far smaller set than the ones Config
/// accepts, so refusing every identifier outside them would refuse identifiers Config
/// offers. An identifier that is merely unknown to this emulator is therefore accepted,
/// as Config accepts the hundreds it offers; only one Config no longer offers at all is
/// refused.
pub const WITHDRAWN_MANAGED_RULE_IDENTIFIERS: [&str; 1] = [SOURCE_IDENTIFIER_PROCESS_CHECK];

/// The identifier `source` names that Config no longer offers as a managed rule, or
/// `None` when it names one Config does offer — or none at all.
pub fn withdrawn_managed_rule_identifier(source: &Source) -> Option<&str> {
    managed_rule_identifier(source)
        .filter(|identifier| WITHDRAWN_MANAGED_RULE_IDENTIFIERS.contains(identifier))
}

/// How Config words `InvalidParameterValueException` for a rule whose source identifier
/// names no managed rule Config offers.
///
/// RECORDED behaviour: the sentence states the identifier the request carried and where
/// the list of the ones Config does offer is — the only two things a caller needs to
/// pick another — and the identifier is the ONE part of it taken from the request.
pub fn invalid_source_identifier_message(identifier: &str) -> String {
    format!(
        "The sourceIdentifier {identifier} is invalid. Please refer to the documentation \
         for a list of valid sourceIdentifiers that can be used when AWS is the Owner."
    )
}
