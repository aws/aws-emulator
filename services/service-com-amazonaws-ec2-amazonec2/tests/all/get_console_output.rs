// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetConsoleOutput`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation SERVES THE NAMED INSTANCE'S CONSOLE AS TEXT, base64-encoded ("the
//!    console output, base64-encoded", `GetConsoleOutputResult$Output`), beside that
//!    instance's own id and the instant the output was last updated: the text is what that
//!    instance's own GUEST WROTE while it came up — a Linux guest's log, printk-timestamped
//!    kernel lines and init-system lines, carrying the stored instance's own facts, the same
//!    facts `DescribeInstances` reports — so no two instances share a console;
//! 2. a console carries what was WRITTEN to it, so an instance whose guest has not come up
//!    yet has no console output at all, and one whose guest has carries that boot's log;
//! 3. every instance id this operation cannot resolve is refused
//!    `InvalidInstanceID.Malformed`, whether or not the value reads like a form EC2 issues:
//!    the two console readers take the strict reading of an instance id, where a well-formed
//!    id that names no instance of the account is "The instance ID '<id>' is malformed" and a
//!    value that is no instance id at all is the id space's own `Invalid id: "<id>"`;
//! 4. `DryRun` is the permission probe, answered BEFORE the instance store is read: it
//!    answers `DryRunOperation` whatever the id names — an instance the account holds, one
//!    it does not, or a value that is no id at all — and writes nothing;
//! 5. the operation is a READ: two calls about one instance answer the same bytes, neither
//!    moves the instance, and `Latest` — the console this service derives from the stored
//!    instance IS the latest — changes nothing either;
//! 6. the `Timestamp` beside the output is read off the SAME console that output is: "the
//!    time at which the output was last updated" (`GetConsoleOutputResult$Timestamp`) is the
//!    latest instant the served output itself shows it was written at — the instance's launch
//!    and each boot its guest has had — so it advances exactly when the guest writes the
//!    buffer, never goes backwards, and reports no update for an event that has written
//!    nothing yet (a reboot `RebootInstances` has only QUEUED).
//!
//! Each table row is a distinct input along what the rule generalises over — the instance
//! type and how many instances share a launch, and for the id rule every way an id can fall
//! outside the issued form beside two inside it that name no instance — so no row is the
//! specification.

use std::time::Duration;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, elements, error_code, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope, since a
/// console output carries no Region member.
const REGION: &str = "us-east-2";

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
            name: "console-output".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch `count` instances of `instance_type` from `image_id`, answering their ids in
/// launch order.
async fn launch(
    router: &MultiServiceRouter,
    image_id: &str,
    instance_type: &str,
    count: usize,
) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", instance_type),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// One `GetConsoleOutput` call with `extra` parameters beside `InstanceId`: the whole body,
/// having pinned it answered 200 and named the instance the request asked about.
async fn output_body(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![p("InstanceId", instance_id)];
    params.extend_from_slice(extra);
    let (status, body) = api(router, "GetConsoleOutput", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConsoleOutput must succeed, got {status}: {body}"
    );
    assert_eq!(
        member(&body, "instanceId"),
        Some(instance_id.to_string()),
        "case {case:?}: the response must name the instance the service resolved, got {body}"
    );
    body
}

/// The console text one instance's `GetConsoleOutput` reported: the `output` member decoded
/// from base64, having pinned that it IS base64.
async fn console_text(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let body = output_body(router, case, instance_id, &[]).await;
    decoded_output(case, &body)
}

/// The `output` member of `body` as the text it encodes.
fn decoded_output(case: &str, body: &str) -> String {
    let encoded = member(body, "output")
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry output: {body}"));
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: output must be base64 ({error}): {encoded}")
        });
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the console text must be UTF-8: {error}"))
}

/// The value of the console line reporting the instance fact `label`, or `None` when the
/// console carries no such line.
///
/// A guest publishes the facts it discovered about the machine it is on through its init
/// system, as `ci-info: <name>: <value>` — so that is where a client reads one.
fn console_value(text: &str, label: &str) -> Option<String> {
    let marker = format!("ci-info: {label}: ");
    text.lines()
        .find_map(|line| line.split_once(&marker))
        .map(|(_, value)| value.trim().to_string())
}

