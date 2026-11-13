// SPDX-License-Identifier: CC0-1.0

//! The Kubernetes objects one cluster's API server holds, and the answers it gives
//! about them.
//!
//! `Cluster$endpoint` is "the endpoint for your Kubernetes API server", so what a
//! client reaches there is a Kubernetes API: it addresses `/api/v1/...`, sends and
//! reads object manifests, and is answered a `Status` object when a request cannot
//! be served. That API is not in the AWS Smithy surface, so the shapes here are
//! the Kubernetes ones (`Pod`, `ServiceAccount`, `Namespace`, `TokenRequest`,
//! `Status`) written as the JSON documents a client sends and reads.
//!
//! Everything a request is answered with is computed from these records: a pod is
//! reported with the `metadata.uid` the create minted for it, a token is issued
//! only for a service account the cluster holds and only bound to a pod it holds,
//! and a read of an object that was never created is the Kubernetes not-found
//! `Status`. Nothing is derived from the request's own text.
//!
//! The ROUTING and the socket live in [`super::kubernetes_api`]; this module is
//! the objects and the rules over them, so both the published address and any
//! later in-process caller answer from one place.

use super::*;

/// The namespaces a cluster's API server starts with, as Kubernetes creates them
/// with the cluster: the one objects are created in by default, and the three the
/// control plane's own components live in.
pub const INITIAL_NAMESPACES: [&str; 4] = [
    DEFAULT_NAMESPACE,
    "kube-system",
    "kube-public",
    "kube-node-lease",
];

/// The namespace a request that names none addresses, which is also the one a pod
/// identity check creates its pod in.
pub const DEFAULT_NAMESPACE: &str = "default";

/// The service account Kubernetes creates in every namespace, and the one a pod
/// that names none runs as.
pub const DEFAULT_SERVICE_ACCOUNT: &str = "default";

/// How many namespaces one cluster's API server holds.
///
/// CHOSEN, not published: the Kubernetes API puts no limit on namespaces, and a
/// caller can create them without end, so the store needs a bound of its own. It
/// is far above the handful a cluster of this emulator is driven with.
pub const MAX_NAMESPACES_PER_CLUSTER: usize = 64;

/// How many pods one namespace holds. CHOSEN, like [`MAX_NAMESPACES_PER_CLUSTER`]:
/// a create past it is refused rather than letting one caller grow the store
/// without end.
pub const MAX_PODS_PER_NAMESPACE: usize = 256;

/// How many service accounts one namespace holds. CHOSEN, like
/// [`MAX_NAMESPACES_PER_CLUSTER`].
pub const MAX_SERVICE_ACCOUNTS_PER_NAMESPACE: usize = 256;

/// The lifetime a `TokenRequest` that asks for none is issued for.
///
/// The Kubernetes API server's own default service account token lifetime is one
/// hour, which is what a `TokenRequest` with no `spec.expirationSeconds` is served
/// with.
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// The shortest lifetime a `TokenRequest` is SERVED: the Kubernetes API server
/// validates `spec.expirationSeconds` and refuses a request asking for less than
/// ten minutes ("may not specify a duration less than 10 minutes") rather than
/// extending it to the floor.
pub const MIN_TOKEN_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The longest lifetime a `TokenRequest` is served. The API server CAPS a request
/// that asks for more at its maximum rather than refusing it, and one day is the
/// maximum a Kubernetes API server is configured with by default.
pub const MAX_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The `apiVersion` of the core Kubernetes API group, which every object under
/// `/api/v1` carries.
pub const CORE_API_VERSION: &str = "v1";

/// The `apiVersion` a `TokenRequest` carries: the authentication API group.
pub const AUTHENTICATION_API_VERSION: &str = "authentication.k8s.io/v1";

/// One cluster's Kubernetes objects: its namespaces and what each one holds.
///
/// They live in this service's own store, keyed by the cluster
/// ([`ComAmazonawsEksAwswesleyfrontendState::kubernetes_api`]), so deleting the
/// cluster takes them with it and two clusters never share an object.
#[derive(Debug, Default)]
pub struct ClusterObjects {
    /// The cluster's namespaces by name. A `BTreeMap` so a listing walks a
    /// stable, sorted order; bounded by [`MAX_NAMESPACES_PER_CLUSTER`].
    pub namespaces: BTreeMap<String, NamespaceObject>,
}

