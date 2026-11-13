// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` DATA plane: how a request addressed to a deployed REST API stage's
//! invoke URL is recognised and answered.
//!
//! A REST API with a deployment is reached at
//! `https://{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}/{resource-path}` —
//! the endpoint the `RestApi$disableExecuteApiEndpoint` documentation names ("the default
//! `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint"). That host is NOT the
//! control plane's (`apigateway.{region}.amazonaws.com`): its first label is the API's own
//! id, which names no service, so the emulator's front door can resolve no target service
//! from such a request and answers it `404 UnknownService` — for a URL this very service
//! told the caller to use. The one thing that is never correct is for the host to be
//! unknown to the service.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving
//! ([`serve`], reached from the one seam the router offers before it authenticates
//! anything). Every status it answers is computed from the control plane's own
//! records, and the three things it WRITES are the metering a usage plan's throttle consists
//! of ([`metered_refusal`]) — the rate a key's requests are admitted at cannot be computed
//! without recording the ones already admitted — the stage's own RESPONSE CACHE
//! ([`crate::state::StageEntry::response_cache`]), for the same reason: a cached response
//! is the response a previous request was served, held and served again ("whether
//! responses should be cached and returned for requests",
//! `MethodSetting$cachingEnabled`), so a method whose settings enable caching cannot
//! answer from the cache without the serving that filled it — and the stage's AUTHORIZER
//! cache ([`crate::state::StageEntry::authorizer_results`]), which is that same rule again:
//! "the TTL in seconds of cached authorizer results"
//! (`Authorizer$authorizerResultTtlInSeconds`) is a result a previous request obtained,
//! held and reused. Nothing else about a stage,
//! an api or a key changes because it was invoked, and none of those three is reported by
//! any operation of this service.
//!
//! A client on the machine this emulator runs on cannot reach that host at all: nothing
//! resolves `{rest-api-id}.execute-api.{region}.amazonaws.com` there, so it points at the
//! emulator's own endpoint and carries in the PATH what it cannot carry in the host — this
//! data plane and the api id behind the reserved `/_aws/execute-api/{api-id}` prefix, the
//! api id ahead of the stage, or the stage alone. [`resolve`] reads all four addressings,
//! and each one is checked against this service's own records, so one request still has
//! one claimant.
//!
//! # What it resolves, in order
//!
//! 1. the request names a stored `RestApi` — by the host's first label, by the api id
//!    behind the data-plane path prefix, by the first path segment, or, on a host that names
//!    no service, by the stage segment of its path ([`resolve`]) — whose
//!    `disableExecuteApiEndpoint` must not have turned this endpoint off;
//! 2. the first segment of the path WITHIN that api names a `Stage` of it, and that
//!    stage's `deploymentId` names a stored `Deployment`;
//! 3. the rest of the path matches one of the resource paths THAT DEPLOYMENT holds
//!    (`Deployment$apiSummary`), most specific match first: a literal segment beats a
//!    `{parameter}`, which beats a greedy `{proxy+}`;
//! 4. the request's verb names a method of that resource in the deployment, or the
//!    `ANY` catch-all;
//! 5. the method's deployed `authorizationType` is enforced — for `AWS_IAM` that is the
//!    SigV4 credential being AUTHENTICATED against this emulator's own credentials
//!    ([`sigv4_authentication`]) and then AUTHORIZED for the principal it speaks for
//!    ([`execute_api_authorization`]), and for a `CUSTOM` authorizer it is the RETURN VALUE
//!    of the Lambda function its `authorizerUri` names ([`custom_authorizer`]), obtained by
//!    invoking that function and then HELD on the stage for the authorizer's own
//!    `authorizerResultTtlInSeconds` — and then the API's own
//!    RESOURCE POLICY is applied ([`super::resource_policy`]): `RestApi$policy` "applies to
//!    this RestApi regardless of the caller and Method configuration", so a document the
//!    developer attached decides every request this endpoint answers — a private API pinned
//!    to one of its interface VPC endpoints admits the request that arrived through that
//!    endpoint and refuses the one that did not;
//! 6. `apiKeyRequired` is enforced — a key-required method admits only an enabled API key
//!    the records associate with THIS stage, through a usage plan over that api stage or
//!    the key's own deprecated `stageKeys`, and refuses every other request before the
//!    integration is reached;
//! 7. the method's REQUEST VALIDATOR is applied — the `RequestValidator` the live method's
//!    `requestValidatorId` names decides whether the request's parameters
//!    ([`parameters_refusal`]) and its payload ([`PendingBodyCheck::refusal`]) are checked
//!    before the integration is reached;
//! 8. the usage plan metering the presented API key admits the request at the plan's
//!    throttle, or the request is shed ([`metered_refusal`]);
//! 9. the STAGE's own cache is consulted for the method whose settings enable caching
//!    ([`cache_plan`]): a cluster holding an entry for this call answers it without the
//!    integration being performed at all, until that entry's TTL passes or a flush drops
//!    it, and a request asking for it to be reloaded is honoured only as the stage's
//!    `requireAuthorizationForCacheControl` and `unauthorizedCacheControlHeaderStrategy`
//!    say;
//! 10. the method's integration decides the response, read with the STAGE's own variables
//!    resolved into its configuration ([`with_stage_variables`]): a reference to one
//!    stands where the configuration carries an address, a connection or a role, so one
//!    deployment reached through two stages addresses each stage's own backend — and what
//!    it answers is what the stage's cache then holds.
//!
//! Every step from 4 on reads the method out of the DEPLOYMENT
//! ([`crate::state::DeploymentEntry::resources`]), not out of the api as it now stands: "a
//! Deployment resource is a snapshot of your REST API configuration" and a stage serves the
//! deployment its `deploymentId` names, so an edit to a method, its integration, that
//! integration's templates, responses or cache configuration is served only once the api is
//! deployed again. What a deployment does NOT hold is the records a method names BY ID — its
//! `Authorizer`, its `RequestValidator`, the `Model` a validator checks a payload against —
//! and the api's own `policy` and `binaryMediaTypes`: `DeploymentEntry` snapshots the
//! resource tree alone, so those are read from the api as it stands. That is a narrowing of
//! the snapshot rule, stated here rather than hidden: editing one of them takes effect at
//! the stage without a redeploy, where AWS would want one.
//!
//! A step that resolves nothing is answered the way API Gateway answers it: through the
//! API's GATEWAY RESPONSE for the matching response type ([`GatewayRefusal`]), so a
//! developer's `PutGatewayResponse` customization of that type — its status code and its
//! template — is what the caller sees, and the uncustomized case falls back to the status
//! code the response type documents ([`crate::state::default_gateway_status_code`]). Such a
//! refusal also NAMES ITS EXCEPTION CLASS, in the header an SDK reads an error code out of
//! ([`GatewayRefusal::error_type`]), so a client classifies a refusal instead of reading its
//! prose — and the class is the endpoint's own, which the control-plane model does not
//! declare for the data plane any more than it declares the endpoint itself.
//!
//! Whatever the outcome, the response NAMES THE REQUEST it answered: every response this
//! endpoint serves carries a per-request id in the header API Gateway reports it in, which
//! is what lets a client correlate the answer it holds with what the API processed. The id is
//! minted ONCE per request, by [`serve`] — the one funnel both seams reach, so no outcome can
//! answer without one — and it is the same id the call carries as `$context.requestId` and
//! `$context.extendedRequestId` ([`call_context`]), so a mapping template, a proxy event and
//! the response header cannot name one request three ways.
//!
//! # What it does not do, and what it answers instead
//!
//! Only the `MOCK` integration type returns a backend's answer without a backend: it is
//! "for testing without actually invoking the backend" (`IntegrationType`), so its
//! response is entirely the API's own records — the integration response the call's
//! request template selects, the response template for the negotiated content type, and
//! the headers the method response declares. That path is served in full, from state. One
//! narrowing is worth stating: the status the request template names selects the
//! integration response stored UNDER THAT STATUS CODE, and otherwise the default one — a
//! `selectionPattern` is not matched against it, because a pattern is documented as
//! choosing "an integration response based on the response from the back end" and a `MOCK`
//! produces no back-end response to match. What a `MOCK` DID answer is its request
//! template rendered against the call, so that is the payload its response template reads
//! `$input` off.
//!
//! An `HTTP_PROXY` integration — the type `IntegrationType` gives "for integrating with the
//! HTTP proxy integration" — is PASSED THROUGH rather than mapped: its answer is the
//! endpoint's own. Where its `uri` names an endpoint THIS EMULATOR serves, which is every
//! `execute-api` invoke endpoint of this account, that pass-through is performed IN PROCESS
//! ([`passed_through`]): the caller's own request is re-addressed at the endpoint the `uri`
//! names, carrying the headers `requestParameters` maps onto it, and handed back to
//! [`serve_resolved`] to be served by that endpoint's own records — including its api's
//! resource policy, which is how a REGIONAL api fronting a PRIVATE one through a VPC link is
//! gated on the interface endpoint the request ARRIVES through: the endpoint the link's own
//! targets are the network interfaces of ([`private_delivery`]), since for a private
//! integration "the URI is not used for routing" (`Integration$uri`). No socket is opened
//! and no name is
//! resolved: an address outside this emulator's own endpoints is never dialled, it is
//! refused, and nothing is invented either — the response is the one this data plane
//! computes for the request it passed on, whatever that is. What the onward request does NOT
//! carry is the caller's own SigV4 credential, in either the header or the presigned-query
//! form ([`HEADERS_NOT_PASSED_THROUGH`]): a signature over the address the caller used
//! authenticates nothing at the address the request is passed on to. A private integration
//! is carried only over a VPC link this account holds that has finished provisioning
//! ([`unusable_vpc_link`]), the chain of hops is bounded ([`MAX_PASSED_THROUGH_HOPS`]), and a
//! `requestParameters` entry outside the mapping forms below is an integration this call
//! cannot perform rather than a mapping silently dropped.
//!
//! Every other integration type names a backend outside this api's own records:
//!
//! * an `AWS` integration names ANOTHER SERVICE'S ACTION, and where that action is one this
//!   emulator serves it is PERFORMED by making the call, in process, over the transport the
//!   router hands this plane ([`aws_integration`]): the method response is then the answer
//!   that service gave, mapped through the integration response whose `selectionPattern`
//!   matches the status it answered with. An action of a service this emulator does not
//!   make the call to is left to the refusal below;
//! * `AWS_PROXY` invokes a LAMBDA FUNCTION, which is a call this emulator makes: the
//!   function the `uri` names is invoked in process over the transport the router hands this
//!   plane, with the proxy event the whole method request becomes, and what the handler
//!   returned IS the method response — its status, its headers and its body, mapped by
//!   nothing, because "the client request [is] passed through as-is" (`IntegrationType`).
//!   The grant that admits the invocation is the function's own resource policy, read from
//!   Lambda itself ([`lambda_proxy`]);
//! * `HTTP`, and an `HTTP_PROXY` whose `uri` names an endpoint outside this emulator,
//!   address an endpoint over the network, and this emulator makes no outbound connection.
//!   The standard `HTTP` integration is not served even against an endpoint of this
//!   emulator, because its response is chosen by matching the BACKEND's response against
//!   each integration response's `selectionPattern` and then rendered through that
//!   response's templates — a mapping half a proxy integration has none of.
//!
//! None of those is faked: a body, a status or a header invented for a backend that never
//! ran would be indistinguishable from a real one. They are answered as API Gateway
//! answers an integration it could not perform — the `API_CONFIGURATION_ERROR` gateway
//! response, `500` with `{"message":"Internal server error"}` — and the reason is logged.
//! The same goes for a mapping template that would have to be EXECUTED. A mapping template
//! is "a map of Velocity templates" whose REFERENCES name values of the call — the method
//! request, the stage's variables and the call's context — so the template that keys the
//! call is RENDERED from those values ([`rendered_mapping_template`],
//! [`mapping_template`]) and the text it produced is the integration request and the
//! response body. What is NOT rendered is the half that is statements rather than values —
//! a DIRECTIVE (`#set`, `#if`, …) — and a reference naming anything outside that set: a
//! hand-written stand-in for either would silently produce the wrong body, so such a
//! template is an integration this call cannot perform rather than a body built by
//! guesswork.
//!
//! # The stage's response cache
//!
//! A method whose stage settings enable caching is answered from the stage's CACHE CLUSTER
//! rather than by performing its integration again. Everything about it is the stage's own
//! records ([`cache_plan`]):
//!
//! * WHETHER this call is cached: `cachingEnabled` on the method settings in force for it
//!   — the entry keyed by this method, else the `*/*` entry that names every method of the
//!   stage — and, because "a cache cluster must be enabled on the stage for responses to be
//!   cached", a cluster that is actually up ([`crate::state::StageEntry::cache_cluster_status`]).
//!   A `cacheTtlInSeconds` of zero caches nothing: "the higher the TTL, the longer the
//!   response will be cached".
//! * WHICH entry it is: the integration's `cacheNamespace` ("a group of related cached
//!   parameters … you can specify the same cacheNamespace across resources to return the
//!   same cached data"), the method's verb, and the value this request carries for each of
//!   the integration's `cacheKeyParameters` ("a list of request parameters whose values API
//!   Gateway caches") — [`cache_key`]. The namespace is the group, so two resources a
//!   developer grouped under one share an entry and two carrying the per-resource default
//!   `PutIntegration` writes do not.
//! * HOW LONG it lasts: the TTL, after which a read of it is a miss — the expiry is a
//!   transition the clock owes, materialised from the stored instant rather than by a timer
//!   — and `FlushStageCache` drops every entry the cluster holds before that.
//! * WHAT a client asking for it to be reloaded gets: `Cache-Control: max-age=0` asks for
//!   the entry to be reloaded from the integration, and
//!   `requireAuthorizationForCacheControl` decides whether that ask needs the
//!   [`resource_policy::INVALIDATE_CACHE_ACTION`] grant. An ask that needs one and has none
//!   is answered as `unauthorizedCacheControlHeaderStrategy` says: `FAIL_WITH_403` refuses
//!   the request — as the missing-token refusal where the caller presented no credential at
//!   all, and with the IAM-shaped sentence for that action where it presented one
//!   ([`response_cache::invalidation_refusal`]) — and the two `SUCCEED_…`
//!   values serve the request with the header ignored — stating that in the response
//!   ([`IGNORED_CACHE_CONTROL_WARNING`]) or not, as the value says.
//!
//! What is cached is what the INTEGRATION answered. A refusal the api itself produced (a
//! gateway response for an unroutable path, an unauthorized call or an integration this
//! emulator cannot perform) is not an integration's answer, so it is never held: the next
//! request earns its own.
//!
//! # The half `TestInvokeMethod` shares
//!
//! `TestInvokeMethod` simulates "the invocation of a Method in your RestApi" — the same
//! integration, read out of the same records, for a request that never reached a socket. So
//! the performing half of this module takes no wire request: [`performed_integration`]
//! reads what an integration needs through [`MethodRequest`], and [`refusal_rendering`]
//! gives the status and body a call no integration can serve is answered with. That
//! operation REPORTS what this module performs FROM THE API'S OWN RECORDS — including the
//! call an `AWS` integration makes to another service, which it is handed the same
//! transport for — so neither can answer one stored configuration differently. Passing a
//! request through is not that: the answer is another endpoint's response to a request this
//! one forwarded, and a simulated invocation forwards nothing, so it keeps reporting such
//! an integration as a configuration error. Neither is the stage's response cache:
//! `TestInvokeMethod` is not a request to the stage, so it neither reads nor fills what the
//! cluster holds — it reports what the integration answers for the records as they stand.

pub mod aws_integration;
pub mod cognito_user_pools;
mod custom_authorizer;
pub mod execute_api_authorization;
/// What an authorizer's `identitySource` declaration extracts from a request, read the one
/// way by this data plane and by a SIMULATED request ([`simulated_authorizer`]).
mod identity_source;
pub mod lambda_proxy;
mod onward_endpoint;
/// Which interface VPC endpoint a request carried over a VPC link arrives at this plane
/// through, which is what a private api's resource policy is conditioned on.
mod private_delivery;
pub mod sigv4_authentication;
/// What one `Authorizer` makes of a SIMULATED request, which is what
/// `TestInvokeAuthorizer` reports. `pub(crate)` because that operation is its one caller.
pub(crate) mod simulated_authorizer;
/// What a SIMULATED request carries, read the one way for both `TestInvoke*` operations.
pub(crate) mod simulated_request;
/// What the stage LOGS about a request it served: its access-log record and the execution
/// log its method settings turn on.
mod stage_logs;
/// The X-Ray trace context this stage propagates onto the call its integration makes.
mod trace_context;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, OperationContext};

use crate::ops::put_integration::{
    DEFAULT_CONNECTION_TYPE, HTTP_PROXY_INTEGRATION_TYPE, MOCK_INTEGRATION_TYPE,
    VPC_LINK_CONNECTION_TYPE,
};
use crate::ops::put_integration_response::static_mapping_literal;
use crate::ops::put_method::method_key;
use crate::ops::put_method_response::METHOD_RESPONSE_HEADER_PREFIX;
use crate::ops::types::{GetIntegrationOutput, GetIntegrationResponseOutput, MethodSetting};
use crate::state::{
    ApiKeyEntry, CachedResponse, DeploymentEntry, MethodEntry, RestApiEntry, StageEntry,
};

use ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
// The record that same web ACL logs about the request, and the delivery of it: WAFv2
// composes it, this plane hands it to the destination the configuration names.
use ::service_com_amazonaws_wafv2_awswaf_20190729::service::logs as waf_logs;

use super::mapping_template::{self, TemplateInputs};
use super::resource_policy;

/// The second label of every `execute-api` endpoint host: `{api-id}.execute-api.…`.
///
/// From `RestApi$disableExecuteApiEndpoint`: "clients can invoke your API with the default
/// `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint".
pub const EXECUTE_API_LABEL: &str = "execute-api";

/// The reserved path segment that names a DATA plane on the one endpoint this emulator
/// serves its whole surface at, ahead of [`EXECUTE_API_LABEL`] and the api id:
/// `/_aws/execute-api/{api-id}/{stage}/{resource-path}`.
///
/// A client reaching AWS addresses the api by host; a client reaching this emulator has
/// one host for every service and every plane, so the two parts of an invoke URL that
/// AWS puts in the host — that it is the `execute-api` data plane, and which api — travel
/// in the path instead. The segment begins with `_`, which no modelled path segment of any
/// service does, so the prefix cannot collide with a control-plane route however a stage or
/// a resource is named.
///
/// Public because the custom domain name front door
/// ([`super::custom_domain_socket`]) addresses this plane in exactly this form for a request
/// it accepts on a domain name's own socket.
pub const EMULATOR_PATH_NAMESPACE: &str = "_aws";

/// The path segment that belongs to ApiGatewayManagementApi rather than to a stage.
///
/// `@connections` is bound on the SAME `execute-api` host — that service's documentation
/// gives its endpoint as `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}` and
/// binds `DELETE|GET|POST /@connections/{ConnectionId}` under it — so a request naming it
/// is not a stage invoke and is left to the service that models it. Claiming it here would
/// make two services claim one request, which the router rejects as `AmbiguousService`.
const CONNECTIONS_SEGMENT: &str = "@connections";

// The prefix of every VPC endpoint id, which is also the first label of the
// endpoint-specific DNS names an interface endpoint answers on.
//
// An interface endpoint to the Region's `execute-api` service is given endpoint-specific
// names of the form `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com` (and one
// `{vpc-endpoint-id}-{zone}.…` per Availability Zone), which is how a client addresses a
// PRIVATE api through one endpoint rather than another — see `endpoint_of_host`. The id's
// whole form is `crate::state::is_vpc_endpoint_id`, this crate's one reading of it.
use crate::state::VPC_ENDPOINT_ID_PREFIX;

/// The header a client names the api in when it addresses a PRIVATE api by its VPC
/// endpoint's own DNS name, which names the endpoint rather than the api: AWS documents
/// invoking such an api through the endpoint-specific hostname with the api id carried in
/// `x-apigw-api-id`.
const API_ID_HEADER: &str = "x-apigw-api-id";

/// The verb-independent catch-all a method may be put under, from
/// [`crate::ops::put_method::HTTP_METHODS`]: "plus the `ANY` catch-all that matches every
/// verb".
const ANY_METHOD: &str = "ANY";

/// The `authorizationType` that lets a request through with no authorization at all. From
/// `Method$authorizationType`: "the method's authorization type. Valid values are `NONE`
/// for open access …".
const AUTHORIZATION_NONE: &str = "NONE";

/// The `authorizationType` whose caller must sign the request: "`AWS_IAM` for using AWS
/// IAM permissions" (`Method$authorizationType`).
const AUTHORIZATION_AWS_IAM: &str = "AWS_IAM";

/// The `authorizationType`s served by an `Authorizer` of the API: "`CUSTOM` for using a
/// custom authorizer, or `COGNITO_USER_POOLS` for using a Cognito user pool"
/// (`Method$authorizationType`).
const AUTHORIZATION_BY_AUTHORIZER: &[&str] = &["CUSTOM", AUTHORIZATION_COGNITO_USER_POOLS];

/// The one `authorizationType` whose authorizer's decision is a function of the request and
/// of the api's own records rather than of code this emulator cannot run: "`COGNITO_USER_POOLS`
/// for using a Cognito user pool" (`Method$authorizationType`), evaluated in full by
/// [`cognito_user_pools`].
const AUTHORIZATION_COGNITO_USER_POOLS: &str = "COGNITO_USER_POOLS";

/// The header an API key travels in when the API's `apiKeySource` is `HEADER`, which is
/// what that member documents: "`HEADER` to read the API key from the `X-API-Key` header of
/// a request".
const API_KEY_HEADER: &str = "x-api-key";

/// The `apiKeySource` that reads the key from the request header; see [`API_KEY_HEADER`].
const API_KEY_SOURCE_HEADER: &str = "HEADER";

/// The request header a client asks for the stage's CACHED entry to be reloaded with.
///
/// API Gateway's documented ask is one directive of it: "A client of your API can
/// invalidate an existing cache entry and reload it from the integration endpoint … the
/// request must contain the `Cache-Control: max-age=0` header."
const CACHE_CONTROL_HEADER: &str = "cache-control";

/// The `Cache-Control` directive that carries the ask; see [`CACHE_CONTROL_HEADER`]. A
/// `max-age` of zero is the ask, and any other value of it is an ordinary freshness
/// requirement that leaves the stage's cache alone.
const MAX_AGE_DIRECTIVE: &str = "max-age";

/// The `unauthorizedCacheControlHeaderStrategy` that refuses an unauthorized ask outright,
/// from the modelled value set (`smithy_query(query="shape",
/// shape="UnauthorizedCacheControlHeaderStrategy")`, which
/// [`crate::ops::types::UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES`] carries:
/// `FAIL_WITH_403`, `SUCCEED_WITHOUT_RESPONSE_HEADER`, `SUCCEED_WITH_RESPONSE_HEADER`).
const STRATEGY_FAIL_WITH_403: &str = "FAIL_WITH_403";

/// The strategy that serves the request with the cache-control header ignored and SAYS so;
/// see [`STRATEGY_FAIL_WITH_403`] for the value set and [`IGNORED_CACHE_CONTROL_WARNING`]
/// for what it says.
const STRATEGY_SUCCEED_WITH_RESPONSE_HEADER: &str = "SUCCEED_WITH_RESPONSE_HEADER";

/// The response header a stage states an IGNORED cache-control header in: HTTP's own
/// `Warning`, which is the field a response carries "additional information about the
/// status or transformation of a message" in.
const WARNING_HEADER: &str = "warning";

/// What that header says: the sentence AWS answers an ignored cache-control header with,
/// whole. The warn-code is 199, the "miscellaneous warning" HTTP reserves for text a client
/// may show its user, and nothing in the line is taken from the request — AWS emits this
/// same text for every request this strategy serves.
const IGNORED_CACHE_CONTROL_WARNING: &str =
    "199 Cache-control headers were ignored because the caller was unauthorized.";

/// The `methodSettings` key that names EVERY method of a stage, which is the fallback for a
/// method the stage holds no settings of its own for: `UpdateStage` keys a patch of
/// `/*/*/…` under it.
const EVERY_METHOD_SETTINGS_KEY: &str = "*/*";

/// The JSON Pointer escape a `methodSettings` key spells the `/` of a resource path with:
/// a patch addresses `/~1pets/GET/caching/enabled`, and the key the stage stores is the
/// method path exactly as the patch spelled it.
const ESCAPED_PATH_SEPARATOR: &str = "~1";

/// The mapping-expression prefix of a request HEADER, which is how both an authorizer's
/// identity source and a method's request parameter name one.
///
/// From `Method$requestParameters`: "a key is a method request parameter name matching the
/// pattern of `method.request.{location}.{name}`, where `location` is `querystring`,
/// `path`, or `header`"; `Authorizer$identitySource` names a header the same way ("the
/// request header mapping expression for the custom header holding the authorization token
/// … `method.request.header.Auth`").
const METHOD_REQUEST_HEADER_PREFIX: &str = "method.request.header.";

/// The mapping-expression prefix of a query-string parameter; see
/// [`METHOD_REQUEST_HEADER_PREFIX`] for the pattern both members share.
const METHOD_REQUEST_QUERY_PREFIX: &str = "method.request.querystring.";

/// The mapping-expression prefix of a PATH parameter, the third `location`
/// [`METHOD_REQUEST_HEADER_PREFIX`] quotes.
const METHOD_REQUEST_PATH_PREFIX: &str = "method.request.path.";

/// The mapping-expression prefix of a query-string parameter taken with every one of its
/// values, the multi-value spelling of [`METHOD_REQUEST_QUERY_PREFIX`].
const METHOD_REQUEST_MULTI_QUERY_PREFIX: &str = "method.request.multivaluequerystring.";

