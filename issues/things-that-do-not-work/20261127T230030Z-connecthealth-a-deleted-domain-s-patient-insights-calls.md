Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connecthealth.patient-insights-job-unreachable-after-domain-deleted
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth.patient-insights-job-unreachable-after-domain-deleted|run#35|AssertionError|assert code == "AccessDeniedException", f"expected {what} in the deleted domain to be AccessDeniedException, got {code!r}"
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# connecthealth: a deleted domain's patient-insights calls are refused 404 ResourceNotFoundException instead of the account-scoped AccessDeniedException 'Domain cannot be found for this account.'

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPatientInsightsJob

## Description
REOPENS 20261029T183834Z-connecthealth-startpatientinsightsjob-answers-a-nonexistent-domain-404-resourcenotfoundexception.md: its fix left the call refused. That issue (status fixed) claimed StartPatientInsightsJob would stop answering an unresolvable domain with 404 ResourceNotFoundException and would take the AccessDenied path instead; replaying the behavioral suite against this checkout shows both runtime patient-insights operations still refuse an unresolvable (here: deleted) domain with ResourceNotFoundException, so the refusal class this issue's predecessor was closed on is back.

One root cause, observed on both runtime operations bound to hostPrefix 'streaming/runtime' domain resolution.

Cause 1 of 1: a domain whose DeleteDomain has completed is no longer resolvable for the account, and every patient-insights call naming it must be refused as an authorization failure, not as a missing resource.
Failing check: connecthealth.patient-insights-job-unreachable-after-domain-deleted.
AWS: Smithy (services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json) models GetPatientInsightsJob GET /domain/{domainId}/patient-insights-job/{jobId} and StartPatientInsightsJob POST /domain/{domainId}/patient-insights-job, with errors AccessDeniedException (errorKind client, httpError 401, single member `message`), ResourceNotFoundException (client, httpError 404, member `message`), ValidationException (400), ThrottlingException (429), InternalServerException (500) — plus ConflictException (409) on Start; domainId -> DomainId @required @httpLabel @length(20,25) @pattern ^(hai-|dom-)[a-z0-9]+$. The model does not word the refusal, so the wording is recorded behavior only: once the domain is deleted, GetDomain for it is refused ResourceNotFoundException with message `Domain not found: <domainId>` (fixed text `Domain not found: ` plus the requested id — this part the emulator already gets right and the check passes on it), while GetPatientInsightsJob and StartPatientInsightsJob for the same id are refused with error code AccessDeniedException and a message whose fixed text is `Domain cannot be found` ... `for this account.` — i.e. the recorded sentence `Domain cannot be found for this account.`, entirely fixed text with NOTHING interpolated: AWS deliberately does not name or confirm the domain id on the runtime path, because the refusal is account-scoped authorization, not an existence answer. The same recorded refusal is documented from the wire side in open issue 20261127T101150Z-connecthealth-patient-insights-runtime-refusals-diverge-wrong-status.md (its Cause 3 and Cause 6, recorded as HTTP 403 with Message 'Domain cannot be found for this account.'); that issue carries no behavioral replay key for this check (the suite does not report this check as owned) and the emulator's current answer here (ResourceNotFoundException) is not the answer either of those causes recorded, so it is filed separately and cross-referenced.
Emulator: both calls on the deleted domain are refused with error code ResourceNotFoundException (the modelled 404 shape). The check reads the code off the refusal and gets 'ResourceNotFoundException' where AWS gives 'AccessDeniedException'; the message consequently also fails the fixed-text assertion (it does not start with `Domain cannot be found` nor end with `for this account.`). The positive controls in the same check all pass: the job is readable and SUCCEEDED while the domain lives, ListDomains lists it before and omits it after deletion, and GetDomain's own ResourceNotFoundException wording is correct.
Why: a client keys its handling on the error class. On AWS, AccessDenied on the runtime path means 'this domain is not yours / no longer yours — re-provision or check credentials', and botocore surfaces it through the AccessDeniedException exception class, routing callers into credential/authorization handling; ResourceNotFoundException means 'that job is gone' and routes callers into retry-with-another-job or create-the-job logic. The emulator sends every caller down the wrong branch for a deleted domain, and it converts an authorization refusal into an existence oracle: a caller learns from the emulator whether a domain id it does not own exists, which the recorded AWS refusal is specifically shaped to avoid.
Fix: the shared domain-resolution step in front of the runtime patient-insights handlers in the connecthealth crate (GetPatientInsightsJob and StartPatientInsightsJob) must map a DELETED or otherwise unresolvable-for-this-account domainId onto the AccessDeniedException refusal with the fixed message `Domain cannot be found for this account.`, and must keep ResourceNotFoundException for the control-plane GetDomain/DeleteDomain surface where it is correct. Do not interpolate the requested domainId into the runtime refusal.
Traces: connecthealth.patient-insights-job-unreachable-after-domain-deleted.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 25/35 passed (71.4%), 4 mismatched, 6 errored, 0 skipped, 0 unloadable, bootstrap_signal=false. Heading '### connecthealth': 25/33 (76%), 8 failed. connecthealth is bootstrapped (get_bootstrap_status reports service-com-amazonaws-connecthealth-connecthealth in the bootstrapped set; the pending set holds only abusesignal, connectscheduling, elbproxy and opensearchmemory).

