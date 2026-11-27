// SPDX-License-Identifier: CC0-1.0

//! The BLOCK I/O one instance's GUEST performs on the volumes attached to it: the stable device
//! links the kernel publishes for an attached volume, the program that copies bytes through one,
//! and the bytes that copy leaves ON that volume.
//!
//! # Why a guest needs this
//!
//! A volume is attached in order to be WRITTEN: "after you attach an Amazon EBS volume to your
//! instance, it is exposed as a block device" (EC2 User Guide, "Make an Amazon EBS volume
//! available for use"), and on an instance built on the Nitro System that device carries the
//! volume's own identity — "the volume ID … is in the serial number field" of the NVMe device,
//! reachable through the stable link `/dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_<serial>`
//! (EC2 User Guide, "Amazon EBS and NVMe on Linux instances"). That link is how a program inside
//! the machine addresses EXACTLY the volume it meant to: the kernel's own name for a device
//! depends on the order the devices were discovered, the volume id does not.
//!
//! So an attachment the control plane reports but the guest cannot see is worse than a refusal: a
//! script proceeds, writes nowhere, and nothing can tell which volume the bytes landed on.
//!
//! # One derivation, one place the bytes live
//!
//! The devices here are the ones the guest's own `lsblk` reports
//! ([`super::guest_block_devices`]), read off the one record of an attachment this crate holds —
//! so the link, the serial `lsblk` prints and `DescribeVolumes` cannot disagree about which volume
//! sits at which device.
//!
//! What a write LEAVES is held on the VOLUME ([`super::VolumeRecord::guest_data`]) and not on the
//! machine: that is what makes a read-back answer the bytes that were written, and what makes the
//! data — and the volume's own `AWS/EBS` series ([`super::volume_telemetry`]) — follow the volume
//! when it is detached from one instance and attached to another. A volume that is not attached is
//! not a device of any guest, so nothing can be read or written through it at all.
//!
//! # What is real here, and what is bounded
//!
//! The copy is PERFORMED, over the bytes the machine's own storage and the volume hold, by the
//! same bounded, in-process shell session the guest's own boot script runs in
//! ([`super::guest_console`], which states why that session and not a host process): nothing here
//! opens a device, starts a process or reaches a path of the host. The bytes one volume holds for
//! its guest are bounded ([`MAX_VOLUME_GUEST_BYTES`]) — a write past that is answered the way a
//! write past the end of a device is — and a session's streams and files carry TEXT, which is why
//! the machine's own random source yields printable bytes ([`RANDOM_ALPHABET`]).

use ::std::collections::BTreeMap;
use ::std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use ::std::time::SystemTime;

use ::service_common::posix_shell_session::{
    EnvironmentPrograms, ProgramFiles, ProgramInvocation, ProgramOutcome, ProgramStreams,
};

use ::rand::RngCore;

use super::guest_block_devices::GuestBlockDevice;
use super::ComAmazonawsEc2Amazonec2State;

/// The directory the kernel publishes one STABLE link per block device under, named for what the
/// device itself reports about its identity.
pub const DEVICE_BY_ID_DIRECTORY: &str = "/dev/disk/by-id";

/// What the link of an EBS volume on a Nitro instance is named: the model the NVMe device presents
/// with the volume's serial after it — `nvme-Amazon_Elastic_Block_Store_<volume ID>` (EC2 User
/// Guide, "Amazon EBS and NVMe on Linux instances"), the volume id written without its `-`.
pub const EBS_NVME_LINK_PREFIX: &str = "nvme-Amazon_Elastic_Block_Store_";

/// The name the kernel gives a device presented by the NVMe driver begins with, which is the driver
/// that presents a volume attached to an instance built on the Nitro System — and so the devices a
/// link of [`DEVICE_BY_ID_DIRECTORY`] is published for.
const NVME_NODE_PREFIX: &str = "nvme";

/// The program a guest copies bytes through a device with.
pub const COPY_PROGRAM: &str = "dd";

/// The program a guest resolves a link with.
pub const LINK_READER_PROGRAM: &str = "readlink";

/// The status a program of this guest exits with when it did what it was asked.
const SUCCESS: i32 = 0;

/// The status it exits with when it could not: "non-zero if an error occurred" for both programs
/// here (`dd(1)`, `readlink(1)`).
const FAILED: i32 = 1;

/// What one volume holds for its guest, and the bounds on it — the module this crate keeps those
/// bytes in ([`super::volume_data`]), re-exported here because the programs below are what reads
/// and writes them.
pub use super::volume_data::{VolumeData, DATA_BLOCK_BYTES, MAX_VOLUME_GUEST_BYTES};

/// How many bytes ONE copy may move at once ([`COPY_PROGRAM`]).
///
/// The bytes one copy materialises — the bytes read off a device, the bytes a destination is grown
/// to — are bounded, the invocation being caller-supplied text; and they are bounded by what the
/// DEVICE can take and by nothing smaller: as much as one volume holds for its guest
/// ([`MAX_VOLUME_GUEST_BYTES`]), which is the one ceiling a copy through a device meets. So a copy
/// moves every byte the invocation asked for wherever the device has room for them, and reports a
/// short transfer only where it has none — the way a device at its end answers one — rather than
/// cutting a transfer the device could have taken whole.
pub const MAX_VOLUME_TRANSFER_BYTES: usize = MAX_VOLUME_GUEST_BYTES;

/// How many bytes one RUN of a guest's programs may move through its devices and its storage in
/// TOTAL.
///
/// The bound on the PRODUCT of the two factors above: a run performs many commands
/// ([`super::guest_commands::MAX_GUEST_RUN_COMMANDS`]) and each copy moves at most
/// [`MAX_VOLUME_TRANSFER_BYTES`], so without this a script could make one request copy that
/// product. Every copy of the run is charged against it ([`GuestDisks::allowance`]), and a copy
/// with nothing left to spend answers the way one at the end of a device does.
///
/// It is a bound on the work one request costs and NOT on what the process holds — a copy's bytes
/// are materialised while it is performed and gone after it, and what OUTLIVES a run is bounded
/// where it is kept ([`MAX_GUEST_DATA_BYTES`]). So it is stated for the SEQUENCE a script that uses
/// a volume performs: generating a payload on the machine's own storage, copying it through the
/// device and reading the range back is three copies of a whole one's worth, and a script that does
/// that for several volumes, or checks its work twice, performs several such sequences. Sixteen
/// whole copies — far above any of those, and far below what would cost this process anything
/// noticeable, the bytes of one copy being all it holds at once.
pub const MAX_GUEST_RUN_TRANSFER_BYTES: usize = 16 * MAX_VOLUME_TRANSFER_BYTES;

/// How many bytes of CALLER-WRITTEN GUEST data this store retains, for EACH of the two kinds of it:
/// the machines' own storage ([`super::guest_console::MAX_GUEST_FILE_BYTES`] is what one machine
/// may hold of it) and the volumes' data ([`MAX_VOLUME_GUEST_BYTES`] what one volume may).
///
/// The ceiling on those bytes, and the reason the two bounds above are per-record figures rather
/// than the whole story: a client can create many instances and many volumes, so what the store
/// retains for all of them at once is bounded here. A write that would take it past this is
/// answered the way a write past the end of a device is ([`NO_SPACE`]), and the room comes back
/// when the volume or the instance holding it goes.
///
/// It is counted PER KIND and not over the two together: what a client left on one machine's
/// filesystem is not what refuses a write to some volume's device, and the bytes on a volume are
/// not what leaves a machine's filesystem — which the machine itself reports gibibytes of
/// ([`super::guest_filesystem`]) — with no room for a payload.
pub const MAX_GUEST_DATA_BYTES: usize = 256 * 1024 * 1024;

/// How many of those bytes the store KEEPS for the volumes' data once a run has finished.
///
/// What a guest wrote through a device is the volume's and outlives the run that wrote it, so
/// without this the volumes written EARLIEST would come to hold the whole ceiling above and a copy
/// to a device with room for it would be refused for want of store room — a refusal no device
/// gives, and one that depends on what some other volume was written long before rather than on
/// anything about this one. So this is what the volumes keep between runs, and the rest of the
/// ceiling is the room a run's own writes and the machines' own storage have
/// ([`ComAmazonawsEc2Amazonec2State::reclaim_volume_guest_data`], which takes the bytes back from
/// the volumes whose data a guest has not touched for longest).
pub const MAX_RETAINED_VOLUME_BYTES: usize = MAX_GUEST_DATA_BYTES / 2;

/// The block size one copy uses when the invocation names none: "read and write up to BYTES bytes
/// at a time (default: 512)" (`dd(1)`).
const DEFAULT_BLOCK_BYTES: usize = 512;

/// The device a read answers nothing but zeroes from, and the one that discards what is written to
/// it — the two the kernel publishes on every machine.
const ZERO_DEVICE: &str = "/dev/zero";
const NULL_DEVICE: &str = "/dev/null";

/// The devices a read answers RANDOM bytes from.
const RANDOM_DEVICES: &[&str] = &["/dev/urandom", "/dev/random"];

