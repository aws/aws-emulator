Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.grok-data-sixth-pattern-rejected logs.grok-host-sixth-pattern-rejected logs.grok-hostport-sixth-use-rejected-transformer-kept logs.grok-path-five-captures-mix-url-and-windows-sixth-rejected logs.grok-space-sixth-use-rejected-on-untransformed-group logs.grok-winpath-five-captures-extract-on-ingest-sixth-rejected logs.grok-iporhost-parses-ipv6-and-host-names-on-ingest logs.grok-base16num-twenty-signed-hex-captures-twenty-first-rejected logs.grok-date-us-twenty-captures-both-separators-twenty-first-rejected logs.grok-posint-twenty-first-pattern-rejected logs.grok-timestamp-iso8601-twenty-first-pattern-rejected logs.grok-uuid-twenty-first-pattern-rejected logs.grok-windowsmac-twenty-captures-transform-ingested-event logs.grok-unixpath-twenty-query-paths-extract-twenty-first-rejected logs.grok-undefined-pattern-name-rejected-while-notspace-applies logs.grok-loglevel-names-each-severity-spelling-on-ingest logs.grok-monthnum-on-parsed-field-accepts-one-to-twelve logs.grok-syslogtimestamp-extracts-month-name-dates-on-ingest logs.grok-date-eu-captures-slash-dot-and-dash-dates-on-ingest logs.grok-monthday-keeps-leading-zero-on-ingest logs.grok-urihost-captures-host-and-separate-port-on-ingest logs.grok-uripath-captures-whole-path-and-skips-path-with-query-on-ingest logs.move-keys-on-ingest-overwrite-flag-decides-existing-key logs.transformer-processor-chain-round-trips-and-second-put-replaces-it
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.grok-data-sixth-pattern-rejected|_expect_error#14|AssertionError|raise AssertionError(f"the request should have been refused, but returned {reply!r}")
Issue-Key: logs.grok-host-sixth-pattern-rejected|_code_and_message#14|AssertionError|raise AssertionError(f"expected a refusal, got a success: {answer!r}")
Issue-Key: logs.grok-hostport-sixth-use-rejected-transformer-kept|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call was accepted: {answer!r}")
Issue-Key: logs.grok-path-five-captures-mix-url-and-windows-sixth-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: logs.grok-space-sixth-use-rejected-on-untransformed-group|_error_of#7|AssertionError|raise AssertionError(f"the call was expected to fail but returned {answer!r}")
Issue-Key: logs.grok-winpath-five-captures-extract-on-ingest-sixth-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: logs.grok-iporhost-parses-ipv6-and-host-names-on-ingest|_refusal#8|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: logs.grok-base16num-twenty-signed-hex-captures-twenty-first-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: logs.grok-date-us-twenty-captures-both-separators-twenty-first-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: logs.grok-posint-twenty-first-pattern-rejected|_refused#13|AssertionError|raise AssertionError(f"the call was accepted but should have been refused: {got!r}")
Issue-Key: logs.grok-timestamp-iso8601-twenty-first-pattern-rejected|_rejection#13|AssertionError|raise AssertionError(f"the request was not refused: {body!r}")
Issue-Key: logs.grok-uuid-twenty-first-pattern-rejected|_refusal_of#13|AssertionError|raise AssertionError(f"expected an error response, received {response!r}")
Issue-Key: logs.grok-windowsmac-twenty-captures-transform-ingested-event|_code_and_message#11|AssertionError|raise AssertionError(f"a refusal was expected, the call returned {reply!r}")
Issue-Key: logs.grok-unixpath-twenty-query-paths-extract-twenty-first-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: logs.grok-undefined-pattern-name-rejected-while-notspace-applies|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: logs.grok-loglevel-names-each-severity-spelling-on-ingest|run#21|AssertionError|assert row.get("severity") == word, (
Issue-Key: logs.grok-monthnum-on-parsed-field-accepts-one-to-twelve|run#21|AssertionError|assert row.get("month") == month, (
Issue-Key: logs.grok-syslogtimestamp-extracts-month-name-dates-on-ingest|run#16|AssertionError|assert captured == _STAMPS + [None], (
Issue-Key: logs.grok-date-eu-captures-slash-dot-and-dash-dates-on-ingest|_wait_for#15|Timeout|time.sleep(every_s)
Issue-Key: logs.grok-monthday-keeps-leading-zero-on-ingest|_wait_for#15|Timeout|time.sleep(every_s)
Issue-Key: logs.grok-urihost-captures-host-and-separate-port-on-ingest|_wait_for#15|Timeout|time.sleep(every_s)
Issue-Key: logs.grok-uripath-captures-whole-path-and-skips-path-with-query-on-ingest|_wait_for#15|Timeout|time.sleep(every_s)
Issue-Key: logs.move-keys-on-ingest-overwrite-flag-decides-existing-key|run#17|AssertionError|assert transformed.get("meta.level") == "fresh", (
Issue-Key: logs.transformer-processor-chain-round-trips-and-second-put-replaces-it|run#8|AssertionError|assert first.get("logGroupIdentifier") == group, (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs transformers: 6 net-new causes — grok per-pattern use caps and undefined pattern names are not enforced, seven grok patterns extract nothing on ingest, moveKeys ignores overwriteIfExists, and GetTransformer returns an ARN where the name was given

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
Cause 1 of 6 — the five-use cap on grok patterns is not enforced.
AWS: a grok `matchPattern` may use each of the high-cardinality patterns at most five times; a sixth use is refused by `PutTransformer` with an HTTP 400 `InvalidParameterException` naming the pattern and its cap, and the previously stored transformer is left untouched. Recorded behavior only (the per-pattern caps are documented, not modelled); Smithy (logs): `PutTransformer` models `InvalidParameterException`.
Emulator: the sixth use is accepted — HTTP 200 with an empty body (`content-length: '2'`). Affected patterns in this suite: DATA, HOST, HOSTPORT, PATH, SPACE, WINPATH, IPORHOST.
Traces: logs.grok-data-sixth-pattern-rejected, logs.grok-host-sixth-pattern-rejected, logs.grok-hostport-sixth-use-rejected-transformer-kept, logs.grok-path-five-captures-mix-url-and-windows-sixth-rejected, logs.grok-space-sixth-use-rejected-on-untransformed-group, logs.grok-winpath-five-captures-extract-on-ingest-sixth-rejected, logs.grok-iporhost-parses-ipv6-and-host-names-on-ingest.

