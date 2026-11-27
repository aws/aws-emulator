// SPDX-License-Identifier: CC0-1.0

//! Whether a launched task can FETCH the images its containers run, and the stop a task
//! that cannot earns.
//!
//! A container runs an image, and the image is fetched from its registry before the
//! container starts — so a task whose image cannot be fetched never runs at all.
//! `Task$stopCode` is "the stop code indicating why a task was stopped", and the
//! `TaskStopCode` member for a task that failed before its containers started is
//! [`STOP_CODE_TASK_FAILED_TO_START`]; `Task$stoppedReason` ("The reason that the task was
//! stopped") carries the fetch failure itself, which the container runtime words as a
//! `CannotPullContainerError` ([`cannot_pull_container_message`] and its siblings).
//!
//! Two things have to hold for the fetch, and each is answered by the service that owns the
//! facts:
//!
//! 1. **a path to the registry**, which is Amazon VPC's and is read off the placement's own
//!    subnet:
//!
//!    * a subnet whose route table sends `0.0.0.0/0` to a PUBLIC NAT gateway reaches the
//!      internet — "with a public NAT gateway, internet-bound traffic from a private subnet
//!      can be routed to the NAT gateway" (`CreateNatGateway`), which a PRIVATE one does
//!      not do;
//!    * a subnet whose route table sends `0.0.0.0/0` to an internet gateway reaches it only
//!      for a task that was given a public address — "to use an internet gateway, your
//!      subnet's route table must contain a route that directs internet-bound traffic to
//!      the internet gateway" (Amazon VPC User Guide) and the instance must have a public
//!      address, which for an `awsvpc` task is `AwsVpcConfiguration$assignPublicIp`
//!      ([`ASSIGN_PUBLIC_IP_ENABLED`]);
//!    * a subnet with no route off the VPC at all can still reach AMAZON ECR, through the
//!      endpoints that service publishes: an interface endpoint for `ecr.api`, one for
//!      `ecr.dkr` and a gateway endpoint for Amazon S3, which the image layers come from
//!      (Amazon ECR User Guide, "Amazon ECR interface VPC endpoints"). A public registry is
//!      not an AWS service and publishes no endpoint, so an image held in one needs the
//!      internet.
//!
//! 2. **an image the registry it names serves**, for an image reference that names a
//!    registry this emulator HOLDS, which is either of two:
//!
//!    * a repository of an AMAZON ECR private registry, which Amazon ECS pulls
//!      from as the task's EXECUTION ROLE — "the task execution role that grants the Amazon
//!      ECS container agent permission to make Amazon Web Services API calls on your behalf"
//!      (`TaskDefinition$executionRoleArn`) — so Amazon ECR decides the pull: whose registry
//!      the reference addresses, the policies in force on that role and the repository's own
//!      policy, the reference itself, and the upstream image it caches when a pull through
//!      cache rule covers the repository
//!      ([`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull`]).
//!      A registry belongs to one account, which shares its contents only through its own
//!      resource-based policy, so a reference into ANOTHER account's Amazon ECR registry is a
//!      cross-account pull nothing here serves and the task does not start;
//!    * a repository under the registry ALIAS of the one public gallery this emulator
//!      serves, which resolves the reference against the repositories and tags it holds
//!      ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::gallery_pull`]),
//!      or a repository that gallery publishes ITSELF whose image this emulator holds, which
//!      the releases it publishes decide
//!      ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::published_images`]);
//!    * the repository one of those held images is MIRRORED FROM, which a reference names
//!      with or without that registry's own namespace and which a container runtime resolves
//!      a reference naming no registry at all against ([`DOCKER_HUB_REGISTRY`]) — the same
//!      image, so the same releases decide it
//!      ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::published_images::official_image`]):
//!      a tag that names a release of it is fetched, and a tag it publishes no release under
//!      resolves to no image whichever of the two spellings the reference carries;
//!    * the NAMESPACE that mirror carries those images from, whose repository list is itself
//!      the published catalogue of them, so a reference into it naming a repository the
//!      catalogue does not list names no repository at all and the registry answers the
//!      request with no pull scope for it
//!      ([`repository_does_not_exist_message`]) — while a repository the catalogue DOES list
//!      whose releases this emulator holds none of is left undecided, as a repository nothing
//!      here holds a store for is.
//!
//!    Either way the registry that holds the image is the one that answers, and the manifest
//!    digest it resolved is what the container reports for the image it ran
//!    (`Container$imageDigest`). A reference naming any OTHER registry — an alias that
//!    gallery was not assigned, which names another public registry, another registry
//!    altogether, or another namespace of the mirrored registry — is no registry's here to
//!    answer, so nothing beyond the path above is decided for it and no digest is known for
//!    it.
//!
//! The pull is attempted at LAUNCH, by the call that places the task: the instants it took
//! are recorded on the task (`Task$pullStartedAt`/`pullStoppedAt`) and a failure with them
//! ([`TaskRecord::launch_failure`]); the clock then stops the task when the attempt that
//! failed it ENDED ([`TaskRecord::fail_launch`]), so a caller reading in between still sees
//! the task come up. A failure the fetch was ANSWERED — a registry's refusal, a role, a
//! secret — ended the moment it was asked for, so that stop falls due where the task's own
//! start would have been; a placement with no path to the registry answers nothing, so the
//! attempt ends in the connection timeout the `stoppedReason` reports
//! ([`FAILED_IMAGE_FETCH_TIMEOUT`]) and the task is coming up, with the interface it was
//! allocated attached, until it does. It is attempted only for a task whose launch roles have
//! already been put to IAM ([`TaskRecord::launch_roles_assumed`]), because the pull is made
//! AS the execution role. The attempt is made once per task: a task that already
//! carries its pull instants is not pulled again, so a later placement by another call
//! re-decides nothing and imports nothing twice.
//!
//! The pass that attempts it settles the OTHER execution resource the agent retrieves in
//! that same step, as that same role: the `secrets` the revision references
//! ([`super::container_secrets`]). The agent's own account of a failure here —
//! "unable to pull secrets or registry auth: execution resource retrieval failed" — names
//! them together because it is one step, so one verdict is recorded for the task and either
//! half can be what fails its launch.
//!
//! The REGISTRY AUTHENTICATION that account names beside them belongs to that step too, and
//! comes before every fetch: the agent obtains ONE authorization token for the Amazon ECR
//! registry as the execution role, granted on the registry rather than on any repository, so
//! it is one question for the task ([`withheld_registry_auth`]) and a role refused it has
//! reached no image at all — such a task stops with the agent's account of the execution
//! resource it could not retrieve ([`unable_to_retrieve_registry_auth_message`]), reported on
//! the task alone, rather than with the runtime's account of a fetch that was never made.
//!
//! LOCK ORDER: the cross-service reads here take the Amazon EC2 store's lock and then the
//! Amazon ECR store's (which takes the IAM store's), each holding no other while it does,
//! and hand back owned values — so they are called with NO guard of this service's store
//! held, which is what keeps a lock cycle between the stores unreachable. The same contract
//! [`unattached_target_groups`] has.

use super::*;

use ::service_com_amazonaws_ec2_amazonec2::ops::types::{Route, RouteTable};
use ::service_com_amazonaws_ec2_amazonec2::state::{
    CONNECTIVITY_TYPE_PUBLIC, NAT_GATEWAY_STATE_AVAILABLE, NAT_GATEWAY_STATE_PENDING,
    ROUTE_STATE_BLACKHOLE, VPC_ENDPOINT_STATE_AVAILABLE, VPC_ENDPOINT_STATE_PENDING,
    VPC_ENDPOINT_TYPE_GATEWAY, VPC_ENDPOINT_TYPE_INTERFACE,
};
use ::service_com_amazonaws_ec2_amazonec2::ComAmazonawsEc2Amazonec2State;

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all: the
/// `TaskStopCode` enum's `TaskFailedToStart` member
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
pub const STOP_CODE_TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The `stoppedReason` of a task whose container Amazon ECS could not CREATE because nothing
/// names a program for it to run.
///
/// Not a fetch failure: the image was fetched, and what the container runtime then could not
/// do is START a container out of it. A container runs an argument vector, which is its entry
/// point followed by its command (`ContainerDefinition$entryPoint`, and
/// `ContainerDefinition$command` — "the command that's passed to the container", which
/// "overrides the default command from the Docker image"), so a container whose definition
/// names neither and whose image declares neither
/// ([`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program`])
/// has no process for the runtime to start: the daemon refuses the create with the sentence
/// below, under the `CannotStartContainerError` class the ECS Developer Guide's "Stopped tasks
/// error codes" lists such a stop under. The text has no variable part — the daemon names
/// neither the image nor the container in it.
pub const CANNOT_START_CONTAINER_NO_COMMAND_REASON: &str =
    "CannotStartContainerError: Error response from daemon: No command specified";

