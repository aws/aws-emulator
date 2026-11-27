Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.delete-slot-type-rejected-while-a-slot-uses-it lexv2-models.deleted-locale-import-forgets-its-record-but-keeps-the-imported-locale lexv2-models.imported-custom-vocabulary-batch-edits-persist lexv2-models.start-import-rejects-append-for-bot-archive lexv2-models.start-import-rejects-malformed-test-set-name lexv2-models.start-import-rejects-unissued-import-id lexv2-models.test-set-import-serves-uploaded-records lexv2-models.update-test-set-renames-what-describe-and-list-serve lexv2-models.vocabulary-batch-create-rejects-existing-and-repeated-phrases-per-entry lexv2-models.vocabulary-batch-delete-rejects-unknown-id-beside-deleted-entry lexv2-models.vocabulary-batch-update-rejects-every-entry-and-changes-nothing
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.delete-slot-type-rejected-while-a-slot-uses-it|run#19|AssertionError|assert (code, status) == ("ConflictException", 409), (
Issue-Key: lexv2-models|CreateUploadUrl|NotImplemented
Issue-Key: lexv2-models.start-import-rejects-unissued-import-id|run#26|AssertionError|assert got == (_GONE, _GONE_MSG), (
Issue-Commit: fff8505842aad03b204d6100e6a2dc09c6204c11

# lexv2-models: 3 net-new causes — CreateUploadUrl, StartImport and DeleteSlotType are still 501 scaffold stubs, so the whole import/upload flow and the slot-type in-use refusal are unreachable

