Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-list-and-filter-rules
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-list-and-filter-rules
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/3dcb3abd-bc80-4826-a408-50372844bfad"].Id
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id
Issue-Commit: 792e2d56472eaa6a918bc292baf30b2d329971ab

# codestar-notifications mints notification rule ids with no recorded provenance, breaking ListNotificationRules Id on every page

> _Note: this issue was identified against the code base at commit `792e2d56472eaa6a918bc292baf30b2d329971ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationRules

## Description
Three replay clusters, all service=codestar-notifications / operation=ListNotificationRules / expected_status=200 -> actual_status=200, diverge on the SAME output member: NotificationRuleSummary.Id. Every one of them answers the modelled 200 with the modelled members and selects the correct rules; what diverges is the rule IDENTITY the listing reports, for EVERY entry of EVERY page, filtered or unfiltered. This issue bundles THREE causes and a fixer must close all THREE (task#6, task#7 and task#8) before submitting.

Shared root cause. The id a listing reports is the id state minted for the rule. In this checkout crate::state::new_rule_id draws a random version-4 UUID (services/.../src/state.rs:575-577, `::uuid::Uuid::new_v4().as_hyphenated().to_string()`), crate::state::notification_rule_arn embeds that same value as the ARN's `notificationrule/<id>` segment, and ListNotificationRules echoes the stored id straight into NotificationRuleSummary.Id (list_notification_rules.rs:193-201). One minting decision therefore decides what every listing reports, and nothing in the id is derived from the rule the request describes — the doc comment on new_rule_id states this outright: the id "is not a function of anything this service knows (not the creation order, not the clock, not the request)". Note that the replay detail prints expected and actual with the SAME text, so the divergence is NOT a visible textual difference in one recording's value: the comparator binds a listed entry by its Arn and still rejects the Id the emulator carries for that entry, on every replay, while the printed value changes from run to run. Hypothesis: the identity the recording holds for a rule is not reconstructible from a per-run random draw, so the listing's Id cannot be matched back to it.

Cause 1 of 3: unfiltered ListNotificationRules page (two rules listed) diverges at NotificationRuleSummary.Id for BOTH entries. Fixture codestar-notifications-list-and-filter-rules, step task#6.
  AWS: Smithy: ListNotificationRules — http POST /listNotificationRules code 200, @paginated(inputToken=NextToken, outputToken=NextToken, items=NotificationRules, pageSize=MaxResults), errors InvalidNextTokenException (400) and ValidationException (400), neither involved here. Output ListNotificationRulesResult.NotificationRules targets NotificationRuleBatch of NotificationRuleSummary{Id -> NotificationRuleId @length min 1 max 40, "The unique ID of the notification rule"; Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$}. The model constrains only the id's LENGTH and never its internal form or provenance, so for the form this is recorded behavior only: the recording shows AWS reporting, for each listed rule, the unique rule id it minted for that rule, byte-for-byte identical to the resource segment of that rule's ARN. No status, error code or error message is involved — both sides are 200.
  Emulator: answers 200 with the right page and the right members, verdict `mismatch`: value_mismatch at $.NotificationRules[Arn="...notificationrule/3dcb3abd-bc80-4826-a408-50372844bfad"].Id and $.NotificationRules[Arn="...notificationrule/88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id — two of two listed rules. The id reported is whatever state::new_rule_id drew in this run; those two uuids are per-run fill-ins, not the requirement.
  Why: Id is the member a client uses to correlate a listing entry with a rule it already knows, and together with Arn it is the rule's wire identity that DescribeNotificationRule / Subscribe / UpdateNotificationRule / DeleteNotificationRule and the tag calls read back. A listing whose ids cannot be matched against the ids AWS issues breaks that correlation for every rule in the account, on every page, for any client that persisted, logged or asserted an id recorded from AWS.
  Fix: the id minting in crate::state::new_rule_id (and through it crate::state::notification_rule_arn) — NOT the ListNotificationRules handler, which already returns the stored id unchanged, and not the filter path, which selects the right rules. Whatever the mint becomes, a summary's Id must stay byte-for-byte identical to the resource segment of that summary's Arn.
  Traces: codestar-notifications-list-and-filter-rules#task#6.

Cause 2 of 3: the same Id divergence on a FILTERED page returning a single rule. Fixture codestar-notifications-list-and-filter-rules, step task#7.
  AWS: as cause 1 — Smithy: NotificationRuleSummary.Id -> NotificationRuleId @length 1..40 (no @pattern); HTTP 200. Filtering (ListNotificationRulesRequest.Filters, OR within one filter name and AND across names) narrows WHICH rules are listed and changes nothing about the identity reported for a listed rule; the id's form is recorded behavior only.
  Emulator: 200, the filter selects the correct single rule (the comparator's Arn selector matches it), and that one entry's $.NotificationRules[Arn="...notificationrule/88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id still reports value_mismatch.
  Why: a filtered listing is how a client finds one rule by resource, event type, target address or creator; the one entry it gets back carries an id it cannot match against AWS's.
  Fix: same component as cause 1 (crate::state::new_rule_id). Nothing in rule_satisfies / grouped_filters is implicated.
  Traces: codestar-notifications-list-and-filter-rules#task#7.

Cause 3 of 3: the same Id divergence on the OTHER filtered page. Fixture codestar-notifications-list-and-filter-rules, step task#8.
  AWS: as cause 1/2 — recorded behavior only for the id's form; the model gives NotificationRuleId @length 1..40 and nothing else. HTTP 200, no error.
  Emulator: 200, correct rule selected, $.NotificationRules[Arn="...notificationrule/3dcb3abd-bc80-4826-a408-50372844bfad"].Id diverges (value_mismatch) for the single entry.
  Why: as cause 2 — every listing this service serves, filtered or unfiltered, reports a rule id a client cannot match.
  Fix: as cause 1/2.
  Traces: codestar-notifications-list-and-filter-rules#task#8.

Open issues that do NOT own these three clusters' replay keys (this replay reports owned_by EMPTY for all three, so none of them gates these steps and a fixer closing them would not re-run these keys): 20261118T184204Z-codestar-notifications-random-per-run-new-rule-id.md, 20261118T041710Z-codestar-notifications-rule-id-provenance-makes-every-listnotificationrules.md, 20261110T182312Z-codestar-notifications-listnotificationrules-reports-unmatched-rule-id-on.md and 20261110T210013Z-codestar-notifications-listnotificationrules-reports-unmatchable-notificationrulesummary-id-on.md all describe the same minting root cause, but their Issue-Key lines name per-run ARNs from earlier replays (notificationrule/330e3604-..., /8e481aba-..., /4f2b7532-..., /ad4ff815-..., /dd14b920-..., /fcca133e-..., /2dcfbdba-..., /7b85c7a4-...), and because the key itself embeds the per-run random id, ownership can NEVER match while the mint stays random — which is itself a consequence of this defect. 20261110T174500Z-codestar-notifications-rule-identifiers-diverge-createnotificationrule-arn-shape.md owns only the CreateNotificationRule $.Arn cluster (codestar-notifications-rule-lifecycle#task#4), which this replay reports as owned and which is skipped here. The two 20261110T18/21 issues additionally describe new_rule_id as a sha-256 of "notificationrule/<sequence>/<nanos>"; the code has since drifted to a random uuid::Uuid::new_v4 draw (state.rs:575-577) while the divergence persists unchanged.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: steps_total=95, steps_passed=91, steps_mismatched=4, steps_errored=0, steps_blocked=0, divergence_count=4, score_pct=95.8, emulator_up=true ("/__admin/health answered 200 and the :36109 data listener accepts"), bootstrap_signal=false, not_implemented=0. Three of the four clusters are the ones filed here; the fourth (CreateNotificationRule $.Arn, codestar-notifications-rule-lifecycle#task#4, detail "$.Arn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:codestar-notifications:...')") reports owned_by the open rule-identifiers issue and is skipped.

All three clusters filed here: service=codestar-notifications, operation=ListNotificationRules, verdict=mismatch, expected_status=200, actual_status=200, stub_path empty, owned_by empty, affected_count=1.

Cause 1 — fixture codestar-notifications-list-and-filter-rules, step task#6 (affected: codestar-notifications-list-and-filter-rules#task#6):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/3dcb3abd-bc80-4826-a408-50372844bfad"].Id: value_mismatch (expected '3dcb3abd-bc80-4826-a408-50372844bfad', got '3dcb3abd-bc80-4826-a408-50372844bfad')
           $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id: value_mismatch (expected '88e0d6e4-48fa-40c7-aa5c-a41c2444d18c', got '88e0d6e4-48fa-40c7-aa5c-a41c2444d18c')
  divergence_key: service=codestar-notifications|ListNotificationRules|status=200->200|keys=$.NotificationRules[Arn="...3dcb3abd-bc80-4826-a408-50372844bfad"].Id,$.NotificationRules[Arn="...88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id
  Expected: each summary's Id equal to the recorded rule identity for that rule — the resource segment of that rule's ARN, in the identity AWS minted for it.
  Actual: the per-run id drawn by state::new_rule_id; the comparison fails for EVERY entry of the page. The uuids quoted are per-run fill-ins, not the requirement: earlier replays of the same steps carried 330e3604-.../8e481aba-... and 4f2b7532-.../ad4ff815-... while the verdict was identical.

Cause 2 — same fixture, step task#7 (affected: codestar-notifications-list-and-filter-rules#task#7):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/88e0d6e4-48fa-40c7-aa5c-a41c2444d18c"].Id: value_mismatch (expected '88e0d6e4-48fa-40c7-aa5c-a41c2444d18c', got '88e0d6e4-48fa-40c7-aa5c-a41c2444d18c') — single filtered entry.

Cause 3 — same fixture, step task#8 (affected: codestar-notifications-list-and-filter-rules#task#8):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/3dcb3abd-bc80-4826-a408-50372844bfad"].Id: value_mismatch (expected '3dcb3abd-bc80-4826-a408-50372844bfad', got '3dcb3abd-bc80-4826-a408-50372844bfad') — single filtered entry.

Code under the three causes (same checkout the emulator is built from):
  services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:562-577 — new_rule_id(): `::uuid::Uuid::new_v4().as_hyphenated().to_string()`, documented as "not a function of anything this service knows" and as OPAQUE.
  .../src/state.rs (notification_rule_arn) — emulator_arn("codestar-notifications", region, "notificationrule/<id>"), i.e. the ARN carries the same minted value.
  .../src/ops/list_notification_rules.rs:193-201 — NotificationRuleSummary { id: Some(rule.id.clone()), arn: Some(rule.arn.clone()) }, then page truncation and NextToken; the listing is faithful to the stored id.

Smithy model consulted (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): ListNotificationRules http POST /listNotificationRules code 200, @paginated(inputToken=NextToken, outputToken=NextToken, items=NotificationRules, pageSize=MaxResults), errors InvalidNextTokenException 400 / ValidationException 400; NotificationRuleSummary.Id -> NotificationRuleId @length min 1 max 40, no @pattern (so the id's internal form is recorded behavior only); NotificationRuleSummary.Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus against the booted emulator).
3. Cause 1: replay fixture codestar-notifications-list-and-filter-rules through step task#6 — an unfiltered ListNotificationRules (POST /listNotificationRules) after the fixture's rules have been created. The response is 200 with both rules present and correctly shaped, but $.NotificationRules[Arn="..."].Id reports value_mismatch for BOTH entries.
4. Cause 2: continue to step task#7 — a filtered ListNotificationRules returning one rule. 200, the correct rule is selected, and the single entry's $.NotificationRules[Arn="..."].Id diverges.
5. Cause 3: continue to step task#8 — the other filtered listing. Same 200, same single-entry Id divergence.
6. Confirm the provenance, not one recorded value, is at fault: replay the corpus twice. The rule ids reported change between runs (state::new_rule_id draws a fresh uuid::Uuid::new_v4 per rule) while all three clusters diverge identically every time, and the replay detail prints expected and actual with the same text.
7. After a fix, re-replay codestar-notifications-list-and-filter-rules#task#6, #task#7 and #task#8; all three must pass, and each summary's Id must remain byte-for-byte identical to the resource segment of that summary's Arn.