/// How long the agent spends on an image fetch that cannot REACH the registry before it
/// gives the attempt up — the wait the `stoppedReason` of such a task reports as having
/// timed out ([`cannot_pull_container_message`]: "dial tcp ...: i/o timeout").
///
/// EMULATOR-CHOSEN — AWS publishes no timeout for the fetch — but what it stands for is not:
/// a placement with no path to the registry answers NOTHING, so the attempt ends in a
/// connection timeout, and Amazon ECS has nothing to report about the task until the agent
/// has spent it. A task is therefore still coming up for this long after its interface was
/// attached, which is what a client that reads the attachment of a task whose image cannot
/// be fetched — and joins it to Amazon EC2's record of that interface, which exists only
/// while the task does — observes.
///
/// What the value has to be is OBSERVABLE, which is why it is not one lifecycle span: the
/// wait IS what a client does with such a task. The client places it, reads it back, reacts
/// to the statuses it publishes and acts on what it finds — stops it, reads its attachment,
/// waits on the event bus for its next move — and every one of those steps is a round trip
/// of that client's own, so a task that had reached its terminal status before the first of
/// them came back would report a coming-up lifecycle nothing could ever act on, and every
/// stop asked for during it would arrive at a task that had already stopped. This span is
/// many client round trips wide and still far below the minutes a real connect timeout and
/// the retries around it take, so the attempt is one a caller can both observe and interrupt
/// while the emulator stays quick.
pub const FAILED_IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The destination of the route that carries traffic OFF a VPC: "a route table contains a
/// set of rules, called routes" and the one for everything not inside the VPC is the
/// default route (Amazon VPC User Guide, "Route tables").
pub const DEFAULT_IPV4_DESTINATION: &str = "0.0.0.0/0";

/// The id prefix of an internet gateway, which EC2 spells `igw-<hex>` — what a route's
/// `GatewayId` carries when the route points at one.
pub const INTERNET_GATEWAY_ID_PREFIX: &str = "igw-";

/// The registry a reference that names none is pulled from: the Docker Hub registry a
/// container runtime resolves a bare name (`nginx:latest`) against.
pub const DOCKER_HUB_REGISTRY: &str = "registry-1.docker.io";

/// The endpoint service suffix of the Amazon ECR API, as an endpoint's `ServiceName`
/// spells it (`com.amazonaws.<region>.ecr.api`).
const ECR_API_ENDPOINT_SUFFIX: &str = ".ecr.api";

/// The endpoint service suffix of the Amazon ECR Docker registry
/// (`com.amazonaws.<region>.ecr.dkr`).
const ECR_DKR_ENDPOINT_SUFFIX: &str = ".ecr.dkr";

/// The endpoint service suffix of Amazon S3, which an ECR image's layers are fetched from
/// (`com.amazonaws.<region>.s3`).
const S3_ENDPOINT_SUFFIX: &str = ".s3";

/// What ONE launched task's image fetch depends on: where the launch placed it, what its
/// containers run, and the principal Amazon ECS pulls as.
///
/// Collected from the stored tasks under this service's lock
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::launches_awaiting_image_pull`])
/// and answered against the Amazon EC2 and Amazon ECR stores under those stores' own, so no
/// two locks are ever held at once.
#[derive(Debug, Clone)]
pub struct TaskLaunch {
    /// The task the verdict belongs to.
    pub task_arn: String,
    /// `AwsVpcConfiguration$subnets`: "The IDs of the subnets associated with the task or
    /// service", in the order the launch named them. Empty for a task that shares the
    /// networking of the host that runs it, which carries no placement of its own.
    pub subnets: Vec<String>,
    /// `AwsVpcConfiguration$assignPublicIp`: "Whether the task's elastic network interface
    /// receives a public IP address", and `None` when the launch named no value.
    pub assign_public_ip: Option<String>,
    /// The images the task's containers run, as `Container$image` holds them.
    pub images: Vec<String>,
    /// The task execution role Amazon ECS makes the pull as
    /// ([`TaskRecord::execution_role_arn`]), and `None` for a task whose revision and
    /// overrides name none.
    pub execution_role_arn: Option<String>,
    /// The values the agent fetches for the task's containers before any of them starts —
    /// the `secrets` its revision references ([`super::container_secrets`]) — which are
    /// retrieved as that same execution role, in the same step as the registry
    /// authentication this fetch takes.
    pub secrets: Vec<ContainerSecret>,
    /// Where the `awslogs` driver of each of the task's containers delivers that
    /// container's output ([`super::log_driver_init`]), which the agent initialises as that
    /// same execution role before the container it is for runs.
    pub log_targets: Vec<AwsLogsTarget>,
    /// The images of the task's containers that nothing NAMES a program for: neither the
    /// container's own definition, nor the launch's overrides, nor the catalog of images whose
    /// program this emulator knows ([`super::container_runtime::container_program`]).
    ///
    /// What such a container runs is whatever its IMAGE declares, which is the registry's to
    /// answer ([`unstartable_container_image`]): an image that declares no program either
    /// leaves the container nothing to start.
    pub images_naming_no_program: Vec<String>,
}

/// The `stoppedReason` of a task whose containers could not be launched because an image
/// could not be fetched from where the task is placed.
///
/// The error CLASS is the container runtime's `CannotPullContainerError`, which is what
/// Amazon ECS surfaces for a failed fetch; the variable parts are the image the request
/// asked for and the registry that reference resolves to ([`registry_host`]), with the
/// network condition a placement that cannot reach it produces.
pub fn cannot_pull_container_message(image: &str) -> String {
    let host = registry_host(image);
    format!(
        "CannotPullContainerError: failed to resolve ref {image}: failed to do request: \
         Get \"https://{host}/v2/\": dial tcp {host}:443: i/o timeout"
    )
}

/// The `stoppedReason` of a task whose execution role may not obtain the AUTHORIZATION
/// TOKEN for the Amazon ECR registry its images come from.
///
/// That token is not an image: it is one of the EXECUTION RESOURCES the agent retrieves for
/// the task — as the execution role, in the same step as the revision's `secrets` and before
/// any manifest is fetched — so a role refused it fails the task in that step and the stop
/// carries the agent's `ResourceInitializationError` for the resource it could not retrieve
/// ([`unable_to_retrieve_resource_message`], the one wording of that stop), naming the
/// registry authentication rather than any image. `refusal` is Amazon ECR's own answer,
/// under the class that service refuses an unauthorized call with
/// ([`ACCESS_DENIED_CLASS`]).
pub fn unable_to_retrieve_registry_auth_message(refusal: &str) -> String {
    unable_to_retrieve_resource_message(
        ECR_REGISTRY_AUTH_RESOURCE,
        &format!("{ACCESS_DENIED_CLASS}: {refusal}"),
    )
}

/// What the agent calls the execution resource it retrieves from Amazon ECR: the
/// authorization token for the registry (`ecr`), obtained once for the task whatever its
/// containers pull from it.
const ECR_REGISTRY_AUTH_RESOURCE: &str = "ecr registry auth";

/// The `stoppedReason` of a task whose image the registry REFUSED to serve to its execution
/// role.
///
/// The same `CannotPullContainerError` class, worded for what the runtime saw: the registry
/// answers a manifest request no authorization covers with `403 Forbidden`
/// ([`FORBIDDEN_STATUS`]), and the agent reports the request it made and the status it got
/// back, having retried the fetch to the end of its budget ([`MANIFEST_FETCH_RETRIES`]) as it
/// does for every manifest it cannot resolve. The authorization decision itself is the
/// registry's own and is not on this wire: a `docker pull` is answered with an HTTP status,
/// not with an IAM sentence.
///
/// `manifest_url` is the request the registry refused ([`manifest_url`]).
pub fn pull_access_denied_message(image: &str, manifest_url: &str) -> String {
    format!(
        "CannotPullContainerError: pull image manifest has been retried \
         {MANIFEST_FETCH_RETRIES} time(s): failed to resolve ref {image}: unexpected status \
         from HEAD request to {manifest_url}: {FORBIDDEN_STATUS}"
    )
}

/// The HTTP status a registry answers a request the credential presented does not cover
/// with: `403 Forbidden` (RFC 9110, "the server understood the request but refuses to
/// authorize it"), which is what the Docker Registry HTTP API V2 answers such a manifest
/// request and therefore what the runtime reports for it.
const FORBIDDEN_STATUS: &str = "403 Forbidden";

/// The URL a container runtime fetches the MANIFEST of `image` at, for a reference that
/// addresses `reference` of an Amazon ECR registry: the registry the reference was written
/// against ([`registry_host`]) and the Docker Registry HTTP API V2 manifest route of the
/// repository it names (`GET /v2/<name>/manifests/<reference>`, where the reference is the
/// tag or the digest asked for).
///
/// The repository and the tag or digest are the registry's OWN reading of the reference
/// ([`registry_pull::parse_registry_reference`]), so the request named here is the one that
/// pull makes rather than a second reading of its own.
fn manifest_url(
    image: &str,
    reference: &::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull::RegistryReference,
) -> String {
    let asked_for = reference
        .digest
        .as_deref()
        .or(reference.tag.as_deref())
        .unwrap_or_default();
    format!(
        "https://{}/v2/{}/manifests/{asked_for}",
        registry_host(image),
        reference.repository_name
    )
}

/// How many times the container agent retried the image MANIFEST fetch before it reported
/// the failure, which is the count its account of a reference that resolves to nothing
/// carries ([`image_not_found_message`]).
///
/// A reference the registry ANSWERS for — it holds no such repository, and no such tag or
/// digest in one it does hold — resolves to nothing however often it is asked, so the fetch
/// is retried until the agent's whole retry budget is spent and that budget is what the
/// failure reports. The number is the agent's own, not a quota of the API: between two such
/// failures what differs is the reference, not the count.
const MANIFEST_FETCH_RETRIES: u32 = 7;

/// The `stoppedReason` of a task whose image the registry holds nothing for: the same
/// class, with the condition the container runtime words for a reference that resolves to
/// no content.
///
/// The image's MANIFEST is what is fetched first, and the agent retries that fetch before it
/// gives up ([`MANIFEST_FETCH_RETRIES`]); the runtime then names the reference it could not
/// resolve twice — once as the reference the fetch asked for, and once as the answer the
/// registry gave about it, which for a reference that resolves to nothing is that same
/// reference and `not found`.
pub fn image_not_found_message(image: &str) -> String {
    format!(
        "CannotPullContainerError: pull image manifest has been retried \
         {MANIFEST_FETCH_RETRIES} time(s): failed to resolve ref {image}: {image}: not found"
    )
}

/// The `stoppedReason` of a task whose image reference names a repository the registry does
/// not PUBLISH at all.
///
/// The same `CannotPullContainerError` class, worded for what the runtime saw: a registry
/// grants the pull scope of a repository it holds, so a request for one it holds none of is
/// answered with no such scope rather than with a missing manifest — the runtime reports that
/// the access was denied and that the repository may not be there, having retried the fetch to
/// the end of its budget ([`MANIFEST_FETCH_RETRIES`]) as it does for every manifest it cannot
/// resolve.
pub fn repository_does_not_exist_message(image: &str) -> String {
    format!(
        "CannotPullContainerError: pull image manifest has been retried \
         {MANIFEST_FETCH_RETRIES} time(s): failed to resolve ref {image}: pull access denied, \
         repository does not exist or may require authorization: server message: \
         insufficient_scope: authorization failed"
    )
}

/// The REGISTRY an image reference is fetched from: the part before the first `/` when
/// that part is a host — it carries a `.` or a `:`, or is `localhost` — and
/// [`DOCKER_HUB_REGISTRY`] otherwise, which is how a container runtime resolves a
/// reference that names no registry.
pub fn registry_host(image: &str) -> &str {
    match image.split_once('/') {
        Some((host, _))
            if host.contains('.') || host.contains(':') || host == "localhost" =>
        {
            host
        }
        _ => DOCKER_HUB_REGISTRY,
    }
}

/// The hosts that address the registry the gallery's mirrored images are carried FROM:
/// [`DOCKER_HUB_REGISTRY`] itself, the name a reference usually spells it by, and the index
/// host — all one registry, which is also the registry a reference naming no host at all is
/// resolved against.
const MIRRORED_REGISTRY_HOSTS: [&str; 3] = ["docker.io", "index.docker.io", DOCKER_HUB_REGISTRY];

/// The tag a reference that names none asks for: the moving tag a container runtime completes
/// a reference with, which is the one both registries this emulator serves read such a
/// reference as naming
/// ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::gallery_pull::DEFAULT_IMAGE_TAG`]).
const DEFAULT_IMAGE_TAG: &str =
    ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::gallery_pull::DEFAULT_IMAGE_TAG;

