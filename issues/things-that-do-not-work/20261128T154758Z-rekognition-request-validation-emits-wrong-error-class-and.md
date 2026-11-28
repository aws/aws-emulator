Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-celebrity-info-id-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-celebrity-info-id-validation rekognition-delete-faces-face-id-validation rekognition-face-search-collection-refusals rekognition-get-content-moderation-request-validation rekognition-start-celebrity-recognition-video-source-validation
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|GetCelebrityInfo|4xx->4xx|$.Code
Issue-Key: rekognition|GetCelebrityInfo|4xx->4xx|$.Message
Issue-Key: rekognition|GetCelebrityInfo|4xx->4xx|$.__type
Issue-Key: rekognition|DeleteFaces|4xx->4xx|$.message
Issue-Key: rekognition|ListFaces|4xx->4xx|$.message
Issue-Key: rekognition|GetContentModeration|4xx->4xx|$.message
Issue-Key: rekognition|StartCelebrityRecognition|4xx->4xx|$.message
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# Rekognition request validation emits wrong error class and wrong constraint message text for blank, list-member and pattern-constrained inputs

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCelebrityInfo

## Description
Five net-new root causes in how the emulated Rekognition service refuses a malformed request: one validates too late and returns the wrong error CLASS, two render a list-member constraint violation with a path and value shape AWS does not use, and two strip the anchors off the regular expression they quote. All five are the request-validation layer, not the operations' business logic.

No open issue covers these. The open rekognition issues are unimplemented-operation stubs (DetectModerationLabels, ListProjectPolicies, DetectText, DisassociateFaces, GetFaceDetection, StartCelebrityRecognition, StartContentModeration, StartSegmentDetection, StartTextDetection), DetectLabels label content (20261127T000000Z-..., 20261127T065852Z-...), celebrity catalogue content (20261125T185721Z-...), a CloudFormation name-conflict event, and the PPE byte-limit/video-job bundle (20261127T144756Z-rekognition-ppe-4-mib-image-limit-not-enforced.md). Cause 5 below reaches StartCelebrityRecognition's INPUT VALIDATION, which the emulator does serve and which answers before the stub, so the open StartCelebrityRecognition stub issue does not cover it.

=== Cause 1 of 5: GetCelebrityInfo with a blank Id looks the celebrity up instead of refusing the parameter ===
AWS: GetCelebrityInfo.Id is required and constrained to RekognitionUniqueId @pattern ^[0-9A-Za-z]*$, which an empty string satisfies, so the service applies its own non-blank check BEFORE any catalogue lookup and refuses with HTTP 400, error code InvalidParameterException and the message "Id cannot be blank" (Smithy: GetCelebrityInfo errors include both InvalidParameterException and ResourceNotFoundException at httpError 400; InvalidParameterException docs: "Input parameter violated a constraint. Validate your parameter before calling the API operation again."). "Id cannot be blank" is entirely fixed text whose only variable part is the input member's name, Id; it interpolates no value from the request.
Emulator: it skips the blank check, performs the lookup with the empty id, and reports the lookup's failure — HTTP 400 with $.__type and $.Code both "ResourceNotFoundException" and $.Message "No Celebrity found with passed Id".
Why: the two error classes mean opposite things to a caller. InvalidParameterException says "your request is malformed, fix it and never retry"; ResourceNotFoundException says "this id does not exist, the request itself was fine". A client that distinguishes a bad argument from a miss (to validate user input, to decide whether to fall back to a different id, or to classify the failure in metrics) takes the wrong branch, and a validation bug in the caller is reported as a missing celebrity.
Fix: GetCelebrityInfo's input validation — the required-parameter/non-blank check must run before the state lookup and raise InvalidParameterException with AWS's sentence.
Traces: rekognition-celebrity-info-id-validation#task#1.

