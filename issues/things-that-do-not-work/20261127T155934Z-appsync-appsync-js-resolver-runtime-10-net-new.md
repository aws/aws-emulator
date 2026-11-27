Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.js-resolver-util-math-rounds-half-up-and-bounds-pair appsync.js-resolver-util-transform-builds-dynamodb-expressions appsync.js-resolver-util-url-encode-follows-form-encoding appsync.js-util-time-converts-epoch-and-parses-iso-in-graphql-response appsync.js-resolver-util-str-normalize-applies-unicode-forms appsync.js-resolver-util-http-restricted-response-header-rejected appsync.graphql-js-append-error-keeps-data-and-filters-error-data-by-selection appsync.js-util-error-data-follows-selection-set-error-info-does-not appsync.js-response-handler-util-unauthorized-nested-alias-field-rejected appsync.js-early-return-skip-to-next-or-end-in-pipeline-function appsync.vtl-raw-interpolation-of-quoted-argument-rejected-while-to-json-serves-it appsync.graphql-ws-start-of-undefined-subscription-field-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.js-resolver-util-math-rounds-half-up-and-bounds-pair|run#17|AssertionError|assert "errors" not in body, f"the query answered errors: {body.get('errors')!r}"
Issue-Key: appsync.js-resolver-util-transform-builds-dynamodb-expressions|run#18|AssertionError|assert "errors" not in body, f"the query answered errors: {body.get('errors')!r}"
Issue-Key: appsync.js-resolver-util-url-encode-follows-form-encoding|run#20|AssertionError|assert "errors" not in body, f"the query answered errors: {body.get('errors')!r}"
Issue-Key: appsync.js-util-time-converts-epoch-and-parses-iso-in-graphql-response|run#26|AssertionError|assert "errors" not in body, f"the resolver raised GraphQL errors: {body['errors']!r}"
Issue-Key: appsync.js-resolver-util-str-normalize-applies-unicode-forms|run#16|TypeError|forms = json.loads(body["data"]["forms"])
Issue-Key: appsync.js-resolver-util-http-restricted-response-header-rejected|run#18|AssertionError|assert body.get("data") == {"stamp": f"stamped:{note}"}, f"the field answered {body.get('data')!r}"
Issue-Key: appsync.graphql-js-append-error-keeps-data-and-filters-error-data-by-selection|run#20|AssertionError|assert got == want, (
Issue-Key: appsync.js-util-error-data-follows-selection-set-error-info-does-not|run#23|AssertionError|assert entry.get("data") == {"id": "sku-7"}, (
Issue-Key: appsync.js-response-handler-util-unauthorized-nested-alias-field-rejected|run#24|AssertionError|assert got == (
Issue-Key: appsync.js-early-return-skip-to-next-or-end-in-pipeline-function|run#16|AssertionError|assert got == trail, f"{field}: expected trail {trail!r}, got {got!r}"
Issue-Key: appsync.vtl-raw-interpolation-of-quoted-argument-rejected-while-to-json-serves-it|run#26|KeyError|got = (errors[0]["errorType"], errors[0]["message"], errors[0]["path"], errors[0]["locations"])
Issue-Key: appsync.graphql-ws-start-of-undefined-subscription-field-rejected|run#22|AssertionError|assert errors == expected, f"the error carried {errors!r}, expected {expected!r}"
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync: APPSYNC_JS resolver runtime — 10 net-new causes (missing util namespaces, error-envelope shaping, early return, undefined subscription field)

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EvaluateCode

## Description
The GraphQL data plane's APPSYNC_JS/VTL evaluator is missing documented `util` namespaces and shapes the GraphQL error envelope differently from AWS. Ten distinct causes; each must be closed before submitting.

Cause 1 of 10: util.math is absent.
AWS: the APPSYNC_JS runtime exposes util.math with roundNum, minVal and maxVal; a resolver calling util.math.roundNum(x) returns the half-up rounded number and the query answers data with no errors.
Emulator: the field resolves to null and the query carries an error [{'path': ['numbers'], 'message': "cannot read property 'roundNum' of undefined", 'errorType': 'TypeError'}] — the `util.math` object itself does not exist.
Why: any resolver using the documented math helpers fails at runtime.
Fix: the APPSYNC_JS runtime's util namespace registration (add math).
Traces: appsync.js-resolver-util-math-rounds-half-up-and-bounds-pair

