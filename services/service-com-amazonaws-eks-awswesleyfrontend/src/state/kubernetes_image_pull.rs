// SPDX-License-Identifier: CC0-1.0

//! The IMAGE PULL a node's kubelet makes before it starts a pod's containers, and
//! what the pod and its events report about it.
//!
//! A container runs an image, and the image is fetched from its registry before the
//! container starts — so what a pod whose image the registry will not serve reports
//! is that pull failure, and a pod whose image it served reports the pull that
//! succeeded. On Amazon EKS the kubelet makes that pull AS THE NODE'S IAM ROLE
//! ("your nodes need permissions to pull images from Amazon ECR" — the node role of
//! the node group the pod was scheduled onto), so an image held in this account's
//! private Amazon ECR registry is decided by THAT service: the policies in force on
//! the role, the repository's own policy, the reference itself and the image the
//! repository holds ([`registry_pull::PullPlane`]). A reference naming a registry
//! this emulator does not hold is nobody's here to answer, so nothing is decided
//! about it and no pull event is recorded for it.
//!
//! The pull happens when the node TAKES the pod, which is the instant the pod's
//! containers are owed their start ([`PodObject::concludes_at`]) — so it is
//! materialised like every other clock-owed step of this service: the pods whose
//! pull is due are read, the registry decides each one, and the verdict is written
//! back with the events the kubelet recorded for it. It is attempted ONCE per pod
//! (a pod that already carries a verdict is not pulled again), and it writes only
//! what was already owed at that instant, so a later caller reads exactly what an
//! earlier one did.
//!
//! What the kubelet does NEXT with an image it was served — create the container and
//! run the program it declares — is [`super::kubernetes_container_run`], which reads
//! the verdict recorded here: a container whose image was refused is never created, so
//! there is no program of its to perform.
//!
//! LOCK ORDER: the verdict is taken with NO store of this service held — the
//! registry takes its own lock and IAM's — which is why this runs before a request
//! to the cluster's API server is routed ([`super::kubernetes_api::serve`]) rather
//! than inside the routing.

use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull::{
    self, PullPrincipal, PullRefusal,
};

use super::kubernetes_events::{
    EventObject, RecordedEvent, EVENT_SOURCE_KUBELET, EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING,
    INVOLVED_KIND_POD,
};
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;
use super::*;

/// The `reason` a kubelet records when it STARTS pulling an image.
pub const PULL_REASON_PULLING: &str = "Pulling";

/// The `reason` a kubelet records when the registry served the image.
pub const PULL_REASON_PULLED: &str = "Pulled";

/// The `reason` a kubelet records for a step of a container that failed — the pull
/// that was refused, and the container start that could not be made.
pub const PULL_REASON_FAILED: &str = "Failed";

/// The `reason` a kubelet records when it waits before pulling again.
pub const PULL_REASON_BACK_OFF: &str = "BackOff";

/// The `status.containerStatuses[].state.waiting.reason` of a container whose image
/// could not be pulled, and the one it carries while the kubelet waits before
/// trying again — the two readings Kubernetes reports for a container waiting on an
/// image.
pub const WAITING_REASON_ERR_IMAGE_PULL: &str = "ErrImagePull";
pub const WAITING_REASON_IMAGE_PULL_BACK_OFF: &str = "ImagePullBackOff";

/// What the node's kubelet made of the images ONE pod's containers run.
///
/// Recorded on the pod when the node took it, so every later read of that pod — its
/// status, its container statuses and its events — reports the same pull.
#[derive(Debug, Clone, Default)]
pub struct ImagePullRecord {
    /// The manifest digest each pulled image resolved to, by container name: the
    /// image's identity in the registry that served it, which is what
    /// `ContainerStatus$imageID` ("ImageID of the container's image") reports.
    /// Empty for a pod none of whose images a registry here answers for.
    pub digests: BTreeMap<String, String>,
    /// The container whose image could not be pulled, with the sentence the kubelet
    /// words that failure in — and `None` for a pod whose images were served.
    pub failure: Option<ImagePullFailure>,
}

