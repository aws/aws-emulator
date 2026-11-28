Issue-Source: trace-sweep-divergence
Issue-Trace: sns-http-origin-header-on-signed-query-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-http-origin-header-on-signed-query-requests sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: sns|GetTopicAttributes|4xx->4xx|status
Issue-Key: sns|CreateTopic|2xx->4xx|$.CreateTopicResponse
Issue-Key: sns|CreateTopic|2xx->4xx|status
Issue-Key: sns|GetTopicAttributes|4xx->5xx|$.ErrorResponse
Issue-Key: sns|GetTopicAttributes|4xx->5xx|$headers.content-type
Issue-Key: sns|GetTopicAttributes|4xx->5xx|status
Issue-Key: sns|ListSubscriptionsByTopic|4xx->5xx|$.ErrorResponse
Issue-Key: sns|ListSubscriptionsByTopic|4xx->5xx|$headers.content-type
Issue-Key: sns|ListSubscriptionsByTopic|4xx->5xx|status
Issue-Key: sns|GetTopicAttributes|2xx->4xx|$.GetTopicAttributesResponse
Issue-Key: sns|GetTopicAttributes|2xx->4xx|status
Issue-Key: sns|ListSubscriptionsByTopic|2xx->4xx|$.ListSubscriptionsByTopicResponse
Issue-Key: sns|ListSubscriptionsByTopic|2xx->4xx|status
Issue-Commit: c8af1db729b355d4375dc5660957683cc548873e

# sns: request-envelope handling refuses Origin-bearing, unsigned and presigned requests instead of answering them

