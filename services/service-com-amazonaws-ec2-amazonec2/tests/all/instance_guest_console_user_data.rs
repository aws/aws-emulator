// SPDX-License-Identifier: CC0-1.0

//! What an instance's GUEST does with that instance's own user data, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! RULE: "the user data to make available to the instance" is not a value a client reads
//! back — an instance's init system RUNS it when the guest comes up, and what a client
//! observes of it is on that instance's CONSOLE ("the instance console output displays the
//! exact console output that would normally be displayed on a physical monitor attached to a
//! computer", `GetConsoleOutput`). So:
//!
//! * a console carries the GUEST's log: printk-timestamped kernel lines and the init
//!   system's own `cloud-init[…]` lines, once per boot;
//! * a command the user data gives the init system to run on EVERY boot runs on every boot,
//!   and one it gives it to run once runs on the first boot only;
//! * a line that command writes to the guest's console device is on the console exactly as
//!   the command wrote it, so a value the command computed — an identity the kernel
//!   published to this boot, a count the guest keeps on its own storage — is observable;
//! * a console is APPENDED to, so what the earlier boots wrote is still there, byte for
//!   byte, after a reboot — and a reboot restarts the operating system rather than the
//!   instance, so the instance keeps its id, its launch time and its `running` state.
//!
//! The rows differ along what the rule generalises over: the FORM the user data takes (a
//! `#cloud-config` with the every-boot module, with the once-only module, with an argument
//! vector rather than a line of text, a `#!` script, a form cloud-init runs nothing for, and
//! no user data at all) and what the command computes (nothing, a counter kept across boots,
//! the boot identity the kernel published). No row is the specification.

use std::time::Duration;

use base64::Engine as _;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, element, elements, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The nonce this suite's user data writes beside the boot identity, so a marker line can be
/// told from anything else on the console.
const NONCE: &str = "nonce-7f3a";

/// The every-boot program: it keeps a count on the guest's own storage, reads the boot
/// identity the kernel published to THIS boot, and writes one marker line to the console
/// device.
const EVERY_BOOT_PROGRAM: &str = "#cloud-config\n\
    bootcmd:\n\
    \x20 - |\n\
    \x20   COUNT=$(cat /var/lib/sim2real/boots 2>/dev/null || echo 0)\n\
    \x20   COUNT=$((COUNT + 1))\n\
    \x20   mkdir -p /var/lib/sim2real\n\
    \x20   echo $COUNT > /var/lib/sim2real/boots\n\
    \x20   echo \"SIM2REAL_BOOT nonce-7f3a $(cat /proc/sys/kernel/random/boot_id) $COUNT\" > /dev/console\n";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One emulator over its own engine, with one AMI of the calling account seeded the way a