/// The bytes the machine's random source yields, as the characters they are written with.
///
/// A session's streams and files carry TEXT ([`::service_common::posix_shell_text_filters`]), so
/// the random source of a machine of this emulator yields printable bytes: a payload a guest
/// generates with it is one its own storage can hold, one a digest can be taken over, and one a
/// read-back can be compared against byte for byte.
const RANDOM_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// What `dd` reports when the destination cannot take the bytes: "No space left on device", which
/// is what the end of a device answers.
const NO_SPACE: &str = "No space left on device";

/// ... and what it reports for a path the machine holds nothing at.
const NO_SUCH_FILE: &str = "No such file or directory";

/// ... and for an invocation whose operands it read but cannot perform: a copy from a device that
/// answers without end, bounded by nothing.
const INVALID_ARGUMENT: &str = "Invalid argument";

/// One block device of one guest, as the programs here address it: the volume behind it and how
/// much it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestDisk {
    /// The name the kernel gives the device — `nvme1n1`, `xvdf`.
    pub node: String,
    /// The volume attached there (`VolumeAttachment$VolumeId`).
    pub volume_id: String,
    /// The SERIAL the device presents, which is the volume id without its `-`
    /// ([`GuestBlockDevice::serial`]).
    pub serial: String,
    /// How many bytes the device holds, which is the volume's own size.
    pub capacity: usize,
}

impl GuestDisk {
    /// The device of the guest `device` is, with the volume's size as its capacity.
    #[must_use]
    pub fn of(device: &GuestBlockDevice) -> Self {
        Self {
            node: device.node.clone(),
            volume_id: device.volume_id.clone(),
            serial: device.serial(),
            capacity: usize::try_from(device.size_bytes()).unwrap_or(usize::MAX),
        }
    }

    /// The path of the device node in the guest.
    #[must_use]
    pub fn path(&self) -> String {
        format!("/dev/{}", self.node)
    }

    /// The name of the stable link published for this device.
    #[must_use]
    pub fn by_id_name(&self) -> String {
        format!("{EBS_NVME_LINK_PREFIX}{}", self.serial)
    }

    /// The STABLE link the kernel publishes for this device, for a device the NVMe driver presents
    /// — and `None` for every other, which carries no such link.
    #[must_use]
    pub fn by_id_path(&self) -> Option<String> {
        self.node
            .starts_with(NVME_NODE_PREFIX)
            .then(|| format!("{DEVICE_BY_ID_DIRECTORY}/{}", self.by_id_name()))
    }

    /// What that link HOLDS: the device it points at, as a link of that directory names one —
    /// relative to the directory the link is in, which is two levels under `/dev`.
    #[must_use]
    pub fn by_id_target(&self) -> String {
        format!("../../{}", self.node)
    }

    /// Whether `path` names this device: its node, its own path, or the stable link published for
    /// it — by the whole path or by the link's own name, which is how a program that read that
    /// directory names one.
    #[must_use]
    pub fn is_named_by(&self, path: &str) -> bool {
        let named = path.trim();
        if named == self.path() || named == self.node {
            return true;
        }
        self.by_id_path()
            .is_some_and(|link| named == link || named == self.by_id_name())
    }
}

/// Every file the kernel publishes for `devices` so that a guest can ADDRESS one, as
/// `(path, contents)`: the device node itself and the stable link that points at it.
///
/// The node is published with nothing in it — a block device holds no text, and what it carries is
/// read and written through the copy program below — and the link holds the device it points at,
/// which is what resolving it answers ([`LINK_READER_PROGRAM`]).
#[must_use]
pub fn published_device_files(devices: &[GuestBlockDevice]) -> Vec<(String, String)> {
    let mut published: Vec<(String, String)> = Vec::new();
    for disk in devices.iter().map(GuestDisk::of) {
        published.push((disk.path(), String::new()));
        if let Some(link) = disk.by_id_path() {
            published.push((link, disk.by_id_target()));
        }
    }
    published
}

/// What the guest's I/O did to ONE volume in one run: the operations it performed on it and the
/// bytes they moved.
///
/// The counts are what the volume's own series are published from ([`super::volume_telemetry`]), so
/// the bytes a guest really wrote and the telemetry a client reads about them are one fact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VolumeIo {
    /// The read operations performed on the volume.
    pub read_ops: i64,
    /// The bytes those reads moved.
    pub read_bytes: i64,
    /// The write operations performed on it.
    pub write_ops: i64,
    /// The bytes those writes moved.
    pub write_bytes: i64,
}

impl VolumeIo {
    /// Whether anything happened at all, which is what makes this something to publish.
    #[must_use]
    pub fn happened(&self) -> bool {
        self.read_ops > 0 || self.write_ops > 0
    }
}

/// What one run of a guest's programs LEFT on the volumes attached to it: the bytes each of them
/// holds now, and the I/O that put them there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestDiskWrites {
    /// The data each volume holds now, by `VolumeId` — only the volumes something read or wrote.
    pub data: BTreeMap<String, VolumeData>,
    /// What the guest's I/O did to each of them, by `VolumeId`.
    pub io: BTreeMap<String, VolumeIo>,
}

/// The DEVICES one guest can address, and what its programs have done to them so far.
///
/// Shared with the programs the session runs ([`disk_programs`]) the way the machine's running
/// services are ([`super::guest_services::GuestServices`]): a program performs the copy against
/// this, and the caller that reached into the guest takes what the run left ([`Self::taken`]) and
/// writes it back onto the volumes
/// ([`ComAmazonawsEc2Amazonec2State::apply_guest_disk_writes`]). Nothing of the store is reachable
/// from a program — no guard, no engine and no host handle, only these bytes.
#[derive(Debug, Clone)]
pub struct GuestDisks {
    /// The devices of this guest, read once when the programs were installed.
    disks: Arc<Vec<GuestDisk>>,
    /// What the run has done to them, shared with the programs it runs.
    performed: Arc<Mutex<DiskActivity>>,
}

/// What one run has done to the devices it can address: what it LEFT on them, how many bytes its
/// copies have moved so far — which is what the run's own budget is spent from
/// ([`MAX_GUEST_RUN_TRANSFER_BYTES`]) — and how many more bytes the STORE has room to hold for its
/// volumes ([`MAX_GUEST_DATA_BYTES`]).
#[derive(Debug, Default)]
struct DiskActivity {
    /// What the run left on the volumes.
    written: GuestDiskWrites,
    /// The bytes its copies have moved, read and written together.
    moved: usize,
    /// The bytes the store has room to hold for these volumes.
    room: usize,
}

impl GuestDisks {
    /// The devices `devices` are, each starting from the bytes `data` holds for its volume, with
    /// room for as much as one volume may hold.
    ///
    /// A caller that knows what the STORE has room for states it instead ([`Self::with_room`]).
    #[must_use]
    pub fn holding(devices: &[GuestBlockDevice], data: BTreeMap<String, VolumeData>) -> Self {
        Self {
            disks: Arc::new(devices.iter().map(GuestDisk::of).collect()),
            performed: Arc::new(Mutex::new(DiskActivity {
                written: GuestDiskWrites {
                    data,
                    io: BTreeMap::new(),
                },
                moved: 0,
                room: MAX_VOLUME_GUEST_BYTES,
            })),
        }
    }

    /// The same devices, with `room` as the bytes the STORE has room to hold for them
    /// ([`MAX_GUEST_DATA_BYTES`]): a write past it is a short write, the way a write past the end
    /// of a device is.
    #[must_use]
    pub fn with_room(self, room: usize) -> Self {
        {
            let mut held = self.held();
            held.room = room;
        }
        self
    }

    /// A guest with NO device at all, which is a machine nothing is attached to.
    #[must_use]
    pub fn none() -> Self {
        Self::holding(&[], BTreeMap::new())
    }

    /// The devices this guest can address.
    #[must_use]
    pub fn disks(&self) -> &[GuestDisk] {
        &self.disks
    }

    /// What the run left: the bytes each volume holds now and the I/O that put them there.
    #[must_use]
    pub fn taken(&self) -> GuestDiskWrites {
        self.held().written.clone()
    }

    /// How many bytes a copy of this run may still move: what one copy may move at most, less what
    /// the run has already spent of its own budget.
    fn allowance(&self) -> usize {
        MAX_GUEST_RUN_TRANSFER_BYTES
            .saturating_sub(self.held().moved)
            .min(MAX_VOLUME_TRANSFER_BYTES)
    }

    /// Charge `bytes` against that budget, which is what a copy that moved them spends.
    fn charge(&self, bytes: usize) {
        let mut held = self.held();
        held.moved = held.moved.saturating_add(bytes);
    }

