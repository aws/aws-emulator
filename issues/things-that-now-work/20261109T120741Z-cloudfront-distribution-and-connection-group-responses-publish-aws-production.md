Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-distribution-etag-chain
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-distribution-etag-chain cfval-caller-reference-reuse-conflict cfval-create-distribution-empty-body cfval-custom-error-response-code-invalid cfval-custom-error-response-error-code-invalid cfval-custom-error-response-page-path-relative cfval-default-root-object-invalid-chars cfval-geo-invalid-location-code cfval-geo-whitelist-empty cfval-query-string-cache-keys-without-query-string cfval-trusted-key-group-does-not-exist cfval-ttl-order-min-exceeds-max cloudfront-distribution-alias-association-error cloudfront-invalidation-workflow cloudfront-create-distribution-with-tags-and-list cloudfront-distribution-create-with-tags-and-inspect cloudfront-distribution-webacl-monitoring-lifecycle cloudfront-domain-and-alias-association-error-paths cloudfront-distribution-create-invalidate-monitor-webacl cloudfront-create-distribution-with-tags-lifecycle cloudfront-distribution-lifecycle-with-tags cfetag-connection-group-etag-chain cloudfront-connection-group-errors cloudfront-connection-group-lifecycle cloudfront-resource-policy-lifecycle-on-connection-group
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DomainName
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$headers.location
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DomainName
Issue-Key: cloudfront|CreateDistributionWithTags|2xx->2xx|$.Distribution.DomainName
Issue-Key: cloudfront|CreateDistributionWithTags|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateConnectionGroup|2xx->2xx|$.ConnectionGroup.RoutingEndpoint
Issue-Key: cloudfront|UpdateConnectionGroup|2xx->2xx|$.ConnectionGroup.RoutingEndpoint
Issue-Related: 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md
Issue-Related: 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# Distribution and connection-group responses publish AWS production domain names the emulator does not serve

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Ten causes, one shared fault: the DNS names CloudFront derives for a distribution (`Distribution.DomainName`) and for a connection group (`ConnectionGroup.RoutingEndpoint`), and the distribution Location URI, are emitted as real AWS production names (`<generated>.cloudfront.net`, `https://cloudfront.amazonaws.com/...`) rather than names the emulator itself answers on. Every one of these clusters fails with verdict `host_not_own`, and in cause 2 the emitted Location is byte-identical to the recorded one and still diverges - so the requirement is that the announced endpoint be the one the emulator is reached on, not that the recorded host be reproduced.

This is the consumer-facing twin of the Location-header bundle filed alongside it; the open issue 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md owns the ConnectionGroup Id/Arn SHAPE but not the RoutingEndpoint host, and no open issue carries the replay keys below.

Cause 1 of 10: CreateDistribution publishes an unserved DomainName and an off-endpoint Location.
AWS: 201 whose `Distribution.DomainName` is the distribution's edge DNS name, resolvable and served by the same deployment that answered the create, and whose Location header is rooted at the service endpoint the request was sent to (Smithy: CreateDistribution output members Distribution (httpPayload) + Location/ETag httpHeaders, http {POST /2020-05-31/distribution, 201}).
Emulator: DomainName is a `*.cloudfront.net` name it does not serve, and Location is rooted at `cloudfront.amazonaws.com`.
Why: DomainName is the address a client fetches content from and the value it pins DNS/CDN assertions to; announcing a host the emulator does not answer means every subsequent edge request leaves the emulator (or fails to resolve), and the create->fetch path can never succeed.
Fix: CreateDistribution's response construction / the distribution domain-name derivation in state - it must mint a name the emulator's own listener answers.
Traces: cfetag-distribution-etag-chain#task#1 (12 affected steps, incl. cfval-create-distribution-empty-body#task#2, cfval-ttl-order-min-exceeds-max#task#2).

Cause 2 of 10: CreateDistribution's Location - the echoed-recording case.
AWS: as cause 1.
Emulator: emits exactly the recorded `https://cloudfront.amazonaws.com/2020-05-31/distribution/<id>` and the step still diverges, i.e. echoing AWS's production authority is itself the defect.
Why/Fix: as cause 1 (Location derivation).
Traces: cfval-caller-reference-reuse-conflict#task#3 (2 affected steps).

Cause 3 of 10: CreateDistribution's DomainName in the invalidation workflow.
AWS/Emulator/Why/Fix: as cause 1 (DomainName only).
Traces: cloudfront-invalidation-workflow#task#1.

Cause 4 of 10: GetDistribution reads back the unserved DomainName.
AWS: 200 whose `Distribution.DomainName` is the served edge name.
Emulator: returns the stored `*.cloudfront.net` name.
Why: the readback is where SDK waiters and client code pick up the domain; the bad value propagates even to clients that never saw the create response.
Fix: the distribution domain-name derivation in state (shared by the readback serializer).
Traces: cfval-create-distribution-empty-body#task#3 (9 affected steps).

Cause 5 of 10: CreateDistributionWithTags publishes the unserved DomainName.
AWS/Emulator/Why/Fix: as cause 1, in CreateDistributionWithTags.
Traces: cloudfront-create-distribution-with-tags-and-list#task#1 (4 affected steps).

Cause 6 of 10: CreateDistributionWithTags publishes both the unserved DomainName and an off-endpoint Location.
AWS/Emulator/Why/Fix: as cause 1, in CreateDistributionWithTags.
Traces: cloudfront-distribution-create-invalidate-monitor-webacl#setup#1.

