// SPDX-License-Identifier: CC0-1.0

//! Serving an image PULL this registry is asked for by a compute service, on a ROLE's
//! behalf, and the upstream import a pull through cache rule makes on the first one.
//!
//! A container runtime does not call this API: it logs in to the registry and fetches a
//! manifest. The emulated compute services are what stand in for that runtime — Amazon ECS
//! pulls the image its task definition names before it starts a container — and they reach
//! the registry in-process, through this module, so the registry answers one question
//! however it is asked:
//!
//! 1. **may this principal pull** — a pull takes the two calls the agent makes against the
//!    registry, the authorization token for the registry
//!    ([`GET_AUTHORIZATION_TOKEN_ACTION`], granted on nothing narrower than the registry)
//!    and the image manifest itself ([`BATCH_GET_IMAGE_ACTION`], granted on the repository)
//!    — decided over the documents in force on the role and the repository's own policy,
//!    through the same gate a signed request of this service goes through
//!    ([`super::authorization`]);
//! 2. **does this registry hold the image** — the repository the reference names and the
//!    tag or digest in it, resolved through the repository's own lookup
//!    ([`Repo::lookup_image`]), so an in-process pull sees exactly what `BatchGetImage`
//!    serves;
//! 3. **can it be imported** — "A pull through cache rule provides a way to cache images
//!    from an upstream registry source in your Amazon ECR private registry"
//!    (`CreatePullThroughCacheRule`): a reference under a rule's `ecrRepositoryPrefix` is
//!    served by caching the upstream image, so the FIRST pull creates the cache repository
//!    and stores the image under the tag that was asked for, and every later read of that
//!    repository — `DescribeImages`, `ListImages`, `BatchGetImage` — reports it.
//!
//! That import is [`import_covered_image`], and it is reached by every plane a pull of a
//! covered reference arrives on — an in-process pull through [`serve_pull`], the
//! `BatchGetImage` call a client makes to fetch a manifest, and the manifest `GET` of the
//! registry endpoint a `docker pull` talks to — so the registry caches one pull the same way
//! however it is asked, and a repository under a rule's prefix is never reported absent
//! while the rule could have cached it.
//!
//! What an import stores is the registry's own record of the cached image: a manifest this
//! registry writes, describing one configuration blob and ONE filesystem layer it also
//! stores, with the digest and the size of each computed over the bytes held
//! ([`content_digest`]). No upstream registry is contacted — this emulator makes no
//! outbound connection — so the layer is not the upstream image's own bytes but a root
//! filesystem this registry writes and keeps ([`cached_image_filesystem_layer`]). Every
//! blob the cached manifest references is therefore really in the repository, which is what
//! lets every read of a cached image report the content this registry has: its manifest, the
//! availability and the download of its layers, its size, and the filesystem a basic scan
//! of it reads.
//!
//! Nothing is withheld on evidence this emulator does not have. An engine carrying no IAM
//! store, a role that store holds no record for, and a role carrying an attachment whose
//! document this account does not hold (an entry of AWS's own catalog) each show nothing
//! about the role, so the pull proceeds — the same rule every other gate over an execution
//! role's documents in this tree keeps. A pull made with no principal at all is not
//! decided either: a caller that names no role states nothing for a policy to be read
//! against.
//!
//! WHOSE registry a reference addresses is decided before any of that, for the caller that
//! holds a reference rather than a resolved one ([`serve_reference_pull`]): a registry
//! belongs to ONE account ("the Amazon Web Services account ID associated with the
//! registry"), its contents are shared only through its own resource-based policy, and this
//! emulator serves exactly one account's registry — so a reference into another account's is
//! served by nothing here and is refused as the cross-account call it is
//! ([`cross_account_refusal`]), never resolved against the same-named repository of this
//! registry. That question is about the reference and not about who is pulling, so it is
//! answered for a pull that names no principal too.
//!
//! LOCK ORDER: the authorization decision is taken BEFORE this service's store is locked
//! (it reads the IAM store), and the resolution and the import hold this store's lock
//! alone. A caller reaches this with no lock of its own held.

use super::*;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The registry-level call a pull begins with: "Retrieves an authorization token ... A
/// client can use the returned token to access any Amazon ECR registry that the IAM
/// principal has access to" (`GetAuthorizationToken`). It addresses the registry rather
/// than one repository, so it is decided on [`ANY_RESOURCE`].
pub const GET_AUTHORIZATION_TOKEN_ACTION: &str = "GetAuthorizationToken";

/// The call that fetches the image manifest a pull resolves: "Gets detailed information for
/// an image ... to pull an image" (`BatchGetImage`), decided on the repository the
/// reference names.
pub const BATCH_GET_IMAGE_ACTION: &str = "BatchGetImage";

/// The resource a registry-level action is decided on: a grant for it is written on
/// nothing narrower, which is how AWS's own task-execution policy states
/// `ecr:GetAuthorizationToken`.
const ANY_RESOURCE: &str = "*";

/// The tag a reference that names neither a tag nor a digest resolves to, which is the tag
/// a container runtime appends to a bare reference.
pub const DEFAULT_IMAGE_TAG: &str = "latest";

/// The media type of the configuration blob an imported manifest describes, in the OCI
/// spelling that goes with [`OCI_IMAGE_MANIFEST_MEDIA_TYPE`].
const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// The media type of the gzip compressed filesystem layer an imported manifest declares, in
/// the same OCI spelling as [`OCI_IMAGE_CONFIG_MEDIA_TYPE`].
const OCI_IMAGE_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// How many blobs a repository may hold before an IMPORT adds none.
///
/// See [`MAX_IMAGES_PER_REPOSITORY`]: the same reasoning for the blob map, which an import
/// grows by one configuration blob per cached image — every cached image of a repository
/// shares the one filesystem layer, which is content-addressed like any other blob — and
/// which no other cap bounds the entry COUNT of. It is set at the image cap because a
/// repository at this bound is one whose images are capped too. The bound is read BEFORE an
/// import, which then adds the one configuration blob (and, for the first cached image of
/// the repository, the shared filesystem layer), so a repository one entry below it ends at
/// most one entry above it — the heap this bounds is the entries themselves. Without it a
/// client pulling one tag after another under a cache rule would grow this process's heap
/// without end.
const MAX_IMPORTED_BLOBS_PER_REPOSITORY: usize = MAX_IMAGES_PER_REPOSITORY;

/// The separator a reference names a DIGEST after, as a registry client writes one
/// (`<repository>@sha256:<hex>`).
const DIGEST_SEPARATOR: char = '@';

