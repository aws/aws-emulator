Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.put-log-events-entity-attribute-key-length-bounds-rejected logs.put-log-events-entity-attribute-value-length-bounds-rejected logs.put-log-events-entity-key-attribute-over-512-chars-rejected logs.put-log-events-unrecognised-entity-key-reports-invalid-key-attributes logs.put-log-events-entity-key-attribute-name-length-bounds-rejected logs.put-log-events-signed-by-unknown-key-rejected-and-writes-nothing logs.otlp-batch-out-of-window-records-rejected-fresh-record-stored logs.otlp-oversized-attribute-truncated-with-pointer-field logs.otlp-oversized-body-stored-truncated-with-object-reference logs.otlp-post-to-absent-log-stream-rejected-until-stream-created logs.otlp-post-without-log-stream-header-rejected logs.put-query-definition-client-token-replay-creates-one-definition logs.infrequent-access-log-group-refuses-an-anomaly-detector logs.configuration-templates-lowercase-destination-type-rejected logs.live-tail-stream-prefix-list-caps-at-one-hundred logs.put-subscription-filter-bogus-destination-rejected logs.test-metric-filter-empty-event-message-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.put-log-events-entity-attribute-key-length-bounds-rejected|run#13|AssertionError|assert one.get("rejectedEntityInfo") == unknown, (
Issue-Key: logs.put-log-events-entity-attribute-value-length-bounds-rejected|run#14|AssertionError|assert widest.get("rejectedEntityInfo") == {"errorType": "EntitySizeTooLarge"}, (
Issue-Key: logs.put-log-events-entity-key-attribute-over-512-chars-rejected|run#12|AssertionError|assert at_limit.get("rejectedEntityInfo") == {"errorType": "EntitySizeTooLarge"}, (
Issue-Key: logs.put-log-events-unrecognised-entity-key-reports-invalid-key-attributes|run#11|AssertionError|assert got == expected, f"{label}: rejectedEntityInfo was {got!r}, expected {expected!r}"
Issue-Key: logs.put-log-events-entity-key-attribute-name-length-bounds-rejected|_refused_with#15|AssertionError|raise AssertionError(f"expected PutLogEvents to be refused, it returned {reply!r}")
Issue-Key: logs.put-log-events-signed-by-unknown-key-rejected-and-writes-nothing|run#15|AssertionError|assert got == ("UnrecognizedClientException", _UNKNOWN_KEY_MESSAGE, 400), (
Issue-Key: logs.otlp-batch-out-of-window-records-rejected-fresh-record-stored|run#10|AssertionError|assert status == 200, f"mixed batch: expected HTTP 200, got {status}: {raw[:400]!r}"
Issue-Key: logs.otlp-oversized-attribute-truncated-with-pointer-field|run#9|AssertionError|assert small_status == 200, f"the OTLP endpoint answered {small_status} for a small record"
Issue-Key: logs.otlp-oversized-body-stored-truncated-with-object-reference|run#10|AssertionError|assert status == 200, f"oversized record: expected HTTP 200, got {status}: {raw[:300]!r}"
Issue-Key: logs.otlp-post-to-absent-log-stream-rejected-until-stream-created|run#7|AssertionError|assert status == 200, f"control batch: expected HTTP 200, got {status}: {raw[:300]!r}"
Issue-Key: logs.otlp-post-without-log-stream-header-rejected|run#7|AssertionError|assert status == 400, f"no stream header: expected HTTP 400, got {status}: {raw[:300]!r}"
Issue-Key: logs.put-query-definition-client-token-replay-creates-one-definition|_refusal#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: logs.infrequent-access-log-group-refuses-an-anomaly-detector|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: logs.configuration-templates-lowercase-destination-type-rejected|run#2|AssertionError|assert found, f"filtering on {wanted!r} returned no configuration templates"
Issue-Key: logs.live-tail-stream-prefix-list-caps-at-one-hundred|run#20|AssertionError|assert seen, (
Issue-Key: logs.put-subscription-filter-bogus-destination-rejected|run#27|AssertionError|assert message == (
Issue-Key: logs.test-metric-filter-empty-event-message-rejected|run#7|AssertionError|assert got == want, f"the pattern matched {got!r}, want {want!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs: 9 net-new causes — PutLogEvents entity validation never reports rejectedEntityInfo, an unknown signing key gives AccessDenied/403, the OTLP endpoint answers 403 to every signed post, a clientToken replay creates a second query definition, and five more divergences

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
Cause 1 of 9 — `PutLogEvents` entity validation never populates `rejectedEntityInfo`.
AWS: when the request's `entity` breaks a bound or names an unrecognised key, the events are still accepted but the response carries `rejectedEntityInfo: {errorType: <EntityRejectionErrorType>}`. Smithy (logs): `PutLogEventsResponse.rejectedEntityInfo.errorType` is the `EntityRejectionErrorType` enum (InvalidEntity|InvalidTypeValue|InvalidKeyAttributes|InvalidAttributes|EntitySizeTooLarge|UnsupportedLogGroupType|MissingRequiredFields).
Emulator: `rejectedEntityInfo` is `None` for an unrecognised 1-character key, for an over-long attribute value, and for a 512-character key-attribute value; it reports `InvalidAttributes` where AWS reports `InvalidKeyAttributes` for an identifier on `service`; and in one case it returns a 200 with `rejectedEntityInfo` set where AWS refuses the call outright.
Traces: logs.put-log-events-entity-attribute-key-length-bounds-rejected, logs.put-log-events-entity-attribute-value-length-bounds-rejected, logs.put-log-events-entity-key-attribute-over-512-chars-rejected, logs.put-log-events-unrecognised-entity-key-reports-invalid-key-attributes, logs.put-log-events-entity-key-attribute-name-length-bounds-rejected.

