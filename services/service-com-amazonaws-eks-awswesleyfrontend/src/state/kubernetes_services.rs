// SPDX-License-Identifier: CC0-1.0

//! The core `v1` SERVICES one cluster's API server holds, and the addresses they
//! answer at.
//!
//! A Service is how a client names a workload inside a cluster, so it is one of
//! the first objects anything deployed through the published endpoint creates: a
//! `405` on the collection says the verb is unsupported by the resource, which is a
//! permanent answer a client gives up on.
//!
//! Its address is not invented here. `KubernetesNetworkConfigResponse$serviceIpv4Cidr`
//! is "the CIDR block that Kubernetes `Pod` and `Service` object IP addresses are
//! assigned from", and `serviceIpv6Cidr` says the same for a cluster whose
//! `ipFamily` is `ipv6` — so a `clusterIP` is ALLOCATED out of the block the
//! cluster's own record publishes ([`ServiceNetwork`]), and the control plane and
//! the Kubernetes API cannot disagree about which addresses a cluster's services
//! have. The allocation is stored on the object, so every later read reports the
//! address the create assigned, and deleting the service gives that address back.
//!
//! Every cluster also holds the Service its API server itself is reached by inside
//! the cluster — `default/kubernetes`, on the first address of that block — which is
//! materialised like every other clock-owed fact of this plane
//! ([`ClusterObjects::materialize_api_server_service`]): it was owed from the
//! instant the cluster was created, whether or not anybody called.
//!
//! The objects live inside their namespace ([`super::kubernetes_objects`]), so
//! deleting the cluster takes them with it and two clusters never share one.

use ::std::net::IpAddr;
use ::std::ops::RangeInclusive;

use super::kubernetes_lists::{listed, ListRequest};
use super::kubernetes_metadata::{invalid_member, string_value_map, ObjectMetadata};
use super::kubernetes_objects::{
    deleted_status_json, manifest_name, new_object_uid, ApiRefusal, ClusterObjects,
    DEFAULT_NAMESPACE, CORE_API_VERSION,
};
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;
use super::*;

/// The resource a Service is addressed and refused by.
pub const SERVICES_RESOURCE: &str = "services";

/// The `kind` of one Service object, and of the list a collection read answers.
pub const SERVICE_KIND: &str = "Service";
const SERVICE_LIST_KIND: &str = "ServiceList";

/// How many Services one namespace holds.
///
/// CHOSEN, not published: the Kubernetes API puts no limit on how many Services a
/// namespace holds, and a caller can create them without end, so the store needs a
/// bound of its own. It is the bound this plane already puts on the pods, the
/// service accounts and the ConfigMaps of one namespace.
pub const MAX_SERVICES_PER_NAMESPACE: usize = 256;

/// How many ports ONE Service declares.
///
/// CHOSEN, not published: Kubernetes bounds a Service's `ports` only by the size of
/// the request that creates it, and every port is kept on the stored object and
/// walked by every later read and by the node-port allocation — so the store needs a
/// bound of its own, far above the handful a Service declares. It is the bound this
/// plane already puts on the entries of one RBAC rule
/// ([`super::kubernetes_rbac::MAX_ENTRIES_PER_RULE`]).
pub const MAX_PORTS_PER_SERVICE: usize = 64;

/// The `spec.clusterIP` a client asks for when it wants NO address at all: "A
/// headless Service does not get a cluster IP", which is how a client asks for one.
pub const HEADLESS_CLUSTER_IP: &str = "None";

/// The `ServiceType` members this resource serves, from the Kubernetes API
/// reference for `ServiceSpec$type`: "ClusterIP, NodePort, LoadBalancer and
/// ExternalName", with `ClusterIP` the default for a manifest that names none.
pub const SERVICE_TYPE_CLUSTER_IP: &str = "ClusterIP";
/// See [`SERVICE_TYPE_CLUSTER_IP`].
pub const SERVICE_TYPE_NODE_PORT: &str = "NodePort";
/// See [`SERVICE_TYPE_CLUSTER_IP`].
pub const SERVICE_TYPE_LOAD_BALANCER: &str = "LoadBalancer";
/// See [`SERVICE_TYPE_CLUSTER_IP`].
pub const SERVICE_TYPE_EXTERNAL_NAME: &str = "ExternalName";

