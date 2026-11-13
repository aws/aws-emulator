Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event
Issue-Key: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event|_routed_request#32|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# codeartifact__events__sqs: the package-version state-change event path cannot be driven because GetRepositoryEndpoint publishes a real-AWS d.codeartifact host the emulator does not serve

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Cause 1 of 1: the cross-service check that a publish and an upstream retention each emit a CodeArtifact Package Version State Change event onto EventBridge (delivered to an SQS queue) is blocked before the first publish, because GetRepositoryEndpoint announces a repositoryEndpoint on a real AWS production authority rather than on an address the emulator serves. The event-emission behavior is therefore entirely unobserved.

AWS: Smithy (com.amazonaws.codeartifact#CodeArtifactControlPlaneService) GetRepositoryEndpoint — GET /v1/repository/endpoint, success 200; required httpQuery `domain`, `repository`, `format` (PackageFormat enum), optional `domain-owner`, `endpointType`; output GetRepositoryEndpointResult$repositoryEndpoint (String), "A string that specifies the URL of the returned endpoint". On real AWS that host is served by AWS, so the URL is the address an npm client publishes through — and publishing through it is what makes CodeArtifact emit the `CodeArtifact Package Version State Change` event (recorded behavior only for the event payload the check then asserts on). The requirement here is the invariant, not a literal string: the announced authority must be one the responder serves; the variable parts (domain, domain owner, region, format, repository) come from the request and state, the fixed parts are the scheme and the /<format>/<repository>/ path shape.

Emulator: repositoryEndpoint comes back as https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/npm/<repository>/<...>, an authority the emulator does not listen on, so the check's first request against it is refused by the harness (PublishedUrlRejected, "its host is not one of the emulator's own hosts") and no publish, no upstream retention and hence no event ever happens.

Why: a client wiring CodeArtifact publishes to EventBridge/SQS cannot reach the publish path at all on this emulator, so it cannot verify that the event is emitted, that it is emitted once per state change, or that its detail fields match. The failure appears as an unreachable host rather than as a missing event.

Fix: same component as the single-service finding — the GetRepositoryEndpoint handler's endpoint/URL synthesis in the codeartifact service crate must publish the host and port the request arrived on (or the per-resource listener it stands up) while keeping the modeled /<format>/<repository>/ path, plus the repository data-plane routing that answers on that authority and the event emission it triggers. Filed separately from the codeartifact-only issue because the check is reported under a different service heading; related open issue 20261109T130849Z-codeartifact-getrepositoryendpoint-announces-a-real-aws-d-codeartifact.md describes the announced-host defect from the wire corpus and does not own this check.

Traces: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event.

## Evidence
[mismatch] codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event
  File "context.py", line 572, in endpoint_route -> _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-ca-dom-<id>-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/sim2real-ca-up-<id>/sim2real-evt-<id> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:44035/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: GetRepositoryEndpoint -> 200 with repositoryEndpoint on a host:port the emulator serves; publishing an npm version through it succeeds, and one `CodeArtifact Package Version State Change` event per state change is delivered to the subscribed SQS queue.
Actual: 200 with repositoryEndpoint on the AWS production authority <domain>-<account>.d.codeartifact.<region>.amazonaws.com; the publish request is never sent, so no event is emitted or delivered.

The host id, account and port above are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator. CreateDomain + CreateRepository (with an upstream/external connection as the check does), create an EventBridge rule for source `aws.codeartifact`, detail-type `CodeArtifact Package Version State Change`, targeting an SQS queue.
2. GetAuthorizationToken(domain=<d>), then GetRepositoryEndpoint(domain=<d>, repository=<r>, format=npm).
3. Observe the returned repositoryEndpoint authority is <d>-<account>.d.codeartifact.<region>.amazonaws.com rather than the host:port the request was sent to.
4. Attempt the npm publish against that URL: it never reaches the emulator, so the queue receives no state-change event.
