Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.op-contract-delete-api-cache
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.op-contract-delete-api-cache|run#21|AssertionError|assert message3 == "Unable to get the cache as it doesn't exist, please create the cache first.", (
Issue-Commit: 751e4cd59e0742391b3a649cdf34c459ab8203ec

# AppSync ApiCache: a sequential redundant DeleteApiCache no longer returns the API to the never-configured state, so GetApiCache keeps reporting the "has been deleted" sentence (1 cause)

> _Note: this issue was identified against the code base at commit `751e4cd59e0742391b3a649cdf34c459ab8203ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiCache

## Description
One net-new root cause in the AppSync ApiCache teardown state machine, grounded in the single failing behavioral check of this service that is not already owned by an open issue. 1 cause in this issue; it must be closed before the issue is submitted.

Context on what does NOT cover it: the open issue 20261112T005932Z-appsync-apicache-teardown-2-net-new-causes-a.md is the direct neighbour — its Cause 1 asked for the deletion marker to SURVIVE a teardown that removes nothing (the concurrent/racing duplicate delete), and it explicitly named appsync.op-contract-delete-api-cache as the regression guard for the sequential leg ("that leg ... still passes"). That guard now fails: the marker was made unconditionally sticky, so the sequential leg regressed. That issue does not carry this check's replay key and the suite does not mark this failure as owned. The resolved issues 20261029T174359Z-appsync-apicache-wrong-notfoundexception-badrequestexception-messages-for-get.md (absent-cache wording) and 20261030T141024Z-appsync-deleteapicache-is-not-idempotent-a-redundant-delete.md (the redundant delete answering 200) touched this path but are fixed, and neither covers the state transition below: the refusal class and status the emulator returns here are correct, only which of the two absent-cache sentences applies is wrong.

Cause 1 of 1: a redundant DeleteApiCache issued AFTER the teardown has fully settled does not clear the "this API's cache was deleted" state, so the following GetApiCache answers the torn-down sentence where AWS answers the never-configured sentence.
AWS: Smithy (com.amazonaws.appsync#AWSDeepdishControlPlaneService) — DeleteApiCache DELETE /v1/apis/{apiId}/ApiCaches, input apiId (String, @httpLabel, required), empty DeleteApiCacheResponse, success 200; GetApiCache GET /v1/apis/{apiId}/ApiCaches, output apiCache (ApiCache); both model BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Recorded behavior only for the teardown window and for which absent-cache sentence applies: right after a cache that was AVAILABLE is torn down and the teardown settles, GetApiCache is refused 404 NotFoundException with "Unable to get the cache as it has been deleted, please re-create the cache."; a SECOND, sequential DeleteApiCache on that same API (issued once the slot is already empty) is accepted as a no-op (200, empty body), and it returns the API to the never-configured state, so the GetApiCache after it is refused 404 NotFoundException with "Unable to get the cache as it doesn't exist, please create the cache first." In both sentences everything is fixed text except the operation verb ("get" here; "update"/"flush" for the sibling cache operations), which the attempted operation supplies — no request value, apiId, name, id or timestamp appears in either message, so nothing from a single recording is part of the requirement.
Emulator: the sequential redundant DeleteApiCache is accepted (200, empty body — correct), but the terminal GetApiCache is refused 404 NotFoundException with "Unable to get the cache as it has been deleted, please re-create the cache." — the torn-down sentence for an API AWS has returned to the never-configured state. The status (404) and the error code (NotFoundException) match; the message does not. The earlier leg of the same check (GetApiCache straight after the first delete) matched, so only the transition the SECOND delete should perform is missing.
Why: a client cannot tell "your cache was torn down, re-create it" from "caching was never configured on this API" once any cleanup path has run its teardown twice — the normal shape of idempotent cleanup, retried teardown and stack deletion. Remediation logic and negative tests that key on the never-configured wording never fire, and probe-then-provision code is told to re-create a cache on an API that AWS now reports as never having had one. The two sentences are the only signal AppSync gives for this distinction, so after the first redundant delete the emulator's answer is unrecoverably ambiguous.
Fix: the DeleteApiCache handler's empty-slot branch together with the ApiCache state component's deletion marker — a teardown that finds the slot already empty AND settled must clear the marker (returning the API to the never-configured state) while a teardown arriving inside the DELETING window must still leave it set, so both the concurrent leg and this sequential leg hold. Today the marker is written only on removal and cleared only by a create, so the two legs cannot be told apart.
Traces: appsync.op-contract-delete-api-cache. Code sites for orientation: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs lines 208-218 (api.take_api_cache() then the empty 200, with no distinction between "removed an instance" and "slot already empty"); services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs lines 205-224 (take_api_cache sets api_cache_deleted only when it removes an instance and never clears it; api_cache_was_deleted feeds the message choice) and state.rs lines 2923-2954 (cache_not_found_message / cache_deleted_message / absent_cache_message already render both sentences correctly once the marker says which applies).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 199/424 passed, bootstrap_signal false (the emulator serves; the appsync control plane answered every earlier step of this check).

[mismatch] appsync.op-contract-delete-api-cache (GROUP error-semantics, SERVICES=["appsync"]):
  File "check_appsync_op_contract_delete_api_cache.py", line 76, in run
    assert message3 == "Unable to get the cache as it doesn't exist, please create the cache first."
  AssertionError: expected the exact NotFoundException message AWS returns, got 'Unable to get the cache as it has been deleted, please re-create the cache.'

The check reached that assertion, so everything before it matched AWS: CreateGraphqlApi(authenticationType="API_KEY"), CreateApiCache(ttl=300, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL"), the poll to status AVAILABLE, DeleteApiCache (accepted), the poll until GetApiCache stops reporting a cache, GetApiCache refused 404 NotFoundException with "Unable to get the cache as it has been deleted, please re-create the cache." (line 65 of the check — matched), and the redundant sequential DeleteApiCache accepted as a no-op (line 71 — matched).

Expected (AWS), GetApiCache after the redundant delete: HTTP 404, code NotFoundException, message "Unable to get the cache as it doesn't exist, please create the cache first."
Actual (emulator): HTTP 404, code NotFoundException, message "Unable to get the cache as it has been deleted, please re-create the cache."
Fixed text in both sentences is everything but the operation verb ("get"), which the attempted operation supplies; no per-run value (apiId, request id, timestamp) belongs in either.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay simparity-behavioral-aws-v1; appsync.op-contract-delete-api-cache fails on the assertion above.
3. By hand against the emulator's appsync endpoint:
   a. CreateGraphqlApi(name=<unique>, authenticationType="API_KEY") -> note apiId.
   b. CreateApiCache(apiId, ttl=300, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL").
   c. Poll GetApiCache(apiId) until apiCache.status == "AVAILABLE".
   d. DeleteApiCache(apiId) -> expect 200 with an empty body.
   e. Poll GetApiCache(apiId) until it is refused; expect 404 NotFoundException "Unable to get the cache as it has been deleted, please re-create the cache." (this leg already matches).
   f. DeleteApiCache(apiId) again, sequentially, now that the slot is settled and empty -> expect 200 with an empty body (this leg already matches).
   g. GetApiCache(apiId). Expect 404 NotFoundException "Unable to get the cache as it doesn't exist, please create the cache first."; the emulator still answers "Unable to get the cache as it has been deleted, please re-create the cache."
   h. Same check with FlushApiCache/UpdateApiCache in place of the final GetApiCache must carry the same sentence with their own verbs ("flush"/"update").
4. Regression guard for the neighbouring open issue 20261112T005932Z-appsync-apicache-teardown-2-net-new-causes-a.md: with two CONCURRENT DeleteApiCache calls on an AVAILABLE cache, the terminal GetApiCache must still report the torn-down sentence — clearing the marker must be conditional on the teardown having settled, not on the slot merely being empty.
