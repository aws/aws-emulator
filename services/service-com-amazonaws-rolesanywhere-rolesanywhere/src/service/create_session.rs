// SPDX-License-Identifier: CC0-1.0

//! `CreateSession`: the native data plane that exchanges an X.509 identity for temporary
//! AWS credentials.
//!
//! # Not a modelled operation
//!
//! Every operation this service's Smithy model describes is control plane: it creates,
//! reads, enables, disables and tags trust anchors, profiles and certificate revocation
//! lists. The call a workload actually makes — `POST /sessions` — is NOT in that model
//! (`smithy_query(query="operation", operation="CreateSession")` answers "not found"), so
//! nothing in the generated routing table can name it, and it carries no SigV4 credential
//! scope and no `X-Amz-Target` for the front door to resolve this service by: a workload
//! using IAM Roles Anywhere holds an X.509 certificate instead of an AWS credential, which
//! is the whole point of the product. It is therefore reached through the two seams
//! [`crate::service::EmulatedService`] has for a request the model's routing key cannot
//! name ([`claims_parts`] from `can_handle`, and this plane from `override_operation`), and
//! it declares no AWS auth scheme: there is no SigV4 signature for the emulator's
//! authenticating layer to verify, because the request is signed with the private key of
//! the presented certificate under AWS's own `AWS4-X509-*` scheme. That signature is not a
//! formality here — it is checked, against the certificate, in
//! [`crate::service::x509_signature`], before anything is resolved or vended.
//!
//! # What is served
//!
//! `POST /sessions` with `{profileArn, roleArn, trustAnchorArn}`, the DER certificate
//! base64 in `X-Amz-X509` and the `AWS4-X509-…` `Authorization` header, answers **201**
//! with one `credentialSet` entry:
//!
//! * `roleArn` — the role the request asked for;
//! * `assumedRoleUser` — the `assumed-role` identity the credential speaks for,
//!   `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, where the
//!   session name defaults to the signing certificate's serial number in hex;
//! * `credentials` — a REAL session of that role, minted through the token service's own
//!   mint ([`::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::mint_role_session`]),
//!   so `sts:GetCallerIdentity` under it answers that assumed-role identity and every
//!   service judges a call it signs on the role's policies;
//!
//! and `subjectArn`, the subject this request was recorded against
//! ([`crate::state::subjects`]).
//!
//! Nothing is vended unless ALL of this holds, and each failure is answered rather than
//! served:
//!
//! | The request | The answer |
//! |---|---|
//! | names a profile or trust anchor this account does not hold | `404 ResourceNotFoundException` |
//! | omits a required member, or asks for a session outside the published duration range | `400 ValidationException` |
//! | presents a certificate no enabled trust anchor's authority issued, or a signature that does not verify under it | `403 AccessDeniedException` |
//! | names a DISABLED profile or trust anchor | `403 AccessDeniedException` |
//! | names a role the profile does not list, that this account does not hold, or whose trust policy does not let this service assume it | `403 AccessDeniedException` |
//!
//! "When disabled, temporary credential requests with this profile fail"
//! (`DisableProfile`, this service's model) is the second of those rows: the kill switch an
//! operator revokes a fleet with is this gate, so the flag is read on every request rather
//! than at create time.
//!
//! # What it writes
//!
//! Two things, both of which the spec says this call performs: the SESSION, in the token
//! service's registry (minting a credential is what the operation is for), and the SUBJECT
//! — "the subject stores auditing information such as the status of the last authentication
//! attempt, the certificate data used in the attempt, and the last time the associated
//! identity attempted authentication" (`GetSubject`, this service's model) — which is
//! recorded for a refused attempt as well, with `failed` set, as that member is documented.
//!
//! The subject is recorded only once the certificate is one an ENABLED trust anchor of this
//! account issued ([`NativeCreateSession::vend`] states the order and why): a caller can
//! mint a certificate and sign its own request with it, so a request that gets no further
//! than that writes nothing, and the store bounds how many subjects it holds in any case
//! ([`crate::state::MAX_SUBJECTS`]). Nothing else moves: no profile, trust anchor or CRL is
//! touched.
//!
//! # What it does not establish
//!
//! A trust anchor whose source is an AWS Private CA (`AWS_ACM_PCA`) holds no certificates
//! of its own here, so this plane cannot verify a chain against it and refuses rather than
//! vending on evidence it does not have; reading that authority out of AWS Private CA is
//! the subject of that source's own issue. A certificate revoked by a CRL this service
//! holds is likewise not yet refused — `ImportCrl` stores the list, and consulting it on
//! this path is its own gap.