/// The multi-value spelling of [`METHOD_REQUEST_HEADER_PREFIX`].
const METHOD_REQUEST_MULTI_HEADER_PREFIX: &str = "method.request.multivalueheader.";

/// The mapping-expression prefix of a header of the INTEGRATION request — the request the
/// backend receives — which is how `Integration$requestParameters` names one it is to carry:
/// "The key is an integration request parameter name", written
/// `integration.request.{location}.{name}` over the same three locations a method request
/// parameter is written over.
const INTEGRATION_REQUEST_HEADER_PREFIX: &str = "integration.request.header.";

/// The header API Gateway adds to the request it passes to an HTTP integration endpoint,
/// naming the api the request came through: a backend shared by several apis reads which one
/// called it off this header, and it is added by the api rather than by the caller.
const INTEGRATION_API_ID_HEADER: &str = "x-amzn-apigateway-api-id";

/// The same prefix for a PATH parameter of the integration request: the value the `uri`'s
/// own `{name}` placeholder is addressed with; see [`INTEGRATION_REQUEST_HEADER_PREFIX`].
const INTEGRATION_REQUEST_PATH_PREFIX: &str = "integration.request.path.";

/// The same prefix for a QUERY-STRING parameter of the integration request: a pair the
/// backend is called with; see [`INTEGRATION_REQUEST_HEADER_PREFIX`].
const INTEGRATION_REQUEST_QUERY_PREFIX: &str = "integration.request.querystring.";

/// The query parameters a PRESIGNED SigV4 credential is carried in — the query-string form
/// of the headers [`HEADERS_NOT_PASSED_THROUGH`] withholds, and the set AWS documents a
/// presigned request carries (`X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`,
/// `X-Amz-Expires`, `X-Amz-SignedHeaders`, `X-Amz-Signature`, `X-Amz-Security-Token`,
/// `X-Amz-Content-Sha256`). Compared lowercased; every OTHER query parameter the caller
/// sent travels on, whatever it is named.
const SIGNED_QUERY_PARAMETERS: &[&str] = &[
    "x-amz-algorithm",
    "x-amz-credential",
    "x-amz-date",
    "x-amz-expires",
    "x-amz-signedheaders",
    "x-amz-signature",
    "x-amz-security-token",
    "x-amz-content-sha256",
];

/// How a STAGE VARIABLE is referenced where an integration's configuration carries a
/// value: `${stageVariables.<name>}`, whose `<name>` is a key of the stage's own map —
/// "a map that defines the stage variables for a Stage resource. Variable names can have
/// alphanumeric and underscore characters" (`Stage$variables`).
///
/// A reference is replaced by that key's value before anything reads the configuration
/// ([`with_stage_variables`]), which is what lets ONE deployment address a different
/// backend per stage.
const STAGE_VARIABLE_REFERENCE_PREFIX: &str = "${stageVariables.";

/// The character that closes a [`STAGE_VARIABLE_REFERENCE_PREFIX`] reference.
const STAGE_VARIABLE_REFERENCE_CLOSE: char = '}';

/// How many passed-through integrations one invoke may follow.
///
/// A proxy integration whose `uri` names an endpoint of this emulator may reach an api whose
/// own method passes the request on again, so the chain is bounded: a configuration that
/// leads back to itself is answered through the api's `API_CONFIGURATION_ERROR` gateway
/// response rather than followed until the stack is gone. API Gateway publishes no hop
/// count, so this is the emulator's own bound and is deliberately small — a front api in
/// front of a private one is one hop.
const MAX_PASSED_THROUGH_HOPS: u8 = 4;

/// The request headers a passed-through request does NOT carry over from the caller's.
///
/// `host` addresses the endpoint, and the endpoint the request is passed on to is the one
/// the integration's `uri` names, not the one the caller addressed; `content-length` is
/// recomputed for the body as it is sent.
///
/// The SigV4 authentication headers are not carried either, and that is the substantive one.
/// A caller's signature is computed over the endpoint, the path and the headers of the
/// request the caller made, so at the endpoint the request is passed on to it authenticates
/// nothing: carrying it would make an onward request LOOK credentialed to a method whose
/// `authorizationType` is `AWS_IAM` on the strength of a signature over somebody else's
/// address. API Gateway's own integration credentials are `Integration$credentials` — "the
/// credentials required for the integration" — which is a record of the integration rather
/// than anything the caller sent. Everything else the caller sent travels on, which is what
/// "passed through" means.
const HEADERS_NOT_PASSED_THROUGH: &[&str] = &[
    "host",
    "content-length",
    "authorization",
    "x-amz-security-token",
    "x-amz-date",
    "x-amz-content-sha256",
];

/// The media type a gateway response and an untemplated integration response are rendered
/// with. API Gateway's own default gateway-response templates are
/// `application/json`-keyed, and a `MOCK` integration with no response template answers an
/// empty JSON body.
///
/// `pub` because a caller that REPORTS a rendering instead of sending it
/// (`TestInvokeMethod`) has to report the media type it was rendered with.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// The `passthroughBehavior` that passes an unmapped body through whenever no template
/// keys the request's content type; the default one
/// ([`crate::ops::put_integration::DEFAULT_PASSTHROUGH_BEHAVIOR`]).
const PASSTHROUGH_WHEN_NO_MATCH: &str = "WHEN_NO_MATCH";

/// The `passthroughBehavior` that passes no unmapped body through at all, which
/// `PutIntegrationRequest$passthroughBehavior` documents as rejecting the request "with an
/// HTTP 415 Unsupported Media Type response".
const PASSTHROUGH_NEVER: &str = "NEVER";

/// The member a gateway response body carries the reason under: API Gateway's default
/// gateway-response template is `{"message":$context.error.messageString}`.
const MESSAGE_MEMBER: &str = "message";

/// The member an AUTHORIZATION refusal's body carries its sentence under.
///
/// The `execute-api` endpoint answers a request its api's resource policy does not allow
/// with the IAM-shaped body — `{"Message":"User: … is not authorized to perform: …"}` — so
/// a client reading that sentence reads it from this member rather than from the
/// [`MESSAGE_MEMBER`] every other gateway response carries.
const AUTHORIZATION_MESSAGE_MEMBER: &str = "Message";

/// The status code a refusal falls back to when its response type names none and no
/// customization overrides it. `DEFAULT_5XX`'s own class: this emulator could not place
/// the refusal, which is a server-side fault, never a caller's.
const UNPLACEABLE_REFUSAL_STATUS: u16 = 500;

/// The gateway response type whose customization applies to any 4xx the API has no
/// specific customization for; from [`crate::state`]'s own note on the two rows that carry
/// no status code of their own, "each is a fallback for a whole class of responses".
const DEFAULT_4XX: &str = "DEFAULT_4XX";

/// The gateway response type whose customization applies to any 5xx; see [`DEFAULT_4XX`].
const DEFAULT_5XX: &str = "DEFAULT_5XX";

/// One refusal of the data plane: the GATEWAY RESPONSE TYPE it is answered through — which
/// decides its status code, and which customization of the API applies to it — and the
/// sentence API Gateway's own default body carries for it.
///
/// The type is the unit of customization (`PutGatewayResponse` takes a `responseType`), so
/// naming it here is what lets a developer's customization reach the caller without this
/// module knowing anything about it.
#[derive(Debug, Clone)]
pub struct GatewayRefusal {
    /// A value of the model's `GatewayResponseType` enum.
    response_type: &'static str,
    /// The sentence the uncustomized response body carries. Owned-or-borrowed because a
    /// few refusals state what the request was missing, which is read off the request
    /// rather than fixed in the binary.
    message: Cow<'static, str>,
    /// The error shape this refusal is the wire form of, reported in
    /// [`ERROR_TYPE_HEADER`], or `None` for a refusal whose exception class nothing in
    /// reach names.
    ///
    /// The class is the one the `execute-api` endpoint itself reports, which is NOT
    /// confined to the exceptions this service's CONTROL-plane model declares: the data
    /// plane is not in that model at all (see the module docs), and it answers a refused
    /// invoke with the exception class of the envelope it sends — the `Forbidden` body
    /// with `ForbiddenException`, the missing-token body with
    /// `MissingAuthenticationTokenException`, the IAM-shaped denial with
    /// `AccessDeniedException`. A client's error handling keys off this header, so
    /// omitting it on a refusal whose class IS known leaves the caller unable to classify
    /// a 403 it can otherwise read only as prose.
    error_type: Option<&'static str>,
    /// What a VALIDATION of the request found, as the gateway response's
    /// `$context.error.validationErrorString` reports it; empty for every refusal that is
    /// not the outcome of validating the request against what the method declares.
    validation_detail: Cow<'static, str>,
    /// The member API Gateway's own default body carries the sentence under. Every gateway
    /// response's own default template is `{"message":$context.error.messageString}`, so
    /// that is [`MESSAGE_MEMBER`] for all but the AUTHORIZATION refusal, which API Gateway
    /// answers with the IAM-shaped [`AUTHORIZATION_MESSAGE_MEMBER`] body.
    message_member: &'static str,
}

impl GatewayRefusal {
    /// The `GatewayResponseType` this refusal is answered through — the one a developer's
    /// `PutGatewayResponse` customization of it would apply to. Read by the caller that
    /// REPORTS a refusal rather than answering it on the wire (`TestInvokeMethod`'s
    /// execution log), so the report names the same gateway response the data plane uses.
    pub fn response_type(&self) -> &'static str {
        self.response_type
    }

    /// The media type the response states for this refusal's body.
    ///
    /// Every gateway response the API renders is `application/json` — that is the content
    /// type its own default template is keyed by ([`JSON_CONTENT_TYPE`]). The one exception
    /// is the refusal the EDGE takes before the api's records are read at all
    /// ([`REQUEST_TOO_LARGE`]), which the endpoint states with the media type a recorded
    /// over-sized invoke was answered with.
    fn content_type(&self) -> &'static str {
        if self.response_type == REQUEST_TOO_LARGE_TYPE {
            EDGE_REFUSAL_CONTENT_TYPE
        } else {
            JSON_CONTENT_TYPE
        }
    }
}

/// The media type the `execute-api` endpoint states for the refusal it takes at the edge;
/// see [`GatewayRefusal::content_type`].
const EDGE_REFUSAL_CONTENT_TYPE: &str = "text; charset=utf-8";

