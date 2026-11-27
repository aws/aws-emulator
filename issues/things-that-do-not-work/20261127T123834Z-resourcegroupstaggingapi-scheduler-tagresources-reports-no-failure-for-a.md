Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi__scheduler.tagging-api-missing-schedule-group-rejected-in-batch resourcegroupstaggingapi__scheduler.tagging-api-tag-resources-schedule-group
Issue-Key: resourcegroupstaggingapi__scheduler.tagging-api-missing-schedule-group-rejected-in-batch|run#11|AssertionError|assert list(failed) == [missing_arn], (
Issue-Key: resourcegroupstaggingapi__scheduler.tagging-api-tag-resources-schedule-group|_poll#9|Timeout|time.sleep(2)
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# resourcegroupstaggingapi__scheduler: TagResources reports no failure for a schedule-group ARN that does not exist, and a tag written through the Tagging API never reaches the Scheduler resource

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
Two net-new causes in the Resource Groups Tagging API's handling of EventBridge Scheduler resources.

Cause 1 of 2: a ResourceARNList entry naming a schedule group that does not exist is reported as a success — FailedResourcesMap comes back empty.
AWS: TagResources is per-ARN: an ARN whose resource does not exist is reported in FailedResourcesMap keyed by that ARN, with exactly the members StatusCode, ErrorCode and ErrorMessage, where StatusCode is 404, ErrorCode is "ResourceNotFoundException" and ErrorMessage is Scheduler's own not-found sentence "Schedule group <name> does not exist." — fixed text with the group name taken from the ARN. The other ARNs in the same batch still succeed (they are NOT in the map), nothing is created for the failed entry (GetScheduleGroup still answers ResourceNotFoundException with that same message), and the live group in the batch does carry the new tag. Reference: Resource Groups Tagging API, TagResources.
Emulator: FailedResourcesMap is empty — "failures were keyed by 0 ARN(s)" — so the missing schedule group is treated as tagged.
Why: a client batching tag writes gets no signal that part of the batch targeted nothing, so a typo'd or deleted ARN is silently swallowed and the caller believes the tag landed.
Fix: the resourcegroupstaggingapi crate's TagResources per-ARN resolution — resolve each ARN against its owning service and populate FailedResourcesMap with the 404/ResourceNotFoundException entry and the owning service's message.
Traces: resourcegroupstaggingapi__scheduler.tagging-api-missing-schedule-group-rejected-in-batch

Cause 2 of 2: a tag written through the Tagging API on a schedule group is not the tag Scheduler itself reports, and GetResources does not find the group by it.
AWS: TagResources and UntagResources write through to the owning service's tag store, so the tag is immediately the one scheduler:ListTagsForResource reports on that ARN (alongside the tags created with the group), rgt:GetResources with TagFilters [{Key, Values}] and ResourceTypeFilters ["scheduler"] returns exactly that ARN with its whole tag set, and UntagResources removes it again from Scheduler's view.
Emulator: the check never converged — it timed out while polling scheduler:ListTagsForResource for the key the Tagging API had just written (the call itself reported no failures), so the Tagging API's write does not reach the Scheduler resource.
Why: the two tag views disagree, so tag-driven inventory, cost allocation and tag-based access control over Scheduler resources are inconsistent depending on which API wrote the tag.
Fix: the same per-ARN path in the resourcegroupstaggingapi crate — write and remove tags in the owning service's tag store (and index them so GetResources can match them) rather than in a separate one.
Traces: resourcegroupstaggingapi__scheduler.tagging-api-tag-resources-schedule-group

## Evidence
Cause 1 — resourcegroupstaggingapi__scheduler.tagging-api-missing-schedule-group-rejected-in-batch:
  TagResources(ResourceARNList=[<live schedule-group ARN>, <same ARN form with a name never created>], Tags={<key>: <value>})
  expected FailedResourcesMap keyed by exactly the missing ARN, entry {StatusCode: 404, ErrorCode: "ResourceNotFoundException", ErrorMessage: "Schedule group <missing name> does not exist."}
  got      "expected exactly the missing group to fail; failures were keyed by 0 ARN(s), live included: False"

Cause 2 — resourcegroupstaggingapi__scheduler.tagging-api-tag-resources-schedule-group:
  TagResources(ResourceARNList=[<schedule-group ARN>], Tags={"sim2real-costcentre": <value>}) returned FailedResourcesMap {}
  expected scheduler ListTagsForResource on that ARN to report {"sim2real": <check>, "sim2real-costcentre": <value>}, and rgt GetResources(TagFilters=[{Key: "sim2real-costcentre", Values: [<value>]}], ResourceTypeFilters=["scheduler"]) to return that ARN
  got      the key never appeared in Scheduler's ListTagsForResource — "Check timed out after 60.0s" while polling for it

## Steps to Reproduce
1. Boot the emulator. scheduler CreateScheduleGroup with Tags [{Key: sim2real, Value: <v>}] and note its ARN.
2. Cause 1: build a second ARN identical in form but with a group name never created. resourcegroupstaggingapi TagResources(ResourceARNList=[<live ARN>, <missing ARN>], Tags={"sim2real-owner": "<v>"}) — expect FailedResourcesMap keyed by the missing ARN with StatusCode 404, ErrorCode ResourceNotFoundException and ErrorMessage "Schedule group <missing name> does not exist."; observe an empty FailedResourcesMap.
3. Cause 2: TagResources(ResourceARNList=[<live ARN>], Tags={"sim2real-costcentre": "<v>"}), then poll scheduler ListTagsForResource(ResourceArn=<live ARN>) — expect the new key present; observe it never appears. Then GetResources with that TagFilter and ResourceTypeFilters ["scheduler"] and UntagResources, both of which cannot be reached.
