Issue-Source: trace-sweep-divergence
Issue-Trace: cwm-unit-out-of-enum
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwm-unit-out-of-enum
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutMetricData|4xx->4xx|$.__type
Issue-Key: monitoring|PutMetricData|4xx->4xx|$headers.x-amzn-query-error
Issue-Related: 20261029T203422Z-monitoring-cloudwatch-published-metric-data-is-unqueryable-and-metric.md
Issue-Commit: 431058ad591415bfb50dfcb201144b6c4e439813

# PutMetricData out-of-enum Unit raises ValidationException instead of InvalidParameterValue

> _Note: this issue was identified against the code base at commit `431058ad591415bfb50dfcb201144b6c4e439813`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: PutMetricData rejects an unmodelled MetricData[].Unit value with the wrong error class, the wrong awsQuery error code and an emulator-authored message. Fixture cwm-unit-out-of-enum, step task#2.

AWS: Smithy (com.amazonaws.cloudwatch) binds PutMetricData to exactly four errors - InternalServiceFault, InvalidParameterCombinationException, InvalidParameterValueException and MissingRequiredParameterException. There is NO ValidationException on this operation. MetricDatum.Unit targets the StandardUnit enum, whose value set is [Seconds, Microseconds, Milliseconds, Bytes, Kilobytes, Megabytes, Gigabytes, Terabytes, Bits, Kilobits, Megabits, Gigabits, Terabits, Percent, Count, Bytes/Second, Kilobytes/Second, Megabytes/Second, Gigabytes/Second, Terabytes/Second, Bits/Second, Kilobits/Second, Megabits/Second, Gigabits/Second, Terabits/Second, Count/Second, None]. A Unit outside that set is an out-of-range parameter VALUE, which the model classifies as InvalidParameterValueException: errorKind client, httpError 400, awsQueryError {code InvalidParameterValue, httpResponseCode 400} (Smithy: InvalidParameterValueException doc "The value of an input parameter is bad or out-of-range."). The recorded response therefore carries status 400, body __type 'com.amazon.coral.service#InvalidParameterValueException' and header x-amzn-query-error 'InvalidParameterValue;Sender'.
  Message rule (from the recording; recorded behavior only for the exact wording): the fixed text is 'The parameter ' ... ' must be a value in the set [ ' ... ' ].' - note the single space after '[' and before ']', and ', ' between members. Its variable parts are (a) the fully qualified awsQuery parameter path of the offending member, taken from the request - 'MetricData.member.<n>.Unit' where <n> is the 1-based position of the datum in MetricData (the recording's '1' is this request's index, not part of the template) - and (b) the StandardUnit value set, which is fixed by the model and rendered as a SET (the recording's ordering is incidental; membership, not order, is the requirement).

Emulator: answers 400 but with __type 'com.amazon.coral.validate#ValidationException', header x-amzn-query-error 'ValidationError;Sender', and the generic constraint-report message "1 validation error detected: Value at 'metricData.1.member.unit' failed to satisfy constraint: Member must satisfy enum value set: [Bits, Bits/Second, Bytes, ...]". So all three requirement-bearing fields diverge: error class, awsQuery error code, and message. It also uses a lowercased, differently-shaped member path ('metricData.1.member.unit' rather than 'MetricData.member.1.Unit') and omits the offending value from the message entirely.

Why: an SDK maps __type / x-amzn-query-error onto a concrete exception type, so a client that catches CloudWatch's InvalidParameterValueException (or branches on the InvalidParameterValue query code) does not catch this refusal at all and the publish failure escapes as an unmodelled error. Code that parses the message to find which datum and which parameter was rejected also fails, because the emulator emits a different member path and drops the bad value.

Fix: PutMetricData's input validation for the MetricData/EntityMetricData Unit member in the CloudWatch service crate - the enum check must raise the operation's modelled InvalidParameterValueException (emitting __type com.amazon.coral.service#InvalidParameterValueException and x-amzn-query-error 'InvalidParameterValue;Sender') with the parameter-set message above, instead of falling through to the generic Smithy constraint-violation/ValidationException path used for structurally invalid input.

Related but NOT covering this: open issue 20261029T203422Z-monitoring-cloudwatch-published-metric-data-is-unqueryable-and-metric.md owns other PutMetricData divergences (entity key-attribute message, namespace acceptance, StatisticValues consistency, GetMetricData query validation); the replay report leaves this cluster unowned, so it is filed separately.

Traces: cwm-unit-out-of-enum#task#2.

## Evidence
Fixture cwm-unit-out-of-enum, step task#2, operation PutMetricData (service monitoring). Expected status 400, actual 400 - but:
  $.__type: expected 'com.amazon.coral.service#InvalidParameterValueException', got 'com.amazon.coral.validate#ValidationException'
  $headers.x-amzn-query-error: expected 'InvalidParameterValue;Sender', got 'ValidationError;Sender'
  $.message: expected 'The parameter MetricData.member.1.Unit must be a value in the set [ Megabits, Terabits, Gigabits, Count, Bytes, Gigabytes, Gigabytes/Second, Kilobytes, Kilobits/Second, Terabytes, Terabits/Second, Bytes/Second, Percent, Megabytes, Megabits/Second, Milliseconds, Microseconds, Kilobytes/Second, Gigabits/Second, Megabytes/Second, Bits, Bits/Second, Count/Second, Seconds, Kilobits, Terabytes/Second, None ].'
                got "1 validation error detected: Value at 'metricData.1.member.unit' failed to satisfy constraint: Member must satisfy enum value set: [Bits, Bits/Second, Bytes, Bytes/Second, Count, Count/Second, Gigabits, Gigabits/Second, Gigabytes, Gigabytes/Second, Kilobits, Kilobits/Second, Kilobytes, Kilobytes/Second, Megabits, Megabits/Second, Megabytes, Megabytes/Second, Microseconds, Milliseconds, None, Percent, Seconds, Terabits, Terabits/Second, Terabytes, Terabytes/Second]"
Model citation: Smithy PutMetricData errors = {InternalServiceFault, InvalidParameterCombinationException, InvalidParameterValueException, MissingRequiredParameterException} - ValidationException is not among them; InvalidParameterValueException -> httpError 400, awsQueryError code InvalidParameterValue; MetricDatum.Unit -> StandardUnit enum (27 values).

## Steps to Reproduce
1. Boot the emulator from this checkout and point a CloudWatch (service token 'monitoring', JSON 1.0 target protocol) client at it.
2. Call PutMetricData with a valid Namespace and a single MetricData entry whose Unit is a string outside the StandardUnit value set (as recorded in fixture cwm-unit-out-of-enum, step task#2).
3. Inspect the 400 response: read the body's __type and message and the x-amzn-query-error response header.
4. Observed: __type 'com.amazon.coral.validate#ValidationException', x-amzn-query-error 'ValidationError;Sender', generic '1 validation error detected...' message with member path 'metricData.1.member.unit'.
5. Expected: __type 'com.amazon.coral.service#InvalidParameterValueException', x-amzn-query-error 'InvalidParameterValue;Sender', message 'The parameter MetricData.member.1.Unit must be a value in the set [ <StandardUnit values> ].'
6. Re-replay with replay_wire_corpus and confirm cwm-unit-out-of-enum#task#2 passes.