/// The API, the stage or the endpoint does not serve this request at all: API Gateway
/// answers `403` with `{"message":"Forbidden"}` for an API id it does not serve, a stage
/// that has no deployment, and an endpoint a developer has turned off.
const FORBIDDEN: GatewayRefusal = GatewayRefusal {
    response_type: "ACCESS_DENIED",
    message: Cow::Borrowed("Forbidden"),
    error_type: Some(FORBIDDEN_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// Nothing in the DEPLOYMENT matches the path and verb, or the method's authorization
/// needs a credential the request did not carry: both are
/// `{"message":"Missing Authentication Token"}` with `403`, which is what API Gateway
/// answers for a route it cannot place and for an `AWS_IAM` method reached unsigned.
const MISSING_AUTHENTICATION_TOKEN: GatewayRefusal = GatewayRefusal {
    response_type: "MISSING_AUTHENTICATION_TOKEN",
    message: Cow::Borrowed("Missing Authentication Token"),
    error_type: Some("MissingAuthenticationTokenException"),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method requires an API key and the request presented none this API's records
/// associate with this stage: `403` `{"message":"Forbidden"}` through `INVALID_API_KEY`.
const INVALID_API_KEY: GatewayRefusal = GatewayRefusal {
    response_type: "INVALID_API_KEY",
    message: Cow::Borrowed("Forbidden"),
    error_type: Some(FORBIDDEN_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// An authorizer's identity source is absent from the request, so the authorizer is not
/// even called, or what it carried is no credential the authorizer could evaluate at all:
/// `401` `{"message":"Unauthorized"}`.
const UNAUTHORIZED: GatewayRefusal = GatewayRefusal {
    response_type: "UNAUTHORIZED",
    message: Cow::Borrowed("Unauthorized"),
    error_type: Some(UNAUTHORIZED_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The exception class the endpoint reports a request that authenticated nobody in, which is
/// the class of every refusal answered through the one `401` gateway response type
/// ([`UNAUTHORIZED`], [`EXPIRED_IDENTITY_TOKEN`]).
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// The identity token an authorizer's identity source carried is a signed token whose own
/// `exp` has passed: `401`, carrying the sentence API Gateway words an EXPIRED token with
/// rather than the one a request that presented nothing earns — a holder whose token ran out
/// has to obtain another, which is a different remedy from presenting a credential at all.
///
/// Answered through the same `UNAUTHORIZED` gateway response as [`UNAUTHORIZED`]: that is the
/// one type a request that authenticated nobody is answered through, and a gateway response's
/// own default template renders THIS refusal's sentence
/// (`{"message":$context.error.messageString}`), so one type carries both sentences and a
/// developer's customization of it reaches both. The catalog's `EXPIRED_TOKEN` type is the
/// `403` of an expired SigV4 security token ([`crate::state::default_gateway_status_code`]),
/// which is a credential of the signing protocol rather than a pool-minted identity token.
const EXPIRED_IDENTITY_TOKEN: GatewayRefusal = GatewayRefusal {
    response_type: "UNAUTHORIZED",
    message: Cow::Borrowed("The incoming token has expired"),
    error_type: Some(UNAUTHORIZED_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's configuration cannot serve the call: no integration on the deployed
/// method, an integration whose type names a backend this emulator cannot call, or a
/// mapping template it cannot evaluate. `500` `{"message":"Internal server error"}`.
const API_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "API_CONFIGURATION_ERROR",
    message: Cow::Borrowed("Internal server error"),
    error_type: Some(INTERNAL_SERVER_ERROR_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The integration's backend had not answered by the integration's own `timeoutInMillis`
/// ("custom timeout between 50 and 29,000 milliseconds", `Integration$timeoutInMillis`), so
/// the call has no backend answer to map: `504` through `INTEGRATION_TIMEOUT`
/// ([`crate::state::default_gateway_status_code`] gives that type's own status), carrying
/// the sentence API Gateway answers an integration that ran out of time with.
///
/// This is what bounds a request to a deployed stage whose integration reaches a backend
/// that runs for its own time ([`lambda_proxy`]): the wait is the api's own stored number
/// rather than anything a request chooses, and a request that reaches it is ANSWERED rather
/// than held open.
const INTEGRATION_TIMEOUT: GatewayRefusal = GatewayRefusal {
    response_type: "INTEGRATION_TIMEOUT",
    message: Cow::Borrowed("Endpoint request timed out"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method is authorized by an `Authorizer` whose decision this emulator cannot
/// obtain — the authorizer names a function this emulator cannot invoke at all.
/// `500` `{"message":"Internal server error"}`, the same class API Gateway reports when it
/// cannot invoke the authorizer.
const AUTHORIZER_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "AUTHORIZER_CONFIGURATION_ERROR",
    message: Cow::Borrowed("Internal server error"),
    error_type: Some(INTERNAL_SERVER_ERROR_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's `Authorizer` RAN and produced no decision: its function errored, did not
/// answer in time, or returned something that is not an authorizer output. `500`
/// `{"message":"Internal server error"}` through `AUTHORIZER_FAILURE`, the gateway response
/// type API Gateway answers an authorizer that failed with — a type of its own, so a
/// developer's customization of it reaches a caller whose authorizer broke without also
/// covering an api that cannot invoke one at all.
const AUTHORIZER_FAILURE: GatewayRefusal = GatewayRefusal {
    response_type: "AUTHORIZER_FAILURE",
    message: Cow::Borrowed("Internal server error"),
    error_type: Some(INTERNAL_SERVER_ERROR_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The request's content type maps to no integration request template and the
/// integration's `passthroughBehavior` does not let an unmapped body through: `415`
/// `{"message":"Unsupported Media Type"}`.
const UNSUPPORTED_MEDIA_TYPE: GatewayRefusal = GatewayRefusal {
    response_type: "UNSUPPORTED_MEDIA_TYPE",
    message: Cow::Borrowed("Unsupported Media Type"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The documented limit on the payload of one request to a REST API: API Gateway's
/// "Payload size" quota is 10,485,760 bytes, and it is one of the quotas the service lists
/// as not adjustable.
pub const MAX_REQUEST_PAYLOAD_BYTES: usize = 10_485_760;

/// The request's payload is larger than [`MAX_REQUEST_PAYLOAD_BYTES`]: `413` through
/// `REQUEST_TOO_LARGE` ([`crate::state::default_gateway_status_code`] gives that type's own
/// status), carrying the sentence API Gateway documents for that condition, under the media
/// type the endpoint declares for this refusal ([`EDGE_REFUSAL_CONTENT_TYPE`]) — so the body
/// is that sentence as text rather than the JSON envelope a gateway response of an api
/// renders.
///
/// Refused at the EDGE, with the api's own records unread and the stage, the route, the
/// authorization, the validator and the integration never reached: the limit is on what the
/// endpoint ADMITS, so it cannot depend on what the api or the method declares — and a
/// developer's customization of a gateway response, which is a record OF an api, therefore
/// does not reach it.
const REQUEST_TOO_LARGE: GatewayRefusal = GatewayRefusal {
    response_type: REQUEST_TOO_LARGE_TYPE,
    message: Cow::Borrowed("Request Too Long"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The refusal a request whose payload is over [`MAX_REQUEST_PAYLOAD_BYTES`] earns, or
/// `None` for one within it.
///
/// Read at the funnel every invoke enters ([`serve`]), so it is the ENDPOINT's limit: it
/// bounds what arrives from a client, not what one integration passes on to another.
///
/// A body the transport did not buffer in memory is one whose size is not known here, and
/// is admitted: the limit refuses a payload that IS over it rather than one that might be.
fn oversized_payload(req: &RtRequest) -> Option<GatewayRefusal> {
    let sent = req.body().bytes()?.len();
    (sent > MAX_REQUEST_PAYLOAD_BYTES).then_some(REQUEST_TOO_LARGE)
}

/// The gateway response type a payload the method's `Model` does not admit is answered
/// through; see [`bad_request_body`].
const BAD_REQUEST_BODY_TYPE: &str = "BAD_REQUEST_BODY";

/// The gateway response type a request missing a parameter the method declares required is
/// answered through; see [`bad_request_parameters`].
const BAD_REQUEST_PARAMETERS_TYPE: &str = "BAD_REQUEST_PARAMETERS";

/// The two gateway response types the method's REQUEST VALIDATOR produces.
///
/// Their default bodies are the VALIDATOR's own serialization, which writes a SPACE after
/// the member separator — `{"message": "Invalid request body"}`, as the Developer Guide's
/// request-validation walkthrough shows both of them — where every OTHER default body is
/// what a gateway response's own default template produces
/// ([`crate::state::DEFAULT_GATEWAY_RESPONSE_TEMPLATE`],
/// `{"message":$context.error.messageString}`), which is compact. One list, read by
/// [`default_refusal_body`], so the two renderings cannot drift apart.
const VALIDATION_RESPONSE_TYPES: [&str; 2] = [BAD_REQUEST_BODY_TYPE, BAD_REQUEST_PARAMETERS_TYPE];

/// The gateway response type an over-sized payload is refused through; see
/// [`REQUEST_TOO_LARGE`].
const REQUEST_TOO_LARGE_TYPE: &str = "REQUEST_TOO_LARGE";

/// The method's request validator declares `validateRequestBody` and the request's payload
/// does not satisfy the JSON Schema of the `Model` the method maps its content type to:
/// `400` `{"message": "Invalid request body"}` through `BAD_REQUEST_BODY`.
///
/// The sentence is FIXED — it names neither the model nor what about the payload failed.
/// WHAT failed is the validation's own report, which a gateway response template reads
/// through `$context.error.validationErrorString`: `detail` is that report, and it is the
/// validator's own finding about THIS payload rather than a sentence written here.
fn bad_request_body(detail: String) -> GatewayRefusal {
    GatewayRefusal {
        response_type: BAD_REQUEST_BODY_TYPE,
        message: Cow::Borrowed("Invalid request body"),
        error_type: Some("BadRequestException"),
        validation_detail: Cow::Owned(detail),
        message_member: MESSAGE_MEMBER,
    }
}

/// The usage plan metering this request's API key admits no further request at this
/// instant: `429` `{"message":"Too Many Requests"}` through `THROTTLED`. The plan's
/// numeric limits are what decide the refusal, not what it says.
const THROTTLED: GatewayRefusal = GatewayRefusal {
    response_type: "THROTTLED",
    message: Cow::Borrowed("Too Many Requests"),
    error_type: Some("TooManyRequestsException"),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The usage plan metering this request's API key admits no further request in the current
/// metering period: `429` `{"message":"Limit Exceeded"}` through `QUOTA_EXCEEDED`. The
/// plan's quota is what decides the refusal, not what it says.
const QUOTA_EXCEEDED: GatewayRefusal = GatewayRefusal {
    response_type: "QUOTA_EXCEEDED",
    message: Cow::Borrowed("Limit Exceeded"),
    error_type: Some("LimitExceededException"),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's request validator declares `validateRequestParameters` and the request
/// does not carry every request parameter the method declares REQUIRED: `400` through
/// `BAD_REQUEST_PARAMETERS`.
///
/// The sentence's fixed part states the condition and its variable part is the list of
/// parameter NAMES the request was missing, read off the method's own
/// `requestParameters` declarations — which is what tells the caller what to add, and
/// which is also the validation's whole finding, so it is reported as the detail too.
fn bad_request_parameters(missing: &[String]) -> GatewayRefusal {
    let named = missing.join(", ");
    GatewayRefusal {
        response_type: BAD_REQUEST_PARAMETERS_TYPE,
        message: Cow::Owned(format!("Missing required request parameters: [{named}]")),
        error_type: Some("BadRequestException"),
        validation_detail: Cow::Owned(format!("[{named}]")),
        message_member: MESSAGE_MEMBER,
    }
}

/// The api's own RESOURCE POLICY does not let this request invoke the method: `403`,
/// carrying the sentence AWS refuses an unauthorized invoke with
/// ([`resource_policy::denial`]).
///
/// Answered through `ACCESS_DENIED`, the gateway response type an authorization failure is
/// customized under, so a developer's `PutGatewayResponse` for it reaches this refusal too.
/// The body's member is the IAM-shaped [`AUTHORIZATION_MESSAGE_MEMBER`] rather than the
/// [`MESSAGE_MEMBER`] every other gateway response carries, because that is the body API
/// Gateway answers an authorization refusal with.
fn access_denied(message: String) -> GatewayRefusal {
    GatewayRefusal {
        response_type: "ACCESS_DENIED",
        message: Cow::Owned(message),
        error_type: Some("AccessDeniedException"),
        validation_detail: Cow::Borrowed(""),
        message_member: AUTHORIZATION_MESSAGE_MEMBER,
    }
}

/// The SigV4 credential an `AWS_IAM` method's caller presented does not authenticate: the
/// access key id names no credential this emulator holds, or the signature is not the one
/// that credential's secret produces over this request
/// ([`sigv4_authentication::Unauthenticated`]).
///
/// Answered through `INVALID_SIGNATURE` — the gateway response type a request whose
/// signature does not conform is customized under, whose own documented status is `403`
/// ([`crate::state::default_gateway_status_code`]) — carrying the sentence AWS words that
/// class with and the exception class the endpoint reports it in, so a client tells a
/// credential it must replace from a signature it must recompute.
fn invalid_signature(class: sigv4_authentication::Unauthenticated) -> GatewayRefusal {
    GatewayRefusal {
        response_type: "INVALID_SIGNATURE",
        message: class.message(),
        error_type: Some(class.error_type()),
        validation_detail: Cow::Borrowed(""),
        message_member: MESSAGE_MEMBER,
    }
}

/// The request was refused by the WAFv2 web ACL in front of the stage: `403` through
/// `WAF_FILTERED`, the gateway response type API Gateway answers a request its web ACL
/// blocked with ([`crate::state::default_gateway_status_code`] gives that type's own
/// status).
///
/// Answered through a gateway response, so a developer's `PutGatewayResponse`
/// customization of `WAF_FILTERED` reaches a blocked caller — and so a `BlockAction`
/// that configures its OWN `CustomResponse` bypasses this entirely, because then WAF
/// serves the response itself ([`EdgeOutcome`]).
const WAF_FILTERED: GatewayRefusal = GatewayRefusal {
    response_type: "WAF_FILTERED",
    message: Cow::Borrowed("Forbidden"),
    error_type: Some(FORBIDDEN_EXCEPTION),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The header API Gateway names the error shape of a refused request in, which is what an
/// SDK reads a restJson1 error code out of.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The exception class every refusal whose body is the `Forbidden` envelope is the wire
/// form of: the endpoint refusing the call outright, whether because the API, the stage or
/// the endpoint does not serve it ([`FORBIDDEN`]), because the method requires an API key
/// the request did not present ([`INVALID_API_KEY`]), or because the web ACL in front of
/// the stage blocked it ([`WAF_FILTERED`]). One sentence, one status, one class: a caller
/// that reads the class reads the same one for all three, as the recorded keyless invoke of
/// a key-required method shows.
const FORBIDDEN_EXCEPTION: &str = "ForbiddenException";

/// The exception class the endpoint reports every refusal whose body is the
/// `Internal server error` envelope is the wire form of: an integration the stage cannot
/// perform ([`API_CONFIGURATION_ERROR`]), an authorizer it cannot invoke at all
/// ([`AUTHORIZER_CONFIGURATION_ERROR`]) and an authorizer whose run produced no decision
/// ([`AUTHORIZER_FAILURE`]). One envelope, one status, one class: a client's retry and
/// alarm handling keys off this header, so a 5xx left without it is an untyped failure a
/// caller can read only as prose.
const INTERNAL_SERVER_ERROR_EXCEPTION: &str = "InternalServerErrorException";

/// What an invoke addresses, as the wire carries it: the request's host — absent on a
/// request that carries neither a `Host` header nor an absolute URI — its path, and the api
/// id a request addressed to a VPC endpoint's own DNS name carries in a header.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the two serving seams read, so one rule resolves both and
/// the claim cannot resolve a different api than the answer does.
#[derive(Debug)]
pub struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    /// The [`API_ID_HEADER`] the request carries, which is how a request on an endpoint's
    /// own DNS name — a host that names the ENDPOINT, not the api — names the api.
    api_id_header: Option<&'a str>,
    /// The interface VPC endpoint this plane ITSELF delivered the request through, when it
    /// passed it on over a private connection whose targets are that endpoint's own network
    /// interfaces ([`private_delivery`]).
    ///
    /// Not read off the wire, and nothing a client can state: it is what THIS plane did with
    /// a request it passed on, handed to the serving of the onward request as an argument
    /// ([`serve_resolved`]), which is why a caller cannot name an endpoint its request did
    /// not arrive through.
    delivered_through: Option<&'a str>,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a http::request::Parts) -> Self {
        Self {
            host: request_host(parts),
            path: parts.uri.path(),
            api_id_header: parts
                .headers
                .get(API_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            delivered_through: None,
        }
    }

    /// The addressing of a runtime-API request, which carries its URI as a string.
    fn of_request(req: &'a RtRequest) -> Self {
        Self {
            host: req.headers().get("host").or_else(|| authority_of(req.uri())),
            path: path_of(req.uri()),
            api_id_header: req.headers().get(API_ID_HEADER),
            delivered_through: None,
        }
    }

    /// The same addressing, of a request this plane DELIVERED through `endpoint`.
    fn delivered_through(mut self, endpoint: Option<&'a str>) -> Self {
        self.delivered_through = endpoint;
        self
    }
}

/// What an invoke request addresses, as this service's records resolve it: the api, the
/// path WITHIN that api's endpoint — `/{stage}/{resource-path}`, which is what the stage
/// and the resource are read from — and the VPC endpoint the request arrived through, when
/// it was addressed on one's own DNS name or this plane delivered it through one.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    api: &'s RestApiEntry,
    invoke_path: &'r str,
    /// The id of the interface VPC endpoint the request ARRIVED through: the one this plane
    /// delivered it through ([`Addressed::delivered_through`]), else the one whose
    /// endpoint-specific DNS name it was addressed on. `None` on every other addressing —
    /// where the request arrived through no endpoint this service can name.
    source_vpce: Option<&'r str>,
}

/// What `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// FIVE addressings resolve, because a client reaches this emulator at a different
/// address than it reaches AWS at:
///
/// * HOST-STYLE, the addressing `RestApi$disableExecuteApiEndpoint` documents: the host's
///   first label names the api (`{api_id}.execute-api.{region}.amazonaws.com`) and the
///   whole path is the invoke path. A host that IS an `execute-api` endpoint but whose api
///   id this service does not hold resolves to nothing at all: that host is another api's
///   (an API Gateway V2 api is reached on the same host shape, and its own service claims
///   it), so reading its path as one of THIS service's stages would make two services claim
///   one request;
/// * VPC-ENDPOINT HOST-STYLE, the addressing a PRIVATE api is reached by through one of its
///   interface VPC endpoints: the host is that ENDPOINT's own DNS name
///   (`{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com`, the name EC2 reports in
///   the endpoint's `DnsEntries`), which names no api at all, so the api travels in the
///   [`API_ID_HEADER`]. This is one of the two addressings that tell this service WHICH
///   endpoint a request arrived through — the other being a request THIS plane delivered
///   through one ([`Addressed::delivered_through`]) — which is what the api's resource policy
///   can then be conditioned on ([`super::resource_policy::SOURCE_VPCE_KEY`]);
/// * PREFIXED PATH-STYLE, the addressing a client reaching an EMULATOR uses: the data
///   plane is named by the reserved namespace segment [`EMULATOR_PATH_NAMESPACE`] and the
///   [`EXECUTE_API_LABEL`], then the api id, then the invoke path —
///   `/_aws/execute-api/{api_id}/{stage}/{resource-path}`. One
///   endpoint serves this emulator's whole surface, control plane and data plane alike, so
///   a client that cannot reach `{api_id}.execute-api.{region}.amazonaws.com` names the
///   data plane in the path before naming the api, which is what keeps an invoke path off
///   the control plane's own modelled path space whatever a stage is called;
/// * PATH-STYLE: the api id is the FIRST PATH SEGMENT and the invoke path is what follows
///   it. A client on the machine this emulator runs on resolves
///   `{api_id}.execute-api.{region}.amazonaws.com` nowhere, so it reaches the emulator's own
///   endpoint and carries in the path what it cannot carry in the host — the same move S3
///   path-style addressing makes with a bucket, and the same one Lightsail's container-service
///   URL makes with the labels AWS puts in a hostname. It is read only on a host that names
///   NO SERVICE (the emulator's own endpoint) or names THIS one, so a first segment that
///   happens to equal an api id cannot take a request away from the service whose endpoint
///   the host names;
/// * STAGE-ONLY, for a client that carries the invoke path alone: a host that names NO
///   SERVICE at all is resolved by the STAGE segment of the path, the only part of such a
///   request that names a deployed stage. That is the host-independent claim
///   ApiGatewayManagementApi makes over its own `@connections` path space and
///   `service-com-amazonaws-apigatewayv2-apigatewayv2`'s WebSocket data plane makes over a
///   stage's. Which service a host names is the FRONT DOOR's own rule
///   ([`service_named_by_host`]), so the stage is read exactly for the request the front door
///   could place nowhere, and a host that names some service — this service's own control
///   plane, or another service's endpoint — keeps answering as that service. The host alone
///   decides it, and not the whole routing key: an `AWS_IAM` method is invoked with a SigV4
///   credential, whose scope resolves a routing key for every such request, so a claim
///   conditioned on the key having resolved nothing would leave exactly the signed invokes
///   unclaimed. When several apis publish a stage of that name the request names no ONE
///   api, so nothing is resolved rather than one of them guessed at; each is reachable by an
///   addressing that names it, which is what the prefixed form above is for.
///
/// This service's own CONTROL-plane path space is never read as a stage: a request whose
/// first segment is one of the model's route segments is a control-plane call on whatever
/// host it arrives at, and answering it from the data plane would take `/restapis` away
/// from the operations that serve it. A stage named like one of those segments is reachable
/// through the three addressings above, where the api is named before the stage is.
fn resolve<'s, 'r>(
    store: &'s crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    // The `@connections` path space belongs to ApiGatewayManagementApi, on either host —
    // see [`CONNECTIONS_SEGMENT`].
    if addresses_connections(addressed.path) {
        return None;
    }
    // VPC-ENDPOINT HOST-STYLE first: such a host IS an `execute-api` endpoint host, so the
    // api-id reading below would take its endpoint id for an api id and resolve nothing.
    if let Some(endpoint) = addressed.host.and_then(endpoint_of_host) {
        return addressed
            .api_id_header
            .and_then(|api_id| store.rest_apis.get(api_id))
            .map(|api| Resolved {
                api,
                invoke_path: addressed.path,
                // The endpoint this plane DELIVERED the request through is where it really
                // arrived, whatever name it was addressed on; a request nothing delivered
                // arrived at the endpoint whose own name it names.
                source_vpce: addressed.delivered_through.or(Some(endpoint)),
            });
    }
    if let Some(api_id) = addressed.host.and_then(api_id_of_host) {
        return store.rest_apis.get(api_id).map(|api| Resolved {
            api,
            invoke_path: addressed.path,
            source_vpce: addressed.delivered_through,
        });
    }
    // PREFIXED PATH-STYLE: the path names this data plane before it names the api, so it is
    // read on whatever host it arrives at — the prefix is in no service's modelled path
    // space, and an api id this service does not hold is served by nothing here.
    if let Some((api_id, invoke_path)) = split_execute_api_prefix(addressed.path) {
        return store.rest_apis.get(api_id).map(|api| Resolved {
            api,
            invoke_path,
            source_vpce: addressed.delivered_through,
        });
    }
    let (first, rest) = split_first_segment(addressed.path)?;
    // Which service the host names, by the front door's own rule: `None` is the emulator's
    // own endpoint, and this service's own key is its control-plane host.
    let host_service = addressed.host.and_then(service_named_by_host);
    if host_service
        .as_deref()
        .is_some_and(|service| service != crate::ROUTE_KEY)
    {
        // Another service's endpoint: whatever its path carries is that service's to
        // answer, so neither addressing below is read and one request keeps one claimant.
        return None;
    }
    // PATH-STYLE: the first segment names an api this service holds, and the invoke path
    // is what follows it.
    if let Some(api) = store.rest_apis.get(first) {
        return Some(Resolved {
            api,
            invoke_path: rest,
            source_vpce: addressed.delivered_through,
        });
    }
    if host_service.is_some() {
        // This service's own control-plane host: a path segment that is no api id of this
        // account is a control-plane call, not a stage.
        return None;
    }
    if crate::ROUTE_PATH_SEGMENTS.contains(&first) {
        return None;
    }
    // STAGE-ONLY: the first segment names a stage, and the api is the one that holds a
    // stage of that name. When several hold one the request names no ONE api, so nothing is
    // resolved rather than one of them guessed at — the three addressings above carry the
    // api id and are never ambiguous.
    let mut holding = store
        .rest_apis
        .values()
        .filter(|api| api.stages.contains_key(first));
    let api = holding.next()?;
    holding.next().is_none().then_some(Resolved {
        api,
        invoke_path: addressed.path,
        source_vpce: addressed.delivered_through,
    })
}

/// The api id and the invoke path of a PREFIXED PATH-STYLE address, or `None` when `path`
/// does not carry that prefix.
///
/// The prefix is [`EMULATOR_PATH_NAMESPACE`] then [`EXECUTE_API_LABEL`], by exact segment
/// equality, and what follows is the api id and then the invoke path the host-style
/// addressing carries whole — `/_aws/execute-api/{api-id}/{stage}/{resource-path}`. The
/// invoke path is a SLICE of the request's own path, so nothing is copied, and a prefix
/// with no api id after it resolves nothing.
fn split_execute_api_prefix(path: &str) -> Option<(&str, &str)> {
    let (prefix, after_prefix) = split_first_segment(path)?;
    if prefix != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (label, after_label) = split_first_segment(after_prefix)?;
    if label != EXECUTE_API_LABEL {
        return None;
    }
    split_first_segment(after_label)
}

/// The first non-empty segment of `path` and the path that follows it, or `None` for a
/// path with no segment at all.
///
/// The remainder keeps no leading `/` — [`path_segments`] drops empty segments, so the two
/// spellings read the same — and is a SLICE of the path, so a resolved addressing borrows
/// the request rather than copying it.
fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.split_once('/') {
        Some((first, rest)) => (first, rest),
        None => (trimmed, ""),
    };
    (!first.is_empty()).then_some((first, rest))
}

/// The service `host` names to the emulator's front door, or `None` when it names none.
///
/// The front door's OWN host reader decides it (`emulator_server::service_from_host`, the
/// last tier of the routing key every front door resolves), rather than a second rule
/// written here: the request the two path addressings are read for is exactly the one that
/// reader can place nowhere — a loopback address or `localhost`, which is the only endpoint
/// a client on this machine can reach. It is handed the host as a `Host` header because
/// that is the shape it reads, and a host no header can carry names no service.
fn service_named_by_host(host: &str) -> Option<String> {
    let mut headers = http::HeaderMap::new();
    let value = http::HeaderValue::from_str(host).ok()?;
    headers.insert(http::header::HOST, value);
    ::emulator_server::service_from_host(&headers)
}

/// Whether THIS data plane serves the request these parts came from — the data-plane half
/// of `Service::can_handle`.
///
/// Narrowed to an api this service HOLDS, read from the one shared store, so nothing is
/// claimed that this service cannot answer about and exactly one service claims a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    claims(engine, &Addressed::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the two serving seams are handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Addressed::of_request(req))
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// The first label of `host` when `host` is an `execute-api` endpoint: the API id.
///
/// The shape is `{api-id}.execute-api.{region}.amazonaws.com` — a first label, then the
/// [`EXECUTE_API_LABEL`] label, then at least a region and a domain ([`endpoint_labels`]).
fn api_id_of_host(host: &str) -> Option<&str> {
    endpoint_labels(host).map(|(api_id, _region)| api_id)
}

/// The two parts every `execute-api` endpoint host carries: its FIRST label — which names
/// the api on an api's own endpoint and the VPC endpoint on an endpoint-specific one — and
/// the Region label that follows the [`EXECUTE_API_LABEL`].
///
/// One reader for both forms, because both put the same two parts in the same places:
/// `{api-id}.execute-api.{region}.amazonaws.com` and
/// `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com`. Any `:port` and userinfo is
/// removed first, as the front door's own host reader does. The region and the domain are
/// not matched against a list: an emulator serves whatever region a caller addresses, and a
/// FIPS or a dualstack spelling of the same endpoint is the same endpoint.
fn endpoint_labels(host: &str) -> Option<(&str, &str)> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    let mut labels = without_port.split('.');
    let first = labels.next()?;
    let second = labels.next()?;
    // The label is `execute-api` itself, or one of its endpoint VARIANTS, which AWS spells
    // with a suffix after a hyphen (`execute-api-fips`). A label that merely STARTS with
    // those characters (`execute-apifoo`) is a different host and is not claimed.
    let execute_api = second == EXECUTE_API_LABEL
        || second
            .strip_prefix(EXECUTE_API_LABEL)
            .is_some_and(|variant| variant.starts_with('-') && variant.len() > 1);
    if first.is_empty() || !execute_api {
        return None;
    }
    // A region label and a domain: `{api-id}.execute-api.{region}.amazonaws.com` has four
    // labels after the id, and the shortest form this accepts still has two, so a bare
    // `execute-api.something` (no api id in front) is not read as one.
    let region = labels.next()?;
    (labels.count() >= 1).then_some((first, region))
}

/// The id of the interface VPC endpoint whose own DNS name `host` is, or `None` for a host
/// that is not one.
///
/// An interface endpoint to the Region's `execute-api` service answers on the
/// endpoint-specific names EC2 reports in its `DnsEntries` — the regional
/// `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com` and one zonal
/// `{vpc-endpoint-id}-{zone}.…` per Availability Zone its interfaces sit in — so the id is
/// the first label up to the zone suffix, and the zonal and regional names resolve to the
/// SAME endpoint. The id is [`VPC_ENDPOINT_ID_PREFIX`] and one of the hexadecimal widths
/// EC2's id pattern admits ([`crate::state::is_vpc_endpoint_id`]), which is also what tells an
/// endpoint's own name apart from an api id that merely begins with those characters.
fn endpoint_of_host(host: &str) -> Option<&str> {
    let (first, _region) = endpoint_labels(host)?;
    let hex = first
        .strip_prefix(VPC_ENDPOINT_ID_PREFIX)?
        .split('-')
        .next()?;
    // By `get`, never by a byte-index slice: the label is read off the wire, and the id's
    // own width is all that has been established about it.
    first
        .get(..VPC_ENDPOINT_ID_PREFIX.len() + hex.len())
        .filter(|id| crate::state::is_vpc_endpoint_id(id))
}

/// Whether `path` addresses the `@connections` space of ApiGatewayManagementApi, in either
/// of the two forms that service's callers use: with the stage prefix the SDK's endpoint
/// carries (`/{stage}/@connections/{id}`) and without it (`/@connections/{id}`).
fn addresses_connections(path: &str) -> bool {
    path_segments(path)
        .into_iter()
        .take(2)
        .any(|segment| segment == CONNECTIONS_SEGMENT)
}

/// Serve one request addressed to a deployed stage's invoke URL, from the control plane's
/// own records.
///
/// Free-standing and reached with the ONE shared engine, from the one seam that serves
/// this data plane — the operation `Service::override_operation` names for a claimed
/// invoke, which declares no auth scheme so the router serves it whatever credential it
/// carries — so every invoke of a deployed stage is answered from the same records.
///
/// `ctx` is what the router hands that operation: the IN-PROCESS transport an integration
/// naming another emulated service's action is performed over ([`aws_integration`]).
///
/// Every response leaves here NAMING THE REQUEST it answered, in the header API Gateway
/// reports a request id in: this is the one funnel every outcome reaches, so a served
/// integration, a gateway refusal and an endpoint that serves nothing all report one, and
/// a client can correlate the response it holds with the request the API processed.
/// `service_common` owns both the minting and the header, so this data plane reports the
/// id in the one spelling the rest of the emulator reports it in.
pub async fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> RtResponse {
    // Minted HERE, before anything is served, because the id names the request in TWO
    // places: the header this response reports it in and the `$context.requestId` the
    // call's own mapping templates and proxy event read, which a client correlates the
    // answer it holds with what the API processed by. `service_common` owns the minting and
    // the header, so this data plane reports the id in the one spelling the rest of the
    // emulator reports it in.
    let request_id = ::service_common::query_error_envelope::new_request_id();
    // MEASURED over the whole invoke, because that is what the access log reports as the
    // latency of answering this request: the clock starts before anything is resolved and is
    // read once the response has been computed.
    let started = ::std::time::Instant::now();
    // What this request's own AUTHORIZATION published about the caller — a custom authorizer's
    // values and principal, a user pool's claims — filled by the resolution below. The STAGE's
    // access-log record is rendered from the call's `$context` values, and these are part of
    // them (`$context.authorizer.principalId` is an access-log variable of its own), so they
    // are carried out to the delivery rather than reaching the integration alone.
    let mut authorized: BTreeMap<String, String> = BTreeMap::new();
    let served = ::service_common::query_error_envelope::with_stated_request_id_header(
        // The documented payload limit is the ENDPOINT's, and it is answered HERE, before
        // anything the api's own records decide: not the stage, the route, the
        // authorization, the validator or the integration, and not the content coding
        // `minimumCompressionSize` asks for either — so a developer's records cannot change
        // the bytes of a refusal taken before they were read.
        match oversized_payload(req) {
            Some(refusal) => {
                tracing::warn!(
                    limit = MAX_REQUEST_PAYLOAD_BYTES,
                    "the request's payload is over the payload limit of a REST API"
                );
                refusal_response(None, refusal)
            }
            // COMPRESSED at the one funnel every other outcome reaches, so what the api's
            // `minimumCompressionSize` asks for applies to a served integration, a cached
            // response and a gateway refusal alike ([`compression`]).
            None => compression::compressed(
                engine,
                req,
                serve_resolved(engine, req, ctx, 0, &request_id, &mut authorized, None).await,
            ),
        },
        &request_id,
    );
    // What the STAGE logs about the request it just answered — its access-log record and the
    // execution log its method settings turn on — delivered to the destinations the stage
    // and the account name ([`stage_logs`]). After the response is computed and never
    // before: a record is about a request that was answered.
    stage_logs::delivered(
        engine,
        req,
        ctx,
        &request_id,
        &served,
        started.elapsed(),
        &authorized,
    )
    .await;
    served
}

/// The response this data plane's own records compute for `req`, before the request id is
/// reported on it — the resolution and the serving, which is everything the module docs
/// describe.
///
/// `hop` counts the passed-through integrations already followed to get here: a proxy
/// integration is served by passing the request on to this same function, so the count is
/// what bounds a chain of them ([`MAX_PASSED_THROUGH_HOPS`]). The future is BOXED for
/// exactly that reason: a function that awaits itself has no sized future.
///
/// `request_id` is the id the response this computes is reported under, which is also what
/// the call carries as `$context.requestId`.
///
/// `delivered_through` is the interface VPC endpoint this plane itself delivered this request
/// through, when it is the onward request of a private integration carried over a VPC link
/// ([`private_delivery`]): `None` for every request that arrived from outside, which is what
/// keeps the endpoint a call arrived through out of a caller's hands.
fn serve_resolved<'a, 'c: 'a, Eng: Send + Sync + 'static>(
    engine: &'a Eng,
    req: &'a RtRequest,
    ctx: &'a OperationContext<'c>,
    hop: u8,
    request_id: &'a str,
    authorized: &'a mut BTreeMap<String, String>,
    delivered_through: Option<&'a str>,
) -> BoxFuture<'a, RtResponse> {
    Box::pin(async move {
        serving(
            engine,
            req,
            ctx,
            hop,
            request_id,
            authorized,
            delivered_through,
        )
        .await
    })
}

/// [`serve_resolved`]'s body, written as an ordinary `async fn` so only the recursion
/// above pays for a box.
async fn serving<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    hop: u8,
    request_id: &str,
    authorized: &mut BTreeMap<String, String>,
    delivered_through: Option<&str>,
) -> RtResponse {
    let Some(state) = crate::state::service_state(engine) else {
        return refusal_response(None, API_CONFIGURATION_ERROR);
    };
    // The WAFv2 WEB ACL in front of the stage decides before the stage does. A web ACL
    // is "a set of rules that defines the strategy that WAF uses to inspect and handle
    // web requests" and it sits in FRONT of the resource it is associated with, so it
    // is asked ahead of everything the api itself declares about the request — the
    // route, the authorization, the api key, the validator and the integration, none of
    // which a request it blocks ever reaches. A stage with no web ACL associated is
    // served exactly as it was before web ACLs were read at all.
    let with_inserted_headers;
    let (outcome, logged) = web_acl_decision(engine, &state, req);
    // What the web ACL LOGS about the request it has just decided, delivered to the
    // destination its own `LoggingConfiguration` names before the stage is reached: WAF
    // records the request it inspected, so the record is delivered for a request it
    // blocked at the edge as much as for one it let through to the api.
    if let Some(logged) = logged.as_ref() {
        waf_logs::deliver(ctx, logged).await;
    }
    let req = match outcome {
        EdgeOutcome::Refused(response) => return *response,
        EdgeOutcome::Unchanged => req,
        // An `AllowAction`'s (or a `CountAction`'s) `CustomRequestHandling` inserts
        // headers "into the web request", so the request the stage goes on to serve —
        // and the one a proxy integration passes to the origin — is the request
        // carrying them.
        EdgeOutcome::Allowed(inserted) => match request_with_headers(req, &inserted) {
            Some(rebuilt) => {
                with_inserted_headers = rebuilt;
                &with_inserted_headers
            }
            None => req,
        },
    };
    let addressed = Addressed::of_request(req).delivered_through(delivered_through);
    // What this request RESOLVES to: the integration that is to answer it, what VALIDATING
    // its payload needs, and what METERING it is subject to. All three are read under a
    // shared read of the store, which is RELEASED before any of them runs: validating a
    // payload against a caller-stored JSON Schema is the one piece of work here whose cost
    // a caller's own records decide, so it must not hold the lock every other request to
    // this service waits on, and recording a metered request needs an exclusive lock of
    // its own.
    //
    // The resolution runs at most TWICE, because one step of it is a call: a method whose
    // `CUSTOM` authorizer the stage holds no result for cannot be decided without INVOKING
    // the authorizer's function, which is made with no lock held ([`custom_authorizer`]) and
    // then handed back to the resolution, which decides the request with the result in hand.
    let mut decided: Option<CustomDecision> = None;
    // The X-Ray trace context this REQUEST propagates, decided on the first pass and carried
    // across the second: a request that reaches both an authorizer's function and the
    // integration's must belong to ONE trace, so the chain, the decision and the stage's own
    // span are decided once per request rather than once per pass ([`trace_context`]).
    let mut carried_trace: Option<trace_context::Propagated> = None;
    let plan = loop {
        let planned = 'planned: {
            let store = state.lock();
            let Some(resolved) = resolve(&store, &addressed) else {
                // The api was deleted between the claim and now, or another process minted
                // the host: an endpoint that serves no api of this account is `403 Forbidden`.
                break 'planned Planned::Answered(Box::new(refusal_response(None, FORBIDDEN)));
            };
            let api = resolved.api;
            if api.disable_execute_api_endpoint {
                // `disableExecuteApiEndpoint`: "Specifies whether clients can invoke your API
                // by using the default execute-api endpoint." Turned off, the endpoint serves
                // nothing.
                break 'planned Planned::answered(refusal_response(Some(api), FORBIDDEN));
            }
            let Some((stage_name, resource_path)) = split_stage(resolved.invoke_path) else {
                break 'planned Planned::answered(refusal_response(Some(api), FORBIDDEN));
            };
            let Some(stage) = api.stages.get(stage_name) else {
                break 'planned Planned::answered(refusal_response(Some(api), FORBIDDEN));
            };
            let Some(deployment) = api.deployments.get(&stage.deployment_id) else {
                break 'planned Planned::answered(refusal_response(Some(api), FORBIDDEN));
            };

            // The DEPLOYMENT decides what is served: a method put on the API after the
            // stage's deployment was created is not in it, and one deleted afterwards still
            // is.
            let Some(matched_path) = match_resource_path(deployment, &resource_path) else {
                break 'planned Planned::answered(refusal_response(
                    Some(api),
                    MISSING_AUTHENTICATION_TOKEN,
                ));
            };
            let Some((verb, snapshot)) = match_method(deployment, matched_path, req.method())
            else {
                break 'planned Planned::answered(refusal_response(
                    Some(api),
                    MISSING_AUTHENTICATION_TOKEN,
                ));
            };

            let authorization_type = snapshot
                .authorization_type
                .as_deref()
                .unwrap_or(AUTHORIZATION_NONE);
            // The whole method is the DEPLOYMENT's own record of it, not the api's live one:
            // "a Deployment resource is a snapshot of your REST API configuration"
            // (`Deployment`), and a stage serves the deployment its `deploymentId` names, so
            // the integration this call is served through — its templates, its responses, its
            // cache configuration, the authorizer and the request validator it names — is the
            // one that stood when the deployment was created
            // ([`crate::state::DeploymentEntry::resources`] holds that tree). An edit made
            // after it is not served until the api is redeployed, which is what makes a stage
            // serve one configuration while the developer changes another.
            // `Deployment$apiSummary`'s `MethodSnapshot` cannot stand in for it: it models
            // only `authorizationType` and `apiKeyRequired`.
            let deployed_resource = deployment
                .resources
                .values()
                .find(|resource| resource.path == matched_path);
            let deployed_method =
                deployed_resource.and_then(|resource| resource.methods.get(verb));

            // An `AWS_IAM` method is authorized by IAM, and that is a decision about the
            // principal a credential speaks for — so a credential the request DOES carry is
            // AUTHENTICATED first: the access key id must name one this emulator holds and
            // the signature must be the one that credential's secret produces. A made-up key
            // id, and a real one signed with the wrong secret, are refused here, with the
            // api's own document unread and the integration never performed
            // ([`sigv4_authentication`]). A request carrying NO credential is not this
            // refusal: it is the missing-token one the method's own authorization states
            // below ([`authorization_refusal`]).
            if authorization_type == AUTHORIZATION_AWS_IAM && carries_sigv4_credential(req) {
                if let Some(unauthenticated) = sigv4_authentication::refusal(req, ctx) {
                    tracing::warn!(
                        api_id = %api.id,
                        stage = %stage.stage_name,
                        path = %matched_path,
                        verb = %verb,
                        class = ?unauthenticated,
                        "the credential this invoke presented does not authenticate"
                    );
                    break 'planned Planned::answered(refusal_response(
                        Some(api),
                        invalid_signature(unauthenticated),
                    ));
                }
            }

            // Who the request speaks for is the METHOD's authorization, not what the request
            // carries: API Gateway authenticates nobody for a method whose
            // `authorizationType` is `NONE`, so such a call speaks for nobody however it is
            // signed, while an `AWS_IAM` method speaks for the principal the credential it
            // signed with was minted for — the session STS holds for it, which is what makes
            // a credential an identity pool VENDED a principal this api's policies can be
            // written about ([`execute_api_authorization`]).
            let signing = (authorization_type == AUTHORIZATION_AWS_IAM).then(|| {
                execute_api_authorization::signing_caller(
                    engine,
                    sigv4_access_key_id(req).unwrap_or_default().as_str(),
                    ctx.credential_store(),
                    SystemTime::now(),
                )
            });
            let signed_as = signing.as_ref().map(|caller| caller.arn.clone());
            // A credential the clock has already run out on speaks for nobody, so nothing
            // below has a principal to decide about: it is refused here, BEFORE the api's own
            // document is asked, rather than letting a wildcard `Principal` match a
            // credential that names no one. (In the assembled emulator the layer that
            // authenticates a request refuses such a credential before this plane is reached
            // at all.)
            if let Some(message) = signing
                .as_ref()
                .and_then(execute_api_authorization::expired_refusal)
            {
                break 'planned Planned::answered(refusal_response(
                    Some(api),
                    access_denied(message),
                ));
            }
            // Built once, because FOUR decisions are taken about this call: what the method's
            // own authorizer says about the method it addressed, whether the caller's own
            // policies let it invoke that method, whether the document the api carries does,
            // and whether that document lets the caller ask for the stage's cached response
            // to be reloaded.
            let call_of = resource_policy::Call {
                // The api's own Region, from the record: an api lives in exactly one
                // Region, so the ARN a statement names its methods by carries the Region
                // the api was created in, not one read off whichever endpoint this
                // caller reached the emulator by (the path-addressed data plane reaches
                // it at a host that spells no Region at all).
                region: api.arn_region(),
                api_id: &api.id,
                stage: &stage.stage_name,
                verb,
                resource_path: matched_path,
                // The resource an authorization decision is taken about is the path the
                // CLIENT addressed, not the template the deployment matched by it: a
                // statement is written about the paths a client reaches.
                request_path: &resource_path,
                source_vpce: resolved.source_vpce,
                caller: match signed_as.as_deref() {
                    Some(principal) => resource_policy::Caller::Signed(principal),
                    None => resource_policy::Caller::Anonymous,
                },
            };
            let method_arn = call_of.method_arn();

            let mut call = MethodRequest::of_request(
                req,
                // What a mapping template of this call renders against: the method request as
                // it arrived, the path parameters the matched resource template binds, and
                // the STAGE's own variables and identity.
                TemplateInputs {
                    payload: req.body().bytes().map(<[u8]>::to_vec),
                    path: matched_path_parameters(matched_path, &resource_path),
                    querystring: sent_query_parameters(query_of(req.uri())),
                    header: sent_headers(req),
                    stage_variables: stage.variables.clone(),
                    context: call_context(
                        &api.id,
                        &stage.stage_name,
                        verb,
                        matched_path,
                        req,
                        request_id,
                        SystemTime::now(),
                    ),
                },
                // What a PROXY integration's event names this call by, which a template has
                // no reference for: the request as the CLIENT sent it (its own verb, every
                // value a repeated parameter or header carries, the endpoint it arrived on)
                // and the records it resolved to — the `Resource` whose path matched, and the
                // ARN of the method a grant to API Gateway is narrowed by.
                lambda_proxy::ProxyInputs {
                    resource_id: deployed_resource
                        .map(|resource| resource.id.clone())
                        .unwrap_or_default(),
                    http_method: req.method().to_ascii_uppercase(),
                    request_path: resource_path.clone(),
                    domain_name: request_host_of(req).unwrap_or_default(),
                    multi_value_querystring: every_query_parameter(query_of(req.uri())),
                    multi_value_header: every_header(req),
                    source_arn: Some(method_arn.clone()),
                    // What the api declares BINARY, which decides whether a base64 body the
                    // function returns reaches the client as the bytes it names or as the
                    // base64 text it is ([`lambda_proxy`]).
                    binary_media_types: api.binary_media_types.clone(),
                    // Whether this STAGE is tracing, which is the sampling decision the
                    // call this stage makes onward carries when the caller took none
                    // ([`trace_context`]).
                    tracing_enabled: stage.tracing_enabled,
                },
                &mut carried_trace,
            );

            // What the METHOD's own authorization makes of the call. A `CUSTOM` authorizer
            // the stage holds no result for stops the resolution here: its function has to
            // be asked, which happens off this lock.
            let published = match authorization_refusal(
                engine,
                api,
                deployed_method,
                authorization_type,
                req,
                CustomAuthorization {
                    stage,
                    method_arn: &method_arn,
                    call: &call,
                    decided: decided.as_ref(),
                },
            ) {
                Authorized::Published(published) => published,
                Authorized::Refused(refusal) => {
                    break 'planned Planned::answered(refusal_response(Some(api), refusal))
                }
                Authorized::Pending(pending) => {
                    // Asked once per request: a second pass that still has no result is an
                    // authorizer this call cannot obtain a decision from at all.
                    if decided.is_some() {
                        break 'planned Planned::answered(refusal_response(
                            Some(api),
                            AUTHORIZER_FAILURE,
                        ));
                    }
                    break 'planned Planned::Pending(pending);
                }
            };
            // What the authorization published is part of the call: a mapping template reads
            // a user pool's claims and a custom authorizer's own values off `$context`. The
            // STAGE's own access-log record is rendered from those same values, so what the
            // authorization published is handed back to the funnel that delivers it
            // ([`stage_logs`]) rather than being left to the integration alone.
            *authorized = published.clone();
            call.inputs.context.extend(published);

            // The api's own RESOURCE POLICY, which "applies to this RestApi regardless of the
            // caller and Method configuration" (`RestApi$policy`): a document the developer
            // attached decides every request this endpoint answers, so it is applied here —
            // before the api key, the request validator, the metering and the integration,
            // none of which a request it refuses ever reaches. An api carrying no document is
            // asked nothing and is served as it was before resource policies were read at
            // all.
            let attached_policy = api
                .policy
                .as_deref()
                .filter(|policy| !policy.trim().is_empty());
            if let Some(policy) = attached_policy {
                if let Some(message) = resource_policy::denial(policy, &call_of) {
                    tracing::warn!(
                        api_id = %api.id,
                        stage = %stage.stage_name,
                        path = %matched_path,
                        verb = %verb,
                        "the api's resource policy does not allow this invoke"
                    );
                    break 'planned Planned::answered(refusal_response(
                        Some(api),
                        access_denied(message),
                    ));
                }
            }
            // An `AWS_IAM` method is authorized by IAM, and that is a decision about the
            // PRINCIPAL the verified credential speaks for: `execute-api:Invoke` on this
            // method's own ARN, allowed by the caller's identity-based policies or by the
            // document the api carries. Taken after the resource policy's own deny, so an
            // explicit deny there keeps answering first.
            if let Some(signing) = signing.as_ref() {
                let allowed_by_policy = resource_policy::allows_invoke(attached_policy, &call_of);
                if let Some(message) =
                    execute_api_authorization::refusal(signing, &call_of, allowed_by_policy)
                {
                    tracing::warn!(
                        api_id = %api.id,
                        stage = %stage.stage_name,
                        path = %matched_path,
                        verb = %verb,
                        "the caller is not authorized to invoke this method"
                    );
                    break 'planned Planned::answered(refusal_response(
                        Some(api),
                        access_denied(message),
                    ));
                }
            }
            // A key-required method is admitted by the key the records associate with this
            // stage, and the association is also what says which usage plans METER the call.
            let admission = if snapshot.api_key_required {
                match api_key_admission(&store, api, &stage.stage_name, req) {
                    Some(admission) => Some(admission),
                    None => {
                        break 'planned Planned::answered(refusal_response(
                            Some(api),
                            INVALID_API_KEY,
                        ))
                    }
                }
            } else {
                // A method that requires no key presents the records no key to meter: a usage
                // plan is what "specify[ies] who can access associated API stages", so it
                // meters the calls it admits.
                None
            };

            // The method's own request validator, before the integration is reached: a
            // request the method declares invalid never gets to the backend. The PARAMETERS
            // half is decided here, from the request's own query string and headers; the BODY
            // half is prepared here and decided below, off the lock.
            if let Some(refusal) = parameters_refusal(api, deployed_method, &call) {
                break 'planned Planned::answered(refusal_response(Some(api), refusal));
            }
            let pending_body = pending_body_check(api, deployed_method, &call);

            // The STAGE's own cache, which answers this call instead of its integration when
            // the cluster holds an entry for it: WHICH entry, for how long, and what became
            // of a cache-control header asking for it to be reloaded are all the stage's own
            // settings ([`cache_plan`]). The lookup itself happens below, once the request
            // has got past everything the method declares about it.
            let caching = match cache_plan(
                stage,
                deployed_method.and_then(|method| method.integration.as_ref()),
                &resource_path,
                req,
                attached_policy,
                &call_of,
            ) {
                Caching::Off => None,
                Caching::On(plan) => Some(plan),
                // The stage requires authorization for a cache-control header, this caller
                // holds none and the stage's strategy is to fail such a request: answered
                // through the refusal this endpoint takes for an authorization it cannot
                // take at all ([`invalidation_refusal`]).
                Caching::Refused(refusal) => {
                    tracing::warn!(
                        api_id = %api.id,
                        stage = %stage.stage_name,
                        path = %matched_path,
                        verb = %verb,
                        "the caller is not authorized to invalidate this stage's cached response"
                    );
                    break 'planned Planned::answered(refusal_response(Some(api), refusal));
                }
            };

            // The integration is what answers, and it is READ here and PERFORMED below, after
            // the payload check: the step list this module opens with has the request
            // validator ahead of the integration, so a payload the method's model does not
            // admit is refused with the integration never performed at all. Performing it is
            // the half this module shares with `TestInvokeMethod`, so the SAME records decide
            // a simulated invocation and a real one.
            // The STAGE's own variables are part of what the integration addresses: a
            // reference to one in its configuration resolves against the stage this call
            // arrived at, so one deployment reached through two stages addresses each stage's
            // own backend ([`with_stage_variables`]).
            let performed_method = match deployed_method {
                Some(method) => match with_stage_variables(method, &stage.variables) {
                    Ok(resolved) => Some(resolved),
                    Err(unserved) => {
                        tracing::warn!(
                            api_id = %api.id,
                            stage = %stage.stage_name,
                            path = %matched_path,
                            verb = %verb,
                            reason = %unserved.reason,
                            "the stage cannot serve this call through the method's integration"
                        );
                        break 'planned Planned::answered(refusal_response(
                            Some(api),
                            unserved.refusal,
                        ));
                    }
                },
                None => None,
            };
            let integration = ResolvedIntegration {
                api_id: api.id.clone(),
                stage: stage.stage_name.clone(),
                path: matched_path.to_string(),
                verb: verb.to_string(),
                region: api.arn_region().to_string(),
                // The private connection a passed-through integration is carried over is this
                // service's own record, so it is resolved here, under the one lock that read
                // the integration itself — off the configuration the stage's variables have
                // already been resolved into, so a `connectionId` that named one names the
                // link that variable's value names.
                unusable_connection: performed_method
                    .as_ref()
                    .and_then(|method| method.integration.as_ref())
                    .and_then(|integration| unusable_vpc_link(&store, integration)),
                // What that private connection TARGETS, read under the same lock and off
                // the same resolved configuration: where those targets are an interface
                // VPC endpoint's own network interfaces, the request this integration
                // passes on arrives at this plane through that endpoint
                // ([`private_delivery`]).
                connection_load_balancers: performed_method
                    .as_ref()
                    .and_then(|method| method.integration.as_ref())
                    .and_then(|integration| {
                        private_delivery::connection_load_balancers(&store, integration)
                    }),
                method: performed_method,
                call,
            };
            // The 429s this call earns if the plans metering it have no request left for
            // it — one for the quota's count and one for the throttle's rate — rendered now
            // because each is rendered from the api's own gateway responses.
            let metering = admission
                .filter(|admission| !admission.plans.is_empty())
                .map(|admission| Metering {
                    quota_exceeded: refusal_response(Some(api), QUOTA_EXCEEDED),
                    throttled: refusal_response(Some(api), THROTTLED),
                    admission,
                });
            Planned::Ready(Box::new(Plan {
                integration,
                pending_body,
                metering,
                caching,
            }))
        };
        match planned {
            Planned::Answered(response) => return *response,
            // The authorizer is asked with NO lock held, and what it returned is handed back
            // to the resolution above — which then decides this request, and every later one
            // the stage serves from the result it now holds.
            Planned::Pending(pending) => {
                decided = Some(obtained_decision(ctx, &state, *pending).await);
            }
            Planned::Ready(plan) => break *plan,
        }
    };
    let Plan {
        integration,
        pending_body,
        metering,
        caching,
    } = plan;

    // The usage plan METERS the call before its payload is checked, as API Gateway's
    // method-request flow does: a request the plan sheds is answered 429 whatever its
    // payload says. This is the exclusive-lock half, so it is taken on its own.
    if let Some(metering) = metering {
        let mut store = state.lock();
        match metered_refusal(&mut store, &metering.admission) {
            Some(Shed::Quota) => return metering.quota_exceeded,
            Some(Shed::Throttle) => return metering.throttled,
            None => {}
        }
    }

    // OFF the lock: the payload against the model's JSON Schema. A refused payload is
    // answered through the api's own gateway response for `BAD_REQUEST_BODY`, so the
    // records are read again — briefly, and only to render that refusal.
    if let Some(pending) = pending_body {
        if let Some(refusal) = pending.refusal(req.body().bytes()) {
            let store = state.lock();
            let api = resolve(&store, &addressed).map(|resolved| resolved.api);
            return refusal_response(api, refusal);
        }
    }

    // The request has got past everything the method declares about it, so now — and only
    // now — the STAGE'S CACHE is asked, and then, on a miss, the integration is performed.
    //
    // A cluster holding an entry for this call answers it with the integration never
    // performed: that is what caching a response means. A request that asked for the entry
    // to be reloaded, and that the stage honoured, skips this read, so what it gets is the
    // integration's own fresh answer.
    if let Some(plan) = caching.as_ref().filter(|plan| !plan.reload) {
        let held = {
            let store = state.lock();
            store
                .rest_apis
                .get(&integration.api_id)
                .and_then(|api| api.stages.get(&integration.stage))
                .and_then(|stage| stage.cached_response(&plan.key))
                .cloned()
        };
        if let Some(held) = held {
            return warned(cached_response(held), plan.warn);
        }
    }

    // Every outcome an integration this call cannot be performed through earns is rendered
    // the same way: through the api's own gateway response for the refusal, so the records
    // are read again — briefly, and only to render it.
    let refuse = |unserved: UnservedIntegration| -> RtResponse {
        tracing::warn!(
            api_id = %integration.api_id,
            stage = %integration.stage,
            path = %integration.path,
            verb = %integration.verb,
            reason = %unserved.reason,
            "the stage cannot serve this call through the method's integration"
        );
        let store = state.lock();
        let api = resolve(&store, &addressed).map(|resolved| resolved.api);
        refusal_response(api, unserved.refusal)
    };

    // An integration that PASSES THE REQUEST THROUGH to an endpoint this emulator serves is
    // performed by serving the request it passes on, so the answer is that endpoint's own.
    // It is reached before the api's own records are asked to produce a response, because
    // for such an integration they produce none.
    let passed_on = integration
        .method
        .as_ref()
        .and_then(|method| method.integration.as_ref())
        .and_then(|record| {
            passed_through(record, req, hop, &integration.call.inputs.path, &integration.api_id)
        });
    let served = if let Some(passed_on) = passed_on {
        if let Some(reason) = integration.unusable_connection.clone() {
            return refuse(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason,
            });
        }
        match passed_on {
            PassedThrough::Unserved(unserved) => return refuse(unserved),
            PassedThrough::Request(onward) => {
                // The onward request is a request of its own at another endpoint, so it is
                // answered under its own id, as AWS names each request it processes by one.
                let onward_id = ::service_common::query_error_envelope::new_request_id();
                // A request of its own: what the endpoint it reaches publishes about ITS
                // own authorization belongs to that request, not to this one's record.
                let mut onward_authorized = BTreeMap::new();
                // The VPC ENDPOINT that onward request ARRIVES through, asked for only now
                // that there IS one to serve, and with no lock of this service held because
                // it asks the two services that own those records ([`private_delivery`]):
                // the request is carried over the link's own targets, since "for a private
                // HTTP integration, the URI is not used for routing" (`Integration$uri`),
                // so what it arrives through is what those targets are and not the host the
                // `uri` names. It travels as an ARGUMENT rather than on the request: it is
                // what this plane did with a request it passed on, so no caller can state
                // it.
                let delivered_through = private_delivery::arrival_endpoint(
                    engine,
                    ctx,
                    &integration.region,
                    integration.connection_load_balancers.as_deref(),
                )
                .await;
                serve_resolved(
                    engine,
                    &onward,
                    ctx,
                    hop + 1,
                    &onward_id,
                    &mut onward_authorized,
                    delivered_through.as_deref(),
                )
                .await
            }
            // Another endpoint of this emulator: the request crosses the in-process
            // transport, and what the service claiming that endpoint answered IS the
            // method response.
            PassedThrough::Onward(onward) => match onward_endpoint::answered(ctx, *onward).await {
                Ok(answered) => answered,
                Err(unserved) => return refuse(unserved),
            },
        }
    } else {
        match performed_integration(
            ctx,
            &integration.region,
            integration.method.as_ref(),
            &integration.call,
        )
        .await
        {
            Ok(served) => served_response(served),
            Err(unserved) => return refuse(unserved),
        }
    };

    // What the integration answered is what the stage's cache now holds, for as long as
    // the method's TTL says — which is how the NEXT request to it is answered without the
    // integration being performed again. Only an integration's own answer is held: a
    // refusal the api itself produced returned above, so none of them reaches here.
    if let Some(plan) = &caching {
        if let Some(entry) = cached_entry(&served, plan.ttl) {
            let mut store = state.lock();
            if let Some(stage) = store
                .rest_apis
                .get_mut(&integration.api_id)
                .and_then(|api| api.stages.get_mut(&integration.stage))
            {
                stage.cache_response(plan.key.clone(), entry);
            }
        }
    }
    warned(served, caching.is_some_and(|plan| plan.warn))
}

