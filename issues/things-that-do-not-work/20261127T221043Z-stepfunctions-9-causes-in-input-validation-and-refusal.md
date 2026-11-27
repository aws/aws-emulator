Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.create-activity-tag-value-over-256-chars-rejected stepfunctions.create-state-machine-alias-version-arn-of-257-chars-rejected stepfunctions.delete-activity-arn-over-256-chars-rejected stepfunctions.delete-state-machine-alias-arn-over-256-chars-rejected stepfunctions.get-execution-history-next-token-length-bounds-rejected stepfunctions.list-executions-map-run-arn-length-bound-rejected stepfunctions.list-tags-for-resource-arn-over-256-characters-rejected stepfunctions.start-execution-trace-header-over-256-characters-rejected stepfunctions.describe-execution-arn-length-bounds-rejected stepfunctions.create-state-machine-alias-name-length-bounds-rejected stepfunctions.untag-resource-tag-key-outside-1-to-128-chars-rejected stepfunctions.test-state-context-over-262144-bytes-rejected stepfunctions.test-state-mock-result-size-bound-rejected stepfunctions.test-state-variables-over-262144-utf8-bytes-rejected stepfunctions.retry-interval-seconds-above-ceiling-rejected-at-create-state-machine stepfunctions.retry-intervalseconds-above-limit-rejected-at-create-state-machine stepfunctions.retry-max-attempts-above-ceiling-rejected-at-create-state-machine stepfunctions.retry-maxdelayseconds-above-limit-rejected-at-create-state-machine stepfunctions.retry-jitter-strategy-lowercase-value-rejected-at-create-state-machine stepfunctions.catch-result-path-with-wildcard-rejected-on-create-and-update stepfunctions.jsonpath-filter-and-deep-scan-select-items stepfunctions.jsonpath-wildcard-flattens-parallel-results
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.create-activity-tag-value-over-256-chars-rejected|run#9|AssertionError|assert got == expected, f"a 257-character tag value gave {got!r}, expected {expected!r}"
Issue-Key: stepfunctions.create-state-machine-alias-version-arn-of-257-chars-rejected|run#19|AssertionError|assert got_over == (
Issue-Key: stepfunctions.delete-activity-arn-over-256-chars-rejected|run#8|AssertionError|assert got == expected, f"a 257-character activity ARN gave {got!r}, expected {expected!r}"
Issue-Key: stepfunctions.delete-state-machine-alias-arn-over-256-chars-rejected|run#18|AssertionError|assert got == expected, f"a 257-character alias ARN gave {got!r}, expected {expected!r}"
Issue-Key: stepfunctions.get-execution-history-next-token-length-bounds-rejected|run#23|AssertionError|assert got == (
Issue-Key: stepfunctions.list-executions-map-run-arn-length-bound-rejected|run#18|AssertionError|assert got == (
Issue-Key: stepfunctions.list-tags-for-resource-arn-over-256-characters-rejected|run#14|AssertionError|assert (code, message) == (
Issue-Key: stepfunctions.start-execution-trace-header-over-256-characters-rejected|run#20|AssertionError|assert (code, message) == (
Issue-Key: stepfunctions.describe-execution-arn-length-bounds-rejected|run#19|AssertionError|assert got == want, f"DescribeExecution refusals were {got!r}, expected {want!r}"
Issue-Key: stepfunctions.create-state-machine-alias-name-length-bounds-rejected|run#17|AssertionError|assert got == (
Issue-Key: stepfunctions.untag-resource-tag-key-outside-1-to-128-chars-rejected|run#11|AssertionError|assert got == expected, f"a 129-character key was refused with {got!r}, expected {expected!r}"
Issue-Key: stepfunctions.test-state-context-over-262144-bytes-rejected|run#11|AssertionError|assert got == expected, f"a context of {label} was answered with {got!r}"
Issue-Key: stepfunctions.test-state-mock-result-size-bound-rejected|run#11|AssertionError|assert got == refusal, f"a {_RESULT_MAX + 1}-byte ASCII mock result was answered {got!r}"
Issue-Key: stepfunctions.test-state-variables-over-262144-utf8-bytes-rejected|run#13|AssertionError|assert got == expected, f"a {_utf8(ascii_over)}-byte ASCII document: expected {expected!r}, got {got!r}"
Issue-Key: stepfunctions.retry-interval-seconds-above-ceiling-rejected-at-create-state-machine|_refusal#14|AssertionError|raise AssertionError(f"CreateStateMachine accepted IntervalSeconds={_CEILING + 1} and made a machine")
Issue-Key: stepfunctions.retry-intervalseconds-above-limit-rejected-at-create-state-machine|_refusal#11|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: stepfunctions.retry-max-attempts-above-ceiling-rejected-at-create-state-machine|_refusal#14|AssertionError|raise AssertionError(f"CreateStateMachine accepted MaxAttempts={_CEILING + 1} and made a machine")
Issue-Key: stepfunctions.retry-maxdelayseconds-above-limit-rejected-at-create-state-machine|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: stepfunctions.retry-jitter-strategy-lowercase-value-rejected-at-create-state-machine|run#21|AssertionError|assert got == want, f"JitterStrategy 'full' was refused with {got!r}, expected {want!r}"
Issue-Key: stepfunctions.catch-result-path-with-wildcard-rejected-on-create-and-update|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {answer!r}")
Issue-Key: stepfunctions.jsonpath-filter-and-deep-scan-select-items|run#4|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.jsonpath-wildcard-flattens-parallel-results|run#3|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 9 causes in input validation and refusal messages — ValidationException truncates the offending value, empty ARNs answer the wrong error, byte-size limits reported as character length, Retry bounds unenforced, and JSONPath filter/wildcard paths rejected

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Cause 1 of 9: the ValidationException message truncates the offending value instead of quoting it in full.
AWS: a @length violation is reported as "1 validation error detected: Value '<the whole value as submitted>' at '<member path>' failed to satisfy constraint: Member must have length less than or equal to <max>". The fixed text is AWS's; the quoted value, the member path and the limit come from the request and the model. Smithy: Arn @length max 256 (activityArn, stateMachineAliasArn, resourceArn, routingConfiguration[].stateMachineVersionArn), LongArn @length max 2000 (mapRunArn), TraceHeader @length max 256, PageToken @length max 1024, Tag value @length max 256.
Emulator: the value is cut off and replaced with an ellipsis and a byte count — "Value 'arn:aws:states:...zzzz... [257 bytes]' at 'activityArn' failed to satisfy constraint: ...".
Why: a client that echoes, logs or parses the rejected value out of the message gets a mangled value; the message is not the one AWS returns for the same request.
Fix: the validation-error message formatter shared by the Step Functions handlers (quote the submitted value verbatim).
Traces: stepfunctions.create-activity-tag-value-over-256-chars-rejected, stepfunctions.create-state-machine-alias-version-arn-of-257-chars-rejected, stepfunctions.delete-activity-arn-over-256-chars-rejected, stepfunctions.delete-state-machine-alias-arn-over-256-chars-rejected, stepfunctions.get-execution-history-next-token-length-bounds-rejected, stepfunctions.list-executions-map-run-arn-length-bound-rejected, stepfunctions.list-tags-for-resource-arn-over-256-characters-rejected, stepfunctions.start-execution-trace-header-over-256-characters-rejected.