/// The `(repository path, tag)` of `image` for a reference into that mirrored registry, and
/// `None` for a reference naming any other registry.
///
/// The path is what follows the host, which the registry reads as its own namespaces, so it
/// is handed on exactly as written for that registry's catalogue to decide
/// ([`::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::published_images::official_image`]).
/// A reference that names NO tag names the moving one a container runtime completes it with
/// ([`DEFAULT_IMAGE_TAG`]), the same reading the registries this emulator serves take of their
/// own references. A reference that addresses a DIGEST names content rather than a release, so
/// the releases a repository publishes state nothing about it and it is not answered here.
fn mirrored_image_reference(image: &str) -> Option<(String, String)> {
    let path = match image.split_once('/') {
        Some((host, rest)) if MIRRORED_REGISTRY_HOSTS.contains(&host) => rest,
        // A reference whose first segment is not one of those hosts is this registry's only
        // when it names no host at all ([`registry_host`]).
        _ if registry_host(image) == DOCKER_HUB_REGISTRY => image,
        _ => return None,
    };
    if path.contains('@') {
        return None;
    }
    let (repository_path, image_tag) = path
        .rsplit_once(':')
        .unwrap_or((path, DEFAULT_IMAGE_TAG));
    Some((repository_path.to_string(), image_tag.to_string()))
}

/// Attempt the image fetch of every task that has just been launched, and record what it
/// took and why it failed.
///
/// Called by the operations that LAUNCH a task, and by the clock pump for a task the CLOCK
/// placed — the scheduler's replacement for a task a service lost
/// ([`super::service_steady_state`]), which no call is there to attempt the pull for. Either
/// way with this service's guard dropped: the verdict is a question about the Amazon VPC
/// networking the launch named and about what Amazon ECR serves the task's execution role, so
/// each half is read out of that service's own store through the one shared engine, under that
/// store's lock and no other.
///
/// Nothing is recorded for a fact this emulator does not hold — a subnet the Amazon EC2
/// store does not hold is not evidence of a blocked path, and an engine carrying no Amazon
/// ECR store says nothing about an image — and nothing is attempted twice: a task that
/// already carries its pull instants is not pulled again.
pub fn evaluate_image_pulls<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let launches = state.lock().launches_awaiting_image_pull();
    if launches.is_empty() {
        return;
    }
    // The network half, under the Amazon EC2 store's lock alone: every launch is answered
    // in one pass, so the lock is taken once however many tasks were placed.
    let mut failures: Vec<ImagePullVerdict> = match ::service_com_amazonaws_ec2_amazonec2::state::service_state(engine) {
        Some(ec2) => {
            let store = ec2.lock();
            launches
                .iter()
                .map(|launch| {
                    ImagePullVerdict::of_unreachable_registry(
                        launch,
                        unfetchable_image(&store, launch),
                    )
                })
                .collect()
        }
        None => launches
            .iter()
            .map(|launch| ImagePullVerdict::of(launch, None))
            .collect(),
    };
    // The registry half, with no lock of this service's or Amazon EC2's held: the registry
    // answers each pull itself, which is also what imports an upstream image through a pull
    // through cache rule, and states the digest of the manifest it resolved.
    let now = SystemTime::now();
    for (launch, verdict) in launches.iter().zip(failures.iter_mut()) {
        if verdict.failure.is_none() {
            // The registry AUTHENTICATION the agent retrieves for the task comes first: one
            // token for the registry, obtained as the execution role before any manifest is
            // fetched, so it is one question for the task and a role refused it has reached
            // no image ([`withheld_registry_auth`]). Reported on the task alone, like every
            // other execution resource the agent retrieves for it.
            if let Some(reason) = withheld_registry_auth(engine, launch) {
                verdict.failure = Some(reason);
                continue;
            }
            let fetched = fetch_launch_images(engine, launch, now);
            if let Some((image, reason)) = fetched.refused {
                verdict.failed_image = Some(image);
                verdict.failure = Some(reason);
            }
            verdict.image_digests = fetched.digests;
            verdict.cached_image_sources = fetched.cached_sources;
        }
    }
    // The SECRETS the agent fetches for the task's containers, in the same step and as the
    // same role: "unable to pull secrets or registry auth" is one resource-retrieval step of
    // the agent's, so a task that cannot retrieve one of its secrets fails to start exactly
    // as one that cannot fetch an image does ([`super::container_secrets`]). Decided after
    // the images, with no lock held: the verdict is the owning service's.
    for (launch, failure) in launches.iter().zip(failures.iter_mut()) {
        if failure.failure.is_none() {
            failure.failure = unretrievable_secret(engine, launch);
        }
    }
    // The LOGGER each container's `awslogs` configuration describes, which the agent
    // initialises as that same role before it starts the container: a task whose logger
    // cannot be validated fails to start exactly as one whose execution resources could not
    // be retrieved does ([`super::log_driver_init`]). Decided after them, because the agent
    // retrieves the task's resources and then creates each container, and with no lock held:
    // the verdict is the owning service's.
    for (launch, failure) in launches.iter().zip(failures.iter_mut()) {
        if failure.failure.is_none() {
            failure.failure = uninitialisable_logger(engine, launch);
        }
    }
    // And the CONTAINERS themselves, which the runtime creates once the task's resources and
    // its images are in hand: a container nothing names a program for has no process to start,
    // so the create is refused and the task fails to start
    // ([`CANNOT_START_CONTAINER_NO_COMMAND_REASON`]). Decided last, because creating the
    // containers is the last step of a launch, and with no lock held: what an image declares
    // is the registry's own answer.
    for (launch, verdict) in launches.iter().zip(failures.iter_mut()) {
        if verdict.failure.is_none() {
            if let Some(image) = unstartable_container_image(engine, launch) {
                verdict.failed_image = Some(image);
                verdict.failure = Some(CANNOT_START_CONTAINER_NO_COMMAND_REASON.to_string());
            }
        }
    }
    state.lock().record_image_pulls(&failures, now);
}

