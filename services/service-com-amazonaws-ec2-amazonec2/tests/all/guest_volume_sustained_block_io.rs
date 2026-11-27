// SPDX-License-Identifier: CC0-1.0

//! A guest's BLOCK I/O through an attached volume's device, after other volumes of the account
//! have already been written through — driven over the wire the service speaks (`ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness) and through the one seam a program of
//! a guest runs through (`run_in_instance_guest`, which is what the agent inside a guest delivers
//! a script to).
//!
//! THE RULE: what a guest writes — through the device of a volume the control plane reports
//! attached, or into a file of its own root filesystem — is bounded by what that DEVICE or that
//! FILESYSTEM can take (the volume's own capacity, the filesystem's own size) and by what one copy
//! may move, never by the bytes this emulator happens to be retaining for volumes written earlier
//! or for payloads an earlier run left. So every copy transfers the bytes its invocation asked
//! for, exits 0 and reports that count, and a read of the same range answers exactly those bytes —
//! with whatever utility the guest reads the device with, and on the hundredth volume of a
//! long-lived emulator exactly as on the first. Every one of those transfers is I/O on the VOLUME
//! it went through, which is what that volume's own `AWS/EBS` series measure — the root volume
//! for a write into a file of the root filesystem, since that filesystem IS that volume.
//!
//! The rows differ along what the rule generalises over: the payload's size, the block size the
//! copy moves it in, how much was written through OTHER volumes before it, how many copies the
//! SAME run performed before it, where the payload comes from (the machine's random source, its
//! zero device — a payload whose last byte is not a line terminator, which is what a device read
//! at the whole of one copy's worth carries through a pipeline — and a file on the machine's own
//! storage), whether the copy's destination is the device or that storage, and WHICH UTILITY the
//! guest reads the device back with.
//!
//! Every expected figure is spelled out here rather than imported, so a wrong constant in the
//! implementation cannot make a case pass.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::element;
use super::instance_metric_publication::{
    cloudwatch_ok, ec2_ok, emulator, p, sample_minutes, span_base, MINUTE, SPAN_MINUTES,
};

/// The instance type the cases launch: one built on the Nitro System, whose volumes the guest's
/// kernel presents as NVMe devices.
const NITRO_INSTANCE_TYPE: &str = "m5.large";

/// The image every launch names.
const AMI: &str = "ami-0123456789abcdef0";

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The directory the kernel publishes a block device's stable link under, and what the link of an
/// EBS volume on a Nitro instance is named.
const BY_ID_LINK: &str = "/dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_";

/// The digest of NO bytes at all, which is what a read that answered nothing comes to.
const EMPTY_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// How long the polls below keep asking before giving up: far above the span any move they wait
/// for is owed in, so a loaded host is not what decides a case.
const POLL_DEADLINE: Duration = Duration::from_secs(90);

/// How often they look again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The size, in GiB, of every volume these cases create: a device far larger than any payload
/// below, so nothing a case writes is near the end of it.
const VOLUME_SIZE_GIB: &str = "1";

/// How many bytes one mebibyte is, for the figures the cases expect.
const MIB: usize = 1024 * 1024;

/// How many bytes ONE copy through a device may move, which is a bound this emulator publishes on
/// a copy and the largest payload a case writes.
const WHOLE_COPY_BYTES: usize = 64 * MIB;

/// How many volumes are written through BEFORE the rows below, each taking a whole copy's worth.
///
/// The point of the case: by the last of them the bytes written through volumes of this account
/// come to several times anything this emulator retains for them at once, so a row that writes
/// afterwards is a row written with every one of those bytes already behind it.
const VOLUMES_WRITTEN_FIRST: usize = 5;

/// How many whole copies ONE script performs in the case about a long run: more than the three a
/// payload sequence (generate, write through the device, read back) comes to, so a copy of it is
/// one with several whole copies of the same run already behind it.
const COPIES_IN_ONE_RUN: usize = 6;

/// The window a case reads a volume's datapoints over, either side of the instant the I/O
/// happened, and the period it reads them at — the one Amazon EBS publishes a volume's series at.
const METRIC_WINDOW_SECS: i64 = 900;
const METRIC_PERIOD_SECS: i64 = 60;