/// One namespace and the objects inside it.
#[derive(Debug)]
pub struct NamespaceObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The service accounts of this namespace, by name. Bounded by
    /// [`MAX_SERVICE_ACCOUNTS_PER_NAMESPACE`].
    pub service_accounts: BTreeMap<String, ServiceAccountObject>,
    /// The pods of this namespace, by name. Bounded by
    /// [`MAX_PODS_PER_NAMESPACE`].
    pub pods: BTreeMap<String, PodObject>,
}

/// One service account: the identity a pod runs as, and what a `TokenRequest`
/// under it mints a token for.
#[derive(Debug)]
pub struct ServiceAccountObject {
    pub uid: String,
    pub created_at: SystemTime,
}

/// One pod object. It is never scheduled — no node of this emulator runs
/// containers — so it stays in the phase a pod that has not been bound to a node
/// is in, which is what a client that only needs the object's identity reads.
#[derive(Debug)]
pub struct PodObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The service account the pod runs as: `spec.serviceAccountName`, or the
    /// namespace's `default` account for a pod that names none, which is what
    /// Kubernetes fills in.
    pub service_account_name: String,
}

impl ClusterObjects {
    /// The objects a brand new cluster's API server holds: the namespaces
    /// Kubernetes creates with a cluster ([`INITIAL_NAMESPACES`]), each already
    /// holding the `default` service account Kubernetes creates in every
    /// namespace.
    ///
    /// `seed` makes every uid of this cluster distinct from another cluster's;
    /// uids are minted, never derived from a request.
    #[must_use]
    pub fn for_new_cluster(now: SystemTime) -> Self {
        let mut namespaces = BTreeMap::new();
        for name in INITIAL_NAMESPACES {
            let mut service_accounts = BTreeMap::new();
            service_accounts.insert(
                DEFAULT_SERVICE_ACCOUNT.to_string(),
                ServiceAccountObject {
                    uid: new_object_uid(),
                    created_at: now,
                },
            );
            namespaces.insert(
                name.to_string(),
                NamespaceObject {
                    uid: new_object_uid(),
                    created_at: now,
                    service_accounts,
                    pods: BTreeMap::new(),
                },
            );
        }
        Self { namespaces }
    }

    /// The namespace `name`, or the Kubernetes not-found `Status` for one this
    /// cluster does not hold.
    fn namespace(&self, name: &str) -> Result<&NamespaceObject, ApiRefusal> {
        self.namespaces
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", name))
    }

