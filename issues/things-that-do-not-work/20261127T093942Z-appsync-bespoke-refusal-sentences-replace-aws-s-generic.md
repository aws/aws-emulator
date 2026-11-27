Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-create-data-source-relational-source-type-enum
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-create-data-source-relational-source-type-enum appsync-http-data-source-authorization-type-enum appsync-create-graphql-api-lambda-authorizer-ttl-bounds appsync-create-resolver-response-template-length-bounds appsync-resolver-request-template-size-limit appsync-evaluate-code-length-bounds evaluate-code-context-length-floor appsync-introspection-database-name-length-bound appsync-introspection-secret-arn-length-bound appsync-list-apis-max-results-bounds
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|EvaluateCode|4xx->4xx|$.message
Issue-Key: appsync|StartDataSourceIntrospection|4xx->4xx|$.message
Issue-Key: appsync|ListApis|4xx->4xx|$.message
Issue-Key: appsync|ListApis|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261127T093942Z-appsync-enum-member-refusals-use-an-unmodeled-validationexception.md
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync bespoke refusal sentences replace AWS's generic validation-constraint message on length and range members

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
On six length/range/config members the emulator invents its own refusal sentence where AWS publishes a specific text — in three cases AWS's generic Smithy constraint frame, in three cases a short service sentence. The status (400) is right everywhere; only the message diverges, and the message is part of the refusal contract. 6 causes in this issue; all six must be closed before submitting. This is the mirror image of the enum bundle (the emulator emits the generic frame where AWS words it itself); the open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md names the same family for other members but carries none of the replay keys below, so none of these causes is covered by it.

=== Cause 1 of 6: CreateDataSource with relationalDatabaseSourceType=RDS_HTTP_ENDPOINT and no rdsHttpEndpointConfig ===
AWS: Smithy (appsync) CreateDataSource, RelationalDatabaseDataSourceConfig = {relationalDatabaseSourceType, rdsHttpEndpointConfig}; modeled BadRequestException|400. Recorded behavior when the source type is RDS_HTTP_ENDPOINT and rdsHttpEndpointConfig is absent is 400 with the fixed message 'Missing RDS Http config' — entirely fixed text, no request values. It names the missing config block, in the same family as AWS's 'Missing <Type> data source config' texts.
Emulator: 400 'Invalid endpoint configuration.' — a different sentence that does not say which config block is missing.
Why: the per-type required-config diagnosis is the only way a client learns which block to add; a generic 'Invalid endpoint configuration.' cannot be matched or acted on.
Fix: the relational-database config validation in the CreateDataSource handler (and UpdateDataSource).
Traces: appsync-create-data-source-relational-source-type-enum#task#3 (cluster also covers appsync-http-data-source-authorization-type-enum#task#6).

=== Cause 2 of 6: CreateGraphqlApi lambdaAuthorizerConfig.authorizerResultTtlInSeconds above its ceiling ===
AWS: Smithy (appsync) CreateGraphqlApi, LambdaAuthorizerConfig$authorizerResultTtlInSeconds: TTL @range 0-3600 (default 0, documented maximum one hour); modeled BadRequestException|400. Recorded behavior for a value above 3600 is 400 with the fixed message 'Authorizer result TTL cannot be > 3600' — fixed text including the literal ceiling as AWS words it.
Emulator: 400 'Invalid authorizerResultTtlInSeconds given.' — it names the member but drops the rule and the ceiling.
Why: the ceiling in the message is what tells a caller what to lower the value to.
Fix: the LambdaAuthorizerConfig range validation in the CreateGraphqlApi handler (and UpdateGraphqlApi).
Traces: appsync-create-graphql-api-lambda-authorizer-ttl-bounds#task#4 (cluster also covers #task#5).

=== Cause 3 of 6: CreateResolver responseMappingTemplate over its size limit ===
AWS: Smithy (appsync) CreateResolver, responseMappingTemplate: MappingTemplate @length 1-65536; modeled BadRequestException|400. Recorded behavior for a template longer than the ceiling is 400 with the fixed message 'Invalid response template given, must be less than 64kb in size.' — fixed text; it states the limit in kb, as AWS words this class (the request-template variant is worded the same way for 'request template').
Emulator: 400 'Invalid responseMappingTemplate given. Must be between 1 and 65536 characters long.' — the member's wire name and a byte range instead of AWS's sentence.
Why: same refusal, non-AWS text; a client or gate asserting the documented size diagnosis does not match.
Fix: the mapping-template length validation shared by the CreateResolver/UpdateResolver handlers (request and response variants each have their own wording).
Traces: appsync-create-resolver-response-template-length-bounds#task#7 (cluster also covers #task#8 and appsync-resolver-request-template-size-limit#task#6/#task#8).

=== Cause 4 of 6: EvaluateCode code over its length ceiling ===
AWS: Smithy (appsync) EvaluateCode — POST /v1/dataplane-evaluatecode, code: Code @required @length 1-32768; modeled BadRequestException|400. Recorded behavior for code longer than 32768 is 400 with the generic constraint frame "1 validation error detected: Value at 'code' failed to satisfy constraint: Member must have length less than or equal to 32768" — fixed frame; the variable parts are the member name 'code' and the modeled ceiling, and the submitted value is NOT echoed.
Emulator: 400 'Code must be 32768 bytes or less' — a hand-written sentence in place of the modeled frame.
Why: this member is validated by AWS's generic constraint layer, so clients see the standard frame for it; a bespoke sentence breaks anything keyed on the documented constraint text.
Fix: the code-length validation in the EvaluateCode handler must go through the generic constraint-message formatter.
Traces: appsync-evaluate-code-length-bounds#task#2 (cluster also covers #task#3 and evaluate-code-context-length-floor#task#2).