/// An image reference that addresses THIS registry: which repository it names, in which
/// Region, and whether it asks for a tag or a digest.
///
/// Read from the reference alone ([`parse_reference`]), so the repository a pull resolves
/// is the one the reference spells rather than anything a caller passed beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryReference {
    /// The Region the registry host names, which the repository's ARN carries.
    pub region: String,
    /// The repository the reference names, as this registry keys one.
    pub repository_name: String,
    /// The tag the reference asks for, and `None` when it addresses a digest.
    pub tag: Option<String>,
    /// The digest the reference asks for, and `None` when it addresses a tag.
    pub digest: Option<String>,
}

impl RegistryReference {
    /// The `ImageIdentifier` this reference resolves through, which is the identifier
    /// `BatchGetImage` would carry for the same pull.
    ///
    /// Reachable inside this store, so a sibling reading of a reference resolves it exactly
    /// as the pull does ([`super::image_program`]) rather than assembling an identifier of
    /// its own.
    pub(in crate::state) fn identifier(&self) -> ImageIdentifier {
        ImageIdentifier {
            image_digest: self.digest.clone(),
            image_tag: self.tag.clone(),
        }
    }
}

/// Why this registry did not serve a pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRefusal {
    /// No policy in force on the principal permits one of the calls the pull makes: the
    /// action that was withheld, and the sentence AWS refuses it with.
    NotAuthorized {
        /// The fully qualified IAM action (`ecr:<OperationName>`) that was withheld.
        action: String,
        /// The authorization refusal itself ([`super::authorization`]).
        message: String,
    },
    /// Nothing in this registry answers to the reference, and no pull through cache rule
    /// covers the repository it names: the message the repository's own lookup words.
    NotFound {
        /// Why no image was found, in this registry's own words.
        message: String,
    },
}

/// Serve the pull of `reference` made on behalf of `principal`, importing the upstream
/// image when a pull through cache rule covers the repository and the registry does not
/// hold it yet.
///
/// `principal` is the ROLE the pull is made as — a task's execution role, with the session
/// the pulling service assumed it in ([`PullPrincipal`]) — and `None` for a pull whose caller
/// names none, which states nothing for a policy to be read against. `now` is the instant the
/// pull is made at, which is what an imported image records as its push instant.
///
/// What the pull SERVED — the manifest the registry resolved the reference to, reported as
/// its DIGEST, which is the image's identity in this registry and what a compute service
/// pulling for a container reports for it (`Container$imageDigest` in Amazon ECS is "The
/// container image manifest digest"). `None` comes back where no store of this service is
/// there to resolve one. The manifest itself stays in this registry, where every read of the
/// repository reports it.
///
/// A WRITE, and only through the import: a pull of an image this registry already holds
/// changes nothing a later caller sees.
pub fn serve_pull<Eng: Send + Sync + 'static>(
    engine: &Eng,
    reference: &RegistryReference,
    principal: Option<PullPrincipal<'_>>,
    now: SystemTime,
) -> Result<Option<String>, PullRefusal> {
    PullPlane::of(engine).serve_pull(reference, principal, now)
}

/// The stores a pull is decided from, as HANDLES: this registry's own and the IAM store the
/// pulling principal's documents are held in.
///
/// A compute service that stands in for a container runtime does not always hold the engine
/// when the pull happens — a pull a workload's own clock owes is decided from a plane that
/// service opened earlier — so the two stores a pull reads are taken once
/// ([`PullPlane::of`], with a handler's `&*self.engine`) and kept. They are the SAME stores
/// the engine-borne entry points resolve, so a pull decided through either answers alike.
#[derive(Clone)]
pub struct PullPlane {
    /// This registry's store, and `None` for an engine that carries none.
    registry: Option<::engine::ServiceState<ComAmazonawsEcrAmazonec2containerregistryV20150921State>>,
    /// The IAM store the principal's identity documents are read from, and `None` for an
    /// engine with no IAM — which shows nothing about the role, so nothing is withheld on it.
    identities: Option<
        ::engine::ServiceState<
            iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        >,
    >,
}

/// Written by hand rather than derived: what is diagnosable is whether the stores are there
/// at all, not what they hold.
impl ::std::fmt::Debug for PullPlane {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PullPlane")
            .field("registry", &self.registry.is_some())
            .field("identities", &self.identities.is_some())
            .finish()
    }
}

impl PullPlane {
    /// The stores in `engine`.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self {
            registry: service_state(engine),
            identities: iam_store::service_state(engine),
        }
    }

    /// This registry's store, for a sibling reading that answers out of it
    /// ([`super::image_program`]) — the SAME store a pull decided through this plane
    /// resolves against, so the two cannot answer about different registries.
    pub(in crate::state) fn registry_store(
        &self,
    ) -> Option<&::engine::ServiceState<ComAmazonawsEcrAmazonec2containerregistryV20150921State>>
    {
        self.registry.as_ref()
    }

    /// [`serve_reference_pull`], decided from these stores.
    pub fn serve_reference_pull(
        &self,
        image: &str,
        principal: Option<PullPrincipal<'_>>,
        now: SystemTime,
    ) -> Option<Result<Option<String>, PullRefusal>> {
        let (addressed_registry, reference) = parse_registry_reference(image)?;
        if addressed_registry != registry_id() {
            return Some(Err(cross_account_refusal(
                &addressed_registry,
                &reference,
                principal,
            )));
        }
        Some(self.serve_pull(&reference, principal, now))
    }

    /// [`withheld_registry_authorization`], decided from these stores.
    pub fn withheld_registry_authorization(
        &self,
        principal: Option<PullPrincipal<'_>>,
    ) -> Option<String> {
        match PreparedPrincipal::prepare(self, principal)?
            .withheld(GET_AUTHORIZATION_TOKEN_ACTION, ANY_RESOURCE, None)?
        {
            PullRefusal::NotAuthorized { message, .. } => Some(message),
            // [`PreparedPrincipal::withheld`] words every refusal it takes as the withheld
            // call it is; nothing there answers about the contents of a repository.
            PullRefusal::NotFound { .. } => None,
        }
    }

    /// [`serve_pull`], decided from these stores.
    pub fn serve_pull(
        &self,
        reference: &RegistryReference,
        principal: Option<PullPrincipal<'_>>,
        now: SystemTime,
    ) -> Result<Option<String>, PullRefusal> {
        if let Some(refusal) = self.withheld_pull(reference, principal) {
            return Err(refusal);
        }
        let Some(state) = self.registry.clone() else {
            // No store of this service to ask: nothing is known about the image, so the pull
            // is not refused on it.
            return Ok(None);
        };
        let mut store = state.lock();
        let identifier = reference.identifier();
        if let Ok(repository) = store.repository(&reference.region, &reference.repository_name) {
            if let Ok(image) = repository.lookup_image(&identifier) {
                return Ok(Some(image.digest.clone()));
            }
        }
        match import_covered_image(
            &mut store,
            &reference.region,
            &reference.repository_name,
            reference.tag.as_deref(),
            now,
        ) {
            CacheImport::Imported(digest) => Ok(Some(digest)),
            // A rule covers the repository and nothing was cached through it: the container
            // cannot start either way, and what it is told is why.
            CacheImport::Unavailable(message) | CacheImport::AtCapacity(message) => {
                Err(PullRefusal::NotFound { message })
            }
            CacheImport::NotCovered => Err(PullRefusal::NotFound {
                message: not_found_message(&store, reference),
            }),
        }
    }
}

