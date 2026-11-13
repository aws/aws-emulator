// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eksauth-eksauthfrontend service.
//!
//! EKS Auth binds exactly one operation, `AssumeRoleForPodIdentity`, and models no
//! create/list/delete calls: the cluster and the EKS Pod Identity association that
//! operation authenticates against are created through the EKS control plane
//! (`CreateCluster` / `CreatePodIdentityAssociation`), which is a different service.
//! So this module owns the records that operation reads, plus the registration seam
//! that puts them there:
//!
//! * [`ComAmazonawsEksauthEksauthfrontendState::register_cluster`] registers a cluster,
//!   as the EKS control plane would.
//! * [`Cluster::associate_pod_identity`] registers one pod identity association on it.
//! * [`Cluster::issue_service_account_token`] is the emulated cluster's Kubernetes API
//!   server minting a projected service account token for a pod — the token the EKS Pod
//!   Identity Agent presents to `AssumeRoleForPodIdentity`.
//! * [`Cluster::verify_service_account_token`] is the other half: only a token THIS
//!   cluster issued, for the audience pod identity tokens are issued to, and still
//!   within its lifetime, names a service account.
//! * [`parse_service_account_token`] is the step BEFORE either of those: whether the
//!   value presented is a JSON Web Token at all. It reads nothing from state, so a
//!   caller presenting a token that is not a JWT is answered the same way whichever
//!   cluster the request names — including one that is not registered.
//!
//! ## Why the token is a record rather than a signature
//!
//! A pod identity token is minted by a cluster's own Kubernetes API server, and only
//! that cluster can say whose service account it names. This emulator IS the cluster, so
//! each cluster keeps the tokens it issued ([`Cluster::issued_tokens`]) with the identity,
//! audience and lifetime it issued them for, and verification is a lookup in that record
//! followed by the clock — never an inspection of the request. A token a cluster did not
//! issue therefore names nobody, which is what the real service answers for a token
//! signed by keys it does not hold, and no claim the caller writes can change who a token
//! names.
//!
//! The token's FORM is still a JWT, because that part is observable: a projected
//! Kubernetes service account token is three base64url segments — a JOSE header naming
//! the signing algorithm and key (`alg`, `kid`), a claim set, and a signature — and the
//! real service rejects a value that is not one before it looks anything up. So the
//! tokens this module mints carry a real header and a real claim set (see
//! [`Cluster::issue_service_account_token`]) and [`parse_service_account_token`] checks
//! that form on a presented token. Only the HEADER segment of a presented token is read,
//! though: reading a token stops at the key its header names, so neither the claim set
//! nor the signature of a presented value is decoded or judged, and nothing in them can
//! change how the value is refused. The claims a minted token carries are descriptive
//! only: nothing in them is ever trusted, since who a token names comes from the issuing
//! cluster's record.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Encode `bytes` as one segment of a JSON Web Token: base64url without padding
/// (RFC 7515 §2, which is RFC 4648 §5 "with all trailing '=' characters omitted").
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet encoder the wire
/// serializer already uses for every Smithy blob, plus the two alphabet substitutions
/// (`+` -> `-`, `/` -> `_`) and the dropped padding that turn RFC 4648 §4 into §5 — the
/// same translation the Amazon Cognito service applies to its own base64url values. No
/// second encoder is introduced for it.
pub fn encode_jwt_segment(bytes: impl AsRef<[u8]>) -> String {
    ::aws_smithy_types::base64::encode(bytes.as_ref())
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// Read one segment of a JSON Web Token, whether or not the presenter padded it — the
/// modelled `JwtToken` pattern admits `=`, and a padded token is one a client really
/// sends. `None` when the segment is not base64url at all.
///
/// The inverse of [`encode_jwt_segment`]: the two substitutions back, then the padding
/// §5 omits restored, so the standard-alphabet decoder reads it.
pub fn decode_jwt_segment(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(&standard).ok()
}

/// The JOSE header parameter JWS makes mandatory (RFC 7515 §4.1.1): `alg` names the
/// algorithm the signature was produced with. A header object that does not carry it as a
/// string is not a JOSE header at all, so it cannot be read rather than being a header
/// with something to fix.
pub const ALGORITHM_HEADER: &str = "alg";

/// The JOSE header parameter naming the key a token was signed with (RFC 7515 §4.1.4),
/// which a Kubernetes API server always sets so a verifier can select the key out of the
/// set its OIDC provider publishes.
pub const KEY_ID_HEADER: &str = "kid";

/// The algorithm a cluster's Kubernetes API server signs projected service account
/// tokens with, as the `alg` header of the tokens this module mints — and the only value
/// a presented token's `alg` may name, since no cluster publishes a key for any other.
pub const TOKEN_SIGNING_ALGORITHM: &str = "RS256";

/// Bytes in the signature segment of a minted token.
///
/// The emulator holds no signing key — a token is authenticated by the issuing cluster's
/// record, not by its signature — so the segment is random bytes of a plausible length
/// rather than a signature over the other two segments.
const TOKEN_SIGNATURE_BYTES: usize = 32;

/// Bytes a signing key id is the digest of.
///
/// A Kubernetes API server derives a key's id from the key itself: the id is the
/// base64url-encoded SHA-256 digest of the public key's DER encoding, which is why every
/// kid in a cluster's JWKS is [`SIGNING_KEY_ID_LEN`] unpadded base64url characters. This
/// emulator holds no key pair — a token is authenticated by the issuing cluster's record,
/// not by a signature — so a cluster's key id is a digest-sized value of its own instead,
/// rendered in exactly that form.
const SIGNING_KEY_DIGEST_BYTES: usize = 32;

/// Characters in a signing key id: base64url of [`SIGNING_KEY_DIGEST_BYTES`] without
/// padding, which is 43.
const SIGNING_KEY_ID_LEN: usize = 43;

/// Whether `value` is a key id a service account token can name — the form a Kubernetes
/// API server derives for every key it signs with: [`SIGNING_KEY_ID_LEN`] characters of
/// unpadded base64url. A value of any other length or alphabet names no key in any
/// cluster's JWKS, so it is not a key id at all, whichever cluster a request names.
pub fn is_signing_key_id(value: &str) -> bool {
    value.len() == SIGNING_KEY_ID_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The greatest number of bytes a value this service will read as a token may carry.
///
/// CHOSEN, not modelled: `JwtToken` declares a `@length` minimum of 1 and no maximum, and
/// the parse below base64-decodes two caller-controlled segments, so without a bound the
/// bytes one request can make this service allocate are whatever the transport accepted.
/// A projected Kubernetes service account token is a header, a claim set and one RSA
/// signature — a couple of kilobytes — so this is far above any token a cluster issues
/// and far below a request-sized allocation.
const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// The identity a pod identity service account token is issued to, and the only value
/// `AssumeRoleForPodIdentity` reports in its `audience` output member: the model
/// documents it as "always `pods.eks.amazonaws.com`".
pub const PODS_AUDIENCE: &str = "pods.eks.amazonaws.com";

/// The region every ARN this service mints is scoped to when a caller of the
/// registration seam names no other. EKS Auth is a regional service; emulators in this
/// project default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a minted pod identity session stays valid.
///
/// CHOSEN, not documented: AWS does not publish the exact lifetime of an EKS Pod
/// Identity session, so this is the emulator's own span, picked long enough that a
/// caller sees a plainly future `expiration` and short enough that an emulator left
/// running recycles its sessions.
pub const SESSION_DURATION: Duration = Duration::from_secs(6 * 60 * 60);

/// The ARN of the EKS cluster named `name` in `region`:
/// `arn:<partition>:eks:<region>:<account>:cluster/<name>`.
///
/// A cluster is an EKS resource, so its ARN carries the `eks` namespace rather than the
/// `eks-auth` one this service's ACTIONS are published under. Stated once here because it
/// is read in two places — the record a registered cluster carries, and the resource an
/// authorization decision about a request naming that cluster is taken against — and the
/// two must spell one ARN.
#[must_use]
pub fn cluster_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("eks", region, &format!("cluster/{name}"))
}

/// Per-service state for com-amazonaws-eksauth-eksauthfrontend. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEksauthEksauthfrontendState {
    /// Registered clusters, by cluster name (the `clusterName` http label).
    pub clusters: HashMap<String, Cluster>,
    /// Every pod identity session this service minted, by the access key id that
    /// identifies it.
    pub sessions: HashMap<String, PodIdentitySession>,
    /// The id IAM gave each role this service has seen, by role ARN.
    ///
    /// IAM mints a role's unique id when the role is created and that id stays with the
    /// role; this service holds no IAM records, so it mints one the first time a role is
    /// associated and keeps it here. That is what makes the id a property of the ROLE:
    /// the same role associated in two clusters reports one id, and re-pointing an
    /// association at a role it was already associated with does not change it.
    pub role_ids: HashMap<String, String>,
}