Cause 2 of 10: util.transform is absent.
AWS: util.transform.toDynamoDBFilterExpression(filter) (and the sibling transform helpers) build a DynamoDB expression object from a filter map.
Emulator: [{'path': ['expressions'], 'message': "cannot read property 'toDynamoDBFilterExpression' of undefined", 'errorType': 'TypeError'}] — `util.transform` does not exist.
Why: the documented way to build DynamoDB filters from GraphQL arguments is unavailable, so those resolvers cannot run at all.
Fix: the APPSYNC_JS runtime's util namespace registration (add transform).
Traces: appsync.js-resolver-util-transform-builds-dynamodb-expressions

Cause 3 of 10: util.urlEncode and util.time members exist but are not callable.
AWS: util.urlEncode/util.urlDecode apply form encoding, and util.time.epochMilliSecondsToISO8601 / parseISO8601ToEpochMilliSeconds (and siblings) convert timestamps; both answer data with no errors.
Emulator: both answer errorType 'TypeError' with message 'not a function' — the members are present on `util` but hold non-callable values.
Why: time and URL handling are in most resolvers; they fail with a TypeError instead of returning a value.
Fix: the APPSYNC_JS runtime's util bindings for urlEncode/urlDecode and the util.time namespace.
Traces: appsync.js-resolver-util-url-encode-follows-form-encoding, appsync.js-util-time-converts-epoch-and-parses-iso-in-graphql-response

Cause 4 of 10: util.str.normalize returns nothing.
AWS: util.str.normalize(s, form) applies the named Unicode normalization form (nfc/nfd/nfkc/nfkd) and the resolver answers the normalized string.
Emulator: the field answers null — the check's json.loads of the response member raises "the JSON object must be str, bytes or bytearray, not NoneType", i.e. nothing was produced.
Why: a resolver normalizing user input silently loses its value.
Fix: util.str.normalize in the APPSYNC_JS runtime.
Traces: appsync.js-resolver-util-str-normalize-applies-unicode-forms

Cause 5 of 10: util.http response-header restrictions are not applied.
AWS: a resolver reading a restricted response header through the util.http helpers is refused that header, and the field reports the refusal rather than resolving.
Emulator: the field answers {'stamp': None} with no error — the restriction is neither enforced nor reported (recorded behavior only).
Why: a client cannot tell a blocked header from an absent one.
Fix: the util.http helper's response-header allow-list in the APPSYNC_JS runtime.
Traces: appsync.js-resolver-util-http-restricted-response-header-rejected

Cause 6 of 10: util.error/util.appendError data is not cut to the selection set.
AWS: the `data` argument of util.error/util.appendError is filtered to the fields the query actually selected, while `errorInfo` is returned whole. With a selection of {sku label} and data {sku, label, weight}, errors[0].data is {"sku": ..., "label": ...} and errorInfo keeps every member.
Emulator: returns the data map unfiltered ({'sku': 'p-100', 'label': 'flagged', 'weight': 99}), and in the util.error case the nested selection case comes back as None instead of the single selected subfield.
Why: the error envelope leaks fields the client did not ask for, and clients that read errors[0].data positionally get the wrong shape.
Fix: the GraphQL error-envelope serializer — apply the field's selection set to errors[].data and not to errors[].errorInfo.
Traces: appsync.graphql-js-append-error-keeps-data-and-filters-error-data-by-selection, appsync.js-util-error-data-follows-selection-set-error-info-does-not

Cause 7 of 10: util.unauthorized() on a nested aliased field reports the wrong errorType/message/path.
AWS: util.unauthorized() in a response handler produces errorType "Unauthorized" with AWS's message naming the field and its parent type, and the path addressing the ALIAS the query used.
Emulator: reports ('Unauthorized', 'Not Authorized to access balance on type Int', ['high', 'balance']) — the type named is the field's own scalar type rather than the declaring type, and the path carries the field name instead of the alias.
Why: a client mapping denials back to the fields it requested cannot find them under the alias it sent.
Fix: the util.unauthorized() error construction in the response-handler path (message type naming and alias-aware path).
Traces: appsync.js-response-handler-util-unauthorized-nested-alias-field-rejected

