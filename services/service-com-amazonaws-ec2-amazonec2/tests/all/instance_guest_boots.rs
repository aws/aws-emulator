// SPDX-License-Identifier: CC0-1.0

//! What a BOOT of an instance's guest is, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! RULE: an instance's guest boots whenever its operating system starts — at the instance's
//! launch, when a stopped instance is started again, and when a queued reboot takes effect —
//! and each boot APPENDS its own block to the console the instance writes, carrying a boot
//! identity of its own. So the console of an instance that has restarted still holds, byte
//! for byte, what its earlier boots wrote, with the new boot's lines after them; and because
//! a reboot restarts the operating system rather than the instance, the instance keeps its
//! id, its launch time and the `running` state it was in.
//!
//! A reboot is owed to the CLOCK, not to the call: `RebootInstances` "only queues a request
//! to reboot the specified instances", so the new boot falls due a moment later, whether or
//! not anybody calls again — nothing here sleeps for a transition it has not read.
//!
//! The rows differ along what the rule generalises over: HOW MANY times the guest is
//! restarted (none, once, twice, three times), HOW the restart is asked for (one call per
//! reboot, one call naming the instance twice, and a stop/start cycle rather than a reboot),
//! and how many instances share the launch — so no row is the specification.

use std::time::Duration;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, described_instance_state, elements, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The marker a guest's init system writes the facts it discovered about its machine under,
/// which is where a client reads one. Spelled out here rather than imported, so a changed
/// rendering fails this suite.
const CI_INFO_MARKER: &str = "ci-info: ";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
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
            name: "instance-guest-boots".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch `count` instances, answering their ids in launch order.
async fn launch(router: &MultiServiceRouter, case: &str, image_id: &str, count: usize) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", "t3.small"),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status}: {body}"
    );
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "case {case:?}: RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// One instance's console as text, beside the instant the response says it was last updated.
///
/// The text is empty for an instance whose guest has written nothing yet, which is an
/// instance that carries no `output` member at all.
async fn console(router: &MultiServiceRouter, case: &str, instance_id: &str) -> (String, f64) {
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
    let text = match member(&body, "output") {
        Some(encoded) => {
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: output must be base64 ({error})")
                    });
            String::from_utf8(bytes).unwrap_or_else(|error| {
                panic!("case {case:?}: the console text must be UTF-8: {error}")
            })
        }
        None => String::new(),
    };
    let timestamp = member(&body, "timestamp")
        .and_then(|value| as_timestamp(&value))
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a timestamp: {body}"));
    (text, timestamp)
}

/// Every value of `text` the guest published the fact `label` under, in the order the
/// console carries them: a guest's init system writes one as `ci-info: <name>: <value>`, so
/// a name that is the prefix of another cannot be read for it.
fn console_values(text: &str, label: &str) -> Vec<String> {
    let marker = format!("{CI_INFO_MARKER}{label}: ");
    text.lines()
        .filter_map(|line| line.split_once(&marker))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

/// Whether `value` has the form of a UUID: five hyphen-separated groups of 8-4-4-4-12
/// hexadecimal digits, which is what a guest's boot identity is.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// What `DescribeInstances` reports about one instance: the launch time it keeps across a
/// restart.
async fn launch_time(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = api(router, "DescribeInstances", &[p("InstanceId.1", instance_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must succeed, got {status}: {body}"
    );
    super::ec2_query::element(&body, "launchTime")
        .unwrap_or_else(|| panic!("case {case:?}: DescribeInstances must report a launchTime: {body}"))
}

/// How a row restarts the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restart {
    /// One `RebootInstances` call per restart.
    Reboot,
    /// One `RebootInstances` call naming the instance TWICE: one reboot, not two.
    RebootNamingItTwice,
    /// `StopInstances` and then `StartInstances`: the guest comes up again.
    StopStart,
}

/// One row: how the guest is restarted, how often, and how many instances share the launch.
struct Row {
    case: &'static str,
    how: Restart,
    /// How many times the row asks for a restart.
    times: usize,
    /// How many boots the console must then report.
    boots: usize,
    /// How many instances share the launch; the row acts on the FIRST and the rest are
    /// bystanders that must stay on their first boot.
    instances: usize,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a-launched-guest-boots-once",
            how: Restart::Reboot,
            times: 0,
            boots: 1,
            instances: 1,
        },
        Row {
            case: "one-reboot-boots-the-guest-again",
            how: Restart::Reboot,
            times: 1,
            boots: 2,
            instances: 2,
        },
        Row {
            case: "two-reboots-boot-it-twice-more",
            how: Restart::Reboot,
            times: 2,
            boots: 3,
            instances: 1,
        },
        Row {
            case: "three-reboots-boot-it-three-times-more",
            how: Restart::Reboot,
            times: 3,
            boots: 4,
            instances: 1,
        },
        Row {
            // "One instance is rebooted once however many times the batch names it", so the
            // call that names it twice is one restart.
            case: "one-call-naming-the-instance-twice-is-one-reboot",
            how: Restart::RebootNamingItTwice,
            times: 1,
            boots: 2,
            instances: 1,
        },
        Row {
            // A stopped instance has no running operating system, so starting it again is a
            // boot of its own — the same rule, reached another way.
            case: "a-stop-start-cycle-boots-the-guest-again",
            how: Restart::StopStart,
            times: 1,
            boots: 2,
            instances: 1,
        },
    ]
}

