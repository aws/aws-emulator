Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b1897063ccdb45970c36bccf3e0acb7db093fbab

# The creates store a caller's whole tags map unchecked, so a create bypasses the per-resource tag bound TagResource enforces

_Filed: 2026-11-19T08:04:09Z_
_Operation: CreateLink | Class: validation_

> _Note: this issue was identified against the code base at commit `b1897063ccdb45970c36bccf3e0acb7db093fbab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE per-resource tag bound and enforces it on exactly one of the two paths that write a resource's `tags` map.

`TagResource` checks the bound before it writes, and its own comment says a refused call leaves the record as it was. The creates that write the SAME map — `CreateLink`, `CreateRequesterGateway`, `CreateResponderGateway` and `CreateLinkRoutingRule` — store `input.tags.unwrap_or_default()` with no check at all. The model gives `TagsMap` no member-count constraint (only per-key/per-value length and pattern), so the framework's boundary validator refuses nothing either: a single `CreateLink` carrying 60 tags persists a link whose record holds 60 tags.

Two consequences, both contradicting the crate's own stated rule:
* the invariant `state/tagging.rs:43-44` states ("50 is the bound a single resource's tag map is held to") and the test named `one_resource_holds_at_most_its_quota_of_tags` pins is false for any record a create over-filled — `ListTagsForResource` then reports more tags than the surface says a resource holds;
* the same comment claims the bound "is also what stops a caller growing one record without end, since the keys and values are caller-chosen". With the create path unchecked, one call can grow one record to an arbitrary number of caller-chosen keys (each key up to 128 chars, each value up to 1600), which is the growth the bound exists to stop.

The over-filled record is then stuck: every later `TagResource` on it is refused 400, because `tags.len() + added > TAGS_PER_RESOURCE_QUOTA` is already true. The fix is for the creates to apply the check the crate already has; no response of any currently passing test changes, since no test creates a resource with more than the bound.

## Evidence

The rule, services/service-com-amazonaws-rtbfabric-rtbfabric/src/state/tagging.rs:39-46 — stated as a property of the RESOURCE, not of one operation:
```
/// How many tags one resource holds before `TagResource` refuses to add another.
///
/// AWS's tagging limit is 50 tags per resource, and this service's own
/// `TagKeyList` is bounded at 50 members too
/// (`smithy_query(query="shape", shape="TagKeyList")`), so 50 is the bound a
/// single resource's tag map is held to. It is also what stops a caller growing one
/// record without end, since the keys and values are caller-chosen.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;
```

The side that enforces it, src/ops/tag_resource.rs:160-177:
```
        // Assigning a tag whose key the resource already carries REPLACES that tag's
        // value rather than adding a second one, so what the bound is checked against
        // is how many keys the record would hold afterwards. Checked BEFORE anything is
        // written: a refused call leaves the resource's tags exactly as they were.
        let added = input
            .tags
            .keys()
            .filter(|key| !tags.contains_key(key.as_str()))
            .count();
        if tags.len() + added > crate::state::TAGS_PER_RESOURCE_QUOTA {
            return Err(TagResourceErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "a resource holds at most {} tags",
                    crate::state::TAGS_PER_RESOURCE_QUOTA
                ),
            })
            .into());
        }
```

The side that skips it, src/ops/create_link.rs:274 (and the identical lines in the sibling creates: src/ops/create_requester_gateway.rs:253, src/ops/create_responder_gateway.rs:344, src/ops/create_link_routing_rule.rs:287):
```
            tags: input.tags.unwrap_or_default(),
```

Nothing refuses it earlier: `smithy_query(query="shape", shape="TagsMap")` returns `{"type": "map", "key": {"target": "TagKey", "length": {"min": 1, "max": 128}, ...}, "value": {"target": "TagValue", "length": {"min": 0, "max": 1600}}}` — no constraint on how many entries the map carries, so the framework's boundary validator cannot refuse a 60-entry map.

The invariant the crate's own test asserts, tests/all/resource_tagging.rs:619 and 652-657:
```
async fn one_resource_holds_at_most_its_quota_of_tags() {
...
    assert_eq!(
        listed.len(),
        TAGS_PER_RESOURCE_QUOTA,
        "case {case:?}: expected the resource to still hold {TAGS_PER_RESOURCE_QUOTA} tags, got {}",
```
That test reaches the bound only through `TagResource`; no test creates a resource whose create body carries more tags than the bound, so the create path is uncovered.

## Steps to Reproduce

1. POST /requester-gateway and POST /responder-gateway (bodies with vpcId/subnetIds/securityGroupIds, plus port/protocol/domainName for the responder) to mint a requester id Q and a responder id R.
2. POST /gateway/{Q}/create-link with `peerGatewayId` R, the required `logSettings`, and a `tags` map carrying 60 distinct keys (each key matching ^[a-zA-Z0-9+\-=._:/@]+$).
3. Observed: HTTP 200, and the link record holds all 60 tags — GET /gateway/{Q}/link/{linkId} reports 60 entries under `tags`, and GET /tags/arn:aws:rtbfabric:{region}:{account}:gateway/{Q}/link/{linkId} lists all 60.
4. POST /tags/arn:aws:rtbfabric:{region}:{account}:gateway/{Q}/link/{linkId} with one further tag.
5. Observed: HTTP 400 ValidationException "a resource holds at most 50 tags" — the resource is already past the bound the tagging surface says it holds, and no further tag can ever be assigned to it.
6. Expected (the rule state/tagging.rs:43-44 states): the create applies the same bound before it stores, so no resource is ever persisted holding more tags than TagResource would allow.
