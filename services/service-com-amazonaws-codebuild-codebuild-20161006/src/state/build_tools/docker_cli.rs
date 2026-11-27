// SPDX-License-Identifier: CC0-1.0

//! The Docker CLI as a build environment provides it: the login, the image build and the
//! PUSH a build's commands make against this emulator's own container registry.
//!
//! # The rule this module implements
//!
//! The commands a container build runs are the ones AWS documents for pushing an image to
//! Amazon ECR and for building one FROM an image of a private registry, and each does what
//! the CLI documents it as doing:
//!
//! * `docker login --username AWS --password-stdin <registry>` authenticates to the registry
//!   it names; the password it is given has to be one that registry ISSUED
//!   ([`super::aws_cli`]), so a build that never asked for one — or one that presents
//!   anything else — is not logged in, and the push or pull that follows fails the way one
//!   with no credential fails;
//! * `docker build` composes an image out of the Dockerfile and the context it names
//!   ([`super::container_image`]) and tags it, so the image exists in the build's own
//!   environment and nowhere else yet; a Dockerfile whose `FROM` names an image this
//!   environment HOLDS composes an image built ON it, carrying that base's own layers;
//! * `docker info` reports what the daemon serving this build's commands says about itself,
//!   which is how AWS's own documented container build WAITS for that daemon before it builds
//!   anything — and a build environment that may run no daemon ([`super::docker_daemon`]) has
//!   none to report, so the command fails there the way the CLI's own does;
//! * `docker tag` gives an image another reference, which is what a push then addresses;
//! * `docker pull` DOWNLOADS the image its reference names out of that registry — the
//!   manifest the repository holds and every blob it names, through the registry's own data
//!   plane ([`super::registry_client`]) — so the environment then holds the very image the
//!   registry holds, and a pull no policy in force lets the build's role make downloads
//!   nothing;
//! * `docker push` UPLOADS an image to the repository its reference names: each blob
//!   through the registry's layer-upload calls and then the manifest through `PutImage`, so
//!   the repository really holds the image once the command has exited 0 — and a push the
//!   registry refused, or one no policy in force lets the build's role make (the role's own
//!   documents and the repository's own policy decided together, `super::build_tools`), exits
//!   non-zero with nothing uploaded.
//!
//! A reference is `[<registry>/]<repository>[:<tag>]`, and a push or a pull addresses the
//! registry its own reference names: this environment reaches the registries of THIS emulator,
//! and the LOGIN is what says a host is one of them — a login only records a registry whose
//! own authorization token it presented — so a transfer to or from a host nothing logged in to
//! is refused the way one with no credential is.
//!
//! Every other command of the CLI, and every form of these five that carries an option they
//! do not take, is an invocation this program performs nothing of — the session reports it as
//! a command it could not run.

use super::container_image::{self, LocalImage};
use super::registry_client;
use super::{
    answered, service_answered, BuildTools, DOCKER_FAILED, MAX_LOCAL_IMAGES, PROGRAM_SUCCEEDED,
};
use ::service_common::posix_shell_session::{ProgramFiles, ProgramOutcome};

/// The name a command line invokes the Docker CLI by.
pub const DOCKER_PROGRAM: &str = "docker";

/// The tag a reference that names none addresses, which is the CLI's own default.
const DEFAULT_TAG: &str = "latest";

/// The host a registry of this emulator is addressed at: Amazon ECR publishes a private
/// registry as `<account>.dkr.ecr.<region>.amazonaws.com`, so the Region of the registry a
/// push addresses is read out of the host it names.
const ECR_HOST_INFIX: &str = ".dkr.ecr.";

/// See [`ECR_HOST_INFIX`].
const ECR_HOST_SUFFIX: &str = ".amazonaws.com";

/// The command that asks the daemon about itself: "docker info: display system-wide
/// information". It takes no operand, and its one option (`--format`) decides the FORM of what
/// it writes, which this program performs nothing of.
const INFO_COMMAND: &str = "info";

/// What the CLI reports when there is no daemon serving it, in the CLI's own wording — which
/// is what a build environment that may run none answers ([`super::docker_daemon`]).
///
/// The CLI names the address it tried in that sentence; this environment's daemon is served in
/// process and listens nowhere, so there is no address to name.
const CANNOT_CONNECT_MESSAGE: &str = "Cannot connect to the Docker daemon. Is the docker daemon \
     running?";

/// The IAM actions a push performs, in the order the push performs them ("Actions defined by
/// Amazon Elastic Container Registry", IAM Service Authorization Reference, and the pushing
/// permissions the Amazon ECR User Guide lists for a repository).
const PUSH_ACTIONS: [&str; 4] = [
    "ecr:InitiateLayerUpload",
    "ecr:UploadLayerPart",
    "ecr:CompleteLayerUpload",
    "ecr:PutImage",
];

/// The IAM actions a pull performs on the repository it addresses, in the order the pull
/// performs them: the manifest it starts from and then the layers that manifest names (the
/// pulling permissions the Amazon ECR User Guide lists for a repository, beside the
/// registry-wide login the pull presents).
const PULL_ACTIONS: [&str; 2] = ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"];

