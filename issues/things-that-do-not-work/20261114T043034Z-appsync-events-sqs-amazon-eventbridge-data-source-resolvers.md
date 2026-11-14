Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__events__sqs.xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only appsync__events__sqs.eventbridge-datasource-batch-entries-route-independently-in-order appsync__events__sqs.eventbridge-datasource-non-object-detail-rejects-whole-batch appsync__events__sqs.eventbridge-datasource-role-without-put-events-denied appsync__events__sqs.eventbridge-datasource-role-scoped-to-other-bus-denied appsync__events__sqs.eventbridge-datasource-update-repoints-mutations-to-new-bus
Issue-Key: appsync__events__sqs.xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only|_invoke_ok#24|AssertionError|raise AssertionError(f"mutation for {order_id!r} failed unexpectedly: {errs!r}")
Issue-Key: appsync__events__sqs.eventbridge-datasource-batch-entries-route-independently-in-order|_invoke_ok#27|AssertionError|raise AssertionError(f"{field} failed unexpectedly: {errs!r}")
Issue-Key: appsync__events__sqs.eventbridge-datasource-non-object-detail-rejects-whole-batch|_invoke_ok#23|AssertionError|raise AssertionError(f"{field} failed unexpectedly: {errs!r}")
Issue-Key: appsync__events__sqs.eventbridge-datasource-role-without-put-events-denied|_invoke_ok#22|AssertionError|raise AssertionError(f"{field} for {order_id!r} failed unexpectedly: {errs!r}")
Issue-Key: appsync__events__sqs.eventbridge-datasource-role-scoped-to-other-bus-denied|_invoke_ok#22|AssertionError|raise AssertionError(f"{field} failed unexpectedly: {errs!r}")
Issue-Key: appsync__events__sqs.eventbridge-datasource-update-repoints-mutations-to-new-bus|_invoke_ok#24|AssertionError|raise AssertionError(f"{field} failed unexpectedly: {errs!r}")
Issue-Commit: 7446e1137d863a6c5f0117bdbe1b0c65f0c299cf

# appsync__events__sqs: AMAZON_EVENTBRIDGE data-source resolvers are never dispatched — the mutation returns a GraphQL error and no event reaches the bus