Cause 2 of 6 — the twenty-use cap on grok patterns is not enforced.
AWS: the twenty-one-th use of a twenty-cap pattern is refused with the same 400 InvalidParameterException.
Emulator: accepted, HTTP 200. Affected patterns: BASE16NUM, DATE_US, POSINT, TIMESTAMP_ISO8601, UUID, WINDOWSMAC, UNIXPATH.
Traces: logs.grok-base16num-twenty-signed-hex-captures-twenty-first-rejected, logs.grok-date-us-twenty-captures-both-separators-twenty-first-rejected, logs.grok-posint-twenty-first-pattern-rejected, logs.grok-timestamp-iso8601-twenty-first-pattern-rejected, logs.grok-uuid-twenty-first-pattern-rejected, logs.grok-windowsmac-twenty-captures-transform-ingested-event, logs.grok-unixpath-twenty-query-paths-extract-twenty-first-rejected.

Cause 3 of 6 — an undefined grok pattern name is accepted.
AWS: `%{NOT_A_PATTERN:field}` is refused with a 400 InvalidParameterException naming the unknown pattern, while a defined one (NOTSPACE) in the same chain applies normally.
Emulator: accepted, HTTP 200.
Traces: logs.grok-undefined-pattern-name-rejected-while-notspace-applies.

Cause 4 of 6 — grok patterns extract no fields on ingest.
AWS: an event ingested into a log group with a grok transformer gets the captured fields added to the record, so an Insights query over the group returns them. Emulator: the ingested row carries only `@message`/`@log`/`@ptr` — no captured field at all (and in four cases the check never settles, ending `Check timed out after 60.0s`). Affected patterns: LOGLEVEL, MONTHNUM, SYSLOGTIMESTAMP, DATE_EU, MONTHDAY, URIHOST, URIPATH.
Traces: logs.grok-loglevel-names-each-severity-spelling-on-ingest, logs.grok-monthnum-on-parsed-field-accepts-one-to-twelve, logs.grok-syslogtimestamp-extracts-month-name-dates-on-ingest, logs.grok-date-eu-captures-slash-dot-and-dash-dates-on-ingest, logs.grok-monthday-keeps-leading-zero-on-ingest, logs.grok-urihost-captures-host-and-separate-port-on-ingest, logs.grok-uripath-captures-whole-path-and-skips-path-with-query-on-ingest.