/// What one `docker` command line asks for — `None` for every invocation this program
/// performs nothing of.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Requested {
    /// `login`: authenticate to a registry.
    Login {
        /// The registry the login names, as the command wrote it.
        registry: String,
        /// The password the command carries, and `None` for one that reads it from standard
        /// input.
        password: Option<String>,
    },
    /// `build`: compose an image from a Dockerfile and a context.
    Build {
        /// The references the built image is tagged with.
        tags: Vec<String>,
        /// The Dockerfile the image is built from.
        dockerfile: Option<String>,
        /// The `--build-arg` values the command carried, keyed by variable name: "set
        /// build-time variables", which take effect where the Dockerfile declares an `ARG`
        /// for them ([`container_image`]).
        build_arguments: ::std::collections::BTreeMap<String, String>,
        /// The context the image is built from.
        context: String,
    },
    /// `tag`: give an image another reference.
    Tag {
        /// The image being tagged.
        source: String,
        /// The reference it is given.
        target: String,
    },
    /// `push`: upload an image to the repository its reference names.
    Push {
        /// The image being pushed.
        reference: String,
    },
    /// `pull`: download the image a reference names out of the repository it names.
    Pull {
        /// The image being pulled.
        reference: String,
    },
    /// `info`: what the daemon serving these commands says about itself.
    Info,
}

/// What the words of a `docker` command line ask for.
///
/// The options read are the ones these five commands take and nothing else: a line carrying
/// any other option changes what the command does, so it is an invocation this program
/// performs nothing of.
fn requested(arguments: &[String]) -> Option<Requested> {
    let mut words = arguments.iter().map(String::as_str);
    let command = words.next()?;
    if command == INFO_COMMAND {
        // The command takes nothing at all, so a line carrying any further word is one this
        // program performs nothing of ([`INFO_COMMAND`]).
        return words.next().is_none().then_some(Requested::Info);
    }
    let mut operands: Vec<&str> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    let mut dockerfile: Option<String> = None;
    let mut build_arguments: ::std::collections::BTreeMap<String, String> =
        ::std::collections::BTreeMap::new();
    let mut password: Option<String> = None;
    let mut password_stdin = false;
    let mut username: Option<String> = None;
    while let Some(word) = words.next() {
        match word {
            "-t" | "--tag" => tags.push(words.next()?.to_string()),
            "-f" | "--file" => dockerfile = Some(words.next()?.to_string()),
            "-u" | "--username" => username = Some(words.next()?.to_string()),
            "-p" | "--password" => password = Some(words.next()?.to_string()),
            "--build-arg" => {
                let (name, value) = build_argument(words.next()?)?;
                build_arguments.insert(name, value);
            }
            "--password-stdin" => password_stdin = true,
            option if option.starts_with('-') => {
                // An option written with its value attached (`--tag=x`), which is the same
                // option; every other option is one these commands do not take.
                let (name, value) = option.split_once('=')?;
                match name {
                    "-t" | "--tag" => tags.push(value.to_string()),
                    "-f" | "--file" => dockerfile = Some(value.to_string()),
                    "-u" | "--username" => username = Some(value.to_string()),
                    "-p" | "--password" => password = Some(value.to_string()),
                    "--build-arg" => {
                        let (name, value) = build_argument(value)?;
                        build_arguments.insert(name, value);
                    }
                    _ => return None,
                }
            }
            operand => operands.push(operand),
        }
    }
    match command {
        "login" => {
            // The login has to name WHO is logging in and WITH WHAT: the documented login
            // takes the user and reads the password from standard input.
            username?;
            if password.is_some() == password_stdin {
                return None;
            }
            let [registry] = operands[..] else {
                return None;
            };
            Some(Requested::Login {
                registry: registry.to_string(),
                password,
            })
        }
        "build" => {
            let [context] = operands[..] else {
                return None;
            };
            Some(Requested::Build {
                tags,
                dockerfile,
                build_arguments,
                context: context.to_string(),
            })
        }
        "tag" => {
            let [source, target] = operands[..] else {
                return None;
            };
            Some(Requested::Tag {
                source: source.to_string(),
                target: target.to_string(),
            })
        }
        "push" => {
            let [reference] = operands[..] else {
                return None;
            };
            Some(Requested::Push {
                reference: reference.to_string(),
            })
        }
        "pull" => {
            let [reference] = operands[..] else {
                return None;
            };
            Some(Requested::Pull {
                reference: reference.to_string(),
            })
        }
        _ => None,
    }
}

