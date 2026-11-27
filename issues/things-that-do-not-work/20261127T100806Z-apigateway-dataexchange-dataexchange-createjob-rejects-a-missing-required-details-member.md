Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected
Issue-Key: apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected|run#18|AssertionError|assert without_protocol == expected, (
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# dataexchange CreateJob rejects a missing required Details member with the generic Smithy constraint message instead of DataExchange's "The following parameter(s) are not valid" wording

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Cause 1 of 1: DataExchange CreateJob refuses a request that omits a required member of the Type-specific Details structure with a generically-generated Smithy/Coral constraint-violation message rather than DataExchange's own ValidationException wording, so the error text (and therefore any client that parses or asserts on it) diverges. Failing check: apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected

AWS: Smithy (services/service-com-amazonaws-dataexchange-dataexchange) models CreateJob POST /v1/jobs with Details (RequestDetails) and Type both required, and ImportAssetFromApiGatewayApiRequestDetails.ProtocolType as @required (alongside ApiId, ApiName, ApiSpecificationMd5Hash, DataSetId, RevisionId, Stage). The modeled client error is ValidationException (HTTP 400) whose Message member "informs you about what was invalid about the request". The model does not state the message text, so the wording is recorded behavior only: the recording shows AWS answering a CreateJob for Type=IMPORT_ASSET_FROM_API_GATEWAY_API whose Details.ImportAssetFromApiGatewayApi omits ProtocolType with code ValidationException and message 'The following parameter(s) are not valid: "Details.ImportAssetFromApiGatewayApi.ProtocolType" is required.' In that message the fixed text is 'The following parameter(s) are not valid: ' then the quoted member path, then ' is required.'; the only variable part is the dotted member path, taken from the request's own shape in the API's PascalCase member names (here Details.ImportAssetFromApiGatewayApi.ProtocolType) — the concrete member name is a value from the request, not part of the template. No job is created by the refused request.
Emulator: the same call is refused with code ValidationException but the message '1 validation error detected: Value null at \'details.importAssetFromApiGatewayApi.protocolType\' failed to satisfy constraint: Member must not be null'. That is the generic constraint-violation template ('<n> validation error(s) detected: Value <v> at \'<path>\' failed to satisfy constraint: <constraint>') with a lowerCamelCase wire path, which is not how DataExchange words a missing required parameter.
Why: a client that branches on, logs, or asserts the refusal text — the suite's own check does — cannot recognise the refusal, and a client that extracts the offending parameter from the message gets a lowerCamelCase wire path ('details.importAssetFromApiGatewayApi.protocolType') instead of the API member path AWS reports ('Details.ImportAssetFromApiGatewayApi.ProtocolType'). The status (400) and the error code (ValidationException) are correct, so only the message diverges, but the message is part of the contract for this error class.
Fix: DataExchange's CreateJob request validation / its ValidationException message formatter — the required-member check for Details and its Type-specific sub-structures must emit DataExchange's 'The following parameter(s) are not valid: "<Member.Path>" is required.' form with the API's PascalCase member path, rather than falling through to the shared Smithy constraint-violation message builder. The accept path (job created in state WAITING with the Details echoed) is not implicated.
Traces: apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected

## Evidence
Check apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected, assertion at check line 140.
Request: CreateJob(Type="IMPORT_ASSET_FROM_API_GATEWAY_API", Details={"ImportAssetFromApiGatewayApi": {ApiId, ApiName, ApiSpecificationMd5Hash, DataSetId, RevisionId, Stage}}) — i.e. every required member of ImportAssetFromApiGatewayApiRequestDetails except ProtocolType (sent with botocore parameter_validation=False so the request reaches the service).
Expected (code, message): ('ValidationException', 'The following parameter(s) are not valid: "Details.ImportAssetFromApiGatewayApi.ProtocolType" is required.')
Actual (code, message): ('ValidationException', "1 validation error detected: Value null at 'details.importAssetFromApiGatewayApi.protocolType' failed to satisfy constraint: Member must not be null")
Raised as: AssertionError: a job request without ProtocolType must be refused, got ('ValidationException', "1 validation error detected: Value null at 'details.importAssetFromApiGatewayApi.protocolType' failed to satisfy constraint: Member must not be null")
The HTTP status and error code match AWS; the divergence is confined to the message text and the casing/style of the member path it names.

## Steps to Reproduce
1. Build and boot the emulator; point botocore apigateway and dataexchange clients at it.
2. Create a REST API with a deployed stage (CreateRestApi, PutMethod/PutIntegration on /, CreateDeployment stageName="published").
3. dataexchange.create_data_set(AssetType="API_GATEWAY_API", Name=<unique>) and create_revision(DataSetId=...).
4. With a dataexchange client built with botocore Config(parameter_validation=False), call create_job(Type="IMPORT_ASSET_FROM_API_GATEWAY_API", Details={"ImportAssetFromApiGatewayApi": {"ApiId": <api id>, "ApiName": "sim2real-api", "ApiSpecificationMd5Hash": <base64 md5 of the spec>, "DataSetId": <id>, "RevisionId": <id>, "Stage": "published"}}) — note: no ProtocolType.
5. Observe HTTP 400 ValidationException with message "1 validation error detected: Value null at 'details.importAssetFromApiGatewayApi.protocolType' failed to satisfy constraint: Member must not be null" instead of 'The following parameter(s) are not valid: "Details.ImportAssetFromApiGatewayApi.ProtocolType" is required.'
6. Re-replay apigateway__dataexchange.import-api-asset-job-without-protocol-type-rejected to confirm a fix (the check also asserts the refused request leaves no jobs on the revision, and that the same request with ProtocolType="REST" is created WAITING with Details echoed).