/// The four, in the order the API server lists them when it refuses a value
/// outside them.
const SERVICE_TYPES: [&str; 4] = [
    SERVICE_TYPE_CLUSTER_IP,
    SERVICE_TYPE_EXTERNAL_NAME,
    SERVICE_TYPE_LOAD_BALANCER,
    SERVICE_TYPE_NODE_PORT,
];

/// The ports a `NodePort` is allocated from: the API server's own
/// `--service-node-port-range`, whose default is `30000-32767`.
const NODE_PORT_RANGE: RangeInclusive<i64> = 30000..=32767;

/// The port numbers a Service's `port` and `targetPort` may name: a TCP/UDP port.
const PORT_RANGE: RangeInclusive<i64> = 1..=65535;

/// The `protocol` a port that names none runs, which is the default the Kubernetes
/// API fills in.
const DEFAULT_PROTOCOL: &str = "TCP";

/// The protocols a Service port may name (`ServicePort$protocol`: "TCP, UDP, or
/// SCTP").
const PROTOCOLS: [&str; 3] = ["TCP", "UDP", "SCTP"];

/// The Service every cluster holds for its own API server: `kubernetes` in the
/// `default` namespace, which is how anything running inside the cluster reaches
/// the control plane.
pub const API_SERVER_SERVICE: &str = "kubernetes";

/// The port that Service answers on, which is the HTTPS port a Kubernetes API
/// server is reached at.
const API_SERVER_SERVICE_PORT: i64 = 443;

/// The name its one port carries, as every cluster's `default/kubernetes` carries
/// it.
const API_SERVER_SERVICE_PORT_NAME: &str = "https";

/// The fields a SERVICE may be SELECTED by, as `(the field label a client writes,
/// the member of the service document it names)` — the two every namespaced
/// object's registry indexes, and the two of a Service's own spec the API server
/// indexes.
pub const SERVICE_FIELD_SELECTORS: &[(&str, &str)] = &[
    ("metadata.name", "metadata.name"),
    ("metadata.namespace", "metadata.namespace"),
    ("spec.clusterIP", "spec.clusterIP"),
    ("spec.type", "spec.type"),
];

/// The block a cluster's Service addresses are allocated out of, as the cluster's
/// own record publishes it.
///
/// `serviceIpv4Cidr` is "the CIDR block that Kubernetes `Pod` and `Service` object
/// IP addresses are assigned from" and `serviceIpv6Cidr` is the same block for a
/// cluster created with `ipFamily: ipv6`, and the response "contains a value for
/// serviceIpv6Cidr or serviceIpv4Cidr, but not both" — so one block is read, and a
/// record publishing neither (a cluster `RegisterCluster` connected, which has no
/// Amazon EKS control plane) has none.
#[derive(Debug, Clone, Default)]
pub struct ServiceNetwork {
    block: Option<::ipnet::IpNet>,
}

impl ServiceNetwork {
    /// The block `cidr` states, and no block for a value that is not a CIDR block
    /// at all. The reading is `ipnet`'s, the one this crate already reads a
    /// cluster's service CIDR with.
    #[must_use]
    pub fn of_cidr(cidr: Option<&str>) -> Self {
        Self {
            block: cidr.and_then(|cidr| cidr.parse::<::ipnet::IpNet>().ok()),
        }
    }

    /// The block as the caller stated it, which a refusal names as the range of
    /// valid addresses.
    #[must_use]
    pub fn cidr(&self) -> Option<String> {
        self.block.map(|block| block.to_string())
    }

    /// The `ipFamilies` member a Service of this block reports: the family the
    /// block itself is in, which is the family Kubernetes assigns from.
    fn ip_family(&self) -> &'static str {
        match self.block {
            Some(::ipnet::IpNet::V6(_)) => "IPv6",
            _ => "IPv4",
        }
    }

    /// Whether this block holds `address`.
    fn holds(&self, address: IpAddr) -> bool {
        self.block.is_some_and(|block| block.contains(&address))
    }

    /// The lowest address of this block that `taken` does not hold, and `None` for
    /// a cluster with no block and for a block whose addresses are all taken.
    ///
    /// The walk is the block's own address order and stops at the first free
    /// address, so it takes at most one step per address already allocated — which
    /// is bounded by the services a cluster holds
    /// ([`MAX_SERVICES_PER_NAMESPACE`] per namespace).
    fn lowest_free(&self, taken: &::std::collections::BTreeSet<IpAddr>) -> Option<IpAddr> {
        self.block?
            .hosts()
            .find(|address| !taken.contains(address))
    }
}

