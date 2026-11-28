// SPDX-License-Identifier: CC0-1.0

//! @op TestInvokeAuthorizer
//! IMPLEMENTED: simulates the execution of one stored `Authorizer` of an api against the
//! request the caller describes — the headers it carries, the path and query string it
//! addresses, the body it sends and the stage variables it would be served under — and
//! reports the DECISION that execution produced: the status a client would have received
//! (`0` when the authorizer authorized the call), the principal the authorizer named, the
//! policy document it returned, the values it published, the claims of an identity token it
//! accepted, the execution log of the simulation and the latency it took. A `restApiId` or
//! an `authorizerId` no record of this account carries is `NotFoundException`.
//!
//! ## What a simulated authorizer execution is
//!
//! The authorizer is EXECUTED, by the same steps a request to a deployed stage executes it
//! by — the identity source read off the request, a `COGNITO_USER_POOLS` authorizer's
//! presented token verified against the user pools it names, a `TOKEN` or `REQUEST`
//! authorizer's Lambda function INVOKED with the event its own type documents, and the policy
//! it returned applied to the method the request addresses. All of that is
//! [`crate::service::invoke::simulated_authorizer`], the module the data plane shares every
//! one of those steps with, so a simulated execution and a request to a deployed stage cannot
//! answer one stored authorizer differently.
//!
//! The reported `clientStatus` is "the HTTP status code that the client would have received.
//! Value is 0 if the authorizer succeeded", and an authorizer SUCCEEDED once it produced a
//! decision: its function ran and returned an authorizer output, or the identity token it
//! evaluates was accepted. Whether the policy that decision carries admits the method is the
//! decision's CONTENT, not a failure of the authorizer, so a returned deny is reported at `0`
//! beside the document that denied it. A non-zero status is for a call that earned no decision
//! at all — an identity source the request did not carry, an authorizer that could not be
//! asked, that did not answer, that answered with something that is not an authorizer output,
//! or a presented token it did not accept — and it is then the status of the GATEWAY RESPONSE
//! that refused request would have been answered through, including a developer's
//! customization of that response type, read through the one renderer of a refusal
//! ([`crate::service::invoke::refusal_rendering`]).
//!
//! What the authorizer RETURNED is reported whichever way it decided: a developer testing an
//! authorizer that denies the call is testing the very document that denied it, so the
//! principal, the policy and the published values are reported beside the execution log naming
//! the gateway response that call would have met.
//!
//! `claims` is "the open identity claims … returned from the Cognito Your User Pool configured
//! for the API", so the member is served only by a `COGNITO_USER_POOLS` authorizer that
//! accepted an identity token and is ABSENT for a `TOKEN` or a `REQUEST` authorizer, whose
//! verdict is a function's return value: what such a function published reaches the caller
//! through `authorization` and the log.
//!
//! ## Which method the simulation addresses
//!
//! This operation models no `httpMethod` and no `resourceId`: its only request-bound members
//! are the headers, `pathWithQueryString`, the body, the stage variables and the additional
//! context. So the ARN the authorizer is handed, and the ARN its returned policy is judged
//! against, is the api's own method ARN under the stage AWS reserves for a SIMULATED
//! AUTHORIZER EXECUTION ([`TEST_INVOKE_ARN_STAGE`]) — naming the [`SIMULATED_VERB`] of the
//! path the caller described, which is the request `pathWithQueryString` alone can describe.
//!
//! ## What it does not change
//!
//! Nothing. The authorizer's result is not HELD: a simulation reaches no stage, so there is
//! no stage whose `authorizerResultTtlInSeconds` could keep it and no later request to any
//! stage is answered differently for this call having been made. The authorizer's own
//! function is invoked, which is what this operation is for — "simulate the execution of an
//! Authorizer" — and that invocation is the only thing a simulation causes.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TestInvokeAuthorizer::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `TestInvokeAuthorizer`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")
//! ```
//!
//! Simulate the execution of an Authorizer in your RestApi with headers, parameters, and an
//! incoming request body.
//!
//! ## Input
//!
//! * `additionalContext` (MapOfStringToString) — A key-value map of additional context
//!   variables.
//! * `authorizerId` (String) **required** — Specifies a test invoke authorizer request's
//!   Authorizer ID.
//! * `body` (String) — The simulated request body of an incoming invocation request.
//! * `headers` (MapOfStringToString) — A key-value map of headers to simulate an incoming
//!   invocation request. This is where the incoming authorization token, or identity source,
//!   should be specified.
//! * `multiValueHeaders` (MapOfStringToList) — The headers as a map from string to list of
//!   values to simulate an incoming invocation request. This is where the incoming
//!   authorization token, or identity source, may be specified.
//! * `pathWithQueryString` (String) — The URI path, including query string, of the simulated
//!   invocation request. Use this to specify path parameters and query string parameters.
//! * `restApiId` (String) **required** — The string identifier of the associated RestApi.
//! * `stageVariables` (MapOfStringToString) — A key-value map of stage variables to simulate
//!   an invocation on a deployed Stage.
//!
//! ## Output
//!
//! * `authorization` (MapOfStringToList) — The authorization response.
//! * `claims` (MapOfStringToString) — The open identity claims, with any supported custom
//!   attributes, returned from the Cognito Your User Pool configured for the API.
//! * `clientStatus` (Integer, default 0) **required** — The HTTP status code that the client
//!   would have received. Value is 0 if the authorizer succeeded.
//! * `latency` (Long, default 0) **required** — The execution latency, in ms, of the test
//!   authorizer request.
//! * `log` (String) — The API Gateway execution log for the test authorizer request.
//! * `policy` (String) — The JSON policy document returned by the Authorizer
//! * `principalId` (String) — The principal identity returned by the Authorizer
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The submitted request is not valid, for example, the
//!   input is incomplete or incorrect. See the accompanying error message for details.
//! * `NotFoundException` (HTTP 404) — The requested resource is not found. Make sure that the
//!   request URI is correct.
//! * `TooManyRequestsException` (HTTP 429) — The request has reached its throttling limit.
//!   Retry after the specified time period.
//! * `UnauthorizedException` (HTTP 401) — The request is denied because the caller has
//!   insufficient permissions.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, HashMap};