    /// Record that `bytes` were WRITTEN through the volume `volume_id` by something other than the
    /// copy program: a write to a file of the machine's own storage, which is a write through the
    /// volume that storage IS.
    ///
    /// Only the I/O is recorded, never the bytes: those are the machine's filesystem's to keep
    /// ([`super::guest_console`]), and what the volume's own series measure is the transfer
    /// ([`super::volume_telemetry`]).
    fn wrote_through(&self, volume_id: &str, bytes: usize) {
        let moved = i64::try_from(bytes).unwrap_or(i64::MAX);
        if moved <= 0 {
            return;
        }
        let mut held = self.held();
        let performed = held.written.io.entry(volume_id.to_string()).or_default();
        performed.write_bytes = performed.write_bytes.saturating_add(moved);
        performed.write_ops = performed
            .write_ops
            .saturating_add(super::volume_telemetry::operations_for(moved));
    }

    /// The shared record, with a lock an earlier panic poisoned taken rather than propagated: the
    /// bytes of a machine outlive the program that was writing them.
    fn held(&self) -> MutexGuard<'_, DiskActivity> {
        self.performed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The device `path` names, or `None` for a path that is no device of this guest.
    fn disk_at(&self, path: &str) -> Option<GuestDisk> {
        self.disks
            .iter()
            .find(|disk| disk.is_named_by(path))
            .cloned()
    }

    /// Read `wanted` bytes of `disk` from `offset`, as the device answers them: the bytes the volume
    /// holds there, zeroes for the part of the device nothing has written, and nothing at all past
    /// its end.
    ///
    /// One read answers at most [`MAX_VOLUME_TRANSFER_BYTES`], whatever the device's own size and
    /// whatever the invocation asked for: the answer is MATERIALISED here, and the invocation is
    /// caller-supplied text, so a request for more is a SHORT read — which is what a device answers
    /// at its end, and what the program then reports it moved.
    fn read(&self, disk: &GuestDisk, offset: usize, wanted: usize) -> Vec<u8> {
        let length = disk
            .capacity
            .saturating_sub(offset)
            .min(wanted)
            .min(MAX_VOLUME_TRANSFER_BYTES);
        if length == 0 {
            return Vec::new();
        }
        let mut held = self.held();
        let answered = held
            .written
            .data
            .entry(disk.volume_id.clone())
            .or_default()
            .read(offset, length);
        let performed = held
            .written
            .io
            .entry(disk.volume_id.clone())
            .or_default();
        let moved = i64::try_from(answered.len()).unwrap_or(i64::MAX);
        performed.read_bytes = performed.read_bytes.saturating_add(moved);
        performed.read_ops = performed
            .read_ops
            .saturating_add(super::volume_telemetry::operations_for(moved));
        answered
    }

    /// Write `bytes` to `disk` at `offset`, answering how many of them the device took: the whole of
    /// them, or as many as fit before its end — and as many as the volume can hold for its guest
    /// ([`MAX_VOLUME_GUEST_BYTES`]).
    fn write(&self, disk: &GuestDisk, offset: usize, bytes: &[u8]) -> usize {
        let room = disk.capacity.saturating_sub(offset);
        let writing = room.min(bytes.len()).min(MAX_VOLUME_TRANSFER_BYTES);
        if writing == 0 {
            return 0;
        }
        let mut held = self.held();
        // What the STORE has room to hold for these volumes, which is what another block of this
        // one is taken out of.
        let may_hold = held.room;
        let data = held
            .written
            .data
            .entry(disk.volume_id.clone())
            .or_default();
        let before = data.held_bytes();
        let taken = data.write(offset, &bytes[..writing], may_hold);
        let added = data.held_bytes().saturating_sub(before);
        held.room = held.room.saturating_sub(added);
        if taken == 0 {
            return 0;
        }
        let performed = held
            .written
            .io
            .entry(disk.volume_id.clone())
            .or_default();
        let moved = i64::try_from(taken).unwrap_or(i64::MAX);
        performed.write_bytes = performed.write_bytes.saturating_add(moved);
        performed.write_ops = performed
            .write_ops
            .saturating_add(super::volume_telemetry::operations_for(moved));
        taken
    }
}

impl ::service_common::posix_shell_volume::BlockDeviceBytes for GuestDisks {    /// The bytes the block device at `path` holds, as a READ of the device from its start answers
    /// them: what the volume behind it holds, and zeroes for the part of it nothing has written.
    ///
    /// Why a guest's tree is given this: a block device is not a file of the machine's
    /// filesystem, so what it holds is not what the tree keeps at the path the kernel publishes
    /// the device at — it is the volume's own bytes, held here. A program of the guest reads a
    /// device with whatever utility it likes (a digest, `cat`, `head`, the copy program), and
    /// every one of them has to read those bytes: a device answering nothing to all but one
    /// utility is a device presenting none of what was just written to it.
    ///
    /// Bounded exactly like a read the copy program performs ([`MAX_VOLUME_TRANSFER_BYTES`], and
    /// what the run has left of its own budget): the answer is MATERIALISED here, so a device
    /// larger than that answers its first bytes — the short read a device gives at the end of
    /// what one read may move — and the read is charged against the run that performed it and
    /// counted into the volume's own series, because it is I/O on that volume like any other.
    fn device_bytes(&self, path: &str) -> Option<Vec<u8>> {
        let disk = self.disk_at(path)?;
        let allowance = self.allowance();
        if allowance == 0 {
            return Some(Vec::new());
        }
        let answered = self.read(&disk, 0, allowance);
        self.charge(answered.len());
        Some(answered)
    }
}

/// Where a write to a file of the machine's OWN STORAGE is reported: the volume that storage IS,
/// which is the instance's ROOT volume — and `None` for a machine whose devices carry no root
/// volume, whose filesystem is on nothing this store holds.
///
/// Why a guest's tree reports its writes here: the root filesystem of an instance IS an attached
/// Amazon EBS volume, so the bytes a program writes into a file of it are bytes written THROUGH
/// that volume, which is what the volume's own series measure ("the total number of bytes
/// transferred", `VolumeWriteBytes`). A root volume whose series knew nothing about those writes
/// would be a volume taking a client's whole payload and reporting no write at all, while the same
/// payload written through a data volume's device reports one.
///
/// The BYTES stay where they are: a file of the machine's storage is held by that machine
/// ([`super::guest_console`]), and what goes onto the volume here is the I/O, not a second copy of
/// the file.
#[must_use]
pub fn storage_write_sink(
    disks: &GuestDisks,
    devices: &[GuestBlockDevice],
) -> Option<::std::sync::Arc<dyn ::service_common::posix_shell_volume::FileWrites>> {
    let volume_id = devices
        .iter()
        .find(|device| device.root)
        .map(|device| device.volume_id.clone())?;
    Some(Arc::new(StorageWrites {
        disks: disks.clone(),
        volume_id,
    }))
}

/// The sink [`storage_write_sink`] hands over: the devices the writes are recorded against, and
/// the volume the machine's own storage is.
#[derive(Debug)]
struct StorageWrites {
    /// What the run has done to the machine's volumes.
    disks: GuestDisks,
    /// The volume the machine's root filesystem is.
    volume_id: String,
}

impl ::service_common::posix_shell_volume::FileWrites for StorageWrites {
    /// One write the machine's filesystem took, as I/O on the volume that filesystem is: the bytes
    /// the file holds now are what the filesystem put on the volume for it.
    fn wrote(&self, written: &::service_common::posix_shell_volume::FileWritten<'_>) {
        self.disks
            .wrote_through(&self.volume_id, written.contents.len());
    }

    /// A removal moves no bytes onto the volume, so it is no transfer to measure.
    fn removed(&self, path: &str) {
        let _ = path;
    }
}

/// The programs a guest performs block I/O with, for a session that runs a program of that guest.
#[must_use]
pub fn disk_programs(disks: &GuestDisks) -> Box<dyn EnvironmentPrograms> {
    Box::new(DiskPrograms {
        disks: disks.clone(),
    })
}

/// The block-I/O programs of one machine's guest.
#[derive(Debug)]
struct DiskPrograms {
    /// The devices they work over, and what they have done to them.
    disks: GuestDisks,
}

impl EnvironmentPrograms for DiskPrograms {
    /// Neither program answers out of its operands alone — the copy reads the stream it was handed
    /// — so the invocation form below is the one this environment performs.
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        let _ = (name, arguments, files);
        None
    }

    /// `dd` copies bytes between the machine's storage, its devices and its streams; `readlink`
    /// resolves a link of the machine.
    ///
    /// A program named by its absolute path is the same program, as a shell resolving a name on
    /// `PATH` finds. An invocation this guest performs nothing of — an operand form neither program
    /// reads — answers `None` rather than an outcome that would not be the answer to what was
    /// asked.
    fn run_invocation(
        &self,
        invocation: &ProgramInvocation<'_>,
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramStreams> {
        let named = invocation.name;
        let named = named.rsplit('/').next().unwrap_or(named);
        match named {
            COPY_PROGRAM => {
                let transfer = Transfer::of(invocation.arguments)?;
                Some(transfer.performed(&self.disks, invocation.standard_input, files))
            }
            LINK_READER_PROGRAM => resolved_links(invocation.arguments, &self.disks, files),
            _ => None,
        }
    }
}

