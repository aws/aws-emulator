// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services Secrets Manager secrets PCS GENERATES for a cluster's Slurm keys,
//! and the members a cluster reports them by.
//!
//! A Slurm cluster is held together by a SHARED KEY: every daemon authenticates with the same
//! secret, so anything a client provisions that has to talk to the cluster's controller — a
//! login node, a Slurm client outside the compute node groups, tooling that reads the cluster
//! secret — needs to read that key. PCS generates it for the cluster it creates, keeps it in a
//! Secrets Manager secret, and reports that secret as the cluster's
//! `slurmConfiguration.authKey`: "The shared Slurm key for authentication, also known as the
//! cluster secret", by the `secretArn` and `secretVersion` that shape declares `@required`.
//! A cluster whose Slurm REST API is enabled needs a second key, the one the REST daemon
//! verifies its JSON web tokens with, and PCS keeps that in a secret of its own, reported as
//! `slurmConfiguration.jwtAuth.jwtKey` — "the Amazon Web Services Secrets Manager secret
//! containing the JWT key".
//!
//! THE RULE this module implements: what a cluster reports for either key names a secret this
//! emulator REALLY HOLDS. Each secret is created through Secrets Manager's own `CreateSecret`,
//! so `GetSecretValue` on the ARN a cluster reports answers with the key material, and the
//! version a cluster reports is the version that service staked. Secrets Manager is reached
//! the way this service reaches every other — over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process; the Region the request was signed for is
//! read as a Region LABEL before it can reach an endpoint authority, exactly as
//! [`super::ec2_client`] reads it.
//!
//! WHO creates it matters as much as that it exists. Such a secret is PCS's OWN: it is named
//! under the prefix AWS reserves for the service that manages a secret
//! ([`MANAGED_SECRET_NAME_PREFIX`]) and is not a caller's to change or delete, which is true
//! only of a create made as this service's own service principal
//! ([`super::service_principal_call`]).
//!
//! Nothing is reported for a secret that was not created. A hop this emulator could not make
//! — no Secrets Manager mounted beside this service, no Region label to ask in, a store that
//! would not take the secret — leaves the member ABSENT rather than reporting an ARN no secret
//! answers for, which is the same stance [`super::ec2_subnets`] takes on silence; the failure
//! is logged, because it IS why a cluster reports no key.
//!
//! Minting happens on the paths that CHANGE a cluster — its create, and an update that turns
//! the Slurm REST API on — and never on a read: a cluster already holding a key is never
//! given a second one ([`owed`]), so reading a cluster changes nothing a later caller sees.

use super::{ClusterRecord, ClusterSlurmConfiguration, JwtAuth, JwtKey, SlurmAuthKey};

/// The prefix every secret PCS manages is named under: this service's own id and the character
/// AWS reserves for a secret an Amazon Web Services service owns.
///
/// A secret in this form is not a caller's to create — the mark is not one of the characters a
/// secret name may carry ("ASCII letters, numbers, and the following characters: /_+=.@-",
/// `CreateSecretRequest$Name`) — which is what makes the name itself say the secret is this
/// service's. It is accepted from the service whose own principal is making the create
/// ([`super::service_principal_call`]).
pub const MANAGED_SECRET_NAME_PREFIX: &str = "pcs!";

/// The name of the secret holding one cluster's shared Slurm key.
///
/// The fixed text is the reserved prefix and the kind of key the secret holds; what makes it
/// the secret of ONE cluster is that cluster's generated id, which never changes and is never
/// minted twice — so a cluster's secret is still its own after the cluster is renamed, and a
/// cluster created where an earlier one was deleted never collides with the name that one
/// held.
#[must_use]
pub fn auth_key_secret_name(cluster_id: &str) -> String {
    format!("{MANAGED_SECRET_NAME_PREFIX}slurm-secret-{cluster_id}")
}

/// The name of the secret holding one cluster's JWT key, formed like
/// [`auth_key_secret_name`] and naming the other kind of key, so one cluster's two secrets
/// can never be one secret.
#[must_use]
pub fn jwt_key_secret_name(cluster_id: &str) -> String {
    format!("{MANAGED_SECRET_NAME_PREFIX}slurm-jwt-{cluster_id}")
}