use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::serde_json::{json, Value};
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use ::smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use super::x509_signature::{self, PresentedCertificate, SignedRequest};
use crate::state::{self, ProfileRecord, RecordedAttempt, TrustAnchorRecord};

/// The path this plane is served at, which is the path AWS serves `CreateSession` at.
pub const SESSIONS_PATH: &str = "/sessions";

/// The status a vended session is answered with, which is the status AWS answers
/// `CreateSession` with.
const CREATED_STATUS: u16 = 201;

/// The query parameter a caller may ask for a particular role session name in.
const ROLE_SESSION_NAME_PARAMETER: &str = "roleSessionName";

/// The body member carrying that same request.
const SESSION_NAME_MEMBER: &str = "sessionName";

/// The shortest session AWS vends for this call: "valid range: minimum value of 900"
/// (`CreateSession`, the IAM Roles Anywhere API reference, `durationSeconds`).
const MIN_DURATION_SECONDS: i64 = 900;

/// The longest session AWS vends for this call: "maximum value of 3600", which is also the
/// default when the request asks for none.
const MAX_DURATION_SECONDS: i64 = 3600;

/// The largest request body this plane reads.
///
/// The body carries three ARNs, a duration and a session name; this ceiling sits far above
/// any such document and bounds the work one request can ask for before it is parsed.
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

/// The characters a role session name may carry, and the lengths it may have:
/// `@pattern("[\w+=,.@-]*")` with `@length(2, 64)` (`AssumeRoleRequest$RoleSessionName`,
/// the token service's model, which is the name this call passes to that mint).
const MIN_SESSION_NAME_LENGTH: usize = 2;
/// See [`MIN_SESSION_NAME_LENGTH`].
const MAX_SESSION_NAME_LENGTH: usize = 64;

/// The sentence a refused request carries: "you do not have sufficient access to perform
/// this action" (`smithy_query(query="shape", shape="AccessDeniedException")`), this
/// service's own wording for the error class every refusal of an X.509 identity on this
/// plane belongs to.
const ACCESS_DENIED_MESSAGE: &str = "You do not have sufficient access to perform this action.";

/// Whether the buffered request parts `Service::can_handle` is handed address this plane.
///
/// A native session request is a `POST` of this plane's path carrying the X.509 scheme's
/// own marks — the certificate header, or an `Authorization` header in that scheme. One of
/// those has to be there: the path alone is not this service's (another service's model
/// binds a `/sessions` path of its own), and the request carries no credential scope and no
/// target to tell them apart by. Read by the same rule [`NativeCreateSession::claims`]
/// applies, so the claim and the answer cannot disagree about which requests this plane
/// serves.
pub fn claims_parts(parts: &http::request::Parts) -> bool {
    parts.method == http::Method::POST
        && addresses_sessions(&parts.uri.to_string())
        && (parts.headers.contains_key(x509_signature::CERTIFICATE_HEADER)
            || parts
                .headers
                .get(::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(x509_signature::names_the_scheme))
}

/// Whether `uri` addresses this plane's path.
fn addresses_sessions(uri: &str) -> bool {
    let Ok(parsed) = uri.parse::<::http::Uri>() else {
        return false;
    };
    parsed.path().trim_end_matches('/') == SESSIONS_PATH
}

/// The native session plane, as the one [`ErasedOperation`] the service holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is
/// computed from the SAME per-service store the control plane's handlers write: a profile
/// `DisableProfile` turned off vends nothing on the next request.
pub struct NativeCreateSession<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> NativeCreateSession<Eng> {
    /// Build the plane from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for NativeCreateSession<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match self.vend(req, ctx) {
                Ok(answer) => answer,
                Err(refusal) => refusal.response(),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A workload calling this plane holds an X.509 certificate and no AWS credential,
        // so there is no SigV4 signature for the emulator's authenticating layer to verify
        // and no modelled operation to read a scheme from. The request is authenticated
        // HERE instead, cryptographically: the certificate has to chain to an enabled trust
        // anchor and the `AWS4-X509-…` signature has to verify under that certificate's
        // public key before anything is resolved or vended
        // ([`crate::service::x509_signature`]).
        Some(&[])
    }
}