impl ServiceStore {
    /// The block the cluster `cluster_name` in `region` allocates its Service
    /// addresses out of, read from the record the create wrote — so the block a
    /// Service's address comes out of is the one `DescribeCluster` reports.
    #[must_use]
    pub fn service_network(&self, region: &str, cluster_name: &str) -> ServiceNetwork {
        let network = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
            .and_then(|cluster| cluster.kubernetes_network_config.as_ref());
        ServiceNetwork::of_cidr(
            network
                .and_then(|network| network.service_ipv4_cidr.as_deref())
                .or_else(|| network.and_then(|network| network.service_ipv6_cidr.as_deref())),
        )
    }
}

/// The address one Service answers at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterAddress {
    /// The address allocated out of the cluster's service CIDR.
    Allocated(IpAddr),
    /// A headless Service, which asked for no address: `spec.clusterIP` is
    /// reported as `None`.
    Headless,
    /// A Service that has no `clusterIP` member at all, which is what an
    /// `ExternalName` Service is.
    Unaddressed,
}

/// One port a Service answers on.
#[derive(Debug, Clone)]
pub struct ServicePortObject {
    /// `name`: required once a Service declares more than one port, and absent on
    /// a single-port Service that named none.
    pub name: Option<String>,
    pub protocol: String,
    pub port: i64,
    /// `targetPort`: "the port to access on the pods targeted by the service",
    /// which is a number or the NAME of a port of the pod — so it is kept as the
    /// value the manifest carried. A manifest that names none targets the
    /// Service's own port, which is what Kubernetes fills in.
    pub target_port: ::serde_json::Value,
    /// `nodePort`: the port every node answers this Service on, allocated out of
    /// [`NODE_PORT_RANGE`] for the types that have one.
    pub node_port: Option<i64>,
}

impl ServicePortObject {
    /// This port as a client reads it back.
    #[must_use]
    fn to_json(&self) -> ::serde_json::Value {
        let mut reported = ::serde_json::Map::new();
        if let Some(name) = &self.name {
            reported.insert("name".to_string(), ::serde_json::json!(name));
        }
        reported.insert("protocol".to_string(), ::serde_json::json!(self.protocol));
        reported.insert("port".to_string(), ::serde_json::json!(self.port));
        reported.insert("targetPort".to_string(), self.target_port.clone());
        if let Some(node_port) = self.node_port {
            reported.insert("nodePort".to_string(), ::serde_json::json!(node_port));
        }
        ::serde_json::Value::Object(reported)
    }
}

/// One Service: the address the cluster allocated it, the pods its selector names,
/// and the ports it answers on.
#[derive(Debug)]
pub struct ServiceObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that stored this Service was stamped with.
    pub resource_version: u64,
    /// The labels and annotations the manifest carried.
    pub metadata: ObjectMetadata,
    /// `spec.type`.
    pub service_type: String,
    /// The address this Service answers at.
    pub address: ClusterAddress,
    /// `spec.externalName`, which only an `ExternalName` Service carries.
    pub external_name: Option<String>,
    /// `spec.selector`: "route service traffic to pods with label keys and values
    /// matching this selector". Empty for a Service that selects no pods.
    pub selector: BTreeMap<String, String>,
    pub ports: Vec<ServicePortObject>,
}

