// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of a job created FROM a job template.
//!
//! The rule under test: a job that names a job template states only what it wants to
//! differ and inherits every member it leaves out — its `Settings` member by member, and
//! the job-shaping members the template also carries (its queue, priority, status-update
//! interval, acceleration settings and hop destinations) — and a `JobTemplate` reference
//! this account holds no template for is refused with the modelled `NotFoundException`
//! naming the member the reference came from, exactly as a `Queue` reference is. A member
//! the request DOES state replaces the template's whole value for it.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{arn, elemental_inference_forms, Emulator, ROLE};

/// A `JobTemplateSettings` with one file output group, one input and a timecode config —
/// the members a template normally owns when a caller submits only the input.
fn template_settings(destination: &str, modifier: &str) -> Value {
    json!({
        "adAvailOffset": 2,
        "inputs": [{
            "timecodeSource": "ZEROBASED",
            "psiControl": "USE_PSI",
            "audioSelectors": { "Audio Selector 1": { "defaultSelection": "DEFAULT" } }
        }],
        "timecodeConfig": { "source": "EMBEDDED" },
        "outputGroups": [{
            "name": "File Group",
            "outputs": [{
                "nameModifier": modifier,
                "containerSettings": { "container": "MP4" }
            }],
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": destination }
            }
        }]
    })
}

/// One row: a template, the `CreateJob` body that names it, and the `Job` members that
/// must read back.
struct Case {
    label: &'static str,
    /// The template this row creates, so each row inherits from its own.
    template: &'static str,
    /// The `CreateJobTemplate` body.
    created: Value,
    /// The `CreateJob` body, minus the `jobTemplate` member the harness adds — so a row
    /// can name the template by name or by ARN.
    job: Value,
    /// How the row references the template: `true` for its ARN, `false` for its name.
    by_arn: bool,
    /// `(JSON pointer under /job, expected value)` for every member this row pins.
    expected: Vec<(&'static str, Value)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only Inputs stated, so every output member is inherited",
            template: "inherit-outputs",
            created: json!({
                "name": "inherit-outputs",
                "settings": template_settings("s3://example-bucket/out/", "_tpl")
            }),
            job: json!({
                "role": ROLE,
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/one.mp4" }] }
            }),
            by_arn: false,
            expected: vec![
                // The request's own member wins: its inputs, not the template's.
                (
                    "/settings/inputs/0/fileInput",
                    json!("s3://example-bucket/in/one.mp4"),
                ),
                // Everything the request omitted comes from the template.
                ("/settings/outputGroups/0/name", json!("File Group")),
                (
                    "/settings/outputGroups/0/outputGroupSettings/type",
                    json!("FILE_GROUP_SETTINGS"),
                ),
                (
                    "/settings/outputGroups/0/outputGroupSettings/fileGroupSettings/destination",
                    json!("s3://example-bucket/out/"),
                ),
                (
                    "/settings/outputGroups/0/outputs/0/containerSettings/container",
                    json!("MP4"),
                ),
                ("/settings/outputGroups/0/outputs/0/nameModifier", json!("_tpl")),
                ("/settings/timecodeConfig/source", json!("EMBEDDED")),
                ("/settings/adAvailOffset", json!(2)),
            ],
        },
        Case {
            label: "the template referenced by its ARN",
            template: "inherit-by-arn",
            created: json!({
                "name": "inherit-by-arn",
                "settings": template_settings("s3://example-bucket/arn-out/", "_arn")
            }),
            job: json!({
                "role": ROLE,
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/two.mp4" }] }
            }),
            by_arn: true,
            expected: vec![
                ("/jobTemplate", json!("inherit-by-arn")),
                (
                    "/settings/outputGroups/0/outputGroupSettings/fileGroupSettings/destination",
                    json!("s3://example-bucket/arn-out/"),
                ),
            ],
        },
        Case {
            label: "the request's own output groups replace the template's",
            template: "override-outputs",
            created: json!({
                "name": "override-outputs",
                "settings": template_settings("s3://example-bucket/tpl-out/", "_tpl")
            }),
            job: json!({
                "role": ROLE,
                "settings": {
                    "inputs": [{ "fileInput": "s3://example-bucket/in/three.mp4" }],
                    "outputGroups": [{
                        "name": "My Group",
                        "outputs": [{
                            "nameModifier": "_mine",
                            "containerSettings": { "container": "MP4" }
                        }],
                        "outputGroupSettings": {
                            "type": "FILE_GROUP_SETTINGS",
                            "fileGroupSettings": { "destination": "s3://example-bucket/mine/" }
                        }
                    }]
                }
            }),
            by_arn: false,
            expected: vec![
                ("/settings/outputGroups/0/name", json!("My Group")),
                (
                    "/settings/outputGroups/0/outputGroupSettings/fileGroupSettings/destination",
                    json!("s3://example-bucket/mine/"),
                ),
                // A member the request did not state is still inherited.
                ("/settings/timecodeConfig/source", json!("EMBEDDED")),
            ],
        },
        Case {
            label: "no Inputs stated either, so the template's inputs are the job's",
            template: "inherit-inputs",
            created: json!({
                "name": "inherit-inputs",
                "settings": template_settings("s3://example-bucket/in-out/", "_in")
            }),
            job: json!({ "role": ROLE, "settings": {} }),
            by_arn: false,
            expected: vec![
                ("/settings/inputs/0/timecodeSource", json!("ZEROBASED")),
                ("/settings/inputs/0/psiControl", json!("USE_PSI")),
                (
                    "/settings/inputs/0/audioSelectors/Audio Selector 1/defaultSelection",
                    json!("DEFAULT"),
                ),
                // `InputTemplate` models no `FileInput`, so the inherited input carries
                // none rather than one invented for it.
                ("/settings/inputs/0/fileInput", Value::Null),
            ],
        },
        Case {
            label: "the job-shaping members the template carries are inherited too",
            template: "inherit-shape",
            created: json!({
                "name": "inherit-shape",
                "queue": "template-queue",
                "priority": 25,
                "statusUpdateInterval": "SECONDS_120",
                "accelerationSettings": { "mode": "DISABLED" },
                "hopDestinations": [{ "priority": 10, "waitMinutes": 5 }],
                "settings": template_settings("s3://example-bucket/shape-out/", "_shape")
            }),
            job: json!({
                "role": ROLE,
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/four.mp4" }] }
            }),
            by_arn: false,
            expected: vec![
                // `Job$Queue` is projected as the queue's ARN.
                ("/queue", json!(arn("queues/template-queue"))),
                ("/priority", json!(25)),
                ("/statusUpdateInterval", json!("SECONDS_120")),
                ("/accelerationSettings/mode", json!("DISABLED")),
                ("/hopDestinations/0/priority", json!(10)),
                ("/hopDestinations/0/waitMinutes", json!(5)),
            ],
        },
        Case {
            label: "the request's own job-shaping members replace the template's",
            template: "override-shape",
            created: json!({
                "name": "override-shape",
                "queue": "template-queue",
                "priority": 25,
                "statusUpdateInterval": "SECONDS_120",
                "settings": template_settings("s3://example-bucket/shape2-out/", "_shape2")
            }),
            job: json!({
                "role": ROLE,
                "queue": "job-queue",
                "priority": -50,
                "statusUpdateInterval": "SECONDS_60",
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/five.mp4" }] }
            }),
            by_arn: false,
            expected: vec![
                ("/queue", json!(arn("queues/job-queue"))),
                ("/priority", json!(-50)),
                ("/statusUpdateInterval", json!("SECONDS_60")),
            ],
        },
    ]
}