impl ComAmazonawsEksauthEksauthfrontendState {
    /// Register a cluster under `name` in `region`, or return the one already there.
    ///
    /// The registration seam the EKS control plane would drive. `clusterName` is the only
    /// cluster identity a request to this service carries, so a cluster name identifies
    /// one cluster here: the FIRST registration of a name fixes its region, and `region`
    /// is ignored on any later registration of that same name.
    ///
    /// Refused (`None`) for a name EKS itself refuses: the modelled `ClusterName` admits
    /// 1 to 100 characters matching `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, so a name outside that
    /// never names a cluster a request could reach.
    pub fn register_cluster(&mut self, name: &str, region: &str) -> Option<&mut Cluster> {
        if !is_cluster_name(name) {
            return None;
        }
        Some(
            self.clusters
                .entry(name.to_string())
                .or_insert_with(|| Cluster::new(name, region)),
        )
    }

    /// The cluster registered under `name`, if any.
    pub fn cluster(&self, name: &str) -> Option<&Cluster> {
        self.clusters.get(name)
    }

    /// The cluster registered under `name` for modification — what the emulated
    /// Kubernetes API server needs to record a token it issues.
    pub fn cluster_mut(&mut self, name: &str) -> Option<&mut Cluster> {
        self.clusters.get_mut(name)
    }

    /// Register a pod identity association on the cluster named `cluster_name`, binding
    /// `namespace`/`service_account` in it to `role_arn`.
    ///
    /// A (namespace, service account) pair carries at most one association in a cluster,
    /// so registering the pair again re-points THAT association at `role_arn` — what
    /// `UpdatePodIdentityAssociation` does — rather than silently keeping the role it was
    /// first registered with, which no caller could tell from a successful create.
    ///
    /// The association id and ARN take the shapes EKS uses (`a-` plus 17 characters;
    /// `podidentityassociation/<cluster>/<id>`), and the role's id comes from
    /// [`Self::role_ids`], so one role carries one id wherever it is associated.
    ///
    /// Refused with the reason the EKS control plane refuses on: an unregistered cluster,
    /// a `roleArn` that is not an IAM role ARN, or a namespace or service account name
    /// Kubernetes would not accept. Nothing is recorded for a refused registration — not
    /// even the role's id.
    pub fn associate_pod_identity(
        &mut self,
        cluster_name: &str,
        namespace: &str,
        service_account: &str,
        role_arn: &str,
    ) -> Result<&PodIdentityAssociationRecord, AssociationRefusal> {
        if !self.clusters.contains_key(cluster_name) {
            return Err(AssociationRefusal::UnknownCluster);
        }
        if !is_role_arn(role_arn) {
            return Err(AssociationRefusal::RoleArn);
        }
        if !is_dns_label(namespace) {
            return Err(AssociationRefusal::Namespace);
        }
        if !is_dns_subdomain(service_account) {
            return Err(AssociationRefusal::ServiceAccount);
        }
        // The role's id is state the whole service shares, so it is resolved — and minted
        // if this is the first association of the role — only once the registration is
        // going to be recorded.
        let role_id = self.role_id(role_arn);
        let cluster = self
            .clusters
            .get_mut(cluster_name)
            .expect("the cluster checked above is present");
        Ok(cluster.upsert_association(namespace, service_account, role_arn, role_id))
    }