/// Whether `line` carries the printk timestamp a kernel prefixes a console line with, as
/// `[    6.096338]`.
fn printk_stamped(line: &str) -> bool {
    let Some(stamp) = line.strip_prefix('[').and_then(|rest| rest.split_once(']')) else {
        return false;
    };
    let Some((seconds, micros)) = stamp.0.trim().split_once('.') else {
        return false;
    };
    !seconds.is_empty()
        && seconds.chars().all(|character| character.is_ascii_digit())
        && micros.len() == 6
        && micros.chars().all(|character| character.is_ascii_digit())
}

/// What `DescribeInstances` reports about one instance, as the facts the console is read
/// against: its type, its Availability Zone and its private address.
async fn described_facts(router: &MultiServiceRouter, instance_id: &str) -> [(String, String); 3] {
    let (status, body) = api(
        router,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances for {instance_id} must succeed, got {status}: {body}"
    );
    let read = |name: &str| {
        super::ec2_query::element(&body, name)
            .unwrap_or_else(|| panic!("DescribeInstances must report {name}: {body}"))
    };
    [
        ("instance-type".to_string(), read("instanceType")),
        ("availability-zone".to_string(), read("availabilityZone")),
        ("private-ip-address".to_string(), read("privateIpAddress")),
    ]
}

/// RULE 1: the console text is what the instance's own guest wrote while it came up — a
/// Linux guest's log reporting the stored instance's own facts — and the response carries
/// the instance's id and the instant that output was written.
#[tokio::test]
async fn the_console_output_is_the_stored_instance() {
    // Rows differ in the instance type and in how many instances share one launch — two
    // instances of one launch must not share a console.
    let cases: [(&str, &str, usize); 4] = [
        ("one t3.micro", "t3.micro", 1),
        ("one m5.large", "m5.large", 1),
        ("one c5.xlarge", "c5.xlarge", 1),
        ("three t3.small", "t3.small", 3),
    ];

    for (case, instance_type, count) in cases {
        let (router, image_id) = emulator();
        let ids = launch(&router, &image_id, instance_type, count).await;
        // The guest comes up as the launch finishes starting the instance, so the console it
        // wrote is there once that move has fallen due.
        past_the_transition().await;

        let mut texts: Vec<String> = Vec::new();
        for instance_id in &ids {
            let case = format!("{case}: {instance_id}");
            let body = output_body(&router, &case, instance_id, &[]).await;
            let text = decoded_output(&case, &body);

            assert_eq!(
                console_value(&text, "instance-id").as_deref(),
                Some(instance_id.as_str()),
                "case {case:?}: the console must name its own instance, got {text:?}"
            );
            for (label, expected) in described_facts(&router, instance_id).await {
                assert_eq!(
                    console_value(&text, &label).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected the console to report {label:?} as \
                     {expected:?}, got {text:?}"
                );
            }
            assert!(
                console_value(&text, "launch-time").is_some(),
                "case {case:?}: the console must report the launch time, got {text:?}"
            );
            // A console carries the GUEST's log, so the kernel's own lines are on it, each
            // with the printk timestamp a kernel writes.
            let stamped = text.lines().filter(|line| printk_stamped(line)).count();
            assert!(
                stamped >= 20,
                "case {case:?}: expected the guest's kernel lines on the console, got \
                 {stamped} printk-stamped line(s) of {text:?}"
            );
            assert!(
                text.lines().any(|line| line.contains("cloud-init[")),
                "case {case:?}: expected the init system's own lines, got {text:?}"
            );

            // "The time at which the output was last updated" (`$Timestamp`): the guest
            // wrote its console when it BOOTED, which is the instant that boot reports.
            let timestamp = member(&body, "timestamp")
                .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp in {body}"));
            let boot_time = console_value(&text, "boot-time").unwrap_or_default();
            assert_eq!(
                as_timestamp(&timestamp),
                as_timestamp(&boot_time),
                "case {case:?}: expected the timestamp to be when the guest came up \
                 {boot_time:?}, got {timestamp:?}"
            );
            texts.push(text);
        }

        for (index, text) in texts.iter().enumerate() {
            for (other, against) in texts.iter().enumerate() {
                assert!(
                    index == other || text != against,
                    "case {case:?}: instances {index} and {other} must not share a console"
                );
            }
        }
    }
}

