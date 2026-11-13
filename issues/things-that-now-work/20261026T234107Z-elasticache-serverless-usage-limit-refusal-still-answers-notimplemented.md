Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.serverless-cache-usage-limits-inverted-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.serverless-cache-usage-limits-inverted-rejected|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), (
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# ElastiCache serverless usage-limit refusal still answers NotImplemented: CreateServerlessCache with an inverted CacheUsageLimits bound never reaches its modeled 400 (1 cause)

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServerlessCache

## Description
REOPENS 20261025T020531Z-elasticache-serverless-cache-usage-limits-modifyserverlesscache-is-still.md: its fix left the call refused.

One cause, grounded in the single surviving failing check of this run. Every other failing elasticache / ec2__elasticache / dynamodb__ec2__elasticache / elasticache__logs / elasticache__sns check in this replay was marked ALREADY FILED (blocked by the unimplemented EC2 CreateRouteTable / CreateInternetGateway scaffold stubs) or OWNED by an open issue (20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md, 20261023T155010Z-elasticache-read-validate-fidelity-unknown-engine-accepted-events.md, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md) and was skipped.

Cause 1 of 1: a CreateServerlessCache whose CacheUsageLimits invert Minimum and Maximum is answered with error code NotImplemented (the scaffold's HTTP 501) instead of the modeled HTTP 400 client refusal, so the semantic usage-limit contract is still unobservable end to end.
AWS: Smithy: CreateServerlessCache models InvalidParameterValueException {errorKind client, httpError 400, awsQueryError code InvalidParameterValue}; CacheUsageLimits.DataStorage{Minimum,Maximum,Unit} and CacheUsageLimits.ECPUPerSecond{Minimum,Maximum} are plain IntegerOptional with NO @range trait (confirmed by querying shapes CacheUsageLimits / DataStorage / ECPUPerSecond), so no client-side validation exists and a contradictory Minimum > Maximum envelope reaches the service, which refuses it with that modeled 400. Recorded behavior for the message: the fixed sentence template "Invalid values for <Member>. Minimum must not be greater than Maximum." — the fixed text is "Invalid values for ", ". Minimum must not be greater than Maximum."; the only variable part is the member name taken from the request's offending limit (DataStorage or ECPUPerSecond). ModifyServerlessCache models the same InvalidParameterValueException (400 / InvalidParameterValue) and must refuse the same contradiction without mutating the live cache.
Emulator: the check's first CreateServerlessCache (Engine=valkey, CacheUsageLimits={DataStorage:{Minimum:50,Maximum:10,Unit:GB}, ECPUPerSecond:{Minimum:1000,Maximum:5000}}, SubnetIds from the harness network fixture) is answered with error code NotImplemented, i.e. the scaffold's 501 refusal, not a 400. The check asserts `expected InvalidParameterValueException, got NotImplemented` and dies on that first call, so neither the ECPU inversion, the valid-create control case, nor any ModifyServerlessCache assertion is ever reached. Both ElastiCache handlers on this path are now implemented and already carry the rule: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_serverless_cache.rs calls crate::state::validate_cache_usage_limits and returns InvalidParameterValueException, and services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_serverless_cache.rs is implemented and merges+validates the limits before writing — so the 501 is emitted elsewhere on this call's path, not by the ElastiCache operation itself. Hypothesis: the check builds SubnetIds inside the same expression as the refused call (`SubnetIds=ctx.network().subnet_ids[:3]`), so an EC2 scaffold stub reached while standing up that network fixture raises the ClientError the check's helper catches first; this same suite reports EC2 CreateInternetGateway as an unimplemented stub blocking 42 checks (open issue 20261022T202249Z-ec2-createinternetgateway-stub.md) and EC2 CreateRouteTable likewise. Neither of those issues carries this check's replay key, and neither states the requirement below, so this is not covered by them.
Why: a client that sends a semantically contradictory usage-limit envelope gets a 501 instead of a 400. 501 is not a modeled error, is not classified as a client fault by any SDK's error handling, and is not retried, so a caller cannot distinguish "your request is invalid" from "this service is unavailable", and the whole serverless usage-limit negative path (including the guarantee that a refused ModifyServerlessCache leaves the live cache's limits byte-identical and Status available) cannot be validated against the emulator at all. The previous fix added the rule to both handlers but left the call still refused with NotImplemented, so the behavior a client sees has not changed.
Fix: whichever operation on this check's path still answers the scaffolded 501 must answer its modeled response instead, so that CreateServerlessCache's existing inversion rule is actually reachable: identify the operation emitting NotImplemented on this request sequence before changing code (the ElastiCache CreateServerlessCache/ModifyServerlessCache handlers already hold the validation and need no further change), then implement that operation's handler — if it is the EC2 network scaffold, the fix belongs in the EC2 crate's ops for that operation and this cause closes by pointing at it; if any ElastiCache wire path still routes CreateServerlessCache through the scaffold error channel in the elasticache crate's request walk, that routing is the defect.
Traces: elasticache.serverless-cache-usage-limits-inverted-rejected (its first _assert_error, "CreateServerlessCache with DataStorage Minimum=50 > Maximum=10").

## Evidence
Failing check elasticache.serverless-cache-usage-limits-inverted-rejected [mismatch], from this replay of suite simparity-behavioral-ga-prod-v1 (29/104 passed):

  File "check_elasticache_serverless_cache_usage_limits_inverted_rejected.py", line 157, in run
    _assert_error(
      lambda: ec.create_serverless_cache(
        ServerlessCacheName=<fresh name>, Engine="valkey",
        CacheUsageLimits={"DataStorage": {"Minimum": 50, "Maximum": 10, "Unit": "GB"},
                          "ECPUPerSecond": {"Minimum": 1000, "Maximum": 5000}},
        SubnetIds=ctx.network().subnet_ids[:3]),
      "InvalidParameterValueException",
      "Invalid values for DataStorage. Minimum must not be greater than Maximum.",
      "CreateServerlessCache with DataStorage Minimum=50 > Maximum=10")
  File "check_elasticache_serverless_cache_usage_limits_inverted_rejected.py", line 55, in _assert_error
    assert _fault_base(code) == _fault_base(expected)
  AssertionError: CreateServerlessCache with DataStorage Minimum=50 > Maximum=10: expected InvalidParameterValueException, got NotImplemented

Expected: HTTP 400, error code InvalidParameterValue / InvalidParameterValueException (modeled on CreateServerlessCache, awsQueryError code InvalidParameterValue, httpError 400), message "Invalid values for DataStorage. Minimum must not be greater than Maximum." (fixed text with the member name as its only variable part). The same is expected for ECPUPerSecond Minimum=9000 > Maximum=1000, which the check asserts next, and for the ModifyServerlessCache half (400 InvalidParameterValue with limits unchanged and Status still "available"; 404 ServerlessCacheNotFoundFault after delete).
Actual: error code NotImplemented (the scaffold's HTTP 501 "operation ... is scaffolded but not implemented in this emulator"), raised on the first CreateServerlessCache expression, which aborts the check.

Code state in this checkout (showing the refusal is NOT the ElastiCache operation's own validation):
- services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_serverless_cache.rs lines 4-5 declare IMPLEMENTED (no SCAFFOLDED marker); lines 283-287 call crate::state::validate_cache_usage_limits(limits) and return invalid_value(...) -> InvalidParameterValueException (lines 525-532).
- services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_serverless_cache.rs lines 4-5 declare IMPLEMENTED; lines 292-302 merge the requested limits onto the stored ones and validate before any write, returning InvalidParameterValueException.
- services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs line 4 still carries "SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented"; this suite's report attributes 42 blocked checks to it.

Model evidence: smithy_query(operation="CreateServerlessCache") -> errors include InvalidParameterValueException {errorKind: client, httpError: 400, awsQueryError: {code: InvalidParameterValue, httpResponseCode: 400}}; smithy_query(shape="DataStorage,ECPUPerSecond") -> Minimum/Maximum are IntegerOptional with no range constraint (Unit is a required DataStorageUnit enum).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay elasticache.serverless-cache-usage-limits-inverted-rejected.
2. Call CreateServerlessCache with Engine=valkey, a fresh ServerlessCacheName, SubnetIds taken from the harness network fixture (the check uses ctx.network().subnet_ids[:3], which stands up a VPC/subnets/gateway first), and CacheUsageLimits={"DataStorage":{"Minimum":50,"Maximum":10,"Unit":"GB"},"ECPUPerSecond":{"Minimum":1000,"Maximum":5000}}. Observe error code NotImplemented (HTTP 501); expect HTTP 400 InvalidParameterValue with message "Invalid values for DataStorage. Minimum must not be greater than Maximum.".
3. Capture the wire exchange for step 2 and identify WHICH operation returned the 501 (the ElastiCache CreateServerlessCache call itself, or an EC2 operation such as CreateInternetGateway/CreateRouteTable reached while building the network fixture). Do this before changing any code.
4. Repeat step 2 with hard-coded SubnetIds that already resolve, to confirm whether CreateServerlessCache alone answers the modeled 400: expect 400 InvalidParameterValue; a NotImplemented here localizes the defect to the ElastiCache request path.
5. Repeat with CacheUsageLimits={"DataStorage":{"Minimum":1,"Maximum":10,"Unit":"GB"},"ECPUPerSecond":{"Minimum":9000,"Maximum":1000}}: expect 400 InvalidParameterValue, message "Invalid values for ECPUPerSecond. Minimum must not be greater than Maximum.".
6. Then create a cache with valid limits (DataStorage 1-10 GB, ECPUPerSecond 1000-5000), poll DescribeServerlessCaches to Status=available, call ModifyServerlessCache with CacheUsageLimits={"ECPUPerSecond":{"Minimum":9000,"Maximum":1000}} and expect 400 InvalidParameterValue with the cache's limits byte-identical and Status still "available"; then apply a legal widening (DataStorage Maximum=20, ECPUPerSecond Maximum=9000) and expect it to take effect.