/// A job created from a job template inherits every member it did not state, and states
/// its own wherever it did — and `GetJob` reads back exactly that.
#[tokio::test]
async fn a_job_created_from_a_template_inherits_what_it_did_not_state() {
    let emulator = Emulator::new();
    // The queues the rows name. Both are PAUSED, so no case races the clock: a held job
    // stays SUBMITTED and its settings stay readable for the whole suite.
    for queue in ["template-queue", "job-queue"] {
        let created = emulator
            .create_queue(json!({ "name": queue, "status": "PAUSED" }))
            .await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {queue:?} must be created, got {} with body {}",
            created.status, created.body
        );
    }

    for case in cases() {
        let label = case.label;
        let created = emulator.create_job_template(case.created.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture template must be created, got {} with body {}",
            created.status, created.body
        );

        let reference = if case.by_arn {
            arn(&format!("jobTemplates/{}", case.template))
        } else {
            case.template.to_string()
        };
        let mut body = case.job.clone();
        body["jobTemplate"] = json!(reference);
        let job = emulator.create_job(body).await;
        assert_eq!(
            job.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            job.status, job.body
        );
        // "The job template that the job is created from" — the template's NAME, however
        // the request referenced it.
        assert_eq!(
            job.string("/job/jobTemplate"),
            case.template,
            "case {label:?}: Job.JobTemplate must name the template, got {}",
            job.body
        );

        let id = job.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: the job must read back, got {} with body {}",
            fetched.status, fetched.body
        );
        for (pointer, expected) in &case.expected {
            assert_eq!(
                job.at(&format!("/job{pointer}")),
                expected,
                "case {label:?}: CreateJob's {pointer} must be {expected}, got {}",
                job.body
            );
            assert_eq!(
                fetched.at(&format!("/job{pointer}")),
                expected,
                "case {label:?}: GetJob's {pointer} must be {expected}, got {}",
                fetched.body
            );
        }
    }
}

