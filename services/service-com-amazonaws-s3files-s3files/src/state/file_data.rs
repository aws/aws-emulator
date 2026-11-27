// SPDX-License-Identifier: CC0-1.0

//! What a file system HOLDS: the POSIX tree a client that has mounted it reads and writes,
//! and the WRITE-THROUGH that makes every file of that tree an object of the bucket behind
//! it.
//!
//! # The rule this module implements
//!
//! A file system of this service is a view of a bucket: `CreateFileSystem` "creates an S3
//! File System resource scoped to a bucket or prefix within a bucket, enabling file system
//! access", so a file written into the file system IS an object of that bucket under that
//! prefix, and a file taken away IS a deleted object. That is the whole premise of the
//! service: a client writes through the mount and then reads the key with the object store's
//! own API.
//!
//! So the tree below is where a mounted client's writes land ([`MountedFileSystem`]) — shared
//! by every client of one file system, never copied, which is what makes a mount the thing
//! data is handed over through — and every write and removal the tree takes is reported to
//! [`BucketWriteThrough`], which records it in the object store on the SAME engine this
//! service is mounted on. What that store then serves for the key is what this wrote: nothing
//! is kept twice and nothing is invented for a read.
//!
//! # What the object a write-through stores carries
//!
//! The bytes are the file's own, and the properties are the ones the object store settles for
//! any whole-object write of a service of this emulator — the entity tag over the bytes, its
//! default checksum algorithm over all of them whole, and the encryption a write that names a
//! KMS key or none is stored under
//! ([`::service_com_amazonaws_s3_amazons3::state::written_object_encryption`], so a file
//! system created with a `kmsKeyId` stores its objects under that key and one created without
//! takes the bucket's own default). The media type is the one the file's name states
//! ([`::service_common::web_media_type`]), which is the reading every data plane of this
//! workspace that serves a stored file takes of a file name.
//!
//! Beside them the object carries the FILE-SYSTEM attributes of the file it was written
//! from, as user metadata: who the file belongs to, the permissions it carries, its three
//! POSIX timestamps, the file system that wrote it (with the generation and inode of the
//! entry) and the agent that performed the write. They are the provenance a
//! permission-preserving restore reads back off the object, so they are stamped from the
//! tree's own record of the entry rather than from anything a caller stated. The entry's
//! CREATION instant and its inode number are the ones its first write minted
//! ([`EntryTable`]), so a second write of the same file reports the same entry, created when
//! it was created and written when it was written.
//!
//! # What the write-through does NOT carry
//!
//! Two limits of this seam, stated here because the rule above is stated here:
//!
//! * The object store answers whether it TOOK the write — a bucket a client has since deleted,
//!   one of its own ceilings — and the tree has already taken the file by then: a write is
//!   reported after it happened ([`::service_common::posix_shell_volume::FileWrites`]), so a
//!   refusal cannot be handed back to the writing program. Such a write leaves the file in the
//!   file system and no object behind it, and says so in a warning; nothing is invented for the
//!   key, which is what makes a later read of it the store's own honest answer.
//! * A later change of an entry's OWNER or PERMISSIONS alone is not a write of the file, so it
//!   is not reported through this seam and the object keeps the attributes the last write of
//!   its contents stamped. A client that changes a mode and then writes the file carries the
//!   new one.
//!
//! # Bounds
//!
//! The tree is in memory and bounded by [`FILE_SYSTEM_LIMITS`]: how many files and
//! directories it holds and how many bytes they come to. A write past one does not happen and
//! is reported to the writing client as the failure it is. No host path is opened, created or
//! read, and no socket: the object store is reached through its own state accessor on the
//! shared engine.
//!
//! LOCK ORDER: a write is reported while the FILE SYSTEM's tree is held, and takes the object
//! store's lock under it — this tree first, that store second, never the other way round. The
//! object store holds no tree of this service, so the pair cannot close a cycle.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use ::service_common::posix_shell_volume::{
    FileWritten, FileWrites, MountFacts, MountedTree, ShellVolume, VolumeLimits,
};

