Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.batch-put-profile-object-keyless-entries-refused-by-id-rejected customer-profiles.batch-put-profile-object-rejects-a-repeated-item-id customer-profiles.batch-put-profile-object-unknown-object-type-refuses-every-entry-rejected customer-profiles.list-profile-objects-rejects-a-filter-with-two-values customer-profiles.put-profile-object-rejects-an-object-type-the-domain-lacks customer-profiles.start-upload-job-rejects-a-job-of-another-domain customer-profiles.start-upload-job-rejects-a-job-whose-file-is-not-uploaded customer-profiles.segment-stream-association-reads-running-then-stopped
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.batch-put-profile-object-keyless-entries-refused-by-id-rejected|run#13|AssertionError|assert sorted(ok_ids) == ["v-a", "v-c"], f"Successful should be v-a and v-c, got {ok_ids!r}"
Issue-Key: customer-profiles.batch-put-profile-object-rejects-a-repeated-item-id|_refusal#9|AssertionError|raise AssertionError(f"the batch was expected to be refused, but it returned {answer!r}")
Issue-Key: customer-profiles.batch-put-profile-object-unknown-object-type-refuses-every-entry-rejected|_until_found#12|Timeout|time.sleep(2)
Issue-Key: customer-profiles.list-profile-objects-rejects-a-filter-with-two-values|_profile_for#10|AssertionError|raise AssertionError(f"no profile was indexed under shipEmail={email!r} in {budget_s}s")
Issue-Key: customer-profiles.put-profile-object-rejects-an-object-type-the-domain-lacks|run#8|AssertionError|assert got == ("ResourceNotFoundException", f"The ObjectTypeName {_TYPE} is not found in the domain"), (
Issue-Key: customer-profiles.start-upload-job-rejects-a-job-of-another-domain|run#8|AssertionError|assert got == ("ResourceNotFoundException", f"Upload Job with Id {job} not found"), (
Issue-Key: customer-profiles.start-upload-job-rejects-a-job-whose-file-is-not-uploaded|_refusal#6|AssertionError|raise AssertionError(f"the start was expected to be refused, but it returned {answer!r}")
Issue-Key: customer-profiles.segment-stream-association-reads-running-then-stopped|run#20|AssertionError|assert code == "ResourceNotFoundException", f"a domain with no associated stream answered {code!r}"
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# customer-profiles: 7 net-new behavioral causes — BatchPutProfileObject skips per-item key and duplicate-Id validation, object-keyed SearchProfiles never resolves, two refusal messages are emulator-worded, StartUploadJob accepts a fileless job, GetStreamForSegments is 501

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutProfileObject

## Description
Seven distinct root causes in the customer-profiles emulator, all grounded in failing behavioral checks of the customer-profiles suite. THIS ISSUE HAS 7 CAUSES; all seven must be fixed before it is closed.

Cause 1 of 7: BatchPutProfileObject accepts batch items that carry none of the object type's UNIQUE or PROFILE key values, instead of failing those items per Id.
AWS: Smithy BatchPutProfileObject -> http {method PUT, uri /domains/{DomainName}/profiles/objects/batch-put-profile-object, code 200}; output carries Successful (BatchPutProfileObjectResponseList) and Failed (BatchPutProfileObjectErrorList). BatchPutProfileObjectErrorItem has required Id ("The unique identifier of the item in the batch request that failed"), required Code (responseCode, range 200-599, "The HTTP status code for the error") and Message. Recorded behavior: an item whose Object populates no field named by any of the type's Keys (StandardIdentifiers UNIQUE or PROFILE) is reported in Failed under its own request Id with Code 400 and Message "The UNIQUE key or PROFILE key does not exist in the profile object" — that Message is entirely fixed text, interpolating nothing from the request; interleaved items that do carry a key are accepted in Successful and persist, and no profile ever resolves by a key value a refused item carried until that item is resent complete.
Emulator: every item is accepted. For a five-item batch whose items v-b (PROFILE key only, no UNIQUE value), v-d (UNIQUE value only, no PROFILE value) and v-e (a JSON array, not an object) must all land in Failed, the emulator answered Successful = ['v-a','v-b','v-c','v-d'] — i.e. v-b and v-d were written with no key at all and Failed was not populated for them.
Why: a client batching up to ten items (Items @length min 1 max 10) is told keyless objects were stored. They are unreachable (no key resolves them), so the caller never resends them, and the domain accumulates profile objects AWS would have rejected — silent data corruption that only surfaces as missing search hits much later.
Fix: the BatchPutProfileObject handler's per-item validation — resolve the named object type's Keys against each item's parsed Object and collect a 400 Failed entry per keyless item rather than writing it (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/batch_put_profile_object.rs).
Traces: check customer-profiles.batch-put-profile-object-keyless-entries-refused-by-id-rejected.