/// Serve the pull of the reference `image` spells, whichever Amazon ECR registry it
/// addresses — the one entry point a compute service standing in for a container runtime
/// reaches a pull through, so the registry rather than the caller decides whose registry a
/// reference names.
///
/// `None` is a reference this registry does not answer for at all: a path that addresses no
/// Amazon ECR registry (another registry altogether, or a bare name a container runtime
/// resolves against a public registry). Otherwise the answer is the pull's: the manifest
/// digest it served the reference under ([`serve_pull`]), or the refusal it was withheld by.
///
/// A reference into another account's registry is never served here: "The Amazon Web Services
/// account ID associated with the registry" is one account's (`registryId`), the contents of
/// that registry are shared only through ITS own resource-based policy, and this emulator
/// serves exactly one account's registry — so no policy that could allow the pull is here to
/// read, and the repository the reference names in that registry is not the same-named
/// repository of this one ([`cross_account_refusal`]). WHOSE registry a host names does not
/// depend on who is pulling, so that refusal is answered for a pull made with no principal
/// too.
///
/// `principal` is the ARN of the ROLE the pull is made as and `now` the instant it is made
/// at, exactly as [`serve_pull`] takes them.
pub fn serve_reference_pull<Eng: Send + Sync + 'static>(
    engine: &Eng,
    image: &str,
    principal: Option<PullPrincipal<'_>>,
    now: SystemTime,
) -> Option<Result<Option<String>, PullRefusal>> {
    PullPlane::of(engine).serve_reference_pull(image, principal, now)
}

/// The refusal a pull of `reference` into the registry of the account `registry_id` earns,
/// made as `principal`.
///
/// For a pull made AS a role: the manifest call the pull makes ([`BATCH_GET_IMAGE_ACTION`]) on
/// the repository the reference names IN THAT registry, withheld because only that account's
/// own resource-based policy could allow it and this emulator holds no such registry. The
/// sentence is the tree's one wording of a grant no resource-based policy gives
/// ([`iam::not_authorized_by_resource_policy`]), which is the document AWS names when it
/// refuses a cross-account call — the same reading `super::authorization::registry_account`
/// takes for a signed request that addresses another account's registry.
///
/// For a pull that names NO principal there is nobody for an authorization sentence to blame,
/// and what is left is that no repository of that registry is here at all — this registry's
/// own wording for a repository a registry does not hold
/// ([`repository_not_found_in_message`]). Either way the pull is refused: a reference into a
/// registry this emulator does not serve is served by nothing, whoever asks.
fn cross_account_refusal(
    registry_id: &str,
    reference: &RegistryReference,
    principal: Option<PullPrincipal<'_>>,
) -> PullRefusal {
    let Some(principal) = principal else {
        return PullRefusal::NotFound {
            message: repository_not_found_in_message(&reference.repository_name, registry_id),
        };
    };
    let role_arn = principal.reported_arn();
    let action = super::authorization::action_for(BATCH_GET_IMAGE_ACTION);
    let resource = repository_arn_in(&reference.region, registry_id, &reference.repository_name);
    // The refusal is the security-relevant event of this pull, so it is attributable the way
    // every other withheld pull of this gate is: who the pull was made as, what it asked for,
    // and the outcome.
    ::tracing::info!(
        target: "emulator::audit",
        audit = true,
        operation = "ServeImagePull",
        outcome = "refused_access_denied",
        principal = %role_arn,
        action = %action,
        resource = %resource,
        "the registry this pull addresses belongs to another account, which shares none of \
         it with this principal"
    );
    PullRefusal::NotAuthorized {
        message: iam::not_authorized_by_resource_policy(role_arn, &action, &resource),
        action,
    }
}

/// The reference `image` names when it addresses a repository of THIS registry, and `None`
/// for every other reference — one in another account's registry, in another registry
/// altogether, or a bare name a container runtime resolves against a public registry.
///
/// The form is the one the Amazon ECR User Guide gives for a pull:
/// `<account>.dkr.ecr.<region>.amazonaws.com/<repository>[:<tag>|@<digest>]`, so the
/// Region and the repository are read out of the reference itself. A reference naming no
/// tag and no digest asks for [`DEFAULT_IMAGE_TAG`].
#[must_use]
pub fn parse_reference(image: &str) -> Option<RegistryReference> {
    let (addressed_registry, reference) = parse_registry_reference(image)?;
    (addressed_registry == registry_id()).then_some(reference)
}

/// The registry a pull reference ADDRESSES — the account id its host names — and the
/// reference into it, for a path addressing ANY account's Amazon ECR registry; `None` for a
/// path that addresses no Amazon ECR registry at all (another registry altogether, or a bare
/// name a container runtime resolves against a public registry).
///
/// The same reading [`parse_reference`] takes, with the account the host names REPORTED
/// rather than held to the account this registry serves: a caller that has to tell a
/// reference into another account's registry from one that addresses no registry — a compute
/// service deciding whether the image a request names is one it can pull — reads the registry
/// out of the path here rather than walking the host itself.
///
/// The form is the one the Amazon ECR User Guide gives for a pull:
/// `<account>.dkr.ecr.<region>.amazonaws.com/<repository>[:<tag>|@<digest>]`, so the account,
/// the Region and the repository are read out of the reference itself
/// ([`registry_host_parts`] for the host). A reference carrying the ADDRESS this registry
/// announced instead (`super::registry_address`) names no Region of its own — one endpoint
/// serves every Region's registry — so a caller that holds one brings it into this form for
/// the Region it is resolving in before asking (the compute services'
/// `reference_into_this_registry`). A reference naming no tag and no digest asks for
/// [`DEFAULT_IMAGE_TAG`].
#[must_use]
pub fn parse_registry_reference(image: &str) -> Option<(String, RegistryReference)> {
    let (host, path) = image.trim().split_once('/')?;
    let lowercased = host.to_ascii_lowercase();
    let (addressed_registry, region) = registry_host_parts(&lowercased)
        .map(|parts| (parts.registry_id.to_string(), parts.region.to_string()))?;
    let reference = reference_into(region, path)?;
    Some((addressed_registry, reference))
}