/// The image of the FIRST container of `launch` that has NO program to run at all — nothing
/// named one for it and the image it runs declares none — and `None` when every container of
/// the task has a program, or when nothing states what one of their images declares.
///
/// The images asked about are the ones whose container's launch named no program
/// ([`TaskLaunch::images_naming_no_program`]); what is left is what the IMAGE declares, which
/// the registry that holds it answers
/// ([`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program`]):
/// an image whose configuration states neither an entry point nor a command carries no program
/// of its own. A registry this emulator does not hold the image of states nothing about it, so
/// such a container is left exactly as it was — its program is the image's own, which this
/// emulator starts no image to run and therefore reports no outcome for
/// ([`super::container_shell::NO_DECLARED_PROGRAM_DETAIL`]).
///
/// Called with no guard of this service's store held: the answer is the registry's, taken
/// under that store's lock alone.
fn unstartable_container_image<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: &TaskLaunch,
) -> Option<String> {
    use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program::{
        declared_image_program, ImageProgram,
    };

    launch
        .images_naming_no_program
        .iter()
        .find(|image| {
            let addressed = reference_into_this_registry(engine, image, &launch.task_arn);
            declared_image_program(engine, &addressed) == ImageProgram::Undeclared
        })
        .cloned()
}

/// What the image fetch of ONE launched task answered: why its containers could not be
/// fetched, and the manifest digest of each image that WAS fetched.
#[derive(Debug, Clone)]
pub struct ImagePullVerdict {
    /// The task the verdict belongs to.
    pub task_arn: String,
    /// The `stoppedReason` of the stop a task whose containers could not be fetched earns,
    /// and `None` when nothing was decided against the fetch.
    pub failure: Option<String>,
    /// The image reference whose fetch is what failed, when the failure is ONE image's: the
    /// container that runs that image is the container the failure is reported on
    /// ([`TaskRecord::launch_failure_container`]). `None` for a verdict that failed on
    /// something other than an image, and for one that failed on nothing.
    pub failed_image: Option<String>,
    /// `(image reference, manifest digest)` for each of the task's images a registry this
    /// emulator holds served: `Container$imageDigest` is "The container image manifest
    /// digest", and the registry that served the pull is what states it.
    pub image_digests: Vec<(String, String)>,
    /// `(image reference, upstream reference)` for each of the task's images whose repository
    /// holds a CACHED COPY of an upstream one: such a container runs the upstream image, so
    /// that is the reference what runs in it is read off
    /// ([`ContainerRecord::running_image`]). Empty for a task every image of which names a
    /// repository that holds an image of its own.
    pub cached_image_sources: Vec<(String, String)>,
    /// Whether the failure is one the fetch WAITED for rather than one it was answered: a
    /// placement with no path to the registry answers nothing, so the attempt ends in a
    /// connection timeout ([`FAILED_IMAGE_FETCH_TIMEOUT`]), while a registry that refused the
    /// pull, a role the launch may not assume and a secret the agent could not retrieve are
    /// each known the moment they are asked for.
    pub fetch_timed_out: bool,
}

impl ImagePullVerdict {
    /// The verdict of `launch` with `failure` — the image whose fetch failed and the reason
    /// it did — recorded, and no image fetched yet.
    ///
    /// Nothing is waited out here: every failure but the PATH half's is one something
    /// ANSWERED the moment it was asked for ([`Self::of_unreachable_registry`] is the one
    /// that is not).
    fn of(launch: &TaskLaunch, failure: Option<(String, String)>) -> Self {
        let (failed_image, failure) = match failure {
            Some((image, reason)) => (Some(image), Some(reason)),
            None => (None, None),
        };
        Self {
            task_arn: launch.task_arn.clone(),
            fetch_timed_out: false,
            failure,
            failed_image,
            image_digests: Vec::new(),
            cached_image_sources: Vec::new(),
        }
    }

    /// The same verdict for the PATH half of the fetch ([`unfetchable_image`]), the one
    /// failure of the four that nothing answers: an image the placement cannot reach is one
    /// the attempt WAITED for until it timed out ([`FAILED_IMAGE_FETCH_TIMEOUT`]), which is
    /// what [`Self::fetch_timed_out`] records — so the kind of failure is decided here, where
    /// it is known, rather than implied by which call site built the verdict.
    fn of_unreachable_registry(launch: &TaskLaunch, failure: Option<(String, String)>) -> Self {
        let mut verdict = Self::of(launch, failure);
        verdict.fetch_timed_out = verdict.failure.is_some();
        verdict
    }
}

/// Why the agent could not retrieve the Amazon ECR registry AUTHENTICATION `launch`'s images
/// need — the `stoppedReason` that earns — and `None` when it could, or when no image of the
/// task comes from a registry this emulator holds.
///
/// "The task execution role ... grants the Amazon ECS container agent permission to make
/// Amazon Web Services API calls on your behalf" (`TaskDefinition$executionRoleArn`), and the
/// first of those calls is the registry's authorization token: it is granted on the registry
/// rather than on a repository, so ONE answer covers every image the task pulls from that
/// registry and the question is asked once per task — which is also the order the agent makes
/// them in, the token before any manifest. Amazon ECR decides it, over the documents in force
/// on the role, exactly as it decides the manifest call
/// ([`registry_pull::withheld_registry_authorization`]); a reference into another registry
/// states nothing about this one, so a task pulling from none asks nothing.
///
/// Called with no guard of this service's store held: the verdict is Amazon ECR's, taken
/// under that service's own lock and IAM's.
fn withheld_registry_auth<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: &TaskLaunch,
) -> Option<String> {
    use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull;

    let pulls_from_this_registry = launch.images.iter().any(|image| {
        registry_pull::parse_reference(&reference_into_this_registry(
            engine,
            image,
            &launch.task_arn,
        ))
        .is_some()
    });
    if !pulls_from_this_registry {
        return None;
    }
    let refusal = registry_pull::withheld_registry_authorization(
        engine,
        pull_identity(launch).as_ref().map(PullIdentity::principal),
    )?;
    Some(unable_to_retrieve_registry_auth_message(&refusal))
}

/// The principal Amazon ECS makes a launched task's pulls as: the task's EXECUTION ROLE,
/// whose documents are in force, and the SESSION the agent assumed it in for that task, which
/// is what AWS names as the caller of a call made under one — the very pair the execution
/// resources of the task are retrieved as ([`execution_role_session_arn`]), because the pull
/// is one of them.
#[derive(Debug, Clone)]
struct PullIdentity {
    /// The execution role's ARN.
    role_arn: String,
    /// The session that role was assumed in for this task, and `None` where its ARN states no
    /// role to name a session of.
    session_arn: Option<String>,
}

impl PullIdentity {
    /// This identity as the registry takes a pull's principal.
    fn principal(
        &self,
    ) -> ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull::PullPrincipal<'_>
    {
        ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull::PullPrincipal {
            role_arn: &self.role_arn,
            session_arn: self.session_arn.as_deref(),
        }
    }
}

/// The identity `launch`'s pulls are made as, and `None` for a task whose revision and
/// overrides name no execution role — which states nothing for a policy to be read against.
fn pull_identity(launch: &TaskLaunch) -> Option<PullIdentity> {
    let role_arn = launch.execution_role_arn.clone()?;
    let task_id = launch
        .task_arn
        .rsplit('/')
        .next()
        .unwrap_or(launch.task_arn.as_str());
    let session_arn = execution_role_session_arn(&role_arn, task_id);
    Some(PullIdentity {
        role_arn,
        session_arn,
    })
}

