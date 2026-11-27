Issue-Source: trace-sweep-divergence
Issue-Trace: describe-rule-name-length-bound
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: describe-rule-name-length-bound disable-rule-event-bus-name-length-bound events-cancel-replay-name-length events-list-event-sources-name-prefix-length-bounds events-list-targets-by-rule-rule-name-length-bounds-rejected events-list-targets-by-rule-rule-name-outside-pattern-rejected list-endpoints-name-prefix-length-bound events-list-api-destinations-next-token-length
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|DescribeRule|4xx->4xx|$.message
Issue-Key: events|DisableRule|4xx->4xx|$.message
Issue-Key: events|CancelReplay|4xx->4xx|$.message
Issue-Key: events|ListEventSources|4xx->4xx|$.message
Issue-Key: events|ListTargetsByRule|4xx->4xx|$.message
Issue-Key: events|ListEndpoints|4xx->4xx|$.message
Issue-Key: events|ListApiDestinations|4xx->4xx|$.message
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: multi-constraint ValidationException messages order the length error before the pattern error

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRule

## Description
7 replayed divergence clusters, 2 root causes. Both are about the TEXT of a 400 ValidationException whose status and error code already match.

Cause 1 of 2: when one member violates both its @pattern and its @length, the two clauses are emitted in the wrong order.
AWS: refuses with HTTP 400 and __type ValidationException (the exception is not in the model - EventBridge's model declares no ValidationException shape, so this is recorded behavior) and words it '<n> validation errors detected: ' followed by the per-constraint clauses, with the REGULAR EXPRESSION clause first and the LENGTH clause second: "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+; Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1". The fixed text is the '<n> validation errors detected: ', 'Value '...' at '<member>' failed to satisfy constraint: ', 'Member must satisfy regular expression pattern: ' and 'Member must have length greater than or equal to <min>' templates; the variable parts are the offending value, the lowerCamel member path, the pattern and the bound, all taken from the request and the model (Smithy: DescribeRule.Name -> RuleName @length 1..64 @pattern ^[\\.\\-_A-Za-z0-9]+$; DisableRule.EventBusName -> EventBusNameOrArn @length 1..1600 @pattern ^(arn:aws[\\w-]*:events:[a-z]+-[a-z]+-[\\w-]+:[0-9]{12}:event-bus\\/)?[/\\.\\-_A-Za-z0-9]+$; CancelReplay.ReplayName -> ReplayName @length 1..64 @pattern ^[\\.\\-_A-Za-z0-9]+$; ListEventSources.NamePrefix, ListEndpoints.NamePrefix, ListTargetsByRule.Rule likewise). Note AWS also reports the pattern WITHOUT the model's ^...$ anchors here.
Emulator: emits the same two clauses with the LENGTH clause first and the pattern clause second - '2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+' - on all six operations below.
Why: the message is a requirement, not a free-form string: clients, SDK integration tests and log-matching rules that compare, parse or assert on AWS's validation text see a different sentence for the same refusal, and anything that reads only the first clause is told the wrong reason (length rather than grammar).
Fix: the events request-validation layer's message assembly - the order in which the per-constraint clauses for one member are appended (pattern before length), shared by every operation above.
Traces: describe-rule-name-length-bound#task#4 (DescribeRule), disable-rule-event-bus-name-length-bound#task#4 (DisableRule), events-cancel-replay-name-length#task#3 (CancelReplay), events-list-event-sources-name-prefix-length-bounds#task#1 (ListEventSources), events-list-targets-by-rule-rule-name-length-bounds-rejected#task#7 (ListTargetsByRule), list-endpoints-name-prefix-length-bound#task#3 (ListEndpoints).

Cause 2 of 2: the two-error refusal for an out-of-bounds ListApiDestinations NextToken is worded differently from AWS.
AWS: refuses an over-long pagination token with HTTP 400 ValidationException reading '2 validation errors detected: Value '<the token>' at 'nextToken' failed to satisfy constraint: ...' (Smithy: ListApiDestinations.NextToken -> NextToken @length 1..2048; the exception itself is unmodelled, so the wording is recorded behavior).
Emulator: answers 400 with a $.message that differs from that recorded text (the replay report truncates the actual string at the cluster's detail limit, so only the divergence - not the full emulator sentence - is visible; it is almost certainly the same clause-ordering defect as Cause 1, which the fixer should confirm on re-replay).
Why: same as Cause 1 - the refusal text for a paging token is what a client logs and matches on.
Fix: the same validation-message assembly, reached through ListApiDestinations' NextToken validation.
Traces: events-list-api-destinations-next-token-length#task#9 (the cluster also covers #task#10 and #task#11 of that fixture).

## Evidence
Cause 1 (replay detail, DescribeRule, describe-rule-name-length-bound#task#4, HTTP 400 both sides):
expected $.message = "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+; Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1"
got      $.message = "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+"
Identical inversion on DisableRule ('eventBusName'), CancelReplay ('replayName'), ListEventSources ('namePrefix'), ListTargetsByRule ('rule') and ListEndpoints ('namePrefix').

Cause 2 (ListApiDestinations, events-list-api-destinations-next-token-length#task#9, HTTP 400 both sides): '$.message: value_mismatch (expected "2 validation errors detected: Value \'BBBB...\' ...", got <diverging text, truncated in the replay report>)'.

## Steps to Reproduce
1. Call DescribeRule with Name="" (empty string violates both @length min 1 and the @pattern) and compare $.message clause order against AWS's: pattern clause first, length clause second.
2. Repeat with DisableRule EventBusName="", CancelReplay ReplayName="", ListEventSources NamePrefix="", ListTargetsByRule Rule="", ListEndpoints NamePrefix="".
3. Call ListApiDestinations with a NextToken longer than 2048 characters and compare the whole two-error message against the recorded AWS text.
4. Re-replay the fixtures listed under Traces.
