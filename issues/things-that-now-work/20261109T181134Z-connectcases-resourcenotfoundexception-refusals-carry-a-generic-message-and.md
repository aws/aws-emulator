Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-case-field-values-round-trip-and-update
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-case-field-values-round-trip-and-update connectcases-template-and-case-lifecycle connectcases-case-rule-and-layout-lifecycle connectcases-layout-lifecycle connectcases-related-item-crud-and-search
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|GetCase|4xx->4xx|$.message
Issue-Key: connectcases|GetCase|4xx->4xx|$.resourceType
Issue-Key: connectcases|UpdateLayout|4xx->4xx|$.resourceType
Issue-Key: connectcases|CreateRelatedItem|4xx->4xx|$.message
Issue-Key: connectcases|CreateRelatedItem|4xx->4xx|$.resourceType
Issue-Key: connectcases|CreateCase|4xx->4xx|$.message
Issue-Key: connectcases|CreateCase|4xx->4xx|$.resourceType
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# connectcases ResourceNotFoundException refusals carry a generic message and capitalized resourceType

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCase

## Description
Four net-new wire divergences from the simparity-wire-aws-v1 replay, all of them the SAME fault in one place: the emulator builds every connectcases ResourceNotFoundException from one generic helper, so (a) the `message` is a single fixed sentence 'Requested resource not found.' for every resource class, and (b) `resourceType` is emitted as a capitalized Rust-side type name ('Case', 'Layout', 'Domain') rather than the lowercase resource token AWS sends. The HTTP status (404) and the error code (ResourceNotFoundException) already match in all four; only the body members diverge. THIS ISSUE HAS FOUR CAUSES - close all four before submitting.

Note on an adjacent open issue: 20261030T032733Z-connectcases-getcase-updatecase-on-a-deleted-or-nonexistent.md (behavioral corpus) records the GetCase message only, and against an older emulator wording ('Case <id> was not found'); the emulator has since drifted to 'Requested resource not found.', and that issue names neither the resourceType casing nor UpdateLayout / CreateRelatedItem / CreateCase, which is what the causes below record. It does not cover them.

=== Cause 1 of 4: GetCase on a case the domain does not hold (fixture connectcases-case-field-values-round-trip-and-update, step task#11) ===
AWS: Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): GetCase = POST /domains/{domainId}/cases/{caseId}, errors ResourceNotFoundException (errorKind client, httpError 404); the shape's required members are message, resourceId and resourceType, and resourceType is documented as 'Type of the resource affected.'. The message TEXT and the resourceType token are recorded behavior only: AWS answers 404 / ResourceNotFoundException with resourceType `case` (lowercase) and message `A case with id '<caseId>' could not be found.` - fixed text `A case with id '` + `' could not be found.`, the ONLY variable part being the caseId taken verbatim from the request path label and single-quoted. No domain id and no resource-type word appear in the sentence.
Emulator: status 404 and code ResourceNotFoundException are right; message is `Requested resource not found.` and resourceType is `Case`.
Why: a client that branches on resourceType (lowercase `case` vs `domain` vs `layout`) to decide which identifier was bad sees a token that matches none of its arms, and anything that surfaces or matches the message never sees AWS's sentence, so 'this case was deleted / never existed' handling degrades to a generic error path.
Fix: the shared ResourceNotFoundException constructor in the service's state module plus the GetCase handler's not-found mapping - the message must be chosen PER RESOURCE CLASS (see the other causes: the sentences genuinely differ between classes and even between operations on the same class), and resourceType must carry the lowercase wire token.
Traces: connectcases-case-field-values-round-trip-and-update#task#11 (also connectcases-template-and-case-lifecycle#task#12, same cluster).

