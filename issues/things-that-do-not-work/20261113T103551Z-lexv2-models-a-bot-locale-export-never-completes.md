Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.bot-locale-export-download-serves-created-content
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.bot-locale-export-download-serves-created-content|run#10|AssertionError|assert export["exportStatus"] == "Completed", (
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# lexv2-models: a bot-locale export never completes — CreateExport deliberately settles every job at 'Failed' and no archive or downloadUrl is ever served

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
One net-new root cause in the lexv2-models export path.

Cause 1 of 1: an accepted export job can never reach 'Completed', so the export data plane does not exist. Failing check ids: lexv2-models.bot-locale-export-download-serves-created-content.

AWS: Smithy (com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json) — CreateExport is documented as "Creates a zip archive containing the contents of a bot or a bot locale. The archive contains a directory structure that contains JSON files that define the bot." Its output member exportStatus targets ExportStatus (one of Completed | Deleting | Failed | InProgress) and is documented "The status of the export. When the status is Completed, you can use the DescribeExport operation to get the pre-signed S3 URL link to your exported bot or bot locale." DescribeExport (GET /exports/{exportId}, 200) declares downloadUrl (PresignedS3Url, len 1..1024), documented "A pre-signed S3 URL that points to the bot or bot locale archive. The URL is only available for 5 minutes after calling the DescribeExport operation", plus failureReasons (list) which carries text only for a job that genuinely failed. So for a resourceSpecification.botLocaleExportSpecification naming a bot version and locale that DO resolve, and fileFormat=LexJson, AWS accepts the job (202), DescribeExport reports exportStatus moving InProgress -> Completed with no failureReasons, and downloadUrl is a fetchable URL whose body is a zip (local-file-header signature PK\x03\x04). Recorded behavior for the archive layout (the model does not state it; it is documented at https://docs.aws.amazon.com/lexv2/latest/dg/export.html): entries are keyed by the bot's name and the locale, i.e. <botName>/BotLocales/<localeId>/BotLocale.json and <botName>/BotLocales/<localeId>/Intents/<intentName>/Intent.json, where <botName>, <localeId> and <intentName> are values taken from the request/state, not fixed text. BotLocale.json carries identifier = the localeId created and nluConfidenceThreshold = the nluIntentConfidenceThreshold the locale was created with; Intent.json carries name = the intent's name and sampleUtterances = exactly the utterances CreateIntent was given.

Emulator: CreateExport mints an export id and records the job, but by construction settles it on the FAILED terminal status instead of Completed, and no operation of the crate ever assembles or serves an archive. DescribeExport therefore reports exportStatus='Failed' with failureReasons=['The export archive was not created: this emulator records the export request but does not assemble the archive, so there is nothing to download.'] and no downloadUrl, for a specification that resolved cleanly against the store. The replay fails at the first assertion after the job settles: "AssertionError: the export settled at 'Failed', expected 'Completed'". The handler's own module docs state the intent ("What this records is the JOB, not the zip archive the job assembles: nothing in this emulator produces one ... so the status a recorded job settles on is the modelled failure status carrying the reason that says so"), so this is a deliberate stand-in, not a crash — but it is still a fidelity divergence: AWS has no such status for a valid request, and 'Failed' + a reason string is not a response any real caller can encounter here.

Why: a client that exports a bot locale and then downloads the archive — the ordinary backup/CI/promotion flow for Lex V2 bots, and the whole point of the operation — gets a terminal failure for a well-formed request. Code that branches on exportStatus takes the error path forever; code that reads downloadUrl gets KeyError/None; retry loops burn their budget because the status is terminal. Nothing in the failureReasons text is an AWS string, so a client matching on AWS reasons cannot classify it either. The emulator cannot be used to test any export/import round trip.

Fix: the CreateExport handler and the export job's state machine, plus DescribeExport's serializer and the download endpoint. The job must settle on ExportStatus 'Completed' with empty/absent failureReasons for a specification that resolves, the state must assemble the LexJson archive from the bot/locale/intent/slot-type records already held in the store (keyed <botName>/BotLocales/<localeId>/...), and DescribeExport must emit a downloadUrl the emulator itself serves, returning that zip. Remove the EXPORT_ARCHIVE_UNSUPPORTED_REASON stand-in once the archive exists. Note the open issue 20261031T190907Z-lex-models-v2-createexport-stub.md lists this same check id, but it covers only CreateExport answering NotImplemented/501 as a scaffold; CreateExport now answers and persists a job, so that issue does not cover this divergence.

Traces: lexv2-models.bot-locale-export-download-serves-created-content. Emulator code: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_export.rs lines 15-19 (the documented decision to settle on the failure status) and lines 236-254 (the job record built via crate::state::Export::created); the reason constant and settling clock live in services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/state.rs (EXPORT_ARCHIVE_UNSUPPORTED_REASON, EXPORT_SETTLES_AFTER).

## Evidence
Check lexv2-models.bot-locale-export-download-serves-created-content [mismatch].

Setup the check performs, all of which succeeded: CreateBot -> Available; CreateBotLocale(botVersion=DRAFT, localeId=en_US, nluIntentConfidenceThreshold=0.40) -> leaves Creating; CreateIntent(intentName=OrderPizza, sampleUtterances=[{utterance: "I want to order a pizza"}]); CreateExport(resourceSpecification={botLocaleExportSpecification:{botId, botVersion:DRAFT, localeId:en_US}}, fileFormat=LexJson) -> returns an exportId.

Expected (AWS): DescribeExport(exportId) polls to exportStatus == 'Completed', failureReasons absent/empty, downloadUrl present; GET of downloadUrl returns HTTP 200 whose first four bytes are PK\x03\x04; the zip contains <botName>/BotLocales/en_US/BotLocale.json (identifier == 'en_US', nluConfidenceThreshold == 0.4) and <botName>/BotLocales/en_US/Intents/OrderPizza/Intent.json (name == 'OrderPizza', sampleUtterances == the one utterance created).

Actual (emulator): DescribeExport settles at exportStatus = 'Failed' with
  failureReasons = ['The export archive was not created: this emulator records the export request but does not assemble the archive, so there is nothing to download.']
and no downloadUrl, so the download, the zip signature and every archive-content assertion are never reached.

Replay output:
  File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  assert export["exportStatus"] == "Completed", (
  AssertionError: the export settled at 'Failed', expected 'Completed'; failureReasons=['The export archive was not created: this emulator records the export request but does not assemble the archive, so there is nothing to download.']

Message anatomy: the failureReasons string above is wholly emulator-authored — AWS emits no such text, and for this request AWS emits no failureReasons at all. The fixed text AWS owes here is the enum value 'Completed' in exportStatus; the variable parts of the archive contract (botName, localeId 'en_US', intentName 'OrderPizza', the utterance, the 0.40 threshold) are values taken from the request and the store, not literals of this one run.

## Steps to Reproduce
1. Boot the emulator and point a botocore lexv2-models client at it.
2. CreateBot(botName=<any>, roleArn=<any>, dataPrivacy={childDirected:false}, idleSessionTTLInSeconds=300, botType='Bot'); poll DescribeBot until botStatus == 'Available'.
3. CreateBotLocale(botId=<id>, botVersion='DRAFT', localeId='en_US', nluIntentConfidenceThreshold=0.40); poll DescribeBotLocale until botLocaleStatus != 'Creating'.
4. CreateIntent(botId=<id>, botVersion='DRAFT', localeId='en_US', intentName='OrderPizza', sampleUtterances=[{'utterance':'I want to order a pizza'}]).
5. CreateExport(resourceSpecification={'botLocaleExportSpecification':{'botId':<id>,'botVersion':'DRAFT','localeId':'en_US'}}, fileFormat='LexJson') -> note exportId.
6. Poll DescribeExport(exportId=<exportId>) until exportStatus is terminal. Observe exportStatus == 'Failed' with the emulator-authored failureReasons text and NO downloadUrl, where AWS reaches 'Completed' with a downloadUrl.
7. There is no URL to fetch, so the zip and its BotLocale.json / Intent.json entries cannot be checked at all.
