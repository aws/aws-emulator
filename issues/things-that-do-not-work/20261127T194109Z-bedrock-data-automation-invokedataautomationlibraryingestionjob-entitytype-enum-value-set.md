Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation.ingestion-job-invalid-entity-type-rejected
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation.ingestion-job-invalid-entity-type-rejected|run#9|AssertionError|assert message == (
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# bedrock-data-automation InvokeDataAutomationLibraryIngestionJob entityType enum value set omits RECOGNIZED_PERSON, so the ValidationException message is wrong (1 cause)

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeDataAutomationLibraryIngestionJob

## Description
Cause 1 of 1: the enum value set the emulator enforces (and reports) for InvokeDataAutomationLibraryIngestionJob's required `entityType` member is missing the RECOGNIZED_PERSON member, so the synchronous ValidationException for an out-of-enum entityType names the wrong supported set.

AWS: InvokeDataAutomationLibraryIngestionJob refuses an `entityType` outside the EntityType enum with HTTP 400 ValidationException (Smithy: InvokeDataAutomationLibraryIngestionJob.entityType -> EntityType, required, type enum; modelled error ValidationException, client, httpError 400 — model path services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json). The message is AWS's standard constraint-violation template: the fixed text `<N> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<comma-separated enum members>]`, where N, the member name and the member list are the variable parts. The recording shows AWS enumerating TWO members for this operation, so the recorded message for a single bad entityType is `1 validation error detected: Value at 'entityType' failed to satisfy constraint: Member must satisfy enum value set: [VOCABULARY, RECOGNIZED_PERSON]` — the fixed text is everything but `1`, `entityType` and the bracketed list; `1` and `entityType` come from the request, and the bracketed list is the service's supported EntityType set (VOCABULARY and RECOGNIZED_PERSON), not a value of one run.

Emulator: it refuses with the right code (ValidationException) and the right template, but its enum value set has only one member: the message reads `1 validation error detected: Value at 'entityType' failed to satisfy constraint: Member must satisfy enum value set: [VOCABULARY]`. The on-disk Smithy shape com.amazonaws.bedrockdataautomation#EntityType declares values: ["VOCABULARY"] only, and the centralized deserialize -> validate walk builds both the acceptance test and the message from that set.

Why: a client that reads the refusal to learn which entityTypes are supported is told RECOGNIZED_PERSON does not exist, and message-exact error handling (tests, retry/branch logic keyed on the supported-set text) diverges from AWS. Hypothesis: because the same value set gates acceptance, a legitimate `entityType=RECOGNIZED_PERSON` ingestion job is also refused with ValidationException instead of being accepted — the check only asserts the message, so the acceptance side is not directly observed here.

Fix: the EntityType value set used by this service's centralized input validation for InvokeDataAutomationLibraryIngestionJob (the enum constraint validation that renders the 'Member must satisfy enum value set' message, and the enum shape the generated validator reads) must carry both VOCABULARY and RECOGNIZED_PERSON, in that order, and must accept RECOGNIZED_PERSON wherever an entityType is taken (ingestion, GetDataAutomationLibraryEntity, listings).

Traces: behavioral check bedrock-data-automation.ingestion-job-invalid-entity-type-rejected (assertion at check_bedrock_data_automation_ingestion_job_invalid_entity_type_rejected.py:44). Relevant source: services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/invoke_data_automation_library_ingestion_job.rs:59 (doc records `entityType` as "one of: VOCABULARY") and the enum shape in that crate's smithy-model JSON.

## Evidence
Expected (real AWS, recorded): code ValidationException, message "1 validation error detected: Value at 'entityType' failed to satisfy constraint: Member must satisfy enum value set: [VOCABULARY, RECOGNIZED_PERSON]".
Actual (emulator): code ValidationException, message "1 validation error detected: Value at 'entityType' failed to satisfy constraint: Member must satisfy enum value set: [VOCABULARY]".
Suite output: AssertionError: unexpected message: "1 validation error detected: Value at 'entityType' failed to satisfy constraint: Member must satisfy enum value set: [VOCABULARY]" (check_bedrock_data_automation_ingestion_job_invalid_entity_type_rejected.py line 44).
Model evidence: smithy_query shape EntityType -> {"type": "enum", "values": ["VOCABULARY"]} — the emulator's set is short one member relative to the recorded AWS set.

## Steps to Reproduce
1. CreateDataAutomationLibrary to get a libraryArn; create an S3 output bucket.
2. Call InvokeDataAutomationLibraryIngestionJob(libraryArn=<arn>, entityType="NOT_A_REAL_ENTITY_TYPE", operationType="UPSERT", inputConfiguration={inlinePayload:{upsertEntitiesInfo:[{vocabulary:{entityId:<id>, language:"EN", phrases:[{text:"x"}]}}]}}, outputConfiguration={s3Uri:"s3://<bucket>/output/"}).
3. Observe the ClientError: Code is ValidationException (correct) but Message's enum value set is "[VOCABULARY]" instead of "[VOCABULARY, RECOGNIZED_PERSON]".
4. Also call the same operation with entityType="RECOGNIZED_PERSON" and a matching payload: AWS accepts it; confirm whether the emulator refuses it with the same ValidationException.
