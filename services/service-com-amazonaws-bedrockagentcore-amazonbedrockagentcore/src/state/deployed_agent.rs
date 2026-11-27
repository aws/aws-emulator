// SPDX-License-Identifier: CC0-1.0

//! WHAT a runtime endpoint runs: the artifact the version an invocation is addressed to was
//! deployed from, read off the component that OWNS agent runtimes.
//!
//! An agent runtime and its endpoints are created through the CONTROL plane, so the
//! deployment is that plane's record and nothing here keeps a copy of it: an invocation
//! resolves the endpoint it addresses ([`super::resolve_runtime_endpoint`]) and then reads
//! the version that endpoint points at out of the owner's store. What that version carries
//! is its `agentRuntimeArtifact` — "the artifact of the agent" — which is either a container
//! image or a `codeConfiguration` naming source code in object storage (source:
//! smithy_query query="shape" shape="AgentRuntimeArtifact,CodeConfiguration").
//!
//! Only one of the two can be RUN here. A `codeConfiguration` artifact is source code, and
//! this emulator holds a contained interpreter for the language its `runtime` member names
//! ([`super::run_agent`]), so the deployed code itself answers the invocation. A
//! `containerConfiguration` artifact is an image this process cannot start — no container
//! runtime is reachable from inside this emulator — so an invocation addressed to one is
//! reported as work nothing ran ([`AgentUnavailable::NotRunnable`]) rather than answered
//! with a body this emulator composed.

use std::collections::BTreeMap;

use ::service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::{
    self as package, PackageModule, ReadBudget,
};

use control_plane::ops::types::{AgentRuntimeArtifact, Code, S3Location};

/// Why the deployment an invocation addresses cannot be run, with the sentence it is refused
/// with. Which modelled error carries the sentence is the operation's own choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentUnavailable {
    /// Neither the agent nor the version the endpoint points at resolved: the modelled
    /// `ResourceNotFoundException`, worded by [`super::runtime_endpoint_not_found`] so an
    /// invocation and a session stop cannot word the same condition differently.
    NotFound(String),
    /// The component that owns agent runtimes could not be reached: a modelled
    /// `InternalServerException`.
    StoreUnavailable(String),
    /// The deployment exists and nothing in this emulator can run it, so no invocation of it
    /// is served: the modelled `RuntimeClientError`.
    NotRunnable(String),
}

/// The DEPLOYMENT one endpoint of one agent runtime serves.
#[derive(Debug, Clone)]
pub struct DeployedAgent {
    /// The version of the runtime the endpoint points at, as the wire renders it.
    pub version: String,
    /// The object-storage location the deployed source code is stored at.
    pub code: S3Location,
    /// The `AgentManagedRuntimeType` the code is run under (source: smithy_query
    /// query="shape" shape="AgentManagedRuntimeType").
    pub runtime: String,
    /// The `entryPoint` the invocation is delivered through: "the entry point for the code
    /// execution, specifying the function or method that should be invoked when the code
    /// runs" (`smithy_query shape=CodeConfiguration`).
    pub entry_point: Vec<String>,
    /// The environment the deployed code runs with, as the runtime version configures it.
    pub environment: BTreeMap<String, String>,
}

