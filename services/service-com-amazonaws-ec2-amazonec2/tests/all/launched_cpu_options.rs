// SPDX-License-Identifier: CC0-1.0

//! The CPU topology an instance runs as, over the wire the service speaks (`ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness).
//!
//! The rule under test: **an instance reports the CPU options its LAUNCH asked for** — "the
//! CPU options for the instance" (`RunInstancesRequest$CpuOptions` of `CpuOptionsRequest`,
//! whose `ThreadsPerCore` is "the number of threads per CPU core. To disable multithreading
//! for the instance, specify a value of `1`. Otherwise, specify the default value of `2`") —
//! so `DescribeInstances` answers `Instance$CpuOptions` with the core count and the threads
//! per core the request named, and with the instance type's own counts only for a member the
//! request left out. A count the type cannot be configured with refuses the launch instead
//! of being replaced.
//!
//! The counts a type offers are not written here: they are read off
//! `DescribeInstanceTypes`'s own `VCpuInfo$ValidCores`, `$ValidThreadsPerCore`,
//! `$DefaultCores` and `$DefaultThreadsPerCore` for that type, so these cases and the
//! catalog cannot disagree — and a type whose published counts change moves the expectation
//! with it.
//!
//! The rows differ along what the rule generalises over: the instance type (and with it how
//! many cores and threads per core it offers), WHICH of the offered counts the launch asks
//! for (the smallest, the largest, and multithreading turned off), which MEMBERS the launch
//! names (both, one, neither, and the two option words beside them), and whether the launch
//! names its topology itself or inherits it from a launch template.

use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_type, element, reported_instance_type_list};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// The instance types the rows span: a two-vCPU type that offers one core, a larger one of
/// the same family, and a compute type of another family — so the expectations below are
/// taken from more than one published CPU layout.
const TYPES: [&str; 3] = ["m5.large", "m5.4xlarge", "c5.2xlarge"];

/// Look again until `instance_id` reports the state `wanted`, or a generous deadline passes.
///
/// The move to `running` is work the CLOCK owes the launch, so the condition is looked at
/// AGAIN in a loop rather than waited out: nothing here depends on how fast the host runs,
/// and the deadline is a multiple of the service's own transition span.
async fn poll_until_state(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    wanted: &str,
) {
    let deadline = std::time::Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = super::ec2_query::described_instance_state(router, REGION, instance_id).await;
        if state == wanted {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {instance_id} must reach {wanted:?}, last read {state:?}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
}

/// One call that must answer 200, and the body it answered.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The counts `DescribeInstanceTypes` publishes for `instance_type`: its valid core counts,
/// its valid threads-per-core counts, and the defaults it comes with.
///
/// Read through the operation that OWNS the catalog, so no row writes a count of its own.
async fn published_counts(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
) -> (Vec<i32>, Vec<i32>, i32, i32) {
    let body = described_instance_type(case, router, REGION, instance_type).await;
    let numbers = |path: &[&str]| -> Vec<i32> {
        reported_instance_type_list(case, &body, path)
            .iter()
            .filter_map(|value| value.parse::<i32>().ok())
            .collect()
    };
    let valid_cores = numbers(&["vCpuInfo", "validCores"]);
    let valid_threads = numbers(&["vCpuInfo", "validThreadsPerCore"]);
    let default_cores = element(&body, "defaultCores")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: {instance_type} publishes no defaultCores"));
    let default_threads = element(&body, "defaultThreadsPerCore")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: {instance_type} publishes no defaultThreadsPerCore")
        });
    assert!(
        !valid_cores.is_empty() && !valid_threads.is_empty(),
        "case {case:?}: {instance_type} must publish the counts it can be configured with, \
         got cores {valid_cores:?} and threads {valid_threads:?}"
    );
    (valid_cores, valid_threads, default_cores, default_threads)
}

