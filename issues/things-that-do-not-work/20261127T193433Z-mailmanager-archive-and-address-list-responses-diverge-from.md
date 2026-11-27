Issue-Source: trace-sweep-divergence
Issue-Trace: mailmanager-archive-rename-retention-and-tags
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mailmanager-archive-rename-retention-and-tags mailmanager-stop-archive-export-id-refusals mailmanager-stop-import-job-not-found-refusals
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|GetArchive|2xx->2xx|$.ArchiveArn
Issue-Key: mailmanager|UpdateArchive|4xx->4xx|$.Message
Issue-Key: mailmanager|UpdateArchive|4xx->4xx|$.__type
Issue-Key: mailmanager|UpdateArchive|4xx->4xx|status
Issue-Key: mailmanager|StopArchiveExport|4xx->4xx|$.Message
Issue-Key: mailmanager|CreateAddressListImportJob|2xx->2xx|$.PreSignedUrl
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# mailmanager archive and address-list responses diverge from AWS wire shape and refusal semantics

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetArchive, UpdateArchive, StopArchiveExport, CreateAddressListImportJob

## Description
Replay of the simparity-wire-aws-v1 corpus against the mailmanager emulator found FOUR independent defects in how archive and address-list responses are built. They are bundled here because they are all mailmanager response-construction faults; a fixer must close ALL FOUR before submitting. Neither open mailmanager issue covers them (20261109T125742Z-cloudformation-mailmanager-createstack-refuses-every-aws-ses-mailmanager.md is about CloudFormation resource types; 20261126T223139Z-mailmanager-createingresspoint-stub.md is the CreateIngressPoint stub).

Cause 1 of 4 - GetArchive (and every archive response carrying ArchiveArn) emits the wrong ARN resource-type token.
AWS: Smithy: GetArchive.ArchiveArn targets com.amazonaws.mailmanager#ArchiveArn (string, @arnReference), required. The model does not spell out the ARN text, so: recorded behavior only - the recording shows MailManager archive ARNs are built as arn:<partition>:ses:<region>:<account-id>:mailmanager-archive/<ArchiveId>, i.e. the service part is 'ses' and the resource type is the single token 'mailmanager-archive', followed by the archive id from the request/state.
Emulator: returns the same ARN with the resource type hyphenated as 'mail-manager-archive' instead of 'mailmanager-archive'.
Why: ArchiveArn is an @arnReference, so clients, IAM policy documents, CloudFormation/Terraform state and tag/ARN-keyed lookups round-trip it. An ARN whose resource type does not exist on AWS fails any policy match, any ARN parse that validates the resource type, and any cross-reference back into the emulator that was given a real-looking ARN - and the wrong ARN is persisted into client state, so the breakage outlives the call.
Fix: the archive ARN construction used by the archive handlers/state layer (the one place that formats arn:...:ses:...:<resource-type>/<ArchiveId>), so GetArchive/CreateArchive/ListArchives/UpdateArchive all change together. The divergence is reported on three steps of the same fixture, which is consistent with one shared formatter.
Traces: mailmanager-archive-rename-retention-and-tags#task#2 (representative), and #task#4, #task#10.

Cause 2 of 4 - UpdateArchive on an archive that is not ACTIVE refuses with the wrong status and the wrong error class.
AWS: Smithy: UpdateArchive models both ValidationException (httpError 400) and ConflictException (httpError 409); ArchiveState is an enum of ACTIVE | PENDING_DELETION. The recording shows that modifying an archive that is not in ACTIVE state is a VALIDATION failure, not a conflict: HTTP 400 with __type 'com.amazon.pancetta#ValidationException' and Message 'Archive is not Active'. That message is entirely fixed text - it interpolates nothing from the request or state (no archive id, no state name).
Emulator: answers HTTP 409 with __type 'ConflictException' and Message 'Archive <ArchiveId> is pending deletion; no further modifications can be made.' - wrong status, wrong error shape, an unqualified __type rather than the recorded 'com.amazon.pancetta#<Error>' form, and an invented message that additionally interpolates the archive id.
Why: 400 and 409 are different retry classes - SDK/clients treat 409 ConflictException as a transient conflict worth retrying, so a client that updates an archive scheduled for deletion will retry-loop instead of failing fast; error-code-based branching (catch ValidationException) never fires; and any assertion on the message text fails.
Fix: the UpdateArchive handler's pre-condition validation (the archive-state check) plus the error serializer that writes __type, so the refusal is raised as ValidationException/400 with the documented fixed message and __type is namespace-qualified as the wire records it.
Traces: mailmanager-archive-rename-retention-and-tags#task#11.

Cause 3 of 4 - StopArchiveExport's refusal for an unknown/ill-formed ExportId omits the Message member entirely.
AWS: Smithy: StopArchiveExport models only AccessDeniedException(403), ThrottlingException(429) and ValidationException(400); ValidationException carries a Message member (target ErrorMessage) and its doc says 'The request validation has failed. For details, see the accompanying error message.' The recording shows the body carries Message 'Invalid ExportId' - all fixed text, with no part taken from the request (the offending id is not echoed).
Emulator: returns the right status (400) but the body has NO Message key at all ($.Message missing), so the accompanying error message the model promises is absent.
Why: every AWS SDK surfaces the error message as the exception's message; with Message absent the client raises an empty-message ValidationException, so the operator/log has no statement of what was wrong, and any client branching or test asserting on the message text breaks.
Fix: the StopArchiveExport validation path / the mailmanager error serializer - populate ValidationException.Message when refusing an ExportId, rather than serializing a bare error envelope.
Traces: mailmanager-stop-archive-export-id-refusals#task#1 (representative), and #task#2.