impl<Eng: Send + Sync + 'static> NativeCreateSession<Eng> {
    /// Whether this plane serves `req`, by the one rule the claim uses
    /// ([`claims_parts`]).
    pub fn claims(&self, req: &RtRequest) -> bool {
        req.method() == http::Method::POST.as_str()
            && addresses_sessions(req.uri())
            && (req
                .headers()
                .get(x509_signature::CERTIFICATE_HEADER)
                .is_some()
                || req
                    .headers()
                    .get(::http::header::AUTHORIZATION.as_str())
                    .is_some_and(x509_signature::names_the_scheme))
    }

    /// Serve one native session request.
    ///
    /// Three steps, in this order, because the order is what bounds what an unknown caller
    /// can make this plane do:
    ///
    /// 1. the certificate and the proof its holder has the private key ([`Self::identity`]).
    ///    A caller can mint those itself, so passing this step establishes only that the
    ///    request is internally consistent — NOTHING is read or written for it;
    /// 2. whether this ACCOUNT admits that identity ([`Self::admitted`]): an enabled trust
    ///    anchor it holds whose own authority issued the certificate. Still no write: a
    ///    certificate no stored anchor issued leaves nothing behind, whatever it carries;
    /// 3. the session itself ([`Self::session_for`]) — and from here the attempt IS recorded
    ///    against the subject the certificate identifies, with `failed` set when no
    ///    credential was vended, because that is what the member the subject carries is
    ///    documented to report. Only a certificate this account's own authority issued can
    ///    reach that write, and the store bounds how many subjects it will hold
    ///    ([`crate::state::MAX_SUBJECTS`]).
    fn vend(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> Result<RtResponse, Refusal> {
        let now = SystemTime::now();
        let body = req.body().bytes().unwrap_or_default();
        if body.len() > MAX_REQUEST_BODY_BYTES {
            return Err(Refusal::Invalid(format!(
                "the request body is longer than the {MAX_REQUEST_BODY_BYTES} bytes this \
                 operation reads"
            )));
        }
        let asked: Value = if body.is_empty() {
            Value::Null
        } else {
            ::serde_json::from_slice(body)
                .map_err(|_| Refusal::Invalid("the request body is not JSON".to_string()))?
        };

        let identity = self.identity(req, body)?;
        let admitted = self.admitted(&asked, &identity, now)?;
        let served = self.session_for(req, &asked, &identity, &admitted, ctx, now);
        if served.is_err() {
            let _ = self.record_attempt(&admitted.region, now, &identity, true);
        }
        served
    }

    /// Whether this account admits the identity `identity` presents, and what it names: the
    /// profile and trust anchor the request asks for, and the Region they belong to.
    ///
    /// Reads only. The trust anchor has to be one this account holds and ENABLED, and its
    /// own bundle has to hold the authority that issued the certificate — so a self-issued
    /// certificate, however well it signs its own request, gets no further and leaves no
    /// record.
    fn admitted(
        &self,
        asked: &Value,
        identity: &PresentedCertificate,
        now: SystemTime,
    ) -> Result<Admitted, Refusal> {
        let profile_arn = required_member(asked, "profileArn")?;
        let role_arn = required_member(asked, "roleArn")?;
        let trust_anchor_arn = required_member(asked, "trustAnchorArn")?;
        let (profile, trust_anchor) = self.resolved(&profile_arn, &trust_anchor_arn)?;
        let issued = trust_anchor.enabled
            && match trust_anchor.source.source_data.as_ref() {
                Some(crate::ops::types::SourceData::x509CertificateData(bundle)) => {
                    x509_signature::chains_to_authority_bundle(identity, bundle, now)
                }
                // A trust anchor that names an AWS Private CA holds no certificates here;
                // see the module note. Nothing is vended on evidence this emulator does
                // not have.
                _ => false,
            };
        if !issued {
            return Err(Refusal::Denied);
        }
        Ok(Admitted {
            // The Region the attempt is recorded in is the one the trust anchor that
            // admitted the identity belongs to — read off the ARN this account minted for
            // it, never off the request, so a caller cannot choose what it is recorded
            // under.
            region: state::region_in_arn(&trust_anchor.trust_anchor_arn)
                .unwrap_or(state::DEFAULT_REGION)
                .to_string(),
            profile,
            role_arn,
        })
    }

    /// The session the request asks for, for an identity this account admits.
    fn session_for(
        &self,
        req: &RtRequest,
        asked: &Value,
        identity: &PresentedCertificate,
        admitted: &Admitted,
        ctx: &OperationContext<'_>,
        now: SystemTime,
    ) -> Result<RtResponse, Refusal> {
        let Admitted {
            profile,
            role_arn,
            region,
        } = admitted;

        // The session name, as the PROFILE admits it.
        let session_name = self.session_name(req, asked, profile, identity)?;

        // WHAT MAY BE ASSUMED: an enabled profile that lists the role, a role this account
        // holds, and a trust policy that lets this service assume it.
        if !profile.enabled || !profile.role_arns.iter().any(|named| named == role_arn) {
            return Err(Refusal::Denied);
        }
        let role = self.role(role_arn, &session_name)?;
        // How long the session lasts: what the request asked for, capped by the profile
        // and by the role's own `MaxSessionDuration`, which is the ceiling IAM puts on any
        // session of it.
        let duration_seconds = duration_seconds(asked, profile)?.min(role.max_session_duration);

        // The credential itself, minted where every session of this emulator is minted, so
        // it resolves to the role everywhere a later request presents it.
        let registry = sts::service_state(&*self.engine).ok_or(Refusal::Unavailable)?;
        let identity_of_session = sts::Identity {
            user_id: format!("{}:{session_name}", role.role_id),
            account: role.account.clone(),
            arn: sts::sts_arn(
                &role.account,
                &format!("assumed-role/{}/{session_name}", role.role_name),
            ),
        };
        let minted = sts::mint_role_session(
            &*self.engine,
            ctx.credential_store(),
            &registry,
            now,
            sts::NewSession {
                role_arn: Some(role.arn.clone()),
                // A profile's session policy and managed policies scope what the session
                // it vends may do, exactly as the policies passed to an assume do.
                policies: sts::SessionPolicies {
                    inline: profile.session_policy.clone(),
                    managed_arns: profile.managed_policy_arns.clone(),
                },
                ..sts::NewSession::new(
                    identity_of_session.clone(),
                    sts::SessionKind::AssumedRole,
                    duration_seconds,
                )
            },
        )
        .map_err(|why| Refusal::Invalid(why.message()))?;

        let subject = self.record_attempt(region, now, identity, false)?;
        let expiration = minted
            .credentials
            .expiration
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .map_err(|_| Refusal::Unavailable)?;
        let answer = json!({
            "credentialSet": [{
                "assumedRoleUser": {
                    "arn": identity_of_session.arn,
                    "assumedRoleId": identity_of_session.user_id,
                },
                "credentials": {
                    "accessKeyId": minted.credentials.access_key_id,
                    "secretAccessKey": minted.credentials.secret_access_key,
                    "sessionToken": minted.credentials.session_token,
                    "expiration": expiration,
                },
                "roleArn": role.arn,
            }],
            "subjectArn": subject,
        });
        Ok(response(CREATED_STATUS, None, &answer))
    }

    /// The identity `req` presents: the certificate, once the request's own signature has
    /// proved that whoever sent it holds that certificate's private key.
    ///
    /// A caller can mint both, so this establishes only that the request is internally
    /// consistent — what makes the identity one this account admits is [`Self::admitted`].
    fn identity(&self, req: &RtRequest, body: &[u8]) -> Result<PresentedCertificate, Refusal> {
        let certificate = req
            .headers()
            .get(x509_signature::CERTIFICATE_HEADER)
            .and_then(PresentedCertificate::from_header_value)
            .ok_or(Refusal::Denied)?;
        let authorization = req
            .headers()
            .get(::http::header::AUTHORIZATION.as_str())
            .and_then(x509_signature::parse_authorization)
            .ok_or(Refusal::Denied)?;
        let verified = x509_signature::signature_verifies(
            &authorization,
            &certificate,
            &SignedRequest {
                method: req.method(),
                uri: req.uri(),
                headers: req.headers(),
                body,
            },
        );
        if !verified {
            return Err(Refusal::Denied);
        }
        Ok(certificate)
    }

    /// The profile and the trust anchor the request names, as this account holds them.
    ///
    /// An ARN naming neither is a 404 reporting that ARN, the wording this service words an
    /// ARN-keyed miss with, rather than a refusal that says nothing about which half of the
    /// request was wrong.
    fn resolved(
        &self,
        profile_arn: &str,
        trust_anchor_arn: &str,
    ) -> Result<(ProfileRecord, TrustAnchorRecord), Refusal> {
        let state = state::service_state(&*self.engine).ok_or(Refusal::Unavailable)?;
        let store = state.lock();
        let profile = store
            .profiles
            .values()
            .find(|record| record.profile_arn == profile_arn)
            .cloned()
            .ok_or_else(|| Refusal::NotFound(state::arn_not_found_message(profile_arn)))?;
        let trust_anchor = store
            .trust_anchors
            .values()
            .find(|record| record.trust_anchor_arn == trust_anchor_arn)
            .cloned()
            .ok_or_else(|| {
                Refusal::NotFound(state::trust_anchor_not_found_message(
                    state::trust_anchor_id_in_arn(trust_anchor_arn),
                ))
            })?;
        Ok((profile, trust_anchor))
    }

    /// The role `role_arn` names, as IAM holds it, when its trust policy lets this service
    /// assume it for `session_name`.
    ///
    /// Both halves are the token service's own readers, so the role a native session is a
    /// session of is the very record `sts:AssumeRole` would resolve, and the trust policy
    /// is read by the one evaluation of an IAM document this workspace has. A role this
    /// account does not hold, or one that does not admit this service's principal, is
    /// refused: a session is never minted for a role nothing lets this service assume.
    fn role(&self, role_arn: &str, session_name: &str) -> Result<sts::ResolvedRole, Refusal> {
        let iam = sts::iam_state(&*self.engine).ok_or(Refusal::Unavailable)?;
        let role = {
            let iam = iam.lock();
            sts::resolve_role(&iam, role_arn)
        };
        let role = role.ok_or(Refusal::Denied)?;
        let admitted = role.trust_policy_admits(
            sts::ASSUME_ROLE_ACTION,
            &[state::SERVICE_PRINCIPAL.to_string()],
            &[(
                "sts:RoleSessionName".to_string(),
                session_name.to_string(),
            )],
            sts::ASSUME_ROLE_PUBLISHED_CONDITION_KEYS,
        );
        if !admitted {
            return Err(Refusal::Denied);
        }
        Ok(role)
    }

    /// The role session name this request vends under: the one the caller asked for when
    /// the profile accepts one, else the signing certificate's serial number in hex, which
    /// is the default name AWS gives a session of this plane.
    ///
    /// `acceptRoleSessionName` is the profile's own switch for that
    /// (`smithy_query(query="shape", shape="ProfileDetail")`), so a profile that does not
    /// accept one vends under the certificate's name whatever the request asked for.
    fn session_name(
        &self,
        req: &RtRequest,
        asked: &Value,
        profile: &ProfileRecord,
        identity: &PresentedCertificate,
    ) -> Result<String, Refusal> {
        let requested = query_parameter(req.uri(), ROLE_SESSION_NAME_PARAMETER).or_else(|| {
            asked
                .get(SESSION_NAME_MEMBER)
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        match requested.filter(|_| profile.accept_role_session_name) {
            Some(name) => {
                if !names_a_session(&name) {
                    return Err(Refusal::Invalid(state::validation_message(
                        ROLE_SESSION_NAME_PARAMETER,
                        &format!(
                            "Member must have length between {MIN_SESSION_NAME_LENGTH} and \
                             {MAX_SESSION_NAME_LENGTH}, Member must satisfy regular \
                             expression pattern: [\\w+=,.@-]*"
                        ),
                    )));
                }
                Ok(name)
            }
            None => Ok(identity.serial_hex()),
        }
    }

    /// Record this request against the subject its certificate identity names, and answer
    /// that subject's ARN.
    ///
    /// Reached only for an identity this account's own trust anchor issued ([`Self::vend`]),
    /// and bounded by [`crate::state::MAX_SUBJECTS`]: at that ceiling a request naming a
    /// certificate identity this emulator has not seen before is refused rather than
    /// recorded, so the store cannot be grown without limit.
    fn record_attempt(
        &self,
        region: &str,
        now: SystemTime,
        identity: &PresentedCertificate,
        failed: bool,
    ) -> Result<String, Refusal> {
        let state = state::service_state(&*self.engine).ok_or(Refusal::Unavailable)?;
        let attempt = RecordedAttempt {
            x509_subject: identity.subject(),
            serial_number: identity.serial_hex(),
            issuer: identity.issuer(),
            x509_certificate_data: identity.pem().unwrap_or_default(),
            failed,
        };
        let recorded = state
            .lock()
            .record_credential_attempt(region, now, &attempt)
            .map_err(|refusal| Refusal::Invalid(refusal.message()))?;
        Ok(recorded.subject_arn)
    }
}

/// What this account admits about one request's identity: the profile and the role it
/// names, and the Region the trust anchor that admitted the identity belongs to.
///
/// Built only once the certificate is one an enabled trust anchor of this account issued,
/// so holding one of these is what says a request may be recorded and may vend.
struct Admitted {
    profile: ProfileRecord,
    role_arn: String,
    region: String,
}

/// How this plane refuses a request, as the error class each refusal belongs to.
enum Refusal {
    /// The request breaks a rule of its own shape: a missing member, a duration outside the
    /// published range, a session name outside the pattern.
    Invalid(String),
    /// The identity, the profile, the trust anchor or the role does not admit this request.
    /// One class, one sentence: a caller learns that it was refused, not which gate refused
    /// it.
    Denied,
    /// The request names a resource this account does not hold.
    NotFound(String),
    /// The emulator could not serve the request at all.
    Unavailable,
}

impl Refusal {
    /// The response this refusal is answered with: the status, the error code a client
    /// branches on, and the sentence.
    fn response(&self) -> RtResponse {
        let (status, code, message) = match self {
            Self::Invalid(message) => (400u16, "ValidationException", message.clone()),
            Self::Denied => (403, "AccessDeniedException", ACCESS_DENIED_MESSAGE.to_string()),
            Self::NotFound(message) => (404, "ResourceNotFoundException", message.clone()),
            Self::Unavailable => (
                500,
                "InternalFailure",
                "this emulator could not mint a session for the request".to_string(),
            ),
        };
        let body = json!({ "__type": code, "message": message });
        response(status, Some(code), &body)
    }
}

/// One response on this wire: the status, the error code the restJson1 envelope carries in
/// `x-amzn-errortype` when there is one, and the JSON document.
fn response(status: u16, error_code: Option<&str>, body: &Value) -> RtResponse {
    let rendered = body.to_string().into_bytes();
    let length = rendered.len();
    let mut answer = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(rendered),
    );
    let headers = answer.headers_mut();
    headers.insert("content-type", "application/json");
    headers.insert("content-length", length.to_string());
    if let Some(code) = error_code {
        // restJson1 resolves an error SHAPE from this header rather than from the body, so
        // this plane states it exactly as the service's modelled errors do.
        headers.insert("x-amzn-errortype", code.to_string());
    }
    answer
}

/// The value of one required string member of the request body.
fn required_member(asked: &Value, member: &str) -> Result<String, Refusal> {
    match asked.get(member).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(Refusal::Invalid(state::validation_message(
            member,
            "Member must not be null",
        ))),
    }
}

