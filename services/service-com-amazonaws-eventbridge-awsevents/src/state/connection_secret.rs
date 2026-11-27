// SPDX-License-Identifier: CC0-1.0

//! The AWS Secrets Manager secret a CONNECTION's credential is held in — the one
//! `DescribeConnection$SecretArn` names.
//!
//! "The ARN of the secret created from the authorization parameters specified for the
//! connection" (`DescribeConnection$SecretArn`): the credential a connection is created
//! or updated with is not kept by EventBridge for a client to read back — it is put into
//! a secret in Secrets Manager that EventBridge OWNS, and the connection reports that
//! secret's ARN. So the ARN a read hands out has to be the one Secrets Manager allocated
//! for a secret it really holds: every Secrets Manager read of it answers while the
//! connection exists, and the protections of a service-linked secret (its
//! `OwningService`, and the refusal of a caller's own write or delete of it) are in force
//! because the secret is a real one of that service.
//!
//! Three things follow from one secret per connection, and all three happen here:
//!
//! 1. a CREATE puts the credential into a new secret, named under the prefix AWS reserves
//!    for this service ([`connection_secret_name`]), and the connection records the ARN
//!    Secrets Manager allocated ([`super::ConnectionRecord::secret_arn`]);
//! 2. an UPDATE that supplies credentials stakes a NEW VERSION of that same secret, so
//!    the value a client reads back is the one last written and the secret's version
//!    history is the connection's credential history;
//! 3. removing the connection removes the secret, so the credential it held can no longer
//!    be read and nothing reports an ARN that resolves to nothing.
//!
//! WHICH credential the call presents is what makes the secret this service's own. The
//! call is signed with a credential minted in the emulator's own credential store for
//! that one call, and recorded under that credential's access key id is the service
//! principal the call is made as and the ARN of the connection it is made for
//! ([`::service_common::service_principal_calls`], the one directory in the shared engine
//! both sides read). Secrets Manager resolves that record from the access key id the
//! request presents — the one thing about a request a client cannot choose for itself —
//! and so serves the call as EventBridge's own: that is what lets the secret be named
//! under the reserved prefix no caller can mint and report EventBridge as its
//! `OwningService`.
//!
//! Secrets Manager is reached the way this emulator reaches any other service: the AWS
//! SDK driven over the emulator's own in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]). No socket is opened and
//! nothing outside this process is reachable from it.
//!
//! Nothing here may be called with this service's store guard held: every call awaits.

use ::std::time::{Duration, SystemTime};

use super::invocation::ConnectionSecret;

/// The prefix every secret this service owns is named under: this service's own id, the
/// mark AWS reserves a service's secret namespace with, and the resource kind inside it.
///
/// A name in this form is one only this service's own call may create — the mark is not a
/// character a secret name may otherwise carry — which is what makes the secret
/// service-linked rather than an ordinary secret of the account.
pub const CONNECTION_SECRET_NAME_PREFIX: &str = "events!connection/";

/// The name the secret holding one connection's credential is created under: the reserved
/// prefix, the connection's name, and the unique id segment of the connection's own ARN.
///
/// A FUNCTION of the connection, not of the request: the id is the one minted when the
/// connection was created, so a connection deleted and created again under one name gets
/// a secret of its own rather than inheriting the old one's. Every character of it is one
/// Secrets Manager admits in a secret name after the reserved mark (`ASCII letters,
/// numbers, and the following characters: /_+=.@-`, `CreateSecretRequest$Name`), because
/// `ConnectionName` is `^[\.\-_A-Za-z0-9]+$` and the id segment is hyphens and hex.
#[must_use]
pub fn connection_secret_name(name: &str, id: &str) -> String {
    format!("{CONNECTION_SECRET_NAME_PREFIX}{name}/{id}")
}

/// That same name, read off the connection's OWN ARN
/// (`…:connection/<name>/<id>`), for a call that holds the connection rather than
/// the request that created it.
///
/// `None` for a value that is not a connection ARN, which names no connection to hold a
/// secret for.
#[must_use]
pub fn connection_secret_name_for(connection_arn: &str) -> Option<String> {
    let resource = connection_arn.split_once(":connection/")?.1;
    (!resource.is_empty()).then(|| format!("{CONNECTION_SECRET_NAME_PREFIX}{resource}"))
}

