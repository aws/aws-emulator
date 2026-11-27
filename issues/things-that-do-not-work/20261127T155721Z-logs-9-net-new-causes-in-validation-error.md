Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.describe-log-groups-thirteen-digit-account-identifier-rejected logs.list-log-groups-field-index-name-over-512-chars-rejected logs.configuration-templates-resource-type-length-bound-rejected logs.untag-resource-key-outside-pattern-rejected logs.untag-log-group-invalid-key-character-rejected logs.put-transformer-empty-delete-key-rejected logs.create-delivery-record-fields-over-bound-or-duplicated-refused logs.describe-metric-filters-metric-name-with-dollar-sign-rejected logs.describe-configuration-templates-dotted-service-name-rejected logs.list-tags-for-resource-wildcard-suffixed-arn-rejected logs.aggregate-summaries-pattern-below-min-length-rejected logs.anomaly-detector-empty-name-rejected logs.list-log-groups-name-pattern-term-length-bounds-rejected logs.delete-data-protection-policy-identifier-outside-pattern-rejected logs.delete-data-protection-policy-overlong-identifier-rejected-masking-kept logs.describe-queries-status-outside-enum-rejected logs.describe-log-groups-name-prefix-length-bounds-rejected logs.start-live-tail-stream-name-prefix-over-512-characters-rejected logs.get-log-object-pointer-length-bound-rejected logs.put-query-definition-client-token-outside-36-to-128-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.describe-log-groups-thirteen-digit-account-identifier-rejected|run#11|AssertionError|assert (code, message) == (
Issue-Key: logs.list-log-groups-field-index-name-over-512-chars-rejected|run#15|AssertionError|assert got == want, f"a 513-character field index name answered {got!r}, expected {want!r}"
Issue-Key: logs.configuration-templates-resource-type-length-bound-rejected|run#9|AssertionError|assert got == (
Issue-Key: logs.untag-resource-key-outside-pattern-rejected|run#14|AssertionError|assert got == (
Issue-Key: logs.untag-log-group-invalid-key-character-rejected|run#8|AssertionError|assert (code, message) == (
Issue-Key: logs.put-transformer-empty-delete-key-rejected|run#9|AssertionError|assert got == (
Issue-Key: logs.create-delivery-record-fields-over-bound-or-duplicated-refused|_refusal#22|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: logs.describe-metric-filters-metric-name-with-dollar-sign-rejected|run#14|AssertionError|assert got == want, f"metricName {metric + '$'!r} answered {got!r}, expected {want!r}"
Issue-Key: logs.describe-configuration-templates-dotted-service-name-rejected|run#13|AssertionError|assert got == ("ValidationException", _PATTERN_MESSAGE), (
Issue-Key: logs.list-tags-for-resource-wildcard-suffixed-arn-rejected|run#16|AssertionError|assert (code, message) == ("ValidationException", "Invalid resourceArn"), (
Issue-Key: logs.aggregate-summaries-pattern-below-min-length-rejected|run#18|AssertionError|assert msg == expected, f"2-character pattern message was {msg!r}"
Issue-Key: logs.anomaly-detector-empty-name-rejected|run#12|AssertionError|assert message == expected, f"refusal message, clauses sorted, was {message!r}"
Issue-Key: logs.list-log-groups-name-pattern-term-length-bounds-rejected|run#21|AssertionError|assert (code, _clauses(message)) == (want[0], _clauses(want[1])), (
Issue-Key: logs.delete-data-protection-policy-identifier-outside-pattern-rejected|run#12|AssertionError|assert got == want, f"identifier with '!' gave {got!r}, expected {want!r}"
Issue-Key: logs.delete-data-protection-policy-overlong-identifier-rejected-masking-kept|run#20|AssertionError|assert got == expected, f"a 2049-character name identifier was refused with {got!r}"
Issue-Key: logs.describe-queries-status-outside-enum-rejected|run#16|AssertionError|assert (code, msg) == ("InvalidParameterException", expected), (
Issue-Key: logs.describe-log-groups-name-prefix-length-bounds-rejected|run#14|AssertionError|assert got == want, f"a 513-character prefix gave {got!r}, expected {want!r}"
Issue-Key: logs.start-live-tail-stream-name-prefix-over-512-characters-rejected|run#15|AssertionError|assert message.startswith(head) and message.endswith(tail), (
Issue-Key: logs.get-log-object-pointer-length-bound-rejected|run#10|AssertionError|assert got == want, f"a 513-character pointer gave {got!r}, expected {want!r}"
Issue-Key: logs.put-query-definition-client-token-outside-36-to-128-rejected|run#6|AssertionError|assert short == (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs: 9 net-new causes in validation-error fidelity — list-member violations are reported per element, regex patterns are echoed anchored, only one of several violated constraints is reported, and four operations use the wrong code or message template

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
Nine distinct defects in how CloudWatch Logs renders validation refusals. The HTTP status, the error code and the message are all part of the contract, so each of these is a divergence a client can see.