use crate::service::invoke::lambda_proxy::ProxyInputs;
use crate::service::invoke::{simulated_authorizer, simulated_request, MethodRequest};
use crate::service::mapping_template::TemplateInputs;

use super::test_invoke_method::{ExecutionLog, TEST_INVOKE_STAGE};

/// The verb the ARN of a simulated authorizer execution names.
///
/// This operation models no HTTP method and no resource of the api, so there is no method of
/// it for the simulation to address: what the authorizer is handed, and what its returned
/// policy is judged against, is the path the caller described under the test-invoke stage,
/// READ — the one request `pathWithQueryString` by itself describes, and the verb AWS's own
/// method ARN for a test invocation of an authorizer carries.
const SIMULATED_VERB: &str = "GET";

/// The stage segment of the method ARN API Gateway SYNTHESIZES for a simulated authorizer
/// execution: the reserved test-invoke stage, spelled `ESTestInvoke-stage`.
///
/// This operation addresses no method of the api, so the ARN handed to the authorizer as
/// `methodArn`, which its returned policy names in turn, is
/// `arn:aws:execute-api:{region}:{account}:{api-id}/ESTestInvoke-stage/{verb}/{path}`. It is a
/// spelling of its own, distinct from the stage a test invocation's REQUEST belongs to
/// ([`TEST_INVOKE_STAGE`], which `$context.stage` reports and which the `aws:SourceArn` of a
/// grant for a simulated METHOD invocation is narrowed by), so a function whose policy is
/// scoped to the ARN AWS hands it decides a simulation here as it decides one there.
pub(crate) const TEST_INVOKE_ARN_STAGE: &str = "ESTestInvoke-stage";

/// The path a simulated request that names none addresses: the api's root resource.
const ROOT_PATH: &str = "/";

/// The simulated request header that declares the media type of the body, as
/// `Integration$requestTemplates` keys one.
const CONTENT_TYPE: &str = "content-type";