connecthealth.patient-insights-job-unreachable-after-domain-deleted [mismatch]:
  File "check_connecthealth_patient_insights_job_unreachable_after_domain_deleted.py", line 207, in run
    assert code == "AccessDeniedException", f"expected {what} in the deleted domain to be AccessDeniedException, got {code!r}"
  AssertionError: expected reading its job in the deleted domain to be AccessDeniedException, got 'ResourceNotFoundException'

The check's sequence up to that point all passed: healthlake datastore created and a FHIR Patient seeded and searchable; CreateDomain -> domainId matching ^(hai-|dom-)[a-z0-9]{16,21}$; StartPatientInsightsJob -> jobId matching the UUID form; GetPatientInsightsJob polled to SUCCEEDED and echoing encounterContext.encounterReason; ListDomains listing the live domain; DeleteDomain; GetDomain then refused ResourceNotFoundException with a message starting 'Domain not found: ' and ending in the deleted domain id (asserted and passing).

Then, for each of the two runtime calls on the deleted domain —
  (a) GetPatientInsightsJob(domainId=<deleted domain>, jobId=<the job that had SUCCEEDED>)
  (b) StartPatientInsightsJob(domainId=<deleted domain>, full valid body, fresh clientToken)
the check asserts: error code == 'AccessDeniedException' and message.startswith('Domain cannot be found') and message.endswith('for this account.').
  expected: code 'AccessDeniedException', message 'Domain cannot be found for this account.' (whole sentence fixed text, no interpolation)
  actual:   code 'ResourceNotFoundException' (the assertion fires on call (a) before (b) is reached)

Model citations used (services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json): GetPatientInsightsJob GET /domain/{domainId}/patient-insights-job/{jobId}, errors AccessDeniedException 401 / InternalServerException 500 / ResourceNotFoundException 404 / ThrottlingException 429 / ValidationException 400; StartPatientInsightsJob POST /domain/{domainId}/patient-insights-job, same errors plus ConflictException 409; AccessDeniedException = structure, errorKind client, httpError 401, member message ('This error is thrown when the client does not supply proper credentials to the API.'); ResourceNotFoundException = structure, client, httpError 404, member message; DomainId @length(20,25) @pattern ^(hai-|dom-)[a-z0-9]+$.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore connecthealth / healthlake / s3 clients at it. Re-replay connecthealth.patient-insights-job-unreachable-after-domain-deleted, or by hand:

1. healthlake CreateFHIRDatastore(DatastoreTypeVersion='R4'), poll DescribeFHIRDatastore to ACTIVE, take DatastoreProperties.DatastoreEndpoint; SigV4-sign (service 'healthlake') PUT <endpoint>/Patient/<patient-id> with a minimal Patient whose identifier[0].value == <patient-id>; confirm GET <endpoint>/Patient?identifier=<patient-id> returns one entry.
2. s3 CreateBucket(<unique>) for the job's outputDataConfig.
3. connecthealth CreateDomain(name=<unique>) -> domainId.
4. StartPatientInsightsJob(domainId, clientToken=<unique>, patientContext={patientId,dateOfBirth,pronouns:'THEY_THEM'}, insightsContext={insightsType:'PRE_VISIT'}, encounterContext={encounterReason:'Follow up visit'}, userContext={role:'CLINICIAN',userId,specialty:'PRIMARY_CARE'}, inputDataConfig={fhirServer:{fhirEndpoint:<endpoint>}}, outputDataConfig={s3OutputPath:'s3://<bucket>/insights/'}) -> jobId; poll GetPatientInsightsJob to SUCCEEDED.
5. DeleteDomain(domainId=<that domain>); poll GetDomain until it is refused. Expect and observe ResourceNotFoundException 'Domain not found: <domainId>' (this part is correct today).
6. GetPatientInsightsJob(domainId=<the deleted domain>, jobId=<the job from step 4>).
   Expect: refused with error code AccessDeniedException and message exactly 'Domain cannot be found for this account.' (no domain id interpolated).
   Observe: refused with error code ResourceNotFoundException.
7. StartPatientInsightsJob(domainId=<the deleted domain>, the same valid body with a fresh clientToken).
   Expect: the identical AccessDeniedException refusal with the same fixed message.
   Observe: ResourceNotFoundException (step 6 already fails the check, so fix both calls).
8. ListDomains must still omit the deleted domain (passes today; keep it passing).