Cause 7 of 10: CreateDistributionWithTags publishes the unserved DomainName and drops the S3 origin's read timeout.
AWS: as cause 1, and the created distribution's `Origins.Items.Origin.S3OriginConfig` carries `OriginReadTimeout` (Smithy: S3OriginConfig.OriginReadTimeout, integer, 'Specifies how long, in seconds, CloudFront waits for a response from the origin'; the recording shows AWS defaulting it to 30 when the request omits it).
Emulator: DomainName on `*.cloudfront.net` and `S3OriginConfig.OriginReadTimeout` absent from the response.
Why: a client that round-trips the config it just received back into UpdateDistribution loses the origin read timeout; a client that reads it sees no value where AWS always reports one. (The OriginReadTimeout omission alone is also described in 20261029T212223Z-cloudfront-distribution-config-s3-origin-default-omitted-and.md, which does not carry this cluster's replay key.)
Fix: CreateDistributionWithTags' response construction plus the S3OriginConfig default in distribution state.
Traces: cloudfront-create-distribution-with-tags-lifecycle#task#1 (2 affected steps).

Cause 8 of 10: CreateConnectionGroup's RoutingEndpoint (with mis-shaped Id/Arn).
AWS: 201 whose `ConnectionGroup.RoutingEndpoint` is 'the routing endpoint (also known as the DNS name) that is assigned to the connection group' (Smithy: ConnectionGroup.RoutingEndpoint) - a name the serving deployment answers.
Emulator: a `*.cloudfront.net` name it does not serve. (The same cluster's `Id`/`Arn` shape failures are owned by 20261029T212223Z-cloudfront-generated-identifiers-location-uris-and-post-create.md; the RoutingEndpoint cause is net-new.)
Why: distribution tenants are addressed through the connection group's routing endpoint; pointing it at production means tenant traffic never reaches the emulator.
Fix: the connection-group routing-endpoint derivation in state.
Traces: cfetag-connection-group-etag-chain#task#1.

Cause 9 of 10: CreateConnectionGroup's RoutingEndpoint, second cluster.
AWS/Emulator/Why/Fix: as cause 8 (RoutingEndpoint only).
Traces: cloudfront-connection-group-errors#task#1 (3 affected steps).

Cause 10 of 10: UpdateConnectionGroup reads back the unserved RoutingEndpoint.
AWS: 200 with the connection group's served routing endpoint.
Emulator: the stored `*.cloudfront.net` name.
Why/Fix: as cause 8, in UpdateConnectionGroup's response construction / shared state value.
Traces: cloudfront-connection-group-errors#teardown#6.

The specific generated labels in the recorded names are per-run; the requirements are the name's form and that it be an address this deployment answers.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). Ten mismatch clusters, all `host_not_own`:
1. CreateDistribution 201->201 - cfetag-distribution-etag-chain#task#1: $.Distribution.DomainName got 'd0lc535g268ho1.cloudfront.net'; $headers.location got 'https://cloudfront.amazonaws.com/2020-05-31/distribution/E7IYCGPKTANSF3' (12 affected).
2. CreateDistribution 201->201 - cfval-caller-reference-reuse-conflict#task#3: $headers.location expected 'https://cloudfront.amazonaws.com/2020-05-31/distribution/E7IYCGPKTANSF3', got the identical string, verdict host_not_own (2 affected).
3. CreateDistribution 201->201 - cloudfront-invalidation-workflow#task#1: $.Distribution.DomainName got 'd0lc535g268ho1.cloudfront.net'.
4. GetDistribution 200->200 - cfval-create-distribution-empty-body#task#3: $.Distribution.DomainName got 'd0lc535g268ho1.cloudfront.net' (9 affected).
5. CreateDistributionWithTags 201->201 - cloudfront-create-distribution-with-tags-and-list#task#1: $.Distribution.DomainName got 'd0lc535g268ho1.cloudfront.net' (4 affected).
6. CreateDistributionWithTags 201->201 - cloudfront-distribution-create-invalidate-monitor-webacl#setup#1: DomainName + Location both host_not_own.
7. CreateDistributionWithTags 201->201 - cloudfront-create-distribution-with-tags-lifecycle#task#1: DomainName host_not_own AND $.Distribution.DistributionConfig.Origins.Items.Origin.S3OriginConfig.OriginReadTimeout missing (expected '30', got None) (2 affected).
8. CreateConnectionGroup 201->201 - cfetag-connection-group-etag-chain#task#1: $.ConnectionGroup.RoutingEndpoint got 'd0lc535g268ho1.cloudfront.net' (plus Id 'cg_7IYCGPKTANSF3' / matching Arn shape failures owned elsewhere).
9. CreateConnectionGroup 201->201 - cloudfront-connection-group-errors#task#1: RoutingEndpoint got 'd0lc535g268ho1.cloudfront.net' (3 affected).
10. UpdateConnectionGroup 200->200 - cloudfront-connection-group-errors#teardown#6: RoutingEndpoint got 'd0lc535g268ho1.cloudfront.net'.
No stub_path and no owned_by on any of the ten clusters.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay the wire corpus (simparity-wire-aws-v1).
2. Representative step: fixture cfetag-distribution-etag-chain, step task#1 (CreateDistribution) - read Distribution.DomainName and the Location header; both name AWS production hosts, not the emulator's endpoint.
3. GetDistribution the same distribution (cfval-create-distribution-empty-body#task#3) - the same unserved DomainName is read back.
4. CreateDistributionWithTags (cloudfront-create-distribution-with-tags-lifecycle#task#1) - unserved DomainName and the S3 origin's OriginReadTimeout is absent from the response.
5. CreateConnectionGroup / UpdateConnectionGroup (cloudfront-connection-group-errors#task#1, #teardown#6) - ConnectionGroup.RoutingEndpoint is a *.cloudfront.net name the emulator does not answer.
6. Try resolving/requesting any announced name against the emulator: it is not an address this deployment serves.