/// The simulated request header that declares the media types the caller accepts.
const ACCEPT: &str = "accept";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TestInvokeAuthorizerError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestInvokeAuthorizerError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.apigateway#TestInvokeAuthorizer")]
#[derive(Debug)]
pub enum TestInvokeAuthorizerErrorModeled {
    BadRequestException(BadRequestException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_invoke_authorizer::TestInvokeAuthorizerError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestInvokeAuthorizerError = crate::scaffolded::Scaffolded<TestInvokeAuthorizerErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestInvokeAuthorizerErrorModeled> for TestInvokeAuthorizerError {
    fn from(modeled: TestInvokeAuthorizerErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TestInvokeAuthorizer` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestInvokeAuthorizer<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestInvokeAuthorizer<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestInvokeAuthorizer<Eng>
{
    type Input = TestInvokeAuthorizerInput;
    type Output = TestInvokeAuthorizerOutput;
    type Error = TestInvokeAuthorizerError;

    const NAME: &'static str = "TestInvokeAuthorizer";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_INVOKE_AUTHORIZER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_INVOKE_AUTHORIZER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `latency` is "the execution latency, in ms, of the test authorizer request", so it
        // is MEASURED over the simulation rather than stated: the clock is started before the
        // records are read and read once the decision is in hand.
        let started = ::std::time::Instant::now();
        // WHAT is simulated, read under ONE read of the store and taken off it as OWNED
        // records: the api (what a refusal's status is rendered from) and the authorizer the
        // request names. The store is released with that read, because asking a Lambda
        // authorizer means CALLING another service in process and no lock of this service's
        // is held while another service answers.
        let (api, authorizer) = {
            let state = crate::state::service_state(&*self.engine)
                .ok_or_else(crate::state::state_unavailable)?;
            let store = state.lock();
            let api = store
                .rest_apis
                .get(&input.rest_api_id)
                .ok_or_else(|| not_found(&crate::state::no_such_rest_api(&input.rest_api_id)))?;
            let authorizer = api
                .authorizers
                .get(&input.authorizer_id)
                .ok_or_else(|| not_found(crate::state::NO_SUCH_AUTHORIZER))?;
            (api.clone(), authorizer.clone())
        };

        // What the simulated request carries, read by the one reader of a simulated
        // request's members ([`simulated_request`]), which is what the sibling simulation
        // reads its own request through.
        let headers = simulated_request::SimulatedHeaders::of(
            input.headers.as_ref(),
            input.multi_value_headers.as_ref(),
        );
        let (path, query) =
            simulated_request::split_query(input.path_with_query_string.as_deref());
        let request_path = path.unwrap_or(ROOT_PATH);
        // The ARN of the method this simulation addresses; see the module docs for why it is
        // the test-invoke stage's and why the verb is [`SIMULATED_VERB`]. Built by the one
        // builder of a method ARN, so the ARN an authorizer is handed here has the form every
        // other reader of this service builds.
        let method_arn = crate::service::resource_policy::Call {
            region: api.arn_region(),
            api_id: &input.rest_api_id,
            stage: TEST_INVOKE_ARN_STAGE,
            verb: SIMULATED_VERB,
            resource_path: request_path,
            request_path,
            source_vpce: None,
            // A SIMULATED call arrived over no connection, so it carries no address for a
            // policy clause to be decided against.
            source_ip: None,
            caller: crate::service::resource_policy::Caller::Anonymous,
        }
        .method_arn();

        let mut log = ExecutionLog::started();
        log.line(format!(
            "Authorizer: {id}, type: {kind}, identity source: {source}",
            id = authorizer.id,
            kind = authorizer.r#type,
            source = authorizer.identity_source.as_deref().unwrap_or("none"),
        ));
        log.line(format!("Method request path: {request_path}"));
        // Stated whatever the simulated request carried, through the one rendering of a
        // parameter map the sibling `TestInvokeMethod` states one with: a request that named
        // no query string states the EMPTY map rather than leaving the step out of the log.
        log.line(format!(
            "Method request query string: {}",
            super::test_invoke_method::stated_parameters(
                &crate::service::invoke::sent_query_parameters(query.unwrap_or_default())
            )
        ));
        log.line(crate::service::invoke::stage_logs::traced_header_names_line(
            &headers.names(),
        ));
        // The method request AS THE CALLER DESCRIBED IT, at the first transformation point an
        // execution log states, through the one line builder a payload is stated with
        // ([`crate::service::invoke::stage_logs::traced_payload_line`]) — so the two
        // `TestInvoke*` operations word and bound a transformation point alike. The log is
        // this operation's answer about the authorizer it was asked to exercise and travels
        // back to the caller that supplied the body, so stating it discloses nothing that
        // caller does not hold.
        log.line(crate::service::invoke::stage_logs::traced_payload_line(
            "Method request body before transformations",
            input.body.as_deref().unwrap_or_default().as_bytes(),
        ));
        let stage_variables = simulated_request::map_names(input.stage_variables.as_ref());
        if !stage_variables.is_empty() {
            log.line(format!("Stage variables: [{}]", stage_variables.join(", ")));
        }
        // `additionalContext` is "a key-value map of additional context variables" the caller
        // simulates the request with. The event an authorizer is handed is built by the one
        // builder this data plane hands an authorizer or a proxy integration, whose members
        // are read off the method request itself, so these variables name no member of it:
        // the log reports which ones the simulation carried, and nothing is invented under a
        // member AWS does not document them in.
        let additional_context = simulated_request::map_names(input.additional_context.as_ref());
        if !additional_context.is_empty() {
            log.line(format!(
                "Additional context: [{}]",
                additional_context.join(", ")
            ));
        }

        // What the simulated request IS, in the two views the shared machinery reads it
        // through: the members a mapping expression names (which is what an
        // `identitySource` is read from) and the members a `REQUEST` authorizer's event
        // describes the call by.
        let simulated_variables: BTreeMap<String, String> = input
            .stage_variables
            .iter()
            .flatten()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let inputs = TemplateInputs {
            payload: Some(
                input
                    .body
                    .as_deref()
                    .unwrap_or_default()
                    .as_bytes()
                    .to_vec(),
            ),
            // A simulation addresses no resource of the api, so there is no resource
            // template for a path parameter to be bound against.
            path: BTreeMap::new(),
            querystring: crate::service::invoke::sent_query_parameters(query.unwrap_or_default()),
            header: headers.single_valued(),
            stage_variables: simulated_variables,
            context: BTreeMap::from([
                ("apiId".to_string(), input.rest_api_id.clone()),
                // The stage a test invocation's REQUEST belongs to, which is what
                // `$context.stage` stands for — the reserved name of the test-invoke stage
                // itself, not the stage segment the synthesized method ARN carries
                // ([`TEST_INVOKE_ARN_STAGE`]).
                ("stage".to_string(), TEST_INVOKE_STAGE.to_string()),
                ("httpMethod".to_string(), SIMULATED_VERB.to_string()),
                ("resourcePath".to_string(), request_path.to_string()),
                ("path".to_string(), request_path.to_string()),
            ]),
        };
        let call = MethodRequest::simulated(
            headers.value(CONTENT_TYPE).as_deref(),
            headers.value(ACCEPT).as_deref(),
            inputs,
            ProxyInputs {
                // A simulation addresses no resource, so the event names none.
                resource_id: String::new(),
                http_method: SIMULATED_VERB.to_string(),
                request_path: request_path.to_string(),
                domain_name: String::new(),
                multi_value_querystring: simulated_request::every_query_parameter(query),
                multi_value_header: headers.every_value(),
                // The `aws:SourceArn` a grant for an AUTHORIZER's function is narrowed by is
                // the AUTHORIZER's own ARN rather than a method's, which is what the
                // simulation judges the invocation by
                // ([`crate::service::invoke::simulated_authorizer`]); this names the method
                // the simulated request addresses, for the event that reports it.
                source_arn: Some(method_arn.clone()),
                binary_media_types: api.binary_media_types.clone(),
                // A simulation reached no stage, so there is no stage whose tracing setting
                // could decide a sampling decision the caller did not take.
                tracing_enabled: false,
            },
        );

        let simulation = simulated_authorizer::simulated(
            ctx,
            &*self.engine,
            &api,
            &authorizer,
            &call,
            &method_arn,
        )
        .await;
        for step in &simulation.steps {
            log.line(step.clone());
        }
        // "The HTTP status code that the client would have received. Value is 0 if the
        // authorizer succeeded": an authorizer that produced a DECISION succeeded, whichever
        // way its policy decided, so the status is `0` for it and the gateway response a
        // refused request would have met is what the log reports. Only a call that earned no
        // decision carries that response's own status, rendered from THIS api's own records so
        // a developer's customization of the response type is what the report carries.
        let client_status = match simulation.refusal {
            None => {
                log.line("Successfully completed execution".to_string());
                0
            }
            Some(refusal) => {
                let response_type = refusal.response_type();
                let status = crate::service::invoke::refusal_rendering(Some(&api), refusal).status;
                log.line(format!(
                    "Answered through this API's {response_type} gateway response: the client \
                     would have received HTTP {status}"
                ));
                if simulation.decided {
                    0
                } else {
                    i32::from(status)
                }
            }
        };

        Ok(TestInvokeAuthorizerOutput {
            client_status,
            log: Some(log.rendered()),
            latency: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            principal_id: simulation.principal_id,
            policy: simulation.policy,
            // "The authorization response": the values the authorizer PUBLISHED beside its
            // policy — the `context` map a mapping template reads as
            // `$context.authorizer.{key}` — in the multi-valued shape the member is modelled
            // as. An authorizer that published none reports none.
            authorization: Some(
                simulation
                    .context
                    .into_iter()
                    .map(|(key, value)| (key, vec![value]))
                    .collect::<HashMap<String, Vec<String>>>(),
            ),
            // "The open identity claims … returned from the Cognito Your User Pool configured
            // for the API": the member is a user-pool authorizer's own, so it is served only
            // where such an authorizer accepted an identity token and is absent for every
            // authorizer that evaluates none.
            claims: simulation
                .claims
                .map(|claims| claims.into_iter().collect::<HashMap<String, String>>()),
        })
    }
}

/// The modelled 404 this operation answers for an identifier no record of this account
/// carries, worded as every other reader of the same records words it.
fn not_found(message: &str) -> TestInvokeAuthorizerError {
    TestInvokeAuthorizerErrorModeled::NotFoundException(NotFoundException {
        message: Some(message.to_string()),
    })
    .into()
}