    /// Program the pod identity associations the EKS control plane holds for
    /// `cluster_name` into this plane, registering the cluster if it is not registered
    /// yet, and answer how many were programmed.
    ///
    /// This is the registration seam [`Self::associate_pod_identity`] is for a caller that
    /// has no control plane behind it, with the difference that matters for what
    /// `AssumeRoleForPodIdentity` REPORTS: the association id and ARN are the ones the
    /// control plane published, not ones minted here, so the `podIdentityAssociation` this
    /// service answers with is the resource that control plane created.
    ///
    /// It takes the WHOLE set, so one call states what the cluster holds: an association
    /// the control plane created appears, one it re-pointed at another role is re-pointed
    /// here, and one it deleted is gone — which is what makes an exchange against a
    /// deleted association resolve to nothing. An entry naming something no Kubernetes
    /// identity or IAM role could be is skipped, on the same rules as the other seam.
    pub fn program_pod_identity_associations(
        &mut self,
        cluster_name: &str,
        region: &str,
        programmed: &[ProgrammedAssociation],
    ) -> usize {
        let eligible: Vec<&ProgrammedAssociation> = programmed
            .iter()
            .filter(|association| {
                is_role_arn(&association.role_arn)
                    && is_dns_label(&association.namespace)
                    && is_dns_subdomain(&association.service_account)
            })
            .collect();
        // The role ids are state the whole service shares, so they are resolved — and
        // minted for a role this service has not seen — before the cluster is borrowed.
        let role_ids: Vec<String> = eligible
            .iter()
            .map(|association| self.role_id(&association.role_arn))
            .collect();
        let Some(cluster) = self.register_cluster(cluster_name, region) else {
            return 0;
        };
        cluster.associations = eligible
            .iter()
            .zip(role_ids)
            .map(|(association, role_id)| PodIdentityAssociationRecord {
                association_id: association.association_id.clone(),
                association_arn: association.association_arn.clone(),
                namespace: association.namespace.clone(),
                service_account: association.service_account.clone(),
                role_arn: association.role_arn.clone(),
                role_id,
                policy: association.policy.clone(),
                target_role_arn: association.target_role_arn.clone(),
                external_id: association.external_id.clone(),
                disable_session_tags: association.disable_session_tags,
            })
            .collect();
        cluster.associations.len()
    }

    /// Forget the cluster registered under `name` — what the EKS control plane deleting a
    /// cluster does to this plane: the tokens its Kubernetes API server issued and the
    /// associations it held go with it, so nothing a deleted cluster issued names anybody
    /// afterwards. Answers whether a cluster was there.
    pub fn forget_cluster(&mut self, name: &str) -> bool {
        self.clusters.remove(name).is_some()
    }

    /// The id IAM gave the role named by `role_arn`, minting and recording one the first
    /// time this service sees the role.
    pub fn role_id(&mut self, role_arn: &str) -> String {
        self.role_ids
            .entry(role_arn.to_string())
            .or_insert_with(|| unique_id("AROA"))
            .clone()
    }

    /// Materialise everything the clock has already made due, and report the access key
    /// ids whose credentials the caller must now delete.
    ///
    /// Two records fall due with time, whether or not anybody called:
    ///
    /// * a session past its `expiration` is over, so it is forgotten and its credential
    ///   named for deletion;
    /// * a token record far enough past the instant its token expired that no caller can
    ///   still be owed [`TokenRejection::Expired`] for it is forgotten too, which is what
    ///   keeps a long-running emulator's token records bounded.
    pub fn expire(&mut self, now: SystemTime) -> Vec<String> {
        // Collected and removed by the MAP KEY, which is the access key id a later
        // request presents: a record keyed under anything else would otherwise survive
        // its own expiry for ever. One pass: `retain` collects as it drops.
        let mut expired = Vec::new();
        self.sessions.retain(|access_key_id, session| {
            let live = session.expiration > now;
            if !live {
                expired.push(access_key_id.clone());
            }
            live
        });
        for cluster in self.clusters.values_mut() {
            cluster.forget_stale_tokens(now);
        }
        expired
    }
}

/// A registered EKS cluster: its identity, the projected service account tokens its
/// Kubernetes API server issued, and its pod identity associations.
#[derive(Debug)]
pub struct Cluster {
    pub name: String,
    pub arn: String,
    /// The region this cluster's ARNs are scoped to.
    pub region: String,
    /// The issuer URL of this cluster's OIDC provider, which is what the tokens it issues
    /// name in their `iss` claim. EKS gives every cluster one of the form
    /// `https://oidc.eks.<region>.amazonaws.com/id/<cluster identity>`.
    pub oidc_issuer: String,
    /// The id of the key this cluster's Kubernetes API server signs service account
    /// tokens with — the `kid` header of every token it issues.
    ///
    /// It identifies a key; it does not authenticate a token. A caller can copy it into a
    /// token of its own and that token still names nobody, because the identity comes
    /// from [`Self::issued_tokens`].
    pub signing_key_id: String,
    /// Every projected service account token this cluster's Kubernetes API server issued,
    /// by the token itself: who it names, what it was issued to, and when it expires.
    ///
    /// Kept per cluster, so a token one cluster issued names nobody in another. A record
    /// outlives its own expiry on purpose: a token presented after it expired has to be
    /// answered as expired, not as unknown.
    pub issued_tokens: HashMap<String, IssuedToken>,
    pub associations: Vec<PodIdentityAssociationRecord>,
}

