// SPDX-License-Identifier: CC0-1.0

//! @op EnableOutboundWebIdentityFederation
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming one of the conditions
//! this operation models. The input, output, error enum and schemas a generated stub would
//! have carried are declared in
//! `src/ops/types/enable_outbound_web_identity_federation_types.rs` (the layout every other
//! operation of this service follows) and reached through `super::types::*`; `build.rs`
//! picks the operation up from the `@op` line above and wires it into the service's routing
//! table, exactly as it does for the hand-authored
//! `src/ops/get_account_password_policy.rs`.
//!
//! ## What this operation answers on the wire
//!
//! A MUTATION: it "enables the outbound identity federation feature for your Amazon Web
//! Services account" and "also generates a unique issuer URL for your Amazon Web Services
//! account". The feature is an account-level flag, so what a call does is decided by the
//! account's own record of it
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::outbound_web_identity_federation`]):
//!
//! * the feature off (never enabled, or enabled and disabled again): it is turned on and
//!   the call answers `200` with the account's `IssuerIdentifier` — minted here the first
//!   time and kept from then on, because the issuer belongs to the ACCOUNT rather than to
//!   one enablement
//!   ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::enable_outbound_web_identity_federation`]);
//! * the feature already on: refused with the modelled `FeatureEnabledException` (HTTP 409,
//!   awsQuery code `FeatureEnabled`) — "you cannot enable the feature multiple times" —
//!   leaving the account exactly as it was.
//!
//! The issuer is a URL under the base this emulator is served at, because it is an address
//! a caller fetches later and an address this emulator announces has to lead back to this
//! emulator; how it is built is [`crate::state::outbound_web_identity_issuer`].
//!
//! A caller whose identity-based policies do not permit the call is refused before anything
//! is written, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]), so a request that was not allowed enables
//! nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `EnableOutboundWebIdentityFederation`
//!
//! Enables the outbound identity federation feature for your Amazon Web Services account.
//! When enabled, IAM principals in your account can use the `GetWebIdentityToken` API to
//! obtain JSON Web Tokens (JWTs) for secure authentication with external services. This
//! operation also generates a unique issuer URL for your Amazon Web Services account.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `IssuerIdentifier` (stringType) — A unique issuer URL for your Amazon Web Services
//!   account that hosts the OpenID Connect (OIDC) discovery endpoints at
//!   `/.well-known/openid-configuration and /.well-known/jwks.json`.
//!
//! ## Errors
//!
//! * `FeatureEnabledException` (HTTP 409, awsQuery code `FeatureEnabled`) — The request
//!   failed because outbound identity federation is already enabled for your Amazon Web
//!   Services account. You cannot enable the feature multiple times.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `EnableOutboundWebIdentityFederationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation. `ServiceFailureException` is widened in through
// `extra_errors`: this SERVICE models it ("the request processing has failed because of an
// unknown error, exception or failure", HTTP 500), which is the class a failure of this
// process rather than of the request belongs to — the key generation the enable needs not
// completing at all.
#[smithy_macros::operation_error(
    "com.amazonaws.iam#EnableOutboundWebIdentityFederation",
    extra_errors = ["ServiceFailureException"]
)]
#[derive(Debug)]
pub enum EnableOutboundWebIdentityFederationErrorModeled {
    FeatureEnabledException(FeatureEnabledException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::enable_outbound_web_identity_federation::EnableOutboundWebIdentityFederationError`).
pub type EnableOutboundWebIdentityFederationError =
    crate::scaffolded::Scaffolded<EnableOutboundWebIdentityFederationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<EnableOutboundWebIdentityFederationErrorModeled>
    for EnableOutboundWebIdentityFederationError
{
    fn from(modeled: EnableOutboundWebIdentityFederationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `EnableOutboundWebIdentityFederation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct EnableOutboundWebIdentityFederation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> EnableOutboundWebIdentityFederation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for EnableOutboundWebIdentityFederation<Eng>
{
    type Input = EnableOutboundWebIdentityFederationInput;
    type Output = EnableOutboundWebIdentityFederationOutput;
    type Error = EnableOutboundWebIdentityFederationError;

    const NAME: &'static str = "EnableOutboundWebIdentityFederation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::ENABLE_OUTBOUND_WEB_IDENTITY_FEDERATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::ENABLE_OUTBOUND_WEB_IDENTITY_FEDERATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it does is decided by the account's own
        // record of the feature.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // This operation models no server error, so an unreachable store is reported
            // through the crate's generic refusal channel, exactly as the other operations
            // of this service whose model declares none report one.
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "the IAM store is unavailable",
            ));
        };

        // A MUTATION, taken in three steps so that the key generation it needs runs neither
        // on this async worker nor under the store's lock (see below).
        //
        // First, under a short-lived guard: whether the caller may enable at all, and
        // whether the feature is already on. IAM authorizes its own API: the signing
        // principal's identity-based policies are evaluated before anything is written. The
        // feature is the ACCOUNT's, not one entity's, and the Service Authorization
        // Reference lists no resource type for the action, so the decision is made against
        // the account-wide resource — exactly as the other account-wide operations of this
        // service decide.
        let caller_akid = ::engine::extract_akid(&ctx.parts().headers);
        {
            let store = state.lock();
            crate::state::authorization::authorize(
                &store,
                ctx,
                crate::state::authorization::ENABLE_OUTBOUND_WEB_IDENTITY_FEDERATION,
                crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
            )?;

            // The feature already on is the one refusal this operation models, and it is
            // answered before anything is written and before a key is generated: the account
            // keeps the issuer and the flag it already had. The same state is read again
            // under the guard that writes, which is what DECIDES it.
            if store.served_outbound_web_identity_federation().is_some() {
                ::tracing::info!(
                    audit = true,
                    operation = "EnableOutboundWebIdentityFederation",
                    outcome = "refused_feature_enabled",
                    caller_akid = %caller_akid,
                    "outbound identity federation was already enabled"
                );
                return Err(
                    EnableOutboundWebIdentityFederationErrorModeled::FeatureEnabledException(
                        FeatureEnabledException {
                            message: Some(
                                crate::state::outbound_federation_already_enabled_message(),
                            ),
                        },
                    )
                    .into(),
                );
            }
        }

        // Then the key material the issuer publishes, which is CPU-bound work: it runs on
        // the blocking pool with no state guard held, the construct every key-minting
        // operation of this workspace mints through
        // ([`crate::state::IssuerSigningKeys::generate`]). A generation that does not
        // complete — the task not joining, or a key the cryptography library would not
        // produce — is a failure of this process rather than of the request, and enables
        // nothing: an issuer that cannot sign the algorithms its model documents is not one
        // this account is given.
        let keys = ::tokio::task::spawn_blocking(crate::state::IssuerSigningKeys::generate)
            .await
            .ok()
            .flatten();
        let Some(keys) = keys else {
            ::tracing::warn!(
                operation = "EnableOutboundWebIdentityFederation",
                outcome = "refused_keys_not_generated",
                "the issuer's signing keys could not be generated; the feature was not enabled"
            );
            return Err(
                EnableOutboundWebIdentityFederationErrorModeled::ServiceFailureException(
                    ServiceFailureException {
                        message: Some(
                            "the account's web identity issuer keys could not be generated"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        // Finally the write, under its own guard: the feature is turned on and the issuer
        // the account is served under is answered — minted here the first time and reported
        // unchanged after a disable and a second enable. A call that lost the race to
        // another enable while its keys were being generated is told the feature was
        // already on, and the keys it generated are dropped unused.
        let mut store = state.lock();
        let Some(issuer_identifier) =
            store.enable_outbound_web_identity_federation(ctx.endpoint().as_str(), keys)
        else {
            ::tracing::info!(
                audit = true,
                operation = "EnableOutboundWebIdentityFederation",
                outcome = "refused_feature_enabled",
                caller_akid = %caller_akid,
                "outbound identity federation was already enabled"
            );
            return Err(
                EnableOutboundWebIdentityFederationErrorModeled::FeatureEnabledException(
                    FeatureEnabledException {
                        message: Some(crate::state::outbound_federation_already_enabled_message()),
                    },
                )
                .into(),
            );
        };

        ::tracing::info!(
            audit = true,
            operation = "EnableOutboundWebIdentityFederation",
            outcome = "enabled",
            caller_akid = %caller_akid,
            "outbound identity federation was enabled for the account"
        );

        Ok(EnableOutboundWebIdentityFederationOutput {
            issuer_identifier: Some(issuer_identifier),
        })
    }
}