Cause 2 of 7: BatchPutProfileObject accepts a batch that repeats one item Id instead of refusing the whole request.
AWS: Smithy BatchPutProfileObjectRequestItem.Id is documented "A unique identifier for this item in the batch request. Used to correlate items in the response", and BadRequestException (client, httpError 400) is modeled on the operation. Recorded behavior: a batch carrying two items with the same Id is refused whole with 400 BadRequestException and Message "Id <Id> is duplicated in request". Fixed text: "Id " ... " is duplicated in request"; the single variable part is the repeated Id value taken from the request (the value in the recording is only an example).
Emulator: HTTP 200 with Successful listing the same Id twice, each with its own ProfileObjectUniqueKey, and Failed = [] — the duplicate is written twice.
Why: Id is the correlation handle for the response. With duplicates accepted, a client cannot map Successful/Failed entries back to the items it sent, and two objects are stored where AWS stored none.
Fix: BatchPutProfileObject's request validation — reject the request with the AWS-worded BadRequestException when Items contains a repeated Id, before any item is written.
Traces: check customer-profiles.batch-put-profile-object-rejects-a-repeated-item-id.

Cause 3 of 7: profiles created or enriched by PutProfileObject / BatchPutProfileObject are never resolvable through SearchProfiles by the object type's own key names, so key-driven reads stall indefinitely.
AWS: Smithy SearchProfiles -> POST /domains/{DomainName}/profiles/search, required KeyName (name, 1-64) and Values; output Items (ProfileList). Recorded behavior: after PutProfileObject / BatchPutProfileObject stores an object of a type whose Keys name a PROFILE identifier over a mapped field (e.g. Keys {"shipEmail": [{StandardIdentifiers:[PROFILE], FieldNames:["mail"]}]} with Fields.mail.Target=_profile.EmailAddress), SearchProfiles with that KeyName and the stored field value resolves the profile the object created or attached to, within seconds; its ProfileId is 32 hex digits.
Emulator: the search returns Items = [] for as long as the check polls. Evidence: "no profile was indexed under shipEmail='<generated address>' in 14.97s" after a successful PutProfileObject that returned a ProfileObjectUniqueKey; and the waybill check, which polls SearchProfiles(KeyName="carrierRef") after an accepted BatchPutProfileObject, never gets a hit and is cut off by the 60 s check budget.
Why: every key-driven read path downstream is unreachable: a client cannot find the profile an ingested object created, so ListProfileObjects (which needs the ProfileId), profile enrichment and de-duplication flows all dead-end. The emulator looks like an eventually-consistent service that never converges.
Fix: the PutProfileObject / BatchPutProfileObject ingestion path and the profile key index SearchProfiles reads — a stored object must register its type's key names and values against the resolved profile (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/put_profile_object.rs, .../batch_put_profile_object.rs and the domain's profile-key state).
Traces: checks customer-profiles.list-profile-objects-rejects-a-filter-with-two-values and customer-profiles.batch-put-profile-object-unknown-object-type-refuses-every-entry-rejected. (Because the key search never resolves, these checks never reach their own graded assertions — the ListProfileObjects ObjectFilter two-value refusal and the per-item 404 batch answer — so those are untested here, not cleared.)

Cause 4 of 7: PutProfileObject's unknown-object-type refusal is worded by the emulator, not by AWS.
AWS: Smithy PutProfileObject -> errors include ResourceNotFoundException (client, httpError 404). Recorded behavior for an ObjectTypeName the domain has not defined: 404 with Message "The ObjectTypeName <ObjectTypeName> is not found in the domain". Fixed text: "The ObjectTypeName " ... " is not found in the domain"; the only variable part is the ObjectTypeName copied from the request.
Emulator: the code is right (ResourceNotFoundException) but the Message is "ObjectType <ObjectTypeName> not found" — a different sentence.
Why: clients and suites that branch on AWS's error text do not recognise the refusal; the status code hides the divergence until it fails deep in client code.
Fix: PutProfileObject's object-type resolution error construction — emit AWS's fixed sentence around the request's ObjectTypeName. (Note: open issue 20261127T100509Z-customer-profiles-error-responses-diverge-from-aws-wrong covers the ListProfileObjects and BatchPutProfileObject wordings, not PutProfileObject's; this cause is net-new.)
Traces: check customer-profiles.put-profile-object-rejects-an-object-type-the-domain-lacks.

