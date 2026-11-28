Issue-Source: trace-sweep-divergence
Issue-Trace: omics-read-set-part-upload-refusals
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: omics-read-set-part-upload-refusals
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics|CompleteMultipartReadSetUpload|4xx->4xx|$.message
Issue-Commit: 429bdfecbf56545571d755c8b6ad852b4e3ed1bc

# omics CompleteMultipartReadSetUpload refusals carry emulator-authored messages instead of AWS's fixed sentences

> _Note: this issue was identified against the code base at commit `429bdfecbf56545571d755c8b6ad852b4e3ed1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteMultipartReadSetUpload

## Description
Two net-new root causes in CompleteMultipartReadSetUpload's refusal messages. Both times the HTTP status and the error code are right and only the message text diverges -- but the message is part of the requirement, and in both cases the emulator substitutes a sentence of its own that interpolates request/state values AWS never echoes. Each cause must be closed on its own before submitting.

Relationship to already-open work: the open BEHAVIORAL issue 20261127T141502Z-omics-7-net-new-root-causes-read-set.md describes the same class of emulator-authored upload prose (its Cause 1 names the same expected part-checksum sentence, its Cause 2 the upload-not-found sentence), but it is grounded in behavioral check ids only and carries NO wire replay key, so these two wire clusters are not covered by it and were reported as net-new by the replay. Cause 2 below is additionally a DIFFERENT expectation from anything that issue states: the recorded message for this step is the generic resource sentence, not the two-sentence upload sentence that issue asks for. Fixing one does not close the other; both wire steps below must be re-replayed.

=== Cause 1 of 2: the part-checksum refusal is composed by the handler instead of AWS's one fixed sentence ===
AWS: Smithy: CompleteMultipartReadSetUpload.parts -> CompleteReadSetUploadPartList @required; the operation declares ValidationException httpError 400. Recorded behavior: a completion whose parts list names a part/partNumber pair the upload does not hold with that checksum is refused 400 ValidationException with the single FIXED sentence "One or more of the specified parts could not be found. The part might not have been uploaded, or the specified checksum might not have matched the part's checksum". Every word of it is fixed: it interpolates NOTHING from the request -- no part number, no partSource, no uploadId, no sequenceStoreId -- and deliberately collapses the two distinguishable conditions (part never uploaded vs checksum mismatch) into one sentence.
Emulator: status 400 and code ValidationException are correct, but the handler builds its own template of the form "the checksum listed for part <partNumber> of <partSource> is not the checksum of the part upload <uploadId> holds". Observed: 'the checksum listed for part 1 of SOURCE1 is not the checksum of the part upload 2000000000 holds'. The part number, part source and upload id in that string are values from the request and from state filling the emulator's template -- they are not part of any requirement; AWS's sentence has no value slots at all.
Why: a client, SDK error handler, test or operator runbook that matches, logs or displays this refusal sees text AWS never produces, and the emulator additionally discloses WHICH of the two conditions applied -- a distinction the service collapses on purpose, so a caller written against the emulator learns a diagnostic that disappears against AWS.
Fix: the message construction in CompleteMultipartReadSetUpload's handler validation (the unknown-part branch and the checksum-mismatch branch of its handle) -- both branches must answer the one fixed sentence above, built from no request value.
Traces: omics-read-set-part-upload-refusals#task#8.

=== Cause 2 of 2: an unresolvable upload is refused with the upload-specific sentence where AWS answers the generic resource sentence ===
AWS: Smithy: CompleteMultipartReadSetUpload declares ResourceNotFoundException httpError 404, whose documentation is "The target resource was not found in the current Region."; its uploadId -> UploadId @required @httpLabel @length(10..36) @pattern "^[0-9]+$". Recorded behavior: for this step the refusal is 404 ResourceNotFoundException with the FIXED sentence "The specified resource does not exist." -- all fixed text, echoing no uploadId, no sequenceStoreId and no part information.
Emulator: status 404 and code ResourceNotFoundException are correct, but the message is the shared upload-specific constant, 'The specified upload does not exist.' (services/service-com-amazonaws-omics-omics/src/state.rs, MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE, read by the CompleteMultipartReadSetUpload and UploadReadSetPart handlers).
Hypothesis: omics words this not-found refusal per the resource CLASS the request could not resolve, so a completion that cannot resolve its target at all answers the generic resource sentence while other upload-reachable conditions get the upload-specific wording; the recording is the requirement for this step either way.
Why: the message is a requirement. A caller that matches or surfaces the refusal of an unresolvable completion sees a sentence AWS does not return for this case, and a caller that branches on the resource-class wording to decide whether to re-initiate the upload or to re-resolve the store takes the wrong branch.
Fix: the not-found classification in CompleteMultipartReadSetUpload's handler and the message constants it reads from crate::state -- this condition must answer AWS's "The specified resource does not exist." rather than the shared upload constant.
Traces: omics-read-set-part-upload-refusals#task#10.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (126 steps, 51 passed, 61 mismatched, 12 clusters, all service=omics). Both clusters diverged only in $.message, with expected and actual status identical:

Cause 1 -- fixture omics-read-set-part-upload-refusals, step task#8, CompleteMultipartReadSetUpload, 400 -> 400, key $.message: expected "One or more of the specified parts could not be found. The part might not have been uploaded, or the specified checksum might not have matched the part's checksum", got 'the checksum listed for part 1 of SOURCE1 is not the checksum of the part upload 2000000000 holds'. (The part number 1, partSource SOURCE1 and upload id 2000000000 are per-run values in the emulator's own template; the defect is the template.) Handler evidence: services/service-com-amazonaws-omics-omics/src/ops/complete_multipart_read_set_upload.rs -- the "the checksum listed for part {} of {} ..." and "upload {} holds no part {} of {}" format! templates in its validation branches.

Cause 2 -- fixture omics-read-set-part-upload-refusals, step task#10, CompleteMultipartReadSetUpload, 404 -> 404, key $.message: expected 'The specified resource does not exist.', got 'The specified upload does not exist.'. State evidence: services/service-com-amazonaws-omics-omics/src/state.rs lines 315-316 define MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE = "The specified upload does not exist.", which this handler returns for the condition.

## Steps to Reproduce
1. Cause 1: CreateSequenceStore; CreateMultipartReadSetUpload(sourceFileType=FASTQ, subjectId, sampleId, name); UploadReadSetPart(partSource="SOURCE1", partNumber=1, payload=<bytes>) and keep the returned checksum. Then call CompleteMultipartReadSetUpload(parts=[{partNumber:1, partSource:"SOURCE1", checksum:<a different string of the same length>}]). Expect 400 ValidationException with the exact fixed sentence "One or more of the specified parts could not be found. The part might not have been uploaded, or the specified checksum might not have matched the part's checksum"; observe the emulator's "the checksum listed for part 1 of SOURCE1 is not the checksum of the part upload <uploadId> holds".
2. Cause 2: against the same (or any) sequence store, call CompleteMultipartReadSetUpload for an uploadId that the service cannot resolve. Expect 404 ResourceNotFoundException with the fixed sentence "The specified resource does not exist."; observe 'The specified upload does not exist.'.
3. Re-replay omics-read-set-part-upload-refusals steps task#8 and task#10 to confirm both causes are closed.
