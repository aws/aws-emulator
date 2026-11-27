// SPDX-License-Identifier: CC0-1.0

//! The in-memory TREE a bounded shell session reads and writes, for the emulated services
//! whose caller-supplied program is a program of a GUEST.
//!
//! # Why a session needs one
//!
//! A program that runs on a machine writes to that machine: a boot script keeps a counter
//! on the root volume, writes a line to the console device and reads an identity the kernel
//! publishes. A session with nowhere to read or write cannot perform one of those lines at
//! all, so the service that holds the machine (an EC2 instance and its volumes) hands the
//! session this tree, which is the WHOLE of what its commands can reach. There is no host
//! path behind it: every entry is one this process holds in memory, every one of them was
//! put here by the service that owns the machine, and nothing here opens, resolves or lists
//! a path of the host.
//!
//! # What it holds
//!
//! * FILES the program may read and write, which is what the machine's own storage is —
//!   the caller takes them back out ([`ShellVolume::stored_files`]) and keeps them beside
//!   the machine, so what one run wrote the next run reads;
//! * SEALED files, which the service seeded and a program may only READ: the values a
//!   kernel or an agent publishes to a guest rather than values the guest chose;
//! * SEALED DIRECTORIES ([`ShellVolume::seal_directory`]), which are the same statement about
//!   a whole subtree: a filesystem mounted READ-ONLY, where nothing under the root may be
//!   written, created or removed however deep it lies;
//! * DEVICES, which a program may only WRITE: a sink that keeps the lines written to it
//!   ([`ShellVolume::device_lines`], which is how a console gets what a boot script wrote)
//!   or one that discards them, the way `/dev/null` does;
//! * DIRECTORIES, so a write to a path whose directory the machine does not have fails the
//!   way it fails on a machine rather than silently inventing the tree.
//!
//! # What a listing of it reports
//!
//! A program asks what the tree HOLDS as well as what is in a file: a long listing names
//! what each entry is, how many bytes it holds and when it was last written
//! ([`ShellVolume::facts`]), and a listing of a directory names the entries directly under
//! it ([`ShellVolume::entry_names`]). Those are facts of the tree itself — the entry's own
//! kind, the bytes it holds and the instant a write of it happened — so they are kept here
//! beside the contents rather than computed by whoever reports them; how a caller WORDS a
//! listing is the caller's, because the form belongs to the utility it stands for.
//!
//! # What is MOUNTED into it
//!
//! Not everything a program reaches through this tree is the machine's own storage: a
//! filesystem of ANOTHER component — a network filesystem whose files are held beside the
//! resource that owns them — is MOUNTED at a directory of this tree
//! ([`ShellVolume::mount`]), and from then on every path under that directory is a path of
//! that filesystem instead. The mounted tree is SHARED, not copied: what a program on one
//! machine writes under the mount point, a program on another machine that mounted the same
//! filesystem reads under its own, which is the whole of what mounting one is for. Taking it
//! away again ([`ShellVolume::unmount`]) leaves the directory it was mounted over exactly as
//! it was, and leaves the filesystem's own files with whoever holds them: they were never
//! this machine's storage, so they are not among the files the caller keeps beside the
//! machine ([`ShellVolume::stored_files`]).
//!
//! What a mount REPORTS about itself — the filesystem type a listing of the mounts names, the
//! type a statfs of a path on it names, what was mounted and with which options
//! ([`MountFacts`]) — is the mounting component's to state, and is kept beside the mount so
//! the programs that ask about it (a guest's `findmnt`, its `stat -f`) answer out of the one
//! mount table rather than out of a second notion of what is mounted where.
//!
//! # What the guest SENDS rather than stores
//!
//! One family of paths a program writes is not storage at all: a shell opens a socket for a
//! redirection to `/dev/udp/<host>/<port>` ([`DATAGRAM_PATH_PREFIX`]), which is how a program
//! inside a machine hands a datagram to something listening in it without a transfer program.
//! A tree given a SINK for those addresses ([`ShellVolume::with_datagram_sink`]) hands such a
//! write to it — the machine's own network stack, held by whoever owns the machine — instead of
//! keeping a file at that path, and the sink decides what, if anything, receives it. A tree
//! given none keeps the write as what it is to this tree: a path like any other.
//!
//! # What a write to the tree is REPORTED to
//!
//! A filesystem whose whole premise is that a file written into it becomes something else —
//! an object of a bucket behind it — has to learn about the write as it happens. A tree given
//! a sink for its writes ([`ShellVolume::with_file_write_sink`]) reports every write and every
//! removal of one of its OWN files to it ([`FileWrites`]), after the tree has taken it, so the
//! component that holds the filesystem records what the program wrote without a second notion
//! of what the tree holds. A tree given none reports nothing, which is every tree that is only
//! storage.
//!
//! # Bounds
//!
//! Every write is bounded by [`VolumeLimits`], held here rather than in the program's
//! reach: how many files the tree holds and how many bytes they come to, how long one path
//! may be, and how many lines and bytes one device keeps. A write past a bound does not
//! happen and is reported as the failure it is, so a caller-supplied program cannot grow
//! this process without a stated ceiling. A write that lands in a MOUNTED filesystem is
//! bounded by that filesystem's own limits, held by whoever holds its files.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

/// How much one volume may hold. Every field is the EMULATOR's number: the programs are
/// caller-supplied text, so what one of them can store is bounded here.
#[derive(Debug, Clone, Copy)]
pub struct VolumeLimits {
    /// How many files the tree holds at once, sealed ones included.
    pub files: usize,
    /// How many bytes those files' paths and contents come to at once.
    pub bytes: usize,
    /// The longest path this tree accepts.
    pub path_bytes: usize,
    /// How many directories the tree holds at once.
    pub directories: usize,
    /// How many lines ONE device keeps. Past it a write is refused rather than kept.
    pub device_lines: usize,
    /// How many bytes ONE line written to a device may carry. A longer line is truncated on
    /// a character boundary, so what the program wrote is still reported.
    pub device_line_bytes: usize,
}

impl Default for VolumeLimits {
    /// Bounds far above any boot script a guest carries and far below what would cost this
    /// process anything noticeable.
    fn default() -> Self {
        Self {
            files: 256,
            bytes: 256 * 1024,
            path_bytes: 1024,
            directories: 256,
            device_lines: 512,
            device_line_bytes: 4 * 1024,
        }
    }
}

/// The prefix of the paths a shell opens a UDP socket for rather than a file: a redirection to
/// `/dev/udp/<host>/<port>` sends what it writes to that address, which is how a program in a
/// machine hands a datagram to something listening in it with no transfer program at all.
pub const DATAGRAM_PATH_PREFIX: &str = "/dev/udp/";