/// Launch one instance with `params` beside the image and the type, and answer the id it was
/// given.
async fn launch(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
    params: &[Param],
) -> String {
    let mut fields = vec![
        param("ImageId", AMI),
        param("InstanceType", instance_type),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    fields.extend_from_slice(params);
    let body = ok(router, case, "RunInstances", &fields).await;
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instanceId: {body}"))
}

/// The `cpuOptions` members `DescribeInstances` reports for `instance_id`: the core count,
/// the threads per core, and the two option words — each `None` when the instance reports no
/// such member.
async fn reported_cpu_options(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let body = ok(
        router,
        case,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    let parsed = document(case, &body);
    let options = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "cpuOptions")
        .unwrap_or_else(|| panic!("case {case:?}: the instance reports no cpuOptions: {body}"));
    (
        child_text(options, "coreCount"),
        child_text(options, "threadsPerCore"),
        child_text(options, "amdSevSnp"),
        child_text(options, "nestedVirtualization"),
    )
}

/// Whether `DescribeInstances` reports a `cpuOptions` member for `instance_id` at all.
async fn reports_cpu_options(router: &MultiServiceRouter, case: &str, instance_id: &str) -> bool {
    let body = ok(
        router,
        case,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    let parsed = document(case, &body);
    parsed
        .descendants()
        .any(|node| node.is_element() && node.tag_name().name() == "cpuOptions")
}

/// THE RULE: every (type, core count, threads per core) the catalog offers reads back
/// exactly as the launch asked for it.
#[tokio::test]
async fn an_instance_reports_the_cpu_topology_its_launch_asked_for() {
    let (router, _engine) = super::ec2_query::emulator();

    for instance_type in TYPES {
        let case = format!("the counts {instance_type} offers");
        let (valid_cores, valid_threads, _, _) =
            published_counts(&router, &case, instance_type).await;
        // The smallest and the largest core count the type offers, so a row exists at each
        // published bound rather than only at one of them.
        let cores = [
            valid_cores.first().copied().unwrap_or_default(),
            valid_cores.last().copied().unwrap_or_default(),
        ];
        for core_count in cores {
            for threads_per_core in &valid_threads {
                let case = format!(
                    "{instance_type} with {core_count} core(s) at {threads_per_core} \
                     thread(s) per core"
                );
                let instance_id = launch(
                    &router,
                    &case,
                    instance_type,
                    &[
                        param("CpuOptions.CoreCount", &core_count.to_string()),
                        param("CpuOptions.ThreadsPerCore", &threads_per_core.to_string()),
                    ],
                )
                .await;
                let (reported_cores, reported_threads, amd_sev_snp, nested) =
                    reported_cpu_options(&router, &case, &instance_id).await;
                assert_eq!(
                    reported_cores.as_deref(),
                    Some(core_count.to_string().as_str()),
                    "case {case:?}: expected cpuOptions.coreCount {core_count}, got \
                     {reported_cores:?}"
                );
                assert_eq!(
                    reported_threads.as_deref(),
                    Some(threads_per_core.to_string().as_str()),
                    "case {case:?}: expected cpuOptions.threadsPerCore {threads_per_core}, \
                     got {reported_threads:?}"
                );
                assert_eq!(
                    (amd_sev_snp.as_deref(), nested.as_deref()),
                    (None, None),
                    "case {case:?}: a launch that asked for neither option word reports \
                     neither, got {amd_sev_snp:?} and {nested:?}"
                );
            }
        }
    }
}

/// THE SAME RULE across the launch's lifecycle and the member that names NO topology: a
/// launch that asks for no CPU options at all runs on the counts its instance type comes
/// with, and so does one that names only an option WORD of the same member — and the
/// topology survives the move the clock owes the instance.
#[tokio::test]
async fn a_launch_that_asks_for_no_topology_runs_on_the_instance_types_own() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[1];
    let case = format!("the published defaults of {instance_type}");
    let (_, _, default_cores, default_threads) =
        published_counts(&router, &case, instance_type).await;

    // (case, the CpuOptions fields the launch carries)
    let rows: Vec<(String, Vec<Param>)> = vec![
        (
            "a launch naming no CpuOptions at all".to_string(),
            Vec::new(),
        ),
        (
            // The member is named, but neither COUNT is: nothing about the topology was
            // asked for, so the type's own counts stand.
            "a launch naming only an option word of the member".to_string(),
            vec![param("CpuOptions.AmdSevSnp", "disabled")],
        ),
    ];

    for (case, fields) in rows {
        let instance_id = launch(&router, &case, instance_type, &fields).await;
        // Read once the instance has reached `running` too: the topology is a fact of the
        // instance, so the move the clock owes the launch does not rewrite it.
        poll_until_state(&router, &case, &instance_id, "running").await;
        let (reported_cores, reported_threads, _, _) =
            reported_cpu_options(&router, &case, &instance_id).await;
        assert_eq!(
            (reported_cores.as_deref(), reported_threads.as_deref()),
            (
                Some(default_cores.to_string().as_str()),
                Some(default_threads.to_string().as_str())
            ),
            "case {case:?}: expected the type's own {default_cores}/{default_threads}, got \
             {reported_cores:?}/{reported_threads:?}"
        );
    }
}

/// THE OTHER HALF of the member's own rule: "the CPU options for the instance. Both the core
/// count and threads per core must be specified in the request" (`CpuOptionsRequest`) — so a
/// launch naming ONE of the two counts without the other is refused `InvalidParameterValue`,
/// the code EC2's parameter layer answers for a member it will not take, and nothing is
/// launched.
///
/// The rows span which of the two the request names, and which value it names it with.
#[tokio::test]
async fn a_launch_naming_one_count_without_the_other_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[1];
    let case = format!("the counts {instance_type} offers");
    let (valid_cores, valid_threads, _, _) = published_counts(&router, &case, instance_type).await;

    let rows: Vec<(String, Vec<Param>)> = valid_cores
        .iter()
        .map(|cores| {
            (
                format!("a launch naming only the core count {cores}"),
                vec![param("CpuOptions.CoreCount", &cores.to_string())],
            )
        })
        .chain(valid_threads.iter().map(|threads| {
            (
                format!("a launch naming only the {threads} thread(s) per core"),
                vec![param("CpuOptions.ThreadsPerCore", &threads.to_string())],
            )
        }))
        .collect();

    for (case, fields) in rows {
        let mut params = vec![
            param("ImageId", AMI),
            param("InstanceType", instance_type),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ];
        params.extend_from_slice(&fields);
        let (status, body) = call(&router, REGION, "RunInstances", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = super::ec2_query::error_code(&body);
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
        );
        let message = super::ec2_query::error_message(&body);
        assert_eq!(
            message,
            "When specifying CpuOptions you must specify both CoreCount and ThreadsPerCore.",
            "case {case:?}: expected the sentence EC2 words this refusal with, got {message:?}"
        );
        let described = ok(&router, &case, "DescribeInstances", &[]).await;
        assert!(
            super::ec2_query::elements(&described, "instanceId").is_empty(),
            "case {case:?}: a refused launch mints no instance, got {described}"
        );
    }
}

