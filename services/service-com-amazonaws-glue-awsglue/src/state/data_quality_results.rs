// SPDX-License-Identifier: CC0-1.0

//! What a data quality ruleset evaluation run EVALUATED, and the results a client reads it
//! back through.
//!
//! "The evaluation computes results which you can retrieve with the GetDataQualityResult
//! API" (`StartDataQualityRulesetEvaluationRun`), so a run's work is this: read the rows
//! behind the table its `DataSource` addresses ([`super::table_rows`]), check every rule of
//! every ruleset it was started for against those rows
//! ([`super::data_quality_rules`]), and keep what each rule answered. One RESULT is what one
//! ruleset's evaluation produced, and the run reports one result id per ruleset it
//! evaluated.
//!
//! The evaluation is performed where the run is STARTED, with no store guard held, and what
//! it produced travels with the run record: the clock then owes the run its terminal status
//! one span later, and settling it is what publishes the results — so a caller that reads a
//! run still in flight sees no result id, and every result a finished run names holds the
//! per-rule answers that run's own reading produced.

use super::*;

/// Ceiling on the data quality results this account keeps.
///
/// This emulator's own capacity, NOT an AWS quota, for the reason every other ceiling here
/// is stated: a run is startable by any caller and each one mints a result per ruleset. The
/// OLDEST result is dropped first.
pub const MAX_DATA_QUALITY_RESULTS: usize = 1_000;

/// What one rule of a ruleset answered over the rows the run read.
#[derive(Debug, Clone)]
pub struct EvaluatedDataQualityRule {
    /// `DataQualityRuleResult$Name`: the rule's place in its ruleset's list
    /// ([`data_quality_rule_name`]).
    pub name: String,
    /// `DataQualityRuleResult$Description`: the rule as its author wrote it, which is the
    /// entry of the ruleset's rule list this result is of.
    pub rule: String,
    /// Whether the rows satisfied the rule, which is what the result reports as `PASS` or
    /// `FAIL` ([`DATA_QUALITY_RULE_RESULT_PASS`]).
    pub passed: bool,
}

/// What ONE ruleset's evaluation produced: the answer of each of its rules, in the order the
/// ruleset declares them.
#[derive(Debug, Clone)]
pub struct EvaluatedDataQualityRuleset {
    /// The ruleset's name, as the start listed it.
    pub ruleset_name: String,
    /// One entry per rule the ruleset declares.
    pub rules: Vec<EvaluatedDataQualityRule>,
}

impl EvaluatedDataQualityRuleset {
    /// `Score`: "an aggregate data quality score. Represents the ratio of rules that passed
    /// to the total number of rules" (`GetDataQualityResultResponse$Score`) — so it is the
    /// rules that passed over the rules evaluated, which is inside the member's modelled
    /// `range 0..1`. A ruleset declaring no rule has no ratio to report, so it scores 0.
    pub fn score(&self) -> f64 {
        if self.rules.is_empty() {
            return 0.0;
        }
        let passed = self.rules.iter().filter(|rule| rule.passed).count();
        passed as f64 / self.rules.len() as f64
    }
}

/// What a run's evaluation of its rulesets over its data source produced.
#[derive(Debug, Clone)]
pub enum DataQualityEvaluationOutcome {
    /// The rules were checked against the data that was read: one entry per ruleset the run
    /// evaluated, in the order the start listed them.
    Evaluated(Vec<EvaluatedDataQualityRuleset>),
    /// Nothing was evaluated, for the reason the run reports as its `ErrorString`.
    Failed(String),
}

/// `DataQualityRuleResult$Result` of a rule the rows satisfied: of the modelled
/// `DataQualityRuleResultStatus` (`PASS | FAIL | ERROR`), the passing one.
pub const DATA_QUALITY_RULE_RESULT_PASS: &str = "PASS";

/// See [`DATA_QUALITY_RULE_RESULT_PASS`]: the value of a rule the rows did not satisfy.
pub const DATA_QUALITY_RULE_RESULT_FAIL: &str = "FAIL";

/// `DataQualityRuleResult$Name` of the rule at `position` in its ruleset's rule list,
/// counted from one: a rule of a ruleset is named by where it stands in that list, since
/// DQDL gives a rule no name of its own.
pub fn data_quality_rule_name(position: usize) -> String {
    format!("Rule_{}", position + 1)
}

