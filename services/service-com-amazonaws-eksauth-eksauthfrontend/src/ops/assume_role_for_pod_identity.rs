// SPDX-License-Identifier: CC0-1.0

//! @op AssumeRoleForPodIdentity
//!
//! The one operation EKS Auth binds: the EKS Pod Identity Agent presents the projected
//! Kubernetes service account token of a pod, and this operation exchanges it for the
//! temporary IAM credentials of the role the cluster's pod identity association binds
//! that service account to.
//!
//! Every output member is computed from state (see [`crate::state`]):
//!
//! 1. the `token` must be a JSON Web Token — three base64url segments whose first is a
//!    JOSE header, a JSON object naming the algorithm and the key the token was signed
//!    with — otherwise `InvalidTokenException` (400). This is a question about the VALUE,
//!    so it is answered BEFORE any state is read: a token that is not a JWT is refused as
//!    such whether or not the cluster the URI names exists. Reading the value stops at
//!    the header, so the segments past it are never decoded and no claim is ever read:
//!    the modelled `ExpiredTokenException` is reachable only from the issuing cluster's
//!    own record of what it issued (step 4), never from an `exp` the caller wrote;
//! 2. the `kid` the header names must resolve to a key some cluster this service holds
//!    publishes — otherwise the header names no key any token this service can read was
//!    signed with, and the answer is `InvalidTokenException` (400) again. Resolving the
//!    key id is the last layer of reading the header, and it is reached for every
//!    `clusterName` the model admits: the key material a request presents is judged
//!    before the label is resolved as a resource, so a caller holding no key material a
//!    cluster published learns nothing here about which cluster names exist;
//! 3. the `clusterName` label must name a cluster this service holds — otherwise the
//!    request addresses a resource that is not there and the answer is the modelled
//!    `ResourceNotFoundException` (404). The label is a RESOURCE identity, so it is
//!    resolved as one, once the presented header has resolved: that is what lets a caller
//!    holding a cluster-issued token tell "this agent is pointed at a cluster that is not
//!    there" (non-retryable) from "my projected token is stale" (re-read it and retry).
//!    Only a caller IAM has allowed this action on that cluster gets this far (see
//!    `crate::service::identity_authorization`, which runs ahead of this handler). The
//!    key the header names then has to be one THAT cluster publishes, since a key another
//!    cluster published signed no token for this one — the same header refusal as above;
//! 4. the token must be one that cluster issued for the pod identity audience and still
//!    within its lifetime — otherwise `ExpiredTokenException` or `InvalidTokenException`
//!    (400), decided by the cluster's own record of what it issued, never by inspecting
//!    the request; where that verdict is about a CLAIM, the refusal names the claim and
//!    the value it must carry (see [`token_rejection`]);
//! 5. the cluster must hold a pod identity association for the (namespace, service
//!    account) pair the token names — otherwise `ResourceNotFoundException` (404), the
//!    answer a token whose association was deleted, or never created, gets;
//! 6. the credentials are minted through the emulator's credential store, so the
//!    session they identify is one the emulator will accept, and the session record is
//!    stored under its access key id with the instant it expires.
//!
//! A value that fails a constraint the MODEL declares never reaches this handler at all:
//! the framework refuses it first, and `crate::service` words that refusal as this
//! service's own `InvalidParameterException`.
//!
//! Nor does a request IAM does not allow: the caller's own identity-based policies are
//! judged against `eks-auth:AssumeRoleForPodIdentity` on the cluster the request addresses
//! BEFORE either of those, in `crate::service::identity_authorization`, which answers the
//! modelled `AccessDeniedException` (400) itself. So the `AccessDeniedException` this file
//! constructs is only the other thing that shape says — the cluster holds no pod identity
//! association for the service account the token names (step 5).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions` —
//! plus the emulator's credential store, which is where the returned credentials are
//! created.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssumeRoleForPodIdentity`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json")
//! ```
//!
//! The Amazon EKS Auth API and the AssumeRoleForPodIdentity action are only used by the EKS
//! Pod Identity Agent. We recommend that applications use the Amazon Web Services SDKs to
//! connect to Amazon Web Services services; if credentials from an EKS Pod Identity
//! association are available in the pod, the latest versions of the SDKs use them
//! automatically.
//!
//! ## Input
//!
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of the cluster for the request.
//! * `eksNodeName` (String) — The Kubernetes node name of the worker node where the pod is
//!   running.
//! * `instanceId` (String) — The Amazon EC2 instance ID of the worker node where the pod is
//!   running.
//! * `token` (JwtToken, len 1.., pattern ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$)
//!   **required** — The token of the Kubernetes service account for the pod.
//! * `zone` (String) — The Availability Zone ID of the worker node where the pod is running.
//!
//! ## Output
//!
//! * `assumedRoleUser` (AssumedRoleUser) **required** — An object with the permanent IAM role
//!   identity and the temporary session name. The ARN of the IAM role that the temporary
//!   credentials authenticate to. The session name of the temporary session requested to STS.
//!   The value is a unique identifier that contains the role ID, a colon (:), and the role
//!   session name of the role that is being assumed. The role ID is generated by IAM when the
//!   role is created. The role session name part of the value follows this format:
//!   eks-clustername-podname-random UUID
//! * `audience` (String) **required** — The identity that is allowed to use the credentials.
//!   This value is always pods.eks.amazonaws.com.
//! * `credentials` (Credentials) **required** — The Amazon Web Services Signature Version 4
//!   type of temporary credentials.
//! * `podIdentityAssociation` (PodIdentityAssociation) **required** — The Amazon Resource
//!   Name (ARN) and ID of the EKS Pod Identity association.
//! * `subject` (Subject) **required** — The name of the Kubernetes service account inside the
//!   cluster to associate the IAM credentials with.
//! * `workloadIdentityProviderArn` (String) — The ARN of the workload identity provider for
//!   the returned credentials. Returned only for associations with workload identity
//!   enabled.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `ExpiredTokenException` (HTTP 400) — The specified Kubernetes service account token is
//!   expired.
//! * `InternalServerException` (HTTP 500) — These errors are usually caused by a server-side
//!   issue.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — This exception is thrown if the request contains
//!   a semantic error. The precise meaning will depend on the API, and will be documented in
//!   the error message.
//! * `InvalidTokenException` (HTTP 400) — The specified Kubernetes service account token is
//!   invalid.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.
//! * `ThrottlingException` (HTTP 429) — The request was denied because your request rate is
//!   too high. Reduce the frequency of requests.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{
    parse_service_account_token, PodIdentitySession, TokenMalformation, TokenRejection,
    KEY_ID_HEADER, PODS_AUDIENCE, SESSION_DURATION,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssumeRoleForPodIdentityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eksauth-eksauthfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssumeRoleForPodIdentityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eksauth#AssumeRoleForPodIdentity")]