/// What the request's own records make of it: how a pass of [`serving`]'s resolution ends.
#[derive(Debug)]
enum Planned {
    /// The records answered the request outright — a refusal, or a response served from
    /// them. Boxed: a whole response is far larger than the other variants.
    Answered(Box<RtResponse>),
    /// The resolution needs a `CUSTOM` authorizer's decision, which is a CALL: this is what
    /// making it needs, and the resolution runs again with what it returned.
    Pending(Box<custom_authorizer::Pending>),
    /// The request got past everything the method declares about it: this is what serving it
    /// then needs.
    Ready(Box<Plan>),
}

impl Planned {
    /// [`Planned::Answered`] for a response built in place.
    fn answered(response: RtResponse) -> Self {
        Self::Answered(Box::new(response))
    }
}

/// What a resolved request needs to be SERVED, read under one shared read of the store and
/// used with no lock held.
#[derive(Debug)]
struct Plan {
    /// The integration that is to answer, with the call it answers.
    integration: ResolvedIntegration,
    /// What validating the payload against the method's model needs, when the method
    /// declares a validator that checks it.
    pending_body: Option<PendingBodyCheck>,
    /// The metering this call is subject to, and the `429`s it earns when the plans
    /// metering it have no request left for it.
    metering: Option<Metering>,
    /// How the stage's cache serves this call, when the method's settings cache it.
    caching: Option<CachePlan>,
}

/// Ask the `CUSTOM` authorizer `pending` names, and HOLD what it returned on the stage for
/// as long as the authorizer's own TTL says.
///
/// Made with NO lock held, which is why it is a step of its own: the invocation crosses the
/// in-process transport into Lambda, and nothing else about this service may wait on that.
/// The result is held because that is what `authorizerResultTtlInSeconds` means — the next
/// request presenting the same identity is decided from it, with the function not invoked
/// again — and an authorizer that caches nothing (a TTL of zero) holds nothing.
async fn obtained_decision(
    ctx: &OperationContext<'_>,
    state: &::engine::ServiceState<crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState>,
    pending: custom_authorizer::Pending,
) -> CustomDecision {
    match custom_authorizer::obtained(ctx, &pending).await {
        custom_authorizer::Obtained::Returned(result) => {
            if let Some(key) = pending.cache_key.clone() {
                let mut store = state.lock();
                if let Some(stage) = store
                    .rest_apis
                    .get_mut(&pending.api_id)
                    .and_then(|api| api.stages.get_mut(&pending.stage))
                {
                    stage.cache_authorizer_result(key, result.clone());
                }
            }
            CustomDecision::Returned(result)
        }
        custom_authorizer::Obtained::Unobtained { refusal, reason } => {
            tracing::warn!(
                api_id = %pending.api_id,
                stage = %pending.stage,
                authorizer = %pending.authorizer_name,
                %reason,
                "the method's authorizer produced no decision for this call"
            );
            CustomDecision::Unobtained(refusal)
        }
    }
}

/// What the WAFv2 web ACL in front of the addressed stage made of the request.
#[derive(Debug)]
enum EdgeOutcome {
    /// No web ACL is associated with the stage, or the one that is allowed the request
    /// and inserted nothing into it: the stage serves the request as it arrived.
    Unchanged,
    /// The web ACL allowed the request and a `CustomRequestHandling` on the way
    /// inserted these headers, which the origin is to see.
    Allowed(Vec<(String, String)>),
    /// The web ACL refused the request at the edge: this is the response to serve, and
    /// nothing of the api is reached.
    ///
    /// Boxed: a whole response is far larger than the other two variants, which is what
    /// the sibling [`PassedThrough`] boxes a request for.
    Refused(Box<RtResponse>),
}