/// The repository and the tag or digest the PATH part of a pull reference names, in the
/// Region its host named.
fn reference_into(region: String, path: &str) -> Option<RegistryReference> {
    // A digest is written after `@`, a tag after the LAST `:` of the path — a repository
    // name carries neither character, so whichever is present decides which of the two the
    // reference asks for.
    if let Some((repository_name, digest)) = path.split_once(DIGEST_SEPARATOR) {
        if repository_name.is_empty() || digest.is_empty() {
            return None;
        }
        return Some(RegistryReference {
            region,
            repository_name: repository_name.to_string(),
            tag: None,
            digest: Some(digest.to_string()),
        });
    }
    let (repository_name, tag) = match path.rsplit_once(':') {
        Some((repository_name, tag)) if !repository_name.is_empty() && !tag.is_empty() => {
            (repository_name, tag.to_string())
        }
        Some(_) => return None,
        None => (path, DEFAULT_IMAGE_TAG.to_string()),
    };
    if repository_name.is_empty() {
        return None;
    }
    Some(RegistryReference {
        region,
        repository_name: repository_name.to_string(),
        tag: Some(tag),
        digest: None,
    })
}

/// The Region the registry host `host` names, when `host` is THIS registry's host: this
/// account's id, the two fixed labels, one Region label and the zone the registry is served
/// in — AWS's own partition suffix, or a name an operator points a client at.
///
/// The host layout itself is read by the one walk of it this service states
/// ([`registry_host_parts`]); what this adds is that the account has to be the account this
/// registry serves — a repository of another account's registry is not ours to pull from. It
/// is the one reading of "which Region did this client dial" every plane of this registry
/// takes, the data plane included (`crate::service`'s registry endpoint).
pub fn host_region(host: &str) -> Option<String> {
    let parts = registry_host_parts(host)?;
    (parts.registry_id == registry_id()).then(|| parts.region.to_ascii_lowercase())
}

/// Why no image in this registry answers to `reference`: the repository is not here at all,
/// or it is and holds nothing the reference names — the two sentences this registry already
/// words for a request that addresses either ([`repository_not_found_message`],
/// [`Repo::resolve_image`]).
fn not_found_message(
    store: &ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    reference: &RegistryReference,
) -> String {
    let identifier = reference.identifier();
    match store.repository(&reference.region, &reference.repository_name) {
        Err(_) => repository_not_found_message(&reference.repository_name),
        // Only reached for a reference the resolution above did NOT answer, so the
        // resolution here is what words why. A repository that resolves it after all is
        // worded as this registry words an identifier that names no image of it, so no
        // branch reports anything but a sentence.
        Ok(repository) => repository.resolve_image(&identifier).err().map_or_else(
            || not_within_repository("image", &repository.name, &repository.registry_id),
            |fault| fault.message().to_string(),
        ),
    }
}

impl PullPlane {
    /// The first of the calls a pull makes that no policy in force on `principal`
    /// permits, decided from these stores — and `None` when every one of them is
    /// permitted, or when nothing of the kind can be shown (see the module docs).
    fn withheld_pull(
        &self,
        reference: &RegistryReference,
        principal: Option<PullPrincipal<'_>>,
    ) -> Option<PullRefusal> {
        let decided = PreparedPrincipal::prepare(self, principal)?;
        // The document the repository SHARES itself with, which is the other way a pull is
        // allowed: a repository policy naming the role grants it with no identity-based grant
        // at all.
        let shared = self.registry.as_ref().and_then(|state| {
            let store = state.lock();
            store
                .repository(&reference.region, &reference.repository_name)
                .ok()
                .and_then(|repository| repository.policy_text.clone())
        });
        let repository = repository_arn(&reference.region, &reference.repository_name);
        // The registry-level call first, the way the agent makes them: a role that cannot
        // obtain a token never reaches a manifest. A registry-level action is granted on the
        // registry, which no repository policy is the document for.
        decided
            .withheld(GET_AUTHORIZATION_TOKEN_ACTION, ANY_RESOURCE, None)
            .or_else(|| decided.withheld(BATCH_GET_IMAGE_ACTION, &repository, shared.as_deref()))
    }
}

/// Why no policy in force on `principal` permits it the REGISTRY-level authorization token
/// a pull begins with ([`GET_AUTHORIZATION_TOKEN_ACTION`]), in AWS's own words, and `None`
/// when it is permitted — or when nothing of the kind can be shown (see the module docs).
///
/// This is the one call of a pull that addresses no repository: "A client can use the
/// returned token to access any Amazon ECR registry that the IAM principal has access to"
/// (`GetAuthorizationToken`), decided on [`ANY_RESOURCE`] and on the identity documents
/// alone. A compute service's agent obtains ONE token for the registry before it fetches any
/// manifest, so it asks this question once for a task however many of that task's images
/// come from this registry, and a principal that is refused the token has not reached an
/// image at all.
///
/// A READ: the stores are only consulted, so asking changes nothing a later caller sees.
pub fn withheld_registry_authorization<Eng: Send + Sync + 'static>(
    engine: &Eng,
    principal: Option<PullPrincipal<'_>>,
) -> Option<String> {
    PullPlane::of(engine).withheld_registry_authorization(principal)
}

/// The principal a pull is made as.
///
/// A compute service pulls as a ROLE it ASSUMED for the workload — "the task execution role
/// that grants the Amazon ECS container agent permission to make Amazon Web Services API
/// calls on your behalf" (`TaskDefinition$executionRoleArn`) — so the documents in force are
/// the role's while the caller AWS reports is the session, which is the same split every
/// other gate over an assumed role in this tree takes.
#[derive(Debug, Clone, Copy)]
pub struct PullPrincipal<'a> {
    /// The IAM ROLE whose identity-based documents are in force on the pull, and which a
    /// repository policy grants the pull by naming.
    pub role_arn: &'a str,
    /// What a refusal NAMES as the principal: the ARN of the SESSION the pulling service
    /// assumed that role in, which is what AWS reports for a call made under one. `None`
    /// names the role itself, for a caller that holds no session of its own.
    pub session_arn: Option<&'a str>,
}

impl<'a> PullPrincipal<'a> {
    /// The pull made as the role `role_arn` itself, with no session of its own to name.
    #[must_use]
    pub fn of_role(role_arn: &'a str) -> Self {
        Self {
            role_arn,
            session_arn: None,
        }
    }

    /// The ARN a refusal names as the principal (see [`Self::session_arn`]).
    fn reported_arn(&self) -> &'a str {
        self.session_arn.unwrap_or(self.role_arn)
    }
}

/// Everything the authorization of a pull is decided from about the PRINCIPAL it is made as:
/// the role, the identity documents in force on it, the ceiling its permissions boundary
/// sets, and every spelling a resource-based policy may name it by.
///
/// Prepared ONCE ([`PreparedPrincipal::prepare`]) and then asked about each call the pull
/// makes ([`PreparedPrincipal::withheld`]), so the IAM store is read one time however many of
/// them there are.
struct PreparedPrincipal<'role> {
    /// The principal the pull is made as, which a refusal names ([`PullPrincipal`]).
    principal: PullPrincipal<'role>,
    /// The identity documents in force on it, prepared for the decisions below.
    prepared: Vec<iam::PreparedDocument>,
    /// The permissions boundary's document, prepared the same way, and `None` for a role
    /// carrying none.
    capped: Option<Vec<iam::PreparedDocument>>,
    /// Every spelling a resource-based policy may name this pull's principal by: the session
    /// it is made in, the role whose permissions are in force, and the account each belongs to
    /// — read the way every other service in this tree reads them.
    principals: Vec<String>,
}