/// The variable and the value one `--build-arg` word names: "`--build-arg <varname>=<value>`".
///
/// `None` for a word naming no value, which takes the variable's value from the environment
/// the builder itself runs in — an environment this program is handed none of, so that form is
/// not performed rather than resolved against something else.
fn build_argument(written: &str) -> Option<(String, String)> {
    let (name, value) = written.split_once('=')?;
    (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
}

/// Perform the invocation `arguments` asks for, or answer `None` for one this program
/// performs nothing of.
pub(super) fn performed(
    tools: &BuildTools,
    arguments: &[String],
    standard_input: Option<&str>,
    files: &mut dyn ProgramFiles,
) -> Option<ProgramOutcome> {
    Some(match requested(arguments)? {
        Requested::Login { registry, password } => {
            // The password a login reads from standard input is what the stage before it
            // wrote, without the newline that stage ended it with.
            let presented = password.unwrap_or_else(|| standard_input.unwrap_or("").to_string());
            logged_in(tools, &registry, presented.trim())
        }
        Requested::Build {
            tags,
            dockerfile,
            build_arguments,
            context,
        } => built(
            tools,
            &tags,
            dockerfile.as_deref(),
            &build_arguments,
            &context,
            files,
        ),
        Requested::Tag { source, target } => tagged(tools, &source, &target),
        Requested::Push { reference } => pushed(tools, &reference),
        Requested::Pull { reference } => pulled(tools, &reference),
        Requested::Info => information(tools),
    })
}

/// `docker login`: record this build as logged in to `registry` when `presented` is a
/// password that registry issued it.
fn logged_in(tools: &BuildTools, registry: &str, presented: &str) -> ProgramOutcome {
    if presented.is_empty() {
        return tools.refused(
            DOCKER_PROGRAM,
            "Error: Cannot perform an interactive login from a non TTY device",
            DOCKER_FAILED,
        );
    }
    if !tools.state().issued_passwords.contains(presented) {
        // The registry never issued this credential, so it is not one it honours.
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "Error response from daemon: login attempt to https://{} failed with status: \
                 401 Unauthorized",
                registry_host(registry)
            ),
            DOCKER_FAILED,
        );
    }
    tools
        .state()
        .logins
        .insert(registry_host(registry).to_string(), presented.to_string());
    answered(PROGRAM_SUCCEEDED, vec!["Login Succeeded".to_string()])
}

/// `docker info`: what the daemon serving this build's commands says about itself.
///
/// Whether there IS such a daemon is what the build environment's `privilegedMode` decides —
/// that member "enables running the Docker daemon inside a Docker container" — so a build
/// environment that was not started in that mode answers the CLI's own cannot-connect failure,
/// exactly as the daemon's own program refuses to start there ([`super::docker_daemon`]). That
/// is what makes this command the readiness probe AWS's documented container buildspec waits on
/// before it builds anything.
///
/// What is reported is what this environment KNOWS about itself and nothing else: it runs no
/// container, and the images it holds are the ones its own commands composed or pulled.
fn information(tools: &BuildTools) -> ProgramOutcome {
    if !tools.privileged {
        return tools.refused(DOCKER_PROGRAM, CANNOT_CONNECT_MESSAGE, DOCKER_FAILED);
    }
    let images = tools.state().images.len();
    answered(
        PROGRAM_SUCCEEDED,
        vec!["Containers: 0".to_string(), format!("Images: {images}")],
    )
}

/// `docker build`: compose the image the Dockerfile and the context name, ON the base that
/// Dockerfile's own `FROM` names where this environment holds it, and hold the result under
/// each reference the command tags it with.
fn built(
    tools: &BuildTools,
    tags: &[String],
    dockerfile: Option<&str>,
    build_arguments: &::std::collections::BTreeMap<String, String>,
    context: &str,
    files: &dyn ProgramFiles,
) -> ProgramOutcome {
    let dockerfile = dockerfile.map_or_else(
        || match context.trim_end_matches('/') {
            "" | "." => container_image::DEFAULT_DOCKERFILE.to_string(),
            held => format!("{held}/{}", container_image::DEFAULT_DOCKERFILE),
        },
        str::to_string,
    );
    // The image the build is built ON: the one this environment HOLDS under the reference the
    // document's `FROM` names — which a `docker pull` is what puts there. Two cases answer
    // `None` here and they are different in kind: a `FROM` naming no image of a registry
    // (`scratch`, which IS no base, and a managed image this environment holds nothing of),
    // and a registry reference nothing pulled. Neither is a refusal, because nothing in this
    // emulator fetches an image off it ([`super::container_image`]): the build then carries
    // its own context as its single layer.
    let base = container_image::declared_base(files, &dockerfile, build_arguments)
        .as_deref()
        .and_then(Reference::parse)
        .and_then(|base| tools.state().images.get(&base.held()).cloned());
    let image = match container_image::composed(
        files,
        context,
        &dockerfile,
        build_arguments,
        base.as_ref(),
    ) {
        Ok(image) => image,
        Err(detail) => return tools.refused(DOCKER_PROGRAM, &detail, DOCKER_FAILED),
    };
    let digest = container_image::content_digest(image.manifest().as_bytes());
    let mut written = vec![format!(
        "Successfully built {}",
        short_digest(&digest)
    )];
    {
        let mut state = tools.state();
        for tag in tags {
            let Some(reference) = Reference::parse(tag) else {
                return tools.refused(
                    DOCKER_PROGRAM,
                    &format!("invalid reference format: {tag}"),
                    DOCKER_FAILED,
                );
            };
            if state.images.len() >= MAX_LOCAL_IMAGES && !state.images.contains_key(&reference.held())
            {
                return tools.refused(
                    DOCKER_PROGRAM,
                    &format!(
                        "no space left on device: a build composes at most {MAX_LOCAL_IMAGES} \
                         images"
                    ),
                    DOCKER_FAILED,
                );
            }
            state.images.insert(reference.held(), image.clone());
            written.push(format!("Successfully tagged {}", reference.held()));
        }
    }
    answered(PROGRAM_SUCCEEDED, written)
}