impl Cluster {
    /// A cluster named `name` in `region`, with no issued tokens and no associations.
    fn new(name: &str, region: &str) -> Self {
        // The cluster identity in the OIDC issuer URL is the 32 hex characters EKS puts
        // there, minted per cluster like the rest of this cluster's identifiers.
        let identity = random_hex(32).to_uppercase();
        Self {
            name: name.to_string(),
            arn: cluster_arn(region, name),
            region: region.to_string(),
            oidc_issuer: format!("https://oidc.eks.{region}.amazonaws.com/id/{identity}"),
            signing_key_id: encode_jwt_segment(random_bytes(SIGNING_KEY_DIGEST_BYTES)),
            issued_tokens: HashMap::new(),
            associations: Vec::new(),
        }
    }

    /// Record the association binding `namespace`/`service_account` in this cluster to
    /// `role_arn`, whose IAM role id is `role_id`.
    ///
    /// Private: an association's role id is state the whole service shares, so
    /// registration goes through
    /// [`ComAmazonawsEksauthEksauthfrontendState::associate_pod_identity`], which
    /// resolves the role id first.
    fn upsert_association(
        &mut self,
        namespace: &str,
        service_account: &str,
        role_arn: &str,
        role_id: String,
    ) -> &PodIdentityAssociationRecord {
        if let Some(index) = self
            .associations
            .iter()
            .position(|a| a.namespace == namespace && a.service_account == service_account)
        {
            let existing = &mut self.associations[index];
            if existing.role_arn != role_arn {
                existing.role_arn = role_arn.to_string();
                existing.role_id = role_id;
            }
            return &self.associations[index];
        }
        let association_id = format!("a-{}", random_hex(ASSOCIATION_ID_SUFFIX_LEN));
        let association_arn = ::engine::emulator_arn(
            "eks",
            &self.region,
            &format!("podidentityassociation/{}/{association_id}", self.name),
        );
        self.associations.push(PodIdentityAssociationRecord {
            association_id,
            association_arn,
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            role_arn: role_arn.to_string(),
            role_id,
            // The seam a caller with no control plane behind it drives records the
            // binding and nothing else: an association's session policy, target role
            // and external id are the control plane's to publish, and
            // [`ComAmazonawsEksauthEksauthfrontendState::program_pod_identity_associations`]
            // is what carries them here.
            policy: None,
            target_role_arn: None,
            external_id: None,
            disable_session_tags: None,
        });
        self.associations
            .last()
            .expect("the association just pushed is present")
    }

    /// The association this cluster holds for `namespace`/`service_account`, if any.
    ///
    /// A pod identity association is unique per (namespace, service account) pair within
    /// a cluster, so at most one can match.
    pub fn association_for(
        &self,
        namespace: &str,
        service_account: &str,
    ) -> Option<&PodIdentityAssociationRecord> {
        self.associations
            .iter()
            .find(|a| a.namespace == namespace && a.service_account == service_account)
    }

    /// Mint a projected service account token for `request` and record what it names —
    /// what this cluster's Kubernetes API server hands a pod, and what the EKS Pod
    /// Identity Agent presents to `AssumeRoleForPodIdentity`.
    ///
    /// The token is a JSON Web Token, as [`parse_service_account_token`] requires of any
    /// token presented to this service: a JOSE header naming this cluster's signing
    /// algorithm and key, the claim set a projected token carries, and a signature
    /// segment. What the token NAMES, though, lives in this cluster's record and not in
    /// its claims — the claims describe the token, and rewriting them changes nothing
    /// about who it names, because the record is keyed by the whole token value.
    ///
    /// Returns `None` for a request a Kubernetes API server would not serve: one naming
    /// no service account it could issue for (a namespace that is not a DNS label or a
    /// service account that is not a DNS subdomain, which includes the empty name), or
    /// one whose validity window is empty (`expires_at` at or before `issued_at`).
    pub fn issue_service_account_token(
        &mut self,
        request: &ServiceAccountTokenRequest,
    ) -> Option<String> {
        if !is_dns_label(&request.namespace)
            || !is_dns_subdomain(&request.service_account)
            || request.expires_at <= request.issued_at
        {
            return None;
        }
        let header = ::serde_json::json!({
            "alg": TOKEN_SIGNING_ALGORITHM,
            "kid": self.signing_key_id,
            "typ": "JWT",
        });
        // The claims a projected service account token carries. `jti` is what makes two
        // tokens of the same service account distinct values, so a token is never
        // reissued under a record that is already there.
        let claims = ::serde_json::json!({
            "iss": self.oidc_issuer,
            "aud": request.audience,
            "sub": format!(
                "system:serviceaccount:{}:{}",
                request.namespace, request.service_account
            ),
            "iat": epoch_seconds(request.issued_at),
            "exp": epoch_seconds(request.expires_at),
            "jti": random_uuid(),
            "kubernetes.io": {
                "namespace": request.namespace,
                "serviceaccount": { "name": request.service_account },
                "pod": request.pod_name.as_ref().map(|name| ::serde_json::json!({
                    "name": name,
                    "uid": request.pod_uid,
                })),
            },
        });
        let token = format!(
            "{}.{}.{}",
            encode_jwt_segment(header.to_string()),
            encode_jwt_segment(claims.to_string()),
            encode_jwt_segment(random_bytes(TOKEN_SIGNATURE_BYTES))
        );
        self.issued_tokens.insert(
            token.clone(),
            IssuedToken {
                namespace: request.namespace.clone(),
                service_account: request.service_account.clone(),
                pod_name: request.pod_name.clone(),
                pod_uid: request.pod_uid.clone(),
                audience: request.audience.clone(),
                issued_at: request.issued_at,
                expires_at: request.expires_at,
            },
        );
        Some(token)
    }