impl<'role> PreparedPrincipal<'role> {
    /// What the documents in force on `principal` decide a pull by, and `None` where nothing
    /// of the kind can be shown: a pull made with no principal, an engine carrying no IAM
    /// store, a role that store holds no record for, or one carrying an attachment whose
    /// document this account does not hold (see the module docs).
    fn prepare(plane: &PullPlane, principal: Option<PullPrincipal<'role>>) -> Option<Self> {
        let principal = principal?;
        let role_arn = principal.role_arn;
        let iam = plane.identities.clone()?;
        let (statements, boundary) = {
            let store = iam.lock();
            // A role this account's IAM holds no record for may belong to another account, so
            // its documents are not this store's to read.
            store.roles.values().find(|role| role.arn == role_arn)?;
            let policies = store.principal_policies(role_arn)?;
            // An attachment whose document this account does not hold — an entry of AWS's own
            // catalog — may be the very policy that carries the grant, so nothing is withheld
            // on it.
            if !policies.uncarried.is_empty() {
                return None;
            }
            (
                policies.documents,
                store.permissions_boundary_document(role_arn),
            )
        };
        let mut principals = vec![role_arn.to_string()];
        if let Some(session_arn) = principal.session_arn {
            principals.push(session_arn.to_string());
        }
        principals.extend(iam::account_principals(role_arn));
        Some(Self {
            principal,
            prepared: iam::prepare_documents(&statements),
            capped: boundary
                .map(|document| iam::prepare_documents(::std::slice::from_ref(&document))),
            principals,
        })
    }

    /// The refusal ONE call of a pull earns — `action` on `resource`, with `shared` the
    /// resource-based policy of the repository it addresses and `None` for a registry-level
    /// call no repository policy is the document for — and `None` when it is permitted.
    fn withheld(&self, action: &str, resource: &str, shared: Option<&str>) -> Option<PullRefusal> {
        let action = super::authorization::action_for(action);
        let request = iam::Request::new(&action, resource, &self.principals);
        let identity = iam::identity_decision_prepared(&self.prepared, &request);
        let stated = shared.map_or(Decision::NotStated, |document| {
            iam::resource_decision(document, &request)
        });
        let granted = iam::is_allowed(identity, stated);
        // A permissions boundary CAPS what the documents grant; it grants nothing itself,
        // and a call an explicit deny already refuses reads no ceiling.
        let refused_outright = identity == Decision::Denied || stated == Decision::Denied;
        let ceiling = if refused_outright {
            Decision::Allowed
        } else {
            self.capped.as_deref().map_or(Decision::Allowed, |boundary| {
                iam::identity_decision_prepared(boundary, &request)
            })
        };
        if granted && ceiling == Decision::Allowed {
            return None;
        }
        // Why the grant was withheld: the two documents' own clauses are this service's one
        // wording of them ([`super::authorization::withheld_because`]), and the ceiling's are
        // the shared evaluator's — so an explicit deny in the REPOSITORY's policy is reported
        // as that rather than as a missing identity grant.
        let reason = if ceiling == Decision::Denied {
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string()
        } else if !granted {
            super::authorization::withheld_because(identity, stated, &action)
        } else {
            iam::no_permissions_boundary_allows(&action)
        };
        // The refusal is the security-relevant event of this pull, so it is attributable in
        // the emulator's audit log the way this service's own gate logs a refused request:
        // who the pull was made as, what it asked for, and the outcome. The policy
        // documents themselves are not logged.
        ::tracing::info!(
            target: "emulator::audit",
            audit = true,
            operation = "ServeImagePull",
            outcome = "refused_access_denied",
            principal = %self.principal.reported_arn(),
            action = %action,
            resource = %resource,
            explicit_deny = refused_outright || ceiling == Decision::Denied,
            "no policy in force permits this role to pull this image"
        );
        Some(PullRefusal::NotAuthorized {
            message: iam::not_authorized_by_identity_policy(
                self.principal.reported_arn(),
                &action,
                resource,
                &reason,
            ),
            action,
        })
    }
}

/// The call a pull makes against the LAYERS of the image it resolved: "Retrieves the
/// pre-signed Amazon S3 download URL corresponding to an image layer"
/// (`GetDownloadUrlForLayer`), decided on the repository the reference names — the second
/// of the two repository-scoped calls a pull of a private image takes.
pub const GET_DOWNLOAD_URL_FOR_LAYER_ACTION: &str = "GetDownloadUrlForLayer";

/// The calls a pull makes on the REPOSITORY, in the order a client makes them: the manifest
/// first, then the layers it references.
pub const REPOSITORY_PULL_ACTIONS: [&str; 2] =
    [BATCH_GET_IMAGE_ACTION, GET_DOWNLOAD_URL_FOR_LAYER_ACTION];

/// The first of [`REPOSITORY_PULL_ACTIONS`] that the repository `repository_name`'s own
/// policy explicitly DENIES the SERVICE principal `principal`, as the fully qualified IAM
/// action (`ecr:<OperationName>`) — and `None` when its policy denies none of them.
///
/// A pull an AWS service makes as its OWN service principal is decided by the repository's
/// resource-based policy alone: a service principal is no principal of this account's IAM,
/// so no identity-based document is in force on it, and a service that is entitled to the
/// images of this account's repositories carries that entitlement itself. What a policy
/// therefore decides for such a pull is whether it REFUSES it — "even if multiple
/// statements allow the operation on the resource, if only one statement denies that
/// operation, then the explicit deny overrides any allow" — so a repository carrying no
/// policy, and one whose policy says nothing about this principal, deny nothing.
///
/// A READ: the store is only consulted, so deciding a pull changes nothing a later caller
/// sees. `region` names the repository's ARN, which is the resource the decision is taken
/// on.
#[must_use]
pub fn service_pull_denied_by_repository_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    repository_name: &str,
    principal: &str,
) -> Option<String> {
    let document = service_state(engine).and_then(|state| {
        let store = state.lock();
        store
            .repository(region, repository_name)
            .ok()
            .and_then(|repository| repository.policy_text.clone())
    })?;
    let principals = [principal.to_string()];
    let resource = repository_arn(region, repository_name);
    for action in REPOSITORY_PULL_ACTIONS {
        let action = super::authorization::action_for(action);
        let request = iam::Request::new(&action, &resource, &principals);
        if iam::resource_decision(&document, &request) != Decision::Denied {
            continue;
        }
        // The refusal is the security-relevant event of this pull, so it is attributable in
        // the emulator's audit log the way the role-borne pull above logs its own: who the
        // pull was made as, what it asked for, and the outcome. The policy document itself
        // is not logged.
        ::tracing::info!(
            target: "emulator::audit",
            audit = true,
            operation = "ServeImagePull",
            outcome = "refused_access_denied",
            principal = %principal,
            action = %action,
            resource = %resource,
            explicit_deny = true,
            "this repository's policy denies this service principal the pull"
        );
        return Some(action);
    }
    None
}

