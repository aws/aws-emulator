Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elasticache.deploy-memcached-cachecluster-configuration-endpoint-snapshot-retention-rejected
Issue-Key: cloudformation__elasticache.deploy-memcached-cachecluster-configuration-endpoint-snapshot-retention-rejected|run#19|AssertionError|assert got == _REJECTED, f"Cluster reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 50a80390bb1e2ddab0fc937c65e59ad0dcd543f4

# cloudformation__elasticache: a memcached AWS::ElastiCache::CacheCluster with SnapshotRetentionLimit is refused with the emulator's own prose instead of ElastiCache's 'Engine does not support snapshotting. Snapshot retention limit parameter should not be specified.'

> _Note: this issue was identified against the code base at commit `50a80390bb1e2ddab0fc937c65e59ad0dcd543f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
ONE cause (Cause 1 of 1), grounded in the single cloudformation__elasticache check the replay report binds to no open issue.

Cause 1 of 1 — CreateCacheCluster for a memcached cluster carrying SnapshotRetentionLimit is correctly refused and the stack correctly rolls back, but the refusal message is the emulator's own sentence rather than ElastiCache's, and it carries none of the service/status/request-id suffix the CloudFormation resource handler reports.
AWS: Smithy com.amazonaws.elasticache#CreateCacheCluster accepts SnapshotRetentionLimit (IntegerOptional) and models InvalidParameterCombinationException / InvalidParameterValueException, both httpError 400 (awsQueryError codes InvalidParameterCombination / InvalidParameterValue); snapshotting is a Valkey/Redis-only feature, so the combination memcached + SnapshotRetentionLimit is refused 400. Recorded behavior: the AWS::ElastiCache::CacheCluster handler surfaces that refusal as a CREATE_FAILED event whose ResourceStatusReason is `Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`, where <message> is ElastiCache's own message plus the SDK's exception suffix: fixed text 'Engine does not support snapshotting. Snapshot retention limit parameter should not be specified. (Service: ElastiCache, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)'. The fixed parts are the two sentences, the literals 'Service: ElastiCache', 'Status Code: 400', 'Request ID:', 'SDK Attempt Count:' and the HandlerErrorCode 'InvalidRequest'; the request id, the request token and the attempt count are per-run values (the harness normalises them to <id>/<n>). The stack ends ROLLBACK_COMPLETE and no cluster exists (DescribeCacheClusters on the name is refused CacheClusterNotFound 'CacheCluster not found: <CacheClusterId>').
Emulator: the handler reason message is 'SnapshotRetentionLimit is only valid for a cluster running valkey or redis, not for memcached.' with HandlerErrorCode 'InvalidRequest' — different fixed text, and the '(Service: ElastiCache, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)' suffix is absent entirely, so nothing in the reason identifies the originating service or the 400. Everything else in the check passed: the positive-control memcached stack reached CREATE_COMPLETE with GetAtt ConfigurationEndpoint.Address matching DescribeCacheClusters, and the bad stack did reach ROLLBACK_COMPLETE.
Why: the CloudFormation failure reason is the only machine-readable account of why a stack rolled back. A client that matches on AWS's wording (or parses out 'Status Code: 400' / the service name to decide retry vs. fix-the-template) sees an unrecognised sentence with no status code, and cannot tell a parameter-combination refusal from any other handler failure; operators comparing emulator and AWS stack events get a different diagnosis for the same template.
Fix: the elasticache CreateCacheCluster validation must emit ElastiCache's message for the unsupported-engine-snapshotting combination, and the CloudFormation AWS::ElastiCache::CacheCluster handler's error wrapping must carry the originating service error through into the resource status reason with the '(Service: ..., Status Code: ..., Request ID: ...) (SDK Attempt Count: ...)' suffix.
Traces: cloudformation__elasticache.deploy-memcached-cachecluster-configuration-endpoint-snapshot-retention-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: cloudformation__elasticache 0/10, nine of the ten failures OWNED by 20261109T151249Z-cloudformation-elasticache-resources-two-types-are-unrecognized-and.md; this is the tenth.

[mismatch] cloudformation__elasticache.deploy-memcached-cachecluster-configuration-endpoint-snapshot-retention-rejected, check file line 332:
'AssertionError: Cluster reason ('SnapshotRetentionLimit is only valid for a cluster running valkey or redis, not for memcached.', 'InvalidRequest'), expected ('Engine does not support snapshotting. Snapshot retention limit parameter should not be specified. (Service: ElastiCache, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')'

The tuple is (handler message, HandlerErrorCode) parsed out of the CREATE_FAILED event for logical id 'Cluster' with the regex `^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$`, with ids normalised. The HandlerErrorCode matches; the message does not, and the emulator's message has no service/status/request-id suffix. Template: a stack-created VPC with two subnets in distinct AZs, a security group, an AWS::ElastiCache::SubnetGroup and an AWS::ElastiCache::CacheCluster (Engine memcached) with SnapshotRetentionLimit: 5; the identical template without that property reached CREATE_COMPLETE in the same check.

Model evidence (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): CreateCacheCluster input member SnapshotRetentionLimit -> IntegerOptional; modeled errors include InvalidParameterCombinationException (httpError 400, awsQueryError InvalidParameterCombination) and InvalidParameterValueException (httpError 400, awsQueryError InvalidParameterValue).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and run suite simparity-behavioral-aws-v1.
2. CreateStack with a template containing an AWS::EC2::VPC, two AWS::EC2::Subnet in distinct AZs, an AWS::EC2::SecurityGroup, an AWS::ElastiCache::SubnetGroup over those subnets and an AWS::ElastiCache::CacheCluster with Engine 'memcached', CacheNodeType 'cache.t3.micro', NumCacheNodes 1, the subnet group, and SnapshotRetentionLimit: 5.
3. Poll DescribeStacks until the stack reaches ROLLBACK_COMPLETE, then read DescribeStackEvents and take the CREATE_FAILED event for the CacheCluster's logical id.
4. Parse its ResourceStatusReason as `Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)`. Expect <code> 'InvalidRequest' and <message> 'Engine does not support snapshotting. Snapshot retention limit parameter should not be specified. (Service: ElastiCache, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)'; observe 'SnapshotRetentionLimit is only valid for a cluster running valkey or redis, not for memcached.' with no suffix.
5. Re-replay cloudformation__elasticache.deploy-memcached-cachecluster-configuration-endpoint-snapshot-retention-rejected; it must also keep passing its other legs (the plain memcached stack reaching CREATE_COMPLETE with GetAtt ConfigurationEndpoint.Address equal to DescribeCacheClusters' value, and CacheClusterNotFound 'CacheCluster not found: <id>' for the rolled-back cluster).