    /// Whether `key_id` names a key this cluster's OIDC provider publishes — resolving a
    /// presented token's `kid` through this cluster's JWKS.
    ///
    /// This cluster publishes exactly one key, [`Self::signing_key_id`], the one its
    /// Kubernetes API server signs projected tokens with, so resolution is that one
    /// comparison. A key id is resolved against the provider of the cluster the request
    /// names, so a key id this cluster does not publish names no key for that request
    /// even when another cluster publishes it. It identifies a key; it does not
    /// authenticate the token, which [`Self::verify_service_account_token`] does from
    /// this cluster's record.
    pub fn publishes_signing_key(&self, key_id: &str) -> bool {
        self.signing_key_id == key_id
    }

    /// Report whose service account `token` names, as of `now`.
    ///
    /// A token is accepted only when it names this cluster's signing key, THIS cluster
    /// issued it, it was issued to the pod identity audience, and `now` lies inside the
    /// window it was issued for. Anything else — a token another cluster issued, one no
    /// cluster issued, one whose characters were changed, one issued to a different
    /// audience, one presented before its own issue instant — names nobody, because only
    /// the issuing cluster's record can say who a token belongs to.
    ///
    /// `headers` is the already-parsed JOSE header of `token`, so a token signed by a key
    /// this cluster does not publish is refused for that reason rather than for being
    /// unknown. It is NOT what authenticates the token: the record lookup below is, which
    /// is why copying a cluster's `kid` into a token of one's own gains nothing.
    pub fn verify_service_account_token(
        &self,
        token: &str,
        headers: &JoseHeaders,
        now: SystemTime,
    ) -> Result<ServiceAccountIdentity, TokenRejection> {
        if !self.publishes_signing_key(&headers.key_id) {
            return Err(TokenRejection::Invalid);
        }
        let issued = self
            .issued_tokens
            .get(token)
            .ok_or(TokenRejection::Invalid)?;
        if !issued
            .audience
            .iter()
            .any(|audience| audience == PODS_AUDIENCE)
        {
            return Err(TokenRejection::Invalid);
        }
        // A token is not yet a token before the instant it was issued at, the way a
        // projected token carries a not-before claim its holder cannot anticipate.
        if now < issued.issued_at {
            return Err(TokenRejection::Invalid);
        }
        // No leeway: a token whose lifetime has passed is expired, not nearly expired.
        if issued.expires_at <= now {
            return Err(TokenRejection::Expired);
        }
        Ok(ServiceAccountIdentity {
            namespace: issued.namespace.clone(),
            service_account: issued.service_account.clone(),
            pod_name: issued.pod_name.clone(),
            pod_uid: issued.pod_uid.clone(),
        })
    }

    /// Forget the token records whose tokens expired longer ago than
    /// [`TOKEN_RECORD_GRACE`], so the records this cluster keeps stay bounded while a
    /// token that has only just expired is still answered as expired.
    fn forget_stale_tokens(&mut self, now: SystemTime) {
        // `checked_add`: a record whose expiry plus the grace period is not representable
        // is one no clock reaches, so it is kept rather than panicking the sweep.
        self.issued_tokens.retain(|_, issued| {
            issued
                .expires_at
                .checked_add(TOKEN_RECORD_GRACE)
                .is_none_or(|forget_at| forget_at > now)
        });
    }
}

/// How long a cluster keeps the record of a token after that token expired.
///
/// CHOSEN, not documented: the record only has to outlive the token long enough that a
/// holder presenting a just-expired token is told it expired rather than that it was
/// never issued, so this is the emulator's own span.
pub const TOKEN_RECORD_GRACE: Duration = Duration::from_secs(60 * 60);

/// Why the registration seam refused to record a pod identity association — the reasons
/// the EKS control plane refuses on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationRefusal {
    /// No cluster is registered under that name.
    UnknownCluster,
    /// `roleArn` is not the ARN of an IAM role.
    RoleArn,
    /// The namespace is not a Kubernetes DNS label.
    Namespace,
    /// The service account name is not a Kubernetes DNS subdomain.
    ServiceAccount,
}