/// What one `dd` invocation copies: from where to where, in what blocks, how much of it and from
/// which offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transfer {
    /// `if=FILE`: "read from FILE instead of stdin".
    input: Option<String>,
    /// `of=FILE`: "write to FILE instead of stdout".
    output: Option<String>,
    /// `ibs=BYTES`: "read up to BYTES bytes at a time".
    input_block: usize,
    /// `obs=BYTES`: "write BYTES bytes at a time".
    output_block: usize,
    /// `count=N`: "copy only N input blocks" — `None` for a copy that reads its input to the end.
    count: Option<usize>,
    /// `skip=N`: "skip N ibs-sized input blocks".
    skip: usize,
    /// `seek=N`: "skip N obs-sized output blocks".
    seek: usize,
    /// `conv=notrunc`: "do not truncate the output file", which is what makes a copy into a file
    /// replace a RANGE of it rather than the whole of it.
    keep_rest: bool,
    /// Whether the invocation asks for the transfer statistics at all: `status=none`
    /// "suppress[es] everything but error messages".
    report: bool,
    /// Whether it asks for the TRANSFER figures beside the record counts: `status=noxfer`
    /// "suppress[es] the final transfer statistics", leaving the records the copy moved.
    transfer_figures: bool,
    /// `iflag=count_bytes`: "treat 'count=N' as a byte count", rather than a count of blocks.
    count_in_bytes: bool,
    /// `iflag=skip_bytes`: the same for `skip=N`.
    skip_in_bytes: bool,
    /// `oflag=seek_bytes`: the same for `seek=N`.
    seek_in_bytes: bool,
}

/// The operands whose value only changes HOW the copy is performed, each read below — so an operand
/// naming something else is an invocation this guest performs nothing of.
const TRANSFER_FLAG_OPERANDS: &[&str] = &["iflag", "oflag", "conv"];

/// The values those operands take: the ways a real copy asks for its writes to reach the device
/// before it answers, the ways it asks for a whole block to be read at a time, and the ones it
/// passes over an error with.
///
/// Every one of them is honoured by this copy already — a write reaches the volume as part of
/// performing it, there being no cache between them, and a read of this machine's own storage
/// answers the whole of what was asked for — beside the four below, which change what the copy
/// does.
const TRANSFER_FLAG_VALUES: &[&str] = &[
    "direct",
    "dsync",
    "sync",
    "fsync",
    "fdatasync",
    "nocache",
    "noatime",
    "nonblock",
    "noerror",
    "nocreat",
    "sparse",
    "fullblock",
    "excl",
];

/// The value of [`TRANSFER_FLAG_OPERANDS`] that keeps the rest of the destination.
const KEEP_REST_FLAG: &str = "notrunc";

/// ... and the three that state a count in BYTES rather than in blocks.
const COUNT_BYTES_FLAG: &str = "count_bytes";
const SKIP_BYTES_FLAG: &str = "skip_bytes";
const SEEK_BYTES_FLAG: &str = "seek_bytes";

impl Transfer {
    /// What `arguments` ask for, or `None` for an invocation this guest performs nothing of: an
    /// operand it does not read, a size it cannot read, or a flag value it does not know.
    fn of(arguments: &[String]) -> Option<Self> {
        let mut transfer = Self {
            input: None,
            output: None,
            input_block: DEFAULT_BLOCK_BYTES,
            output_block: DEFAULT_BLOCK_BYTES,
            count: None,
            skip: 0,
            seek: 0,
            keep_rest: false,
            report: true,
            transfer_figures: true,
            count_in_bytes: false,
            skip_in_bytes: false,
            seek_in_bytes: false,
        };
        for argument in arguments {
            let (operand, value) = argument.split_once('=')?;
            match operand {
                "if" => transfer.input = Some(value.to_string()),
                "of" => transfer.output = Some(value.to_string()),
                "bs" => {
                    let size = block_bytes(value)?;
                    transfer.input_block = size;
                    transfer.output_block = size;
                }
                "ibs" => transfer.input_block = block_bytes(value)?,
                "obs" => transfer.output_block = block_bytes(value)?,
                "count" => transfer.count = Some(block_bytes(value)?),
                "skip" => transfer.skip = block_bytes(value)?,
                "seek" => transfer.seek = block_bytes(value)?,
                "status" => match value {
                    "none" => transfer.report = false,
                    "noxfer" => transfer.transfer_figures = false,
                    "progress" => transfer.report = true,
                    _ => return None,
                },
                named if TRANSFER_FLAG_OPERANDS.contains(&named) => {
                    for flag in value.split(',').filter(|flag| !flag.is_empty()) {
                        match flag {
                            // The three flags that change what a COUNT is stated in.
                            COUNT_BYTES_FLAG => transfer.count_in_bytes = true,
                            SKIP_BYTES_FLAG => transfer.skip_in_bytes = true,
                            SEEK_BYTES_FLAG => transfer.seek_in_bytes = true,
                            KEEP_REST_FLAG => transfer.keep_rest = true,
                            // ... and the ones this copy honours already.
                            flag if TRANSFER_FLAG_VALUES.contains(&flag) => {}
                            _ => return None,
                        }
                    }
                }
                _ => return None,
            }
        }
        (transfer.input_block > 0 && transfer.output_block > 0).then_some(transfer)
    }

    /// How many bytes this copy reads at most, and `None` for one that reads its input to the end.
    fn wanted(&self) -> Option<usize> {
        let unit = if self.count_in_bytes {
            1
        } else {
            self.input_block
        };
        self.count.map(|count| count.saturating_mul(unit))
    }

    /// The offset in the INPUT this copy starts at, which `skip` states in blocks — or in bytes,
    /// for an invocation that asked for that (`iflag=skip_bytes`: "treat 'skip=N' as a byte
    /// count").
    fn skip_offset(&self) -> usize {
        let unit = if self.skip_in_bytes {
            1
        } else {
            self.input_block
        };
        self.skip.saturating_mul(unit)
    }

    /// The offset in the OUTPUT it writes at, which `seek` states the same two ways
    /// (`oflag=seek_bytes`).
    fn seek_offset(&self) -> usize {
        let unit = if self.seek_in_bytes {
            1
        } else {
            self.output_block
        };
        self.seek.saturating_mul(unit)
    }

    /// Perform the copy, answering what `dd` wrote on each of its streams.
    fn performed(
        &self,
        disks: &GuestDisks,
        standard_input: Option<&str>,
        files: &mut dyn ProgramFiles,
    ) -> ProgramStreams {
        // What this run may still move ([`MAX_GUEST_RUN_TRANSFER_BYTES`]): a copy is charged
        // against the run's own budget, so the bytes one request can make this process
        // materialise are bounded by that budget rather than by the commands it carries.
        let allowance = disks.allowance();
        if allowance == 0 {
            return refused(format!("{COPY_PROGRAM}: {NO_SPACE}"));
        }
        // The copy is performed in process, and this is how long performing it TOOK: the figure the
        // report states is measured here rather than claimed.
        let started = ::std::time::Instant::now();
        let read = match self.source(disks, standard_input, files, allowance) {
            Ok(bytes) => bytes,
            Err(diagnostic) => return refused(diagnostic),
        };
        disks.charge(read.len());
        let (written, output, short) = match self.destination(&read, disks, files) {
            Ok(answered) => answered,
            Err(diagnostic) => return refused(diagnostic),
        };
        let took = started.elapsed();
        let mut errors: Vec<String> = Vec::new();
        let failed = short.is_some();
        errors.extend(short);
        if self.report {
            errors.extend(self.statistics(read.len(), written, took));
        }
        ProgramStreams {
            status: if failed { FAILED } else { SUCCESS },
            output,
            errors,
        }
    }

    /// The bytes this copy READS, at most `allowance` of them, or the diagnostic `dd` writes for an
    /// input it could not read.
    fn source(
        &self,
        disks: &GuestDisks,
        standard_input: Option<&str>,
        files: &dyn ProgramFiles,
        allowance: usize,
    ) -> Result<Vec<u8>, String> {
        let Some(path) = self.input.as_deref() else {
            // "read from FILE instead of stdin": with no input named, the copy reads the stream the
            // stage before it wrote, or the redirection it was given.
            return Ok(self.taken_from(standard_input.unwrap_or_default().as_bytes(), allowance));
        };
        if path == NULL_DEVICE {
            return Ok(Vec::new());
        }
        if let Some(disk) = disks.disk_at(path) {
            // Bounded by what this copy may move, whatever the device's own size: the read is
            // materialised, so a request for more answers a short read ([`GuestDisks::read`]).
            let wanted = self.wanted().unwrap_or(disk.capacity).min(allowance);
            return Ok(disks.read(&disk, self.skip_offset(), wanted));
        }
        if path == ZERO_DEVICE || RANDOM_DEVICES.contains(&path) {
            // A device that answers without end: what it answers is bounded only by the count the
            // invocation asked for, so one that named none is a copy this guest performs nothing
            // of.
            let wanted = self
                .wanted()
                .ok_or_else(|| format!("{COPY_PROGRAM}: reading '{path}': {INVALID_ARGUMENT}"))?
                .min(allowance);
            return Ok(if path == ZERO_DEVICE {
                vec![0_u8; wanted]
            } else {
                random_bytes(wanted)
            });
        }
        let held = files
            .read(path)
            .ok_or_else(|| format!("{COPY_PROGRAM}: failed to open '{path}': {NO_SUCH_FILE}"))?;
        Ok(self.taken_from(held.as_bytes(), allowance))
    }