Cause 8 of 10: util.runtime.earlyReturn does not end the pipeline.
AWS: in a pipeline function, util.runtime.earlyReturn(obj) with the end-of-pipeline form returns obj as the resolver's result immediately — no later pipeline function and no resolver response handler runs.
Emulator: continues the pipeline: the trail recorded 's2f4ddc0b3c0a>stop-request>append-request>append-response' where only 's2f4ddc0b3c0a>stop-request' should appear.
Why: side effects AWS never performs are performed here, and the returned value is overwritten by later stages.
Fix: the pipeline executor's handling of the earlyReturn signal.
Traces: appsync.js-early-return-skip-to-next-or-end-in-pipeline-function

Cause 9 of 10: a VTL interpolation failure produces a GraphQL error with no errorType.
AWS: when raw interpolation of a quoted argument is refused, the error entry in the GraphQL response carries an errorType (the check reads errors[0]['errorType']), while $util.toJson of the same argument serves it.
Emulator: the error entry has no 'errorType' member at all (the check raises KeyError: 'errorType').
Why: errorType is how clients classify data-plane errors; an entry without it cannot be dispatched.
Fix: the VTL evaluator's error-to-envelope mapping — every error entry must carry errorType.
Traces: appsync.vtl-raw-interpolation-of-quoted-argument-rejected-while-to-json-serves-it

Cause 10 of 10: a graphql-ws start naming an undefined subscription field is answered as a control-plane not-found.
AWS: starting a subscription on a field the schema does not declare is a GraphQL VALIDATION error: the frame carries [{"message": "Validation error of type FieldUndefined: Field 'onErase' in type 'Subscription' is undefined @ 'onErase'"}] — the field name and type come from the request and the schema.
Emulator: answers [{'errorType': 'NotFoundException', 'errorCode': 404, 'message': "This API's schema declares no subscription field named onErase."}].
Why: a client treats document validation failures (fix the query) differently from not-found (fix the resource); it gets the wrong class and a message no AppSync client recognises.
Fix: the realtime subscription start path — validate the document against the schema and emit the GraphQL validation error.
Traces: appsync.graphql-ws-start-of-undefined-subscription-field-rejected

## Evidence
Cause 1: errors [{'path': ['numbers'], 'message': "cannot read property 'roundNum' of undefined", 'errorType': 'TypeError'}].
Cause 2: errors [{'path': ['expressions'], 'message': "cannot read property 'toDynamoDBFilterExpression' of undefined", 'errorType': 'TypeError'}].
Cause 3: errors [{'path': ['form'], 'message': 'not a function', 'errorType': 'TypeError'}] and [{'path': ['clock'], 'message': 'not a function', 'errorType': 'TypeError'}].
Cause 4: the normalized value came back as None (TypeError: the JSON object must be str, bytes or bytearray, not NoneType).
Cause 5: the field answered {'stamp': None}.
Cause 6: expected data cut to the selection {'sku': 'p-100', 'label': 'flagged'} with errorInfo whole {'reason': 'manual-review', 'weight': 99}; got data {'sku': 'p-100', 'label': 'flagged', 'weight': 99}. Second check: 'error data must keep only the selected subfield id, got None'.
Cause 7: denial errorType/message/path were ('Unauthorized', 'Not Authorized to access balance on type Int', ['high', 'balance']).
Cause 8: viaEnd: expected trail 's2f4ddc0b3c0a>stop-request', got 's2f4ddc0b3c0a>stop-request>append-request>append-response'.
Cause 9: KeyError: 'errorType' when reading errors[0].
Cause 10: got [{'errorType': 'NotFoundException', 'errorCode': 404, 'message': "This API's schema declares no subscription field named onErase."}], expected [{'message': "Validation error of type FieldUndefined: Field 'onErase' in type 'Subscription' is undefined @ 'onErase'"}].

## Steps to Reproduce
Create a GraphQL API with APPSYNC_JS resolvers and query each helper: (1) util.math.roundNum; (2) util.transform.toDynamoDBFilterExpression; (3) util.urlEncode and util.time.epochMilliSecondsToISO8601; (4) util.str.normalize; (5) a util.http request reading a restricted response header; (6) util.appendError/util.error with a data map wider than the selection set plus an errorInfo map; (7) util.unauthorized() in a response handler for a nested aliased field; (8) a pipeline function calling util.runtime.earlyReturn in the end-of-pipeline form; (9) a VTL template raw-interpolating a quoted argument; (10) a graphql-ws start on a subscription field the schema does not declare. Replay the checks in trace_ids.