=== Cause 2 of 5: DeleteFaces renders a FaceIds member-pattern violation per element instead of as one list-level constraint ===
AWS: DeleteFaces.FaceIds is a required FaceIdList (@length 1-4096) whose member is FaceId @pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$, and a member violation is reported as ONE validation error against the LIST, with the offending values rendered as a bracketed collection and the member rule nested inside a "Member must satisfy constraint: [...]" clause: "<n> validation error(s) detected: Value '[<offending values>]' at '<listMemberName>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: <pattern>]". Fixed text: the "N validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: " / "]" scaffolding. Variable parts: the error count, the bracketed list of the request's rejected member values, the lowerCamel input member path (faceIds), and the pattern as AWS quotes it for FaceId — unanchored, without ^ and $.
Emulator: it reports the ELEMENT, not the list: "1 validation error detected: Value 'face-0001' at 'faceIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-..." — an indexed member path 'faceIds.1.member' that AWS does not emit, the bare value instead of the bracketed list, and the nested "Member must satisfy constraint: [...]" clause dropped.
Why: the message is the only machine-readable description of a constraint failure on this surface; tests and client code that assert on, parse or display AWS's wording (and in particular that read the member path out of it to point a user at the offending field) do not match, and an indexed path like 'faceIds.1.member' misattributes the failure to one element of a rule AWS reports against the whole list.
Fix: the shared constraint-violation message builder used by DeleteFaces' validation — list-member violations must collapse to the list path with the bracketed value list and the nested member-constraint clause.
Traces: rekognition-delete-faces-face-id-validation#task#3 (also task#4, task#5).

=== Cause 3 of 5: ListFaces renders the same FaceIds violation the same wrong way ===
AWS: ListFaces.FaceIds is the same FaceIdList shape (@length 1-4096, member FaceId with the UUID pattern), so the refusal uses the identical list-level template described in cause 2 — "1 validation error detected: Value '[<values>]' at 'faceIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: <FaceId pattern>]" (Smithy: ListFaces.FaceIds -> FaceIdList; errors include InvalidParameterException at httpError 400).
Emulator: again the per-element form, "... Value 'not-a-face-id' at 'faceIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ...". This is a second operation exhibiting the shared builder's defect, so a fix confined to DeleteFaces would leave it diverging.
Why: same as cause 2 — the refusal text and the field path a client surfaces to its user are wrong on the read path as well as the delete path.
Fix: the same shared constraint-violation message builder, reached from ListFaces' validation.
Traces: rekognition-face-search-collection-refusals#task#5.

=== Cause 4 of 5: GetContentModeration quotes the JobId pattern with its anchors stripped ===
AWS: GetContentModeration.JobId is required, JobId @length 1-64 @pattern ^[a-zA-Z0-9-_]+$, and a violation is refused with HTTP 400 and the message "1 validation error detected: Value '<value>' at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" (Smithy: GetContentModeration errors include InvalidParameterException at httpError 400). Fixed text: the "N validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy regular expression pattern: " scaffolding. Variable parts: the rejected value taken verbatim from the request, the lowerCamel member path jobId, and the pattern quoted EXACTLY as the model states it for this member — anchors included.
Emulator: identical except that it emits the pattern as "[a-zA-Z0-9-_]+", having stripped the leading ^ and trailing $ from the modelled pattern.
Why: the quoted pattern is the actionable half of the message — it is what a caller shows a user or uses to pre-validate the next attempt. An unanchored regex means something materially different (it matches any string CONTAINING a legal run), so a client that lifts the pattern out of the message and reuses it will accept values AWS rejects.
Fix: the shared constraint-violation message builder's pattern rendering — it must reproduce the member's modelled pattern verbatim rather than normalising its anchors away.
Traces: rekognition-get-content-moderation-request-validation#task#2.