/// The pull that STOPPED a pod: which container's image, and what the kubelet said.
#[derive(Debug, Clone)]
pub struct ImagePullFailure {
    /// The container of the pod whose image it was.
    pub container: String,
    /// The image reference the pull was made with.
    pub image: String,
    /// The sentence the kubelet records for it ([`failed_to_pull_message`]), which
    /// is also what the pod's container status carries.
    pub message: String,
}

impl ImagePullRecord {
    /// Whether this pull stopped the pod.
    #[must_use]
    pub fn stopped_the_pod(&self) -> bool {
        self.failure.is_some()
    }
}

/// `Pulling image "<image>"`: what a kubelet records when it begins a pull.
#[must_use]
pub fn pulling_message(image: &str) -> String {
    format!("Pulling image \"{image}\"")
}

/// `Successfully pulled image "<image>"`: what a kubelet records when the registry
/// served the image.
///
/// The fixed sentence alone: a kubelet also reports how long the pull took, and this
/// emulator transfers no bytes over a network, so there is no duration measured here
/// to report.
#[must_use]
pub fn pulled_message(image: &str) -> String {
    format!("Successfully pulled image \"{image}\"")
}

/// `Failed to pull image "<image>": <what the registry answered>`: the sentence a
/// kubelet records for a pull the registry did not serve.
#[must_use]
pub fn failed_to_pull_message(image: &str, answered: &str) -> String {
    format!("Failed to pull image \"{image}\": {answered}")
}

/// `Error: ErrImagePull`: the second sentence a kubelet records for a container
/// whose image it could not pull — the error the container itself is left in.
#[must_use]
pub fn err_image_pull_message() -> String {
    format!("Error: {WAITING_REASON_ERR_IMAGE_PULL}")
}

/// `Back-off pulling image "<image>"`: what a kubelet records while it waits before
/// pulling again.
#[must_use]
pub fn back_off_message(image: &str) -> String {
    format!("Back-off pulling image \"{image}\"")
}

/// The pull of one pod the kubelet of the node it was bound to owes at `now`: what
/// to pull, for whom, and where to record it.
#[derive(Debug)]
struct DuePull {
    /// The Region the cluster runs in, which is the Region of the registry a
    /// reference carrying this emulator's own address names
    /// ([`reference_into_this_registry`]).
    region: String,
    namespace: String,
    pod_name: String,
    pod_uid: String,
    /// The node that took the pod, which is the host its kubelet records events
    /// under.
    node: String,
    /// The IAM role of that node, which the pull is made as; `None` for a node whose
    /// node group states none, which states nothing for a policy to be read against.
    node_role: Option<String>,
    /// The instant the node took the pod, which is when the pull was made.
    at: SystemTime,
    /// The containers of the pod, as `(container name, image)` in the order the spec
    /// declares them — the order the kubelet pulls them in.
    containers: Vec<(String, String)>,
}

/// The registries the kubelet of a node of one cluster pulls a pod's images from,
/// as the stores that answer a pull: this account's private Amazon ECR registry
/// (with the IAM store the node role's documents are in) and the Amazon ECR Public
/// gallery this emulator serves.
///
/// Taken once, when the cluster's API address is opened, and held for as long as
/// that address is: the pull happens when a node TAKES a pod, which is an instant
/// no call of this service is holding the engine at.
#[derive(Clone)]
pub struct PullRegistries {
    /// The private registry's own decision surface ([`registry_pull::PullPlane`]).
    private: registry_pull::PullPlane,
    /// The gallery's store, which answers whether a registry alias is the one it
    /// was assigned; `None` for an engine that carries no gallery, which then has
    /// no registry here for any alias to name.
    gallery: Option<
        ::engine::ServiceState<
            ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::ComAmazonawsEcrpublicSpencerfrontendserviceState,
        >,
    >,
}

/// Written by hand rather than derived: a store handle does not render, and what is
/// diagnosable is whether the registries are there at all.
impl ::std::fmt::Debug for PullRegistries {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PullRegistries")
            .field("private", &self.private)
            .field("gallery", &self.gallery.is_some())
            .finish()
    }
}