Cause 2 of 9: an empty ARN is answered with ValidationException instead of InvalidArn.
AWS: DescribeExecution with executionArn="" answers InvalidArn (HTTP 400) with the message "Invalid Arn: 'Invalid ARN prefix: '" — the ARN syntax check runs before the length constraint. Smithy: DescribeExecution errors include InvalidArn (client, 400) and executionArn targets Arn @length 1..256.
Emulator: ValidationException "1 validation error detected: Value '' at 'executionArn' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: a client that handles InvalidArn separately (an un-retryable malformed-ARN path) never sees it; the error code and message both differ.
Fix: the ARN validation order in the DescribeExecution handler (and the other ARN-taking reads that share it).
Traces: stepfunctions.describe-execution-arn-length-bounds-rejected.

Cause 3 of 9: several constraint violations on one member are not aggregated into a multi-error ValidationException.
AWS: when one value breaks two constraints, both are reported: CreateStateMachineAlias with name="" answers "2 validation errors detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?=.*[a-zA-Z_\\-\\.])[a-zA-Z0-9_\\-\\.]+$". Smithy: CreateStateMachineAlias.name targets CharacterRestrictedName @length 1..80 with that @pattern.
Emulator: only the length violation is reported — "1 validation error detected: Value '' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: a caller fixing the input from the message is told about one problem and gets refused again; the error count in the leading clause is wrong too.
Fix: the validation-error aggregation in the shared Step Functions input validator (collect all violations of a member, then render the count).
Traces: stepfunctions.create-state-machine-alias-name-length-bounds-rejected.