=== Cause 5 of 5: StartCelebrityRecognition quotes the ClientRequestToken pattern with its anchors stripped ===
AWS: StartCelebrityRecognition.ClientRequestToken is ClientRequestToken @length 1-64 @pattern ^[a-zA-Z0-9-_]+$, and a violating token is refused with HTTP 400 and "1 validation error detected: Value '<token>' at 'clientRequestToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" — same template as cause 4, with the request's token as the quoted value, clientRequestToken as the member path and the anchored pattern (Smithy: StartCelebrityRecognition is @idempotent; errors include InvalidParameterException at httpError 400).
Emulator: emits the pattern as "[a-zA-Z0-9-_]+" — anchors stripped, confirming the defect is in the shared renderer and not in one operation's hand-written string.
Why: same as cause 4, and on an idempotency token, where callers commonly derive or sanitise the token programmatically from the quoted pattern.
Fix: the same shared constraint-violation message builder, reached from StartCelebrityRecognition's validation.
Traces: rekognition-start-celebrity-recognition-video-source-validation#task#3.

## Evidence
Cause 1 — GetCelebrityInfo, rekognition-celebrity-info-id-validation#task#1: status 400 on both sides; expected $.__type and $.Code 'InvalidParameterException' with $.Message 'Id cannot be blank'; actual $.__type and $.Code 'ResourceNotFoundException' with $.Message 'No Celebrity found with passed Id'.

Cause 2 — DeleteFaces, rekognition-delete-faces-face-id-validation#task#3: status 400 on both sides; expected $.message "1 validation error detected: Value '[face-0001]' at 'faceIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}]"; actual $.message "1 validation error detected: Value 'face-0001' at 'faceIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}". Same cluster: task#4, task#5. Diffs: value not bracketed, path 'faceIds.1.member' instead of 'faceIds', nested "Member must satisfy constraint: [...]" clause absent.

Cause 3 — ListFaces, rekognition-face-search-collection-refusals#task#5: expected $.message "1 validation error detected: Value '[not-a-face-id]' at 'faceIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9a-f]{8}-...-[0-9a-f]{12}]"; actual "... Value 'not-a-face-id' at 'faceIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ...".

Cause 4 — GetContentModeration, rekognition-get-content-moderation-request-validation#task#2: expected $.message "1 validation error detected: Value 'job!id' at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$"; actual the same sentence ending "pattern: [a-zA-Z0-9-_]+" — the ^ and $ are missing. ('job!id' is the fixture's value filling the template.)

Cause 5 — StartCelebrityRecognition, rekognition-start-celebrity-recognition-video-source-validation#task#3: expected $.message "1 validation error detected: Value 'token with spaces' at 'clientRequestToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$"; actual the same sentence ending "pattern: [a-zA-Z0-9-_]+". ('token with spaces' is the fixture's value filling the template.)

## Steps to Reproduce
1. Point an AWS SDK/botocore Rekognition client at the emulator's data endpoint.
2. Cause 1: call GetCelebrityInfo(Id='') and read (Code, Message). AWS: ('InvalidParameterException', 'Id cannot be blank'). Emulator: ('ResourceNotFoundException', 'No Celebrity found with passed Id').
3. Cause 2: create a collection, then call DeleteFaces(CollectionId=<id>, FaceIds=['face-0001']) and read the message. AWS reports one error against 'faceIds' with the value bracketed and the member rule nested in "Member must satisfy constraint: [...]"; the emulator reports it against 'faceIds.1.member' with the bare value and no nested clause.
4. Cause 3: call ListFaces(CollectionId=<id>, FaceIds=['not-a-face-id']) and compare the message the same way.
5. Cause 4: call GetContentModeration(JobId='job!id') and check whether the quoted pattern is '^[a-zA-Z0-9-_]+$' (AWS) or '[a-zA-Z0-9-_]+' (emulator).
6. Cause 5: call StartCelebrityRecognition(Video={'S3Object':{'Bucket':'b','Name':'clip.mp4'}}, ClientRequestToken='token with spaces') and check the quoted pattern the same way.
7. Or re-replay the fixture steps named in Issue-Trace/Issue-Step and trace_ids with the wire corpus.