#[derive(Debug)]
pub enum AssumeRoleForPodIdentityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ExpiredTokenException(ExpiredTokenException),
    InternalServerException(InternalServerException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    InvalidTokenException(InvalidTokenException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::assume_role_for_pod_identity::AssumeRoleForPodIdentityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssumeRoleForPodIdentityError = crate::scaffolded::Scaffolded<AssumeRoleForPodIdentityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssumeRoleForPodIdentityErrorModeled> for AssumeRoleForPodIdentityError {
    fn from(modeled: AssumeRoleForPodIdentityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssumeRoleForPodIdentity` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssumeRoleForPodIdentity<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssumeRoleForPodIdentity<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssumeRoleForPodIdentity<Eng>
{
    type Input = AssumeRoleForPodIdentityInput;
    type Output = AssumeRoleForPodIdentityOutput;
    type Error = AssumeRoleForPodIdentityError;

    const NAME: &'static str = "AssumeRoleForPodIdentity";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSUME_ROLE_FOR_POD_IDENTITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSUME_ROLE_FOR_POD_IDENTITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            internal_server_error("the EKS Auth state store is unavailable on this engine")
        })?;

        // Who presented this request, for the access record below. It comes from a
        // request header, so it is recorded with `?` (Debug): the value is quoted and
        // its control characters escaped, and cannot forge a line in the record.
        let caller_akid = ::engine::extract_akid(&ctx.parts().headers);

        // WHETHER THE PRESENTED VALUE IS A TOKEN AT ALL comes first, before anything is
        // read from state. It is a question about the value, so the answer cannot depend
        // on which cluster the request names: a value that is not a JWT is the modeled
        // invalid-token failure even when the cluster in the URI is not registered.
        let token_headers =
            parse_service_account_token(&input.token).map_err(|malformation| {
                refused(&caller_akid, &input.cluster_name, "InvalidTokenException");
                malformed_token(malformation)
            })?;

        // A session whose expiration has passed is over whether or not anybody called,
        // and a token record far enough past its token's expiry is owed to nobody, so
        // materialise both here and forget the credentials with them. The state guard is
        // dropped before the credential store is touched.
        let now = SystemTime::now();
        let expired = state.lock().expire(now);
        for access_key_id in expired {
            match ctx.credential_store().delete(&access_key_id) {
                Ok(()) => ::tracing::info!(
                    operation = Self::NAME,
                    access_key_id = %access_key_id,
                    "forgot an expired EKS Pod Identity session"
                ),
                // The session record is gone but its credential is not: say so rather
                // than claim the session was forgotten.
                Err(error) => ::tracing::warn!(
                    operation = Self::NAME,
                    access_key_id = %access_key_id,
                    error = ?error,
                    "an expired EKS Pod Identity session's credential could not be deleted"
                ),
            }
        }

        // The whole authorization decision reads state: the cluster, its token signing
        // key, and its associations.
        let (association, identity, cluster_arn) = {
            let guard = state.lock();
            // RESOLVING THE KEY THE HEADER NAMES finishes reading that header, and it is
            // answered BEFORE the `clusterName` label is resolved as a resource: a `kid`
            // is resolved against the signing keys this service holds, so a key id no
            // cluster published names no key a token this service can read was signed
            // with — a verdict about the presented value alone. Answering it first is
            // what keeps this layer reachable for every name the model admits, and what
            // keeps a caller holding no key material from learning here whether a cluster
            // name resolves.
            if !guard.resolves_signing_key(&token_headers.key_id) {
                refused(&caller_akid, &input.cluster_name, "InvalidTokenException");
                return Err(malformed_token(TokenMalformation::InvalidHeader(
                    KEY_ID_HEADER,
                )));
            }
            // THE CLUSTER THE LABEL NAMES is resolved next, as the resource it is: a
            // `clusterName` that names no cluster is the modelled not-found, because the
            // request addresses a resource that is not there. It is reached by a caller
            // whose `kid` resolved, which is a caller that already holds key material
            // some cluster published. Only a caller IAM allowed this action on that
            // cluster reaches this line, so what it learns about the cluster is what it
            // was granted.
            let cluster = guard.cluster(&input.cluster_name).ok_or_else(|| {
                refused(&caller_akid, &input.cluster_name, "ResourceNotFoundException");
                no_cluster(&input.cluster_name)
            })?;
            // The key the header names has to be one THAT cluster publishes: a key id
            // another cluster published signed no token for this one, so the header still
            // names no key this request could have been signed with — the same refusal
            // the resolution above gives.
            if !cluster.publishes_signing_key(&token_headers.key_id) {
                refused(&caller_akid, &input.cluster_name, "InvalidTokenException");
                return Err(malformed_token(TokenMalformation::InvalidHeader(
                    KEY_ID_HEADER,
                )));
            }
            let identity = cluster
                .verify_service_account_token(&input.token, &token_headers, now)
                .map_err(|rejection| {
                    refused(
                        &caller_akid,
                        &input.cluster_name,
                        match rejection {
                            TokenRejection::Expired => "ExpiredTokenException",
                            TokenRejection::InvalidClaim { .. } | TokenRejection::Invalid => {
                                "InvalidTokenException"
                            }
                        },
                    );
                    token_rejection(rejection)
                })?;
            let association = cluster
                .association_for(&identity.namespace, &identity.service_account)
                .cloned()
                .ok_or_else(|| {
                    refused(&caller_akid, &input.cluster_name, "ResourceNotFoundException");
                    no_association()
                })?;
            (association, identity, cluster.arn.clone())
        };

        // The credentials this call returns: a SESSION OF THE ROLE the association binds
        // the service account to, minted where every session of this emulator is minted
        // and recorded where every service resolves a temporary credential's principal —
        // so what the pod may then do is what that role's policies allow, narrowed by the
        // association's own policy (see [`crate::state::vend_pod_identity_session`]).
        // Neither the id nor the secrets are chosen here.
        //
        // The role session name AWS documents for this value:
        // `eks-<clustername>-<podname>-<random UUID>`. The pod name comes from the
        // token's record of the pod it was projected into; a token issued without one (a
        // plain service account token) names its service account instead.
        let pod_name = identity
            .pod_name
            .clone()
            .unwrap_or_else(|| identity.service_account.clone());
        let session_name = format!(
            "eks-{}-{pod_name}-{}",
            input.cluster_name,
            crate::state::random_uuid()
        );
        // "EKS Pod Identity adds session tags" — unless the association turns them off:
        // "disableSessionTags" (`PodIdentityAssociation$disableSessionTags`).
        let tags = match association.disable_session_tags {
            Some(true) => Vec::new(),
            _ => crate::state::session_tags(&input.cluster_name, &cluster_arn, &identity),
        };
        let plan = crate::state::PodIdentitySessionPlan {
            role_arn: association.role_arn.clone(),
            target_role_arn: association.target_role_arn.clone(),
            session_name,
            fallback_role_id: association.role_id.clone(),
            duration: SESSION_DURATION,
            tags,
            inline_policy: association.policy.clone(),
            external_id: association.external_id.clone(),
        };
        let vended = crate::state::vend_pod_identity_session(
            &*self.engine,
            ctx.credential_store(),
            now,
            &plan,
        )
        .map_err(|refusal| match refusal {
            crate::state::SessionRefusal::TargetRoleNotTrusted => {
                refused(&caller_akid, &input.cluster_name, "AccessDeniedException");
                target_role_not_trusted()
            }
            crate::state::SessionRefusal::Unvendable => {
                internal_server_error("a session of the association's role could not be vended")
            }
        })?;
        let expiration = vended.expires_at;

        let session = PodIdentitySession {
            access_key_id: vended.access_key_id.clone(),
            expiration,
            cluster_name: input.cluster_name.clone(),
            association_id: association.association_id.clone(),
            assumed_role_arn: association.role_arn.clone(),
            assume_role_id: vended.assume_role_id.clone(),
            namespace: identity.namespace.clone(),
            service_account: identity.service_account.clone(),
            eks_node_name: input.eks_node_name.clone(),
            instance_id: input.instance_id.clone(),
            zone: input.zone.clone(),
        };
        state
            .lock()
            .sessions
            .insert(session.access_key_id.clone(), session);

        // The access record for a state-changing, credential-issuing call: who called,
        // what was granted, and until when. Never the secret or the session token. Every
        // field that came from the request or from a token is recorded with `?`, so its
        // value is quoted and its control characters escaped and cannot forge a line.
        ::tracing::info!(
            operation = Self::NAME,
            caller_akid = ?caller_akid,
            cluster_name = ?input.cluster_name,
            namespace = ?identity.namespace,
            service_account = ?identity.service_account,
            association_id = %association.association_id,
            role_arn = %association.role_arn,
            assume_role_id = %vended.assume_role_id,
            access_key_id = %vended.access_key_id,
            expires_at = crate::state::epoch_seconds(expiration),
            "issued EKS Pod Identity credentials"
        );

        Ok(AssumeRoleForPodIdentityOutput {
            subject: Subject {
                namespace: identity.namespace,
                service_account: identity.service_account,
            },
            audience: PODS_AUDIENCE.to_string(),
            pod_identity_association: PodIdentityAssociation {
                association_arn: association.association_arn,
                association_id: association.association_id,
            },
            assumed_role_user: AssumedRoleUser {
                // The ARN of the SESSION, which is what AWS reports for an assumed-role
                // identity — the role's own ARN is the `roleArn` the caller already holds
                // on the association.
                arn: vended.assumed_role_user_arn,
                assume_role_id: vended.assume_role_id,
            },
            credentials: Credentials {
                access_key_id: vended.access_key_id,
                secret_access_key: vended.secret_access_key,
                session_token: vended.session_token,
                expiration: ::aws_smithy_types::DateTime::from(expiration),
            },
            // "The ARN of the workload identity provider for the returned credentials.
            // Returned only for associations with workload identity enabled" — so it is
            // the provider ARN the association the exchange resolved to carries, and
            // nothing at all for an association that carries none. Reported from that
            // record rather than built here: the provider is a resource the control plane
            // that enabled the association's workload identity holds, the same way the
            // association's own ARN is.
            workload_identity_provider_arn: association.workload_identity_provider_arn,
        })
    }
}

/// The access record for a refused exchange: who called, which cluster, and the modeled
/// error the caller was answered with. No part of the token is recorded, and the
/// request-derived cluster name is recorded with `?` so it cannot forge a line.
fn refused(caller_akid: &str, cluster_name: &str, error_code: &str) {
    ::tracing::info!(
        operation = "AssumeRoleForPodIdentity",
        caller_akid = ?caller_akid,
        cluster_name = ?cluster_name,
        error_code = %error_code,
        "refused an EKS Pod Identity credential exchange"
    );
}

/// How a token this service cannot read is reported: the modeled invalid-token failure.
///
/// Each layer of the read has its own message, so two tokens that are wrong in different
/// ways are told apart by the one thing each has to change. The caller is told WHICH JOSE
/// header names nothing, and which one names something this service cannot use; every way
/// a value fails to be a readable JWT at all — its length, its segments, the encoding of
/// its header segment, a header segment that is not a JSON object, a header object that is
/// not a JOSE header — is one answer, "it could not be parsed", since the caller's fix is
/// the same and the part that failed is a detail of the parse. Which part it was goes to
/// the log instead. No segment past the header reaches this function at all: none of them
/// is read, so none of them can be the reason a value was refused.
///
/// NO PART OF THE TOKEN IS INTERPOLATED. `token` is `@sensitive` and carries a
/// credential, and an error body is logged by the clients that read it, so the message
/// states the rule the value broke and never the value: the only thing interpolated is
/// the name of a header member.
fn malformed_token(malformation: TokenMalformation) -> AssumeRoleForPodIdentityError {
    // The malformation is derived from the token's FORM, not from its characters, so it
    // carries no part of the value.
    ::tracing::info!(
        operation = "AssumeRoleForPodIdentity",
        malformation = ?malformation,
        "refused a service account token this service cannot read"
    );
    let message = match malformation {
        TokenMalformation::MissingHeader(name) => {
            format!("The token included in the request has missing header - {name}.")
        }
        TokenMalformation::InvalidHeader(name) => {
            format!("The token included in the request has invalid header - {name}.")
        }
        TokenMalformation::Length
        | TokenMalformation::Segments
        | TokenMalformation::HeaderEncoding
        | TokenMalformation::HeaderNotJson
        | TokenMalformation::NotJoseHeader => {
            "Failed to parse the token included in the request.".to_string()
        }
    };
    AssumeRoleForPodIdentityErrorModeled::InvalidTokenException(InvalidTokenException {
        message: Some(message),
    })
    .into()
}

/// How the cluster's verdict on a token is reported to the caller: a token whose
/// lifetime has passed is the modeled expired-token failure, anything else the modeled
/// invalid-token one.
///
/// A verdict about a CLAIM says which claim it was, and the value that claim has to
/// carry where exactly one value is admissible — so a projected token volume asking for
/// the wrong `audiences:` is diagnosable from the refusal, rather than collapsing into
/// one catch-all sentence with every other cluster-side verdict. A value the cluster has
/// no record of issuing reached no claim, so it is refused on the rule it broke instead.
///
/// NO PART OF THE TOKEN IS INTERPOLATED: `token` is `@sensitive` and carries a
/// credential, so the claim NAME and the value the service requires are the only things
/// the message states — never the value the rejected token carried.
fn token_rejection(rejection: TokenRejection) -> AssumeRoleForPodIdentityError {
    match rejection {
        TokenRejection::Expired => AssumeRoleForPodIdentityErrorModeled::ExpiredTokenException(
            ExpiredTokenException {
                message: Some("The service account token is expired.".to_string()),
            },
        )
        .into(),
        TokenRejection::InvalidClaim { claim, expected } => {
            // The clause naming the value the claim must carry, for a claim that admits
            // exactly one; a claim judged against the clock states the claim alone.
            let expected_clause = expected
                .map(|expected| format!(" Expected {expected}."))
                .unwrap_or_default();
            let message = format!(
                "The token included in the request has an invalid claim value for \
                 {claim}.{expected_clause}"
            );
            AssumeRoleForPodIdentityErrorModeled::InvalidTokenException(InvalidTokenException {
                message: Some(message),
            })
            .into()
        }
        TokenRejection::Invalid => AssumeRoleForPodIdentityErrorModeled::InvalidTokenException(
            InvalidTokenException {
                message: Some(format!(
                    "The service account token is invalid: it must be a token the cluster \
                     issued for the audience {PODS_AUDIENCE}."
                )),
            },
        )
        .into(),
    }
}

/// The `clusterName` label names no cluster this service holds, so the request addresses
/// a resource that is not there: the modelled `ResourceNotFoundException` (404), which is
/// what a caller whose token names a key this service published, pointed at a deleted or
/// misspelled cluster, is answered with.
///
/// AWS's not-found sentence for a cluster name, whose only variable part is the name the
/// request URI carried — a value the caller sent, so echoing it tells it nothing it did
/// not already have. `clusterName` is matched exactly, so a differently-cased spelling of
/// a registered cluster is a different cluster name and is answered here.
fn no_cluster(cluster_name: &str) -> AssumeRoleForPodIdentityError {
    AssumeRoleForPodIdentityErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(format!("No cluster found for name: {cluster_name}.")),
    })
    .into()
}

/// The cluster holds no pod identity association for the service account the token
/// names, so there is no role to assume: the modelled `ResourceNotFoundException` (404),
/// which is what a token whose association was deleted — or never created — is answered
/// with.
///
/// ONE fixed sentence, naming nothing of the request: what is missing is an association
/// for the identity the TOKEN names, and the token is `@sensitive`, so neither the
/// namespace nor the service account it carries is echoed back to a caller that did not
/// send them. It is the same answer whether the association was deleted or never existed,
/// because the state the exchange resolves against is the same.
fn no_association() -> AssumeRoleForPodIdentityError {
    AssumeRoleForPodIdentityErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(
            "The token included in the request has no service account role association for it."
                .to_string(),
        ),
    })
    .into()
}

/// The association names a TARGET role its own role may not assume, so there is no
/// credential to vend: the modelled `AccessDeniedException` (400), which is the failure of
/// a permission rather than of the token or of a resource that is missing.
///
/// ONE fixed sentence: what is missing is a grant in the TARGET role's trust policy for
/// the association's role and the external id the control plane generated for the chain,
/// and neither value is the caller's to be told about — the caller holds a pod's token,
/// not the association.
fn target_role_not_trusted() -> AssumeRoleForPodIdentityError {
    AssumeRoleForPodIdentityErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(
            "The target role of the EKS Pod Identity association cannot be assumed by the \
             association's role with the association's external ID."
                .to_string(),
        ),
    })
    .into()
}

/// A server-side failure this operation cannot serve through: the modeled 500.
///
/// `detail` says which internal step failed and goes to the log, never to the caller:
/// the response carries what the real service's `InternalServerException` says, which
/// names no component of the service.
fn internal_server_error(detail: &str) -> AssumeRoleForPodIdentityError {
    ::tracing::error!(
        operation = "AssumeRoleForPodIdentity",
        detail = %detail,
        "failed to serve an EKS Pod Identity credential exchange"
    );
    AssumeRoleForPodIdentityErrorModeled::InternalServerException(InternalServerException {
        message: Some(
            "The request processing has failed because of an unknown error. Retry the operation."
                .to_string(),
        ),
    })
    .into()
}