impl ServiceObject {
    /// This Service as the object a client reads: the spec the create admitted,
    /// the address it was allocated, and the status of a Service nothing has
    /// provisioned an external address for.
    #[must_use]
    pub fn to_json(
        &self,
        namespace: &str,
        name: &str,
        network: &ServiceNetwork,
    ) -> ::serde_json::Value {
        let mut spec = ::serde_json::Map::new();
        spec.insert("type".to_string(), ::serde_json::json!(self.service_type));
        let ports: Vec<::serde_json::Value> =
            self.ports.iter().map(ServicePortObject::to_json).collect();
        if !ports.is_empty() {
            spec.insert("ports".to_string(), ::serde_json::json!(ports));
        }
        if !self.selector.is_empty() {
            spec.insert("selector".to_string(), ::serde_json::json!(self.selector));
        }
        match self.address {
            ClusterAddress::Allocated(address) => {
                let reported = address.to_string();
                spec.insert("clusterIP".to_string(), ::serde_json::json!(reported));
                // "ClusterIPs is a list of IP addresses assigned to this service
                // ... ClusterIP and ClusterIPs must agree": one address is
                // allocated here, so the list carries that one.
                spec.insert("clusterIPs".to_string(), ::serde_json::json!([reported]));
                spec.insert(
                    "ipFamilies".to_string(),
                    ::serde_json::json!([network.ip_family()]),
                );
                spec.insert(
                    "ipFamilyPolicy".to_string(),
                    ::serde_json::json!("SingleStack"),
                );
                spec.insert(
                    "internalTrafficPolicy".to_string(),
                    ::serde_json::json!("Cluster"),
                );
            }
            ClusterAddress::Headless => {
                spec.insert(
                    "clusterIP".to_string(),
                    ::serde_json::json!(HEADLESS_CLUSTER_IP),
                );
                spec.insert(
                    "clusterIPs".to_string(),
                    ::serde_json::json!([HEADLESS_CLUSTER_IP]),
                );
            }
            ClusterAddress::Unaddressed => {}
        }
        if let Some(external_name) = &self.external_name {
            spec.insert(
                "externalName".to_string(),
                ::serde_json::json!(external_name),
            );
        }
        // The two members the API server fills in on every Service it stores.
        spec.insert("sessionAffinity".to_string(), ::serde_json::json!("None"));
        ::serde_json::json!({
            "kind": SERVICE_KIND,
            "apiVersion": CORE_API_VERSION,
            "metadata": self.metadata.render(
                name,
                Some(namespace),
                &self.uid,
                self.created_at,
                self.resource_version,
            ),
            "spec": ::serde_json::Value::Object(spec),
            // "LoadBalancer contains the current status of the load-balancer, if
            // one is present": nothing in this emulator provisions an external
            // load balancer, so no ingress is reported for one.
            "status": { "loadBalancer": {} },
        })
    }

    /// The address this Service holds, for the allocator to leave alone.
    fn allocated(&self) -> Option<IpAddr> {
        match self.address {
            ClusterAddress::Allocated(address) => Some(address),
            _ => None,
        }
    }
}

