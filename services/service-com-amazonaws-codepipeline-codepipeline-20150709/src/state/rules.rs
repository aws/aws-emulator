// SPDX-License-Identifier: CC0-1.0

//! The stage-CONDITION plane: what evaluating a stage's conditions records, and what
//! the execution does with the outcome.
//!
//! # The rule this module implements
//!
//! A stage may declare conditions at three sites (`StageDeclaration$beforeEntry`,
//! `$onSuccess`, `$onFailure`), and a condition is made up of `rules` and the `result`
//! to apply. Evaluating one of those rules is what brings a RULE EXECUTION into being
//! — the record `ListRuleExecutions` reports — so a pipeline whose stages declare
//! conditions has one rule execution per rule per evaluation, in the order the rules
//! were evaluated, and a pipeline that has never run has none.
//!
//! The conditions of the two sites an execution reaches on its way FORWARD are
//! evaluated here: `beforeEntry` when the execution is about to enter the stage, and
//! `onSuccess` once the stage's actions have all succeeded. `onFailure` is the
//! remediation site — its `result` is a `ROLLBACK` or a `RETRY` of a stage that has
//! already failed — and this emulator starts neither on its own, so nothing is
//! evaluated there and no rule execution is recorded for it rather than one being
//! reported for work nothing performed.
//!
//! # What this plane can and cannot evaluate
//!
//! A rule is evaluated by its PROVIDER (`RuleTypeId$provider`, "such as the
//! `DeploymentWindow` rule"; the providers are "the rules listed in the CodePipeline
//! rule reference" the member's documentation links to). One of them decides its
//! verdict from this service's OWN state and is therefore evaluated here:
//! `VariableCheck`, which compares a variable of the execution against a value with an
//! operator. Every other published provider decides from somewhere this emulator
//! cannot reach — a deployment window from the account's calendar, an alarm from
//! CloudWatch, a Lambda rule from the function it invokes — so its evaluation reaches
//! no verdict and is recorded with the `RuleExecutionStatus` of an evaluation that was
//! given up (`Abandoned`) carrying the reason, never `Succeeded`, which would report a
//! verdict nothing reached.
//!
//! `Condition$result` is "the action to be done when the condition is MET. For example,
//! rolling back an execution for a failure condition", so the result is applied only
//! when a rule actually reached a FAILING verdict. An evaluation that reached none has
//! not met the condition: the execution carries on and what it reports about the gate
//! is the abandoned rule execution, because a stage failed or skipped on a verdict
//! nobody reached would be as invented as one passed on it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    Condition, ErrorDetails, RuleDeclaration, RuleExecutionDetail, RuleExecutionInput,
    RuleExecutionOutput, RuleExecutionResult, RuleTypeId,
};

use super::{random_uuid_v4, to_date_time};

/// The `ConditionType` of the conditions that gate ENTRY to a stage, from the model's
/// enum (`crate::ops::types::CONDITION_TYPE_VALUES` -> `BEFORE_ENTRY`, `ON_SUCCESS`).
pub const CONDITION_TYPE_BEFORE_ENTRY: &str = crate::ops::types::CONDITION_TYPE_VALUES[0];

/// The `ConditionType` of the conditions evaluated once a stage has succeeded.
pub const CONDITION_TYPE_ON_SUCCESS: &str = crate::ops::types::CONDITION_TYPE_VALUES[1];

/// The `RuleExecutionStatus` a rule execution carries when its evaluation was given up
/// without a verdict, from the model's enum
/// (`crate::ops::types::RULE_EXECUTION_STATUS_VALUES` -> `Abandoned`, `Failed`,
/// `InProgress`, `Succeeded`).
pub const RULE_EXECUTION_ABANDONED: &str = "Abandoned";

/// The `RuleExecutionStatus` of an evaluation whose verdict was that the rule holds,
/// from the same enum.
pub const RULE_EXECUTION_SUCCEEDED: &str = "Succeeded";

/// The `RuleExecutionStatus` of an evaluation whose verdict was that it does not.
pub const RULE_EXECUTION_FAILED: &str = "Failed";

/// The `Result` that SKIPS the stage whose condition was met, from the model's enum
/// (`crate::ops::types::RESULT_VALUES` -> `FAIL`, `RETRY`, `ROLLBACK`, `SKIP`).
pub const RESULT_SKIP: &str = "SKIP";

/// The `Result` that FAILS the stage whose condition was met — and what a condition
/// stating none resolves to, because a stage whose gate tripped cannot simply proceed.
pub const RESULT_FAIL: &str = "FAIL";

