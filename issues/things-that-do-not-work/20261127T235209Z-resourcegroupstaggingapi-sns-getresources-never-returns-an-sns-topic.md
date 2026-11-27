Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one
Issue-Key: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one|_get_resources#14|Timeout|time.sleep(poll_s)
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# resourcegroupstaggingapi__sns: GetResources never returns an SNS topic for the tag it was created with, so tag-based discovery of topics finds nothing

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: Resource Groups Tagging API GetResources does not report SNS topics by their tags.
  AWS: recorded behavior only (the Tagging API's resource coverage is not expressed in the Smithy model). A topic created with `CreateTopic(Name=..., Tags=[{Key:K, Value:V}])` is discoverable through `GetResources(TagFilters=[{Key:K, Values:[V]}])`: the topic's ARN appears in `ResourceTagMappingList[].ResourceARN` (within the check's 60 s convergence budget), does NOT appear for a filter naming a value the topic was never tagged with, and stops appearing once `UntagResource` removes the key. Reference used by the recording: https://docs.aws.amazon.com/sns/latest/dg/sns-tags.html.
  Emulator: the matching GetResources call returns no mapping for the topic at all — the check polls GetResources with the exact Key/Value for the full budget and the topic ARN never appears, so the check exhausts its 60 s and is reported `Error: Check timed out after 60.0s`. The negative and post-UntagResource legs are never reached because the positive leg never converges.
  Why: any client that discovers resources by tag (cost/ownership inventories, cleanup sweeps, `aws resourcegroupstaggingapi get-resources` driven tooling) sees an account with no SNS topics, so it silently skips or deletes nothing where AWS would list the topic; a tag filter's negative answer is also indistinguishable from the emulator's blanket empty answer.
  Fix: the resourcegroupstaggingapi GetResources handler and the tag index it reads — SNS topic tags written by CreateTopic/TagResource must be registered in the cross-service tag inventory GetResources queries, and withdrawn by UntagResource.
  Traces: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one.

## Evidence
Expected: after CreateTopic(Name=N, Tags=[{Key:K,Value:V}]), GetResources(TagFilters=[{Key:K,Values:[V]}]) includes the topic ARN; GetResources with a never-applied value excludes it; after UntagResource(TagKeys=[K]) it is excluded again. Actual: the paginated GetResources(TagFilters=[{Key:K,Values:[V]}]) poll returns an empty ResourceTagMappingList for the whole 60 s budget — the check ends `Error: Check timed out after 60.0s` on its first (positive) assertion, with the emulator having answered every GetResources call with no matches.

## Steps to Reproduce
1. sns.CreateTopic(Name=<unique>, Tags=[{Key:'<unique-key>', Value:'<unique-value>'}]).
2. resourcegroupstaggingapi.GetResources(TagFilters=[{Key:'<unique-key>', Values:['<unique-value>']}]) (paginate) — expect the topic ARN in ResourceTagMappingList; observe an empty list however long it is polled.
3. GetResources with the same key and a different value — expect the topic absent.
4. sns.UntagResource(ResourceArn=<topic>, TagKeys=['<unique-key>']), then repeat step 2 — expect the topic absent.