/// What the secret HOLDS: the connection's authorization credential, as the one JSON
/// document the secret's `SecretString` is.
///
/// The members are the credential members of the `AuthParameters` block in force, spelled
/// the way the service-linked secret spells them — `ApiKeyName`/`ApiKeyValue` as
/// `api_key_name`/`api_key_value`, `Username`/`Password` as `username`/`password`, and
/// `ClientID`/`ClientSecret` as `client_id`/`client_secret`. Exactly one block carries a
/// connection's credentials ("you must include only authorization parameters for the
/// AuthorizationType you specify", `CreateConnection$AuthParameters`), so the document
/// carries exactly one pair.
///
/// `None` for a connection holding no credential at all: there is nothing to put in a
/// secret, so no secret is created for it.
#[must_use]
pub fn credential_document(secret: &ConnectionSecret) -> Option<String> {
    let document = if let Some(key) = secret.api_key.as_ref() {
        ::serde_json::json!({ "api_key_name": key.name, "api_key_value": key.value })
    } else if let Some(basic) = secret.basic.as_ref() {
        ::serde_json::json!({ "username": basic.username, "password": basic.password })
    } else {
        let oauth = secret.oauth.as_ref()?;
        ::serde_json::json!({ "client_id": oauth.client_id, "client_secret": oauth.client_secret })
    };
    Some(document.to_string())
}

/// Create the secret holding `document` for the connection `connection_arn` names, and
/// report the ARN Secrets Manager allocated for it.
///
/// `name` is [`connection_secret_name`] for that connection. A name this account already
/// holds a secret under is a secret of this very connection, standing from an earlier
/// write whose ARN the record lost, so the credential goes into it as a new version
/// rather than being reported as unwritable: what is asked for is that the secret under
/// this name holds this credential afterwards.
///
/// `Err` carries the rendering of what came back, for the trace the caller records. Every
/// failure here is this EMULATOR failing to do what EventBridge does — Secrets Manager
/// not mounted beside this service, or a store that would not take the value — never
/// anything the request carried: the values a request carries are judged before this is
/// called.
pub async fn store_connection_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    connection_arn: &str,
    name: &str,
    document: &str,
) -> Result<String, String> {
    let acting = ServicePrincipalCall::begin(engine, ctx, connection_arn)
        .ok_or_else(|| unactable_principal("created", connection_arn))?;
    let secrets = acting.secrets_manager_client(ctx, region)?;
    let created = secrets
        .create_secret()
        .name(name)
        .client_request_token(fresh_version_token())
        .secret_string(document)
        .send()
        .await;
    match created {
        Ok(answer) => allocated_arn(answer.arn, "CreateSecret"),
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_exists_exception()) =>
        {
            let staked = secrets
                .put_secret_value()
                .secret_id(name)
                .client_request_token(fresh_version_token())
                .secret_string(document)
                .send()
                .await
                .map_err(|error| rendered(&error))?;
            allocated_arn(staked.arn, "PutSecretValue")
        }
        Err(error) => Err(rendered(&error)),
    }
}

/// Stake `document` as the CURRENT value of the secret `secret_id` names: the new version
/// an update that supplied credentials puts on the connection's own secret.
///
/// The secret keeps its ARN and its earlier versions — it is the same secret, holding the
/// credential the connection holds now — so a client reading the ARN the connection
/// reports reads the credential last written to it.
///
/// `Err` carries the rendering of what came back, as in [`store_connection_secret`].
pub async fn rotate_connection_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    connection_arn: &str,
    secret_id: &str,
    document: &str,
) -> Result<(), String> {
    let acting = ServicePrincipalCall::begin(engine, ctx, connection_arn)
        .ok_or_else(|| unactable_principal("rotated", connection_arn))?;
    acting
        .secrets_manager_client(ctx, region)?
        .put_secret_value()
        .secret_id(secret_id)
        .client_request_token(fresh_version_token())
        .secret_string(document)
        .send()
        .await
        .map_err(|error| rendered(&error))?;
    Ok(())
}

/// Delete the secret `secret_id` names, so the credential a connection held can no longer
/// be read once the connection is gone.
///
/// The delete goes through Secrets Manager's own `DeleteSecret`, which owns a secret's
/// lifecycle, and it takes the secret out of service AT ONCE rather than into a recovery
/// window: the secret belongs to this service and nobody else could take it back out of
/// one, and a connection whose credential is still readable is exactly what reporting an
/// ARN for a deleted connection would mean.
///
/// `Err` carries the rendering of what came back, as in [`store_connection_secret`].
pub async fn release_connection_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    connection_arn: &str,
    secret_id: &str,
) -> Result<(), String> {
    let acting = ServicePrincipalCall::begin(engine, ctx, connection_arn)
        .ok_or_else(|| unactable_principal("deleted", connection_arn))?;
    acting
        .secrets_manager_client(ctx, region)?
        .delete_secret()
        .secret_id(secret_id)
        .force_delete_without_recovery(true)
        .send()
        .await
        .map_err(|error| rendered(&error))?;
    Ok(())
}