Cause 4 of 9: UntagResource reports the tagKeys constraint per member instead of at the list level.
AWS: an out-of-bounds tag key is reported against the list: "1 validation error detected: Value '[<k1>, <k2>]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]" — the quoted value is the whole list rendered '[a, b]'. Smithy: UntagResource.tagKeys targets TagKeyList whose member is length 1..128.
Emulator: "Value '<the single key>' at 'tagKeys.2.member' failed to satisfy constraint: Member must have length less than or equal to 128".
Why: the member path and the quoted value both differ from AWS, so message-driven clients and recorded expectations do not match.
Fix: the list-member constraint rendering in the shared validation-error formatter.
Traces: stepfunctions.untag-resource-tag-key-outside-1-to-128-chars-rejected.

Cause 5 of 9: TestState's 262144-byte limits are reported as a character length, not a UTF-8 byte size.
AWS: context, mock.result and variables are byte-limited, and the refusal reads "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have size less than or equal to 262144 bytes in UTF-8 encoding" — so a multibyte document under 262144 characters but over 262144 bytes is also refused. Smithy: TestState.context/variables target SensitiveData @length 0..262144.
Emulator: "... failed to satisfy constraint: Member must have length less than or equal to 262144" — the wrong sentence, and a character-count rule.
Why: the message differs, and multibyte payloads are admitted here that AWS refuses (or vice versa), so the size boundary a client tests against moves.
Fix: TestState's input validation in its handler (measure UTF-8 bytes and use AWS's size wording).
Traces: stepfunctions.test-state-context-over-262144-bytes-rejected, stepfunctions.test-state-mock-result-size-bound-rejected, stepfunctions.test-state-variables-over-262144-utf8-bytes-rejected.

Cause 6 of 9: Retry field numeric ceilings are not validated at CreateStateMachine.
AWS: a Retrier's IntervalSeconds, MaxAttempts and MaxDelaySeconds have documented maxima; a definition exceeding them is refused with InvalidDefinition "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: ... at /States/<state>/Retry[<i>]/<field>'" — the fixed text is AWS's, the path comes from the definition.
Emulator: CreateStateMachine accepts IntervalSeconds=100000000, MaxAttempts=100000000 and an over-limit MaxDelaySeconds and returns a stateMachineArn.
Why: definitions that AWS refuses deploy here, so the refusal a client depends on never happens and the retrier behaves out of spec at run time.
Fix: definition validation in the CreateStateMachine handler (Retrier field ranges).
Traces: stepfunctions.retry-interval-seconds-above-ceiling-rejected-at-create-state-machine, stepfunctions.retry-intervalseconds-above-limit-rejected-at-create-state-machine, stepfunctions.retry-max-attempts-above-ceiling-rejected-at-create-state-machine, stepfunctions.retry-maxdelayseconds-above-limit-rejected-at-create-state-machine.

Cause 7 of 9: the JitterStrategy enum is listed in the wrong order in the SCHEMA_VALIDATION_FAILED message.
AWS: the refusal reads "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: Value should be one of the following: [NONE, FULL] at /States/<state>/Retry[<i>]/JitterStrategy'" — the bracketed list is fixed text in that order.
Emulator: "Value should be one of the following: [FULL, NONE] at ...".
Why: the message is not AWS's; a client comparing or displaying the allowed-value list sees a different one.
Fix: the enum rendering for JitterStrategy in the definition validator.
Traces: stepfunctions.retry-jitter-strategy-lowercase-value-rejected-at-create-state-machine.

Cause 8 of 9: a Catcher's ResultPath containing a wildcard is accepted at create and update.
AWS: ResultPath must be a reference path, which admits no wildcard; CreateStateMachine and UpdateStateMachine both refuse such a definition with InvalidDefinition / SCHEMA_VALIDATION_FAILED naming /States/<state>/Catch[<i>]/ResultPath.
Emulator: both calls succeed and return a stateMachineArn.
Why: an invalid definition deploys, and the wildcard path is then mis-evaluated at run time instead of being refused.
Fix: definition validation in the CreateStateMachine/UpdateStateMachine handlers (reference-path validation of Catch ResultPath).
Traces: stepfunctions.catch-result-path-with-wildcard-rejected-on-create-and-update.

