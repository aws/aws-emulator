Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-replication-management-rejections
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-replication-management-rejections
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|FailoverReplication|4xx->4xx|$.message
Issue-Key: es|FailoverReplication|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|FailoverReplication|4xx->4xx|status
Issue-Commit: 429bdfecbf56545571d755c8b6ad852b4e3ed1bc

# es FailoverReplication applies the AllowDataLoss gate before resolving the domain

> _Note: this issue was identified against the code base at commit `429bdfecbf56545571d755c8b6ad852b4e3ed1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FailoverReplication

## Description
One cause.

Cause 1 of 1: FailoverReplication evaluates its AllowDataLoss business-rule precondition before it resolves the domain named in the request, so a call naming a domain that does not exist is refused as a request-validation fault instead of a missing-resource fault. Fixture opensearch-domain-replication-management-rejections, step task#4.

AWS: Smithy (com.amazonaws.opensearch#FailoverReplication) binds POST /2021-01-01/opensearch/domain/{DomainName}/replication/failover and requires DomainName (@httpLabel, @length min 3 max 28, @pattern ^[a-z][a-z0-9\-]+$), PeerDomainArn (@length 1..512, @pattern ^arn:aws[a-z\-]*:[a-z]+:[a-z0-9\-]+:[0-9]+:domain\/[a-z0-9\-]+$) and AllowDataLoss (boolean). Its modeled errors are DisabledOperationException, InternalException, ValidationException and ResourceNotFoundException, the latter documented as "An exception for accessing or deleting a resource that doesn't exist." The recorded response for a failover against an unresolvable domain is HTTP 404 with header x-amzn-errortype: ResourceNotFoundException and body message "Domain not found: <domain-arn>". The requirement in that message is the fixed text "Domain not found: " followed by the ARN of the domain that could not be resolved, in the form arn:aws<partition>:es:<region>:<account-id>:domain/<DomainName> — the region, account id and domain name are values taken from the request and the caller's identity, not literals of any one run (the recorded run happens to show arn:aws:es:us-west-2:000000000000:domain/sim2real-osr4-8cf4cda6). Note that the on-disk model declares ResourceNotFoundException with httpError 409 while the recording shows 404; the recorded wire status is what AWS serves here and is the requirement.

Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and message "AllowDataLoss must be true to fail over, because writes that had not yet replicated are lost". The replay reports all three facets diverging for this step: status expected 404, got 400; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'ValidationException'; $.message expected 'Domain not found: <domain-arn>', got the AllowDataLoss sentence. Ordering is the root cause: the handler reaches its AllowDataLoss precondition without ever having looked the domain up in state, so whichever request member it dislikes first wins and the absent domain is never reported.

Why: a client branches on the error class, not on the prose. Code that catches ResourceNotFoundException to discover that a replication peer or domain has been deleted (and then recreates it, or stops polling) never sees that class at all; it sees a 400 ValidationException naming a parameter the caller supplied deliberately, so it concludes its own request is malformed, flips AllowDataLoss to true to "fix" it — a semantically dangerous change, since that flag authorises data loss — and only then discovers the domain is gone. Retry/5xx-vs-4xx and 404-vs-400 driven control flow is likewise wrong: a missing resource is reported with the status reserved for a bad request.

Fix: the FailoverReplication handler's precondition ordering (its request-validation/state-resolution sequence). Resolve DomainName against domain state — and the replication relationship it names — and raise ResourceNotFoundException (404, x-amzn-errortype ResourceNotFoundException, message "Domain not found: " + the resolved domain ARN) before any AllowDataLoss or other business-rule gate is evaluated. Only a request whose domain resolves should reach the AllowDataLoss check.

Traces: opensearch-domain-replication-management-rejections#task#4 (FailoverReplication).

## Evidence
Fixture opensearch-domain-replication-management-rejections, step task#4, operation FailoverReplication, service es.

Expected (recorded AWS):
  status: 404
  x-amzn-errortype: ResourceNotFoundException
  $.message: "Domain not found: arn:aws:es:us-west-2:000000000000:domain/sim2real-osr4-8cf4cda6"
    (fixed text "Domain not found: "; the ARN is request/identity-derived: partition, region, account id and the request's DomainName)

Actual (emulator):
  status: 400
  x-amzn-errortype: ValidationException
  $.message: "AllowDataLoss must be true to fail over, because writes that had not yet replicated are lost"

Replay divergence key: service=es|FailoverReplication|status=404->400|keys=$.message,$headers.x-amzn-errortype,status (verdict: mismatch, affected_count 1).

Model evidence: com.amazonaws.opensearch#FailoverReplication errors include ResourceNotFoundException and ValidationException; ResourceNotFoundException is documented "An exception for accessing or deleting a resource that doesn't exist."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or reproduce by hand:
2. Pick a domain name that does not exist in the emulator's state, e.g. a freshly generated name matching ^[a-z][a-z0-9-]+$ of length 3..28.
3. POST /2021-01-01/opensearch/domain/<absent-domain>/replication/failover against the es endpoint with body {"PeerDomainArn": "arn:aws:es:<region>:<account>:domain/<peer>", "AllowDataLoss": false}.
4. Observe HTTP 400 / x-amzn-errortype ValidationException with the AllowDataLoss sentence. AWS answers HTTP 404 / x-amzn-errortype ResourceNotFoundException with message "Domain not found: arn:aws:es:<region>:<account>:domain/<absent-domain>".
5. Fixture replay: opensearch-domain-replication-management-rejections, step task#4.
