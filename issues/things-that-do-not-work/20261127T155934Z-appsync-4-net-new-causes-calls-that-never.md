Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.environment-variable-key-with-leading-digit-rejected appsync.environment-variable-value-over-512-chars-rejected appsync.graphql-resolver-reads-updated-environment-variable appsync.js-resolver-serves-replaced-environment-variables-over-graphql appsync.js-util-base64-encodes-utf8-bytes-with-standard-alphabet appsync.event-publish-batch-above-five-events-rejected appsync.domain-api-association-lifecycle appsync.list-apis-item-agrees-with-get-api appsync.list-channel-namespaces-item-matches-get-and-handler-filters-publish appsync.evaluate-code-non-json-context-reports-parse-error
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.environment-variable-key-with-leading-digit-rejected|_read_setting#16|Timeout|time.sleep(min(2 ** attempt, 15))
Issue-Key: appsync.environment-variable-value-over-512-chars-rejected|_served_until#15|Timeout|time.sleep(3)
Issue-Key: appsync.graphql-resolver-reads-updated-environment-variable|_poll_motto#15|Timeout|time.sleep(3)
Issue-Key: appsync.js-resolver-serves-replaced-environment-variables-over-graphql|_served#15|Timeout|time.sleep(3)
Issue-Key: appsync.js-util-base64-encodes-utf8-bytes-with-standard-alphabet|_code#13|Timeout|time.sleep(2.0)
Issue-Key: appsync.event-publish-batch-above-five-events-rejected|run#29|Timeout|while _stored(ddb, table) != set(five):
Issue-Key: appsync.domain-api-association-lifecycle|_create_domain_name_expect_rejection#8|Timeout|time.sleep(interval_s)
Issue-Key: appsync.list-apis-item-agrees-with-get-api|run#23|AssertionError|assert served == [], f"the ListApis item carries {served!r}, which only GetApi serves"
Issue-Key: appsync.list-channel-namespaces-item-matches-get-and-handler-filters-publish|run#24|AssertionError|assert set(item) == set(single) - detail_only, (
Issue-Key: appsync|EvaluateCode|BadRequestException
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync: 4 net-new causes — calls that never settle (7 checks time out at 60s), ListApis/ListChannelNamespaces item shapes, EvaluateCode non-JSON context

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutGraphqlApiEnvironmentVariables

## Description
Four distinct causes; each must be closed before submitting.

Cause 1 of 4: seven checks never complete — the emulator never settles these paths inside the 60-second budget.
AWS: each of these sequences completes in seconds: PutGraphqlApiEnvironmentVariables refusing an invalid key or an over-512-character value and answering the refusal; a resolver reading an environment variable that a later Put replaced (the new value is served); util.base64Encode/Decode answering over GraphQL; an Event API publish carrying more than five events being refused; and the DomainName/ApiAssociation lifecycle (CreateDomainName, AssociateApi, GetApiAssociation, DisassociateApi, DeleteDomainName) reaching its terminal states.
Emulator: every one of these checks ends with "Check timed out after 60.0s" — no response or no terminal state is reached, so nothing further in the check is exercised. Hypothesis: the env-var path and the domain-association path never reach the state the caller polls for, and the data-plane calls block rather than answer.
Why: a client hangs instead of getting an answer or an error — the worst failure mode for a caller with a deadline, and it blocks every downstream check.
Fix: PutGraphqlApiEnvironmentVariables and its propagation into the resolver runtime, the DomainName/ApiAssociation state transitions, the Event API publish batch path, and the util.base64 helpers — each must reach a terminal answer.
Traces: appsync.environment-variable-key-with-leading-digit-rejected, appsync.environment-variable-value-over-512-chars-rejected, appsync.graphql-resolver-reads-updated-environment-variable, appsync.js-resolver-serves-replaced-environment-variables-over-graphql, appsync.js-util-base64-encodes-utf8-bytes-with-standard-alphabet, appsync.event-publish-batch-above-five-events-rejected, appsync.domain-api-association-lifecycle

Cause 2 of 4: ListApis items carry members only GetApi serves.
AWS: the Api items ListApis returns do not carry `dns` or `eventConfig`; those are served only by GetApi. Every other member agrees between the two calls.
Emulator: each ListApis item carries dns and eventConfig as well.
Why: a client that treats the listing as the canonical projection reads endpoints from a listing AWS never populates, and a diff of list-vs-get disagrees.
Fix: the ListApis output projection.
Traces: appsync.list-apis-item-agrees-with-get-api

Cause 3 of 4: ListChannelNamespaces items omit codeHandlers.
AWS: a ListChannelNamespaces item carries the same members as GetChannelNamespace, including `codeHandlers`.
Emulator: the list entry carries ['apiId', 'channelNamespaceArn', 'created', 'handlerConfigs', 'lastModified', 'name', 'publishAuthModes', 'subscribeAuthModes', 'tags'] while GetChannelNamespace additionally carries 'codeHandlers' — the two disagree.
Why: a client enumerating namespaces to read their handler code gets nothing and must re-fetch each one.
Fix: the ListChannelNamespaces output projection.
Traces: appsync.list-channel-namespaces-item-matches-get-and-handler-filters-publish

Cause 4 of 4: EvaluateCode refuses a non-JSON context instead of reporting the parse error.
AWS: EvaluateCode given a `context` that is not JSON SUCCEEDS at the API level and answers an `error` member whose message carries the parser's diagnosis, whose first line is "Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')" — the token comes from the request body.
Emulator: raises BadRequestException "Invalid context given.", so there is no error detail at all.
Why: EvaluateCode exists to debug templates; the client loses the parse diagnosis and gets a refusal where AWS returns a result.
Fix: EvaluateCode's context handling — parse failures belong in the response's error member, not in a refusal.
Traces: appsync.evaluate-code-non-json-context-reports-parse-error

## Evidence
Cause 1: seven checks report "Error: Check timed out after 60.0s" (appsync.domain-api-association-lifecycle, appsync.environment-variable-key-with-leading-digit-rejected, appsync.environment-variable-value-over-512-chars-rejected, appsync.event-publish-batch-above-five-events-rejected, appsync.graphql-resolver-reads-updated-environment-variable, appsync.js-resolver-serves-replaced-environment-variables-over-graphql, appsync.js-util-base64-encodes-utf8-bytes-with-standard-alphabet).
Cause 2: AssertionError: the ListApis item carries ['dns', 'eventConfig'], which only GetApi serves.
Cause 3: AssertionError: the list entry carries [... no 'codeHandlers' ...]; GetChannelNamespace carries ['apiId', 'channelNamespaceArn', 'codeHandlers', 'created', 'handlerConfigs', 'lastModified', 'name', 'publishAuthModes', 'subscribeAuthModes', 'tags'].
Cause 4: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the EvaluateCode operation: Invalid context given. Expected: a 200 whose error.message begins "Unrecognized token 'arguments': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')".

## Steps to Reproduce
(1) Call PutGraphqlApiEnvironmentVariables with a leading-digit key and with a 513-character value; have a JS and a VTL resolver read an env var and replace it with a second Put; query util.base64Encode over GraphQL; POST more than five events to an Event API; run CreateDomainName/AssociateApi/GetApiAssociation/DisassociateApi/DeleteDomainName — each of these hangs past 60s. (2) Create an Event API and compare a ListApis item with GetApi. (3) Create a channel namespace with codeHandlers and compare a ListChannelNamespaces item with GetChannelNamespace. (4) Call EvaluateCode with context='arguments...' (not JSON). Replay the checks in trace_ids.
