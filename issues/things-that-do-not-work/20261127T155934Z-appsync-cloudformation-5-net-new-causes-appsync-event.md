Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudformation.deploy-channel-namespace-cfn-getatt-arn-is-service-assigned appsync__cloudformation.deploy-appsync-data-source-template-sets-read-only-arn-ignored appsync__cloudformation.deploy-event-api-cfn-template-api-id-overridden-by-minted-id appsync__cloudformation.deploy-channel-namespace-cfn-api-id-change-replaces-old-api-publish-rejected appsync__cloudformation.deploy-event-api-cfn-getatt-dns-http-serves-publish appsync__cloudformation.deploy-event-api-cfn-dns-http-endpoint-rejects-publish-to-undeclared-namespace appsync__cloudformation.deploy-graphql-api-cfn-getatt-arn-addresses-api-tags
Issue-Key: appsync__cloudformation.deploy-channel-namespace-cfn-getatt-arn-is-service-assigned|run#8|AssertionError|assert status == "CREATE_COMPLETE", f"the stack settled at {status!r}, expected CREATE_COMPLETE"
Issue-Key: appsync__cloudformation.deploy-appsync-data-source-template-sets-read-only-arn-ignored|run#19|AssertionError|assert updated["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Key: appsync__cloudformation.deploy-event-api-cfn-template-api-id-overridden-by-minted-id|run#8|AssertionError|assert status == "CREATE_COMPLETE", f"the stack settled at {status!r}, expected CREATE_COMPLETE"
Issue-Key: appsync__cloudformation.deploy-channel-namespace-cfn-api-id-change-replaces-old-api-publish-rejected|run#17|AssertionError|assert status == "UPDATE_COMPLETE", f"the ApiId update settled at {status!r}, expected UPDATE_COMPLETE"
Issue-Key: appsync__cloudformation.deploy-event-api-cfn-getatt-dns-http-serves-publish|run#8|AssertionError|assert status == "CREATE_COMPLETE", f"the stack settled at {status!r}, expected CREATE_COMPLETE"
Issue-Key: appsync__cloudformation.deploy-event-api-cfn-dns-http-endpoint-rejects-publish-to-undeclared-namespace|run#17|AssertionError|assert (code, answer) == (401, {"errors": [{"errorType": "UnauthorizedException"}]}), (
Issue-Key: appsync__cloudformation.deploy-graphql-api-cfn-getatt-arn-addresses-api-tags|_tier_is#23|Timeout|held = appsync.list_tags_for_resource(resourceArn=arn)["tags"]
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync__cloudformation: 5 net-new causes — AppSync event-API stacks roll back, read-only properties fail the stack, ApiId change does not replace, undeclared-namespace publish is a 404

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct causes in the CloudFormation deployment of AppSync resources; each must be closed before submitting.

Cause 1 of 5: a template that writes a read-only property makes the stack fail, where CloudFormation accepts it and the service-assigned value wins.
AWS: AWS::AppSync::ChannelNamespace with a ChannelNamespaceArn written into Properties, AWS::AppSync::DataSource with DataSourceArn written into Properties, and AWS::AppSync::Api with ApiId written into Properties all deploy normally — CloudFormation takes the service's own assigned value, so Fn::GetAtt and Ref resolve to the minted ARN/id (e.g. the namespace ARN is exactly <ApiArn>/channelNamespace/<name>, never the ARN the template spelled) and the stack settles CREATE_COMPLETE / UPDATE_COMPLETE.
Emulator: these stacks settle at ROLLBACK_COMPLETE on create and UPDATE_ROLLBACK_COMPLETE on update (the DataSource case reports failures [('Local', 'UPDATE_FAILED')]).
Why: a template that deploys on AWS cannot be deployed here at all, so no resource exists and every dependent output is unavailable.
Fix: the AppSync resource providers' property handling — read-only/derived properties present in a template must be ignored, not rejected.
Traces: appsync__cloudformation.deploy-channel-namespace-cfn-getatt-arn-is-service-assigned, appsync__cloudformation.deploy-appsync-data-source-template-sets-read-only-arn-ignored, appsync__cloudformation.deploy-event-api-cfn-template-api-id-overridden-by-minted-id

