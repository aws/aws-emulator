Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-share-tagging
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-share-tagging
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|TagResource|2xx->2xx|$
Issue-Key: ram|UntagResource|2xx->2xx|$
Issue-Commit: 8c779968f63825f959c68d9ef908c64428218fcd

# ram TagResource and UntagResource on a resource share answer 200 with an empty body

> _Note: this issue was identified against the code base at commit `8c779968f63825f959c68d9ef908c64428218fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
RAM's tagging operations on a resource-share ARN are accepted (HTTP 200) and the tags are applied, but the emulator serializes an EMPTY JSON object as the response body. AWS reports what it changed: TagResource echoes the created tags and UntagResource echoes the deleted ones. 2 causes.

Cause 1 of 2 — TagResource returns {} instead of tagsCreated (fixture ram-resource-share-tagging, step task#2).
AWS: TagResource (Smithy: TagResourceRequest.resourceShareArn:String, resourceArn:String, tags:TagList required; http POST /tagresource, success code 200). The response body is recorded behavior only: the on-disk model's TagResourceResponse declares no members, but the recording shows AWS answering 200 with `{"tagsCreated": [ ... ]}`, one Tag entry per tag the call actually created, each carrying the `key` and `value` taken verbatim from the request's tags list (the recorded run shows the single entry it submitted). The rule: the response lists the tags created by this call, with their request-supplied keys and values — not a fixed list.
Emulator: HTTP 200 with body `{}` — no tagsCreated member at all (reported as `$: empty_body`, expected {'tagsCreated': [{'key': ..., 'value': ...}]}, got {}).
Why: a client that confirms a tagging call by reading tagsCreated — to distinguish tags it created from tags that already existed, or to log exactly what changed — sees an empty response and either treats the call as a no-op or crashes dereferencing the absent list.
Fix: the TagResource handler's response serializer for the resource-share path (it must project the tags it just wrote onto the share), and the TagResourceResponse shape, which currently has no member to carry them.

Cause 2 of 2 — UntagResource returns {} instead of deletedTags (fixture ram-resource-share-tagging, step task#4).
AWS: UntagResource (Smithy: UntagResourceRequest.resourceShareArn:String, resourceArn:String, tagKeys:TagKeyList required; http POST /untagresource, success code 200). Response body is recorded behavior only: the recording shows 200 with `{"deletedTags": [ ... ]}`, one entry per key removed, each carrying the removed `key` and NO `value` member. The rule: the keys come from the request's tagKeys list, restricted to those actually present on the target.
Emulator: HTTP 200 with body `{}` (reported as `$: empty_body`, expected {'deletedTags': [{'key': ...}]}, got {}).
Why: a client cannot tell which of the requested keys were actually removed versus absent to begin with, so reconciliation loops re-issue untag calls or report success for keys that were never there.
Fix: the UntagResource handler's response serializer for the resource-share path and the UntagResourceResponse shape.

Related open issue, which does NOT cover these: 20261103T042821Z-ram-tagging-operations-reject-managed-permission-arns-so.md covers the same two operations when the target is a managed-permission ARN (they answer 400 there). These two causes are the resource-share path, which is accepted with 200 and still returns no body.

Traces: ram-resource-share-tagging#task#2; ram-resource-share-tagging#task#4.

## Evidence
Cause 1 — ram-resource-share-tagging#task#2, TagResource, expected 200 / actual 200: $ empty_body — expected {'tagsCreated': [{'__type': 'Tag', 'key': 'cost-center', 'value': '42'}]}, got {}. (The key/value shown are the request's own tag from that run; the requirement is that the submitted tags are echoed.)
Cause 2 — ram-resource-share-tagging#task#4, UntagResource, expected 200 / actual 200: $ empty_body — expected {'deletedTags': [{'__type': 'Tag', 'key': 'team'}]}, got {}. Note the deleted entry carries only `key`, no `value`.

## Steps to Reproduce
1. POST /createresourceshare with tags -> 200.
2. POST /tagresource with resourceShareArn = the share ARN and tags [{key: <k>, value: <v>}] -> emulator answers 200 with body {} instead of {"tagsCreated": [{"key": <k>, "value": <v>}]} (ram-resource-share-tagging#task#2).
3. POST /untagresource with resourceShareArn = the share ARN and tagKeys [<k2>] -> emulator answers 200 with body {} instead of {"deletedTags": [{"key": <k2>}]} (ram-resource-share-tagging#task#4).
4. GetResourceShares confirms the tag mutations themselves did land — only the response bodies are empty.
