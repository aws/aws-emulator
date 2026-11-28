// SPDX-License-Identifier: CC0-1.0

//! The NATIVE data plane of S3 Files: what a client inside the VPC reaches when it MOUNTS a
//! file system, and what the bucket behind that file system then holds.
//!
//! RULE under test: a file system is a view of a bucket, so a file written through a mount of
//! it IS an object of that bucket at the file system's prefix followed by the file's path —
//! carrying the file's own bytes, the media type the file's name states, the encryption the
//! file system's key or the bucket's default settles, the whole-object checksum the object
//! store computes for any write, and the file-system attributes of the file as user metadata —
//! and a file removed through the mount is an object removed from the bucket. An `available`
//! mount target of the client's own VPC is what serves that file system, and nothing else does.
//!
//! The bucket, the file system and the mount target are created through the front door — the
//! object store's own API and S3 Files' own restJson1 routes, with the subnets minted by Amazon
//! EC2 on the same engine — the mount itself is performed the way it is performed in a machine,
//! through the client programs this service hands the component that reaches into one
//! (`state::mount_client_programs`) over a tree of that machine's own, and every object is read
//! back through the object store's own `GetObject`. Nothing reaches into a state struct.
//!
//! The rows span what the rule generalises over: the media type a file's name states (a text
//! type, a structured one, a tabular one and a name stating none), the size of the file (empty,
//! a few bytes, tens of bytes), how deep under the mount point it lies (the mount root, one
//! directory, two), the prefix the file system is scoped to (none and one), and — for the
//! second rule — how the mount names what it mounts, which VPC the client stands in and the
//! lifecycle state of the mount target.
//!
//! Nothing here sleeps for a lifecycle span and nothing asserts on elapsed time: the control
//! plane is polled for the CONDITION it reports, and the exports a client is offered are read
//! AT an instant each case names, so both verdicts are the same however fast or loaded the host
//! is.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;
use service_com_amazonaws_s3files_s3files::state;
use service_common::posix_shell_session::{EnvironmentPrograms, ProgramFiles};
use service_common::posix_shell_volume::{MountFacts, MountedTree, ShellVolume, VolumeLimits};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::file_system_lifecycle::{call, Answer, REGION, ROLE_ARN};

/// The status a program exits with when it did what it was asked.
const SUCCESS: i32 = 0;

/// The status `mount` and `umount` exit with when the mount itself failed: "32 mount failure"
/// (`mount(8)`, EXIT STATUS).
const MOUNT_FAILURE: i32 = 32;

/// The status a shell answers for a command nothing resolved, which is what a mount this data
/// plane performs nothing of comes back as.
const COULD_NOT_RUN: i32 = 127;

/// The directory a machine of this suite mounts a file system at.
const MOUNT_POINT: &str = "/mnt/s3files";

/// The options the documented mount of an S3 File System carries, as the mount helper takes
/// them.
const MOUNT_OPTIONS: &str = "tls,iam";

/// A generous deadline for a transition the clock owes to have come due. Nothing waits for it:
/// [`Aws::settled`] looks again until the condition holds or this passes.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long [`Aws::settled`] waits between two looks. Inside the poll loop, so it bounds how
/// often the condition is re-read, never how long the test may take.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The lifecycle state a usable resource of this service carries.
const AVAILABLE: &str = state::STATUS_AVAILABLE;

/// The checksum algorithm the object store applies to a write that names none, and so the one
/// every object a mount write materialises carries: "if it was uploaded without a checksum (and
/// Amazon S3 added the default checksum, `CRC64NVME`, to the uploaded object)"
/// (`PutObjectOutput$ChecksumCRC64NVME`).
const DEFAULT_CHECKSUM_ALGORITHM: &str = "CRC64NVME";

/// The checksum type a single whole-object write records, there being no parts to compose one
/// from: "for `PutObject` uploads, the checksum type is always `FULL_OBJECT`"
/// (`PutObjectRequest$ChecksumType`).
const FULL_OBJECT_CHECKSUM_TYPE: &str = "FULL_OBJECT";

/// The single-layer server-side encryption a bucket with no configuration of its own applies to
/// every object written into it, which is what a read of such an object reports.
const DEFAULT_ENCRYPTION: &str = "AES256";

/// What a read of any object reports it accepts ranges in: "indicates that a range of bytes was
/// specified in the request" is the member's own meaning, and the unit is bytes.
const ACCEPT_RANGES: &str = "bytes";

/// The input the CRC-64/NVME specification publishes a check value for, and that published
/// value: the digest of `123456789` is `0xAE8B14860A799888`, which anchors the checksum one
/// row reports to the algorithm the object store names rather than to whatever this emulator
/// happens to compute.
const CHECKSUM_CHECK_INPUT: &str = "123456789";
const CHECKSUM_CHECK_VALUE: u64 = 0xAE8B_1486_0A79_9888;

/// How long after a write this suite still accepts the second its POSIX timestamps name.
/// Generous on purpose: it pins the UNIT those timestamps are kept in, not how fast the host
/// runs, so nothing here waits for it or bounds a wait by it.
const STAMP_WINDOW: Duration = Duration::from_secs(3600);

/// How many digits the NANOSECOND field of a POSIX time carries: a count of nanoseconds inside
/// one second runs from 0 to 999999999, so the field is nine digits wide, zero-padded.
const NANOSECOND_DIGITS: usize = 9;

/// How much of one machine's own tree this suite gives a case. Far above what a case writes.
const MACHINE_LIMITS: VolumeLimits = VolumeLimits {
    files: 64,
    bytes: 64 * 1024,
    path_bytes: 4096,
    directories: 64,
    device_lines: 0,
    device_line_bytes: 0,
};

