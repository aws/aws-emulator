Issue-Source: trace-sweep-divergence
Issue-Trace: backup-stop-backup-job-job-id-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-stop-backup-job-job-id-validation backup-stop-backup-job-unknown-job
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|StopBackupJob|4xx->4xx|$.Code
Issue-Key: backup|StopBackupJob|4xx->4xx|$.Message
Issue-Key: backup|StopBackupJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 73eb0236dcc4936472514f03502662b62d32fdac

# StopBackupJob refusals omit the Code error member and misclassify a malformed BackupJobId as not-found (2 causes)

> _Note: this issue was identified against the code base at commit `73eb0236dcc4936472514f03502662b62d32fdac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopBackupJob

## Description
StopBackupJob's two refusal paths both diverge, and they share one root fault: the handler resolves the job before it validates the BackupJobId's grammar, and the error it renders never populates the modeled `Code` member. Both causes are on the `backup` service, operation StopBackupJob (POST /backup-jobs/{BackupJobId}).

Cause 1 of 2: a syntactically invalid BackupJobId is answered as a not-found instead of a parameter-value refusal.
  AWS: Smithy: StopBackupJob input member BackupJobId is a required @httpLabel string; StopBackupJob models InvalidParameterValueException, InvalidRequestException, MissingParameterValueException and ResourceNotFoundException, all client errors at HTTP 400, and each of those shapes carries the members Code, Message, Type and Context (Smithy: InvalidParameterValueException {Code, Message, Type, Context}). Recorded behavior: AWS validates the identifier's shape first and refuses with HTTP 400, x-amzn-errortype of the parameter-value error class, `Code` = "ERROR_3406" and `Message` = "The BackupJobID must be a Universally Unique Identifier (UUID) in this format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx. Recheck the ID and try again." That whole message is FIXED text for this error class — it interpolates nothing from the request; the literal "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" is the UUID shape template AWS prints, not a value from the recording.
  Emulator: HTTP 400 with x-amzn-errortype `ResourceNotFoundException`, no `Code` member at all, and `Message` = "BackupJob not found" — an emulator-authored string. So the refusal arrives in the wrong error class with an emulator-invented message and no error code.
  Why: a client that distinguishes "you sent me a malformed id" (non-retryable, fix the input) from "that job does not exist" (a state condition it may poll or tolerate) branches on x-amzn-errortype and on the `Code` member. Here both the class and the code are wrong, so input-validation failures are swallowed as missing resources, and any client keying remediation off ERROR_3406 never sees it.
  Fix: StopBackupJob's input validation in its handler — validate BackupJobId against the UUID grammar before any state lookup, and raise the parameter-value error class with AWS's fixed message; plus the error serializer, which must emit `Code`.
  Traces: backup-stop-backup-job-job-id-validation#task#1, backup-stop-backup-job-unknown-job#task#2.

Cause 2 of 2: the refusal for a well-formed but unknown BackupJobId drops the `Code` member.
  AWS: Smithy: the modeled error shapes for StopBackupJob each declare a `Code` member (ResourceNotFoundException {Code, Message, Type, Context}), and recorded behavior shows AWS populating it: `Code` = "ERROR_2405" on this path. The error code is a fixed per-error-class token, not a per-run value.
  Emulator: the status and message line up, but `Code` is absent from the body (expected 'ERROR_2405', got None) — the error is serialized without its code member.
  Why: clients and tooling that switch on AWS Backup's ERROR_* codes (the stable, message-independent discriminator for Backup refusals) get no discriminator at all and must fall back on parsing prose.
  Fix: the error serializer / error-construction path shared by StopBackupJob's refusals — populate `Code` for every modeled Backup error it emits.
  Traces: backup-stop-backup-job-job-id-validation#task#2, backup-stop-backup-job-unknown-job#task#1.

Related but NOT covering this: the open issues 20261127T091209Z-backup-job-start-and-vault-delete-refusals-diverge.md and 20261127T091209Z-backup-startcopyjob-refusals-drop-the-code-context-error-members.md report the same missing-`Code` family on StartBackupJob/StartRestoreJob/StartScanJob/StartReportJob/DeleteBackupVault and on StartCopyJob respectively; neither carries any StopBackupJob replay key, so StopBackupJob is unfixed and unexercised by them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200).

Cause 1 — fixture backup-stop-backup-job-job-id-validation, step task#1 (cluster also covers backup-stop-backup-job-unknown-job#task#2); expected status 400, actual 400:
  $.Code: missing (expected 'ERROR_3406', got None)
  $.Message: value_mismatch (expected 'The BackupJobID must be a Universally Unique Identifier (UUID) in this format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx. Recheck the ID and try again.', got 'BackupJob not found')
  $headers.x-amzn-errortype: shape_mismatch (expected the parameter-value error class, got 'ResourceNotFoundException')

Cause 2 — fixture backup-stop-backup-job-job-id-validation, step task#2 (cluster also covers backup-stop-backup-job-unknown-job#task#1); expected status 400, actual 400:
  $.Code: missing (expected 'ERROR_2405', got None)

Model evidence (smithy_query, services/service-com-amazonaws-backup-cryocontrollerusermanager): StopBackupJob input {BackupJobId: string, httpLabel, required}; errors InvalidParameterValueException / InvalidRequestException / MissingParameterValueException / ResourceNotFoundException (client, 400) / ServiceUnavailableException (500); each error structure declares members Code, Message, Type, Context.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture `backup-stop-backup-job-job-id-validation` and observe step `task#1`: call StopBackupJob (POST /backup-jobs/{BackupJobId}) with a BackupJobId that is not a UUID. Expected HTTP 400 in the parameter-value error class with Code 'ERROR_3406' and the fixed UUID-format message; the emulator answers 400 ResourceNotFoundException, no Code, Message 'BackupJob not found'.
3. In the same fixture observe step `task#2` (and backup-stop-backup-job-unknown-job#task#1): call StopBackupJob with a well-formed UUID that names no backup job. Status and message match, but the response body has no `Code` member where AWS returns 'ERROR_2405'.
4. Re-replay backup-stop-backup-job-job-id-validation#task#1, backup-stop-backup-job-job-id-validation#task#2 and backup-stop-backup-job-unknown-job#task#1/#task#2 to confirm both causes are closed.