/// Record `secret_arn` on the connection this call created or updated it for — or DELETE
/// the secret when this call no longer names that connection.
///
/// A connection deleted while its secret was being written, or a name another create has
/// taken since, is a different resource: the secret this call made belongs to neither, so
/// it is released rather than named by one of them. The release is best effort — the
/// answer to the caller is about its own request and a Secrets Manager call that fails
/// here cannot change it — so it is recorded in the trace and the answer stands.
///
/// Call it with NO store guard held: it awaits.
pub async fn record_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    connection_name: &str,
    connection_arn: &str,
    secret_arn: String,
) {
    let orphaned = match super::service_state(engine) {
        Some(state) => {
            let mut store = state.lock();
            match store.connection_mut(connection_name) {
                Some(connection) if connection.arn == connection_arn => {
                    connection.secret_arn = Some(secret_arn);
                    None
                }
                _ => Some(secret_arn),
            }
        }
        None => Some(secret_arn),
    };
    let Some(secret_arn) = orphaned else {
        return;
    };
    if let Err(detail) =
        release_connection_secret(engine, ctx, region, connection_arn, &secret_arn).await
    {
        ::tracing::debug!(
            connection = connection_arn,
            secret = secret_arn,
            %detail,
            "the secret written for a connection this call no longer names was not deleted"
        );
    }
}

/// The write a connection's secret is owed once the credential the connection holds has
/// been replaced — what an `UpdateConnection` that supplied authorization parameters
/// leaves to be done after the store guard is released.
///
/// Read off the STORED connection ([`Self::owed`]) rather than off the request, so what
/// goes into the secret is what the connection holds.
pub struct CredentialWrite {
    /// The connection whose secret this is, which the write is made on behalf of.
    connection_arn: String,
    /// The secret the connection already names, which gets a new VERSION.
    secret_arn: Option<String>,
    /// The name the secret of a connection naming none yet is created under.
    name: String,
    /// The credential, as the secret holds it ([`credential_document`]).
    document: String,
}

/// Names the connection and its secret, and never what the write is CARRYING: the
/// document is the credential itself, so no formatting of this value — a trace field, a
/// panic message — can put it anywhere but in the secret it is written to.
impl ::std::fmt::Debug for CredentialWrite {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("CredentialWrite")
            .field("connection_arn", &self.connection_arn)
            .field("secret_arn", &self.secret_arn)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl CredentialWrite {
    /// The write `connection` is owed, or `None` when it holds no credential to write and
    /// so has nothing to put in a secret.
    #[must_use]
    pub fn owed(connection: &super::ConnectionRecord) -> Option<Self> {
        Some(Self {
            connection_arn: connection.arn.clone(),
            secret_arn: connection.secret_arn.clone(),
            name: connection_secret_name_for(&connection.arn)?,
            document: credential_document(&connection.secret)?,
        })
    }

    /// Perform it: a new VERSION of the secret the connection already names, so the ARN a
    /// read reports is unchanged and the secret's versions are the connection's credential
    /// history — or the secret itself for a connection that names none yet, whose ARN is
    /// then recorded on it.
    ///
    /// A secret this connection names can only have been removed by this service, which
    /// removes it with the connection, so a version this emulator could not stake is
    /// recorded in the trace rather than answered by creating a second secret for one
    /// connection.
    ///
    /// Call it with NO store guard held: it awaits.
    pub async fn perform<Eng: Send + Sync + 'static>(
        self,
        engine: &Eng,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
        connection_name: &str,
    ) {
        if let Some(secret_arn) = self.secret_arn.as_deref() {
            if let Err(detail) = rotate_connection_secret(
                engine,
                ctx,
                region,
                &self.connection_arn,
                secret_arn,
                &self.document,
            )
            .await
            {
                ::tracing::debug!(
                    connection = self.connection_arn,
                    secret = secret_arn,
                    %detail,
                    "the connection's new credential was not staked on its secret"
                );
            }
            return;
        }
        match store_connection_secret(
            engine,
            ctx,
            region,
            &self.connection_arn,
            &self.name,
            &self.document,
        )
        .await
        {
            Ok(secret_arn) => {
                record_secret(
                    engine,
                    ctx,
                    region,
                    connection_name,
                    &self.connection_arn,
                    secret_arn,
                )
                .await;
            }
            Err(detail) => ::tracing::debug!(
                connection = self.connection_arn,
                %detail,
                "the connection's credential was not stored in AWS Secrets Manager"
            ),
        }
    }
}

