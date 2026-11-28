// SPDX-License-Identifier: CC0-1.0

//! What the EMR 200 READ paths project: the step's execution metadata, the members every
//! cluster read carries, the precision every `Date` member is serialized at, and the Amazon
//! Linux catalog a release label publishes.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, and reads every outcome back through `DescribeStep`, `ListSteps`,
//! `DescribeCluster`, `ListClusters` and `DescribeReleaseLabel`, so nothing here is evidence of
//! anything but the persisted state.
//!
//! FOUR RULES are under test.
//!
//! 1. A step's `AdditionalInfo` reports the resource manager its EXECUTION was submitted to and
//!    the applications that manager admitted for it. A step whose execution has not started has
//!    reached no manager, so it reports none; one that has reports the manager and one
//!    application id per submission, recorded once and the same on every later read and on both
//!    the single read and the listing.
//! 2. A `com.amazonaws.emr#Date` member is epoch seconds at MILLISECOND precision — the
//!    precision Amazon EMR serializes a timestamp at — on every read that serves one.
//! 3. A cluster read reports the placement its launch REQUESTED (the candidate list where it
//!    named one, otherwise the single subnet or zone it named outright), and the members every
//!    cluster read carries whatever its launch said: its Kerberos configuration (the empty
//!    structure for a cluster that configured none), how it executes its steps, and whether the
//!    logs its nodes produce are uploaded.
//! 4. `DescribeReleaseLabel` publishes the WHOLE Amazon Linux catalog of the generation the
//!    requested release is built on, newest release first.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{
    at, await_cluster_state, await_settled_step, kept_alive_request, minimal_request, step_config,
    Emr, INSTANCE_PROFILE, SERVICE_ROLE,
};

/// The resource manager an Amazon EMR cluster schedules every step's execution through.
const RESOURCE_MANAGER: &str = "YARN";

/// The jar every step below submits. No bytes of it exist here, so each step ends in the
/// modelled failure state — which is beside the point: what is under test is the metadata of a
/// step whose execution RAN, whatever it came to.
const STEP_JAR: &str = "s3://jars/projection.jar";

/// A cluster that stays up, with `steps` steps and `concurrency` of them running at once.
fn launch_with_steps(name: &str, steps: usize, concurrency: i32) -> Value {
    let mut request = kept_alive_request(name);
    request["StepConcurrencyLevel"] = json!(concurrency);
    request["Steps"] = Value::Array(
        (0..steps)
            .map(|index| {
                step_config(
                    &format!("{name} step {index}"),
                    STEP_JAR,
                    json!([index.to_string()]),
                    Value::Null,
                )
            })
            .collect(),
    );
    request
}

