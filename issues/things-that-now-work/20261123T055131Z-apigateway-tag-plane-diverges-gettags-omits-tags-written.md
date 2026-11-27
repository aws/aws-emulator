Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-tagging-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-tagging-lifecycle apigatewayv2-tag-resource-workflow
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetTags|2xx->2xx|$.tags.Stage
Issue-Key: apigateway|GetTags|2xx->2xx|$.tags.Environment
Issue-Key: apigateway|GetTags|2xx->2xx|$.tags.Team
Issue-Key: apigateway|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 42b42c3bdabf251ec917d999bf0a8b3e884f4f35

# apigateway tag plane diverges: GetTags omits tags written after create and the UntagResource 204 omits Content-Length (4 causes)

> _Note: this issue was identified against the code base at commit `42b42c3bdabf251ec917d999bf0a8b3e884f4f35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTags

## Description
This bundle carries FOUR net-new causes on the API Gateway tagging plane, all reported by the replay with service token `apigateway` (API Gateway v1 and v2 share the `apigateway` endpoint prefix, which is why these apigatewayv2-* fixtures are attributed to that token). Three of them are the same shared fault on three different reads — a tag written to a resource AFTER creation is never visible to GetTags — and the fourth is the response framing of the 204 that UntagResource answers. A fixer MUST close all four before submitting.

Cause 1 of 4 — GetTags omits a tag written onto an existing API (fixture apigatewayv2-api-tagging-lifecycle, step task#4).
AWS: Smithy: GetTags = GET /tags/{resourceArn} (v1) / GET /v2/tags/{ResourceArn} (v2), response code 200, output member `tags` (a string-to-string map); TagResource = PUT /tags/{resourceArn} (v1, code 204) / POST /v2/tags/{ResourceArn} (v2, code 201). AWS tagging is add-or-update on the resource's existing collection: a key TagResource names is written (added, or its value replaced) and every other key keeps its value, so the read after the write reports the written key alongside the keys the resource already carried.
Emulator: the GetTags 200 body omits the written key entirely — replay reports `$.tags.<key>: missing (expected '<written value>', got None)`. The 200 status and the response envelope are right; the map simply has no entry for the key that was just written.
Why: a client that tags a resource and reads the tags back sees its write silently lost, with no error ever returned. Anything keyed on tags — cost allocation, tag-based authorization conditions, inventory and cleanup filters, a read-after-write confirmation loop — concludes the tag was never applied.
Fix: the tag-write path of this service and the per-resource tag collection the tag-read path projects; the write must land in the same collection GetTags reads, merged over (never replacing) the tags the resource already carries. Related open issues that do NOT cover this: `20261117T193424Z-apigateway-5-net-new-causes-methodsettings-key-loses.md` (methodSettings key escaping, documentation parts, OpenAPI extensions) and `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md` (204 framing of the Delete* operations only).
Traces: apigatewayv2-api-tagging-lifecycle#task#4.

Cause 2 of 4 — GetTags omits BOTH tags written by a TagResource that named two keys (fixture apigatewayv2-tag-resource-workflow, step task#3).
AWS: as Cause 1 — a single TagResource carrying two entries writes both, and the following GetTags 200 reports both keys with the values that were sent.
Emulator: GetTags answers 200 with neither key present — replay reports `$.tags.<key A>: missing (expected '<value A>', got None)` and `$.tags.<key B>: missing (expected '<value B>', got None)`.
Why: the loss is not a per-key or partial-merge problem — a multi-entry tag write lands nothing at all, so a client batching its tags into one call loses the whole batch.
Fix: as Cause 1.
Traces: apigatewayv2-tag-resource-workflow#task#3.

Cause 3 of 4 — after one of the two keys is removed by UntagResource, GetTags omits the key that should have SURVIVED (fixture apigatewayv2-tag-resource-workflow, step task#5).
AWS: Smithy: UntagResource = DELETE /tags/{resourceArn} (v1) / DELETE /v2/tags/{ResourceArn} (v2), code 204, with `tagKeys` bound to the query string. Untagging removes ONLY the keys the request names; every other key the resource carries is left in place, so the GetTags after it still reports the unnamed key with its value.
Emulator: GetTags answers 200 with the surviving key absent — `$.tags.<surviving key>: missing (expected '<its value>', got None)`.
Why: a client cannot tell a selective tag removal from a wipe. Removing one tag appears to drop the others too, so tag state after any untag is unusable for authorization or allocation decisions.
Fix: as Cause 1 — the same write/read collection; the untag path must subtract exactly the named keys from the collection GetTags projects.
Traces: apigatewayv2-tag-resource-workflow#task#5.

Cause 4 of 4 — the UntagResource 204 response carries no Content-Length header (fixture apigatewayv2-api-tagging-lifecycle, step task#5).
AWS: Smithy: UntagResource DELETE /tags/{resourceArn} (v1) / DELETE /v2/tags/{ResourceArn} (v2), response code 204, output Unit. AWS frames that empty-bodied 204 with an explicit `Content-Length: 0`; the recording asserts the header present with value `0` — the fixed text here is the header name and the only value an empty body can take, with nothing request- or state-derived in it. The status itself is correct in the emulator.
Emulator: the 204 is answered with the `content-length` header absent entirely — replay reports `$headers.content-length: missing (expected '0', got None)`.
Why: a client or intermediary that reads message framing from Content-Length rather than inferring it from the status sees an unframed response, which strict HTTP clients, recording proxies and SDK length checks handle differently from AWS's — a connection can be left waiting on a body that never arrives instead of completing the tag removal.
Fix: this service's response serialization for empty-bodied success responses — the 204 writer must emit `Content-Length: 0` for every operation whose output is Unit, UntagResource included. Related open issues that do NOT cover this operation: `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md`, `20261031T154120Z-apigateway-content-length-0-missing-on-204-deletes-for.md` and `20261123T034529Z-apigateway-response-serialization-gaps-204-delete-framing-and.md` all own Delete* operations (DeleteApiMapping, DeleteAuthorizer, DeleteCorsConfiguration, DeleteDeployment, DeleteAccessLogSettings, DeleteRouteRequestParameter, DeleteRouteSettings); the replay's ownership marking does not place UntagResource under any of them, so this cause is net-new and must be closed here.
Traces: apigatewayv2-api-tagging-lifecycle#task#5 (cluster also covers apigatewayv2-tag-resource-workflow#task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (116 fixtures, 1057 steps, score 86.2%).

Cause 1 — fixture apigatewayv2-api-tagging-lifecycle step task#4, GetTags, expected 200 / actual 200:
  $.tags.Stage: missing (expected 'probe', got None)

Cause 2 — fixture apigatewayv2-tag-resource-workflow step task#3, GetTags, expected 200 / actual 200:
  $.tags.Environment: missing (expected 'probe', got None)
  $.tags.Team: missing (expected 'platform', got None)

Cause 3 — fixture apigatewayv2-tag-resource-workflow step task#5, GetTags, expected 200 / actual 200:
  $.tags.Environment: missing (expected 'probe', got None)

Cause 4 — fixture apigatewayv2-api-tagging-lifecycle step task#5, UntagResource, expected 204 / actual 204:
  $headers.content-length: missing (expected '0', got None)
  (cluster also covers apigatewayv2-tag-resource-workflow#task#4)

Model evidence: smithy_query(operation="GetTags,UntagResource", model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json") — GetTags GET /tags/{resourceArn} code 200 with output member 'tags' (MapOfStringToString); UntagResource DELETE /tags/{resourceArn} code 204, output Unit, tagKeys @httpQuery. The same contract in the v2 model: TagResource POST /v2/tags/{ResourceArn} 201, GetTags GET /v2/tags/{ResourceArn} 200 (jsonName 'tags'), UntagResource DELETE /v2/tags/{ResourceArn} 204.

## Steps to Reproduce
1. git clone the repo and build the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture apigatewayv2-api-tagging-lifecycle and inspect step task#4 — the GetTags 200 body's `tags` map has no entry for the key written by the preceding TagResource step.
4. Cause 2: replay fixture apigatewayv2-tag-resource-workflow step task#3 — neither of the two keys written by the single preceding TagResource call is present in the GetTags 200 body.
5. Cause 3: replay the same fixture's step task#5 — after UntagResource removes one of the two keys, the key it did not name is also absent from the GetTags 200 body.
6. Cause 4: replay fixture apigatewayv2-api-tagging-lifecycle step task#5 — the UntagResource response is 204 but carries no `content-length` response header.
7. Equivalent by hand: create an API, write two tags onto it with TagResource, GET the tags (both missing), untag one key and GET again (the other key is gone too), and inspect the untag response headers (no Content-Length: 0).