impl PullRegistries {
    /// The registries in `engine`.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self {
            private: registry_pull::PullPlane::of(engine),
            gallery: ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::service_state(
                engine,
            ),
        }
    }

    /// Whether the gallery this emulator serves was ASSIGNED `registry_alias`, which
    /// is what makes a reference carrying it one that gallery has an answer for.
    fn serves_gallery_alias(&self, registry_alias: &str) -> bool {
        self.gallery
            .as_ref()
            .is_some_and(|state| state.lock().is_this_registry(registry_alias))
    }

    /// The PROGRAM the image `image` declares as its own, as the registry that serves
    /// that image reports it — and `None` for every reference no registry of this
    /// emulator states a program for.
    ///
    /// It is what a container whose pod declares no `command` runs: "the container
    /// image's ENTRYPOINT is used if this is not provided" (`Container$command`), so
    /// the vector comes from the IMAGE rather than from the manifest. The registry is
    /// what can answer it — an image declares its program in the configuration blob of
    /// its own repository — so each of the two registries this emulator holds answers
    /// for its own images: the private registry off the configuration a push stored
    /// ([`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program`]),
    /// and the public gallery off the published images it holds
    /// ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::published_images`]).
    /// The reference is read exactly as the PULL reads it, so the image whose program
    /// is answered is the image that was pulled.
    #[must_use]
    pub fn image_declared_program(&self, image: &str, region: &str) -> Option<Vec<String>> {
        use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program::ImageProgram;
        use ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
            gallery_pull, published_images,
        };

        let addressed = reference_into_this_registry(self, image, region);
        if let ImageProgram::Declared(program) = self.private.declared_image_program(&addressed) {
            return Some(program);
        }
        gallery_pull::parse_gallery_reference(image)
            .as_ref()
            .and_then(published_images::published_image_program)
    }
}

/// How many of the pulls a request finds DUE it settles before it is answered.
///
/// CHOSEN, not published: the pulls a cluster owes accumulate while nobody calls —
/// a cluster holds up to
/// [`super::kubernetes_objects::MAX_PODS_PER_NAMESPACE`] pods per namespace — and
/// each one asks the registry over the policy documents in force on the node's role,
/// so one request must not drain an arbitrary backlog of them. The rest stay due and
/// are settled by the next request, which is what a kubelet working through its pods
/// does anyway: nothing is lost, and no verdict changes, because each pull is decided
/// from the instant it was owed at.
const MAX_PULLS_PER_REQUEST: usize = 16;

/// Settle every image pull the clock already owes the pods of the cluster keyed
/// `key`, recording each verdict and the events the kubelet recorded for it.
///
/// Called with NO store of this service held (see the module docs), and idempotent:
/// a pod that already carries a verdict is left exactly as it was, so running this
/// again — which every later request does — changes nothing a caller sees.
pub fn settle_due_pulls(
    state: &::engine::ServiceState<ServiceStore>,
    registries: &PullRegistries,
    key: &(String, String),
    now: SystemTime,
) {
    let mut due = due_pulls(state, key, now);
    due.truncate(MAX_PULLS_PER_REQUEST);
    for pull in due {
        let (record, events) = attempt(registries, &pull);
        let mut store = state.lock();
        let Some(plane) = store.kubernetes_api.get_mut(key) else {
            return;
        };
        let Some(pod) = plane
            .objects
            .namespaces
            .get_mut(&pull.namespace)
            .and_then(|namespace| namespace.pods.get_mut(&pull.pod_name))
        else {
            continue;
        };
        // The pod may have been read — and its pull settled — by another request
        // while this one was asking the registry; the verdict already recorded is
        // the one that stands.
        if pod.pull.is_some() || pod.uid != pull.pod_uid {
            continue;
        }
        pod.pull = Some(record);
        for event in events {
            plane.objects.record_event(&pull.namespace, event);
        }
    }
}