/// One file a case writes through the mount, and the object the bucket must then hold for it.
struct FileRow {
    case: &'static str,
    /// The path under the mount point, as the program writing it names it.
    path: &'static str,
    /// The bytes written.
    payload: &'static str,
    /// The media type the file's own name states.
    content_type: &'static str,
}

/// The table: four media types (one of them the generic type a name stating none is served
/// under), three sizes including an empty file, and three depths under the mount point. One
/// row's bytes are the input the checksum algorithm publishes a check value for, so the
/// reported checksum is pinned to the algorithm rather than to this emulator's own
/// computation.
const FILE_ROWS: &[FileRow] = &[
    FileRow {
        case: "a text file at the mount root",
        path: "sim2real-roundtrip.txt",
        payload: "sim2real-s3files-wire-payload",
        content_type: "text/plain",
    },
    FileRow {
        case: "a structured document one directory down",
        path: "exports/report.json",
        payload: "{\"rows\":2,\"complete\":true}",
        content_type: "application/json",
    },
    FileRow {
        case: "a tabular file two directories down",
        path: "exports/2026/rows.csv",
        payload: "left,right\n1,2\nthree,four\nfive,six\n",
        content_type: "text/csv",
    },
    FileRow {
        case: "a file whose name states no media type",
        path: "data/payload",
        payload: "bytes with no stated type",
        content_type: "application/octet-stream",
    },
    FileRow {
        case: "an empty file",
        path: "empty.txt",
        payload: "",
        content_type: "text/plain",
    },
    FileRow {
        case: "the file whose bytes have a published checksum",
        path: "check/value.txt",
        payload: CHECKSUM_CHECK_INPUT,
        content_type: "text/plain",
    },
];

/// S3 Files, the object store it is a view of, and Amazon EC2 — the owner of the subnets a
/// mount target is placed in — on ONE shared engine, the way the router mounts them.
struct Aws {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised elsewhere.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3files_s3files::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// A handle to this emulator for driving it through an AWS SDK client: the same in-process
    /// seam the handlers reach the object store through.
    fn callback(&self) -> EmbeddedEmulator {
        EmbeddedEmulator::from_router(self.router.clone())
    }