/// What a run has to evaluate: the rulesets it was started for, each with the DQDL text
/// stored under its name, and where the data they are evaluated against lives.
///
/// Built from the store under its guard and then carried out with no guard held, so a run
/// evaluates the rulesets and the table as they stood when it started.
#[derive(Debug, Clone)]
pub struct DataQualityEvaluationPlan {
    /// The rulesets the run evaluates: the name the start listed and the DQDL text the
    /// stored ruleset holds, in the order the start listed them.
    pub rulesets: Vec<(String, String)>,
    /// Where the rows the rules are checked against live.
    pub table: TableDataPlan,
}

/// How many rule CHECKS one run carries out: the product of the rules it evaluates and the
/// rows each of them is checked against, which is the work the evaluation really does —
/// neither factor bounds it alone, because a caller writes both the rules of a ruleset and
/// the objects under a table's location.
///
/// CHOSEN, not an AWS quota, for the reason the bounds on reading a table's data are chosen:
/// a run past it is a run that evaluated nothing rather than one reporting part of its rules.
pub const MAX_DATA_QUALITY_RULE_CHECKS: usize = 2_000_000;

/// Evaluate the rulesets `plan` names against the rows behind the table it names, reading
/// that table's data out of the emulator's own Amazon S3.
///
/// Awaited with NO store guard held. Every answer is grounded in what was read: a location
/// whose data could not be read evaluates nothing and reports why, and so does a ruleset
/// holding a rule whose truth cannot be decided from the rows — nothing is reported as
/// passed or failed unless this evaluation actually decided it.
///
/// BOUNDED in both directions a caller can grow: how much data is read
/// ([`read_table_rows`]) and how many rule checks are carried out over it
/// ([`MAX_DATA_QUALITY_RULE_CHECKS`]). The walk hands the runtime back once per rule, so the
/// request's own cancellation and every other request can land while it goes on.
pub async fn evaluate_data_quality_rulesets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &DataQualityEvaluationPlan,
) -> DataQualityEvaluationOutcome {
    let rows = match read_table_rows(ctx, region, &plan.table).await {
        Ok(rows) => rows,
        Err(detail) => {
            // The table's own values are rendered in their escaped `Debug` form, so a
            // location carrying a newline cannot forge a log line.
            ::tracing::debug!(
                location = ?plan.table.location,
                detail = %detail,
                "a Glue data quality ruleset evaluation run could not read the data source"
            );
            return DataQualityEvaluationOutcome::Failed(
                messages::data_quality_data_source_unreadable(),
            );
        }
    };

    let mut evaluated = Vec::with_capacity(plan.rulesets.len());
    // How many rule checks this run has carried out so far, against the bound above.
    let mut checks = 0usize;
    for (ruleset_name, ruleset) in &plan.rulesets {
        let mut rules = Vec::new();
        for (position, rule) in dqdl_rules(ruleset).into_iter().enumerate() {
            checks = checks.saturating_add(rows.rows.len().max(1));
            if checks > MAX_DATA_QUALITY_RULE_CHECKS {
                ::tracing::debug!(
                    ruleset = %ruleset_name,
                    "a Glue data quality ruleset evaluation run had more to evaluate than one \
                     run evaluates"
                );
                return DataQualityEvaluationOutcome::Failed(
                    messages::data_quality_too_much_to_evaluate(),
                );
            }
            let Some(outcome) = evaluate_dqdl_rule(&rule, &rows) else {
                ::tracing::debug!(
                    ruleset = %ruleset_name,
                    rule = ?rule,
                    "a Glue data quality rule could not be evaluated against the data source"
                );
                return DataQualityEvaluationOutcome::Failed(
                    messages::data_quality_rule_not_evaluated(&rule),
                );
            };
            rules.push(EvaluatedDataQualityRule {
                name: data_quality_rule_name(position),
                rule,
                passed: outcome.passed,
            });
            // Checking one rule against every row is this emulator's own work on the
            // request's task, so it hands the runtime back between rules.
            ::tokio::task::yield_now().await;
        }
        evaluated.push(EvaluatedDataQualityRuleset {
            ruleset_name: ruleset_name.clone(),
            rules,
        });
    }
    DataQualityEvaluationOutcome::Evaluated(evaluated)
}