/// The member of a cluster secret's JSON document the shared Slurm key is carried under. The
/// document is what a client reads the key material out of, and the key is binary, so it is
/// carried base64-encoded ([`generated_key`]).
const AUTH_KEY_DOCUMENT_MEMBER: &str = "slurm_key";

/// The member of the JWT secret's document the JWT key is carried under, written the same way.
const JWT_KEY_DOCUMENT_MEMBER: &str = "jwt_key";

/// How many bytes of key material one generated Slurm key carries: a 256-bit shared key, which
/// is the width of the HMAC-SHA256 token signing the Slurm REST API's JSON web tokens use and
/// enough for the controller's shared key.
const SLURM_KEY_BYTES: i64 = 32;

/// Which of a cluster's two keys PCS still owes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Owed {
    /// The shared Slurm key, which EVERY cluster has: `SchedulerType` declares one member,
    /// `SLURM` (`smithy_query(query="shape",shape="SchedulerType")`), so every cluster this
    /// service provisions is a Slurm cluster with a cluster secret of its own.
    pub auth_key: bool,
    /// The JWT key, which a cluster whose Slurm REST API is enabled has: the member is "the
    /// JWT authentication configuration for Slurm REST API access"
    /// (`ClusterSlurmConfiguration$jwtAuth`), and the REST API is on exactly when the
    /// cluster's `slurmRest.mode` is `STANDARD` ([`ClusterRecord::slurm_rest_enabled`]).
    pub jwt_key: bool,
}

impl Owed {
    /// Whether anything is owed at all, so a call that owes nothing makes no hop.
    #[must_use]
    pub fn any(self) -> bool {
        self.auth_key || self.jwt_key
    }
}

/// The keys one call generated, each `None` when it was not owed or could not be created.
#[derive(Debug, Clone, Default)]
pub struct Minted {
    /// The shared Slurm key, as the cluster reports it.
    pub auth_key: Option<SlurmAuthKey>,
    /// The JWT key, as the cluster reports it inside `jwtAuth`.
    pub jwt_key: Option<JwtKey>,
}

impl Minted {
    /// Whether this call generated anything, so a record is only written when it did.
    #[must_use]
    fn any(&self) -> bool {
        self.auth_key.is_some() || self.jwt_key.is_some()
    }

    /// The ARNs of the secrets this call created, for the caller that has to release the ones
    /// the stored cluster did not take.
    #[must_use]
    pub fn secret_arns(&self) -> Vec<String> {
        self.auth_key
            .as_ref()
            .map(|key| key.secret_arn.clone())
            .into_iter()
            .chain(self.jwt_key.as_ref().map(|key| key.secret_arn.clone()))
            .collect()
    }
}

/// The keys `record` is still owed.
///
/// A key is owed only while the record carries NONE: the secret a cluster already holds is the
/// cluster's key, so no later call generates a second one and no read can move what a cluster
/// reports.
#[must_use]
pub fn owed(record: &ClusterRecord) -> Owed {
    let slurm = record.slurm_configuration.as_ref();
    Owed {
        auth_key: slurm.is_none_or(|slurm| slurm.auth_key.is_none()),
        jwt_key: record.slurm_rest_enabled()
            && slurm.is_none_or(|slurm| {
                slurm
                    .jwt_auth
                    .as_ref()
                    .is_none_or(|jwt_auth| jwt_auth.jwt_key.is_none())
            }),
    }
}