    /// An Amazon S3 client over that seam, built the one way this tree builds it.
    fn s3(&self) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), REGION)
    }

    /// An Amazon EC2 client over the same seam.
    fn ec2(&self) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(self.callback().aws_credentials_provider())
            .http_client(self.callback().http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// One subnet of the account in [`REGION`], as EC2 reports it: its id and the VPC it
    /// belongs to. Nothing here derives an identifier of its own.
    async fn account_subnet(&self) -> (String, String) {
        let described = self
            .ec2()
            .describe_subnets()
            .send()
            .await
            .expect("EC2 must report the account's subnets");
        described
            .subnets()
            .iter()
            .filter(|subnet| {
                subnet
                    .availability_zone()
                    .is_some_and(|zone| zone.starts_with(REGION))
            })
            .find_map(|subnet| {
                Some((
                    subnet.subnet_id()?.to_string(),
                    subnet.vpc_id()?.to_string(),
                ))
            })
            .expect("the account must have a subnet in this region")
    }

    /// A VERSIONED bucket of the object store, created through its own API, and the ARN that
    /// names it to a file system.
    async fn bucket(&self, name: &str) -> String {
        self.s3()
            .create_bucket()
            .bucket(name)
            .create_bucket_configuration(
                ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(
                        REGION,
                    ))
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("CreateBucket({name}) must succeed: {error:?}"));
        // Versioned, so every object a write-through materialises is a version the store
        // minted an id for.
        self.s3()
            .put_bucket_versioning()
            .bucket(name)
            .versioning_configuration(
                ::aws_sdk_s3::types::VersioningConfiguration::builder()
                    .status(::aws_sdk_s3::types::BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("PutBucketVersioning({name}) must succeed: {error:?}"));
        format!("arn:aws:s3:::{name}")
    }

    /// A file system over `bucket_arn` scoped to `prefix`, polled to `available`.
    async fn file_system(&self, bucket_arn: &str, prefix: &str) -> String {
        let mut request = json!({ "bucket": bucket_arn, "roleArn": ROLE_ARN });
        if !prefix.is_empty() {
            request["prefix"] = json!(prefix);
        }
        let created = call(&self.router, "PUT", "/file-systems", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "CreateFileSystem({bucket_arn:?}) must be served, got {} / {:?}",
            created.status, created.body
        );
        let file_system_id = created.text("fileSystemId");
        self.settled(
            &format!("/file-systems/{file_system_id}"),
            &format!("file system {file_system_id}"),
        )
        .await;
        file_system_id
    }

    /// A mount target of `file_system_id` in `subnet_id`, polled to `available`: its id and the
    /// IPv4 address the control plane reports for it.
    async fn mount_target(&self, file_system_id: &str, subnet_id: &str) -> (String, String) {
        let created = call(
            &self.router,
            "PUT",
            "/mount-targets",
            Some(json!({ "fileSystemId": file_system_id, "subnetId": subnet_id })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "CreateMountTarget({file_system_id:?}) must be served, got {} / {:?}",
            created.status, created.body
        );
        let mount_target_id = created.text("mountTargetId");
        let address = created.text("ipv4Address");
        self.settled(
            &format!("/mount-targets/{mount_target_id}"),
            &format!("mount target {mount_target_id}"),
        )
        .await;
        (mount_target_id, address)
    }

    /// Look at `path` until the resource it reads reports `available`, or
    /// [`SETTLE_DEADLINE`] passes. It polls the CONDITION rather than waiting a span.
    async fn settled(&self, path: &str, what: &str) {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let read = call(&self.router, "GET", path, None).await;
            assert_eq!(
                read.status, 200,
                "the read of {what} must be served, got {} / {:?}",
                read.status, read.body
            );
            if read.text("status") == AVAILABLE {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what} must reach {AVAILABLE:?} within {SETTLE_DEADLINE:?}, and reported {:?}",
                read.field("status")
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The file-system client programs for a machine standing in `vpc_id`, as the exports stand
    /// at `at`.
    fn client(&self, at: SystemTime, vpc_id: Option<&str>) -> Box<dyn EnvironmentPrograms> {
        state::mount_client_programs(&*self.engine, at, vpc_id)
            .expect("this engine holds the store of this service")
    }

    /// Read the object at `key` of `bucket` through the object store's own `GetObject`, with
    /// integrity asked for so the checksum the object carries is reported.
    ///
    /// A refusal is handed back as the text the store answered with, so a case that expects one
    /// names it without carrying the SDK's own error type around.
    async fn object(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<::aws_sdk_s3::operation::get_object::GetObjectOutput, String> {
        self.s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .checksum_mode(::aws_sdk_s3::types::ChecksumMode::Enabled)
            .send()
            .await
            .map_err(|error| format!("{error:?}"))
    }

    /// The user metadata the object at `key` carries, read through that same `GetObject`: a
    /// key the store does not serve is a failed case rather than an empty map.
    async fn object_metadata(&self, bucket: &str, key: &str) -> BTreeMap<String, String> {
        let read = self
            .object(bucket, key)
            .await
            .unwrap_or_else(|error| panic!("the bucket must serve {key:?}, got {error:?}"));
        metadata_of(&read)
    }
}

/// The tree of ONE machine, as the client programs of a data plane reach it: the same seam the
/// shell session of a guest hands them, over a tree of this machine's own.
#[derive(Debug)]
struct Machine {
    tree: ShellVolume,
}

impl Machine {
    /// A machine holding the directory [`MOUNT_POINT`], which is what a script makes with
    /// `mkdir -p` before it mounts anything there.
    fn new() -> Self {
        let mut tree = ShellVolume::new(MACHINE_LIMITS);
        assert!(
            tree.make_directories(MOUNT_POINT),
            "the machine must hold the directory a file system is mounted over"
        );
        Self { tree }
    }
}

impl ProgramFiles for Machine {
    fn read(&self, path: &str) -> Option<String> {
        self.tree.read(path)
    }

    fn write(&mut self, path: &str, contents: &str) -> bool {
        self.tree.write(path, contents, false)
    }

    fn remove(&mut self, path: &str) -> bool {
        self.tree.remove_file(path)
    }

    fn entries_in(&self, path: &str) -> Vec<String> {
        self.tree.entry_names(path)
    }

    fn make_directories(&mut self, path: &str) -> bool {
        self.tree.make_directories(path)
    }

    fn mount(&mut self, path: &str, tree: MountedTree) -> bool {
        self.tree.mount(path, tree)
    }

    fn unmount(&mut self, path: &str) -> bool {
        self.tree.unmount(path)
    }

    fn mounted_at(&self, path: &str) -> Option<MountFacts> {
        self.tree.mount_facts(path).cloned()
    }

    fn file_system_at(&self, path: &str) -> Option<MountFacts> {
        self.tree.file_system_facts(path).cloned()
    }
}

/// Run one command line — the words as a shell would hand them over — through `client`, and
/// answer the status it exited with, or [`COULD_NOT_RUN`] for an invocation that client
/// performs nothing of.
fn run(client: &dyn EnvironmentPrograms, machine: &mut Machine, line: &str) -> i32 {
    let mut words = line.split_whitespace().map(str::to_string);
    let name = words.next().expect("a command line names a program");
    let arguments: Vec<String> = words.collect();
    client
        .run(&name, &arguments, machine)
        .map_or(COULD_NOT_RUN, |outcome| outcome.status)
}

/// The user metadata one read reports, as a map.
fn metadata_of(read: &::aws_sdk_s3::operation::get_object::GetObjectOutput) -> BTreeMap<String, String> {
    read.metadata()
        .map(|metadata| {
            metadata
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// What `metadata` holds under `name`, or the empty text when it holds nothing there.
fn entry(metadata: &BTreeMap<String, String>, name: &str) -> String {
    metadata.get(name).cloned().unwrap_or_default()
}

/// The POSIX time `stamped` names, as the pair a file system holds for an entry: the second
/// since the epoch it counts and the nanoseconds inside that second — and `None` for text that
/// is no such pair at all. The times an object carries for the entry it was written from are
/// raw POSIX ones, the whole `(tv_sec, tv_nsec)` pair, in the form the owner, the group and the
/// mode beside them carry: the seconds, a point, and the nanoseconds in the nine digits that
/// field counts.
fn stamped_time(stamped: &str) -> Option<(u64, u32)> {
    let (seconds, nanoseconds) = stamped.split_once('.')?;
    if nanoseconds.len() != NANOSECOND_DIGITS {
        return None;
    }
    Some((seconds.parse().ok()?, nanoseconds.parse().ok()?))
}

/// The second of the POSIX time `stamped` names, and `None` for text that names no such time.
fn stamped_second(stamped: &str) -> Option<u64> {
    stamped_time(stamped).map(|(seconds, _)| seconds)
}

/// RULE: a file written through a mount of a file system is an object of the bucket the file
/// system is a view of, at its prefix followed by the file's path, carrying the file's bytes,
/// the media type its name states, the bucket's encryption, the store's whole-object checksum
/// and the file-system attributes of the file as user metadata — and a file removed through the
/// mount is an object removed from the bucket.
#[tokio::test]
async fn a_file_written_through_a_mount_is_an_object_of_the_backing_bucket() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket = "s3files-write-through";
    let bucket_arn = aws.bucket(bucket).await;
    let file_system_id = aws.file_system(&bucket_arn, "").await;
    let (_mount_target_id, _address) = aws.mount_target(&file_system_id, &subnet_id).await;

    let client = aws.client(SystemTime::now(), Some(&vpc_id));
    let mut machine = Machine::new();
    assert_eq!(
        run(
            &*client,
            &mut machine,
            &format!("mount -t s3files -o {MOUNT_OPTIONS} {file_system_id}:/ {MOUNT_POINT}")
        ),
        SUCCESS,
        "the documented mount of file system {file_system_id} must succeed"
    );

    // The second the writes below start at, which is what pins the UNIT of the POSIX
    // timestamps the objects carry: a stamp in any other unit cannot lie in the window from
    // here to a generously later second. Nothing waits for that window to pass.
    let writing_from = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    let stamped_window = writing_from..=writing_from.saturating_add(STAMP_WINDOW.as_secs());

    for row in FILE_ROWS {
        let case = row.case;
        let path = format!("{MOUNT_POINT}/{}", row.path);
        if let Some(directory) = ::service_common::posix_shell_volume::parent_path(&path) {
            assert!(
                machine.make_directories(&directory),
                "case {case:?}: the directories above {path:?} must be creatable through the \
                 mount"
            );
        }
        assert!(
            machine.write(&path, row.payload),
            "case {case:?}: the write of {path:?} through the mount must happen"
        );
    }

    for row in FILE_ROWS {
        let case = row.case;
        let key = row.path;
        let read = aws
            .object(bucket, key)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the bucket must serve {key:?} after the mount write, got \
                     {error:?}"
                )
            });
        // Everything the response REPORTS about the object, taken before its body is read:
        // reading the body consumes the stream the response carries it on.
        let accept_ranges = read.accept_ranges().map(ToString::to_string);
        let content_type = read.content_type().map(ToString::to_string);
        let encryption = read
            .server_side_encryption()
            .map(|value| value.as_str().to_string());
        let checksum = read.checksum_crc64_nvme().map(ToString::to_string);
        let checksum_type = read.checksum_type().map(|value| value.as_str().to_string());
        let version_id = read.version_id().map(ToString::to_string);
        let last_modified = read.last_modified().copied();
        let metadata = metadata_of(&read);
        let body = read
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: {key:?} must arrive in full: {error:?}"))
            .into_bytes()
            .to_vec();
        assert_eq!(
            body.as_slice(),
            row.payload.as_bytes(),
            "case {case:?}: expected the bytes written through the mount at {key:?}, got {:?}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            accept_ranges.as_deref(),
            Some(ACCEPT_RANGES),
            "case {case:?}: expected accept-ranges {ACCEPT_RANGES:?}, got {accept_ranges:?}"
        );
        assert_eq!(
            content_type.as_deref(),
            Some(row.content_type),
            "case {case:?}: expected the media type {:?} the file name states, got \
             {content_type:?}",
            row.content_type
        );
        assert_eq!(
            encryption.as_deref(),
            Some(DEFAULT_ENCRYPTION),
            "case {case:?}: expected the bucket's own encryption {DEFAULT_ENCRYPTION:?}, got \
             {encryption:?}"
        );
        let expected_checksum = ::service_com_amazonaws_s3_amazons3::state::full_object_checksum(
            DEFAULT_CHECKSUM_ALGORITHM,
            row.payload.as_bytes(),
        );
        assert_eq!(
            checksum, expected_checksum,
            "case {case:?}: expected the {DEFAULT_CHECKSUM_ALGORITHM} checksum of the bytes \
             written, got {checksum:?}"
        );
        if row.payload == CHECKSUM_CHECK_INPUT {
            // The one row whose expectation is the algorithm's own published check value,
            // rather than what this emulator computed for the bytes.
            let published = ::aws_smithy_types::base64::encode(CHECKSUM_CHECK_VALUE.to_be_bytes());
            assert_eq!(
                checksum.as_deref(),
                Some(published.as_str()),
                "case {case:?}: expected the published {DEFAULT_CHECKSUM_ALGORITHM} check value \
                 {published:?}, got {checksum:?}"
            );
        }
        assert_eq!(
            checksum_type.as_deref(),
            Some(FULL_OBJECT_CHECKSUM_TYPE),
            "case {case:?}: expected the checksum type {FULL_OBJECT_CHECKSUM_TYPE:?}, got \
             {checksum_type:?}"
        );
        assert!(
            version_id.as_ref().is_some_and(|id| !id.is_empty()),
            "case {case:?}: a write into a versioned bucket must report the version it stored, \
             got {version_id:?}"
        );
        assert!(
            last_modified.is_some(),
            "case {case:?}: the object must report when it was written, got {last_modified:?}"
        );

        assert_eq!(
            (
                entry(&metadata, state::FILE_OWNER_METADATA),
                entry(&metadata, state::FILE_GROUP_METADATA),
                entry(&metadata, state::FILE_PERMISSIONS_METADATA),
                entry(&metadata, state::USER_AGENT_METADATA),
            ),
            (
                "0".to_string(),
                "0".to_string(),
                "0100644".to_string(),
                state::S3_FILES_USER_AGENT.to_string(),
            ),
            "case {case:?}: expected the POSIX identity and the agent of the write, got \
             {metadata:?}"
        );
        for name in [
            state::FILE_BIRTH_TIME_METADATA,
            state::FILE_ACCESS_TIME_METADATA,
            state::FILE_MODIFIED_TIME_METADATA,
        ] {
            let stamped = entry(&metadata, name);
            // The POSIX timestamp of the entry: the second it was written at, in the raw
            // form the owner, group and permissions beside it carry.
            let second = stamped_second(&stamped);
            assert!(
                second.is_some_and(|second| stamped_window.contains(&second)),
                "case {case:?}: expected {name} to carry the file's own POSIX timestamp in \
                 seconds since the epoch, naming a second inside {stamped_window:?}, got \
                 {stamped:?}"
            );
        }
        let identity = entry(&metadata, state::FILE_SYSTEM_METADATA);
        let parts: Vec<&str> = identity.split(':').collect();
        assert!(
            parts.len() == 3
                && parts[0] == file_system_id
                && parts[1].parse::<u64>().is_ok()
                && parts[2].parse::<u64>().is_ok_and(|inode| inode > 1),
            "case {case:?}: expected {} to name the writing file system, its generation and the \
             entry's inode, got {identity:?}",
            state::FILE_SYSTEM_METADATA
        );
    }

    // Two files of one file system are two entries of it, so their identities differ.
    let first = aws
        .object(bucket, FILE_ROWS[0].path)
        .await
        .expect("the first row's object must be served");
    let second = aws
        .object(bucket, FILE_ROWS[1].path)
        .await
        .expect("the second row's object must be served");
    let (first_identity, second_identity) = (
        entry(&metadata_of(&first), state::FILE_SYSTEM_METADATA),
        entry(&metadata_of(&second), state::FILE_SYSTEM_METADATA),
    );
    assert_ne!(
        first_identity, second_identity,
        "case \"two files of one file system\": expected two entries to carry two identities, \
         got {first_identity:?} twice"
    );

    // A file REMOVED through the mount is an object removed from the bucket.
    let removed = FILE_ROWS[0].path;
    assert!(
        machine.remove(&format!("{MOUNT_POINT}/{removed}")),
        "case \"a file removed through the mount\": the removal must happen"
    );
    let read = aws.object(bucket, removed).await;
    assert!(
        read.is_err(),
        "case \"a file removed through the mount\": the bucket must no longer serve {removed:?}"
    );

    // And the mount comes away again, leaving the file system's objects where they are.
    assert_eq!(
        run(&*client, &mut machine, &format!("umount {MOUNT_POINT}")),
        SUCCESS,
        "the mount must come away again"
    );
    assert_eq!(
        machine.mounted_at(MOUNT_POINT),
        None,
        "nothing must be mounted at {MOUNT_POINT} after the unmount"
    );
    assert!(
        aws.object(bucket, FILE_ROWS[1].path).await.is_ok(),
        "the objects a mount write materialised must outlive the mount"
    );
}

/// RULE: the key an object lands at is the file system's PREFIX followed by the path of the
/// file inside the file system — so two file systems over one bucket write into their own
/// prefixes, and neither writes outside it.
#[tokio::test]
async fn the_object_a_write_lands_at_carries_the_file_systems_prefix() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket = "s3files-prefix-scoped";
    let bucket_arn = aws.bucket(bucket).await;

    // (case, the prefix the file system is scoped to, the path written, the key it lands at)
    let rows = [
        ("no prefix at all", "", "notes.txt", "notes.txt"),
        ("one prefix segment", "team/", "notes.txt", "team/notes.txt"),
        (
            "a prefix and a directory of the file system",
            "team/shared/",
            "reports/day.txt",
            "team/shared/reports/day.txt",
        ),
    ];
    let payload = "written through the mount";
    for (case, prefix, path, expected_key) in rows {
        let file_system_id = aws.file_system(&bucket_arn, prefix).await;
        aws.mount_target(&file_system_id, &subnet_id).await;
        let client = aws.client(SystemTime::now(), Some(&vpc_id));
        let mut machine = Machine::new();
        assert_eq!(
            run(
                &*client,
                &mut machine,
                &format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT}")
            ),
            SUCCESS,
            "case {case:?}: the mount of {file_system_id} must succeed"
        );
        let written = format!("{MOUNT_POINT}/{path}");
        if let Some(directory) = ::service_common::posix_shell_volume::parent_path(&written) {
            assert!(
                machine.make_directories(&directory),
                "case {case:?}: the directories above {written:?} must be creatable"
            );
        }
        assert!(
            machine.write(&written, payload),
            "case {case:?}: the write of {written:?} must happen"
        );
        let read = aws.object(bucket, expected_key).await;
        let body = match read {
            Ok(read) => read
                .body
                .collect()
                .await
                .expect("the object must arrive in full")
                .into_bytes()
                .to_vec(),
            Err(error) => panic!(
                "case {case:?}: expected the write to land at {expected_key:?}, and the bucket \
                 answered {error:?}"
            ),
        };
        assert_eq!(
            String::from_utf8_lossy(&body),
            payload,
            "case {case:?}: expected the written bytes at {expected_key:?}, got {:?}",
            String::from_utf8_lossy(&body)
        );
    }
}

