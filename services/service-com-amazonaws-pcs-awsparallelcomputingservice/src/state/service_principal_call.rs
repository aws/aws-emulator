// SPDX-License-Identifier: CC0-1.0

//! The identity a call this service makes OF ITS OWN presents to the service serving it.
//!
//! Some of what PCS does for a cluster is not done as the caller at all. The shared Slurm
//! key a cluster reports ([`super::cluster_secrets`]) lives in a secret PCS OWNS: AWS
//! creates it as the `pcs.amazonaws.com` service principal, for the cluster it belongs to,
//! which is why such a secret is named under the prefix AWS reserves for the service that
//! manages it and reports that service as its owner — neither of which a caller's own
//! `CreateSecret` can do, and neither of which a caller's own call may then change.
//!
//! WHICH credential the call presents is what says so. This mints one in the emulator's own
//! credential store for the one call, and records under that credential's access key id the
//! principal the call is made as and the ARN of the resource it is made for
//! ([`::service_common::service_principal_calls`], the one directory in the shared engine
//! both sides read). The service serving the call resolves the record from the access key
//! id the request presents — the one thing about a request a client cannot choose for
//! itself — and so serves it as this service's own call.
//!
//! The credential and the record live only as long as the call: the returned value deletes
//! both when it is dropped, which is every path out of the call, and the record carries a
//! deadline of its own ([`IN_FLIGHT_RECORD_TTL`]) so neither outlives the call it describes
//! even if nothing drops it.

use std::time::{Duration, SystemTime};

/// The service principal AWS makes PCS's own calls as, which is the identity that owns the
/// secrets this service manages. It is this service's endpoint prefix followed by the
/// service-principal suffix every AWS service's is formed with (`pcs` is this service's own
/// signing name and ARN namespace, `smithy_query(query="service_info")`).
pub const PCS_SERVICE_PRINCIPAL: &str = "pcs.amazonaws.com";

/// How long a record of one in-flight call stays readable when nothing has dropped it.
///
/// The guard below is what ENDS a record; this is the floor that bounds one nothing dropped.
/// It is far above the time an in-process call takes (there is no socket and no retry) and
/// far below any human timescale, so a record can neither expire while the serving handler is
/// still deciding the call nor accumulate.
pub const IN_FLIGHT_RECORD_TTL: Duration = Duration::from_secs(60);

/// One call this service is making as its own service principal: the credential it is signed
/// with, and the record that says whose call it is.
pub struct ServicePrincipalCall<'a, Eng: Send + Sync + 'static> {
    engine: &'a Eng,
    credentials: &'a ::emulator_server::CredentialStore,
    /// The access key id of the credential minted for this one call.
    access_key_id: String,
    /// Its secret, which signs this one call and goes nowhere else.
    secret: String,
}

/// Prints the access key id of the credential, never its secret.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for ServicePrincipalCall<'_, Eng> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServicePrincipalCall")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

impl<'a, Eng: Send + Sync + 'static> ServicePrincipalCall<'a, Eng> {
    /// Begin a call made as [`PCS_SERVICE_PRINCIPAL`] on behalf of the resource `source_arn`
    /// names.
    ///
    /// `source_arn` has to name a resource OF THIS SERVICE IN THIS ACCOUNT
    /// ([`names_a_resource_of_this_service`]): the account in the record is what the service
    /// serving the call matches on before it serves the call as this service's own, so an ARN
    /// that names anything else would attribute a privileged call to a resource this service
    /// does not own. The caller passes the ARN off a record this service itself minted, and the
    /// check is what keeps that true of every future caller.
    ///
    /// `None` when the call cannot be made that way: an ARN that is not one of this service's
    /// own resources, an engine carrying no directory of such calls, or a directory already
    /// holding as many in-flight calls as it records. The caller then makes no call at all,
    /// because what it had to do needed this identity.
    #[must_use]
    pub fn begin(
        engine: &'a Eng,
        ctx: &'a ::smithy_server_runtime::OperationContext<'_>,
        source_arn: &str,
    ) -> Option<Self> {
        let directory = in_flight_calls(engine)?;
        let source_account = names_a_resource_of_this_service(source_arn)?.to_string();
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
                principal: PCS_SERVICE_PRINCIPAL.to_string(),
                source_arn: source_arn.to_string(),
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

    /// A Secrets Manager client over the in-process callback that signs with THIS call's
    /// credential, so the service serving it resolves the record written above.
    ///
    /// It IS this service's one Secrets Manager client
    /// ([`super::cluster_secrets::secrets_manager_client_signed_as`]) — the same in-process
    /// transport, the same disabled retries and timeouts, and the same reading of the Region as
    /// a label rather than as text to interpolate — with the identity of this call presented
    /// instead of the emulator's own.
    pub fn secrets_manager_client(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Option<::aws_sdk_secretsmanager::Client> {
        super::cluster_secrets::secrets_manager_client_signed_as(
            ctx,
            region,
            Some(::aws_sdk_secretsmanager::config::Credentials::new(
                self.access_key_id.clone(),
                self.secret.clone(),
                None,
                None,
                "aws-emulator",
            )),
        )
    }
}

/// Dropping the call forgets its record and DELETES its credential: an identity nothing holds
/// can sign nothing, and a record nothing holds decides nothing.
impl<Eng: Send + Sync + 'static> Drop for ServicePrincipalCall<'_, Eng> {
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

/// The account of `source_arn`, when it names a resource of THIS service in THIS account —
/// `None` for every other value.
///
/// Three things have to hold, read through the tree's one reading of the ARN grammar
/// (`service_common::arn::parse`): the ARN's vendor is this service's own namespace
/// ([`ARN_NAMESPACE`]), its resource part names one of the kinds this service has (a value
/// naming none reports [`crate::state::UNNAMED_RESOURCE`]), and its account is the account
/// this emulator serves ([`::engine::emulator_account_id`]). Nothing else may be called for:
/// the record written for the call carries that account, the service serving it matches a
/// request against it, and a call attributed to another account's resource would reach state
/// this service does not own.
#[must_use]
fn names_a_resource_of_this_service(source_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse(source_arn)?;
    let of_this_service = arn.vendor == ARN_NAMESPACE
        && crate::state::arn_resource(source_arn).0 != crate::state::UNNAMED_RESOURCE;
    let of_this_account = arn.account == ::engine::emulator_account_id();
    (of_this_service && of_this_account).then_some(arn.account)
}

/// The ARN namespace of this service's own resources, which the first label of
/// [`PCS_SERVICE_PRINCIPAL`] is also formed from (`arnNamespace`,
/// `smithy_query(query="service_info")`).
const ARN_NAMESPACE: &str = "pcs";
