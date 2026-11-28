Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.attachment-scanner-verdict-sets-transcript-attachment-status
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant.attachment-scanner-verdict-sets-transcript-attachment-status|run#15|AssertionError|assert final == expected, f"expected transcript verdicts {sorted(expected.values())!r}, got {sorted(final.values())!r}"
Issue-Commit: b2bfeaac456e7e589126ea50cb0c365f97af1ebe

# connectparticipant: FILE_SCANNER verdict is never applied — GetTranscript reports every completed attachment as APPROVED

> _Note: this issue was identified against the code base at commit `b2bfeaac456e7e589126ea50cb0c365f97af1ebe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTranscript

## Description
Cause 1 of 1: the emulator never consults the instance's FILE_SCANNER integration when settling a chat attachment, so AttachmentItem.Status in GetTranscript is hard-APPROVED for every completed upload. Failing check: connectparticipant.attachment-scanner-verdict-sets-transcript-attachment-status.

AWS: Smithy (com.amazonaws.connectparticipant) models AttachmentItem.Status -> ArtifactStatus, an enum whose values are exactly APPROVED | REJECTED | IN_PROGRESS, documented as "Status of the attachment." The model itself does not state how the value is chosen, so for the selection rule this is recorded behavior plus AWS's documented attachment-scanning contract (https://docs.aws.amazon.com/connect/latest/adminguide/setup-attachment-scanning.html): when the Amazon Connect instance has an integration association of IntegrationType FILE_SCANNER pointing at a Lambda function, Connect invokes that function for each chat file once its bytes are uploaded and CompleteAttachmentUpload is called, passing the file's metadata (including File.FileName), and records the verdict the function returns in its response Status field. The attachment then surfaces in GetTranscript's Transcript[].Attachments[] with Status = that verdict: REJECTED for a file the scanner turns away, APPROVED for one it allows, and IN_PROGRESS while the scan has not yet returned. The verdict is therefore a function of the scanner's reply only — it is not derivable from the file name, size, content type or extension configuration, all of which are identical for both files in the recording except the random name the scanner's own code keys on.

Emulator: with a FILE_SCANNER integration association created on the instance (connect CreateIntegrationAssociation, IntegrationType=FILE_SCANNER, IntegrationArn = the Lambda), two .txt attachments uploaded via StartAttachmentUpload + pre-signed PUT + CompleteAttachmentUpload both settle to Status "APPROVED". The scanner function, whose handler returns {"Status": "REJECTED"} for the one file name baked into its source and {"Status": "APPROVED"} otherwise, is evidently never invoked: the file it turns away is reported APPROVED. The check polls GetTranscript for up to 180s for a final (non-IN_PROGRESS) verdict on both attachments, and gets a terminal APPROVED for both, so this is not a timing artifact — the emulator reaches a steady state with the wrong verdict.

Why: a client relying on AWS's behavior uses AttachmentItem.Status as the authoritative malware/content gate before it offers a chat file to an agent or an end user. Against this emulator that gate is inert: every file passes regardless of what the customer's scanner says, so a test suite or an application that validates "the scanner blocks this file" cannot fail, and code paths that handle REJECTED (suppressing the attachment, surfacing a blocked-file notice, skipping the GetAttachment download) are never exercised. A FILE_SCANNER integration configured against the emulator silently has no effect.

Fix: the attachment state machine behind CompleteAttachmentUpload and its projection in the GetTranscript handler for this service — the completion path must look up the contact's instance for a FILE_SCANNER integration association, invoke the associated Lambda with the file event, and store the returned verdict on the attachment (holding it IN_PROGRESS until the invocation returns) instead of writing a constant APPROVED; GetTranscript must then serialize that stored verdict in AttachmentItem.Status. The cross-service hook (reading the connect integration association and invoking lambda) belongs with it.

Traces: connectparticipant.attachment-scanner-verdict-sets-transcript-attachment-status. Not covered by any open connectparticipant issue: 20261126T092626Z-...attachment-data-path-does-not-complete-the (duplicate-CompleteAttachmentUpload budget), 20261127T101140Z-...sendevent-sendmessage-startattachmentupload (requests AWS refuses that are accepted), 20261127T101140Z-...refusal-messages-are (refusal message text) and 20261109T180950Z-...error-paths-miss-accessdenied (error paths) are all about refusals and message text, not the scanner verdict on a successful upload.

## Evidence
Check connectparticipant.attachment-scanner-verdict-sets-transcript-attachment-status, assertion at line 85:

  AssertionError: expected transcript verdicts [('a6aa424d4b984b46ba3a1136f8c14d43.txt', 'REJECTED'), ('f3f986b20a9c4e1ca6052cbab73c742a.txt', 'APPROVED')], got [('a6aa424d4b984b46ba3a1136f8c14d43.txt', 'APPROVED'), ('f3f986b20a9c4e1ca6052cbab73c742a.txt', 'APPROVED')]

(The two .txt names are per-run uuid4 values; the requirement is the mapping, not the literal names — the file whose name the attached scanner Lambda returns REJECTED for must come back REJECTED, the other APPROVED.)

Expected (AWS): GetTranscript -> Transcript[].Attachments[] entry for the scanner-rejected file has Status "REJECTED" (ArtifactStatus enum), the other "APPROVED"; Status may read "IN_PROGRESS" transiently before the scan returns.
Actual (emulator): both entries settle to Status "APPROVED" and stay there for the full 180s poll budget; the scanner Lambda's verdict has no effect on the response.

No HTTP status or error-code divergence is involved — GetTranscript returns 200 in both cases; the divergence is the value of AttachmentItem.Status.

## Steps to Reproduce
1. connect CreateInstance (IdentityManagementType=CONNECT_MANAGED, InboundCallsEnabled=true) and wait for InstanceStatus ACTIVE.
2. Create an S3 bucket, associate it as the instance's ATTACHMENTS storage config (AssociateInstanceStorageConfig, StorageType=S3) and UpdateAttachedFilesConfiguration with AttachmentScope=CHAT, AllowedExtensions=[{Extension: "txt"}], MaximumSizeLimitInBytes=1048576.
3. Create a Lambda (python3.13) whose handler returns {"Status": "REJECTED"} when event["File"]["FileName"] equals one chosen name and {"Status": "APPROVED"} otherwise; AddPermission for principal connect.amazonaws.com.
4. connect CreateIntegrationAssociation(InstanceId=..., IntegrationType="FILE_SCANNER", IntegrationArn=<that function ARN>).
5. Publish a simple greeting contact flow, StartChatContact on it, then connectparticipant CreateParticipantConnection(Type=["CONNECTION_CREDENTIALS"]) to get a ConnectionToken.
6. For each of two distinct .txt names (one the scanner rejects, one it allows): StartAttachmentUpload(ContentType="text/plain", AttachmentName=<name>, AttachmentSizeInBytes=len(body), ClientToken=uuid, ConnectionToken=token), PUT the body to UploadMetadata.Url with HeadersToInclude, then CompleteAttachmentUpload(AttachmentIds=[AttachmentId], ClientToken=uuid, ConnectionToken=token).
7. Poll GetTranscript(ConnectionToken=token, SortOrder="ASCENDING", MaxResults=100, paging on NextToken) until both AttachmentIds appear with a non-IN_PROGRESS Status.
8. Observe both Attachments report Status "APPROVED"; AWS reports "REJECTED" for the name the scanner turns away.
