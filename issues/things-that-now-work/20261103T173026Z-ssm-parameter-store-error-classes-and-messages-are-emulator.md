Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-parameter-store-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-parameter-store-lifecycle ssm-parameter-hierarchy-and-batch-operations ssm-alarm-firing-latency-mode-parameter-fix ssm-public-parameter-hierarchy-paging-and-errors ssm-missing-parameter-error-matrix ssm-parameter-labels-and-policy-validation ssm-public-parameter-ami-lookup-and-errors
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|PutParameter|4xx->4xx|$.message
Issue-Key: ssm|PutParameter|4xx->4xx|$.__type
Issue-Key: ssm|GetParametersByPath|4xx->4xx|$.message
Issue-Key: ssm|GetParametersByPath|4xx->4xx|$.Message
Issue-Key: ssm|GetParameter|4xx->4xx|$.message
Issue-Key: ssm|LabelParameterVersion|4xx->4xx|$.message
Issue-Key: ssm|UnlabelParameterVersion|4xx->4xx|$.message
Issue-Key: ssm|DeleteParameter|4xx->4xx|$.Message
Issue-Key: ssm|DeleteParameter|4xx->4xx|$.__type
Issue-Key: ssm|GetParameter|4xx->4xx|$.__type
Issue-Key: ssm|GetParametersByPath|4xx->2xx|$.Message
Issue-Key: ssm|GetParametersByPath|4xx->2xx|$.__type
Issue-Key: ssm|GetParametersByPath|4xx->2xx|status
Issue-Commit: 133bbb645e9618d24e6206d20cee2b832cb9c7e9

# Parameter Store error classes and messages are emulator-authored, not AWS's

