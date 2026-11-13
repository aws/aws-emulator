// SPDX-License-Identifier: CC0-1.0

//! @op CreateEnvironmentConnector
//! Creates one connector in a stored Amazon EVS environment and reports the record it
//! persisted, which [`crate::ops::list_environment_connectors`] reads back and
//! [`crate::ops::update_environment_connector`] reconfigures. An `environmentId` this
//! service holds no live environment for in the request's Region is a
//! `ResourceNotFoundException`, and nothing is stored for it.
//!
//! The connector starts `CREATING` and the clock owes it the move to `ACTIVE`: "A
//! connector allows the Amazon EVS control plane to interface with VCF appliances
//! using a fully qualified domain name", which is a connection to establish rather
//! than a record to write.
//!
//! "You can create only one connector of each type per environment", so the
//! `ConnectorType` is the identity of a connector within its environment. The
//! operation is `@idempotent`
//! (`smithy_query(query="operation",operation="CreateEnvironmentConnector")`): a
//! repeat call naming a type the environment already holds a connector of reports that
//! stored record when the request asks for the same appliance and secret, and is
//! refused with the modelled `ValidationException` when it asks for different ones —
//! the second connector of a type cannot be created, and silently reconfiguring the
//! first is `UpdateEnvironmentConnector`'s work, not a create's. "The same appliance and
//! secret" is what the request NAMES, not how it spelled it: the appliance is matched as
//! the domain name an `applianceFqdn` is ([`crate::state::same_dns_name`]) and the
//! secret as the ARN its `secretIdentifier` resolves to
//! ([`crate::state::secret_resources::resolve_secret_arn`]).
//!
//! The record a repeat reports is the one the caller is handed as the created
//! connector, so it is reported only while it is still a connector a later call
//! resolves ([`crate::state::StoredConnector::is_live`]): a connector whose teardown
//! this service has already accepted is on its way to `DELETED` and resolves to
//! nothing, so the type it still occupies ([`crate::state::live_connector_of_type`]) is
//! the conflict above whatever metadata the request names.
//!
//! The credential secret is named on input by ARN OR by name (`secretIdentifier`) and
//! reported through `Connector$secretArn`, so the identifier is resolved to that
//! secret's ARN before it is stored or compared
//! ([`crate::state::secret_resources::resolve_secret_arn`]): the record this operation
//! reports, and every later read of it, carries an ARN.
//!
//! A connector is only ever created against credentials it can really connect with: "A
//! connector establishes a connection to the given appliance type using the credentials
//! stored in Amazon Web Services Secrets Manager" (`Connector` doc), so the secret the
//! request names has to be one the account holds and to store the fields this connector
//! type requires ("SDDC_MANAGER requires an apiKey field; OPERATIONS_MANAGER and VCENTER
//! require username and password fields", `secretIdentifier` doc). Both are resolved in
//! Secrets Manager's own store ([`crate::state::secret_resources`]), the way the network
//! an environment names is resolved in Amazon EC2's, and a request failing either is
//! refused with the modelled `ValidationException` naming `secretIdentifier` — nothing is
//! stored for it, so no connector reaches `ACTIVE` on credentials that are not there.
//!
//! The SDDC_MANAGER connector Amazon EVS creates automatically "for environments where
//! Amazon EVS installs VCF" is deliberately NOT minted by `CreateEnvironment` here:
//! its appliance FQDN and its credential secret are produced by the VCF installation,
//! which this emulator does not run, so both values would have to be invented.
//!
//! Like every implemented operation of this service, the request is authorized before
//! the environment it names is looked up ([`crate::state::authorization`]).
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEnvironmentConnector::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `CreateEnvironmentConnector`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")
//! ```
//!
//! Creates a connector for an Amazon EVS environment. A connector allows the Amazon EVS
//! control plane to interface with VCF appliances using a fully qualified domain name. You
//! can create only one connector of each type per environment. For environments where
//! Amazon EVS installs VCF, the SDDC_MANAGER connector is created automatically. Amazon EVS
//! requires an active connector to SDDC Manager or VCF Operations Manager to monitor
//! environment health and license compliance.
//!
//! ## Input
//!
//! * `applianceFqdn` (ApplianceFqdn, len 1..253, pattern
//!   ^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$)
//!   **required** — The fully qualified domain name (FQDN) of the VCF appliance that the
//!   connector targets.
//! * `clientToken` (ClientToken, len 1..100, pattern ^[!-~]+$, @idempotencyToken) — This
//!   parameter is not used in Amazon EVS currently. If you supply input for this parameter,
//!   it will have no effect. A unique, case-sensitive identifier that you provide to ensure
//!   the idempotency of the connector creation request. If you do not specify a client token,
//!   a randomly generated token is used for the request to ensure idempotency.
//! * `environmentId` (EnvironmentId, pattern ^(env-[a-zA-Z0-9]{10})$) **required** — A unique
//!   ID for the environment to create the connector in.
//! * `secretIdentifier` (SecretIdentifier, len 1..2048, pattern
//!   ^(arn:aws:secretsmanager:[a-z0-9-]+:[0-9]{12}:secret:[a-zA-Z0-9/_+=.@!-]+|[a-zA-Z0-9/_+=.@!-]+)$)
//!   **required** — The ARN or name of the Amazon Web Services Secrets Manager secret that
//!   stores the credentials for the VCF appliance. SDDC_MANAGER requires an apiKey field;
//!   OPERATIONS_MANAGER and VCENTER require username and password fields. Do not use
//!   credentials with Administrator privileges. We recommend using a service account with
//!   read-only permissions.
//! * `type` (ConnectorType, one of: OPERATIONS_MANAGER | SDDC_MANAGER | VCENTER) **required**
//!   — The type of connector to create. OPERATIONS_MANAGER: Connector to an Operations
//!   Manager appliance. Required for VCF 9x environments. SDDC_MANAGER: Connector to an SDDC
//!   Manager appliance. Required for VCF 5.x environments. VCENTER: Connector to a vCenter
//!   Server appliance. Required for features that depend on vCenter, such as Windows Server
//!   license-included.
//!
//! ## Output
//!
//! * `connector` (Connector) — A description of the created connector.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — A service resource associated with the request
//!   could not be found. The resource might not be specified correctly, or it may have a
//!   state of DELETED.
//! * `ThrottlingException` (HTTP 429) — The operation could not be performed because the
//!   service is throttling requests. This exception is thrown when the service endpoint
//!   receives too many concurrent requests.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the specified constraints.
//!   You will see this exception if invalid inputs are provided for any of the Amazon EVS
//!   environment operations, or if a list operation is performed on an environment resource
//!   that is still initializing.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEnvironmentConnectorError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEnvironmentConnectorError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.evs#CreateEnvironmentConnector",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum CreateEnvironmentConnectorErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
    /// Widened in from the service's own error set (this operation does not list it):
    /// the one honest answer when this service's store cannot be reached at all.
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_environment_connector::CreateEnvironmentConnectorError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEnvironmentConnectorError = crate::scaffolded::Scaffolded<CreateEnvironmentConnectorErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEnvironmentConnectorErrorModeled> for CreateEnvironmentConnectorError {
    fn from(modeled: CreateEnvironmentConnectorErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEnvironmentConnector` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEnvironmentConnector<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEnvironmentConnector<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEnvironmentConnector<Eng>
{
    type Input = CreateEnvironmentConnectorInput;
    type Output = CreateEnvironmentConnectorOutput;
    type Error = CreateEnvironmentConnectorError;

    const NAME: &'static str = "CreateEnvironmentConnector";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_CONNECTOR_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_CONNECTOR_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEnvironmentConnectorErrorModeled::InternalServerException(
                InternalServerException {
                    message: "The Amazon EVS environment store is unavailable.".to_string(),
                },
            )
        })?;
        // `clientToken` is read and ignored on purpose: "This parameter is not used in
        // Amazon EVS currently. If you supply input for this parameter, it will have no
        // effect." (`clientToken` doc). The operation is idempotent through the stored
        // records instead — see the same-type match below.
        let _ = &input.client_token;
        let region = crate::state::request_region(ctx.parts());
        // Authorization first: AWS decides it at the IAM layer, before the resource the
        // request names is evaluated, so the ARN is the one the REQUEST names.
        if let Some(denied) = crate::state::authorization::denied(
            &*self.engine,
            ctx,
            Self::NAME,
            Some(&crate::state::environment_arn(&region, &input.environment_id)),
        ) {
            return Err(denied);
        }
        // `secretIdentifier` is "The ARN or name of" the credential secret while the
        // member it feeds, `Connector$secretArn`, is "The Amazon Resource Name (ARN) of"
        // it: the identifier is resolved to that secret's ARN before anything compares or
        // stores it, so one secret has one `secretArn` however the request named it.
        let secret_arn = crate::state::secret_resources::resolve_secret_arn(
            &region,
            &input.secret_identifier,
        );
        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so the environment and its
        // connectors are in the state that is due at this instant before the create is
        // evaluated.
        state.materialise_due_transitions();
        // One instant for everything this call changes, so the new connector record and
        // the environment whose inventory it joins are stamped alike. Read with the store
        // lock HELD and after the materialisation above: the connector is added at this
        // instant, not at the one the request arrived at, so its stamp cannot predate a
        // transition the same call just materialised onto the environment and its VLANs —
        // and the re-stamp of the environment below is not dropped by the guard that keeps
        // a stamp from going backwards.
        let created_at_ms = crate::state::now_ms();
        let connector_id = state.mint_connector_id();

        // The environment this create addresses, resolved through the rule every handler
        // that acts on an environment shares (`live_environment_in_region_mut`): an
        // environment exists at the endpoint of the Region it was created in, so a
        // connector can only be created in one from that Region, and an environment that
        // has reached DELETED is gone.
        let environment = state
            .live_environment_in_region_mut(&region, &input.environment_id)
            .ok_or_else(|| {
                CreateEnvironmentConnectorErrorModeled::ResourceNotFoundException(
                    crate::state::environment_not_found(&input.environment_id),
                )
            })?;
        let environment_id = environment.environment_id.clone();

        // "You can create only one connector of each type per environment": the type is
        // the identity of a connector within its environment, so a second create for a
        // type already connected is either this request again or a conflict.
        if let Some(existing) = crate::state::live_connector_of_type(environment, &input.r#type) {
            // `@idempotent`: the same create again reports the stored record and mints
            // nothing. The caller is handed that record AS the connector the call
            // created, so it is reported only while it is still one a later call
            // resolves: a connector whose teardown this service has already accepted is
            // on its way to `DELETED` and resolves to nothing, so reporting it would
            // report a connector that never comes to exist. It still occupies its type,
            // which is what makes such a request the conflict below.
            // The stored record is the one this request asks for when it names the same
            // appliance and the same secret — what each value NAMES, not how the request
            // spelled it: an appliance FQDN is a domain name
            // ([`crate::state::same_dns_name`]) and the secret is compared as the
            // ARN its identifier resolved to.
            if existing.is_live()
                && crate::state::same_dns_name(&existing.appliance_fqdn, &input.appliance_fqdn)
                && existing.secret_arn == secret_arn
            {
                return Ok(CreateEnvironmentConnectorOutput {
                    connector: Some(existing.to_wire(&environment_id)),
                });
            }
            return Err(CreateEnvironmentConnectorErrorModeled::ValidationException(
                crate::state::field_validation_error(
                    "type",
                    &format!(
                        "names a connector type environment {environment_id} already has a \
                         connector of ({}); only one connector of each type can be created per \
                         environment",
                        existing.connector_id
                    ),
                ),
            )
            .into());
        }

        // Establishing the connection to the appliance has started but is not finished:
        // the connector is CREATING and the clock owes it the move to ACTIVE.
        //
        // Which is only possible with credentials to establish it WITH: the secret the
        // request names is one the account holds, storing the fields this connector type
        // requires. Evaluated here, where a connector is really about to be created: the
        // repeat above stores nothing and the conflict creates nothing, so neither needs
        // the credentials of a connection no new connector makes.
        //
        // LOCK ORDER. This reads Secrets Manager's store while this service's guard is
        // held, which is the one order either lock is ever taken in.
        let credential_failures = crate::state::secret_resources::credential_secret_failures(
            &*self.engine,
            &region,
            "secretIdentifier",
            &input.secret_identifier,
            &input.r#type,
        );
        if !credential_failures.is_empty() {
            return Err(CreateEnvironmentConnectorErrorModeled::ValidationException(
                crate::state::aggregate_validation_error(credential_failures),
            )
            .into());
        }
        let pending =
            crate::state::PendingConnectorState::starting_now(crate::state::CONNECTOR_ACTIVE);
        let due_at = pending.due_at;
        let connector = crate::state::StoredConnector::creating(
            connector_id.clone(),
            &input.r#type,
            &input.appliance_fqdn,
            secret_arn,
            created_at_ms,
            pending,
        );
        let created = match environment.connectors.entry(connector_id) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(connector).to_wire(&environment_id)
            }
            // Unreachable: ids are minted from a strictly increasing seed. Answering
            // honestly beats reporting some other connector as the created one.
            std::collections::btree_map::Entry::Occupied(taken) => {
                return Err(
                    CreateEnvironmentConnectorErrorModeled::InternalServerException(
                        InternalServerException {
                            message: format!(
                                "The connector id {} is already in use.",
                                taken.key()
                            ),
                        },
                    )
                    .into(),
                );
            }
        };
        // Creating a connector changes the environment's connector inventory, which is
        // part of the environment: the record is re-stamped, as removing one re-stamps
        // it. The idempotent repeat above returns before here, having changed nothing.
        environment.note_child_change(created_at_ms);
        state.note_transition_due(due_at);
        // The environment's health checks read its connectors, and this call changed that
        // population, so they are recomputed from the state the create leaves behind
        // rather than left standing on the connectors that were there before it.
        state.recompute_environment_checks();

        Ok(CreateEnvironmentConnectorOutput {
            connector: Some(created),
        })
    }
}
