Issue-Source: trace-sweep-divergence
Issue-Trace: events-put-targets-capacity-provider-base-range-bound
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-put-targets-capacity-provider-base-range-bound events-put-targets-capacity-provider-strategy-size-bounds events-put-targets-ecs-task-definition-arn-length events-put-targets-header-parameter-key-charset events-put-targets-http-parameters-key-shape events-put-targets-http-path-parameter-blank-and-empty-values-rejected events-put-targets-http-query-string-value-length-bound events-put-targets-input-paths-map-key-shape events-put-targets-kinesis-partition-key-path-length-bound events-put-targets-placement-constraint-type-enum events-put-targets-placement-strategy-type-enum put-targets-ecs-capacity-provider-name-length-bound put-targets-ecs-placement-constraints-count-bounds put-targets-ecs-placement-strategy-count-bound put-targets-ecs-reference-id-length-boundary put-targets-header-parameter-key-length-bound put-targets-http-query-string-key-length-bounds put-targets-query-string-key-control-character-rejected put-targets-run-command-target-key-length-bounds put-targets-run-command-target-key-pattern put-targets-run-command-target-values-count-bounds events-remove-targets-id-list-size-bounds put-targets-api-gateway-query-string-value-carriage-return-kept-bell-refused events-put-targets-header-parameter-value-length events-put-targets-id-length-bounds
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutTargets|4xx->2xx|$.__type
Issue-Key: events|PutTargets|4xx->2xx|$.message
Issue-Key: events|PutTargets|4xx->2xx|status
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[*]
Issue-Key: events|PutTargets|4xx->4xx|$.message
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[Id="blank-path-param"]
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[Id="empty-path-param"]
Issue-Key: events|DeleteRule|2xx->4xx|status
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[Id="t1"]
Issue-Key: events|ListTargetsByRule|2xx->2xx|$.Targets[Id="t2"]
Issue-Key: events|PutTargets|2xx->4xx|$.FailedEntries
Issue-Key: events|PutTargets|2xx->4xx|$.FailedEntryCount
Issue-Key: events|PutTargets|2xx->4xx|status
Issue-Key: events|RemoveTargets|4xx->2xx|$.__type
Issue-Key: events|RemoveTargets|4xx->2xx|$.message
Issue-Key: events|RemoveTargets|4xx->2xx|status
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: PutTargets stores targets AWS refuses - no target-type parameter check, no key grammar checks, and one valid value wrongly rejected

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
12 replayed divergence clusters, 6 root causes, all in the PutTargets/RemoveTargets validation path. EventBridge models no ValidationException, so the refusal bodies below are recorded behavior: HTTP 400 with __type 'ValidationException'.

Cause 1 of 6: PutTargets does not check a parameter block against the TARGET TYPE, so parameter blocks that cannot apply to the target's ARN are accepted and stored.
AWS: refuses the whole request with 'Parameter(s) <BlockName> not supported for target: <target id>.' - the fixed text is 'Parameter(s) ' / ' not supported for target: ' / '.', the variable parts are the block name (EcsParameters, HttpParameters, RunCommandParameters, ...) and the Target.Id from the request. This check runs BEFORE the member constraints of that block: the recorded refusal for an SQS target carrying an over-long header value is the 'not supported' message, not a length message (Smithy: Target.Arn -> TargetArn is what selects the applicable block; EcsParameters/HttpParameters/RunCommandParameters are separate optional members of Target).
Emulator: answers HTTP 200 with FailedEntryCount 0 and stores the target, so ListTargetsByRule then returns a target AWS would never hold (an SQS-ARN target with EcsParameters.CapacityProviderStrategy, with 10 PlacementConstraints or an empty PlacementConstraints list, with HttpParameters.QueryStringParameters, or with RunCommandParameters) - and where it does refuse, it refuses with the wrong message (a HeaderValue length clause instead of 'Parameter(s) HttpParameters not supported for target: q1.').
Traces: events-put-targets-capacity-provider-base-range-bound#task#8 (PutTargets, 400 expected -> 200; cluster covers 33 steps), #task#9 (ListTargetsByRule shows the stored target), events-put-targets-header-parameter-value-length#task#4 (wrong refusal message), put-targets-ecs-placement-constraints-count-bounds#task#9, put-targets-http-query-string-key-length-bounds#task#9, put-targets-run-command-target-key-length-bounds#task#9.

