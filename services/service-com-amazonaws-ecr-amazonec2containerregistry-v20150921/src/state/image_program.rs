// SPDX-License-Identifier: CC0-1.0

//! The PROGRAM an image this registry holds declares as its own — what runs in a container
//! of it whose definition names no command.
//!
//! An image declares its program in its CONFIGURATION, and the configuration is a blob of
//! the repository: the Docker image manifest V2 schema 2 and the OCI image manifest both
//! reference it with their `config` descriptor, and that document's `config` object carries
//! the image's `Entrypoint` and `Cmd` (the Docker and OCI image-configuration
//! specifications). So the registry is what can answer the question — it holds the manifest
//! the push stored and the blobs that manifest references — which is why a compute service
//! standing in for a container runtime asks it here rather than reading an image itself.
//!
//! Three answers are possible and the third matters as much as the other two
//! ([`ImageProgram`]): a reference this registry resolves no image for, a document that is
//! not an image manifest at all, a manifest whose configuration blob this registry does not
//! hold and a configuration that does not read as one each state NOTHING about what the
//! image declares, so nothing is claimed for them.
//!
//! A READ throughout: deciding this changes nothing a later caller sees.

use super::*;

use super::registry_pull::{parse_registry_reference, RegistryReference};

/// What the image a pull reference resolves to declares as its own program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageProgram {
    /// Its configuration states an entry point, a command, or both: the argument vector a
    /// container of it is exec'd as, the entry point first and the command after it — the
    /// order a container runtime passes the two in.
    Declared(Vec<String>),
    /// Its configuration states NEITHER, so the image carries no program of its own: a
    /// container of it has nothing to run unless something else names a program for it.
    Undeclared,
    /// Nothing this registry holds states what this image declares.
    Unknown,
}

/// The members of an image configuration that declare its program, in the order a container
/// runtime exec's them: the entry point, then the command passed to it as its arguments.
///
/// Spelled as the image-configuration specifications spell them — the Docker image
/// specification's `config.Entrypoint` and `config.Cmd`, which the OCI image configuration
/// carries under the same two names.
const PROGRAM_MEMBERS: [&str; 2] = ["Entrypoint", "Cmd"];

/// The program the image the reference `image` resolves to declares, for a reference into
/// THIS registry.
///
/// [`ImageProgram::Unknown`] for every reference this registry does not answer for — one
/// addressing another registry altogether, another account's, a repository this registry does
/// not hold or an identifier no image of it answers to — because nothing of this registry's
/// states anything about such an image.
///
/// Takes this service's lock itself, and nothing else: the answer is read off the stored
/// manifest and the stored blob it references.
pub fn declared_image_program<Eng: Send + Sync + 'static>(
    engine: &Eng,
    image: &str,
) -> ImageProgram {
    let Some((addressed_registry, reference)) = parse_registry_reference(image) else {
        return ImageProgram::Unknown;
    };
    // A registry belongs to ONE account and this emulator serves one account's, so a
    // reference into another's names an image nothing here holds
    // ([`registry_pull::serve_reference_pull`] refuses the pull of one for the same reason).
    if addressed_registry != registry_id() {
        return ImageProgram::Unknown;
    }
    let Some(state) = service_state(engine) else {
        return ImageProgram::Unknown;
    };
    let store = state.lock();
    resolved_image_program(&store, &reference)
}

/// The same answer, read out of the store `store` for the resolved reference `reference`.
fn resolved_image_program(
    store: &ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    reference: &RegistryReference,
) -> ImageProgram {
    let Ok(repository) = store.repository(&reference.region, &reference.repository_name) else {
        return ImageProgram::Unknown;
    };
    let Ok(held) = repository.lookup_image(&reference.identifier()) else {
        return ImageProgram::Unknown;
    };
    let Some(digest) = manifest_config_digest(&held.manifest) else {
        return ImageProgram::Unknown;
    };
    let Some(blob) = repository.layer_blobs.get(&digest) else {
        // The manifest references a configuration this repository does not hold, so what the
        // image declares is not here to read.
        return ImageProgram::Unknown;
    };
    configured_program(&blob.content)
}