impl ClusterObjects {
    /// `POST /api/v1/namespaces/{namespace}/services`: store the Service the
    /// manifest names, with the address the cluster allocated it, and answer it as
    /// the object that was stored.
    pub fn create_service(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
        network: &ServiceNetwork,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, SERVICE_KIND)?;
        let metadata = ObjectMetadata::from_manifest(SERVICE_KIND, &name, manifest)?;
        let spec = manifest.get("spec");
        let service_type = service_type_of(&name, spec)?;
        let external_name = external_name_of(&name, spec, &service_type)?;
        let selector = string_value_map(
            SERVICE_KIND,
            &name,
            "spec.selector",
            spec.and_then(|spec| spec.get("selector")),
        )?;
        let declared = declared_ports(&name, spec, &service_type)?;
        let address = self.address_for(&name, spec, &service_type, network)?;
        let ports = self.allocated_ports(declared, &service_type);
        let resource_version = self.versions.next();
        // The manifest is read and validated first — a refused manifest stores
        // nothing wherever it was addressed — and only then is the namespace
        // resolved: an object cannot be created in a namespace that does not
        // exist.
        let held = self.namespace_for_create(namespace, SERVICES_RESOURCE, &name)?;
        if held.services.contains_key(&name) {
            return Err(ApiRefusal::already_exists(SERVICES_RESOURCE, &name));
        }
        if held.services.len() >= MAX_SERVICES_PER_NAMESPACE {
            return Err(ApiRefusal::too_many(
                SERVICES_RESOURCE,
                MAX_SERVICES_PER_NAMESPACE,
            ));
        }
        held.services.insert(
            name.clone(),
            ServiceObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                metadata,
                service_type,
                address,
                external_name,
                selector,
                ports,
            },
        );
        self.service_json(namespace, &name, network)
    }

    /// `GET /api/v1/namespaces/{namespace}/services/{name}`.
    pub fn service_json(
        &self,
        namespace: &str,
        name: &str,
        network: &ServiceNetwork,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let service = self
            .namespace(namespace)?
            .services
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found(SERVICES_RESOURCE, name))?;
        Ok(service.to_json(namespace, name, network))
    }

    /// `GET /api/v1/namespaces/{namespace}/services`: the namespace's Services, as
    /// the `ServiceList` a client reads — one chunk of it
    /// ([`super::kubernetes_lists`]).
    pub fn service_list_json(
        &self,
        namespace: &str,
        request: &ListRequest,
        network: &ServiceNetwork,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self.namespace(namespace)?.services.keys().cloned().collect();
        Ok(listed(SERVICE_LIST_KIND, &names, request, |name| {
            self.service_json(namespace, name, network).ok()
        }))
    }

    /// `DELETE /api/v1/namespaces/{namespace}/services/{name}`: give up the
    /// Service and the address it held, which the next allocation may then hand
    /// out.
    pub fn delete_service(
        &mut self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        held.services
            .remove(name)
            .ok_or_else(|| ApiRefusal::not_found(SERVICES_RESOURCE, name))?;
        Ok(deleted_status_json(SERVICES_RESOURCE, name, None))
    }

    /// Materialise the Service every Kubernetes cluster holds for its own API
    /// server: `kubernetes` in the `default` namespace, on the first address of the
    /// cluster's service block.
    ///
    /// It was owed from the instant the cluster was created — every Kubernetes
    /// cluster has it, and it is how anything running inside the cluster reaches the
    /// control plane — so materialising it on a read changes nothing a later caller
    /// would not have seen. IDEMPOTENT: a cluster that already holds the object
    /// keeps it exactly as it is, and a cluster whose record publishes no service
    /// block gets none rather than an address out of nothing.
    pub fn materialize_api_server_service(
        &mut self,
        network: &ServiceNetwork,
        now: SystemTime,
    ) {
        if self
            .namespaces
            .get(DEFAULT_NAMESPACE)
            .is_none_or(|held| held.services.contains_key(API_SERVER_SERVICE))
        {
            return;
        }
        let Some(address) = network.lowest_free(&self.allocated_addresses()) else {
            return;
        };
        let resource_version = self.versions.next();
        let Some(held) = self.namespaces.get_mut(DEFAULT_NAMESPACE) else {
            return;
        };
        held.services.insert(
            API_SERVER_SERVICE.to_string(),
            ServiceObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                metadata: ObjectMetadata::default(),
                service_type: SERVICE_TYPE_CLUSTER_IP.to_string(),
                address: ClusterAddress::Allocated(address),
                external_name: None,
                // The API server is not a pod of the cluster, so no selector names
                // it: the endpoints behind this Service are the control plane's own
                // address.
                selector: BTreeMap::new(),
                ports: vec![ServicePortObject {
                    name: Some(API_SERVER_SERVICE_PORT_NAME.to_string()),
                    protocol: DEFAULT_PROTOCOL.to_string(),
                    port: API_SERVER_SERVICE_PORT,
                    target_port: ::serde_json::json!(API_SERVER_SERVICE_PORT),
                    node_port: None,
                }],
            },
        );
    }

    /// Every address the Services of this cluster hold, which is what the next
    /// allocation leaves alone.
    fn allocated_addresses(&self) -> ::std::collections::BTreeSet<IpAddr> {
        self.namespaces
            .values()
            .flat_map(|held| held.services.values())
            .filter_map(ServiceObject::allocated)
            .collect()
    }

    /// Every node port the Services of this cluster hold.
    fn allocated_node_ports(&self) -> ::std::collections::BTreeSet<i64> {
        self.namespaces
            .values()
            .flat_map(|held| held.services.values())
            .flat_map(|service| service.ports.iter())
            .filter_map(|port| port.node_port)
            .collect()
    }

    /// The address a Service of `service_type` whose manifest carried `spec` is
    /// created with: the one it asked for, the lowest free address of the cluster's
    /// block, or no address at all.
    ///
    /// "clusterIP: `None`" asks for a headless Service, an `ExternalName` Service
    /// has no address of its own, and a manifest naming an address asks for THAT
    /// address — which the API server allocates only when the cluster's block holds
    /// it and nothing else does.
    fn address_for(
        &self,
        name: &str,
        spec: Option<&::serde_json::Value>,
        service_type: &str,
        network: &ServiceNetwork,
    ) -> Result<ClusterAddress, ApiRefusal> {
        let requested = spec
            .and_then(|spec| spec.get("clusterIP"))
            .and_then(::serde_json::Value::as_str)
            .filter(|requested| !requested.is_empty());
        if service_type == SERVICE_TYPE_EXTERNAL_NAME {
            return Ok(ClusterAddress::Unaddressed);
        }
        if requested == Some(HEADLESS_CLUSTER_IP) {
            return Ok(ClusterAddress::Headless);
        }
        let taken = self.allocated_addresses();
        if let Some(requested) = requested {
            let address = requested
                .parse::<IpAddr>()
                .ok()
                .filter(|address| network.holds(*address))
                .ok_or_else(|| {
                    unallocatable_address(
                        name,
                        requested,
                        &format!(
                            "the provided IP ({requested}) is not in the valid range. The range \
                             of valid IPs is {}",
                            network.cidr().unwrap_or_default()
                        ),
                    )
                })?;
            if taken.contains(&address) {
                return Err(unallocatable_address(
                    name,
                    requested,
                    "provided IP is already allocated",
                ));
            }
            return Ok(ClusterAddress::Allocated(address));
        }
        network
            .lowest_free(&taken)
            .map(ClusterAddress::Allocated)
            .ok_or_else(|| {
                ApiRefusal::internal(
                    "Internal error occurred: failed to allocate a serviceIP: range is full",
                )
            })
    }

    /// The ports the Service is stored with: the ones the manifest declared, each
    /// with the node port the cluster allocated it when the type has one.
    ///
    /// "NodePort: Exposes the Service on each Node's IP at a static port", and a
    /// `LoadBalancer` is a `NodePort` with an external address in front of it — so
    /// both allocate one, out of the API server's node port range, and the other two
    /// types allocate none.
    fn allocated_ports(
        &self,
        declared: Vec<ServicePortObject>,
        service_type: &str,
    ) -> Vec<ServicePortObject> {
        if service_type != SERVICE_TYPE_NODE_PORT && service_type != SERVICE_TYPE_LOAD_BALANCER {
            return declared;
        }
        let mut taken = self.allocated_node_ports();
        declared
            .into_iter()
            .map(|mut port| {
                if let Some(asked) = port.node_port.filter(|asked| !taken.contains(asked)) {
                    taken.insert(asked);
                    return port;
                }
                port.node_port = NODE_PORT_RANGE
                    .clone()
                    .find(|candidate| !taken.contains(candidate));
                if let Some(allocated) = port.node_port {
                    taken.insert(allocated);
                }
                port
            })
            .collect()
    }
}