use super::{bucket_name_of, ComAmazonawsS3filesS3filesState, FileSystem};

/// How much ONE file system holds. Every field is the EMULATOR's number, not a quota of the
/// service: the tree is this process's memory, so it is bounded far above what a scenario
/// stages through a file system and far below what would cost this process anything
/// noticeable.
pub const FILE_SYSTEM_LIMITS: VolumeLimits = VolumeLimits {
    files: 1024,
    bytes: 4 * 1024 * 1024,
    path_bytes: 4096,
    directories: 1024,
    device_lines: 0,
    device_line_bytes: 0,
};

/// The user-metadata name an object carries the uid of the file it was written from under.
///
/// The names are the ones the recorded objects of this service carry, each without the
/// `x-amz-meta-` prefix the protocol adds (`GetObjectOutput$Metadata` is
/// `@httpPrefixHeaders("x-amz-meta-")`).
pub const FILE_OWNER_METADATA: &str = "file-owner";

/// ... its gid.
pub const FILE_GROUP_METADATA: &str = "file-group";

/// ... its permissions, as an octal `st_mode` (the file type bits and the permission bits,
/// which is how a POSIX mode is written: `0100644` for a regular file a write created).
pub const FILE_PERMISSIONS_METADATA: &str = "file-permissions";

/// ... when the file was created, last read and last written, each as the POSIX timestamp of
/// the entry: seconds since the epoch ([`posix_timestamp`]). The creation time is the instant
/// the entry came into being, which a later write of the same file does not move.
pub const FILE_BIRTH_TIME_METADATA: &str = "file-btime";
pub const FILE_ACCESS_TIME_METADATA: &str = "file-atime";
pub const FILE_MODIFIED_TIME_METADATA: &str = "file-mtime";

/// ... which file system wrote it, as `<file system id>:<generation>:<inode>` — the file
/// system the entry belongs to and the identity of the entry inside it.
pub const FILE_SYSTEM_METADATA: &str = "fs-id";

/// ... and WHAT performed the write: the file system's own agent, which is the one value of
/// the set that is the same for every object this data plane writes.
pub const USER_AGENT_METADATA: &str = "user-agent";

/// The value [`USER_AGENT_METADATA`] carries: the agent of this service, as the recorded
/// objects of a mount write carry it.
pub const S3_FILES_USER_AGENT: &str = "aws-s3-files";

/// The uid and gid an entry nobody stated an owner for belongs to: the superuser, which is
/// who a command delivered to a machine by an agent runs as, and so who the files such a
/// command writes belong to.
const DEFAULT_OWNER: (i64, i64) = (0, 0);

/// The permission bits an entry nobody stated a mode for carries: `0644`, which is what a
/// write performed under the default file-creation mask leaves a regular file with.
const DEFAULT_PERMISSIONS: u32 = 0o644;

/// The `st_mode` file-type bits of a REGULAR file (`S_IFREG`), which every file of this tree
/// is: the mode an object reports is the whole `st_mode`, not the permission bits alone.
const REGULAR_FILE_MODE: u32 = 0o100_000;

/// The inode number the ROOT of a file system holds, so the first entry written into one is
/// the next number after it.
const ROOT_INODE: u64 = 1;

/// A handle onto the files ONE file system holds, shared by every client that has mounted it:
/// what one writes through its handle, another reads through its own.
#[derive(Debug, Clone)]
pub struct MountedFileSystem {
    /// The one tree the file system holds.
    files: Arc<Mutex<ShellVolume>>,
}

impl MountedFileSystem {
    /// An empty file system — one nothing has written to yet — whose writes are reported to
    /// `writes` where the caller resolved a write-through for it.
    fn new(writes: Option<Arc<dyn FileWrites>>) -> Self {
        let tree = ShellVolume::new(FILE_SYSTEM_LIMITS);
        let tree = match writes {
            Some(sink) => tree.with_file_write_sink(sink),
            None => tree,
        };
        Self {
            files: Arc::new(Mutex::new(tree)),
        }
    }