/// Write the keys `minted` onto the cluster's stored Slurm configuration: the ONE place the
/// members a cluster reports them by are set, so a create and an update cannot disagree about
/// where a cluster's keys live.
///
/// A cluster whose create carried no `slurmConfiguration` at all still has a cluster secret, so
/// the configuration it reports the key through is created here, carrying the key and nothing
/// else — none of the members a request would have named.
///
/// A member the record ALREADY carries is left exactly as it is, and the key that was generated
/// for it is answered instead: the secret a cluster holds is the cluster's key, so nothing moves
/// it — not even a call that decided the key was owed and was overtaken by another while it was
/// generating one. The caller releases what comes back ([`release`]), so the secret nothing
/// reports is not left behind.
#[must_use]
pub fn record(record: &mut ClusterRecord, minted: Minted) -> Vec<String> {
    if !minted.any() {
        return Vec::new();
    }
    let mut unused = Vec::new();
    let slurm = record
        .slurm_configuration
        .get_or_insert_with(ClusterSlurmConfiguration::default);
    if let Some(auth_key) = minted.auth_key {
        if slurm.auth_key.is_none() {
            slurm.auth_key = Some(auth_key);
        } else {
            unused.push(auth_key.secret_arn);
        }
    }
    if let Some(jwt_key) = minted.jwt_key {
        let held = slurm
            .jwt_auth
            .as_ref()
            .is_some_and(|jwt_auth| jwt_auth.jwt_key.is_some());
        if held {
            unused.push(jwt_key.secret_arn);
        } else {
            slurm.jwt_auth = Some(JwtAuth {
                jwt_key: Some(jwt_key),
            });
        }
    }
    unused
}

/// The ARNs of the secrets this cluster holds, in the order its members declare them.
///
/// Read off the record, so the secrets released when a cluster is deleted are exactly the ones
/// it reported.
#[must_use]
pub fn held(record: &ClusterRecord) -> Vec<String> {
    let slurm = record.slurm_configuration.as_ref();
    let auth_key = slurm.and_then(|slurm| slurm.auth_key.as_ref());
    let jwt_key = slurm
        .and_then(|slurm| slurm.jwt_auth.as_ref())
        .and_then(|jwt_auth| jwt_auth.jwt_key.as_ref());
    auth_key
        .map(|key| key.secret_arn.clone())
        .into_iter()
        .chain(jwt_key.map(|key| key.secret_arn.clone()))
        .collect()
}

/// Take the cluster's keys OFF the record and report the ARNs of the secrets they lived in,
/// for the call that releases them.
///
/// The members go with the secrets: a cluster whose secrets have been released reports no key
/// at all, rather than an ARN naming a secret that is gone.
pub fn take(record: &mut ClusterRecord) -> Vec<String> {
    let released = held(record);
    if let Some(slurm) = record.slurm_configuration.as_mut() {
        slurm.auth_key = None;
        slurm.jwt_auth = None;
    }
    released
}

/// Generate the keys `owed` says the cluster is owed and report what was created.
///
/// `cluster_arn` is the resource the secrets belong to: it is what the create is made ON BEHALF
/// OF, it says which Region they are created in ([`secrets_region`]), and with `cluster_id` it
/// is what names them ([`auth_key_secret_name`]). Every failure here is this EMULATOR failing
/// to do what PCS does — Secrets Manager not mounted beside this service, a Region this
/// emulator cannot ask in, a store that would not take the secret — so it reports no key for
/// the one that failed and leaves the cluster to report none, rather than an ARN nothing
/// answers for.
pub async fn mint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    cluster_arn: &str,
    cluster_id: &str,
    owed: Owed,
) -> Minted {
    let mut minted = Minted::default();
    if !owed.any() {
        return minted;
    }
    let region = secrets_region(cluster_arn).unwrap_or_default();
    let Some(secrets) = secrets_manager_client(ctx, &region) else {
        unreported(cluster_id, "the cluster's own ARN names no Region to ask in");
        return minted;
    };
    if owed.auth_key {
        minted.auth_key = created(
            engine,
            ctx,
            &region,
            &secrets,
            cluster_arn,
            &auth_key_secret_name(cluster_id),
            AUTH_KEY_DOCUMENT_MEMBER,
        )
        .await
        .map(|(secret_arn, secret_version)| SlurmAuthKey {
            secret_arn,
            secret_version,
        });
        if minted.auth_key.is_none() {
            unreported(cluster_id, "the shared Slurm key was not created");
        }
    }
    if owed.jwt_key {
        minted.jwt_key = created(
            engine,
            ctx,
            &region,
            &secrets,
            cluster_arn,
            &jwt_key_secret_name(cluster_id),
            JWT_KEY_DOCUMENT_MEMBER,
        )
        .await
        .map(|(secret_arn, secret_version)| JwtKey {
            secret_arn,
            secret_version,
        });
        if minted.jwt_key.is_none() {
            unreported(cluster_id, "the JWT key was not created");
        }
    }
    minted
}