/// Fetch each image of `launch` from the registry its reference names, and answer what that
/// took: the FIRST image a registry this emulator holds would not serve with why it could
/// not be fetched, and the manifest digest of each image that was served.
///
/// Two registries are this emulator's to answer for, and an image reference names one of
/// them by its host (the module docs stand this on what each registry documents):
///
/// * an AMAZON ECR private registry, which decides whose registry the reference addresses,
///   the authorization, the resolution of the reference and the upstream image it caches
///   where a pull through cache rule covers the repository — which is why the import happens
///   on the FIRST pull of such a reference and is visible to every later read of that
///   repository, and why a reference into ANOTHER account's registry is answered as that
///   registry's own cross-account refusal rather than resolved against the same-named
///   repository of this account's;
/// * the one public gallery this emulator serves, which resolves a reference into ITS OWN
///   registry alias against the repositories and tags it holds.
///
/// Nothing else is decided: a reference naming an alias that gallery was not assigned names
/// another public registry, and one naming any other registry — or a bare name a container
/// runtime resolves against Docker Hub — names a registry this emulator does not hold, so
/// neither is evidence about an image and the network path above is all that was decided for
/// it.
fn fetch_launch_images<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: &TaskLaunch,
    now: SystemTime,
) -> LaunchFetch {
    let mut digests: Vec<(String, String)> = Vec::new();
    let mut cached_sources: Vec<(String, String)> = Vec::new();
    for image in &launch.images {
        match fetch_image(engine, launch, image, now) {
            ImageFetch::Undecided => {}
            ImageFetch::Served {
                digest,
                cached_from,
            } => {
                if let Some(digest) = digest {
                    digests.push((image.clone(), digest));
                }
                if let Some(cached_from) = cached_from {
                    cached_sources.push((image.clone(), cached_from));
                }
            }
            // The first image that cannot be fetched is what stops the task: its containers
            // are not started, so the images after it are not fetched either.
            ImageFetch::Refused(reason) => {
                return LaunchFetch {
                    refused: Some((image.clone(), reason)),
                    digests,
                    cached_sources,
                }
            }
        }
    }
    LaunchFetch {
        refused: None,
        digests,
        cached_sources,
    }
}

/// What the fetch of a whole launch's images answered ([`fetch_launch_images`]).
struct LaunchFetch {
    /// `(image reference, reason)` of the FIRST image a registry this emulator holds would
    /// not serve, and `None` where nothing was decided against the fetch.
    refused: Option<(String, String)>,
    /// `(image reference, manifest digest)` for each image such a registry served.
    digests: Vec<(String, String)>,
    /// `(image reference, upstream reference)` for each image the repository it names holds
    /// as a CACHED COPY of an upstream one ([`ImageFetch::Served`]).
    cached_sources: Vec<(String, String)>,
}

/// What the fetch of ONE image reference answered.
enum ImageFetch {
    /// A registry this emulator holds served the reference.
    Served {
        /// The manifest digest it resolved the reference to, and `None` where that registry
        /// states no digest for it.
        digest: Option<String>,
        /// The UPSTREAM reference the repository it resolved holds a cached copy of, for a
        /// repository an Amazon ECR pull through cache rule covers
        /// ([`registry_pull::cached_upstream_reference`]) — and `None` for a repository that
        /// holds an image of its own registry, which is what the reference already names.
        cached_from: Option<String>,
    },
    /// The reference names no registry this emulator holds, so nothing is decided about it.
    Undecided,
    /// A registry this emulator holds would not serve it: the `stoppedReason` of the stop
    /// the task earns.
    Refused(String),
}

/// What the fetch of ONE of `launch`'s image references answered ([`fetch_launch_images`]
/// for which registries are answered for).
fn fetch_image<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: &TaskLaunch,
    image: &str,
    now: SystemTime,
) -> ImageFetch {
    use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull;
    use ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
        gallery_pull, published_images,
    };

    // Amazon ECR answers for every account's registry its own reading of a reference names,
    // this one's included, so whose registry it is comes back from there rather than being
    // decided here.
    let addressed = reference_into_this_registry(engine, image, &launch.task_arn);
    if let Some(served) = registry_pull::serve_reference_pull(
        engine,
        &addressed,
        pull_identity(launch).as_ref().map(PullIdentity::principal),
        now,
    ) {
        return match served {
            Ok(digest) => ImageFetch::Served {
                digest,
                // The repository the reference resolved may be a pull through CACHE
                // repository, which holds the upstream image the rule covering it caches: the
                // image this container runs is then that upstream one, so the reference it
                // resolves to is recorded beside the digest
                // ([`registry_pull::cached_upstream_reference`]).
                cached_from: registry_pull::parse_registry_reference(&addressed).and_then(
                    |(_, reference)| registry_pull::cached_upstream_reference(engine, &reference),
                ),
            },
            // The registry refused the manifest request the fetch makes, which the runtime
            // reports as the status it got back: the request is the one the registry's own
            // reading of the reference names.
            Err(registry_pull::PullRefusal::NotAuthorized { .. }) => {
                let refused = registry_pull::parse_registry_reference(&addressed)
                    .map(|(_, reference)| manifest_url(image, &reference))
                    .unwrap_or_default();
                ImageFetch::Refused(pull_access_denied_message(image, &refused))
            }
            Err(registry_pull::PullRefusal::NotFound { .. }) => {
                ImageFetch::Refused(image_not_found_message(image))
            }
        };
    }
    // The public gallery answers for the alias it was ASSIGNED: the pull is a question for
    // the registry the reference names, and an alias this gallery was not given names a
    // registry this emulator does not hold.
    let Some(reference) = gallery_pull::parse_gallery_reference(image) else {
        // The repository one of the gallery's MIRRORED images is carried from is the same
        // repository under another registry's spelling, so the releases this emulator holds
        // of that image decide a reference through it too ([`mirrored_image_reference`]).
        return match mirrored_image_reference(image)
            .and_then(|(path, tag)| published_images::official_image(&path, &tag))
        {
            Some(published_images::PublishedImage::Published) => ImageFetch::Served {
                digest: None,
                cached_from: None,
            },
            Some(published_images::PublishedImage::NotPublished) => {
                ImageFetch::Refused(image_not_found_message(image))
            }
            Some(published_images::PublishedImage::NoSuchRepository) => {
                ImageFetch::Refused(repository_does_not_exist_message(image))
            }
            None => ImageFetch::Undecided,
        };
    };
    if gallery_pull::serves_registry_alias(engine, &reference.registry_alias) {
        return match gallery_pull::serve_pull(engine, &reference) {
            Ok(digest) => ImageFetch::Served {
                digest,
                cached_from: None,
            },
            Err(gallery_pull::GalleryPullRefusal::NotFound { .. }) => {
                ImageFetch::Refused(image_not_found_message(image))
            }
        };
    }
    // A repository that gallery publishes ITSELF, whose image this emulator holds: what the
    // gallery publishes decides the reference, so a tag it publishes no release under resolves
    // to no image ([`published_images`]). No digest is stated for such an image: the
    // repository's published tags name releases, not the content a digest addresses.
    match published_images::published_image(&reference) {
        Some(published_images::PublishedImage::Published) => ImageFetch::Served {
            digest: None,
            cached_from: None,
        },
        Some(published_images::PublishedImage::NotPublished) => {
            ImageFetch::Refused(image_not_found_message(image))
        }
        Some(published_images::PublishedImage::NoSuchRepository) => {
            ImageFetch::Refused(repository_does_not_exist_message(image))
        }
        None => ImageFetch::Undecided,
    }
}