/// `docker tag`: give the image `source` names the reference `target`.
fn tagged(tools: &BuildTools, source: &str, target: &str) -> ProgramOutcome {
    let (Some(source), Some(target)) = (Reference::parse(source), Reference::parse(target)) else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!("invalid reference format: {source} or {target}"),
            DOCKER_FAILED,
        );
    };
    let mut state = tools.state();
    let Some(image) = state.images.get(&source.held()).cloned() else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "Error response from daemon: No such image: {}",
                source.held()
            ),
            DOCKER_FAILED,
        );
    };
    if state.images.len() >= MAX_LOCAL_IMAGES && !state.images.contains_key(&target.held()) {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!("no space left on device: a build composes at most {MAX_LOCAL_IMAGES} images"),
            DOCKER_FAILED,
        );
    }
    state.images.insert(target.held(), image);
    answered(PROGRAM_SUCCEEDED, Vec::new())
}

/// `docker push`: upload the image `reference` names to the repository of the registry it
/// names.
fn pushed(tools: &BuildTools, reference: &str) -> ProgramOutcome {
    let Some(reference) = Reference::parse(reference) else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!("invalid reference format: {reference}"),
            DOCKER_FAILED,
        );
    };
    let Some(image) = tools.state().images.get(&reference.held()).cloned() else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "An image does not exist locally with the tag: {}",
                reference.image()
            ),
            DOCKER_FAILED,
        );
    };
    let Some(registry) = reference.registry.clone() else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "Error response from daemon: push access denied for {}: this build environment \
                 reaches no registry outside this emulator",
                reference.repository
            ),
            DOCKER_FAILED,
        );
    };
    if !tools.state().logins.contains_key(&registry) {
        // A push to Amazon ECR presents the credential a login recorded; a daemon holding
        // none for the registry has nothing to present. It is also what says the registry
        // the push addresses is one of THIS emulator's: a login only records a registry
        // whose own authorization token it presented ([`logged_in`]).
        return tools.refused(
            DOCKER_PROGRAM,
            &format!("no basic auth credentials for {registry}"),
            DOCKER_FAILED,
        );
    }
    // WHICH registry: the Region the host names when it is one Amazon ECR publishes, and
    // otherwise the Region the build itself runs in — the endpoint this emulator serves its
    // registries at names no Region, and one registry of this account serves each.
    let region = registry_region(&registry).unwrap_or_else(|| tools.region.clone());
    let repository_arn = BuildTools::repository_arn(&region, &reference.repository);
    // A repository is reached only where a policy in force allows it, and TWO documents can
    // allow one: the role's own identity-based policies and the repository's own
    // resource-based policy, which is what shares a repository with a principal that has no
    // identity-based grant — and what withholds it from one that has, when a statement of it
    // denies. The document is read once, through the service that owns it, and then decides
    // every action the push performs.
    let shared = tools.repository_policy(&region, &reference.repository);
    for action in PUSH_ACTIONS {
        if let Some(denial) = tools.withheld_with_shared(action, &repository_arn, shared.as_ref()) {
            return tools.refused(DOCKER_PROGRAM, &format!("denied: {denial}"), DOCKER_FAILED);
        }
    }
    match uploaded(tools, &region, &reference, &image) {
        Ok(digest) => {
            let mut written = vec![format!(
                "The push refers to repository [{registry}/{}]",
                reference.repository
            )];
            // One line per layer the push sent, as the CLI reports each.
            written.extend(
                image
                    .layer_blobs()
                    .iter()
                    .map(|(_blob, digest)| format!("{}: Pushed", short_digest(digest))),
            );
            written.push(format!(
                "{}: digest: {digest} size: {}",
                reference.tag,
                image.manifest().len()
            ));
            answered(PROGRAM_SUCCEEDED, written)
        }
        Err(detail) => tools.refused(DOCKER_PROGRAM, &detail, DOCKER_FAILED),
    }
}

