Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__logs.kinesis-subscription-filter-rejected-until-role-may-put-records kinesis__logs.put-destination-rejected-until-role-may-put-records kinesis__logs.space-delimited-filter-ships-gzip-records-with-extracted-fields
Issue-Key: kinesis__logs.kinesis-subscription-filter-rejected-until-role-may-put-records|_refusal#20|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: kinesis__logs.put-destination-rejected-until-role-may-put-records|_refusal#15|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: kinesis__logs.space-delimited-filter-ships-gzip-records-with-extracted-fields|run#40|AssertionError|assert event.get("extractedFields") == expected, (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs: 3 net-new causes — PutSubscriptionFilter and PutDestination accept a role that may not PutRecord, and space-delimited filter deliveries carry no extractedFields

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestination

## Description
Cause 1 of 3 — `PutSubscriptionFilter` does not perform the destination test delivery.
AWS: PutSubscriptionFilter assumes `roleArn` and writes a test record to the Kinesis stream; if the role lacks `kinesis:PutRecord` on that stream the call is refused with HTTP 400 `InvalidParameterException` / `Could not deliver test message to specified destination. Check if the destination is valid.` (all fixed text). Smithy (logs): `PutSubscriptionFilter` models `InvalidParameterException`.
Emulator: HTTP 200 with an empty body; the filter is created.
Traces: kinesis__logs.kinesis-subscription-filter-rejected-until-role-may-put-records.

Cause 2 of 3 — `PutDestination` does not perform the destination test delivery.
AWS: same probe, same refusal, before any destination is stored.
Emulator: HTTP 200 and the destination is stored — `{'destination': {'destinationName': '<name>', 'targetArn': 'arn:aws:kinesis:<region>:<account>:stream/<stream>', 'roleArn': 'arn:aws:iam::<account>:role/<role>', 'arn': 'arn:aws:logs:<region>:<account>:destination:<name>', 'creationTime': <ms>}}`.
Traces: kinesis__logs.put-destination-rejected-until-role-may-put-records.

Cause 3 of 3 — a space-delimited filter pattern delivers records without `extractedFields`.
AWS: when a subscription filter's pattern is space-delimited with named tokens (e.g. `[client, verb, path, status, size]`), each delivered `logEvents` entry in the gzip-encoded CloudWatch Logs record carries an `extractedFields` map of token name to the captured substring. Recorded behavior only (the subscription record format is documented, not modelled).
Emulator: the delivered event for `192.0.2.11 GET /missing.png 404 0` carries `extractedFields` = `None`, where the expected map is `{'client': '192.0.2.11', 'verb': 'GET', 'path': '/missing.png', 'status': '404', 'size': '0'}` — the keys are the pattern's token names and the values are the substrings of the event message.
Traces: kinesis__logs.space-delimited-filter-ships-gzip-records-with-extracted-fields.

Why: causes 1 and 2 let a client wire a log feed to a destination it cannot write to, with no error at configuration time — the failure appears only as missing data at the stream. Cause 3 breaks every consumer that reads the parsed tokens out of the subscription record instead of re-parsing the raw message; the field is absent rather than empty, so consumers see a schema change, not a value.

Fix: the PutSubscriptionFilter and PutDestination handlers' validation (assume the role and perform the test delivery before storing), and the subscription-delivery record serializer (populate `extractedFields` from the filter pattern's named tokens).

## Evidence
Cause 1: "expected a refusal, but the call succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200, 'content-length': '2' ...}}".
Cause 2: "expected a refusal, but the call succeeded: {'destination': {'destinationName': 'sim2real-xsvc-cwld-7de15d8d13ca', 'targetArn': '...stream/sim2real-xsvc-cwld-36b0aa324377', 'roleArn': '...role/sim2real-xsvc-cwld-c8462d931257', ...}}".
Cause 3: "event '192.0.2.11 GET /missing.png 404 0' carried extractedFields None, expected {'client': '192.0.2.11', 'verb': 'GET', 'path': '/missing.png', 'status': '404', 'size': '0'}".

## Steps to Reproduce
1. Create a Kinesis stream and a logs-trusted role without kinesis:PutRecord; call logs:PutSubscriptionFilter and logs:PutDestination against that stream with that role — both return 200.
2. Create a subscription filter with a space-delimited pattern '[client, verb, path, status, size]', write a matching event, read the delivered gzip record from the stream — logEvents entries have no extractedFields.