    /// The tree, with a lock poisoned by an earlier panic taken rather than propagated: a
    /// file system outlives the client that was writing to it, so one failed write does not
    /// take the file system's data with it.
    pub fn files(&self) -> MutexGuard<'_, ShellVolume> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This file system as a client MOUNTS it into its own tree: the one tree above — shared,
    /// never copied — rooted at `root` inside it and reporting `facts`.
    ///
    /// `None` for a root that is not an absolute path at all. Whether the file system HOLDS
    /// that directory is the mount's own question, asked where the mount is made
    /// ([`::service_common::posix_shell_volume::ShellVolume::mount`]).
    #[must_use]
    pub fn mounted_tree(&self, root: &str, facts: MountFacts) -> Option<MountedTree> {
        MountedTree::new(Arc::clone(&self.files), root, facts)
    }
}

impl ComAmazonawsS3filesS3filesState {
    /// A handle onto the files the file system `file_system_id` holds, for a client that has
    /// MOUNTED it — and `None` when this store holds no such file system, which is a mount of
    /// nothing.
    ///
    /// The handle is MINTED on the first mount and kept with the store from then on, so the
    /// second client to mount the same file system is handed the same tree — and `writes`,
    /// the write-through the caller resolved for that file system, is installed on the tree as
    /// it is minted, so one file system has one write-through however many clients mount it.
    /// Handles whose file system is gone are dropped as one is handed out, so a deleted file
    /// system takes its data with it and this map is bounded by the file systems the store
    /// holds.
    pub fn mounted_file_system(
        &mut self,
        file_system_id: &str,
        writes: Option<Arc<dyn FileWrites>>,
    ) -> Option<MountedFileSystem> {
        if !self.file_systems.contains_key(file_system_id) {
            return None;
        }
        let held = &self.file_systems;
        self.file_system_data
            .retain(|id, _| held.contains_key(id.as_str()));
        Some(
            self.file_system_data
                .entry(file_system_id.to_string())
                .or_insert_with(|| MountedFileSystem::new(writes))
                .clone(),
        )
    }
}

/// The WRITE-THROUGH of one file system: where a write that landed in its tree is recorded as
/// an object of the bucket the file system is a view of.
///
/// Everything it needs is read off the file system's own record when the write-through is
/// resolved — the bucket the file system is scoped to, the prefix inside it, the key its
/// objects are encrypted under and its id — so a write costs one lock of the object store and
/// nothing else.
pub struct BucketWriteThrough {
    /// The object store this service is mounted beside, on the one shared engine.
    objects: ::engine::ServiceState<
        ::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State,
    >,
    /// The bucket the file system is a view of, by name — `FileSystem::bucket` names it as an
    /// ARN, and the object store holds it by name.
    bucket: String,
    /// The prefix inside that bucket the file system is scoped to: "an S3 File System
    /// resource scoped to a bucket or prefix within a bucket", so a file at `/report.txt` of
    /// a file system scoped to `team/` is the object `team/report.txt`.
    prefix: String,
    /// The KMS key the file system's objects are encrypted under, for a file system created
    /// with one.
    kms_key_id: Option<String>,
    /// The file system's own id, which every object it writes names itself by.
    file_system_id: String,
    /// The GENERATION of this file system, as the identity of an entry names it: one number
    /// per file system, derived from its id, so two file systems never stamp the same
    /// identity on their entries.
    generation: u64,
    /// The IDENTITY each path of the tree holds: its inode number and the instant the entry
    /// was created ([`EntryTable`]).
    entries: Mutex<EntryTable>,
}

/// What one ENTRY of a file system is, across every write of it: the inode number its path
/// holds and the instant the entry was created.
#[derive(Debug, Clone, Copy)]
struct EntryIdentity {
    /// The inode number, assigned on the first write of the path.
    inode: u64,
    /// When the entry came into being, which is the first write of it: a `btime` is the
    /// creation of the file, so a later write of the same file does not move it.
    born_at: SystemTime,
}