Cause 2 of 5: changing ApiId on AWS::AppSync::ChannelNamespace does not replace the resource.
AWS: ApiId is a replacement property: an UpdateStack that points the namespace at another API creates the namespace on the new API, deletes it from the old one, and settles UPDATE_COMPLETE — after which a publish to the old API's namespace is refused.
Emulator: the update settles UPDATE_ROLLBACK_COMPLETE, so the namespace never moves.
Why: a client's stack update fails and the deployment is left on the previous API.
Fix: the AWS::AppSync::ChannelNamespace provider's update path — mark ApiId as requiring replacement and implement create-then-delete.
Traces: appsync__cloudformation.deploy-channel-namespace-cfn-api-id-change-replaces-old-api-publish-rejected

Cause 3 of 5: an AWS::AppSync::Api stack whose output reads Fn::GetAtt Dns.Http rolls back.
AWS: AWS::AppSync::Api exposes Dns.Http (and Dns.Realtime) through Fn::GetAtt; the stack settles CREATE_COMPLETE and the value is the endpoint that serves /event publishes.
Emulator: the stack settles ROLLBACK_COMPLETE, so there is no endpoint to publish to.
Why: the documented way to wire an event API's endpoint into other resources does not deploy.
Fix: the AWS::AppSync::Api provider's GetAtt attribute support (Dns.Http / Dns.Realtime).
Traces: appsync__cloudformation.deploy-event-api-cfn-getatt-dns-http-serves-publish

Cause 4 of 5: a publish through a CFN-deployed event API to an undeclared namespace is answered 404 instead of 401.
AWS: an Event API HTTP publish to a channel whose namespace is not declared is answered HTTP 401 with {"errors": [{"errorType": "UnauthorizedException"}]} — errorType only, and the missing namespace is not disclosed.
Emulator: answers HTTP 404 with {"errors": [{"errorType": "NotFoundException", "message": "Channel namespace not found."}]}.
Why: the status and code a client branches on are both wrong, and namespace existence leaks.
Fix: the Event API publish authorization path reached through the stack's Dns.Http endpoint.
Traces: appsync__cloudformation.deploy-event-api-cfn-dns-http-endpoint-rejects-publish-to-undeclared-namespace

Cause 5 of 5: a GraphQL-API stack whose output reads Fn::GetAtt Arn never settles.
AWS: AWS::AppSync::GraphQLApi exposes Arn through Fn::GetAtt, the stack settles CREATE_COMPLETE, and that ARN addresses the API's tags through ListTagsForResource.
Emulator: the check never completes — it times out after 60 seconds, so the stack reached no terminal state.
Why: the caller hangs instead of getting a stack outcome.
Fix: the AWS::AppSync::GraphQLApi provider's create/settle path and its Arn attribute.
Traces: appsync__cloudformation.deploy-graphql-api-cfn-getatt-arn-addresses-api-tags

## Evidence
Cause 1: 'the stack settled at ROLLBACK_COMPLETE, expected CREATE_COMPLETE' (ChannelNamespace with a templated ChannelNamespaceArn, and AWS::AppSync::Api with a templated ApiId); 'update naming DataSourceArn finished as UPDATE_ROLLBACK_COMPLETE, failures [(\'Local\', \'UPDATE_FAILED\')]'.
Cause 2: 'the ApiId update settled at UPDATE_ROLLBACK_COMPLETE, expected UPDATE_COMPLETE'.
Cause 3: 'the stack settled at ROLLBACK_COMPLETE, expected CREATE_COMPLETE' for the Fn::GetAtt [Events, Dns.Http] template.
Cause 4: 'publish to an undeclared namespace answered 404 with {\'errors\': [{\'errorType\': \'NotFoundException\', \'message\': \'Channel namespace not found.\'}]}'.
Cause 5: 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
(1) CreateStack with an AWS::AppSync::ChannelNamespace whose Properties include ChannelNamespaceArn (Fn::Sub of a fabricated ARN) and outputs Fn::GetAtt [<ns>, ChannelNamespaceArn]; likewise a DataSource naming DataSourceArn, and an AWS::AppSync::Api naming ApiId. (2) UpdateStack moving a ChannelNamespace's ApiId to a second AWS::AppSync::Api. (3) CreateStack with an AWS::AppSync::Api output Fn::GetAtt [Events, Dns.Http]. (4) POST /event through that endpoint to a channel in a namespace the stack did not declare. (5) CreateStack with an AWS::AppSync::GraphQLApi output Fn::GetAtt [Api, Arn] and read ListTagsForResource on it. Replay the checks in trace_ids.
