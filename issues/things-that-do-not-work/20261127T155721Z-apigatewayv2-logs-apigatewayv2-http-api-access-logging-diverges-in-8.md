Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__logs.http-api-access-log-carries-the-lambda-integration-status-and-latency apigatewayv2__logs.http-api-access-log-data-processed-is-a-byte-count apigatewayv2__logs.http-api-access-log-protocol-is-the-request-protocol apigatewayv2__logs.http-api-access-log-resolves-lambda-authorizer-context-keys apigatewayv2__logs.access-log-format-change-reshapes-later-records-only apigatewayv2__logs.update-stage-access-log-destination-named-without-an-arn-is-rejected apigatewayv2__logs.update-stage-access-log-format-without-a-request-id-is-rejected apigatewayv2__logs.update-stage-access-log-to-a-log-group-that-does-not-exist-is-rejected
Issue-Key: apigatewayv2__logs.http-api-access-log-carries-the-lambda-integration-status-and-latency|run#10|AssertionError|assert record.get("integrationStatus") == "200", (
Issue-Key: apigatewayv2__logs.http-api-access-log-data-processed-is-a-byte-count|run#11|AssertionError|assert str(record.get("dataProcessed")).isdigit() and int(record["dataProcessed"]) > 0, (
Issue-Key: apigatewayv2__logs.http-api-access-log-protocol-is-the-request-protocol|run#10|AssertionError|assert record.get("protocol") == "HTTP/1.1", (
Issue-Key: apigatewayv2__logs.http-api-access-log-resolves-lambda-authorizer-context-keys|run#18|AssertionError|assert (record.get("tenant"), record.get("n")) == ("t1", "7"), (
Issue-Key: apigatewayv2__logs.access-log-format-change-reshapes-later-records-only|run#17|AssertionError|assert _REQUEST_ID.fullmatch(record["rid"]), (
Issue-Key: apigatewayv2__logs.update-stage-access-log-destination-named-without-an-arn-is-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected UpdateStage to be refused, but it returned {result!r}")
Issue-Key: apigatewayv2__logs.update-stage-access-log-format-without-a-request-id-is-rejected|run#16|AssertionError|assert (code, message) == ("BadRequestException", want), (
Issue-Key: apigatewayv2__logs.update-stage-access-log-to-a-log-group-that-does-not-exist-is-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected UpdateStage to be refused, but it returned {result!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# apigatewayv2: HTTP API access logging diverges in 8 ways — $context.integrationStatus/dataProcessed/protocol/authorizer.* all render '-', requestId is not an HTTP-API request id, and UpdateStage skips three AccessLogSettings validations

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Eight net-new causes in HTTP API access logging. Causes 1-5 are in the access-log record writer, causes 6-8 in UpdateStage's AccessLogSettings validation.

Cause 1 of 8 — $context.integrationStatus is not resolved.
AWS: for a Lambda-proxy integration `$context.integrationStatus` is the status code the integration returned (200 for a successful Lambda invocation). Recorded behavior only (the variable is documented in the HTTP API logging variable list, not in the Smithy model).
Emulator: the written record carries the literal `-` (the unresolved-variable placeholder).
Traces: apigatewayv2__logs.http-api-access-log-carries-the-lambda-integration-status-and-latency.

Cause 2 of 8 — $context.dataProcessed is not resolved.
AWS: `$context.dataProcessed` is a byte count for the request.
Emulator: `-`.
Traces: apigatewayv2__logs.http-api-access-log-data-processed-is-a-byte-count.

Cause 3 of 8 — $context.protocol is not resolved.
AWS: `$context.protocol` is the request protocol, e.g. `HTTP/1.1`.
Emulator: `-`.
Traces: apigatewayv2__logs.http-api-access-log-protocol-is-the-request-protocol.

Cause 4 of 8 — $context.authorizer.<key> is not resolved.
AWS: a Lambda authorizer's returned `context` map is addressable as `$context.authorizer.<key>` and each key resolves to the value the authorizer returned for that request.
Emulator: every `$context.authorizer.<key>` renders `-`.
Traces: apigatewayv2__logs.http-api-access-log-resolves-lambda-authorizer-context-keys.

Cause 5 of 8 — $context.requestId is not an HTTP API request id.
AWS: an HTTP API assigns a request id of fifteen URL-safe base64 characters followed by one `=` (e.g. `<15 base64url chars>=`). The shape is the requirement; the characters themselves are per-request.
Emulator: the record carries a 16-character lowercase hex id (e.g. `18ebf0638fcfdd6f`), which cannot match that grammar.
Traces: apigatewayv2__logs.access-log-format-change-reshapes-later-records-only.

Cause 6 of 8 — UpdateStage accepts an AccessLogSettings.DestinationArn that is not an ARN.
AWS: `DestinationArn` must be a CloudWatch Logs log-group ARN; a bare log-group name is refused with HTTP 400 `BadRequestException`. Smithy: `UpdateStage` models `BadRequestException`; `AccessLogSettings.DestinationArn` is a string member whose ARN requirement is documented behavior.
Emulator: HTTP 200, and the stage stores `AccessLogSettings.DestinationArn = '/sim2real/xsvc/apigw-dest-<id>'` (a plain name).
Traces: apigatewayv2__logs.update-stage-access-log-destination-named-without-an-arn-is-rejected.

Cause 7 of 8 — the refusal message for a Format with no request id is re-worded.
AWS: HTTP 400 `BadRequestException` with the message `The following errors are found in Access Log Format: Access Log format must include either $context.requestId or $context.extendedRequestId : '<the format string from the request>'`. Fixed text: everything up to and including `either $context.requestId or $context.extendedRequestId : `; the single-quoted format string is the value from the request.
Emulator: the code is right but the message is `Invalid value for AccessLogSettings: the Format must include at least $context.requestId, got '<format>'.` — different fixed text, and it names only `$context.requestId` where AWS also accepts `$context.extendedRequestId`.
Traces: apigatewayv2__logs.update-stage-access-log-format-without-a-request-id-is-rejected.

Cause 8 of 8 — UpdateStage accepts a destination log group that does not exist.
AWS: enabling access logging against a log group that does not exist is refused with HTTP 400 `BadRequestException`.
Emulator: HTTP 200 and the setting is stored against the absent group.
Traces: apigatewayv2__logs.update-stage-access-log-to-a-log-group-that-does-not-exist-is-rejected.

Why: every $context variable a client asks for is a field in its log pipeline. Records with `-` in integrationStatus, dataProcessed, protocol and authorizer keys make latency/size/authorization dashboards and alarms read as missing data rather than as values, and a request id outside the HTTP API grammar breaks correlation with any other record keyed on it. The three missing UpdateStage validations let a stage be configured so that it can never deliver, with no error at configuration time.

Fix: the HTTP API access-log record writer (the $context variable resolution table and the request-id generator) for causes 1-5, and the UpdateStage handler's AccessLogSettings validation for causes 6-8.

## Evidence
Cause 1: "$context.integrationStatus must be Lambda's 200, got '-'".
Cause 2: "$context.dataProcessed must be a byte count, got '-'".
Cause 3: "$context.protocol must be HTTP/1.1, got '-'".
Cause 4: "$context.authorizer.tenant/n must be 't1'/'7', got ('-', '-')".
Cause 5: "a record under the first format has no request id shaped like one: {'rid': '18ebf0638fcfdd6f', 'path': '/before-...', 'code': '403'}".
Cause 6: "expected UpdateStage to be refused, but it returned ... 'AccessLogSettings': {'DestinationArn': '/sim2real/xsvc/apigw-dest-5a88dca164d2', ...}" with HTTPStatusCode 200.
Cause 7: got ('BadRequestException', "Invalid value for AccessLogSettings: the Format must include at least $context.requestId, got '{\"path\": \"$context.path\", \"code\": \"$context.status\"}'."); expected ('BadRequestException', "The following errors are found in Access Log Format: Access Log format must include either $context.requestId or $context.extendedRequestId : '<format>'").
Cause 8: "expected UpdateStage to be refused, but it returned ..." with HTTPStatusCode 200 and DestinationArn naming a log group that was never created.

## Steps to Reproduce
1. Create an HTTP API with a Lambda-proxy integration and a Lambda authorizer that returns a context map; enable access logging on $default with a Format naming $context.requestId, $context.protocol, $context.integrationStatus, $context.dataProcessed and $context.authorizer.<key>.
2. Send a request, then read the access-log record from the log group: integrationStatus/dataProcessed/protocol/authorizer.* are '-' and requestId is 16 hex characters.
3. Call UpdateStage with AccessLogSettings.DestinationArn set to a bare log-group name — 200 instead of BadRequestException.
4. Call UpdateStage with a Format that omits $context.requestId — BadRequestException with a re-worded message.
5. Call UpdateStage with a DestinationArn naming a log group that does not exist — 200 instead of BadRequestException.