/// The reference `image` makes into AMAZON ECR, in the AWS-hosted spelling that registry's
/// own reading of a pull reference takes ([`registry_pull::parse_registry_reference`]).
///
/// One registry is addressed by two spellings here, and both name it: AWS's own registry
/// host (`<account>.dkr.ecr.<region>.<suffix>`), and the ADDRESS the registry announces to a
/// client — `Repository$repositoryUri` is "The URI for the repository. You can use this URI
/// for container image push and pull operations", and on this machine that URI is built on
/// an address of the emulator serving the registry, because AWS's own registry host resolves
/// nowhere here ([`addresses_this_emulators_registry`]). A container definition written from
/// what Amazon ECR handed the client therefore carries that address, so it is rewritten into
/// the published spelling and resolved exactly like one that carries it already, rather than
/// read a second time with a reading of this service's own.
///
/// A host that ALREADY names an Amazon ECR registry in AWS's published spelling is left
/// exactly as it is, whichever account it names: such a reference is one that registry reads
/// itself, so rewriting it would move a reference into another account's registry into this
/// account's and serve the same-named repository of ours for it. Which account it names is
/// then Amazon ECR's to decide, as every other part of the pull is. The test for an address
/// of this emulator cannot answer that question on its own, because a registry host's first
/// label is an account id and an all-digit label names no service.
///
/// One address serves every Region's registry, so such a reference names no Region of its
/// own: the Region is the one the TASK runs in, which its own ARN carries. The host is
/// composed by Amazon ECR's own composer of its published spelling (`registry_host`, which
/// carries that Region's partition suffix), the inverse of the walk that registry resolves
/// the rewritten reference by — so a task of any partition's Region resolves, not only one of
/// the commercial partition.
///
/// That one address serves BOTH registries this emulator holds, so a reference this emulator's
/// PUBLIC gallery answers for is left exactly as it is too
/// ([`addresses_this_emulators_gallery`]): rewriting it would turn a pull from the gallery
/// into a pull of the same-named path from the private registry.
fn reference_into_this_registry<'image, Eng: Send + Sync + 'static>(
    engine: &Eng,
    image: &'image str,
    task_arn: &str,
) -> ::std::borrow::Cow<'image, str> {
    use ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state as ecr_store;

    let Some((authority, path)) = image.split_once('/') else {
        return ::std::borrow::Cow::Borrowed(image);
    };
    if ecr_store::registry_pull::parse_registry_reference(image).is_some()
        || !addresses_this_emulators_registry(authority)
        || addresses_this_emulators_gallery(engine, image)
    {
        return ::std::borrow::Cow::Borrowed(image);
    }
    let Some(region) = ::service_common::arn::region_of(task_arn, ARN_NAMESPACE) else {
        return ::std::borrow::Cow::Borrowed(image);
    };
    ::std::borrow::Cow::Owned(format!(
        "{}/{path}",
        ecr_store::registry_host(ecr_store::registry_id(), region)
    ))
}

/// Why the containers of `launch` cannot be launched where the task is placed — the first
/// image no path reaches and the `stoppedReason` that earns — or `None` when every image can
/// be fetched, or when this emulator holds no network facts about the placement at all.
fn unfetchable_image(
    store: &ComAmazonawsEc2Amazonec2State,
    launch: &TaskLaunch,
) -> Option<(String, String)> {
    // The subnet the task's elastic network interface was given, which is the one the
    // launch named first
    // ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::mint_task_attachments`]).
    let subnet_id = launch.subnets.first()?;
    let vpc_id = store.subnets.get(subnet_id)?.vpc_id.clone()?;
    let internet =
        subnet_reaches_internet(store, subnet_id, &vpc_id, launch.assign_public_ip.as_deref());
    launch.images.iter().find_map(|image| {
        let reachable = internet || endpoints_reach_registry(store, &vpc_id, image);
        (!reachable).then(|| (image.clone(), cannot_pull_container_message(image)))
    })
}

/// Whether traffic leaving `subnet_id` reaches the internet, for a task whose
/// `assignPublicIp` is `assign_public_ip`.
///
/// Answered off the route table that APPLIES to the subnet ([`effective_route_table`]) and
/// the default route in it, which is the rule the Amazon VPC User Guide states for every
/// kind of subnet: traffic bound for anything outside the VPC goes where that route sends
/// it, and a subnet whose table has no such route keeps it inside the VPC.
fn subnet_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    subnet_id: &str,
    vpc_id: &str,
    assign_public_ip: Option<&str>,
) -> bool {
    let Some(table) = effective_route_table(store, subnet_id, vpc_id) else {
        return false;
    };
    table
        .routes
        .iter()
        .flatten()
        .filter(|route| {
            route.destination_cidr_block.as_deref() == Some(DEFAULT_IPV4_DESTINATION)
                // `Route$state` is `blackhole` while "the specified gateway isn't attached
                // to the VPC, or the specified NAT instance has been terminated", and such
                // a route carries nothing.
                && route.state.as_deref() != Some(ROUTE_STATE_BLACKHOLE)
        })
        .any(|route| route_reaches_internet(store, route, assign_public_ip))
}

/// Whether ONE default route carries traffic to the internet, for a task whose
/// `assignPublicIp` is `assign_public_ip`.
fn route_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    route: &Route,
    assign_public_ip: Option<&str>,
) -> bool {
    if let Some(nat_gateway_id) = route.nat_gateway_id.as_deref() {
        return nat_gateway_reaches_internet(store, nat_gateway_id);
    }
    if route
        .gateway_id
        .as_deref()
        .is_some_and(|gateway_id| gateway_id.starts_with(INTERNET_GATEWAY_ID_PREFIX))
    {
        // An internet gateway carries traffic for an address that is reachable from the
        // internet, which for an `awsvpc` task is the public address `assignPublicIp` asks
        // for — "If you don't specify an IP address ... the task won't have access to the
        // internet" is the same rule the member's own doc states for a task in a public
        // subnet.
        return assign_public_ip == Some(ASSIGN_PUBLIC_IP_ENABLED);
    }
    // A transit gateway carries the traffic off the VPC, to whatever its own route table
    // sends it to.
    route.transit_gateway_id.is_some()
}

/// Whether the NAT gateway `nat_gateway_id` carries internet-bound traffic.
///
/// "With a public NAT gateway, internet-bound traffic from a private subnet can be routed
/// to the NAT gateway ... With a private NAT gateway, internet-bound traffic from a private
/// subnet can't be routed to the gateway" (`CreateNatGateway`), and a gateway that is being
/// deleted, is deleted, or failed to come up carries nothing.
fn nat_gateway_reaches_internet(
    store: &ComAmazonawsEc2Amazonec2State,
    nat_gateway_id: &str,
) -> bool {
    let Some(gateway) = store.nat_gateways.get(nat_gateway_id) else {
        return false;
    };
    let public = gateway
        .connectivity_type
        .as_deref()
        .unwrap_or(CONNECTIVITY_TYPE_PUBLIC)
        == CONNECTIVITY_TYPE_PUBLIC;
    let in_service = matches!(
        gateway.state.as_deref(),
        Some(NAT_GATEWAY_STATE_AVAILABLE | NAT_GATEWAY_STATE_PENDING)
    );
    public && in_service
}

/// Whether the VPC `vpc_id` holds the endpoints that reach the registry `image` is held
/// in, with no route off the VPC at all.
///
/// Only Amazon ECR is reachable this way, and only through all three of the endpoints that
/// service's own documentation names: an interface endpoint for the ECR API, one for its
/// Docker registry, and a gateway endpoint for Amazon S3, which the image layers are
/// fetched from.
fn endpoints_reach_registry(
    store: &ComAmazonawsEc2Amazonec2State,
    vpc_id: &str,
    image: &str,
) -> bool {
    if !is_ecr_image(image) {
        return false;
    }
    [
        (ECR_API_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_INTERFACE),
        (ECR_DKR_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_INTERFACE),
        (S3_ENDPOINT_SUFFIX, VPC_ENDPOINT_TYPE_GATEWAY),
    ]
    .iter()
    .all(|(suffix, endpoint_type)| {
        store.vpc_endpoints.values().any(|endpoint| {
            endpoint.vpc_id.as_deref() == Some(vpc_id)
                && endpoint.vpc_endpoint_type.as_deref() == Some(*endpoint_type)
                && endpoint
                    .service_name
                    .as_deref()
                    .is_some_and(|name| name.ends_with(*suffix))
                // An endpoint Amazon EC2 is still bringing into service is one the VPC
                // has; one that was deleted, rejected or failed is not.
                && matches!(
                    endpoint.state.as_deref(),
                    Some(VPC_ENDPOINT_STATE_AVAILABLE | VPC_ENDPOINT_STATE_PENDING)
                )
        })
    })
}

/// Whether `image` names a repository in Amazon ECR, whose registry is an AWS service and
/// so has VPC endpoints: an ECR image is pulled from a registry host of that service
/// (`<account>.dkr.ecr.<region>.<zone>`, the registry URI a pull uses), read through Amazon
/// ECR's own one reading of such a host
/// (`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_host_parts`),
/// so the spelling AWS publishes and the one that registry publishes HERE — its own labels in
/// the zone this emulator is reached in — are both read as the registry they name; or from
/// the ADDRESS that registry announces its own repositories at when that address names no
/// registry at all ([`addresses_this_emulators_registry`]), which names the very same
/// registry.
fn is_ecr_image(image: &str) -> bool {
    let host = registry_host(image);
    ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_host_parts(
        host,
    )
    .is_some()
        || addresses_this_emulators_registry(host)
}