/// The rule provider whose verdict is a function of the EXECUTION's own state, and so
/// the one this plane evaluates.
///
/// `RuleDeclaration$name` names it as the example of a rule ("such as
/// `VariableCheck`"), and what it checks is a variable of the pipeline execution
/// against a value — nothing outside this service — so the verdict is computed here
/// from the variables the execution carries rather than reported without one.
pub const PROVIDER_VARIABLE_CHECK: &str = "VariableCheck";

/// The `VariableCheck` rule's configuration keys.
///
/// `RuleDeclaration$configuration` is a free-form `RuleConfigurationMap` ("the action
/// configuration fields for the rule"), so the keys are the PROVIDER's, from the
/// CodePipeline rule reference `RuleTypeId$provider` links to: the variable to check,
/// the value to check it against, and the operator to compare them with.
const CONFIG_VARIABLE: &str = "Variable";
/// See [`CONFIG_VARIABLE`].
const CONFIG_VALUE: &str = "Value";
/// See [`CONFIG_VARIABLE`].
const CONFIG_OPERATOR: &str = "Operator";

/// The comparison operators the `VariableCheck` rule reference publishes, each with
/// what it compares: equality, inequality, the four orderings, and the two text
/// predicates — whether the variable CONTAINS the value, and whether it MATCHES the
/// regular expression the value states.
///
/// Every one of them compares the two sides the configuration names and nothing else,
/// so each is a verdict this plane computes. An operator outside this set is one it
/// cannot apply, and the evaluation reaches no verdict rather than guessing at one.
const OPERATOR_EQ: &str = "EQ";
/// See [`OPERATOR_EQ`].
const OPERATOR_NE: &str = "NE";
/// See [`OPERATOR_EQ`].
const OPERATOR_GT: &str = "GT";
/// See [`OPERATOR_EQ`].
const OPERATOR_GE: &str = "GE";
/// See [`OPERATOR_EQ`].
const OPERATOR_LT: &str = "LT";
/// See [`OPERATOR_EQ`].
const OPERATOR_LE: &str = "LE";
/// See [`OPERATOR_EQ`].
const OPERATOR_CONTAINS: &str = "CONTAINS";
/// See [`OPERATOR_EQ`].
const OPERATOR_MATCHES: &str = "MATCHES";

/// The most rule executions this emulator retains per pipeline; recording another
/// drops the oldest beyond this window.
///
/// A bound is required for the same reason the execution history has one: a record is
/// appended for every rule of every condition every execution evaluates, and nothing
/// releases them but the pipeline being deleted. AWS bounds its own rule-execution
/// history by age rather than by count, so the figure is this emulator's, sized like
/// [`super::MAX_EXECUTION_HISTORY_PER_PIPELINE`].
pub const MAX_RULE_EXECUTIONS_PER_PIPELINE: usize = 1000;

/// The rule executions recorded per pipeline name, oldest first (so the newest-first
/// listing is this sequence reversed).
///
/// A field of the store of its own, so the driver can write it while it holds the
/// execution it is stepping — the same disjoint-field split the job queue uses.
pub type RuleExecutionLog = BTreeMap<String, Vec<RuleExecutionRecord>>;

/// One evaluation of one stage condition's rule: what `ListRuleExecutions` reports.
#[derive(Debug, Clone)]
pub struct RuleExecutionRecord {
    /// The id this evaluation is reported under.
    pub rule_execution_id: String,
    /// The execution whose stage the condition belongs to.
    pub pipeline_execution_id: String,
    /// The pipeline version that execution pinned — the declaration the rule was read
    /// from.
    pub pipeline_version: i32,
    /// The stage whose condition declared the rule.
    pub stage_name: String,
    /// `RuleDeclaration$name`.
    pub rule_name: String,
    /// Which of the stage's condition sites declared it, as a `ConditionType` value.
    pub condition_type: String,
    /// `RuleDeclaration$ruleTypeId`.
    pub rule_type_id: RuleTypeId,
    /// The configuration as DECLARED.
    pub configuration: BTreeMap<String, String>,
    /// The same configuration with every `#{namespace.key}` placeholder resolved
    /// against the variables the execution carries.
    pub resolved_configuration: BTreeMap<String, String>,
    /// `RuleDeclaration$roleArn`, when the declaration states one.
    pub role_arn: Option<String>,
    /// `RuleDeclaration$region`, when the declaration states one.
    pub region: Option<String>,
    /// A `RuleExecutionStatus` value.
    pub status: String,
    /// Why the rule reached that status.
    pub error: Option<(String, String)>,
    /// When the evaluation started.
    pub start_time: SystemTime,
    /// When this record last changed.
    pub last_update_time: SystemTime,
}