/// The ids `ListSteps` reports for `cluster_id`, newest first.
async fn step_ids(emr: &Emr, cluster_id: &str) -> Vec<String> {
    let listed = emr.list_steps(json!({ "ClusterId": cluster_id })).await;
    listed["Steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| at(step, &["Id"]).as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `AdditionalInfo` `ListSteps` reports for one step of `cluster_id`.
async fn listed_additional_info(emr: &Emr, cluster_id: &str, step_id: &str) -> Value {
    let listed = emr
        .list_steps(json!({ "ClusterId": cluster_id, "StepIds": [step_id] }))
        .await;
    at(&listed, &["Steps"])
        .as_array()
        .and_then(|steps| steps.first().cloned())
        .map(|step| step["AdditionalInfo"].clone())
        .unwrap_or(Value::Null)
}

/// The application ids one `AdditionalInfo` carries, as a list.
fn application_ids(case: &str, info: &Value) -> Vec<String> {
    at(info, &["ResourceManagerApplicationIds"])
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ResourceManagerApplicationIds must be a list, got {info}")
        })
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `(cluster timestamp, sequence)` of a YARN application id, or `None` when the id is not of
/// the `application_<cluster timestamp>_<sequence>` form the resource manager names one by.
fn parsed_application_id(id: &str) -> Option<(u128, String)> {
    let body = id.strip_prefix("application_")?;
    let (timestamp, sequence) = body.split_once('_')?;
    if sequence.len() < 4 || !sequence.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    Some((timestamp.parse().ok()?, sequence.to_string()))
}

// ---------------------------------------------------------------------------
// RULE 1 — the step's resource-manager metadata
// ---------------------------------------------------------------------------

/// THE RULE: a step reports, under `AdditionalInfo`, the resource manager its execution was
/// submitted to and the applications that manager admitted for it — nothing at all while its
/// execution has not started, and the same thing on `DescribeStep` and `ListSteps` once it has.
///
/// The rows differ along how many steps the queue holds and how many run at once, which is what
/// decides how many applications the cluster's manager admits and in what order.
#[tokio::test]
async fn a_step_reports_the_resource_manager_applications_its_execution_produced() {
    struct Case {
        label: &'static str,
        steps: usize,
        concurrency: i32,
    }

    let cases = [
        Case {
            label: "one step, one slot",
            steps: 1,
            concurrency: 1,
        },
        Case {
            label: "three steps, one slot",
            steps: 3,
            concurrency: 1,
        },
        Case {
            label: "two steps, two slots",
            steps: 2,
            concurrency: 2,
        },
    ];

    for Case {
        label,
        steps,
        concurrency,
    } in cases
    {
        let emr = Emr::new();
        let cluster_id = emr
            .run_job_flow(launch_with_steps(label, steps, concurrency))
            .await;

        // While the cluster is still coming up no step of it has been submitted to the
        // resource manager, so neither read reports one.
        let ids = step_ids(&emr, &cluster_id).await;
        assert_eq!(
            ids.len(),
            steps,
            "case {label:?}: the launch's steps must all be queued, got {ids:?}"
        );
        for step_id in &ids {
            let step = emr
                .ok(
                    "DescribeStep",
                    json!({ "ClusterId": cluster_id, "StepId": step_id }),
                )
                .await["Step"]
                .clone();
            if at(&step, &["Status", "State"]).as_str() != Some("PENDING") {
                continue;
            }
            assert!(
                step["AdditionalInfo"].is_null(),
                "case {label:?}: a step whose execution has not started has reached no \
                 resource manager, got {step}"
            );
            assert!(
                listed_additional_info(&emr, &cluster_id, step_id)
                    .await
                    .is_null(),
                "case {label:?}: ListSteps must agree that a queued step has no \
                 AdditionalInfo"
            );
        }

        // Once each step has run, both reads carry the manager and the one application its
        // submission produced.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut timestamps: BTreeSet<u128> = BTreeSet::new();
        // `ListSteps` reports the queue in reverse order, so the ids are walked back into
        // SUBMISSION order: each step is waited for after the one in front of it, which is the
        // order the cluster runs them in.
        for step_id in ids.iter().rev() {
            let settled = await_settled_step(&emr, &cluster_id, step_id).await;
            let info = settled["AdditionalInfo"].clone();
            assert_eq!(
                at(&info, &["ResourceManager"]).as_str(),
                Some(RESOURCE_MANAGER),
                "case {label:?}: a step that ran reports the manager it ran under, got {settled}"
            );
            let reported = application_ids(label, &info);
            assert_eq!(
                reported.len(),
                1,
                "case {label:?}: one execution submits one application, got {reported:?}"
            );
            let (timestamp, sequence) = parsed_application_id(&reported[0]).unwrap_or_else(|| {
                panic!(
                    "case {label:?}: {:?} must be application_<cluster timestamp>_<sequence>",
                    reported[0]
                )
            });
            assert!(
                timestamp > 0,
                "case {label:?}: the cluster timestamp must be the instant the manager \
                 started, got {timestamp} from {sequence}"
            );
            timestamps.insert(timestamp);
            assert!(
                seen.insert(reported[0].clone()),
                "case {label:?}: two steps must not share an application id, got {reported:?} \
                 among {seen:?}"
            );

            // The same metadata on the listing, and the same again on a second read: the id is
            // recorded once, not re-derived per call.
            assert_eq!(
                listed_additional_info(&emr, &cluster_id, step_id).await,
                info,
                "case {label:?}: ListSteps must report the step's AdditionalInfo as \
                 DescribeStep does"
            );
            let reread = emr
                .ok(
                    "DescribeStep",
                    json!({ "ClusterId": cluster_id, "StepId": step_id }),
                )
                .await["Step"]["AdditionalInfo"]
                .clone();
            assert_eq!(
                reread, info,
                "case {label:?}: a step's AdditionalInfo must not change between reads"
            );
        }
        assert_eq!(
            timestamps.len(),
            1,
            "case {label:?}: every application of one cluster carries that cluster's own \
             manager timestamp, got {timestamps:?}"
        );
    }
}

/// THE RULE's other half: a step the queue never ran reaches no resource manager, so it reports
/// no execution metadata however it ended.
#[tokio::test]
async fn a_step_that_never_ran_reports_no_resource_manager() {
    let emr = Emr::new();
    let cluster_id = emr
        .run_job_flow(launch_with_steps("never ran", 1, 1))
        .await;
    let ids = step_ids(&emr, &cluster_id).await;
    let step_id = ids
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the launch's step must be queued, got {ids:?}"));

    // The cluster is shut down while its step is still queued, which cancels the step before
    // its turn comes.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [cluster_id] }))
        .await;
    let settled = await_settled_step(&emr, &cluster_id, &step_id).await;
    assert_eq!(
        at(&settled, &["Status", "State"]).as_str(),
        Some("CANCELLED"),
        "a step cancelled with its cluster never ran, got {settled}"
    );
    assert!(
        settled["AdditionalInfo"].is_null(),
        "a step that never reached the resource manager reports none, got {settled}"
    );
    assert!(
        listed_additional_info(&emr, &cluster_id, &step_id)
            .await
            .is_null(),
        "ListSteps must agree that a step that never ran has no AdditionalInfo"
    );
}

// ---------------------------------------------------------------------------
// RULE 2 — the precision every Date member is served at
// ---------------------------------------------------------------------------

/// The magnitude at or above which a number of this surface is a timestamp: epoch seconds of
/// 2001-09-09T01:46:40Z, decades before any instant this service can serve and orders of
/// magnitude above every count, size and concurrency level these reads also report.
const EPOCH_SECONDS_FLOOR: f64 = 1_000_000_000.0;

/// Every epoch-seconds number `body` carries, with the path it sits at.
///
/// A timestamp is recognised by its MAGNITUDE rather than by its form: this protocol renders
/// one as epoch seconds, so an instant whose millisecond part is zero is rendered as a WHOLE
/// number — the shape a count has — and a collector keyed on the fractional form would miss it
/// and report a read that served a timestamp as serving none.
///
/// The members it therefore walks are the `com.amazonaws.emr#Date` ones these reads carry —
/// `Cluster$Status$Timeline` (`CreationDateTime`, `ReadyDateTime`, `EndDateTime`),
/// `Step$Status$Timeline`, `InstanceGroup$Status$Timeline` and
/// `SecurityConfigurationSummary$CreationDateTime` — beside which every other number of this
/// surface is a count, a size or a concurrency level, orders of magnitude below
/// [`EPOCH_SECONDS_FLOOR`].
fn epoch_numbers(path: &str, body: &Value, into: &mut Vec<(String, f64)>) {
    match body {
        Value::Object(members) => {
            for (name, member) in members {
                epoch_numbers(&format!("{path}.{name}"), member, into);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                epoch_numbers(&format!("{path}[{index}]"), item, into);
            }
        }
        Value::Number(number) => {
            if let Some(seconds) = number
                .as_f64()
                .filter(|seconds| *seconds >= EPOCH_SECONDS_FLOOR)
            {
                into.push((path.to_string(), seconds));
            }
        }
        _ => {}
    }
}

/// Every epoch-seconds number `payload` carries is at millisecond precision.
fn assert_millisecond_precision(case: &str, payload: &Value) {
    let mut numbers = Vec::new();
    epoch_numbers("$", payload, &mut numbers);
    assert!(
        !numbers.is_empty(),
        "case {case:?}: this read must serve at least one timestamp, got {payload}"
    );
    for (path, seconds) in numbers {
        let millis = seconds * 1_000.0;
        assert!(
            (millis - millis.round()).abs() < 1e-3,
            "case {case:?}: {path} must be epoch seconds at millisecond precision, got \
             {seconds}"
        );
    }
}

/// THE RULE: a `Date` member is epoch seconds at millisecond precision, on every read that
/// serves one. The rows are the reads of this surface that carry a timeline.
#[tokio::test]
async fn every_date_a_read_serves_carries_at_most_millisecond_precision() {
    let emr = Emr::new();
    emr.ok(
        "CreateSecurityConfiguration",
        json!({
            "Name": "precision",
            "SecurityConfiguration": "{\"EncryptionConfiguration\":{\"EnableInTransitEncryption\":false,\"EnableAtRestEncryption\":false}}",
        }),
    )
    .await;
    let cluster_id = emr.run_job_flow(launch_with_steps("precision", 1, 1)).await;
    let ids = step_ids(&emr, &cluster_id).await;
    let step_id = ids
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the launch's step must be queued, got {ids:?}"));
    await_settled_step(&emr, &cluster_id, &step_id).await;

    // The cluster is then taken down, so the reads below also carry an end instant.
    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [cluster_id.clone()] }),
    )
    .await;
    await_cluster_state(&emr, &cluster_id, "precision", "TERMINATED").await;

    let reads = [
        ("DescribeCluster", json!({ "ClusterId": cluster_id })),
        (
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": step_id }),
        ),
        ("ListSteps", json!({ "ClusterId": cluster_id })),
        ("ListClusters", json!({})),
        ("ListInstanceGroups", json!({ "ClusterId": cluster_id })),
        (
            "DescribeSecurityConfiguration",
            json!({ "Name": "precision" }),
        ),
        ("ListSecurityConfigurations", json!({})),
    ];

    for (operation, request) in reads {
        let payload = emr.ok(operation, request).await;
        assert_millisecond_precision(operation, &payload);
    }
}