/// What a pull through cache rule answered for an image this registry does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheImport {
    /// Nothing was cached because nothing was to be: no rule of this registry covers the
    /// repository name, or the reference addresses a digest rather than a tag — an import
    /// has no upstream manifest here to match a digest against, so only a tag is cached.
    NotCovered,
    /// The upstream image was cached, under the digest this registry stores it at.
    Imported(String),
    /// A rule DOES cover the repository and the upstream cannot be reached THROUGH it:
    /// why, read off the stored rule ([`pull_through_cache_upstream_failure`]).
    Unavailable(String),
    /// A rule DOES cover the repository and this REGISTRY is at one of its own bounds, so
    /// nothing was cached: which bound, in this emulator's own words. It is a local limit
    /// rather than anything about the upstream, so a caller is told so.
    AtCapacity(String),
}

/// Import the image `tag` names in `repository_name` into the cache repository, when a pull
/// through cache rule of this registry covers that repository.
///
/// "A pull through cache rule provides a way to cache images from an upstream registry
/// source in your Amazon ECR private registry" (`CreatePullThroughCacheRule`): a repository
/// name under a registered rule's `ecrRepositoryPrefix` is served by caching the upstream
/// image, so the FIRST read of a reference under it CREATES the cache repository and stores
/// the image under the tag that was asked for, and every later read of that repository
/// reports it. This is the one place that import happens, so the API operations and the
/// registry endpoint cache one pull the same way.
///
/// [`CacheImport::Unavailable`] for a rule the upstream cannot be reached THROUGH
/// ([`pull_through_cache_upstream_failure`]), and [`CacheImport::AtCapacity`] for a registry
/// at a bound a cached copy would push it past: an import stores a repository, an image, a
/// tag and the blobs that image references, so it is held to the cap on each of them.
/// EVERY bound is checked before ANYTHING is written, so a refused import leaves the store
/// exactly as it found it — a client pulling tag after tag under a cache rule is bounded by
/// those caps rather than growing this process without end.
///
/// `region` names the cache repository's ARN, and `now` is the instant it and the image it
/// holds record as theirs.
pub fn import_covered_image(
    store: &mut ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    region: &str,
    repository_name: &str,
    tag: Option<&str>,
    now: SystemTime,
) -> CacheImport {
    let Some(rule) = covering_cache_rule(store, repository_name) else {
        return CacheImport::NotCovered;
    };
    // A pull addressing a DIGEST names content, and an import has none to match: the
    // upstream manifest is not here to be digested, so only a tag is cached.
    let Some(tag) = tag else {
        return CacheImport::NotCovered;
    };
    // The rule is what the upstream is reached THROUGH, so a rule whose stored credential
    // reference cannot be read caches nothing: "This is usually caused because of an issue
    // with the Secrets Manager secret containing the credentials for the upstream registry"
    // (`UnableToGetUpstreamImageException`), read off the rule by the one walk of it this
    // service states ([`pull_through_cache_upstream_failure`]).
    if let Some(failure) = pull_through_cache_upstream_failure(&rule) {
        return CacheImport::Unavailable(failure.reason);
    }
    let upstream = upstream_reference(&rule, repository_name, tag);
    match store.repository(region, repository_name) {
        // A cached copy the registry has no room for is not cached at all.
        Err(_) if store.repositories.len() >= MAX_REPOSITORIES => {
            return CacheImport::AtCapacity(format!(
                "this registry already holds the maximum of {MAX_REPOSITORIES} repositories, \
                 so no cache repository is created for '{repository_name}'"
            ))
        }
        Ok(repository)
            if repository.images.len() >= MAX_IMAGES_PER_REPOSITORY
                || repository.tag_index.len() >= MAX_TAGS_PER_REPOSITORY
                || repository.layer_blobs.len() >= MAX_IMPORTED_BLOBS_PER_REPOSITORY =>
        {
            return CacheImport::AtCapacity(format!(
                "the cache repository '{repository_name}' already holds as many images, tags \
                 or layers as this registry caches into one, so '{upstream}' is not cached"
            ))
        }
        _ => {}
    }
    let imported = imported_image(&upstream);
    let digest = manifest_digest(&imported.manifest);
    let config_digest = content_digest(imported.config.as_bytes());
    let layer_digest = content_digest(&imported.layer);
    let repository = store
        .repositories
        .entry(repository_key(region, repository_name))
        // The repository a first pull through the cache creates, with the settings a
        // repository this registry creates without a request carries: tags mutable, nothing
        // scanned on push, and the service-default encryption ([`Repo::created`]).
        .or_insert_with(|| Repo::created(region, repository_name, now));
    // The configuration document the imported manifest describes is held as the blob it
    // describes, under the digest of its own bytes, so the manifest references nothing the
    // repository does not have.
    repository.layer_blobs.insert(
        config_digest.clone(),
        LayerBlob {
            digest: config_digest,
            size: i64::try_from(imported.config.len()).ok(),
            media_type: Some(OCI_IMAGE_CONFIG_MEDIA_TYPE.to_string()),
            filesystem: LayerFilesystem::default(),
            content: imported.config.into_bytes(),
        },
    );
    // The FILESYSTEM LAYER the imported manifest declares is held the same way, under the
    // digest of its own bytes and with what READING those bytes identified — the record
    // `CompleteLayerUpload` writes for a layer a client pushed ([`scan_layer_filesystem`]) —
    // so a reader of the cached image, a basic scan included, sees the filesystem this
    // repository really holds rather than a claim about one.
    repository.layer_blobs.insert(
        layer_digest.clone(),
        LayerBlob {
            digest: layer_digest,
            size: i64::try_from(imported.layer.len()).ok(),
            media_type: Some(OCI_IMAGE_LAYER_MEDIA_TYPE.to_string()),
            filesystem: scan_layer_filesystem(&imported.layer),
            content: imported.layer,
        },
    );
    repository.images.entry(digest.clone()).or_insert_with(|| {
        ImageRecord::pushed(
            digest.clone(),
            imported.manifest,
            Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
            now,
        )
    });
    repository.move_tag(tag, &digest);
    repository.refresh_image_sizes();
    CacheImport::Imported(digest)
}