> _Note: this issue was identified against the code base at commit `133bbb645e9618d24e6206d20cee2b832cb9c7e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
The Parameter Store error paths reject with locally invented sentences, with the wrong error class, with no message member at all, or (for an invalid NextToken) do not reject. 10 CAUSES. Each AWS message below is a template: the fixed text is quoted verbatim and the variable parts (parameter name, version number, filter key, allowedPattern, token) come from the request and the emulator's own state - no value from a single recording is itself the requirement.

Cause 1 of 10 - PutParameter on an existing name without Overwrite. AWS: 400 ParameterAlreadyExists (Smithy: PutParameter error ParameterAlreadyExists, httpError 400) with the message, whose text is entirely fixed and contains no name: 'The parameter already exists. To overwrite this value, set the overwrite option in the request to true.' Emulator: 400 ParameterAlreadyExists but message 'Parameter <name> already exists; set Overwrite to update it'. Why: a client told to set 'Overwrite' by AWS's own sentence gets emulator prose instead, and anything matching the message fails. Fix: PutParameter handler's already-exists error construction. Traces: ssm-parameter-store-lifecycle#task#3.

Cause 2 of 10 - PutParameter value violating AllowedPattern uses the wrong error class. AWS: 400 __type ParameterPatternMismatchException, message 'Parameter value, cannot be validated against allowedPattern: ' + the request's AllowedPattern (Smithy: PutParameter models both ParameterPatternMismatchException - 'The parameter name isn't valid' - and InvalidAllowedPatternException - 'The request doesn't meet the regular expression requirement', i.e. the supplied pattern itself is malformed). Emulator: __type InvalidAllowedPatternException, message "Value doesn't satisfy the regular expression requirement '<pattern>'". Why: the two classes mean opposite things, so a caller that catches ParameterPatternMismatchException to re-prompt for a value never sees it, and one that catches InvalidAllowedPatternException wrongly concludes its own pattern is bad. Fix: PutParameter validation - classify a value/allowedPattern mismatch as ParameterPatternMismatchException. Traces: ssm-parameter-hierarchy-and-batch-operations#task#5.

Cause 3 of 10 - GetParametersByPath with a Path that does not start with '/'. AWS: 400 whose message is the fixed parameter-name-requirements paragraph with no variable part: 'The parameter doesn''t meet the parameter name requirements. The parameter name must begin with a forward slash "/". It can''t be prefixed with \"aws\" or \"ssm\" (case-insensitive). It must use only letters, numbers, or the following symbols: . (period), - (hyphen), _ (underscore). Special characters are not allowed. All sub-paths, if specified, must use the forward slash symbol "/". Valid example: /get/parameters2-/by1./path0_.' Emulator: 400 with the authored sentence "Path '<path>' doesn't start with a forward slash (/): a parameter hierarchy starts with a forward slash (/)". Fix: GetParametersByPath path validation message. Traces: ssm-alarm-firing-latency-mode-parameter-fix#task#3, ssm-public-parameter-hierarchy-paging-and-errors#task#3.

Cause 4 of 10 - GetParametersByPath with an unsupported ParameterFilters key returns no message. AWS: 400 InvalidFilterKey carrying $.Message 'The following filter key is not valid: <key>. Valid filter keys include: [Type, KeyId, Label].' - fixed text around the rejected key and the valid-key list (recorded behavior only: Smithy InvalidFilterKey declares NO members, yet the service populates Message). Emulator: 400 with $.Message absent. Why: the caller gets an empty error body and cannot learn which key was rejected or what is legal. Fix: GetParametersByPath filter validation - populate InvalidFilterKey.Message. Traces: ssm-alarm-firing-latency-mode-parameter-fix#task#5, ssm-public-parameter-hierarchy-paging-and-errors#task#2.

Cause 5 of 10 - GetParameter with a version selector that does not exist. AWS: 400 ParameterVersionNotFound, message 'Systems Manager could not find version <version> of <full parameter name>. Verify the version and try again.' (Smithy: GetParameter error ParameterVersionNotFound). Emulator: "Parameter <name> has no version or label '<selector>'". Fix: GetParameter selector resolution error construction. Traces: ssm-alarm-firing-latency-mode-parameter-fix#task#12.

Cause 6 of 10 - LabelParameterVersion on a missing parameter returns no message. AWS: 400 ParameterNotFound with message 'Parameter <name> not found.' (Smithy: LabelParameterVersion error ParameterNotFound). Emulator: 400 with $.message absent. Fix: LabelParameterVersion not-found path. Traces: ssm-missing-parameter-error-matrix#task#4.

Cause 7 of 10 - UnlabelParameterVersion with an unknown version. AWS: 400 ParameterVersionNotFound, message 'Systems Manager could not find version <version> of <name>. Verify the version and try again.' (Smithy: UnlabelParameterVersion error ParameterVersionNotFound). Emulator: 'Parameter <name> has no version <version>'. Fix: UnlabelParameterVersion version lookup error construction - share the sentence with cause 5. Traces: ssm-parameter-labels-and-policy-validation#task#5.

Cause 8 of 10 - DeleteParameter on an AWS-published /aws/... parameter is classified not-found instead of denied. AWS: 400 __type AccessDeniedException with $.Message 'Cannot access/update data for the restricted parameter.' - a fixed sentence with no variable part (recorded behavior only: Smithy DeleteParameter models only InternalServerError and ParameterNotFound). Emulator: __type ParameterNotFound with no Message. Why: the restricted public namespace is reported as merely absent, so a caller retries or recreates the name instead of recognising a refusal. Fix: DeleteParameter authorization check for the reserved /aws/ hierarchy. Traces: ssm-public-parameter-ami-lookup-and-errors#task#4.

Cause 9 of 10 - GetParameter with a version selector on a public parameter uses the wrong class and no message. AWS: 400 __type ParameterVersionNotFound with 'Systems Manager could not find version <version> of <name>. Verify the version and try again.' Emulator: __type ParameterNotFound with no message. Why: the parameter does exist; only the version does not, and the caller cannot tell the two apart. Fix: GetParameter selector resolution for the public hierarchy (see also the open issues 20261030T030533Z-ssm-getparameter-cannot-resolve-aws-published-aws-parameters.md and 20261103T154947Z-ssm-getparameter-cannot-resolve-aws-published-public-aws.md, which own the plain public-parameter read but not this version-selector classification). Traces: ssm-public-parameter-ami-lookup-and-errors#task#5.

Cause 10 of 10 - GetParametersByPath silently accepts a bogus NextToken. AWS: 400 InvalidNextToken with $.Message 'The following token is invalid: <token>.' (Smithy: GetParametersByPath error InvalidNextToken with member Message). Emulator: 200 with a normal first page - the token is ignored, not validated. Why: a paginating client that corrupts or reuses a stale token silently restarts the scan and can loop or double-process instead of failing fast. Fix: GetParametersByPath pagination-token decoding and its InvalidNextToken path. Traces: ssm-public-parameter-hierarchy-paging-and-errors#task#1, ssm-public-parameter-hierarchy-paging-and-errors#task#4.

## Evidence
Replay of simparity-wire-aws-v1. Representative (ssm-parameter-store-lifecycle#task#3, PutParameter): expected 400 / actual 400; $.message value_mismatch - expected 'The parameter already exists. To overwrite this value, set the overwrite option in the request to true.', got 'Parameter sim2real-probe-ssm-param-10c56f94 already exists; set Overwrite to update it'. Cause 2 (ssm-parameter-hierarchy-and-batch-operations#task#5): $.__type expected 'ParameterPatternMismatchException', got 'InvalidAllowedPatternException'; $.message expected 'Parameter value, cannot be validated against allowedPattern: ^[a-z0-9.]+$', got "Value doesn't satisfy the regular expression requirement '^[a-z0-9.]+$'". Cause 4 (ssm-alarm-firing-latency-mode-parameter-fix#task#5): $.Message missing, expected 'The following filter key is not valid: Name. Valid filter keys include: [Type, KeyId, Label].'. Cause 6 (ssm-missing-parameter-error-matrix#task#4): $.message missing, expected 'Parameter sim2real-probe-ssm-missing-467ac256 not found.'. Cause 8 (ssm-public-parameter-ami-lookup-and-errors#task#4): $.__type expected 'AccessDeniedException', got 'ParameterNotFound'; $.Message missing, expected 'Cannot access/update data for the restricted parameter.'. Cause 10 (ssm-public-parameter-hierarchy-paging-and-errors#task#1): status expected 400, got 200; $.__type expected 'InvalidNextToken' and $.Message expected 'The following token is invalid: sim2real-probe-not-a-real-token.' were both absent. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1. 2. Representative: fixture ssm-parameter-store-lifecycle step task#3 - PutParameter for an existing name without Overwrite; compare $.message. 3. Then, per trace_ids: PutParameter with AllowedPattern='^[a-z0-9.]+$' and a non-matching Value; GetParametersByPath with a Path lacking the leading '/'; GetParametersByPath with ParameterFilters key 'Name'; GetParameter with name:3 where only two versions exist; LabelParameterVersion on an absent parameter; UnlabelParameterVersion with ParameterVersion=99; DeleteParameter on /aws/service/ami-amazon-linux-latest/...; GetParameter on a public parameter with :999999999; GetParametersByPath with NextToken='sim2real-probe-not-a-real-token'.