/// The pulls `now` is past the due instant of, for the pods of that cluster whose
/// kubelet has not made one yet — at most [`MAX_PULLS_PER_REQUEST`] of them, the
/// rest staying due for the next request.
fn due_pulls(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    now: SystemTime,
) -> Vec<DuePull> {
    let store = state.lock();
    let nodes = store.scheduler_nodes(&key.0, &key.1);
    let Some(plane) = store.kubernetes_api.get(key) else {
        return Vec::new();
    };
    let mut due = Vec::new();
    for (namespace, held) in &plane.objects.namespaces {
        for (pod_name, pod) in &held.pods {
            if due.len() >= MAX_PULLS_PER_REQUEST {
                return due;
            }
            if pod.pull.is_some() {
                continue;
            }
            let Some(at) = pod.concludes_at.filter(|at| now >= *at) else {
                continue;
            };
            let Some(node) = pod.node_name.clone() else {
                continue;
            };
            let node_role = nodes
                .iter()
                .find(|held| held.name == node)
                .and_then(|held| (!held.node_role.is_empty()).then(|| held.node_role.clone()));
            due.push(DuePull {
                region: key.0.clone(),
                namespace: namespace.clone(),
                pod_name: pod_name.clone(),
                pod_uid: pod.uid.clone(),
                node,
                node_role,
                at,
                containers: pod_containers(&pod.spec),
            });
        }
    }
    due
}

/// The `(container name, image)` pairs a pod's admitted spec declares, in its own
/// order. A container the spec declares no image for is not pulled.
fn pod_containers(spec: &::serde_json::Value) -> Vec<(String, String)> {
    spec.get("containers")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|container| {
            let name = container
                .get("name")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let image = container
                .get("image")
                .and_then(::serde_json::Value::as_str)
                .filter(|image| !image.is_empty())?
                .to_string();
            Some((name, image))
        })
        .collect()
}

/// Make `pull`: ask the registry each reference names for the image, in the order
/// the pod declares its containers, and answer the verdict with the events the
/// kubelet recorded.
///
/// The first image a registry here will not serve STOPS the pod — no container of it
/// can be created without the image it runs — so the images after it are not pulled,
/// exactly as a kubelet stops at the pull that failed.
fn attempt(
    registries: &PullRegistries,
    pull: &DuePull,
) -> (ImagePullRecord, Vec<EventObject>) {
    let mut record = ImagePullRecord::default();
    let mut events = Vec::new();
    for (container, image) in &pull.containers {
        let principal = pull.node_role.as_deref().map(PullPrincipal::of_role);
        let addressed = reference_into_this_registry(registries, image, &pull.region);
        // Amazon ECR answers for every account's registry its own reading of a
        // reference names, so WHOSE registry it is comes back from there rather than
        // being decided here; `None` is a reference no registry this emulator holds
        // answers for at all, which nothing is decided about.
        let Some(served) = registries
            .private
            .serve_reference_pull(&addressed, principal, pull.at)
        else {
            continue;
        };
        events.push(kubelet_event(
            pull,
            container,
            PULL_REASON_PULLING,
            pulling_message(image),
            EVENT_TYPE_NORMAL,
        ));
        match served {
            Ok(digest) => {
                if let Some(digest) = digest {
                    record.digests.insert(container.clone(), digest);
                }
                events.push(kubelet_event(
                    pull,
                    container,
                    PULL_REASON_PULLED,
                    pulled_message(image),
                    EVENT_TYPE_NORMAL,
                ));
            }
            Err(refusal) => {
                let answered = match refusal {
                    PullRefusal::NotAuthorized { message, .. }
                    | PullRefusal::NotFound { message } => message,
                };
                let message = failed_to_pull_message(image, &answered);
                events.push(kubelet_event(
                    pull,
                    container,
                    PULL_REASON_FAILED,
                    message.clone(),
                    EVENT_TYPE_WARNING,
                ));
                events.push(kubelet_event(
                    pull,
                    container,
                    PULL_REASON_FAILED,
                    err_image_pull_message(),
                    EVENT_TYPE_WARNING,
                ));
                events.push(kubelet_event(
                    pull,
                    container,
                    PULL_REASON_BACK_OFF,
                    back_off_message(image),
                    EVENT_TYPE_NORMAL,
                ));
                record.failure = Some(ImagePullFailure {
                    container: container.clone(),
                    image: image.clone(),
                    message,
                });
                break;
            }
        }
    }
    (record, events)
}