/// A fresh version token for a write that stakes a version, as
/// `CreateSecretRequest$ClientRequestToken` takes one: a UUID, which is what that member
/// documents ("If you use the Amazon Web Services SDK … it generates a random UUID for
/// you") and satisfies `SecretVersionIdType`'s 32..64 characters.
fn fresh_version_token() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// What came back from a call, rendered for a trace — the whole error chain, which is what
/// says which failure this was.
fn rendered<E: ::std::error::Error>(error: &E) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
}

/// The ARN Secrets Manager allocated, or the failure of an answer that carried none: a
/// connection reports the ARN THAT service allocated and never one this service built, so
/// no connection can name a secret nothing holds.
fn allocated_arn(arn: Option<String>, call: &str) -> Result<String, String> {
    arn.filter(|arn| !arn.is_empty())
        .ok_or_else(|| format!("AWS Secrets Manager answered {call} with no secret ARN"))
}

/// The sentence a call that could not be made as this service's own principal is reported
/// with.
///
/// Such a call cannot touch that secret at all — the reserved name is refused for anybody
/// else, and so is a write to a secret this service owns — so it is reported as this
/// emulator failing rather than quietly creating or changing an ordinary secret of the
/// account.
fn unactable_principal(what: &str, connection_arn: &str) -> String {
    format!(
        "the connection's secret could not be {what} as the {principal} service principal \
         for {connection_arn}",
        principal = super::SERVICE_PRINCIPAL
    )
}

/// How long a record of one in-flight call stays readable when nothing has dropped it.
///
/// The guard below is what ENDS a record; this is the floor that bounds one nothing
/// dropped. It is far above the time an in-process call takes (there is no socket and no
/// retry) and far below any human timescale, so a record can neither expire while the
/// serving handler is still deciding the call nor accumulate.
const IN_FLIGHT_RECORD_TTL: Duration = Duration::from_secs(60);

/// One call this service is making as its own service principal: the credential it is
/// signed with, and the record that says whose call it is.
struct ServicePrincipalCall<'a, Eng: Send + Sync + 'static> {
    engine: &'a Eng,
    credentials: &'a ::emulator_server::CredentialStore,
    /// The access key id of the credential minted for this one call.
    access_key_id: String,
    /// Its secret, which signs this one call and goes nowhere else.
    secret: String,
}