    /// The part of `read` this copy takes: from the offset `skip` names, as much as `count` asks
    /// for, and never more than `allowance`.
    fn taken_from(&self, read: &[u8], allowance: usize) -> Vec<u8> {
        let from = self.skip_offset().min(read.len());
        let rest = &read[from..];
        let wanted = self.wanted().unwrap_or(rest.len()).min(allowance);
        rest[..wanted.min(rest.len())].to_vec()
    }

    /// Put `read` where this copy writes it, answering how many bytes went there, the lines it wrote
    /// to standard output and the diagnostic of a SHORT write — or the diagnostic `dd` writes for a
    /// destination it could not write at all.
    #[allow(clippy::type_complexity)]
    fn destination(
        &self,
        read: &[u8],
        disks: &GuestDisks,
        files: &mut dyn ProgramFiles,
    ) -> Result<(usize, Vec<String>, Option<String>), String> {
        let Some(path) = self.output.as_deref() else {
            // "write to FILE instead of stdout".
            return Ok((read.len(), stdout_lines(read), None));
        };
        if path == NULL_DEVICE {
            return Ok((read.len(), Vec::new(), None));
        }
        let at = self.seek_offset();
        if let Some(disk) = disks.disk_at(path) {
            let written = disks.write(&disk, at, read);
            let short = (written < read.len())
                .then(|| format!("{COPY_PROGRAM}: writing to '{path}': {NO_SPACE}"));
            return Ok((written, Vec::new(), short));
        }
        // A destination on the machine's own storage: the bytes replace what the file holds, from
        // the offset the invocation named, and the rest of it is kept only where the invocation
        // asked for that.
        //
        // The file is GROWN to that offset, so the bytes between its start and the offset are
        // materialised too: they are bounded and charged exactly like the bytes read, before
        // anything is allocated, and a copy whose budget cannot cover them is answered the way a
        // write past the end of a device is.
        if at > disks.allowance() {
            return Err(format!("{COPY_PROGRAM}: writing '{path}': {NO_SPACE}"));
        }
        disks.charge(at);
        let end = at.saturating_add(read.len());
        let mut held: Vec<u8> = if self.keep_rest || at > 0 {
            files.read(path).unwrap_or_default().into_bytes()
        } else {
            Vec::new()
        };
        if held.len() < end {
            held.resize(end, 0);
        }
        held[at..end].copy_from_slice(read);
        if !self.keep_rest {
            held.truncate(end);
        }
        let contents = String::from_utf8(held)
            .map_err(|_| format!("{COPY_PROGRAM}: writing '{path}': {INVALID_ARGUMENT}"))?;
        if !files.write(path, &contents) {
            return Err(format!("{COPY_PROGRAM}: writing '{path}': {NO_SPACE}"));
        }
        Ok((read.len(), Vec::new(), None))
    }

    /// The transfer statistics `dd` reports: the blocks it read, the blocks it wrote, and — unless
    /// the invocation asked for the record counts alone — the bytes it copied, how long that took
    /// and the rate it came to.
    ///
    /// The line is the utility's own: `"<bytes> bytes (<decimal>, <binary>) copied, <seconds> s,
    /// <rate>/s"`, with the parenthetical left out below the first multiple, which is what `dd`
    /// writes. The duration and the rate are MEASURED: the copy is performed in process, so how
    /// long it took is a span this process really spent ([`Self::performed`]) rather than a figure
    /// nothing timed.
    fn statistics(&self, read: usize, written: usize, took: ::std::time::Duration) -> Vec<String> {
        let mut lines = vec![
            format!("{} records in", blocks(read, self.input_block)),
            format!("{} records out", blocks(written, self.output_block)),
        ];
        if self.transfer_figures {
            let seconds = took.as_secs_f64().max(f64::EPSILON);
            #[allow(clippy::cast_precision_loss)]
            let rate = written as f64 / seconds;
            lines.push(format!(
                "{written} bytes{} copied, {} s, {}/s",
                multiples_of(written),
                general(seconds),
                decimal_multiple(rate)
            ));
        }
        lines
    }
}

/// The invocation `dd` could not perform at all, as the status and the diagnostic it answers with.
fn refused(diagnostic: String) -> ProgramStreams {
    ProgramStreams {
        status: FAILED,
        output: Vec::new(),
        errors: vec![diagnostic],
    }
}

/// How many blocks of `block` bytes `bytes` came to, as `dd` counts them: the whole blocks and
/// whether a partial one followed ("records in" is written `N+M`).
fn blocks(bytes: usize, block: usize) -> String {
    let block = block.max(1);
    let whole = bytes / block;
    let partial = usize::from(!bytes.is_multiple_of(block));
    format!("{whole}+{partial}")
}

/// The multiples of `bytes` `dd` states beside the figure itself, in the parentheses it writes
/// them in — the decimal one and the binary one — and NOTHING for a figure below the first
/// multiple, which is what the utility writes for one.
fn multiples_of(bytes: usize) -> String {
    if bytes < 1000 {
        return String::new();
    }
    #[allow(clippy::cast_precision_loss)]
    let figure = bytes as f64;
    format!(
        " ({}, {})",
        decimal_multiple(figure),
        binary_multiple(figure)
    )
}

/// `figure` in the largest DECIMAL multiple of a byte it is at least one of, as the utility writes
/// one: a thousand to each step.
fn decimal_multiple(figure: f64) -> String {
    scaled(figure, 1000.0, &["B", "kB", "MB", "GB", "TB"])
}

/// ... and in the largest BINARY multiple of a byte, which steps by 1024.
fn binary_multiple(figure: f64) -> String {
    scaled(figure, 1024.0, &["B", "KiB", "MiB", "GiB", "TiB"])
}

/// `figure` in the largest of `units` it is at least one of, each step being `base` of the one
/// below.
///
/// Written as the utility writes a human-readable figure: one decimal place below ten, and the
/// nearest whole number above it.
fn scaled(figure: f64, base: f64, units: &[&str]) -> String {
    let mut value = figure;
    let mut unit = 0;
    while value >= base && unit + 1 < units.len() {
        value /= base;
        unit += 1;
    }
    let named = units[unit];
    if unit == 0 {
        return format!("{value:.0} {named}");
    }
    if value < 10.0 {
        format!("{value:.1} {named}")
    } else {
        format!("{value:.0} {named}")
    }
}