impl RuleExecutionRecord {
    /// This evaluation as `GetPipelineState` reports it inside the state of the rule it
    /// evaluated (`RuleState$latestExecution`).
    ///
    /// The same run, reported from the same record as the listing above: only the members
    /// `RuleExecution` carries differ. `token` is absent — it is "the system-generated
    /// token used to identify a unique request", and no request of a caller's ran this
    /// rule; the condition plane did — and so is `lastUpdatedBy`, for the reason the
    /// listing's `updatedBy` is: nobody changed this run.
    pub fn to_rule_execution(&self) -> crate::ops::types::RuleExecution {
        crate::ops::types::RuleExecution {
            rule_execution_id: Some(self.rule_execution_id.clone()),
            status: Some(self.status.clone()),
            summary: self
                .error
                .as_ref()
                .map(|(_, message)| message.clone())
                .or_else(|| Some(self.status.clone())),
            last_status_change: Some(to_date_time(self.last_update_time)),
            token: None,
            last_updated_by: None,
            // The three `external*` members report the run the PROVIDER performed, and
            // no provider ran, so none of them is invented.
            external_execution_id: None,
            external_execution_url: None,
            error_details: self.error.as_ref().map(|(code, message)| ErrorDetails {
                code: Some(code.clone()),
                message: Some(message.clone()),
            }),
        }
    }

    /// This evaluation as `ListRuleExecutions` reports it.
    pub fn to_detail(&self) -> RuleExecutionDetail {
        RuleExecutionDetail {
            pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            rule_execution_id: Some(self.rule_execution_id.clone()),
            pipeline_version: Some(self.pipeline_version),
            stage_name: Some(self.stage_name.clone()),
            rule_name: Some(self.rule_name.clone()),
            start_time: Some(to_date_time(self.start_time)),
            last_update_time: Some(to_date_time(self.last_update_time)),
            // `RuleExecutionDetail$updatedBy` is "the ARN of the user who changed the
            // rule execution details": nobody changed this one — the condition plane
            // evaluated it — so the member is absent rather than invented.
            updated_by: None,
            status: Some(self.status.clone()),
            input: Some(RuleExecutionInput {
                rule_type_id: Some(self.rule_type_id.clone()),
                configuration: non_empty(&self.configuration),
                resolved_configuration: non_empty(&self.resolved_configuration),
                role_arn: self.role_arn.clone(),
                region: self.region.clone(),
                // `RuleExecutionInput$inputArtifacts` reports the artifacts the rule
                // consumed; a rule nothing evaluated consumed none.
                input_artifacts: None,
            }),
            output: Some(RuleExecutionOutput {
                execution_result: Some(RuleExecutionResult {
                    // The three `external*` members report the run the PROVIDER
                    // performed, and no provider ran, so none of them is invented.
                    external_execution_id: None,
                    external_execution_summary: None,
                    external_execution_url: None,
                    error_details: self.error.as_ref().map(|(code, message)| ErrorDetails {
                        code: Some(code.clone()),
                        message: Some(message.clone()),
                    }),
                }),
            }),
        }
    }
}