Cause 5 of 6 — `moveKeys` ignores `overwriteIfExists`.
AWS: with `overwriteIfExists: true` the moved value replaces the target key's existing value; with `false` the existing value is kept. Emulator: with `true` the target keeps its existing value — the ingested record shows `site.region` still `'kept'` and the incoming value left in its source key, so the flag has no effect.
Traces: logs.move-keys-on-ingest-overwrite-flag-decides-existing-key.

Cause 6 of 6 — `GetTransformer` echoes an ARN where the request gave a name.
AWS: `logGroupIdentifier` is returned as it was supplied — a log group *name* when a name was given (an ARN only when an ARN was given). Smithy (logs): `GetTransformerResponse.logGroupIdentifier` is the same `LogGroupIdentifier` string the request carried.
Emulator: the response reads back `arn:aws:logs:<region>:<account>:log-group:<name>` for a request made with the bare name.
Traces: logs.transformer-processor-chain-round-trips-and-second-put-replaces-it.

Why: causes 1-3 let a client store a transformer AWS would reject, so the configuration works in the emulator and fails in production — and the 'transformer kept' guarantee on a refusal cannot be tested. Cause 4 is the feature itself: with no extracted fields, every query, metric filter and index built on a transformed field returns nothing. Cause 5 silently produces different records from AWS for the same configuration. Cause 6 breaks round-trip/idempotency code that compares what it sent with what it reads back.

Fix: the PutTransformer handler's grok validation (per-pattern use caps and the known-pattern set), the ingest-time transformer evaluator (grok capture emission and the moveKeys overwrite flag), and the GetTransformer response serializer (echo the supplied logGroupIdentifier).

## Evidence
Caps/undefined pattern: "the call was expected to be refused but succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200, 'content-length': '2' ...}}" (and the same shape worded as "expected a refusal, got a success" / "the call was accepted but should have been refused" / "a refusal was expected, the call returned ...") for DATA, HOST, HOSTPORT, PATH, SPACE, WINPATH, IPORHOST, BASE16NUM, DATE_US, POSINT, TIMESTAMP_ISO8601, UUID, WINDOWSMAC, UNIXPATH and the undefined pattern name.
No extraction: "LOGLEVEL should capture 'Warning' from 'Warning <id> 0'; the row was {'@message': 'Warning <id> 0', '@log': '<account>:<group>', '@ptr': '...'}"; "MONTHNUM should capture '5' exactly as written; the row was {...}"; "dateTime per event, oldest first, was [None, None, None]; expected ['Nov 29 14:30:00', 'January 5 9:07:3', None]"; four others end "Check timed out after 60.0s".
moveKeys: "overwriteIfExists true did not replace the target's existing key: {'@message': '{\"marker\": \"round-1\", \"site\": {\"region\": \"kept\"}, \"region\": \"incoming\", ...}', 'site.region': 'kept', 'region': 'incoming', ...}".
GetTransformer: "logGroupIdentifier read back as 'arn:aws:logs:us-west-2:000000000000:log-group:sim2real-xchain-73e2082a0a4c', expected the group name".

## Steps to Reproduce
1. Call logs:PutTransformer with a grok matchPattern using %{DATA} six times (and similarly for the other capped patterns, and %{POSINT} twenty-one times) — HTTP 200 instead of InvalidParameterException.
2. Call PutTransformer with an undefined pattern name — HTTP 200.
3. Put a grok transformer using %{LOGLEVEL:lvl} (and MONTHNUM, SYSLOGTIMESTAMP, DATE_EU, MONTHDAY, URIHOST, URIPATH), ingest a matching event, and query the group — no captured field appears.
4. Put a moveKeys processor with overwriteIfExists: true onto an existing target key, ingest a record — the target keeps its old value.
5. Call PutTransformer/GetTransformer with logGroupIdentifier set to the log group NAME — the response echoes an ARN.