/// The address the resolved `path` names, for a path under [`DATAGRAM_PATH_PREFIX`] that names
/// a host and a port — and `None` for every other path, including one under that prefix whose
/// last segment is no port number at all.
#[must_use]
pub fn datagram_address(path: &str) -> Option<(String, u16)> {
    let (host, port) = path.strip_prefix(DATAGRAM_PATH_PREFIX)?.split_once('/')?;
    if host.is_empty() || port.contains('/') {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// Where a datagram a program of this machine SENDS goes: the machine's own network stack,
/// held by whoever owns the machine.
///
/// A tree is given one so that a write to `/dev/udp/<host>/<port>` is a send rather than a file
/// ([`DATAGRAM_PATH_PREFIX`]). The implementation answers out of state its own holder keeps —
/// which of the machine's own programs is listening on that port — and nothing here opens a
/// socket, resolves a name or reaches the host: a datagram this process delivers travels inside
/// it, and one nothing is listening for is dropped, which is what a datagram sent to a port
/// nothing listens on does.
pub trait DatagramSink: ::std::fmt::Debug + Send + Sync {
    /// Take the datagram `payload` addressed to `port` at `host`, answering whether the SEND
    /// happened — `false` being what the command performing it reports as its own failure.
    ///
    /// A send to a port nothing listens on still happened: the datagram was handed to the
    /// stack, which is why delivery is not what this answers.
    fn send(&self, host: &str, port: u16, payload: &str) -> bool;
}

/// ONE write a program made to a tree, as the component that HOLDS that tree is told about
/// it ([`FileWrites`]).
///
/// Everything a filesystem records about a file it just took: the path inside itself, the
/// bytes that path holds NOW (an append reports the whole file, not the part appended), who
/// the entry belongs to and the permissions it carries for an entry a caller stated them for
/// ([`ShellVolume::set_owner`], [`ShellVolume::set_mode`]), and the instant the write
/// happened — which is the instant the tree recorded for the entry, not one computed
/// afterwards.
#[derive(Debug, Clone, Copy)]
pub struct FileWritten<'a> {
    /// The path inside the tree that was written, resolved.
    pub path: &'a str,
    /// What that path holds now.
    pub contents: &'a [u8],
    /// Who the entry belongs to, as `(uid, gid)`, for an entry a caller stated an owner for.
    pub owner: Option<(i64, i64)>,
    /// The permission bits the entry carries, for an entry a caller stated them for.
    pub mode: Option<u32>,
    /// When the write happened.
    pub written_at: SystemTime,
}

/// Where a write that lands in a tree is REPORTED: the component that holds the filesystem,
/// for a filesystem whose premise is that a write into it is also something else.
///
/// A tree is given one ([`ShellVolume::with_file_write_sink`]) so that the component holding
/// it learns of a write as the program makes it, rather than by re-reading the tree later and
/// guessing what changed. The implementation records the write wherever that component's own
/// state belongs; nothing here opens a socket, starts a process or reaches a host path, and
/// what it answers is nothing at all — the tree has already taken the write, and a sink that
/// could refuse one would make the file's contents and what was recorded for it disagree.
///
/// It is told about the WRITES and REMOVALS of the tree's OWN files, and about nothing else: a
/// path that lies in a filesystem MOUNTED into this tree is that filesystem's, and is reported
/// to ITS sink; a later change of an entry's owner or permissions alone
/// ([`ShellVolume::set_owner`], [`ShellVolume::set_mode`]) is not a write of the file and is
/// not reported.
pub trait FileWrites: ::std::fmt::Debug + Send + Sync {
    /// Take the write `written`, which the tree has already made.
    fn wrote(&self, written: &FileWritten<'_>);

    /// Take the removal of the file at `path`, which the tree has already made.
    fn removed(&self, path: &str);
}

/// Where the BYTES one of the machine's BLOCK DEVICES come from: the component that holds the
/// storage behind that device.
///
/// A tree is given one ([`ShellVolume::with_block_device_bytes`]) because a block device of a
/// machine is not a file of its filesystem: what it holds lives wherever the component owning the
/// storage keeps it — an attached volume's own bytes — and a program of the guest reads the device
/// with whatever utility it likes (`cat`, `head`, a digest, a copy), not only with the one that
/// knows about that component. Without this, every utility but that one reads such a device as an
/// empty file, which is a device presenting none of the bytes something just wrote to it.
///
/// What the implementation answers is read out of its own state and bounded by it — a device can
/// be far larger than anything this process would materialise — and nothing here opens a device,
/// starts a process or reaches a host path.
pub trait BlockDeviceBytes: ::std::fmt::Debug + Send + Sync {
    /// The bytes the block device at the resolved `path` holds, as a read of the device from its
    /// start answers them — `None` for a path that is no block device of this machine, which is
    /// then whatever else the tree holds there.
    fn device_bytes(&self, path: &str) -> Option<Vec<u8>>;
}

/// What one DEVICE of the tree does with the lines written to it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Device {
    /// Keep them, up to the volume's bounds — a console, whose whole purpose is that
    /// something reads back what was written to it.
    Keep(Vec<String>),
    /// Discard them, which is the whole of what the null device does.
    Discard,
}

/// What one entry of a tree IS — which is the first thing a long listing of it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file: something with contents a program reads and writes.
    File,
    /// A directory: something that holds the names of the entries under it.
    Directory,
    /// A device, which a program writes to rather than keeps bytes in.
    Device,
}

/// One entry of a tree as a long listing reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFacts {
    /// What the entry is.
    pub kind: EntryKind,
    /// How many bytes it holds: a file's contents, and for a directory the names it holds,
    /// which is what a directory's own bytes are.
    pub size: usize,
    /// When this entry was last written — a write of the entry itself, and for a directory
    /// the creation or removal of something under it, which is what changes a directory.
    pub modified: SystemTime,
}
/// What a MOUNTED filesystem reports about itself, as the programs that ask about the mounts
/// of a machine report it.
///
/// Every field is the MOUNTING component's statement about the filesystem it mounted, kept
/// beside the mount so a guest's `findmnt` and its `stat -f` answer out of the one mount
/// table: the type a mount listing names a filesystem by, the type a statfs of a path on it
/// names (the two differ — a network filesystem mounted as `nfs4` is a `nfs` filesystem to
/// statfs), what was mounted, and the options it was mounted with as the command wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountFacts {
    /// The filesystem TYPE a listing of the mounts reports — `findmnt`'s own `FSTYPE`.
    pub file_system_type: String,
    /// The type a statfs of a path on this filesystem reports — `stat -f`'s own `%T`.
    pub statfs_type: String,
    /// WHAT was mounted, as the mounting command's first operand named it — `findmnt`'s own
    /// `SOURCE`.
    pub source: String,
    /// The options the filesystem was mounted with, as the command wrote them —
    /// `findmnt`'s own `OPTIONS`.
    pub options: String,
}

/// A tree ANOTHER component holds, as a volume mounts it: the files themselves, the path
/// INSIDE them the mount point shows, and what the mount reports about itself.
///
/// The files are shared rather than copied — that is what makes a mount the thing data is
/// handed over through — so two machines that mount the same filesystem reach one tree, and a
/// lock an earlier panic poisoned is taken rather than propagated: a filesystem outlives the
/// program that was writing to it.
#[derive(Debug, Clone)]
pub struct MountedTree {
    /// The one tree the mounted filesystem holds.
    files: Arc<Mutex<ShellVolume>>,
    /// The path inside that tree the mount point shows, resolved — the whole filesystem for
    /// a mount of its root, and a directory of it for a mount rooted deeper.
    root: String,
    /// What the mount reports about itself.
    facts: MountFacts,
}

impl MountedTree {
    /// The mount of `files` rooted at `root` inside them, reporting `facts` — `None` for a
    /// root that is not an absolute path at all.
    #[must_use]
    pub fn new(files: Arc<Mutex<ShellVolume>>, root: &str, facts: MountFacts) -> Option<Self> {
        Some(Self {
            files,
            root: resolved_path(root)?,
            facts,
        })
    }

    /// What this mount reports about itself.
    #[must_use]
    pub fn facts(&self) -> &MountFacts {
        &self.facts
    }

    /// Whether the mounted filesystem is one this tree can mount AT ALL, and holds the
    /// directory this mount is rooted at — what makes it something to mount: a filesystem that
    /// holds nothing there would show an empty tree at the mount point rather than the
    /// directory the mount names, and one that already MOUNTS something itself would make the
    /// mount a chain rather than one filesystem at one directory.
    ///
    /// Asked WITHOUT waiting for the mounted tree's lock ([`Self::unlocked_files`]): the only
    /// way that lock is already held at this point is that the filesystem being mounted is the
    /// one being mounted INTO, or one reached through it, which is a mount this tree will not
    /// make rather than one it blocks for — a lock of this kind is not reentrant, so waiting
    /// for it would wedge the thread for good. A filesystem something else is working on at
    /// this instant is answered the same way, and the command that asked for the mount reports
    /// the failure as its own.
    #[must_use]
    pub fn can_be_mounted(&self) -> bool {
        self.unlocked_files()
            .is_some_and(|files| files.mounts.is_empty() && files.is_directory(&self.root))
    }

