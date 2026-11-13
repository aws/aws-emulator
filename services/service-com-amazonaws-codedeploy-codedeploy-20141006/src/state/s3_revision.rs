// SPDX-License-Identifier: CC0-1.0

//! The revision a deployment deploys, FETCHED from the object store that holds it.
//!
//! A revision that names an S3 object keeps the deployment's whole instruction set
//! in that object: `S3Location` carries the `bucket`, the `key`, the `bundleType`,
//! and — as part of the object's IDENTITY — its `version` and its `eTag`
//! (`smithy_query(query="shape",shape="S3Location")`, whose `eTag` is "the ETag of
//! the Amazon S3 object that represents the bundled artifacts for the application
//! revision"). So such a deployment can only do what that object says, and only if
//! the object the triple names is there: the revision is resolved against Amazon
//! S3's OWN object store through the shared engine — the very store `PutObject`
//! wrote — and a deployment whose revision cannot be located there FAILS, carrying
//! the `INVALID_REVISION` `ErrorCode` the model lists
//! (`smithy_query(query="shape",shape="ErrorCode")`).
//!
//! The verdict is IN BAND, not a refusal of a call: `CreateDeployment` accepts a
//! revision whose object it has not fetched — nothing about the object's identity
//! is a question about the shape of that request — and the deployment then reaches
//! the terminal `Failed` status carrying the error information. That is why this
//! runs where [`super::service_role_permissions`] runs, from [`super::with_state`],
//! rather than in a handler: the fetch is owed at the instant the deployment starts
//! whether or not anybody calls CodeDeploy again.
//!
//! What a located object DECIDES is what its bytes declare: the traffic shift and
//! the lifecycle event hooks [`super::appspec`] reads out of an AppSpec document,
//! exactly as they are read out of a revision that carries that document inline.
//! The revision itself is still held verbatim; nothing fetched is written into it.
//!
//! Nothing is refused on evidence this emulator does not have. A bucket the object
//! store holds no record of shows nothing about the object — it may belong to
//! another account, or to a client that never told this emulator about it — so such
//! a deployment goes ahead, the same reading [`super::service_role_permissions`]
//! applies to a role IAM does not hold. A bucket this emulator DOES hold is the
//! authority on what is in it, so a key it has nothing at, or an object whose
//! entity tag is not the one the revision names, is a revision that cannot be
//! located.
//!
//! Like [`super::alarm_gate`], it runs with THIS store unlocked while the object
//! store is locked: the two are never held at once.

use ::service_com_amazonaws_s3_amazons3::state as objects;

use crate::ops::types::{ErrorInformation, RevisionLocation};

use super::{
    appspec, deployment_is_terminal, lifecycle_hook, name_lambda_targets, now_ms, rollback,
    service_state, ComAmazonawsCodedeployCodedeploy20141006State, Deployment,
    DEPLOYMENT_STATUS_FAILED, DEPLOYMENT_STATUS_IN_PROGRESS,
};

/// The `ErrorCode` CodeDeploy reports on a deployment whose revision it could not
/// locate (`smithy_query(query="shape",shape="ErrorCode")` carries it).
pub const ERROR_CODE_INVALID_REVISION: &str = "INVALID_REVISION";

/// Whether `revision` names an object this emulator fetches before the deployment
/// can do anything: the `S3` form, the one that keeps the AppSpec outside the
/// request.
///
/// Called where a deployment is recorded, so the record says from the start
/// whether it still owes a fetch.
#[must_use]
pub fn awaits_fetch(revision: &RevisionLocation) -> bool {
    revision
        .s3_location
        .as_ref()
        .is_some_and(|location| location.bucket.is_some() && location.key.is_some())
}

/// Fetch the revision of every deployment that owes one at this instant, and
/// either let it go ahead with what the object declares or fail it because the
/// object could not be located.
///
/// A store holding no deployment that owes a fetch costs one read of this store
/// and no object-store lock at all.
pub fn resolve_due_revisions<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = now_ms();
    let pending = {
        let store = state.lock();
        due_fetches(&store, now)
    };
    if pending.is_empty() {
        return;
    }
    // No object store beside this service is no evidence about any object.
    let Some(held) = objects::service_state(engine) else {
        return;
    };
    let located: Vec<(Fetch, Located)> = {
        let store = held.lock();
        pending
            .into_iter()
            .map(|fetch| {
                let outcome = locate(&store, &fetch);
                (fetch, outcome)
            })
            .collect()
    };
    let mut store = state.lock();
    for (fetch, outcome) in &located {
        record(&mut store, fetch, outcome, now);
    }
}