/// Apply the WAFv2 web ACL in front of the stage this request addresses, and answer what
/// it decided beside what it LOGS about the request.
///
/// A web ACL is bound to a stage by `AssociateWebACL` over the stage's own ARN, so the
/// stage is resolved from THIS service's records, its ARN is built the one way this
/// service builds it ([`crate::state::tagged_resource_arn`]), and the decision itself
/// is WAFv2's ([`waf_edge::decide`]) — this module holds no second reading of a rule
/// tree. A request addressing no stage of this account resolves no ARN and so no web
/// ACL: it is refused by the api's own records further down.
///
/// The record is WAFv2's too: this plane only DELIVERS it, because the write is a call
/// made from the operation serving the request. `None` when the web ACL logs nothing
/// about this request — a web ACL with no logging configuration, or a configuration whose
/// filter drops the record.
///
/// The two stores are locked one after the other and never at the same time, and the
/// wafv2 side locks nothing of this service's, so the two cannot deadlock.
fn web_acl_decision<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState>,
    req: &RtRequest,
) -> (EdgeOutcome, Option<waf_logs::LoggedRecord>) {
    let addressed = Addressed::of_request(req);
    // The stage's ARN, and the invoke path WITHIN the api — which is the path AWS's
    // edge inspects, `/{stage}/{resource-path}`, whichever addressing the client
    // reached this emulator by.
    let (stage_arn, invoke_path) = {
        let store = state.lock();
        let Some(resolved) = resolve(&store, &addressed) else {
            return (EdgeOutcome::Unchanged, None);
        };
        let Some((stage_name, _)) = split_stage(resolved.invoke_path) else {
            return (EdgeOutcome::Unchanged, None);
        };
        if !resolved.api.stages.contains_key(stage_name) {
            return (EdgeOutcome::Unchanged, None);
        }
        (
            crate::state::stage_arn(
                // The Region comes from the api's own record, because the ARN the web ACL
                // was bound by is the stage's one ARN: `AssociateWebACL` is given the ARN
                // of a stage created in a Region, while a client invoking that stage
                // reaches this emulator at a host that need not spell any Region, so a
                // Region read off the request would look up a key nothing ever stored.
                resolved.api.arn_region(),
                &resolved.api.id,
                stage_name,
            ),
            resolved.invoke_path.to_string(),
        )
    };

    let inspected = inspected_request(req, &invoke_path);
    // The decision, and the lock discipline it needs, are wafv2's own: `decide` reads
    // the records under a shared lock, evaluates the web ACL with the lock RELEASED,
    // writes the data plane's own bookkeeping under an exclusive one, and publishes the
    // `AWS/WAFV2` counters of the dimensions that acted once that lock is gone. It is
    // given the shared engine because reporting what the web ACL did spans two stores,
    // and it answers `None` when the wafv2 service is not mounted in this process at
    // all, so no binding can exist and the stage serves the request unchanged. This
    // service's own store is not held across the call, and the wafv2 side locks nothing
    // of this service's, so the two cannot deadlock.
    let Some(decision) = waf_edge::decide(engine, &stage_arn, &inspected) else {
        // No web ACL in front of this stage.
        return (EdgeOutcome::Unchanged, None);
    };
    let logged = decision.log_record.clone();
    if decision.action.is_allow() {
        if decision.inserted_headers.is_empty() {
            return (EdgeOutcome::Unchanged, logged);
        }
        return (EdgeOutcome::Allowed(decision.inserted_headers), logged);
    }
    tracing::info!(
        stage_arn = %stage_arn,
        rule = decision.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this stage does not let the request through"
    );
    // A terminating action that configures its own `CustomResponse` is served BY WAF:
    // the status, the headers and the body are the web ACL's own, and the api's gateway
    // responses are not consulted, because API Gateway never produced the response.
    if let Some(custom) = decision.custom_response {
        return (
            EdgeOutcome::Refused(Box::new(edge_response(custom))),
            logged,
        );
    }
    let refused = match decision.action {
        waf_edge::EdgeAction::Block => {
            // Answered through the api's own `WAF_FILTERED` gateway response, so a
            // developer's customization of that type reaches the blocked caller.
            let store = state.lock();
            let api = resolve(&store, &addressed).map(|resolved| resolved.api);
            EdgeOutcome::Refused(Box::new(refusal_response(api, WAF_FILTERED)))
        }
        // A `Captcha` or `Challenge` action with no custom response is answered with
        // the status WAF serves its interstitial under. The interstitial ITSELF is a
        // script WAF serves from its own endpoint, which this emulator serves none of,
        // so the response carries the status and no body rather than a page nothing
        // produced.
        action => EdgeOutcome::Refused(Box::new(response_with(
            action.default_status().unwrap_or(403),
            JSON_CONTENT_TYPE,
            Vec::new(),
        ))),
    };
    (refused, logged)
}

/// The request as WAFv2 inspects it: every component a `FieldToMatch` can name.
///
/// `invoke_path` is the path WITHIN the api rather than the path this request arrived
/// on, because that is the path a client addressing AWS would have sent and so the one
/// a web ACL's criteria are written against; the emulator's own addressing prefix is
/// how the request reached this process, not part of the request the stage serves.
fn inspected_request(req: &RtRequest, invoke_path: &str) -> waf_edge::InspectedRequest {
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    // The address the request came from. A request served in process crossed no socket
    // from a remote peer, so there is no connection address; one that traversed a proxy
    // still states where it came from, in its forwarded-for header.
    let client_ip = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(waf_edge::FORWARDED_FOR_HEADER))
        .and_then(|(_, value)| value.split(',').next())
        .map(str::trim)
        .filter(|address| address.parse::<std::net::IpAddr>().is_ok())
        .unwrap_or(waf_edge::IN_PROCESS_CLIENT_IP)
        .to_string();
    waf_edge::InspectedRequest {
        method: req.method().to_string(),
        uri_path: invoke_path.to_string(),
        query_string: query_of(req.uri()).to_string(),
        headers,
        body: request_payload(req),
        client_ip,
    }
}

/// `req` with `inserted` added to its headers, or `None` when the request cannot be
/// rebuilt at all.
///
/// Every other part travels unchanged — the method, the URI and the body — because the
/// only thing a `CustomRequestHandling` changes about a request is which headers it
/// carries. An inserted header REPLACES one the client sent under the same name: the
/// prefix WAF gives it is there so the origin can trust it came from WAF.
fn request_with_headers(req: &RtRequest, inserted: &[(String, String)]) -> Option<RtRequest> {
    let mut rebuilt = RtRequest::new(SdkBody::from(request_payload(req)));
    rebuilt.set_uri(req.uri()).ok()?;
    rebuilt.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        if let Err(refused) = rebuilt
            .headers_mut()
            .try_append(name.to_string(), value.to_string())
        {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the caller's header cannot be carried onto the request the web acl \
                 allowed, and is left off it"
            );
        }
    }
    for (name, value) in inserted {
        if let Err(refused) = rebuilt.headers_mut().try_insert(name.clone(), value.clone()) {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the header the web acl's custom request handling inserts is not one a \
                 header can carry, and is left off the request"
            );
        }
    }
    Some(rebuilt)
}

/// The response a web ACL's own `CustomResponse` serves, as it leaves the edge.
fn edge_response(custom: waf_edge::EdgeResponse) -> RtResponse {
    let mut response = response_with(
        custom.status,
        custom
            .content_type
            .as_deref()
            .unwrap_or(JSON_CONTENT_TYPE),
        custom.body,
    );
    for (name, value) in custom.headers {
        if let Err(refused) = response.headers_mut().try_insert(name.clone(), value) {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the header the web acl's custom response configures is not one a header \
                 can carry, and is left off the response"
            );
        }
    }
    response
}

/// The integration a resolved call is to be performed through, carried OUT of the store
/// lock so that it is performed only once the request has got past the method's request
/// validator and the usage plans metering it.
///
/// Owned rather than borrowed for the same reason [`PendingBodyCheck`] is: both run with
/// the lock released, and the integration is performed after the payload check, so what it
/// reads has to outlive the lock. The copy is of ONE method's own records — what a
/// control-plane call had to store for it — so it is bounded by the method's configuration
/// rather than by anything the invoke carries.
#[derive(Debug)]
struct ResolvedIntegration {
    /// The api the call resolved to, for the server log an integration this emulator
    /// cannot perform leaves.
    api_id: String,
    /// The stage it was invoked through, for that same log.
    stage: String,
    /// The resource path the request matched, for that same log.
    path: String,
    /// The verb the method is addressed by, for that same log.
    verb: String,
    /// The api's OWN Region, read off its ARN: the Region an integration naming another
    /// service's action is made at when its own ARN names none ([`aws_integration`]).
    region: String,
    /// The LIVE method the deployed one names, which carries the integration; `None` when
    /// the deployment holds a method the api no longer carries, which is an integration
    /// there is nothing to call.
    method: Option<MethodEntry>,
    /// Why the private connection the integration is carried over cannot carry this call,
    /// when it names one and it cannot ([`unusable_vpc_link`]).
    unusable_connection: Option<String>,
    /// The load balancers that private connection targets, when it names a VPC link this
    /// account holds: what decides which VPC ENDPOINT the request it passes on arrives
    /// through ([`private_delivery`]).
    connection_load_balancers: Option<Vec<String>>,
    /// What the integration reads off this method request.
    call: MethodRequest,
}

/// The metering a call is subject to, and the refusal each limit of the plans metering it
/// earns when that limit has nothing left.
#[derive(Debug)]
struct Metering {
    /// Which key is metered, and by which plans.
    admission: Metered,
    /// The `429` for a plan whose QUOTA has no request left in this metering period.
    quota_exceeded: RtResponse,
    /// The `429` for a plan whose THROTTLE has no request left at this instant.
    throttled: RtResponse,
}

/// Which limit of a usage plan shed a request.
#[derive(Debug, Clone, Copy)]
enum Shed {
    /// A plan's `quota` — "the target maximum number of requests that can be made in a
    /// given time period" — has admitted that many already in this period.
    Quota,
    /// A plan's `throttle` admits no further request at this instant.
    Throttle,
}

/// Record this request against every usage plan that meters the presented API key, and
/// report which limit sheds it, if one does.
///
/// A usage plan carries two limits and both are the plan's FOR EACH of its keys, so both
/// are accounted per `(plan, key)` pair: the `quota` is the number of requests a key may
/// send in the current metering period
/// ([`crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState::quota_admits`]) and
/// the `throttle` the rate it may send them at
/// ([`crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState::throttle_admits`]).
/// A key metered by two plans has to satisfy both plans' limits.
///
/// The quota is accounted first, because it is the one that decides whether the request
/// COUNTS: a request past the period's limit is not admitted and is not counted against it,
/// while a request the quota admitted is counted even if a plan's throttle then sheds it —
/// a shed request is one the key SENT. Every plan is accounted for the same reason, even
/// when an earlier one sheds the request.
fn metered_refusal(
    store: &mut crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    admission: &Metered,
) -> Option<Shed> {
    let now = ::std::time::Instant::now();
    let day = crate::state::usage_day_of(SystemTime::now());
    let mut shed = None;
    for (plan_id, limits) in &admission.plans {
        if !store.quota_admits(plan_id, &admission.key_id, day) {
            shed = Some(Shed::Quota);
            // The period has nothing left for this key, so this plan's throttle is not
            // asked: the request it would have admitted is one the quota already refused.
            continue;
        }
        if let Some(limits) = limits {
            if !store.throttle_admits(plan_id, &admission.key_id, *limits, now) {
                shed = shed.or(Some(Shed::Throttle));
            }
        }
    }
    shed
}

/// What the method's `authorizationType` makes of this request: the claims the authorizer
/// publishes to the integration, or the refusal the request earns.
///
/// `NONE` admits every request and publishes nothing. `AWS_IAM` needs a SigV4 credential, so
/// only its PRESENCE is read here; whether that credential AUTHENTICATES — the access key id
/// naming a credential this emulator holds and the signature matching its secret — is
/// [`sigv4_authentication`]'s decision, and WHETHER the principal it speaks for may invoke
/// the method is [`execute_api_authorization`]'s, both applied by [`serving`] once the method
/// is known.
///
/// An `Authorizer`-backed type is read in two steps. The part settled before the authorizer
/// is called at all is the identity source: a source the request does not carry is `401`
/// without the authorizer running (`Authorizer$identitySource`). Then the DECISION:
///
/// * `COGNITO_USER_POOLS` is decided in full, because it is a function of the token, of the
///   pools the authorizer's `providerARNs` name and of the method's own
///   `authorizationScopes` — all of which are records this emulator holds
///   ([`cognito_user_pools`]) — and the claims of an accepted token are what the
///   integration sees as `$context.authorizer.claims`; a token it does not accept earns the
///   refusal of the CLASS that failure belongs to ([`cognito_refusal`]);
/// * `CUSTOM` is a Lambda function's RETURN VALUE ([`custom_authorizer`]): the result the
///   stage is already holding for this identity decides the call, and otherwise the function
///   has to be asked — which is a call this cannot make, so it answers
///   [`Authorized::Pending`] and [`serving`] asks before deciding.
///
/// The api's own RESOURCE policy is a separate decision, taken on every request whatever the
/// authorization type — [`super::resource_policy`], applied by [`serving`] straight after
/// this.
fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    authorization_type: &str,
    req: &RtRequest,
    custom: CustomAuthorization<'_>,
) -> Authorized {
    if authorization_type == AUTHORIZATION_NONE {
        return Authorized::Published(BTreeMap::new());
    }
    if authorization_type == AUTHORIZATION_AWS_IAM {
        return if carries_sigv4_credential(req) {
            Authorized::Published(BTreeMap::new())
        } else {
            Authorized::Refused(MISSING_AUTHENTICATION_TOKEN)
        };
    }
    if !AUTHORIZATION_BY_AUTHORIZER.contains(&authorization_type) {
        // A value outside the documented set is a method this emulator cannot authorize.
        return Authorized::Refused(API_CONFIGURATION_ERROR);
    }
    // A method whose `authorizerId` names no authorizer of this API cannot authorize the
    // call at all, which is a configuration the stage cannot serve.
    let Some(authorizer) = method
        .and_then(|method| method.authorizer_id.as_deref())
        .and_then(|id| api.authorizers.get(id))
    else {
        return Authorized::Refused(API_CONFIGURATION_ERROR);
    };
    // The value the authorizer EXTRACTS from the request, which is what it then evaluates:
    // the first identity source that names a part of the request, in the order the authorizer
    // declares them, with every one of them keying the result the stage holds. The
    // declaration is read by the one reader of it ([`identity_source`]), so a request to this
    // stage and a SIMULATED request are asked about the same sources.
    let presented = match identity_source::presented(
        authorizer.identity_source.as_deref(),
        |header| {
            req.headers()
                .get(header.to_ascii_lowercase())
                .map(str::to_string)
        },
        |parameter| {
            query_of(req.uri()).split('&').find_map(|pair| {
                pair.split_once('=')
                    .filter(|(key, value)| *key == parameter && !value.is_empty())
                    .map(|(_, value)| value.to_string())
            })
        },
    ) {
        Ok(presented) => presented,
        // Each identity source the authorizer declares must be PRESENT, or the authorizer is
        // not called at all: a request carrying none of them authorized nobody.
        Err(_absent) => return Authorized::Refused(UNAUTHORIZED),
    };
    let identity_values = presented.values;
    let presented = presented.first;
    if authorization_type == AUTHORIZATION_COGNITO_USER_POOLS
        && authorizer.r#type == crate::ops::create_authorizer::AUTHORIZER_TYPE_COGNITO_USER_POOLS
    {
        // No value extracted at all is an authorizer that was not called: the identity source
        // it declares is one this emulator could not read, or it declares none.
        let Some(presented) = presented else {
            return Authorized::Refused(UNAUTHORIZED);
        };
        return match cognito_user_pools::evaluate(
            engine,
            authorizer,
            // The METHOD's own `authorizationScopes`, which "are used with a
            // `COGNITO_USER_POOLS` authorizer to authorize the method invocation"
            // (`Method$authorizationScopes`) — read off the live method, since
            // `Deployment$apiSummary`'s `MethodSnapshot` models them not at all.
            method.map_or(&[][..], |method| method.authorization_scopes.as_slice()),
            &presented,
            SystemTime::now(),
        ) {
            // The claims of an accepted token, each under the `$context` member path a
            // mapping template reads one by.
            cognito_user_pools::Decision::Authorized(authorized) => Authorized::Published(
                authorized
                    .claims
                    .into_iter()
                    .map(|(claim, value)| (format!("{AUTHORIZER_CLAIMS_MEMBER}.{claim}"), value))
                    .collect(),
            ),
            cognito_user_pools::Decision::Refused(refusal, reason) => {
                tracing::warn!(
                    authorizer = %authorizer.name,
                    %reason,
                    "refusing an invoke: the user-pool authorizer does not accept the presented \
                     token"
                );
                Authorized::Refused(cognito_refusal(refusal))
            }
        };
    }
    // A `CUSTOM` authorizer of either Lambda type: the decision is its function's return
    // value. A `TOKEN` authorizer carries no request beyond its token, so a call presenting
    // none was not authorized at all; a `REQUEST` authorizer declaring no identity source is
    // called for every request, which is what makes its caching turn off
    // (`crate::ops::create_authorizer` holds that rule).
    let token = authorizer.r#type == crate::ops::create_authorizer::AUTHORIZER_TYPE_TOKEN;
    if token && presented.is_none() {
        return Authorized::Refused(UNAUTHORIZED);
    }
    // Where this stage holds a result for this identity, which is both where one is looked
    // for and where the one the caller obtains is stored.
    let cache_key = authorizer_cache_key(authorizer, &identity_values);
    // What the authorizer returned for THIS identity, from the stage or from the invocation
    // the caller has just made on this module's behalf.
    let held = match custom.decided {
        Some(CustomDecision::Returned(held)) => Cow::Borrowed(held),
        // The function was asked and produced no result: the request is answered through the
        // gateway response that outcome earns, never by guessing at a decision.
        Some(CustomDecision::Unobtained(refusal)) => return Authorized::Refused(refusal.clone()),
        None => match cache_key
            .as_deref()
            .and_then(|key| custom.stage.cached_authorizer_result(key))
        {
            Some(held) => Cow::Borrowed(held),
            // Nothing held for this identity: the function has to be asked, which the caller
            // does off the store lock before deciding this request.
            None => {
                let event = if token {
                    custom_authorizer::token_event(
                        presented.as_deref().unwrap_or_default(),
                        custom.method_arn,
                    )
                } else {
                    lambda_proxy::request_authorizer_event(custom.call, custom.method_arn)
                };
                return Authorized::Pending(Box::new(custom_authorizer::Pending {
                    api_id: api.id.clone(),
                    stage: custom.stage.stage_name.clone(),
                    region: api.arn_region().to_string(),
                    authorizer_name: authorizer.name.clone(),
                    authorizer_uri: authorizer.authorizer_uri.clone().unwrap_or_default(),
                    authorizer_arn: authorizer_arn(api, &authorizer.id),
                    authorizer_credentials: authorizer.authorizer_credentials.clone(),
                    event,
                    // The trace context this stage propagates onto the authorizer's own
                    // invocation: the SAME chain and decision the integration's call carries
                    // ([`trace_context`]), decided once for this request when the method
                    // request was built, under the stage's own span — an authorizer's function
                    // is called by the stage itself rather than through the integration's
                    // call out.
                    trace_header: custom.call.trace.authorizer_header.clone(),
                    cache_key,
                    ttl: authorizer_result_ttl(authorizer),
                }));
            }
        },
    };
    match custom_authorizer::decided(&held, custom.method_arn) {
        custom_authorizer::Judged::Published(published) => Authorized::Published(published),
        custom_authorizer::Judged::Refused(sentence) => {
            tracing::warn!(
                authorizer = %authorizer.name,
                kind = %authorizer.r#type,
                "refusing an invoke: the authorizer's policy does not allow this method"
            );
            Authorized::Refused(access_denied(sentence.to_string()))
        }
    }
}

/// The gateway response a `COGNITO_USER_POOLS` authorizer's refusal of a presented token is
/// answered through — the class of the failure ([`cognito_user_pools::Refusal`]) decides it.
///
/// A request this authorizer does not admit authorized nobody, so [`UNAUTHORIZED`] is what
/// almost every one of its refusals is answered through — a value that is not a signed token,
/// an issuer the authorizer does not list, an audience its validation expression does not
/// match, a method scope the token does not claim. Two classes this endpoint words differently:
/// a token whose own `exp` has elapsed carries the sentence of an expired token at that same
/// `401` ([`EXPIRED_IDENTITY_TOKEN`]), and a token claiming a pool the authorizer TRUSTS that
/// does not verify against that pool's published key set is refused as the authorizer DENYING
/// the call — the same `403` [`access_denied`] a `CUSTOM` authorizer's refusal earns, carrying
/// the same sentence a call the authorizer allows no statement for is refused with
/// ([`custom_authorizer::NOT_AUTHORIZED_MESSAGE`]), so a developer's customization of an
/// authorization failure (`ACCESS_DENIED`) reaches both kinds of authorizer.
fn cognito_refusal(refusal: cognito_user_pools::Refusal) -> GatewayRefusal {
    match refusal {
        cognito_user_pools::Refusal::Unauthorized => UNAUTHORIZED,
        cognito_user_pools::Refusal::Expired => EXPIRED_IDENTITY_TOKEN,
        cognito_user_pools::Refusal::Unverified => {
            access_denied(custom_authorizer::NOT_AUTHORIZED_MESSAGE.to_string())
        }
    }
}

/// What a `CUSTOM` authorizer's decision needs that the authorization step cannot read off
/// the method alone: where a held result lives, what the call is, and what has already been
/// obtained for it.
#[derive(Debug)]
struct CustomAuthorization<'a> {
    /// The stage the request reached, which is what HOLDS an authorizer result.
    stage: &'a StageEntry,
    /// The ARN of the method the request addressed, which the returned policy is evaluated
    /// against and which the authorizer's event names.
    method_arn: &'a str,
    /// The call, for the event a `REQUEST` authorizer is handed.
    call: &'a MethodRequest,
    /// What the caller has already obtained from the authorizer for this request, on a
    /// second pass through the resolution; `None` on the first.
    decided: Option<&'a CustomDecision>,
}

/// What asking a `CUSTOM` authorizer produced, as [`serving`] holds it between the two
/// passes of the resolution.
#[derive(Debug)]
enum CustomDecision {
    /// The authorizer's own return value, which decides this call and every later one the
    /// stage serves from the held result.
    Returned(crate::state::CachedAuthorizerResult),
    /// No result at all: this is the gateway response the request is answered through.
    Unobtained(GatewayRefusal),
}

/// What the authorization step made of a request.
#[derive(Debug)]
enum Authorized {
    /// The request is authorized, and these are the `$context` members the method's
    /// authorization published for the integration — empty for a type that publishes none.
    Published(BTreeMap<String, String>),
    /// The request is refused, through this gateway response.
    Refused(GatewayRefusal),
    /// The request is authorized by a `CUSTOM` authorizer whose result nothing holds, so its
    /// function has to be asked: this is what asking it needs. Boxed because it is far
    /// larger than the other two variants.
    Pending(Box<custom_authorizer::Pending>),
}

/// The ARN of one `Authorizer` of `api`:
/// `arn:aws:execute-api:{region}:{account}:{api-id}/authorizers/{authorizer-id}`.
///
/// The `aws:SourceArn` a `lambda:InvokeFunction` grant to `apigateway.amazonaws.com` for an
/// authorizer's function is narrowed by — the resource an `AddPermission` for an authorizer
/// names, as the invoked METHOD's own ARN is for an integration's function
/// ([`resource_policy::Call::method_arn`] builds that one the same way).
fn authorizer_arn(api: &RestApiEntry, authorizer_id: &str) -> String {
    ::emulator_server::emulator_arn(
        "execute-api",
        api.arn_region(),
        &format!("{api_id}/authorizers/{authorizer_id}", api_id = api.id),
    )
}

/// How long a result this authorizer returns stays usable:
/// "`authorizerResultTtlInSeconds`: the TTL in seconds of cached authorizer results. If it
/// equals 0, authorization caching is disabled." A stored value below zero is no duration at
/// all, which is the same no-caching answer.
fn authorizer_result_ttl(authorizer: &crate::state::AuthorizerEntry) -> ::std::time::Duration {
    ::std::time::Duration::from_secs(
        u64::try_from(authorizer.authorizer_result_ttl_in_seconds).unwrap_or_default(),
    )
}

/// Where the stage holds the result this authorizer returned for the identity
/// `identity_values`, or `None` when it holds none at all — an authorizer whose TTL is zero
/// ("if it equals 0, authorization caching is disabled") and one the request presented no
/// identity value to.
fn authorizer_cache_key(
    authorizer: &crate::state::AuthorizerEntry,
    identity_values: &[String],
) -> Option<String> {
    if authorizer.authorizer_result_ttl_in_seconds <= 0 || identity_values.is_empty() {
        return None;
    }
    Some(format!(
        "{id}|{identity}",
        id = authorizer.id,
        identity = identity_values.join("|")
    ))
}

/// What the records say about the API key a request presented: which stored key admitted
/// the call, and the usage plans that METER it for this stage.
///
/// The key id rather than the key's value, because the value is the client's credential
/// and the accounting is per stored key; the plans are the ones whose throttle declares a
/// limit at all, so a plan that limits nothing adds no accounting.
#[derive(Debug)]
struct Metered {
    /// The stored `ApiKey`'s id.
    key_id: String,
    /// The `(usage plan id, throttle limits)` pairs this call is metered against, the
    /// limits absent for a plan whose `throttle` declares none. EVERY plan that meters the
    /// key for this stage is here, whether or not it throttles: a plan's quota counts the
    /// requests its keys send whatever rate they send them at.
    plans: Vec<(String, Option<crate::state::ThrottleLimits>)>,
}

/// The admission the API key a request presents earns at `stage`, or `None` when the
/// records associate no such key with it.
///
/// The key travels in [`API_KEY_HEADER`] when the API's `apiKeySource` is `HEADER`; the
/// other documented source reads it from a custom authorizer's returned context, which is
/// a decision this emulator cannot obtain, so no key is presented for it.
///
/// A key admits the call when it is ENABLED and this api's `stage` is among the stages the
/// records associate it with — see [`plans_metering`] and `ApiKey$stageKeys` for the two
/// associations that say so. A value no stored key carries, a disabled key and a key
/// associated with some other stage are each a key that does not admit the call.
fn api_key_admission(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    api: &RestApiEntry,
    stage: &str,
    req: &RtRequest,
) -> Option<Metered> {
    let source = api
        .api_key_source
        .as_deref()
        .unwrap_or(crate::state::DEFAULT_API_KEY_SOURCE);
    if source != API_KEY_SOURCE_HEADER {
        return None;
    }
    let presented = req.headers().get(API_KEY_HEADER).filter(|key| !key.is_empty())?;
    store
        .api_keys
        .values()
        .filter(|key| key.value == presented && key.enabled)
        .find_map(|key| {
            let associating = plans_associating(store, key, api, stage);
            // The key's own `stageKeys` is the OLDER association, which
            // `CreateApiKey$stageKeys` documents as "DEPRECATED FOR USAGE PLANS -
            // Specifies stages associated with the API key". It is honoured beside the
            // plan because the operations that write it still accept it, and a deprecated
            // association is still an association — but it names no plan, so it carries no
            // limits to meter against.
            let by_stage_key = key.stage_keys.contains(&format!("{}/{stage}", api.id));
            (!associating.is_empty() || by_stage_key).then(|| Metered {
                key_id: key.id.clone(),
                // Every associating plan, with the throttle limits it declares: a plan
                // that declares none still meters the request against its quota, and a
                // plan with neither limit admits every request it associates.
                plans: associating,
            })
        })
}