/// `docker pull`: download the image `reference` names out of the repository of the registry
/// it names, and hold it under that reference.
fn pulled(tools: &BuildTools, reference: &str) -> ProgramOutcome {
    let Some(reference) = Reference::parse(reference) else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!("invalid reference format: {reference}"),
            DOCKER_FAILED,
        );
    };
    let Some(registry) = reference.registry.clone() else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "Error response from daemon: pull access denied for {}: this build environment \
                 reaches no registry outside this emulator",
                reference.repository
            ),
            DOCKER_FAILED,
        );
    };
    // A pull from a private registry presents the credential a login recorded, which is also
    // what says the registry it addresses is one of THIS emulator's ([`logged_in`]).
    let Some(password) = tools.state().logins.get(&registry).cloned() else {
        return tools.refused(
            DOCKER_PROGRAM,
            &format!(
                "Error response from daemon: pull access denied for {}, repository does not \
                 exist or may require 'docker login'",
                reference.image()
            ),
            DOCKER_FAILED,
        );
    };
    // WHICH registry: the Region the host names when it is one Amazon ECR publishes, and
    // otherwise the Region the build itself runs in — the same reading a push takes of the
    // host it addresses.
    let region = registry_region(&registry).unwrap_or_else(|| tools.region.clone());
    let repository_arn = BuildTools::repository_arn(&region, &reference.repository);
    // The two documents that can allow a pull of this repository decide it together, exactly
    // as they decide a push: the role's own identity-based policies and the repository's own
    // resource-based policy, read once through the service that owns it.
    let shared = tools.repository_policy(&region, &reference.repository);
    for action in PULL_ACTIONS {
        if let Some(denial) = tools.withheld_with_shared(action, &repository_arn, shared.as_ref()) {
            return tools.refused(DOCKER_PROGRAM, &format!("denied: {denial}"), DOCKER_FAILED);
        }
    }
    // What this environment may still hold: the image a pull delivers is held for the length
    // of the build, so a pull past that bound is refused rather than fetched.
    {
        let state = tools.state();
        if state.images.len() >= MAX_LOCAL_IMAGES && !state.images.contains_key(&reference.held())
        {
            return tools.refused(
                DOCKER_PROGRAM,
                &format!("no space left on device: a build holds at most {MAX_LOCAL_IMAGES} images"),
                DOCKER_FAILED,
            );
        }
    }
    let pulled = match registry_client::fetched(
        tools,
        &registry,
        &password,
        &reference.repository,
        &reference.tag,
    ) {
        Ok(pulled) => pulled,
        Err(detail) => return tools.refused(DOCKER_PROGRAM, &detail, DOCKER_FAILED),
    };
    let mut written = vec![format!(
        "{}: Pulling from {}",
        reference.tag, reference.repository
    )];
    written.extend(
        pulled
            .image
            .layer_blobs()
            .iter()
            .map(|(_blob, digest)| format!("{}: Pull complete", short_digest(digest))),
    );
    written.push(format!("Digest: {}", pulled.digest));
    written.push(format!(
        "Status: Downloaded newer image for {}",
        reference.held()
    ));
    tools.state().images.insert(reference.held(), pulled.image);
    answered(PROGRAM_SUCCEEDED, written)
}

/// Upload `image`'s blobs and then its manifest, and answer the digest the registry stored
/// the image under.
///
/// What the repository ALREADY holds is not sent again — a blob it holds, and the very image
/// under the very reference this push addresses — because that is what the registry's own
/// refusal of each says, and a push with nothing left to send has delivered what it
/// addressed. Every other refusal is the push's failure.
fn uploaded(
    tools: &BuildTools,
    region: &str,
    reference: &Reference,
    image: &LocalImage,
) -> Result<String, String> {
    // Every blob the manifest references has to be in the repository before the manifest is
    // pushed: "after you upload the image layers, the PutImage API is called once".
    for (blob, _digest) in ::std::iter::once(image.config_blob()).chain(image.layer_blobs()) {
        pushed_blob(tools, region, &reference.repository, blob)?;
    }
    tools.made(|| async {
        match tools
            .ecr(region)
            .put_image()
            .repository_name(&reference.repository)
            .image_manifest(image.manifest())
            .image_manifest_media_type(container_image::MANIFEST_MEDIA_TYPE)
            .image_tag(&reference.tag)
            .send()
            .await
        {
            Ok(answer) => answer
                .image()
                .and_then(|image| image.image_id())
                .and_then(|id| id.image_digest.clone())
                .ok_or_else(|| "The PutImage operation answered no image digest".to_string()),
            // The repository ALREADY holds this very image under this very reference, which
            // is what the registry's refusal of the manifest says — the same reading the
            // layers get ([`pushed_blob`]): there was nothing left for this push to send. What
            // the repository SERVES for the reference is what says so, so it is read back
            // before the push reports it delivered ([`delivered`]); a reference holding
            // another image is a different refusal and the push fails on that.
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service| service.is_image_already_exists_exception()) =>
            {
                delivered(tools, region, reference, image).await
            }
            Err(error) => Err(service_answered("PutImage", &error)),
        }
    })
}