Cause 1 of 9 — a violation on a *list member* is reported per element instead of as AWS's aggregated clause on the list.
AWS: when an element of a list member violates its constraints, the clause names the LIST member and carries every constraint in a bracketed list, e.g. `1 validation error detected: Value at 'accountIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: ^\d{12}$]`. Fixed text: the sentence template and the bracketed constraint list (taken from the model's constraints on the member); variable: the member path.
Emulator: it reports one clause per element with an indexed path, e.g. `2 validation errors detected: Value at 'accountIdentifiers.1.member' failed to satisfy constraint: Member must have length less than or equal to 12; Value at 'accountIdentifiers.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$`. The same shape appears for `fieldIndexNames.1.member`, `resourceTypes.1.member`, `tagKeys.2.member`, `tags.2.member`, `transformerConfig.2.member.deleteKeys.withKeys.1.member` and `recordFields`.
Traces: logs.describe-log-groups-thirteen-digit-account-identifier-rejected, logs.list-log-groups-field-index-name-over-512-chars-rejected, logs.configuration-templates-resource-type-length-bound-rejected, logs.untag-resource-key-outside-pattern-rejected, logs.untag-log-group-invalid-key-character-rejected, logs.put-transformer-empty-delete-key-rejected, logs.create-delivery-record-fields-over-bound-or-duplicated-refused.

Cause 2 of 9 — the regex in the message is echoed with added `^`/`$` anchors.
AWS: the pattern is reproduced exactly as the model declares it, unanchored, e.g. `Member must satisfy regular expression pattern: [^:*$]*` and `... pattern: [\w_-]*`.
Emulator: it adds anchors — `^[^:*$]*$`, `^[\w_-]*$`, `^[\w+=/:,.@-]*$`, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`.
Traces: logs.describe-metric-filters-metric-name-with-dollar-sign-rejected, logs.describe-configuration-templates-dotted-service-name-rejected, logs.list-tags-for-resource-wildcard-suffixed-arn-rejected.

Cause 3 of 9 — only one of several violated constraints on one member is reported.
AWS: a value that breaks both the length bound and the pattern yields `2 validation errors detected: ` with both clauses (in either order).
Emulator: `1 validation error detected: ` with the length clause only, e.g. `Value at 'logGroupNamePattern' failed to satisfy constraint: Member must have length greater than or equal to 3` with the pattern clause missing; likewise for `detectorName`.
Traces: logs.aggregate-summaries-pattern-below-min-length-rejected, logs.anomaly-detector-empty-name-rejected, logs.list-log-groups-name-pattern-term-length-bounds-rejected.

Cause 4 of 9 — `DeleteDataProtectionPolicy` uses `InvalidParameterException` where AWS uses `ValidationException` for a pattern violation.
AWS: `('ValidationException', "1 validation error detected: Value at 'logGroupIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w#+=/:,.@-]*")`.
Emulator: the same message under code `InvalidParameterException`.
Traces: logs.delete-data-protection-policy-identifier-outside-pattern-rejected.

Cause 5 of 9 — the over-long `logGroupIdentifier` refusals on `DeleteDataProtectionPolicy` use the generic template instead of AWS's two bespoke messages.
AWS: an over-long NAME gives `('InvalidParameterException', 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.')` and an over-long ARN gives `('ValidationException', 'Could not parse value as a valid ARN.')` — both entirely fixed text.
Emulator: `('InvalidParameterException', "1 validation error detected: Value at 'logGroupIdentifier' failed to satisfy constraint: Member must have length less than or equal to 2048")`.
Traces: logs.delete-data-protection-policy-overlong-identifier-rejected-masking-kept.

Cause 6 of 9 — an enum value set is listed alphabetically and carries a value AWS does not list.
AWS: `Member must satisfy enum value set: [Timeout, Scheduled, Cancelled, Complete, Failed, Running]` — the model's member order.
Emulator: `[Cancelled, Complete, Failed, Running, Scheduled, Timeout, Unknown]` — sorted, and with `Unknown` included.
Traces: logs.describe-queries-status-outside-enum-rejected.

Cause 7 of 9 — length-bound refusals on `logGroupNamePrefix`/`logStreamNamePrefix` use a bespoke sentence where AWS uses the standard template.
AWS: `('InvalidParameterException', "1 validation error detected: Value at 'logGroupNamePrefix' failed to satisfy constraint: Member must have length less than or equal to 512")`.
Emulator: `('InvalidParameterException', 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.')`.
Traces: logs.describe-log-groups-name-prefix-length-bounds-rejected, logs.start-live-tail-stream-name-prefix-over-512-characters-rejected.

Cause 8 of 9 — the message omits the offending value.
AWS: `1 validation error detected: Value '<the value sent>' at 'logObjectPointer' failed to satisfy constraint: Member must have length less than or equal to 512` — the quoted value is the request's value.
Emulator: `Value at 'logObjectPointer' ...` — the `Value '<...>'` form is dropped.
Traces: logs.get-log-object-pointer-length-bound-rejected.

Cause 9 of 9 — `PutQueryDefinition.clientToken` reports two clauses where AWS reports one.
AWS: the refusal carries a single clause for the member.
Emulator: `2 validation errors detected: Value at 'clientToken' failed to satisfy constraint: Member must have length greater than or equal to 36; Value at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\S{36,128}$`.
Traces: logs.put-query-definition-client-token-outside-36-to-128-rejected.

Why: a client surfaces these messages to its own users and frequently matches on them (to map a service refusal to a field in a form, or to decide whether to retry). A different error code changes the client's branch entirely; an anchored pattern, an indexed member path, a sorted enum list, a dropped value or a missing clause all make the message say something AWS never says about which input was wrong.

Fix: the shared validation-error renderer for the logs service — aggregate list-member constraints onto the list member, echo the model's pattern verbatim, emit every violated constraint, preserve the model's enum member order, keep the `Value '<v>' at '<member>'` form, and the per-operation error classification for DeleteDataProtectionPolicy, DescribeLogGroups and StartLiveTail prefix bounds.

## Evidence
Cause 1: got "2 validation errors detected: Value at 'accountIdentifiers.1.member' failed to satisfy constraint: Member must have length less than or equal to 12; ... pattern: ^\\d{12}$"; expected "1 validation error detected: Value at 'accountIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: ^\\d{12}$]". Same shape for fieldIndexNames.1.member, resourceTypes.1.member, tagKeys.2.member, tags.2.member, transformerConfig.2.member.deleteKeys.withKeys.1.member.
Cause 2: got "... pattern: ^[^:*$]*$", expected "... pattern: [^:*$]*"; got "^[\\w_-]*$", expected "[\\w_-]*".
Cause 3: got "1 validation error detected: Value at 'logGroupNamePattern' ... length greater than or equal to 3", expected "2 validation errors detected: ... pattern ...; ... length greater than or equal to 3".
Cause 4: got InvalidParameterException, expected ValidationException (same message).
Cause 5: got ('InvalidParameterException', "1 validation error detected: Value at 'logGroupIdentifier' ... less than or equal to 2048"); expected ('InvalidParameterException', 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.') for a name and ('ValidationException', 'Could not parse value as a valid ARN.') for an ARN.
Cause 6: got "[Cancelled, Complete, Failed, Running, Scheduled, Timeout, Unknown]", want "[Timeout, Scheduled, Cancelled, Complete, Failed, Running]".
Cause 7: got 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.', expected the templated "1 validation error detected: Value at 'logGroupNamePrefix' ...".
Cause 8: got "Value at 'logObjectPointer' ...", expected "Value '<pointer>' at 'logObjectPointer' ...".
Cause 9: got "2 validation errors detected: ... clientToken ... greater than or equal to 36; ... pattern: ^\\S{36,128}$".

## Steps to Reproduce
1. Call DescribeLogGroups with includeLinkedAccounts=True and a 13-digit accountIdentifiers entry; compare the message with AWS's aggregated single-clause form.
2. Call DescribeMetricFilters with metricName='OrdersPlaced$' and DescribeConfigurationTemplates with a dotted service name; compare the echoed regex.
3. Call GetLogAnomalyDetectors/CreateLogAnomalyDetector with an empty detectorName and the aggregate-summaries API with a 2-character logGroupNamePattern; count the reported clauses.
4. Call DeleteDataProtectionPolicy with an identifier containing '!' and with a 2049-character name/ARN; compare code and message.
5. Call DescribeQueries with status='Finished'; compare the bracketed enum list.
6. Call DescribeLogGroups with a 513-character logGroupNamePrefix and StartLiveTail with a 513-character stream prefix; compare the message template.
7. Call GetLogObject with a 513-character pointer and PutQueryDefinition with a 35-character clientToken.
