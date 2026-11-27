Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-domain-tag-key-length-and-deleted-domain
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-domain-tag-key-length-and-deleted-domain connecthealth-domain-tags-refused-after-domain-deleted connecthealth-patient-insights-job-read-rejects-malformed-identifiers connecthealth-patient-insights-job-start-rejects-out-of-range-context connecthealth-patient-insights-runtime-refuses-deleted-domain connecthealth-patient-insights-job-unknown-datastore-refused
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|CreateDomain|2xx->2xx|$.createdAt
Issue-Key: connecthealth|CreateDomain|2xx->2xx|$.onboardingGeneration
Issue-Key: connecthealth|GetDomain|2xx->2xx|$.createdAt
Issue-Key: connecthealth|TagResource|4xx->4xx|$.message
Issue-Key: connecthealth|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connecthealth|ListTagsForResource|4xx->4xx|$.message
Issue-Key: connecthealth|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connecthealth|UntagResource|4xx->4xx|$.message
Issue-Key: connecthealth|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# connecthealth domain and tag responses diverge: epoch-microsecond createdAt, LEGACY onboardingGeneration, re-worded tag refusals and bare x-amzn-errortype

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Seven distinct root causes on the connecthealth Domain and resource-tagging surface, all found by replaying trace set simparity-wire-aws-v1 against the emulator built from this checkout. They are bundled because they share one service; each is numbered below and carries its own fixture/step. A fixer must close ALL SEVEN before submitting. None of them is reported as stub-blocked or owned by an open issue. Model citations below are from services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json (ConnectHealth, version 2025-01-29, restJson1, arnNamespace health-agent).

=== Cause 1 of 7: Domain `createdAt` is serialized as a microsecond-precision epoch float, which is not the recorded wire form ===
AWS: Smithy models CreateDomain as POST /domain (code 201) with output member createdAt -> smithy.api#Timestamp (@required), and GetDomain as GET /domain/{domainId} (code 200) with the same createdAt member. Neither carries @timestampFormat, so the restJson1 default (epoch seconds) applies to the JSON body. The exact numeric form is recorded behavior only: the recording's createdAt does NOT match the form the emulator emits, and the replay reports the value as failing the recorded shape rather than merely differing in value (timestamps are per-run values and are never compared literally). Hypothesis: AWS emits health-agent epoch-second timestamps with at most millisecond precision (three fractional digits, e.g. 1795773940.987) or as a whole-second number, never with the six fractional digits the emulator writes; the requirement is the numeric FORM, not any particular instant.
Emulator: both CreateDomain's 201 body and GetDomain's 200 body carry createdAt as a float with microsecond precision — the replay shows `$.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773940.987463)` on CreateDomain and `got 1795774023.2839015` on GetDomain. The status and every other asserted member are correct; only the timestamp's serialized form diverges.
Why: a client that round-trips or re-serializes the field, or that asserts on the service's timestamp granularity, sees a precision AWS never emits; SDKs and schema validators that model the member as epoch-seconds-with-millis can reject or silently truncate it, and any client comparing a Domain's createdAt against a value it stored from a real-AWS response gets a mismatch in the sub-second digits.
Fix: the Timestamp serializer shared by the Domain response projections in the connecthealth crate (it feeds both the CreateDomain and the GetDomain handler) must emit epoch seconds in the recorded form instead of handing out the full sub-second precision of the emulator clock.
Traces: connecthealth-domain-tag-key-length-and-deleted-domain#setup#1 (CreateDomain, representative), connecthealth-domain-tags-refused-after-domain-deleted#setup#1 (CreateDomain; the same cluster also covers connecthealth-patient-insights-job-read-rejects-malformed-identifiers#setup#1, connecthealth-patient-insights-job-start-rejects-out-of-range-context#setup#3 and connecthealth-patient-insights-runtime-refuses-deleted-domain#setup#3), connecthealth-patient-insights-job-unknown-datastore-refused#task#4 (GetDomain).