/// RULE: a file system is served to a client in the VPC of a mount target the control plane
/// reports `available`, by every name that mount target's endpoint answers to — and to no other
/// client, by no other name, and not before the mount target is `available`.
#[tokio::test]
async fn only_an_available_mount_target_of_the_clients_own_vpc_serves_the_file_system() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket_arn = aws.bucket("s3files-mount-reachability").await;

    // READ FIRST, at the instant just after the create: a mount target the control plane still
    // holds `creating` serves nothing, and a transition the clock makes due later is not undone.
    let creating_file_system = {
        let mut request = json!({ "bucket": bucket_arn, "roleArn": ROLE_ARN });
        request["clientToken"] = json!("creating-row");
        let created: Answer = call(&aws.router, "PUT", "/file-systems", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "CreateFileSystem must be served, got {} / {:?}",
            created.status, created.body
        );
        created.text("fileSystemId")
    };
    let created_at = SystemTime::now();
    {
        let client = aws.client(created_at, Some(&vpc_id));
        let mut machine = Machine::new();
        let status = run(
            &*client,
            &mut machine,
            &format!("mount -t s3files {creating_file_system}:/ {MOUNT_POINT}"),
        );
        assert_eq!(
            status, MOUNT_FAILURE,
            "case \"a file system the control plane still holds creating\": expected the mount \
             to fail, and it exited {status}"
        );
    }

    let file_system_id = aws.file_system(&bucket_arn, "").await;
    let (_mount_target_id, address) = aws.mount_target(&file_system_id, &subnet_id).await;
    // A VPC of the account that is NOT the one the mount target stands in: a client there has
    // no network path to the endpoint.
    let elsewhere = aws
        .ec2()
        .create_vpc()
        .cidr_block("10.91.0.0/16")
        .send()
        .await
        .expect("EC2 must create a VPC")
        .vpc
        .and_then(|vpc| vpc.vpc_id)
        .expect("the created VPC must report its id");
    let serving_at = SystemTime::now();
    let host_name = format!("{file_system_id}.s3files.{REGION}.amazonaws.com");

    // (case, the client's VPC, the command, the status it must exit with)
    let rows: Vec<(&str, Option<&str>, String, i32)> = vec![
        (
            "the documented mount of the file system",
            Some(vpc_id.as_str()),
            format!("mount -t s3files -o {MOUNT_OPTIONS} {file_system_id}:/ {MOUNT_POINT}"),
            SUCCESS,
        ),
        (
            "the mount helper invoked directly",
            Some(vpc_id.as_str()),
            format!("mount.s3files {file_system_id}:/ {MOUNT_POINT}"),
            SUCCESS,
        ),
        (
            "the address the control plane reports for the mount target",
            Some(vpc_id.as_str()),
            format!("mount -t nfs4 {address}:/ {MOUNT_POINT}"),
            SUCCESS,
        ),
        (
            "a host name whose first label is the file system",
            Some(vpc_id.as_str()),
            format!("mount -t s3files {host_name}:/ {MOUNT_POINT}"),
            SUCCESS,
        ),
        (
            "the file system with no export path at all",
            Some(vpc_id.as_str()),
            format!("mount -t s3files {file_system_id}: {MOUNT_POINT}"),
            SUCCESS,
        ),
        (
            "a file system this account does not hold",
            Some(vpc_id.as_str()),
            format!("mount -t s3files fs-{:0>17}:/ {MOUNT_POINT}", "abc"),
            MOUNT_FAILURE,
        ),
        (
            "a client standing in another VPC",
            Some(elsewhere.as_str()),
            format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT}"),
            MOUNT_FAILURE,
        ),
        (
            "a client this emulator can place in no VPC at all",
            None,
            format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT}"),
            MOUNT_FAILURE,
        ),
        (
            "a filesystem type this data plane does not serve",
            Some(vpc_id.as_str()),
            format!("mount -t ext4 /dev/nvme1n1 {MOUNT_POINT}"),
            COULD_NOT_RUN,
        ),
        (
            "a mount of another data plane's own address",
            Some(vpc_id.as_str()),
            format!("mount -t nfs4 10.91.7.4:/ {MOUNT_POINT}"),
            COULD_NOT_RUN,
        ),
    ];

    for (case, client_vpc, line, expected) in rows {
        let client = aws.client(serving_at, client_vpc);
        let mut machine = Machine::new();
        let status = run(&*client, &mut machine, &line);
        assert_eq!(
            status, expected,
            "case {case:?}: expected {line:?} to exit {expected}, got {status}"
        );
        let mounted = machine.mounted_at(MOUNT_POINT);
        assert_eq!(
            mounted.is_some(),
            expected == SUCCESS,
            "case {case:?}: expected the machine to hold a mount at {MOUNT_POINT} = {}, got \
             {mounted:?}",
            expected == SUCCESS
        );
        if let Some(facts) = mounted {
            assert_eq!(
                (
                    facts.file_system_type.as_str(),
                    facts.statfs_type.as_str()
                ),
                (state::MOUNTED_FILE_SYSTEM_TYPE, state::MOUNTED_STATFS_TYPE),
                "case {case:?}: expected the mount to report the types the kernel holds it as, \
                 got {facts:?}"
            );
            assert!(
                line.contains(&facts.source),
                "case {case:?}: expected the mount to report what the command mounted, got \
                 {facts:?}"
            );
        }
    }
}