/// The digest the repository SERVES for `reference`, when that is `image`'s own — the read
/// that confirms a push the registry answered had nothing to send really delivered what the
/// command addressed.
///
/// `Err` carries what the command reports instead: the read's own refusal, and a reference the
/// repository resolves to another image, which is not the image this push addressed.
async fn delivered(
    tools: &BuildTools,
    region: &str,
    reference: &Reference,
    image: &LocalImage,
) -> Result<String, String> {
    let digest = container_image::content_digest(image.manifest().as_bytes());
    let served = tools
        .ecr(region)
        .batch_get_image()
        .repository_name(&reference.repository)
        .image_ids(
            ::aws_sdk_ecr::types::ImageIdentifier::builder()
                .image_tag(&reference.tag)
                .build(),
        )
        .send()
        .await
        .map_err(|error| service_answered("BatchGetImage", &error))?;
    let held = served
        .images()
        .first()
        .and_then(|held| held.image_id())
        .and_then(|id| id.image_digest.clone());
    if held.as_deref() == Some(digest.as_str()) {
        return Ok(digest);
    }
    Err(format!(
        "Error response from daemon: the repository {} holds {} under {}, not the image this \
         push addressed",
        reference.repository,
        held.unwrap_or_else(|| "no image".to_string()),
        reference.tag
    ))
}

/// Upload ONE blob of an image into `repository`, and answer the digest the registry
/// addresses it by.
///
/// A blob the repository ALREADY holds is one the push does not need to send again, which is
/// what the registry's own refusal of a completed upload says; every other refusal is the
/// push's failure.
fn pushed_blob(
    tools: &BuildTools,
    region: &str,
    repository: &str,
    blob: &[u8],
) -> Result<String, String> {
    let digest = container_image::content_digest(blob);
    tools.made(|| async {
        let client = tools.ecr(region);
        let upload = client
            .initiate_layer_upload()
            .repository_name(repository)
            .send()
            .await
            .map_err(|error| service_answered("InitiateLayerUpload", &error))?;
        let upload_id = upload
            .upload_id()
            .ok_or_else(|| {
                "The InitiateLayerUpload operation answered no upload id".to_string()
            })?
            .to_string();
        let last_byte = i64::try_from(blob.len())
            .map_err(|_| "The blob is larger than one upload carries".to_string())?
            - 1;
        client
            .upload_layer_part()
            .repository_name(repository)
            .upload_id(&upload_id)
            .part_first_byte(0)
            .part_last_byte(last_byte)
            .layer_part_blob(::aws_sdk_ecr::primitives::Blob::new(blob.to_vec()))
            .send()
            .await
            .map_err(|error| service_answered("UploadLayerPart", &error))?;
        match client
            .complete_layer_upload()
            .repository_name(repository)
            .upload_id(&upload_id)
            .layer_digests(&digest)
            .send()
            .await
        {
            Ok(completed) => Ok(completed
                .layer_digest()
                .unwrap_or(digest.as_str())
                .to_string()),
            Err(error) => {
                if error.as_service_error().is_some_and(|service| {
                    service.is_layer_already_exists_exception()
                }) {
                    Ok(digest.clone())
                } else {
                    Err(service_answered("CompleteLayerUpload", &error))
                }
            }
        }
    })
}

/// The first twelve hex characters of `digest`, which is how the CLI shows an image id.
fn short_digest(digest: &str) -> String {
    let hex = digest.split_once(':').map_or(digest, |(_, hex)| hex);
    hex.chars().take(12).collect()
}

/// The HOST half of what a login names: a login takes a registry address, which may be
/// written with a scheme and a trailing path, and the host is what a push then addresses.
fn registry_host(registry: &str) -> &str {
    let held = registry
        .trim()
        .strip_prefix("https://")
        .or_else(|| registry.trim().strip_prefix("http://"))
        .unwrap_or_else(|| registry.trim());
    held.split('/').next().unwrap_or(held)
}

/// The Region of the registry `host` addresses, or `None` when the host names no Region:
/// this emulator's own endpoint, which serves every Region's registry, or a host that is no
/// Amazon ECR registry host at all.
///
/// A private registry's published host is `<account>.dkr.ecr.<region>.amazonaws.com`, so a
/// reference written on one names the Region its repository lives in; a reference written on
/// the endpoint this emulator announces its registries at ([`logged_in`] is what records that
/// the registry there is one of this emulator's) names none, and the build's own Region is
/// then the one its push addresses.
fn registry_region(host: &str) -> Option<String> {
    let (_, after) = host.split_once(ECR_HOST_INFIX)?;
    let region = after.strip_suffix(ECR_HOST_SUFFIX)?;
    (!region.is_empty() && !region.contains('.')).then(|| region.to_string())
}

/// An image REFERENCE: `[<registry>/]<repository>[:<tag>]`, as a command line writes one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reference {
    /// The registry the reference names, and `None` for one that names none.
    registry: Option<String>,
    /// The repository path within that registry.
    repository: String,
    /// The tag, which is [`DEFAULT_TAG`] for a reference that names none.
    tag: String,
}