/// The map a configuration is reported as, or `None` when the declaration stated none.
fn non_empty(
    configuration: &BTreeMap<String, String>,
) -> Option<std::collections::HashMap<String, String>> {
    (!configuration.is_empty()).then(|| {
        configuration
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

/// Record `record` as the newest rule execution of `pipeline_name`, dropping the
/// oldest beyond [`MAX_RULE_EXECUTIONS_PER_PIPELINE`].
///
/// The ONE write path into a [`RuleExecutionLog`], so the bound cannot be bypassed —
/// the driver reaches it with only this field of the store borrowed, exactly as it
/// reaches the job queue.
pub fn record_rule_execution_into(
    log: &mut RuleExecutionLog,
    pipeline_name: &str,
    record: RuleExecutionRecord,
) {
    let recorded = log.entry(pipeline_name.to_string()).or_default();
    recorded.push(record);
    let overflow = recorded.len().saturating_sub(MAX_RULE_EXECUTIONS_PER_PIPELINE);
    recorded.drain(..overflow);
}

/// What one rule's evaluation reached.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The rule holds.
    Passed,
    /// The rule does not hold, and why.
    Failed(String),
    /// No verdict was reached, and why.
    Undecided(String),
}

impl Verdict {
    /// The `RuleExecutionStatus` this verdict is recorded under.
    fn status(&self) -> &'static str {
        match self {
            Self::Passed => RULE_EXECUTION_SUCCEEDED,
            Self::Failed(_) => RULE_EXECUTION_FAILED,
            Self::Undecided(_) => RULE_EXECUTION_ABANDONED,
        }
    }

    /// Why, for the verdicts that carry a reason.
    fn reason(&self) -> Option<&str> {
        match self {
            Self::Passed => None,
            Self::Failed(reason) | Self::Undecided(reason) => Some(reason),
        }
    }
}

/// The most characters of a regular expression the `MATCHES` comparison compiles.
///
/// The `Value` a rule states is resolved text, which [`evaluate_conditions`] allows up
/// to [`super::MAX_RESOLVED_CONFIGURATION_VALUE_BYTES`] of, and compiling an expression
/// is work that grows with its length: a longer one is refused BEFORE anything is
/// compiled, so no declaration sizes the work of an evaluation.
const MAX_EXPRESSION_CHARS: usize = 1_000;

/// The most memory one compiled expression may take.
///
/// The crate matches without backtracking — the time of a match is linear in the text
/// it is applied to — so what is left to bound is the compiled program, stated here
/// rather than left at the crate's own default.
const MAX_EXPRESSION_PROGRAM_BYTES: usize = 64 * 1024;

/// The expression `value` states, compiled within the bounds above, or `None` when it
/// is not one this plane can apply.
fn compiled_expression(value: &str) -> Option<::regex::Regex> {
    if value.chars().count() > MAX_EXPRESSION_CHARS {
        return None;
    }
    ::regex::RegexBuilder::new(value)
        .size_limit(MAX_EXPRESSION_PROGRAM_BYTES)
        .dfa_size_limit(MAX_EXPRESSION_PROGRAM_BYTES)
        .build()
        .ok()
}

/// Evaluate one rule against the configuration the execution resolved for it.
///
/// `VariableCheck` compares the variable the configuration names against the value it
/// names, so both sides come from the execution's own state and the request's own
/// declaration: a verdict is computed. Every other provider's verdict is somewhere
/// this emulator cannot reach, and so is one it does not reach either.
fn evaluate_rule(rule: &RuleDeclaration, resolved: &BTreeMap<String, String>) -> Verdict {
    let provider = &rule.rule_type_id.provider;
    if provider != PROVIDER_VARIABLE_CHECK {
        return Verdict::Undecided(format!(
            "the verdict of the {provider} rule is decided outside this service, which \
             this emulator does not reach, so the rule was not evaluated"
        ));
    }
    let (Some(variable), Some(value)) = (resolved.get(CONFIG_VARIABLE), resolved.get(CONFIG_VALUE))
    else {
        return Verdict::Undecided(format!(
            "the rule's configuration states no {CONFIG_VARIABLE} and {CONFIG_VALUE} to \
             compare"
        ));
    };
    // A `#{namespace.key}` reference the execution carries no value for is left as
    // written when the configuration is resolved, so what the variable holds is not
    // known here and the comparison has nothing to decide.
    if variable.contains("#{") {
        return Verdict::Undecided(format!(
            "the rule checks {variable}, which this execution carries no value for, so \
             the comparison was not performed"
        ));
    }
    let Some(operator) = resolved.get(CONFIG_OPERATOR) else {
        return Verdict::Undecided(format!(
            "the rule's configuration states no {CONFIG_OPERATOR} to compare with"
        ));
    };
    // The orderings compare numbers when both sides are numbers, and otherwise the
    // text, so a check on a count behaves as a count and one on a name as a name.
    let ordering = match (variable.parse::<f64>(), value.parse::<f64>()) {
        (Ok(left), Ok(right)) => left.partial_cmp(&right),
        _ => Some(variable.as_str().cmp(value.as_str())),
    };
    let holds = match operator.as_str() {
        OPERATOR_EQ => variable == value,
        OPERATOR_NE => variable != value,
        // The two text predicates read the value as a substring and as a regular
        // expression respectively: reading the expression is the `regex` crate's job,
        // not this emulator's, and one it could not read within the bounds an
        // evaluation is allowed is one the comparison cannot be performed with at all.
        OPERATOR_CONTAINS => variable.contains(value.as_str()),
        OPERATOR_MATCHES => match compiled_expression(value) {
            Some(expression) => expression.is_match(variable),
            None => {
                return Verdict::Undecided(format!(
                    "the rule's {CONFIG_VALUE} is not a regular expression this plane \
                     can apply, so the comparison was not performed"
                ))
            }
        },
        OPERATOR_GT => ordering == Some(::std::cmp::Ordering::Greater),
        OPERATOR_GE => matches!(
            ordering,
            Some(::std::cmp::Ordering::Greater | ::std::cmp::Ordering::Equal)
        ),
        OPERATOR_LT => ordering == Some(::std::cmp::Ordering::Less),
        OPERATOR_LE => matches!(
            ordering,
            Some(::std::cmp::Ordering::Less | ::std::cmp::Ordering::Equal)
        ),
        other => {
            return Verdict::Undecided(format!(
                "the rule compares with the operator {other}, which this emulator does \
                 not apply, so the comparison was not performed"
            ))
        }
    };
    if holds {
        Verdict::Passed
    } else {
        Verdict::Failed(format!(
            "the rule checks that {variable} {operator} {value}, which does not hold"
        ))
    }
}

/// What evaluating one condition set decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionOutcome {
    /// No condition of the set was MET — every rule either passed or reached no
    /// verdict — so no `Result` applies and the execution carries on.
    NotMet,
    /// A condition was met: a rule of it reached a failing verdict, so the `Result` it
    /// declares applies.
    Met {
        /// The `Result` value the condition declared, defaulted to [`RESULT_FAIL`].
        result: String,
        /// What the execution reports as the reason.
        detail: String,
    },
}

