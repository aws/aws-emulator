Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction
Issue-Key: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction|run#29|AssertionError|assert json.loads(b)["rid"] == hdrs.get("x-amzn-requestid"), (
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway__dynamodb: the stage cache stores the fully-rendered response instead of the integration response, so a cache hit replays a stale $context.requestId in the body

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: with per-method caching enabled on a stage, a cache HIT is served from a stored FINAL response rather than from the stored INTEGRATION response, so the integration-response mapping template is not re-evaluated for the hit. Anything the template derives from the current request — here $context.requestId — is replayed from the request that populated the entry, while the response's own x-amzn-RequestId header is the current request's id. The two therefore disagree on every hit.

AWS: Smithy: UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}) applies patchOperations to a stage's per-method settings, and the Stage/methodSettings map carries cachingEnabled and cacheTtlInSeconds (exercised here with /~1short/GET/caching/enabled, /~1short/GET/caching/ttlInSeconds, and the same for /~1long/GET); the model says nothing about what is cached. Recorded behavior for that: the stage cache holds the INTEGRATION response, and the integration-response mapping template is re-evaluated per request, so on a cache hit the backend is NOT invoked (the out-of-band DynamoDB invocation counter does not move, and the cached payload value is unchanged) yet $context.requestId renders to THIS request's id — identical to the x-amzn-RequestId header of the very response carrying it. The invariant is the equality between the body's rendered request id and that response's x-amzn-RequestId header, on every hit; the entry is otherwise served unchanged for its cacheTtlInSeconds and evicted by FlushStageCache.

Emulator: on a hit the body carries the request id of the earlier request that populated the entry while the response header carries the current one, e.g. body rid '72670ccb-…' against x-amzn-requestid 'fdf2a5b1-…'. The hit is otherwise correct (the backend was not invoked — the counter was unchanged, which is how the check knew it was a hit), so the defect is precisely that the cached unit is the post-mapping response rather than the integration response.

Why: a client correlating a response body with its response headers or with its own access/execution logs follows a request id that belongs to a different, earlier request — every cached response points its diagnostics at the wrong request, and the misdirection is invisible because the body is otherwise valid JSON. More generally, any mapping template that reads $context (requestId, identity, path, stage variables) or the current request's parameters returns the populating request's values for the whole TTL, so per-request response shaping silently stops working as soon as caching is turned on.

Fix: the execute-api stage cache in the data plane — the cache entry must store the INTEGRATION response (status, headers, payload) keyed by the method's cache key, and the integration-response mapping template (with a fresh $context for the current request) must be evaluated on the hit path as well as the miss path, before the response is serialized.

Traces: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction. Neighbouring cache checks are owned by other issues that do not carry this check's replay key: apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it and apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it are OWNED by 20261107T124627Z-apigateway-3-net-new-causes-updatestage-refuses-a.md, apigateway.flush-caches-bypass-ttl by 20261107T222601Z-apigateway-2-net-new-causes-a-deployed-stage.md, and apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only by 20261110T163347Z-apigateway-2-net-new-causes-five-stage-data.md.

## Evidence
Replay output for apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction:

  File "run.py", line 445, in <lambda>
  File "check_apigateway__dynamodb_deep_apigateway_1_apigw_methodsetting_cache_ttl_expiry_and_flush_eviction.py", line 97, in run
  assert json.loads(b)["rid"] == hdrs.get("x-amzn-requestid"), (
  AssertionError: cache hit body rid '72670ccb-0a5e-4bb2-b0bf-14d610c27b0b' != this response's request id 'fdf2a5b1-f341-4390-84d6-355614394b59'

The assertion is reached only on a confirmed HIT — the check first reads its out-of-band DynamoDB counter and takes this branch only when the counter did NOT move, and it has already asserted the cached payload value (n) is the entry in force. The check states the contract inline: "The cache holds the INTEGRATION response; the response mapping template is re-evaluated per request, so $context.requestId is this request's even on a hit."

Expected on every hit: json.loads(body)["rid"] == response header x-amzn-RequestId (the body's rendered $context.requestId is the CURRENT request's id).
Actual: the body's rid is the id of the request that populated the entry; the header is the current request's id.

(The two uuids above are per-run values and are not themselves the requirement — the requirement is the equality between the body's rendered request id and that same response's x-amzn-RequestId header.)

## Steps to Reproduce
1. Create a REST API with two AWS-integration GET methods, /short and /long, whose integration increments a DynamoDB counter (so backend invocations are countable out of band) and whose 200 integration-response template renders both the counter value and "rid": "$context.requestId".
2. CreateDeployment, then CreateStage(stageName="ttl", cacheClusterEnabled=true, cacheClusterSize="0.5"); wait for cacheClusterStatus AVAILABLE.
3. UpdateStage patchOperations: replace /~1short/GET/caching/enabled=true, /~1short/GET/caching/ttlInSeconds=10, /~1long/GET/caching/enabled=true, /~1long/GET/caching/ttlInSeconds=300. Confirm GetStage methodSettings echoes both TTLs.
4. GET /short until a response arrives WITHOUT the DynamoDB counter moving — that response is a cache hit.
5. Compare json.loads(body)["rid"] with that response's x-amzn-RequestId header.
   Expected: equal (the template was re-evaluated for this request).
   Observed: the body carries the request id of the earlier request that populated the entry.
6. Repeat across the entry's TTL and after FlushStageCache: every hit must render the current request's id.
