// SPDX-License-Identifier: CC0-1.0

//! Running the builds the clock owes a run, where the account's object store can be
//! reached.
//!
//! # The rule this module implements
//!
//! A build's run is not only its commands: its INPUT comes out of the object store (the
//! source archive it was pointed at, the cache its project carries) and its OUTPUT goes
//! back into it (the artifact objects the buildspec declared, the cache it leaves for the
//! next build). Those are calls to another service, so a build is run HERE — from an
//! operation, over the emulator's in-process transport — rather than inside the store
//! lock, which can reach nothing.
//!
//! What this module adds is only WHERE the steps happen; the order is the build's own:
//!
//! 1. the source archive is downloaded and unpacked into the working tree, and the cached
//!    paths are restored into it ([`super::build_source`], [`super::build_cache`]);
//! 2. the buildspec's phases run over that tree ([`super::run_commands`]);
//! 3. what the commands produced is put where the build's artifacts say
//!    ([`super::artifact_upload`]), and the declared cache paths are saved;
//! 4. the build records what every step answered ([`super::finish_build`]).
//!
//! # Why a reporting operation drives it
//!
//! The transition out of `IN_PROGRESS` is owed at the instant the clock made it due
//! whether or not anybody called, so it is materialised by whoever reads the build next —
//! the same rule every other settle in this crate follows. What is different here is only
//! that the run happens OUTSIDE the store's lock: it reads a caller-supplied buildspec and
//! it calls another service, and neither belongs in a critical section every other request
//! of this service shares. A run is CLAIMED out of the store before it starts
//! ([`ComAmazonawsCodebuildCodebuild20161006State::claim_due_build_runs`]) and recorded
//! into it after, so two calls that arrive together run one build once; a build whose run
//! is still in flight is still `IN_PROGRESS`, which is what it is.
//!
//! What one run costs is bounded before it starts: the bytes a source or cache object
//! contributes ([`super::build_source`], [`super::build_cache`]), the entries and the
//! expansion of the archive they arrive in ([`super::source_archive`]), the working tree
//! the commands write, and the command budget every command of the buildspec spends
//! (`::service_common::posix_shell_session`), whatever form that command takes.

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{
    artifact_upload, build_cache, build_source, now_millis, BuildRun, BuildSeed,
    ComAmazonawsCodebuildCodebuild20161006State,
};
use crate::ops::types::{Build, BuildArtifacts, ProjectArtifacts};

/// One build whose run the clock owes, claimed out of the store
/// ([`ComAmazonawsCodebuildCodebuild20161006State::claim_due_build_runs`]) by the
/// operation that is performing it.
#[derive(Debug, Clone)]
pub struct ClaimedRun {
    /// The id the store keys this build by, which is the id the build reports.
    pub id: String,
    /// The build's record as it stands, which is what the run is driven from.
    pub build: Build,
    /// The project this build belongs to, read off the record or off the id it is keyed
    /// by, so the cache it carries is scoped to a project rather than to nothing.
    pub project_name: String,
    /// The destination of the build's primary output artifacts.
    pub artifacts: Option<ProjectArtifacts>,
    /// The destinations of its secondary output artifacts.
    pub secondary_artifacts: Vec<ProjectArtifacts>,
    /// Epoch millis the build was started at.
    pub start_millis: i64,
    /// Epoch millis its terminal transition became due at.
    pub due_millis: i64,
    /// Whether this build may restore its project's cache: a project whose cache was
    /// reset after the last save has nothing a build may restore.
    pub cache_restorable: bool,
}

/// Run every build whose run the clock owes and which needs the object store, and record
/// what each one answered.
///
/// Called from an operation that is about to READ or decide on a build, with no guard of
/// the store held: it takes the lock itself, once to claim the runs and once per run to
/// record it, and the steps between them are calls to another emulated service.
pub async fn settle_due_builds(
    callback: &EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsCodebuildCodebuild20161006State>,
    region: &str,
) {
    let claimed = state.lock().claim_due_build_runs(now_millis());
    for run in claimed {
        let recorded = perform(callback, region, &run).await;
        state.lock().record_build_run(&run, recorded);
    }
}

/// What one build's run came to, as the store records it.
#[derive(Debug, Clone)]
pub struct RecordedRun {
    /// What running the build answered: its status, its phases and its exported
    /// variables.
    pub run: BuildRun,
    /// What the build reports for its primary output artifacts once they were put at
    /// their destination — `None` for a build that put none there.
    pub artifacts: Option<BuildArtifacts>,
    /// The same for each of its secondary output artifacts, by the identifier that
    /// addresses one.
    pub secondary_artifacts: Vec<(String, BuildArtifacts)>,
    /// The instant this build's cache was saved at — `None` when it saved none.
    pub cache_saved_millis: Option<i64>,
}