/// A `JobTemplate` reference this account holds no template for is the modelled 404
/// naming the member it came from — the job is not created against settings that do not
/// exist.
#[tokio::test]
async fn a_job_naming_a_template_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_job_template(json!({
            "name": "real-template",
            "settings": template_settings("s3://example-bucket/out/", "_t")
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created, got {} with body {}",
        created.status, created.body
    );
    let deleted = emulator.delete_job_template("real-template").await;
    assert_eq!(
        deleted.status, 202,
        "the fixture template must be deleted, got {} with body {}",
        deleted.status, deleted.body
    );

    // The rows span the kinds of dead reference a client can send: a name never used, a
    // name whose template has just been deleted, and each of those as an ARN.
    for (label, reference) in [
        ("a name never used", "never-created".to_string()),
        ("a template just deleted", "real-template".to_string()),
        ("an ARN for a name never used", arn("jobTemplates/never-created")),
        ("an ARN for a template just deleted", arn("jobTemplates/real-template")),
    ] {
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "jobTemplate": reference,
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/x.mp4" }] }
            }))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("/message"),
            "The resource you specified in jobTemplate was not found.",
            "case {label:?}: expected the member-naming refusal, got {}",
            reply.body
        );
    }

    // A template this account DOES hold is still accepted, so the refusal is the
    // reference and not the member.
    let recreated = emulator
        .create_job_template(json!({
            "name": "real-template",
            "settings": template_settings("s3://example-bucket/out/", "_t")
        }))
        .await;
    assert_eq!(
        recreated.status, 201,
        "the template must be creatable again, got {} with body {}",
        recreated.status, recreated.body
    );
    let accepted = emulator
        .create_job(json!({
            "role": ROLE,
            "jobTemplate": "real-template",
            "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/x.mp4" }] }
        }))
        .await;
    assert_eq!(
        accepted.status, 201,
        "a job naming a stored template must be created, got {} with body {}",
        accepted.status, accepted.body
    );
}

/// The Elemental Inference settings are one of the job-shaping members a job template
/// carries, so they follow the same inheritance rule as the rest: a job that states none
/// of its own takes the template's, a job that states its own replaces the template's
/// whole value for it, and a job that states none from a template that carries none
/// carries none — the service invents no value for the member.
///
/// The table spans the pairings of what the template carries and what the job states,
/// over the forms the member takes (`super::job_lifecycle::elemental_inference_forms`).
#[tokio::test]
async fn a_job_inherits_the_templates_elemental_inference_settings_unless_it_states_its_own() {
    let emulator = Emulator::new();
    let forms = elemental_inference_forms();

    // Every (what the template carries, what the job states) pairing: the job's own
    // statement wins wherever it made one, and the template's value stands wherever it
    // did not.
    for (template_index, (template_case, on_template)) in forms.iter().enumerate() {
        for (job_index, (job_case, on_job)) in forms.iter().enumerate() {
            let case = format!("template {template_case:?}, job {job_case:?}");
            let name = format!("inference-{template_index}-{job_index}");
            let mut template = json!({
                "name": name,
                "settings": template_settings(
                    &format!("s3://example-bucket/out-{template_index}-{job_index}/"),
                    "_tpl",
                )
            });
            if !on_template.is_null() {
                template["elementalInferenceSettings"] = on_template.clone();
            }
            let created_template = emulator.create_job_template(template).await;
            assert_eq!(
                created_template.status, 201,
                "case {case}: the fixture template must be created, got {} with body {}",
                created_template.status, created_template.body
            );

            let mut body = json!({
                "role": ROLE,
                "jobTemplate": name,
                "settings": {
                    "inputs": [{
                        "fileInput": format!(
                            "s3://example-bucket/in/{template_index}-{job_index}.mp4"
                        )
                    }]
                }
            });
            if !on_job.is_null() {
                body["elementalInferenceSettings"] = on_job.clone();
            }
            let job = emulator.create_job(body).await;
            assert_eq!(
                job.status, 201,
                "case {case}: expected HTTP 201, got {} with body {}",
                job.status, job.body
            );

            // What the job states, else what the template carries, else nothing at all.
            let expected = if on_job.is_null() { on_template } else { on_job };
            let id = job.string("/job/id");
            let fetched = emulator.get_job(&id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: the job must read back, got {} with body {}",
                fetched.status, fetched.body
            );
            for reply in [&job, &fetched] {
                assert_eq!(
                    reply.at("/job/elementalInferenceSettings"),
                    expected,
                    "case {case}: expected {expected}, got {}",
                    reply.body
                );
            }
        }
    }
}