    /// The mounted tree, with a lock poisoned by an earlier panic taken rather than
    /// propagated.
    ///
    /// LOCK ORDER: the mounting tree is held before the mounted one, never the other way
    /// round, and a mounted tree carries no mounts of its own ([`Self::can_be_mounted`]) — so
    /// delegating one path operation takes exactly these two locks and no third.
    fn files(&self) -> MutexGuard<'_, ShellVolume> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// [`Self::files`] for a caller that must not WAIT for that lock, answering `None` when
    /// something holds it at this instant — which is what makes a mount of a filesystem into
    /// itself a refusal instead of a thread that never returns.
    fn unlocked_files(&self) -> Option<MutexGuard<'_, ShellVolume>> {
        match self.files.try_lock() {
            Ok(files) => Some(files),
            Err(::std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(::std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

/// ONE filesystem mounted into a tree, as that tree holds it.
#[derive(Debug, Clone)]
struct Mount {
    /// The directory of the mounting tree the filesystem is mounted at, resolved.
    point: String,
    /// The filesystem mounted there.
    tree: MountedTree,
}

impl Mount {
    /// Whether the resolved `path` lies in this mount — the mount point itself, or anything
    /// under it. Only a SEPARATOR after the mount point puts a path inside it: `/datax` is
    /// not inside the mount at `/data`.
    fn holds(&self, path: &str) -> bool {
        match path.strip_prefix(self.point.as_str()) {
            Some("") => true,
            Some(under) => under.starts_with('/'),
            None => false,
        }
    }

    /// The path INSIDE the mounted filesystem that the resolved `path` names.
    fn source_of(&self, path: &str) -> String {
        let relative = path
            .strip_prefix(self.point.as_str())
            .unwrap_or_default()
            .trim_start_matches('/');
        if relative.is_empty() {
            return self.tree.root.clone();
        }
        if self.tree.root.ends_with('/') {
            return format!("{}{relative}", self.tree.root);
        }
        format!("{}/{relative}", self.tree.root)
    }
}

/// The tree one session's commands read and write. See the module docs.
#[derive(Debug, Clone)]
pub struct ShellVolume {
    /// The files a program may read and write, by absolute path. A file is BYTES: a program
    /// writing text writes its encoding, and one writing bytes that are no text at all writes
    /// exactly those ([`ShellVolume::write_bytes`]).
    files: BTreeMap<String, Vec<u8>>,
    /// Who each entry of [`Self::files`] and [`Self::directories`] BELONGS TO, as
    /// `(uid, gid)`, for the entries a caller stated an owner for
    /// ([`ShellVolume::set_owner`]) — the identity that created it, which is what a listing
    /// of it reports. An entry nobody stated an owner for belongs to whoever the caller
    /// reporting it says its entries belong to.
    owners: BTreeMap<String, (i64, i64)>,
    /// The PERMISSIONS each entry carries, for the entries a caller stated them for
    /// ([`ShellVolume::set_mode`]) — the mode it was created with or last changed to. An
    /// entry nobody stated a mode for carries whatever the caller reporting it says its
    /// entries carry.
    modes: BTreeMap<String, u32>,
    /// The paths of [`Self::files`] entries a program may only READ.
    sealed: BTreeSet<String>,
    /// The roots of the subtrees a program may only READ ([`Self::seal_directory`]): every
    /// path at or under one of them is read-only, whether or not this tree holds anything
    /// there yet.
    sealed_roots: BTreeSet<String>,
    /// The directories the tree has, by absolute path, always including the root.
    directories: BTreeSet<String>,
    /// The devices the tree has, by absolute path.
    devices: BTreeMap<String, Device>,
    /// When each entry of the tree was last written, by absolute path — what
    /// [`EntryFacts::modified`] reports. An entry is recorded here as it is written, so the
    /// instant is the one the write happened at rather than one computed afterwards.
    modified: BTreeMap<String, SystemTime>,
    /// How many bytes the files come to, kept as they are written rather than recomputed.
    held_bytes: usize,
    /// The filesystems of other components mounted into this tree, DEEPEST mount point
    /// first, so a path lands in the deepest mount that holds it — which is how a mount
    /// inside another mount's directory shadows it.
    mounts: Vec<Mount>,
    /// Where a write to one of the machine's datagram addresses goes, for a tree whose owner
    /// gave it one ([`ShellVolume::with_datagram_sink`]).
    datagrams: Option<Arc<dyn DatagramSink>>,
    /// Where a write to one of this tree's OWN files is reported, for a tree whose owner gave
    /// it one ([`ShellVolume::with_file_write_sink`]).
    writes: Option<Arc<dyn FileWrites>>,
    /// Where the bytes of the machine's BLOCK DEVICES come from, for a tree whose owner gave it
    /// one ([`ShellVolume::with_block_device_bytes`]).
    block_devices: Option<Arc<dyn BlockDeviceBytes>>,
    limits: VolumeLimits,
}

impl Default for ShellVolume {
    fn default() -> Self {
        Self::new(VolumeLimits::default())
    }
}

/// The directory every tree has, which every absolute path is resolved against.
pub const ROOT: &str = "/";

/// `path` with the empty, `.` and `..` segments a path may carry resolved, as a process
/// resolves an ABSOLUTE path — and `None` for a path that is not absolute at all, which a
/// caller resolves against its own working directory first.
///
/// `..` above the root resolves to the root, as it does on a machine, which is also what keeps
/// a path under a mount point from climbing out of what is mounted there.
///
/// The one reading of that rule in this workspace: [`ShellVolume::resolve`] is this with the
/// tree's own path bound applied, and a caller that has to resolve a path BEFORE it knows
/// which tree holds it calls this.
#[must_use]
pub fn resolved_path(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            named => segments.push(named),
        }
    }
    if segments.is_empty() {
        return Some(ROOT.to_string());
    }
    Some(format!("/{}", segments.join("/")))
}

/// The directory the resolved `path` lies in, and `None` for a path with no directory above
/// it — the ROOT itself, which lies in no directory.
#[must_use]
pub fn parent_path(path: &str) -> Option<String> {
    let (parent, _) = path.trim_end_matches('/').rsplit_once('/')?;
    if parent.is_empty() {
        return Some(ROOT.to_string());
    }
    Some(parent.to_string())
}

impl ShellVolume {
    /// An empty tree — the root directory and nothing else — bounded by `limits`.
    #[must_use]
    pub fn new(limits: VolumeLimits) -> Self {
        Self {
            files: BTreeMap::new(),
            owners: BTreeMap::new(),
            modes: BTreeMap::new(),
            sealed: BTreeSet::new(),
            sealed_roots: BTreeSet::new(),
            directories: BTreeSet::from([ROOT.to_string()]),
            devices: BTreeMap::new(),
            modified: BTreeMap::from([(ROOT.to_string(), SystemTime::now())]),
            held_bytes: 0,
            mounts: Vec::new(),
            datagrams: None,
            writes: None,
            block_devices: None,
            limits,
        }
    }

    /// The same tree, with `sink` as where a write to one of its own files is reported
    /// ([`FileWrites`]) — so the component holding the filesystem records what a program
    /// wrote as it writes it.
    #[must_use]
    pub fn with_file_write_sink(mut self, sink: Arc<dyn FileWrites>) -> Self {
        self.writes = Some(sink);
        self
    }

    /// The same tree, with `sink` as where a write to one of the machine's datagram addresses
    /// goes ([`DatagramSink`]) — so such a write is a SEND rather than a file of this tree.
    #[must_use]
    pub fn with_datagram_sink(mut self, sink: Arc<dyn DatagramSink>) -> Self {
        self.datagrams = Some(sink);
        self
    }

    /// The same tree, with `bytes` as where the bytes of the machine's BLOCK DEVICES come from
    /// ([`BlockDeviceBytes`]) — so a READ of one answers what the storage behind it holds, with
    /// whatever utility the program reads it.
    #[must_use]
    pub fn with_block_device_bytes(mut self, bytes: Arc<dyn BlockDeviceBytes>) -> Self {
        self.block_devices = Some(bytes);
        self
    }

    /// Mount `tree` at `path`, so every path under it is a path of that filesystem from now
    /// on — what mounting a filesystem onto a directory of a machine does.
    ///
    /// `false` is a mount that did not happen, which the command performing it reports as its
    /// own failure: a path this tree does not accept, a path it holds no DIRECTORY of its own
    /// at (a filesystem is mounted over a directory, and one inside another mount is not this
    /// tree's to mount over), a point something is already mounted at, and a filesystem that
    /// is not one this tree can mount — one that does not hold the directory the mount would
    /// be rooted at, one that already mounts something itself, and this tree itself or any
    /// tree reached through it ([`MountedTree::can_be_mounted`], which is also what keeps one
    /// path operation to two locks and makes a mount of a filesystem into itself a refusal
    /// rather than a thread that never returns).
    pub fn mount(&mut self, path: &str, tree: MountedTree) -> bool {
        let Some(point) = self.resolve(path) else {
            return false;
        };
        if !self.directories.contains(&point) {
            return false;
        }
        if self.mounts.iter().any(|mount| mount.point == point) {
            return false;
        }
        if !tree.can_be_mounted() {
            return false;
        }
        self.mounts.push(Mount { point, tree });
        // Deepest first: a path is matched against the deepest mount that holds it.
        self.mounts.sort_by(|left, right| {
            right
                .point
                .len()
                .cmp(&left.point.len())
                .then_with(|| left.point.cmp(&right.point))
        });
        true
    }

    /// Take the filesystem mounted AT `path` away again, answering whether one was mounted
    /// there — `false` being what a command unmounting a path nothing is mounted at reports.
    ///
    /// The directory the filesystem was mounted over is left exactly as it was, and the
    /// filesystem's own files are left with whoever holds them.
    pub fn unmount(&mut self, path: &str) -> bool {
        let Some(point) = self.resolve(path) else {
            return false;
        };
        let held = self.mounts.len();
        self.mounts.retain(|mount| mount.point != point);
        self.mounts.len() < held
    }

    /// What is mounted AT `path` exactly, and `None` for a path that is not a mount point of
    /// this tree — what a listing of the mounts answers for a path it was given.
    #[must_use]
    pub fn mount_facts(&self, path: &str) -> Option<&MountFacts> {
        let resolved = self.resolve(path)?;
        self.mounts
            .iter()
            .find(|mount| mount.point == resolved)
            .map(|mount| mount.tree.facts())
    }

    /// What filesystem the resolved `path` lies ON — the deepest mount holding it — and
    /// `None` for a path on the tree's own storage, whose filesystem is the machine's and not
    /// something mounted into it.
    #[must_use]
    pub fn file_system_facts(&self, path: &str) -> Option<&MountFacts> {
        let resolved = self.resolve(path)?;
        self.mounts
            .iter()
            .find(|mount| mount.holds(&resolved))
            .map(|mount| mount.tree.facts())
    }

    /// Read the tree the resolved `path` lies in, when that is a MOUNTED filesystem rather
    /// than this tree's own storage — with the path inside that filesystem.
    ///
    /// LOCK ORDER: the mounted tree's lock is taken while this tree's caller holds whatever it
    /// holds, and a mounted tree carries no mounts of its own ([`MountedTree::can_be_mounted`]),
    /// so this takes exactly one further lock and never a third.
    fn in_mount<T>(&self, resolved: &str, read: impl FnOnce(&ShellVolume, &str) -> T) -> Option<T> {
        let mount = self.mounts.iter().find(|mount| mount.holds(resolved))?;
        let inside = mount.source_of(resolved);
        let files = mount.tree.files();
        Some(read(&files, &inside))
    }

    /// [`Self::in_mount`] for a change of that filesystem. It takes `&self`: what is being
    /// changed is the MOUNTED tree, which is shared and reached through its own lock, and not
    /// this tree.
    fn in_mount_mut<T>(
        &self,
        resolved: &str,
        write: impl FnOnce(&mut ShellVolume, &str) -> T,
    ) -> Option<T> {
        let mount = self.mounts.iter().find(|mount| mount.holds(resolved))?;
        let inside = mount.source_of(resolved);
        let mut files = mount.tree.files();
        Some(write(&mut files, &inside))
    }

    /// `path` as this tree resolves it: absolute, with the empty, `.` and `..` segments a
    /// path may carry resolved ([`resolved_path`]), or `None` for a path this tree does not
    /// accept — a relative one (a session has no working directory of its own), or one past
    /// [`VolumeLimits::path_bytes`].
    #[must_use]
    pub fn resolve(&self, path: &str) -> Option<String> {
        if path.len() > self.limits.path_bytes {
            return None;
        }
        resolved_path(path)
    }

    /// The directory `path` lies in, or `None` for the root itself.
    fn parent_of(path: &str) -> Option<String> {
        parent_path(path)
    }

    /// Seed a file a program may read and WRITE, with the directories above it — what the
    /// machine's storage already held when this run started.
    ///
    /// `false` when the tree is at its bounds and the file was not seeded.
    ///
    /// A seed is the MACHINE's own storage and is MOUNT-BLIND, as the devices below are: the
    /// file is put in this tree, so a path that lies under a mount point is shadowed by what
    /// is mounted there, exactly as the machine's own file under a mount point is on a machine.
    /// A service seeds before a program runs and mounts happen while it runs, so the two do
    /// not meet in practice.
    pub fn put_file(&mut self, path: &str, contents: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        self.make_directories_of(&resolved);
        self.write_resolved(&resolved, contents.as_bytes(), false)
    }

    /// Seed a file a program may only READ: a value the machine publishes to its guest
    /// rather than one the guest chose. A write to it fails the way a write to a read-only
    /// file fails.
    ///
    /// `false` when the tree is at its bounds and the file was not seeded. Mount-blind, for
    /// the reason [`Self::put_file`] states.
    pub fn seal_file(&mut self, path: &str, contents: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        self.make_directories_of(&resolved);
        if !self.write_resolved(&resolved, contents.as_bytes(), false) {
            return false;
        }
        self.sealed.insert(resolved);
        true
    }

    /// Seal the DIRECTORY `path` and everything under it: a filesystem the machine mounted
    /// READ-ONLY, where a program may read what is there and write nothing at all.
    ///
    /// It is the subtree form of [`Self::seal_file`], and it is a statement about PATHS rather
    /// than about the entries the tree holds now: a write, a creation, a removal and a
    /// directory made at or under `path` all fail the way they fail on a read-only filesystem
    /// ([`Self::is_read_only`]), including at a path this tree holds nothing at. The SERVICE's
    /// own seeding is not a program's write and goes on reaching it ([`Self::put_file`],
    /// [`Self::seal_file`]), which is what lets a caller seal the root first and then seed
    /// what the read-only filesystem carries.
    pub fn seal_directory(&mut self, path: &str) {
        if let Some(resolved) = self.resolve(path) {
            self.make_directories_in(&resolved);
            self.sealed_roots.insert(resolved);
        }
    }

    /// Whether `path` is one a program may not write: a sealed FILE, or a path at or under a
    /// sealed DIRECTORY ([`Self::seal_directory`]).
    ///
    /// What the utility performing the write reports it as — the read-only filesystem it is,
    /// rather than a path that is not there.
    #[must_use]
    pub fn is_read_only(&self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.sealed.contains(&resolved) {
            return true;
        }
        self.sealed_roots.iter().any(|root| {
            resolved == *root || resolved.starts_with(&format!("{}/", root.trim_end_matches('/')))
        })
    }

    /// Add a device that KEEPS what is written to it, so the caller reads those lines back
    /// ([`Self::device_lines`]).
    pub fn add_keeping_device(&mut self, path: &str) {
        if let Some(resolved) = self.resolve(path) {
            self.make_directories_of(&resolved);
            self.devices.insert(resolved.clone(), Device::Keep(Vec::new()));
            self.record_written(&resolved);
        }
    }

    /// Add a device that DISCARDS what is written to it, which is the whole of what the
    /// null device does. Reading one answers nothing, as reading it does on a machine.
    pub fn add_discarding_device(&mut self, path: &str) {
        if let Some(resolved) = self.resolve(path) {
            self.make_directories_of(&resolved);
            self.devices.insert(resolved.clone(), Device::Discard);
            self.record_written(&resolved);
        }
    }

    /// Make `path` a directory, with every directory above it — `mkdir -p`'s whole effect.
    ///
    /// `false` when the tree is at [`VolumeLimits::directories`], when a FILE already
    /// holds one of those paths, which is what makes the command fail rather than replace
    /// it, or when the path is one a program may not write ([`Self::is_read_only`]).
    pub fn make_directories(&mut self, path: &str) -> bool {
        if self.is_read_only(path) {
            return false;
        }
        self.make_directories_in(path)
    }

    /// [`Self::make_directories`] as the SERVICE's own seeding performs it, which a sealed
    /// subtree does not stand in the way of: the read-only filesystem's own directories are
    /// made by whoever mounted it.
    fn make_directories_in(&mut self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if let Some(made) = self.in_mount_mut(&resolved, |files, inside| {
            files.make_directories(inside)
        }) {
            return made;
        }
        let mut built = String::new();
        let mut made: Vec<String> = Vec::new();
        for segment in resolved.split('/').filter(|segment| !segment.is_empty()) {
            built.push('/');
            built.push_str(segment);
            if self.files.contains_key(&built) || self.devices.contains_key(&built) {
                return false;
            }
            if !self.directories.contains(&built) {
                made.push(built.clone());
            }
        }
        if self.directories.len().saturating_add(made.len()) > self.limits.directories {
            return false;
        }
        for directory in &made {
            self.record_written(directory);
        }
        self.directories.extend(made);
        true
    }

    /// Record `path` as written NOW, which is what [`EntryFacts::modified`] reports for it.
    fn record_written(&mut self, path: &str) {
        self.modified.insert(path.to_string(), SystemTime::now());
    }

    /// Record the directory `path` lies in as written, which is what creating or removing
    /// something under a directory does to that directory.
    fn record_parent_written(&mut self, path: &str) {
        if let Some(parent) = Self::parent_of(path) {
            if self.directories.contains(&parent) {
                self.record_written(&parent);
            }
        }
    }

    /// The directories above `path`, made as far as the bounds allow. Used where a seed the
    /// SERVICE performs implies them: a machine that has the file has the directories.
    fn make_directories_of(&mut self, path: &str) {
        if let Some(parent) = Self::parent_of(path) {
            let _ = self.make_directories_in(&parent);
        }
    }

    /// What reading `path` answers: a file's contents as TEXT, a sealed file's contents, or
    /// the empty text a discarding device answers — and `None` for a path the tree does not
    /// hold, a directory, or a device whose lines are kept (a console is written, not read).
    ///
    /// A file holds BYTES, so a file whose bytes are no text at all reads as the text they
    /// decode to with the replacement character where they do not: a caller that means to read
    /// the file ITSELF reads [`Self::read_bytes`].
    ///
    /// A path inside a MOUNTED filesystem is read out of that filesystem, which is where its
    /// files are.
    #[must_use]
    pub fn read(&self, path: &str) -> Option<String> {
        self.read_bytes(path)
            .map(|contents| String::from_utf8_lossy(&contents).into_owned())
    }

    /// The BYTES the file at `path` holds, which is the file itself — `None` for the paths
    /// [`Self::read`] answers `None` for.
    #[must_use]
    pub fn read_bytes(&self, path: &str) -> Option<Vec<u8>> {
        let resolved = self.resolve(path)?;
        if let Some(read) = self.in_mount(&resolved, |files, inside| files.read_bytes(inside)) {
            return read;
        }
        if let Some(Device::Discard) = self.devices.get(&resolved) {
            return Some(Vec::new());
        }
        // A BLOCK DEVICE of the machine holds what the component owning its storage holds, and
        // not what this tree keeps at the path the device is published at: asked before the
        // tree's own files, so every utility that reads a path reads the device's own bytes.
        if let Some(bytes) = self
            .block_devices
            .as_ref()
            .and_then(|devices| devices.device_bytes(&resolved))
        {
            return Some(bytes);
        }
        self.files.get(&resolved).cloned()
    }

    /// Who the entry at `path` BELONGS TO, as `(uid, gid)`, for an entry a caller stated an
    /// owner for ([`Self::set_owner`]) — and `None` for one nobody did, whose owner is
    /// whatever the caller reporting it says its entries belong to.
    #[must_use]
    pub fn owner_of(&self, path: &str) -> Option<(i64, i64)> {
        let resolved = self.resolve(path)?;
        if let Some(owner) = self.in_mount(&resolved, |files, inside| files.owner_of(inside)) {
            return owner;
        }
        self.owners.get(&resolved).copied()
    }

    /// Record that the entry at `path` belongs to `uid` and `gid`, as the identity that
    /// CREATED it does on a filesystem: the owner travels with the entry, so every client that
    /// reads it afterwards reports the same one.
    ///
    /// `false` for a path this tree holds no entry at. An entry that goes takes its owner with
    /// it.
    pub fn set_owner(&mut self, path: &str, uid: i64, gid: i64) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if let Some(set) = self.in_mount_mut(&resolved, |files, inside| {
            files.set_owner(inside, uid, gid)
        }) {
            return set;
        }
        if !self.files.contains_key(&resolved) && !self.directories.contains(&resolved) {
            return false;
        }
        self.owners.insert(resolved, (uid, gid));
        true
    }

    /// The PERMISSIONS the entry at `path` carries, for an entry a caller stated them for
    /// ([`Self::set_mode`]) — and `None` for one nobody did.
    #[must_use]
    pub fn mode_of(&self, path: &str) -> Option<u32> {
        let resolved = self.resolve(path)?;
        if let Some(mode) = self.in_mount(&resolved, |files, inside| files.mode_of(inside)) {
            return mode;
        }
        self.modes.get(&resolved).copied()
    }

    /// Record the permissions the entry at `path` carries — the mode it was created with, or
    /// the one a later change set. Like the owner, the mode travels with the entry.
    ///
    /// `false` for a path this tree holds no entry at.
    pub fn set_mode(&mut self, path: &str, mode: u32) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if let Some(set) = self.in_mount_mut(&resolved, |files, inside| files.set_mode(inside, mode))
        {
            return set;
        }
        if !self.files.contains_key(&resolved) && !self.directories.contains(&resolved) {
            return false;
        }
        self.modes.insert(resolved, mode);
        true
    }

