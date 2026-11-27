Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-case-rule-and-layout-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-case-rule-and-layout-lifecycle connectcases-case-rule-lifecycle-and-batch-get
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|BatchGetCaseRule|2xx->2xx|$.unprocessedCaseRules
Issue-Key: connectcases|BatchGetCaseRule|4xx->2xx|$.message
Issue-Key: connectcases|BatchGetCaseRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: connectcases|BatchGetCaseRule|4xx->2xx|status
Issue-Key: connectcases|BatchGetCaseRule|2xx->2xx|$.caseRules[*].description
Issue-Key: connectcases|ListCaseRules|2xx->2xx|$.caseRules[*]
Issue-Commit: bca2a3c4747c231834aeb06478a800a15cab68f6

# connectcases case-rule read path diverges: BatchGetCaseRule omits unprocessedCaseRules and never validates ids, updates are not reflected, ListCaseRules still returns deleted rules

> _Note: this issue was identified against the code base at commit `bca2a3c4747c231834aeb06478a800a15cab68f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCaseRule

## Description
The emulator's Amazon Connect Cases case-rule read path (BatchGetCaseRule and ListCaseRules) diverges from AWS in four distinct ways. All four are independent root causes and ALL FOUR must be fixed and re-replayed before this issue is closed.

Cause 1 of 4: BatchGetCaseRule response omits the unprocessedCaseRules member entirely.
AWS: Smithy: BatchGetCaseRule -> output shape BatchGetCaseRuleResponse has members caseRules (BatchGetCaseRuleList, @required, @length max 50), errors (BatchGetCaseRuleErrorList, @required, @length max 50) and unprocessedCaseRules (BatchGetCaseRuleUnprocessedList of CaseRuleId, @length max 50, doc "A list of unprocessed case rule identifiers"). The member is not @required in the model, so: recorded behavior only — the recording shows AWS always serializes unprocessedCaseRules, emitting an empty JSON array [] on a fully successful batch rather than leaving the key out.
Emulator: the 200 body carries no unprocessedCaseRules key at all; replay reports "$.unprocessedCaseRules: missing (expected [], got None)".
Why: a client that reads response.unprocessedCaseRules to find out which requested ids the service could not process gets a null/absent collection instead of an empty one. Strongly typed SDK clients and any code that iterates that list without a null guard either throw or silently treat "nothing unprocessed" and "field absent" as the same thing, so partial-batch handling can never be exercised against the emulator.
Fix: the BatchGetCaseRule output serializer (and/or its handler's response construction) must always populate unprocessedCaseRules, defaulting to an empty list, the same way caseRules and errors are always emitted.
Traces: connectcases-case-rule-and-layout-lifecycle#task#5 (representative), and the same omission recurs in connectcases-case-rule-lifecycle-and-batch-get#task#8.

Cause 2 of 4: BatchGetCaseRule accepts syntactically invalid caseRuleId values and answers 200 instead of 400 ValidationException.
AWS: Smithy: BatchGetCaseRule models ValidationException (errorKind client, httpError 400, member message @required) among its errors; input member caseRules is CaseRuleIdentifierList, @required, @length min 1 max 50, whose ids are CaseRuleId (@length min 1 max 500). When the identifiers in the batch are not well-formed case-rule identifiers, AWS rejects the whole call rather than reporting them per-item: the recorded response is HTTP 400 with x-amzn-errortype naming the modeled error shape and body {"message": "The CaseRuleId(s) provided are not valid"}. That message is entirely fixed text for this error class — it has no variable part interpolated from the request (it deliberately does not echo the offending ids).
Emulator: answers HTTP 200 with a normal batch body. Replay reports "status: expected 400, got 200", "$.message: missing (expected 'The CaseRuleId(s) provided are not valid', got None)" and "$headers.x-amzn-errortype: missing".
Why: a client sending malformed identifiers is told the request succeeded. Error handling for bad input is unreachable against the emulator, and callers conclude the rules simply do not exist (empty caseRules) rather than that their request was rejected, so retry/repair logic keyed on ValidationException never fires.
Fix: BatchGetCaseRule's input validation — validate the shape of every caseRules identifier before the state lookup and raise the modeled ValidationException (400, x-amzn-errortype: ValidationException) with exactly the fixed message text above.
Traces: connectcases-case-rule-lifecycle-and-batch-get#task#5.

Cause 3 of 4: an UpdateCaseRule-supplied description is not reflected by a subsequent BatchGetCaseRule.
AWS: Smithy: UpdateCaseRule input has description (CaseRuleDescription, @length max 255); BatchGetCaseRule returns GetCaseRuleResponse items whose description member (CaseRuleDescription, @length max 255, doc "Description of a case rule") carries the rule's current description. After a successful UpdateCaseRule that sets a description, the recorded read-back returns that description on the matching caseRules item.
Emulator: the BatchGetCaseRule item has no description member — replay reports "$.caseRules[0].description: missing (expected 'sim2real updated description', got None)". (The quoted string is the value the fixture happened to send on UpdateCaseRule; the requirement is that whatever description the update supplied is read back, not that literal text.)
Why: UpdateCaseRule reports success but the mutation is lost on the read path, so the emulator is not read-your-writes for case rules. A client that updates a rule's description and then re-reads it sees the old (absent) value and will retry the update forever or report drift.
Fix: UpdateCaseRule's state mutation must persist description onto the stored case rule, and the BatchGetCaseRule handler/serializer must project it onto each returned GetCaseRuleResponse item when set.
Traces: connectcases-case-rule-lifecycle-and-batch-get#task#8.

Cause 4 of 4: ListCaseRules keeps returning a case rule after it has been deleted.
AWS: Smithy: DeleteCaseRule (DELETE /domains/{domainId}/case-rules/{caseRuleId}) removes the rule; ListCaseRules output caseRules is a CaseRuleSummaryList (@required, @length max 100) of CaseRuleSummary. The recording shows that once DeleteCaseRule has succeeded the rule is no longer enumerated by ListCaseRules — the summary list for that domain comes back without it (empty, in this fixture).
Emulator: the deleted rule is still enumerated. Replay reports "$.caseRules[0]: extra (expected None, got {caseRuleId, name, caseRuleArn, ruleType})" — i.e. the emulator lists a case-rule summary where AWS lists none. The caseRuleId/name/ARN in that record are per-run generated values and are not themselves part of the requirement; the defect is that a deleted rule remains visible.
Why: delete is not durable from the client's point of view. Any workflow that deletes a rule and then enumerates to confirm, or that rebuilds configuration from ListCaseRules, re-discovers phantom rules; cleanup loops never terminate and name-uniqueness checks fail against rules that no longer exist.
Fix: ListCaseRules' state query must exclude case rules removed (or tombstoned) by DeleteCaseRule — DeleteCaseRule must mark/remove the rule in the domain's case-rule state and the list handler must honour that.
Traces: connectcases-case-rule-lifecycle-and-batch-get#task#10.

Related but NOT covering this issue: the open issue 20261109T181134Z-connectcases-validationexception-bodies-use-an-enumerated-constraint-template.md owns the generic ValidationException body-wording divergences (CreateDomain/CreateField/BatchGetField), and it is the reason those three replay clusters are skipped here. It is about the template used to render constraint violations; Cause 2 above is a missing validation entirely (HTTP 200 instead of any error) on a different operation, so it is not covered by that issue. The other open connectcases issues (UpdateField stub, UpdateField 404, UntagResource 204, ListTemplates summaries) concern different operations.

## Evidence
Replay of trace set simparity-wire-aws-v1 (13 fixtures, 182 steps, 157 passed, 25 mismatched, score 86.3%). Four net-new connectcases clusters, none carrying stub_path or owned_by:

1) fixture connectcases-case-rule-and-layout-lifecycle step task#5, BatchGetCaseRule, verdict mismatch, expected_status 200 / actual_status 200:
   detail: "$.unprocessedCaseRules: missing (expected [], got None)"
   Expected: 200 body includes "unprocessedCaseRules": []. Actual: key absent.

2) fixture connectcases-case-rule-lifecycle-and-batch-get step task#5, BatchGetCaseRule, verdict mismatch, expected_status 400 / actual_status 200:
   detail: "status: expected 400, got 200\n$.message: missing (expected 'The CaseRuleId(s) provided are not valid', got None)\n$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
   Expected: HTTP 400, x-amzn-errortype: ValidationException, body {"message": "The CaseRuleId(s) provided are not valid"}. Actual: HTTP 200, no message, no errortype header.

3) fixture connectcases-case-rule-lifecycle-and-batch-get step task#8, BatchGetCaseRule, verdict mismatch, expected_status 200 / actual_status 200:
   detail: "$.caseRules[0].description: missing (expected 'sim2real updated description', got None)\n$.unprocessedCaseRules: missing (expected [], got None)"
   Expected: the item for the just-updated rule carries the description set by UpdateCaseRule, and unprocessedCaseRules is []. Actual: both members absent.

4) fixture connectcases-case-rule-lifecycle-and-batch-get step task#10, ListCaseRules, verdict mismatch, expected_status 200 / actual_status 200:
   detail: "$.caseRules[0]: extra (expected None, got {'caseRuleId': '50bb68c6-...', 'name': 'sim2real-probe-cc-rule-4e3e4c53', 'caseRuleArn': 'arn:aws:cases:us-west-2:000000000000:domain/ff2ad871-.../case-rule/50bb68c6-...', 'ruleType': 'Required'})"
   Expected: caseRules does not contain the deleted rule. Actual: the deleted rule is still enumerated. (ids/names/ARNs shown are per-run values, not requirements.)

Model evidence used (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): BatchGetCaseRule POST /domains/{domainId}/rules-batch, output {caseRules @required, errors @required, unprocessedCaseRules @length max 50}, errors include ValidationException(400); GetCaseRuleResponse.description = CaseRuleDescription @length max 255; UpdateCaseRule PUT /domains/{domainId}/case-rules/{caseRuleId} accepts description; DeleteCaseRule DELETE /domains/{domainId}/case-rules/{caseRuleId}; ListCaseRules POST /domains/{domainId}/rules-list/ output caseRules = CaseRuleSummaryList @required.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or drive the four causes by hand against the connectcases endpoint:

Cause 1 — CreateDomain, CreateCaseRule, then POST /domains/{domainId}/rules-batch with {"caseRules":[{"id":"<created caseRuleId>"}]}. Observe the 200 body has no "unprocessedCaseRules" key; AWS returns "unprocessedCaseRules": []. Fixture connectcases-case-rule-and-layout-lifecycle step task#5.

Cause 2 — POST /domains/{domainId}/rules-batch with a caseRules entry whose identifier is not a well-formed case-rule id. Observe HTTP 200; AWS returns HTTP 400, x-amzn-errortype: ValidationException, {"message":"The CaseRuleId(s) provided are not valid"}. Fixture connectcases-case-rule-lifecycle-and-batch-get step task#5.

Cause 3 — CreateCaseRule, then PUT /domains/{domainId}/case-rules/{caseRuleId} with a "description", then POST /domains/{domainId}/rules-batch for that id. Observe the returned item has no "description"; AWS echoes the description just set. Fixture connectcases-case-rule-lifecycle-and-batch-get step task#8.

Cause 4 — CreateCaseRule, DELETE /domains/{domainId}/case-rules/{caseRuleId} (succeeds), then POST /domains/{domainId}/rules-list/. Observe the deleted rule is still present in caseRules; AWS omits it. Fixture connectcases-case-rule-lifecycle-and-batch-get step task#10.

Re-replay all four fixture#step bindings (the representative plus the three in trace_ids) to confirm a fix.