impl Reference {
    /// The reference `written` names, or `None` when it is not a reference at all.
    ///
    /// The first component is the REGISTRY when it looks like a host — "if the first part
    /// contains a `.` or `:`, or is `localhost`, it is interpreted as a registry" (the
    /// distribution reference grammar) — and part of the repository path otherwise.
    fn parse(written: &str) -> Option<Self> {
        let written = written.trim();
        if written.is_empty() || written.contains(char::is_whitespace) {
            return None;
        }
        let (registry, path) = match written.split_once('/') {
            Some((first, rest))
                if first.contains('.') || first.contains(':') || first == "localhost" =>
            {
                (Some(first.to_string()), rest)
            }
            _ => (None, written),
        };
        // A tag is written after the LAST path component, so a colon inside an earlier one
        // (a port in a registry host) is not one.
        let (repository, tag) = match path.rsplit_once(':') {
            Some((repository, tag)) if !tag.contains('/') => (repository, tag),
            _ => (path, DEFAULT_TAG),
        };
        if repository.is_empty() || tag.is_empty() {
            return None;
        }
        Some(Self {
            registry,
            repository: repository.to_string(),
            tag: tag.to_string(),
        })
    }

    /// The reference as the environment HOLDS the image under: the whole reference with its
    /// tag stated, so `<repo>` and `<repo>:latest` are the one image.
    fn held(&self) -> String {
        format!("{}:{}", self.image(), self.tag)
    }