/// The devices the volumes of a case are attached at, one per volume, so a volume never waits for
/// the one before it to finish detaching: "/dev/sd[f-p]" is the range AWS publishes for a data
/// volume of a Linux instance (EC2 User Guide, "Device names on Linux instances").
const DEVICES: &[&str] = &[
    "/dev/sdf", "/dev/sdg", "/dev/sdh", "/dev/sdi", "/dev/sdj", "/dev/sdk", "/dev/sdl",
    "/dev/sdm", "/dev/sdn", "/dev/sdo", "/dev/sdp",
];

/// The marker each value a guest's program reports is written under.
const DEVICE_MARKER: &str = "DEVICE";
const SERIAL_MARKER: &str = "SERIAL";
const PAYLOAD_MARKER: &str = "PAYLOAD";
const WROTE_MARKER: &str = "WROTE";
const BYTES_MARKER: &str = "BYTES";
const READBACK_MARKER: &str = "READBACK";
const EXPECTED_MARKER: &str = "EXPECTED";

/// One row: a payload to write through a device and read back.
struct Row {
    /// The case, named in every assertion.
    case: &'static str,
    /// The block size the copy moves the payload in, as `dd` reads one.
    block: &'static str,
    /// How many of those blocks the payload is.
    blocks: usize,
    /// How many bytes that comes to, which is what the copy has to report.
    bytes: usize,
}

/// The rows: payloads of different sizes, moved in different blocks.
const ROWS: &[Row] = &[
    Row {
        case: "a mebibyte in kibibyte blocks",
        block: "64K",
        blocks: 16,
        bytes: MIB,
    },
    Row {
        case: "three mebibytes in mebibyte blocks",
        block: "1M",
        blocks: 3,
        bytes: 3 * MIB,
    },
    Row {
        case: "a payload of one short block",
        block: "512",
        blocks: 7,
        bytes: 7 * 512,
    },
];

/// The serial the device carrying `volume_id` presents: the volume id without its `-`.
fn serial_of(volume_id: &str) -> String {
    volume_id.replace('-', "")
}

