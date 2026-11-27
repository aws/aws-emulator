Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one
Issue-Key: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one|_get_resources#14|Timeout|time.sleep(poll_s)
Issue-Commit: 5e8364ffd1cc635503e41df160d30e11335e3874

# resourcegroupstaggingapi__sns: GetResources never discovers an SNS topic tagged through CreateTopic, so a tag-filter inventory reaches no verdict

> _Note: this issue was identified against the code base at commit `5e8364ffd1cc635503e41df160d30e11335e3874`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
One net-new root cause, grounded in the single failing check the suite lists under `### resourcegroupstaggingapi__sns` (1/2, 1 failed), which the replay marked neither ALREADY FILED nor OWNED. Related OPEN issues in the same area that carry other checks' replay keys and so do not cover this one: 20261117T150031Z-ec2-resourcegroupstaggingapi-3-causes-getresources-never-discovers-ec2.md and 20261120T152132Z-ec2-resourcegroupstaggingapi-getresources-never-discovers-an-ec2-internet.md (EC2 resources, not SNS topics).

Cause 1 of 1: GetResources does not index SNS topics, so a topic created with a tag set is never returned for a TagFilter naming that tag's key and value. Failing check: resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one.
AWS:      Smithy: resourcegroupstaggingapi GetResources takes TagFilters (a list of TagFilter{Key, Values}) and answers ResourceTagMappingList of ResourceTagMapping{ResourceARN, Tags}, paginated by PaginationToken; sns CreateTopic takes Tags (a TagList of Tag{Key, Value}) and sns UntagResource removes keys from a topic's tag set. On AWS a topic tagged at CreateTopic is part of the account's tag inventory in that region: GetResources with TagFilters=[{Key: k, Values: [v]}] returns the topic's ARN (tag writes are eventually consistent, so a client polls for it), returns it for no other value of k, and stops returning it once UntagResource removes k. The particular key, value and topic name of a run are not requirements; the match relation is.
Emulator: GetResources answers 200 with an empty ResourceTagMappingList for the key/value the topic was created with. The check paginates GetResources in a bounded poll waiting for the topic's ARN to appear and never sees it, so the check is consumed by that wait and killed at the suite's 60 s cap ("Check timed out after 60.0s") before it can even test the negative cases (a different value must not match, and the match must disappear after UntagResource).
Why:      a client that inventories or governs resources by tag — cost allocation, cleanup, compliance sweeps, `aws resourcegroupstaggingapi get-resources --tag-filters` — sees none of its SNS topics. A sweep built on that answer concludes the account holds no tagged topics and silently skips or mis-reports every one of them, and a cleanup driven by it deletes nothing.
Fix:      the resource-groups tagging inventory behind GetResources — SNS topics (and their tag writes from CreateTopic/TagResource/UntagResource) must be visible to it — and the TagFilters matching that selects by exact key and value.
Traces:   resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one.

## Evidence
Suite replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 574/632 passed (90.8%), bootstrap_signal false. Under `### resourcegroupstaggingapi__sns`: 1/2 (50%), 1 failed, not OWNED and not ALREADY FILED.

  [mismatch] resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one
    Error: Check timed out after 60.0s
    (the check's only unbounded wait is `_get_resources(rgta, key, value)`, which paginates GetResources with TagFilters=[{"Key": key, "Values": [value]}] every 2 s until an ARN appears; it returned an empty set on every poll)

Expected (AWS): after `CreateTopic(Name=<name>, Tags=[{Key: k, Value: v}])`, GetResources with TagFilters=[{Key: k, Values: [v]}] includes the topic's ARN in ResourceTagMappingList; the same filter with a value the topic was never tagged with does not include it; after `UntagResource(ResourceArn=<topic>, TagKeys=[k])` the original filter no longer includes it.
Actual: ResourceTagMappingList stays empty for the matching filter for the whole 60 s budget, so the topic is never discoverable and the two negative assertions are never reached. The key, value and topic name are per-run values.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from that checkout, and replay simparity-behavioral-aws-v1.
2. `sns create-topic --name <name> --tags Key=<k>,Value=<v>` and keep the returned TopicArn.
3. `resourcegroupstaggingapi get-resources --tag-filters Key=<k>,Values=<v>` (paginating). Observed: ResourceTagMappingList is empty on every call for at least 60 s. Expected: it contains the topic's ARN.
4. Repeat with a value the topic was never tagged with and confirm the topic is NOT returned.
5. `sns untag-resource --resource-arn <topic> --tag-keys <k>`, then repeat step 3 and confirm the topic is no longer returned.
6. Re-replay resourcegroupstaggingapi__sns.get-resources-tag-filter-matches-an-sns-topic-by-value-not-a-different-one to confirm the fix.