=== Cause 2 of 7: a newly created Domain reports onboardingGeneration 'LEGACY' instead of 'A2A_STREAMING' ===
AWS: recorded behavior only — the Smithy model's CreateDomainOutput does not declare an onboardingGeneration member at all (its modelled members are domainId, arn, name, kmsKeyArn, encryptionContext, status, webAppUrl, webAppConfiguration, createdAt), so there is no modelled enum to cite. The recording shows CreateDomain answering 201 with onboardingGeneration = 'A2A_STREAMING' for a plainly created domain (no webAppSetupConfiguration-dependent variation is exercised by the fixture), i.e. the value is a fixed string for this class of request, not a per-run value.
Emulator: the same request is answered 201 with onboardingGeneration = 'LEGACY' — `$.onboardingGeneration: value_mismatch (expected 'A2A_STREAMING', got 'LEGACY')`.
Why: onboardingGeneration is how a client decides which onboarding/streaming path a domain supports. A client told 'LEGACY' takes the legacy branch for every domain the emulator mints, so the A2A streaming path is never exercised against the emulator and code written against real AWS behaviour diverges at the first branch after CreateDomain.
Fix: the Domain projection/state default in the connecthealth crate that fills this recorded (unmodelled) member — the value a freshly created domain is stamped with, shared by the CreateDomain and GetDomain projections.
Traces: connecthealth-domain-tag-key-length-and-deleted-domain#setup#1.

=== Cause 3 of 7: TagResource's over-long tag-key refusal names the wrong member and uses the single-member constraint sentence instead of AWS's map-key constraint sentence ===
AWS: Smithy models TagResource as POST /tags/{resourceArn} (code 204) with input tags -> TagMap (@required), whose key targets TagKey @length(min 1, max 128) and whose value targets TagValue @length(0, 256); ValidationException is a client error with httpError 400 and a single `message` member. For a tag key longer than 128 characters the recorded refusal is 400 with the standard AWS validation sentence worded for a MAP KEY violation: `1 validation error detected: Value '{<key>=<value>}' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]`. Fixed text: `1 validation error detected: Value '`, `' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]`, the member path `addedTags` (the service's own name for TagResource's tags input, NOT the wire member name `tags`), and the `{key=value}` rendering of the offending map ENTRY. Variable parts: the offending tag key and its value, taken from the request.
Emulator: answers the right status (400) but a re-worded message — `1 validation error detected: Value '<key>' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128`. Three defects in one sentence: the member path is 'tags' instead of 'addedTags'; the offending value is rendered as the bare key instead of the `{key=value}` entry; and the constraint clause is the scalar-member form instead of AWS's `Map keys must satisfy constraint: [...]` list, which drops the lower-bound (`Member must have length greater than or equal to 1`) clause entirely.
Why: callers and test suites match on this text to tell a key-length violation from a value-length one and to point a user at the offending field; an emulator that blames `tags` and reports only the upper bound sends them to the wrong member and hides that the same constraint also rejects an empty key. The same cluster covers the second attempt in the fixture, so this is the general shape of the refusal, not one input.
Fix: TagResource's validation path in the connecthealth crate — the tag-entry bound check must emit the map-key constraint sentence against the `addedTags` member path with the whole `{key=value}` entry interpolated.
Traces: connecthealth-domain-tag-key-length-and-deleted-domain#task#3 (representative; the cluster also covers #task#4).

=== Cause 4 of 7: TagResource's not-found refusal replaces AWS's fixed sentence with an emulator sentence that echoes the requested ARN ===
AWS: Smithy models ResourceNotFoundException as a client error with httpError 404 and a single `message` member, documented 'This error is thrown when the requested resource is not found.' Recorded behavior: TagResource against the ARN of a domain that has been deleted is refused 404 with message exactly `Resource was not found` — the WHOLE sentence is fixed text, with nothing interpolated from the request or from state.
Emulator: answers 404 but with `Resource not found: arn:aws:health-agent:<region>:<account>:domain/dom-<id>` — different wording ('Resource not found:' vs 'Resource was not found') plus the requested resourceArn appended. The ARN in the recording is a per-run value and is not the point; the point is that AWS's message has no variable part at all and the emulator's does.
Why: a client that branches on the refusal text never matches, and echoing the caller-supplied, unbounded resourceArn back into the message lets one request size the response it is answered with — a property every other refusal on this service holds (see resolved issue 20261029T135949Z-connecthealth-listtagsforresource-error-response.md, which fixed exactly this echo for ListTagsForResource; the same defect is back on all three tagging operations).
Fix: TagResource's resource-resolution path in the connecthealth crate must raise ResourceNotFoundException with the fixed message and no interpolation.
Traces: connecthealth-domain-tag-key-length-and-deleted-domain#task#8 (representative; the cluster also covers connecthealth-domain-tags-refused-after-domain-deleted#task#6).