/// The `spec.type` the manifest names, or the `Status` the API server refuses a
/// value outside the four with.
///
/// A manifest that names none is a `ClusterIP` Service, which is the default the
/// API server fills in.
fn service_type_of(
    name: &str,
    spec: Option<&::serde_json::Value>,
) -> Result<String, ApiRefusal> {
    let named = spec
        .and_then(|spec| spec.get("type"))
        .and_then(::serde_json::Value::as_str)
        .filter(|named| !named.is_empty())
        .unwrap_or(SERVICE_TYPE_CLUSTER_IP);
    if !SERVICE_TYPES.contains(&named) {
        return Err(ApiRefusal::invalid(format!(
            "Service \"{name}\" is invalid: spec.type: Unsupported value: \"{named}\": \
             supported values: {}",
            SERVICE_TYPES
                .iter()
                .map(|supported| format!("\"{supported}\""))
                .collect::<Vec<String>>()
                .join(", ")
        )));
    }
    Ok(named.to_string())
}

/// The `spec.externalName` an `ExternalName` Service carries, and `None` for every
/// other type.
///
/// "externalName is the external reference that discovery mechanisms will return as
/// an alias for this service ... Requires `type` to be ExternalName" — so the member
/// is required on that type and ignored on the others, which is how the API server
/// reads it.
fn external_name_of(
    name: &str,
    spec: Option<&::serde_json::Value>,
    service_type: &str,
) -> Result<Option<String>, ApiRefusal> {
    if service_type != SERVICE_TYPE_EXTERNAL_NAME {
        return Ok(None);
    }
    let named = spec
        .and_then(|spec| spec.get("externalName"))
        .and_then(::serde_json::Value::as_str)
        .filter(|named| !named.is_empty())
        .ok_or_else(|| {
            ApiRefusal::invalid(format!(
                "Service \"{name}\" is invalid: spec.externalName: Required value"
            ))
        })?;
    Ok(Some(named.to_string()))
}

