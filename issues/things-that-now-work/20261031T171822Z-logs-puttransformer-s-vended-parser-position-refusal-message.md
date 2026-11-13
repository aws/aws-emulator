Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected logs.invalid-parameter-validation-on-create
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected|run#11|AssertionError|assert message == "Log processor parseWAF must be in the first position of the processor list", (
Issue-Key: logs.invalid-parameter-validation-on-create|run#17|AssertionError|assert msg_stream == expected, f"expected message {expected!r}, got {msg_stream!r}"
Issue-Commit: 74eb505d6f038f764bd1308b44906372fa72a184

# logs: PutTransformer's vended-parser-position refusal message is rewritten, and CreateLogStream's validation message echoes the anchored Smithy regex instead of the pattern AWS publishes (2 causes)

> _Note: this issue was identified against the code base at commit `74eb505d6f038f764bd1308b44906372fa72a184`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
Two independent net-new root causes in the CloudWatch Logs emulator's refusal-message construction, both grounded in failing behavioral checks of the `logs` service. Cause count: 2.

=== Cause 1 of 2: PutTransformer's "vended parser must be first" refusal carries emulator-authored wording ===
AWS: Smithy PutTransformer takes logGroupIdentifier (@required, @length 1-2048) and transformerConfig -> Processors (@required, @length 1-20) and models InvalidParameterException (client, HTTP 400). The on-disk model states the vended-parser position rule only in the Processor member documentation, so the refusal text is recorded behavior only. The recording shows AWS refuses a chain that places a vended parser anywhere but position 0 with HTTP 400, code InvalidParameterException and the message "Log processor <processorName> must be in the first position of the processor list". The FIXED text of that message is "Log processor " + " must be in the first position of the processor list" (no trailing period); the only VARIABLE part is <processorName>, the name of the mis-positioned vended processor taken from the request's transformerConfig — here parseWAF, because the submitted chain was [{"parseJSON": {}}, {"parseWAF": {}}]. The position index is NOT part of AWS's message.
Emulator: the status (400) and the code (InvalidParameterException) are right, but the message is rewritten to "<processorName> must be the first processor in a transformer, and this transformer has it at transformerConfig[<index>]." — none of AWS's fixed text survives, and the emulator adds a request-derived index clause AWS does not emit.
Why: a client that classifies the several InvalidParameterException causes PutTransformer shares (no leading parser, vended parser out of position, bad processor arguments, too many processors) by the message text mis-reads the emulator's refusal, and tooling that string-matches AWS's documented sentence does not match at all. Note the sibling cause filed for service cloudwatch__logs (same suite run, check cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected) shows the same handler also rewrites the DIFFERENT "transformer config should begin with parser" message; both texts must be emitted verbatim, they are not interchangeable.
Fix: the error-message construction in the PutTransformer handler's transformerConfig validation (the vended-parser-position rule), which must emit AWS's fixed sentence with only the processor name substituted and must not append the position index.
Traces: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected.