=== Cause 5 of 7: ListTagsForResource's not-found refusal has the same re-worded, ARN-echoing message ===
AWS: Smithy models ListTagsForResource as GET /tags/{resourceArn} (code 200); ResourceNotFoundException is 404 with a single `message` member. Recorded behavior: for the ARN of a deleted domain the refusal is 404 with message exactly `Resource was not found` — entirely fixed text.
Emulator: 404 with `Resource not found: arn:aws:health-agent:<region>:<account>:domain/dom-<id>` — same wording drift and same echo of the request's resourceArn as Cause 4, on a different handler, so it must be fixed separately (or by single-homing the refusal).
Why: as Cause 4 — text-matching clients miss the refusal, and the message grows with the caller's input.
Fix: ListTagsForResource's resource-resolution path in the connecthealth crate; ideally the three tagging handlers share one not-found constructor so this cannot drift again.
Traces: connecthealth-domain-tags-refused-after-domain-deleted#task#7.

=== Cause 6 of 7: UntagResource's not-found refusal has the same re-worded, ARN-echoing message ===
AWS: Smithy models UntagResource as DELETE /tags/{resourceArn} (code 204) with tagKeys -> TagKeyList (@required, @length 1..50, httpQuery 'tagKeys'); ResourceNotFoundException is 404 with a single `message` member. Recorded behavior: against the ARN of a deleted domain the refusal is 404 with message exactly `Resource was not found`, all fixed text.
Emulator: 404 with `Resource not found: arn:aws:health-agent:<region>:<account>:domain/dom-<id>`.
Why: as Causes 4 and 5.
Fix: UntagResource's resource-resolution path in the connecthealth crate.
Traces: connecthealth-domain-tags-refused-after-domain-deleted#task#8.