    /// Whether the tree holds anything at `path` at all — a file, a directory or a device.
    #[must_use]
    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|resolved| {
            if let Some(holds) = self.in_mount(&resolved, |files, inside| files.exists(inside)) {
                return holds;
            }
            self.files.contains_key(&resolved)
                || self.directories.contains(&resolved)
                || self.devices.contains_key(&resolved)
        })
    }

    /// Whether `path` is a file of the tree (sealed or not).
    #[must_use]
    pub fn is_file(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|resolved| {
            if let Some(is_file) = self.in_mount(&resolved, |files, inside| files.is_file(inside)) {
                return is_file;
            }
            self.files.contains_key(&resolved)
        })
    }

    /// Whether `path` is a directory of the tree.
    #[must_use]
    pub fn is_directory(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|resolved| {
            if let Some(is_directory) =
                self.in_mount(&resolved, |files, inside| files.is_directory(inside))
            {
                return is_directory;
            }
            self.directories.contains(&resolved)
        })
    }

    /// Write `contents` at `path`, replacing what is there or APPENDING to it.
    ///
    /// `false` is a write that did not happen, which is what the command performing it
    /// reports as its own failure: a path this tree does not accept, a sealed file, a path
    /// under a sealed directory ([`Self::seal_directory`]), a
    /// directory, a path whose directory the tree does not have, or a write past
    /// [`VolumeLimits::files`] or [`VolumeLimits::bytes`]. A device takes the write as the
    /// lines it was given, up to [`VolumeLimits::device_lines`].
    pub fn write(&mut self, path: &str, contents: &str, append: bool) -> bool {
        self.write_bytes(path, contents.as_bytes(), append)
    }

    /// [`Self::write`] of BYTES, which is what a file holds: a program writing bytes that are
    /// no text at all writes exactly those, and a later read of the file answers them
    /// ([`Self::read_bytes`]).
    pub fn write_bytes(&mut self, path: &str, contents: &[u8], append: bool) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.is_read_only(&resolved) {
            return false;
        }
        // A path inside a MOUNTED filesystem is written to that filesystem, under its own
        // bounds, which is what makes the write one another client of it reads.
        if let Some(wrote) = self.in_mount_mut(&resolved, |files, inside| {
            files.write_bytes(inside, contents, append)
        }) {
            return wrote;
        }
        // A write to one of the machine's DATAGRAM addresses is a send rather than storage
        // ([`DATAGRAM_PATH_PREFIX`]): it is handed to the machine's own network stack, and this
        // tree keeps no file at that path. A tree whose owner gave it no stack keeps the write
        // as the file it is here.
        if let Some(sink) = self.datagrams.clone() {
            if let Some((host, port)) = datagram_address(&resolved) {
                // A datagram carries the TEXT of those bytes, which is what the stack this
                // tree was given takes.
                return sink.send(&host, port, &String::from_utf8_lossy(contents));
            }
        }
        if self.sealed.contains(&resolved) || self.directories.contains(&resolved) {
            return false;
        }
        if self.devices.contains_key(&resolved) {
            // A device takes LINES, so what is written to one is the text of those bytes.
            let written = String::from_utf8_lossy(contents).into_owned();
            return self.write_device(&resolved, &written);
        }
        // A machine will not create a file in a directory it does not have.
        if Self::parent_of(&resolved).is_some_and(|parent| !self.directories.contains(&parent)) {
            return false;
        }
        if !self.write_resolved(&resolved, contents, append) {
            return false;
        }
        // The write the tree has just taken, reported to whoever holds this filesystem — a
        // PROGRAM's write, which is what such a component learns about: the files the service
        // itself seeds into the tree ([`Self::put_file`], [`Self::seal_file`]) are its own
        // already.
        self.report_written(&resolved);
        true
    }

    /// Report the write of `resolved` to the tree's sink, for a tree whose owner gave it one.
    ///
    /// The bytes reported are the ones the path holds NOW, so an append reports the whole
    /// file; the owner, the mode and the instant are the ones the tree recorded for the entry.
    fn report_written(&self, resolved: &str) {
        let Some(sink) = self.writes.as_ref() else {
            return;
        };
        let Some(contents) = self.files.get(resolved) else {
            return;
        };
        sink.wrote(&FileWritten {
            path: resolved,
            contents,
            owner: self.owners.get(resolved).copied(),
            mode: self.modes.get(resolved).copied(),
            written_at: self
                .modified
                .get(resolved)
                .copied()
                .unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }

    /// [`Self::write`] once the path is resolved and known to name a file.
    fn write_resolved(&mut self, resolved: &str, contents: &[u8], append: bool) -> bool {
        let held = self.files.get(resolved);
        if held.is_none() && self.files.len() >= self.limits.files {
            return false;
        }
        let kept = held.map_or(0, Vec::len);
        let addition = if append {
            contents.len()
        } else {
            contents.len().saturating_sub(kept)
        };
        let path_bytes = if held.is_none() { resolved.len() } else { 0 };
        let would_hold = if append {
            self.held_bytes.saturating_add(addition)
        } else {
            self.held_bytes
                .saturating_sub(kept)
                .saturating_add(contents.len())
        }
        .saturating_add(path_bytes);
        if would_hold > self.limits.bytes {
            return false;
        }
        self.held_bytes = would_hold;
        let created = held.is_none();
        match self.files.get_mut(resolved) {
            Some(file) if append => file.extend_from_slice(contents),
            Some(file) => *file = contents.to_vec(),
            None => {
                self.files.insert(resolved.to_string(), contents.to_vec());
            }
        }
        self.record_written(resolved);
        if created {
            self.record_parent_written(resolved);
        }
        true
    }

    /// Write to a DEVICE: one line per line of `contents`, kept or discarded as the device
    /// says. A line past [`VolumeLimits::device_line_bytes`] is truncated on a character
    /// boundary; a write past [`VolumeLimits::device_lines`] does not happen.
    fn write_device(&mut self, resolved: &str, contents: &str) -> bool {
        let limits = self.limits;
        let Some(device) = self.devices.get_mut(resolved) else {
            return false;
        };
        let Device::Keep(lines) = device else {
            return true;
        };
        // The device takes whole lines: a write with no newline of its own is still one
        // line, which is what a console shows for it.
        let written: Vec<&str> = contents.split_inclusive('\n').collect();
        if lines.len().saturating_add(written.len()) > limits.device_lines {
            return false;
        }
        for line in written {
            let mut line = line.trim_end_matches('\n').to_string();
            if line.len() > limits.device_line_bytes {
                let mut cut = limits.device_line_bytes;
                while cut > 0 && !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.truncate(cut);
            }
            lines.push(line);
        }
        self.record_written(resolved);
        true
    }

    /// Remove the file at `path`, answering whether there was one to remove. A sealed file,
    /// a directory and a device are not removed: a program does not take away what the
    /// machine published to it.
    pub fn remove_file(&mut self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.is_read_only(&resolved) {
            return false;
        }
        if let Some(removed) = self.in_mount_mut(&resolved, |files, inside| {
            files.remove_file(inside)
        }) {
            return removed;
        }
        if self.sealed.contains(&resolved) {
            return false;
        }
        match self.files.remove(&resolved) {
            Some(contents) => {
                self.held_bytes = self
                    .held_bytes
                    .saturating_sub(contents.len().saturating_add(resolved.len()));
                self.modified.remove(&resolved);
                self.owners.remove(&resolved);
                self.modes.remove(&resolved);
                self.record_parent_written(&resolved);
                // The removal the tree has just made, reported to whoever holds this
                // filesystem, exactly as a write is ([`Self::report_written`]).
                if let Some(sink) = self.writes.as_ref() {
                    sink.removed(&resolved);
                }
                true
            }
            None => false,
        }
    }

    /// Remove everything at or under `path` — the file it names, or the whole SUBTREE under
    /// the directory it names, directories included — answering whether anything was removed.
    ///
    /// The sibling of [`Self::make_directories`]: that one makes a chain of directories, this
    /// one takes a whole tree away, which is what a recursive removal does on a machine.
    ///
    /// What a program may NOT remove keeps itself and the directories above it: a sealed file,
    /// a device, and anything under a read-only filesystem ([`Self::is_read_only`]) stay, and
    /// so does every directory still holding one of them — so this never reports a removal it
    /// did not make. The ROOT is the tree itself rather than an entry of it, and is never
    /// removed.
    pub fn remove_tree(&mut self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.is_read_only(&resolved) {
            return false;
        }
        if let Some(removed) = self.in_mount_mut(&resolved, |files, inside| {
            files.remove_tree(inside)
        }) {
            return removed;
        }
        if resolved == ROOT {
            return false;
        }
        let under = format!("{resolved}/");
        let held: Vec<String> = self
            .files
            .keys()
            .filter(|path| path.as_str() == resolved || path.starts_with(under.as_str()))
            .cloned()
            .collect();
        let mut removed = false;
        for file in held {
            removed |= self.remove_file(&file);
        }
        // DEEPEST first, so a directory is weighed only once the ones under it have gone.
        let directories: Vec<String> = self
            .directories
            .iter()
            .filter(|path| path.as_str() == resolved || path.starts_with(under.as_str()))
            .cloned()
            .rev()
            .collect();
        for directory in directories {
            if self.holds_anything_under(&directory) {
                continue;
            }
            self.directories.remove(&directory);
            self.modified.remove(&directory);
            self.owners.remove(&directory);
            self.modes.remove(&directory);
            removed = true;
        }
        self.record_parent_written(&resolved);
        removed
    }

    /// Whether anything of this tree lies under the directory `resolved`, which is what keeps
    /// that directory from going away with the subtree around it.
    fn holds_anything_under(&self, resolved: &str) -> bool {
        let under = format!("{resolved}/");
        let holds = |path: &String| path.starts_with(under.as_str());
        self.files.keys().any(holds)
            || self.devices.keys().any(holds)
            || self.directories.iter().any(holds)
    }

    /// What the tree holds at `path`, as a long listing of it reports that entry — and
    /// `None` for a path the tree holds nothing at.
    ///
    /// A FILE's size is the bytes it holds. A DIRECTORY holds the names of the entries under
    /// it, so its size is the bytes those names come to. A DEVICE keeps no bytes of its own,
    /// so its size is none.
    #[must_use]
    pub fn facts(&self, path: &str) -> Option<EntryFacts> {
        let resolved = self.resolve(path)?;
        if let Some(facts) = self.in_mount(&resolved, |files, inside| files.facts(inside)) {
            return facts;
        }
        let (kind, size) = if let Some(contents) = self.files.get(&resolved) {
            (EntryKind::File, contents.len())
        } else if self.directories.contains(&resolved) {
            let held = self
                .entry_names(&resolved)
                .iter()
                .map(|name| name.len())
                .sum();
            (EntryKind::Directory, held)
        } else if self.devices.contains_key(&resolved) {
            (EntryKind::Device, 0)
        } else {
            return None;
        };
        Some(EntryFacts {
            kind,
            size,
            // An entry the tree holds was written when it was put there; a tree whose entry
            // predates this record reports the start of the clock's own epoch rather than an
            // instant nothing happened at.
            modified: self
                .modified
                .get(&resolved)
                .copied()
                .unwrap_or(SystemTime::UNIX_EPOCH),
        })
    }

    /// The names of the entries directly under the DIRECTORY `path`, sorted — what a listing
    /// of a directory names. Empty for a path that is not a directory of this tree.
    #[must_use]
    pub fn entry_names(&self, path: &str) -> Vec<String> {
        let Some(resolved) = self.resolve(path) else {
            return Vec::new();
        };
        if let Some(names) = self.in_mount(&resolved, |files, inside| files.entry_names(inside)) {
            return names;
        }
        if !self.directories.contains(&resolved) {
            return Vec::new();
        }
        let under = if resolved == ROOT {
            ROOT.to_string()
        } else {
            format!("{resolved}/")
        };
        let mut names: Vec<String> = Vec::new();
        let held = self
            .files
            .keys()
            .chain(self.directories.iter())
            .chain(self.devices.keys());
        for entry in held {
            let Some(relative) = entry.strip_prefix(under.as_str()) else {
                continue;
            };
            // Only what lies DIRECTLY under the directory is an entry of it: a deeper path
            // names the directory between them instead.
            let name = relative.split('/').next().unwrap_or(relative);
            if !name.is_empty() && !names.iter().any(|held| held == name) {
                names.push(name.to_string());
            }
        }
        names.sort();
        names
    }

    /// The lines written to the device at `path`, in the order they were written — empty for
    /// a path that is not a keeping device.
    #[must_use]
    pub fn device_lines(&self, path: &str) -> &[String] {
        match self.resolve(path).and_then(|resolved| {
            self.devices.get(&resolved).map(|device| match device {
                Device::Keep(lines) => lines.as_slice(),
                Device::Discard => &[],
            })
        }) {
            Some(lines) => lines,
            None => &[],
        }
    }

    /// The files a program may write, as `(path, contents)` — what the caller keeps beside
    /// the machine so the next run of a program on it reads what this one wrote. The sealed
    /// files are left out: they are the service's own seed, not the machine's storage.
    ///
    /// A file holds BYTES, and these are the ones that are TEXT: a file whose bytes are no
    /// text at all is not among them, because what the callers of this keep beside a machine
    /// is the text its programs wrote. A caller that means EVERY file the tree holds reads
    /// [`Self::stored_bytes`].
    pub fn stored_files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.stored_bytes()
            .filter_map(|(path, contents)| Some((path, ::std::str::from_utf8(contents).ok()?)))
    }

    /// Every file a program may write, as `(path, bytes)` — [`Self::stored_files`] without the
    /// reading that they are text, for a caller that means the files themselves (how many
    /// bytes a filesystem holds, whatever is in them).
    pub fn stored_bytes(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files
            .iter()
            .filter(|(path, _)| !self.sealed.contains(*path))
            .map(|(path, contents)| (path.as_str(), contents.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryKind, ShellVolume, VolumeLimits, ROOT};

    /// RULE: a path is resolved the way a machine resolves one — absolute, with empty, `.`
    /// and `..` segments resolved and `..` above the root staying at the root — and a
    /// relative path is one this tree does not accept at all.
    #[test]
    fn a_path_is_resolved_the_way_a_machine_resolves_one() {
        let volume = ShellVolume::default();
        // (case, the path written, what it resolves to)
        let rows = [
            ("already resolved", "/var/lib/counter", Some("/var/lib/counter")),
            ("a doubled separator", "/var//lib///counter", Some("/var/lib/counter")),
            ("a here segment", "/var/./lib/counter", Some("/var/lib/counter")),
            ("an up segment", "/var/lib/../counter", Some("/var/counter")),
            ("up above the root", "/../../counter", Some("/counter")),
            ("the root itself", "/", Some("/")),
            ("only up segments", "/..", Some("/")),
            ("a relative path", "var/lib/counter", None),
            ("an empty path", "", None),
        ];
        for (case, path, expected) in rows {
            let resolved = volume.resolve(path);
            assert_eq!(
                resolved.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: what a program may do to an entry is what the entry IS — a file it may read and
    /// write, a sealed file it may only read, a keeping device it may only write, a
    /// discarding device that answers nothing — so no write replaces what the service
    /// published to the guest.
    #[test]
    fn what_a_program_may_do_to_an_entry_is_what_the_entry_is() {
        let mut volume = ShellVolume::default();
        volume.put_file("/var/lib/counter", "1");
        volume.seal_file("/proc/sys/kernel/random/boot_id", "a-boot-id");
        volume.add_keeping_device("/dev/console");
        volume.add_discarding_device("/dev/null");

        // (case, path, may be read to, may be written)
        let rows = [
            ("a stored file", "/var/lib/counter", Some("1"), true),
            (
                "a sealed file",
                "/proc/sys/kernel/random/boot_id",
                Some("a-boot-id"),
                false,
            ),
            ("a keeping device", "/dev/console", None, true),
            ("a discarding device", "/dev/null", Some(""), true),
            ("a path the tree does not hold yet", "/var/lib/fresh", None, true),
            ("a directory", "/var/lib", None, false),
        ];
        for (case, path, readable, writable) in rows {
            let read = volume.read(path);
            assert_eq!(
                read.as_deref(),
                readable,
                "case {case:?}: expected a read of {readable:?}, got {read:?}"
            );
            let wrote = volume.write(path, "written\n", true);
            assert_eq!(
                wrote, writable,
                "case {case:?}: expected a write to answer {writable}, got {wrote}"
            );
        }
        assert_eq!(
            volume.read("/proc/sys/kernel/random/boot_id").as_deref(),
            Some("a-boot-id"),
            "the refused write left the sealed file as the service seeded it"
        );
        assert_eq!(
            volume.device_lines("/dev/console"),
            ["written"],
            "the device kept the line written to it, got {:?}",
            volume.device_lines("/dev/console")
        );
    }

    /// RULE: a write to a path whose directory the machine does not have fails, and `mkdir
    /// -p` is what makes it succeed.
    #[test]
    fn a_write_needs_the_directory_above_it() {
        let mut volume = ShellVolume::default();
        assert!(
            !volume.write("/var/lib/sim/counter", "1", false),
            "a write into a directory the tree does not have did not fail"
        );
        assert!(
            volume.make_directories("/var/lib/sim"),
            "mkdir -p did not make the directories"
        );
        assert!(
            volume.write("/var/lib/sim/counter", "1", false),
            "a write into the directory just made did not succeed"
        );
        assert_eq!(volume.read("/var/lib/sim/counter").as_deref(), Some("1"));
    }

    /// RULE: every bound is the volume's own, and a write past one does not happen.
    #[test]
    fn every_bound_refuses_the_write() {
        let limits = VolumeLimits {
            files: 2,
            bytes: 64,
            path_bytes: 32,
            directories: 4,
            device_lines: 2,
            device_line_bytes: 8,
        };
        let mut volume = ShellVolume::new(limits);
        assert!(volume.write("/one", "x", false), "the first file fits");
        assert!(volume.write("/two", "x", false), "the second file fits");
        assert!(
            !volume.write("/three", "x", false),
            "a file past the file count was still written"
        );
        assert!(
            !volume.write("/one", &"x".repeat(128), false),
            "a write past the byte bound was still performed"
        );
        assert!(
            !volume.write(&format!("/{}", "p".repeat(64)), "x", false),
            "a path past the path bound was still accepted"
        );
        volume.add_keeping_device("/dev/console");
        assert!(
            volume.write("/dev/console", "first\nsecond\n", true),
            "the device took the lines that fit"
        );
        assert!(
            !volume.write("/dev/console", "third\n", true),
            "a device write past the line bound was still kept"
        );
        assert_eq!(
            volume.device_lines("/dev/console").len(),
            2,
            "the device kept more lines than its bound allows"
        );
        let mut truncating = ShellVolume::new(limits);
        truncating.add_keeping_device("/dev/console");
        truncating.write("/dev/console", "0123456789abcdef\n", true);
        assert_eq!(
            truncating.device_lines("/dev/console"),
            ["01234567"],
            "a line past the device's line bound was not truncated, got {:?}",
            truncating.device_lines("/dev/console")
        );
    }

    /// RULE: the files a program wrote are what the caller takes back out, and the service's
    /// own sealed seed is not among them.
    #[test]
    fn the_files_a_program_wrote_come_back_out() {
        let mut volume = ShellVolume::default();
        volume.seal_file("/proc/sys/kernel/random/boot_id", "a-boot-id");
        volume.put_file("/var/lib/counter", "1");
        volume.write("/var/lib/counter", "2", false);
        volume.make_directories("/var/log");
        volume.write("/var/log/boot", "up\n", true);
        let stored: Vec<(&str, &str)> = volume.stored_files().collect();
        assert_eq!(
            stored,
            [("/var/lib/counter", "2"), ("/var/log/boot", "up\n")],
            "expected only the files the program may write, got {stored:?}"
        );
        assert!(
            volume.remove_file("/var/log/boot"),
            "a file the program wrote could not be removed"
        );
        assert!(
            !volume.remove_file("/proc/sys/kernel/random/boot_id"),
            "a sealed file was removed by the program"
        );
    }

    /// RULE: what a listing of the tree reports about an entry is the entry's own facts —
    /// what it is, the bytes it holds and when it was last written — and a listing of a
    /// directory names the entries directly under it, each once.
    #[test]
    fn a_listing_reports_each_entrys_own_facts() {
        let mut volume = ShellVolume::default();
        let before = ::std::time::SystemTime::now();
        volume.make_directories("/data/deep/under");
        volume.write("/data/payload", "abcdef\n", false);
        volume.add_discarding_device("/data/sink");

        // (case, path, what it is, the bytes it holds)
        let rows = [
            ("a file", "/data/payload", Some((EntryKind::File, 7))),
            ("a directory", "/data/deep", Some((EntryKind::Directory, "under".len()))),
            ("a device", "/data/sink", Some((EntryKind::Device, 0))),
            ("a path the tree holds nothing at", "/data/absent", None),
            ("a relative path", "data/payload", None),
        ];
        for (case, path, expected) in rows {
            let facts = volume.facts(path);
            let reported = facts.map(|facts| (facts.kind, facts.size));
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            if let Some(facts) = facts {
                assert!(
                    facts.modified >= before,
                    "case {case:?}: expected a write instant at or after {before:?}, got {:?}",
                    facts.modified
                );
            }
        }

        let names = volume.entry_names("/data");
        assert_eq!(
            names,
            ["deep", "payload", "sink"],
            "expected the entries directly under the directory, got {names:?}"
        );
        let root = volume.entry_names("/");
        assert_eq!(
            root, ["data"],
            "expected the root to hold the one directory made under it, got {root:?}"
        );
        let of_a_file = volume.entry_names("/data/payload");
        assert!(
            of_a_file.is_empty(),
            "a path that is not a directory names no entries, got {of_a_file:?}"
        );
    }

    /// RULE: a recursive removal takes the whole subtree away — the files and the directories
    /// under it, and the directory it named — while what a program may not remove keeps itself
    /// and every directory above it, so the answer is never a removal that did not happen.
    ///
    /// The rows span the shapes of a subtree: one holding only files, a nested one, one holding
    /// a sealed file the program may not take away, the file case, the root itself and a path
    /// the tree holds nothing at.
    #[test]
    fn a_recursive_removal_takes_the_subtree_away() {
        let mut volume = ShellVolume::default();
        volume.make_directories("/flat");
        volume.write("/flat/one", "1", false);
        volume.write("/flat/two", "2", false);
        volume.make_directories("/deep/under/here");
        volume.write("/deep/under/here/one", "1", false);
        volume.write("/deep/one", "1", false);
        volume.make_directories("/kept/under");
        volume.seal_file("/kept/under/published", "by the machine");
        volume.write("/alone", "1", false);

        // (case, the path removed, whether anything was removed, what the tree must still hold)
        let rows: [(&str, &str, bool, &[&str]); 6] = [
            ("a directory of files", "/flat", true, &[]),
            ("a nested directory", "/deep", true, &[]),
            (
                "a directory holding a file the program may not remove",
                "/kept",
                false,
                &["/kept", "/kept/under", "/kept/under/published"],
            ),
            ("one file", "/alone", true, &[]),
            ("the root of the tree", "/", false, &["/"]),
            ("a path the tree holds nothing at", "/absent", false, &[]),
        ];
        for (case, path, expected, still_held) in rows {
            let removed = volume.remove_tree(path);
            assert_eq!(
                removed, expected,
                "case {case:?}: expected the removal to answer {expected}, got {removed}"
            );
            for held in still_held {
                assert!(
                    volume.exists(held),
                    "case {case:?}: expected the tree to still hold {held}"
                );
            }
        }
        for gone in [
            "/flat",
            "/flat/one",
            "/deep",
            "/deep/one",
            "/deep/under",
            "/deep/under/here",
            "/deep/under/here/one",
            "/alone",
        ] {
            assert!(
                !volume.exists(gone),
                "expected {gone} to be gone with the subtree it was in"
            );
        }
        assert!(
            volume.exists(ROOT),
            "the root is the tree itself and is never removed"
        );
    }
}