/// register operation would store it — answered beside its image id.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.insert_image(ImageSpec {
            name: "al2023-ami-2023.6.20241111.0-kernel-6.1-x86_64".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch one instance from `image_id` with `user_data` as its user data (base64, as the API
/// carries it), answering its id.
async fn launch(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
    user_data: Option<&str>,
) -> String {
    let mut params = vec![
        p("ImageId", image_id),
        p("InstanceType", "t3.micro"),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    if let Some(user_data) = user_data {
        params.push(p(
            "UserData",
            base64::engine::general_purpose::STANDARD.encode(user_data),
        ));
    }
    let (status, body) = api(router, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status}: {body}"
    );
    elements(&body, "instanceId")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report an instance: {body}"))
}

/// One instance's console as text — empty for an instance whose guest has written nothing,
/// which is an instance that carries no `output` member at all.
async fn console(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = api(
        router,
        "GetConsoleOutput",
        &[p("InstanceId", instance_id), p("Latest", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConsoleOutput must succeed, got {status}: {body}"
    );
    let Some(encoded) = member(&body, "output") else {
        return String::new();
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: output must be base64 ({error})"));
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the console text must be UTF-8: {error}"))
}

/// Ask for one reboot of `instance_id` and wait for the boot it queues to fall due:
/// `RebootInstances` "only queues a request to reboot the specified instances".
async fn reboot(router: &MultiServiceRouter, case: &str, instance_id: &str) {
    let (status, body) = api(
        router,
        "RebootInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RebootInstances must succeed, got {status}: {body}"
    );
    past_the_transition().await;
}

/// Whether `line` carries the printk timestamp a kernel prefixes a console line with, as
/// `[    6.096338]`.
fn printk_stamped(line: &str) -> bool {
    let Some((stamp, _)) = line.strip_prefix('[').and_then(|rest| rest.split_once(']')) else {
        return false;
    };
    let Some((seconds, micros)) = stamp.trim().split_once('.') else {
        return false;
    };
    !seconds.is_empty()
        && seconds.chars().all(|character| character.is_ascii_digit())
        && micros.len() == 6
        && micros.chars().all(|character| character.is_ascii_digit())
}

/// The marker lines this suite's user data wrote, in the order the console carries them.
fn markers(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.starts_with("SIM2REAL_BOOT "))
        .collect()
}

/// The boot identities the guest published on the console, in the order it booted.
fn boot_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once("ci-info: boot-id: "))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

/// Whether `value` reads as a UUID the way `/proc/sys/kernel/random/boot_id` publishes one:
/// 36 characters of lowercase hexadecimal and hyphens.
fn reads_as_a_boot_id(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
        && value.chars().all(|character| !character.is_ascii_uppercase())
}

/// One row of the user-data table: the form the user data takes and what each boot's console
/// must then carry.
struct Row {
    case: &'static str,
    /// The user data the launch carries, or `None` for an instance with none.
    user_data: Option<String>,
    /// The lines the FIRST boot's console must carry.
    first_boot: Vec<String>,
    /// The lines a LATER boot's console must carry again.
    every_boot: Vec<String>,
    /// The lines no boot after the first may write.
    only_once: Vec<String>,
}

/// RULE: the init system runs the instance's own user data on every boot it is given for,
/// and what it writes to the console device is on the console as written.
#[tokio::test]
async fn the_guest_runs_the_instance_s_user_data_and_the_console_carries_it() {
    let line = |text: &str| vec![text.to_string()];
    let rows: [Row; 6] = [
        Row {
            case: "an every-boot command computing from the guest's own state",
            user_data: Some(EVERY_BOOT_PROGRAM.to_string()),
            first_boot: Vec::new(),
            every_boot: Vec::new(),
            only_once: Vec::new(),
        },
        Row {
            case: "an every-boot command written as a line of text",
            user_data: Some(
                "#cloud-config\nbootcmd:\n  - echo EVERY-BOOT > /dev/console\n".to_string(),
            ),
            first_boot: line("EVERY-BOOT"),
            every_boot: line("EVERY-BOOT"),
            only_once: Vec::new(),
        },
        Row {
            case: "an every-boot command written as an argument vector",
            user_data: Some(
                "#cloud-config\nbootcmd:\n  - [ sh, -c, \"echo VECTOR-BOOT > /dev/console\" ]\n"
                    .to_string(),
            ),
            first_boot: line("VECTOR-BOOT"),
            every_boot: line("VECTOR-BOOT"),
            only_once: Vec::new(),
        },
        Row {
            case: "a once-only command",
            user_data: Some(
                "#cloud-config\nruncmd:\n  - echo ONCE-ONLY > /dev/console\n".to_string(),
            ),
            first_boot: line("ONCE-ONLY"),
            every_boot: Vec::new(),
            only_once: line("ONCE-ONLY"),
        },
        Row {
            case: "a user-data script, which the init system runs once",
            user_data: Some("#!/bin/sh\necho FROM-THE-SCRIPT > /dev/console\n".to_string()),
            first_boot: line("FROM-THE-SCRIPT"),
            every_boot: Vec::new(),
            only_once: line("FROM-THE-SCRIPT"),
        },
        Row {
            case: "a form the init system runs nothing for",
            user_data: Some("#include\nhttps://example.invalid/program\n".to_string()),
            first_boot: Vec::new(),
            every_boot: Vec::new(),
            only_once: Vec::new(),
        },
    ];

    for Row {
        case,
        user_data,
        first_boot,
        every_boot,
        only_once,
    } in rows
    {
        let (router, image_id) = emulator();
        let instance_id = launch(&router, case, &image_id, user_data.as_deref()).await;
        past_the_transition().await;

        let after_launch = console(&router, case, &instance_id).await;
        // Whatever the user data, the boot's own log is on the console.
        let stamped = after_launch.lines().filter(|line| printk_stamped(line)).count();
        assert!(
            stamped >= 20,
            "case {case:?}: expected the guest's kernel lines on the console, got {stamped} \
             printk-stamped line(s) of {after_launch:?}"
        );
        assert!(
            after_launch.lines().any(|line| line.contains("cloud-init[")),
            "case {case:?}: expected the init system's own lines, got {after_launch:?}"
        );
        for expected in &first_boot {
            assert!(
                after_launch.lines().any(|line| line == expected),
                "case {case:?}: expected the first boot's console to carry {expected:?}, got \
                 {after_launch:?}"
            );
        }

        reboot(&router, case, &instance_id).await;
        let after_reboot = console(&router, case, &instance_id).await;
        assert!(
            after_reboot.starts_with(&after_launch),
            "case {case:?}: a console is appended to, so the first boot's output must still \
             be there byte for byte; got {after_reboot:?} after {after_launch:?}"
        );
        let written_since = &after_reboot[after_launch.len()..];
        for expected in &every_boot {
            assert!(
                written_since.lines().any(|line| line == expected),
                "case {case:?}: expected the later boot to write {expected:?} again, got \
                 {written_since:?}"
            );
        }
        for refused in &only_once {
            assert!(
                !written_since.lines().any(|line| line == refused),
                "case {case:?}: expected a once-only command not to run again, got \
                 {written_since:?}"
            );
        }

        // A reboot restarts the operating system, not the instance.
        let (status, body) = api(
            &router,
            "DescribeInstances",
            &[p("InstanceId.1", &instance_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInstances must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "instanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: the instance must keep its id across a reboot, got {body}"
        );
        assert_eq!(
            described_instance_state(&router, REGION, &instance_id).await,
            "running",
            "case {case:?}: the instance must still be running after a reboot"
        );
        let launch_time = element(&body, "launchTime")
            .unwrap_or_else(|| panic!("case {case:?}: expected a launch time in {body}"));
        let reported_launch = after_launch
            .lines()
            .find_map(|line| line.split_once("ci-info: launch-time: "))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default();
        assert!(
            !reported_launch.is_empty(),
            "case {case:?}: the guest must report the launch time it came up with, got \
             {after_launch:?}"
        );
        assert!(
            after_reboot
                .lines()
                .filter_map(|line| line.split_once("ci-info: launch-time: "))
                .all(|(_, value)| value.trim() == reported_launch),
            "case {case:?}: a reboot must not change the launch time, got {after_reboot:?}"
        );
        assert!(
            !launch_time.is_empty(),
            "case {case:?}: DescribeInstances must report a launch time, got {body}"
        );
    }
}

/// RULE: a command that computes from the guest's OWN state computes it afresh each boot —
/// the identity the kernel published to that boot, and a count kept on the guest's storage —
/// and the console carries one marker per boot with the earlier ones unchanged.
#[tokio::test]
async fn the_guest_s_program_computes_from_this_boot_s_own_state() {
    let case = "a marker line per boot";
    let (router, image_id) = emulator();
    let instance_id = launch(&router, case, &image_id, Some(EVERY_BOOT_PROGRAM)).await;
    past_the_transition().await;

    let first = console(&router, case, &instance_id).await;
    let first_markers = markers(&first);
    let first_ids = boot_ids(&first);
    assert_eq!(
        first_markers.len(),
        1,
        "case {case:?}: expected one marker after the first boot, got {first_markers:?}"
    );
    assert_eq!(
        first_ids.len(),
        1,
        "case {case:?}: expected one boot on the console, got {first_ids:?}"
    );
    assert!(
        reads_as_a_boot_id(&first_ids[0]),
        "case {case:?}: expected the boot identity to read as a UUID, got {:?}",
        first_ids[0]
    );
    assert_eq!(
        first_markers[0],
        format!("SIM2REAL_BOOT {NONCE} {} 1", first_ids[0]),
        "case {case:?}: expected the marker to carry this boot's own identity and the first \
         count, got {:?}",
        first_markers[0]
    );

    reboot(&router, case, &instance_id).await;
    let second = console(&router, case, &instance_id).await;
    let second_markers = markers(&second);
    let second_ids = boot_ids(&second);
    assert_eq!(
        second_markers.len(),
        2,
        "case {case:?}: expected one marker per boot, got {second_markers:?}"
    );
    assert_eq!(
        second_markers[0], first_markers[0],
        "case {case:?}: the first boot's marker must be unchanged, got {:?}",
        second_markers[0]
    );
    assert_eq!(
        second_ids.len(),
        2,
        "case {case:?}: expected two boots on the console, got {second_ids:?}"
    );
    assert_ne!(
        second_ids[0], second_ids[1],
        "case {case:?}: each boot must have an identity of its own, got {second_ids:?}"
    );
    assert!(
        reads_as_a_boot_id(&second_ids[1]),
        "case {case:?}: expected the second boot's identity to read as a UUID, got {:?}",
        second_ids[1]
    );
    assert_eq!(
        second_markers[1],
        format!("SIM2REAL_BOOT {NONCE} {} 2", second_ids[1]),
        "case {case:?}: expected the marker to carry the new boot's identity and the next \
         count, got {:?}",
        second_markers[1]
    );

    // What lies BETWEEN the two markers is the second boot's own log: the kernel's
    // printk-timestamped lines and the init system's.
    let between = {
        let first_end = second
            .find(second_markers[0])
            .map(|at| at + second_markers[0].len())
            .unwrap_or_else(|| panic!("case {case:?}: the first marker must be on the console"));
        let second_start = second
            .find(second_markers[1])
            .unwrap_or_else(|| panic!("case {case:?}: the second marker must be on the console"));
        assert!(
            second_start > first_end,
            "case {case:?}: the second boot's marker must come after the first's"
        );
        &second[first_end..second_start]
    };
    assert!(
        between.len() >= 500,
        "case {case:?}: expected the second boot's own log between the markers, got {} \
         byte(s): {between:?}",
        between.len()
    );
    let stamped = between.lines().filter(|line| printk_stamped(line)).count();
    assert!(
        stamped >= 20,
        "case {case:?}: expected at least 20 printk-timestamped lines between the markers, \
         got {stamped} of {between:?}"
    );
    assert!(
        between.lines().any(|line| line.contains("cloud-init[")),
        "case {case:?}: expected the init system's own lines between the markers, got \
         {between:?}"
    );
}