/// THE RULE for the two option WORDS of the same member: `CpuOptions$AmdSevSnp` and
/// `$NestedVirtualization` are reported for a launch that asked for a setting, and for one
/// that asked for neither they are reported for neither — AWS reporting no member for a
/// setting nothing asked for.
///
/// The rows span the words the two modelled enums publish
/// (`smithy_query(query="shape",shape="AmdSevSnpSpecification,NestedVirtualizationSpecification")`
/// — `disabled` and `enabled` for each) rather than only the one that turns a setting on.
#[tokio::test]
async fn the_cpu_option_words_a_launch_asked_for_are_reported() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[0];

    for word in ["enabled", "disabled"] {
        for member in ["AmdSevSnp", "NestedVirtualization"] {
            let case = format!("{instance_type} launched with {member}={word}");
            let instance_id = launch(
                &router,
                &case,
                instance_type,
                &[param(&format!("CpuOptions.{member}"), word)],
            )
            .await;
            let (_, _, amd_sev_snp, nested) =
                reported_cpu_options(&router, &case, &instance_id).await;
            let reported = if member == "AmdSevSnp" {
                (amd_sev_snp.clone(), nested.clone())
            } else {
                (nested.clone(), amd_sev_snp.clone())
            };
            assert_eq!(
                reported.0.as_deref(),
                Some(word),
                "case {case:?}: expected cpuOptions.{member} {word:?}, got {:?}",
                reported.0
            );
            assert_eq!(
                reported.1, None,
                "case {case:?}: the other option word was not asked for, so it is not \
                 reported, got {:?}",
                reported.1
            );
        }
    }

    // The negative half: a launch that asked for neither reports neither, whatever its type.
    let case = "a launch asking for neither option word";
    let instance_id = launch(&router, case, instance_type, &[]).await;
    let (_, _, amd_sev_snp, nested) = reported_cpu_options(&router, case, &instance_id).await;
    assert_eq!(
        (amd_sev_snp.as_deref(), nested.as_deref()),
        (None, None),
        "case {case:?}: expected neither option word, got {amd_sev_snp:?} and {nested:?}"
    );
}