/// The digest of the CONFIGURATION blob `manifest` references, and `None` for a document
/// that references none.
///
/// A manifest LIST references other manifests rather than a configuration of its own, and a
/// document that is not a manifest at all (an artifact this registry stores as pushed)
/// describes no image — so neither states a program, and neither is answered here.
fn manifest_config_digest(manifest: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(manifest).ok()?;
    if document.get("manifests").is_some() {
        return None;
    }
    document
        .get("config")?
        .get("digest")?
        .as_str()
        .map(ToString::to_string)
}

/// What the bytes of an image CONFIGURATION declare as the image's program.
///
/// [`ImageProgram::Unknown`] for bytes that do not read as an image configuration — one
/// whose `config` object is not there at all — because such a blob states nothing about a
/// program; [`ImageProgram::Undeclared`] for a configuration that reads as one and names
/// neither member, which is an image carrying no program of its own.
fn configured_program(configuration: &[u8]) -> ImageProgram {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(configuration) else {
        return ImageProgram::Unknown;
    };
    let Some(configured) = document.get("config") else {
        return ImageProgram::Unknown;
    };
    let declared: Vec<String> = PROGRAM_MEMBERS
        .iter()
        .filter_map(|member| configured.get(*member))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToString::to_string)
        .collect();
    if declared.is_empty() {
        ImageProgram::Undeclared
    } else {
        ImageProgram::Declared(declared)
    }
}

#[cfg(test)]
mod tests {
    use super::{configured_program, manifest_config_digest, ImageProgram};

    /// RULE: the configuration of an image declares its program in `config.Entrypoint` and
    /// `config.Cmd`, entry point first — and a configuration that reads as one and names
    /// neither declares no program at all, while bytes that do not read as a configuration
    /// declare nothing either way.
    #[test]
    fn a_configuration_declares_the_program_its_two_members_name() {
        for (case, configuration, expected) in [
            (
                "a configuration naming a command alone",
                r#"{"architecture":"amd64","os":"linux","config":{"Cmd":["nginx","-g","daemon off;"]}}"#,
                ImageProgram::Declared(vec![
                    "nginx".to_string(),
                    "-g".to_string(),
                    "daemon off;".to_string(),
                ]),
            ),
            (
                "a configuration naming an entry point alone",
                r#"{"config":{"Entrypoint":["/usr/bin/app"]}}"#,
                ImageProgram::Declared(vec!["/usr/bin/app".to_string()]),
            ),
            (
                "a configuration naming both, the entry point first",
                r#"{"config":{"Cmd":["--serve"],"Entrypoint":["/app"]}}"#,
                ImageProgram::Declared(vec!["/app".to_string(), "--serve".to_string()]),
            ),
            (
                "a configuration naming neither",
                r#"{"architecture":"amd64","os":"linux","config":{},"rootfs":{"type":"layers"}}"#,
                ImageProgram::Undeclared,
            ),
            (
                "a configuration whose two members are empty vectors",
                r#"{"config":{"Cmd":[],"Entrypoint":[]}}"#,
                ImageProgram::Undeclared,
            ),
            (
                "bytes that carry no configuration object",
                r#"{"architecture":"amd64"}"#,
                ImageProgram::Unknown,
            ),
            ("bytes that are not a document at all", "not json", ImageProgram::Unknown),
        ] {
            assert_eq!(
                configured_program(configuration.as_bytes()),
                expected,
                "case {case:?}: expected {expected:?} from {configuration}"
            );
        }
    }

    /// RULE: the configuration a manifest references is its `config` descriptor's digest, and
    /// a document that references none — a manifest list, a document that is not a manifest —
    /// names no configuration.
    #[test]
    fn a_manifest_names_the_configuration_its_descriptor_references() {
        for (case, manifest, expected) in [
            (
                "an image manifest",
                r#"{"schemaVersion":2,"config":{"digest":"sha256:abc"},"layers":[]}"#,
                Some("sha256:abc".to_string()),
            ),
            (
                "a manifest list",
                r#"{"schemaVersion":2,"manifests":[{"digest":"sha256:abc"}]}"#,
                None,
            ),
            (
                "a document referencing no configuration",
                r#"{"schemaVersion":2,"layers":[]}"#,
                None,
            ),
            ("a document that is not a manifest", "{}", None),
        ] {
            assert_eq!(
                manifest_config_digest(manifest),
                expected,
                "case {case:?}: expected {expected:?} from {manifest}"
            );
        }
    }
}