/// Evaluate `conditions` for one stage of one execution, recording a rule execution
/// per declared rule, and answer what the execution is to do.
///
/// Every rule of every condition is evaluated, so a caller reading
/// `ListRuleExecutions` sees each of them and not only the first with a verdict; a
/// condition that declares no rule has nothing to evaluate and records nothing. The
/// SET's outcome is the first condition a rule of which reached a FAILING verdict,
/// because the first such `result` is the one the stage acts on.
///
/// The walk is bounded by the MODEL, before any declaration reaches the store: a
/// condition site carries `ConditionList`, `@length(min: 1, max: 1)`, and a condition's
/// `rules` are a `RuleDeclarationList`, `@length(min: 1, max: 5)`, so at most five
/// rules are evaluated per site per stage entry, each with at most the 200 entries
/// `RuleConfigurationMap` allows — and each value's resolution is held to
/// [`super::MAX_RESOLVED_CONFIGURATION_VALUE_BYTES`] by
/// [`super::execution::resolve_placeholders`]. The boundary validator refuses anything
/// past those lengths, so nothing here is sized by the request body.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_conditions(
    log: &mut RuleExecutionLog,
    pipeline_name: &str,
    pipeline_execution_id: &str,
    pipeline_version: i32,
    stage_name: &str,
    condition_type: &str,
    conditions: &[Condition],
    variables: &BTreeMap<String, String>,
    now: SystemTime,
) -> ConditionOutcome {
    let mut outcome = ConditionOutcome::NotMet;
    for condition in conditions {
        for rule in condition.rules.as_deref().unwrap_or_default() {
            let configuration: BTreeMap<String, String> = rule
                .configuration
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            let resolved_configuration: BTreeMap<String, String> = configuration
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        super::execution::resolve_placeholders(value, variables),
                    )
                })
                .collect();
            let verdict = evaluate_rule(rule, &resolved_configuration);
            record_rule_execution_into(
                log,
                pipeline_name,
                RuleExecutionRecord {
                    rule_execution_id: random_uuid_v4(),
                    pipeline_execution_id: pipeline_execution_id.to_string(),
                    pipeline_version,
                    stage_name: stage_name.to_string(),
                    rule_name: rule.name.clone(),
                    condition_type: condition_type.to_string(),
                    rule_type_id: rule.rule_type_id.clone(),
                    configuration,
                    resolved_configuration,
                    role_arn: rule.role_arn.clone(),
                    region: rule.region.clone(),
                    status: verdict.status().to_string(),
                    error: verdict.reason().map(|reason| {
                        (
                            super::execution::FAILURE_CONFIGURATION_ERROR.to_string(),
                            reason.to_string(),
                        )
                    }),
                    start_time: now,
                    last_update_time: now,
                },
            );
            if let (Verdict::Failed(reason), ConditionOutcome::NotMet) = (&verdict, &outcome) {
                outcome = ConditionOutcome::Met {
                    result: condition
                        .result
                        .clone()
                        .unwrap_or_else(|| RESULT_FAIL.to_string()),
                    detail: format!(
                        "rule {} of the stage's {condition_type} condition did not pass: \
                         {reason}",
                        rule.name
                    ),
                };
            }
        }
    }
    outcome
}