/// THE RULE on a launch that inherits its topology from a LAUNCH TEMPLATE: the version's own
/// `CpuOptions` reaches the instance exactly as a launch naming the member itself does.
#[tokio::test]
async fn a_launch_template_version_carries_the_cpu_topology_it_declares() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[2];
    let case = format!("a template version of {instance_type}");
    let (valid_cores, valid_threads, _, _) = published_counts(&router, &case, instance_type).await;
    let core_count = valid_cores.first().copied().unwrap_or_default();
    let threads_per_core = valid_threads.first().copied().unwrap_or_default();

    let created = ok(
        &router,
        &case,
        "CreateLaunchTemplate",
        &[
            param("LaunchTemplateName", "cpu-topology"),
            param("LaunchTemplateData.ImageId", AMI),
            param("LaunchTemplateData.InstanceType", instance_type),
            param(
                "LaunchTemplateData.CpuOptions.CoreCount",
                &core_count.to_string(),
            ),
            param(
                "LaunchTemplateData.CpuOptions.ThreadsPerCore",
                &threads_per_core.to_string(),
            ),
        ],
    )
    .await;
    let template_id = element(&created, "launchTemplateId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateLaunchTemplate reported no id: {created}")
    });

    let launched = ok(
        &router,
        &case,
        "RunInstances",
        &[
            param("LaunchTemplate.LaunchTemplateId", &template_id),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    let instance_id = element(&launched, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instanceId: {launched}"));
    let (reported_cores, reported_threads, _, _) =
        reported_cpu_options(&router, &case, &instance_id).await;
    assert_eq!(
        (reported_cores.as_deref(), reported_threads.as_deref()),
        (
            Some(core_count.to_string().as_str()),
            Some(threads_per_core.to_string().as_str())
        ),
        "case {case:?}: expected the version's own {core_count}/{threads_per_core}, got \
         {reported_cores:?}/{reported_threads:?}"
    );
}

/// THE OTHER HALF of the rule: a count the instance type cannot be configured with refuses
/// the launch — `InvalidParameterValue` naming the parameter, the value and the counts the
/// type offers — and nothing is launched.
///
/// The rows sit one past each published bound of the type's own counts: one core below the
/// smallest it offers, one above the largest, and one thread per core above the most it runs.
#[tokio::test]
async fn a_count_the_instance_type_does_not_offer_refuses_the_launch() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[1];
    let case = format!("the bounds of {instance_type}");
    let (valid_cores, valid_threads, _, _) = published_counts(&router, &case, instance_type).await;
    let smallest = valid_cores.first().copied().unwrap_or_default();
    let largest = valid_cores.last().copied().unwrap_or_default();
    let most_threads = valid_threads.last().copied().unwrap_or_default();
    // Both counts are always named: one without the other is the refusal the sibling case
    // covers, so each row below names a count the type DOES offer beside the one it does not.
    let threads = param("CpuOptions.ThreadsPerCore", &most_threads.to_string());
    let cores = param("CpuOptions.CoreCount", &largest.to_string());

    // (case, the CpuOptions fields, the parameter the refusal must name)
    let rows: Vec<(String, Vec<Param>, &str)> = vec![
        (
            format!("one core below the {smallest} {instance_type} offers"),
            vec![
                param("CpuOptions.CoreCount", &(smallest - 1).to_string()),
                threads.clone(),
            ],
            "cpuOptions.coreCount",
        ),
        (
            format!("one core above the {largest} {instance_type} offers"),
            vec![
                param("CpuOptions.CoreCount", &(largest + 1).to_string()),
                threads.clone(),
            ],
            "cpuOptions.coreCount",
        ),
        (
            format!("one thread per core above the {most_threads} it runs"),
            vec![
                cores,
                param("CpuOptions.ThreadsPerCore", &(most_threads + 1).to_string()),
            ],
            "cpuOptions.threadsPerCore",
        ),
    ];

    for (case, fields, member) in rows {
        let mut params = vec![
            param("ImageId", AMI),
            param("InstanceType", instance_type),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ];
        params.extend_from_slice(&fields);
        let (status, body) = call(&router, REGION, "RunInstances", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = super::ec2_query::error_code(&body);
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
        );
        let message = super::ec2_query::error_message(&body);
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name the parameter {member}, got {message:?}"
        );
        let described = ok(&router, &case, "DescribeInstances", &[]).await;
        assert!(
            super::ec2_query::elements(&described, "instanceId").is_empty(),
            "case {case:?}: a refused launch mints no instance, got {described}"
        );
    }
}

