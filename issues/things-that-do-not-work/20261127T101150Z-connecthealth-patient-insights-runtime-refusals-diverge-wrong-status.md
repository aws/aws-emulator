Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-patient-insights-job-read-rejects-malformed-identifiers
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-patient-insights-job-read-rejects-malformed-identifiers connecthealth-patient-insights-runtime-refuses-deleted-domain connecthealth-patient-insights-job-start-rejects-out-of-range-context connecthealth-patient-insights-job-unknown-datastore-refused
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|GetPatientInsightsJob|4xx->4xx|$.message
Issue-Key: connecthealth|GetPatientInsightsJob|4xx->4xx|$.Message
Issue-Key: connecthealth|GetPatientInsightsJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connecthealth|GetPatientInsightsJob|4xx->4xx|status
Issue-Key: connecthealth|StartPatientInsightsJob|4xx->4xx|$.Message
Issue-Key: connecthealth|StartPatientInsightsJob|4xx->4xx|$.message
Issue-Key: connecthealth|StartPatientInsightsJob|4xx->4xx|status
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# connecthealth patient-insights runtime refusals diverge: wrong status class, echoed sensitive values and re-worded messages

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPatientInsightsJob

## Description
Six distinct root causes in the connecthealth patient-insights runtime surface (the two operations bound to hostPrefix 'runtime.': GetPatientInsightsJob and StartPatientInsightsJob), found by replaying trace set simparity-wire-aws-v1 against the emulator built from this checkout. They are bundled because they share one service; each is numbered below and carries its own fixture/step. A fixer must close ALL SIX before submitting. None is stub-blocked or owned by an open issue — the open issue 20261114T103624Z-connecthealth-3-net-new-causes-in-startpatientinsightsjob-clienttoken.md is behavioral and covers clientToken dedup, unseeded-patient acceptance and the missing summary object; it carries none of these replay keys and none of these causes. Model citations are from services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json.