/// Whether `name` is a cluster name a request to this service could carry: the modelled
/// `ClusterName` admits 1 to 100 characters of `^[0-9A-Za-z][A-Za-z0-9\-_]*$`.
fn is_cluster_name(name: &str) -> bool {
    /// The modelled `ClusterName` length bound.
    const MAX_LEN: usize = 100;
    let mut characters = name.chars();
    // One pass: the length is taken from the same iterator the pattern walks, and the
    // non-empty lower bound is what `next()` yielding a character already says.
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters
            .enumerate()
            .all(|(index, c)| {
                index + 2 <= MAX_LEN && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
}

/// Whether `value` is an RFC 1123 DNS label, which is what Kubernetes requires of a
/// namespace name: 1 to 63 characters of lowercase alphanumerics and `-`, starting and
/// ending alphanumeric.
fn is_dns_label(value: &str) -> bool {
    const MAX_LABEL: usize = 63;
    !value.is_empty()
        && value.len() <= MAX_LABEL
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && value
            .bytes()
            .next_back()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Whether `value` is an RFC 1123 DNS subdomain, which is what Kubernetes requires of a
/// service account name: up to 253 characters of DNS labels joined by `.`.
fn is_dns_subdomain(value: &str) -> bool {
    const MAX_SUBDOMAIN: usize = 253;
    !value.is_empty() && value.len() <= MAX_SUBDOMAIN && value.split('.').all(is_dns_label)
}

/// Whether `arn` is the ARN of an IAM role: `arn:<partition>:iam::<account>:role/<path
/// and name>`, the form EKS requires of a pod identity association's `roleArn`. IAM is a
/// global service, so the region field is empty, and the account is twelve digits.
fn is_role_arn(arn: &str) -> bool {
    let mut fields = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("iam"),
        Some(""),
        Some(account),
        Some(resource),
        None,
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && resource
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty() && !name.ends_with('/'))
}

/// One projected service account token a cluster issued: who it names, what it was
/// issued to, and the window it is valid in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    pub namespace: String,
    pub service_account: String,
    /// The pod the token was projected into, when it was issued for one.
    pub pod_name: Option<String>,
    /// That pod's `metadata.uid`, when the token was bound to a pod — one of the
    /// session tags EKS Pod Identity puts on the session it vends.
    pub pod_uid: Option<String>,
    /// The audiences the token was issued to; only [`PODS_AUDIENCE`] is the one
    /// `AssumeRoleForPodIdentity` accepts.
    pub audience: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

/// The JOSE header of a presented service account token, as
/// [`parse_service_account_token`] read it.
///
/// Only the headers JWS requires are kept: everything else a header may carry (`typ`,
/// `x5t`, …) says nothing about which key signed the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoseHeaders {
    /// The `alg` header: the signature algorithm the token names.
    pub algorithm: String,
    /// The `kid` header: the key the token names.
    pub key_id: String,
}

/// Why a presented value is not a JSON Web Token at all.
///
/// Every variant is about the VALUE's form, so none of them needs state to decide — which
/// is what lets this service answer a malformed token before it resolves the cluster the
/// request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMalformation {
    /// Longer than any token could be ([`MAX_TOKEN_BYTES`]), so it is not read at all.
    Length,
    /// Not three non-empty dot-separated segments.
    Segments,
    /// The header segment — the only segment that is read — is not base64url.
    HeaderEncoding,
    /// The header segment is base64url but does not decode to a JSON object.
    HeaderNotJson,
    /// The header segment IS a JSON object, but not a JOSE header: the one parameter JWS
    /// makes mandatory ([`ALGORITHM_HEADER`]) is absent or is not a string, or the
    /// [`KEY_ID_HEADER`] the object does carry holds a JSON value that could never be a
    /// key id (anything other than a string or `null`).
    ///
    /// A value whose header cannot be read as a JOSE header at all is not a token whose
    /// header has something to fix: it did not parse, so it is reported as the parse
    /// failure the other structural variants are.
    NotJoseHeader,
    /// A JOSE header this service requires names nothing: it is absent from the header
    /// object, or it is there holding `null`, the empty string or whitespace only, each
    /// of which selects nothing and is therefore the same as absent.
    MissingHeader(&'static str),
    /// A JOSE header is there and names something, but not something this service can
    /// use: an `alg` other than [`TOKEN_SIGNING_ALGORITHM`], or a `kid` that resolves to
    /// no key — one outside the form every key id takes ([`is_signing_key_id`]), or one
    /// no cluster's OIDC provider publishes.
    InvalidHeader(&'static str),
}

/// Read `token` as a JSON Web Token and report its JOSE header.
///
/// A JWT is three base64url segments joined by `.`: a base64url-encoded JSON header, a
/// claim set, and a signature. This checks that the value HAS those three segments and
/// then reads the FIRST one only, layer by layer, from the shallowest defect to the
/// deepest, so a header that is wrong in several ways is always reported on the
/// shallowest one:
///
/// 1. the header segment must read as a JOSE header at all — base64url of a JSON object
///    whose `alg` is there and is a string, and whose `kid`, if it carries one, is a
///    string or `null` — otherwise it did not parse ([`TokenMalformation::NotJoseHeader`]);
/// 2. `alg` must name the algorithm clusters sign projected tokens with
///    ([`TOKEN_SIGNING_ALGORITHM`]), otherwise it names a signature nothing here produces;
/// 3. `kid` must name something — absent, `null`, empty and blank all name nothing, so all
///    four are the missing header;
/// 4. `kid` must have the form every key id in a JWKS takes ([`is_signing_key_id`]); a
///    value outside it resolves to no key under any provider, which is the part of
///    resolving a key id that needs no state.
///
/// The segments PAST the header — the claim set and the signature — are not decoded and
/// not judged here or anywhere on this path: reading a presented token stops at the key
/// its header names, so a payload that is not even JSON and a well-formed claim set
/// carrying the same header are the same value as far as this read is concerned.
///
/// Never who the token names, which is [`Cluster::verify_service_account_token`]'s
/// question, and never WHICH provider publishes the key it names, which is
/// [`Cluster::publishes_signing_key`]'s.
///
/// Reads NOTHING from state and nothing about any cluster, so the answer is the same for
/// every cluster name a request could carry, registered or not.
pub fn parse_service_account_token(token: &str) -> Result<JoseHeaders, TokenMalformation> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(TokenMalformation::Length);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(TokenMalformation::Segments);
    };
    if header.is_empty() || claims.is_empty() || signature.is_empty() {
        return Err(TokenMalformation::Segments);
    }

    // ONLY the header segment is read. `claims` and `signature` are required to BE there
    // and to be non-empty (above) and nothing more: no claim gates this exchange — who a
    // token names comes from the issuing cluster's record — and the signature is
    // authenticated by that record too, so neither segment's content can decide how a
    // value is refused.
    let header = header_object(header)?;

    // LAYER 1, the structural parse of the header itself: a JOSE header names the
    // algorithm its signature was produced with, so an object that does not carry `alg`
    // as a string is not one, and the value is reported as unparseable rather than as a
    // header with a member to fix.
    let Some(algorithm) = header
        .get(ALGORITHM_HEADER)
        .and_then(::serde_json::Value::as_str)
    else {
        return Err(TokenMalformation::NotJoseHeader);
    };
    // `kid` is read the same way, one value apart: `null` names nothing, which is what an
    // absent member names too, so it falls to the missing-header layer below; any other
    // non-string value is a member no key id could ever be, so the header does not read.
    match header.get(KEY_ID_HEADER) {
        None | Some(::serde_json::Value::Null) | Some(::serde_json::Value::String(_)) => {}
        Some(_) => return Err(TokenMalformation::NotJoseHeader),
    }

    // LAYER 2: a cluster's Kubernetes API server signs projected service account tokens
    // with exactly one algorithm, so a header naming any other names a signature no
    // cluster's OIDC provider publishes a key for.
    if algorithm != TOKEN_SIGNING_ALGORITHM {
        return Err(TokenMalformation::InvalidHeader(ALGORITHM_HEADER));
    }

    // LAYER 3: the header has to name a key. Absent, `null`, empty and blank all name
    // nothing — no key is selected by blank space — so each is the same report: add it.
    let Some(key_id) = string_header(&header, KEY_ID_HEADER) else {
        return Err(TokenMalformation::MissingHeader(KEY_ID_HEADER));
    };

    // LAYER 4, the part of resolving a key id that needs no state: a value outside the
    // form every key id in a JWKS takes names no key under any OIDC provider, so it is a
    // `kid` to fix. WHICH provider publishes a well-formed one is state's answer.
    if !is_signing_key_id(key_id) {
        return Err(TokenMalformation::InvalidHeader(KEY_ID_HEADER));
    }

    Ok(JoseHeaders {
        algorithm: algorithm.to_string(),
        key_id: key_id.to_string(),
    })
}