/// The value the guest wrote under `marker`, or `None` when it wrote the marker with nothing after
/// it.
fn marker(written: &[String], marker: &str) -> Option<String> {
    written
        .iter()
        .filter_map(|line| line.strip_prefix(&format!("{marker} ")))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Launch one Nitro instance and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let body = ec2_ok(
        router,
        case,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("InstanceType", NITRO_INSTANCE_TYPE),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report an instance: {body}"))
}

/// The Availability Zone the control plane placed `instance_id` in.
async fn zone_of(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let body = ec2_ok(
        router,
        case,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    element(&body, "availabilityZone")
        .unwrap_or_else(|| panic!("case {case:?}: the instance must report its zone: {body}"))
}

/// Look again until the guest of `instance_id` can run a program at all.
async fn until_the_guest_is_up(engine: &engine::InMemoryEngine, case: &str, instance_id: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let up = {
            let state = ec2::state::service_state(engine).expect("the EC2 store");
            let mut store = state.lock();
            store.materialize_due_transitions();
            store.instance_guest_is_up(instance_id)
        };
        if up {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the guest of {instance_id} never came up"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a volume in `zone` and attach it to `instance_id` at `device`, answering its id —
/// waiting for the CONDITION a device the guest can see rests on, which is the attachment the
/// control plane reports as `attached`.
async fn attached_volume(
    router: &MultiServiceRouter,
    case: &str,
    zone: &str,
    instance_id: &str,
    device: &str,
) -> String {
    let body = ec2_ok(
        router,
        case,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone),
            p("Size", VOLUME_SIZE_GIB),
            p("VolumeType", "gp3"),
        ],
    )
    .await;
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume must report a volume: {body}"));
    // A volume reaches `available` a transition span after its create, so the attach is asked for
    // again until it is taken rather than after a wait of this suite's own choosing.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) = super::ec2_query::call(
            router,
            REGION,
            "AttachVolume",
            &[
                p("VolumeId", &volume_id),
                p("InstanceId", instance_id),
                p("Device", device),
            ],
        )
        .await;
        if status == 200 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: AttachVolume never succeeded for {volume_id}, last answer {status}: \
             {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    loop {
        let body = ec2_ok(
            router,
            case,
            "DescribeVolumes",
            &[p("VolumeId.1", &volume_id)],
        )
        .await;
        if super::ec2_query::elements(&body, "status")
            .iter()
            .any(|state| state == "attached")
        {
            return volume_id;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the attachment of {volume_id} never reached `attached`: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Take the volume `volume_id` back off `instance_id`, the way a client that has finished with it
/// does.
async fn detach(router: &MultiServiceRouter, case: &str, volume_id: &str, instance_id: &str) {
    let _ = ec2_ok(
        router,
        case,
        "DetachVolume",
        &[p("VolumeId", volume_id), p("InstanceId", instance_id)],
    )
    .await;
}

/// Run `script` in that instance's guest and answer the lines it wrote.
fn in_guest(
    engine: &engine::InMemoryEngine,
    case: &str,
    instance_id: &str,
    script: &str,
) -> Vec<String> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let mut store = state.lock();
    let run = store
        .run_in_instance_guest(instance_id, &[script.to_string()])
        .unwrap_or_else(|| panic!("case {case:?}: the guest of {instance_id} must run a program"));
    assert_eq!(
        run.status, 0,
        "case {case:?}: expected the script to succeed, got status {} with errors {:?} and \
         output {:?}",
        run.status, run.errors, run.output
    );
    run.output
}

/// The program that writes a whole copy's worth of the machine's zero device through `link` and
/// reports what the copy moved.
fn fill_program(link: &str, blocks: usize) -> String {
    format!(
        "set -u\n\
         DEV=$(readlink -f {link})\n\
         REPORT=$(dd if=/dev/zero of=$DEV bs=1M count={blocks} oflag=direct conv=fsync 2>&1)\n\
         echo \"{WROTE_MARKER} $?\"\n\
         echo \"{BYTES_MARKER} $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"\n"
    )
}

/// The program a client runs to use an attached volume: find the device behind the volume's own
/// link, generate a payload, write it through the device and read that range back.
fn block_io_program(link: &str, row: &Row) -> String {
    let (block, blocks) = (row.block, row.blocks);
    format!(
        "set -u\n\
         DEV=$(readlink -f {link})\n\
         echo \"{DEVICE_MARKER} $DEV\"\n\
         echo \"{SERIAL_MARKER} $(lsblk -dno SERIAL $DEV)\"\n\
         dd if=/dev/urandom of=/tmp/payload bs={block} count={blocks} status=none\n\
         echo \"{PAYLOAD_MARKER} $(sha256sum /tmp/payload | cut -d' ' -f1)\"\n\
         REPORT=$(dd if=/tmp/payload of=$DEV bs={block} oflag=direct conv=fsync 2>&1)\n\
         echo \"{WROTE_MARKER} $?\"\n\
         echo \"{BYTES_MARKER} $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"\n\
         echo \"{READBACK_MARKER} $(dd if=$DEV bs={block} count={blocks} status=none | \
         sha256sum | cut -d' ' -f1)\"\n"
    )
}

/// THE RULE: a copy through an attached volume's device moves what it was asked for and reads back
/// identically, whatever was written through the account's other volumes before it.
#[tokio::test]
async fn a_copy_through_a_device_is_not_refused_by_what_other_volumes_were_written() {
    let (router, engine) = emulator();
    let case = "a Nitro instance with volumes attached in turn";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;

    // First: several volumes, each written through with a whole copy's worth and then taken off
    // the instance — far more bytes than this emulator retains for the account's volumes at once.
    // Every one of those copies has to move the bytes it was asked for as well: the volume being
    // the sixth one written is no reason for a device with room to take less.
    for (round, device) in DEVICES.iter().enumerate().take(VOLUMES_WRITTEN_FIRST) {
        let named = format!("volume {round} of the ones written first");
        let case = named.as_str();
        let volume_id = attached_volume(&router, case, &zone, &instance_id, device).await;
        let link = format!("{BY_ID_LINK}{}", serial_of(&volume_id));
        let filled = in_guest(
            &engine,
            case,
            &instance_id,
            &fill_program(&link, WHOLE_COPY_BYTES / MIB),
        );
        assert_eq!(
            marker(&filled, WROTE_MARKER).as_deref(),
            Some("0"),
            "case {case:?}: expected the copy to succeed, got {filled:?}"
        );
        assert_eq!(
            marker(&filled, BYTES_MARKER).as_deref(),
            Some(WHOLE_COPY_BYTES.to_string().as_str()),
            "case {case:?}: expected the copy to report the {WHOLE_COPY_BYTES} bytes it moved, \
             got {filled:?}"
        );
        detach(&router, case, &volume_id, &instance_id).await;
    }

    // ... and then the rows, each on a volume of its own, written with all of those bytes behind
    // them.
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let device = DEVICES[VOLUMES_WRITTEN_FIRST + index];
        let volume_id = attached_volume(&router, case, &zone, &instance_id, device).await;
        let serial = serial_of(&volume_id);
        let link = format!("{BY_ID_LINK}{serial}");
        let written = in_guest(&engine, case, &instance_id, &block_io_program(&link, row));

        let device = marker(&written, DEVICE_MARKER).unwrap_or_default();
        assert!(
            device.starts_with("/dev/nvme"),
            "case {case:?}: expected the link to resolve to the NVMe device carrying \
             {volume_id}, got {device:?} of {written:?}"
        );
        let reported = marker(&written, SERIAL_MARKER).unwrap_or_default();
        assert_eq!(
            reported, serial,
            "case {case:?}: expected the device behind the link to report serial {serial:?}, got \
             {reported:?} of {written:?}"
        );
        assert_eq!(
            marker(&written, WROTE_MARKER).as_deref(),
            Some("0"),
            "case {case:?}: expected the write to succeed, got {written:?}"
        );
        let moved = row.bytes;
        assert_eq!(
            marker(&written, BYTES_MARKER).as_deref(),
            Some(moved.to_string().as_str()),
            "case {case:?}: expected the copy to report the {moved} bytes it moved, got \
             {written:?}"
        );
        let payload = marker(&written, PAYLOAD_MARKER).unwrap_or_default();
        let readback = marker(&written, READBACK_MARKER).unwrap_or_default();
        assert!(
            payload != EMPTY_DIGEST && !payload.is_empty(),
            "case {case:?}: expected a payload of real bytes, got {payload:?} of {written:?}"
        );
        assert_eq!(
            readback, payload,
            "case {case:?}: expected the read-back of the written range to be the payload, got \
             {readback:?} against {payload:?}"
        );
        detach(&router, case, &volume_id, &instance_id).await;
    }
}

/// THE RULE at the published bound: a copy of the whole of what one copy may move stores those
/// bytes, and a read of the same range answers them — so a digest taken over the read-back is the
/// digest of those bytes and never of no bytes at all.
#[tokio::test]
async fn a_copy_of_a_whole_transfers_worth_reads_back_the_bytes_it_wrote() {
    let (router, engine) = emulator();
    let case = "a whole copy's worth of the zero device";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;
    let volume_id = attached_volume(&router, case, &zone, &instance_id, DEVICES[0]).await;
    let link = format!("{BY_ID_LINK}{}", serial_of(&volume_id));
    let blocks = WHOLE_COPY_BYTES / MIB;
    // The payload is the machine's own zero device, so its last byte is NOT a line terminator:
    // what the read-back hands the digest is the whole of a copy's worth and the newline the one
    // line it comes to is closed by.
    let script = format!(
        "set -u\n\
         DEV=$(readlink -f {link})\n\
         REPORT=$(dd if=/dev/zero of=$DEV bs=1M count={blocks} oflag=direct conv=fsync 2>&1)\n\
         echo \"{WROTE_MARKER} $?\"\n\
         echo \"{BYTES_MARKER} $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"\n\
         echo \"{READBACK_MARKER} $(dd if=$DEV bs=1M count={blocks} status=none | sha256sum | \
         cut -d' ' -f1)\"\n\
         echo \"{EXPECTED_MARKER} $(dd if=/dev/zero bs=1M count={blocks} status=none | \
         sha256sum | cut -d' ' -f1)\"\n"
    );
    let written = in_guest(&engine, case, &instance_id, &script);
    assert_eq!(
        marker(&written, WROTE_MARKER).as_deref(),
        Some("0"),
        "case {case:?}: expected the write to succeed, got {written:?}"
    );
    assert_eq!(
        marker(&written, BYTES_MARKER).as_deref(),
        Some(WHOLE_COPY_BYTES.to_string().as_str()),
        "case {case:?}: expected the copy to report the {WHOLE_COPY_BYTES} bytes it moved, got \
         {written:?}"
    );
    let readback = marker(&written, READBACK_MARKER).unwrap_or_default();
    let expected = marker(&written, EXPECTED_MARKER).unwrap_or_default();
    assert!(
        readback != EMPTY_DIGEST && !readback.is_empty(),
        "case {case:?}: expected the read-back of {WHOLE_COPY_BYTES} bytes to digest those \
         bytes, got {readback:?} of {written:?}"
    );
    assert_eq!(
        readback, expected,
        "case {case:?}: expected the read-back to digest the bytes that were written, got \
         {readback:?} against {expected:?}"
    );
}

/// THE RULE within ONE run: a copy is not refused for what the copies BEFORE it in the same script
/// moved — a script that generates a payload, writes it through a device and reads it back does
/// that several times over, and every one of those copies moves the bytes it asked for.
#[tokio::test]
async fn every_copy_of_a_long_run_moves_the_bytes_it_asked_for() {
    let (router, engine) = emulator();
    let case = "a script of many whole copies";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;
    let volume_id = attached_volume(&router, case, &zone, &instance_id, DEVICES[0]).await;
    let link = format!("{BY_ID_LINK}{}", serial_of(&volume_id));
    let blocks = WHOLE_COPY_BYTES / MIB;
    // Copies of a whole one's worth, more of them than the few a payload sequence performs, each
    // reporting what it moved under a marker of its own.
    let mut script = format!("set -u\nDEV=$(readlink -f {link})\n");
    for copy in 0..COPIES_IN_ONE_RUN {
        script.push_str(&format!(
            "REPORT=$(dd if=/dev/zero of=$DEV bs=1M count={blocks} oflag=direct conv=fsync 2>&1)\n\
             echo \"{WROTE_MARKER}{copy} $?\"\n\
             echo \"{BYTES_MARKER}{copy} $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"\n"
        ));
    }
    let written = in_guest(&engine, case, &instance_id, &script);
    for copy in 0..COPIES_IN_ONE_RUN {
        assert_eq!(
            marker(&written, &format!("{WROTE_MARKER}{copy}")).as_deref(),
            Some("0"),
            "case {case:?}: expected copy {copy} of the run to succeed, got {written:?}"
        );
        assert_eq!(
            marker(&written, &format!("{BYTES_MARKER}{copy}")).as_deref(),
            Some(WHOLE_COPY_BYTES.to_string().as_str()),
            "case {case:?}: expected copy {copy} to report the {WHOLE_COPY_BYTES} bytes it \
             moved, got {written:?}"
        );
    }
}

/// THE RULE for the READING program: the bytes on the volume are what the guest reads through the
/// device, with WHATEVER utility reads it — a block device is not an empty file to every program
/// but one.
///
/// The rows are the utilities a client reads a device back with: the copy program, a digest taken
/// over the device itself, the one that writes its first bytes, the one that writes the whole of
/// it, and the one that counts its bytes.
#[tokio::test]
async fn every_utility_that_reads_the_device_reads_what_the_volume_holds() {
    let (router, engine) = emulator();
    let case = "the utilities a client reads a device with";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;
    let volume_id = attached_volume(&router, case, &zone, &instance_id, DEVICES[0]).await;
    let link = format!("{BY_ID_LINK}{}", serial_of(&volume_id));
    // A payload of a whole copy's worth, written through the device, then read back five ways:
    // the copy program itself, the device digested whole, its first bytes, the whole of it
    // through a pipe, and its byte count. The device node and the volume's own stable link are
    // both used, since both name the same device.
    let bytes = WHOLE_COPY_BYTES;
    let blocks = WHOLE_COPY_BYTES / MIB;
    let script = format!(
        "set -u\n\
         dev={link}\n\
         DEV=$(readlink -f $dev)\n\
         dd if=/dev/urandom of=/tmp/payload bs=1M count={blocks} 2>/dev/null\n\
         dd if=/tmp/payload of=$dev bs=1M oflag=direct conv=fsync 2>/dev/null\n\
         echo \"{PAYLOAD_MARKER} $(sha256sum /tmp/payload | cut -d' ' -f1)\"\n\
         echo \"COPY $(dd if=$dev bs=1M count={blocks} iflag=direct 2>/dev/null | sha256sum | \
         cut -d' ' -f1)\"\n\
         echo \"FIRST $(head -c {bytes} $DEV | sha256sum | cut -d' ' -f1)\"\n\
         echo \"WHOLE $(cat $DEV | sha256sum | cut -d' ' -f1)\"\n\
         echo \"DIGEST $(sha256sum $DEV | cut -d' ' -f1)\"\n\
         echo \"LINKED $(sha256sum $dev | cut -d' ' -f1)\"\n\
         echo \"COUNTED $(wc -c < $DEV)\"\n"
    );
    let written = in_guest(&engine, case, &instance_id, &script);
    let payload = marker(&written, PAYLOAD_MARKER).unwrap_or_default();
    assert!(
        payload != EMPTY_DIGEST && !payload.is_empty(),
        "case {case:?}: expected a payload of real bytes, got {payload:?} of {written:?}"
    );
    for utility in ["COPY", "FIRST", "WHOLE", "DIGEST", "LINKED"] {
        let read = marker(&written, utility).unwrap_or_default();
        assert_eq!(
            read, payload,
            "case {case:?}: expected {utility} to read the payload off the device, got {read:?} \
             against {payload:?} of {written:?}"
        );
    }
    assert_eq!(
        marker(&written, "COUNTED").as_deref(),
        Some(bytes.to_string().as_str()),
        "case {case:?}: expected the device to count {bytes} bytes, got {written:?}"
    );
}

/// THE RULE for a payload the guest generates on its OWN storage: a filesystem the machine reports
/// gibibytes of takes a file of a whole copy's worth beside the machine's own files, and the copy
/// of that file through the device stores it — so the read-back digests the payload the client
/// generated.
#[tokio::test]
async fn a_payload_file_of_a_whole_copys_worth_is_written_through_the_device() {
    let (router, engine) = emulator();
    let case = "a payload generated on the machine's own storage";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;
    let volume_id = attached_volume(&router, case, &zone, &instance_id, DEVICES[0]).await;
    let link = format!("{BY_ID_LINK}{}", serial_of(&volume_id));
    let blocks = WHOLE_COPY_BYTES / MIB;
    let script = format!(
        "set -u\n\
         DEV=$(readlink -f {link})\n\
         REPORT=$(dd if=/dev/urandom of=/tmp/payload bs=1M count={blocks} 2>&1)\n\
         echo \"{WROTE_MARKER} $?\"\n\
         echo \"{BYTES_MARKER} $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"\n\
         echo \"{PAYLOAD_MARKER} $(sha256sum /tmp/payload | cut -d' ' -f1)\"\n\
         dd if=/tmp/payload of=$DEV bs=1M oflag=direct conv=fsync 2>/dev/null\n\
         echo \"{EXPECTED_MARKER} $?\"\n\
         echo \"{READBACK_MARKER} $(dd if=$DEV bs=1M count={blocks} status=none | sha256sum | \
         cut -d' ' -f1)\"\n"
    );
    let written = in_guest(&engine, case, &instance_id, &script);
    assert_eq!(
        marker(&written, WROTE_MARKER).as_deref(),
        Some("0"),
        "case {case:?}: expected the machine's own storage to take the payload, got {written:?}"
    );
    assert_eq!(
        marker(&written, BYTES_MARKER).as_deref(),
        Some(WHOLE_COPY_BYTES.to_string().as_str()),
        "case {case:?}: expected the copy to report the {WHOLE_COPY_BYTES} bytes it wrote into \
         the file, got {written:?}"
    );
    assert_eq!(
        marker(&written, EXPECTED_MARKER).as_deref(),
        Some("0"),
        "case {case:?}: expected the copy of the payload through the device to succeed, got \
         {written:?}"
    );
    let payload = marker(&written, PAYLOAD_MARKER).unwrap_or_default();
    let readback = marker(&written, READBACK_MARKER).unwrap_or_default();
    assert!(
        payload != EMPTY_DIGEST && !payload.is_empty(),
        "case {case:?}: expected a payload of real bytes, got {payload:?} of {written:?}"
    );
    assert_eq!(
        readback, payload,
        "case {case:?}: expected the read-back of the written range to be the payload, got \
         {readback:?} against {payload:?}"
    );
}

/// THE RULE where the destination is the machine's OWN STORAGE: that storage IS the instance's
/// root volume, so the bytes a guest writes into a file of it are a transfer through that volume —
/// which is what the root volume's own `AWS/EBS` series measure, under exactly its `VolumeId`.
#[tokio::test]
async fn a_write_to_the_machines_own_storage_is_io_on_its_root_volume() {
    let (router, engine) = emulator();
    let case = "a payload written into a file of the root filesystem";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    // The volume the machine's root filesystem is, as the control plane reports it: the volume of
    // the instance's own block device mapping.
    let body = ec2_ok(
        &router,
        case,
        "DescribeInstances",
        &[p("InstanceId.1", &instance_id)],
    )
    .await;
    let root_volume = element(&body, "volumeId").unwrap_or_else(|| {
        panic!("case {case:?}: the instance must report its root volume: {body}")
    });

    // Two writes, each of a stated size, into files of that filesystem.
    let payload_bytes = 4 * MIB;
    let blocks = payload_bytes / MIB;
    let script = format!(
        "set -u\n\
         dd if=/dev/urandom of=/tmp/first bs=1M count={blocks} status=none\n\
         echo \"{WROTE_MARKER} $?\"\n\
         dd if=/dev/urandom of=/tmp/second bs=1M count={blocks} status=none\n\
         echo \"{EXPECTED_MARKER} $?\"\n"
    );
    let written = in_guest(&engine, case, &instance_id, &script);
    for marked in [WROTE_MARKER, EXPECTED_MARKER] {
        assert_eq!(
            marker(&written, marked).as_deref(),
            Some("0"),
            "case {case:?}: expected the write into the root filesystem to succeed, got \
             {written:?}"
        );
    }

    // The datapoints are published off the answer to a request, so they are asked for again until
    // they are there rather than waited for.
    let wanted = f64::from(u32::try_from(payload_bytes).unwrap_or_default());
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let written_bytes = volume_metric_sum(&router, case, &root_volume, "VolumeWriteBytes").await;
        let written_ops = volume_metric_sum(&router, case, &root_volume, "VolumeWriteOps").await;
        if written_bytes >= wanted && written_ops > 0.0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the root volume {root_volume} to publish at least {wanted} \
             written bytes and the operations that moved them, got {written_bytes} bytes over \
             {written_ops} operations"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// THE RULE at the attach boundary, over the CADENCE: "data is only reported to CloudWatch when
/// the volume is attached to an instance", so an attached volume is reported for every period of
/// its own cadence — a period nothing moved in carrying a reading of zero — and a volume attached
/// to nothing is reported for no period at all.
///
/// The periods are driven by asking the publisher for the readings due at instants of this case's
/// choosing, never by waiting for minutes to pass.
#[tokio::test]
async fn an_attached_volume_is_reported_every_period_and_a_detached_one_not_at_all() {
    let (router, engine) = emulator();
    let case = "a volume attached, then taken off";
    let instance_id = launch(&router, case).await;
    until_the_guest_is_up(&engine, case, &instance_id).await;
    let zone = zone_of(&router, case, &instance_id).await;
    let volume_id = attached_volume(&router, case, &zone, &instance_id, DEVICES[0]).await;

    // The periods of a span laid ahead of the wall clock, so the publisher's own looks land in
    // earlier periods than the ones this case drives.
    let attached_base = span_base(0);
    sample_minutes(&engine, attached_base, 0..SPAN_MINUTES);
    let while_attached = volume_periods(
        &router,
        case,
        &volume_id,
        "VolumeWriteOps",
        attached_base,
        attached_base + SPAN_MINUTES * MINUTE,
    )
    .await;
    assert!(
        while_attached.len() >= 2,
        "case {case:?}: expected the attached volume to be reported in every period of the span, \
         got {while_attached:?}"
    );
    for (at, sum) in &while_attached {
        assert!(
            at.rem_euclid(MINUTE) == 0,
            "case {case:?}: expected each reading on the minute grid Amazon EBS reports an SSD \
             volume on, got {at} of {while_attached:?}"
        );
        assert!(
            sum.abs() < f64::EPSILON,
            "case {case:?}: expected a period nothing moved in to read zero, got {sum} at {at}"
        );
    }

    // Taken off the instance, the volume is one Amazon EBS reports nothing for: the periods after
    // the detach carry no reading at all.
    let _ = ec2_ok(
        &router,
        case,
        "DetachVolume",
        &[p("VolumeId", &volume_id), p("InstanceId", &instance_id)],
    )
    .await;
    // ... waiting for the CONDITION, which is the attachment being GONE: a detach answers an
    // attachment in `detaching`, and the volume reads `available` once the move is over.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let body = ec2_ok(
            &router,
            case,
            "DescribeVolumes",
            &[p("VolumeId.1", &volume_id)],
        )
        .await;
        if body.contains("<status>available</status>") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the detach never took the attachment down: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let detached_base = span_base(1);
    sample_minutes(&engine, detached_base, 0..SPAN_MINUTES);
    let while_detached = volume_periods(
        &router,
        case,
        &volume_id,
        "VolumeWriteOps",
        detached_base,
        detached_base + SPAN_MINUTES * MINUTE,
    )
    .await;
    assert!(
        while_detached.is_empty(),
        "case {case:?}: expected nothing reported for a volume attached to nothing, got \
         {while_detached:?}"
    );
}

/// The datapoints of `metric` for `volume_id` inside `[start, end)`, as `(instant, sum)` oldest
/// first, read at one-minute granularity so every reported period is visible on its own.
async fn volume_periods(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    metric: &str,
    start: i64,
    end: i64,
) -> Vec<(i64, f64)> {
    let answer = cloudwatch_ok(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": "AWS/EBS",
            "MetricName": metric,
            "Dimensions": [{"Name": "VolumeId", "Value": volume_id}],
            "StartTime": start,
            "EndTime": end,
            "Period": METRIC_PERIOD_SECS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let mut points: Vec<(i64, f64)> = answer["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Timestamp"].as_f64().unwrap_or_default().round() as i64,
                point["Sum"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    points.sort_by_key(|(at, _)| *at);
    points
}

/// The `Sum` of `metric` for `volume_id` over the window around now, read back through Amazon
/// CloudWatch's own `GetMetricStatistics` — zero for a series carrying no datapoint at all.
async fn volume_metric_sum(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    metric: &str,
) -> f64 {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or_default();
    let answer = cloudwatch_ok(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": "AWS/EBS",
            "MetricName": metric,
            "Dimensions": [{"Name": "VolumeId", "Value": volume_id}],
            "StartTime": now - METRIC_WINDOW_SECS,
            "EndTime": now + METRIC_WINDOW_SECS,
            "Period": METRIC_PERIOD_SECS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answer["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| point["Sum"].as_f64().unwrap_or_default())
        .sum()
}