/// The usage plans that associate `key` with `stage` of `api`, each with the limits its
/// `throttle` declares — `None` for a plan that declares none.
///
/// A `UsagePlan` is "used to specify who can access associated API stages", its
/// `apiStages` name the api and stage its limits apply to, and a `UsagePlanKey` is the
/// association of one stored API key with one plan. So a client creates the key, a plan
/// over the api stage, and a plan key — and that key is what the stage admits, metered by
/// that plan.
fn plans_associating(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    key: &ApiKeyEntry,
    api: &RestApiEntry,
    stage: &str,
) -> Vec<(String, Option<crate::state::ThrottleLimits>)> {
    store
        .usage_plans
        .values()
        .filter(|plan| {
            plan.keys.contains_key(&key.id)
                && plan.api_stages.iter().any(|api_stage| {
                    api_stage.api_id.as_deref() == Some(api.id.as_str())
                        && api_stage.stage.as_deref() == Some(stage)
                })
        })
        .map(|plan| {
            (
                plan.id.clone(),
                crate::state::ThrottleLimits::declared(plan.throttle.as_ref()),
            )
        })
        .collect()
}

/// Where a request parameter a method declares travels on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParameterIn {
    /// The query string, in either its single- or multi-value spelling.
    Query,
    /// A request header, in either spelling.
    Header,
    /// A path segment of the resource the method sits on.
    Path,
}

/// Where `expression` says a request parameter travels and under what NAME, or `None` for
/// a key that is no `method.request.{location}.{name}` mapping expression at all.
fn parameter_source(expression: &str) -> Option<(ParameterIn, &str)> {
    // The multi-value spellings are tried first: each is a longer prefix than the
    // single-value one it extends, and both name the same parameter.
    [
        (METHOD_REQUEST_MULTI_QUERY_PREFIX, ParameterIn::Query),
        (METHOD_REQUEST_QUERY_PREFIX, ParameterIn::Query),
        (METHOD_REQUEST_MULTI_HEADER_PREFIX, ParameterIn::Header),
        (METHOD_REQUEST_HEADER_PREFIX, ParameterIn::Header),
        (METHOD_REQUEST_PATH_PREFIX, ParameterIn::Path),
    ]
    .into_iter()
    .find_map(|(prefix, source)| {
        expression
            .strip_prefix(prefix)
            .filter(|name| !name.is_empty())
            .map(|name| (source, name))
    })
}

/// What an integration reads off the METHOD REQUEST it serves: the media type the request
/// declares, which selects the integration REQUEST template, and the media type the caller
/// accepts, which selects the integration RESPONSE template.
///
/// Both callers of [`perform_integration`] build one — the data plane off the wire
/// ([`Self::of_request`]), `TestInvokeMethod` out of the members a caller simulates a
/// request with ([`Self::simulated`]) — so an invocation that reached a socket and one that
/// never did select the same templates from the same records.
#[derive(Debug)]
pub struct MethodRequest {
    /// The media type the request declares, as a `requestTemplates` key is compared.
    media_type: String,
    /// The first media type of the request's `Accept`, or `None` when it accepts anything.
    accepted: Option<String>,
    /// What a MAPPING TEMPLATE this call selects is rendered against: the method request's
    /// own payload, parameters and context, and the stage's variables
    /// ([`mapping_template`]).
    inputs: TemplateInputs,
    /// What a PROXY integration's event carries about the call beyond what a template reads
    /// off it ([`lambda_proxy::ProxyInputs`]).
    proxy: lambda_proxy::ProxyInputs,
    /// The X-Ray trace context this request propagates onto EVERY call this stage makes on
    /// its behalf, and what the stage records about it ([`trace_context`]). Decided ONCE per
    /// request, here, so an authorizer's function and the integration's belong to one trace.
    trace: trace_context::Propagated,
}

impl MethodRequest {
    /// What a request off the wire declares and accepts, what its templates render
    /// against, and what a proxy integration's event names it by.
    ///
    /// `carried` is this REQUEST's trace context: decided here on the first pass of the
    /// resolution and reused on the second, because a request whose `CUSTOM` authorizer has to
    /// be asked is planned twice and both passes belong to one trace ([`trace_context`]).
    fn of_request(
        req: &RtRequest,
        inputs: TemplateInputs,
        proxy: lambda_proxy::ProxyInputs,
        carried: &mut Option<trace_context::Propagated>,
    ) -> Self {
        let trace = carried
            .get_or_insert_with(|| trace_context::propagated(&proxy, SystemTime::now()))
            .clone();
        Self {
            media_type: declared_media_type(req.headers().get("content-type")),
            accepted: first_media_type(req.headers().get("accept")),
            inputs,
            trace,
            proxy,
        }
    }

    /// What a SIMULATED request declares and accepts, from the two header values a caller
    /// supplied as members rather than on the wire, and what its templates render against.
    pub fn simulated(
        content_type: Option<&str>,
        accept: Option<&str>,
        inputs: TemplateInputs,
        proxy: lambda_proxy::ProxyInputs,
    ) -> Self {
        Self {
            media_type: declared_media_type(content_type),
            accepted: first_media_type(accept),
            inputs,
            trace: trace_context::propagated(&proxy, SystemTime::now()),
            proxy,
        }
    }

    /// The media type this request declares; named in the sentence a refusal of an
    /// unmapped content type carries.
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// The PAYLOAD this call carries, or `None` when it carries none a reader can see —
    /// what the method's request validator validates against the model's schema
    /// ([`PendingBodyCheck::refusal`]).
    pub fn payload(&self) -> Option<&[u8]> {
        self.inputs.payload.as_deref()
    }
}

/// The response an integration this emulator CAN perform produces, every part of it
/// computed from the API's own records.
#[derive(Debug)]
pub struct IntegratedResponse {
    /// The status code of the integration response the call mapped to.
    pub status: u16,
    /// The media type the rendered body is keyed by.
    pub content_type: String,
    /// The rendered body, empty when no response template applies.
    pub body: Vec<u8>,
    /// The response headers, keyed by the header name as the integration declares it, each
    /// with EVERY value that name carries. One name carries several values where the
    /// backend sent several (a proxy integration's `multiValueHeaders`), and exactly one
    /// where a mapped header declares a single value. `content-type` is NOT among them: it
    /// is [`Self::content_type`].
    pub headers: BTreeMap<String, Vec<String>>,
}

/// An integration this call cannot be performed through: the GATEWAY RESPONSE it is
/// answered by, and why in one sentence — for the server log, and for the execution log
/// `TestInvokeMethod` reports to its caller.
#[derive(Debug)]
pub struct UnservedIntegration {
    /// The refusal the call earns; see [`GatewayRefusal`].
    pub refusal: GatewayRefusal,
    /// Why the integration could not be performed. States the configuration, never a
    /// backend answer, because there is no backend answer.
    pub reason: String,
}

/// The method's own records with every STAGE VARIABLE the integration's configuration
/// names replaced by the value `variables` gives it, or the integration this call cannot
/// be performed through because no such variable is defined.
///
/// A stage variable is one entry of "a map that defines the stage variables for a Stage
/// resource" (`Stage$variables`), and a reference to one
/// ([`STAGE_VARIABLE_REFERENCE_PREFIX`]) stands where the integration's configuration
/// carries a value a stage may differ in: the endpoint the integration addresses (`uri`),
/// the VPC link a private connection runs over (`connectionId`), and the role the call is
/// made under (`credentials`). The map is THE STAGE'S, so the reference resolves per
/// stage: two stages of one deployment address two different backends, which is what a
/// configuration written this way is for. It is resolved here, once, before anything
/// reads the configuration, so no reader downstream has to know a variable was involved.
///
/// A name the map holds no entry for resolves to nothing: the configuration has a hole in
/// it, so the integration cannot be performed, rather than being addressed at whatever
/// text is left behind.
///
/// `requestParameters` is NOT substituted. Its value is modelled as "a method request
/// parameter value or static value that must be enclosed within single quotes", the
/// former matching "the pattern of `method.request.{location}.{name}`"
/// (`Integration$requestParameters`), so a stage variable is not one of the forms that
/// member takes — and a value outside them is already stated as a mapping this emulator
/// does not read ([`mapped_request_parameter`]). `requestTemplates` and
/// `responseTemplates` are not substituted here either: a template is RENDERED against the
/// call when it is applied ([`rendered_mapping_template`]), and the stage's variables are
/// among the values it is rendered from, so rewriting the stored text first would resolve
/// them twice.
pub fn with_stage_variables(
    method: &MethodEntry,
    variables: &BTreeMap<String, String>,
) -> Result<MethodEntry, UnservedIntegration> {
    let mut resolved = method.clone();
    let Some(integration) = resolved.integration.as_mut() else {
        return Ok(resolved);
    };
    // The three members of `Integration` whose value a stage may differ in, each named as
    // the model names it: the reason the integration cannot be performed has to say which
    // part of the configuration has the hole.
    let configured: [(&str, &mut Option<String>); 3] = [
        ("uri", &mut integration.uri),
        ("connectionId", &mut integration.connection_id),
        ("credentials", &mut integration.credentials),
    ];
    for (member, value) in configured {
        let Some(carried) = value.as_deref() else {
            continue;
        };
        match resolved_references(carried, variables) {
            // The value names no stage variable at all, so it is the configuration
            // itself: left exactly as it was stored.
            Ok(None) => {}
            Ok(Some(substituted)) => *value = Some(substituted),
            Err(name) => {
                return Err(UnservedIntegration {
                    refusal: API_CONFIGURATION_ERROR,
                    reason: format!(
                        "the integration's {member} names the stage variable {name}, which \
                         this stage defines no value for"
                    ),
                });
            }
        }
    }
    Ok(resolved)
}

/// `value` with every stage-variable reference in it replaced by the value `variables`
/// gives that name — `None` when it carries no reference at all, so an untouched value is
/// never rewritten — or `Err(name)` for the FIRST reference the map holds no entry for.
///
/// Text that opens a reference and never closes it is not one: it stands as the
/// configuration carries it, and whatever reads the value decides what to make of it (a
/// `uri` still carrying a brace is an address this emulator will not guess at —
/// [`passed_through`]).
///
/// A substituted value is NOT rescanned: scanning continues after the reference it
/// replaced, so a variable whose own value spells a reference cannot expand again, and the
/// result is bounded by the stored configuration and one value per reference in it.
fn resolved_references(
    value: &str,
    variables: &BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    if !value.contains(STAGE_VARIABLE_REFERENCE_PREFIX) {
        return Ok(None);
    }
    let mut resolved = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((before, referenced)) = rest.split_once(STAGE_VARIABLE_REFERENCE_PREFIX) {
        let Some((name, after)) = referenced.split_once(STAGE_VARIABLE_REFERENCE_CLOSE) else {
            break;
        };
        let Some(carried) = variables.get(name) else {
            return Err(name.to_string());
        };
        resolved.push_str(before);
        resolved.push_str(carried);
        rest = after;
    }
    resolved.push_str(rest);
    Ok(Some(resolved))
}

/// What a PASSED-THROUGH integration makes of this request.
#[derive(Debug)]
enum PassedThrough {
    /// The request to serve against the endpoint the integration's `uri` names: the
    /// caller's own request, re-addressed at that endpoint and carrying the headers the
    /// integration maps onto it.
    ///
    /// Boxed: a whole request is far larger than the refusal beside it, and this value
    /// is returned by value from the resolution below.
    Request(Box<RtRequest>),
    /// The same request, addressed at an endpoint of this emulator that is NOT a stage of
    /// this api plane (a Lambda function URL, another service's data plane): it is passed
    /// on over the in-process transport instead, and what that endpoint answered is the
    /// method response ([`onward_endpoint`]).
    Onward(Box<RtRequest>),
    /// The integration passes the request to a backend, or through a mapping, this
    /// emulator cannot perform.
    Unserved(UnservedIntegration),
}

/// How `integration` passes this request on, or `None` when it passes nothing on — so the
/// call is performed from the api's own records instead ([`perform_integration`]).
///
/// Only an `HTTP_PROXY` passes anything on here. A `uri` naming an `execute-api` INVOKE
/// ENDPOINT is a stage of THIS api plane, so the pass-through is a request handed back to
/// [`serve_resolved`] in process — no connection is opened, no name is resolved and no
/// address the caller chose is dialled. The endpoint's own records then decide the answer —
/// which api, which stage, which method, and that api's resource policy — exactly as they
/// would for a client that addressed the endpoint directly, so a proxy integration's answer
/// is never assembled here.
///
/// A `uri` naming ANY OTHER endpoint is passed on just the same, over the in-process
/// transport, because this emulator publishes data planes other than this one — a Lambda
/// function URL is an HTTP endpoint a proxy integration names — and the answer is whichever
/// service CLAIMS that endpoint ([`onward_endpoint`]), or the refusal of an integration this
/// stage cannot perform when nothing claims it at all. The two differ only in how the
/// request is carried: one re-enters this plane's own serving, the other crosses the
/// transport, and neither invents a backend answer.
///
/// The endpoint the request is passed on to, and what the api names ITSELF to that endpoint,
/// are the integration's own: the `uri`'s authority is the `Host` the onward request carries
/// and the api's id travels in [`INTEGRATION_API_ID_HEADER`], both of which replace whatever
/// the caller sent under those names.
///
/// An `HTTP` integration's response is chosen by a mapping this emulator does not evaluate,
/// so it is left to [`perform_integration`]'s refusal. The integration's own
/// `requestParameters` are applied on the way: each entry maps "a method request parameter
/// value or static value" onto a header, a path parameter or a query-string parameter of the
/// integration request, the three locations that member names, and the `uri`'s own `{name}`
/// placeholders are addressed from the path ones ([`expanded_uri`]). An entry whose KEY names
/// none of those three locations, one whose VALUE is in neither documented form, and a `uri`
/// placeholder no mapping names are each stated as an integration this call cannot perform
/// rather than silently dropped. A mapping whose SOURCE the request simply does not carry is
/// a different thing from one written in a form that is not read: the parameter is left off,
/// and the request is still passed on ([`MappedParameter`]).
///
/// `hop` is how many integrations of this emulator's own endpoints this request has already
/// been passed through, and it is checked HERE, before any onward request is built: an
/// integration whose `uri` names the very endpoint it is reached at — or two that name each
/// other — would otherwise re-enter the serving path for ever, so the chain is refused once
/// it reaches [`MAX_PASSED_THROUGH_HOPS`]. The bound sits at the construction site because
/// that is where the loop would be made: nothing can pass a request on without passing it.
fn passed_through(
    integration: &GetIntegrationOutput,
    req: &RtRequest,
    hop: u8,
    path_parameters: &BTreeMap<String, String>,
    api_id: &str,
) -> Option<PassedThrough> {
    if integration.r#type.as_deref() != Some(HTTP_PROXY_INTEGRATION_TYPE) {
        return None;
    }
    if hop >= MAX_PASSED_THROUGH_HOPS {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the request has been passed through {MAX_PASSED_THROUGH_HOPS} integrations \
                 of this emulator's own endpoints and this one passes it on again, so it is \
                 not followed further"
            ),
        }));
    }
    let uri = integration.uri.as_deref().unwrap_or_default();
    // The endpoint the request is passed on to. A `uri` with no authority at all addresses
    // nothing, so nothing is passed on for it; an endpoint that is a stage of THIS plane is
    // served by re-entering it, and any other is reached over the in-process transport.
    //
    // A stage of this plane is named in EITHER of the two addressings this plane publishes
    // for one: the `execute-api` endpoint host AWS gives it, and the prefixed path-style
    // address this emulator announces it at ([`announced_address`],
    // `/_aws/execute-api/{api-id}/{stage}/…`) — which is the invoke URL a client of an
    // emulator holds, and so the one an integration naming a stage of this plane carries.
    // Both re-enter this plane's own serving rather than crossing the transport to come
    // back in through the claim, which is what lets what THIS plane did with the request —
    // the endpoint it delivered it through — travel with it as an argument rather than on
    // the wire.
    let host = authority_of(uri)?;
    let within_this_plane =
        endpoint_labels(host).is_some() || split_execute_api_prefix(path_of(uri)).is_some();

    // What `requestParameters` maps onto the integration request, read BEFORE anything is
    // built from it: the PATH parameters decide the address itself, so the mappings are
    // resolved first and the request is addressed from them.
    let mut mapped = MappedRequest::default();
    for (key, value) in integration.request_parameters.iter().flatten() {
        let Some((location, name)) = integration_request_parameter(key) else {
            return Some(PassedThrough::Unserved(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: format!(
                    "the integration maps {key}, which is not a parameter of the integration \
                     request's header, path or query string, so the mapping is not applied \
                     and the request is not sent"
                ),
            }));
        };
        let value = match mapped_request_parameter(value, req, path_parameters) {
            MappedParameter::Value(mapped) => mapped,
            // The source names a location this emulator reads and the request carries
            // nothing there, so the mapping maps nothing: the backend is called without
            // that parameter, which is what it would be called with on AWS.
            MappedParameter::SourceAbsent => continue,
            MappedParameter::NotRead => {
                return Some(PassedThrough::Unserved(UnservedIntegration {
                    refusal: API_CONFIGURATION_ERROR,
                    reason: format!(
                        "the integration maps {key} from {value}, which is neither a static \
                         value in single quotes nor a method request parameter this emulator \
                         reads"
                    ),
                }));
            }
        };
        match location {
            ParameterIn::Header => mapped.headers.push((name.to_string(), value)),
            ParameterIn::Path => {
                mapped.path.insert(name.to_string(), value);
            }
            ParameterIn::Query => mapped.querystring.push((name.to_string(), value)),
        }
    }

    // The address the backend is called at: the `uri`, with each `{name}` placeholder
    // replaced by the path parameter the integration maps onto that name, and then the
    // query string the call carries onward. A placeholder still left after that names a
    // path parameter the integration maps nothing onto, so the address is unknown.
    let addressed = expanded_uri(uri, &mapped.path);
    if addressed.contains('{') {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's uri carries a placeholder its requestParameters map \
                     no path parameter onto: no request is passed on to a guessed-at address"
                .to_string(),
        }));
    }
    let mut passed = RtRequest::new(SdkBody::from(request_payload(req)));
    if passed
        .set_uri(onward_uri(&addressed, req.uri(), &mapped.querystring).as_str())
        .is_err()
    {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's uri is not a URI a request can be addressed at"
                .to_string(),
        }));
    }
    // "Specifies the integration's HTTP method type" (`Integration$httpMethod`): the verb
    // the backend is called with, which is the caller's own where the integration names
    // none — and where it names the `ANY` catch-all, which is a verb no backend is called
    // with but the stand-in for every one of them, exactly as it is on a method.
    let verb = integration
        .http_method
        .as_deref()
        .filter(|declared| !declared.eq_ignore_ascii_case(ANY_METHOD))
        .unwrap_or_else(|| req.method());
    if passed.set_method(verb).is_err() {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's httpMethod is not an HTTP method a request can be \
                     made with"
                .to_string(),
        }));
    }
    for (name, value) in req.headers().iter() {
        if HEADERS_NOT_PASSED_THROUGH.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        // `append`, not `insert`: `iter` yields one entry per value, so a repeated header
        // reaches the backend with every value it arrived with. Through the FALLIBLE form:
        // these are bytes the caller chose, and one the HTTP layer cannot represent must
        // leave the header off rather than abort the request.
        if let Err(refused) = passed
            .headers_mut()
            .try_append(name.to_string(), value.to_string())
        {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the caller's header cannot be carried onward and is left off the request \
                 the integration passes on"
            );
        }
    }
    // The endpoint the request is passed on to is the one the `uri` names, so that is the
    // host the onward request is ADDRESSED to — the header a client sets from the URL it
    // dials, and what the service claiming that endpoint reads the address off. Set before
    // the integration's own mapped headers, so a mapping onto `host` still has the last
    // word.
    if let Err(refused) = passed.headers_mut().try_insert("host", host.to_string()) {
        tracing::warn!(
            host = %host,
            error = %refused,
            "the endpoint the integration's uri names is not a host a request can carry"
        );
    }
    // The api the request came through, which API Gateway names to the endpoint it passes
    // the request on to ([`INTEGRATION_API_ID_HEADER`]). It is the api's own record rather
    // than anything the caller sent, so it REPLACES a header of that name the caller
    // carried.
    if let Err(refused) = passed
        .headers_mut()
        .try_insert(INTEGRATION_API_ID_HEADER, api_id.to_string())
    {
        tracing::warn!(
            api_id = %api_id,
            error = %refused,
            "the api id is not a value a header can carry, so the integration request does \
             not name the api"
        );
    }
    for (name, value) in mapped.headers {
        // A mapped header REPLACES what the caller sent under that name, which is what a
        // mapping onto it declares; a value the wire cannot carry is stated rather than
        // dropped, because the backend would then be called without it.
        if passed
            .headers_mut()
            .try_insert(name.clone(), value)
            .is_err()
        {
            return Some(PassedThrough::Unserved(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: format!(
                    "the value the integration maps onto \
                     {INTEGRATION_REQUEST_HEADER_PREFIX}{name} is not one a header can carry"
                ),
            }));
        }
    }
    Some(if within_this_plane {
        PassedThrough::Request(Box::new(passed))
    } else {
        PassedThrough::Onward(Box::new(passed))
    })
}

/// What an integration's `requestParameters` map onto the integration request, by the
/// location each parameter travels in.
#[derive(Debug, Default)]
struct MappedRequest {
    /// The headers the integration request carries, in the order the mappings were read.
    headers: Vec<(String, String)>,
    /// The PATH parameters, by name: what the `uri`'s own `{name}` placeholders are
    /// addressed with.
    path: BTreeMap<String, String>,
    /// The query-string pairs the integration request carries, in the order read.
    querystring: Vec<(String, String)>,
}

/// Where `key` says a parameter of the INTEGRATION request travels and under what name, or
/// `None` for a key that names no integration request parameter at all.
///
/// `Integration$requestParameters`' key is "an integration request parameter name", written
/// `integration.request.{location}.{name}` over the three locations a request parameter
/// travels in — `querystring`, `path` or `header`, the same three
/// `Method$requestParameters` names.
fn integration_request_parameter(key: &str) -> Option<(ParameterIn, &str)> {
    [
        (INTEGRATION_REQUEST_HEADER_PREFIX, ParameterIn::Header),
        (INTEGRATION_REQUEST_PATH_PREFIX, ParameterIn::Path),
        (INTEGRATION_REQUEST_QUERY_PREFIX, ParameterIn::Query),
    ]
    .into_iter()
    .find_map(|(prefix, location)| {
        key.strip_prefix(prefix)
            .filter(|name| !name.is_empty())
            .map(|name| (location, name))
    })
}

/// `uri` with each `{name}` placeholder replaced by the value `path` gives that name.
///
/// `Integration$uri` is the address the backend is called at, and a path parameter of the
/// integration request is what one of its segments is: the `{name}` the `uri` carries is
/// the parameter `integration.request.path.{name}` maps a value onto, which is how one
/// integration addresses the path the caller asked for — the greedy `{proxy}` of a catch-all
/// resource among them. A placeholder no mapping names is left in place, so the caller can
/// tell an address it cannot resolve from one it can.
///
/// ONE left-to-right pass over the `uri` itself, so a value that has been substituted is
/// never scanned again: the values come from the request (a header, a query parameter, a
/// bound path segment), so rewriting the whole string once per name would let one request's
/// value carry another name's placeholder and make the result the PRODUCT of the values'
/// sizes rather than their sum. It is the same no-rescan contract
/// [`resolved_references`] states for a stage variable, for the same reason.
fn expanded_uri(uri: &str, path: &BTreeMap<String, String>) -> String {
    let mut expanded = String::with_capacity(uri.len());
    let mut rest = uri;
    while let Some(open) = rest.find('{') {
        let (before, from_open) = rest.split_at(open);
        expanded.push_str(before);
        // A `{` that closes nowhere is text of the address, not a placeholder.
        let Some(close) = from_open.find('}') else {
            expanded.push_str(from_open);
            return expanded;
        };
        let name = from_open.get(1..close).unwrap_or_default();
        match path.get(name) {
            Some(value) => expanded.push_str(value),
            // A placeholder no mapping names travels on as written.
            None => expanded.push_str(from_open.get(..=close).unwrap_or_default()),
        }
        rest = from_open.get(close + 1..).unwrap_or_default();
    }
    expanded.push_str(rest);
    expanded
}

/// The URI the onward request is addressed at: the integration's `uri` (already addressed,
/// see [`expanded_uri`]), carrying the query parameters the integration MAPS onto the
/// integration request and the CALLER's own query string as well as any the `uri` itself
/// names.
///
/// A proxy integration passes the request through, and a request's query string is part of
/// it: a backend method that declares a query parameter must see the one the caller sent.
/// The `uri`'s own pairs come first and the integration's mapped ones follow — both are the
/// configuration, and a caller cannot displace them — and the caller's come last; a caller
/// that sent none and an integration that maps none leave the `uri` untouched.
///
/// What does NOT travel is the credential a PRESIGNED caller carries in the query string
/// ([`SIGNED_QUERY_PARAMETERS`]), for the reason [`HEADERS_NOT_PASSED_THROUGH`] gives for
/// the header form of the same credential: a signature over the address the caller used
/// authenticates nothing at the address the request is passed on to.
fn onward_uri(uri: &str, request_uri: &str, mapped: &[(String, String)]) -> String {
    let mut pairs: Vec<String> = mapped
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    pairs.extend(
        query_of(request_uri)
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter(|pair| {
                let name = pair.split('=').next().unwrap_or(pair).to_ascii_lowercase();
                !SIGNED_QUERY_PARAMETERS.contains(&name.as_str())
            })
            .map(str::to_string),
    );
    if pairs.is_empty() {
        return uri.to_string();
    }
    let sent = pairs.join("&");
    let separator = if query_of(uri).is_empty() { '?' } else { '&' };
    format!("{uri}{separator}{sent}")
}