Cause 4 of 4 - CreateAddressListImportJob returns a PreSignedUrl that is not a presigned upload URL.
AWS: Smithy: CreateAddressListImportJob.PreSignedUrl is required and @sensitive, targeting com.amazonaws.mailmanager#PreSignedUrl. The fixture asserts the value's SHAPE rather than a literal (expected '<shaped-not-asserted>'), because the URL is per-run. Recorded behavior only: AWS returns a presigned S3 object URL - an https URL whose path addresses the upload object and which carries the SigV4 presigning query parameters (X-Amz-Algorithm, X-Amz-Credential, X-Amz-Date, X-Amz-Expires, X-Amz-SignedHeaders, X-Amz-Signature). The host, object key, expiry and signature are per-environment values and are NOT requirements; the presence of a query string with those presigning parameters is.
Emulator: returns a bare internal path URL of the form https://<emulator-host>:<port>/_aws/address-list-import-jobs/<hex-id> with no query string and no presigning parameters, which does not satisfy the recorded PreSignedUrl shape.
Why: a client's next step is to PUT the address-list CSV/JSON to this URL; code that parses the URL for the signature/expiry, or that strips/validates query parameters before uploading, cannot proceed, and a client that checks the URL is presigned before using it rejects the response outright. The import flow therefore cannot be exercised end to end.
Fix: the CreateAddressListImportJob handler's response construction - mint the upload target through the emulator's presigned-URL generator (same mechanism S3 presigning uses) so the returned URL carries SigV4 query parameters, instead of formatting an internal route path.
Traces: mailmanager-stop-import-job-not-found-refusals#task#2.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=56, passed=38, mismatched=18, divergence_count=4, score_pct=67.9.

Cause 1 - GetArchive, fixture mailmanager-archive-rename-retention-and-tags step task#2 (also task#4, task#10); status 200 expected, 200 actual:
  expected $.ArchiveArn = 'arn:aws:ses:us-west-2:000000000000:mailmanager-archive/a-6fa50e59dad90379910a2dec89025cc1'
  actual   $.ArchiveArn = 'arn:aws:ses:us-west-2:000000000000:mail-manager-archive/a-6fa50e59dad90379910a2dec89025cc1'
  (the account id and archive id are per-run values; the diverging token is the resource type 'mail-manager-archive' vs 'mailmanager-archive')

Cause 2 - UpdateArchive, fixture mailmanager-archive-rename-retention-and-tags step task#11:
  expected: status 400, $.__type = 'com.amazon.pancetta#ValidationException', $.Message = 'Archive is not Active'
  actual:   status 409, $.__type = 'ConflictException', $.Message = 'Archive a-6fa50e59dad90379910a2dec89025cc1 is pending deletion; no further modifications can be made.'

Cause 3 - StopArchiveExport, fixture mailmanager-stop-archive-export-id-refusals steps task#1 and task#2; status 400 expected and 400 actual:
  expected $.Message = 'Invalid ExportId'
  actual   $.Message = missing (key absent from the body)

Cause 4 - CreateAddressListImportJob, fixture mailmanager-stop-import-job-not-found-refusals step task#2; status 200 expected, 200 actual:
  expected $.PreSignedUrl = '<shaped-not-asserted>' (shape assertion: presigned https URL carrying SigV4 query parameters)
  actual   $.PreSignedUrl = 'https://127.0.0.1:42393/_aws/address-list-import-jobs/004bca836107271f2c58a1a8015b76d4301f0cf6b954182f5ba69ec246b92a466639376339626232353661633034383039313061326465633839303235636331' (no query string at all; the host/port and id are per-run and not themselves the defect)

Model citations: GetArchive.ArchiveArn -> ArchiveArn (string, @arnReference, required); UpdateArchive errors include ValidationException(400) and ConflictException(409); ArchiveState enum = ACTIVE|PENDING_DELETION; StopArchiveExport errors = AccessDeniedException(403)/ThrottlingException(429)/ValidationException(400), ValidationException.Message -> ErrorMessage; CreateAddressListImportJob.PreSignedUrl required+@sensitive.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or exercise each cause directly against the mailmanager endpoint:

Cause 1: CreateArchive, then GetArchive with that ArchiveId. Inspect ArchiveArn - the resource type must be 'mailmanager-archive'. Fixture mailmanager-archive-rename-retention-and-tags, step task#2 (also task#4, task#10).
Cause 2: create an archive, DeleteArchive it so its ArchiveState becomes PENDING_DELETION, then call UpdateArchive on it (e.g. with a new ArchiveName). Expect HTTP 400, __type 'com.amazon.pancetta#ValidationException', Message 'Archive is not Active'. Fixture mailmanager-archive-rename-retention-and-tags, step task#11.
Cause 3: call StopArchiveExport with an ExportId that does not exist / is not a valid export id. Expect HTTP 400 ValidationException whose body includes Message 'Invalid ExportId'. Fixture mailmanager-stop-archive-export-id-refusals, steps task#1 and task#2.
Cause 4: CreateAddressList, then CreateAddressListImportJob for that AddressListId with a Name and ImportDataFormat. Inspect PreSignedUrl - it must be a presigned upload URL carrying the SigV4 query parameters, not a bare internal route path. Fixture mailmanager-stop-import-job-not-found-refusals, step task#2.