/// Prints the access key id of the credential, never its secret.
impl<Eng: Send + Sync + 'static> ::std::fmt::Debug for ServicePrincipalCall<'_, Eng> {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ServicePrincipalCall")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

impl<'a, Eng: Send + Sync + 'static> ServicePrincipalCall<'a, Eng> {
    /// Begin a call made as this service's own service principal
    /// ([`super::SERVICE_PRINCIPAL`]) on behalf of the connection `source_arn` names.
    ///
    /// `None` when the call cannot be made that way: an engine carrying no directory of
    /// such calls, a resource ARN naming no account (no record could carry the account a
    /// reader matches on), or a directory already holding as many in-flight calls as it
    /// records.
    fn begin(
        engine: &'a Eng,
        ctx: &'a ::smithy_server_runtime::OperationContext<'_>,
        source_arn: &str,
    ) -> Option<Self> {
        let directory = in_flight_calls(engine)?;
        let source_account = ::service_common::arn::parse(source_arn)
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
                principal: super::SERVICE_PRINCIPAL.to_string(),
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
    /// The transport is the emulator's own, so everything a network client needs and this
    /// one cannot have is disabled: retries, timeouts, stalled-stream protection and
    /// identity caching each need a sleep implementation the callback transport is not
    /// given, and there is nothing behind it to retry. The Region is the one the request
    /// being served was signed for — a connection's secret lives where the connection
    /// does — and it is read as a Region LABEL (`::service_common::arn::region_label`), so
    /// caller-supplied text can never name an authority of its own.
    fn secrets_manager_client(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Result<::aws_sdk_secretsmanager::Client, String> {
        let region = ::service_common::arn::region_label(region).ok_or_else(|| {
            "the request names no Region this emulator can reach AWS Secrets Manager in"
                .to_string()
        })?;
        let config = ::aws_sdk_secretsmanager::Config::builder()
            .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
            .region(::aws_sdk_secretsmanager::config::Region::new(
                region.to_string(),
            ))
            .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
            .credentials_provider(::aws_sdk_secretsmanager::config::Credentials::new(
                self.access_key_id.clone(),
                self.secret.clone(),
                None,
                None,
                "aws-emulator",
            ))
            .http_client(ctx.callback().http_client())
            .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        Ok(::aws_sdk_secretsmanager::Client::from_conf(config))
    }
}

/// Dropping the call forgets its record and DELETES its credential: an identity nothing
/// holds can sign nothing, and a record nothing holds decides nothing.
impl<Eng: Send + Sync + 'static> Drop for ServicePrincipalCall<'_, Eng> {
    fn drop(&mut self) {
        if let Some(directory) = in_flight_calls(self.engine) {
            directory.lock().forget(&self.access_key_id);
        }
        let _ = self.credentials.delete(&self.access_key_id);
    }
}

/// The shared directory of the calls a service is making as its own service principal, in
/// the one shared engine: written here and read by the service serving the call.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::invocation::{ApiKeyCredential, BasicCredential};
    use crate::state::oauth::OAuthCredential;

    /// A connection's secret is named under the prefix AWS reserves for this service, and
    /// then by the connection it belongs to: the rows differ along the characters a
    /// `ConnectionName` may carry and along the length of the name.
    #[test]
    fn a_connections_secret_is_named_for_the_connection_it_belongs_to() {
        // The charset `CreateSecretRequest$Name` documents, as the Secrets Manager
        // emulator states it (`state::SECRET_NAME_EXTRA_CHARS`).
        const EXTRA: &str = "/_+=.@-";
        let id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
        for case in [
            "conn",
            "a.connection-with_every.admitted-character_0",
            &"c".repeat(64),
        ] {
            let name = connection_secret_name(case, id);
            let expected = format!("events!connection/{case}/{id}");
            assert_eq!(
                name, expected,
                "case {case:?}: expected {expected:?}, got {name:?}"
            );
            let Some(rest) = name.strip_prefix(CONNECTION_SECRET_NAME_PREFIX) else {
                panic!("case {case:?}: expected the reserved prefix, got {name:?}");
            };
            assert!(
                rest.chars()
                    .all(|c| c.is_ascii_alphanumeric() || EXTRA.contains(c)),
                "case {case:?}: expected only characters a secret name admits after the \
                 reserved mark, got {rest:?}"
            );
            assert!(
                !name.is_empty() && name.len() <= 512,
                "case {case:?}: expected a name inside CreateSecret's 1..512, got {} \
                 characters",
                name.len()
            );
        }
    }

    /// The document holds the credential members of the one authorization block in force,
    /// and nothing else: the rows differ along the authorization type, and the last one
    /// is a connection holding no credential at all.
    #[test]
    fn the_document_holds_the_credential_the_connection_was_given() {
        let api_key = ConnectionSecret {
            api_key: Some(ApiKeyCredential {
                name: "x-api-key".to_string(),
                value: "k-0".to_string(),
            }),
            ..ConnectionSecret::default()
        };
        let basic = ConnectionSecret {
            basic: Some(BasicCredential {
                username: "sam".to_string(),
                password: "p-0".to_string(),
            }),
            ..ConnectionSecret::default()
        };
        let oauth = ConnectionSecret {
            oauth: Some(OAuthCredential {
                client_id: "client-0".to_string(),
                client_secret: "s-0".to_string(),
                authorization_endpoint: "https://auth.example/token".to_string(),
                http_method: "POST".to_string(),
                http_parameters: None,
            }),
            ..ConnectionSecret::default()
        };
        for (case, secret, expected) in [
            (
                "an API key",
                api_key,
                Some(::serde_json::json!({
                    "api_key_name": "x-api-key",
                    "api_key_value": "k-0",
                })),
            ),
            (
                "basic credentials",
                basic,
                Some(::serde_json::json!({ "username": "sam", "password": "p-0" })),
            ),
            (
                "client credentials",
                oauth,
                Some(::serde_json::json!({
                    "client_id": "client-0",
                    "client_secret": "s-0",
                })),
            ),
            ("no credential at all", ConnectionSecret::default(), None),
        ] {
            let written = credential_document(&secret);
            let parsed = written
                .as_deref()
                .map(|text| {
                    ::serde_json::from_str::<::serde_json::Value>(text)
                        .unwrap_or_else(|error| panic!("case {case:?}: {text:?} is not JSON: {error}"))
                });
            assert_eq!(
                parsed, expected,
                "case {case:?}: expected {expected:?}, got {parsed:?}"
            );
        }
    }
}
