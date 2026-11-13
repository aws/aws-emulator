Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-locale-export-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-locale-export-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|DescribeExport|2xx->2xx|$.downloadUrl
Issue-Key: lexv2-models|DescribeExport|2xx->2xx|$.exportStatus
Issue-Key: lexv2-models|DescribeExport|2xx->2xx|$.failureReasons
Issue-Commit: f4539f1192667be98112ffe93cb2cc2853b85427

# DescribeExport settles a valid bot-locale export at Failed with an emulator-authored failureReason and never serves downloadUrl

> _Note: this issue was identified against the code base at commit `f4539f1192667be98112ffe93cb2cc2853b85427`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExport

## Description
One net-new wire cause in the lexv2-models export path.

Cause 1 of 1: DescribeExport reports a terminal failure for an export job whose resource specification resolved cleanly, so an accepted export can never be polled to completion and no archive URL is ever produced.

AWS: Smithy (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json) — CreateExport is PUT /exports with HTTP 202, taking a required resourceSpecification (ExportResourceSpecification) and a required fileFormat (ImportExportFileFormat); it is documented as creating a zip archive containing the contents of a bot or a bot locale. DescribeExport is GET /exports/{exportId} with HTTP 200 and output members exportStatus (enum ExportStatus: Completed | Deleting | Failed | InProgress), failureReasons (list FailureReasons), and downloadUrl (PresignedS3Url, @length 1..1024), documented as a pre-signed S3 URL pointing to the bot or bot locale archive. exportStatus is documented: when the status is Completed you can use DescribeExport to get the pre-signed S3 URL. So for a botLocaleExportSpecification naming a botId/botVersion/localeId that all exist and fileFormat=LexJson, AWS accepts the job and DescribeExport advances exportStatus InProgress -> Completed, with failureReasons absent (it carries text only for a job that genuinely failed) and downloadUrl present. The only modelled errors on DescribeExport are InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400) — there is no AWS path by which a resolvable, well-formed export specification reaches ExportStatus 'Failed'.

Emulator: the step polls DescribeExport on the exportId CreateExport returned and gets HTTP 200, but the body diverges on three members: exportStatus is 'Failed' where 'Completed' is expected; failureReasons is present as a one-element list where AWS sends none, carrying the wholly emulator-authored text "The export archive was not created: this emulator records the export request but does not assemble the archive, so there is nothing to download."; and downloadUrl is missing entirely where a URL is expected. Because 'Failed' is terminal, the fixture's poll aborts after 2 attempts ("poll: 'Failed' is in fail_on after 2 attempt(s)") and every later step that depends on the archive is unreachable. The job is minted and persisted, so this is a deliberate stand-in rather than a crash — but it is still a wire divergence: no AWS response for this request shape has that status or that text.

Message anatomy: the only fixed text AWS owes in this response is the ExportStatus enum value, which must be 'Completed' for a specification that resolves. The failureReasons string the emulator emits is not AWS wording for any error class — AWS emits no failureReasons member at all here — so it must disappear, not be reworded. The variable parts of the contract (exportId, botId, botVersion, localeId, the archive's botName/localeId/intentName keys) are values taken from the request and the emulator's own state and are never literals of one recorded run; the recorded downloadUrl value in particular is a per-run URL and is asserted only as shaped-not-asserted.

Why: a client that exports a bot locale and then downloads the archive — the ordinary backup, CI and promotion flow for Lex V2, and the entire purpose of the operation — is handed a terminal failure for a well-formed request. Code branching on exportStatus takes the error path permanently; code reading downloadUrl gets None/KeyError; poll loops stop immediately because 'Failed' is terminal rather than retrying. Code classifying failureReasons against AWS reason strings cannot match the emulator's text at all. No export/import round trip can be exercised against this emulator.

Fix: the export job's state machine and the DescribeExport serializer in the lexv2-models crate — the CreateExport handler's settling decision plus the state that holds the job record, and the download endpoint that must exist to back the URL. A job whose resourceSpecification resolves must settle on ExportStatus 'Completed' with failureReasons absent/empty, DescribeExport must then serialize a downloadUrl the emulator itself serves, and the unconditional archive-unsupported reason constant must be removed once an archive is assembled from the bot/locale/intent records already in the store.

Related open issue, which does NOT cover this: 20261113T103551Z-lexv2-models-a-bot-locale-export-never-completes.md describes the same root behavior, but it is a behavioral-corpus issue (Issue-Corpus: simparity-behavioral-aws-v1) keyed on the check id lexv2-models.bot-locale-export-download-serves-created-content and filed against CreateExport; it carries no wire replay key, so the fixture step below is not gated by it and would be closed unrun.

Traces: fixture lex-models-v2-bot-locale-export-lifecycle, step task#7 (DescribeExport, verdict mismatch, 200 -> 200). Emulator code to start from: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_export.rs (the documented decision to settle on the failure status and the job record it builds) and services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/state.rs (the archive-unsupported reason constant and the settling clock).

## Evidence
Divergence cluster service=lexv2-models|DescribeExport|status=200->200|keys=$.downloadUrl,$.exportStatus,$.failureReasons — verdict mismatch, expected_status 200, actual_status 200, affected_count 1.

Fixture lex-models-v2-bot-locale-export-lifecycle, step task#7:
  $.exportStatus  expected 'Completed', got 'Failed'
  $.failureReasons expected None (member absent), got ['The export archive was not created: this emulator records the export request but does not assemble the archive, so there is nothing to download.']
  $.downloadUrl    expected present (<shaped-not-asserted>), got None/missing
  poll: 'Failed' is in fail_on after 2 attempt(s) (expected 'Completed', got 'Failed')

Expected (AWS, per Smithy DescribeExport output): HTTP 200 with exportStatus = 'Completed', no failureReasons member, and a downloadUrl (PresignedS3Url, @length 1..1024) that resolves to the bot-locale archive.

Actual (emulator): HTTP 200 with exportStatus = 'Failed', a one-element failureReasons list carrying emulator-authored prose that matches no AWS reason string, and no downloadUrl member. 'Failed' is terminal, so the fixture's poll gives up and the dependent download/archive steps in the lifecycle never run.

## Steps to Reproduce
1. Boot the emulator and point a lexv2-models client at its data listener.
2. CreateBot(botName=<any>, roleArn=<any>, dataPrivacy={childDirected:false}, idleSessionTTLInSeconds=300); poll DescribeBot until botStatus == 'Available'.
3. CreateBotLocale(botId=<id>, botVersion='DRAFT', localeId='en_US', nluIntentConfidenceThreshold=0.40); poll DescribeBotLocale until botLocaleStatus is no longer 'Creating'.
4. CreateIntent(botId=<id>, botVersion='DRAFT', localeId='en_US', intentName=<any>, sampleUtterances=[{'utterance': <any>}]).
5. CreateExport(resourceSpecification={'botLocaleExportSpecification': {'botId': <id>, 'botVersion': 'DRAFT', 'localeId': 'en_US'}}, fileFormat='LexJson') -> 202 with an exportId.
6. Poll DescribeExport(exportId=<exportId>) until exportStatus is terminal. Observe exportStatus == 'Failed', a failureReasons list containing the emulator's archive-unsupported text, and no downloadUrl — where AWS reaches exportStatus == 'Completed' with no failureReasons and a downloadUrl present.
7. Equivalent to replaying fixture lex-models-v2-bot-locale-export-lifecycle and inspecting step task#7.
