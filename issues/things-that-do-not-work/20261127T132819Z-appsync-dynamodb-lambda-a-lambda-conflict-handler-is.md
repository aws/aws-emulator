Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__lambda.appsync-delete-custom-handler-retry-with-stale-condition-rejected appsync__dynamodb__lambda.condition-failure-custom-handler-retry-applies-its-retry-mapping appsync__dynamodb__lambda.condition-failure-differing-put-goes-to-custom-handler-rejected appsync__dynamodb__lambda.lambda-conflict-resolve-replaces-put-item-and-is-refused-for-delete-item
Issue-Key: appsync__dynamodb__lambda.appsync-delete-custom-handler-retry-with-stale-condition-rejected|run#46|AssertionError|assert not current.get("errors"), f"the retried delete reported errors: {current.get('errors')!r}"
Issue-Key: appsync__dynamodb__lambda.condition-failure-custom-handler-retry-applies-its-retry-mapping|run#29|AssertionError|assert (status, stale) == (200, {"data": {"edit": want}}), (
Issue-Key: appsync__dynamodb__lambda.condition-failure-differing-put-goes-to-custom-handler-rejected|run#25|AssertionError|assert (status, same) == (200, {"data": {"claim": {"id": card, "label": "owner"}}}), (
Issue-Key: appsync__dynamodb__lambda.lambda-conflict-resolve-replaces-put-item-and-is-refused-for-delete-item|run#36|AssertionError|assert len(errors) == 1, f"expected one GraphQL error for RESOLVE on a delete: {body!r}"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# appsync__dynamodb__lambda: a LAMBDA conflict handler is never invoked on a condition failure, and its RESOLVE verdict is accepted for DeleteItem (2 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Two net-new root causes in AppSync's LAMBDA conflict-handler path over a DynamoDB data source.

Cause 1 of 2: a resolver whose `syncConfig` sets `conflictHandler: LAMBDA` does not hand a condition failure to the Lambda function.
AWS: when the conditional write fails, AppSync invokes the configured Lambda conflict handler with the stored item and the attempted mutation, then honours its verdict — RESOLVE replaces the write with the handler's item, RETRY re-issues the handler's rewritten mapping, DISCARD reports success with the stored item. A repeat of an identical stored item succeeds without the handler being consulted at all.
Emulator: the condition failure is returned straight to the client in the 200 errors envelope as `{'message': 'The conditional request failed', 'errorType': 'DynamoDB:ConditionalCheckFailedException'}` with `data` null. The Lambda is never invoked, so no retry mapping is ever applied and the identical-repeat case is refused too.
Why: AppSync's documented conflict resolution is inert — every concurrent edit loses instead of being merged by the handler, and clients see a raw DynamoDB error where AWS reports success or the handler's result.
Fix: the AppSync resolver's conflict-detection component — the ConditionalCheckFailed path must invoke the data source's configured conflict handler before surfacing the error.
Traces: appsync__dynamodb__lambda.appsync-delete-custom-handler-retry-with-stale-condition-rejected, appsync__dynamodb__lambda.condition-failure-custom-handler-retry-applies-its-retry-mapping, appsync__dynamodb__lambda.condition-failure-differing-put-goes-to-custom-handler-rejected.

Cause 2 of 2: a LAMBDA conflict handler's RESOLVE verdict is accepted for a DeleteItem, where AWS refuses it.
AWS: RESOLVE supplies a replacement item, which only makes sense for PutItem/UpdateItem; a RESOLVE returned for a DeleteItem conflict is refused and the field carries exactly one GraphQL error.
Emulator: the delete mutation succeeds and returns a resolved item — `{'data': {'dropDoc': {'pk': 'doc-...', 'label': 'resolved-stale', '_version': 4}}}` — with no error, so the delete is silently turned into a write.
Why: a conflict handler that misbehaves corrupts data instead of being refused; a row the client asked to delete stays in the table with a resolved payload.
Fix: the conflict-handler verdict validation in the AppSync resolver — RESOLVE must be rejected for DeleteItem.
Traces: appsync__dynamodb__lambda.lambda-conflict-resolve-replaces-put-item-and-is-refused-for-delete-item.

## Evidence
Cause 1: 'a stale edit must be retried with the handler's mapping; got HTTP 200: {"data": {"edit": None}, "errors": [{"message": "The conditional request failed", "errorType": "DynamoDB:ConditionalCheckFailedException"}]}'; 'a repeat of the stored item must succeed without the handler; got HTTP 200: ... ConditionalCheckFailedException'.
Cause 2: 'expected one GraphQL error for RESOLVE on a delete: {"data": {"dropDoc": {"pk": "doc-...", "label": "resolved-stale", "_version": 4}}}'.

## Steps to Reproduce
1. Create a versioned AMAZON_DYNAMODB data source and a resolver with syncConfig conflictDetection VERSION and conflictHandler LAMBDA pointing at a Lambda that returns a RETRY mapping.
2. Issue a stale conditional write — observe the ConditionalCheckFailedException is returned and the Lambda is never invoked.
3. Point the handler at a function that returns RESOLVE and issue a conflicting DeleteItem — observe it succeeds and returns the resolved item.