=== Cause 2 of 2: CreateLogStream's constraint-violation message echoes the anchored Smithy pattern rather than the pattern AWS publishes ===
AWS: Smithy CreateLogStream.logStreamName targets LogStreamName (@length min 1 max 512, @pattern "^[^:*]*$"), and CreateLogStream models InvalidParameterException (client, HTTP 400). A logStreamName containing ':' is refused with HTTP 400, code InvalidParameterException and the AWS framework validation message. Recorded behavior: that message is "1 validation error detected: Value at 'logStreamName' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:*]*". Its FIXED text is the framework template "<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; its VARIABLE parts are the error count (1), the member name taken from the request shape ('logStreamName') and the pattern. The pattern AWS renders is the UNANCHORED body "[^:*]*" — AWS strips the model's '^' and '$' anchors when it echoes a @pattern in this message. The same check confirms the emulator already renders the sibling CreateLogGroup message correctly, with logGroupName's pattern echoed as "[\\.\\-_/#A-Za-z0-9]+" (also anchor-stripped), so the divergence is specific to this member's rendering, not to the template.
Emulator: the status (400), code (InvalidParameterException), count, member name and template are all right, but the pattern is echoed verbatim from the Smithy trait including the anchors: "... Member must satisfy regular expression pattern: ^[^:*]*$".
Why: AWS's message is a contract clients parse — SDK retry/validation wrappers, CLI users and test harnesses extract the pattern from it and re-apply it client-side. An anchored pattern is a different regex string than AWS ever returns, so an exact-match assertion fails and a client that compiles the echoed pattern as a search (rather than a full match) now rejects names AWS accepts. It also makes the emulator's constraint rendering inconsistent between two members of the same service.
Fix: the constraint-violation message serializer shared by the logs input-validation layer (the @pattern echo must strip a leading '^' and trailing '$' before interpolation), exercised from CreateLogStream's logStreamName validation.
Traces: logs.invalid-parameter-validation-on-create.

## Evidence
Cause 1 — logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected [mismatch]:
AssertionError: expected the first-position refusal for parseWAF, got 'parseWAF must be the first processor in a transformer, and this transformer has it at transformerConfig[1].'
Expected message: "Log processor parseWAF must be in the first position of the processor list".
The code assertion (code == 'InvalidParameterException') passed first, so HTTP 400 and the error code agree; only the message diverges. In that recording 'parseWAF' is the variable part (the mis-positioned processor from the submitted chain [{"parseJSON": {}}, {"parseWAF": {}}]); the rest of AWS's sentence is fixed, and AWS emits no index.

Cause 2 — logs.invalid-parameter-validation-on-create [mismatch]:
AssertionError: expected message "1 validation error detected: Value at 'logStreamName' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:*]*", got "1 validation error detected: Value at 'logStreamName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[^:*]*$"
The check reached that assertion only after the preceding ones passed, i.e. CreateLogStream already answers HTTP 400 with code InvalidParameterException, and the CreateLogGroup arm of the same check already matched AWS exactly with its pattern echoed unanchored as "[\\.\\-_/#A-Za-z0-9]+". Smithy (com.amazonaws.cloudwatchlogs#LogStreamName): string, @length min 1 max 512, @pattern "^[^:*]*$" — the emulator is echoing that trait verbatim; AWS echoes its body without the anchors.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or run the two checks individually.

Cause 1:
1. logs CreateLogGroup(logGroupName=<group>); CreateLogStream(logGroupName=<group>, logStreamName='app').
2. logs PutTransformer(logGroupIdentifier=<group>, transformerConfig=[{"parseJSON": {}}, {"parseWAF": {}}]).
3. Observe HTTP 400 / InvalidParameterException. The message must be exactly "Log processor parseWAF must be in the first position of the processor list" (fixed text, with the processor name taken from the request); the emulator instead returns "parseWAF must be the first processor in a transformer, and this transformer has it at transformerConfig[1]."
4. logs GetTransformer(logGroupIdentifier=<group>) must return an empty transformerConfig — the refused chain must not be installed.

Cause 2:
1. logs CreateLogGroup(logGroupName=<valid-group>).
2. logs CreateLogStream(logGroupName=<valid-group>, logStreamName='invalid:stream:<suffix>') — a logStreamName containing ':'.
3. Observe HTTP 400 / InvalidParameterException. The message must be exactly "1 validation error detected: Value at 'logStreamName' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:*]*"; the emulator returns the same sentence with the pattern anchored as "^[^:*]*$".
4. logs DescribeLogStreams(logGroupName=<valid-group>) must list no stream (the refused call creates nothing) — this part already holds.
5. For contrast in the same flow, CreateLogGroup(logGroupName='<base> !!! ') is refused with the pattern echoed correctly unanchored as "[\\.\\-_/#A-Za-z0-9]+".