Cause 5 of 7: StartUploadJob's cross-domain not-found refusal is worded by the emulator, not by AWS.
AWS: Smithy StartUploadJob -> PUT /domains/{DomainName}/upload-jobs/{JobId}, errors include ResourceNotFoundException (client, httpError 404). Recorded behavior when the JobId exists but belongs to another domain: 404 with Message "Upload Job with Id <JobId> not found". Fixed text: "Upload Job with Id " ... " not found"; the single variable part is the JobId from the request path. The domain name is NOT part of the message.
Emulator: 404 ResourceNotFoundException with Message "The upload job <JobId> does not exist in domain <DomainName>" — its own sentence, and it additionally interpolates the domain.
Why: the message is a requirement of the error class; a client matching AWS's text mis-classifies the failure, and the extra domain interpolation implies a message shape AWS never emits.
Fix: StartUploadJob's job-lookup error construction (.../src/ops/start_upload_job.rs).
Traces: check customer-profiles.start-upload-job-rejects-a-job-of-another-domain.

Cause 6 of 7: StartUploadJob accepts a job whose CSV was never uploaded, answering 200 instead of refusing.
AWS: Smithy StartUploadJob -> BadRequestException (client, httpError 400) is modeled. Recorded behavior: starting an upload job created by CreateUploadJob before anything is PUT to the URL GetUploadJobPath hands out is refused with 400 BadRequestException and Message "Cannot Start import job. No file uploaded for import job <JobId>" — fixed text "Cannot Start import job. No file uploaded for import job " with the request's JobId appended — and the job stays in Status CREATED. The identical call succeeds once the file has been uploaded.
Emulator: HTTP 200 with an empty body, so the refusal never happens and the job is started with no input.
Why: the precondition that distinguishes a startable job from an empty one is gone. A client gets a "started" job that can only end in a bogus terminal state, and the error path AWS clients code against is unreachable.
Fix: StartUploadJob's handler/validation — gate the start on the job's uploaded-file state and emit AWS's BadRequestException sentence.
Traces: check customer-profiles.start-upload-job-rejects-a-job-whose-file-is-not-uploaded.