> _Note: this issue was identified against the code base at commit `fff8505842aad03b204d6100e6a2dc09c6204c11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUploadUrl

## Description
Three Lex Models V2 operations still answer HTTP 501 `NotImplemented` with the scaffold's own sentence instead of the behaviour the model declares for them. Each is an independent cause with its own handler file; all three must be closed before this issue is submitted.

Cause 1 of 3: CreateUploadUrl answers 501 NotImplemented, so no import can even be started.
AWS: Smithy lexv2-models `CreateUploadUrl` — POST /createuploadurl, success status 200, output `importId` (Id, @length min 10 max 10, @pattern ^[0-9a-zA-Z]+$) and `uploadUrl` (PresignedS3Url, @length 1..1024); modelled errors are ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). The model documents it as "Gets a pre-signed S3 write URL that you use to upload the zip archive when importing a bot or a bot locale." 501/NotImplemented is not a response this operation can produce.
Emulator: HTTP 501, error code `NotImplemented`, message `operation CreateUploadUrl is scaffolded but not implemented in this emulator`. The handler still carries the SCAFFOLDED marker and returns `Scaffolded::not_implemented("CreateUploadUrl")` unconditionally.
Why: a client cannot obtain an upload target, so nothing downstream of it exists — bot-locale import, test-set import and custom-vocabulary import all die on the first call. Nine behavioural checks never reach the behaviour they were written to grade (deleted-locale import bookkeeping, custom-vocabulary batch create/update/delete per-entry refusals, test-set import and rename, StartImport's append/merge-strategy and test-set-name refusals).
Fix: the CreateUploadUrl handler — it must mint a 10-character alphanumeric importId, register it as an issued import id in the service state so StartImport can resolve it, and serve a presigned-style write URL the emulator itself can accept the archive on.
Traces: check ids lexv2-models.deleted-locale-import-forgets-its-record-but-keeps-the-imported-locale, lexv2-models.imported-custom-vocabulary-batch-edits-persist, lexv2-models.start-import-rejects-append-for-bot-archive, lexv2-models.start-import-rejects-malformed-test-set-name, lexv2-models.test-set-import-serves-uploaded-records, lexv2-models.update-test-set-renames-what-describe-and-list-serve, lexv2-models.vocabulary-batch-create-rejects-existing-and-repeated-phrases-per-entry, lexv2-models.vocabulary-batch-delete-rejects-unknown-id-beside-deleted-entry, lexv2-models.vocabulary-batch-update-rejects-every-entry-and-changes-nothing. Handler: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_upload_url.rs:163 (and the SCAFFOLDED marker at line 4).

Cause 2 of 3: StartImport answers 501 NotImplemented, so an unissued import id is never refused as absent.
AWS: Smithy lexv2-models `StartImport` — PUT /imports, success status 202, required input `importId` (Id, @length 10..10, @pattern ^[0-9a-zA-Z]+$), `resourceSpecification`, `mergeStrategy`; modelled errors ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400). An importId that CreateUploadUrl never issued is refused with ResourceNotFoundException, whose message for this service is the fixed sentence `Failed to retrieve resource since it does not exist` (recorded behaviour; it carries no request-derived part, and it is the same wording DescribeBot uses for an absent bot). The refusal must also leave no bot named after the import target.
Emulator: the call is answered HTTP 501 with code `NotImplemented` and message `operation StartImport is scaffolded but not implemented in this emulator`, so neither the 404 class nor that message is ever produced.
Why: a client that probes a stale or forged import id gets a server-class 501 it will treat as an emulator fault or retry forever, instead of the 404 that tells it the id is gone; and the import happy path (archive → bot/locale/test-set) does not exist at all.
Fix: the StartImport handler — resolve `importId` against the ids CreateUploadUrl issued, refuse an unknown one with ResourceNotFoundException and that fixed message before any state is written, then record the import and settle its importStatus.
Traces: check id lexv2-models.start-import-rejects-unissued-import-id. Handler: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/start_import.rs.

Cause 3 of 3: DeleteSlotType answers 501 NotImplemented, so a slot type a slot still references is deleted-by-501 rather than refused as in use.
AWS: Smithy lexv2-models `DeleteSlotType` — DELETE /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes/{slotTypeId}, success status 204 with an empty body; modelled errors ConflictException (409), InternalServerException (500), PreconditionFailedException (412), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400). The model's own documentation states: "Deletes a slot type from a bot locale. If a slot is using the slot type, Amazon Lex throws a ResourceInUseException exception. To avoid the exception, set the skipResourceInUseCheck parameter to true." On the wire that in-use refusal is ConflictException with HTTP 409, and its message is `Resource in use: SlotType (id: <slotTypeId>) is being used by existing slots in the locale` — the fixed text is `Resource in use: SlotType (id: ` and `) is being used by existing slots in the locale`, and the only variable part is the slotTypeId taken from the request. The refused delete must change nothing: the slot type keeps its slotTypeValues and the slot keeps referencing it. With `skipResourceInUseCheck=true`, or once the slot is deleted, the delete succeeds 204 and DescribeSlotType then answers ResourceNotFoundException / `Failed to retrieve resource since it does not exist`.
Emulator: every DeleteSlotType is answered HTTP 501 with code `NotImplemented` and message `operation DeleteSlotType is scaffolded but not implemented in this emulator`, so neither the 409 in-use refusal nor the 204 success exists.
Why: a client cannot tear a locale down in dependency order, and it cannot distinguish "a slot still uses this type" (fix your model) from a server fault; the 501 is also outside the operation's modelled error set, so a generated SDK surfaces it as an unmodelled error rather than ConflictException.
Fix: the DeleteSlotType handler — resolve the slot type in the addressed bot/version/locale, scan that locale's slots for a reference unless skipResourceInUseCheck is set, refuse with ConflictException and the in-use message above without mutating state, otherwise remove the slot type and answer 204.
Traces: check id lexv2-models.delete-slot-type-rejected-while-a-slot-uses-it. Handler: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/delete_slot_type.rs:199 (SCAFFOLDED marker at line 4).

## Evidence
Cause 1 (CreateUploadUrl) — expected: HTTP 200 with importId (10 alphanumerics) + uploadUrl. Actual, on nine checks: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateUploadUrl operation: operation CreateUploadUrl is scaffolded but not implemented in this emulator.

Cause 2 (StartImport) — expected ('ResourceNotFoundException', 'Failed to retrieve resource since it does not exist'). Actual: ('NotImplemented', 'operation StartImport is scaffolded but not implemented in this emulator'), HTTP 501. Suite assertion: "StartImport with an unissued import id answered ('NotImplemented', 'operation StartImport is scaffolded but not implemented in this emulator')".

Cause 3 (DeleteSlotType) — expected (code, status) == ('ConflictException', 409) with a message starting 'Resource in use: SlotType (id: ' and ending ') is being used by existing slots in the locale'. Actual: ('NotImplemented', 501). Suite assertion: "deleting a slot type a slot uses was refused with ('NotImplemented', 501)".

All three handler files still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and end in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`.

## Steps to Reproduce
Cause 1: call lexv2-models CreateUploadUrl with no input — observe HTTP 501 NotImplemented instead of 200 + {importId, uploadUrl}.

Cause 2: create a bot, CreateExport it to LexJson and wait for Completed; then call StartImport with a freshly generated but never-issued 10-character importId, a botImportSpecification for a new bot name and mergeStrategy=FailOnConflict — observe HTTP 501 NotImplemented instead of 404 ResourceNotFoundException 'Failed to retrieve resource since it does not exist', and confirm no bot is created either way.

Cause 3: create a bot, a DRAFT en_US locale, a custom slot type with sample values ['amber','teal'], an intent, and a slot whose slotTypeId is that slot type; then call DeleteSlotType for that slot type with skipResourceInUseCheck unset — observe HTTP 501 NotImplemented instead of 409 ConflictException 'Resource in use: SlotType (id: <slotTypeId>) is being used by existing slots in the locale'. Delete the slot and retry: the delete should then answer 204 and DescribeSlotType should answer ResourceNotFoundException.
