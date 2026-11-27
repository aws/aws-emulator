// SPDX-License-Identifier: CC0-1.0

//! The COPIES the registry's replication configuration asks for: how an image pushed into
//! a repository a replication rule covers reaches the destination registry it names.
//!
//! # The rule this module implements
//!
//! A replication rule is an instruction about CONTENT, not a note on the registry's
//! settings: "the contents of all repositories are replicated" (`RepositoryFilter`), and
//! `PutReplicationConfiguration`'s own documentation describes the configuration as what
//! drives "the replication process". So an image pushed into a repository a rule covers is
//! copied into the destination registry the rule names: the destination holds a repository
//! of the SAME NAME — ECR creates it there if it does not exist — and in it the identical
//! image, which means the manifest BYTES as they were pushed (an image is addressed by the
//! content digest of those bytes, so a copy that re-serialised them would be a different
//! image) together with the blobs that manifest references and the tags pointing at it.
//! A client of the destination Region therefore reads the same image back: the same
//! digest, the same manifest and the same layer bytes.
//!
//! A repository NO rule covers is not copied anywhere, so the destination registry holds
//! nothing of it and a read addressed there is refused exactly as a read of a repository
//! that was never created ([`super::repository_not_found_message`]).
//!
//! # When a copy is made
//!
//! Which copies a push starts is decided at the PUSH
//! ([`super::ComAmazonawsEcrAmazonec2containerregistryV20150921State::planned_replications`]):
//! each one is recorded on the stored image as a [`super::ImageReplication`] carrying the
//! instant the clock owes its completion ([`super::IMAGE_REPLICATION_DELAY`]). This module
//! is what MAKES the copy that record promises, at that instant: a copy is the registry's
//! own work rather than a caller's, so it is made whether or not anybody calls again, by a
//! task of the registry's ([`deliver_when_due`]) rather than by whoever next reads the
//! destination. Until the copy is made the record is still pending
//! ([`super::ImageReplication::copied`]), and once it is made the destination holds the
//! image, so the status `DescribeImageReplicationStatus` reports and the content a
//! destination-Region read answers with settle together.
//!
//! One destination is NOT copied into: another account's registry
//! ([`delivers_into_this_registry`]). This emulator serves exactly one account's registries
//! ([`super::registry_id`]), so a cross-account destination names a registry it does not
//! hold — there is no repository of anybody's there to copy into — and the copy ECR makes
//! for it is reported by its status alone.

use ::std::collections::BTreeSet;
use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime};

use super::{
    service_state, ComAmazonawsEcrAmazonec2containerregistryV20150921State, ImageRecord,
    ImageReplication, LayerBlob, Repo,
};

/// How long past a copy's due instant the delivery task looks for work.
///
/// The due instant is stated on the WALL clock while the wait is served by the MONOTONIC
/// one, so a sleep that ends at the due instant can still find the wall clock a moment
/// short of it; this is the margin that keeps the look from being too early. The same
/// margin [`super::scan_events`] waits for its own due instant with.
const DUE_SLACK: Duration = Duration::from_millis(50);

/// How many times in a row the delivery task may look for work and make no copy before it
/// stops looking.
///
/// A look made before the next copy falls due makes none, which is the ordinary case, so
/// this is well above the one or two such looks a copy takes. What it bounds is a task that
/// cannot make progress at all — a due instant a clock jump put far in the future, a copy
/// the destination cannot take whole — which would otherwise be looked at for as long as the
/// process lives. The copies stay pending and the next push arms a task that makes them, the
/// same standing [`super::scan_events`] bounds its own looks with.
const MAX_FRUITLESS_LOOKS: usize = 8;

/// One copy of one image that is due and has not been made yet.
///
/// It NAMES the copy rather than carrying it: the image is read out of the store when the
/// copy is made, so a copy never delivers content that was decided anywhere but on the
/// stored image.
#[derive(Debug, Clone)]
struct DueCopy {
    /// The Region of the registry holding the image that is copied.
    source_region: String,
    /// The repository holding it, whose name the destination's repository shares.
    repository_name: String,
    /// The content digest the image is addressed by, in both registries.
    digest: String,
    /// The Region of the registry the image is copied INTO.
    destination_region: String,
    /// The instant the copy is STORED at, which is the instant the clock owed it: the
    /// destination's record of the image is dated by it, so a copy made late is not dated by
    /// whoever got round to making it.
    at: SystemTime,
}