Cause 7 of 7: GetStreamForSegments answers 501 NotImplemented, so the segment-stream lifecycle (absent -> RUNNING -> STOPPED) cannot be observed at all.
AWS: Smithy GetStreamForSegments -> GET /domains/{DomainName}/segment-streams, readonly; output members AssociatedAt, AssociatedSegments, DomainName, DestinationArn, DestinationRoleArn, State (EventSubscriptionState: RUNNING | UNHEALTHY | STOPPED), DisassociatedAt, FailureReason; errors include ResourceNotFoundException (client, httpError 404). Recorded behavior: for a domain with no association the call is refused 404 ResourceNotFoundException with Message "No segment event stream found for domain <DomainName>" (fixed text "No segment event stream found for domain " plus the request's DomainName); after AssociateStreamForSegments the same call reads State RUNNING with the associated DomainName/DestinationArn/DestinationRoleArn, AssociatedSegments [] and no DisassociatedAt; after DisassociateStreamForSegments it reads State STOPPED, still names its DestinationArn, and carries DisassociatedAt >= AssociatedAt.
Emulator: the call answers error code NotImplemented with Message "operation GetStreamForSegments is scaffolded but not implemented in this emulator" (HTTP 501) for every input.
Why: no client can read whether a domain's segment event stream is live, and the 501 is not a modeled error, so SDK error handling for the modeled ResourceNotFoundException never runs.
Fix: implement the GetStreamForSegments handler over the domain's segment-stream association state (.../src/ops/get_stream_for_segments.rs). This is the same operation the scaffold-stub issue filed for check customer-profiles.segment-stream-association-reads-back-and-disassociation-stops-it names; this cause records the lifecycle and refusal semantics the stub issue does not state, and the check below must replay green too.
Traces: check customer-profiles.segment-stream-association-reads-running-then-stopped.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, replayed against the emulator built from this checkout (44/68 passed; service customer-profiles 44/58).

Cause 1 — customer-profiles.batch-put-profile-object-keyless-entries-refused-by-id-rejected [mismatch]: AssertionError: Successful should be v-a and v-c, got ['v-a', 'v-b', 'v-c', 'v-d']. Expected Failed to hold ('v-b', 400, 'The UNIQUE key or PROFILE key does not exist in the profile object'), ('v-d', 400, same), ('v-e', 400, same).

Cause 2 — customer-profiles.batch-put-profile-object-rejects-a-repeated-item-id [mismatch]: AssertionError: the batch was expected to be refused, but it returned HTTPStatusCode 200 with Successful = [{'Id': 'visit-a', 'ProfileObjectUniqueKey': '7c27cf…'}, {'Id': 'visit-a', 'ProfileObjectUniqueKey': 'dc406c…'}], Failed = []. Expected ('BadRequestException', 'Id visit-a is duplicated in request') — 'visit-a' is that run's repeated Id, the template is "Id <Id> is duplicated in request".

Cause 3 — customer-profiles.list-profile-objects-rejects-a-filter-with-two-values [mismatch]: AssertionError: no profile was indexed under shipEmail='<generated address>' in 14.969s (SearchProfiles KeyName='shipEmail' returned no Items after a PutProfileObject that answered a ProfileObjectUniqueKey). customer-profiles.batch-put-profile-object-unknown-object-type-refuses-every-entry-rejected [mismatch]: Check timed out after 60.0s while polling SearchProfiles(KeyName='carrierRef') for the profile an accepted control batch created.

Cause 4 — customer-profiles.put-profile-object-rejects-an-object-type-the-domain-lacks [mismatch]: AssertionError: putting an object of an undefined type answered ('ResourceNotFoundException', 'ObjectType Shipments not found'); expected ('ResourceNotFoundException', 'The ObjectTypeName Shipments is not found in the domain'). 'Shipments' is the request's ObjectTypeName.

Cause 5 — customer-profiles.start-upload-job-rejects-a-job-of-another-domain [mismatch]: AssertionError: starting the job under a domain that does not own it answered ('ResourceNotFoundException', 'The upload job <JobId> does not exist in domain <DomainName>'); expected ('ResourceNotFoundException', 'Upload Job with Id <JobId> not found').

Cause 6 — customer-profiles.start-upload-job-rejects-a-job-whose-file-is-not-uploaded [mismatch]: AssertionError: the start was expected to be refused, but it returned {'ResponseMetadata': {'HTTPStatusCode': 200, …, 'content-length': '2'}} — expected 400 BadRequestException 'Cannot Start import job. No file uploaded for import job <JobId>' and the job left in Status CREATED.

Cause 7 — customer-profiles.segment-stream-association-reads-running-then-stopped [mismatch]: AssertionError: a domain with no associated stream answered 'NotImplemented' (Message 'operation GetStreamForSegments is scaffolded but not implemented in this emulator'); expected 'ResourceNotFoundException' with Message 'No segment event stream found for domain <DomainName>'.

Model evidence: smithy_query query=operation operation=BatchPutProfileObject,StartUploadJob,SearchProfiles,PutProfileObject,ListProfileObjects,GetStreamForSegments and query=shape shape=BatchPutProfileObjectErrorItem,BatchPutProfileObjectRequestItem,ObjectFilter,EventSubscriptionState on services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/smithy-model/com-amazonaws-customerprofiles-customerprofiles-20200815.json.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1.
3. Cause 1: replay check customer-profiles.batch-put-profile-object-keyless-entries-refused-by-id-rejected — define an object type with Keys {PROFILE over field 'member', UNIQUE over field 'visit'} and send one BatchPutProfileObject with items that carry both keys, only one key, and a JSON array. Expect only the two-key items in Successful; each other item in Failed under its own Id with Code 400 and Message 'The UNIQUE key or PROFILE key does not exist in the profile object'.
4. Cause 2: replay check customer-profiles.batch-put-profile-object-rejects-a-repeated-item-id — BatchPutProfileObject with two Items sharing one Id. Expect 400 BadRequestException 'Id <Id> is duplicated in request' and nothing written.
5. Cause 3: replay checks customer-profiles.list-profile-objects-rejects-a-filter-with-two-values and customer-profiles.batch-put-profile-object-unknown-object-type-refuses-every-entry-rejected — after PutProfileObject / BatchPutProfileObject of an object of a type with a PROFILE key, SearchProfiles(DomainName, KeyName=<that key name>, Values=[<the field value>]) must resolve the profile (32-hex-digit ProfileId) within seconds; today Items stays empty.
6. Cause 4: replay check customer-profiles.put-profile-object-rejects-an-object-type-the-domain-lacks — PutProfileObject with an undefined ObjectTypeName. Expect 404 'The ObjectTypeName <ObjectTypeName> is not found in the domain'.
7. Cause 5: replay check customer-profiles.start-upload-job-rejects-a-job-of-another-domain — StartUploadJob with a JobId created in a different domain. Expect 404 'Upload Job with Id <JobId> not found'.
8. Cause 6: replay check customer-profiles.start-upload-job-rejects-a-job-whose-file-is-not-uploaded — CreateUploadJob, then StartUploadJob without PUTting the CSV to the GetUploadJobPath URL. Expect 400 'Cannot Start import job. No file uploaded for import job <JobId>' and GetUploadJob Status 'CREATED'.
9. Cause 7: replay check customer-profiles.segment-stream-association-reads-running-then-stopped — GetStreamForSegments on a domain with no association must be 404 'No segment event stream found for domain <DomainName>'; after AssociateStreamForSegments it must read State RUNNING with the associated ARNs and AssociatedSegments []; after DisassociateStreamForSegments State STOPPED with DisassociatedAt >= AssociatedAt.
10. All seven causes must replay green before closing.
