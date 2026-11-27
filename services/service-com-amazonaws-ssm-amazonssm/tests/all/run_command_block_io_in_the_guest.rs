// SPDX-License-Identifier: CC0-1.0

//! A Run Command invocation performing real BLOCK I/O on a volume attached to the node it was
//! sent to, driven end to end with Systems Manager and Amazon EC2 on one engine: the node is the
//! managed node a launched instance became, the volume is created and attached over EC2's own
//! wire, and the script reaches the guest through `AmazonSSM.SendCommand`.
//!
//! RULE under test: an invocation is the document's script PERFORMED in the node's guest, so a
//! copy it issues against the device of a volume the control plane reports attached transfers the
//! bytes it asked for, exits 0, reports that count, and a read of the same range answers exactly
//! those bytes. None of that depends on how much the guest has already written: a payload the
//! script generates on the machine's own storage, the copy of that payload through the device and
//! the read-back are three whole copies of one run.
//!
//! Every expected figure is spelled out here rather than imported, so a wrong constant in the
//! implementation cannot make the case pass.

use std::time::{Duration, Instant};

use super::ec2_instance_managed_nodes::{
    launched_long_enough_ago, Emulator, Grant, AL2023_NAME_PATTERN, REGION, SSM_CORE_POLICY_ARN,
};
use super::run_command_in_the_guest::{finish_due, finished, send};

/// The directory the kernel publishes a block device's stable link under, and what the link of
/// an Amazon EBS volume on a Nitro instance is named: the model the NVMe device presents with the
/// volume's serial after it (EC2 User Guide, "Amazon EBS and NVMe on Linux instances").
const BY_ID_LINK: &str = "/dev/disk/by-id/nvme-Amazon_Elastic_Block_Store_";

/// The digest of NO bytes at all, which is what a read that answered nothing comes to.
const EMPTY_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// How many mebibyte blocks the payload is, and how many bytes that comes to: a whole payload of
/// the size a client checking a volume's throughput writes.
const PAYLOAD_BLOCKS: usize = 64;
const PAYLOAD_BYTES: usize = PAYLOAD_BLOCKS * 1024 * 1024;

/// How long the polls below keep asking before giving up: far above the span any move they wait
/// for is owed in, so a loaded host is not what decides the case.
const POLL_DEADLINE: Duration = Duration::from_secs(90);