/// The pull through cache rule that covers `repository_name`: the one whose
/// `ecrRepositoryPrefix` is a leading NAMESPACE of the repository name — "the repository
/// name prefix to use when caching images from the source registry", with "always an
/// assumed `/` applied to the end of the prefix" — and the LONGEST such prefix when
/// several of them do.
fn covering_cache_rule(
    store: &ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    repository_name: &str,
) -> Option<PullThroughCacheRule> {
    store
        .pull_through_cache_rules
        .iter()
        .filter(|(prefix, _)| {
            repository_name
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, rule)| rule.clone())
}

/// The UPSTREAM reference the cached image came from: the upstream registry's URL, the
/// upstream repository prefix the rule matches with ("when this field isn't specified,
/// Amazon ECR will use the `ROOT`", which prepends nothing), the repository path under the
/// rule's own prefix, and the tag that was asked for.
fn upstream_reference(rule: &PullThroughCacheRule, repository_name: &str, tag: &str) -> String {
    let prefix = rule
        .ecr_repository_prefix
        .as_deref()
        .map(normalize_prefix)
        .unwrap_or_default();
    let path = repository_name
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(repository_name);
    let upstream_prefix = rule
        .upstream_repository_prefix
        .as_deref()
        .filter(|value| *value != DEFAULT_UPSTREAM_REPOSITORY_PREFIX)
        .map(|value| format!("{}/", normalize_prefix(value)))
        .unwrap_or_default();
    let host = rule
        .upstream_registry_url
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/');
    format!("{host}/{upstream_prefix}{path}:{tag}")
}

/// The UPSTREAM reference a pull of `reference` is served from, for a reference into a
/// repository of this registry that a pull through cache rule covers — and `None` for every
/// other reference, which names an image this registry holds as its own.
///
/// "A pull through cache rule provides a way to cache images from an upstream registry source
/// in your Amazon ECR private registry" (`CreatePullThroughCacheRule`), so the cache
/// repository under a rule's `ecrRepositoryPrefix` holds the upstream image itself: a
/// reference into it NAMES that image. A caller that reads anything off the image it pulled —
/// a container runtime reading the command the image declares for a container whose definition
/// names none — therefore has to read it off the upstream reference, which is what this
/// answers. The walk is the one the import itself resolves the upstream with
/// ([`import_covered_image`]), so one reference resolves to one upstream however it is asked
/// for.
///
/// A reference addressing a DIGEST is not answered: only a tag is cached, so a digest names
/// content of this registry rather than a release of an upstream repository.
///
/// A READ: the rules and nothing else decide it, and nothing is written — asking cannot change
/// what a later caller sees. Called with no guard of this store held: it takes the lock itself.
#[must_use]
pub fn cached_upstream_reference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    reference: &RegistryReference,
) -> Option<String> {
    let tag = reference.tag.as_deref()?;
    let state = service_state(engine)?;
    let store = state.lock();
    let rule = covering_cache_rule(&store, &reference.repository_name)?;
    Some(upstream_reference(&rule, &reference.repository_name, tag))
}

/// What an import stores for the cached image: the manifest this registry writes for it and
/// the bytes of each blob that manifest references.
struct ImportedImage {
    /// The image manifest, which the cached image is stored under the digest of.
    manifest: String,
    /// The configuration document the manifest's `config` descriptor addresses.
    config: String,
    /// The archive the manifest's one `layers` entry addresses.
    layer: Vec<u8>,
}