/// The image one copy delivers, as it is read out of the source repository: everything the
/// destination has to end up holding.
struct Replica {
    /// The image record the destination stores, carrying the manifest bytes verbatim.
    image: ImageRecord,
    /// The blobs that manifest references — its layers and its image configuration — as the
    /// source repository holds them, bytes included.
    blobs: Vec<LayerBlob>,
    /// The tags pointing at the source image, which point at the copy in the destination.
    tags: BTreeSet<String>,
}

/// Whether `replication` is a copy into a registry this emulator holds, and therefore one
/// that can be made at all.
///
/// Two destinations are not: another account's registry, because this emulator serves
/// exactly one account ([`super::registry_id`]) and holds no registry of anybody else's to
/// copy into; and this account's registry in the Region the image is already in, which is
/// the source registry itself and is refused when the configuration is stored
/// (`PutReplicationConfiguration`), so it is never copied into here either.
fn delivers_into_this_registry(source_region: &str, replication: &ImageReplication) -> bool {
    delivers_into(source_region, &replication.region, &replication.registry_id)
}

/// Whether a copy from the registry in `source_region` into the registry
/// `destination_registry_id` holds in `destination_region` is one this emulator can make.
///
/// The one statement of which destinations this emulator holds a registry for, reached by
/// the copies a push owes ([`delivers_into_this_registry`]) and by the copies a backfill of
/// the registry's existing images owes ([`super::replication_backfill`]) alike: a
/// destination it answers `false` for is one whose registry is not here, so a copy for it is
/// reported by its status rather than made.
pub fn delivers_into(
    source_region: &str,
    destination_region: &str,
    destination_registry_id: &str,
) -> bool {
    destination_registry_id == super::registry_id() && destination_region != source_region
}