/// `value` as a general floating-point figure, the way the utility writes the seconds it took: six
/// significant digits, in exponent form for a figure too small to write otherwise, with the digits
/// that carry nothing dropped.
fn general(value: f64) -> String {
    if value != 0.0 && value.abs() < 1e-4 {
        return format!("{value:e}");
    }
    let rendered = format!("{value:.6}");
    let trimmed = rendered.trim_end_matches('0');
    let trimmed = trimmed.trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `data` as the LINES a program writes it on standard output.
///
/// A program's output reaches the session one line at a time, each closed by a newline, so a payload
/// that is itself newline-terminated — which is what the machine's random source yields — travels
/// through a pipeline byte for byte.
fn stdout_lines(data: &[u8]) -> Vec<String> {
    if data.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(data);
    let body = text.strip_suffix('\n').unwrap_or(&text);
    body.split('\n').map(str::to_string).collect()
}

/// The size `value` states, as `dd` reads one: a number of bytes, with the multiplier its suffix
/// names ("BYTES may be followed by the following multiplicative suffixes", `dd(1)`) — which is
/// also how it reads a number of BLOCKS.
fn block_bytes(value: &str) -> Option<usize> {
    let digits = value.trim_end_matches(|character: char| !character.is_ascii_digit());
    let suffix = value.get(digits.len()..)?;
    let count: usize = digits.parse().ok()?;
    let multiplier: usize = match suffix {
        "" | "c" => 1,
        "w" => 2,
        "b" => 512,
        "kB" => 1_000,
        "K" | "k" | "KiB" => 1024,
        "MB" => 1_000_000,
        "M" | "m" | "MiB" => 1024 * 1024,
        "GB" => 1_000_000_000,
        "G" | "g" | "GiB" => 1024 * 1024 * 1024,
        _ => return None,
    };
    count.checked_mul(multiplier)
}

/// `wanted` random bytes, drawn from the alphabet a machine of this emulator's random source yields
/// ([`RANDOM_ALPHABET`]) and closed by a newline, so what a guest generates with it is text its own
/// storage holds and a pipeline carries unchanged.
fn random_bytes(wanted: usize) -> Vec<u8> {
    if wanted == 0 {
        return Vec::new();
    }
    // The bytes themselves come from the maintained generator, so two reads of the machine's
    // random source answer different bytes; what they are written WITH is the alphabet above.
    let mut drawn = vec![0_u8; wanted];
    ::rand::rng().fill_bytes(&mut drawn);
    for byte in &mut drawn {
        let index = usize::from(*byte) % RANDOM_ALPHABET.len();
        *byte = RANDOM_ALPHABET[index];
    }
    // The stream a guest reads is text, and text ends with a newline.
    if let Some(last) = drawn.last_mut() {
        *last = b'\n';
    }
    drawn
}

/// What `readlink` answers for `arguments` over this machine, or `None` for an invocation this guest
/// performs nothing of.
///
/// The links of a machine of this emulator are the ones the kernel publishes for its block devices
/// ([`published_device_files`]): resolving one answers the device it points at — the device's own
/// path for "canonicalize by following every symlink" (`-f`), and the target as the link holds it
/// otherwise. A path that is not a link of this machine is not one `readlink` answers for, which is
/// what the utility's own non-zero status says.
fn resolved_links(
    arguments: &[String],
    disks: &GuestDisks,
    files: &dyn ProgramFiles,
) -> Option<ProgramStreams> {
    let mut canonical = false;
    let mut operands: Vec<&String> = Vec::new();
    for argument in arguments {
        if let Some(name) = argument.strip_prefix("--") {
            match name {
                "canonicalize" | "canonicalize-existing" | "canonicalize-missing" => {
                    canonical = true;
                }
                // "suppress most error messages" and "do not output the trailing delimiter":
                // neither changes what is written for a link this machine holds.
                "quiet" | "silent" | "no-newline" | "verbose" => {}
                _ => return None,
            }
            continue;
        }
        if let Some(letters) = argument.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            for letter in letters.chars() {
                match letter {
                    'f' | 'e' | 'm' => canonical = true,
                    'q' | 's' | 'n' | 'v' => {}
                    _ => return None,
                }
            }
            continue;
        }
        operands.push(argument);
    }
    if operands.is_empty() {
        return None;
    }
    let mut output: Vec<String> = Vec::new();
    let mut status = SUCCESS;
    for operand in operands {
        match disks.disk_at(operand) {
            // The link of a device: the device it points at, as the invocation asked for it.
            Some(disk) if names_a_link(operand) => output.push(if canonical {
                disk.path()
            } else {
                disk.by_id_target()
            }),
            // A path of the machine that is not a link — a device node, a file — is answered only
            // for a canonicalizing invocation, which writes the path itself.
            _ if canonical && files.read(operand).is_some() => output.push(operand.to_string()),
            _ => status = FAILED,
        }
    }
    Some(ProgramStreams {
        status,
        output,
        errors: Vec::new(),
    })
}