> _Note: this issue was identified against the code base at commit `c8af1db729b355d4375dc5660957683cc548873e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicAttributes

## Description
Four independent root causes in the emulator's SNS request-envelope / authentication path: requests that AWS accepts (or refuses in a specific modeled way) are instead refused by the emulator's front door, so the operation handler is never reached. All four are grounded in replayed wire fixture steps, listed per cause under Traces. 8 divergence clusters in total.

Cause 1 of 4: an `Origin` request header on a correctly SigV4-signed AWS query request makes the emulator refuse the call as a cross-site browser request.
  AWS: SNS's query endpoint authenticates on the SigV4 credential alone; an `Origin` header carries no authorization meaning and is ignored, so the request is answered on its merits. Smithy: GetTopicAttributes models NotFoundException (awsQueryError code `NotFound`, HTTP 404) and CreateTopic answers `CreateTopicResponse{TopicArn}` with HTTP 200; neither models any origin/CORS refusal, and the model declares no `cors` trait for this service. Recorded behavior: GetTopicAttributes for a topic that does not exist answers HTTP 404 with `ErrorResponse.Error` `{Type: Sender, Code: NotFound, Message: "Topic does not exist"}` plus a `RequestId`; the following CreateTopic for the same name answers HTTP 200 with the new TopicArn. The message `Topic does not exist` is entirely fixed text.
  Emulator: both steps are refused HTTP 403 with `Code: AccessDenied` and `Message: "Requests a browser sends from another site are refused"`, and the error body carries no `ErrorResponse.RequestId` element at all. No topic is created, so the whole fixture's state setup is lost.
  Why: any client that sets an `Origin` header while still signing with SigV4 — a browser-based console, an SDK behind a CORS-aware proxy, a test harness replaying captured traffic — is locked out of SNS entirely. It sees `AccessDenied` where AWS returns the real outcome (`NotFound` for a missing topic, success for a create), so credential/permission troubleshooting is sent down a false path and no topic can be provisioned at all.
  Fix: the shared query-protocol front door / request-authentication layer for sns (the pre-dispatch guard that inspects request headers before the operation handler runs) — the `Origin` header must not participate in the accept/refuse decision for a signed query request; also ensure the error serializer always emits `ErrorResponse.RequestId`.
  Traces: sns-http-origin-header-on-signed-query-requests#task#1 (GetTopicAttributes, expected 404 NotFound, got 403 AccessDenied), sns-http-origin-header-on-signed-query-requests#task#2 (CreateTopic, expected 200, got 403).

Cause 2 of 4: an unsigned request is answered with an HTTP 500 in the JSON protocol's content type instead of the modeled query-protocol 403 refusal.
  AWS: a request that carries no authentication material is refused by the SNS query endpoint with HTTP 403 and an XML `ErrorResponse` body `{Error: {Type: Sender, Code: MissingAuthenticationToken, Message: "Request is missing Authentication Token"}, RequestId: <per-request>}`, served as `Content-Type: text/xml`. The code and the message text are fixed for this error class; only the `RequestId` varies per request. Smithy: the service's protocol is awsQuery, so every response — success or error — is XML (`text/xml`), never `application/x-amz-json-1.1`; the model's own client-fault errors are all 4xx and `InternalErrorException` (HTTP 500) is the only server-fault error, which an unauthenticated request must not produce.
  Emulator: the same requests answer HTTP 500 with `Content-Type: application/x-amz-json-1.1` and no `ErrorResponse` element in the body, for both GetTopicAttributes and ListSubscriptionsByTopic.
  Why: a missing-credential mistake is reported to the client as a server fault in the wrong protocol. SDK retry logic treats 500 as retryable and hammers the endpoint instead of failing fast; the XML error parser finds no `ErrorResponse` in a JSON body, so the client surfaces an unparseable-response error rather than `MissingAuthenticationToken`, and the user never learns the request was unsigned.
  Fix: the request-authentication layer plus the error serializer for the awsQuery protocol — an unauthenticated query request must raise the Sender-class `MissingAuthenticationToken` refusal at HTTP 403 and be serialized as `text/xml` through the same `ErrorResponse` writer every other sns error uses, rather than falling through to a generic JSON 500.
  Traces: sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#2 (GetTopicAttributes, expected 403, got 500; cluster also covers task#3), sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#4 (ListSubscriptionsByTopic, expected 403, got 500).

Cause 3 of 4: the emulator fails to recover the `Action` and parameters from an alternately-encoded / presigned query request, so it answers 400 instead of dispatching the operation.
  AWS: the SNS query endpoint accepts the operation name and parameters in any of the forms the signing protocol permits — in the query string of a presigned request as well as in a form-encoded body, and with a body charset other than UTF-8 (ISO-8859-1) — and dispatches the operation normally. Recorded behavior: GetTopicAttributes on the existing topic answers HTTP 200 with the full default attribute set (`Policy`, `Owner`, `TopicArn`, `EffectiveDeliveryPolicy`, `SubscriptionsPending`, `SubscriptionsConfirmed`, `SubscriptionsDeleted`, `DisplayName`); ListSubscriptionsByTopic answers HTTP 200 with an empty `Subscriptions` member; and once the topic has been deleted, GetTopicAttributes answers HTTP 404 `{Code: NotFound, Message: "Topic does not exist"}` (fixed text) — i.e. the request is always parsed far enough to identify the operation and the topic.
  Emulator: these steps are refused HTTP 400. One of them exposes the cause directly: `Code: InvalidAction`, `Message: "unknown action: request is missing the `Action` field"` — the emulator never extracted `Action` from that request form, so it cannot route the call, and the 404-vs-400 difference is a consequence of never reaching the handler. `InvalidAction` is not a modeled sns error for these operations at all.
  Why: presigned SNS URLs and any client that does not send a UTF-8 form-encoded body cannot call the emulator: every such call comes back as `InvalidAction`/400 regardless of whether the topic exists, so existence checks invert (a live topic looks broken, a deleted topic reports the wrong code) and a presign-then-fetch workflow is unusable.
  Fix: the awsQuery request parser / action router for sns — resolve `Action` (and the operation's parameters) from the query string as well as the body, and honour the request's declared body charset, before the dispatch decision; only emit `InvalidAction` when no `Action` is present in either place.
  Traces: sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#5 (GetTopicAttributes, expected 200, got 400), sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#6 (GetTopicAttributes on the deleted topic, expected 404 NotFound/"Topic does not exist", got 400 InvalidAction), sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#7 (ListSubscriptionsByTopic, expected 200 with empty Subscriptions, got 400; cluster also covers task#9 and task#10).

Cause 4 of 4: a request form AWS authenticates is refused HTTP 403 by the emulator, so GetTopicAttributes never answers.
  AWS: the recorded step is answered HTTP 200 with the topic's full default attribute map (the same `Policy`/`Owner`/`TopicArn`/`EffectiveDeliveryPolicy`/subscription-counter/`DisplayName` entries as in Cause 3), i.e. the request's credentials are accepted. Smithy: GetTopicAttributes' 403-class errors are `AuthorizationErrorException` (`AuthorizationError`) and `InvalidSecurityException` (`InvalidSecurity`); neither applies to a request AWS authenticates and authorizes.
  Emulator: HTTP 403 with no `GetTopicAttributesResponse` body.
  Why: the client is told its credentials or permissions are wrong for a request AWS accepts, so an otherwise-correct caller cannot read topic attributes and is driven to re-issue credentials that were never the problem.
  Hypothesis: the same pre-dispatch guard behind Causes 1-3 rejects this request variant on a header it should ignore, rather than the operation's own authorization check failing.
  Fix: the request-authentication layer for sns (the pre-dispatch guard), which must accept every signing form the recorded corpus uses and leave authorization to the operation handler.
  Traces: sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#8 (GetTopicAttributes, expected 200, got 403).

## Evidence
Cause 1: sns-http-origin-header-on-signed-query-requests#task#1 — expected HTTP 404 with ErrorResponse.Error.Code='NotFound', Message='Topic does not exist' and a RequestId; got HTTP 403, Code='AccessDenied', Message='Requests a browser sends from another site are refused', ErrorResponse.RequestId missing. #task#2 — expected HTTP 200 CreateTopicResponse with CreateTopicResult.TopicArn; got HTTP 403 and no body element.
Cause 2: sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic#task#2 and #task#3 (GetTopicAttributes), #task#4 (ListSubscriptionsByTopic) — expected HTTP 403 with ErrorResponse.Error={Type:'Sender', Code:'MissingAuthenticationToken', Message:'Request is missing Authentication Token'} and headers.content-type='text/xml'; got HTTP 500, content-type='application/x-amz-json-1.1', no ErrorResponse element.
Cause 3: #task#5 — expected HTTP 200 GetTopicAttributesResponse with the 8 default attribute entries; got HTTP 400. #task#6 — expected HTTP 404 Code='NotFound'/Message='Topic does not exist'; got HTTP 400 Code='InvalidAction', Message='unknown action: request is missing the `Action` field'. #task#7/#task#9/#task#10 — expected HTTP 200 ListSubscriptionsByTopicResponse with ListSubscriptionsByTopicResult.Subscriptions empty; got HTTP 400.
Cause 4: #task#8 — expected HTTP 200 GetTopicAttributesResponse with the full default attribute map; got HTTP 403 with no body element.

## Steps to Reproduce
1. Cause 1: with valid SigV4 credentials, send a signed SNS query request that additionally carries an `Origin: https://example.com` header. GetTopicAttributes for a nonexistent topic must answer 404 NotFound/'Topic does not exist' with a RequestId, and CreateTopic must answer 200 with a TopicArn; observe 403 AccessDenied 'Requests a browser sends from another site are refused' for both.
2. Cause 2: send GetTopicAttributes and ListSubscriptionsByTopic with no Authorization header and no presigned credentials. Expect 403, Content-Type text/xml, ErrorResponse.Error.Code='MissingAuthenticationToken', Message='Request is missing Authentication Token'; observe 500 with Content-Type application/x-amz-json-1.1.
3. Cause 3: call GetTopicAttributes / ListSubscriptionsByTopic with Action and parameters in the query string of a presigned URL (and with a form body declared as charset ISO-8859-1). Expect 200 with the topic's attributes / an empty Subscriptions list, and 404 NotFound 'Topic does not exist' after DeleteTopic; observe 400, including Code='InvalidAction' with 'unknown action: request is missing the `Action` field'.
4. Cause 4: repeat the GetTopicAttributes read of the live topic in the same request form as step 3's accepted variant. Expect 200 with the default attribute map; observe 403.
Replay: replay_wire_corpus and re-run fixtures sns-http-origin-header-on-signed-query-requests and sns-unsigned-presigned-and-iso-8859-1-header-requests-to-a-topic (steps task#1-task#10).