/// A JOSE header parameter that has to be a non-blank string. A parameter of any other
/// JSON type names no algorithm and no key, and neither does one that is empty or all
/// whitespace — a key id selects a key, and no key is selected by blank space.
fn string_header<'a>(
    header: &'a ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Option<&'a str> {
    header
        .get(name)
        .and_then(::serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

/// The header segment of a JWT read as a JSON object — the one segment of a presented
/// token this service decodes.
fn header_object(
    segment: &str,
) -> Result<::serde_json::Map<String, ::serde_json::Value>, TokenMalformation> {
    let bytes = decode_jwt_segment(segment).ok_or(TokenMalformation::HeaderEncoding)?;
    match ::serde_json::from_slice::<::serde_json::Value>(&bytes) {
        Ok(::serde_json::Value::Object(members)) => Ok(members),
        _ => Err(TokenMalformation::HeaderNotJson),
    }
}

/// The characters after the `a-` prefix of an EKS pod identity association id.
const ASSOCIATION_ID_SUFFIX_LEN: usize = 17;

/// Why a cluster refused a service account token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// The instant the token expires has passed.
    Expired,
    /// The token is not one this cluster issued for pod identity: a token it never
    /// issued, or one it issued to a different audience.
    Invalid,
}

/// Whose service account a verified token names, and the pod it was projected into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountIdentity {
    pub namespace: String,
    pub service_account: String,
    /// The pod the token was projected into, when it was issued for one.
    pub pod_name: Option<String>,
    /// That pod's `metadata.uid`, when the token was bound to a pod.
    pub pod_uid: Option<String>,
}

/// What [`Cluster::issue_service_account_token`] mints a token for.
#[derive(Debug, Clone)]
pub struct ServiceAccountTokenRequest {
    pub namespace: String,
    pub service_account: String,
    pub pod_name: Option<String>,
    /// The `metadata.uid` of the pod the token is projected into, when the request bound
    /// it to one — the second half of the `kubernetes.io/pod` claim a projected token
    /// carries. Descriptive only, like every other claim: who a token names comes from
    /// the issuing cluster's record.
    pub pod_uid: Option<String>,
    pub audience: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

impl ServiceAccountTokenRequest {
    /// A token for `service_account` in `namespace`, projected into pod `pod_name`, with
    /// the pod identity audience and a one-hour lifetime starting now — what the
    /// Kubernetes API server issues for the EKS Pod Identity Agent.
    pub fn for_pod(namespace: &str, service_account: &str, pod_name: &str) -> Self {
        let now = SystemTime::now();
        Self {
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            pod_name: Some(pod_name.to_string()),
            pod_uid: None,
            audience: vec![PODS_AUDIENCE.to_string()],
            issued_at: now,
            expires_at: now + Duration::from_secs(60 * 60),
        }
    }

    /// The same token, bound to the pod whose `metadata.uid` is `pod_uid` — what a
    /// `TokenRequest` carrying a `boundObjectRef` naming a Pod object is issued for.
    pub fn bound_to_pod_uid(mut self, pod_uid: &str) -> Self {
        self.pod_uid = Some(pod_uid.to_string());
        self
    }

    /// The same token, expiring at `expires_at` instead.
    pub fn expiring_at(mut self, expires_at: SystemTime) -> Self {
        self.expires_at = expires_at;
        self
    }

    /// The same token, issued at `issued_at` instead — what a cluster whose clock runs
    /// ahead of the one reading the token hands out.
    pub fn issued_at(mut self, issued_at: SystemTime) -> Self {
        self.issued_at = issued_at;
        self
    }