/// The deployment the endpoint `endpoint_name` of `agent_runtime_id` serves at `now`.
///
/// A READ of the owner's store: nothing of this plane's or that plane's state is changed.
pub fn deployed_agent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    agent_runtime_arn: &str,
    agent_runtime_id: &str,
    endpoint_name: &str,
) -> Result<DeployedAgent, AgentUnavailable> {
    let missing = || {
        AgentUnavailable::NotFound(super::runtime_endpoint_not_found(
            endpoint_name,
            agent_runtime_arn,
        ))
    };
    let Some(control) = control_plane::state::service_state(engine) else {
        return Err(AgentUnavailable::StoreUnavailable(
            super::RUNTIME_STORE_UNAVAILABLE.to_string(),
        ));
    };
    let store = control.lock();
    // A runtime the owner no longer holds, and one whose endpoints hold no record of the
    // alias, are the same answer the resolution of the endpoint gives: that is the one
    // sentence this plane words a call it cannot address with.
    let Some(runtime) = store.agent_runtimes.get(agent_runtime_id) else {
        return Err(AgentUnavailable::NotRunnable(no_deployment(
            agent_runtime_arn,
        )));
    };
    // The endpoint PINS a version, so what answers an invocation is the version that
    // endpoint points at rather than whatever the runtime published last.
    let version = match runtime.endpoints.get(endpoint_name) {
        Some(endpoint) => runtime.versions.get(&endpoint.target_version),
        None => runtime.latest(),
    };
    let Some(version) = version else {
        return Err(missing());
    };
    match &version.agent_runtime_artifact {
        AgentRuntimeArtifact::codeConfiguration(configuration) => {
            let Code::S3(location) = &configuration.code;
            Ok(DeployedAgent {
                version: version.version_string(),
                code: location.clone(),
                runtime: configuration.runtime.clone(),
                entry_point: configuration.entry_point.clone(),
                environment: version.environment_variables.clone(),
            })
        }
        AgentRuntimeArtifact::containerConfiguration(configuration) => {
            Err(AgentUnavailable::NotRunnable(format!(
                "the agent runtime is deployed from the container image {} and this emulator \
                 starts no container, so nothing ran the invocation",
                configuration.container_uri
            )))
        }
    }
}

/// How an ARN that resolves to no DEPLOYMENT at all is reported: the environment a harness
/// runs its own session in is addressed under this plane's ARN too, and nothing was deployed
/// to it, so there is no entry point an invocation could be delivered to.
fn no_deployment(agent_runtime_arn: &str) -> String {
    format!(
        "the agent '{agent_runtime_arn}' holds no deployed artifact, so nothing ran the \
         invocation"
    )
}

/// The `AgentManagedRuntimeType` values this emulator runs with its contained PYTHON
/// interpreter: every Python release the enum names (source: smithy_query query="shape"
/// shape="AgentManagedRuntimeType", values PYTHON_3_10 | PYTHON_3_11 | PYTHON_3_12 |
/// PYTHON_3_13 | PYTHON_3_14 | NODE_22).
pub const PYTHON_RUNTIMES: &[&str] = &[
    "PYTHON_3_10",
    "PYTHON_3_11",
    "PYTHON_3_12",
    "PYTHON_3_13",
    "PYTHON_3_14",
];

/// Whether `runtime` names a release this emulator has an interpreter for.
///
/// A value this list does not know is reported as work nothing ran rather than run as
/// Python on a guess.
pub fn runs_python(runtime: &str) -> bool {
    PYTHON_RUNTIMES.contains(&runtime)
}

/// How a deployment whose declared runtime this emulator has no engine for is reported.
pub fn no_engine_for(runtime: &str) -> String {
    format!(
        "the agent runtime is deployed to run on {runtime} and this emulator holds no engine \
         for it, so nothing ran the invocation"
    )
}