/// The ports the manifest declares, as the Service will hold them, or the `Status`
/// the API server refuses a port list it cannot store with.
///
/// Three rules of the Service registry, in its own wording: a Service of every type
/// but `ExternalName` declares at least one port, a port number is a TCP/UDP port,
/// and "all ports must be named" once a Service declares more than one — a client
/// addresses a multi-port Service by the NAME of the port it wants.
fn declared_ports(
    name: &str,
    spec: Option<&::serde_json::Value>,
    service_type: &str,
) -> Result<Vec<ServicePortObject>, ApiRefusal> {
    let declared = spec
        .and_then(|spec| spec.get("ports"))
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if declared.is_empty() {
        if service_type == SERVICE_TYPE_EXTERNAL_NAME {
            return Ok(Vec::new());
        }
        return Err(ApiRefusal::invalid(format!(
            "Service \"{name}\" is invalid: spec.ports: Required value"
        )));
    }
    // The bound is applied BEFORE anything is read or kept, so a manifest past it
    // stores nothing and is refused in the wording apimachinery words a list past a
    // limit in.
    if declared.len() > MAX_PORTS_PER_SERVICE {
        return Err(ApiRefusal::invalid(format!(
            "Service \"{name}\" is invalid: spec.ports: Too many: {}: must have at most \
             {MAX_PORTS_PER_SERVICE} items",
            declared.len()
        )));
    }
    let mut ports = Vec::with_capacity(declared.len());
    for (index, declared_port) in declared.iter().enumerate() {
        let port = declared_port
            .get("port")
            .and_then(::serde_json::Value::as_i64)
            .filter(|port| PORT_RANGE.contains(port))
            .ok_or_else(|| {
                invalid_member(
                    SERVICE_KIND,
                    name,
                    &format!("spec.ports[{index}].port"),
                    &format!(
                        "must be between {} and {}, inclusive",
                        PORT_RANGE.start(),
                        PORT_RANGE.end()
                    ),
                )
            })?;
        let named = declared_port
            .get("name")
            .and_then(::serde_json::Value::as_str)
            .filter(|named| !named.is_empty());
        if named.is_none() && declared.len() > 1 {
            return Err(ApiRefusal::invalid(format!(
                "Service \"{name}\" is invalid: spec.ports[{index}].name: Required value"
            )));
        }
        let protocol = declared_port
            .get("protocol")
            .and_then(::serde_json::Value::as_str)
            .filter(|protocol| !protocol.is_empty())
            .unwrap_or(DEFAULT_PROTOCOL);
        if !PROTOCOLS.contains(&protocol) {
            return Err(ApiRefusal::invalid(format!(
                "Service \"{name}\" is invalid: spec.ports[{index}].protocol: Unsupported \
                 value: \"{protocol}\": supported values: {}",
                PROTOCOLS
                    .iter()
                    .map(|supported| format!("\"{supported}\""))
                    .collect::<Vec<String>>()
                    .join(", ")
            )));
        }
        ports.push(ServicePortObject {
            name: named.map(str::to_string),
            protocol: protocol.to_string(),
            port,
            // "If this is a string, it will be looked up as a named port in the
            // target Pod's container ports. If this is not specified, the value of
            // the port field is used."
            target_port: declared_port
                .get("targetPort")
                .filter(|target| !target.is_null())
                .cloned()
                .unwrap_or_else(|| ::serde_json::json!(port)),
            node_port: declared_port
                .get("nodePort")
                .and_then(::serde_json::Value::as_i64)
                .filter(|node_port| NODE_PORT_RANGE.contains(node_port)),
        });
    }
    Ok(ports)
}

/// The `Status` the API server refuses a `clusterIP` it cannot allocate with, in
/// the wording the Service registry words a failed allocation in.
fn unallocatable_address(name: &str, requested: &str, why: &str) -> ApiRefusal {
    ApiRefusal::invalid(format!(
        "Service \"{name}\" is invalid: spec.clusterIPs: Invalid value: \
         []string{{\"{requested}\"}}: failed to allocate IP {requested}: {why}"
    ))
}