/// One stored data quality result: what ONE ruleset's evaluation produced inside one run,
/// which is what `GetDataQualityResult` reports.
///
/// Every member a read of the result serves is held here, so the read serves what the run
/// evaluated and computes nothing afresh — the `Score` excepted, which is derived from the
/// per-rule answers ([`EvaluatedDataQualityRuleset::score`]) so the two can never disagree.
#[derive(Debug, Clone)]
pub struct DataQualityResultEntry {
    /// `ResultId`, minted when the run that produced it finished evaluating
    /// ([`new_data_quality_result_id`]), and the id this result is addressed by.
    pub id: String,
    /// `RulesetEvaluationRunId`: the run this result came out of.
    pub ruleset_evaluation_run_id: String,
    /// `RulesetName`: the ruleset whose rules this result reports.
    pub ruleset_name: String,
    /// `DataSource`: "the table associated with the data quality result", which is the data
    /// source the run evaluated, reported as the start named it.
    pub data_source: DataSource,
    /// `StartedOn`: when the run this result came out of started.
    pub started_on: SystemTime,
    /// `CompletedOn`: when that run completed, which is when this result was produced.
    pub completed_on: SystemTime,
    /// What each rule of the ruleset answered.
    pub rules: Vec<EvaluatedDataQualityRule>,
}

impl DataQualityResultEntry {
    /// The result one ruleset's evaluation produced inside the run `run_id`, under a freshly
    /// minted result id.
    pub fn of(
        evaluated: &EvaluatedDataQualityRuleset,
        run_id: &str,
        data_source: DataSource,
        started_on: SystemTime,
        completed_on: SystemTime,
    ) -> Self {
        Self {
            id: new_data_quality_result_id(),
            ruleset_evaluation_run_id: run_id.to_string(),
            ruleset_name: evaluated.ruleset_name.clone(),
            data_source,
            started_on,
            completed_on,
            rules: evaluated.rules.clone(),
        }
    }

    /// `RuleResults`: "a list of DataQualityRuleResult objects representing the results for
    /// each rule" — one per rule of the ruleset, in the order the ruleset declares them,
    /// each naming the rule it is of and whether the data satisfied it.
    pub fn to_rule_results(&self) -> Vec<DataQualityRuleResult> {
        self.rules
            .iter()
            .map(|rule| DataQualityRuleResult {
                name: Some(rule.name.clone()),
                // "The description of the data quality rule": the rule itself, as its
                // author wrote it.
                description: Some(rule.rule.clone()),
                result: Some(
                    if rule.passed {
                        DATA_QUALITY_RULE_RESULT_PASS.to_string()
                    } else {
                        DATA_QUALITY_RULE_RESULT_FAIL.to_string()
                    }
                    ),
                // The members an evaluation of the rule's own metrics would carry are left
                // unset: this evaluation decides whether the rows satisfy the rule, and
                // reports no metric, distribution, label or observation it did not compute.
                evaluation_message: None,
                evaluated_metrics: None,
                evaluated_rule: None,
                rule_metrics: None,
                labels: None,
            })
            .collect()
    }

    /// `Score`: the ratio of this result's rules that passed
    /// ([`EvaluatedDataQualityRuleset::score`] is where that ratio is defined).
    pub fn score(&self) -> f64 {
        EvaluatedDataQualityRuleset {
            ruleset_name: self.ruleset_name.clone(),
            rules: self.rules.clone(),
        }
        .score()
    }
}

/// The results a finished evaluation run produced, and how one is addressed.
impl ComAmazonawsGlueAwsglueState {
    /// The stored data quality result with this id, or `None` when this account holds no
    /// such result.
    ///
    /// A result is ADDRESSED by the id the run that produced it minted
    /// (`GetDataQualityResult` takes that id and nothing else), so the lookup is a walk over
    /// the results this account holds, bounded by [`MAX_DATA_QUALITY_RESULTS`].
    pub fn data_quality_result(&self, id: &str) -> Option<&DataQualityResultEntry> {
        self.data_quality_results
            .iter()
            .find(|result| result.id == id)
    }

    /// Keep the results a settled run produced, in the order they were produced, dropping
    /// the OLDEST once this account holds more than [`MAX_DATA_QUALITY_RESULTS`].
    pub fn record_data_quality_results(
        &mut self,
        results: impl IntoIterator<Item = DataQualityResultEntry>,
    ) {
        self.data_quality_results.extend(results);
        if self.data_quality_results.len() > MAX_DATA_QUALITY_RESULTS {
            let overflow = self.data_quality_results.len() - MAX_DATA_QUALITY_RESULTS;
            self.data_quality_results.drain(..overflow);
        }
    }
}
