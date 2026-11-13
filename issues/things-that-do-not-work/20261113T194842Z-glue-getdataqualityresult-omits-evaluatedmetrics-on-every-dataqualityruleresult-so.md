Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.e2e-data-quality-evaluation-run-produces-result-flow
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.e2e-data-quality-evaluation-run-produces-result-flow|run#29|KeyError|assert passing[0]["EvaluatedMetrics"]["Dataset.*.RowCount"] == float(row_count), passing[0]
Issue-Commit: a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c

# glue: GetDataQualityResult omits EvaluatedMetrics on every DataQualityRuleResult, so the metric a rule was evaluated against is never reported

> _Note: this issue was identified against the code base at commit `a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataQualityResult

## Description
One net-new root cause. The data quality path is NOT a stub here: CreateDataQualityRuleset, StartDataQualityRulesetEvaluationRun and GetDataQualityRulesetEvaluationRun all succeed, the run settles SUCCEEDED with exactly one linked ResultId, and GetDataQualityResult answers 200 with the right RulesetName and two RuleResults that genuinely split PASS/FAIL against the real row count (their Descriptions carry the two different RowCount literals). Only one member of the projected rule result is missing.

Cause 1 of 1: DataQualityRuleResult is serialized without its EvaluatedMetrics map.
AWS: Smithy models GetDataQualityResult's DataQualityResult.RuleResults as a list of DataQualityRuleResult, whose members are Name, Description, EvaluationMessage, Result (enum PASS|FAIL|ERROR), EvaluatedMetrics (target EvaluatedMetricsMap, @sensitive, documented "A map of metrics associated with the evaluation of the rule"), EvaluatedRule, RuleMetrics and Labels. For a ruleset whose rules are row-count rules over a Glue catalog table, AWS populates EvaluatedMetrics with the metric the rule was actually evaluated against, keyed by the metric's fully qualified name — for a dataset-level row count that key is "Dataset.*.RowCount" and its value is the real number of rows in the evaluated data source, as a double. The map is keyed and valued from the evaluated ruleset and the data source, never from a fixed literal: the key is derived from the rule's analyzer and column scope ("*" for a dataset-wide rule) and the value is the measurement taken from the table being evaluated, so a table with N rows reports N.0.
Emulator: GetDataQualityResult returns 200 and each DataQualityRuleResult carries Name/Description/Result (the PASS/FAIL verdicts are correct), but the EvaluatedMetrics member is absent from the response entirely — a client indexing it raises KeyError: 'EvaluatedMetrics'. The emulator therefore decides PASS/FAIL without ever publishing the metric that decision was made from.
Why: EvaluatedMetrics is how a data-quality consumer learns WHAT was measured, not merely whether a rule passed. Dashboards, alarms and remediation jobs read the metric value (row counts, completeness ratios, distinct counts) to trend quality over time and to decide how far off a failing rule is; with the member missing, every such client fails on a missing key rather than degrading, and no caller can distinguish "metric not applicable" from "metric not reported". It also makes the rule-level result unverifiable: a consumer cannot cross-check a PASS against the underlying measurement.
Fix: the Glue data-quality evaluation/result projection in services/service-com-amazonaws-glue-awsglue — the component that evaluates a ruleset for StartDataQualityRulesetEvaluationRun and persists the per-rule outcome, plus the GetDataQualityResult (and GetDataQualityResult/BatchGetDataQualityResult) response serializer — must carry the metrics it computed for each rule into DataQualityRuleResult.EvaluatedMetrics, keyed by the rule's metric name and valued with the measurement taken from the evaluated data source. Note the open issue 20261101T211008Z-glue-getdataqualityresult-stub.md ("Implement glue GetDataQualityResult: unimplemented scaffold stub") does NOT cover this: the operation now answers 200 with a shaped result and the replay reports no owner for this check; what is missing is one member of the rule result, not the operation.
Traces: glue.e2e-data-quality-evaluation-run-produces-result-flow.

## Evidence
Check glue.e2e-data-quality-evaluation-run-produces-result-flow, verdict error.
Setup: a CSV-backed Glue catalog table with 20 rows; ruleset body "Rules = [ RowCount = 20, RowCount = 1019 ]"; StartDataQualityRulesetEvaluationRun over DataSource.GlueTable.
Reached without divergence: evaluation run Status == SUCCEEDED, exactly 1 ResultId, GetDataQualityResult RulesetName matches, 2 RuleResults, exactly 1 PASS and 1 FAIL, each Description carrying its own row-count literal.
Expected (AWS): the PASSing rule result contains EvaluatedMetrics with the dataset row-count metric key "Dataset.*.RowCount" whose value equals the actual row count of the evaluated table as a double (here 20.0 — the 20 is a value from the fixture's data, the requirement is that the value equals the table's real row count).
Actual (emulator): the rule result has no EvaluatedMetrics member at all. Failure detail:
  File "check_glue_e2e_data_quality_evaluation_run_produces_result_flow.py", line 70, in run
    assert passing[0]["EvaluatedMetrics"]["Dataset.*.RowCount"] == float(row_count), passing[0]
  KeyError: 'EvaluatedMetrics'
HTTP status (200) and every other projected member are correct; the single divergence is the omitted EvaluatedMetrics map. Generated ids, ruleset/table/database names and timestamps in this run are per-run values and are not part of the requirement.

## Steps to Reproduce
1. glue CreateDatabase + CreateTable for a CSV table in S3 whose data has a known row count N (e.g. N=20).
2. glue CreateDataQualityRuleset(Name=<r>, Ruleset="Rules = [ RowCount = N, RowCount = N+999 ]", TargetTable={DatabaseName,TableName}).
3. glue StartDataQualityRulesetEvaluationRun(DataSource={GlueTable:{DatabaseName,TableName}}, Role=<glue role>, RulesetNames=[<r>]).
4. Poll glue GetDataQualityRulesetEvaluationRun(RunId=...) until Status is terminal; it reports SUCCEEDED with one ResultIds entry.
5. glue GetDataQualityResult(ResultId=<that id>) and read RuleResults[i]["EvaluatedMetrics"].
6. Observe the member is absent (KeyError) where AWS returns {"Dataset.*.RowCount": N.0}.
Replay check glue.e2e-data-quality-evaluation-run-produces-result-flow.