/// Which IDENTITY each path of one file system holds, the inode numbers assigned in write
/// order from the first number after the root's.
///
/// An entry keeps its number and its birth instant across every later write of it, which is
/// what makes the identity an object carries the identity of the FILE rather than of the
/// write. A path REMOVED through the mount is forgotten ([`Self::forget`]): a file created
/// again at that path is a new entry, with its own number and its own creation instant, as it
/// is in a POSIX file system.
///
/// The table is BOUNDED by how many entries the file system itself holds
/// ([`FILE_SYSTEM_LIMITS`]) — the paths written are a program's, so the table must not grow
/// with the writes a program makes — and past that ceiling the entry assigned longest ago is
/// forgotten: a path written again after that takes a fresh identity, exactly as a path this
/// file system had never held does.
#[derive(Debug, Default)]
struct EntryTable {
    /// The identity each path holds.
    identities: BTreeMap<String, EntryIdentity>,
    /// The paths in the order they were assigned, so the oldest is the one forgotten at the
    /// ceiling.
    order: ::std::collections::VecDeque<String>,
    /// How many numbers this table has assigned, which is what the next one follows.
    assigned: u64,
}

impl EntryTable {
    /// The identity `path` holds, minting one for a path this table does not hold: the next
    /// inode number, and `written_at` as the instant that entry came into being.
    fn identity_of(&mut self, path: &str, written_at: SystemTime) -> EntryIdentity {
        if let Some(held) = self.identities.get(path) {
            return *held;
        }
        while self.order.len() >= FILE_SYSTEM_LIMITS.files {
            if let Some(oldest) = self.order.pop_front() {
                self.identities.remove(&oldest);
            }
        }
        self.assigned = self.assigned.saturating_add(1);
        let minted = EntryIdentity {
            inode: ROOT_INODE.saturating_add(self.assigned),
            born_at: written_at,
        };
        self.identities.insert(path.to_string(), minted);
        self.order.push_back(path.to_string());
        minted
    }

    /// Forget the entry at `path`, which is what its REMOVAL leaves behind: the next file
    /// created there is a new entry rather than the one that was taken away.
    fn forget(&mut self, path: &str) {
        self.identities.remove(path);
        self.order.retain(|held| held != path);
    }
}

impl ::std::fmt::Debug for BucketWriteThrough {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("BucketWriteThrough")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("file_system_id", &self.file_system_id)
            .finish_non_exhaustive()
    }
}