/// RULE 2: a console carries what was WRITTEN to it, so an instance whose guest has not come
/// up yet has no console output at all, and one whose guest has carries that boot's log.
#[tokio::test]
async fn a_console_carries_what_the_guest_wrote() {
    let (router, image_id) = emulator();
    let ids = launch(&router, &image_id, "t3.micro", 1).await;
    let instance_id = &ids[0];

    let body = output_body(&router, "straight after the launch", instance_id, &[]).await;
    assert_eq!(
        member(&body, "output"),
        None,
        "straight after the launch the guest has written nothing, so there must be no \
         output to report, got {body}"
    );

    past_the_transition().await;
    let running = console_text(&router, "once the launch has settled", instance_id).await;
    assert!(
        running.contains("Linux version"),
        "once the launch has settled: expected the boot's own log, got {running:?}"
    );
}

/// RULE 5: reading a console is a READ — two calls answer the same bytes, `Latest` changes
/// nothing, and the instance is where it was.
#[tokio::test]
async fn reading_a_console_changes_nothing() {
    let (router, image_id) = emulator();
    let ids = launch(&router, &image_id, "m5.large", 1).await;
    let instance_id = &ids[0];
    past_the_transition().await;

    let first = console_text(&router, "the first read", instance_id).await;
    let again = console_text(&router, "the second read", instance_id).await;
    assert_eq!(
        first, again,
        "two reads of one console must answer the same text"
    );

    // "When enabled, retrieves the latest console output for the instance. Default: disabled
    // (false)" (`$Latest`): the console this service derives IS the latest, so both readings
    // answer it.
    for latest in ["true", "false"] {
        let case = format!("Latest={latest}");
        let body = output_body(&router, &case, instance_id, &[p("Latest", latest)]).await;
        assert_eq!(
            decoded_output(&case, &body),
            first,
            "case {case:?}: Latest must not change the console this service serves"
        );
    }

    assert_eq!(
        super::ec2_query::described_instance_state(&router, REGION, instance_id).await,
        "running",
        "reading a console must leave the instance where it was"
    );
}