/// The deployed code's own MODULES, as the engine running it imports them.
///
/// An artifact is stored either as the source files themselves or as an archive of them — a
/// `prefix` names "objects in the Amazon S3 bucket", and a deployment package is normally
/// one zip — so each object read is taken as whichever it is: an archive is read for the
/// modules it holds, under the bounds that reader applies
/// ([`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package`]), and an
/// object that is not an archive is the source file it is, at the path it is stored under.
/// An object that is neither is no module of the deployment and nothing is imported from it.
///
/// `Err` is a location whose objects hold no module at all, so there is nothing to deliver
/// an invocation to.
pub fn deployed_modules(
    objects: &[control_plane::state::CodeObject],
) -> Result<Vec<PackageModule>, String> {
    let mut budget = ReadBudget::default();
    let mut modules: Vec<PackageModule> = Vec::new();
    for object in objects {
        match package::python_modules_within(Some(&object.bytes), &mut budget) {
            Ok(held) if !held.is_empty() => modules.extend(held),
            _ => {
                if let Ok(source) = String::from_utf8(object.bytes.clone()) {
                    modules.push(PackageModule {
                        path: object.path.clone(),
                        source,
                    });
                }
            }
        }
    }
    if modules.is_empty() {
        return Err(
            "the agent runtime's code artifact holds no source this emulator can import, so \
             nothing ran the invocation"
                .to_string(),
        );
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use ::std::io::Write as _;

    use super::*;

    /// A .zip archive carrying `entries`, written with the maintained zip writer — a real
    /// archive, which is the form a deployment package is normally stored in.
    fn archive(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
        for (path, source) in entries {
            writer
                .start_file(*path, ::zip::write::SimpleFileOptions::default())
                .expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry is written");
        }
        writer
            .finish()
            .expect("the archive is finished")
            .into_inner()
    }

    /// One object of an artifact, as the component that owns the runtime read it.
    fn object(path: &str, bytes: Vec<u8>) -> control_plane::state::CodeObject {
        control_plane::state::CodeObject {
            path: path.to_string(),
            bytes,
        }
    }

    /// RULE: the deployed code's modules are read out of whichever form the artifact is
    /// stored in — an archive is read for the modules it holds, a source object is the module
    /// it is at its own path — and an artifact holding no importable source at all is
    /// reported as having nothing to deliver an invocation to.
    #[test]
    fn an_artifacts_modules_are_read_from_the_form_it_is_stored_in() {
        let source = "def invoke(payload):\n    return payload\n";
        // (case, the objects the artifact is stored as, the module paths they are imported at)
        let rows: Vec<(&str, Vec<control_plane::state::CodeObject>, Vec<&str>)> = vec![
            (
                "one archive holding the entry module",
                vec![object("main.zip", archive(&[("main.py", source)]))],
                vec!["main.py"],
            ),
            (
                "one archive holding a package of modules",
                vec![object(
                    "agent.zip",
                    archive(&[
                        ("main.py", source),
                        ("tools/__init__.py", source),
                        // A file of the artifact that is not Python is no module of it.
                        ("requirements.txt", "boto3\n"),
                    ]),
                )],
                vec!["main.py", "tools/__init__.py"],
            ),
            (
                "the source file itself, stored as one object",
                vec![object("main.py", source.as_bytes().to_vec())],
                vec!["main.py"],
            ),
            (
                "several source objects stored under one prefix",
                vec![
                    object("agent.py", source.as_bytes().to_vec()),
                    object("shout.py", source.as_bytes().to_vec()),
                ],
                vec!["agent.py", "shout.py"],
            ),
            (
                "an archive beside a source object",
                vec![
                    object("library.zip", archive(&[("shout.py", source)])),
                    object("main.py", source.as_bytes().to_vec()),
                ],
                vec!["shout.py", "main.py"],
            ),
        ];
        for (case, objects, expected) in rows {
            let read = deployed_modules(&objects).unwrap_or_else(|reason| {
                panic!("case {case:?}: expected the artifact's modules, got {reason}")
            });
            let paths: Vec<String> = read.iter().map(|module| module.path.clone()).collect();
            assert_eq!(
                paths, expected,
                "case {case:?}: expected the modules at {expected:?}, got {paths:?}"
            );
            for module in &read {
                assert!(
                    !module.source.is_empty(),
                    "case {case:?}: expected the stored source of {}, got nothing",
                    module.path
                );
            }
        }

        // (case, the objects the artifact is stored as, which hold no importable source)
        let nothing: Vec<(&str, Vec<control_plane::state::CodeObject>)> = vec![
            (
                "an artifact of no objects at all",
                Vec::new(),
            ),
            (
                "an archive holding no Python at all",
                vec![object(
                    "data.zip",
                    archive(&[("requirements.txt", "boto3\n")]),
                )],
            ),
            (
                "an object whose bytes are neither an archive nor text",
                vec![object("image.bin", vec![0xff, 0xfe, 0x00, 0x01])],
            ),
        ];
        for (case, objects) in nothing {
            let read = deployed_modules(&objects);
            assert!(
                read.is_err(),
                "case {case:?}: expected no module to be imported, got {:?}",
                read.map(|modules| modules
                    .iter()
                    .map(|module| module.path.clone())
                    .collect::<Vec<String>>())
            );
        }
    }
}