=== Cause 7 of 7: every refusal above carries an x-amzn-errortype header whose value does not match the recorded form ===
AWS: recorded behavior only — the model declares the error shapes (ValidationException 400, ResourceNotFoundException 404) and the service's CORS config exposes x-amzn-errortype, but it does not state the header's serialized form. On every refusal above the replay reports the header as failing the recorded shape rather than as absent, so the header IS sent in both and only its value form diverges. Hypothesis: AWS's health-agent restJson1 error responses carry the error type qualified with its shape identity (the `<ErrorCode>:<coral-namespace-URI>` form AWS commonly emits), not the bare code; the requirement is that the header match the recorded form for every error this service raises.
Emulator: sends the bare error-code string — `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')` on the Cause 3 refusal and `got 'ResourceNotFoundException'` on the Cause 4/5/6 refusals.
Why: clients that resolve the modelled error from the header (rather than from the body's `__type`) read a value shaped differently from the one AWS sends; any proxy or SDK that splits the header on ':' to recover the code gets a different result against the emulator than against AWS.
Fix: the connecthealth crate's error serializer (the one place that stamps x-amzn-errortype for restJson1 refusals), not the individual handlers.
Traces: connecthealth-domain-tag-key-length-and-deleted-domain#task#3, connecthealth-domain-tag-key-length-and-deleted-domain#task#8, connecthealth-domain-tags-refused-after-domain-deleted#task#7, connecthealth-domain-tags-refused-after-domain-deleted#task#8.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 117 steps, 93 passed, 24 mismatched, 0 errored, 0 blocked, score 79.5%, 14 divergence clusters. connecthealth is in get_bootstrap_status()'s bootstrapped set. The one non-connecthealth cluster (s3 GetObject, connecthealth-patient-insights-job-completes-with-summary#task#13) is OWNED by 20261120T164159Z-s3-getobject-serves-a-placeholder-summary-object-and.md and excluded. Seven of the remaining clusters are the causes above; the other six are filed as the sibling patient-insights-runtime bundle.

Cause 1 — CreateDomain 201 (expected 201, got 201):
  connecthealth-domain-tag-key-length-and-deleted-domain#setup#1 -> $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773940.987463)
  connecthealth-domain-tags-refused-after-domain-deleted#setup#1 -> $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1795773945.3788216); cluster also covers connecthealth-patient-insights-job-read-rejects-malformed-identifiers#setup#1, connecthealth-patient-insights-job-start-rejects-out-of-range-context#setup#3, connecthealth-patient-insights-runtime-refuses-deleted-domain#setup#3
  GetDomain 200, connecthealth-patient-insights-job-unknown-datastore-refused#task#4 -> $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1795774023.2839015)
  The epoch instants are per-run values; the divergence is the six-fractional-digit float form.

Cause 2 — CreateDomain 201, connecthealth-domain-tag-key-length-and-deleted-domain#setup#1:
  $.onboardingGeneration: value_mismatch (expected 'A2A_STREAMING', got 'LEGACY')

Cause 3 — TagResource 400 (expected 400, got 400), connecthealth-domain-tag-key-length-and-deleted-domain#task#3 (+#task#4):
  expected $.message: "1 validation error detected: Value '{sim2real-limit-kkkk...kkkk=over-limit}' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]"
  actual   $.message: "1 validation error detected: Value 'sim2real-limit-kkkk...kkkk' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128"
  (the 129-character key and the value 'over-limit' are this fixture's inputs; the fixed text is everything around them, including the member path 'addedTags' and the two-clause map-key constraint list.)

Cause 4 — TagResource 404 (expected 404, got 404), connecthealth-domain-tag-key-length-and-deleted-domain#task#8 (+connecthealth-domain-tags-refused-after-domain-deleted#task#6):
  expected $.message: 'Resource was not found'
  actual   $.message: 'Resource not found: arn:aws:health-agent:us-west-2:000000000000:domain/dom-bfa9420ca8cea724'

Cause 5 — ListTagsForResource 404 (expected 404, got 404), connecthealth-domain-tags-refused-after-domain-deleted#task#7:
  expected $.message: 'Resource was not found'
  actual   $.message: 'Resource not found: arn:aws:health-agent:us-west-2:000000000000:domain/dom-ec105002e4e9b41e'

Cause 6 — UntagResource 404 (expected 404, got 404), connecthealth-domain-tags-refused-after-domain-deleted#task#8:
  expected $.message: 'Resource was not found'
  actual   $.message: 'Resource not found: arn:aws:health-agent:us-west-2:000000000000:domain/dom-ec105002e4e9b41e'
  (the region, account and dom- ids in Causes 4-6 are per-run values; the requirement is that the message carry NO interpolated part.)

Cause 7 — x-amzn-errortype on the four refusals above:
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException') on task#3
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException') on the three 404s

Model citations used (services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json): CreateDomain POST /domain code 201, output createdAt -> smithy.api#Timestamp @required, no @timestampFormat, no onboardingGeneration member declared; GetDomain GET /domain/{domainId} code 200 @readonly, errors ResourceNotFoundException 404; TagResource POST /tags/{resourceArn} code 204, tags -> TagMap @required with key TagKey @length(1,128) and value TagValue @length(0,256); UntagResource DELETE /tags/{resourceArn} code 204, tagKeys -> TagKeyList @required @length(1,50); ListTagsForResource GET /tags/{resourceArn} code 200; ValidationException httpError 400 member message; ResourceNotFoundException httpError 404 member message; service ConnectHealth 2025-01-29, restJson1, sigv4Name health-agent, CORS exposes x-amzn-errortype.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or drive a botocore 'connecthealth' client at the emulator's data listener.

Cause 1 + Cause 2 (fixture connecthealth-domain-tag-key-length-and-deleted-domain, setup#1; also connecthealth-domain-tags-refused-after-domain-deleted#setup#1):
1. CreateDomain(name=<unique>) -> 201.
   Expect: createdAt serialized in the recorded epoch-seconds form (no six-digit sub-second tail) and onboardingGeneration 'A2A_STREAMING'.
   Observe: createdAt 1795773940.987463 (microsecond precision) and onboardingGeneration 'LEGACY'.
2. GetDomain(domainId=<that domain>) -> 200 (fixture connecthealth-patient-insights-job-unknown-datastore-refused#task#4): same createdAt form defect.

Cause 3 (connecthealth-domain-tag-key-length-and-deleted-domain#task#3, #task#4):
3. TagResource(resourceArn=<the domain's arn>, tags={<a 129-character key>: <any value>}).
   Expect: 400 ValidationException, message "1 validation error detected: Value '{<key>=<value>}' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]".
   Observe: 400 with "1 validation error detected: Value '<key>' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 128".

Causes 4-6 (connecthealth-domain-tag-key-length-and-deleted-domain#task#8; connecthealth-domain-tags-refused-after-domain-deleted#task#6, #task#7, #task#8):
4. DeleteDomain(domainId) and let the domain reach DELETED.
5. TagResource(resourceArn=<that domain's arn>, tags={k: v}) -> expect 404 message exactly 'Resource was not found'; observe 404 'Resource not found: <the arn>'.
6. ListTagsForResource(resourceArn=<that arn>) -> same expectation, same divergence.
7. UntagResource(resourceArn=<that arn>, tagKeys=[k]) -> same expectation, same divergence.

Cause 7:
8. On each refusal in steps 3 and 5-7, inspect the x-amzn-errortype response header. Expect the recorded qualified form; observe the bare 'ValidationException' / 'ResourceNotFoundException'.

Re-replay the six fixture steps listed in trace_ids (plus the representative connecthealth-domain-tag-key-length-and-deleted-domain#setup#1) to confirm all seven causes are closed.
