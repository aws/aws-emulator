Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# The three substring recommended-action filters (CurrentResourceDetailsEbsVolumeType, ResourceTagsKey, ResourceTagsValue) are matched by exact equality against one field, so a documented partial or cross-field value selects nothing

_Filed: 2026-11-27T21:48:02Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RecommendedActionFilter$values` documents three filter names whose matching is explicitly NOT equality against a single field:

* `CurrentResourceDetailsEbsVolumeType` matches when the value appears ANYWHERE in the current resource details — volume type, size, IOPS and throughput — and a PARTIAL value matches.
* `ResourceTagsKey` and `ResourceTagsValue` both search EVERY tag key and tag value, and a partial value matches too (the model's own example is a `ResourceTagsKey` value matching a resource's tag VALUE).

`crate::state::recommended_action_matches_filters` implements all three as whole-string equality against one field: `CurrentResourceDetailsEbsVolumeType` is compared against `ebs_volume_type(...)` alone (the configuration's `type`, never the size, IOPS or throughput) through the shared `filter.values.contains(&value)` tail, `ResourceTagsKey` is compared only against `tag.key` and `ResourceTagsValue` only against `tag.value`.

So a request the model says selects rows is answered with an empty page instead: `CurrentResourceDetailsEbsVolumeType=["gp"]` matches no gp2/gp3 volume, `CurrentResourceDetailsEbsVolumeType=["200"]` matches no volume of 200 GiB, `ResourceTagsKey=["production"]` matches no resource tagged `environment=production`, and `ResourceTagsKey=["env"]` matches no resource whose tag key is `environment`.

The same helper is the one `ListRecommendedActionSummaries` selects with (`src/ops/list_recommended_action_summaries.rs:297`), so that listing's count and savings total are wrong for the same requests.

## Evidence

Model (`smithy_query(query="shape",shape="RecommendedActionFilter")`, member `values`):
- "CurrentResourceDetailsEbsVolumeType — Matches when the value appears anywhere in the current resource details of the recommended action, which include the volume type, size, IOPS, and throughput. Matching is case sensitive, and a partial value also matches. For example, `gp` matches both `gp2` and `gp3` volumes, and `100` matches any volume whose size, IOPS, or throughput contains 100."
- "ResourceTagsKey and ResourceTagsValue — Matches when the value appears anywhere in the tags of the resource. Both filter names search every tag key and tag value. Matching is case sensitive, and a partial value also matches. For example, a `ResourceTagsKey` value of `test` matches a resource that has a tag value of `test`."

Handler/helper, exact equality on one field only — `src/state.rs:4176-4178`:
```
            "CurrentResourceDetailsEbsVolumeType" => {
                ebs_volume_type(action.current_resource_details.as_ref())
            }
```
`src/state.rs:4219`: `        field.is_some_and(|value| filter.values.contains(&value))`
`src/state.rs:2105-2107` — only the volume TYPE is ever read, never size/IOPS/throughput:
```
fn ebs_volume_type(details: Option<&ResourceDetails>) -> Option<String> {
    ebs_configuration(details)?.r#type.clone()
}
```
`src/state.rs:4206-4215` — each tag filter searches ONE side of the tag, by whole-string equality:
```
            "ResourceTagsKey" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.key)
                });
            }
            "ResourceTagsValue" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.value)
                });
            }
```
Callers of that helper: `src/ops/list_recommended_actions.rs:285` and `src/ops/list_recommended_action_summaries.rs:297`.

Test coverage: `tests/all/recommended_actions.rs:351-353` states the rule the implementation follows — "Each `RecommendedActionFilterName` selects on the field it names, a filter matches when the field equals ANY of its values" — and every case it drives is a whole-value equality case (`tests/all/recommended_actions.rs:419-442`: `CurrentResourceDetailsEbsVolumeType` `["gp2"]`/`["io1"]`, `ResourceTagsKey` `["env"]`/`["team"]`, `ResourceTagsValue` `["prod"]`). No case drives a partial value, a size/IOPS/throughput value, or a `ResourceTagsKey` value that occurs as a tag VALUE, so nothing pins the documented behaviour; `tests/all/recommended_action_summaries.rs:697-706` repeats the same three equality cases for the summary listing.

The seeded inventory this is reachable on — `src/state.rs:1488-1530` — carries volumes of type `io1`/`gp2`/`io2` with sizes 200/150/300/1000/80 GiB and tags `("environment", "production")`, `("environment", "staging")`, `("team", "search")`, `("environment", "sandbox")`.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} — the calling account's analysed inventory (five EBS volumes, types io1/gp2/io2/gp2/io1, sizes 200/150/300/1000/80 GiB, tags environment=production, environment=staging + team=search, environment=sandbox) is recorded.
2. ListRecommendedActions {} — note the five actions, their currentResourceDetails.ebsVolume.configuration (type, sizeInGib, iops) and their resourceTags.
3. ListRecommendedActions {"filters":[{"name":"CurrentResourceDetailsEbsVolumeType","values":["gp"]}]} — documented: "a partial value also matches ... `gp` matches both `gp2` and `gp3` volumes", so the two gp2 volumes are selected. Actual: recommendedActions is empty.
4. ListRecommendedActions {"filters":[{"name":"CurrentResourceDetailsEbsVolumeType","values":["200"]}]} — documented: the value matches "anywhere in the current resource details ... volume type, size, IOPS, and throughput", so the 200 GiB volume is selected. Actual: empty — only the volume type is read.
5. ListRecommendedActions {"filters":[{"name":"ResourceTagsKey","values":["production"]}]} — documented: "Both filter names search every tag key and tag value ... a `ResourceTagsKey` value of `test` matches a resource that has a tag value of `test`", so the environment=production volume is selected. Actual: empty.
6. ListRecommendedActions {"filters":[{"name":"ResourceTagsKey","values":["env"]}]} — documented partial match of the tag key `environment`. Actual: empty.
7. ListRecommendedActionSummaries with any of the same filters — the single aggregate row reports recommendedActionCount 0 for the same reason, because it selects with the same helper.
