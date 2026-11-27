// SPDX-License-Identifier: CC0-1.0

//! @op RegisterSlackWorkspaceForOrganization
//!
//! Registers a Slack workspace with this account, recording it in the account's
//! workspace registry (`state::slack_workspace_configurations`) and reporting the
//! stored record. That registry is the precondition
//! `CreateSlackChannelConfiguration` enforces: until a workspace is registered, a
//! channel configuration in it is a `ConflictException`.
//!
//! The call acts on a workspace that ALREADY exists for this API, so the workspace the
//! request identifies is looked up ahead of the registry write — but the role the
//! caller's organization gives it is read first, because that role decides WHICH
//! precondition the call has to meet. A Slack workspace reaches this API through the
//! interactive flow in the Amazon Web Services Support Center console — "Sign in to the
//! Amazon Web Services Support Center and authorize the Slack workspaces where you want
//! your organization to have access to", after which this API "authorize[s] each Slack
//! workspace for the organization" — and no operation of this API performs that flow, so
//! the authorizations it left are state no call writes
//! ([`crate::state::CONSOLE_AUTHORIZED_SLACK_WORKSPACES`]). That manual step is the step
//! the model places on the account that registers a workspace FOR THE ORGANIZATION, so
//! for a caller the model asks to take it a `teamId` absent from those authorizations
//! names no workspace this call can act on and is the modelled
//! `ResourceNotFoundException`
//! ([`crate::state::ComAmazonawsSupportappSupportappState::unauthorized_workspace_not_found`]);
//! nothing is registered. A MEMBER account is exempt — "Member accounts don't need to
//! authorize the Slack workspace manually through the Amazon Web Services Support
//! Center" — so that lookup is not its precondition at all: its call is decided by
//! whether the management account has registered the workspace for the organization (see
//! below).
//!
//! This is also an Organizations-scoped call — "To call this API, your account must be part
//! of an organization in Organizations" — so the account's membership is settled before
//! the registry is written, by reading the Organizations control plane's own store
//! through the shared engine ([`crate::state::caller_account_type`]): an account that
//! belongs to no organization is the first condition this operation's
//! `ConflictException` enumerates, so the call is refused with it and nothing is
//! registered. When an organization does hold the account, `accountType` reports the
//! role that organization's own record gives it — `management` for the account that
//! manages it, `member` for one of its other member accounts. The documented workspace
//! quota bounds the registry past that — see
//! [`crate::state::MAX_SLACK_WORKSPACE_CONFIGURATIONS`].
//!
//! The role the organization gives the account GOVERNS the call, it is not only reported:
//! a workspace is authorized for an organization in two steps — "After the management
//! account authorizes the Slack workspace, member accounts can call this API to authorize
//! the same Slack workspace for their individual accounts" — so a call from an account
//! that is only a member of its organization is served for a workspace the registry
//! already holds and refused, with the second condition this operation's
//! `ConflictException` enumerates, for one it does not. The refusal registers nothing.
//!
//! `teamName` is the name the Slack workspace carries in Slack. A registered workspace
//! has one, and this operation and `ListSlackWorkspaceConfigurations` both report it, so
//! the registration RECORDS a name for the workspace — derived from the workspace's own
//! identifier by [`crate::state::derived_slack_workspace_name`], because the emulator has
//! no Slack directory to ask and no request member carries it — and every later report of
//! that workspace reads back the stored value. The stored record's
//! `allowOrganizationMemberAccount` is what "lets member accounts authorize Slack
//! workspaces", which is what a registration FROM THE MANAGEMENT ACCOUNT grants, so only
//! that call records it — on a workspace the account already held as much as on one it did
//! not — and a member account's call leaves it as the management account's registration
//! left it. That field is what `ListSlackWorkspaceConfigurations` reports.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `RegisterSlackWorkspaceForOrganization::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `RegisterSlackWorkspaceForOrganization`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")
//! ```
//!
//! Registers a Slack workspace for your Amazon Web Services account. To call this API, your
//! account must be part of an organization in Organizations. If you're the management
//! account and you want to register Slack workspaces for your organization, you must
//! complete the following tasks: Sign in to the Amazon Web Services Support Center and
//! authorize the Slack workspaces where you want your organization to have access to. See
//! Authorize a Slack workspace in the Amazon Web Services Support User Guide. Call the
//! RegisterSlackWorkspaceForOrganization API to authorize each Slack workspace for the
//! organization. After the management account authorizes the Slack workspace, member
//! accounts can call this API to authorize the same Slack workspace for their individual
//! accounts. Member accounts don't need to authorize the Slack workspace manually through
//! the Amazon Web Services Support Center. To use the Amazon Web Services Support App, each
//! account must then complete the following tasks: Create an Identity and Access Management
//! (IAM) role with the required permission. For more information, see Managing access to
//! the Amazon Web Services Support App. Configure a Slack channel to use the Amazon Web
//! Services Support App for support cases for that account. For more information, see
//! Configuring a Slack channel.
//!
//! ## Input
//!
//! * `teamId` (teamId, len 1..256, pattern ^\S+$) **required** — The team ID in Slack. This
//!   ID uniquely identifies a Slack workspace, such as T012ABCDEFG. Specify the Slack
//!   workspace that you want to use for your organization.
//!
//! ## Output
//!
//! * `accountType` (AccountType, one of: management | member) — Whether the Amazon Web
//!   Services account is a management or member account that's part of an organization in
//!   Organizations.
//! * `teamId` (teamId, len 1..256, pattern ^\S+$) — The team ID in Slack. This ID uniquely
//!   identifies a Slack workspace, such as T012ABCDEFG.
//! * `teamName` (teamName, len 1..256, pattern ^.+$) — The name of the Slack workspace.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient permission to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Your request has a conflict. For example, you might
//!   receive this error if you try the following: Add, update, or delete a Slack channel
//!   configuration before you add a Slack workspace to your Amazon Web Services account. Add
//!   a Slack channel configuration that already exists in your Amazon Web Services account.
//!   Delete a Slack channel configuration for a live chat channel. Delete a Slack workspace
//!   from your Amazon Web Services account that has an active live chat channel. Call the
//!   RegisterSlackWorkspaceForOrganization API from an Amazon Web Services account that
//!   doesn't belong to an organization. Call the RegisterSlackWorkspaceForOrganization API
//!   from a member account, but the management …
//! * `InternalServerException` (HTTP 500) — We can’t process your request right now because
//!   of a server issue. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource is missing or doesn't
//!   exist, such as an account alias, Slack channel configuration, or Slack workspace
//!   configuration.
//! * `ValidationException` (HTTP 400) — Your request input doesn't meet the constraints that
//!   the Amazon Web Services Support App specifies.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterSlackWorkspaceForOrganizationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportapp-supportapp.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterSlackWorkspaceForOrganizationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.supportapp#RegisterSlackWorkspaceForOrganization")]
#[derive(Debug)]
pub enum RegisterSlackWorkspaceForOrganizationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_slack_workspace_for_organization::RegisterSlackWorkspaceForOrganizationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterSlackWorkspaceForOrganizationError = crate::scaffolded::Scaffolded<RegisterSlackWorkspaceForOrganizationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterSlackWorkspaceForOrganizationErrorModeled> for RegisterSlackWorkspaceForOrganizationError {
    fn from(modeled: RegisterSlackWorkspaceForOrganizationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterSlackWorkspaceForOrganization` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterSlackWorkspaceForOrganization<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterSlackWorkspaceForOrganization<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterSlackWorkspaceForOrganization<Eng>
{
    type Input = RegisterSlackWorkspaceForOrganizationInput;
    type Output = RegisterSlackWorkspaceForOrganizationOutput;
    type Error = RegisterSlackWorkspaceForOrganizationError;

    const NAME: &'static str = "RegisterSlackWorkspaceForOrganization";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_SLACK_WORKSPACE_FOR_ORGANIZATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_SLACK_WORKSPACE_FOR_ORGANIZATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed call at the IAM layer, in FRONT of the service, so the
        // caller's identity-based policies are evaluated before any store is reached: a
        // principal no policy allows this action for is refused, and nothing is registered
        // for it.
        if let Some(denied) = crate::state::authorization_refusal(&*self.engine, ctx, Self::NAME) {
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::AccessDeniedException(denied)
                    .into(),
            );
        }

        // `teamId` declares a `\S`-based `@pattern`, and the request boundary's walk reads a
        // `\S` more widely than the modelled expression does, so the part of that expression
        // the walk admits is applied here, before any store is reached: the same shape with
        // the same `@pattern` is an OUTPUT member of this very operation and of
        // `ListSlackWorkspaceConfigurations`, so a value the expression does not admit could
        // not be reported back through the shape it would be registered under. See
        // [`crate::state::team_id_refusal`].
        if let Some(message) = crate::state::team_id_refusal(&input.team_id) {
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(message),
                    },
                )
                .into(),
            );
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RegisterSlackWorkspaceForOrganizationErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the Support App store is unavailable".to_string()),
                },
            )
        })?;

        // An Organizations-scoped call, so the role the caller's organization gives it is
        // read FIRST — with no guard of this service's own store held — because that role
        // decides WHICH precondition the call has to meet: the manual Support Center
        // console step is the step the model places on the account that registers a
        // workspace for the organization, and it exempts a member account from it. A store
        // that cannot be read at all is this service's own server error, not a conflict the
        // caller can resolve.
        let account_type = crate::state::caller_account_type(&*self.engine).map_err(|_| {
            RegisterSlackWorkspaceForOrganizationErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the Organizations store is unavailable".to_string()),
                },
            )
        })?;

        // The workspace the request IDENTIFIES is looked up before any further
        // precondition on the calling account, exactly as the operations that act on a
        // stored Slack channel configuration do — but only for a caller the model asks to
        // take the console step: a Slack workspace exists for this API once the Support
        // Center console's interactive flow has authorized it, and the account that
        // authorizes a workspace FOR THE ORGANIZATION is the one that takes that step, so
        // for it a `teamId` no console authorization recorded names nothing this call can
        // act on and is the modelled `ResourceNotFoundException`. A member account is
        // exempt — "Member accounts don't need to authorize the Slack workspace manually
        // through the Amazon Web Services Support Center" — so its call is decided by the
        // management account's registration instead (the registry read below), never by a
        // console authorization it is not asked to hold. The guard is dropped before
        // anything else is read.
        if account_type != Some(crate::state::ACCOUNT_TYPE_MEMBER) {
            let not_found = {
                let store = state.lock();
                store.unauthorized_workspace_not_found(&input.team_id)
            };
            if let Some(message) = not_found {
                return Err(
                    RegisterSlackWorkspaceForOrganizationErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(message),
                        },
                    )
                    .into(),
                );
            }
        }

        // An account in no organization is the first condition this operation's
        // `ConflictException` names — "To call this API, your account must be part of an
        // organization in Organizations" — and the refusal registers nothing.
        let account_type = account_type.ok_or_else(|| {
            RegisterSlackWorkspaceForOrganizationErrorModeled::ConflictException(
                ConflictException {
                    message: Some(crate::state::account_not_in_organization_message(Self::NAME)),
                },
            )
        })?;

        // Which call this is, in the two-step authorization the operation documents: the
        // management account's registration is the one that authorizes a workspace FOR THE
        // ORGANIZATION, a member account's only authorizes an already authorized workspace
        // for that one account.
        let registers_for_the_organization = account_type == crate::state::ACCOUNT_TYPE_MANAGEMENT;

        let mut store = state.lock();
        // Registering a workspace the account already holds changes nothing: a `teamId`
        // identifies a workspace, and the conflicts this operation documents are about
        // organization membership, not about a workspace that is already registered.
        // So only a NEW registration counts against the documented quota, which is
        // also what keeps the registry from growing without bound on repeated calls.
        let already_registered = store.workspace_is_registered(&input.team_id);
        // The second condition this operation's `ConflictException` enumerates: a call
        // from an account that is a member of its organization, for a workspace the
        // organization's management account has not registered yet. The registry is what
        // settles that — it holds the workspaces registered for this organization, read
        // through the state struct's own presence rule
        // ([`crate::state::ComAmazonawsSupportappSupportappState::workspace_is_registered`]),
        // the one `CreateSlackChannelConfiguration` decides its conflict by — and the
        // refusal registers nothing, so a member account cannot authorize a workspace, nor
        // record the organization-wide grant, ahead of the management account.
        if !registers_for_the_organization && !already_registered {
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::ConflictException(
                    ConflictException {
                        message: Some(
                            crate::state::workspace_not_authorized_for_organization_message(
                                Self::NAME,
                                &input.team_id,
                            ),
                        ),
                    },
                )
                .into(),
            );
        }

        if !already_registered
            && store.slack_workspace_configurations.len()
                >= crate::state::MAX_SLACK_WORKSPACE_CONFIGURATIONS
        {
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::ValidationException(
                    ValidationException {
                        // The same validation envelope every other request-validation
                        // refusal of this service is worded in, naming the operation
                        // that was called and restating the documented quota.
                        message: Some(crate::state::workspace_quota_exceeded_message(Self::NAME)),
                    },
                )
                .into(),
            );
        }

        let record = store
            .slack_workspace_configurations
            .entry(input.team_id.clone())
            .or_insert_with(|| crate::state::SlackWorkspaceConfig {
                team_id: input.team_id.clone(),
                // A registered workspace has a name, which this operation and the
                // listing both report, so one is recorded here ONCE, when the workspace
                // enters the registry: every later report reads this stored value back,
                // and a second registration of the same workspace leaves it alone.
                team_name: crate::state::derived_slack_workspace_name(&input.team_id),
                allow_organization_member_account: None,
            });
        // `allowOrganizationMemberAccount` is "whether to allow member accounts to
        // authorize Slack workspaces", which is what registering the workspace FOR THE
        // ORGANIZATION from its management account grants — so the management account's
        // call leaves it recorded, on a workspace this account already held as much as on
        // one it did not, and that is the only thing that call changes about an already
        // registered workspace. A member account's call authorizes the workspace for its
        // own account only, so it records no organization-wide grant and leaves the stored
        // record exactly as the management account's registration left it.
        if registers_for_the_organization {
            record.allow_organization_member_account = Some(true);
        }

        Ok(RegisterSlackWorkspaceForOrganizationOutput {
            team_id: Some(record.team_id.clone()),
            team_name: Some(record.team_name.clone()),
            // The role the organization's own record gives this account, not a fixed
            // value: `management` when it manages that organization, `member` when it is
            // one of its other member accounts.
            account_type: Some(account_type.to_string()),
        })
    }
}