/// Release the secrets `secret_arns` names, each of which was created for the cluster
/// `cluster_arn`: the ones it held as it is taken out, and the ones a call generated that the
/// stored cluster did not take ([`record`]).
///
/// The delete goes through Secrets Manager's own `DeleteSecret`, which owns a secret's
/// lifecycle, in the Region the cluster's own ARN names ([`secrets_region`]), and is made as
/// this service's own principal for the same reason the create is: the lifecycle of a secret
/// PCS owns is PCS's, and nobody else's call on it is served. It is a delete WITHOUT a recovery
/// window, because a secret whose cluster is gone is one no caller could restore and nothing
/// can use.
pub async fn release<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    cluster_arn: &str,
    secret_arns: &[String],
) {
    if secret_arns.is_empty() {
        return;
    }
    let Some(acting) =
        super::service_principal_call::ServicePrincipalCall::begin(engine, ctx, cluster_arn)
    else {
        ::tracing::warn!(
            cluster = cluster_arn,
            "the secrets a PCS cluster held were not released: this emulator could not make \
             the call as its own service principal"
        );
        return;
    };
    let region = secrets_region(cluster_arn).unwrap_or_default();
    let Some(secrets) = acting.secrets_manager_client(ctx, &region) else {
        ::tracing::warn!(
            cluster = cluster_arn,
            "the secrets a PCS cluster held were not released: its own ARN names no Region to \
             ask in"
        );
        return;
    };
    for secret_arn in secret_arns {
        let answer = secrets
            .delete_secret()
            .secret_id(secret_arn)
            .force_delete_without_recovery(true)
            .send()
            .await;
        if let Err(error) = answer {
            ::tracing::warn!(
                secret = secret_arn.as_str(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a secret a PCS cluster held was not released"
            );
        }
    }
}

/// The Region a cluster's secrets are created and released in: the one the CLUSTER's own ARN
/// names, read through the tree's one reading of that grammar (`service_common::arn::parse`),
/// and `None` for a value that names none.
///
/// A secret belongs to one Region and resolves in no other, so the Region a cluster's keys live
/// in is the Region the CLUSTER lives in — not the Region a later request happened to be signed
/// for, which would create a key beside a cluster that cannot resolve it and would leave the
/// cluster's secrets behind when it is deleted.
#[must_use]
fn secrets_region(cluster_arn: &str) -> Option<String> {
    ::service_common::arn::parse(cluster_arn)
        .map(|arn| arn.region.to_string())
        .filter(|region| !region.is_empty())
}

/// Create one secret holding a freshly generated key under `document_member`, and report the
/// ARN Secrets Manager allocated and the version it staked — `None` when the secret was not
/// created, or was created without either.
async fn created<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secrets: &::aws_sdk_secretsmanager::Client,
    cluster_arn: &str,
    name: &str,
    document_member: &str,
) -> Option<(String, String)> {
    let key = generated_key(secrets).await?;
    // The secret is one PCS OWNS, so the create is made as PCS's OWN service principal acting
    // for the cluster the secret belongs to: that is what lets it be named under the reserved
    // prefix no caller can mint and report this service as its owner. A call that cannot be
    // made that way cannot create that secret AT ALL — the reserved name is refused for
    // anybody else — so nothing is created rather than an ordinary secret of the account
    // being created under a name a caller could mint and change.
    let acting =
        super::service_principal_call::ServicePrincipalCall::begin(engine, ctx, cluster_arn)?;
    let created = acting
        .secrets_manager_client(ctx, region)?
        .create_secret()
        .name(name)
        .client_request_token(fresh_version_token())
        .secret_string(key_document(document_member, &key))
        .send()
        .await;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            ::tracing::warn!(
                secret = name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a Slurm key secret could not be created in Amazon Web Services Secrets Manager"
            );
            return None;
        }
    };
    Some((
        created.arn()?.to_string(),
        created.version_id()?.to_string(),
    ))
}

