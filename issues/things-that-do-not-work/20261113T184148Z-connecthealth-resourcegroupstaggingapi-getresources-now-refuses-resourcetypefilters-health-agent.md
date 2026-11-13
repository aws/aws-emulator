Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|GetResources|InvalidParameterException
Issue-Commit: 2ea9db7899fab28ca5d17dc0ef7cfb8f8ea8eca4

# connecthealth: resourcegroupstaggingapi GetResources now REFUSES ResourceTypeFilters=["health-agent"] with InvalidParameterException instead of returning the tagged Domain

> _Note: this issue was identified against the code base at commit `2ea9db7899fab28ca5d17dc0ef7cfb8f8ea8eca4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
One net-new root cause found by replaying simparity-behavioral-aws-v1 against the emulator built from this checkout. It is the only failing check under '### connecthealth' that is not already OWNED by an open issue.

Cause 1 of 1: the Resource Groups Tagging API read path hard-refuses a connecthealth (health-agent) resource-type filter, so a Domain tagged through connecthealth TagResource can never be discovered cross-service (failing check: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi).

AWS: ResourceTypeFilters is a free-form service/resource-type filter, not a closed allow-list. Smithy (resourcegroupstaggingapi-20170126) GetResourcesInput.ResourceTypeFilters documents the format as `service[:resourceType]` and states "The string for each service name and resource type is the same as that embedded in a resource's Amazon Resource Name (ARN)", and "All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation." The connecthealth service model (com.amazonaws.connecthealth#ConnectHealth) declares aws.api#service arnNamespace/endpointPrefix/sigv4Name = "health-agent", so a Domain ARN is arn:<partition>:health-agent:<region>:<account>:... and "health-agent" is exactly the service token that filter takes. The only InvalidParameterException the model documents for this input is the ResourceArnList+filters conflict ("You can't specify both this parameter and the ResourceArnList parameter in the same request. If you do, you get an Invalid Parameter exception.") — a service name is never refused for being unrecognised; an unmatched filter yields HTTP 200 with an empty ResourceTagMappingList. Recorded behavior: GetResources(TagFilters=[{Key,Values:[v]}], ResourceTypeFilters=["health-agent"]) answers 200 and, within a bounded propagation window, a ResourceTagMapping whose ResourceARN is the Domain ARN and whose Tags carry the key/value just written; after UntagResource the same query stops returning it.

Emulator: connecthealth's own tag plane is correct (TagResource then ListTagsForResource returns exactly the written tag — that assertion passes), but GetResources rejects the request outright: HTTP 400 InvalidParameterException with message "Unsupported service=health-agent in ResourceTypeFilter - health-agent". In that message the whole text is emulator-authored (AWS has no such refusal); the only request-derived part is the filter value "health-agent". The check now ERRORS on the first GetResources call instead of polling, i.e. the handler validates ResourceTypeFilters against a closed list of known service tokens that omits health-agent.

Note this is NOT the same observable behavior as open issue 20261107T174733Z-connecthealth-behavioral-surface-2-divergences-tagged-domains-are.md, which records the same check failing because GetResources answered 200 with an empty ResourceTagMappingList (an indexing gap). The call is now refused before any lookup happens, so that issue's replay key no longer matches and the suite does not mark this check as owned; closing this one requires both removing the refusal AND indexing connecthealth Domain tags, so re-replay the check against both issues.

Why: a client doing cross-service tag discovery (inventory, cost allocation, tag-compliance sweeps, bulk cleanup) gets a 400 client error for a perfectly legal filter. Unlike an empty result it is not recoverable by retry or by widening the window — SDK callers raise InvalidParameterException and abort the sweep, and callers that pass several service filters in one array lose the whole request rather than the one service. It also hides the underlying index gap behind a validation error.

Fix: the GetResources handler's input validation / resource-type filter table in the resourcegroupstaggingapi service — stop rejecting unrecognised service tokens (only the modelled ResourceArnList-plus-filters conflict and the per-item length bound are refusals; an unknown/unmatched filter must answer 200 with an empty ResourceTagMappingList), and register "health-agent" so it resolves to connecthealth Domains; plus the connecthealth TagResource/UntagResource write path that must publish a Domain's tags into the shared tagging index.

Traces: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 15/26 passed (57.7%), 8 mismatched, 3 errored, 0 skipped, bootstrap_signal=false. Failing checks are listed under '### cloudformation__connecthealth' (2) and '### connecthealth' (9); 10 of the 11 are marked OWNED by existing issues and are excluded here. The one remaining failure is this cause.

Check connecthealth.domain-tags-visible-via-resourcegroupstaggingapi [error]:
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the GetResources operation: Unsupported service=health-agent in ResourceTypeFilter - health-agent

Sequence exercised by the check:
  1. connecthealth CreateDomain(name=<unique>) -> domainId, arn (succeeds).
  2. connecthealth TagResource(resourceArn=<arn>, tags={"sim2real-probe": <value>}) (succeeds).
  3. connecthealth ListTagsForResource(resourceArn=<arn>) -> {"sim2real-probe": <value>} — this assertion PASSES, so the producer-side tag plane is fine.
  4. resourcegroupstaggingapi GetResources(TagFilters=[{"Key":"sim2real-probe","Values":[<value>]}], ResourceTypeFilters=["health-agent"]) -> HTTP 400 InvalidParameterException "Unsupported service=health-agent in ResourceTypeFilter - health-agent". Expected: HTTP 200 with a ResourceTagMappingList entry whose ResourceARN == <arn> and whose Tags contain sim2real-probe=<value>.
  5. Never reached: UntagResource then the same GetResources query must stop returning the ARN.

Model evidence:
  - services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json, GetResourcesInput.ResourceTypeFilters doc: format `service[:resourceType]`; "The string for each service name and resource type is the same as that embedded in a resource's Amazon Resource Name (ARN)"; "All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation"; the only documented InvalidParameterException on this input is ResourceTypeFilters together with ResourceArnList.
  - services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json, com.amazonaws.connecthealth#ConnectHealth aws.api#service: sdkId "ConnectHealth", arnNamespace "health-agent", endpointPrefix "health-agent", sigv4Name "health-agent" — so "health-agent" is the ARN-embedded service token for Domain ARNs.

The domain name, domain id, ARN and tag value in the run (sim2real-conh-dom-* / sim2real-conh-tagval-*) are per-run values and are not part of the requirement; the requirement is the status, the absence of the refusal, and the ARN/Tags content of the returned mapping.

## Steps to Reproduce
Run the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout and re-replay connecthealth.domain-tags-visible-via-resourcegroupstaggingapi. Manually:
1. connecthealth CreateDomain(name=<unique name>) -> note domainId and arn (arn:<partition>:health-agent:<region>:<account>:...).
2. connecthealth TagResource(resourceArn=<arn>, tags={"sim2real-probe": "<value>"}).
3. connecthealth ListTagsForResource(resourceArn=<arn>) -> {"sim2real-probe": "<value>"} (passes today).
4. resourcegroupstaggingapi GetResources(TagFilters=[{"Key":"sim2real-probe","Values":["<value>"]}], ResourceTypeFilters=["health-agent"]).
   Expect: HTTP 200; within a bounded propagation window (the check polls every 3s for 60s) a ResourceTagMappingList entry with ResourceARN == <arn> and Tags containing sim2real-probe=<value>. An unmatched filter must still be 200 with an empty list.
   Observe: HTTP 400 InvalidParameterException "Unsupported service=health-agent in ResourceTypeFilter - health-agent" on the very first call.
5. connecthealth UntagResource(resourceArn=<arn>, tagKeys=["sim2real-probe"]), then repeat step 4: the entry must be absent while the call still answers 200.
6. Sanity check that the refusal is a closed allow-list and not specific to tagging: GetResources(ResourceTypeFilters=["health-agent"]) with no TagFilters must also answer 200.