    /// `POST /api/v1/namespaces`: create the namespace the manifest names.
    pub fn create_namespace(
        &mut self,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "Namespace")?;
        if self.namespaces.contains_key(&name) {
            return Err(ApiRefusal::already_exists("namespaces", &name));
        }
        if self.namespaces.len() >= MAX_NAMESPACES_PER_CLUSTER {
            return Err(ApiRefusal::too_many(
                "namespaces",
                MAX_NAMESPACES_PER_CLUSTER,
            ));
        }
        self.namespaces.insert(
            name.clone(),
            NamespaceObject {
                uid: new_object_uid(),
                created_at: now,
                service_accounts: [(
                    DEFAULT_SERVICE_ACCOUNT.to_string(),
                    ServiceAccountObject {
                        uid: new_object_uid(),
                        created_at: now,
                    },
                )]
                .into_iter()
                .collect(),
                pods: BTreeMap::new(),
            },
        );
        self.namespace_json(&name)
    }

    /// `GET /api/v1/namespaces/{namespace}`.
    pub fn namespace_json(&self, name: &str) -> Result<::serde_json::Value, ApiRefusal> {
        let namespace = self.namespace(name)?;
        Ok(::serde_json::json!({
            "kind": "Namespace",
            "apiVersion": CORE_API_VERSION,
            "metadata": {
                "name": name,
                "uid": namespace.uid,
                "creationTimestamp": rfc3339_seconds(namespace.created_at),
            },
            "spec": {},
            "status": { "phase": "Active" },
        }))
    }

    /// `POST /api/v1/namespaces/{namespace}/serviceaccounts`.
    pub fn create_service_account(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "ServiceAccount")?;
        // The namespace is resolved first: an object cannot be created in a
        // namespace that does not exist.
        self.namespace(namespace)?;
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        if held.service_accounts.contains_key(&name) {
            return Err(ApiRefusal::already_exists("serviceaccounts", &name));
        }
        if held.service_accounts.len() >= MAX_SERVICE_ACCOUNTS_PER_NAMESPACE {
            return Err(ApiRefusal::too_many(
                "serviceaccounts",
                MAX_SERVICE_ACCOUNTS_PER_NAMESPACE,
            ));
        }
        held.service_accounts.insert(
            name.clone(),
            ServiceAccountObject {
                uid: new_object_uid(),
                created_at: now,
            },
        );
        self.service_account_json(namespace, &name)
    }

    /// `GET /api/v1/namespaces/{namespace}/serviceaccounts/{name}`.
    pub fn service_account_json(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let account = self
            .namespace(namespace)?
            .service_accounts
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("serviceaccounts", name))?;
        Ok(::serde_json::json!({
            "kind": "ServiceAccount",
            "apiVersion": CORE_API_VERSION,
            "metadata": {
                "name": name,
                "namespace": namespace,
                "uid": account.uid,
                "creationTimestamp": rfc3339_seconds(account.created_at),
            },
        }))
    }

    /// `GET /api/v1/namespaces/{namespace}/serviceaccounts`: the namespace's service
    /// accounts, as the `ServiceAccountList` a client reads.
    pub fn service_account_list_json(
        &self,
        namespace: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self
            .namespace(namespace)?
            .service_accounts
            .keys()
            .cloned()
            .collect();
        let items: Vec<::serde_json::Value> = names
            .iter()
            .filter_map(|name| self.service_account_json(namespace, name).ok())
            .collect();
        Ok(::serde_json::json!({
            "kind": "ServiceAccountList",
            "apiVersion": CORE_API_VERSION,
            "metadata": {},
            "items": items,
        }))
    }

    /// `POST /api/v1/namespaces/{namespace}/pods`: create the pod object the
    /// manifest names and answer it with the `metadata.uid` this create minted —
    /// the uid a `TokenRequest`'s `boundObjectRef` names.
    pub fn create_pod(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "Pod")?;
        let service_account_name = manifest
            .get("spec")
            .and_then(|spec| {
                spec.get("serviceAccountName")
                    .or_else(|| spec.get("serviceAccount"))
            })
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_SERVICE_ACCOUNT)
            .to_string();
        self.namespace(namespace)?;
        // "the pod's service account must exist": a pod naming a service account
        // the namespace does not hold is refused, as the API server refuses it.
        if !self
            .namespace(namespace)?
            .service_accounts
            .contains_key(&service_account_name)
        {
            return Err(ApiRefusal::not_found(
                "serviceaccounts",
                &service_account_name,
            ));
        }
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        if held.pods.contains_key(&name) {
            return Err(ApiRefusal::already_exists("pods", &name));
        }
        if held.pods.len() >= MAX_PODS_PER_NAMESPACE {
            return Err(ApiRefusal::too_many("pods", MAX_PODS_PER_NAMESPACE));
        }
        held.pods.insert(
            name.clone(),
            PodObject {
                uid: new_object_uid(),
                created_at: now,
                service_account_name,
            },
        );
        self.pod_json(namespace, &name)
    }

    /// `GET /api/v1/namespaces/{namespace}/pods/{name}`.
    pub fn pod_json(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let pod = self
            .namespace(namespace)?
            .pods
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found("pods", name))?;
        Ok(::serde_json::json!({
            "kind": "Pod",
            "apiVersion": CORE_API_VERSION,
            "metadata": {
                "name": name,
                "namespace": namespace,
                "uid": pod.uid,
                "creationTimestamp": rfc3339_seconds(pod.created_at),
            },
            "spec": {
                "serviceAccountName": pod.service_account_name,
                "serviceAccount": pod.service_account_name,
            },
            // No node of this emulator runs containers, so the pod is never
            // bound to one and stays in the phase an unscheduled pod is in.
            "status": { "phase": "Pending" },
        }))
    }

    /// `GET /api/v1/namespaces/{namespace}/pods`: the namespace's pods, as the
    /// `PodList` a client reads.
    pub fn pod_list_json(&self, namespace: &str) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self
            .namespace(namespace)?
            .pods
            .keys()
            .cloned()
            .collect();
        let items: Vec<::serde_json::Value> = names
            .iter()
            .filter_map(|name| self.pod_json(namespace, name).ok())
            .collect();
        Ok(::serde_json::json!({
            "kind": "PodList",
            "apiVersion": CORE_API_VERSION,
            "metadata": {},
            "items": items,
        }))
    }

    /// What a `POST .../serviceaccounts/{name}/token` is to mint, resolved against
    /// the objects this cluster holds: the identity the token names, the pod it is
    /// bound to, and the window it is issued for.
    ///
    /// Reading only — the token itself is minted by the EKS Auth plane, which is
    /// the one that answers for it afterwards (see
    /// [`super::pod_identity_plane`]) — so this never writes.
    pub fn plan_token(
        &self,
        namespace: &str,
        service_account: &str,
        request: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<TokenPlan, ApiRefusal> {
        // The service account the token is issued for has to exist: that read is
        // what makes a token name a Kubernetes identity rather than a string.
        self.service_account_json(namespace, service_account)?;
        let spec = request.get("spec");
        let audiences: Vec<String> = spec
            .and_then(|spec| spec.get("audiences"))
            .and_then(::serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let lifetime = match spec
            .and_then(|spec| spec.get("expirationSeconds"))
            .and_then(::serde_json::Value::as_u64)
        {
            None => DEFAULT_TOKEN_LIFETIME,
            Some(seconds) => {
                let asked = Duration::from_secs(seconds);
                // The floor is VALIDATED and the ceiling is CAPPED, which is what the
                // API server does with each: a request under the floor is refused
                // rather than served for longer than it asked for, and one past the
                // maximum is served at the maximum.
                if asked < MIN_TOKEN_LIFETIME {
                    return Err(ApiRefusal::invalid(format!(
                        "TokenRequest.spec.expirationSeconds: Invalid value: {seconds}: \
                         may not specify a duration less than {} seconds",
                        MIN_TOKEN_LIFETIME.as_secs()
                    )));
                }
                asked.min(MAX_TOKEN_LIFETIME)
            }
        };
        let bound = spec.and_then(|spec| spec.get("boundObjectRef"));
        let (pod_name, pod_uid) = match bound {
            None | Some(::serde_json::Value::Null) => (None, None),
            Some(reference) => {
                let kind = reference
                    .get("kind")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                let name = reference
                    .get("name")
                    .and_then(::serde_json::Value::as_str)
                    .unwrap_or_default();
                if !kind.eq_ignore_ascii_case("Pod") {
                    return Err(ApiRefusal::invalid(format!(
                        "cannot bind token to object of kind {kind:?}"
                    )));
                }
                // The pod the reference names has to be one this cluster holds,
                // and the uid the reference carries — when it carries one — has
                // to be that pod's: a reference naming another object's uid binds
                // the token to nothing.
                let pod = self
                    .namespace(namespace)?
                    .pods
                    .get(name)
                    .ok_or_else(|| ApiRefusal::not_found("pods", name))?;
                if let Some(claimed) = reference
                    .get("uid")
                    .and_then(::serde_json::Value::as_str)
                    .filter(|uid| !uid.is_empty())
                {
                    if claimed != pod.uid {
                        return Err(ApiRefusal::not_found("pods", name));
                    }
                }
                (Some(name.to_string()), Some(pod.uid.clone()))
            }
        };
        Ok(TokenPlan {
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
            pod_name,
            pod_uid,
            audiences,
            issued_at: now,
            expires_at: now + lifetime,
        })
    }
}

/// What a `TokenRequest` is to be served with, as the objects resolved it.
#[derive(Debug, Clone)]
pub struct TokenPlan {
    pub namespace: String,
    pub service_account: String,
    /// The pod the request bound the token to, when it bound it to one.
    pub pod_name: Option<String>,
    /// That pod's `metadata.uid`, which the token's `kubernetes.io/pod` claim
    /// carries beside its name.
    pub pod_uid: Option<String>,
    /// The audiences the request asked for; empty when it asked for none, which
    /// the API server serves as the API server's own audience.
    pub audiences: Vec<String>,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
}

impl TokenPlan {
    /// The `TokenRequest` a client reads back, carrying the minted `token` and the
    /// instant it expires.
    #[must_use]
    pub fn to_response(&self, token: &str) -> ::serde_json::Value {
        ::serde_json::json!({
            "kind": "TokenRequest",
            "apiVersion": AUTHENTICATION_API_VERSION,
            "metadata": {
                "name": self.service_account,
                "namespace": self.namespace,
                "creationTimestamp": rfc3339_seconds(self.issued_at),
            },
            "spec": {
                "audiences": self.audiences,
                "expirationSeconds": self
                    .expires_at
                    .duration_since(self.issued_at)
                    .unwrap_or_default()
                    .as_secs(),
                "boundObjectRef": self.pod_name.as_ref().map(|name| ::serde_json::json!({
                    "kind": "Pod",
                    "name": name,
                    "uid": self.pod_uid,
                })),
            },
            "status": {
                "token": token,
                "expirationTimestamp": rfc3339_seconds(self.expires_at),
            },
        })
    }
}

/// A request the API server does not serve, as the `Status` object Kubernetes
/// answers with: the HTTP status, the machine-readable `reason`, and the sentence
/// that names what could not be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRefusal {
    pub status: u16,
    pub reason: &'static str,
    pub message: String,
    /// The resource the request addressed, as `Status.details.kind` carries it,
    /// and its name — absent for a refusal about no one object.
    pub resource: Option<(String, String)>,
}