/// The reference this registry reads, for an image the client was handed by Amazon
/// ECR ITSELF: `Repository$repositoryUri` is "the URI for the repository. You can
/// use this URI for container image push and pull operations", and on this machine
/// that URI is built on an ADDRESS OF THIS EMULATOR, because AWS's own registry host
/// resolves nowhere here. A pod written from what that service handed the client
/// therefore carries that address, so it is rewritten into the published
/// `<account>.dkr.ecr.<region>.<suffix>` spelling and resolved exactly like a
/// reference that carries it already — rather than read a second time with a reading
/// of this service's own.
///
/// A host that ALREADY names an Amazon ECR registry in that published spelling is
/// left exactly as it is, whichever account it names: such a reference is one that
/// registry reads itself, and rewriting it would move a reference into another
/// account's registry into this account's. The one address serves this emulator's
/// PUBLIC gallery too, so a reference that gallery answers for is left as it is as
/// well: rewriting it would turn a pull from the gallery into a pull of the
/// same-named path from the private registry.
///
/// One address serves every Region's registry, so such a reference names no Region of
/// its own: the Region is the CLUSTER's, which is the Region its node's kubelet pulls
/// in. The host is composed by Amazon ECR's own composer of its published spelling,
/// the inverse of the walk that registry resolves the rewritten reference by.
fn reference_into_this_registry<'image>(
    registries: &PullRegistries,
    image: &'image str,
    region: &str,
) -> ::std::borrow::Cow<'image, str> {
    use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state as ecr_store;

    let Some((authority, path)) = image.split_once('/') else {
        return ::std::borrow::Cow::Borrowed(image);
    };
    if registry_pull::parse_registry_reference(image).is_some()
        || !addresses_this_emulators_registry(authority)
        || addresses_this_emulators_gallery(registries, image)
    {
        return ::std::borrow::Cow::Borrowed(image);
    }
    ::std::borrow::Cow::Owned(format!(
        "{}/{path}",
        ecr_store::registry_host(ecr_store::registry_id(), region)
    ))
}

/// Whether `host` is an address of THIS EMULATOR rather than the name of a registry
/// published on the internet — which makes it the endpoint Amazon ECR announces its
/// own registry at here.
///
/// Read through the tree's one reading of the question
/// (`service_common::data_plane_addressing`): a host-shaped value that names no
/// service is an address of this emulator, whichever interface and port it is served
/// on. The host of the base this emulator reports while it is served on NO port at
/// all is such an address as well, since its first label would otherwise read as a
/// service's.
fn addresses_this_emulators_registry(host: &str) -> bool {
    if !::service_common::data_plane_addressing::is_host_shaped(host) {
        return false;
    }
    let unserved = ::service_common::data_plane_addressing::url_authority(
        ::smithy_server_runtime::UNSERVED_URL_BASE,
    );
    host.eq_ignore_ascii_case(unserved)
        || !::service_common::data_plane_addressing::names_a_service(host)
}

/// Whether this emulator's PUBLIC gallery is the registry `image` addresses: a
/// reference carrying the registry alias that gallery was ASSIGNED, at the host AWS
/// publishes for it or at an address this emulator answers on.
///
/// The gallery's own readings answer it — its reference form and whether the alias is
/// the one it was assigned — so this service writes no rule of its own about either
/// registry's reference layout.
fn addresses_this_emulators_gallery(registries: &PullRegistries, image: &str) -> bool {
    use ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::gallery_pull;

    gallery_pull::parse_gallery_reference(image)
        .is_some_and(|reference| registries.serves_gallery_alias(&reference.registry_alias))
}

/// One event the node's kubelet recorded about a container of the pod `pull` is for.
fn kubelet_event(
    pull: &DuePull,
    container: &str,
    reason: &str,
    message: String,
    event_type: &str,
) -> EventObject {
    EventObject::of(RecordedEvent {
        involved_kind: INVOLVED_KIND_POD,
        involved_name: &pull.pod_name,
        involved_uid: &pull.pod_uid,
        // "If referring to a piece of an object instead of an entire object": the
        // container the pull was for, in the form a kubelet names one.
        field_path: Some(format!("spec.containers{{{container}}}")),
        reason,
        message,
        event_type,
        component: EVENT_SOURCE_KUBELET,
        host: Some(pull.node.clone()),
        at: pull.at,
    })
}