/// One revision to fetch, as the question is asked of the object store.
#[derive(Debug, Clone)]
struct Fetch {
    /// The deployment the revision belongs to.
    deployment_id: String,
    /// Its compute platform, which decides how the fetched document is read.
    compute_platform: String,
    /// The `S3Location` members that address the object.
    bucket: String,
    /// See [`Fetch::bucket`].
    key: String,
    /// See [`Fetch::bucket`]: the members that are part of the object's identity,
    /// when the revision states them.
    version: Option<String>,
    /// See [`Fetch::version`].
    e_tag: Option<String>,
}

/// What the object store answered about one revision.
#[derive(Debug, Clone)]
enum Located {
    /// Nothing this emulator can decide anything from: it holds no such bucket.
    NoEvidence,
    /// The object the revision addresses is not in that bucket under the identity
    /// the revision gave it.
    Unlocatable,
    /// The object is there. Its bytes, as a document, when they are one this
    /// emulator can read as text — a bundle of another form (a `zip` or `tar`
    /// artifact) declares nothing this emulator reads.
    Fetched(Option<String>),
}

/// The fetch every deployment that owes one at `now_ms` owes.
fn due_fetches(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<Fetch> {
    state
        .pending_deployments
        .iter()
        .filter_map(|id| state.deployments.get(id))
        .filter(|deployment| is_under_way(deployment, now_ms))
        .filter_map(fetch_of)
        .collect()
}

/// Whether `deployment` is one the fetch is due for at `now_ms`.
///
/// A deployment that has reached an outcome is owed nothing further, and one the
/// clock has not yet started has not reached for its revision yet — which is also
/// why `CreateDeployment` is answered with the acceptance it would be answered
/// with anyway and the failure follows out of band.
fn is_under_way(deployment: &Deployment, now_ms: i64) -> bool {
    deployment.complete_time_ms.is_none()
        && !deployment_is_terminal(&deployment.status)
        && deployment.starts_at_ms <= now_ms
}

/// The fetch `deployment` owes, or `None` when it owes none — because it has
/// already been resolved, or because its revision carries its content where this
/// emulator already read it.
fn fetch_of(deployment: &Deployment) -> Option<Fetch> {
    if !deployment.awaits_revision_fetch {
        return None;
    }
    let location = deployment.revision.as_ref()?.s3_location.as_ref()?;
    Some(Fetch {
        deployment_id: deployment.deployment_id.clone(),
        compute_platform: deployment.compute_platform.clone(),
        bucket: location.bucket.clone()?,
        key: location.key.clone()?,
        version: location.version.clone(),
        e_tag: location.e_tag.clone(),
    })
}

/// Ask the object store for `fetch`'s object.
///
/// The `version` and the `eTag` are part of what the revision ADDRESSES, so a
/// version the bucket does not hold and a tag that is not the stored object's both
/// mean the revision names no object there. An entity tag is compared on its own
/// value, since the quotes around it are the `ETag` header's syntax rather than
/// part of the tag (Amazon S3 reports it quoted, and a client may carry it either
/// way).
fn locate(store: &objects::ComAmazonawsS3Amazons3State, fetch: &Fetch) -> Located {
    let Some(bucket) = store.bucket(&fetch.bucket) else {
        return Located::NoEvidence;
    };
    let Some(object) = bucket.addressed_object(&fetch.key, fetch.version.as_deref()) else {
        return Located::Unlocatable;
    };
    if let Some(named) = fetch.e_tag.as_deref() {
        if objects::etag_value(named) != objects::etag_value(&object.etag) {
            return Located::Unlocatable;
        }
    }
    Located::Fetched(document_in(object))
}

/// The object's bytes as a document, when they are one this emulator reads as an
/// AppSpec at all.
///
/// The bound comes BEFORE the copy: an object longer than the AppSpec document
/// this emulator parses ([`appspec::max_app_spec_bytes`]) is never copied into
/// this service, so the size of one caller's object does not decide how much this
/// store allocates. A bundle of another form (a `zip` or `tar` artifact) is not
/// text at all, and declares nothing this emulator reads.
fn document_in(object: &objects::Object) -> Option<String> {
    if object.body.len() > appspec::max_app_spec_bytes() {
        return None;
    }
    String::from_utf8(object.body.clone()).ok()
}

/// `errorInformation.message` as CodeDeploy words a revision it could not locate.
///
/// A FIXED sentence: what failed, and what to check about the bucket the revision
/// named. It interpolates nothing — CodeDeploy answers the same three sentences
/// for every revision it cannot locate, whichever part of the object's identity
/// (the key, the version, the entity tag) did not resolve — so nothing here
/// narrows it down, and a client triaging a failed release matches on exactly that
/// text.
pub const UNLOCATABLE_REVISION_MESSAGE: &str =
    "The AppSpec file cannot be located in the specified S3 bucket. Verify your AppSpec file is \
     present and that the name and key value pair specified for your S3 bucket are correct. The \
     S3 bucket must be in your current region";

/// Write back what the object store answered about `fetch`.
///
/// This store was unlocked while the object store was read, so what the fetch was
/// resolved from is checked again: a deployment another caller stopped, or the
/// clock completed, in between keeps the outcome it already has. The fetch is
/// recorded as made in every case, so one deployment reaches for its revision once
/// however many passes run.
fn record(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    fetch: &Fetch,
    outcome: &Located,
    now_ms: i64,
) {
    if !state
        .deployments
        .get(&fetch.deployment_id)
        .is_some_and(|deployment| is_under_way(deployment, now_ms))
    {
        return;
    }
    if let Some(deployment) = state.deployments.get_mut(&fetch.deployment_id) {
        deployment.awaits_revision_fetch = false;
    }
    match outcome {
        Located::NoEvidence => {}
        Located::Unlocatable => fail_deployment(state, fetch, now_ms),
        Located::Fetched(document) => {
            if let Some(document) = document.as_deref() {
                deploy_what_the_document_declares(state, fetch, document);
            }
        }
    }
}

/// Let `fetch`'s deployment deploy what the fetched `document` declares.
///
/// The reading is the one a revision carrying its AppSpec inline gets: the traffic
/// shift its `Resources` entry describes (only a Lambda deployment shifts traffic
/// between function versions) and the lifecycle event hooks its `Hooks` section
/// declares for the events this platform runs through a validation function. The
/// deployment's one Lambda target is then NAMED from that shift, exactly as it is
/// named when the shift was read at create time.
fn deploy_what_the_document_declares(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    fetch: &Fetch,
    document: &str,
) {
    let declared = (fetch.compute_platform == appspec::COMPUTE_PLATFORM_LAMBDA)
        .then(|| appspec::declared_in_app_spec(document))
        .flatten();
    let hook_executions =
        lifecycle_hook::declared_executions_in_app_spec(&fetch.compute_platform, document);
    let Some(deployment) = state.deployments.get_mut(&fetch.deployment_id) else {
        return;
    };
    deployment.lambda_traffic_shift = declared;
    // A hook already held is one a document already declared, so the fetched
    // document only adds the hooks this deployment had none of.
    if deployment.hook_executions.is_empty() {
        deployment.hook_executions = hook_executions;
    }
    let declared = deployment.lambda_traffic_shift.clone();
    name_lambda_targets(&mut deployment.targets, declared.as_ref());
}

/// Fail `fetch`'s deployment for a revision that could not be located, and roll it
/// back when its configuration asks for that on a deployment failure.
///
/// A deployment the clock had not yet started also gets the start it was owed, so
/// one that failed before anybody read it still reports the instant it began.
fn fail_deployment(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    fetch: &Fetch,
    now_ms: i64,
) {
    let deployment_id = fetch.deployment_id.as_str();
    if let Some(deployment) = state.deployments.get_mut(deployment_id) {
        if deployment.start_time_ms.is_none() {
            deployment.start_time_ms = Some(deployment.starts_at_ms);
            deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
        }
    }
    state.settle_deployment(
        deployment_id,
        DEPLOYMENT_STATUS_FAILED,
        Some(ErrorInformation {
            code: Some(ERROR_CODE_INVALID_REVISION.to_string()),
            message: Some(UNLOCATABLE_REVISION_MESSAGE.to_string()),
        }),
        now_ms,
    );
    // A deployment that failed is rolled back when its configuration asks for that
    // on a deployment failure, from the one place that is decided.
    rollback::roll_back_on_failure(state, deployment_id, now_ms);
}