Cause 2 of 9 — a request signed by an unknown access key gets the wrong code and status.
AWS: `('UnrecognizedClientException', 'The security token included in the request is invalid.')` with HTTP 400 — both entirely fixed text. Emulator: `('AccessDenied', 'Sign the request with the emulator credentials')` with HTTP 403.
Traces: logs.put-log-events-signed-by-unknown-key-rejected-and-writes-nothing.

Cause 3 of 9 — the OTLP ingestion endpoint refuses every SigV4-signed post with HTTP 403.
AWS: the OTLP `/v1/logs` endpoint accepts a signed post and answers HTTP 200 (and HTTP 400 for a post with no log-stream header), applying the documented out-of-window, oversized-attribute and oversized-body handling. Emulator: every post is answered HTTP 403 with the body `\x08\x07\x12>Authentication failed: Please make sure your API Key is valid.` — an API-key authentication path that does not belong to a SigV4-signed AWS endpoint, so none of the OTLP behaviour can be exercised.
Traces: logs.otlp-batch-out-of-window-records-rejected-fresh-record-stored, logs.otlp-oversized-attribute-truncated-with-pointer-field, logs.otlp-oversized-body-stored-truncated-with-object-reference, logs.otlp-post-to-absent-log-stream-rejected-until-stream-created, logs.otlp-post-without-log-stream-header-rejected.

Cause 4 of 9 — `PutQueryDefinition` `clientToken` is not honoured for idempotency.
AWS: replaying the same `clientToken` returns the same `queryDefinitionId` and leaves exactly one definition. Emulator: the replay is accepted and a second definition is created with a new id.
Traces: logs.put-query-definition-client-token-replay-creates-one-definition.

Cause 5 of 9 — an `INFREQUENT_ACCESS` log group accepts an anomaly detector.
AWS: the Infrequent Access log class does not support log anomaly detection; `CreateLogAnomalyDetector` over such a group is refused with a 400 error. Emulator: it succeeds and returns `arn:aws:logs:<region>:<account>:anomaly-detector:<uuid>`.
Traces: logs.infrequent-access-log-group-refuses-an-anomaly-detector.

Cause 6 of 9 — `DescribeConfigurationTemplates` returns nothing when filtered by `deliveryDestinationTypes=['XRAY']`.
AWS: the filter is matched against the modelled destination types and returns the XRAY templates. Smithy (logs): `DeliveryDestinationType` enum includes `XRAY`. Emulator: the filtered call returns no configuration templates.
Traces: logs.configuration-templates-lowercase-destination-type-rejected.

Cause 7 of 9 — `StartLiveTail` with a 100-entry `logStreamNamePrefixes` list delivers nothing.
AWS: up to 100 stream-name prefixes are accepted and events from a stream matching any of them (including the 100th) are delivered on the session. Emulator: the session delivers `[]`. Note the open issue 20261114T215119Z-logs-startlivetail-opens-a-session-but-never-streams.md covers other live-tail checks but does not carry this one's replay key; the 100-prefix cap is a distinct selection path.
Traces: logs.live-tail-stream-prefix-list-caps-at-one-hundred.

Cause 8 of 9 — the `PutSubscriptionFilter` refusal for a bogus destination carries the wrong message.
AWS refuses a bogus `destinationArn` with its own message for a malformed/unknown destination; the emulator answers the *delivery-probe* sentence `Could not deliver test message to specified destination. Check if the destination is valid.` instead, conflating a destination that cannot be addressed at all with one that could not be written to.
Traces: logs.put-subscription-filter-bogus-destination-rejected.