impl ApiRefusal {
    /// The `Status` a read of an object that does not exist is answered with:
    /// `404 NotFound`, worded as the API server words it
    /// (`<resource> "<name>" not found`).
    #[must_use]
    pub fn not_found(resource: &str, name: &str) -> Self {
        Self {
            status: 404,
            reason: "NotFound",
            message: format!("{resource} \"{name}\" not found"),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a create of an object that is already there is answered with:
    /// `409 AlreadyExists`, in the API server's own wording.
    #[must_use]
    pub fn already_exists(resource: &str, name: &str) -> Self {
        Self {
            status: 409,
            reason: "AlreadyExists",
            message: format!("{resource} \"{name}\" already exists"),
            resource: Some((resource.to_string(), name.to_string())),
        }
    }

    /// The `Status` a request the API server cannot read is answered with:
    /// `422 Invalid`, which is what an unprocessable manifest is.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 422,
            reason: "Invalid",
            message: message.into(),
            resource: None,
        }
    }

    /// The `Status` a create past what this emulator holds for one cluster is
    /// answered with: `403 Forbidden`, the status Kubernetes answers a create a
    /// quota denies.
    #[must_use]
    pub fn too_many(resource: &str, limit: usize) -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            message: format!(
                "exceeded quota: this cluster holds at most {limit} {resource} per namespace"
            ),
            resource: None,
        }
    }

    /// The `Status` a request to a path the API server does not serve is answered
    /// with: `404 NotFound` naming the path, as the API server answers one.
    #[must_use]
    pub fn no_such_path(path: &str) -> Self {
        Self {
            status: 404,
            reason: "NotFound",
            message: format!("the server could not find the requested resource: {path}"),
            resource: None,
        }
    }

    /// The `Status` a method the addressed resource does not serve is answered
    /// with: `405 MethodNotAllowed`.
    #[must_use]
    pub fn method_not_allowed(method: &str, path: &str) -> Self {
        Self {
            status: 405,
            reason: "MethodNotAllowed",
            message: format!("the server does not allow {method} on {path}"),
            resource: None,
        }
    }

    /// The `Status` a request this emulator could not serve at all is answered
    /// with: `500 InternalError`. `detail` names the internal step and is the
    /// sentence the caller reads, so it never carries request content.
    #[must_use]
    pub fn internal(detail: &'static str) -> Self {
        Self {
            status: 500,
            reason: "InternalError",
            message: detail.to_string(),
            resource: None,
        }
    }

    /// This refusal as the `Status` object a Kubernetes client parses.
    #[must_use]
    pub fn to_status_json(&self) -> ::serde_json::Value {
        let details = self.resource.as_ref().map(|(kind, name)| {
            ::serde_json::json!({ "name": name, "kind": kind })
        });
        ::serde_json::json!({
            "kind": "Status",
            "apiVersion": CORE_API_VERSION,
            "metadata": {},
            "status": "Failure",
            "message": self.message,
            "reason": self.reason,
            "details": details,
            "code": self.status,
        })
    }
}