    /// The image half of the reference — the registry and the repository, without the tag.
    fn image(&self) -> String {
        match &self.registry {
            Some(registry) => format!("{registry}/{}", self.repository),
            None => self.repository.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a reference is `[<registry>/]<repository>[:<tag>]`, where the first component is
    /// the registry only when it looks like a host, and a reference that names no tag names
    /// `latest`.
    ///
    /// The rows differ along what the rule generalises over: whether a registry is written,
    /// whether a tag is written, how deep the repository path is, and whether the host
    /// carries a port.
    #[test]
    fn a_reference_names_a_registry_a_repository_and_a_tag() {
        // (case, written, registry, repository, tag)
        let rows: Vec<(&str, &str, Option<&str>, &str, &str)> = vec![
            ("a bare repository", "app", None, "app", "latest"),
            ("a repository and a tag", "app:v1", None, "app", "v1"),
            (
                "a namespaced repository",
                "team/app:v1",
                None,
                "team/app",
                "v1",
            ),
            (
                "an Amazon ECR registry",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com/app:v2",
                Some("000000000000.dkr.ecr.us-west-2.amazonaws.com"),
                "app",
                "v2",
            ),
            (
                "an Amazon ECR registry with no tag",
                "000000000000.dkr.ecr.eu-west-1.amazonaws.com/team/app",
                Some("000000000000.dkr.ecr.eu-west-1.amazonaws.com"),
                "team/app",
                "latest",
            ),
            (
                "a host carrying a port",
                "localhost:5000/app:v3",
                Some("localhost:5000"),
                "app",
                "v3",
            ),
        ];
        for (case, written, registry, repository, tag) in rows {
            let parsed = Reference::parse(written)
                .unwrap_or_else(|| panic!("case {case:?}: expected a reference for {written}"));
            assert_eq!(
                (
                    parsed.registry.as_deref(),
                    parsed.repository.as_str(),
                    parsed.tag.as_str()
                ),
                (registry, repository, tag),
                "case {case:?}: expected {registry:?} {repository} {tag}, got {parsed:?}"
            );
        }
        for (case, written) in [
            ("nothing at all", ""),
            ("a reference carrying a space", "app :v1"),
            ("an empty tag", "app:"),
        ] {
            assert!(
                Reference::parse(written).is_none(),
                "case {case:?}: expected no reference for {written:?}"
            );
        }
    }

    /// RULE: only the commands this environment performs are performed, in the forms
    /// they take.
    ///
    /// The rows differ along what the rule generalises over: which command is named, how its
    /// options are written, and which forms carry an option or an operand count the command
    /// does not take.
    #[test]
    fn only_the_performed_docker_commands_are_performed() {
        // (case, the words, what they ask for)
        let rows: Vec<(&str, Vec<&str>, Option<Requested>)> = vec![
            (
                "the documented login",
                vec![
                    "login",
                    "--username",
                    "AWS",
                    "--password-stdin",
                    "registry.example",
                ],
                Some(Requested::Login {
                    registry: "registry.example".to_string(),
                    password: None,
                }),
            ),
            (
                "a login carrying its password",
                vec!["login", "-u", "AWS", "-p", "secret", "registry.example"],
                Some(Requested::Login {
                    registry: "registry.example".to_string(),
                    password: Some("secret".to_string()),
                }),
            ),
            (
                "a login naming no user",
                vec!["login", "--password-stdin", "registry.example"],
                None,
            ),
            (
                "a login naming two password sources",
                vec![
                    "login",
                    "-u",
                    "AWS",
                    "-p",
                    "secret",
                    "--password-stdin",
                    "registry.example",
                ],
                None,
            ),
            (
                "a build with one tag",
                vec!["build", "-t", "app:v1", "."],
                Some(Requested::Build {
                    tags: vec!["app:v1".to_string()],
                    dockerfile: None,
                    build_arguments: ::std::collections::BTreeMap::new(),
                    context: ".".to_string(),
                }),
            ),
            (
                "a build with two tags and a Dockerfile",
                vec![
                    "build",
                    "--tag=app:v1",
                    "-t",
                    "app:latest",
                    "-f",
                    "docker/Dockerfile",
                    "src",
                ],
                Some(Requested::Build {
                    tags: vec!["app:v1".to_string(), "app:latest".to_string()],
                    dockerfile: Some("docker/Dockerfile".to_string()),
                    build_arguments: ::std::collections::BTreeMap::new(),
                    context: "src".to_string(),
                }),
            ),
            (
                // "--build-arg <varname>=<value>": the values the Dockerfile's own `ARG`
                // declarations take, in both spellings the option is written in.
                "a build carrying build arguments",
                vec![
                    "build",
                    "--build-arg",
                    "BODY=a b c",
                    "--build-arg=TAG=v2",
                    "-t",
                    "app:v2",
                    ".",
                ],
                Some(Requested::Build {
                    tags: vec!["app:v2".to_string()],
                    dockerfile: None,
                    build_arguments: [("BODY", "a b c"), ("TAG", "v2")]
                        .into_iter()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                    context: ".".to_string(),
                }),
            ),
            (
                // A `--build-arg` naming no value takes it from the environment the builder
                // itself runs in, which this program is handed none of.
                "a build argument with no value",
                vec!["build", "--build-arg", "BODY", "-t", "app:v2", "."],
                None,
            ),
            (
                "a build naming no context",
                vec!["build", "-t", "app:v1"],
                None,
            ),
            (
                "a build carrying an option this program does not take",
                vec!["build", "--no-cache", "-t", "app:v1", "."],
                None,
            ),
            (
                "a tag",
                vec!["tag", "app:v1", "registry.example/app:v1"],
                Some(Requested::Tag {
                    source: "app:v1".to_string(),
                    target: "registry.example/app:v1".to_string(),
                }),
            ),
            (
                "a push",
                vec!["push", "registry.example/app:v1"],
                Some(Requested::Push {
                    reference: "registry.example/app:v1".to_string(),
                }),
            ),
            (
                "a pull",
                vec!["pull", "registry.example/app:v1"],
                Some(Requested::Pull {
                    reference: "registry.example/app:v1".to_string(),
                }),
            ),
            (
                "a pull naming two references",
                vec!["pull", "registry.example/app:v1", "registry.example/app:v2"],
                None,
            ),
            (
                "a pull carrying an option this program does not take",
                vec!["pull", "--all-tags", "registry.example/app"],
                None,
            ),
            (
                "the command that asks the daemon about itself",
                vec!["info"],
                Some(Requested::Info),
            ),
            (
                // It takes no operand, so a word after it is no invocation of it.
                "that command carrying a word of its own",
                vec!["info", "everything"],
                None,
            ),
            (
                // `--format` decides the FORM of what it writes, which is not performed.
                "that command asked for a form of its own",
                vec!["info", "--format", "{{.ServerVersion}}"],
                None,
            ),
            ("another command of the CLI", vec!["images"], None),
            ("no command at all", vec![], None),
        ];
        for (case, words, expected) in rows {
            let arguments: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
            let asked = requested(&arguments);
            assert_eq!(
                asked, expected,
                "case {case:?}: expected {expected:?}, got {asked:?}"
            );
        }
    }

    /// RULE: the Region a push addresses is the one the registry host it names carries, and a
    /// host that names none — the endpoint this emulator announces its registries at — leaves
    /// the push to the build's own Region.
    #[test]
    fn a_push_addresses_the_region_its_registry_host_names() {
        // (case, the host, the Region it names)
        let rows: Vec<(&str, &str, Option<&str>)> = vec![
            (
                "a private registry host",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com",
                Some("us-west-2"),
            ),
            (
                "another Region",
                "123456789012.dkr.ecr.eu-central-1.amazonaws.com",
                Some("eu-central-1"),
            ),
            ("this emulator's own endpoint", "127.0.0.1:9297", None),
            ("a loopback name", "localhost:9297", None),
            ("a public registry", "public.ecr.aws", None),
            ("the Docker Hub", "registry-1.docker.io", None),
            (
                "a host with nothing where the Region goes",
                "000000000000.dkr.ecr..amazonaws.com",
                None,
            ),
        ];
        for (case, host, expected) in rows {
            let region = registry_region(host);
            assert_eq!(
                region.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {region:?}"
            );
        }
    }

    /// RULE: a login takes a registry ADDRESS, and the host it carries is what a push then
    /// addresses.
    #[test]
    fn a_login_records_the_host_a_push_addresses() {
        // (case, what the login named, the host it records)
        let rows: Vec<(&str, &str, &str)> = vec![
            (
                "a bare host",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com",
            ),
            (
                "a host written as a URL",
                "https://000000000000.dkr.ecr.us-west-2.amazonaws.com",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com",
            ),
            (
                "a URL carrying a path",
                "https://000000000000.dkr.ecr.us-west-2.amazonaws.com/v2/",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com",
            ),
        ];
        for (case, named, expected) in rows {
            let host = registry_host(named);
            assert_eq!(
                host, expected,
                "case {case:?}: expected {expected:?}, got {host:?}"
            );
        }
    }
}
