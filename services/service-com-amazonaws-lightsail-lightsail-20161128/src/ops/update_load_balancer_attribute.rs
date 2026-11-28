// SPDX-License-Identifier: CC0-1.0

//! @op UpdateLoadBalancerAttribute
//! Writes ONE of a stored load balancer's configuration options from the value
//! the request carries for the attribute it names, so every later read of that
//! load balancer reports it, and answers the operation-log entry the change
//! appended.
//!
//! This is the ONLY file to touch for this operation.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateLoadBalancerAttribute`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json")
//! ```
//!
//! Updates the specified attribute for a load balancer. You can only update one attribute
//! at a time. The update load balancer attribute operation supports tag-based access
//! control via resource tags applied to the resource identified by load balancer name. For
//! more information, see the Amazon Lightsail Developer Guide.
//!
//! ## Input
//!
//! * `attributeName` (LoadBalancerAttributeName, one of: HealthCheckPath |
//!   HttpsRedirectionEnabled | SessionStickinessEnabled |
//!   SessionStickiness_LB_CookieDurationSeconds | TlsPolicyName) **required** — The name of
//!   the attribute you want to update.
//! * `attributeValue` (StringMax256, len 1..256) **required** — The value that you want to
//!   specify for the attribute name. The following values are supported depending on what you
//!   specify for the attributeName request parameter: If you specify HealthCheckPath for the
//!   attributeName request parameter, then the attributeValue request parameter must be the
//!   path to ping on the target (for example, /weather/us/wa/seattle). If you specify
//!   SessionStickinessEnabled for the attributeName request parameter, then the
//!   attributeValue request parameter must be true to activate session stickiness or false to
//!   deactivate session stickiness. If you specify SessionStickiness_LB_CookieDurationSeconds
//!   for the attributeName request parameter, then the attributeValue request …
//! * `loadBalancerName` (ResourceName, pattern ^\w[\w\-]*\w$) **required** — The name of the
//!   load balancer that you want to modify (my-load-balancer.
//!
//! ## Output
//!
//! * `operations` (OperationList) — An array of objects that describe the result of the
//!   action, such as the status of the request, the timestamp of the request, and the
//!   resources affected by the request.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Lightsail throws this exception when the user
//!   cannot be authenticated or uses invalid credentials to access a resource.
//! * `AccountSetupInProgressException` (HTTP 428) — Lightsail throws this exception when an
//!   account is still in the setup in progress state.
//! * `InvalidInputException` (HTTP 400) — Lightsail throws this exception when user input
//!   does not conform to the validation rules of an input field. Domain and distribution APIs
//!   are only available in the N. Virginia (us-east-1) Amazon Web Services Region. Please set
//!   your Amazon Web Services Region configuration to us-east-1 to create, view, or edit
//!   these resources.
//! * `NotFoundException` (HTTP 404) — Lightsail throws this exception when it cannot find a
//!   resource.
//! * `OperationFailureException` (HTTP 400) — Lightsail throws this exception when an
//!   operation fails to execute.
//! * `RegionSetupInProgressException` (HTTP 428) — Lightsail throws this exception when an
//!   operation is performed on resources in an opt-in Region that is currently being set up.
//! * `ServiceException` (HTTP 500) — A general service exception.
//! * `UnauthenticatedException` (HTTP 401) — Lightsail throws this exception when the user
//!   has not been authenticated.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateLoadBalancerAttributeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lightsail-lightsail-20161128.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateLoadBalancerAttributeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lightsail#UpdateLoadBalancerAttribute")]
#[derive(Debug)]
pub enum UpdateLoadBalancerAttributeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AccountSetupInProgressException(AccountSetupInProgressException),
    InvalidInputException(InvalidInputException),
    NotFoundException(NotFoundException),
    OperationFailureException(OperationFailureException),
    RegionSetupInProgressException(RegionSetupInProgressException),
    ServiceException(ServiceException),
    UnauthenticatedException(UnauthenticatedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_load_balancer_attribute::UpdateLoadBalancerAttributeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateLoadBalancerAttributeError = crate::scaffolded::Scaffolded<UpdateLoadBalancerAttributeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateLoadBalancerAttributeErrorModeled> for UpdateLoadBalancerAttributeError {
    fn from(modeled: UpdateLoadBalancerAttributeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateLoadBalancerAttribute` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateLoadBalancerAttribute<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateLoadBalancerAttribute<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateLoadBalancerAttribute<Eng>
{
    type Input = UpdateLoadBalancerAttributeInput;
    type Output = UpdateLoadBalancerAttributeOutput;
    type Error = UpdateLoadBalancerAttributeError;

    const NAME: &'static str = "UpdateLoadBalancerAttribute";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_LOAD_BALANCER_ATTRIBUTE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_LOAD_BALANCER_ATTRIBUTE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A load balancer is a REGIONAL resource, so the Region the request is
        // signed for is part of the record this call addresses — the same
        // lookup `GetLoadBalancer` and `CreateLoadBalancer` use.
        let region = crate::state::request_region(ctx.parts());

        // `loadBalancerName`, `attributeName` and `attributeValue` are modelled
        // `@required` members, the name carries the modelled `ResourceName`
        // pattern, the attribute name targets the modelled
        // `LoadBalancerAttributeName` enum and the value the modelled
        // `StringMax256` length bound, so the centralized walk refused anything
        // outside them before this body ran. What is left is what the model
        // cannot express: `attributeValue` is ONE string whatever the attribute
        // is, and each attribute documents its own value domain, so the value
        // has to be read out of that string as a value of THAT domain. It is
        // resolved BEFORE the store is reached, so a value the attribute does
        // not admit changes nothing.
        let name = input.load_balancer_name;
        let attribute = parsed_attribute(&input.attribute_name, &input.attribute_value)?;
        let now = crate::state::now_ms();

        crate::state::with_state(&*self.engine, |store| {
            let Some(balancer) = store.load_balancer_mut(&region, &name) else {
                // The load balancer is the resource this operation acts on, and
                // the one tag-based access control is evaluated against
                // ("supports tag-based access control via resource tags applied
                // to the resource identified by load balancer name").
                return Err(not_found(
                    crate::state::RESOURCE_TYPE_LOAD_BALANCER,
                    &name,
                ));
            };
            // "You can only update one attribute at a time": exactly the one
            // the request named is written, and every other configuration
            // option keeps the value it already had.
            balancer.set_attribute(attribute);
            let location = balancer.location.clone();

            // `operations` is a list because a Lightsail call can act on several
            // resources at once; this one acts on the single load balancer it
            // was given, so it answers exactly one entry, left non-terminal
            // until the clock settles the reconfiguration it started.
            let operation = store.start_operation(
                now,
                crate::state::OPERATION_TYPE_UPDATE_LOAD_BALANCER_ATTRIBUTE,
                crate::state::RESOURCE_TYPE_LOAD_BALANCER,
                &name,
                &location,
                Some(now.saturating_add(crate::state::STATE_TRANSITION_MILLIS)),
            );
            Ok(UpdateLoadBalancerAttributeOutput {
                operations: Some(vec![operation.view()]),
            })
        })
        .ok_or_else(unavailable)?
    }
}

/// The value `value` describes for the attribute `attribute_name`, or this
/// operation's refusal when it describes none.
///
/// One arm per member of the modelled `LoadBalancerAttributeName` enum, each
/// reading `value` as the domain that member's own `attributeValue`
/// documentation states.
fn parsed_attribute(
    attribute_name: &str,
    value: &str,
) -> Result<crate::state::LoadBalancerAttribute, UpdateLoadBalancerAttributeError> {
    use crate::state::LoadBalancerAttribute;

    match attribute_name {
        // "must be the path to ping on the target (for example,
        // /weather/us/wa/seattle)" — and `LoadBalancer$healthCheckPath` is a
        // `NonEmptyString`, so a path made only of whitespace is not one any
        // read of this load balancer could answer.
        crate::state::LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH => {
            let Some(path) = crate::state::health_check_path_value(value) else {
                return Err(invalid(format!(
                    "attributeValue '{value}' is not a value of {attribute_name}: it must be \
                     the non-whitespace path to ping on the target"
                )));
            };
            Ok(LoadBalancerAttribute::HealthCheckPath(path))
        }
        // "must be true to activate session stickiness or false to deactivate
        // session stickiness".
        crate::state::LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_ENABLED => Ok(
            LoadBalancerAttribute::SessionStickinessEnabled(boolean_value(
                attribute_name,
                value,
                "session stickiness",
            )?),
        ),
        // "must be an integer that represents the cookie duration in seconds",
        // within the range the load-balancing attribute Lightsail surfaces here
        // admits.
        crate::state::LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_COOKIE_DURATION => {
            let seconds: i64 = value.trim().parse().map_err(|_| {
                invalid(format!(
                    "attributeValue '{value}' is not a value of {attribute_name}: it must be \
                     an integer that represents the cookie duration in seconds"
                ))
            })?;
            if !(crate::state::MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS
                ..=crate::state::MAX_SESSION_STICKINESS_COOKIE_DURATION_SECONDS)
                .contains(&seconds)
            {
                return Err(invalid(format!(
                    "attributeValue '{value}' is not a value of {attribute_name}: the cookie \
                     duration ranges from {} second to {} seconds",
                    crate::state::MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS,
                    crate::state::MAX_SESSION_STICKINESS_COOKIE_DURATION_SECONDS
                )));
            }
            Ok(LoadBalancerAttribute::SessionStickinessCookieDurationSeconds(seconds))
        }
        // "must be true to activate HTTP to HTTPS redirection or false to
        // deactivate HTTP to HTTPS redirection".
        crate::state::LOAD_BALANCER_ATTRIBUTE_HTTPS_REDIRECTION_ENABLED => Ok(
            LoadBalancerAttribute::HttpsRedirectionEnabled(boolean_value(
                attribute_name,
                value,
                "HTTP to HTTPS redirection",
            )?),
        ),
        // "must be the name of the TLS policy. Use the
        // GetLoadBalancerTlsPolicies action to get a list of TLS policy names
        // that you can specify."
        crate::state::LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME => {
            let policy = value.trim();
            if !crate::state::publishes_load_balancer_tls_policy(policy) {
                return Err(invalid(crate::state::unpublished_tls_policy_message(
                    "attributeValue",
                    value,
                )));
            }
            Ok(LoadBalancerAttribute::TlsPolicyName(policy.to_string()))
        }
        // Unreachable through the wire: `attributeName` targets the modelled
        // enum, so the boundary walk refused every other value before this ran.
        // Named rather than ignored so an attribute the model gains is refused
        // as unsupported instead of silently writing nothing.
        other => Err(invalid(format!(
            "attributeName '{other}' is not an attribute this service updates"
        ))),
    }
}

/// `value` read as the boolean the attribute `attribute_name` documents, whose
/// refusal names `activates` — what sending `true` turns on.
///
/// The value is read ignoring the whitespace around it, the same way the path
/// and the cookie duration are: one whitespace rule for every attribute.
fn boolean_value(
    attribute_name: &str,
    value: &str,
    activates: &str,
) -> Result<bool, UpdateLoadBalancerAttributeError> {
    value.trim().parse::<bool>().map_err(|_| {
        invalid(format!(
            "attributeValue '{value}' is not a value of {attribute_name}: it must be true to \
             activate {activates} or false to deactivate {activates}"
        ))
    })
}

/// This operation's 400 refusal for a request the model admits but the service
/// does not: the modelled `InvalidInputException`, whose message names the member
/// that is wrong and the constraint it broke.
fn invalid(message: String) -> UpdateLoadBalancerAttributeError {
    UpdateLoadBalancerAttributeErrorModeled::InvalidInputException(crate::scaffolded::invalid_input(message))
    .into()
}

/// The request names a resource this account does not hold: Lightsail's
/// `DoesNotExist` rejection — HTTP 400, wire error `NotFoundException`, body
/// `code` `DoesNotExist` and a message naming the resource type and the
/// requested name.
fn not_found(resource_type: &str, name: &str) -> UpdateLoadBalancerAttributeError {
    crate::scaffolded::Scaffolded::resource_does_not_exist(resource_type, name)
}

/// The store is unreachable — the engine is not the shared in-memory one. A
/// modelled server fault, never a fresh engine.
fn unavailable() -> UpdateLoadBalancerAttributeError {
    UpdateLoadBalancerAttributeErrorModeled::ServiceException(ServiceException {
        message: Some("the Lightsail load balancer store is unavailable".to_string()),
        ..Default::default()
    })
    .into()
}
