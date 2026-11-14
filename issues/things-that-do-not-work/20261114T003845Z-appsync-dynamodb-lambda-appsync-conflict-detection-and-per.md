Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request
Issue-Key: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request|_poll_until#19|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# appsync__dynamodb__lambda: AppSync conflict detection and per-request Lambda authorization never settle — a Lambda conflict handler's REJECT never surfaces ConflictUnhandled and a ttlOverride of 0 never re-invokes the authorizer

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GraphQL (AppSync data-plane POST on the API's GRAPHQL uri)

## Description
Cause 1 of 1: the AppSync data plane never completes a mutation whose resolver has a LAMBDA conflict handler, and never re-runs an AWS_LAMBDA authorizer when its decision is declared uncacheable, so neither behaviour can be observed at all.
AWS: a resolver on a DynamoDB data source with `conflictDetection: VERSION` and `conflictHandler: LAMBDA` invokes the configured conflict-handler function on a version mismatch; when the handler returns `{"action": "REJECT"}` AppSync answers HTTP 200 with the field null and one `errors` entry whose `errorType` is `ConflictUnhandled`, and the stored item is left unchanged (its version does not advance). Independently, a Lambda authorizer that returns `ttlOverride: 0` declares its decision uncacheable, so AppSync invokes the authorizer again on every subsequent request rather than reusing the first decision. Both are documented in the AppSync developer guide (conflict detection and resolution; Lambda authorization); the on-disk appsync Smithy model covers only the control plane, so the data-plane shapes are recorded behavior only.
Emulator: neither check ever reaches its assertion — both exhaust the whole 60 s check budget with no resolution and no conflict-handler/authorizer invocation observable. This is the same inert AppSync data plane reported for the appsync__lambda family, seen here through the DynamoDB-backed resolver and the per-request authorizer.
Why: a client relying on AppSync's optimistic-concurrency contract gets no answer at all: it cannot tell a rejected conflict (item unchanged, ConflictUnhandled) from a successful write, and an authorizer that must run per request is effectively never consulted, so a revoked grant keeps working.
Fix: the AppSync data-plane resolver executor — the DynamoDB data-source path's VERSION conflict detection and LAMBDA conflict-handler dispatch, and the AWS_LAMBDA authorization-mode cache that must honour ttlOverride: 0.
Traces: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged, appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request.

## Evidence
appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged — Error: Check timed out after 60.0s.
appsync__dynamodb__lambda.deep-appsync-2-appsync-authz-ttl-override-zero-reinvokes-authorizer-every-request — Error: Check timed out after 60.0s.
Expected per AWS: the REJECT leg answers HTTP 200 with the field null and errors[0].errorType == "ConflictUnhandled" and the DynamoDB item's _version unchanged; the ttlOverride: 0 leg shows one authorizer invocation per request.

## Steps to Reproduce
1. Create a GraphQL API with a DynamoDB data source, a resolver with conflictDetection VERSION and conflictHandler LAMBDA pointing at a function that returns {"action": "REJECT"}.
2. Write an item, then submit a mutation with a stale _version. AWS answers 200 with errors[0].errorType == "ConflictUnhandled" and leaves the item unchanged; the emulator never answers within the budget.
3. Reconfigure the API for AWS_LAMBDA authorization with an authorizer returning {"isAuthorized": true, "ttlOverride": 0} and issue two queries: AWS invokes the authorizer twice; the emulator never settles.
4. Replay the check ids in trace_ids.