/// The key material one secret holds: [`SLURM_KEY_BYTES`] bytes from Secrets Manager's OWN
/// generator (`GetRandomPassword`), carried base64-encoded, so a client that reads the secret
/// decodes the document member into the bytes the daemons share.
///
/// The generator is asked for one character per byte with no punctuation and no space, so the
/// material is the same width whatever the generator is asked for, and the codec is the
/// standard-alphabet one the wire serializer already uses (`aws_smithy_types::base64`).
async fn generated_key(secrets: &::aws_sdk_secretsmanager::Client) -> Option<String> {
    let generated = secrets
        .get_random_password()
        .password_length(SLURM_KEY_BYTES)
        .exclude_punctuation(true)
        .include_space(false)
        .send()
        .await;
    match generated {
        Ok(generated) => Some(::aws_smithy_types::base64::encode(
            generated.random_password()?,
        )),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a Slurm key could not be generated in Amazon Web Services Secrets Manager"
            );
            None
        }
    }
}

/// The JSON document one key secret holds: the key material under the member naming the key.
fn key_document(document_member: &str, key: &str) -> String {
    ::serde_json::json!({ document_member: key }).to_string()
}

/// A version id for the secret version this call stakes.
///
/// `ClientRequestToken` becomes the version's id, and "if you generate a raw HTTP request to
/// the Secrets Manager service endpoint, then you must generate a ClientRequestToken and
/// include it in the request" — which is the position this service is in, since it is PCS that
/// creates the secret. The bytes come from the operating system's own entropy and are rendered
/// as the UUID such a token is written as (`service_common::uuid_from_random_bytes`, this
/// tree's one rendering of 16 opaque bytes), so two calls never stake the same version.
fn fresh_version_token() -> String {
    let mut bytes = [0_u8; 16];
    if ::getrandom::fill(&mut bytes).is_err() {
        // Entropy the OS would not hand over: the token still has to be unique, and the
        // clock's reading is what this process can still tell two calls apart by.
        bytes = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
            .to_le_bytes();
    }
    ::service_common::uuid_from_random_bytes(bytes)
}

/// Record that a cluster reports no key, and why.
///
/// At `warn`: a deployment where this hop fails is one where every cluster reports one key
/// fewer, which is worth seeing, and this IS the reason.
fn unreported(cluster_id: &str, reason: &str) {
    ::tracing::warn!(cluster = cluster_id, %reason, "a PCS cluster reports no Slurm key");
}

/// The Secrets Manager client this service generates key material with, over the in-process
/// callback, signed with the emulator's own credentials — `None` when there is no Region label
/// to ask in.
///
/// The create of a secret is NOT made with this client: it is made as this service's own
/// service principal ([`super::service_principal_call`]), which is what a secret PCS owns can
/// be created by — through [`secrets_manager_client_signed_as`], the ONE construction of this
/// hop, so the two differ in the identity they present and in nothing else.
fn secrets_manager_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_secretsmanager::Client> {
    secrets_manager_client_signed_as(ctx, region, None)
}

/// The ONE Secrets Manager client of this service, signed as `signed_as` — or, where that is
/// `None`, with the emulator's own credentials. `None` when there is no Region label to ask in.
///
/// Built like [`super::ec2_client::ec2_client`], for the same reasons: the transport is
/// in-process, so there is nothing to retry, time out or watch for a stalled stream, and the
/// Region a request is signed for is read as a Region LABEL rather than interpolated as given,
/// because here it ends up in an endpoint URL's authority.
pub(super) fn secrets_manager_client_signed_as(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    signed_as: Option<::aws_sdk_secretsmanager::config::Credentials>,
) -> Option<::aws_sdk_secretsmanager::Client> {
    let label = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            label.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{label}.amazonaws.com"))
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        );
    let config = match signed_as {
        Some(credentials) => config.credentials_provider(credentials),
        None => config.credentials_provider(ctx.callback().aws_credentials_provider()),
    };
    Some(::aws_sdk_secretsmanager::Client::from_conf(config.build()))
}