/// RULE: the POSIX timestamps the object a mount write materialises carries are the ENTRY's
/// own: the entry was created by the first write of its path and last written by this one, so
/// a later write of the same file carries the same creation instant, the same entry identity
/// and a modification instant that never moves backwards — while a file created again at a
/// path whose file was removed is a new entry.
#[tokio::test]
async fn a_mount_write_stamps_the_entrys_own_posix_timestamps() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket = "s3files-posix-timestamps";
    let bucket_arn = aws.bucket(bucket).await;
    let file_system_id = aws.file_system(&bucket_arn, "").await;
    aws.mount_target(&file_system_id, &subnet_id).await;

    let client = aws.client(SystemTime::now(), Some(&vpc_id));
    let mut machine = Machine::new();
    assert_eq!(
        run(
            &*client,
            &mut machine,
            &format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT}")
        ),
        SUCCESS,
        "the mount of file system {file_system_id} must succeed"
    );

    // The second the writes below start at, which is what pins the UNIT the stamps are kept
    // in: a stamp in any other unit cannot lie in the window from here to a generously later
    // second. Nothing waits for that window to pass.
    let writing_from = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    let stamped_window = writing_from..=writing_from.saturating_add(STAMP_WINDOW.as_secs());

    // The rows differ in the bytes the entry holds and in how deep under the mount point it
    // lies, which is what the rule generalises over: the stamps are the ENTRY's, whatever the
    // entry holds and wherever it is.
    let rows = [
        ("an entry at the mount root", "stamped.txt", "first bytes"),
        ("an entry one directory down", "day/stamped.txt", ""),
        (
            "an entry two directories down",
            "day/hour/stamped.bin",
            "a longer run of bytes than the row above it holds",
        ),
    ];

    // What each row's object reported for the entry: its creation second, its modification
    // second and the identity the file system stamped on it.
    let mut written_entries: Vec<(&str, u64, u64, String)> = Vec::new();
    for (case, path, payload) in rows {
        let written = format!("{MOUNT_POINT}/{path}");
        if let Some(directory) = ::service_common::posix_shell_volume::parent_path(&written) {
            assert!(
                machine.make_directories(&directory),
                "case {case:?}: the directories above {written:?} must be creatable"
            );
        }
        assert!(
            machine.write(&written, payload),
            "case {case:?}: the write of {written:?} through the mount must happen"
        );
        let metadata = aws.object_metadata(bucket, path).await;
        let mut seconds = Vec::new();
        for name in [
            state::FILE_BIRTH_TIME_METADATA,
            state::FILE_ACCESS_TIME_METADATA,
            state::FILE_MODIFIED_TIME_METADATA,
        ] {
            let stamped = entry(&metadata, name);
            let second = stamped_second(&stamped);
            assert!(
                second.is_some_and(|second| stamped_window.contains(&second)),
                "case {case:?}: expected {name} to name a POSIX second inside \
                 {stamped_window:?}, got {stamped:?}"
            );
            seconds.push(second.unwrap_or_default());
        }
        assert!(
            seconds[2] >= seconds[0],
            "case {case:?}: expected the entry to be written at or after it was created, got \
             {metadata:?}"
        );
        written_entries.push((
            case,
            seconds[0],
            seconds[2],
            entry(&metadata, state::FILE_SYSTEM_METADATA),
        ));
    }

    // A later write of the SAME file is a write of that entry, not the creation of one: the
    // creation instant and the identity stay as they were, and the modification instant never
    // moves backwards.
    let (first_case, born, modified, identity) = written_entries[0].clone();
    let rewritten = format!("{MOUNT_POINT}/{}", rows[0].1);
    assert!(
        machine.write(&rewritten, "second bytes"),
        "the second write of {rewritten:?} through the mount must happen"
    );
    let metadata = aws.object_metadata(bucket, rows[0].1).await;
    let case = format!("a second write of the entry first written by {first_case:?}");
    assert_eq!(
        (
            stamped_second(&entry(&metadata, state::FILE_BIRTH_TIME_METADATA)),
            entry(&metadata, state::FILE_SYSTEM_METADATA),
        ),
        (Some(born), identity.clone()),
        "case {case:?}: expected the entry created at {born} with the identity {identity:?} to \
         keep both, got {metadata:?}"
    );
    assert!(
        stamped_second(&entry(&metadata, state::FILE_MODIFIED_TIME_METADATA))
            .is_some_and(|second| second >= modified && stamped_window.contains(&second)),
        "case {case:?}: expected a modification second at or after {modified}, got {metadata:?}"
    );

    // A file created again where one was REMOVED is a new entry of the file system, so it
    // carries an identity of its own rather than the removed file's.
    assert!(
        machine.remove(&rewritten),
        "the removal of {rewritten:?} through the mount must happen"
    );
    assert!(
        machine.write(&rewritten, "bytes of the file created again"),
        "the write that creates {rewritten:?} again must happen"
    );
    let metadata = aws.object_metadata(bucket, rows[0].1).await;
    let created_again = entry(&metadata, state::FILE_SYSTEM_METADATA);
    assert_ne!(
        created_again, identity,
        "case \"a file created again where one was removed\": expected an identity of its own, \
         got the removed entry's {identity:?}"
    );
}