=== Cause 2 of 4: UpdateLayout on a layout id that does not exist (fixture connectcases-case-rule-and-layout-lifecycle, step task#16) ===
AWS: Smithy: UpdateLayout = PUT /domains/{domainId}/layouts/{layoutId}, errors include ResourceNotFoundException httpError 404. Recorded behavior: resourceType is the lowercase token `layout`. The message for this class ('Requested resource not found.') already matches, so this cause is the resourceType token alone.
Emulator: 404 / ResourceNotFoundException / message correct, but resourceType `Layout`.
Why: same as Cause 1 - a client keying off resourceType to tell a missing layout from a missing domain on the same URI gets an unrecognized token.
Fix: the lowercase wire token in the layout lookup that raises the error (shared not-found constructor + UpdateLayout handler).
Traces: connectcases-case-rule-and-layout-lifecycle#task#16 (also connectcases-layout-lifecycle#task#12, same cluster).

=== Cause 3 of 4: CreateRelatedItem for an unknown caseId (fixture connectcases-related-item-crud-and-search, step task#10) ===
AWS: Smithy: CreateRelatedItem = POST /domains/{domainId}/cases/{caseId}/related-items/, errors ResourceNotFoundException httpError 404. Recorded behavior: resourceType `case`, message `A case with the specified ID was not found.` - this is entirely FIXED text with NO variable parts (notably it does NOT interpolate the caseId, unlike the GetCase sentence in Cause 1: the two operations word the missing-case refusal differently, so neither one may be reused for the other).
Emulator: 404 / ResourceNotFoundException, message `Requested resource not found.`, resourceType `Case`.
Why: a caller creating a comment/task against a stale case id cannot distinguish 'case gone' from any other not-found, and message-pinned contract tests fail.
Fix: CreateRelatedItem's case-existence check and the shared not-found constructor.
Traces: connectcases-related-item-crud-and-search#task#10.

=== Cause 4 of 4: CreateCase against a domain that does not exist (fixture connectcases-template-and-case-lifecycle, step task#10) ===
AWS: Smithy: CreateCase = POST /domains/{domainId}/cases, errors include ResourceNotFoundException httpError 404. Recorded behavior: resourceType `domain`, message `Domain '<domainId>' does not exist.` - fixed text `Domain '` + `' does not exist.` with one variable part, the domain identifier taken verbatim from the request's {domainId} path label and single-quoted.
Emulator: 404 / ResourceNotFoundException, message `Requested resource not found.`, resourceType `Domain`.
Why: the domain-level failure is indistinguishable from a missing template or case, so retry/provisioning logic that re-creates the domain on this signal never fires.
Fix: CreateCase's domain resolution path plus the shared not-found constructor; the message must name the requested domain identifier.
Traces: connectcases-template-and-case-lifecycle#task#10.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up=true, 182 steps, 117 passed, 56 mismatched, score 64.3%, 22 divergence clusters (all service=connectcases). All four clusters below have empty stub_path and empty owned_by.

Cause 1 - connectcases-case-field-values-round-trip-and-update#task#11, GetCase, expected 404 / actual 404 [mismatch]:
  $.message: expected "A case with id '<caseId>' could not be found." (recorded with the per-run id 7911d80dff8dcb759f755e75764086f0), got 'Requested resource not found.'
  $.resourceType: expected 'case', got 'Case'
  cluster also contains connectcases-template-and-case-lifecycle#task#12.

Cause 2 - connectcases-case-rule-and-layout-lifecycle#task#16, UpdateLayout, expected 404 / actual 404 [mismatch]:
  $.resourceType: expected 'layout', got 'Layout' (message and status already match)
  cluster also contains connectcases-layout-lifecycle#task#12.

Cause 3 - connectcases-related-item-crud-and-search#task#10, CreateRelatedItem, expected 404 / actual 404 [mismatch]:
  $.message: expected 'A case with the specified ID was not found.', got 'Requested resource not found.'
  $.resourceType: expected 'case', got 'Case'

Cause 4 - connectcases-template-and-case-lifecycle#task#10, CreateCase, expected 404 / actual 404 [mismatch]:
  $.message: expected "Domain '<domainId>' does not exist." (recorded with the per-run name sim2real-probe-connectcases-missing-domain-8e074ac0), got 'Requested resource not found.'
  $.resourceType: expected 'domain', got 'Domain'

In every quote above, ids/names are per-run values that fill the template; the requirement is the fixed text and which request value is interpolated.

Smithy: ResourceNotFoundException = structure, errorKind client, httpError 404, required members message + resourceId + resourceType; bound as an error on GetCase, UpdateLayout, CreateRelatedItem and CreateCase.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the connectcases emulator (replay_wire_corpus) and read the four clusters named under Traces. Manually, against the emulator's data listener:
1. CreateDomain(name=<unique>) and poll GetDomain until domainStatus == 'Active'.
2. GetCase(domainId, caseId=<32 hex chars never created>, fields=[{id:'title'}]) -> expect 404, ResourceNotFoundException, resourceType 'case', message "A case with id '<caseId>' could not be found."; emulator returns resourceType 'Case' and message 'Requested resource not found.' (Cause 1).
3. UpdateLayout(domainId, layoutId=<never created>, name=<any>) -> expect 404 with resourceType 'layout'; emulator returns 'Layout' (Cause 2).
4. CreateRelatedItem(domainId, caseId=<never created>, type='Comment', content={comment:{body:'x', contentType:'Text/Plain'}}) -> expect 404, resourceType 'case', message 'A case with the specified ID was not found.'; emulator returns 'Case' / 'Requested resource not found.' (Cause 3).
5. CreateCase(domainId=<name/id of a domain that does not exist>, templateId=<any>, fields=[{id:'title', value:{stringValue:'x'}}]) -> expect 404, resourceType 'domain', message "Domain '<domainId>' does not exist."; emulator returns 'Domain' / 'Requested resource not found.' (Cause 4).
