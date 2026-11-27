Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__lambda.event-invocation-returns-null-and-function-runs-later appsync__lambda.vtl-event-invocation-answers-null-while-function-gets-payload appsync__lambda.invoke-payload-arrives-unchanged-for-request-response-and-event appsync__lambda.invoke-invocation-type-outside-enum-rejected appsync__lambda.vtl-lambda-request-lowercase-invoke-operation-rejected appsync__lambda.vtl-lambda-request-unsupported-invocation-type-rejected appsync__lambda.batch-invoke-event-delivers-siblings-as-one-list-payload appsync__lambda.batch-invoke-short-result-list-rejected appsync__lambda.direct-lambda-batch-answer-shorter-than-batch-rejected appsync__lambda.create-function-max-batch-size-above-2000-rejected appsync__lambda.event-api-direct-lambda-on-publish-rewrites-and-filters-broadcast appsync__lambda.event-invoke-on-publish-handler-broadcasts-events-unchanged appsync__lambda.event-namespace-direct-lambda-event-invoke-broadcasts-events-unchanged appsync__lambda.event-namespace-direct-lambda-publish-broadcasts-function-events appsync__lambda.event-namespace-direct-lambda-top-level-error-publish-rejected appsync__lambda.event-api-direct-lambda-on-subscribe-error-refuses-subscription-rejected appsync__lambda.event-api-on-subscribe-lambda-error-payload-rejects-subscription appsync__lambda.lambda-authorizer-event-reflects-each-request-into-resolver-context appsync__lambda.lambda-authorizer-exception-answers-authorizer-failure-and-skips-resolver-rejected appsync__lambda.lambda-authorizer-running-past-ten-seconds-rejected appsync__lambda.js-delete-custom-condition-handler-reject-rejected-item-kept appsync__lambda.vtl-update-custom-condition-handler-reject-rejected-item-kept appsync__lambda.version-pinned-lambda-data-source-unqualified-grant-rejected appsync__lambda.appsync-update-graphql-api-lambda-auth-without-authorizer-config-rejected appsync__lambda.update-graphql-api-to-aws-lambda-without-authorizer-config-rejected
Issue-Key: appsync__lambda.event-invocation-returns-null-and-function-runs-later|run#30|AssertionError|assert fired == {"data": {"fireAndForget": None}}, (
Issue-Key: appsync__lambda.vtl-event-invocation-answers-null-while-function-gets-payload|run#30|AssertionError|assert queued == {"data": {"queued": None}}, (
Issue-Key: appsync__lambda.invoke-payload-arrives-unchanged-for-request-response-and-event|run#37|AssertionError|assert fired == {"data": {"askLater": None}}, f"the Event field answered {fired!r}"
Issue-Key: appsync__lambda.invoke-invocation-type-outside-enum-rejected|run#31|AssertionError|assert refused.get("data") == {"odd": None} and len(errors) == 1, (
Issue-Key: appsync__lambda.vtl-lambda-request-lowercase-invoke-operation-rejected|run#31|AssertionError|assert refused.get("data") == {"lowered": None}, (
Issue-Key: appsync__lambda.vtl-lambda-request-unsupported-invocation-type-rejected|run#31|AssertionError|assert refused.get("data") == {"dryrun": None}, (
Issue-Key: appsync__lambda.batch-invoke-event-delivers-siblings-as-one-list-payload|run#31|AssertionError|assert body == expected, (
Issue-Key: appsync__lambda.batch-invoke-short-result-list-rejected|run#40|AssertionError|assert errors == expected, f"expected one error per sibling {expected!r}, got {errors!r}"
Issue-Key: appsync__lambda.direct-lambda-batch-answer-shorter-than-batch-rejected|<genexpr>#33|KeyError|got = sorted((tuple(e["path"]), e["errorType"], e["message"]) for e in body["errors"])
Issue-Key: appsync__lambda.create-function-max-batch-size-above-2000-rejected|_refused#14|AssertionError|raise AssertionError(f"maxBatchSize {batch} on {source!r} was accepted as {fn['name']!r}")
Issue-Key: appsync__lambda.event-api-direct-lambda-on-publish-rewrites-and-filters-broadcast|run#27|AssertionError|assert latency is not None, "no probe event rewritten by the function reached the subscriber"
Issue-Key: appsync__lambda.event-invoke-on-publish-handler-broadcasts-events-unchanged|run#38|AssertionError|assert _delivered(got) == {"rewritten": True, "nonce": nonce}, (
Issue-Key: appsync__lambda.event-namespace-direct-lambda-event-invoke-broadcasts-events-unchanged|_until#21|AssertionError|raise AssertionError(f"{what}: not reached, last value {last!r}")
Issue-Key: appsync__lambda.event-namespace-direct-lambda-publish-broadcasts-function-events|run#36|AssertionError|assert any(g.get("by") == "fn" for g in got), (
Issue-Key: appsync__lambda.event-namespace-direct-lambda-top-level-error-publish-rejected|run#23|AssertionError|assert status == 403, f"the vetoed publish answered HTTP {status}: {vetoed!r}"
Issue-Key: appsync__lambda.event-api-direct-lambda-on-subscribe-error-refuses-subscription-rejected|run#33|AssertionError|assert refused.get("type") == "subscribe_error", (
Issue-Key: appsync__lambda.event-api-on-subscribe-lambda-error-payload-rejects-subscription|run#33|AssertionError|assert refusal is not None and refusal.get("type") == "subscribe_error", (
Issue-Key: appsync__lambda.lambda-authorizer-event-reflects-each-request-into-resolver-context|run#35|AssertionError|assert seen == expected, (
Issue-Key: appsync__lambda.lambda-authorizer-exception-answers-authorizer-failure-and-skips-resolver-rejected|run#38|AssertionError|assert status_code == 500, (
Issue-Key: appsync__lambda.lambda-authorizer-running-past-ten-seconds-rejected|run#34|AssertionError|assert refused[0] == 401, f"an authorizer still running after 10 s gave HTTP {refused[0]}: {refused[1]!r}"
Issue-Key: appsync__lambda.js-delete-custom-condition-handler-reject-rejected-item-kept|run#46|AssertionError|assert body == {"data": {"erase": {"id": nid, "body": "keep me", "seq": 4}}}, (
Issue-Key: appsync__lambda.vtl-update-custom-condition-handler-reject-rejected-item-kept|run#44|AssertionError|assert errors[0].get("data") == {"id": cid, "label": "final", "rev": 2}, (
Issue-Key: appsync__lambda.version-pinned-lambda-data-source-unqualified-grant-rejected|run#36|AssertionError|assert (refusal.get("errorType"), head) == (
Issue-Key: appsync__lambda.appsync-update-graphql-api-lambda-auth-without-authorizer-config-rejected|run#30|AssertionError|assert got == ("BadRequestException", "Lambda Authorizer Config missing"), (
Issue-Key: appsync__lambda.update-graphql-api-to-aws-lambda-without-authorizer-config-rejected|run#20|AssertionError|assert (code, message) == ("BadRequestException", "Lambda Authorizer Config missing"), (
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync__lambda: 8 net-new causes — Event invocations return the function's answer, batch invocation, invalid invocationType served, Event API direct-Lambda handlers inert, authorizer behaviour

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Eight distinct causes in the AWS Lambda data-source and Lambda-authorizer surface; each must be closed before submitting.