Cause 9 of 9: JSONPath filter, deep-scan and wildcard expressions are not evaluated as reference paths.
AWS: InputPath/ResultSelector accept the JSONPath forms Step Functions supports — a filter ($..interests[?(@.category=='home')]) selects the matching items and a wildcard ($[*][*]) flattens a Parallel state's results; the execution SUCCEEDs.
Emulator: the execution FAILs with causes of the emulator's own — "'$..interests[?(@.category=='home')]' is not a reference path this service evaluates: ... carries an empty member name" and "'$[*][*]' is not a reference path this service evaluates: ... carries the subscript '*', which is neither a quoted member name nor an array index".
Why: documented JSONPath selection in InputPath/ResultSelector cannot be used, and the failure is reported with a sentence that is not an AWS error.
Fix: the JSONPath reference-path evaluator in the Step Functions execution engine.
Traces: stepfunctions.jsonpath-filter-and-deep-scan-select-items, stepfunctions.jsonpath-wildcard-flattens-parallel-results.

## Evidence
Cause 1: e.g. "a 257-character activity ARN gave ('ValidationException', \"... Value 'arn:aws:states:...zzz... [257 bytes]' at 'activityArn' failed to satisfy constraint: Member must have length less than or equal to 256\"), expected ... Value '<full 257-char ARN>' ..." (same shape for tag value, alias ARN, version ARN, mapRunArn 2001 bytes, nextToken 1025 bytes, resourceArn, traceHeader).
Cause 2: expected {'empty': ('InvalidArn', "Invalid Arn: 'Invalid ARN prefix: '")}, got {'empty': ('ValidationException', "...Member must have length greater than or equal to 1")}.
Cause 3: got 1 validation error; expected "2 validation errors detected: ... length greater than or equal to 1; ... must satisfy regular expression pattern: ^(?=.*[a-zA-Z_\\-\\.])[a-zA-Z0-9_\\-\\.]+$".
Cause 4: got "Value 'kkk...' at 'tagKeys.2.member' ..."; expected "Value '[kkk..., kkk...]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]".
Cause 5: got "Member must have length less than or equal to 262144"; expected "Member must have size less than or equal to 262144 bytes in UTF-8 encoding" at 'context', 'mock.result' and 'variables'.
Cause 6: "CreateStateMachine accepted IntervalSeconds=100000000 and made a machine"; "CreateStateMachine accepted MaxAttempts=100000000 and made a machine"; "the call was expected to be refused but returned {'stateMachineArn': ...}".
Cause 7: got "[FULL, NONE]", expected "[NONE, FULL]".
Cause 8: "the call was expected to be refused but succeeded: {'stateMachineArn': ...}".
Cause 9: "a filter InputPath must evaluate, got 'FAILED' cause=\"'$..interests[?(@.category=='home')]' is not a reference path this service evaluates ...\""; "a wildcard ResultSelector must evaluate, got 'FAILED' cause=\"'$[*][*]' is not a reference path this service evaluates ...\"".

## Steps to Reproduce
1. Call DeleteActivity / DeleteStateMachineAlias / ListTagsForResource / ListExecutions / GetExecutionHistory / StartExecution / CreateActivity / CreateStateMachineAlias with an over-length ARN, nextToken, traceHeader, tag value or version ARN and read the quoted value in the message (cause 1).
2. Call DescribeExecution with executionArn="" (cause 2) and CreateStateMachineAlias with name="" (cause 3).
3. Call UntagResource with a 129-character tag key (cause 4) and TestState with a 262145-byte context / mock.result / variables document (cause 5).
4. CreateStateMachine with Retry IntervalSeconds=100000000, MaxAttempts=100000000, an over-limit MaxDelaySeconds (cause 6), JitterStrategy='full' (cause 7) and a Catch ResultPath containing a wildcard (cause 8).
5. Run a state machine whose InputPath is a JSONPath filter and whose ResultSelector uses '$[*][*]' (cause 9).
6. Or replay the check ids in trace_ids.