// ---------------------------------------------------------------------------
// RULE 3 — the members every cluster read carries
// ---------------------------------------------------------------------------

/// One `InstanceFleets` member: the primary fleet, which is the form a launch that states
/// subnet CANDIDATES takes.
fn master_fleet() -> Value {
    json!([{
        "InstanceFleetType": "MASTER",
        "TargetOnDemandCapacity": 1,
        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
    }])
}

/// THE RULE: a cluster read reports the placement its launch requested — the candidates where
/// it named a list, otherwise the one subnet or zone it named outright — and the members every
/// cluster carries whatever its launch said.
///
/// The rows differ along WHICH form the launch stated its placement in, and along whether it
/// configured Kerberos authentication at all.
#[tokio::test]
async fn a_cluster_read_reports_its_requested_placement_and_the_members_every_cluster_carries() {
    struct Case {
        label: &'static str,
        instances: Value,
        kerberos: Option<Value>,
        requested_subnets: &'static [&'static str],
        requested_zones: &'static [&'static str],
        realm: Option<&'static str>,
    }

    let cases = vec![
        Case {
            label: "one subnet named outright",
            instances: json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "Ec2SubnetId": "subnet-0aaa1111bbbb2222c",
            }),
            kerberos: None,
            requested_subnets: &["subnet-0aaa1111bbbb2222c"],
            requested_zones: &[],
            realm: None,
        },
        Case {
            label: "three subnet candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": ["subnet-01", "subnet-02", "subnet-03"],
            }),
            kerberos: None,
            requested_subnets: &["subnet-01", "subnet-02", "subnet-03"],
            requested_zones: &[],
            realm: None,
        },
        Case {
            label: "an empty candidate list",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": [],
            }),
            kerberos: None,
            requested_subnets: &[],
            requested_zones: &[],
            realm: None,
        },
        Case {
            label: "one zone named outright",
            instances: json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "Placement": { "AvailabilityZone": "us-east-1c" },
            }),
            kerberos: None,
            requested_subnets: &[],
            requested_zones: &["us-east-1c"],
            realm: None,
        },
        Case {
            label: "two zone candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Placement": { "AvailabilityZones": ["us-west-2b", "us-west-2d"] },
            }),
            kerberos: None,
            requested_subnets: &[],
            requested_zones: &["us-west-2b", "us-west-2d"],
            realm: None,
        },
        Case {
            label: "no placement at all, and Kerberos configured",
            instances: json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            kerberos: Some(json!({
                "Realm": "EC2.INTERNAL",
                "KdcAdminPassword": "not-reported-back",
                "ADDomainJoinUser": "emr-join",
                "ADDomainJoinPassword": "not-reported-back-either",
            })),
            requested_subnets: &[],
            requested_zones: &[],
            realm: Some("EC2.INTERNAL"),
        },
    ];

    let emr = Emr::new();
    for Case {
        label,
        instances,
        kerberos,
        requested_subnets,
        requested_zones,
        realm,
    } in cases
    {
        let mut request = json!({
            "Name": label,
            "ReleaseLabel": "emr-6.15.0",
            "JobFlowRole": INSTANCE_PROFILE,
            "ServiceRole": SERVICE_ROLE,
            "Instances": instances,
        });
        if let Some(kerberos) = kerberos {
            request["KerberosAttributes"] = kerberos;
        }
        let cluster_id = emr.run_job_flow(request).await;
        let cluster = emr.describe_cluster(&cluster_id).await;
        let attributes = &cluster["Ec2InstanceAttributes"];

        for (member, expected) in [
            ("RequestedEc2SubnetIds", requested_subnets),
            ("RequestedEc2AvailabilityZones", requested_zones),
        ] {
            let reported: Vec<String> = attributes[member]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: {member} must be a list, got {cluster}")
                })
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                reported, expected,
                "case {label:?}: {member} must report what the launch requested, got {cluster}"
            );
        }

        // The members every cluster read carries, whatever the launch said.
        assert_eq!(
            cluster["StepExecutionMode"].as_str(),
            Some("STANDARD"),
            "case {label:?}: a cluster states how it executes its steps, got {cluster}"
        );
        assert_eq!(
            cluster["SystemLogUploadDisabled"].as_bool(),
            Some(false),
            "case {label:?}: a cluster states whether its logs are uploaded, got {cluster}"
        );
        let attributes_of_kerberos = cluster["KerberosAttributes"].clone();
        assert!(
            attributes_of_kerberos.is_object(),
            "case {label:?}: KerberosAttributes is reported as a structure, got {cluster}"
        );
        assert_eq!(
            at(&attributes_of_kerberos, &["Realm"]).as_str(),
            realm,
            "case {label:?}: the realm reported is the one the launch configured, got {cluster}"
        );
        // A password is supplied to a launch and never reported back by a read.
        for member in [
            "KdcAdminPassword",
            "CrossRealmTrustPrincipalPassword",
            "ADDomainJoinPassword",
        ] {
            assert!(
                at(&attributes_of_kerberos, &[member]).is_null(),
                "case {label:?}: {member} must not be reported back, got {cluster}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4 — the Amazon Linux catalog a release label publishes
// ---------------------------------------------------------------------------

/// `(release date, revision)` of an Amazon Linux release label, which is what orders the
/// catalog: the 8-digit date part and the revision after it, both of the dotted release-note
/// form `OSRelease$Label` documents.
fn release_date_and_revision(case: &str, label: &str) -> (u32, u32) {
    let parts: Vec<&str> = label.split('.').collect();
    let date = parts
        .iter()
        .find(|part| part.len() == 8 && part.chars().all(|digit| digit.is_ascii_digit()))
        .and_then(|part| part.parse().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: {label:?} must carry an 8-digit Amazon Linux release date")
        });
    let revision = parts
        .last()
        .and_then(|part| part.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: {label:?} must end in a revision"));
    (date, revision)
}

/// THE RULE: `DescribeReleaseLabel` publishes every Amazon Linux release of the generation the
/// requested Amazon EMR release is built on, NEWEST FIRST, and the newest of them is the one a
/// cluster of that release runs when its launch names none.
///
/// The rows span the generations an Amazon EMR release is built on — Amazon Linux 2, Amazon
/// Linux 2023, and the generation before Amazon Linux 2, which offers no choice at all — and
/// each names one release of its generation the issue did not cite.
#[tokio::test]
async fn describe_release_label_publishes_the_whole_catalog_of_its_generation_newest_first() {
    struct Case {
        label: &'static str,
        /// The Amazon Linux generation every offered release belongs to.
        prefix: &'static str,
        /// One release of that generation this case asserts is offered.
        offered: &'static str,
    }

    let cases = [
        Case {
            label: "emr-5.36.2",
            prefix: "2.0.",
            offered: "2.0.20240620.0",
        },
        Case {
            label: "emr-6.15.0",
            prefix: "2.0.",
            offered: "2.0.20230504.1",
        },
        Case {
            label: "emr-7.9.0",
            prefix: "2023.",
            offered: "2023.5.20240805.0",
        },
    ];

    let emr = Emr::new();
    for Case {
        label,
        prefix,
        offered,
    } in cases
    {
        let payload = emr
            .ok("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        let offered_releases: Vec<String> = at(&payload, &["AvailableOSReleases"])
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: AvailableOSReleases must be a list, got {payload}"))
            .iter()
            .map(|release| at(release, &["Label"]).as_str().unwrap_or_default().to_string())
            .collect();

        // Every entry belongs to the generation the release is built on, and the catalog is
        // the generation's whole published list rather than a handful of it.
        assert!(
            offered_releases.len() > 10,
            "case {label:?}: the generation's catalog must be published whole, got \
             {offered_releases:?}"
        );
        assert!(
            offered_releases.iter().all(|release| release.starts_with(prefix)),
            "case {label:?}: every offered release belongs to {prefix:?}, got \
             {offered_releases:?}"
        );
        assert!(
            offered_releases.iter().any(|release| release == offered),
            "case {label:?}: {offered:?} is a release of this generation and must be offered, \
             got {offered_releases:?}"
        );

        // Newest first, strictly: the catalog is ordered by release date and revision, so
        // index 0 is the current Amazon Linux release and no entry repeats.
        let mut previous: Option<(u32, u32)> = None;
        for release in &offered_releases {
            let current = release_date_and_revision(label, release);
            if let Some(previous) = previous {
                assert!(
                    current < previous,
                    "case {label:?}: the catalog must be newest first, got {release:?} after \
                     {previous:?} in {offered_releases:?}"
                );
            }
            previous = Some(current);
        }

        // And the newest is the release a cluster of that label runs when it names none,
        // which is what ties the published catalog to the launch path.
        let cluster_id = emr
            .run_job_flow({
                let mut request = minimal_request(label);
                request["ReleaseLabel"] = json!(label);
                request
            })
            .await;
        let cluster = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            cluster["OSReleaseLabel"].as_str(),
            offered_releases.first().map(String::as_str),
            "case {label:?}: the default Amazon Linux release is the newest the catalog \
             offers, got {cluster}"
        );
    }

    // A release published before the Amazon Linux 2 generation offers no Amazon Linux
    // release to choose between.
    let payload = emr
        .ok(
            "DescribeReleaseLabel",
            json!({ "ReleaseLabel": "emr-5.29.0" }),
        )
        .await;
    assert_eq!(
        at(&payload, &["AvailableOSReleases"]).as_array(),
        Some(&Vec::new()),
        "a release from before Amazon Linux 2 offers no Amazon Linux release, got {payload}"
    );
}