=== Cause 1 of 6: the over-long jobId refusal echoes the rejected identifier, which AWS withholds ===
AWS: Smithy models GetPatientInsightsJob as GET /domain/{domainId}/patient-insights-job/{jobId} (code 200, @readonly, hostPrefix 'runtime.'), with jobId -> JobId @required @httpLabel @length(min 1, max 36); ValidationException is a client error with httpError 400 carrying a single `message` member. Recorded behavior for a jobId longer than 36 characters: 400 with message exactly `1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must have length less than or equal to 36` — note that AWS withholds the offending VALUE here and writes `Value at 'jobId'`, not `Value '<id>' at 'jobId'`. Fixed text: the whole sentence; variable parts: the member path (`jobId`) and the bound (36), both taken from the model.
Emulator: same status (400) but the message interpolates the rejected identifier — `1 validation error detected: Value 'sim2real-probe-job-702499ba602ad5e5ac' at 'jobId' failed to satisfy constraint: Member must have length less than or equal to 36` (that literal id is this fixture's input; the defect is that ANY value is spliced in).
Why: a client matching the refusal text never matches, and the emulator reflects a caller-supplied path label back into an error body where AWS deliberately does not — so the error body's size is driven by the request, and an identifier that may carry patient-linkable content is logged by anything that logs the message.
Fix: GetPatientInsightsJob's input validation in the connecthealth crate — the jobId length check must emit the value-less `Value at '<member>'` form.
Traces: connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#3 (representative).

=== Cause 2 of 6: a malformed domainId on the runtime endpoint is refused 400 ValidationException instead of 403, and its message carries an SDK-style diagnostic tail ===
AWS: Smithy models domainId -> DomainId @required @httpLabel @length(20,25) @pattern ^(hai-|dom-)[a-z0-9]+$ on GetPatientInsightsJob; the modelled errors are AccessDeniedException 401, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429 and ValidationException 400 — the model declares NO 403 shape, so the recorded status is recorded behavior only. Recorded behavior: a domainId that does not match the DomainId grammar (e.g. a 'res-' prefixed id) is refused by the runtime endpoint's authorization layer with HTTP 403 and a body whose `Message` member is exactly `Domain is not valid.` — the whole sentence is fixed text with nothing interpolated. Hypothesis: the runtime host resolves the domain before the operation's own input validation runs, so a grammar failure surfaces as an authorization refusal rather than a ValidationException.
Emulator: answers HTTP 400 with x-amzn-errortype ValidationException and `Message` = `res-0a1b2c3d4e5f6a7b8c9d is not a valid domain id. (Service: HealthAgent, Status Code: 400, Request ID: 4251ee5b-488f-4a1a-9b0d-d05c1b66f27e) (SDK Attempt Count: 1)`. Three defects: the wrong status class (400 instead of 403), a different sentence that interpolates the rejected id, and an AWS *client-SDK* diagnostic tail (`(Service: ...) (Status Code: ...) (Request ID: ...) (SDK Attempt Count: 1)`) baked into a server-side message — that suffix is something a Java SDK appends when rendering an exception, never something a service puts on the wire. (The id and request id above are per-run values.)
Why: a client's retry/auth handling keys on the status class: 403 routes to credential/authorization handling, 400 to input correction. The emulator sends every caller down the wrong branch, and the embedded SDK tail makes the message unmatchable and leaks a per-attempt counter into a wire field.
Fix: GetPatientInsightsJob's handler in the connecthealth crate — resolve/authorize the domain label ahead of operation validation and refuse an ill-formed domainId with 403 and the fixed message, and strip the SDK-style suffix from whatever error renderer produced it.
Traces: connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#4.

=== Cause 3 of 6: a well-formed but unknown domainId is refused 404 ResourceNotFoundException instead of 403 'Domain cannot be found for this account.' ===
AWS: as Cause 2 for the modelled errors. Recorded behavior: GetPatientInsightsJob for a domainId that matches the DomainId grammar but is not allocated to the calling account is refused HTTP 403 with `Message` exactly `Domain cannot be found for this account.` — entirely fixed text, nothing interpolated. AWS deliberately does not confirm whether the domain exists; the account-scoped 403 is the point.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and `Message` = `Domain not found: dom-0a1b2c3d4e5f6a7b8c9d0` — wrong status class, wrong sentence, and it echoes the requested domain id.
Why: the emulator turns an authorization refusal into a resource-existence answer, which both breaks status-class-based client handling and converts the refusal into an existence oracle for domain ids the caller does not own. Clients written against AWS treat 403 here as 'not mine / check credentials' and 404 as 'the job is gone'; the emulator inverts that.
Fix: the runtime-endpoint domain resolution shared by GetPatientInsightsJob in the connecthealth crate must answer an unresolvable-but-well-formed domainId 403 with the fixed message rather than mapping it onto the modelled 404.
Traces: connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#5 (representative; the cluster also covers connecthealth-patient-insights-runtime-refuses-deleted-domain#task#8).

=== Cause 4 of 6: StartPatientInsightsJob's unknown-datastore refusal drops the sentence-final period ===
AWS: Smithy models StartPatientInsightsJob as POST /domain/{domainId}/patient-insights-job (code 200, hostPrefix 'runtime.'), inputDataConfig -> InputDataConfig @required (carrying fhirServer.fhirEndpoint); ValidationException is 400. Recorded behavior: when the HealthLake datastore named by inputDataConfig.fhirServer.fhirEndpoint cannot be resolved, the refusal is HTTP 400 with `Message` exactly `HealthLake datastore not found.` — a complete sentence, terminal period included, with no interpolated part.
Emulator: answers 400 with `HealthLake datastore not found` — the same words without the trailing period.
Why: the message text is a requirement, and this is the single most-replayed refusal in the corpus (one cluster covering five steps across three fixtures). Every client or test that compares the refusal to AWS's documented sentence fails on a one-character difference, and the mismatch is invisible in logs, so it costs debugging time rather than being caught.
Fix: the datastore-resolution refusal in StartPatientInsightsJob's validation path in the connecthealth crate — reproduce AWS's sentence verbatim, including the period.
Traces: connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#4 (representative; the cluster also covers connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#6 and #task#7, connecthealth-patient-insights-job-unknown-datastore-refused#task#3 and connecthealth-patient-insights-runtime-refuses-deleted-domain#task#5).

=== Cause 5 of 6: a constraint violation on the @sensitive encounterReason is answered with a detailed constraint message instead of AWS's generic 'Request validation failed' ===
AWS: Smithy models StartPatientInsightsJob input encounterContext -> PatientInsightsEncounterContext @required, whose only member encounterReason targets SensitiveNonEmptyString and is @required with @length(min 1, max 256) and @pattern ^[a-zA-Z0-9 .,-]+$; the shape is @sensitive. ValidationException is 400 with a single `message` member. Recorded behavior: an encounterReason longer than 256 characters is refused 400 with message exactly `Request validation failed` — the whole sentence is fixed text, and AWS discloses NEITHER the offending value nor the member path, which is consistent with the member being sensitive (contrast Cause 1 and Cause 4, where the service does name the member).
Emulator: answers 400 but with the generic constraint sentence it uses elsewhere — `1 validation error detected: Value at 'encounterContext.encounterReason' failed to satisfy constraint: Member must have length less than or equal to 256`, disclosing the member path for a sensitive member.
Why: this is the inverse of Cause 1: the emulator applies one validation-message template everywhere, so it is too terse where AWS is specific and too detailed where AWS is deliberately opaque. A client matching the refusal text never matches, and the emulator trains callers (and their log scrubbers) to expect member paths for a sensitive clinical free-text field that AWS keeps out of error bodies.
Fix: StartPatientInsightsJob's validation path in the connecthealth crate must collapse violations on @sensitive members of the request body to the fixed `Request validation failed` message instead of rendering the per-member constraint sentence.
Traces: connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#5.

=== Cause 6 of 6: StartPatientInsightsJob against a DELETED domain answers 401 'Domain is not valid.' instead of 403 'Domain cannot be found for this account.' ===
AWS: modelled errors for StartPatientInsightsJob are AccessDeniedException 401, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429 and ValidationException 400; DomainStatus is an enum ACTIVE|DELETING|DELETED. Recorded behavior: starting a job on a domain whose deletion has completed is refused HTTP 403 with `Message` exactly `Domain cannot be found for this account.` — the same fixed, account-scoped sentence as Cause 3: once deleted, the domain is simply not resolvable for the account, so the refusal is identical to the never-existed case.
Emulator: answers HTTP 401 with `Message` = `Domain is not valid.` — the wrong status class (401, the modelled AccessDeniedException, instead of the recorded 403) AND the wrong sentence (it reuses the malformed-grammar wording of Cause 2 for a domain whose id is perfectly well-formed but gone).
Why: a client cannot distinguish 'your credentials are wrong' (401) from 'that domain is not yours / no longer exists' (403), so SDK credential-refresh and re-auth paths fire for a deleted domain and the caller retries forever instead of recreating the domain. It also shows the two refusal sentences are swapped relative to AWS between the malformed and the unresolvable case, so neither path matches.
Fix: StartPatientInsightsJob's domain resolution in the connecthealth crate — a DELETED (or absent) domain must take the same 403 'Domain cannot be found for this account.' path as Cause 3, and the 'Domain is not valid.' sentence must be reserved for the grammar failure of Cause 2.
Traces: connecthealth-patient-insights-runtime-refuses-deleted-domain#task#9.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 117 steps, 93 passed, 24 mismatched, 0 errored, 0 blocked, score 79.5%, 14 divergence clusters. connecthealth is in get_bootstrap_status()'s bootstrapped set. Six clusters are the causes below; the s3 GetObject cluster is OWNED by 20261120T164159Z-s3-getobject-serves-a-placeholder-summary-object-and.md and excluded; the other seven are filed as the sibling domain/tagging bundle.

Cause 1 — GetPatientInsightsJob, expected 400, got 400, connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#3:
  expected $.message: "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must have length less than or equal to 36"
  actual   $.message: "1 validation error detected: Value 'sim2real-probe-job-702499ba602ad5e5ac' at 'jobId' failed to satisfy constraint: Member must have length less than or equal to 36"

Cause 2 — GetPatientInsightsJob, expected 403, got 400, connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#4:
  status: expected 403, got 400
  expected $.Message: 'Domain is not valid.'
  actual   $.Message: 'res-0a1b2c3d4e5f6a7b8c9d is not a valid domain id. (Service: HealthAgent, Status Code: 400, Request ID: 4251ee5b-488f-4a1a-9b0d-d05c1b66f27e) (SDK Attempt Count: 1)'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  (the request id is a per-run value; the divergence is the status class, the sentence and the presence of any SDK diagnostic tail at all.)

Cause 3 — GetPatientInsightsJob, expected 403, got 404, connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#5 (+connecthealth-patient-insights-runtime-refuses-deleted-domain#task#8):
  status: expected 403, got 404
  expected $.Message: 'Domain cannot be found for this account.'
  actual   $.Message: 'Domain not found: dom-0a1b2c3d4e5f6a7b8c9d0'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 4 — StartPatientInsightsJob, expected 400, got 400, connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#4 (+#task#6, #task#7, connecthealth-patient-insights-job-unknown-datastore-refused#task#3, connecthealth-patient-insights-runtime-refuses-deleted-domain#task#5):
  expected $.Message: 'HealthLake datastore not found.'
  actual   $.Message: 'HealthLake datastore not found'

Cause 5 — StartPatientInsightsJob, expected 400, got 400, connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#5:
  expected $.message: 'Request validation failed'
  actual   $.message: "1 validation error detected: Value at 'encounterContext.encounterReason' failed to satisfy constraint: Member must have length less than or equal to 256"

Cause 6 — StartPatientInsightsJob, expected 403, got 401, connecthealth-patient-insights-runtime-refuses-deleted-domain#task#9:
  status: expected 403, got 401
  expected $.Message: 'Domain cannot be found for this account.'
  actual   $.Message: 'Domain is not valid.'

Model citations used (services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json): GetPatientInsightsJob GET /domain/{domainId}/patient-insights-job/{jobId}, code 200, @readonly, endpoint hostPrefix 'runtime.'; domainId -> DomainId @required @httpLabel @length(20,25) @pattern ^(hai-|dom-)[a-z0-9]+$; jobId -> JobId @required @httpLabel @length(1,36); errors AccessDeniedException 401 / InternalServerException 500 / ResourceNotFoundException 404 / ThrottlingException 429 / ValidationException 400 — no 403 shape is modelled, so the recorded 403 is recorded behavior only. StartPatientInsightsJob POST /domain/{domainId}/patient-insights-job, code 200, hostPrefix 'runtime.'; encounterContext -> PatientInsightsEncounterContext @required with encounterReason -> SensitiveNonEmptyString @required @length(1,256) @pattern ^[a-zA-Z0-9 .,-]+$ (@sensitive); inputDataConfig -> InputDataConfig @required; errors additionally ConflictException 409. ValidationException httpError 400 member message; ResourceNotFoundException httpError 404 member message; AccessDeniedException httpError 401 member message; DomainStatus enum ACTIVE|DELETING|DELETED.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or drive botocore 'connecthealth' (and 'healthlake'/'s3' for setup) clients at the emulator's data listener. Shared setup for every cause: CreateDomain(name=<unique>) -> domainId; for Cause 4 also create an output bucket.

Cause 1 (connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#3):
1. GetPatientInsightsJob(domainId=<the live domain>, jobId=<a 37+ character string>).
   Expect: 400 ValidationException, message "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must have length less than or equal to 36" (no value echoed).
   Observe: the same sentence with "Value '<the submitted jobId>' at 'jobId'".

Cause 2 (#task#4):
2. GetPatientInsightsJob(domainId='res-0a1b2c3d4e5f6a7b8c9d' — a well-sized id that fails the ^(hai-|dom-)[a-z0-9]+$ grammar, jobId=<any valid-length id>).
   Expect: 403 with Message exactly 'Domain is not valid.'
   Observe: 400 ValidationException with '<id> is not a valid domain id. (Service: HealthAgent, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: 1)'.

Cause 3 (#task#5; also connecthealth-patient-insights-runtime-refuses-deleted-domain#task#8):
3. GetPatientInsightsJob(domainId='dom-0a1b2c3d4e5f6a7b8c9d0' — grammar-valid, never allocated, jobId=<any valid-length id>).
   Expect: 403 with Message exactly 'Domain cannot be found for this account.'
   Observe: 404 ResourceNotFoundException 'Domain not found: <that id>'.

Cause 4 (connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#4, #task#6, #task#7; connecthealth-patient-insights-job-unknown-datastore-refused#task#3; connecthealth-patient-insights-runtime-refuses-deleted-domain#task#5):
4. StartPatientInsightsJob(domainId=<live domain>, full required body, inputDataConfig.fhirServer.fhirEndpoint = an endpoint for a HealthLake datastore that does not exist).
   Expect: 400 with Message exactly 'HealthLake datastore not found.' (trailing period).
   Observe: 400 with 'HealthLake datastore not found'.

Cause 5 (connecthealth-patient-insights-job-start-rejects-out-of-range-context#task#5):
5. StartPatientInsightsJob(domainId=<live domain>, encounterContext.encounterReason = a 257+ character string of allowed characters, everything else valid).
   Expect: 400 with message exactly 'Request validation failed' (no member path, no value).
   Observe: 400 with "1 validation error detected: Value at 'encounterContext.encounterReason' failed to satisfy constraint: Member must have length less than or equal to 256".

Cause 6 (connecthealth-patient-insights-runtime-refuses-deleted-domain#task#9):
6. DeleteDomain(domainId) and let the domain reach DELETED, then StartPatientInsightsJob(domainId=<that domain>, valid body).
   Expect: 403 with Message exactly 'Domain cannot be found for this account.'
   Observe: 401 with Message 'Domain is not valid.'

Re-replay the five fixture steps in trace_ids plus the representative connecthealth-patient-insights-job-read-rejects-malformed-identifiers#task#3 to confirm all six causes are closed.