> _Note: this issue was identified against the code base at commit `7446e1137d863a6c5f0117bdbe1b0c65f0c299cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostGraphQL (AppSync GraphQL data plane) / CreateDataSource (AMAZON_EVENTBRIDGE)

## Description
Cause 1 of 1: a resolver attached to an AMAZON_EVENTBRIDGE data source never performs PutEvents; the GraphQL data plane answers the mutation with a resolver error instead.

AWS: AppSync models AMAZON_EVENTBRIDGE as a first-class data-source type (Smithy: DataSourceType enum values include AMAZON_EVENTBRIDGE; DataSource$eventBridgeConfig -> EventBridgeDataSourceConfig with required member eventBusArn; DataSource$serviceRoleArn is "the IAM service role ARN for the data source. The system assumes this role when accessing the data source"). A UNIT resolver bound to such a data source (Smithy: Resolver$kind UNIT — "run a GraphQL query against a single data source") executes its request mapping template / APPSYNC_JS request function, issues events:PutEvents against the bus named by eventBusArn under the data-source service role, and returns the PutEvents result (per-entry EventId, FailedEntryCount) through the response template. On success the HTTP body is 200 with `data.<field>` populated and NO `errors` member, and the events really land on the bus, so EventBridge rules on that bus deliver them to their targets. When the data-source role is not entitled (no events:PutEvents, or a role scoped to a different bus ARN), AWS surfaces the AccessDeniedException from PutEvents as a resolver error on that field — it does not refuse to call the data source at all.

Emulator: every mutation whose field resolves through an AMAZON_EVENTBRIDGE data source is answered HTTP 200 with `data.<field>` null and `errors: [{"path": ["<field>"], "message": "This endpoint does not call the AMAZON_EVENTBRIDGE data source <dataSourceName>."}]`. The fixed text is "This endpoint does not call the AMAZON_EVENTBRIDGE data source " and the only variable part is the data-source name taken from the resolver being executed (observed values sim2real_eb_orders, sim2real_eb_pairs, sim2real_eb_allowed, sim2real_eb_bus_x, sim2real_eb_repoint — per-run names, not part of the requirement). That message is emulator-authored: AWS has no such error class or wording. Because the call is never made, no event is ever put on the bus, so none of the downstream legs (rule matching, per-entry ordering, batch rejection of a non-object detail, role-scoping denials, re-pointing the data source at a new bus) can be observed either.

Why: a client that drives EventBridge through an AppSync mutation gets a successful HTTP 200 whose payload reports a resolver failure and whose side effect never happened — the bus, its rules and the SQS targets stay empty. Authorization checks built on the data-source role (deny when events:PutEvents is missing or the role is scoped to another bus) cannot be exercised, and a legitimate, fully entitled mutation is indistinguishable from a denied one.

Fix: the AppSync GraphQL data-plane resolver-execution path — the component that resolves a field against its DataSource must implement the AMAZON_EVENTBRIDGE type: assume/attribute the data source's serviceRoleArn, call the events service's PutEvents with the resolved entries against eventBridgeConfig.eventBusArn, map the PutEvents response (and any AccessDenied/validation error) back through the resolver's response template, and only then populate `data`/`errors`. Related open issues cover a different cause (the published uris.GRAPHQL naming an unserved real-AWS host, e.g. 20261110T174759Z-appsync-events-sqs-the-graphql-api-s-published.md): the endpoint is now reached, and this is the next, distinct cause.

Traces: appsync__events__sqs.xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only, appsync__events__sqs.eventbridge-datasource-batch-entries-route-independently-in-order, appsync__events__sqs.eventbridge-datasource-non-object-detail-rejects-whole-batch, appsync__events__sqs.eventbridge-datasource-role-without-put-events-denied, appsync__events__sqs.eventbridge-datasource-role-scoped-to-other-bus-denied, appsync__events__sqs.eventbridge-datasource-update-repoints-mutations-to-new-bus.

## Evidence
Expected (AWS): HTTP 200, body {"data": {"placeOrder": {...}}} with no "errors" member, and the event delivered to the bus named by the data source's eventBridgeConfig.eventBusArn (so the matching rule's SQS target receives the envelope); when the data-source role lacks events:PutEvents or is scoped to another bus, an AccessDeniedException-derived resolver error.

Actual (emulator): HTTP 200, body {"data": {"placeOrder": null}, "errors": [{"path": ["placeOrder"], "message": "This endpoint does not call the AMAZON_EVENTBRIDGE data source sim2real_eb_orders."}]}. Same message, with only the data-source name varying, on all six checks:
- xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only: "mutation for 'silver-54a83950f845' failed unexpectedly: [{'path': ['placeOrder'], 'message': 'This endpoint does not call the AMAZON_EVENTBRIDGE data source sim2real_eb_orders.'}]"
- eventbridge-datasource-batch-entries-route-independently-in-order: "placePair failed unexpectedly: [...] 'This endpoint does not call the AMAZON_EVENTBRIDGE data source sim2real_eb_pairs.'"
- eventbridge-datasource-non-object-detail-rejects-whole-batch: same, data source sim2real_eb_pairs
- eventbridge-datasource-role-without-put-events-denied: same, data source sim2real_eb_allowed
- eventbridge-datasource-role-scoped-to-other-bus-denied: same, field placeOnX, data source sim2real_eb_bus_x
- eventbridge-datasource-update-repoints-mutations-to-new-bus: same, data source sim2real_eb_repoint
No event is observed on any bus, so every downstream assertion (routing, ordering, batch rejection, denial, re-pointing) is unreachable.

## Steps to Reproduce
1. CreateGraphqlApi (AWS_IAM auth) and StartSchemaCreation with a schema exposing a mutation field.
2. CreateEventBus, then CreateDataSource type=AMAZON_EVENTBRIDGE with eventBridgeConfig.eventBusArn pointing at it and a serviceRoleArn trusting appsync.amazonaws.com with events:PutEvents on that bus.
3. CreateResolver for the mutation field on that data source (UNIT, request/response templates or APPSYNC_JS code emitting one PutEvents entry).
4. PutRule on the bus matching the entry's source/detail-type, PutTargets to an SQS queue whose policy admits events.amazonaws.com.
5. POST the signed mutation to the API's GraphQL endpoint.
Observed: 200 with data.<field> null and errors[0].message "This endpoint does not call the AMAZON_EVENTBRIDGE data source <name>."; the SQS queue never receives an envelope. Expected: 200 with the mapped PutEvents result and the event delivered to the queue.