Cause 2 of 6: HttpParameters header and query-string KEYS are not validated against their grammar.
AWS: refuses a header key containing a space and a query-string key containing a control character (Smithy: HeaderParametersMap key -> HeaderKey @length 0..512 @pattern ^[!#$%&'*+-.^_`|~0-9a-zA-Z]+$; QueryStringParametersMap key -> QueryStringKey @length 0..512 @pattern ^[^\\x00-\\x1F\\x7F]+$ - which also excludes an empty key and anything over 512 characters).
Emulator: stores them; ListTargetsByRule returns targets whose HeaderParameters key is 'bad header' and whose QueryStringParameters key contains \\x01, plus a 513-character key and an empty key.
Traces: events-put-targets-http-parameters-key-shape#task#6, put-targets-http-query-string-key-length-bounds#task#9 (also under Cause 1 for its SQS-target aspect).

Cause 3 of 6: blank and empty HttpParameters.PathParameterValues entries are accepted.
AWS: refuses them (Smithy: PathParameterList member -> PathParameter @pattern ^(?!\\s*$).+$ - a value that is empty or all whitespace cannot match).
Emulator: stores both; ListTargetsByRule returns targets with PathParameterValues [''] and ['   ']. A knock-on: the fixture's teardown DeleteRule then fails with HTTP 400 where AWS answers 200, because the emulator is holding targets that should never have been created.
Traces: events-put-targets-http-path-parameter-blank-and-empty-values-rejected#task#10, #teardown#12 (the DeleteRule knock-on).

Cause 4 of 6: InputTransformer.InputPathsMap keys are not validated.
AWS: refuses a key containing a space and a key using the reserved dotted form (Smithy: TransformerPaths key -> InputTransformerPathKey @length 1..256 @pattern ^[A-Za-z0-9\\_\\-]+$; the InputPathsMap docs add that keys cannot start with the reserved 'Amazon Web Services.' prefix).
Emulator: stores them; ListTargetsByRule returns InputPathsMap keys 'bad key' and 'aws.events.rule-name'.
Traces: events-put-targets-input-paths-map-key-shape#task#6.

Cause 5 of 6: a query-string VALUE containing a carriage return is wrongly refused.
AWS: accepts it - the value grammar excludes \\x00-\\x09, \\x0B, \\x0C and \\x0E-\\x1F but NOT \\x0D (Smithy: QueryStringParametersMap value -> QueryStringValue @pattern ^[^\\x00-\\x09\\x0B\\x0C\\x0E-\\x1F\\x7F]+$), so PutTargets answers 200 with FailedEntryCount 0 and FailedEntries []; a BELL (\\x07) in the same position IS refused.
Emulator: answers HTTP 400 and returns no FailedEntries/FailedEntryCount at all, so a legitimate API-Gateway target cannot be created.
Traces: put-targets-api-gateway-query-string-value-carriage-return-kept-bell-refused#task#7.

Cause 6 of 6: RemoveTargets does not enforce the per-request target limit.
AWS: refuses more than ten ids with 'At most 10 targets allowed for each request.' - a service limit the model does not state (Smithy: RemoveTargets.Ids -> TargetIdList @length 1..100, so this is recorded behavior).
Emulator: answers HTTP 200 and removes them.
Traces: events-remove-targets-id-list-size-bounds#task#8.

Why: every accepted-but-invalid target is a configuration real EventBridge rejects at PutTargets time, so the failure moves to delivery time (or to the first real deployment), and a client that treats a 200 as 'this rule is wired correctly' has no way to learn otherwise - ListTargetsByRule confirms the bad target. Cause 5 is the reverse: a valid target is impossible to create at all. Cause 3 also leaves rules undeletable. The open issues 'events: 5 causes - PutEvents accepts an over-deep detail, PutRule accepts an over-long RoleArn, and PutTargets accepts a FIFO/enum/control-character v...' (20261127T115057Z-events-5-causes-putevents-accepts-an-over-deep.md) and 'events__sqs: 6 causes ...' (20261127T115057Z-events-sqs-6-causes-a-deleted-rule-keeps.md) describe neighbouring PutTargets gaps from the behavioral corpus, but carry none of these replay keys.
Fix: the PutTargets handler's per-target validation (a target-type/parameter-block compatibility gate ahead of member-constraint checks, plus the map-key, path-parameter and query-string-value grammars) and RemoveTargets' per-request id-count check.

## Evidence
Cause 1 (PutTargets, events-put-targets-capacity-provider-base-range-bound#task#8): "status: expected 400, got 200"; "$.__type: missing (expected 'ValidationException', got None)"; "$.message: missing (expected 'Parameter(s) EcsParameters not supported for target: sqs-sink.', got None)" - 33 affected steps. Follow-up ListTargetsByRule (#task#9): "$.Targets[0]: extra (expected None, got {'Id': 'sqs-sink', 'Arn': 'arn:aws:sqs:...', 'EcsParameters': {...'CapacityProviderStrategy': [{'capacityProvider': 'FARGATE', 'weight': 1, 'base': 100000}]...}})". Wrong-message variant (events-put-targets-header-parameter-value-length#task#4, 400/400): expected 'Parameter(s) HttpParameters not supported for target: q1.', got "1 validation error detected: Value 'vvv...' at 'targets.1.member.httpParameters.headerParameters.x-trace' failed to satisfy constraint: Member must have length less than or equal to 512".
Cause 2 (ListTargetsByRule, events-put-targets-http-parameters-key-shape#task#6): "$.Targets[0]: extra (... 'HttpParameters': {'HeaderParameters': {'bad header': 'v'}})"; "$.Targets[1]: extra (... 'QueryStringParameters': {'k\\x01': 'v'})". put-targets-http-query-string-key-length-bounds#task#9 shows stored keys of 513 chars, '' and 512 chars.
Cause 3 (ListTargetsByRule, ...#task#10): "$.Targets[Id=\"blank-path-param\"]: extra (... 'PathParameterValues': ['   '])"; "$.Targets[Id=\"empty-path-param\"]: extra (... 'PathParameterValues': [''])". Teardown DeleteRule (#teardown#12): "status: expected 200, got 400".
Cause 4 (ListTargetsByRule, events-put-targets-input-paths-map-key-shape#task#6): "$.Targets[Id=\"t1\"]: extra (... 'InputPathsMap': {'bad key': '$.detail'})"; "$.Targets[Id=\"t2\"]: extra (... 'InputPathsMap': {'aws.events.rule-name': '$.detail'})".
Cause 5 (PutTargets, put-targets-api-gateway-query-string-value-carriage-return-kept-bell-refused#task#7): "status: expected 200, got 400"; "$.FailedEntries: missing (expected [], got None)"; "$.FailedEntryCount: missing (expected 0, got None)".
Cause 6 (RemoveTargets, events-remove-targets-id-list-size-bounds#task#8): "status: expected 400, got 200"; "$.message: missing (expected 'At most 10 targets allowed for each request.', got None)".

## Steps to Reproduce
1. PutRule, then PutTargets with a target whose Arn is an SQS queue and which carries EcsParameters (any content) - expect 400 'Parameter(s) EcsParameters not supported for target: <id>.', observe 200; ListTargetsByRule shows it stored. Repeat with HttpParameters and with RunCommandParameters on an SQS target.
2. PutTargets on an api-destination target with HeaderParameters key 'bad header' and QueryStringParameters key containing \\x01, an empty key, and a 513-character key - all expected refused.
3. PutTargets on an api-destination target with HttpParameters.PathParameterValues [''] and ['   '] - expected refused; then DeleteRule and observe the 400.
4. PutTargets with InputTransformer.InputPathsMap keys 'bad key' and 'aws.events.rule-name' - expected refused.
5. PutTargets on an API Gateway target with a QueryStringParameters value containing \\r - expect 200/FailedEntryCount 0, observe 400.
6. RemoveTargets with 11 ids - expect 400 'At most 10 targets allowed for each request.'
7. Re-replay the fixtures listed per cause.
