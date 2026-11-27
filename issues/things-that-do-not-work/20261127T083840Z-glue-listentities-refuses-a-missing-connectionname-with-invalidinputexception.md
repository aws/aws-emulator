Issue-Source: trace-sweep-divergence
Issue-Trace: glue-materialized-view-refresh-and-entity-lookup-without-existing-resources
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-materialized-view-refresh-and-entity-lookup-without-existing-resources
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|ListEntities|4xx->4xx|$.__type
Issue-Key: glue|ListEntities|4xx->4xx|$.message
Issue-Commit: 468af82aaa1d7b60e9b7f70a7d90101f16ac1d70

# Glue ListEntities refuses a missing ConnectionName with InvalidInputException instead of AWS's ValidationException

> _Note: this issue was identified against the code base at commit `468af82aaa1d7b60e9b7f70a7d90101f16ac1d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEntities

## Description
This issue carries 1 cause.

Cause 1 of 1 — ListEntities called without ConnectionName is refused with the wrong error class and an emulator-authored message.

AWS: Smithy (service-com-amazonaws-glue-awsglue): ListEntities input members ConnectionName (NameString, @length 1..255), CatalogId, ParentEntityName, NextToken, DataStoreApiVersion — none of them is marked @required, so there is no modelled required-member constraint for the emulator to echo. The operation models BOTH ValidationException ("A value could not be validated.", member Message, httpError 400) and InvalidInputException ("The input provided was not valid.", members Message/FromFederationSource, httpError 400) among its errors, so the class is a real choice the wire observes. Recorded behavior: AWS answers HTTP 400 with $.__type = "ValidationException" and the body member $.message carrying AWS's standard front-end validation template: the fixed text "<N> validation error(s) detected: Value null at '<member>' failed to satisfy constraint: Member must not be null". In the recording that renders as "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null". Fixed parts of that message: "validation error detected: Value null at '", "' failed to satisfy constraint: Member must not be null". Variable parts: the error count (1 per omitted member) and the member path AWS reports for the omitted input member — for the connection name that path is 'name' (AWS's internal member name), not the public wire name ConnectionName.

Emulator: HTTP 400 (status matches) but with $.__type = "InvalidInputException" and $.message = "ConnectionName must be provided in the request." — a sentence that appears nowhere in AWS's wording, naming the wire member instead of the member path AWS reports, and omitting the validation-error count and the "failed to satisfy constraint: Member must not be null" clause entirely.

Why: ValidationException and InvalidInputException are distinct modelled shapes that SDKs deserialize into distinct exception types and that client code catches separately — a caller with a `catch ValidationException` arm (or an error-mapping/retry layer keyed on __type) never sees the emulator's refusal at all and falls through to its generic handler, while AWS's own front-end validation class is the one a request-shaping client expects for an omitted member. The message divergence compounds it: tooling that parses AWS's "N validation error(s) detected: ... Member must not be null" template to report which member the caller forgot extracts nothing from the emulator's sentence, so the diagnostic that tells a developer what to add is lost.

Fix: the ListEntities handler's input-validation stage (the pre-dispatch required/omitted-member check and the error serializer it raises through) must raise the modelled ValidationException for an omitted ConnectionName and render AWS's front-end validation template — the error count plus the reported member path — rather than raising InvalidInputException with a locally composed sentence. Related but distinct open issue on Glue 400 refusal bodies: 20261127T055032Z-glue-400-refusal-bodies-diverge-an-emulator-authored.md (BatchGetDataQualityRulesetEvaluationRun's invented Message, GetTables' catalog-id error class) — it does not cover ListEntities or the ValidationException class.

Traces: glue-materialized-view-refresh-and-entity-lookup-without-existing-resources#task#2.

## Evidence
Cause 1 (ListEntities, glue-materialized-view-refresh-and-entity-lookup-without-existing-resources#task#2): both expected and actual status are 400, so only the body diverges.
  expected $.__type = "ValidationException"; actual $.__type = "InvalidInputException" (value_mismatch).
  expected $.message = "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null"; actual $.message = "ConnectionName must be provided in the request." (value_mismatch).
Message decomposition: fixed text = "validation error detected: Value null at '" ... "' failed to satisfy constraint: Member must not be null"; variable = the count of omitted members (1 here) and the member path AWS reports for the omitted input member ('name' for the connection name). The emulator's text shares no fixed segment with AWS's.
Smithy evidence used: ListEntities input (ConnectionName NameString @length 1..255, not @required), errors list including both ValidationException and InvalidInputException (each errorKind client, httpError 400); shapes ValidationException{Message} and InvalidInputException{Message, FromFederationSource}.

## Steps to Reproduce
Cause 1: issue a Glue ListEntities request with the ConnectionName member omitted entirely (e.g. an empty request body, or one carrying only CatalogId). The emulator answers HTTP 400 with __type "InvalidInputException" and message "ConnectionName must be provided in the request."; AWS answers HTTP 400 with __type "ValidationException" and the front-end validation message "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null". Replay fixture glue-materialized-view-refresh-and-entity-lookup-without-existing-resources step task#2 to confirm.
