Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifyuibuilder.start-codegen-job-unknown-data-source-type-rejected
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder.start-codegen-job-unknown-data-source-type-rejected|run#7|AssertionError|assert (status, code, message) == (400, "400", ""), (
Issue-Commit: 680677ada8878ce293f02ee08799bbda12b1d232

# StartCodegenJob refuses an out-of-enum genericDataSchema.dataSourceType with the generic "N validation errors detected" ValidationException text instead of amplifyuibuilder's untyped 400 `errors` envelope

> _Note: this issue was identified against the code base at commit `680677ada8878ce293f02ee08799bbda12b1d232`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCodegenJob

## Description
Cause 1 of 1: a payload constraint violation in StartCodegenJob is rendered with the generic AWS validation-error message instead of the untyped 400 refusal envelope amplifyuibuilder's REST front end returns for a body that does not match the modelled shape. Failing check: amplifyuibuilder.start-codegen-job-unknown-data-source-type-rejected.

AWS: StartCodegenJob (POST /app/{appId}/environment/{environmentName}/codegen-jobs, codegenJobToCreate is the @httpPayload, Smithy: StartCodegenJobData.genericDataSchema -> CodegenJobGenericDataSchema, whose dataSourceType targets enum CodegenJobGenericDataSourceType with the single value [DataStore]). The model declares InvalidParameterException (HTTP 400) for this operation, but it does NOT describe how a payload that fails shape validation is refused; the recorded behavior is the requirement here. The recording shows a body-shape violation refused BEFORE the modelled error channel, as an UNTYPED HTTP 400: no error-type header or __type, so botocore surfaces Code "400" and an EMPTY Message (the body carries no top-level "message"/"Message" key), and the body is the front end's own envelope:
  {"errors": {"message": "One or more request parameters were not valid", "fields": {"<offending top-level input member>": {"message": "Could not match the intersection against every type. Issues: <json array>", "value": <that member echoed exactly as sent>}}}}
Fixed text in that envelope: "One or more request parameters were not valid", "Could not match the intersection against every type. Issues: " and, inside each issue entry, "should be one of the following; ". The variable parts come from the request and the model: the field key is the top-level input member that failed (here codegenJobToCreate), the issue entry is keyed by the full dotted path of the offending member (<top-level member>.genericDataSchema.dataSourceType), its "value" is the rejected value taken from the request, its allowed-value list is the target enum's values from the model (for CodegenJobGenericDataSourceType: ['DataStore']), and fields.<member>.value echoes the member exactly as the client sent it. The issues array is compact JSON (no spaces after separators). No codegen job is created by the refused request, and the same request with dataSourceType DataStore starts a job that ListCodegenJobs then lists.

Emulator: the same request is refused HTTP 400 with Code "400" (correct) but a NON-EMPTY Message, the generic constraint text emitted by the shared deserialize -> validate walk: "1 validation error detected: Value 'Relational' at 'codegenJobToCreate.genericDataSchema.dataSourceType' failed to satisfy constraint: Member must satisfy enum value set: [DataStore]" (here 'Relational' is the value from the request and [DataStore] the enum's modelled values; the "<n> validation error(s) detected: Value '<v>' at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [...]" wording is the fixed template). Because that text is serialized as a top-level message, botocore reports it as the error Message, and the body carries no "errors" object at all — so errors.message, errors.fields, the per-field "Could not match the intersection against every type. Issues: ..." message and the echoed value are all absent.

Why: a client that reads amplifyuibuilder's documented refusal envelope to find WHICH field was rejected and WHAT it sent (errors.fields[<member>].message / .value, and the per-member issue naming the dotted path and the allowed values) finds nothing on this emulator and cannot report or repair the bad field; a client that instead keys off the empty Message of an untyped 400 sees an unexpected message and may classify the refusal as a different error class. The status code is the only part that matches.

Fix: belongs in the refusal serialization of StartCodegenJob's request validation — the centralized validate stage of the request walk described in services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/start_codegen_job.rs (deserialize -> validate -> handle -> serialize; the handler body itself is never reached, so this is not a change to handle()). For this service, a constraint violation inside the @httpPayload member must be rendered as the untyped 400 `errors` envelope above (top-level fixed message, one entry per failing top-level input member, the member echoed in "value", and the dotted member path + allowed-value list from the model inside the Issues array) rather than as a generic validation message with a top-level "message" key.

Note: StartCodegenJob itself is implemented and does refuse this request (the stub issue 20261022T142124Z-amplifyuibuilder-startcodegenjob-stub.md is fixed and the refusal here is the expected one), so this is not a reopened stub; the defect is purely the shape and wording of the validation refusal. No open amplifyuibuilder issue listed covers the validation-refusal envelope.

## Evidence
Check amplifyuibuilder.start-codegen-job-unknown-data-source-type-rejected, StartCodegenJob with codegenJobToCreate.genericDataSchema.dataSourceType set to a value outside the modelled enum:

Expected (recorded AWS): (status, Code, Message) == (400, "400", ""), body == {"errors": {"message": "One or more request parameters were not valid", "fields": {"codegenJobToCreate": {"message": "Could not match the intersection against every type. Issues: [{\"codegenJobToCreate.genericDataSchema.dataSourceType\":{\"message\":\"should be one of the following; ['DataStore']\",\"value\":\"<value sent>\"}}]", "value": <codegenJobToCreate echoed as sent>}}}}

Actual (emulator): status 400, Code '400', Message "1 validation error detected: Value 'Relational' at 'codegenJobToCreate.genericDataSchema.dataSourceType' failed to satisfy constraint: Member must satisfy enum value set: [DataStore]"; the response body carries that text under a top-level message key and has no "errors" object, so the assertion
    assert (status, code, message) == (400, "400", "")
failed with: AssertionError: refusal was status 400, code '400', message "1 validation error detected: ..."; expected an untyped 400 with an empty message

Model (smithy_query, services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json): StartCodegenJob input member codegenJobToCreate -> StartCodegenJobData @httpPayload @required; StartCodegenJobData.genericDataSchema -> CodegenJobGenericDataSchema; CodegenJobGenericDataSourceType is an enum with values ["DataStore"]; operation errors: InternalServerException(500), InvalidParameterException(400), ThrottlingException(429).

## Steps to Reproduce
1. CreateApp via amplify to obtain an appId.
2. Call amplifyuibuilder StartCodegenJob with appId, environmentName="codegen" and codegenJobToCreate = {"renderConfig": {"react": {"module": "esnext", "target": "es2020", "script": "tsx"}}, "genericDataSchema": {"dataSourceType": "Relational", "models": {"Note": {"fields": {"id": {"dataType": "ID", "dataTypeValue": "ID", "required": true, "readOnly": false, "isArray": false}}, "primaryKeys": ["id"]}}, "enums": {}, "nonModels": {}}} — any dataSourceType outside the modelled enum reproduces it.
3. Capture the raw HTTP response (e.g. an after-call hook on amplifyuibuilder.StartCodegenJob) and the ClientError.
4. Observe status 400 with Code '400' but a non-empty Message carrying "1 validation error detected: ..." and a body with no "errors" object; AWS returns the untyped 400 whose body is {"errors": {"message": "One or more request parameters were not valid", "fields": {"codegenJobToCreate": {...}}}} and whose client-visible Message is empty.
5. Re-run: amplifyuibuilder.start-codegen-job-unknown-data-source-type-rejected.
