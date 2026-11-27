Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found
Issue-Key: appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found|_expect#2|AssertionError|raise AssertionError(
Issue-Commit: a9288303feb8dead80b5d9b52ed92320ffa70966

# appsync__cloudformation: a stack delete removes AWS::AppSync::DataSource before the AWS::AppSync::Resolver that DependsOn it, so AppSync's in-use guard refuses and the stack ends DELETE_FAILED

> _Note: this issue was identified against the code base at commit `a9288303feb8dead80b5d9b52ed92320ffa70966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
One net-new root cause in the CloudFormation delete path over AWS::AppSync::* resources. It is not covered by the open issues 20261114T191235Z-appsync-cloudformation-4-net-new-causes-in-place.md (in-place PROPERTY UPDATES of AWS::AppSync::{DataSource,GraphQLSchema,ApiKey} answering NotImplemented) or 20261117T184012Z-cloudformation-9-net-new-cloudformation-causes-change-sets-miss.md (change sets, intrinsic functions, refusal wording): here the create and both updates succeed and the divergence is in the ORDER the stack's resources are deleted.

AWS: Smithy (cloudformation) DeleteStack. Recorded behavior only for the ordering rule, which CloudFormation documents: on a stack delete, resources are deleted in reverse dependency order, so a resource that DependsOn another (and one that references another through Fn::GetAtt/Ref) is deleted BEFORE its dependency. In the recording the stack holds AWS::AppSync::GraphQLApi + GraphQLSchema + DataSource ('Source', Type NONE) + Resolver ('Field', DependsOn [Schema, Source], naming that data source in DataSourceName) + ApiKey; after the test deletes by hand the one resolver that UpdateReplacePolicy: Retain had left outside the stack, DeleteStack takes the stack to DELETE_COMPLETE — the stack's own resolver goes first, then the data source it named — and GetGraphqlApi for the deleted API afterwards answers NotFoundException 'API not found.'
Emulator: the stack ends DELETE_FAILED. The stack-level reason is 'The following resource(s) failed to delete: [Source]. ' and the resource event on Source is 'Resource handler returned message: "Resolver for type Query and field <fieldName> associated with data source" (... HandlerErrorCode: InvalidRequest)' — the data source is deleted while the stack's own resolver on Query.<fieldName> still exists and still names it, so AppSync's in-use guard refuses the DeleteDataSource call. The stack, the API, the schema, the data source and the resolver are all left behind. Secondarily, the refusal sentence the AppSync handler produces is truncated: its fixed text stops at 'associated with data source' without the data source name (a value from the request) or the remedial clause, so even as a refusal it is not AWS's sentence.
Why: no stack that owns both an AppSync data source and a resolver over it can ever be torn down. A client's deploy/destroy cycle (CDK, SAM, plain CloudFormation) leaves an undeletable stack and an orphaned API behind on every iteration, and an operator reading the failure sees a sentence that names no data source and no remedy, so the real cause (the delete order) is invisible.
Fix: the delete orchestration of the CloudFormation engine — compute the reverse-dependency order from DependsOn and from intrinsic references (the Resolver's Fn::GetAtt ApiId / DataSourceName edge) and delete dependents before their dependencies, so the AWS::AppSync::Resolver handler runs before the AWS::AppSync::DataSource handler; and, separately, the AppSync DeleteDataSource refusal message must be a complete sentence naming the data source and the resolver holding it.
Traces: appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout. Check appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found, listed under service heading `### appsync__cloudformation`, verdict [mismatch], not marked ALREADY FILED and not OWNED by any open issue (that service's other seven failures are all OWNED).

  File "check_appsync__cloudformation_cfn_appsync_resolver_in_place_update_retained_replacement_then_not_found.py", line 415, in _delete_stack
    _expect(cfn, _settle(ctx, cfn, stack_id), "DELETE_COMPLETE")
  AssertionError: stack ended 'DELETE_FAILED' ('The following resource(s) failed to delete: [Source]. '), expected 'DELETE_COMPLETE'; failed events: [('Source', 'DELETE_FAILED', 'Resource handler returned message: "Resolver for type Query and field second associated with data source" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'), ('<stack-name>', 'DELETE_FAILED', 'The following resource(s) failed to delete: [Source]. ')]

Everything before the delete passed: CREATE_COMPLETE, the in-place Code update (UPDATE_COMPLETE, physical id unchanged), the FieldName replacement under UpdateReplacePolicy: Retain (UPDATE_COMPLETE, new resolver ARN tracked, both resolvers serving), and the manual DeleteResolver of the RETAINED resolver Query.first (GetResolver then NotFoundException 'No resolver found.'). The only resolver still present at DeleteStack time is the stack's own 'Field' resource, Query.second — the resource that DependsOn 'Source'.

Template shape (abridged): Resources {Api: AWS::AppSync::GraphQLApi, Schema: AWS::AppSync::GraphQLSchema, Source: AWS::AppSync::DataSource {Name: none, Type: NONE}, Field: AWS::AppSync::Resolver {DependsOn: [Schema, Source], UpdateReplacePolicy: Retain, Properties: {TypeName: Query, FieldName: <field>, DataSourceName: none, Kind: UNIT, Runtime: APPSYNC_JS, Code: ...}}, Key: AWS::AppSync::ApiKey}.

Expected (AWS): DELETE_COMPLETE, then GetGraphqlApi -> NotFoundException 'API not found.' and GetResolver Query.second -> NotFoundException 'API not found.'
Actual (emulator): DELETE_FAILED with [Source] failing on AppSync's in-use guard; the API and all its resources survive.

## Steps to Reproduce
1. cfn.create_stack with a template whose Resources are: Api (AWS::AppSync::GraphQLApi, AuthenticationType API_KEY), Schema (AWS::AppSync::GraphQLSchema over Fn::GetAtt Api.ApiId, definition 'type Query { first(msg: String): String second(msg: String): String } schema { query: Query }'), Source (AWS::AppSync::DataSource, Name 'none', Type 'NONE'), Field (AWS::AppSync::Resolver, DependsOn [Schema, Source], UpdateReplacePolicy Retain, TypeName Query, FieldName 'first', DataSourceName 'none', Kind UNIT, Runtime APPSYNC_JS, Code ...), Key (AWS::AppSync::ApiKey). Wait for CREATE_COMPLETE.
2. cfn.update_stack with the same template but a changed resolver Code -> UPDATE_COMPLETE, same physical id (passes today).
3. cfn.update_stack with FieldName changed to 'second' (FieldName is create-only, so this REPLACES the resolver; UpdateReplacePolicy Retain keeps the old one) -> UPDATE_COMPLETE, both Query.first and Query.second present (passes today).
4. appsync.delete_resolver(apiId=..., typeName='Query', fieldName='first') to remove the retained resolver.
5. cfn.delete_stack(StackName=...) and poll.
   Expected: DELETE_COMPLETE — the stack's own Resolver (Query.second) is deleted before the DataSource it DependsOn, then the DataSource, Schema, ApiKey and Api; afterwards GetGraphqlApi answers NotFoundException 'API not found.'
   Observed: DELETE_FAILED, stack reason 'The following resource(s) failed to delete: [Source]. ', and a Source DELETE_FAILED event 'Resource handler returned message: "Resolver for type Query and field second associated with data source" (HandlerErrorCode: InvalidRequest)'. A minimal reproduction is any stack with one AWS::AppSync::DataSource plus one AWS::AppSync::Resolver naming it.
Re-replay appsync__cloudformation.cfn-appsync-resolver-in-place-update-retained-replacement-then-not-found to confirm.