Cause 9 of 9 — `TestMetricFilter` tokenises a space-delimited pattern incorrectly.
AWS: with a space-delimited pattern the LAST named token absorbs the remainder of the line, so `PUT 503 extra` yields `{'$verb': 'PUT', '$code': '503 extra'}`. Emulator: it splits on every space and yields `{'$verb': 'PUT', '$code': '503'}`, dropping `extra`.
Traces: logs.test-metric-filter-empty-event-message-rejected.

Why: cause 1 removes the only signal that an entity was rejected, so a client believes its service/resource attribution was accepted when it was dropped. Cause 2 makes a credential problem look like an authorization problem at a different status, which changes an SDK's retry classification. Cause 3 blocks the entire OTLP ingestion path — no OpenTelemetry client can write at all, and the refusal text tells the client to check an API key that AWS does not use. Cause 4 breaks at-least-once callers, leaving duplicate saved queries. Causes 5-9 each let a configuration or a result differ silently from AWS.

Fix: the PutLogEvents entity validator (populate and classify `rejectedEntityInfo`), the request-authentication layer's unknown-access-key classification, the OTLP `/v1/logs` endpoint's authentication (accept SigV4 as every other logs endpoint does), the PutQueryDefinition clientToken idempotency store, the log-class feature matrix for anomaly detectors, the DescribeConfigurationTemplates destination-type filter, the StartLiveTail stream-prefix selection, the PutSubscriptionFilter destination-resolution error, and the TestMetricFilter space-delimited tokeniser.

## Evidence
Cause 1: "a 1-character unrecognised key gave rejectedEntityInfo None"; "a 512-character Host value passes the length bound but the entity is too large; rejectedEntityInfo was None"; "a 512-character key attribute value answered None"; "identifier-on-service: rejectedEntityInfo was {'errorType': 'InvalidAttributes'}, expected {'errorType': 'InvalidKeyAttributes'}"; "expected PutLogEvents to be refused, it returned {'nextSequenceToken': ..., 'rejectedEntityInfo': {'errorType': 'InvalidKeyAttributes'}, ... 'HTTPStatusCode': 200}".
Cause 2: "a write signed by an unknown key answered ('AccessDenied', 'Sign the request with the emulator credentials', 403), expected ('UnrecognizedClientException', 'The security token included in the request is invalid.', 400)".
Cause 3: "mixed batch: expected HTTP 200, got 403: b'\\x08\\x07\\x12>Authentication failed: Please make sure your API Key is valid.'" (and the same for the oversized-record, control-batch and absent-stream cases); "no stream header: expected HTTP 400, got 403"; "the OTLP endpoint answered 403 for a small record".
Cause 4: "expected the call to be refused, but it succeeded: '00000000-0000-4000-8000-000000000002'".
Cause 5: "expected the call to be refused, but it succeeded: 'arn:aws:logs:us-west-2:000000000000:anomaly-detector:00000000-0000-4000-8000-000000000002'".
Cause 6: "filtering on 'XRAY' returned no configuration templates".
Cause 7: "no event from the stream matching the 100th prefix arrived in 2 attempts; the session delivered []".
Cause 8: "unexpected message: 'Could not deliver test message to specified destination. Check if the destination is valid.'".
Cause 9: "the pattern matched {2: ('GET 200', {'$verb': 'GET', '$code': '200'}), 3: ('PUT 503 extra', {'$code': '503', '$verb': 'PUT'})}, want {... 3: ('PUT 503 extra', {'$verb': 'PUT', '$code': '503 extra'})}".

## Steps to Reproduce
1. PutLogEvents with an entity carrying an unrecognised key, an over-long attribute value, and a 512-character key-attribute value — rejectedEntityInfo is None / wrongly classified.
2. Sign a PutLogEvents request with an unknown access key — AccessDenied/403 instead of UnrecognizedClientException/400.
3. POST a SigV4-signed OTLP protobuf payload to the logs OTLP /v1/logs endpoint — HTTP 403 with the API-key message.
4. Call PutQueryDefinition twice with the same clientToken — two definitions.
5. CreateLogGroup with logGroupClass=INFREQUENT_ACCESS and CreateLogAnomalyDetector on it — accepted.
6. DescribeConfigurationTemplates with deliveryDestinationTypes=['XRAY'] — empty.
7. StartLiveTail with 100 logStreamNamePrefixes and write to a stream matching the 100th — no events.
8. PutSubscriptionFilter with a bogus destinationArn — the delivery-probe message.
9. TestMetricFilter with a space-delimited pattern over 'PUT 503 extra' — the last token does not absorb the remainder.
