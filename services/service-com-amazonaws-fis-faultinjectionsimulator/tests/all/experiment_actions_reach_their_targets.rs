// SPDX-License-Identifier: CC0-1.0

//! EXPERIMENT ACTIONS REACH THEIR TARGETS: what a started experiment DOES to the resources
//! its targets resolved to, end to end over HTTP, across the two services it spans.
//!
//! RULE: a started experiment CARRIES OUT the actions its plan keeps — each one against the
//! resources ITS OWN targets resolved to, at the point of the run it belongs to — so the
//! effect is observable in the service that owns those resources, and the action reports its
//! own progress to a terminal `completed` with the `startTime` and `endTime` of the window
//! it was carried out in. An action the plan leaves out is carried out on nothing at all.
//!
//! For `aws:ec2:reboot-instances` ("Reboot the specified EC2 instances") that effect is a
//! reboot of the instance: its guest comes up again, so the console it writes gains a new
//! boot with a new boot identity while the instance keeps its id, its launch time and its
//! `running` state — a reboot restarts the operating system, not the instance.
//!
//! This suite lives beside the service rather than in `core/router` because EC2 is mounted
//! on the SAME engine FIS runs on, exactly as the assembled emulator mounts it: the reboot
//! is read back through the real `ec2:GetConsoleOutput` and `ec2:DescribeInstances`, so what
//! is asserted is what EC2 persisted and not a copy.
//!
//! The rows differ along what the rule generalises over: HOW MANY instances the target
//! identifies (one, two, three, four), the `selectionMode` that scopes them (`ALL`,
//! `COUNT(n)`, `PERCENT(n)`), WHERE in the run the action sits (on its own, and after another action it
//! waits for), whether the experiment's options carry the action out at all (`skip-all`),
//! and whether the resolved ARN names a resource this emulator actually holds. Every row
//! also launches a BYSTANDER instance no target names, which must be left alone — so no row
//! can pass by rebooting everything.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, get_experiment, poll_until_final, send, start_experiment, token, Answer,
    FALLBACK_REGION,
};

/// A role ARN the emulator's IAM store holds no role for: with no stored role there is
/// nothing to show about its trust policy or its permissions, so the run goes ahead.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-actions";

/// The account id every ARN this emulator mints carries, which the instance ARNs below name
/// too.
const ACCOUNT_ID: &str = "000000000000";

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// How long a row waits for its run to reach a terminal status: the emulator's whole
/// compressed run window plus the transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// How long a row waits for EC2 to bring a rebooted guest back up — the reboot is QUEUED, so
/// the boot itself falls due a moment later — plus slack for a loaded machine.
const BOOT_BUDGET: Duration = Duration::from_secs(10);

/// How many experiments may be active at once (the account's published quota), which is what
/// the rows are started in groups of.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// The marker a guest's init system writes the facts it discovered about its machine under,
/// as `ec2`'s own console renders them — `ci-info: <name>: <value>`. Spelled out here rather
/// than imported, so a changed rendering fails this suite.
const CI_INFO_MARKER: &str = "ci-info: ";

/// One emulator carrying FIS and EC2 over ONE shared engine, exactly as the production
/// router builds them. The engine is handed back so the suite can seed the AMI a launch
/// needs, the way a register operation would have stored it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One EC2 call (ec2Query: a form POST answered with XML), answered with the HTTP status and
/// the body.
async fn ec2_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{FALLBACK_REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{FALLBACK_REGION}/ec2/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(::bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(&response.into_body()).to_string(),
    )
}