Cause 1 of 8: an Event invocation returns the function's answer instead of null.
AWS: a Lambda data source invoked with invocationType Event (operation 'Invoke' with invocationType 'Event' in VTL, or the Event invocation type on a direct-Lambda/JS resolver) is fire-and-forget: the field resolves to NULL with no errors, and the function still runs and observes the payload afterwards. The payload itself arrives at the function unchanged.
Emulator: the field resolves to the function's return value ({'fireAndForget': 'returned-by-function'}, {'queued': 'value-from-function'}, the base64 payload for askLater), i.e. the Event invocation is performed synchronously and its result is served.
Why: a client sees data where AWS guarantees null, so a schema marking the field nullable-and-empty is contradicted and latency behaviour differs.
Fix: the Lambda data-source invocation path — honour invocationType Event by discarding the result.
Traces: appsync__lambda.event-invocation-returns-null-and-function-runs-later, appsync__lambda.vtl-event-invocation-answers-null-while-function-gets-payload, appsync__lambda.invoke-payload-arrives-unchanged-for-request-response-and-event

Cause 2 of 8: an invocationType/operation outside the supported set is served instead of refused.
AWS: a Lambda request mapping naming an unsupported operation (lowercase 'invoke') or an unsupported invocationType ('Async', 'DryRun') nulls the field and reports one error; nothing is invoked.
Emulator: resolves the field with the function's answer ({'odd': 'odd'}, {'lowered': 'served-...'}, {'dryrun': 'served-...'}) and reports no error.
Why: a typo in a resolver template silently works here and fails in production, and the client gets data AWS would never return.
Fix: validation of the Lambda request mapping's operation/invocationType in the data-source invoke path.
Traces: appsync__lambda.invoke-invocation-type-outside-enum-rejected, appsync__lambda.vtl-lambda-request-lowercase-invoke-operation-rejected, appsync__lambda.vtl-lambda-request-unsupported-invocation-type-rejected

