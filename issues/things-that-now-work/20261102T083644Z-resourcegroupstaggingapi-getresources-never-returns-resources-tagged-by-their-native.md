Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-bpt-traj-paginated-sweep
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-bpt-traj-paginated-sweep resource-groups-tagging-api-bpt-traj-reference-key-only-sweep resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes resource-groups-tagging-api-bpt-traj-wrong-region-sweep resource-groups-tagging-api-bpt-traj-resource-arn-list-route resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi|GetResources|2xx->2xx|$.PaginationToken
Issue-Key: resourcegroupstaggingapi|GetResources|2xx->2xx|$.ResourceTagMappingList[*]
Issue-Key: resourcegroupstaggingapi|GetResources|2xx->2xx|$.ResourceTagMappingList
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->2xx|$.Message
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->2xx|$.__type
Issue-Key: resourcegroupstaggingapi|GetResources|4xx->2xx|status
Issue-Commit: 46f65a015adce40c0d19696cf4b820bd06724ad6

# GetResources never returns resources tagged by their native service, and ResourceTypeFilters spelling is unvalidated

> _Note: this issue was identified against the code base at commit `46f65a015adce40c0d19696cf4b820bd06724ad6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Six distinct GetResources divergences in the Resource Groups Tagging API, all reported by replay of trace set simparity-wire-aws-v1. Five of them are the resource-discovery path: a resource that was tagged through its OWNING service (here Lambda functions carrying tags) is never returned by GetResources on ANY of its query routes (TagFilters key+value, TagFilters key-only, ResourceARNList, ResourceTypeFilters), so the operation's whole purpose — cross-service tag discovery — is unobservable. The sixth is a missing request-level refusal for an unsupported ResourceTypeFilters value. All six must be closed before this issue is submitted.

Cause 1 of 6 — paginated GetResources returns an empty first page and an empty PaginationToken (fixture resource-groups-tagging-api-bpt-traj-paginated-sweep, step task#4; same cluster also covers task#5).
AWS: GetResources reports every tagged resource in the account/region regardless of HOW the tag was applied; the model's note for ResourceTypeFilters states it explicitly — "All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation". With ResourcesPerPage smaller than the matching set, the response carries the first page in ResourceTagMappingList AND a non-empty PaginationToken that the caller feeds back to get the remainder. Smithy: GetResources is @paginated{inputToken: PaginationToken, outputToken: PaginationToken, items: ResourceTagMappingList, pageSize: ResourcesPerPage}; GetResourcesOutput.PaginationToken -> PaginationToken (length 0..2048). Recorded response: 200 with ResourceTagMappingList[0] = a Lambda function ARN plus its Tags, and a non-empty PaginationToken (the token's characters are opaque per-run values, only its non-emptiness is the requirement).
Emulator: answers 200 with ResourceTagMappingList empty (element 0 missing) and PaginationToken = "" (empty string, i.e. "no more pages").
Why: a paginating client terminates immediately on the empty token and concludes the account holds no tagged resources, so inventory, tag-compliance and cleanup tooling sees nothing to act on even though the tags exist and are readable through the owning service.
Fix: GetResources' handler and the tagging store it reads — the store must index resources tagged through their owning service's tagging operations (not only through TagResources), and the paging code must then mint a continuation token whenever matches remain beyond ResourcesPerPage.
Traces: resource-groups-tagging-api-bpt-traj-paginated-sweep#task#4 (cluster also covers #task#5).

Cause 2 of 6 — the unpaginated TagFilters query returns an empty list where two tagged resources match (fixture resource-groups-tagging-api-bpt-traj-paginated-sweep, step task#6; same cluster also covers resource-groups-tagging-api-bpt-traj-reference-key-only-sweep#task#7).
AWS: a GetResources call with a TagFilters entry naming an existing key and value returns one ResourceTagMapping per matching resource, each with ResourceARN and the resource's full Tags list. Smithy: GetResourcesOutput.ResourceTagMappingList -> ResourceTagMappingList of ResourceTagMapping{ResourceARN, Tags, ComplianceDetails}. Recorded response: 200 with two entries, each a Lambda function ARN and its Tags (the probe-specific ARNs and tag values are per-run values; the requirement is that every resource carrying the filtered tag appears).
Emulator: answers 200 with ResourceTagMappingList[0] and [1] both missing — the list is empty.
Why: the filter route clients use most (match a tag key and value) reports no resources, so a caller cannot locate anything it tagged.
Fix: same component as cause 1 — GetResources' tag-matching read path over the tagging store.
Traces: resource-groups-tagging-api-bpt-traj-paginated-sweep#task#6.

Cause 3 of 6 — a key-only TagFilters entry matches nothing (fixture resource-groups-tagging-api-bpt-traj-reference-key-only-sweep, step task#4; the cluster also covers that fixture's task#5 and task#6, resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#6 and #task#7, and resource-groups-tagging-api-bpt-traj-wrong-region-sweep#task#7 — six steps in all).
AWS: a TagFilter carrying only Key (no Values) matches every resource that has that tag key, whatever its value. Smithy: TagFilter{Key: TagKey, Values: TagValueList} — Values is optional, and the key-only form is the documented "has this tag at all" query. Recorded response: 200 with the matching Lambda function and its complete Tags list (including tags the filter did not name, e.g. a console-set key alongside the probe key).
Emulator: answers 200 with ResourceTagMappingList[0] missing — nothing matches the key-only filter.
Why: the key-only filter is how governance tooling finds resources that merely carry a required tag; an empty answer reads as "no resource has this tag" and drives false compliance conclusions.
Fix: same component as cause 1 — the key-only branch of GetResources' TagFilters evaluation over a store that must include natively-tagged resources.
Traces: resource-groups-tagging-api-bpt-traj-reference-key-only-sweep#task#4.

Cause 4 of 6 — the ResourceARNList route returns nothing, and keeps returning nothing while polled (fixture resource-groups-tagging-api-bpt-traj-resource-arn-list-route, step task#4).
AWS: GetResources with ResourceARNList returns one ResourceTagMapping per listed ARN that exists and is tagged, bypassing filtering entirely. Smithy: GetResourcesInput.ResourceARNList -> ResourceARNListForGet (length 1..100). Recorded response: 200 with one entry per requested ARN, each carrying that resource's Tags.
Emulator: answers 200 with an empty ResourceTagMappingList; the replay polled the query 10 times and observed 0 items where 2 were expected, so this is not eventual-consistency lag but a permanently empty read.
Why: a client that already knows an ARN and asks only for its tags gets an empty answer and cannot distinguish "resource has no tags" from "resource not indexed".
Fix: GetResources' ResourceARNList branch in the operation handler plus the store lookup it performs for a named ARN.
Traces: resource-groups-tagging-api-bpt-traj-resource-arn-list-route#task#4.

Cause 5 of 6 — an unsupported/mis-cased ResourceTypeFilters value is accepted and answered 200 instead of refused 400 InvalidParameterException (fixture resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings, step task#5; the cluster also covers that fixture's task#6 and task#7).
AWS: ResourceTypeFilters entries are 'service[:resourceType]' in the same lower-case spelling the value carries inside a resource ARN; a value whose service token is not a supported, correctly-spelled service is a request-level validation failure refused with HTTP 400, error code InvalidParameterException, and a message of the fixed form "Unsupported service=<service-token> in ResourceTypeFilter - <the whole filter value as sent>". The fixed text is "Unsupported service=" and " in ResourceTypeFilter - "; the service token and the echoed filter value are taken from the request. Smithy: GetResourcesInput.ResourceTypeFilters documents the 'service[:resourceType]' format and that "The string for each service name and resource type is the same as that embedded in a resource's Amazon Resource Name (ARN)"; GetResources models InvalidParameterException (client, HTTP 400). Recorded response: 400, __type InvalidParameterException, Message "Unsupported service=Lambda in ResourceTypeFilter - Lambda:Function".
Emulator: answers HTTP 200 with no __type and no Message, treating the unrecognized filter as simply matching nothing.
Why: a caller that mis-cases or misspells a service token gets a successful, empty result instead of an error, so a typo in a filter silently reads as "no such resources exist" — exactly the failure mode the modeled refusal exists to prevent.
Fix: GetResources' input validation in the operation handler — validate every ResourceTypeFilters entry against the supported-service table before querying, and raise InvalidParameterException (400) with the message above.
Traces: resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings#task#5 (cluster also covers #task#6, #task#7).

Cause 6 of 6 — a correctly spelled ResourceTypeFilters value matches nothing (fixture resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings, step task#8; same cluster also covers resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#10).
AWS: a valid filter such as 'lambda:function' restricts the response to resources of that type, and every tagged resource of that type is returned. Recorded response: 200 with the tagged Lambda function and its Tags.
Emulator: answers 200 with ResourceTagMappingList empty; replay polled 10 times and saw 0 items where 1 was expected.
Why: the type-scoped query — the normal way to enumerate one service's tagged resources — reports none, so even a caller who spells the filter correctly cannot discover resources.
Fix: GetResources' ResourceTypeFilters matching over the tagging store (same store gap as causes 1-4).
Traces: resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings#task#8.

No open issue covers these. The only other open Resource Groups Tagging API trace issue, 20261022T140612Z-resourcegroupstaggingapi-resource-groups-tagging-api-error-contract-diverges-malformed.md, owns the TagResources validation/FailedResourcesMap divergences and the GetResources 'Filters is not allowed when providing a resource list.' message; it says nothing about resource discovery or ResourceTypeFilters validation. The three clusters that issue already owns (resource-arn-list-route#task#8, tag-resources-validation-error-paths#task#4 and #task#5) are deliberately excluded here.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 11 fixtures replayed, 95 steps (66 passed, 29 mismatched, 0 errored, 0 blocked), score 69.5%, emulator_up=true, bootstrap_signal=false. Six of the reported clusters belong to service=resourcegroupstaggingapi / operation GetResources and carry neither stub_path nor owned_by.

Cause 1 — fixture resource-groups-tagging-api-bpt-traj-paginated-sweep step task#4 (affected: #task#4, #task#5), status 200 -> 200:
  $.PaginationToken: value_mismatch (expected a non-empty shaped token, got '')
  $.ResourceTagMappingList[0]: missing (expected {'ResourceARN': 'arn:aws:lambda:<region>:<account>:function:<probe-fn-a>', 'Tags': [{'Key': '<probe-run-key>', 'Value': '<probe-run-value>'}]}, got None)

Cause 2 — fixture resource-groups-tagging-api-bpt-traj-paginated-sweep step task#6 (affected also resource-groups-tagging-api-bpt-traj-reference-key-only-sweep#task#7), status 200 -> 200:
  $.ResourceTagMappingList[0]: missing (expected the <probe-fn-a> mapping, got None)
  $.ResourceTagMappingList[1]: missing (expected the <probe-fn-b> mapping, got None)

Cause 3 — fixture resource-groups-tagging-api-bpt-traj-reference-key-only-sweep step task#4, status 200 -> 200, affected_count 6 (that fixture's task#4/#5/#6, tag-filter-key-and-value-mistakes#task#6/#task#7, wrong-region-sweep#task#7):
  $.ResourceTagMappingList[0]: missing (expected {'ResourceARN': 'arn:aws:lambda:<region>:<account>:function:<probe-fn>', 'Tags': [{'Key': 'lambda-console:blueprint', 'Value': 'true'}, {'Key': '<probe-run-key>', 'Value': '<probe-run-value>'}]}, got None)

Cause 4 — fixture resource-groups-tagging-api-bpt-traj-resource-arn-list-route step task#4, status 200 -> 200:
  $.ResourceTagMappingList[0] and [1]: missing
  $.ResourceTagMappingList: poll: 0 item(s), not 2 after 10 attempt(s) (got [])

Cause 5 — fixture resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings step task#5 (affected: #task#5, #task#6, #task#7):
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected 'Unsupported service=Lambda in ResourceTypeFilter - Lambda:Function', got None)

Cause 6 — fixture resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings step task#8 (affected also resource-groups-tagging-api-bpt-traj-tag-filter-key-and-value-mistakes#task#10), status 200 -> 200:
  $.ResourceTagMappingList[0]: missing
  $.ResourceTagMappingList: poll: 0 item(s), not 1 after 10 attempt(s) (got [])

Model evidence (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json):
  GetResources input: PaginationToken (len 0..2048, pattern ^[\s\S]*$), TagFilters (TagFilterList len 0..50), ResourcesPerPage, TagsPerPage, ResourceTypeFilters (ResourceTypeFilterList), ResourceARNList (ResourceARNListForGet len 1..100); output: ResourceTagMappingList, PaginationToken; errors InternalServiceException(500), InvalidParameterException(400), PaginationTokenExpiredException(400), ThrottledException(400); @paginated{inputToken/outputToken PaginationToken, items ResourceTagMappingList, pageSize ResourcesPerPage}.
  GetResourcesInput.ResourceTypeFilters doc: "The format of each resource type is service[:resourceType]... The string for each service name and resource type is the same as that embedded in a resource's Amazon Resource Name (ARN)... All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation."

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then re-run the bound steps. Every route below first creates a Lambda function carrying tags through the Lambda API (CreateFunction with Tags / TagResource), which is what the probe fixtures do before querying.

Cause 1: resource-groups-tagging-api-bpt-traj-paginated-sweep#task#4 — POST GetResources (X-Amz-Target ResourceGroupsTaggingAPI_20170126.GetResources) with {"TagFilters":[{"Key":"<probe-run-key>","Values":["<probe-run-value>"]}],"ResourcesPerPage":1} after tagging two functions. Expect 200 with one ResourceTagMappingList entry and a non-empty PaginationToken; observe 200 with an empty list and PaginationToken "". Step task#5 of the same fixture is the same cause.

Cause 2: resource-groups-tagging-api-bpt-traj-paginated-sweep#task#6 — same query without ResourcesPerPage. Expect 200 with both function mappings; observe an empty ResourceTagMappingList.

Cause 3: resource-groups-tagging-api-bpt-traj-reference-key-only-sweep#task#4 — POST GetResources with {"TagFilters":[{"Key":"<probe-run-key>"}]} (no Values). Expect 200 listing the tagged function with its full Tags; observe an empty list. The same cause covers that fixture's task#5/task#6, tag-filter-key-and-value-mistakes#task#6/task#7 and wrong-region-sweep#task#7.

Cause 4: resource-groups-tagging-api-bpt-traj-resource-arn-list-route#task#4 — POST GetResources with {"ResourceARNList":["arn:aws:lambda:<region>:<account>:function:<fn-a>","...:<fn-b>"]}. Expect 200 with one mapping per ARN; observe 200 with an empty list, unchanged across repeated polls.

Cause 5: resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings#task#5 — POST GetResources with {"ResourceTypeFilters":["Lambda:Function"]} (mis-cased service token). Expect HTTP 400, __type InvalidParameterException, Message "Unsupported service=Lambda in ResourceTypeFilter - Lambda:Function"; observe HTTP 200 with no error members. task#6/task#7 of the same fixture repeat it with other unsupported spellings.

Cause 6: resource-groups-tagging-api-bpt-traj-resource-type-filter-spellings#task#8 — POST GetResources with {"ResourceTypeFilters":["lambda:function"]}. Expect 200 listing the tagged function; observe 200 with an empty list across repeated polls.