/// RULE: what is reached through a mount is the file system's ONE tree, so what one client
/// writes through its own mount every other client of the same file system reads through
/// theirs — and the object the write materialised is the same object for both.
#[tokio::test]
async fn two_clients_of_one_file_system_read_each_others_files() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket = "s3files-shared-tree";
    let bucket_arn = aws.bucket(bucket).await;
    let file_system_id = aws.file_system(&bucket_arn, "").await;
    aws.mount_target(&file_system_id, &subnet_id).await;

    let client = aws.client(SystemTime::now(), Some(&vpc_id));
    let mut node_a = Machine::new();
    let mut node_b = Machine::new();
    for (case, machine) in [("node A", &mut node_a), ("node B", &mut node_b)] {
        assert_eq!(
            run(
                &*client,
                machine,
                &format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT}")
            ),
            SUCCESS,
            "case {case:?}: the mount of the shared file system must succeed"
        );
    }

    let payload = "written by node A";
    let path = format!("{MOUNT_POINT}/from-a.txt");
    assert!(
        node_a.write(&path, payload),
        "node A must be able to write through its mount"
    );
    assert_eq!(
        node_b.read(&path).as_deref(),
        Some(payload),
        "node B must read what node A wrote through the one shared file system, and read {:?} \
         instead",
        node_b.read(&path)
    );
    let read = aws
        .object(bucket, "from-a.txt")
        .await
        .expect("the write of either client must be an object of the bucket");
    let body = read
        .body
        .collect()
        .await
        .expect("the object must arrive in full")
        .into_bytes()
        .to_vec();
    assert_eq!(
        String::from_utf8_lossy(&body),
        payload,
        "expected the bytes node A wrote, got {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// RULE: each of the three times an object carries for the file it was written from names the
/// WHOLE time the file system holds for that entry — the second since the epoch and the
/// nanoseconds inside that second, in the raw POSIX form the owner, the group and the mode
/// beside it carry — so a reader of the set gets the entry's time at the precision a file's
/// times are kept at rather than a stamp with its sub-second part dropped.
#[tokio::test]
async fn an_objects_file_times_carry_the_seconds_and_the_nanoseconds_of_the_entry() {
    let aws = Aws::new();
    let (subnet_id, vpc_id) = aws.account_subnet().await;
    let bucket = "s3files-file-time-form";
    let bucket_arn = aws.bucket(bucket).await;
    // Scoped to a prefix, so the key the stamps are read back off is the prefixed one.
    let prefix = "team/";
    let file_system_id = aws.file_system(&bucket_arn, prefix).await;
    aws.mount_target(&file_system_id, &subnet_id).await;

    let client = aws.client(SystemTime::now(), Some(&vpc_id));
    let mut machine = Machine::new();
    assert_eq!(
        run(
            &*client,
            &mut machine,
            &format!("mount -t s3files {file_system_id}:/ {MOUNT_POINT} -o {MOUNT_OPTIONS}")
        ),
        SUCCESS,
        "the mount of file system {file_system_id} must succeed"
    );

    // The second the writes below start at, which is what pins the instant each stamp names to
    // the write it came from: a stamp of anything else could not lie in the window from here to
    // a generously later second. Nothing waits for that window to pass.
    let writing_from = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    let stamped_window = writing_from..=writing_from.saturating_add(STAMP_WINDOW.as_secs());

    // The rows differ in what the file holds, how deep under the mount point it lies and what
    // its name states — what the rule generalises over — because the times stamped are the
    // ENTRY's, whatever the entry holds and wherever it is.
    let rows = [
        ("a text file at the mount root", "stamped.txt", "first bytes"),
        ("an empty file one directory down", "day/empty.txt", ""),
        (
            "a file whose name states no media type, two directories down",
            "day/hour/payload",
            "a longer run of bytes than the row above it holds",
        ),
        (
            "a structured document one directory down",
            "day/report.json",
            "{\"rows\":2}",
        ),
    ];

    // The nanosecond field each row's three stamps carried, so the set of them says whether the
    // sub-second part of an entry's time reaches the object at all.
    let mut nanosecond_fields: Vec<(&str, u32)> = Vec::new();
    for (case, path, payload) in rows {
        let written = format!("{MOUNT_POINT}/{path}");
        if let Some(directory) = ::service_common::posix_shell_volume::parent_path(&written) {
            assert!(
                machine.make_directories(&directory),
                "case {case:?}: the directories above {written:?} must be creatable"
            );
        }
        assert!(
            machine.write(&written, payload),
            "case {case:?}: the write of {written:?} through the mount must happen"
        );
        let key = format!("{prefix}{path}");
        let metadata = aws.object_metadata(bucket, &key).await;
        for name in [
            state::FILE_BIRTH_TIME_METADATA,
            state::FILE_ACCESS_TIME_METADATA,
            state::FILE_MODIFIED_TIME_METADATA,
        ] {
            let stamped = entry(&metadata, name);
            // The whole POSIX pair: text carrying no nanosecond field at all is not one, and is
            // a failed case rather than a stamp read as seconds.
            let time = stamped_time(&stamped);
            assert!(
                time.is_some_and(|(seconds, nanoseconds)| stamped_window.contains(&seconds)
                    && nanoseconds < 1_000_000_000),
                "case {case:?}: expected {name} to carry a POSIX second inside \
                 {stamped_window:?} and a {NANOSECOND_DIGITS}-digit nanosecond field inside \
                 that second, got {stamped:?}"
            );
            nanosecond_fields.push((case, time.unwrap_or_default().1));
        }
        // One write leaves the entry read and written at the one instant the tree recorded for
        // it, so those two stamps name that same instant to the nanosecond, and the entry was
        // created no later than it was written.
        let (birth, access, modified) = (
            entry(&metadata, state::FILE_BIRTH_TIME_METADATA),
            entry(&metadata, state::FILE_ACCESS_TIME_METADATA),
            entry(&metadata, state::FILE_MODIFIED_TIME_METADATA),
        );
        assert_eq!(
            access, modified,
            "case {case:?}: expected the entry read and written at one instant, got \
             {access:?} and {modified:?}"
        );
        assert!(
            stamped_time(&birth) <= stamped_time(&modified),
            "case {case:?}: expected the entry to be written no earlier than it was created, \
             got {birth:?} and {modified:?}"
        );
    }

    // The nanosecond field is the entry's own sub-second part, not a fixed filler: across the
    // rows above — each written at its own instant of a clock that counts far finer than a
    // second — at least one of them is past the start of its second.
    assert!(
        nanosecond_fields
            .iter()
            .any(|(_, nanoseconds)| *nanoseconds > 0),
        "case \"the nanosecond field of every stamp of every row\": expected at least one to \
         carry the sub-second part of its own write, got {nanosecond_fields:?}"
    );
}