/// What one `requestParameters` entry maps onto the integration request.
#[derive(Debug)]
enum MappedParameter {
    /// The value to carry onward.
    Value(String),
    /// The entry names a method request parameter this emulator reads and the request
    /// carries none under that name, so the mapping maps nothing at all.
    SourceAbsent,
    /// The entry is written in a form this emulator does not read, so what it would carry
    /// is unknown.
    NotRead,
}

/// What the `requestParameters` value `value` maps onto the integration request.
///
/// The member documents the two forms: "a method request parameter value or static value
/// that must be enclosed within single quotes". The static one is the literal between the
/// quotes; a method request parameter "must match the pattern of
/// `method.request.{location}.{name}`, where `location` is `querystring`, `path`, or
/// `header`", and each of the three is read off this call: its headers, its query string and
/// the PATH PARAMETERS the matched resource template bound (`path_parameters`), which is
/// what lets a catch-all resource's greedy parameter address the backend.
/// The query string is read through `service_common`'s one definition of what a query
/// parameter's value is, so this reads a pair the way the rest of the emulator does; it is
/// the FIRST value under that name, which is the one a single-valued mapping expression
/// names ([`METHOD_REQUEST_MULTI_QUERY_PREFIX`] is the spelling that names them all).
fn mapped_request_parameter(
    value: &str,
    req: &RtRequest,
    path_parameters: &BTreeMap<String, String>,
) -> MappedParameter {
    if let Some(literal) = static_mapping_literal(value) {
        return MappedParameter::Value(literal.to_string());
    }
    if let Some(name) = value.strip_prefix(METHOD_REQUEST_HEADER_PREFIX) {
        return match req.headers().get(name) {
            Some(sent) => MappedParameter::Value(sent.to_string()),
            None => MappedParameter::SourceAbsent,
        };
    }
    if let Some(name) = value.strip_prefix(METHOD_REQUEST_QUERY_PREFIX) {
        return match ::service_common::query_parameter(query_of(req.uri()), name) {
            Some(sent) => MappedParameter::Value(sent.to_string()),
            None => MappedParameter::SourceAbsent,
        };
    }
    if let Some(name) = value.strip_prefix(METHOD_REQUEST_PATH_PREFIX) {
        return match path_parameters.get(name) {
            Some(bound) => MappedParameter::Value(bound.clone()),
            None => MappedParameter::SourceAbsent,
        };
    }
    MappedParameter::NotRead
}

/// The payload of a request as the bytes it is passed on with.
///
/// A body this emulator cannot read as bytes — one that was never buffered — passes nothing
/// on, which is what a request with no payload passes on too: every request this data plane
/// is handed arrives buffered.
fn request_payload(req: &RtRequest) -> Vec<u8> {
    req.body().bytes().unwrap_or_default().to_vec()
}

/// Why the private connection `integration` reaches its endpoint through cannot carry this
/// call, or `None` when it names no private connection or names one that can.
///
/// `connectionType` `VPC_LINK` is "for private connections between API Gateway and a network
/// load balancer in a VPC" and `connectionId` names the link that connection runs over, so
/// the link is this service's OWN record and is read while the store is held. A link this
/// account does not hold, and one that has not finished provisioning
/// ([`crate::state::VPC_LINK_AVAILABLE`]), carries no traffic — so the integration cannot be
/// performed over it, whatever its `uri` names.
fn unusable_vpc_link(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    integration: &GetIntegrationOutput,
) -> Option<String> {
    let connection_type = integration
        .connection_type
        .as_deref()
        .unwrap_or(DEFAULT_CONNECTION_TYPE);
    if connection_type != VPC_LINK_CONNECTION_TYPE {
        return None;
    }
    let Some(connection_id) = integration.connection_id.as_deref() else {
        return Some(format!(
            "the integration's connectionType is {VPC_LINK_CONNECTION_TYPE} and it names no \
             connectionId, so it reaches no network load balancer"
        ));
    };
    // The stage's own variables have already been resolved into the configuration
    // ([`with_stage_variables`]), so a `connectionId` written as a stage-variable
    // reference names the link that variable's value names. A value still carrying `${`
    // is an expression this emulator does not evaluate, so it names no link at all.
    if connection_id.contains("${") {
        return Some(format!(
            "the integration's connectionId is {connection_id}, which is an expression this \
             emulator does not resolve to a VPC link"
        ));
    }
    match store.vpc_links.get(connection_id) {
        None => Some(format!(
            "the integration connects through the VPC link {connection_id}, which this \
             account holds no record of"
        )),
        Some(link) if link.status() != crate::state::VPC_LINK_AVAILABLE => Some(format!(
            "the VPC link {connection_id} this integration connects through is {status}, so \
             it carries no traffic",
            status = link.status()
        )),
        Some(_) => None,
    }
}

/// Perform the call through the method's integration, through the IN-PROCESS transport
/// `ctx` carries where the integration names another emulated service's action.
///
/// The one entry point both sides of this module's work reach — the data plane above and
/// `TestInvokeMethod`, which reports what this performs — so one stored configuration is
/// performed one way whichever side drives it. An `AWS` integration naming a service call
/// this emulator makes is performed by MAKING that call ([`aws_integration`]), an
/// `AWS_PROXY` integration by INVOKING the function it names ([`lambda_proxy`]); every other
/// integration is performed from the api's own records alone ([`perform_integration`]),
/// which needs no transport and is therefore still reachable on its own.
///
/// `region` is the api's own, which is the endpoint an integration ARN that names no Region
/// of its own is called at.
pub async fn performed_integration(
    ctx: &OperationContext<'_>,
    region: &str,
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Result<IntegratedResponse, UnservedIntegration> {
    if let Some(method) = method {
        if let Some(served) = lambda_proxy::performed(ctx, region, method, call).await {
            return served;
        }
        if let Some(served) = aws_integration::performed(ctx, region, method, call).await {
            return served;
        }
    }
    perform_integration(method, call)
}

/// Perform the call through the method's integration, answering the response the API's
/// records produce for it.
///
/// `MOCK` is the one type that needs no backend, and it is performed in full from those
/// records: the integration REQUEST template this call selects IS what the integration
/// returns, and the `statusCode` that template carries names the integration response the
/// call maps to. A call no template maps is passed through or refused as
/// `passthroughBehavior` says, and one whose mapping names no stored response maps to the
/// DEFAULT integration response — the one with no `selectionPattern`, since a `MOCK`
/// produces no backend response for a pattern to match.
///
/// Every other type names a backend whose answer is not this api's records to produce: an
/// `HTTP_PROXY` naming an endpoint of this emulator is served by passing the request on
/// ([`passed_through`], which [`serve_resolved`] reaches before this), an `AWS` integration
/// naming an action this emulator serves is served by MAKING that call
/// ([`aws_integration`]), an `AWS_PROXY` is served by INVOKING the function its `uri` names
/// ([`lambda_proxy`]) — both of which [`performed_integration`] reaches before this — and
/// every other one is an integration this call cannot perform rather than one whose answer
/// is invented — see the module docs.
pub fn perform_integration(
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let Some(integration) = method.and_then(|method| method.integration.as_ref()) else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the method carries no integration, so there is nothing to call"
                .to_string(),
        });
    };
    let integration_type = integration.r#type.as_deref().unwrap_or_default();
    if integration_type != MOCK_INTEGRATION_TYPE {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the {integration_type} integration names a backend this emulator cannot \
                 call: no deployment package runs in process and no outbound connection is \
                 made, so no backend answer exists to map"
            ),
        });
    }

    // The status the call maps to, and what the integration ANSWERED — which a response
    // template reads its payload off. A `MOCK` has no backend, so its answer is the request
    // template's own rendering.
    let (mapped_status, integrated_payload) = match request_mapping(integration, call) {
        // The integration request a `MOCK` answers with is its request template RENDERED
        // against this call: the `statusCode` the rendered text names is the integration
        // response the call maps to, and the rendered text IS what the integration
        // answered, which is what a response template then reads its payload off. A
        // template this emulator does not render is an integration the call cannot be
        // performed through — the same exit its response template takes — rather than a
        // call served with a status and a body nothing produced.
        RequestMapping::Mapped(template) => {
            let body = rendered_mapping_template(template, &call.inputs).map_err(|reason| {
                UnservedIntegration {
                    refusal: API_CONFIGURATION_ERROR,
                    reason: format!(
                        "the {} request template of {} bytes is not one this emulator \
                         renders: {reason}",
                        call.media_type,
                        template.len()
                    ),
                }
            })?;
            (mock_status_code(&body), Some(body.into_bytes()))
        }
        // Nothing is mapped, so the integration answers the method request's own payload:
        // that is what "passes the method request body through the integration request
        // without transformation" means.
        RequestMapping::PassedThrough => (None, call.inputs.payload.clone()),
        RequestMapping::Unsupported => {
            return Err(UnservedIntegration {
                refusal: UNSUPPORTED_MEDIA_TYPE,
                reason: format!(
                    "no integration request template maps {}, and the integration's \
                     passthroughBehavior does not pass an unmapped body through",
                    call.media_type
                ),
            });
        }
    };
    let selected = mapped_status
        .and_then(|code| {
            integration
                .integration_responses
                .as_ref()
                .and_then(|responses| responses.get(&code))
        })
        .or_else(|| default_integration_response(integration));
    let Some(selected) = selected else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the MOCK integration has no integration response for this call, so it \
                     maps to no method response"
                .to_string(),
        });
    };
    let Some(status) = selected
        .status_code
        .as_deref()
        .and_then(|code| code.parse::<u16>().ok())
    else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration response this call maps to carries no status code the \
                     response can be sent with"
                .to_string(),
        });
    };

    // The response template is rendered against the same call, with the payload it reads
    // being the one the INTEGRATION answered rather than the one the client sent.
    let response_inputs = TemplateInputs {
        payload: integrated_payload,
        ..call.inputs.clone()
    };
    let body = response_body(selected, call, &response_inputs).map_err(|template| {
        UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the {} response template of {} bytes is not one this emulator renders: {}",
                template.content_type, template.bytes, template.reason
            ),
        }
    })?;
    let mut headers = mapped_response_headers(method, selected, status);
    // The media type is the method response's own `Content-Type` mapping where it declares
    // one, and `application/json` otherwise — never the key of the response template the
    // caller's `Accept` chose, which selects the BODY.
    let content_type =
        taken_content_type(&mut headers).unwrap_or_else(|| JSON_CONTENT_TYPE.to_string());
    Ok(IntegratedResponse {
        status,
        content_type,
        body,
        headers,
    })
}

/// The response headers the call answers with: API Gateway returns only the headers the
/// METHOD response declares, mapped as the INTEGRATION response says.
///
/// A `MOCK` has no backend response to map from, so the one mapping it can resolve is the
/// documented static value in single quotes ([`static_mapping_literal`]): the literal
/// between the quotes is the header's value.
fn mapped_response_headers(
    method: Option<&MethodEntry>,
    selected: &GetIntegrationResponseOutput,
    status: u16,
) -> BTreeMap<String, Vec<String>> {
    let declared = method
        .and_then(|method| method.method_responses.get(&status.to_string()))
        .and_then(|response| response.response_parameters.as_ref());
    let mut mapped = BTreeMap::new();
    for (key, value) in selected.response_parameters.iter().flatten() {
        let Some(name) = key.strip_prefix(METHOD_RESPONSE_HEADER_PREFIX) else {
            continue;
        };
        if declared.is_none_or(|declared| !declared.contains_key(key)) {
            continue;
        }
        if let Some(literal) = static_mapping_literal(value) {
            // A mapping declares ONE value for the header, so the name carries exactly it.
            mapped.insert(name.to_string(), vec![literal.to_string()]);
        }
    }
    mapped
}

/// The wire response a performed integration answers with.
///
/// A mapped header's name is lowercased on the way out, as HTTP/2 carries one; the name the
/// mapping declared is kept on [`IntegratedResponse::headers`] for the caller that REPORTS
/// the response instead of sending it.
///
/// A header is inserted with `try_insert`, not the panicking `insert`: its name and
/// value come from records a control-plane request wrote, and a stored mapping the wire
/// cannot carry (a value with a newline in it, say) must not fail the REQUEST — it is
/// skipped, and said so, so the rest of the response is still served.
fn served_response(served: IntegratedResponse) -> RtResponse {
    let mut response = response_with(served.status, &served.content_type, served.body);
    for (name, values) in served.headers {
        for value in values {
            // APPENDED, not inserted: a name the backend sent several values under is sent
            // as several field lines, which is the only way one name carries them all.
            if let Err(refused) = response
                .headers_mut()
                .try_append(name.to_ascii_lowercase(), value)
            {
                tracing::warn!(
                    header = %name,
                    error = %refused,
                    "the mapped response header cannot be sent on the wire and is skipped"
                );
            }
        }
    }
    response
}

/// What the integration's request templates and `passthroughBehavior` make of this call.
#[derive(Debug)]
enum RequestMapping<'i> {
    /// The template the request's content type selects, which for a `MOCK` is what the
    /// integration returns.
    Mapped(&'i str),
    /// No template maps this content type, and the behaviour lets the body through
    /// unmapped — so the call produces no mapped response of its own.
    PassedThrough,
    /// No template maps this content type and the behaviour forbids passing it through.
    Unsupported,
}

/// The integration request template this call selects, by the content type it declares.
///
/// `requestTemplates` is "a key-value map ... the key is an `Accept`/`Content-Type` header
/// value and the value is a mapping template", and `passthroughBehavior` decides a request
/// no key matches: `WHEN_NO_MATCH` "passes the method request body through the integration
/// request without transformation", `WHEN_NO_TEMPLATES` allows that only when the
/// integration declares NO template at all, and `NEVER` "rejects the method request with an
/// HTTP 415 Unsupported Media Type response". A request that declares no content type is
/// read as the JSON one, the type API Gateway assumes for template selection.
fn request_mapping<'i>(
    integration: &'i GetIntegrationOutput,
    call: &MethodRequest,
) -> RequestMapping<'i> {
    let behaviour = integration
        .passthrough_behavior
        .as_deref()
        .unwrap_or(crate::ops::put_integration::DEFAULT_PASSTHROUGH_BEHAVIOR);
    let templates = integration
        .request_templates
        .as_ref()
        .filter(|templates| !templates.is_empty());
    let Some(templates) = templates else {
        // No template at all: only `NEVER` refuses such a call, and the other two
        // behaviours pass the body through.
        return if behaviour == PASSTHROUGH_NEVER {
            RequestMapping::Unsupported
        } else {
            RequestMapping::PassedThrough
        };
    };
    if let Some(template) = templates.get(&call.media_type) {
        return RequestMapping::Mapped(template);
    }
    // Templates exist but none keys this content type: `WHEN_NO_MATCH` is the only
    // behaviour that still lets the body through.
    if behaviour == PASSTHROUGH_WHEN_NO_MATCH {
        RequestMapping::PassedThrough
    } else {
        RequestMapping::Unsupported
    }
}

/// The media type a request declares, as a `requestTemplates` key is compared: the
/// `Content-Type` value with any parameter (`; charset=…`) dropped and the type
/// lowercased, defaulting to [`JSON_CONTENT_TYPE`] when the request declares none.
fn declared_media_type(declared: Option<&str>) -> String {
    declared
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|media| !media.is_empty())
        .unwrap_or_else(|| JSON_CONTENT_TYPE.to_string())
}

/// The media type a request ACCEPTS, as a `responseTemplates` key is compared: the first
/// entry of the `Accept` value with its parameters dropped, or `None` when the request
/// accepts anything.
fn first_media_type(accepted: Option<&str>) -> Option<String> {
    accepted
        .and_then(|accept| accept.split(',').next())
        .map(|media| media.split(';').next().unwrap_or(media).trim().to_string())
        .filter(|media| !media.is_empty())
}

/// The integration-response status code the selected `MOCK` request template names, or
/// `None` when it names none.
///
/// A `MOCK` integration's request template is where the call's status comes from: the
/// documented form is a literal object carrying the status to return
/// (`{"statusCode": 200}`), and that code is the key of the integration response the call
/// maps to. This reads the template as it was RENDERED against the call, so a template that
/// computes the status from the request names the code it produced; a rendering that is not
/// such an object names no code, and the call then maps to the default integration response
/// rather than to a code guessed at.
fn mock_status_code(rendered: &str) -> Option<String> {
    let value: ::serde_json::Value = ::serde_json::from_str(rendered).ok()?;
    let status = value.get("statusCode")?;
    // A JSON number, and the string spelling a template written with quotes produces;
    // both name the same integration response.
    match status {
        ::serde_json::Value::Number(number) => number.as_u64().map(|code| code.to_string()),
        ::serde_json::Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

/// The integration response a `MOCK` call maps to when its request mapping names none:
/// the one with no `selectionPattern`.
///
/// `selectionPattern` is "the regular expression (regex) pattern used to choose an
/// integration response based on the response from the back end" — there is no back-end
/// response here, so a patterned response is never chosen and the default one is.
fn default_integration_response(
    integration: &GetIntegrationOutput,
) -> Option<&GetIntegrationResponseOutput> {
    let responses = integration.integration_responses.as_ref()?;
    let mut defaults: Vec<&GetIntegrationResponseOutput> = responses
        .values()
        .filter(|response| {
            response
                .selection_pattern
                .as_deref()
                .is_none_or(str::is_empty)
        })
        .collect();
    // Keyed by status code in the record, so the choice is stable across calls whatever
    // order the map yields.
    defaults.sort_by(|left, right| left.status_code.cmp(&right.status_code));
    defaults.first().copied()
}

/// A response template this emulator does not render, described for the log: the content
/// type it is keyed by, its size and why it was not rendered. The template itself is a
/// developer's configuration and is not copied into the log; the reason names the one
/// reference or bound that stopped the rendering, clipped.
#[derive(Debug)]
struct UnrenderableTemplate {
    content_type: String,
    bytes: usize,
    reason: String,
}

/// `template` rendered against the call, or why this emulator does not render it.
///
/// A mapping template is "a map of Velocity templates" (`Integration$requestTemplates`),
/// which has two halves: REFERENCES, which name values of the call and are rendered from
/// them ([`mapping_template`]), and DIRECTIVES, which are statements that would have to be
/// EXECUTED. This emulator renders the first and states the second, so a template it cannot
/// render is reported as the integration it cannot perform rather than passed through as if
/// it were a literal body. A template with neither half IS the body, and renders to itself
/// through the same path.
fn rendered_mapping_template(template: &str, inputs: &TemplateInputs) -> Result<String, String> {
    if carries_velocity_directive(template) {
        return Err("it carries a Velocity directive, which is a statement this emulator \
                    does not execute"
            .to_string());
    }
    mapping_template::rendered(template, inputs).map_err(|unrendered| unrendered.reason)
}

/// The body the selected integration response renders.
///
/// The template is chosen by the content type the caller asked for (`Accept`), then the
/// JSON one, then the single template an integration response that declares exactly one
/// carries. `Ok` with an EMPTY body when no template applies — a `MOCK` with no template
/// answers no body — and `Err` when the chosen template is not one this emulator renders
/// ([`rendered_mapping_template`]).
///
/// Which template was chosen decides the BODY and nothing else: the media type the response
/// is SERVED with is the method response's own `Content-Type` mapping
/// ([`taken_content_type`]), so a call whose method response declares none is served
/// `application/json` whichever template keyed the chosen body.
fn response_body(
    selected: &GetIntegrationResponseOutput,
    call: &MethodRequest,
    inputs: &TemplateInputs,
) -> Result<Vec<u8>, UnrenderableTemplate> {
    let Some(templates) = selected
        .response_templates
        .as_ref()
        .filter(|templates| !templates.is_empty())
    else {
        return Ok(Vec::new());
    };
    let chosen = call
        .accepted
        .as_deref()
        .and_then(|media| templates.get_key_value(media))
        .or_else(|| templates.get_key_value(JSON_CONTENT_TYPE))
        .or_else(|| {
            let mut entries: Vec<(&String, &String)> = templates.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            (entries.len() == 1).then(|| entries[0])
        });
    let Some((content_type, template)) = chosen else {
        return Ok(Vec::new());
    };
    match rendered_mapping_template(template, inputs) {
        Ok(body) => Ok(body.into_bytes()),
        Err(reason) => Err(UnrenderableTemplate {
            content_type: content_type.clone(),
            bytes: template.len(),
            reason,
        }),
    }
}

/// The `Content-Type` `mapped` declares, REMOVED from the map, or `None` when the method
/// response declares no mapping for that header.
///
/// `Content-Type` is a response parameter like every other one — a method response declares
/// it and the integration response maps it ([`mapped_response_headers`]) — so it is the one
/// thing that changes the media type a call is served with. Taken OUT of the map because a
/// response carries one media type: it is [`IntegratedResponse::content_type`], and leaving
/// it among the mapped headers too would send the name twice.
fn taken_content_type(mapped: &mut BTreeMap<String, Vec<String>>) -> Option<String> {
    let name = mapped
        .keys()
        .find(|name| name.eq_ignore_ascii_case(CONTENT_TYPE_HEADER))?
        .clone();
    mapped.remove(&name)?.into_iter().next()
}

/// The response header a media type travels in, as the wire compares its name.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The Velocity directives a mapping template may carry, from the Velocity Template
/// Language reference (the `#`-introduced statements of the language a mapping template is
/// written in: "a map of Velocity templates", `requestTemplates`). `#` followed by anything
/// else — `#fff` in a colour, say — is literal text.
const VELOCITY_DIRECTIVES: &[&str] = &[
    "set", "if", "elseif", "else", "end", "foreach", "break", "stop", "macro", "parse",
    "include", "evaluate", "define",
];

/// Whether `template` carries a Velocity DIRECTIVE — the half of the language that is
/// statements rather than references, and the half no substitution can stand in for.
///
/// The language has exactly two constructs: a REFERENCE, `$` (optionally `$!`) followed by
/// an identifier or `{`, which names a value and is RENDERED from the call
/// ([`rendered_mapping_template`], [`rendered_gateway_template`]); and a DIRECTIVE, `#`
/// followed by one of [`VELOCITY_DIRECTIVES`] (optionally braced) or by a comment marker
/// (`##`, `#*`), which is a statement that would have to be EXECUTED. This tells the two
/// apart, so the halves that resolve references can refuse the half that cannot be resolved
/// at all.
///
/// TOTAL over every `&str`, including a template carrying non-ASCII text: a template is
/// caller-stored data that nothing restricts to ASCII, and this runs while serving a
/// request, so it must not be able to fail. The byte it keys on is ASCII, so the position
/// after one is always a character boundary; the remainder is still taken with `str::get`,
/// which answers `None` rather than panicking for any position that is not.
pub(crate) fn carries_velocity_directive(template: &str) -> bool {
    for (index, byte) in template.as_bytes().iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        let Some(rest) = template.get(index + 1..) else {
            continue;
        };
        if carries_directive_at(rest) {
            return true;
        }
    }
    false
}

/// Whether the text right after a `#` begins a directive or a comment marker; see
/// [`carries_velocity_directive`] for why the NAME has to end where an identifier cannot
/// continue.
fn carries_directive_at(rest: &str) -> bool {
    if rest.starts_with('#') || rest.starts_with('*') {
        return true;
    }
    let directive = rest.strip_prefix('{').unwrap_or(rest);
    VELOCITY_DIRECTIVES.iter().any(|name| {
        directive.strip_prefix(name).is_some_and(|after| {
            after
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
        })
    })
}

/// The `$context` variables a GATEWAY RESPONSE's template names, and what each one stands
/// for in the refusal being answered.
///
/// A gateway response is answered for a request that reached no integration, so the
/// variables it can resolve are the ones describing the REFUSAL. API Gateway's own default
/// template for every type is `{"message":$context.error.messageString}` — the
/// `messageString` form is "the quoted value of `$context.error.message`", quotes
/// included, which is why that template produces a JSON object — beside the message
/// itself, the gateway response type the refusal is answered through, and the validation
/// detail, which is a PLAIN string and which only a refusal that VALIDATED the request
/// has ([`GatewayRefusal::validation_detail`]).
fn gateway_context(refusal: &GatewayRefusal) -> Vec<(&'static str, String)> {
    // Longest path first: `context.error.message` is a PREFIX of
    // `context.error.messageString`, so substituting it first would leave `String` behind.
    vec![
        ("context.error.validationErrorString", refusal.validation_detail.to_string()),
        (
            "context.error.messageString",
            ::serde_json::Value::String(refusal.message.to_string()).to_string(),
        ),
        ("context.error.responseType", refusal.response_type.to_string()),
        ("context.error.message", refusal.message.to_string()),
    ]
}

/// Emulator resource guard, NOT an AWS quota: the largest body one gateway-response template
/// renders to.
///
/// The same bound the sibling renderer of this language works under
/// (`super::mapping_template`'s own rendered-body cap, a small multiple of a stored
/// template's character cap): a gateway template is rendered while SERVING a request, and the
/// values it substitutes — the refusal's message and its validation detail — carry text a
/// caller influenced, so the body is capped as well as built in one pass. Generous enough
/// that no body a gateway response would answer is refused for its size; a template that
/// renders past it answers API Gateway's own default body for the response type
/// ([`default_refusal_body`]), which is what a template this emulator cannot render answers.
const MAX_RENDERED_GATEWAY_BYTES: usize = 8 * crate::ops::put_integration::MAX_TEMPLATE_CHARS;

/// The body a gateway response's `application/json` template renders to for `refusal`, or
/// `None` when this emulator cannot render it.
///
/// The template's `$context` references are resolved from [`gateway_context`], in both
/// spellings the language writes a reference in (`$context.error.message` and
/// `${context.error.message}`); a reference to anything else is left in the body exactly as
/// the template wrote it, which is what the language itself does with a reference it cannot
/// resolve. A template carrying a DIRECTIVE is a template that has to be EXECUTED, which
/// this emulator does not do: it renders nothing rather than a body it guessed at, and the
/// caller gets API Gateway's own default body for the response type.
///
/// ONE left-to-right pass over the TEMPLATE, so a value that has been substituted is never
/// scanned again, and the body is bounded by [`MAX_RENDERED_GATEWAY_BYTES`]. The values come
/// from the refusal, whose message and validation detail carry text the request influenced,
/// so rewriting the whole template once per name would let one value carry another name's
/// reference and make the body the PRODUCT of the values' sizes rather than their sum. It is
/// the same no-rescan contract [`expanded_uri`] and [`resolved_references`] state, for the
/// same reason.
fn rendered_gateway_template(template: &str, refusal: &GatewayRefusal) -> Option<String> {
    if carries_velocity_directive(template) {
        return None;
    }
    // Both spellings of each name, built once rather than once per reference; longest name
    // first, as [`gateway_context`] orders them, so `$context.error.messageString` is
    // recognized before the `$context.error.message` it opens with.
    let references: Vec<(String, String, String)> = gateway_context(refusal)
        .into_iter()
        .map(|(path, value)| (format!("{{{path}}}"), path.to_string(), value))
        .collect();
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(dollar) = rest.find('$') {
        let (before, from_dollar) = rest.split_at(dollar);
        rendered.push_str(before);
        // `$` is ASCII, so what follows it starts at a character boundary.
        let after_dollar = from_dollar.get(1..).unwrap_or_default();
        let substituted = references.iter().find_map(|(braced, plain, value)| {
            let after = after_dollar
                .strip_prefix(braced.as_str())
                .or_else(|| after_dollar.strip_prefix(plain.as_str()))?;
            Some((value, after))
        });
        match substituted {
            Some((value, after)) => {
                rendered.push_str(value);
                rest = after;
            }
            // A reference this refusal resolves nothing for travels on as the template
            // wrote it: scanning continues after the `$`, so the rest of it is copied as
            // text rather than examined as a reference again.
            None => {
                rendered.push('$');
                rest = after_dollar;
            }
        }
        // The length of what has been built, not a walk over it, checked after each value:
        // one value may carry the total past the cap.
        if rendered.len() > MAX_RENDERED_GATEWAY_BYTES {
            return None;
        }
    }
    rendered.push_str(rest);
    Some(rendered)
}

/// The resource path of the DEPLOYMENT that matches `request_path`, most specific first.
///
/// A literal segment is more specific than a `{parameter}`, which is more specific than a
/// greedy `{proxy+}` — the order API Gateway resolves a request in. Returns `None` when the
/// deployment holds no path that matches.
fn match_resource_path<'d>(
    deployment: &'d DeploymentEntry,
    request_path: &str,
) -> Option<&'d str> {
    deployment
        .api_summary
        .iter()
        .filter(|(_, methods)| !methods.is_empty())
        .filter_map(|(path, _)| match_score(path, request_path).map(|score| (score, path)))
        .max_by(|(left, left_path), (right, right_path)| {
            left.cmp(right).then_with(|| right_path.cmp(left_path))
        })
        .map(|(_, path)| path.as_str())
}