/// Perform one claimed build's run: its input, its commands, and its output.
async fn perform(callback: &EmbeddedEmulator, region: &str, claimed: &ClaimedRun) -> RecordedRun {
    let project_name = claimed.project_name.as_str();
    let mut seed = BuildSeed::default();
    // The build's own source, at the version the build was started with: the pinned
    // object when it names one, whatever the object holds now.
    if let Some(source) = claimed.build.source.as_ref() {
        match build_source::download(
            callback,
            region,
            source,
            claimed.build.source_version.as_deref(),
        )
        .await
        {
            build_source::SourceDownload::NotAnObject => {}
            build_source::SourceDownload::Unpacked(files) => seed.files.extend(files),
            build_source::SourceDownload::Refused(context) => {
                seed.source_failure = Some(context);
            }
        }
    }
    // The cache the project carries, unless it was reset after the cache was written: a
    // build started after `InvalidateProjectCache` restores nothing saved before it.
    let cache = claimed.build.cache.clone().unwrap_or_default();
    if claimed.cache_restorable {
        seed.files
            .extend(build_cache::restore(callback, region, &cache, project_name).await);
    }

    let run = super::run_commands(
        &claimed.build,
        claimed.start_millis,
        claimed.due_millis,
        seed,
    );

    // Nothing is put anywhere for a build that never reached its own finishing work.
    if !run.reached_output {
        return RecordedRun {
            run: super::finish_build(run, None, claimed.due_millis),
            artifacts: None,
            secondary_artifacts: Vec::new(),
            cache_saved_millis: None,
        };
    }
    let spec = run.spec.clone();
    let produced = run.produced.clone();
    // The id the store keys this build by, which is the id it reports: the destination's
    // namespace is composed from it, so it is never an empty segment.
    let build_id = claimed.id.as_str();

    // The primary destination, and then each secondary one: the same upload, each taking
    // the files the buildspec declared for it.
    let mut refusal = None;
    let mut uploaded = None;
    if let Some(destination) = claimed
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifact_upload::destination_of(artifacts, build_id))
    {
        let reported = claimed.build.artifacts.clone().unwrap_or_default();
        match artifact_upload::upload(
            callback,
            region,
            &destination,
            &reported,
            spec.as_ref().and_then(|spec| spec.artifacts.as_ref()),
            &produced,
        )
        .await
        {
            artifact_upload::ArtifactUpload::Uploaded(artifacts) => uploaded = Some(artifacts),
            artifact_upload::ArtifactUpload::Refused(context) => refusal = Some(context),
        }
    }
    let mut secondary_artifacts = Vec::new();
    for artifacts in &claimed.secondary_artifacts {
        let Some(destination) = artifact_upload::destination_of(artifacts, build_id) else {
            continue;
        };
        // The identifier is what addresses this destination's own files in the buildspec
        // (`artifacts/secondary-artifacts/<identifier>`), so a destination with none, or
        // one the buildspec declares no section for, has nothing declared to upload —
        // which the upload below reports as the client error it is.
        let identifier = artifacts.artifact_identifier.clone().unwrap_or_default();
        let reported = claimed
            .build
            .secondary_artifacts
            .iter()
            .flatten()
            .find(|reported| reported.artifact_identifier.as_deref() == Some(&identifier))
            .cloned()
            .unwrap_or_default();
        match artifact_upload::upload(
            callback,
            region,
            &destination,
            &reported,
            spec.as_ref()
                .and_then(|spec| spec.secondary_artifacts.get(&identifier)),
            &produced,
        )
        .await
        {
            artifact_upload::ArtifactUpload::Uploaded(artifacts) => {
                secondary_artifacts.push((identifier, artifacts));
            }
            // One destination that took nothing fails the phase, exactly as the primary
            // one does: the build did not produce what it declared.
            artifact_upload::ArtifactUpload::Refused(context) => {
                refusal = refusal.or(Some(context));
            }
        }
    }

    // The cache the next build of this project restores is saved by the build that ran,
    // from the paths its own buildspec declared.
    let cached = match spec.as_ref().map(|spec| spec.cache_paths.clone()) {
        Some(paths) if !paths.is_empty() => {
            build_cache::save(callback, region, &cache, project_name, &paths, &produced).await
        }
        _ => false,
    };

    RecordedRun {
        run: super::finish_build(run, refusal, claimed.due_millis),
        artifacts: uploaded,
        secondary_artifacts,
        cache_saved_millis: cached.then_some(claimed.due_millis),
    }
}