/// An instance of a type this emulator publishes nothing for reports no CPU layout it
/// invented: with no counts to read and none asked for, the member is absent — and the same
/// launch naming a topology reports exactly that.
#[tokio::test]
async fn an_unpublished_instance_type_reports_only_what_its_launch_asked_for() {
    let (router, engine) = super::ec2_query::emulator();
    // A type no catalog row of this crate publishes, written into the store the way a launch
    // of an offered type would leave it, so the report is driven by the store rather than by
    // a request the service would refuse.
    let case = "an instance of a type the catalog does not publish";
    let instance_id = launch(&router, case, TYPES[0], &[]).await;
    {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        let instance = guard
            .instances
            .get_mut(&instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {instance_id}"));
        instance.instance_type = "zz9.plural-z-alpha".to_string();
        instance.detail.cpu_core_count = None;
        instance.detail.cpu_threads_per_core = None;
    }
    assert!(
        !reports_cpu_options(&router, case, &instance_id).await,
        "case {case:?}: an instance with no published counts and no asked-for topology \
         reports no cpuOptions at all"
    );

    let asked = "the same instance once its launch had asked for a topology";
    {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        let instance = guard
            .instances
            .get_mut(&instance_id)
            .unwrap_or_else(|| panic!("case {asked:?}: the store must hold {instance_id}"));
        instance.detail.cpu_core_count = Some(3);
        instance.detail.cpu_threads_per_core = Some(1);
    }
    let (cores, threads, _, _) = reported_cpu_options(&router, asked, &instance_id).await;
    assert_eq!(
        (cores.as_deref(), threads.as_deref()),
        (Some("3"), Some("1")),
        "case {asked:?}: expected the asked-for 3/1, got {cores:?}/{threads:?}"
    );
}

/// The `cpuOptions` an instance reports is read out of the instance, so a second read
/// reports the same topology: a describe is a report, not a recompute from the type.
#[tokio::test]
async fn two_reads_report_one_instances_topology_the_same() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[1];
    let case = format!("two reads of one {instance_type} instance");
    let (valid_cores, valid_threads, default_cores, default_threads) =
        published_counts(&router, &case, instance_type).await;
    let core_count = valid_cores.first().copied().unwrap_or_default();
    let threads_per_core = valid_threads.first().copied().unwrap_or_default();
    // The row is only evidence when it differs from what the type would have contributed.
    assert_ne!(
        (core_count, threads_per_core),
        (default_cores, default_threads),
        "case {case:?}: the asked-for topology must differ from {instance_type}'s default \
         {default_cores}/{default_threads}"
    );

    let instance_id = launch(
        &router,
        &case,
        instance_type,
        &[
            param("CpuOptions.CoreCount", &core_count.to_string()),
            param("CpuOptions.ThreadsPerCore", &threads_per_core.to_string()),
        ],
    )
    .await;
    let first = reported_cpu_options(&router, &case, &instance_id).await;
    let second = reported_cpu_options(&router, &case, &instance_id).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads must report one topology, got {first:?} then {second:?}"
    );
    assert_eq!(
        first.0.as_deref(),
        Some(core_count.to_string().as_str()),
        "case {case:?}: expected the asked-for {core_count} cores, got {:?}",
        first.0
    );
}

/// `RunInstances` answers with the same `Instance` shape a describe does, so the topology the
/// launch asked for is already in ITS answer rather than only in a later read.
#[tokio::test]
async fn the_launchs_own_answer_reports_the_topology_it_asked_for() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_type = TYPES[0];
    let case = format!("the answer {instance_type}'s own launch gives");
    let (valid_cores, valid_threads, _, _) = published_counts(&router, &case, instance_type).await;
    let core_count = valid_cores.first().copied().unwrap_or_default();
    let threads_per_core = valid_threads.first().copied().unwrap_or_default();

    let body = ok(
        &router,
        &case,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("InstanceType", instance_type),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("CpuOptions.CoreCount", &core_count.to_string()),
            param("CpuOptions.ThreadsPerCore", &threads_per_core.to_string()),
        ],
    )
    .await;
    let parsed = document(&case, &body);
    let options = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "cpuOptions")
        .unwrap_or_else(|| panic!("case {case:?}: the launch reports no cpuOptions: {body}"));
    assert_eq!(
        (
            child(options, "coreCount").and_then(|node| node.text().map(str::to_string)),
            child_text(options, "threadsPerCore"),
        ),
        (
            Some(core_count.to_string()),
            Some(threads_per_core.to_string())
        ),
        "case {case:?}: expected the launch to report {core_count}/{threads_per_core}: {body}"
    );
}