=== Cause 5 of 6: StartDataSourceIntrospection rdsDataApiConfig.resourceArn not matching the cluster-ARN pattern ===
AWS: Smithy (appsync) StartDataSourceIntrospection — POST /v1/datasources/introspections, rdsDataApiConfig: RdsDataApiConfig; modeled BadRequestException|400, NotFoundException|404. Recorded behavior for a resourceArn that is not an RDS cluster ARN is 400 with the generic constraint frame "1 validation error detected: Value '<arn>' at 'rdsDataApiConfig.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z-]*:rds:[a-z0-9-]*:\d{12}:cluster:[0-9A-Za-z_/-]*$" — fixed frame, with the submitted ARN and the anchored pattern as the variable parts (for this member AWS does include the value).
Emulator: 400 'Invalid input resource arn' — neither the member path nor the pattern.
Why: without the member path and the pattern, a caller passing a DB-instance ARN where a cluster ARN is required cannot tell what shape is expected.
Fix: the rdsDataApiConfig validation in the StartDataSourceIntrospection handler must emit the modeled constraint message for the member.
Traces: appsync-introspection-database-name-length-bound#task#3 (cluster also covers #task#4 and appsync-introspection-secret-arn-length-bound#task#1/#task#2/#task#4).

=== Cause 6 of 6: ListApis maxResults above its range ===
AWS: Smithy (appsync) ListApis — GET /v2/apis, maxResults: MaxResults @httpQuery @range 0-25 (default 0); modeled BadRequestException|400. Recorded behavior for a value above 25 is 400 with the generic constraint frame "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25" — the member name and the modeled ceiling are the variable parts; the value is not echoed.
Emulator: 400 'maxResults must be greater than 0 and less than or equal to 25.' with x-amzn-errortype BadRequestException; besides replacing the frame it states a floor of > 0, where the modeled range is 0-25 (0 is the member's default and is accepted).
Why: pagination wrappers key on the documented range message; and the invented floor misstates the contract for maxResults=0.
Fix: the maxResults range validation in the ListApis handler must use the generic constraint-message formatter and the modeled 0-25 range.
Traces: appsync-list-apis-max-results-bounds#task#2 (cluster also covers #task#3).

## Evidence
Cause 1 — appsync-create-data-source-relational-source-type-enum#task#3 CreateDataSource: 400 both sides; $.message expected 'Missing RDS Http config', got 'Invalid endpoint configuration.'
Cause 2 — appsync-create-graphql-api-lambda-authorizer-ttl-bounds#task#4 CreateGraphqlApi: $.message expected 'Authorizer result TTL cannot be > 3600', got 'Invalid authorizerResultTtlInSeconds given.'
Cause 3 — appsync-create-resolver-response-template-length-bounds#task#7 CreateResolver: $.message expected 'Invalid response template given, must be less than 64kb in size.', got 'Invalid responseMappingTemplate given. Must be between 1 and 65536 characters long.'
Cause 4 — appsync-evaluate-code-length-bounds#task#2 EvaluateCode: $.message expected "1 validation error detected: Value at 'code' failed to satisfy constraint: Member must have length less than or equal to 32768", got 'Code must be 32768 bytes or less'.
Cause 5 — appsync-introspection-database-name-length-bound#task#3 StartDataSourceIntrospection: $.message expected "1 validation error detected: Value 'arn:aws:rds:us-west-2:000000000000:db:<cluster>' at 'rdsDataApiConfig.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z-]*:rds:[a-z0-9-]*:\\d{12}:cluster:[0-9A-Za-z_/-]*$", got 'Invalid input resource arn'.
Cause 6 — appsync-list-apis-max-results-bounds#task#2 ListApis: $.message expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25", got 'maxResults must be greater than 0 and less than or equal to 25.'; $headers.x-amzn-errortype shape_mismatch, got 'BadRequestException'.

## Steps to Reproduce
Cause 1: CreateDataSource(apiId, name, type='RELATIONAL_DATABASE', relationalDatabaseConfig={'relationalDatabaseSourceType':'RDS_HTTP_ENDPOINT'}) with no rdsHttpEndpointConfig -> 400 'Missing RDS Http config'.
Cause 2: CreateGraphqlApi(authenticationType='AWS_LAMBDA', lambdaAuthorizerConfig={'authorizerUri':<arn>,'authorizerResultTtlInSeconds':3601}) -> 400 'Authorizer result TTL cannot be > 3600'.
Cause 3: CreateResolver(apiId, typeName, fieldName, responseMappingTemplate=<65537 chars>) -> 400 'Invalid response template given, must be less than 64kb in size.'
Cause 4: EvaluateCode(runtime={'name':'APPSYNC_JS','runtimeVersion':'1.0.0'}, code=<32769 chars>, context='{}') -> 400 with the generic 'Value at \'code\' ... length less than or equal to 32768' frame.
Cause 5: StartDataSourceIntrospection(rdsDataApiConfig={'resourceArn':'arn:aws:rds:us-west-2:000000000000:db:x','secretArn':<arn>,'databaseName':'db'}) -> 400 with the generic frame naming 'rdsDataApiConfig.resourceArn' and the cluster-ARN pattern.
Cause 6: GET /v2/apis?maxResults=26 -> 400 with the generic 'Value at \'maxResults\' ... value less than or equal to 25' frame; GET /v2/apis?maxResults=0 must succeed.