/// RULE 3: every instance id this operation cannot resolve is refused, malformed before
/// unknown.
#[tokio::test]
async fn an_id_the_service_cannot_resolve_is_refused() {
    let (router, image_id) = emulator();
    let known = launch(&router, &image_id, "t3.micro", 1).await.remove(0);

    // The rows differ in the FORM of the id: the two forms EC2 issues (looked up, and not
    // held by the account), then every way a value can fall outside them — the all-zero
    // placeholder, a long form whose leading digit is above zero, another resource's prefix,
    // a known id with a suffix, and something that is not an id at all. Every one of them is
    // the malformed half for a console reader.
    let rows: [(&str, String, &str); 7] = [
        (
            "a well-formed long id nothing was launched under",
            "i-0123456789abcdef0".to_string(),
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed short id nothing was launched under",
            "i-12345678".to_string(),
            "InvalidInstanceID.Malformed",
        ),
        (
            "the null id",
            "i-00000000000000000".to_string(),
            "InvalidInstanceID.Malformed",
        ),
        (
            "a leading digit above zero",
            "i-1234567890abcdef0".to_string(),
            "InvalidInstanceID.Malformed",
        ),
        (
            "another resource's prefix",
            "vol-0123456789abcdef0".to_string(),
            "InvalidInstanceID.Malformed",
        ),
        (
            "a known id with a suffix",
            format!("{known}x"),
            "InvalidInstanceID.Malformed",
        ),
        (
            "not an id at all",
            "not-an-id".to_string(),
            "InvalidInstanceID.Malformed",
        ),
    ];

    for (case, instance_id, expected) in rows {
        let (status, body) = api(&router, "GetConsoleOutput", &[p("InstanceId", &instance_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let message = super::ec2_query::error_message(&body);
        assert!(
            message.contains(&instance_id),
            "case {case:?}: expected the message to name the id, got {message:?}"
        );
    }
}

/// RULE 4: `DryRun` answers the permission probe whatever the id names, and serves no
/// console.
#[tokio::test]
async fn a_dry_run_answers_the_probe_whatever_the_id_names() {
    let (router, image_id) = emulator();
    let known = launch(&router, &image_id, "t3.micro", 1).await.remove(0);

    // The rows differ in what the probe's id names: an instance the account holds, a
    // well-formed id it does not, and a value that is no instance id at all. "Checks whether
    // you have the required permissions … without actually making the request", so all three
    // answer the same thing.
    let rows: [(&str, String); 3] = [
        ("an instance the account holds", known.clone()),
        (
            "a well-formed id the account does not hold",
            "i-0123456789abcdef0".to_string(),
        ),
        ("a value that is no instance id", "not-an-id".to_string()),
    ];

    for (case, instance_id) in rows {
        let (status, body) = api(
            &router,
            "GetConsoleOutput",
            &[p("InstanceId", &instance_id), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            super::ec2_query::error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: expected the wording EC2 answers a permitted probe, got {body}"
        );
        assert!(
            member(&body, "output").is_none(),
            "case {case:?}: a probe must serve no console, got {body}"
        );
    }

    // `DryRun=false` is not a probe: the console the guest wrote is served.
    past_the_transition().await;
    let body = output_body(&router, "DryRun=false", &known, &[p("DryRun", "false")]).await;
    assert!(
        !decoded_output("DryRun=false", &body).is_empty(),
        "DryRun=false must serve the console, got {body}"
    );
}

/// How long a wait for something the CLOCK owes may take before this suite gives up.
/// Generous on purpose: the condition is what is waited for, so a loaded host cannot fail a
/// wait for a move the service owes in a moment.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long to leave between two polls of a condition the clock owes.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Every value the guest published the fact `label` under, in the order the console carries
/// them.
///
/// A guest's init system writes a fact it discovered about its machine as
/// `ci-info: <name>: <value>`, so the name is matched whole and `boot` never reads a
/// `boot-id` line. Spelled out here rather than imported, so a changed rendering fails this
/// suite.
fn console_column(text: &str, label: &str) -> Vec<String> {
    let marker = format!("ci-info: {label}: ");
    text.lines()
        .filter_map(|line| line.split_once(&marker))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

/// One observation of an instance's console: the text served, the instant the response
/// reported the output was last updated at, and that instant as a number.
#[derive(Debug)]
struct Observation {
    text: String,
    reported: String,
    at: f64,
}

/// How many boots the observed console carries.
fn observed_boots(seen: &Observation) -> usize {
    console_column(&seen.text, "boot-id").len()
}

/// Read one instance's console, pinning the invariant that ties the two members of the answer
/// together: the instant reported as "the time at which the output was last updated" IS the
/// latest instant the output served beside it shows it was written at — the instance's
/// `launch-time`, and the `boot-time` of each boot its guest has had.
///
/// An instance whose guest has not come up yet has written NOTHING to its console, so there
/// is no output to serve and no instant on it: the answer then carries only the instant the
/// response reports, and that is what the first read of a launch observes.
async fn observe(router: &MultiServiceRouter, case: &str, instance_id: &str) -> Observation {
    let body = output_body(router, case, instance_id, &[]).await;
    let text = match member(&body, "output") {
        Some(_) => decoded_output(case, &body),
        None => String::new(),
    };
    let reported = member(&body, "timestamp")
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a timestamp: {body}"));
    let at = as_timestamp(&reported).unwrap_or_else(|| {
        panic!("case {case:?}: expected an instant as the timestamp, got {reported:?}")
    });

    let written: Vec<(String, f64)> = console_column(&text, "launch-time")
        .into_iter()
        .chain(console_column(&text, "boot-time"))
        .map(|line| {
            let secs = as_timestamp(&line).unwrap_or_else(|| {
                panic!("case {case:?}: every instant the console names must parse, got {line:?}")
            });
            (line, secs)
        })
        .collect();
    let latest = written
        .iter()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(line, _)| line.clone());
    match latest {
        Some(latest) => assert_eq!(
            reported.as_str(),
            latest.as_str(),
            "case {case:?}: expected the latest instant the output itself shows, {latest:?}, \
             got {reported:?} for console {text:?}"
        ),
        // Nothing has been written to this console, so it shows no instant to compare the
        // reported one against: what must hold is that there is no output at all.
        None => assert!(
            text.is_empty(),
            "case {case:?}: a console that names no instant must be one nothing has written, \
             got {text:?}"
        ),
    }
    Observation { text, reported, at }
}

/// Read the console until the guest has had `want` boots, answering that observation.
///
/// A boot is owed to the CLOCK, so the CONDITION is waited for rather than a span slept
/// through — and every poll on the way checks the invariant [`observe`] pins, which is what
/// covers the window after a reboot has been queued and before its boot has come up.
async fn console_once_booted(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    want: usize,
) -> Observation {
    let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
    loop {
        let seen = observe(router, case, instance_id).await;
        let boots = observed_boots(&seen);
        if boots >= want {
            assert_eq!(
                boots, want,
                "case {case:?}: expected {want} boot(s) on the console, got {boots}: {:?}",
                seen.text
            );
            return seen;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected {want} boot(s) on the console, got {boots}: {:?}",
            seen.text
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the instance reports the state `want`, which the clock owes it.
async fn once_in_state(router: &MultiServiceRouter, case: &str, instance_id: &str, want: &str) {
    let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
    loop {
        let state = super::ec2_query::described_instance_state(router, REGION, instance_id).await;
        if state == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the instance {want:?}, got {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What is asked of an instance between two reads of its console: each path its guest can
/// start again by, and the two events that ask it for no restart at all.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Nothing at all: the console is read again as it stands.
    Nothing,
    /// `RebootInstances`, which "only queues a request to reboot the specified instances" —
    /// so the guest comes up a moment later, not during the call.
    Reboot,
    /// A stop, and then a start once the instance has stopped: the guest comes up again.
    StopStart,
    /// `SendDiagnosticInterrupt`, which crashes the guest's operating system.
    Interrupt,
}

/// One call that must succeed, as the steps below ask for one.
async fn succeeds(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status}: {body}"
    );
}

/// Carry `step` out against `instance_id`.
async fn apply(router: &MultiServiceRouter, case: &str, step: Step, instance_id: &str) {
    match step {
        Step::Nothing => {}
        Step::Reboot => {
            succeeds(
                router,
                case,
                "RebootInstances",
                &[p("InstanceId.1", instance_id)],
            )
            .await;
        }
        Step::StopStart => {
            succeeds(
                router,
                case,
                "StopInstances",
                &[p("InstanceId.1", instance_id)],
            )
            .await;
            once_in_state(router, case, instance_id, "stopped").await;
            succeeds(
                router,
                case,
                "StartInstances",
                &[p("InstanceId.1", instance_id)],
            )
            .await;
            once_in_state(router, case, instance_id, "running").await;
        }
        Step::Interrupt => {
            succeeds(
                router,
                case,
                "SendDiagnosticInterrupt",
                &[p("InstanceId", instance_id)],
            )
            .await;
        }
    }
}

/// RULE 6: the reported instant is the latest one the served output shows it was written at,
/// so it advances exactly when the guest writes the console and never goes backwards.
#[tokio::test]
async fn the_timestamp_is_when_the_console_was_last_written() {
    let (router, image_id) = emulator();
    let instance_id = launch(&router, &image_id, "t3.micro", 1).await.remove(0);

    // The steps differ along what the rule generalises over: WHETHER the guest writes the
    // console between two reads, and WHAT asks it to — nothing at all, a reboot the service
    // only queues, a stop/start cycle, and an interrupt that asks for no boot of its own.
    // `boots` is how many boots the guest must then have had, left open for a step the
    // service owes no boot for.
    let steps: [(&str, Step, Option<usize>); 4] = [
        // The launch's own boot: owed to the clock, so it lands after the first read.
        ("nothing asked the guest to restart", Step::Nothing, Some(1)),
        ("a queued reboot", Step::Reboot, Some(2)),
        ("a stop and a start", Step::StopStart, Some(3)),
        ("a diagnostic interrupt", Step::Interrupt, None),
    ];

    let mut previous = observe(&router, "straight after the launch", &instance_id).await;
    for (case, step, boots) in steps {
        apply(&router, case, step, &instance_id).await;
        let seen = match boots {
            Some(want) => console_once_booted(&router, case, &instance_id, want).await,
            None => observe(&router, case, &instance_id).await,
        };

        assert!(
            seen.at >= previous.at,
            "case {case:?}: the reported instant must never go backwards, got {:?} after {:?}",
            seen.reported,
            previous.reported
        );
        if observed_boots(&seen) > observed_boots(&previous) {
            // The guest wrote the console, so the instant it reports is that write's.
            assert!(
                seen.at > previous.at,
                "case {case:?}: a boot must advance the reported instant, got {:?} after {:?}",
                seen.reported,
                previous.reported
            );
            let boot_times = console_column(&seen.text, "boot-time");
            assert_eq!(
                Some(seen.reported.as_str()),
                boot_times.last().map(String::as_str),
                "case {case:?}: expected the newest boot's own instant, got {:?} for console \
                 {:?}",
                seen.reported,
                seen.text
            );
        } else {
            // Nothing wrote the console, so it reads as it did and reports no update of it.
            assert_eq!(
                seen.text, previous.text,
                "case {case:?}: an unwritten console must answer the same bytes"
            );
            assert_eq!(
                seen.reported, previous.reported,
                "case {case:?}: an unwritten console must report no update of itself"
            );
        }
        previous = seen;
    }
}