/// How often they look again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The value the guest wrote under `marker`, or `None` when the marker carries nothing.
fn marker(output: &str, marker: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix(&format!("{marker} ")))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Look again until a `status` element of `volume`'s describe reads `word` — the CONDITION the
/// case waits on, never a wait of its own choosing.
async fn until(emulator: &Emulator, case: &str, volume: &str, word: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let body = emulator
            .ec2(case, "DescribeVolumes", &[("VolumeId.1", volume)])
            .await;
        if body.contains(&format!("<status>{word}</status>")) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {volume} never reached {word:?}: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the copy the script issues against the attached volume's device moves the bytes, reports
/// them, and the range reads back as what was written.
#[tokio::test]
async fn an_invocation_writes_a_payload_through_an_attached_volume_and_reads_it_back() {
    let emulator = Emulator::new();
    let case = "a payload written through an attached volume";
    let image = emulator.image(case, AL2023_NAME_PATTERN).await;
    let profile = emulator
        .profile(case, "block-io-role", &Grant::Attached(SSM_CORE_POLICY_ARN))
        .await;
    let instance = emulator.launch(case, &image, Some(&profile)).await;
    emulator.launched_ago(&instance, launched_long_enough_ago());
    let nodes = emulator.nodes(case, REGION).await;
    assert!(
        nodes.contains_key(&instance),
        "case {case:?}: the launched instance must become a managed node first, got {:?}",
        nodes.keys().collect::<Vec<_>>()
    );

    // The volume is created in the instance's own zone and attached over EC2's wire, and the case
    // waits for the attachment the control plane reports rather than for a span of its own.
    let body = emulator
        .ec2(case, "DescribeInstances", &[("InstanceId.1", &instance)])
        .await;
    let zone = Emulator::answered(&body, "availabilityZone")
        .unwrap_or_else(|| panic!("case {case:?}: the instance must report its zone: {body}"));
    let body = emulator
        .ec2(
            case,
            "CreateVolume",
            &[
                ("AvailabilityZone", &zone),
                ("Size", "1"),
                ("VolumeType", "gp3"),
            ],
        )
        .await;
    let volume = Emulator::answered(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume must mint a volume: {body}"));
    until(&emulator, case, &volume, "available").await;
    emulator
        .ec2(
            case,
            "AttachVolume",
            &[
                ("VolumeId", &volume),
                ("InstanceId", &instance),
                ("Device", "/dev/sdf"),
            ],
        )
        .await;
    until(&emulator, case, &volume, "attached").await;

    // The script a client sends to use that volume: resolve the device behind the volume's own
    // link, generate a payload on the machine's own storage, copy it through the device and read
    // the same range back.
    let serial = volume.replace('-', "");
    let link = format!("{BY_ID_LINK}{serial}");
    let script: Vec<String> = vec![
        format!("dev={link}"),
        "DEV=$(readlink -f $dev)".to_string(),
        "echo \"EBS-DEV $DEV\"".to_string(),
        "echo \"EBS-SERIAL $(lsblk -dno SERIAL $DEV)\"".to_string(),
        format!("dd if=/dev/urandom of=/tmp/payload bs=1M count={PAYLOAD_BLOCKS} 2>/dev/null"),
        "REPORT=$(dd if=/tmp/payload of=$dev bs=1M oflag=direct conv=fsync 2>&1)".to_string(),
        "echo \"EBS-DD-RC $?\"".to_string(),
        "echo \"EBS-DD $(echo \"$REPORT\" | tail -1 | cut -d' ' -f1)\"".to_string(),
        format!(
            "echo \"EBS-READBACK $(dd if=$dev bs=1M count={PAYLOAD_BLOCKS} iflag=direct \
             2>/dev/null | sha256sum | cut -d' ' -f1)\""
        ),
        "echo \"EBS-PAYLOAD $(sha256sum /tmp/payload | cut -d' ' -f1)\"".to_string(),
    ];
    let lines: Vec<&str> = script.iter().map(String::as_str).collect();
    let command_id = send(&emulator, case, &instance, &lines).await;
    finish_due(&emulator, &command_id);
    let answered = finished(&emulator, case, &command_id, &instance).await;
    assert_eq!(
        answered["Status"].as_str(),
        Some("Success"),
        "case {case:?}: expected the invocation to succeed, got {answered}"
    );
    let output = answered["StandardOutputContent"]
        .as_str()
        .unwrap_or_default();

    // The device behind the volume's own link is a device of this guest, and it reports the
    // volume it carries.
    let device = marker(output, "EBS-DEV").unwrap_or_default();
    assert!(
        device.starts_with("/dev/nvme"),
        "case {case:?}: expected the link to resolve to the NVMe device carrying {volume}, got \
         {device:?} of {output:?}"
    );
    assert_eq!(
        marker(output, "EBS-SERIAL").as_deref(),
        Some(serial.as_str()),
        "case {case:?}: expected the device to report serial {serial:?}, got {output:?}"
    );

    // The copy moved the payload and said so ...
    assert_eq!(
        marker(output, "EBS-DD-RC").as_deref(),
        Some("0"),
        "case {case:?}: expected the copy through the device to succeed, got {output:?}"
    );
    assert_eq!(
        marker(output, "EBS-DD").as_deref(),
        Some(PAYLOAD_BYTES.to_string().as_str()),
        "case {case:?}: expected the copy to report the {PAYLOAD_BYTES} bytes it moved, got \
         {output:?}"
    );

    // ... and the bytes are ON the volume: reading that range back answers exactly them.
    let payload = marker(output, "EBS-PAYLOAD").unwrap_or_default();
    let readback = marker(output, "EBS-READBACK").unwrap_or_default();
    assert!(
        payload != EMPTY_DIGEST && !payload.is_empty(),
        "case {case:?}: expected a payload of real bytes, got {payload:?} of {output:?}"
    );
    assert_eq!(
        readback, payload,
        "case {case:?}: expected the read-back of the written range to be the payload, got \
         {readback:?} against {payload:?}"
    );
}