/// The `metadata.name` a manifest names, or the `Status` the API server answers a
/// manifest that names none: "an empty name is not permitted".
///
/// A name has to be one a Kubernetes object may carry — a lowercase RFC 1123
/// subdomain — which is this service's OWN reading of that form
/// ([`is_kubernetes_name`], the one the control plane judges an association's
/// `namespace` and `serviceAccount` by), called rather than copied so the control
/// plane and the API server cannot disagree about which names exist.
///
/// `generateName` is served too — a client that sends one asks the API server to
/// mint the name — by appending the five random characters Kubernetes appends.
fn manifest_name(manifest: &::serde_json::Value, kind: &str) -> Result<String, ApiRefusal> {
    let metadata = manifest.get("metadata");
    if let Some(name) = metadata
        .and_then(|metadata| metadata.get("name"))
        .and_then(::serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
    {
        if !is_kubernetes_name(name) {
            return Err(ApiRefusal::invalid(format!(
                "{kind} \"{name}\" is invalid: metadata.name: Invalid value: \"{name}\": a \
                 lowercase RFC 1123 subdomain must consist of lower case alphanumeric \
                 characters, '-' or '.', and must start and end with an alphanumeric character"
            )));
        }
        return Ok(name.to_string());
    }
    if let Some(prefix) = metadata
        .and_then(|metadata| metadata.get("generateName"))
        .and_then(::serde_json::Value::as_str)
        .filter(|prefix| !prefix.is_empty())
    {
        let generated = format!("{prefix}{}", generated_name_suffix());
        if !is_kubernetes_name(&generated) {
            return Err(ApiRefusal::invalid(format!(
                "{kind} is invalid: metadata.generateName: Invalid value: \"{prefix}\": a \
                 lowercase RFC 1123 subdomain must consist of lower case alphanumeric \
                 characters, '-' or '.', and must start and end with an alphanumeric character"
            )));
        }
        return Ok(generated);
    }
    Err(ApiRefusal::invalid(format!(
        "{kind} is invalid: metadata.name: Required value: name or generateName is required"
    )))
}

/// The five characters Kubernetes appends to a `generateName`, drawn from the
/// alphabet it draws them from.
fn generated_name_suffix() -> String {
    const ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
    new_object_uid()
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .take(5)
        .map(|byte| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
        .collect()
}

/// A fresh `metadata.uid`: the version-4 UUID Kubernetes gives every object.
///
/// Minted per object through this service's own UUID minter, never derived from
/// the request, so two objects of one name created at different times carry
/// different uids — which is what a `boundObjectRef` naming a uid relies on.
#[must_use]
pub fn new_object_uid() -> String {
    new_peer_request_id()
}

/// `at` as the RFC 3339 instant a Kubernetes `metadata.creationTimestamp` carries:
/// UTC at whole-second resolution, which is the resolution the API serves.
#[must_use]
pub fn rfc3339_seconds(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    let instant = ::aws_smithy_types::DateTime::from_secs(
        i64::try_from(seconds).unwrap_or_default(),
    );
    instant
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