    /// The same token, issued to `audience` instead of the pod identity audience — what
    /// a service account token projected for some other consumer carries.
    pub fn for_audience(mut self, audience: &[&str]) -> Self {
        self.audience = audience.iter().map(|a| (*a).to_string()).collect();
        self
    }
}

/// One pod identity association as the EKS control plane published it, for
/// [`ComAmazonawsEksauthEksauthfrontendState::program_pod_identity_associations`] to
/// program into this plane.
///
/// It carries the association's own identity — the id and ARN that control plane minted —
/// because that is what this service reports back in `podIdentityAssociation`: an id
/// minted here instead would name a resource no `DescribePodIdentityAssociation` could
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgrammedAssociation {
    pub association_id: String,
    pub association_arn: String,
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String,
    /// The association's own session policy, when it carries one: "the permissions for
    /// the Pod Identity association" (`PodIdentityAssociation$policy`), which scopes the
    /// session this service vends down to the intersection of the role's policies and it.
    pub policy: Option<String>,
    /// The role the association chains into, when it names one
    /// (`PodIdentityAssociation$targetRoleArn`).
    pub target_role_arn: Option<String>,
    /// The external id the control plane generated for that chain
    /// (`PodIdentityAssociation$externalId`), which the target role's trust policy
    /// conditions on.
    pub external_id: Option<String>,
    /// Whether the association turns the pod identity session tags off
    /// (`PodIdentityAssociation$disableSessionTags`).
    pub disable_session_tags: Option<bool>,
}

/// One pod identity association: a (namespace, service account) pair in a cluster bound
/// to an IAM role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodIdentityAssociationRecord {
    pub association_id: String,
    pub association_arn: String,
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String,
    /// The IAM role's own id — the first half of `assumedRoleUser.assumeRoleId`.
    pub role_id: String,
    /// See [`ProgrammedAssociation::policy`].
    pub policy: Option<String>,
    /// See [`ProgrammedAssociation::target_role_arn`].
    pub target_role_arn: Option<String>,
    /// See [`ProgrammedAssociation::external_id`].
    pub external_id: Option<String>,
    /// See [`ProgrammedAssociation::disable_session_tags`].
    pub disable_session_tags: Option<bool>,
}

/// One session `AssumeRoleForPodIdentity` minted: the credentials it returned, when they
/// expire, and which association and pod they were minted for.
#[derive(Debug, Clone)]
pub struct PodIdentitySession {
    pub access_key_id: String,
    pub expiration: SystemTime,
    pub cluster_name: String,
    pub association_id: String,
    pub assumed_role_arn: String,
    pub assume_role_id: String,
    pub namespace: String,
    pub service_account: String,
    pub eks_node_name: Option<String>,
    pub instance_id: Option<String>,
    pub zone: Option<String>,
}

/// The alphabet AWS unique ids (`AIDA…`, `AROA…`) are drawn from: uppercase letters and
/// the digits 2-7, which is what the modelled `idType` (`^[\w]+$`) admits.
const UNIQUE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The number of characters after an AWS unique id's four-character type prefix, so that
/// `AROA` plus this is the 21-character id IAM mints for a role.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// An AWS unique id: `prefix` (`AROA` for a role) plus [`UNIQUE_ID_SUFFIX_LEN`]
/// characters of [`UNIQUE_ID_ALPHABET`].
///
/// IAM mints such an id when the resource is created, so this mints one per resource and
/// the caller stores it — see [`ComAmazonawsEksauthEksauthfrontendState::role_ids`]. It
/// is deliberately NOT derived from the resource's name or ARN: a derived id would be the
/// same for two different accounts' roles of the same name, and would change for a role
/// that is renamed, neither of which is true of IAM.
fn unique_id(prefix: &str) -> String {
    let suffix = random_bytes(UNIQUE_ID_SUFFIX_LEN)
        .into_iter()
        .map(|byte| char::from(UNIQUE_ID_ALPHABET[usize::from(byte) % UNIQUE_ID_ALPHABET.len()]));
    prefix.chars().chain(suffix).collect()
}

/// A UUID in the random (version 4) form, which is what the model documents at the end of
/// `assumedRoleUser.assumeRoleId`: `eks-<clustername>-<podname>-<random UUID>`.
///
/// Rendered here from [`random_bytes`] rather than taken from a generator crate, because
/// this crate's ids need to be unique within one emulator process, not reproducible, and
/// the version and variant nibbles are the only structure a v4 UUID has.
pub fn random_uuid() -> String {
    let bytes = random_bytes(16);
    let mut hex = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            hex.push('-');
        }
        // Version 4 in the high nibble of byte 6, variant 0b10 in the high bits of
        // byte 8, as a random UUID renders them.
        let byte = match index {
            6 => (byte & 0x0f) | 0x40,
            8 => (byte & 0x3f) | 0x80,
            _ => *byte,
        };
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// `len` lowercase hex characters of [`random_bytes`] — the form EKS association ids and
/// this service's issued tokens take.
fn random_hex(len: usize) -> String {
    let mut hex = String::with_capacity(len + 1);
    for byte in random_bytes(len.div_ceil(2)) {
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex.truncate(len);
    hex
}

/// `len` bytes for an identifier this service mints.
///
/// The standard library is the whole source: `RandomState` seeds its hasher keys from the
/// operating system, so hashing with a FRESH one yields bits that come from that seed,
/// and a process-wide counter is mixed in so two identifiers minted in the same instant
/// differ even if two `RandomState`s ever repeat their keys. Nothing here is derived from
/// a request, and no identifier is reused.
///
/// This is an identifier source, not a key generator: the one value it produces that
/// grants anything — the token a cluster issues — is only ever minted through this
/// in-process seam, never from a request, and the credentials the operation returns come
/// from the emulator's own credential store instead.
fn random_bytes(len: usize) -> Vec<u8> {
    static MINTED: AtomicU64 = AtomicU64::new(0);
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let word = RandomState::new().hash_one(MINTED.fetch_add(1, Ordering::Relaxed));
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// Whole seconds since the Unix epoch, which is how the operation's access record states
/// a session's expiry.
///
/// A time before the epoch is reported as 0: the record states an expiry, and no session
/// expires before the epoch.
pub(crate) fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEksauthEksauthfrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEksauthEksauthfrontendState>)
}

// ---------------------------------------------------------------------------
// The slice of this module that lives in a file of its own, under `state/`: what
// the credentials this service vends ARE — a session of the role the association
// binds the service account to, minted where every session of this emulator is
// minted and recorded where every service resolves one.
// ---------------------------------------------------------------------------

mod role_session;
pub use role_session::*;