/// The session duration this request vends for: what it asked for, else the profile's own
/// `durationSeconds`, capped by that profile's value either way.
///
/// "In requests, if this value is not provided, the default value will be 3600"
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `durationSeconds`) is the
/// profile's default, and the profile is the ceiling: a profile vends no session longer
/// than it was created for. A request asking outside the published range is refused rather
/// than quietly clamped.
fn duration_seconds(asked: &Value, profile: &ProfileRecord) -> Result<i32, Refusal> {
    let profile_duration = i64::from(profile.duration_seconds);
    let requested = match asked.get("durationSeconds") {
        None | Some(Value::Null) => profile_duration,
        Some(value) => {
            let asked = value.as_i64().ok_or_else(|| {
                Refusal::Invalid(state::validation_message(
                    "durationSeconds",
                    "Member must be an integer",
                ))
            })?;
            if !(MIN_DURATION_SECONDS..=MAX_DURATION_SECONDS).contains(&asked) {
                return Err(Refusal::Invalid(state::validation_message(
                    "durationSeconds",
                    &format!(
                        "Member must have value greater than or equal to \
                         {MIN_DURATION_SECONDS}, Member must have value less than or equal \
                         to {MAX_DURATION_SECONDS}"
                    ),
                )));
            }
            asked
        }
    };
    let vended = requested
        .min(profile_duration)
        .clamp(MIN_DURATION_SECONDS, MAX_DURATION_SECONDS);
    Ok(i32::try_from(vended).unwrap_or(i32::MAX))
}

/// Whether `name` is a role session name the mint accepts: the length and the character
/// class `AssumeRoleRequest$RoleSessionName` declares.
fn names_a_session(name: &str) -> bool {
    // The class is ASCII, as the modelled pattern is: a letter or digit outside it is a
    // name AWS refuses, so it is refused here too.
    (MIN_SESSION_NAME_LENGTH..=MAX_SESSION_NAME_LENGTH).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c))
}

/// The value of one query parameter of `uri`, or `None` when the URI carries none of that
/// name. The query is split by the `http` crate's own URI parse rather than by hand.
fn query_parameter(uri: &str, parameter: &str) -> Option<String> {
    let parsed = uri.parse::<::http::Uri>().ok()?;
    parsed
        .query()?
        .split('&')
        .filter_map(|field| field.split_once('='))
        .find(|(name, _)| *name == parameter)
        .map(|(_, value)| value.to_string())
}