/// Whether `path` names a LINK of this machine rather than the device itself, which is what a
/// non-canonicalizing `readlink` answers the target of.
fn names_a_link(path: &str) -> bool {
    path.trim().contains(EBS_NVME_LINK_PREFIX)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The DEVICES the guest holding `devices` can address, each starting from the bytes its volume
    /// holds.
    ///
    /// The one place a guest's devices are handed to the programs that perform I/O on them, so the
    /// boot's own program and a command something later sends the guest read and write one set of
    /// volumes.
    pub(super) fn guest_disks(&self, devices: &[GuestBlockDevice]) -> GuestDisks {
        let data: BTreeMap<String, VolumeData> = devices
            .iter()
            .filter_map(|device| {
                let record = self.volumes.get(&device.volume_id)?;
                Some((device.volume_id.clone(), record.guest_data.clone()))
            })
            .collect();
        // What the STORE has room to hold for these volumes while this run writes them: its own
        // ceiling less what it holds for the account's OTHER volumes, and what these volumes hold
        // already is theirs to keep. What the machines' own storage holds is not in it: the two
        // kinds of guest data are counted on their own ([`MAX_GUEST_DATA_BYTES`]), so a payload a
        // client left on a machine's filesystem is never what refuses a write to a device.
        let held_here: usize = data.values().map(VolumeData::held_bytes).sum();
        let room = MAX_GUEST_DATA_BYTES
            .saturating_sub(self.volume_guest_data_bytes())
            .saturating_add(held_here);
        GuestDisks::holding(devices, data).with_room(room)
    }

    /// How many bytes of CALLER-WRITTEN guest data this store holds for the VOLUMES now, which is
    /// what [`MAX_GUEST_DATA_BYTES`] is the ceiling on while a run writes them and
    /// [`MAX_RETAINED_VOLUME_BYTES`] the one on what they keep afterwards.
    #[must_use]
    pub(super) fn volume_guest_data_bytes(&self) -> usize {
        self.volumes
            .values()
            .map(|volume| volume.guest_data.held_bytes())
            .sum()
    }

    /// ... and how many it holds for the MACHINES' own storage, the other kind that ceiling bounds.
    #[must_use]
    pub(super) fn machine_guest_data_bytes(&self) -> usize {
        self.instances
            .values()
            .map(|instance| super::guest_memory::stored_bytes(&instance.guest_files))
            .sum()
    }

    /// How many bytes the guest of the instance `instance_id` names may hold on the MACHINE's own
    /// storage: that machine's own bound ([`super::guest_console::MAX_GUEST_FILE_BYTES`]), and
    /// never more than the store's own ceiling leaves for it ([`MAX_GUEST_DATA_BYTES`]).
    ///
    /// Asked wherever the tree a program of that guest works over is built, so what one machine
    /// can make this store retain is bounded by what every other machine holds. What the VOLUMES
    /// hold is not in it: a filesystem the machine itself reports gibibytes of
    /// ([`super::guest_filesystem`]) has room for a payload whatever was written through some
    /// volume's device, and the volumes' own bytes are bounded where they are held
    /// ([`MAX_RETAINED_VOLUME_BYTES`]).
    #[must_use]
    pub(super) fn guest_storage_allowance(&self, instance_id: &str) -> usize {
        let held = self.instances.get(instance_id).map_or(0, |instance| {
            super::guest_memory::stored_bytes(&instance.guest_files)
        });
        MAX_GUEST_DATA_BYTES
            .saturating_sub(self.machine_guest_data_bytes())
            .saturating_add(held)
            .min(super::guest_console::MAX_GUEST_FILE_BYTES)
    }

    /// Keep what one run of a guest's programs left on the volumes attached to it: the bytes each of
    /// them holds now, and the I/O that put them there.
    ///
    /// The bytes go onto the VOLUME, so a read-back answers them and they follow the volume across a
    /// move to another instance; the I/O is recorded as something that HAPPENED at `at`, which is
    /// what the volume's own `AWS/EBS` series is published from ([`super::volume_telemetry`]). A
    /// volume this store no longer holds keeps nothing: there is no volume for the bytes to be on.
    ///
    /// What the store retains for every volume at once is brought back under its ceiling here
    /// ([`Self::reclaim_volume_guest_data`]), once the bytes this run left are on their volumes —
    /// so the room the NEXT run writes in is room this store really has.
    pub(super) fn apply_guest_disk_writes(&mut self, written: GuestDiskWrites, at: SystemTime) {
        let GuestDiskWrites { data, io } = written;
        // The volumes this run reached, which are the ones whose bytes it is holding for the
        // client that just wrote or read them: never the ones reclaimed below.
        let touched: ::std::collections::BTreeSet<String> = data.keys().cloned().collect();
        for (volume_id, bytes) in data {
            if let Some(record) = self.volumes.get_mut(&volume_id) {
                record.guest_data = bytes;
                record.guest_data_at = Some(at);
            }
        }
        for (volume_id, performed) in io {
            if !performed.happened() {
                continue;
            }
            if let Some(record) = self.volumes.get_mut(&volume_id) {
                record.record_guest_io(performed, at);
            }
        }
        self.reclaim_volume_guest_data(&touched);
    }

    /// Bring the bytes this store retains for the volumes' guest data back under
    /// [`MAX_RETAINED_VOLUME_BYTES`], taking them from the volumes that are LEAST likely to be
    /// read next — and never from `keep`, the volumes the run that just finished reached.
    ///
    /// Which volume gives its bytes back first: one attached to nothing, whose data no guest can
    /// read at all without attaching it again, before one a machine holds; and among those, the
    /// one whose data a guest has not touched for longest
    /// ([`super::VolumeRecord::guest_data_at`]). A volume gives back the whole of its data rather
    /// than part of it, the way a device whose bytes are gone reads as zero throughout.
    ///
    /// This is the one place those bytes are reclaimed, so what bounds the store is a bound on what
    /// it KEEPS rather than a refusal of a write the device itself would have taken.
    fn reclaim_volume_guest_data(&mut self, keep: &::std::collections::BTreeSet<String>) {
        let mut retained: usize = self.volume_guest_data_bytes();
        if retained <= MAX_RETAINED_VOLUME_BYTES {
            return;
        }
        let now = ::std::time::Instant::now();
        // The volumes whose bytes may go back, in the order they go: unattached before attached,
        // least recently touched before the rest, and the id last so the order is total.
        let mut reclaimable: Vec<(bool, Option<SystemTime>, String)> = self
            .volumes
            .iter()
            .filter(|(volume_id, volume)| {
                !keep.contains(volume_id.as_str()) && !volume.guest_data.is_empty()
            })
            .map(|(volume_id, volume)| {
                (
                    !volume.attachments_at(now).is_empty(),
                    volume.guest_data_at,
                    volume_id.clone(),
                )
            })
            .collect();
        reclaimable.sort();
        for (_, _, volume_id) in reclaimable {
            if retained <= MAX_RETAINED_VOLUME_BYTES {
                return;
            }
            if let Some(record) = self.volumes.get_mut(&volume_id) {
                retained = retained.saturating_sub(record.guest_data.held_bytes());
                record.guest_data = VolumeData::default();
                record.guest_data_at = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One attached device of a guest: the `index`th NVMe namespace, carrying `volume_id`.
    fn device(index: usize, volume_id: &str, size_gib: i32) -> GuestBlockDevice {
        GuestBlockDevice {
            node: format!("nvme{index}n1"),
            device_name: "/dev/sdf".to_string(),
            volume_id: volume_id.to_string(),
            size_gib,
            root: false,
            major_minor: (259, i64::try_from(index).unwrap_or_default()),
        }
    }

    /// A tree a program reads and writes, as the session hands one over.
    #[derive(Debug, Default)]
    struct Tree {
        held: BTreeMap<String, String>,
    }

    impl ProgramFiles for Tree {
        fn read(&self, path: &str) -> Option<String> {
            self.held.get(path).cloned()
        }

        fn write(&mut self, path: &str, contents: &str) -> bool {
            self.held.insert(path.to_string(), contents.to_string());
            true
        }

        fn remove(&mut self, path: &str) -> bool {
            self.held.remove(path).is_some()
        }
    }

    /// What performing `words` in `disks` over `tree` answered — `None` for an invocation this
    /// guest performs nothing of.
    fn performed(disks: &GuestDisks, tree: &mut Tree, words: &[&str]) -> Option<ProgramStreams> {
        let words: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        let (name, arguments) = words.split_first().expect("a command has a name");
        let environment = BTreeMap::new();
        let invocation = ProgramInvocation {
            name,
            arguments,
            standard_input: None,
            working_directory: "/",
            environment: &environment,
        };
        disk_programs(disks).run_invocation(&invocation, tree)
    }

    /// [`performed`] for an invocation the guest does perform.
    fn run(disks: &GuestDisks, tree: &mut Tree, words: &[&str]) -> ProgramStreams {
        performed(disks, tree, words)
            .unwrap_or_else(|| panic!("the guest performs {words:?}"))
    }

    /// RULE: the stable link of an attached volume names the device whose serial is that volume's id
    /// without its `-`, over a table of volume ids.
    #[test]
    fn the_published_link_names_the_device_carrying_that_volume() {
        // (case, volume id)
        let rows = [
            ("a short id", "vol-123"),
            ("a full id", "vol-0123456789abcdef0"),
            ("an id of zeroes", "vol-00000000000000001"),
        ];
        for (case, volume_id) in rows {
            let attached = device(1, volume_id, 8);
            let published = published_device_files(::std::slice::from_ref(&attached));
            let serial = volume_id.replace('-', "");
            let link = format!("{DEVICE_BY_ID_DIRECTORY}/{EBS_NVME_LINK_PREFIX}{serial}");
            let held = published
                .iter()
                .find(|(path, _)| *path == link)
                .map(|(_, contents)| contents.clone());
            assert_eq!(
                held.as_deref(),
                Some("../../nvme1n1"),
                "case {case:?}: expected the link to point at the device, got {published:?}"
            );
            assert!(
                published.iter().any(|(path, _)| path == "/dev/nvme1n1"),
                "case {case:?}: expected the device node itself, got {published:?}"
            );
            // Resolving that link answers the device, and the device carries that serial.
            let disks = GuestDisks::holding(&[attached], BTreeMap::new());
            let mut tree = Tree::default();
            let resolved = run(&disks, &mut tree, &["readlink", "-f", &link]);
            assert_eq!(
                resolved.output,
                vec!["/dev/nvme1n1".to_string()],
                "case {case:?}: expected the device behind the link, got {resolved:?}"
            );
            assert_eq!(
                disks.disk_at(&link).map(|disk| disk.serial),
                Some(serial),
                "case {case:?}: expected the volume's serial, got {:?}",
                disks.disks()
            );
        }
    }

    /// RULE: bytes written through the device land on the volume, and reading the same range back
    /// answers them — over a table of block sizes and counts.
    #[test]
    fn a_write_through_the_device_lands_on_the_volume() {
        // (case, block size, blocks)
        let rows = [
            ("one short block", "512", 2_usize),
            ("kibibyte blocks", "1K", 4),
            ("mebibyte blocks", "1M", 2),
        ];
        for (case, block, count) in rows {
            let attached = device(1, "vol-0123456789abcdef0", 8);
            let disks = GuestDisks::holding(&[attached], BTreeMap::new());
            let mut tree = Tree::default();
            let made = run(
                &disks,
                &mut tree,
                &[
                    "dd",
                    "if=/dev/urandom",
                    "of=/tmp/payload",
                    &format!("bs={block}"),
                    &format!("count={count}"),
                ],
            );
            assert_eq!(
                made.status, SUCCESS,
                "case {case:?}: expected the payload to be generated, got {made:?}"
            );
            let payload = tree
                .read("/tmp/payload")
                .unwrap_or_else(|| panic!("case {case:?}: the payload is on the machine"));
            let wanted = block_bytes(block).expect("a size") * count;
            assert_eq!(
                payload.len(),
                wanted,
                "case {case:?}: expected {wanted} bytes, got {}",
                payload.len()
            );
            let written = run(
                &disks,
                &mut tree,
                &[
                    "dd",
                    "if=/tmp/payload",
                    "of=/dev/nvme1n1",
                    &format!("bs={block}"),
                    "oflag=direct",
                    "conv=fsync",
                ],
            );
            assert_eq!(
                written.status, SUCCESS,
                "case {case:?}: expected the write to land, got {written:?}"
            );
            // The bytes are on the VOLUME, under its own id.
            let left = disks.taken();
            let held = left
                .data
                .get("vol-0123456789abcdef0")
                .map(|data| data.read(0, payload.len()));
            assert_eq!(
                held.as_deref(),
                Some(payload.as_bytes()),
                "case {case:?}: expected the payload on the volume, got {:?} bytes",
                held.as_ref().map(Vec::len)
            );
            // ... and reading the range back answers exactly those bytes, as the lines a pipeline
            // carries.
            let read = run(
                &disks,
                &mut tree,
                &[
                    "dd",
                    "if=/dev/nvme1n1",
                    &format!("bs={block}"),
                    &format!("count={count}"),
                ],
            );
            assert_eq!(
                read.output.join("\n") + "\n",
                payload,
                "case {case:?}: expected the read-back to be the payload, got {} bytes",
                read.output.iter().map(String::len).sum::<usize>()
            );
            // The I/O that moved them is accounted for, on that volume alone.
            let io = left
                .io
                .get("vol-0123456789abcdef0")
                .copied()
                .unwrap_or_default();
            assert!(
                io.write_bytes == i64::try_from(wanted).unwrap_or_default() && io.write_ops > 0,
                "case {case:?}: expected the written bytes to be accounted for, got {io:?}"
            );
        }
    }

    /// RULE: a write past the end of the device is refused, and a copy this guest cannot bound is
    /// performed by nothing.
    #[test]
    fn the_end_of_the_device_and_an_unbounded_copy_are_refused() {
        // A volume of one gibibyte, written past its end.
        let attached = device(1, "vol-0123456789abcdef0", 1);
        let disks = GuestDisks::holding(&[attached], BTreeMap::new());
        let mut tree = Tree::default();
        let past = run(
            &disks,
            &mut tree,
            &[
                "dd",
                "if=/dev/urandom",
                "of=/dev/nvme1n1",
                "bs=1M",
                "count=1",
                "seek=16384",
            ],
        );
        assert_eq!(
            past.status, FAILED,
            "case \"past the end\": expected a refusal, got {past:?}"
        );
        assert!(
            past.errors.iter().any(|line| line.contains(NO_SPACE)),
            "case \"past the end\": expected the diagnostic, got {past:?}"
        );
        // A write far INSIDE a large device is a write like any other: the volume holds the bytes
        // that were written and nothing for the rest of it, so the device can be addressed whole.
        let far = device(2, "vol-0123456789abcdef1", 64);
        let roomy = GuestDisks::holding(&[far], BTreeMap::new());
        let written = run(
            &roomy,
            &mut tree,
            &[
                "dd",
                "if=/dev/urandom",
                "of=/dev/nvme2n1",
                "bs=1M",
                "count=1",
                "seek=4096",
            ],
        );
        assert_eq!(
            written.status, SUCCESS,
            "case \"four gibibytes in\": expected the write to land, got {written:?}"
        );
        let read = run(
            &roomy,
            &mut tree,
            &[
                "dd",
                "if=/dev/nvme2n1",
                "bs=1M",
                "count=1",
                "skip=4096",
            ],
        );
        let held = roomy
            .taken()
            .data
            .get("vol-0123456789abcdef1")
            .map(VolumeData::held_bytes);
        assert_eq!(
            held,
            Some(DATA_BLOCK_BYTES),
            "case \"four gibibytes in\": expected only the written block to be held, got {held:?}"
        );
        assert_eq!(
            read.output.join("\n").len() + 1,
            DATA_BLOCK_BYTES,
            "case \"four gibibytes in\": expected the block back from where it was written, got \
             {} bytes",
            read.output.iter().map(String::len).sum::<usize>()
        );
        // A copy this guest cannot bound is refused, and so is one that would have to materialise
        // more than a copy may move: a read of a device far larger than that bound answers a SHORT
        // read, and a write at an offset past it is answered the way the end of a device is.
        let huge = device(3, "vol-0123456789abcdef2", 65_536);
        let vast = GuestDisks::holding(&[huge], BTreeMap::new());
        let whole = run(&vast, &mut tree, &["dd", "if=/dev/nvme3n1", "of=/dev/null"]);
        assert_eq!(
            whole.status, SUCCESS,
            "case \"a whole large device\": expected a short read rather than a refusal, got \
             {whole:?}"
        );
        let moved = whole
            .errors
            .last()
            .and_then(|line| line.split(' ').next())
            .and_then(|figure| figure.parse::<usize>().ok())
            .unwrap_or_default();
        assert_eq!(
            moved, MAX_VOLUME_TRANSFER_BYTES,
            "case \"a whole large device\": expected one copy to move at most \
             {MAX_VOLUME_TRANSFER_BYTES} bytes, got {whole:?}"
        );
        let far = run(
            &vast,
            &mut tree,
            &[
                "dd",
                "if=/dev/urandom",
                "of=/tmp/far",
                "bs=1M",
                "count=1",
                "seek=1048576",
            ],
        );
        assert_eq!(
            far.status, FAILED,
            "case \"an offset past what a copy may move\": expected a refusal, got {far:?}"
        );
        assert!(
            far.errors.iter().any(|line| line.contains(NO_SPACE)),
            "case \"an offset past what a copy may move\": expected the diagnostic, got {far:?}"
        );
        // ... and the run's own budget bounds the SUM of what its copies move, so the commands a
        // script carries cannot multiply what one request makes this process materialise: once it
        // is spent, a copy answers the way one at the end of a device does.
        let full_size_copies = MAX_GUEST_RUN_TRANSFER_BYTES / MAX_VOLUME_TRANSFER_BYTES;
        // One copy of the full size a copy may move, stated in the mebibyte blocks the invocation
        // asks for it in, so the loop below spends the budget at the rate the bound itself sets.
        let full_size = format!("count={}", MAX_VOLUME_TRANSFER_BYTES / (1024 * 1024));
        let mut performed_copies = 0_usize;
        let spent = loop {
            let moved = run(
                &vast,
                &mut tree,
                &["dd", "if=/dev/zero", "of=/dev/null", "bs=1M", &full_size],
            );
            if moved.status != SUCCESS {
                break moved;
            }
            performed_copies = performed_copies.saturating_add(1);
            assert!(
                performed_copies <= full_size_copies,
                "case \"the run's budget\": expected the run to stop after at most \
                 {full_size_copies} full-size copies, and it performed {performed_copies}"
            );
        };
        assert!(
            performed_copies > 0,
            "case \"the run's budget\": expected a copy inside the budget to be performed"
        );
        assert!(
            spent.errors.iter().any(|line| line.contains(NO_SPACE)),
            "case \"the run's budget\": expected the diagnostic once it is spent, got {spent:?}"
        );
        // A read of a path the machine holds nothing at is the utility's own failure.
        let absent = run(
            &disks,
            &mut tree,
            &["dd", "if=/tmp/absent", "of=/dev/null", "bs=512"],
        );
        assert_eq!(
            absent.status, FAILED,
            "case \"no such input\": expected a failure, got {absent:?}"
        );
        assert!(
            absent.errors.iter().any(|line| line.contains(NO_SUCH_FILE)),
            "case \"no such input\": expected the diagnostic, got {absent:?}"
        );
        // A copy from a device that answers without end, bounded by no count, is refused.
        let unbounded = run(&disks, &mut tree, &["dd", "if=/dev/zero", "of=/dev/null"]);
        assert_eq!(
            unbounded.status, FAILED,
            "case \"no count\": expected an unbounded read to be refused, got {unbounded:?}"
        );
    }

    /// RULE: the copy REPORTS what it moved — the records in and out, and a final line whose first
    /// field is the bytes copied, which is the figure a script reads the count out of.
    #[test]
    fn the_report_states_the_records_and_the_bytes_the_copy_moved() {
        let attached = device(1, "vol-0123456789abcdef0", 8);
        let disks = GuestDisks::holding(&[attached], BTreeMap::new());
        let mut tree = Tree::default();
        // (case, block size, blocks, the bytes it comes to, the multiples the line states)
        let rows = [
            ("one short block", "512", 1_usize, 512_usize, ""),
            ("kibibyte blocks", "1K", 4, 4096, " (4.1 kB, 4.0 KiB)"),
            ("mebibyte blocks", "1M", 2, 2 * 1024 * 1024, " (2.1 MB, 2.0 MiB)"),
        ];
        for (case, block, count, bytes, multiples) in rows {
            let written = run(
                &disks,
                &mut tree,
                &[
                    "dd",
                    "if=/dev/urandom",
                    "of=/dev/nvme1n1",
                    &format!("bs={block}"),
                    &format!("count={count}"),
                ],
            );
            assert_eq!(
                written.errors.first().map(String::as_str),
                Some(format!("{count}+0 records in").as_str()),
                "case {case:?}: expected the records read, got {written:?}"
            );
            assert_eq!(
                written.errors.get(1).map(String::as_str),
                Some(format!("{count}+0 records out").as_str()),
                "case {case:?}: expected the records written, got {written:?}"
            );
            let last = written
                .errors
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: the copy reports what it moved"));
            assert!(
                last.starts_with(&format!("{bytes} bytes{multiples} copied, ")),
                "case {case:?}: expected the bytes and their multiples, got {last:?}"
            );
            assert_eq!(
                last.split(' ').next(),
                Some(bytes.to_string().as_str()),
                "case {case:?}: expected the first field to be the bytes copied, got {last:?}"
            );
            // The record counts alone, for an invocation that asked for nothing else.
            let quiet = run(
                &disks,
                &mut tree,
                &[
                    "dd",
                    "if=/dev/urandom",
                    "of=/dev/nvme1n1",
                    &format!("bs={block}"),
                    &format!("count={count}"),
                    "status=noxfer",
                ],
            );
            assert_eq!(
                quiet.errors.len(),
                2,
                "case {case:?}: expected the record counts alone, got {quiet:?}"
            );
        }
    }

    /// RULE: what the STORE has room to hold for a volume bounds a write exactly as the end of the
    /// device does — the bytes that fit land, and the rest is a short write.
    #[test]
    fn a_write_past_what_the_store_has_room_for_is_short() {
        let attached = device(1, "vol-0123456789abcdef0", 8);
        // A store with room for ONE block of a volume's data and nothing more.
        let disks =
            GuestDisks::holding(::std::slice::from_ref(&attached), BTreeMap::new())
                .with_room(DATA_BLOCK_BYTES);
        let mut tree = Tree::default();
        let written = run(
            &disks,
            &mut tree,
            &[
                "dd",
                "if=/dev/urandom",
                "of=/dev/nvme1n1",
                "bs=1M",
                "count=4",
            ],
        );
        assert_eq!(
            written.status, FAILED,
            "case \"no room in the store\": expected a short write, got {written:?}"
        );
        assert!(
            written.errors.iter().any(|line| line.contains(NO_SPACE)),
            "case \"no room in the store\": expected the diagnostic, got {written:?}"
        );
        let held = disks
            .taken()
            .data
            .get("vol-0123456789abcdef0")
            .map(VolumeData::held_bytes);
        assert_eq!(
            held,
            Some(DATA_BLOCK_BYTES),
            "case \"no room in the store\": expected only the block there was room for, got \
             {held:?}"
        );
    }

    /// RULE: an operand form neither program reads is performed by nothing.
    #[test]
    fn an_operand_form_the_guest_does_not_read_is_performed_by_nothing() {
        let disks = GuestDisks::none();
        let mut tree = Tree::default();
        // (case, words)
        let rows: [(&str, &[&str]); 4] = [
            ("an operand dd does not read", &["dd", "--help"]),
            (
                "a flag value it does not know",
                &["dd", "if=/dev/null", "oflag=swab"],
            ),
            ("a size it cannot read", &["dd", "if=/dev/null", "bs=4Z"]),
            ("an option readlink does not read", &["readlink", "-z"]),
        ];
        for (case, words) in rows {
            assert!(
                performed(&disks, &mut tree, words).is_none(),
                "case {case:?}: expected the guest to perform nothing of it"
            );
        }
    }
}