/// Whether any of `images` is one a registry serves only to an AUTHENTICATED principal, so
/// that fetching it needs the task execution role Amazon ECS pulls as
/// (`TaskDefinition$executionRoleArn`).
///
/// Amazon ECR is such a registry: a pull from it is two signed calls
/// (`ecr:GetAuthorizationToken` and `ecr:BatchGetImage`), decided over the policies in force
/// on the principal making them. A public registry serves anyone, and a reference naming no
/// registry at all resolves to one ([`DOCKER_HUB_REGISTRY`]), so neither needs a principal —
/// which is why the question is asked of the registry the reference RESOLVES to
/// ([`registry_host`]) rather than of the reference's text.
///
/// The ONE reading both of this service's registration-time rules take: the launch types a
/// revision can run under derive it
/// ([`TaskDefinitionRecord::pull_needs_execution_role`]) and a registration that requires
/// Fargate is refused by it, so the launch types a revision admits and the registrations
/// Amazon ECS accepts cannot disagree. WHETHER a registry serves one particular pull is
/// another question, and Amazon ECR's own to answer about the references it publishes
/// ([`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull`]).
pub fn needs_execution_role_to_pull<'a>(mut images: impl Iterator<Item = &'a str>) -> bool {
    images.any(serves_only_an_authenticated_principal)
}

/// Whether the registry ONE image reference resolves to serves it only to an authenticated
/// principal — the per-reference half of [`needs_execution_role_to_pull`].
fn serves_only_an_authenticated_principal(image: &str) -> bool {
    is_ecr_image(image)
}

/// Whether `host` is an address of THIS EMULATOR rather than the name of a registry
/// published on the internet — which makes it the endpoint Amazon ECR announces its own
/// registry at here.
///
/// `Repository$repositoryUri` is "The URI for the repository. You can use this URI for
/// container image push and pull operations", and the registry is reachable on this machine
/// only where the emulator serves it, so a client that asked that service for the URI is
/// handed an ADDRESS of this emulator and pulls from there. A reference carrying that
/// address therefore names a repository of the account's own registry, exactly as the
/// published `<account>.dkr.ecr.<region>.<suffix>` form does.
///
/// Read through the tree's one reading of the question
/// (`service_common::data_plane_addressing`): a host-shaped value that names no service is
/// an address of this emulator, whichever interface and port it is served on — nothing here
/// reads an environment variable or a bind address, so the answer does not change with how
/// the emulator was started. A reference to a registry of the developer's own addressed by
/// IP reads as this registry too, which is the same answer this emulator gives such an
/// address everywhere else: it is the only registry it serves.
///
/// The host of the base this emulator reports while it is served on NO port at all
/// (`smithy_server_runtime::UNSERVED_URL_BASE`, a name RFC 6761 reserves so that nothing
/// resolves it) is such an address as well: it is what the endpoint reading above answers
/// then, so it is what Amazon ECR announces then, and its first label would otherwise read
/// as a service's.
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

/// Whether this emulator's PUBLIC gallery is the registry `image` addresses: a reference
/// carrying the registry alias that gallery was ASSIGNED, at the host AWS publishes for it or
/// at an address this emulator answers on.
///
/// One address serves both registries this emulator holds, and a private repository's name may
/// itself carry `/` ("project-a/nginx-web-app"), so the first path segment alone does not say
/// which registry a reference addresses. The gallery's own readings answer it — its reference
/// form ([`gallery_pull::parse_gallery_reference`]) and whether the alias is the one it was
/// assigned ([`gallery_pull::serves_registry_alias`]) — so this service writes no rule of its
/// own about either registry's reference layout.
fn addresses_this_emulators_gallery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    image: &str,
) -> bool {
    use ::service_com_amazonaws_ecrpublic_spencerfrontendservice::state::gallery_pull;

    gallery_pull::parse_gallery_reference(image).is_some_and(|reference| {
        gallery_pull::serves_registry_alias(engine, &reference.registry_alias)
    })
}

/// The route table that APPLIES to `subnet_id`: the one a caller explicitly associated
/// with it, and otherwise the MAIN table of its VPC.
///
/// "Each subnet in your VPC must be associated with a route table ... A subnet can be
/// explicitly associated with a route table, or implicitly or explicitly associated with
/// the main route table" (Amazon VPC User Guide, "Route tables"), so exactly one table
/// decides where the subnet's traffic goes. The explicit half is Amazon EC2's own reading
/// of its own records
/// ([`ComAmazonawsEc2Amazonec2State::route_table_associated_with_subnet`]), so an
/// association made through that service resolves here exactly as it does there.
fn effective_route_table<'store>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    subnet_id: &str,
    vpc_id: &str,
) -> Option<&'store RouteTable> {
    store
        .route_table_associated_with_subnet(subnet_id)
        .and_then(|route_table_id| store.route_tables.get(&route_table_id))
        .or_else(|| {
            store.route_tables.values().find(|table| {
                table.vpc_id.as_deref() == Some(vpc_id)
                    && table
                        .associations
                        .iter()
                        .flatten()
                        .any(|association| association.main == Some(true))
            })
        })
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every task whose image fetch has not been attempted yet: one that is on its way UP
    /// (its `desiredStatus` is `RUNNING` and it has not started), whose launch roles have
    /// already been put to the account's IAM records ([`TaskRecord::launch_roles_assumed`])
    /// and that carries no pull instants, with the placement it was launched into, the
    /// images its containers run and the execution role the pull is made as.
    ///
    /// The fetch is made AS the execution role, so a launch whose roles Amazon ECS has not
    /// assumed yet has nothing to fetch with: such a task is left to the pass that follows
    /// the assume rather than fetched for here — which is also what keeps its start withheld
    /// ([`TaskRecord::launch_steps_taken`]).
    ///
    /// A task that shares the networking of the host that runs it carries no placement of
    /// its own, so the PATH half decides nothing for it: the path it fetches an image over
    /// is that host's. The registry half is decided for it like any other task — the image
    /// a reference names is the same image whichever networking the task runs with.
    pub fn launches_awaiting_image_pull(&self) -> Vec<TaskLaunch> {
        self.tasks
            .values()
            .filter(|task| {
                task.launch_failure.is_none()
                    && task.launch_roles_assumed
                    && task.pull_started_at.is_none()
                    && task.desired_status == STATUS_RUNNING
                    && task.is_pending()
            })
            .map(|task| TaskLaunch {
                task_arn: task.task_arn.clone(),
                subnets: task
                    .awsvpc_configuration
                    .as_ref()
                    .map(|awsvpc| awsvpc.subnets.clone())
                    .unwrap_or_default(),
                assign_public_ip: task
                    .awsvpc_configuration
                    .as_ref()
                    .and_then(|awsvpc| awsvpc.assign_public_ip.clone()),
                images: task
                    .containers
                    .iter()
                    .filter_map(|container| container.image.clone())
                    .collect(),
                execution_role_arn: task.execution_role_arn.clone(),
                secrets: self.launched_task_secrets(task),
                log_targets: self.launched_task_log_targets(task),
                images_naming_no_program: self.images_naming_no_program(task),
            })
            .collect()
    }

    /// The images of `task`'s containers that nothing NAMES a program for
    /// ([`TaskLaunch::images_naming_no_program`]).
    ///
    /// Read through the one reading of a container's program this crate has
    /// ([`super::container_runtime::container_program`]), so what counts as a container with
    /// nothing named for it is exactly what the runs that module starts resolve to no program:
    /// the container's own definition, the launch's `containerOverrides` entry for it, and the
    /// halves its image declares in this emulator's catalogs.
    ///
    /// Empty for a task whose registered revision this store no longer holds: what its
    /// containers were given to run is that revision's, and nothing else states it.
    fn images_naming_no_program(&self, task: &TaskRecord) -> Vec<String> {
        let Some(definition) = self.revision_of_arn(&task.task_definition_arn) else {
            return Vec::new();
        };
        task.containers
            .iter()
            .filter_map(|container| {
                let name = container.name.as_deref()?;
                let image = container.image.clone()?;
                let stored = definition
                    .container_definitions
                    .iter()
                    .find(|candidate| candidate.name.as_deref() == Some(name))?;
                let overridden = task
                    .overrides
                    .as_ref()
                    .and_then(|overrides| overrides.container_overrides.as_ref())
                    .and_then(|overrides| {
                        overrides
                            .iter()
                            .find(|candidate| candidate.name.as_deref() == Some(name))
                    })
                    .and_then(|over| over.command.as_deref());
                super::container_runtime::container_program(stored, overridden, Some(&image))
                    .is_none()
                    .then_some(image)
            })
            .collect()
    }

    /// Record what the image fetch of each named task took, and why it failed where it did
    /// — the `stoppedReason` the stop the clock then owes carries
    /// ([`TaskRecord::fail_launch`]).
    ///
    /// `Task$pullStartedAt` is "the time when the container image pull began" and
    /// `pullStoppedAt` "the time when the container image pull completed": the fetch is made
    /// in this process, so one that was ANSWERED began and completed at the instant it was
    /// attempted, and both are recorded whether or not it succeeded — a pull that failed was
    /// still attempted. A fetch that could not REACH the registry is the one failure nothing
    /// answers: it ends in a connection timeout, so the attempt is still going here and
    /// `pullStoppedAt` is stamped at the instant it ends ([`TaskRecord::fail_launch`], which
    /// the stop then falls due at — [`TaskRecord::launch_failure_due_at`]).
    /// The instants are recorded for a task whose start fell due while the fetch was being
    /// decided too: the fetch was made for THAT task, so the instants belong on it however
    /// far the clock has since taken it.
    ///
    /// Only a task that is still on its way UP takes a FAILURE: one that has started has
    /// already fetched its images, and one that is stopping is on its way out either way.
    ///
    /// Each container whose image the fetch RESOLVED reports the manifest digest the registry
    /// resolved it to: `Container$imageDigest` is "The container image manifest digest",
    /// which is known from the pull on — and stays absent for an image no registry here
    /// resolved, because no manifest of it was fetched to digest.
    ///
    /// Each container whose image a repository that CACHES one served records the upstream
    /// reference the pull resolved ([`ContainerRecord::cached_image_source`]), which is the
    /// image the container really runs.
    pub fn record_image_pulls(&mut self, verdicts: &[ImagePullVerdict], now: SystemTime) {
        let at = reported_instant(now);
        for verdict in verdicts {
            if let Some(task) = self.tasks.get_mut(&verdict.task_arn) {
                if task.pull_started_at.is_some() || task.desired_status != STATUS_RUNNING {
                    continue;
                }
                // Whether the attempt this records is one that is STILL GOING: a fetch with
                // no path to the registry waits out its connection timeout, and only a task
                // this failure is the one that blocks is waiting for it.
                let timed_out = verdict.fetch_timed_out
                    && task.launch_failure.is_none()
                    && task.is_pending();
                task.pull_started_at = Some(at);
                task.pull_stopped_at = (!timed_out).then_some(at);
                // The fetch is made for a container Amazon ECS has CREATED on the capacity
                // the task was placed on, so each container reports the runtime id it was
                // created with from here on, whichever way the fetch went
                // ([`ContainerRecord::report_runtime_id`]).
                let task_id = task.task_id.clone();
                for container in &mut task.containers {
                    container.report_runtime_id(&task_id);
                    if let Some((_, digest)) = verdict
                        .image_digests
                        .iter()
                        .find(|(image, _)| Some(image.as_str()) == container.image.as_deref())
                    {
                        container.image_digest = Some(digest.clone());
                    }
                    // And the UPSTREAM image a repository that caches one served it, which is
                    // the image this container really runs
                    // ([`ContainerRecord::running_image`]).
                    if let Some((_, cached_from)) = verdict
                        .cached_image_sources
                        .iter()
                        .find(|(image, _)| Some(image.as_str()) == container.image.as_deref())
                    {
                        container.cached_image_source = Some(cached_from.clone());
                    }
                }
                if task.launch_failure.is_none() && task.is_pending() {
                    task.launch_failure = verdict.failure.clone();
                    // A fetch that failed failed for ONE image, and the container that runs
                    // that image is the one Amazon ECS could not create: it is the container
                    // the failure is reported on when the stop falls due
                    // ([`TaskRecord::launch_failure_container`]).
                    task.launch_failure_container =
                        verdict.failed_image.as_deref().and_then(|image| {
                            task.containers
                                .iter()
                                .find(|container| container.image.as_deref() == Some(image))
                                .and_then(|container| container.name.clone())
                        });
                    // And a fetch nothing answered ends when the agent gives the attempt
                    // up, which is when the stop it earns falls due
                    // ([`TaskRecord::launch_failure_due_at`]).
                    if timed_out {
                        task.launch_failure_due_at = Some(now + FAILED_IMAGE_FETCH_TIMEOUT);
                    }
                }
            }
        }
    }
}