/// A `(key, value)` ec2Query form field.
fn field(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The text of every `<name>…</name>` element of `xml`, in document order.
fn xml_elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// Launch `count` instances from `image_id`, answering their ids in launch order.
async fn launch(router: &MultiServiceRouter, case: &str, image_id: &str, count: usize) -> Vec<String> {
    let (status, body) = ec2_call(
        router,
        "RunInstances",
        &[
            field("ImageId", image_id),
            field("InstanceType", "t3.micro"),
            field("MinCount", count.to_string()),
            field("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status}: {body}"
    );
    let ids = xml_elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "case {case:?}: RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// The ARN of the EC2 instance `instance_id` names, as a target's `resourceArns` carries it.
fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:ec2:{FALLBACK_REGION}:{ACCOUNT_ID}:instance/{instance_id}")
}

/// One console line reporting a machine fact, as the guest's init system writes one:
/// `(name, value)`.
fn console_line(line: &str) -> Option<(String, String)> {
    let (_, fact) = line.split_once(CI_INFO_MARKER)?;
    let (name, value) = fact.split_once(':')?;
    Some((name.trim().to_string(), value.trim().to_string()))
}

/// One instance's console as text, through the real `GetConsoleOutput` — empty for an
/// instance whose guest has written nothing to it yet, which carries no `output` member at
/// all.
async fn console_text(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = ec2_call(
        router,
        "GetConsoleOutput",
        &[field("InstanceId", instance_id), field("Latest", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConsoleOutput for {instance_id} must succeed, got {status}: {body}"
    );
    let Some(encoded) = xml_elements(&body, "output").pop() else {
        return String::new();
    };
    let bytes = ::base64::Engine::decode(&::base64::engine::general_purpose::STANDARD, &encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: output must be base64 ({error})"));
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the console text must be UTF-8: {error}"))
}

/// The boot identities one instance's console reports, in the order its guest had them.
async fn boot_ids(router: &MultiServiceRouter, case: &str, instance_id: &str) -> Vec<String> {
    boot_ids_in(&console_text(router, case, instance_id).await)
}

/// See [`boot_ids`]: the identities a console TEXT already read reports.
fn boot_ids_in(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(console_line)
        .filter(|(label, _)| label == "boot-id")
        .map(|(_, value)| value)
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

/// Wait until `instance_id`'s console reports at least `want` boots, and answer its text as
/// it was last read.
async fn poll_boots(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    want: usize,
) -> String {
    const INTERVAL: Duration = Duration::from_millis(50);
    let deadline = std::time::Instant::now() + BOOT_BUDGET;
    loop {
        let text = console_text(router, case, instance_id).await;
        if boot_ids_in(&text).len() >= want || std::time::Instant::now() >= deadline {
            return text;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

/// What `DescribeInstances` reports about one instance: its state name and its launch time.
async fn described(router: &MultiServiceRouter, case: &str, instance_id: &str) -> (String, String) {
    let (status, body) = ec2_call(
        router,
        "DescribeInstances",
        &[field("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances for {instance_id} must succeed, got {status}: {body}"
    );
    let state = xml_elements(&body, "name")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no instance state in {body}"));
    let launch_time = xml_elements(&body, "launchTime")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no launchTime in {body}"));
    (state, launch_time)
}

/// The resolved targets `ListExperimentResolvedTargets` reports for `experiment_id`.
async fn resolved_targets(router: &MultiServiceRouter, experiment_id: &str) -> Vec<Value> {
    let answer: Answer = send(
        router,
        "GET",
        &format!("/experiments/{experiment_id}/resolvedTargets"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "ListExperimentResolvedTargets must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .member("resolvedTargets")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row: how many instances the target identifies, how its `selectionMode` scopes them,
/// what the template's actions are, and what the run must then have done.
struct Row {
    case: &'static str,
    /// How many instances the row launches and names in its target, in launch order.
    instances: usize,
    /// The target's `selectionMode`, which `CreateExperimentTemplate` requires of every
    /// target.
    selection_mode: &'static str,
    /// The start's `experimentOptions.actionsMode`, absent for a row that relies on the
    /// documented default (`run-all`).
    actions_mode: Option<&'static str>,
    /// Whether the reboot action waits for another action of the template.
    after_a_wait: bool,
    /// Whether the target also names an instance ARN this emulator holds nothing for.
    names_an_absent_instance: bool,
    /// How many of the launched instances — counted from the first — the run must have
    /// rebooted.
    rebooted: usize,
    /// The status the reboot action must report once the run has ended, or `None` when the
    /// run reports no state for it.
    action_status: &'static str,
    /// How many resolved targets the experiment must report.
    resolved: usize,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-instance-under-all",
            instances: 1,
            selection_mode: "ALL",
            actions_mode: None,
            after_a_wait: false,
            names_an_absent_instance: false,
            rebooted: 1,
            action_status: "completed",
            resolved: 1,
        },
        Row {
            case: "two-instances-under-all",
            instances: 2,
            selection_mode: "ALL",
            actions_mode: None,
            after_a_wait: false,
            names_an_absent_instance: false,
            rebooted: 2,
            action_status: "completed",
            resolved: 2,
        },
        Row {
            // `COUNT(1)` keeps one of the three the target identified, so exactly one guest
            // restarts and the other two are left alone.
            case: "three-instances-scoped-by-count",
            instances: 3,
            selection_mode: "COUNT(1)",
            actions_mode: None,
            after_a_wait: false,
            names_an_absent_instance: false,
            rebooted: 1,
            action_status: "completed",
            resolved: 1,
        },
        Row {
            // `PERCENT(50)` of four identified resources keeps two.
            case: "four-instances-scoped-by-percent",
            instances: 4,
            selection_mode: "PERCENT(50)",
            actions_mode: None,
            after_a_wait: false,
            names_an_absent_instance: false,
            rebooted: 2,
            action_status: "completed",
            resolved: 2,
        },
        Row {
            // The action sits at the END of the run, after the action it waits for: it is
            // still carried out, by the time the run has finished.
            case: "the-reboot-waits-for-another-action",
            instances: 1,
            selection_mode: "ALL",
            actions_mode: None,
            after_a_wait: true,
            names_an_absent_instance: false,
            rebooted: 1,
            action_status: "completed",
            resolved: 1,
        },
        Row {
            // The documented use of `skip-all`: the experiment resolves its targets and
            // carries out NONE of its actions, so no guest restarts.
            case: "skip-all-carries-the-reboot-out-on-nothing",
            instances: 1,
            selection_mode: "ALL",
            actions_mode: Some("skip-all"),
            after_a_wait: false,
            names_an_absent_instance: false,
            rebooted: 0,
            action_status: "skipped",
            resolved: 1,
        },
        Row {
            // A well-formed instance ARN this emulator holds no instance for: it resolves
            // (the target NAMED it), the action is carried out, and nothing is invented for
            // the resource that is not there — while the instance that IS there restarts.
            case: "an-arn-naming-no-instance-of-this-emulator",
            instances: 1,
            selection_mode: "ALL",
            actions_mode: None,
            after_a_wait: false,
            names_an_absent_instance: true,
            rebooted: 1,
            action_status: "completed",
            resolved: 2,
        },
    ]
}

/// The create body for `row`, targeting `instance_ids` by ARN.
fn template_body(row: &Row, instance_ids: &[String]) -> Value {
    let mut arns: Vec<String> = instance_ids.iter().map(|id| instance_arn(id)).collect();
    if row.names_an_absent_instance {
        arns.push(instance_arn("i-00000000000000fff"));
    }
    let target = json!({
        "resourceType": "aws:ec2:instance",
        "resourceArns": arns,
        "selectionMode": row.selection_mode
    });
    let mut reboot = json!({
        "actionId": "aws:ec2:reboot-instances",
        "targets": { "Instances": "hosts" }
    });
    let mut actions = json!({});
    if row.after_a_wait {
        actions["settle"] = json!({
            "actionId": "aws:fis:wait",
            "parameters": { "duration": "PT1M" }
        });
        reboot["startAfter"] = json!(["settle"]);
    }
    actions["reboot"] = reboot;
    json!({
        "clientToken": token(&format!("actions-template-{}", row.case)),
        "description": format!("experiment actions reach their targets: {}", row.case),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": { "hosts": target },
        "actions": actions
    })
}

/// The start body for `row`'s template.
fn start_body(row: &Row, template_id: &str) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("actions-start-{}", row.case)),
        "experimentTemplateId": template_id
    });
    if let Some(mode) = row.actions_mode {
        body["experimentOptions"] = json!({ "actionsMode": mode });
    }
    body
}

/// What one row launched and started: the instances its target names, the bystander it must
/// leave alone, the boots each of them had before the run, and the experiment's id.
struct Started {
    instance_ids: Vec<String>,
    bystander: String,
    boots_before: Vec<usize>,
    bystander_boots_before: usize,
    console_before: Vec<String>,
    launch_times: Vec<String>,
    experiment_id: String,
}

#[tokio::test]
async fn an_experiment_carries_out_its_actions_against_the_resources_its_targets_resolved_to() {
    let (router, engine) = emulator();
    let image_id = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        guard.insert_image(ec2::state::ImageSpec {
            name: "fis-action-targets".to_string(),
            ..ec2::state::ImageSpec::default()
        })
    };
    let rows = rows();

    // The rows are started in groups no larger than the account's active-experiment quota,
    // so the runs inside a group overlap instead of being waited out one after another.
    for group in rows.chunks(ACTIVE_EXPERIMENTS_QUOTA) {
        let mut started: Vec<(&Row, Started)> = Vec::new();
        for row in group {
            let case = row.case;
            let instance_ids = launch(&router, case, &image_id, row.instances).await;
            let bystander = launch(&router, case, &image_id, 1)
                .await
                .pop()
                .expect("one bystander instance");

            // Every instance's guest comes up before the experiment starts, so what the run
            // adds is a SECOND boot and not the first one.
            let mut boots_before = Vec::new();
            let mut console_before = Vec::new();
            let mut launch_times = Vec::new();
            for instance_id in &instance_ids {
                let text = poll_boots(&router, case, instance_id, 1).await;
                let ids = boot_ids_in(&text);
                assert_eq!(
                    ids.len(),
                    1,
                    "case {case:?}: the launched instance {instance_id} must report its first \
                     boot before the run, got {ids:?}"
                );
                assert!(
                    is_uuid(&ids[0]),
                    "case {case:?}: a boot identity must be a UUID, got {:?}",
                    ids[0]
                );
                boots_before.push(ids.len());
                console_before.push(text);
                launch_times.push(described(&router, case, instance_id).await.1);
            }
            let bystander_boots_before = boot_ids_in(&poll_boots(&router, case, &bystander, 1).await)
                .len();

            let created = create_template(&router, template_body(row, &instance_ids)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the template to be created, got {} with {:?}",
                created.status, created.body
            );
            let template_id = created.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
                .to_string();

            let start = start_experiment(&router, start_body(row, &template_id)).await;
            assert_eq!(
                start.status, 200,
                "case {case:?}: expected the start to be accepted, got {} with {:?}",
                start.status, start.body
            );
            let experiment_id = start.member("experiment")["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {:?}", start.body))
                .to_string();

            started.push((
                row,
                Started {
                    instance_ids,
                    bystander,
                    boots_before,
                    bystander_boots_before,
                    console_before,
                    launch_times,
                    experiment_id,
                },
            ));
        }

        for (row, run) in &started {
            let case = row.case;
            let experiment = poll_until_final(&router, &run.experiment_id, TERMINAL_BUDGET).await;
            assert_eq!(
                experiment["state"]["status"].as_str().unwrap_or_default(),
                "completed",
                "case {case:?}: expected the run to complete, got {experiment:?}"
            );

            // The action's OWN progress, which is what a client branches on to learn that the
            // fault was injected.
            let state = &experiment["actions"]["reboot"]["state"];
            assert_eq!(
                state["status"].as_str().unwrap_or_default(),
                row.action_status,
                "case {case:?}: expected the reboot action to report {:?}, got {state:?}",
                row.action_status
            );
            assert!(
                !state["reason"].as_str().unwrap_or_default().contains("skip")
                    || row.action_status == "skipped",
                "case {case:?}: an action the run carried out must not say it was skipped, \
                 got {state:?}"
            );
            if row.action_status == "completed" {
                for member in ["startTime", "endTime"] {
                    assert!(
                        !experiment["actions"]["reboot"][member].is_null(),
                        "case {case:?}: an action the run carried out reports its {member}, \
                         got {:?}",
                        experiment["actions"]["reboot"]
                    );
                }
            }

            // What the run resolved is what a client is told it will act on.
            let resolved = resolved_targets(&router, &run.experiment_id).await;
            assert_eq!(
                resolved.len(),
                row.resolved,
                "case {case:?}: expected {} resolved target(s), got {resolved:?}",
                row.resolved
            );

            // Long enough that a reboot the run carried out has landed, so an instance this
            // row expects to be LEFT ALONE is read after the window one would have arrived
            // in rather than before it.
            tokio::time::sleep(Duration::from_millis(900)).await;

            for (index, instance_id) in run.instance_ids.iter().enumerate() {
                let expected_boots = run.boots_before[index] + usize::from(index < row.rebooted);
                let text = poll_boots(&router, case, instance_id, expected_boots).await;
                let ids = boot_ids_in(&text);
                assert_eq!(
                    ids.len(),
                    expected_boots,
                    "case {case:?}: expected instance {index} ({instance_id}) to report \
                     {expected_boots} boot(s) after the run, got {ids:?}"
                );
                // A console is APPENDED to: what the earlier boots wrote is still there,
                // byte for byte, with the new boot's lines after it.
                assert!(
                    text.starts_with(&run.console_before[index]),
                    "case {case:?}: instance {instance_id} must keep the console its earlier \
                     boots wrote, got {text:?}"
                );
                if index < row.rebooted {
                    assert!(
                        is_uuid(&ids[1]) && ids[1] != ids[0],
                        "case {case:?}: the reboot must give instance {instance_id} a NEW \
                         boot identity, got {ids:?}"
                    );
                }
                // A reboot restarts the operating system, not the instance.
                let (state_name, launch_time) = described(&router, case, instance_id).await;
                assert_eq!(
                    state_name, "running",
                    "case {case:?}: instance {instance_id} must still be running after the \
                     reboot, got {state_name:?}"
                );
                assert_eq!(
                    launch_time, run.launch_times[index],
                    "case {case:?}: a reboot must not move instance {instance_id}'s launch \
                     time"
                );
            }

            // No target names the bystander, so nothing restarted it.
            let bystander_boots = boot_ids(&router, case, &run.bystander).await;
            assert_eq!(
                bystander_boots.len(),
                run.bystander_boots_before,
                "case {case:?}: the instance no target names must be left alone, got \
                 {bystander_boots:?}"
            );
        }
    }
}

/// RULE: an action the run has NOT reached yet is carried out on nothing — a stop that
/// arrives first leaves the instance on the boot it was on.
#[tokio::test]
async fn a_stop_before_the_action_leaves_the_target_alone() {
    let (router, engine) = emulator();
    let image_id = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        guard.insert_image(ec2::state::ImageSpec {
            name: "fis-stopped-action".to_string(),
            ..ec2::state::ImageSpec::default()
        })
    };
    let case = "a-stop-before-the-action";
    let instance_id = launch(&router, case, &image_id, 1)
        .await
        .pop()
        .expect("one instance");
    let before = boot_ids_in(&poll_boots(&router, case, &instance_id, 1).await);

    // The reboot waits for a long action, so the run has not reached it when the stop
    // arrives.
    let created = create_template(
        &router,
        json!({
            "clientToken": token("actions-template-stopped"),
            "description": "a reboot the stop arrives before",
            "roleArn": ROLE_ARN,
            "stopConditions": [{ "source": "none" }],
            "targets": {
                "hosts": {
                    "resourceType": "aws:ec2:instance",
                    "resourceArns": [instance_arn(&instance_id)],
                    "selectionMode": "ALL"
                }
            },
            "actions": {
                "settle": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": "PT12H" }
                },
                "reboot": {
                    "actionId": "aws:ec2:reboot-instances",
                    "targets": { "Instances": "hosts" },
                    "startAfter": ["settle"]
                }
            }
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
        .to_string();

    let start = start_experiment(
        &router,
        json!({
            "clientToken": token("actions-start-stopped"),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        start.status, 200,
        "case {case:?}: expected the start to be accepted, got {} with {:?}",
        start.status, start.body
    );
    let experiment_id = start.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {:?}", start.body))
        .to_string();

    let stopped = crate::experiment_lifecycle::stop_experiment(&router, &experiment_id).await;
    assert_eq!(
        stopped.status, 200,
        "case {case:?}: expected the stop to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );
    let experiment = poll_until_final(&router, &experiment_id, TERMINAL_BUDGET).await;
    assert_eq!(
        experiment["state"]["status"].as_str().unwrap_or_default(),
        "stopped",
        "case {case:?}: expected the stopped run to report it, got {experiment:?}"
    );
    assert_eq!(
        experiment["actions"]["reboot"]["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "cancelled",
        "case {case:?}: an action the stop reached before the run did reports it was \
         cancelled, got {:?}",
        experiment["actions"]["reboot"]
    );

    // Long enough that a reboot the run had carried out would have landed.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let after = boot_ids(&router, case, &instance_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: an action the run never reached must leave the instance on the boot \
         it was on"
    );

    // And the read itself is a READ: the experiment reports the same thing again.
    let again = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        again.member("experiment")["actions"]["reboot"]["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "cancelled",
        "case {case:?}: two reads of one experiment report the same action state, got {:?}",
        again.body
    );
}