/// How specifically the resource path `template` matches `request_path`, or `None` when it
/// does not match at all.
///
/// One score per template segment, so the comparison is lexicographic: a longer match of
/// equally specific segments wins, and a literal (3) beats a path parameter (2), which
/// beats a greedy parameter (1).
fn match_score(template: &str, request_path: &str) -> Option<Vec<u8>> {
    let template_segments = path_segments(template);
    let request_segments = path_segments(request_path);
    let mut score = Vec::with_capacity(template_segments.len());
    let mut consumed = 0usize;
    for segment in template_segments {
        let parameter = segment
            .strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'));
        match parameter {
            // A greedy parameter consumes every remaining segment, and needs at least one.
            Some(name) if name.ends_with('+') => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(1);
                return Some(score);
            }
            Some(_) => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(2);
                consumed += 1;
            }
            None => {
                if request_segments.get(consumed) != Some(&segment) {
                    return None;
                }
                score.push(3);
                consumed += 1;
            }
        }
    }
    (consumed == request_segments.len()).then_some(score)
}

/// The deployed method of `path` that serves `verb`: the verb's own method, else the `ANY`
/// catch-all.
fn match_method<'d>(
    deployment: &'d DeploymentEntry,
    path: &str,
    verb: &str,
) -> Option<(&'d str, &'d crate::ops::types::MethodSnapshot)> {
    let methods = deployment.api_summary.get(path)?;
    let requested = method_key(verb);
    methods
        .get_key_value(&requested)
        .or_else(|| methods.get_key_value(ANY_METHOD))
        .map(|(verb, snapshot)| (verb.as_str(), snapshot))
}

/// The stage name and the resource path of an invoke path: the first segment is the stage,
/// and what follows is the path within the API (`/` when nothing follows).
///
/// `None` when the path names no stage at all, which is an endpoint that serves nothing.
fn split_stage(path: &str) -> Option<(&str, String)> {
    let segments = path_segments(path);
    let (stage, rest) = segments.split_first()?;
    Some((stage, format!("/{}", rest.join("/"))))
}

/// The non-empty `/`-separated segments of a path.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|segment| !segment.is_empty()).collect()
}

/// The QUERY-STRING parameters `query` carries, by name: the FIRST value under each name,
/// which is the one a single-valued reference names.
///
/// Decoded by the FRAMEWORK's own query reader
/// (`aws_smithy_schema_server::parse_form_urlencoded`, the reader
/// [`super::query_coercion`] reads a modelled query string with), so a `%XX` escape and a
/// `+` are the characters the rest of this emulator reads them as rather than the text they
/// were written with. One pass over what it decoded: the first pair under a name wins, as
/// that reader's own single-valued read does.
///
/// `pub` because `TestInvokeMethod` reads the query string of the `pathWithQueryString` a
/// caller simulates the request with, and one query string must be read one way.
pub fn sent_query_parameters(query: &str) -> BTreeMap<String, String> {
    let mut sent: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in ::aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes()) {
        if name.is_empty() {
            continue;
        }
        sent.entry(name).or_insert(value);
    }
    sent
}

/// The HEADERS `req` carries, by name as it was sent: the first value under each name.
fn sent_headers(req: &RtRequest) -> BTreeMap<String, String> {
    let mut sent = BTreeMap::new();
    for (name, value) in req.headers().iter() {
        sent.entry(name.to_string())
            .or_insert_with(|| value.to_string());
    }
    sent
}

/// EVERY value `req` carries under each header name, in the order sent.
///
/// The multi-valued view of [`sent_headers`], which is what a PROXY integration's event
/// carries beside the single-valued one: a name sent twice carries two values, and only this
/// view has both.
fn every_header(req: &RtRequest) -> BTreeMap<String, Vec<String>> {
    let mut sent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in req.headers().iter() {
        sent.entry(name.to_string())
            .or_default()
            .push(value.to_string());
    }
    sent
}

/// EVERY value `query` carries under each query-string name, in the order sent.
///
/// The multi-valued view of [`sent_query_parameters`], read by the same framework query
/// reader so one query string is decoded one way.
fn every_query_parameter(query: &str) -> BTreeMap<String, Vec<String>> {
    let mut sent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in ::aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes()) {
        if name.is_empty() {
            continue;
        }
        sent.entry(name).or_default().push(value);
    }
    sent
}

/// The endpoint host `req` was addressed on — the `Host` it carries, or the authority of its
/// own URI — which is the domain name a PROXY integration's event reports the call arrived at.
fn request_host_of(req: &RtRequest) -> Option<String> {
    req.headers()
        .get("host")
        .or_else(|| authority_of(req.uri()))
        .map(str::to_string)
}

/// The `$context` values of this call, keyed by the member path a mapping template writes.
///
/// Only what the api's own records and the request itself say about the call: which api and
/// stage answered it, the method it reached, the path it arrived at, WHICH REQUEST it is and
/// WHEN it arrived, the account the api belongs to, the endpoint it was addressed on and the
/// address it came from. A member no value here names is a reference the template is not
/// rendered from at all.
///
/// `request_id` is the id the response to this call is reported under, so
/// `$context.requestId` and the header a client reads the id off name ONE request; it is
/// also `$context.extendedRequestId`, which is "the same request's id under the member API
/// Gateway reports it in" (`crate::ops::update_stage` holds that reading for an access-log
/// format). The instants are `now`'s, in the units each member documents: `requestTime` in
/// the Common Log Format the access log writes it in and `requestTimeEpoch` in milliseconds.
///
/// What the method's AUTHORIZATION published is added by the caller, once the authorizer has
/// decided: a user pool's claims under `authorizer.claims.<name>` (the member path a
/// template reads a signed-in user by, `$context.authorizer.claims.sub`) and a custom
/// authorizer's own values under `authorizer.<key>`. A method whose authorization publishes
/// none adds no member, so such a reference stays unrendered.
fn call_context(
    api_id: &str,
    stage: &str,
    verb: &str,
    resource_path: &str,
    req: &RtRequest,
    request_id: &str,
    now: SystemTime,
) -> BTreeMap<String, String> {
    let uri = req.uri();
    let mut context: BTreeMap<String, String> = [
        ("apiId", api_id.to_string()),
        ("stage", stage.to_string()),
        ("httpMethod", verb.to_string()),
        ("resourcePath", resource_path.to_string()),
        ("path", path_of(uri).to_string()),
        ("accountId", ::engine::emulator_account_id().to_string()),
        ("requestId", request_id.to_string()),
        ("extendedRequestId", request_id.to_string()),
        ("requestTimeEpoch", epoch_millis(now).to_string()),
        ("identity.sourceIp", source_address_of(req)),
    ]
    .into_iter()
    .map(|(member, value)| (member.to_string(), value))
    .collect();
    if let Some(stated) = common_log_time(now) {
        context.insert("requestTime".to_string(), stated);
    }
    if let Some(host) = request_host_of(req) {
        if let Some((prefix, _)) = host.split_once('.') {
            context.insert("domainPrefix".to_string(), prefix.to_string());
        }
        context.insert("domainName".to_string(), host);
    }
    if let Some(agent) = req.headers().get(USER_AGENT_HEADER) {
        context.insert("identity.userAgent".to_string(), agent.to_string());
    }
    context
}

/// The request header the caller's user agent travels in, which `$context.identity.userAgent`
/// reports.
const USER_AGENT_HEADER: &str = "user-agent";

/// The request header the address the request came from is recorded in where it reached the
/// emulator through a forwarder.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The address a request that no forwarder recorded an address for came from: this emulator
/// is reached in process or over the loopback interface, so that is where such a request
/// arrived from.
const LOOPBACK_SOURCE_ADDRESS: &str = "127.0.0.1";

/// The address this call came from, as `$context.identity.sourceIp` reports it ("the source
/// IP address of the TCP connection making the request"): the FIRST hop a forwarder recorded
/// where the request reached the emulator through one, and the loopback address otherwise.
fn source_address_of(req: &RtRequest) -> String {
    source_address(req.headers().get(FORWARDED_FOR_HEADER))
}

/// The address a request whose forwarded-for header is `forwarded` came from; see
/// [`source_address_of`].
///
/// `pub(super)` because a PROXY integration's event reports the same address under the same
/// rule, off the call's own header map rather than off the wire request
/// ([`lambda_proxy::identity`]) — one rule, one place.
pub(super) fn source_address(forwarded: Option<&str>) -> String {
    forwarded
        .and_then(|forwarded| forwarded.split(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or(LOOPBACK_SOURCE_ADDRESS)
        .to_string()
}

/// The instant `at` in the Common Log Format API Gateway reports `requestTime` in
/// (`dd/Mon/yyyy:HH:mm:ss +0000`), built from the one HTTP-date rendering this tree has, so
/// there is no second calendar here.
fn common_log_time(at: SystemTime) -> Option<String> {
    // The HTTP date is `Wed, 21 Oct 2015 07:28:00 GMT`: the same fields, in the same units.
    let http_date = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::HttpDate)
        .ok()?;
    let (_weekday, rest) = http_date.split_once(", ")?;
    let mut fields = rest.split(' ');
    let day = fields.next()?;
    let month = fields.next()?;
    let year = fields.next()?;
    let time = fields.next()?;
    Some(format!("{day}/{month}/{year}:{time} +0000"))
}

/// The instant `at` in milliseconds since the epoch, which is the unit `requestTimeEpoch`
/// reports.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// The `$context` member path an authorizer's claims are published under.
///
/// `$context.authorizer.claims.<name>` is how a mapping template reads one claim of the token
/// a `COGNITO_USER_POOLS` authorizer accepted, which is what makes the signed-in user
/// available to the integration.
const AUTHORIZER_CLAIMS_MEMBER: &str = "authorizer.claims";

/// The compiled-program budget one caller-STORED regular expression may take, so an
/// expression inside the stored length still cannot compile into an arbitrarily large
/// automaton — the bound is stated here rather than left to the engine's own default, as the
/// other services that evaluate a caller-stored pattern state theirs.
const COMPILED_PATTERN_LIMIT: usize = 64 * 1024;

/// See [`COMPILED_PATTERN_LIMIT`]: the budget for the lazy DFA a match runs through, so a
/// stored expression cannot thrash that cache either.
const PATTERN_DFA_LIMIT: usize = 64 * 1024;

/// `expression` compiled under the two EXPLICIT budgets above, or `None` for a value this
/// engine does not build a regular expression of within them.
///
/// The ONE compile of a caller-stored regular expression this data plane makes: an
/// `IntegrationResponse$selectionPattern` ([`aws_integration`]) and an
/// `Authorizer$identityValidationExpression` ([`cognito_user_pools`]) are both patterns a
/// CONTROL-PLANE call stored and a REQUEST is then matched against, so both are built here —
/// under budgets, by the maintained engine whose matching is linear in the subject with no
/// backtracking blowup, so a stored pattern cannot make a request to a deployed stage
/// expensive. A pattern the budgets refuse is reported as one this emulator does not
/// evaluate, which is the caller's own decision to make.
pub(crate) fn stored_pattern(expression: &str) -> Option<::regex::Regex> {
    ::regex::RegexBuilder::new(expression)
        .size_limit(COMPILED_PATTERN_LIMIT)
        .dfa_size_limit(PATTERN_DFA_LIMIT)
        .build()
        .ok()
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header a
/// signed SDK request carries, or in the `X-Amz-Credential` query parameter of a presigned
/// URL.
///
/// Only PRESENCE is read, which is all a method's `authorizationType` turns on: `AWS_IAM`
/// is "for using AWS IAM permissions" (`Method$authorizationType`), so a request carrying
/// no credential at all is refused here, and one that carries a credential is admitted. The
/// presigned form is read beside the header form because a presigned URL carries its
/// credential in the query string, so reading only the header would refuse a presigned
/// invoke of an `AWS_IAM` method that AWS admits.
///
/// Whether the SIGNATURE over that credential is correct is a second question, and it is
/// taken with the emulator's OWN verifier rather than with a reading of SigV4 written here:
/// the front door verifies a named operation's credential against the signing name of the
/// service that claimed the request, which for this service is its control plane's, while an
/// invoke is signed under the data plane's own — so this plane asks that same verifier about
/// ITS signing name ([`sigv4_authentication`]) once a method's `AWS_IAM` authorization makes
/// the credential matter. WHICH principal the credential speaks for, and whether that
/// principal may invoke the method, is [`execute_api_authorization`]'s decision.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    sigv4_access_key_id(req).is_some()
}

/// The ACCESS KEY ID of the SigV4 credential the request presents, from either form
/// [`carries_sigv4_credential`] reads: the `Credential=<akid>/<scope>` element of the
/// `Authorization` header, or the `X-Amz-Credential` query parameter of a presigned URL.
///
/// One reader for the presence and the value, so a request that is admitted as signed is
/// also one whose principal can be resolved.
fn sigv4_access_key_id(req: &RtRequest) -> Option<String> {
    sigv4_credential_elements(req)?
        .into_iter()
        .next()
        .filter(|key_id| !key_id.is_empty())
}

/// The SIGNING NAME the request's credential scope names — the fourth element of
/// `<akid>/<date>/<region>/<service>/aws4_request` — or `None` when the request carries no
/// credential or a scope with no such element.
///
/// Read beside the access key id by [`sigv4_credential_elements`], because the two are two
/// fields of one value: [`sigv4_authentication`] states a credential scoped to another
/// service differently from one this emulator holds no secret for, and AWS words those two
/// refusals differently.
pub(super) fn sigv4_credential_scope_service(req: &RtRequest) -> Option<String> {
    sigv4_credential_elements(req)?
        .into_iter()
        .nth(3)
        .filter(|service| !service.is_empty())
}

/// The elements of the credential scope the request presents, in the order SigV4 writes
/// them: `<akid>/<date>/<region>/<service>/aws4_request`.
///
/// Both carriages are read here, and the presigned one through the framework's own query
/// reader ([`sent_query_parameters`]) — so its `%2F`-escaped separators are the separators
/// they encode rather than part of a field, which is what lets the SAME field be read out of
/// a presigned URL and out of an `Authorization` header.
fn sigv4_credential_elements(req: &RtRequest) -> Option<Vec<String>> {
    let credential = match req
        .headers()
        .get("authorization")
        .and_then(|value| value.split("Credential=").nth(1))
        .and_then(|credential| credential.split([',', ' ']).next())
    {
        Some(credential) => credential.trim().to_string(),
        None => sent_query_parameters(query_of(req.uri())).remove("X-Amz-Credential")?,
    };
    let elements: Vec<String> = credential.split('/').map(str::to_string).collect();
    (!elements.is_empty()).then_some(elements)
}

/// One gateway-response refusal, rendered as the API serves it on the wire.
///
/// The response NAMES the error shape it is the wire form of, in the header an SDK reads a
/// restJson1 error code out of, for every refusal whose exception class is known
/// ([`GatewayRefusal::error_type`]) — the endpoint's own classes, not only the ones this
/// service's control-plane model declares.
fn refusal_response(api: Option<&RestApiEntry>, refusal: GatewayRefusal) -> RtResponse {
    let error_type = refusal.error_type;
    let content_type = refusal.content_type();
    let rendered = refusal_rendering(api, refusal);
    let mut response = response_with(rendered.status, content_type, rendered.body);
    if let Some(error_type) = error_type {
        let _ = response
            .headers_mut()
            .try_insert(ERROR_TYPE_HEADER, error_type);
    }
    for (name, value) in rendered.headers {
        if let Err(refused) = response
            .headers_mut()
            .try_insert(name.to_ascii_lowercase(), value)
        {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the gateway response's mapped header cannot be sent on the wire and is \
                 skipped"
            );
        }
    }
    response
}

/// What one gateway-response refusal renders to, as the API answers it.
#[derive(Debug)]
pub struct RenderedRefusal {
    /// The status code the response is sent with.
    pub status: u16,
    /// The body it carries.
    pub body: Vec<u8>,
    /// The headers the API's customization of this gateway response writes on it, by the
    /// name the mapping declared; see [`refusal_rendering`].
    pub headers: BTreeMap<String, String>,
}

/// The status, body and headers one gateway-response refusal renders to, as the API answers
/// it.
///
/// The status code is the API's customization of this response type when it has one, then
/// its customization of the whole class (`DEFAULT_4XX` / `DEFAULT_5XX`), then the status
/// code the response type itself documents. The body is the customized
/// `application/json` template with its `$context` references resolved
/// ([`rendered_gateway_template`]), and otherwise the default `{"message": …}` body
/// ([`default_refusal_body`]) — which is what a template this emulator cannot render falls
/// back to.
///
/// The HEADERS are that same customization's `responseParameters`: a key is a mapping
/// expression naming what the customization writes on the response, and a gateway response
/// writes HEADERS ([`crate::ops::put_gateway_response::GATEWAY_RESPONSE_HEADER_PREFIXES`]),
/// so a mapping declared there reaches the caller on the response the type is served with.
/// The mapping this resolves is the documented static value in single quotes
/// ([`static_mapping_literal`]) — the literal between the quotes is the header's value;
/// a mapping that reads a value off the REQUEST names nothing here, because a gateway
/// response is rendered from the refusal rather than from the request that earned it.
///
/// Separate from [`refusal_response`] because the data plane SENDS this rendering while
/// `TestInvokeMethod` REPORTS it as the simulated response, and a developer's
/// customization must reach both.
pub fn refusal_rendering(api: Option<&RestApiEntry>, refusal: GatewayRefusal) -> RenderedRefusal {
    let documented = crate::state::default_gateway_status_code(refusal.response_type)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(UNPLACEABLE_REFUSAL_STATUS);
    let class_default = if documented < 500 { DEFAULT_4XX } else { DEFAULT_5XX };
    let customized = api.and_then(|api| {
        api.gateway_responses
            .get(refusal.response_type)
            .or_else(|| api.gateway_responses.get(class_default))
    });
    let status = customized
        .and_then(|response| response.status_code.as_deref())
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(documented);
    let templated = customized
        .and_then(|response| response.response_templates.as_ref())
        .and_then(|templates| templates.get(JSON_CONTENT_TYPE))
        .and_then(|template| rendered_gateway_template(template, &refusal));
    let body = match templated {
        Some(template) => template.into_bytes(),
        None => default_refusal_body(&refusal),
    };
    RenderedRefusal {
        status,
        body,
        headers: gateway_response_headers(customized),
    }
}

/// The headers the gateway-response customization `customized` writes on the response it is
/// served with; see [`refusal_rendering`] for which mappings resolve.
fn gateway_response_headers(
    customized: Option<&crate::ops::types::GetGatewayResponseOutput>,
) -> BTreeMap<String, String> {
    let mut mapped = BTreeMap::new();
    for (key, value) in customized
        .and_then(|response| response.response_parameters.as_ref())
        .into_iter()
        .flatten()
    {
        let Some(name) = crate::ops::put_gateway_response::GATEWAY_RESPONSE_HEADER_PREFIXES
            .iter()
            .find_map(|prefix| key.strip_prefix(prefix))
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let literal = static_mapping_literal(value);
        if let Some(literal) = literal {
            mapped.insert(name.to_string(), literal.to_string());
        }
    }
    mapped
}

/// API Gateway's OWN default body for `refusal`: the refusal's sentence under the member
/// the response type carries it in.
///
/// Serialized by hand rather than through a JSON writer because the SEPARATOR is part of
/// what API Gateway sends: the two bodies its request validator serializes itself carry a
/// space after it ([`VALIDATION_RESPONSE_TYPES`]) and the ones its gateway-response template
/// renders carry none, so a client comparing or hashing the payload sees the bytes the
/// service sends.
/// The member name and the sentence are both written as JSON strings, so a sentence
/// carrying a quote or a backslash is escaped as the format requires.
fn default_refusal_body(refusal: &GatewayRefusal) -> Vec<u8> {
    // A refusal the endpoint states with a TEXT media type ([`EDGE_REFUSAL_CONTENT_TYPE`])
    // carries the SENTENCE itself: the body a response declares a media type for has to be
    // readable as that type, so a JSON envelope under `text` would be a body no client could
    // parse by what the response says it is.
    if refusal.content_type() != JSON_CONTENT_TYPE {
        return refusal.message.as_bytes().to_vec();
    }
    let member = ::serde_json::Value::String(refusal.message_member.to_string()).to_string();
    let message = ::serde_json::Value::String(refusal.message.to_string()).to_string();
    let separator = if VALIDATION_RESPONSE_TYPES.contains(&refusal.response_type) {
        ": "
    } else {
        ":"
    };
    format!("{{{member}{separator}{message}}}").into_bytes()
}

/// One data-plane response: a status, a content type and a body.
///
/// The data plane is plain HTTPS rather than a modelled operation, so it answers in the
/// envelope API Gateway answers in — a JSON body — instead of this service's restJson1
/// error envelope.
///
/// The content type is a key of a stored `responseTemplates` map, which no operation
/// restricts to a media type, so it is inserted with `try_insert` rather than the panicking
/// `insert`: a value the wire cannot carry is left off the response instead of failing the
/// request.
fn response_with(status: u16, content_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_REFUSAL_STATUS).expect("500 is a valid status"),
        ),
        SdkBody::from(body),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", content_type.to_string())
    {
        tracing::warn!(
            error = %refused,
            "the integration response's content type cannot be sent on the wire and is \
             left off the response"
        );
    }
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn path_of(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn query_of(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests;

/// The STAGE RESPONSE CACHE this data plane serves a call from, in a file of its own;
/// re-exported whole, so the resolution above still reaches each name unqualified.
mod response_cache;
pub use response_cache::*;

/// The RESPONSE COMPRESSION the api asks for, in a file of its own: the one concern that is
/// about the BYTES of an answer rather than about which answer is served.
mod compression;

/// The method's own REQUEST VALIDATOR, in a file of its own; re-exported whole, so the
/// resolution above still reaches each name unqualified.
mod request_validation;
pub use request_validation::*;

/// The invoke address this emulator ANNOUNCES for a deployed stage — the address an exported
/// definition or a generated SDK names — in a file of its own, beside the addressing this
/// plane SERVES; re-exported whole, so what is announced and what is served are read from one
/// module.
mod announced_address;
pub use announced_address::*;