impl ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Make every copy the clock has already made due as of `now`, record each one it made
    /// on the image that started it, and answer HOW MANY it made.
    ///
    /// Idempotent: a copy that has been made is not made again, and a copy the destination
    /// cannot take whole is left pending, so a later call makes it once it can.
    pub fn settle_replications(&mut self, now: SystemTime) -> usize {
        let mut made = 0usize;
        for copy in self.due_copies(now) {
            if self.copy_image(&copy) {
                self.record_copy_made(&copy);
                made = made.saturating_add(1);
            }
        }
        made
    }

    /// When the earliest copy this registry still owes fell or falls due, or `None` when it
    /// owes none.
    ///
    /// Every copy that has not been made yet is reported, whether its instant is still to
    /// come or already past: a copy whose instant passed before anything could make it is
    /// owed MORE urgently, not less, so the delivery task waits for the ones to come and
    /// makes the overdue ones at once. `None` is therefore the one state in which there is
    /// nothing left to do, which is what lets the task stop.
    pub fn next_replication_due(&self) -> Option<SystemTime> {
        self.repositories
            .iter()
            .flat_map(|((region, _), repo)| {
                repo.images
                    .values()
                    .flat_map(move |image| image.replications.iter().map(move |copy| (region, copy)))
            })
            .filter(|(region, copy)| !copy.copied && delivers_into_this_registry(region, copy))
            .map(|(_, copy)| copy.complete_due_at)
            .min()
    }

    /// When the earliest piece of work this registry still owes its destinations fell or
    /// falls due, or `None` when it owes none: the copies a push started
    /// ([`Self::next_replication_due`]) and the transitions a backfill of the registry's
    /// existing images owes
    /// ([`Self::next_replication_backfill_due`]), which one task settles together.
    pub fn next_registry_copy_due(&self) -> Option<SystemTime> {
        [
            self.next_replication_due(),
            self.next_replication_backfill_due(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Take the registry's ONE delivery task slot, or answer `false` when a task already
    /// holds it or there is no copy left to make.
    ///
    /// The slot is taken under the store's lock, exactly as it is given back
    /// ([`DeliveryTask::settle`]), so a push that arrives while a task is deciding to stop
    /// either finds the slot free or is seen by that task's own last look: one of the two
    /// always makes the copy.
    fn claim_delivery_task(&mut self) -> bool {
        if self.replication_delivery_running || self.next_registry_copy_due().is_none() {
            return false;
        }
        self.replication_delivery_running = true;
        true
    }

    /// Every copy that is due and not made yet as of `now`.
    ///
    /// A read of the store: the images are not touched here, so a pass that finds nothing
    /// to do changes nothing.
    fn due_copies(&self, now: SystemTime) -> Vec<DueCopy> {
        let mut due = Vec::new();
        for ((region, name), repo) in &self.repositories {
            for image in repo.images.values() {
                for copy in &image.replications {
                    if copy.copied
                        || copy.complete_due_at > now
                        || !delivers_into_this_registry(region, copy)
                    {
                        continue;
                    }
                    due.push(DueCopy {
                        source_region: region.clone(),
                        repository_name: name.clone(),
                        digest: image.digest.clone(),
                        destination_region: copy.region.clone(),
                        at: copy.complete_due_at,
                    });
                }
            }
        }
        due
    }

    /// The image `copy` delivers, the blobs its manifest references and the tags pointing at
    /// it, read out of the source repository — or `None` when there is nothing to copy yet.
    ///
    /// The manifest travels VERBATIM — the very bytes the push stored — so the copy is
    /// addressed by the same content digest as its source. What the copy does NOT carry is
    /// anything the source's own push did beyond storing the image: the replica starts no
    /// replication of its own (a copy is not a push, so replication is not chained), no
    /// signing job and no scan, and it is in standard storage however the source image has
    /// been transitioned since.
    ///
    /// `None` where the registry no longer holds the repository or the image, and where the
    /// source no longer holds every blob the manifest references: the copy is the WHOLE
    /// image, so a destination holding a manifest whose layers are not beside it would
    /// advertise an image nothing can pull.
    fn replica_of(&self, copy: &DueCopy) -> Option<Replica> {
        let source = self
            .repository(&copy.source_region, &copy.repository_name)
            .ok()?;
        let image = source.images.get(&copy.digest)?;
        if !source.missing_layers(&image.manifest).is_empty() {
            return None;
        }
        let blobs = super::manifest_blobs(&image.manifest)
            .into_iter()
            .filter_map(|descriptor| source.layer_blobs.get(&descriptor.digest).cloned())
            .collect();
        // The replica is born through the one constructor a stored image is born through,
        // from the manifest bytes as they stand, and it is pushed into the destination at
        // the instant the clock owed this copy.
        Some(Replica {
            image: ImageRecord::pushed(
                image.digest.clone(),
                image.manifest.clone(),
                image.manifest_media_type.clone(),
                copy.at,
            ),
            blobs,
            tags: image.tags.clone(),
        })
    }

    /// Copy the image `digest` of the repository `repository_name` of the registry in
    /// `source_region` into this account's registry in `destination_region`, dated `at`, and
    /// answer whether the copy was made.
    ///
    /// The one copy this service makes, whether what asks for it is the push that stored the
    /// image (the registry's replication configuration, [`Self::settle_replications`]) or a
    /// backfill of the images the registry already held
    /// ([`super::replication_backfill`]): both deliver the same whole image —
    /// the manifest bytes verbatim, the blobs it references and the tags pointing at it —
    /// into the same destination, under the same refusal when the destination cannot take it
    /// whole ([`Self::can_take`]).
    pub fn copy_image_into(
        &mut self,
        source_region: &str,
        repository_name: &str,
        digest: &str,
        destination_region: &str,
        at: SystemTime,
    ) -> bool {
        self.copy_image(&DueCopy {
            source_region: source_region.to_string(),
            repository_name: repository_name.to_string(),
            digest: digest.to_string(),
            destination_region: destination_region.to_string(),
            at,
        })
    }

    /// Copy the image `copy` names into the destination registry, creating the same-named
    /// repository there when it holds none, and answer whether the copy was made.
    ///
    /// `false` where there is nothing to copy any more (the source image, its repository or
    /// a blob its manifest references is gone) and where the destination cannot take the
    /// copy whole ([`Self::can_take`]).
    fn copy_image(&mut self, copy: &DueCopy) -> bool {
        let Some(replica) = self.replica_of(copy) else {
            return false;
        };
        if !self.can_take(copy, &replica) {
            return false;
        }

        let key = super::repository_key(&copy.destination_region, &copy.repository_name);
        let destination_region = copy.destination_region.clone();
        let repository_name = copy.repository_name.clone();
        let created_at = replica.image.pushed_at;
        let destination = self.repositories.entry(key).or_insert_with(|| {
            // ECR creates the destination repository for a replicated image, with the
            // settings a repository nobody configured carries.
            Repo::created(&destination_region, &repository_name, created_at)
        });
        for blob in replica.blobs {
            destination
                .layer_blobs
                .entry(blob.digest.clone())
                .or_insert(blob);
        }
        let digest = replica.image.digest.clone();
        // A digest addresses one content, so a destination that already holds this image
        // holds the same manifest bytes: the copy keeps that record rather than replacing
        // it, so a push made at the destination keeps its own pushed instant, scan and
        // signing jobs.
        destination
            .images
            .entry(digest.clone())
            .or_insert(replica.image);
        // The tags pointing at the source image point at the copy, which is what makes the
        // replicated tag resolve in the destination Region. Every one of them is attachable:
        // that the destination allows each to be moved, and has room for it, is what
        // [`Self::can_take`] established before anything was stored.
        for tag in replica.tags {
            destination.move_tag(&tag, &digest);
        }
        // The destination's image set changed, so the sizes it reports are recomputed the
        // one way they are recomputed after a push.
        destination.refresh_image_sizes();
        true
    }

    /// Whether the destination registry can take the copy `copy` names WHOLE — the image,
    /// its blobs and every tag pointing at it — checked before anything of it is stored, so
    /// a copy that cannot be made leaves no part of itself behind, not even an empty
    /// destination repository, and is not recorded as made.
    ///
    /// Two things can refuse it. The store is bounded
    /// ([`super::MAX_REGISTRY_CALLER_BYTES`], [`super::MAX_REPOSITORIES`],
    /// [`super::MAX_IMAGES_PER_REPOSITORY`], [`super::MAX_TAGS_PER_REPOSITORY`]) and a copy
    /// is caller-driven growth exactly as a push is, so past those bounds the copy is not
    /// made rather than made at the cost of the process's memory. And a destination
    /// repository's `imageTagMutability` governs tag reassignment in it whatever moves the
    /// tag, so a copy whose tag the destination holds immutable against another image is not
    /// made either — delivering the image without its tag would leave the destination
    /// answering nothing for the tag the copy was supposed to bring.
    ///
    /// Nothing is dropped to make room: the copy stays pending, and a later pass makes it
    /// once the destination can take it.
    fn can_take(&self, copy: &DueCopy, replica: &Replica) -> bool {
        let held = self.repositories.get(&super::repository_key(
            &copy.destination_region,
            &copy.repository_name,
        ));
        // What the copy would ADD: the manifest and the blobs the destination does not hold
        // already. Re-copying content it holds costs nothing more.
        let manifest_bytes = match held {
            Some(repo) if repo.images.contains_key(&replica.image.digest) => 0,
            _ => replica.image.manifest.len(),
        };
        let added = replica
            .blobs
            .iter()
            .filter(|blob| held.is_none_or(|repo| !repo.layer_blobs.contains_key(&blob.digest)))
            .fold(manifest_bytes, |bytes, blob| {
                bytes.saturating_add(blob.content.len())
            });
        let new_tags = replica
            .tags
            .iter()
            .filter(|tag| held.is_none_or(|repo| repo.digest_of_tag(tag).is_none()))
            .count();
        let room = self.has_byte_budget_for(added)
            && (held.is_some() || self.repositories.len() < super::MAX_REPOSITORIES)
            && held.is_none_or(|repo| {
                (repo.images.contains_key(&replica.image.digest)
                    || repo.images.len() < super::MAX_IMAGES_PER_REPOSITORY)
                    && repo.tag_index.len().saturating_add(new_tags)
                        <= super::MAX_TAGS_PER_REPOSITORY
            });
        let tags_movable = held.is_none_or(|repo| {
            replica.tags.iter().all(|tag| {
                repo.digest_of_tag(tag)
                    .is_none_or(|previous| *previous == replica.image.digest)
                    || repo.tag_is_mutable(tag)
            })
        });
        if !room || !tags_movable {
            ::tracing::warn!(
                repository = %copy.repository_name,
                image = %copy.digest,
                destination = %copy.destination_region,
                room,
                tags_movable,
                "the destination registry cannot take this replicated image whole — it is \
                 holding its bounds, or a tag of the copy is immutable there against \
                 another image; the image is not copied until it can"
            );
        }
        room && tags_movable
    }

    /// Record on the source image that the copy `copy` names has been made, so it is not
    /// made twice.
    fn record_copy_made(&mut self, copy: &DueCopy) {
        let Ok(source) = self.repository_mut(&copy.source_region, &copy.repository_name) else {
            return;
        };
        let Some(image) = source.images.get_mut(&copy.digest) else {
            return;
        };
        for record in &mut image.replications {
            if record.region == copy.destination_region {
                record.copied = true;
            }
        }
    }
}

/// Make the copies this registry owes, each at the instant the clock makes it due.
///
/// Called by the push that started them. It returns at once: the push has to answer while
/// the copies it started are still being made, so they are made on a task of the
/// registry's — a copy happens whether or not anybody reads the destination, exactly as it
/// does in ECR, so no read has to stand in for it and every read of the destination,
/// control plane and data plane alike, sees the same store.
///
/// At most ONE such task runs per registry: it keeps looking until the registry owes no
/// further copy, so a run of pushes is served by the task already running rather than by
/// one task per push. A task that is dropped, aborted or unwound gives its slot back, and
/// the next push arms another, which makes every copy still pending.
///
/// Its looks are bounded ([`MAX_FRUITLESS_LOOKS`]): a due instant a clock jump put far in
/// the future, and a copy the destination cannot take, would otherwise be looked at again
/// and again for as long as the process lives. A task that stops making progress stops, and
/// the next push arms one that makes every copy still pending.
///
/// A caller with no asynchronous runtime around it arms nothing: a copy is made on a task,
/// and there is nowhere to run one, so the slot is left free for the next push rather than
/// taken by a task that was never started.
pub fn deliver_when_due<Eng: Send + Sync + 'static>(engine: Arc<Eng>) {
    let Some(state) = service_state(&*engine) else {
        return;
    };
    if ::tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    if !state.lock().claim_delivery_task() {
        return;
    }
    ::tokio::spawn(async move {
        // The slot is given back however this task ends, including a task the runtime drops
        // at shutdown or one that unwinds: a slot that leaked would stop this registry from
        // ever making another copy.
        let mut task = DeliveryTask {
            state,
            released: false,
        };
        // How many looks in a row have made no copy. A look made before the next copy falls
        // due makes none and is expected; a run of them means this task cannot make the
        // copies it is waiting for.
        let mut fruitless = 0usize;
        loop {
            let (made, next) = task.settle();
            fruitless = if made > 0 {
                0
            } else {
                fruitless.saturating_add(1)
            };
            let Some(due) = next else {
                break;
            };
            if fruitless >= MAX_FRUITLESS_LOOKS {
                ::tracing::warn!(
                    "this registry has looked {MAX_FRUITLESS_LOOKS} times without making a \
                     replicated copy it owes; the copies still pending are made by the next \
                     push"
                );
                task.release();
                break;
            }
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or_default()
                // A due instant a clock jump put far in the future cannot make this task
                // wait longer than the span a copy was promised in.
                .min(super::IMAGE_REPLICATION_DELAY)
                .saturating_add(DUE_SLACK);
            ::tokio::time::sleep(wait).await;
        }
    });
}

/// The registry's ONE delivery task slot, held for as long as the task that took it runs.
struct DeliveryTask {
    /// The store the copies are made in.
    state: ::engine::ServiceState<ComAmazonawsEcrAmazonec2containerregistryV20150921State>,
    /// Whether the task already gave the slot back, which it does under the store's lock as
    /// it stops looking.
    released: bool,
}

impl DeliveryTask {
    /// Make every copy that is due, and answer how many were made with when the next one
    /// still owed falls due — `None` when the registry owes none, in which case the slot has
    /// been given back.
    ///
    /// The slot is released under the SAME lock the last look is made under, so a push that
    /// records a copy after this task has stopped looking finds the slot free and arms a
    /// task of its own: one of the two always makes the copy.
    fn settle(&mut self) -> (usize, Option<SystemTime>) {
        let mut store = self.state.lock();
        let now = SystemTime::now();
        // Both the copies a push owes and the transitions a backfill owes: one registry
        // task makes every copy the registry owes its destinations, whatever asked for it.
        let made = store
            .settle_replications(now)
            .saturating_add(store.settle_replication_backfills(now));
        let next = store.next_registry_copy_due();
        if next.is_none() {
            store.replication_delivery_running = false;
            self.released = true;
        }
        (made, next)
    }

    /// Give the slot back while copies are still pending: this task stops looking for them,
    /// and the next push arms a task that makes them.
    fn release(&mut self) {
        self.state.lock().replication_delivery_running = false;
        self.released = true;
    }
}

impl Drop for DeliveryTask {
    fn drop(&mut self) {
        if !self.released {
            self.state.lock().replication_delivery_running = false;
        }
    }
}