/// The image an import stores for the upstream reference `upstream`.
///
/// An OCI image manifest over ONE configuration document and ONE filesystem layer, each
/// descriptor's digest and size computed over the bytes beside it, so the cached image
/// references nothing the repository does not hold. The upstream image's own bytes are not
/// reachable from this process — this emulator makes no outbound connection — so what is
/// cached is this registry's own copy: the layer is the root filesystem it writes for a
/// cached image ([`cached_image_filesystem_layer`]), and the configuration document records
/// WHICH upstream reference was cached, in the annotation an OCI image carries its reference
/// name in.
fn imported_image(upstream: &str) -> ImportedImage {
    let config = serde_json::json!({
        "mediaType": OCI_IMAGE_CONFIG_MEDIA_TYPE,
        "annotations": { "org.opencontainers.image.ref.name": upstream },
    })
    .to_string();
    let layer = cached_image_filesystem_layer();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": OCI_IMAGE_CONFIG_MEDIA_TYPE,
            "digest": content_digest(config.as_bytes()),
            "size": config.len(),
        },
        "layers": [{
            "mediaType": OCI_IMAGE_LAYER_MEDIA_TYPE,
            "digest": content_digest(&layer),
            "size": layer.len(),
        }],
    })
    .to_string();
    ImportedImage {
        manifest,
        config,
        layer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference to a repository of this registry in `us-east-1`, in the form the Amazon
    /// ECR User Guide gives for a pull — the form a task definition carries.
    fn reference(path: &str) -> String {
        format!(
            "{}.dkr.ecr.us-east-1.amazonaws.com/{path}",
            registry_id()
        )
    }

    /// What a row expects [`parse_reference`] to read out of its reference.
    #[derive(Debug, PartialEq, Eq)]
    struct Named {
        repository: &'static str,
        tag: Option<&'static str>,
        digest: Option<&'static str>,
    }

    /// One row of the reference table: the case, the reference, and what it names — `None`
    /// for a reference that names no repository of THIS registry.
    struct Row {
        case: &'static str,
        image: String,
        names: Option<Named>,
    }

    #[test]
    fn a_reference_of_this_registry_names_its_region_repository_and_tag() {
        let tagged = |repository, tag| Named {
            repository,
            tag: Some(tag),
            digest: None,
        };
        let rows = vec![
            Row {
                case: "a tagged reference",
                image: reference("app:1"),
                names: Some(tagged("app", "1")),
            },
            Row {
                case: "a reference with no tag takes the default one",
                image: reference("app"),
                names: Some(tagged("app", DEFAULT_IMAGE_TAG)),
            },
            Row {
                case: "a namespaced repository",
                image: reference("docker-hub/library/busybox:latest"),
                names: Some(tagged("docker-hub/library/busybox", "latest")),
            },
            Row {
                case: "a reference addressing a digest",
                image: reference("app@sha256:abc"),
                names: Some(Named {
                    repository: "app",
                    tag: None,
                    digest: Some("sha256:abc"),
                }),
            },
            Row {
                case: "another account's registry",
                image: "123456789012.dkr.ecr.us-east-1.amazonaws.com/app:1".to_string(),
                names: None,
            },
            Row {
                // A registry-shaped host in ANOTHER partition's zone: AWS serves a Region's
                // registries only under its own partition's suffix, so no registry answers
                // there and the reference names none of this account's repositories.
                case: "this account and Region under another partition's zone",
                image: format!(
                    "{}.dkr.ecr.us-east-1.amazonaws.com.cn/app:1",
                    registry_id()
                ),
                names: None,
            },
            Row {
                case: "a public registry",
                image: "public.ecr.aws/nginx/nginx:latest".to_string(),
                names: None,
            },
            Row {
                case: "a bare name",
                image: "nginx:1.27".to_string(),
                names: None,
            },
        ];
        for row in &rows {
            let case = row.case;
            let parsed = parse_reference(&row.image);
            match (&row.names, parsed.as_ref()) {
                (Some(expected), Some(parsed)) => {
                    assert_eq!(
                        parsed.repository_name, expected.repository,
                        "case {case:?}: expected repository {:?}, got {:?}",
                        expected.repository, parsed.repository_name
                    );
                    assert_eq!(
                        parsed.tag.as_deref(),
                        expected.tag,
                        "case {case:?}: expected tag {:?}, got {:?}",
                        expected.tag,
                        parsed.tag
                    );
                    assert_eq!(
                        parsed.digest.as_deref(),
                        expected.digest,
                        "case {case:?}: expected digest {:?}, got {:?}",
                        expected.digest,
                        parsed.digest
                    );
                    assert_eq!(
                        parsed.region, "us-east-1",
                        "case {case:?}: expected the host's Region, got {:?}",
                        parsed.region
                    );
                }
                (None, None) => {}
                (expected, parsed) => panic!(
                    "case {case:?}: expected {expected:?} to be named, got {parsed:?}"
                ),
            }
        }
    }

    #[test]
    fn an_imported_image_states_the_digest_and_size_of_every_blob_it_holds() {
        let imported = imported_image("registry-1.docker.io/library/busybox:latest");
        let document: serde_json::Value =
            serde_json::from_str(&imported.manifest).expect("the imported manifest is JSON");
        assert_eq!(
            document["config"]["digest"].as_str(),
            Some(content_digest(imported.config.as_bytes()).as_str()),
            "case \"the configuration descriptor\": expected the digest of the stored bytes, \
             got {}",
            imported.manifest
        );
        assert_eq!(
            document["config"]["size"].as_u64(),
            u64::try_from(imported.config.len()).ok(),
            "case \"the configuration descriptor\": expected the length of the stored bytes, \
             got {}",
            imported.manifest
        );
        assert_eq!(
            document["layers"][0]["digest"].as_str(),
            Some(content_digest(&imported.layer).as_str()),
            "case \"the layer descriptor\": expected the digest of the stored bytes, got {}",
            imported.manifest
        );
        assert_eq!(
            document["layers"][0]["size"].as_u64(),
            u64::try_from(imported.layer.len()).ok(),
            "case \"the layer descriptor\": expected the length of the stored bytes, got {}",
            imported.manifest
        );
        assert!(
            imported
                .config
                .contains("registry-1.docker.io/library/busybox:latest"),
            "case \"the configuration document\": expected it to name the upstream reference, \
             got {}",
            imported.config
        );
    }

    #[test]
    fn the_layer_an_import_stores_is_a_filesystem_a_basic_scan_reads() {
        let imported = imported_image("public.ecr.aws/amazonlinux/amazonlinux:latest");
        let case = "the cached image's layer";
        assert!(
            is_scannable_image_manifest(&imported.manifest),
            "case {case:?}: expected the cached manifest to declare a filesystem layer, got {}",
            imported.manifest
        );
        let identified = scan_layer_filesystem(&imported.layer);
        assert!(
            identified.operating_system.is_some() && identified.package_manager.is_some(),
            "case {case:?}: expected the stored bytes to carry an operating-system release \
             file and a package database, got {identified:?}"
        );
    }

    #[test]
    fn the_upstream_reference_follows_the_rule_that_cached_the_image() {
        let rule = |upstream_prefix: Option<&str>| PullThroughCacheRule {
            ecr_repository_prefix: Some("hub".to_string()),
            upstream_registry_url: Some("registry-1.docker.io".to_string()),
            upstream_repository_prefix: upstream_prefix.map(ToString::to_string),
            ..PullThroughCacheRule::default()
        };
        // (case, the rule's upstreamRepositoryPrefix, the upstream reference)
        let rows: &[(&str, Option<&str>, &str)] = &[
            (
                "the documented default prepends nothing",
                Some(DEFAULT_UPSTREAM_REPOSITORY_PREFIX),
                "registry-1.docker.io/library/busybox:1",
            ),
            (
                "no prefix stated at all",
                None,
                "registry-1.docker.io/library/busybox:1",
            ),
            (
                "a stated prefix leads the upstream path",
                Some("mirror"),
                "registry-1.docker.io/mirror/library/busybox:1",
            ),
        ];
        for (case, upstream_prefix, expected) in rows {
            let actual = upstream_reference(&rule(*upstream_prefix), "hub/library/busybox", "1");
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn an_import_past_a_repository_cap_writes_nothing_at_all() {
        let repository_name = "hub/library/busybox";
        let mut store = ComAmazonawsEcrAmazonec2containerregistryV20150921State::default();
        store.pull_through_cache_rules.insert(
            "hub".to_string(),
            PullThroughCacheRule {
                ecr_repository_prefix: Some("hub".to_string()),
                upstream_registry_url: Some("registry-1.docker.io".to_string()),
                ..PullThroughCacheRule::default()
            },
        );
        let now = SystemTime::UNIX_EPOCH;
        let case = "the first pull of a covered reference";
        assert!(
            matches!(
                import_covered_image(&mut store, "us-east-1", repository_name, Some("1"), now),
                CacheImport::Imported(_)
            ),
            "case {case:?}: expected the upstream image to be cached"
        );

        // The repository at its blob cap: a further pull is the one this bound is about, a
        // client asking for tag after tag.
        let repository = store
            .repository_mut("us-east-1", repository_name)
            .expect("the cached repository");
        for index in 0..MAX_IMPORTED_BLOBS_PER_REPOSITORY {
            let digest = format!("sha256:{index:064x}");
            repository.layer_blobs.insert(
                digest.clone(),
                LayerBlob {
                    digest,
                    size: None,
                    media_type: None,
                    filesystem: LayerFilesystem::default(),
                    content: Vec::new(),
                },
            );
        }
        let held = (
            repository.layer_blobs.len(),
            repository.images.len(),
            repository.tag_index.len(),
        );

        let case = "a pull past the blob cap";
        assert!(
            matches!(
                import_covered_image(&mut store, "us-east-1", repository_name, Some("2"), now),
                CacheImport::AtCapacity(_)
            ),
            "case {case:?}: expected no image to be cached"
        );
        let repository = store
            .repository("us-east-1", repository_name)
            .expect("the cached repository");
        let after = (
            repository.layer_blobs.len(),
            repository.images.len(),
            repository.tag_index.len(),
        );
        assert_eq!(
            after, held,
            "case {case:?}: expected a refused import to write nothing, got {after:?} where \
             the repository held {held:?}"
        );
    }
}