/// Ask for one restart of `instance_id` the way `how` says.
async fn restart(router: &MultiServiceRouter, case: &str, how: Restart, instance_id: &str) {
    match how {
        Restart::Reboot => {
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
        }
        Restart::RebootNamingItTwice => {
            let (status, body) = api(
                router,
                "RebootInstances",
                &[p("InstanceId.1", instance_id), p("InstanceId.2", instance_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: RebootInstances must succeed, got {status}: {body}"
            );
        }
        Restart::StopStart => {
            for action in ["StopInstances", "StartInstances"] {
                let (status, body) = api(router, action, &[p("InstanceId.1", instance_id)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {action} must succeed, got {status}: {body}"
                );
                // The stop has to have settled before the start is asked for: an instance on
                // its way down is not one that can be started.
                past_the_transition().await;
            }
        }
    }
}

/// RULE: every start of the guest is a boot of its own, appended to the console, and the
/// instance itself is left where it was.
#[tokio::test]
async fn every_start_of_the_guest_is_a_boot_appended_to_the_console() {
    let (router, image_id) = emulator();
    for row in rows() {
        let case = row.case;
        let instance_ids = launch(&router, case, &image_id, row.instances).await;
        let instance_id = instance_ids[0].clone();
        let launched_at = launch_time(&router, case, &instance_id).await;

        // The launch's own boot: owed to the clock at the instant the instance finishes
        // starting, so a console read straight after the launch carries none yet.
        let (before_running, _) = console(&router, case, &instance_id).await;
        assert!(
            console_values(&before_running, "boot-id").is_empty(),
            "case {case:?}: an instance whose guest has not come up yet reports no boot, got \
             {before_running:?}"
        );
        past_the_transition().await;

        let mut seen: Vec<String> = Vec::new();
        let mut text = String::new();
        for round in 0..=row.times {
            if round > 0 {
                restart(&router, case, row.how, &instance_id).await;
                past_the_transition().await;
            }
            let (after, timestamp) = console(&router, case, &instance_id).await;
            // A console is APPENDED to: what the earlier boots wrote is still there, byte
            // for byte, with the new boot's lines after it.
            assert!(
                after.starts_with(&text),
                "case {case:?} (round {round}): the console must keep what the earlier boots \
                 wrote, got {after:?} after {text:?}"
            );
            assert!(
                after.len() > text.len(),
                "case {case:?} (round {round}): a boot must add to the console, got \
                 {after:?}"
            );
            assert!(
                timestamp > 0.0,
                "case {case:?} (round {round}): the console reports when it was last \
                 updated, got {timestamp}"
            );
            text = after;
            seen = console_values(&text, "boot-id");
            assert_eq!(
                seen.len(),
                round + 1,
                "case {case:?} (round {round}): expected {} boot(s) on the console, got \
                 {seen:?}",
                round + 1
            );
        }

        assert_eq!(
            seen.len(),
            row.boots,
            "case {case:?}: expected {} boot(s) after {} restart(s), got {seen:?}",
            row.boots,
            row.times
        );
        // Each boot has an identity of its own, and no two boots of one guest share one.
        for (index, boot_id) in seen.iter().enumerate() {
            assert!(
                is_uuid(boot_id),
                "case {case:?}: boot {index} must carry a UUID boot identity, got \
                 {boot_id:?}"
            );
            assert!(
                !seen[..index].contains(boot_id),
                "case {case:?}: boot {index} must carry a NEW identity, got {seen:?}"
            );
        }
        // The boots are numbered from the instance's launch, in order.
        let numbers = console_values(&text, "boot");
        assert_eq!(
            numbers,
            (1..=row.boots).map(|n| n.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the boots numbered from the launch, got {numbers:?}"
        );
        // Every boot says when the guest came up.
        assert_eq!(
            console_values(&text, "boot-time").len(),
            row.boots,
            "case {case:?}: every boot reports when the guest came up, got {text:?}"
        );

        // A restart leaves the instance itself where it was.
        assert_eq!(
            described_instance_state(&router, REGION, &instance_id).await,
            "running",
            "case {case:?}: a restarted instance is running again"
        );
        assert_eq!(
            launch_time(&router, case, &instance_id).await,
            launched_at,
            "case {case:?}: a restart must not move the instance's launch time"
        );

        // An instance nothing restarted stays on its first boot, so no row can pass by
        // booting every instance of the launch.
        for bystander in &instance_ids[1..] {
            let (text, _) = console(&router, case, bystander).await;
            assert_eq!(
                console_values(&text, "boot-id").len(),
                1,
                "case {case:?}: the instance nothing restarted stays on its first boot, got \
                 {text:?}"
            );
        }
    }
}

/// RULE: reading a console is a READ — it reports the boots the guest has had and causes
/// none, so two reads of an instance nothing has restarted answer the same bytes.
#[tokio::test]
async fn reading_a_console_boots_nothing() {
    let (router, image_id) = emulator();
    let case = "reading-a-console-boots-nothing";
    let instance_id = launch(&router, case, &image_id, 1)
        .await
        .pop()
        .expect("one instance");
    past_the_transition().await;

    let (first, _) = console(&router, case, &instance_id).await;
    for round in 0..4 {
        let (again, _) = console(&router, case, &instance_id).await;
        assert_eq!(
            again, first,
            "case {case:?} (read {round}): reading a console must not boot the guest"
        );
    }
    assert_eq!(
        console_values(&first, "boot-id").len(),
        1,
        "case {case:?}: an instance nothing restarted has booted once, got {first:?}"
    );
}

/// RULE: a terminated instance has no operating system to restart, so no reboot is queued
/// for one and its console gains no boot — "requests to reboot terminated instances are
/// ignored".
#[tokio::test]
async fn a_terminated_instance_gains_no_boot() {
    let (router, image_id) = emulator();
    let case = "a-terminated-instance-gains-no-boot";
    let instance_id = launch(&router, case, &image_id, 1)
        .await
        .pop()
        .expect("one instance");
    past_the_transition().await;
    let (before, _) = console(&router, case, &instance_id).await;

    let (status, body) = api(
        &router,
        "TerminateInstances",
        &[p("InstanceId.1", &instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TerminateInstances must succeed, got {status}: {body}"
    );
    // The termination has to have settled: a reboot of an instance on its way down is
    // refused, while one of a terminated instance is ignored.
    past_the_transition().await;

    let (status, body) = api(
        &router,
        "RebootInstances",
        &[p("InstanceId.1", &instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a reboot of a terminated instance is ignored, got {status}: {body}"
    );
    past_the_transition().await;

    let (after, _) = console(&router, case, &instance_id).await;
    assert_eq!(
        console_values(&after, "boot-id"),
        console_values(&before, "boot-id"),
        "case {case:?}: a terminated instance gains no boot, got {after:?}"
    );
}
