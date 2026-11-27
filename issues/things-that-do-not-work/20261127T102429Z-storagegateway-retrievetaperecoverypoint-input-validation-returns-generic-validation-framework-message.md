Issue-Source: trace-sweep-divergence
Issue-Trace: storagegateway-tape-recovery-point-retrieval-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: storagegateway-tape-recovery-point-retrieval-errors
Issue-Shape-Id: com.amazonaws.storagegateway#StorageGateway_20130630
Issue-Key: storagegateway|RetrieveTapeRecoveryPoint|4xx->4xx|$.message
Issue-Commit: e8f09314d35987b191cae688f7fba7f207639dbf

# RetrieveTapeRecoveryPoint input validation returns generic validation-framework message instead of InvalidGatewayRequestException wording

> _Note: this issue was identified against the code base at commit `e8f09314d35987b191cae688f7fba7f207639dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RetrieveTapeRecoveryPoint

## Description
Storage Gateway (awsJson1_1, API version 2013-06-30) rejects a malformed RetrieveTapeRecoveryPoint request with the right HTTP status but the wrong error payload: it emits the shared cross-service validation-framework message instead of Storage Gateway's own InvalidGatewayRequestException wording. One cause.

Cause 1 of 1: RetrieveTapeRecoveryPoint input validation uses the generic validation-framework message text (fixture storagegateway-tape-recovery-point-retrieval-errors, steps task#2, task#3, task#4).
AWS: Smithy: storagegateway#RetrieveTapeRecoveryPoint input RetrieveTapeRecoveryPointInput declares TapeARN (required, @length 50..500, @pattern ^arn:(aws(|-cn|-us-gov|-iso[A-Za-z0-9_-]*|-eusc)):storagegateway:[a-z\-0-9]+:[0-9]+:tape\/[0-9A-Z]{5,16}$) and GatewayARN (required, @length 50..500); its only modelled client error is InvalidGatewayRequestException (httpError 400, members `message` and `error`/StorageGatewayError{errorCode,errorDetails}), alongside InternalServerError (500). For an input member that fails its constraints the recorded response is HTTP 400 whose `message` is ONE aggregated sentence: 'The following request fields contain invalid input: tapeARN.' The fixed text of that message class is 'The following request fields contain invalid input: ' followed by a comma-separated list of member names and a terminating '.'; the only variable part is that list, rendered in the service's lower-camel spelling (tapeARN, gatewayARN) and taken from the request. Every offending field is named in ONE message, and the message never contains the submitted value, the constraint, or the regex. No ValidationException is in this operation's modelled error set.
Emulator: answers HTTP 400 but with the generic validation-framework text "1 validation error detected: Value '<submitted tapeARN>' at 'tapeARN' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modelled regex>" - a different wording that echoes the submitted value and the raw pattern and uses a different member-spelling convention.
Why: the error message is part of the contract a client uses to discover which request fields it got wrong. A caller matching Storage Gateway's sentence sees neither the expected text nor the aggregated field list, so SDK-level message assertions, log scrapers and alarm patterns written against AWS never match; and because the emulator's text embeds the submitted ARN and the regex, the message varies per request and cannot be matched as a constant at all. Leaking the modelled regex also exposes detail AWS does not return.
Fix: the RetrieveTapeRecoveryPoint input validation in the Storage Gateway service - route it through Storage Gateway's aggregated InvalidGatewayRequestException message builder (the same one CreateTapePool needs) which collects ALL invalid/missing member names and renders them as one sentence, rather than the shared per-field validation formatter.
Traces: storagegateway-tape-recovery-point-retrieval-errors#task#2 (representative), plus #task#3 and #task#4 in the same cluster.

Related but NOT covering this: open issue 20261110T034016Z-storagegateway-storage-gateway-error-replies-use-non-aws-exception reports the same generic-formatter fault for CreateTapePool and DescribeGatewayInformation. Its replay key names only the storagegateway-tape-pool-create-validation-errors and storagegateway-gateway-scoped-not-found-errors fixtures, so the RetrieveTapeRecoveryPoint path recorded here is not exercised by it and must be closed separately.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200): divergence cluster service=storagegateway, operation=RetrieveTapeRecoveryPoint, verdict=mismatch, expected_status 400 / actual_status 400, 3 affected steps.

  $.message: value_mismatch
    expected: 'The following request fields contain invalid input: tapeARN.'
    actual:   "1 validation error detected: Value 'arn:aws:storagegateway:<region>:<account>:tape/<tapeId>' at 'tapeARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws(|-cn|-us-gov|-iso[A-Za-z0-9_-]*|-eusc)):storagegateway:[a-z\\-0-9]+:[0-9]+:tape\\/[0-9A-Z]{5,16}$"

Message anatomy - expected: fixed text 'The following request fields contain invalid input: ' + comma-separated offending member names (here the single name 'tapeARN', taken from the request) + '.'. Nothing else varies. The emulator's text instead carries the submitted ARN value and the modelled pattern, neither of which AWS returns in this message.

Cluster members: storagegateway-tape-recovery-point-retrieval-errors#task#2 (representative), #task#3, #task#4.

Model basis: services/service-com-amazonaws-storagegateway-storagegateway-20130630/smithy-model - RetrieveTapeRecoveryPoint input TapeARN (required, @length 50..500, @pattern as above), GatewayARN (required, @length 50..500); errors InvalidGatewayRequestException (client, 400), InternalServerError (server, 500). ValidationException is not modelled for this operation.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. POST to the Storage Gateway endpoint with X-Amz-Target StorageGateway_20130630.RetrieveTapeRecoveryPoint and a body whose TapeARN violates the modelled TapeARN pattern (e.g. a lower-cased tape id segment), with a valid GatewayARN - fixture storagegateway-tape-recovery-point-retrieval-errors, step task#2 (also task#3, task#4).
3. Observe HTTP 400 whose `message` is the generic "1 validation error detected: Value '...' at 'tapeARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." instead of Storage Gateway's InvalidGatewayRequestException message 'The following request fields contain invalid input: tapeARN.'
