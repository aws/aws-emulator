// SPDX-License-Identifier: CC0-1.0

//! The identity a call GuardDuty makes OF ITS OWN presents to the service serving it.
//!
//! The EventBridge rule a Malware Protection plan owns is not a rule the customer wrote: AWS
//! creates it for the plan, which is why `ListRules` reports it with a `ManagedBy` naming the
//! plan's service principal and why a customer's own `DeleteRule` on it is refused unless it
//! forces the delete. Nothing a caller's `PutRule` can send produces such a rule — `ManagedBy`
//! is no member of any request — so the write has to be made AS the service.
//!
//! WHICH credential the call presents is what says so. This mints one in the emulator's own
//! credential store for the one call and records, under that credential's access key id, the
//! principal the call is made as and the ARN of the plan it is made for
//! ([`::service_common::service_principal_calls`], the one directory in the shared engine both
//! sides read). EventBridge resolves the record from the access key id the request presents —
//! the one thing about a request a client cannot choose for itself — and so records the rule as
//! that service's managed one.
//!
//! The credential and the record live only as long as the call: the value returned here deletes
//! both when it is dropped, which is every path out of the call, and the record carries a
//! deadline of its own ([`IN_FLIGHT_RECORD_TTL`]) so neither outlives the call it describes even
//! if nothing drops it.

use std::time::{Duration, SystemTime};

/// How long a record of one in-flight call stays readable when nothing has dropped it.
///
/// The guard below is what ENDS a record; this is the floor that bounds one nothing dropped. It
/// is far above the time an in-process call takes (there is no socket and no retry) and far
/// below any human timescale, so a record can neither expire while the serving handler is still
/// deciding the call nor accumulate.
pub const IN_FLIGHT_RECORD_TTL: Duration = Duration::from_secs(60);

/// One call this service is making as a Malware Protection plan's own service principal
/// ([`super::MALWARE_PROTECTION_PLAN_SERVICE_PRINCIPAL`]): the credential it is signed with, and
/// the record that says whose call it is.
pub struct PlanServicePrincipalCall<'a, Eng: Send + Sync + 'static> {
    engine: &'a Eng,
    credentials: &'a ::emulator_server::CredentialStore,
    /// The access key id of the credential minted for this one call.
    access_key_id: String,
    /// Its secret, which signs this one call and goes nowhere else.
    secret: String,
}

/// Prints the access key id of the credential, never its secret.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for PlanServicePrincipalCall<'_, Eng> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlanServicePrincipalCall")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

impl<'a, Eng: Send + Sync + 'static> PlanServicePrincipalCall<'a, Eng> {
    /// Begin a call made as the Malware Protection plan service principal on behalf of the plan
    /// `plan_arn` names.
    ///
    /// `None` when the call cannot be made that way: an engine carrying no directory of such
    /// calls, a plan ARN naming no account (no record could carry the account a reader matches
    /// on), a store that minted no secret, or a directory already holding as many in-flight
    /// calls as it records. The caller then makes no managed-rule write at all rather than
    /// writing one the customer would own.
    #[must_use]
    pub fn begin(
        engine: &'a Eng,
        ctx: &'a ::smithy_server_runtime::OperationContext<'_>,
        plan_arn: &str,
    ) -> Option<Self> {
        let directory = in_flight_calls(engine)?;
        let source_account = ::service_common::arn::parse(plan_arn)
            .map(|arn| arn.account.to_string())
            .filter(|account| !account.is_empty())?;
        let minted = ctx
            .credential_store()
            .create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
        let secret = minted
            .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
            .map(str::to_string);
        let Some(secret) = secret else {
            let _ = ctx.credential_store().delete(&minted.id);
            return None;
        };
        let now = SystemTime::now();
        let recorded = directory.lock().remember(
            minted.id.clone(),
            ::service_common::service_principal_calls::ServicePrincipalCall {
                principal: super::MALWARE_PROTECTION_PLAN_SERVICE_PRINCIPAL.to_string(),
                source_arn: plan_arn.to_string(),
                source_account,
                expires_at: now + IN_FLIGHT_RECORD_TTL,
            },
            now,
        );
        if !recorded {
            let _ = ctx.credential_store().delete(&minted.id);
            return None;
        }
        Some(Self {
            engine,
            credentials: ctx.credential_store(),
            access_key_id: minted.id,
            secret,
        })
    }

    /// An EventBridge client over the in-process callback that signs with THIS call's
    /// credential, so the service serving it resolves the record written above.
    ///
    /// It is the SHARED construction of that hop
    /// ([`::service_common::eventbridge_client::eventbridge_client_signed_as`]), handed this
    /// call's credential: the same in-process transport — no socket is opened and nothing
    /// outside this process is reachable — configured exactly as every other emulated
    /// service's publication of an event is.
    #[must_use]
    pub fn eventbridge_client(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> ::aws_sdk_eventbridge::Client {
        ::service_common::eventbridge_client::eventbridge_client_signed_as(
            ctx.callback(),
            region,
            ::aws_sdk_eventbridge::config::Credentials::new(
                self.access_key_id.clone(),
                self.secret.clone(),
                None,
                None,
                super::MALWARE_PROTECTION_PLAN_SERVICE_PRINCIPAL,
            ),
        )
    }
}

/// Dropping the call forgets its record and DELETES its credential: an identity nothing holds
/// can sign nothing, and a record nothing holds decides nothing.
impl<Eng: Send + Sync + 'static> Drop for PlanServicePrincipalCall<'_, Eng> {
    fn drop(&mut self) {
        if let Some(directory) = in_flight_calls(self.engine) {
            directory.lock().forget(&self.access_key_id);
        }
        let _ = self.credentials.delete(&self.access_key_id);
    }
}

/// The shared directory of the calls a service is making as its own service principal, in the
/// one shared engine: written here and read by the service serving the call.
fn in_flight_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}