impl TaskRecord {
    /// Whether Amazon ECS could not launch this task's containers AT ALL, which is exactly
    /// the stop [`Self::fail_launch`] records: a task stopped under
    /// [`STOP_CODE_TASK_FAILED_TO_START`] never ran.
    ///
    /// Read off the stop code the stop carries rather than off the desired status, because a
    /// task Amazon ECS marks for the stop a failed launch earns is on its way out like any
    /// other stopping task — what tells it apart from one the scheduler asked to stop is WHY
    /// it is stopping. This is the failure the deployment circuit breaker counts
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::deployment_launch_failures`])
    /// and the one a deployment reports as a failed launch.
    pub fn failed_to_launch(&self) -> bool {
        self.stop_code.as_deref() == Some(STOP_CODE_TASK_FAILED_TO_START)
    }

    /// When that launch failed: the instant the stop it earned was recorded at
    /// ([`Self::fail_launch`] stamps it as the task's `stoppingAt`), and `None` for a task
    /// whose launch did not fail.
    pub fn launch_failed_at(&self) -> Option<SystemTime> {
        self.failed_to_launch()
            .then(|| self.stopping_at.or(self.stopped_at))
            .flatten()
            .and_then(|at| SystemTime::try_from(at).ok())
    }

    /// The `stoppedReason` of the stop this task has ALREADY EARNED, for one whose launch
    /// Amazon ECS could not make: the failure the launch steps recorded against it
    /// ([`TaskRecord::launch_failure`]) while it was still coming up, so the stop it is owed
    /// is [`Self::fail_launch`] whenever that stop is recorded.
    ///
    /// `None` for a task nothing is known to block, and for one that was launched after all:
    /// a task that has STARTED ran, so a stop it takes afterwards is that stop's own.
    pub fn earned_launch_failure(&self) -> Option<String> {
        if self.started_at.is_some() || !self.is_pending() {
            return None;
        }
        self.launch_failure.clone()
    }

    /// Stop this task because Amazon ECS could not launch its containers at all, as of
    /// `at`: the task never ran, so it gets no `startedAt`, its containers no exit code,
    /// and the stop carries [`STOP_CODE_TASK_FAILED_TO_START`] with `reason` as its
    /// `stoppedReason`.
    ///
    /// The task is on its way out from this instant — `desiredStatus` is `STOPPED`, which
    /// is what a `ListTasks` narrowed to stopped tasks reports it under — and `STOPPED`
    /// itself falls due one [`TRANSITION_SPAN`] later, the same two-step stop every other
    /// stop of a task is recorded as. `at` is the instant the ATTEMPT that failed the launch
    /// ended, which for a fetch nothing answered is the end of its timeout
    /// ([`Self::launch_failure_due_at`]) and for every other failure the instant the task's
    /// own start would have been.
    ///
    /// The failure is reported on the CONTAINER it belongs to as well, where the step that
    /// decided it named one ([`TaskRecord::launch_failure_container`]): `Container$reason` is
    /// "a short (1024 max characters) human-readable string to provide additional details
    /// about a running or stopped container", and the additional detail about a container
    /// Amazon ECS could not create is what kept it from being created. A report the AGENT has
    /// made about that container (`SubmitContainerStateChange$reason`) is left as the agent
    /// gave it, and a failure no one container owns — a resource the agent retrieves for the
    /// task as a whole, or a role the launch may not assume — is reported on the task alone.
    pub fn fail_launch(&mut self, reason: &str, at: SystemTime) {
        self.desired_status = STATUS_STOPPED.to_string();
        self.stopped_reason = Some(reason.to_string());
        self.stop_code = Some(STOP_CODE_TASK_FAILED_TO_START.to_string());
        self.stopping_at = Some(reported_instant(at));
        self.pending_due_at = None;
        self.running_due_at = None;
        // The ATTEMPT that failed the launch ends here, where it was waited out rather than
        // answered ([`Self::launch_failure_due_at`]): `pullStoppedAt` is "the time when the
        // container image pull completed", so a fetch that was still going reports this
        // instant as the one it ended at.
        if self.pull_started_at.is_some() && self.pull_stopped_at.is_none() {
            self.pull_stopped_at = Some(reported_instant(at));
        }
        self.launch_failure_due_at = None;
        self.stopped_due_at = Some(at + TRANSITION_SPAN);
        if let Some(name) = self.launch_failure_container.clone() {
            for container in self
                .containers
                .iter_mut()
                .filter(|container| container.name.as_deref() == Some(name.as_str()))
            {
                if container.reason.is_none() {
                    container.reason = Some(reason.to_string());
                }
            }
        }
        self.version += 1;
    }
}
