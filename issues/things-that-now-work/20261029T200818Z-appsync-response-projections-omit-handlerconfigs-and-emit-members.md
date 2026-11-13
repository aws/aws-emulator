Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-event-api-and-channel-namespace-workflow
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-event-api-and-channel-namespace-workflow appsync-event-api-channel-namespace appsync-update-channel-namespace-code-handlers-contract appsync-channel-namespace-code-handlers-persist appsync-event-api-update-error-recovery appsync-cfg-datasource-type-defaults appsync-cfg-graphql-api-owner-contact appsync-graphql-api-core-lifecycle appsync-graphql-api-crud-lifecycle appsync-graphql-api-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateChannelNamespace|2xx->2xx|$.channelNamespace.handlerConfigs
Issue-Key: appsync|GetChannelNamespace|2xx->2xx|$.channelNamespace.handlerConfigs
Issue-Key: appsync|UpdateChannelNamespace|2xx->2xx|$.channelNamespace.handlerConfigs
Issue-Key: appsync|GetChannelNamespace|2xx->2xx|$.channelNamespace.publishAuthModes
Issue-Key: appsync|GetChannelNamespace|2xx->2xx|$.channelNamespace.subscribeAuthModes
Issue-Key: appsync|UpdateApi|4xx->4xx|$.message
Issue-Key: appsync|CreateDataSource|2xx->2xx|$.dataSource.serviceRoleArn
Issue-Key: appsync|UpdateGraphqlApi|2xx->2xx|$.graphqlApi.dns
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync response projections omit handlerConfigs and emit members AWS leaves absent (7 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannelNamespace

## Description
The emulator's serializers disagree with AWS about which optional members appear in a success reply: the channel-namespace projection omits members AWS always emits (handlerConfigs, and the two auth-mode lists), while the data-source and graphql-api projections emit members AWS leaves absent. One Event-API refusal sentence in the same area is also emulator-authored. 7 net-new causes.

Cause 1 of 7 — CreateChannelNamespace omits channelNamespace.handlerConfigs.
AWS: the reply always carries handlerConfigs, as an empty object when no OnPublish/OnSubscribe handler is configured (Smithy: ChannelNamespace.handlerConfigs targets HandlerConfigs {onPublish, onSubscribe}).
Emulator: the member is absent from the reply.
Why: a client that reads response.channelNamespace.handlerConfigs.onPublish dereferences a missing object and fails, rather than seeing "no handlers configured".
Fix: the CreateChannelNamespace serializer must emit handlerConfigs (empty when unset).
Traces: appsync-event-api-and-channel-namespace-workflow#task#6, appsync-event-api-channel-namespace#task#7.

Cause 2 of 7 — GetChannelNamespace omits handlerConfigs.
AWS: handlerConfigs present (empty when no handlers).
Emulator: absent.
Fix: the GetChannelNamespace serializer's projection.
Traces: appsync-event-api-and-channel-namespace-workflow#task#8, appsync-event-api-channel-namespace#task#9, appsync-update-channel-namespace-code-handlers-contract#task#3.

Cause 3 of 7 — UpdateChannelNamespace omits handlerConfigs.
AWS: handlerConfigs present (empty when no handlers).
Emulator: absent.
Fix: the UpdateChannelNamespace serializer's projection.
Traces: appsync-channel-namespace-code-handlers-persist#task#3, appsync-event-api-and-channel-namespace-workflow#task#10, appsync-event-api-channel-namespace#task#12, appsync-update-channel-namespace-code-handlers-contract#task#4.

Cause 4 of 7 — GetChannelNamespace also omits publishAuthModes and subscribeAuthModes.
AWS: both lists are present, as empty lists when the namespace does not override the Api's default authorization (Smithy: ChannelNamespace.publishAuthModes / .subscribeAuthModes target AuthModes, a list).
Emulator: both absent.
Why: a client cannot distinguish "no override configured" (empty list) from "the field was not returned", and iterating the member fails.
Fix: the GetChannelNamespace serializer must emit both lists, empty when unset.
Traces: appsync-channel-namespace-code-handlers-persist#task#4, appsync-update-channel-namespace-code-handlers-contract#task#5.

Cause 5 of 7 — UpdateApi's invalid Event-API connect-auth refusal is emulator-authored.
AWS: 400 BadRequestException with the fixed sentence "Event API configuration connect auth contains an invalid auth type." — no variable parts (recorded behavior; Smithy: UpdateApi errors include BadRequestException httpError 400; Api.eventConfig targets EventConfig, whose connectionAuthModes carry AuthenticationType values).
Emulator: 'connectionAuthModes specifies authorization mode AWS_IAM, which is not configured as an authorization provider for this API.' — a different sentence that interpolates the mode and diagnoses a different rule (not-configured-as-a-provider rather than invalid-auth-type).
Why: text-asserting clients diverge, and the message points the caller at provider configuration rather than at the invalid auth type.
Fix: UpdateApi validation — the eventConfig connect-auth check must raise AWS's sentence.
Traces: appsync-event-api-update-error-recovery#task#3, appsync-event-api-update-error-recovery#task#4.

Cause 6 of 7 — CreateDataSource emits a serviceRoleArn for a data-source type that has none.
AWS: dataSource.serviceRoleArn is absent when the request supplied no serviceRoleArn (Smithy: DataSource.serviceRoleArn, optional String).
Emulator: fabricates and emits a role ARN of the form arn:aws:iam::<account>:role/<generated-name> for the created data source.
Why: a client that round-trips the data source back into UpdateDataSource re-sends a role that does not exist, and IaC drift detection reports a change the user never made. (The ARN's generated name is per-run; what is filed is that the member is emitted at all.)
Fix: the CreateDataSource handler/serializer must not synthesize a service role when the request omitted one.
Traces: appsync-cfg-datasource-type-defaults#task#11.

Cause 7 of 7 — UpdateGraphqlApi emits a dns map AWS leaves absent on this reply.
AWS: graphqlApi.dns is absent from the UpdateGraphqlApi response for the recorded flow (Smithy: GraphqlApi.dns, optional MapOfStringToString, "the DNS records for the API").
Emulator: emits dns with GRAPHQL and REALTIME entries.
Why: a client diffing the update reply against the stored API sees a member appear that AWS does not return, so idempotent updates look like changes. (The host names themselves are environment-specific and are not the requirement; the presence of the member is.)
Fix: the UpdateGraphqlApi serializer's optional-member projection.
Traces: appsync-cfg-graphql-api-owner-contact#task#3, appsync-graphql-api-core-lifecycle#task#3, appsync-graphql-api-crud-lifecycle#task#3, appsync-graphql-api-lifecycle#task#4.

Related open issue that does NOT cover these: "AppSync Event-API/domain control plane: ... CreateChannelNamespace ..." (20261025T041655Z) — it is an authorization/NotImplemented finding and carries none of these replay keys.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-event-api-and-channel-namespace-workflow#task#6 CreateChannelNamespace 200: $.channelNamespace.handlerConfigs expected {}, got None.
2) appsync-event-api-and-channel-namespace-workflow#task#8 GetChannelNamespace 200: $.channelNamespace.handlerConfigs expected {}, got None.
3) appsync-channel-namespace-code-handlers-persist#task#3 UpdateChannelNamespace 200: $.channelNamespace.handlerConfigs expected {}, got None.
4) appsync-channel-namespace-code-handlers-persist#task#4 GetChannelNamespace 200: handlerConfigs expected {} got None; publishAuthModes expected [] got None; subscribeAuthModes expected [] got None.
5) appsync-event-api-update-error-recovery#task#3 UpdateApi 400: $.message expected 'Event API configuration connect auth contains an invalid auth type.', got 'connectionAuthModes specifies authorization mode AWS_IAM, which is not configured as an authorization provider for this API.'
6) appsync-cfg-datasource-type-defaults#task#11 CreateDataSource 200: $.dataSource.serviceRoleArn expected absent (None), got 'arn:aws:iam::000000000000:role/sim2real-appsync-dsrole-<generated>'.
7) appsync-cfg-graphql-api-owner-contact#task#3 UpdateGraphqlApi 200: $.graphqlApi.dns expected absent (None), got {'GRAPHQL': '<apiId>.appsync-api.us-west-2.amazonaws.com', 'REALTIME': '<apiId>.appsync-realtime-api.us-west-2.amazonaws.com'}.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. Fixtures appsync-event-api-and-channel-namespace-workflow, appsync-event-api-channel-namespace, appsync-channel-namespace-code-handlers-persist, appsync-update-channel-namespace-code-handlers-contract, appsync-event-api-update-error-recovery, appsync-cfg-datasource-type-defaults and appsync-cfg-graphql-api-owner-contact mismatch at the steps under Traces.
3. By hand: CreateApi (Event API), CreateChannelNamespace with no handlers, then Get/UpdateChannelNamespace and check handlerConfigs / publishAuthModes / subscribeAuthModes (causes 1-4); UpdateApi with an eventConfig connectionAuthModes entry whose authType is not an API provider (cause 5); CreateDataSource of a type that needs no service role, with no serviceRoleArn, and check the reply (cause 6); UpdateGraphqlApi (e.g. setting ownerContact) and check for a dns member (cause 7).