Cause 3 of 8: batch invocation is not batched, its size contract is not enforced, and the mismatch error is worded wrongly.
AWS: a batched Lambda data source receives its siblings as ONE list payload, so a batched Event invocation leaves every sibling field null rather than individually answered. If the function answers a list shorter than the batch, EVERY sibling field reports errorType 'Lambda:IllegalArgument' with message "Response from Lambda for Batch invocation must match in size and order the given input batch. Expected list of size <n> but got a list of size <m> instead." (<n>/<m> are the request's batch size and the answer's length). CreateFunction with maxBatchSize above 2000 is refused.
Emulator: answers each sibling separately (every touch carried the function's per-item answer); reports errorType None with its own message "The data source's function answered a batched invocation with something other than a list of 3."; one direct-Lambda check finds no 'errorType' member at all (KeyError); and maxBatchSize 2000 was accepted on a data source stored as 'unbatchable'.
Why: batching changes the payload contract a function is written against, and clients classifying on errorType get None.
Fix: the batched Lambda invocation path (single list payload, size/order check and its message) and CreateFunction's maxBatchSize validation.
Traces: appsync__lambda.batch-invoke-event-delivers-siblings-as-one-list-payload, appsync__lambda.batch-invoke-short-result-list-rejected, appsync__lambda.direct-lambda-batch-answer-shorter-than-batch-rejected, appsync__lambda.create-function-max-batch-size-above-2000-rejected

Cause 4 of 8: Event API direct-Lambda onPublish/onSubscribe handlers are not invoked, or their result is ignored.
AWS: a channel namespace whose onPublish integration is a Lambda data source passes the published events through the function — the function's rewritten/filtered events are what subscribers receive, and with the Event invocation type the events are broadcast unchanged. A top-level error returned by the function vetoes the publish (the HTTP publish is refused, not 200).
Emulator: no event rewritten by the function reaches the subscriber; the publish never reaches the function at all in the event-invoke case; subscribers receive the original event instead of the handler's answer; and a vetoed publish answers HTTP 200 with every event in 'successful'.
Why: the handler is the only place an application filters or rewrites events; here it is bypassed, so unauthorized or unfiltered events are broadcast.
Fix: the channel-namespace onPublish integration path for Lambda data sources (invoke, use the answer, honour a returned error).
Traces: appsync__lambda.event-api-direct-lambda-on-publish-rewrites-and-filters-broadcast, appsync__lambda.event-invoke-on-publish-handler-broadcasts-events-unchanged, appsync__lambda.event-namespace-direct-lambda-event-invoke-broadcasts-events-unchanged, appsync__lambda.event-namespace-direct-lambda-publish-broadcasts-function-events, appsync__lambda.event-namespace-direct-lambda-top-level-error-publish-rejected

Cause 5 of 8: an onSubscribe handler's error does not refuse the subscription.
AWS: when the onSubscribe Lambda returns an error (as a thrown error or as an error payload), the subscribe is REFUSED — the client gets a subscribe_error frame, not a subscription.
Emulator: answers subscribe_success ({'type': 'subscribe_success', 'id': ...}) and the subscription is established.
Why: a client is subscribed to a channel the application denied, and then receives events it must not see.
Fix: the channel-namespace onSubscribe integration path — map a handler error to a subscribe refusal.
Traces: appsync__lambda.event-api-direct-lambda-on-subscribe-error-refuses-subscription-rejected, appsync__lambda.event-api-on-subscribe-lambda-error-payload-rejects-subscription

Cause 6 of 8: the Lambda authorizer's event, failure mode and timeout all diverge.
AWS: the authorizer event mirrors each request (a variable the request did not send is absent/null, not the string 'None'), and the resolverContext the resolver sees mirrors that same event; an authorizer that RAISES refuses the request with HTTP 500 and the resolver does not run; an authorizer still running after 10 seconds is cut off and the request is refused.
Emulator: puts the literal string 'None' in resolverContext for the key the request omitted; answers HTTP 401 {'errors': [{'errorType': 'UnauthorizedException', 'message': 'You are not authorized to make this call.'}]} for a raising authorizer; and lets an authorizer run past 10 seconds, answering HTTP 200 with the resolver's data.
Why: a client cannot distinguish "denied" (401) from "authorizer broken" (500), reads the string 'None' as a value, and a hung authorizer serves data AWS would never serve.
Fix: the Lambda-authorizer invocation path — event/resolverContext serialization, exception-to-500 mapping, and the 10-second cutoff.
Traces: appsync__lambda.lambda-authorizer-event-reflects-each-request-into-resolver-context, appsync__lambda.lambda-authorizer-exception-answers-authorizer-failure-and-skips-resolver-rejected, appsync__lambda.lambda-authorizer-running-past-ten-seconds-rejected

Cause 7 of 8: a Lambda conflict handler's reject does not carry the stored item.
AWS: when a Lambda conflict handler rejects a stale write, the GraphQL error carries the item's CURRENT stored value (the data member), and the stored item is left unchanged.
Emulator: the rejection carries no current value (it carried None), and the delete case answers errorType 'DynamoDB:ConditionalCheckFailedException' with message 'The conditional request failed' rather than the handler's rejection carrying the stored item.
Why: optimistic-concurrency clients re-base on the value returned in the rejection; without it they cannot retry.
Fix: the conflict-handler rejection path's error data (data member carrying the stored item).
Traces: appsync__lambda.js-delete-custom-condition-handler-reject-rejected-item-kept, appsync__lambda.vtl-update-custom-condition-handler-reject-rejected-item-kept

Cause 8 of 8: a Lambda invoke denial's message omits the SDK exception prefix and the service/request-id tail.
AWS: when the data source's role may invoke the function but not the pinned version, the field's error carries errorType 'Lambda:IllegalArgument' and the message "software.amazon.awssdk.services.lambda.model.LambdaException: User: <role-session-arn> is not authorized to perform: lambda:InvokeFunction on resource: <version-arn> because no identity-based policy allows the lambda:InvokeFunction action (Service: Lambda, Status Code: 403, Request ID: <uuid>) (SDK Attempt Count: <n>)" — the ARNs come from the request/state, the uuid and count are per-call.
Emulator: emits only the middle sentence: no "software.amazon.awssdk.services.lambda.model.LambdaException: " prefix and no " (Service: Lambda, Status Code: 403, Request ID: ...) (SDK Attempt Count: ...)" tail.
Why: clients and operators parse the Service/Status Code portion to tell an authorization failure from an argument error; it is absent.
Fix: the Lambda-invoke error serializer in the data-source path.
Traces: appsync__lambda.version-pinned-lambda-data-source-unqualified-grant-rejected

## Evidence
Cause 1: 'the Event field answered {\'data\': {\'fireAndForget\': \'returned-by-function\'}}, expected null data and no errors'.
Cause 2: "invocationType 'Async' must null the field with one error: {'data': {'odd': 'odd'}}"; 'the refused field must resolve to null, got data {\'lowered\': \'served-371f79ed3212\'}'.
Cause 3: expected one error per sibling ('Lambda:IllegalArgument', 'Response from Lambda for Batch invocation must match in size and order the given input batch. Expected list of size 3 but got a list of size 2 instead.'), got (None, "The data source's function answered a batched invocation with something other than a list of 3."); 'maxBatchSize 2000 on \'passthrough\' was accepted as \'unbatchable\''; KeyError: 'errorType'.
Cause 4: 'no probe event rewritten by the function reached the subscriber'; 'a publish reaching the function: not reached, last value False'; 'the vetoed publish answered HTTP 200: {\'failed\': [], \'successful\': [...]}'.
Cause 5: "the function returned an error for '/vault/sealed', so the subscription must be refused; got {'type': 'subscribe_success', ...}".
Cause 6: resolverContext mismatched keys ['marker'], got 'marker': 'None'; 'a raising authorizer must refuse the request with 500; got 401 ...'; 'an authorizer still running after 10 s gave HTTP 200: {\'data\': {\'greeting\': \'authorized-greeting\'}}'.
Cause 7: "the rejection should carry the item's current value; it carried None".
Cause 8: got message 'User: arn:aws:sts::...:assumed-role/.../APPSYNC_ASSUME_ROLE is not authorized to perform: lambda:InvokeFunction on resource: ...:1 because no identity-based policy allows the lambda:InvokeFunction action' with errorType 'Lambda:IllegalArgument'; AWS prefixes 'software.amazon.awssdk.services.lambda.model.LambdaException: ' and appends ' (Service: Lambda, Status Code: 403, Request ID: <uuid>) (SDK Attempt Count: <n>)'.

## Steps to Reproduce
Create a GraphQL API with an AWS_LAMBDA data source and resolvers, plus an Event API whose channel namespace integrates a Lambda data source. Then: (1) resolve a field whose Lambda request uses invocationType 'Event' (JS and VTL); (2) use operation 'invoke' (lowercase) and invocationType 'Async'/'DryRun'; (3) resolve a list field against a batched data source whose function answers a 2-element list for a 3-element batch, and CreateFunction with maxBatchSize=2000; (4) publish to a namespace whose onPublish Lambda rewrites/filters events, and one that returns a top-level error; (5) subscribe where onSubscribe throws and where it returns an error payload; (6) attach a Lambda authorizer that echoes its event, one that raises, and one that sleeps past 10s; (7) reject a stale write through a Lambda conflict handler; (8) point a data source at a pinned function version while granting InvokeFunction only on the unqualified ARN. Replay the checks in trace_ids.