impl BucketWriteThrough {
    /// The write-through of `file_system` into the object store on `engine` — and `None`
    /// when this engine has no object store to write into, or when the file system's
    /// `bucket` names no bucket at all: nothing is then recorded for a write, rather than
    /// recorded somewhere it does not belong.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(
        engine: &Eng,
        file_system: &FileSystem,
    ) -> Option<Arc<dyn FileWrites>> {
        let objects = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
        let bucket = bucket_name_of(&file_system.bucket)?.to_string();
        Some(Arc::new(Self {
            objects,
            bucket,
            prefix: file_system.prefix.clone(),
            kms_key_id: file_system.kms_key_id.clone(),
            generation: generation_of(&file_system.file_system_id),
            file_system_id: file_system.file_system_id.clone(),
            entries: Mutex::new(EntryTable::default()),
        }))
    }

    /// The key the file at `path` inside the file system is the object at: the file system's
    /// prefix followed by the path, which has no leading separator as a key.
    fn key_of(&self, path: &str) -> String {
        format!("{}{}", self.prefix, path.trim_start_matches('/'))
    }

    /// The identity the entry at `path` holds, minting one for an entry this file system has
    /// not written before ([`EntryTable`]).
    fn identity_of(&self, path: &str, written_at: SystemTime) -> EntryIdentity {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .identity_of(path, written_at)
    }

    /// The user metadata the object written from `written` carries: the file-system
    /// attributes of the file itself, and the agent that performed the write.
    ///
    /// The three timestamps are the entry's own: it was CREATED when the first write of that
    /// path made it ([`EntryTable`]) and last WRITTEN by this write, and this tree records one
    /// read instant per entry — the write that left it as it is — so the access time this
    /// write stamps is that same instant.
    fn stamped_metadata(&self, written: &FileWritten<'_>) -> BTreeMap<String, String> {
        let (uid, gid) = written.owner.unwrap_or(DEFAULT_OWNER);
        let mode = REGULAR_FILE_MODE | written.mode.unwrap_or(DEFAULT_PERMISSIONS);
        let entry = self.identity_of(written.path, written.written_at);
        let at = posix_timestamp(written.written_at);
        BTreeMap::from([
            (FILE_OWNER_METADATA.to_string(), uid.to_string()),
            (FILE_GROUP_METADATA.to_string(), gid.to_string()),
            (FILE_PERMISSIONS_METADATA.to_string(), format!("0{mode:o}")),
            (
                FILE_BIRTH_TIME_METADATA.to_string(),
                posix_timestamp(entry.born_at),
            ),
            (FILE_ACCESS_TIME_METADATA.to_string(), at.clone()),
            (FILE_MODIFIED_TIME_METADATA.to_string(), at),
            (
                FILE_SYSTEM_METADATA.to_string(),
                format!(
                    "{}:{}:{}",
                    self.file_system_id, self.generation, entry.inode
                ),
            ),
            (
                USER_AGENT_METADATA.to_string(),
                S3_FILES_USER_AGENT.to_string(),
            ),
        ])
    }
}

impl FileWrites for BucketWriteThrough {
    /// Record the file `written` as the object at its key.
    ///
    /// The object is built the way every other service of this emulator that delivers one
    /// builds it ([`::service_com_amazonaws_s3_amazons3::state::Object::stored`]) and then
    /// carries this data plane's own two additions: the encryption the file system's key or
    /// the bucket's default settles, and the file-system attributes of the file. A store that
    /// refuses the write — a bucket it no longer holds, one of its own ceilings — records
    /// nothing, exactly as it records nothing for any other writer it refuses.
    fn wrote(&self, written: &FileWritten<'_>) {
        let metadata = self.stamped_metadata(written);
        let mut store = self.objects.lock();
        let encryption = ::service_com_amazonaws_s3_amazons3::state::written_object_encryption(
            &store,
            &self.bucket,
            self.kms_key_id.as_deref(),
        );
        let mut object = ::service_com_amazonaws_s3_amazons3::state::Object::stored(
            written.contents.to_vec(),
            Some(::service_common::web_media_type::media_type_of_path(written.path).to_string()),
        );
        object.encryption = encryption;
        object.metadata = metadata;
        object.last_modified = written.written_at;
        let key = self.key_of(written.path);
        if store.put_object(&self.bucket, &key, object).is_none() {
            ::tracing::warn!(
                bucket = %self.bucket,
                "the bucket an s3 files file system is a view of is no longer held, so a write \
                 through its mount stored no object"
            );
        }
    }

    /// Delete the object the removed file was: a file taken out of the file system is an
    /// object taken out of the bucket, which is the other half of the same rule.
    ///
    /// The deletion is the object store's own, so a versioned bucket takes the delete MARKER
    /// such a delete lays and an unversioned one takes the key away, exactly as a
    /// `DeleteObject` naming no version does.
    ///
    /// The entry itself is forgotten with it ([`EntryTable::forget`]): a file created again at
    /// that path is a new entry, so it carries its own identity and its own creation instant
    /// rather than those of the file that was taken away.
    fn removed(&self, path: &str) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .forget(path);
        let key = self.key_of(path);
        let mut store = self.objects.lock();
        store.remove_object(&self.bucket, &key);
    }
}

/// The GENERATION number of the file system `file_system_id`: one number per file system,
/// derived from its own id so it is stable for the life of the file system and different for
/// every other one.
fn generation_of(file_system_id: &str) -> u64 {
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    file_system_id.hash(&mut hasher);
    hasher.finish()
}

