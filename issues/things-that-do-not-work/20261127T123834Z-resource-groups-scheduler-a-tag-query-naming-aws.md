Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-groups__scheduler.tag-query-schedule-type-filter-rejected resource-groups__scheduler.tag-query-group-lists-tagged-schedule-group
Issue-Key: resource-groups__scheduler.tag-query-schedule-type-filter-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: resource-groups__scheduler.tag-query-group-lists-tagged-schedule-group|_poll#11|Timeout|time.sleep(2)
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# resource-groups__scheduler: a tag query naming AWS::Scheduler::Schedule is accepted, and a tag-based group on AWS::Scheduler::ScheduleGroup never lists the tagged schedule group

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Two net-new causes in Resource Groups' handling of EventBridge Scheduler resource types.

Cause 1 of 2: CreateGroup accepts a TAG_FILTERS_1_0 query whose ResourceTypeFilters name AWS::Scheduler::Schedule, a type Resource Groups does not support.
AWS: tag-based queries may only name supported resource types. AWS::Scheduler::ScheduleGroup is supported; the individual AWS::Scheduler::Schedule type is not, so CreateGroup is refused with 400 BadRequestException and the message "Query not valid: One or more resource types are not valid: <type>[, <type>…]" — the fixed text is "Query not valid: One or more resource types are not valid: "; the type names are the unsupported entries of the request's ResourceTypeFilters. No group is created, so GetGroup answers NotFoundException. Reference: Resource Groups user guide, supported resources.
Emulator: returns 200 and creates the group, echoing back Group {GroupArn, Name}, the ResourceQuery with Type TAG_FILTERS_1_0 and the unsupported ResourceTypeFilters, and the Tags.
Why: a client that mis-spells or guesses a resource type gets a group that can never match anything, instead of the refusal that names the bad type.
Fix: the resource-groups crate's CreateGroup query validation — check ResourceTypeFilters against the supported-type table.
Traces: resource-groups__scheduler.tag-query-schedule-type-filter-rejected

Cause 2 of 2: a tag-based group whose ResourceTypeFilters name AWS::Scheduler::ScheduleGroup never lists the tagged schedule group, and an ad-hoc SearchResources on the same filter finds nothing either.
AWS: a schedule group carrying the tag the query filters on becomes a member of the group: SearchResources with ResourceQuery {Type TAG_FILTERS_1_0, Query {"ResourceTypeFilters": ["AWS::Scheduler::ScheduleGroup"], "TagFilters": [{"Key": <key>, "Values": [<value>]}]}} returns its ARN, and ListGroupResources on a group created with that query returns exactly {Identifier {ResourceArn: <the schedule group's ARN>, ResourceType: "AWS::Scheduler::ScheduleGroup"}} — and not a schedule group tagged with a different value.
Emulator: the check timed out polling for the membership (a 60 s runner budget inside a 120 s poll), so neither the ad-hoc tag search nor the group's member list ever surfaced the tagged schedule group: Scheduler resources are not indexed for Resource Groups tag queries.
Why: tag-based grouping over Scheduler resources, the only Scheduler type Resource Groups supports, returns nothing, so a client cannot enumerate its schedule groups by tag.
Fix: the resource-groups crate's tag-query resolution — index AWS::Scheduler::ScheduleGroup resources and their tags so TAG_FILTERS_1_0 queries and SearchResources can match them.
Traces: resource-groups__scheduler.tag-query-group-lists-tagged-schedule-group

## Evidence
Cause 1 — resource-groups__scheduler.tag-query-schedule-type-filter-rejected:
  expected BadRequestException "Query not valid: One or more resource types are not valid: AWS::Scheduler::Schedule"
  got      HTTP 200 {"Group": {"GroupArn": "arn:aws:resource-groups:<region>:<account>:group/<name>", "Name": "<name>"}, "ResourceQuery": {"Type": "TAG_FILTERS_1_0", "Query": "{\"ResourceTypeFilters\": [\"AWS::Scheduler::Schedule\"], \"TagFilters\": [{...}]}"}, "Tags": {...}}

Cause 2 — resource-groups__scheduler.tag-query-group-lists-tagged-schedule-group:
  two schedule groups tagged with different values of one key; a group created with ResourceQuery {Type TAG_FILTERS_1_0, Query {"ResourceTypeFilters": ["AWS::Scheduler::ScheduleGroup"], "TagFilters": [{"Key": <key>, "Values": [<value-a>]}]}}
  expected SearchResources to find both schedule groups by their tag values, then ListGroupResources on the group to return exactly {(<group-a ARN>, "AWS::Scheduler::ScheduleGroup")}
  got      the check never got past the barrier poll — "Check timed out after 60.0s" with the tag search returning no schedule groups

## Steps to Reproduce
1. Boot the emulator. Create two schedule groups, tagging one with sim2real-app=<a> and the other with sim2real-app=<b>.
2. Cause 1: resource-groups CreateGroup with ResourceQuery {Type: "TAG_FILTERS_1_0", Query: json of {"ResourceTypeFilters": ["AWS::Scheduler::Schedule"], "TagFilters": [{"Key": "sim2real-app", "Values": ["<a>"]}]}} — expect BadRequestException "Query not valid: One or more resource types are not valid: AWS::Scheduler::Schedule", observe 200 and a created group.
3. Cause 2: resource-groups SearchResources with the same query but ResourceTypeFilters ["AWS::Scheduler::ScheduleGroup"] — expect both schedule-group ARNs; observe none. Then CreateGroup with that query and poll ListGroupResources — expect exactly the <a>-tagged schedule group; observe an empty member list.