/// `at` as a POSIX timestamp of a file: SECONDS since the epoch, the unit a file's own
/// timestamps are counted in (`st_atime`, `st_mtime` and the birth time beside them) — the same
/// RAW POSIX form the owner, the group and the permissions beside it carry, so one restore
/// reads the whole set the same way.
///
/// The second is the one the instant LIES IN: a count of seconds is truncated towards the
/// epoch, never rounded up, so a stamp never names a second the entry had not reached. An
/// instant before the epoch names no POSIX second at all and is stamped as the epoch itself.
fn posix_timestamp(at: SystemTime) -> String {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{posix_timestamp, EntryTable, ROOT_INODE};

    /// RULE: a file's timestamp is stamped as the POSIX second the instant lies in — the
    /// second it reached, never the next one.
    #[test]
    fn a_posix_timestamp_names_the_second_the_instant_lies_in() {
        // The rows span the sub-second part of the instant, which is what the rule generalises
        // over: none at all, the first nanosecond of the second, one past half a second (so a
        // stamp that rounded up would be told apart from one that does not) and the last
        // nanosecond below the next second.
        let rows = [
            ("a whole second", 1_700_000_000_u64, 0_u32, "1700000000"),
            (
                "one nanosecond past the second",
                1_700_000_000,
                1,
                "1700000000",
            ),
            (
                "one nanosecond past half a second",
                1_700_000_000,
                500_000_001,
                "1700000000",
            ),
            (
                "the last nanosecond below the next second",
                1_700_000_000,
                999_999_999,
                "1700000000",
            ),
            ("the epoch itself", 0, 0, "0"),
        ];
        for (case, seconds, nanoseconds, expected) in rows {
            let at = SystemTime::UNIX_EPOCH + Duration::new(seconds, nanoseconds);
            let stamped = posix_timestamp(at);
            assert_eq!(
                stamped, expected,
                "case {case:?}: expected the stamp {expected:?}, got {stamped:?}"
            );
        }
    }

    /// RULE: an instant BEFORE the epoch names no POSIX second, so it is stamped as the epoch
    /// rather than as a count no reader of this set can take.
    #[test]
    fn an_instant_before_the_epoch_is_stamped_as_the_epoch() {
        let stamped = posix_timestamp(SystemTime::UNIX_EPOCH - Duration::from_nanos(1));
        assert_eq!(
            stamped, "0",
            "case \"an instant before the epoch\": expected the epoch's own stamp, got \
             {stamped:?}"
        );
    }

    /// RULE: a path's entry keeps its inode number and its BIRTH instant across every later
    /// write of it — a write of a file is not the creation of one — while another path is
    /// another entry, and a path whose file was removed is a new entry when it is written
    /// again.
    #[test]
    fn an_entry_keeps_its_identity_and_its_birth_instant_across_writes() {
        let born = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let later = born + Duration::from_secs(60);
        let mut table = EntryTable::default();

        let first = table.identity_of("/report.txt", born);
        assert!(
            first.inode > ROOT_INODE && first.born_at == born,
            "case \"the first write of a path\": expected an inode past the root's \
             ({ROOT_INODE}) and the instant of that write, got {first:?}"
        );
        let rewritten = table.identity_of("/report.txt", later);
        assert!(
            rewritten.inode == first.inode && rewritten.born_at == born,
            "case \"a later write of the same path\": expected the entry {first:?} to keep its \
             identity, got {rewritten:?}"
        );
        let other = table.identity_of("/exports/report.txt", later);
        assert!(
            other.inode != first.inode && other.born_at == later,
            "case \"another path of the same file system\": expected an entry of its own, got \
             {other:?}"
        );

        table.forget("/report.txt");
        let created_again = table.identity_of("/report.txt", later);
        assert!(
            created_again.inode != first.inode && created_again.born_at == later,
            "case \"a path created again after its file was removed\": expected a new entry, \
             got {created_again:?}"
        );
    }
}
