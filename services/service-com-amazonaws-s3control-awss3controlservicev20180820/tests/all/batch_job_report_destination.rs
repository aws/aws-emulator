// SPDX-License-Identifier: CC0-1.0

//! WHERE a settled S3 Batch Operations job leaves its completion report, driven
//! end to end over HTTP against IAM, Amazon S3 and S3 Control on one shared
//! engine, exactly as a client drives them.
//!
//! The rule under test: a job created without a confirmation settles on its own
//! at a terminal status a `DescribeJob` poll observes — `Complete` once its one
//! action has been carried out, as the role the create named, against every
//! object of its object list — carrying the `TerminationDate` the model
//! documents as "a timestamp indicating when this job terminated" and a
//! `ProgressSummary` accounting for every row of that list; and the completion
//! report it asked for is an object of the bucket `JobReport$Bucket` names
//! ("the bucket where specified job-completion report will be stored"), under
//! `<Prefix>/job-<JobId>/` ("an optional prefix to describe where in the
//! specified bucket the job-completion report will be stored",
//! `JobReport$Prefix`) — WHICHEVER bucket that is, including the very bucket
//! that holds the job's manifest object and its target objects. Only the
//! objects the manifest LISTS are acted on, so the manifest object itself and
//! the report's own objects keep the (empty) tag set they had, and every object
//! the bucket held before the job still answers afterwards.
//!
//! The rows span what the rule generalises over: whether the report's bucket is
//! the job's own bucket or another one, the prefix the report is asked for under
//! (none, one segment, several), how deep in the bucket the manifest object
//! lives (at the root, under one segment, under several), how many objects the
//! manifest lists and how many tags the action carries.
//!
//! Every role here is created through IAM's own `CreateRole`, and every tag and
//! report object read back is one the job's own run wrote, read through Amazon
//! S3's own operations.

use serde_json::json;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::JOB_STATUS_COMPLETE;

use crate::access_point_lifecycle::{xml_escape, REGION, XMLNS};
use crate::batch_job_execution::{csv_manifest, poll_terminal, progress};
use crate::object_store::ObjectStore;

/// The service principal S3 Batch Operations assumes a job's role as, which
/// every role below grants `sts:AssumeRole` to, so every job here runs.
const BATCH_OPERATIONS_PRINCIPAL: &str = "batchoperations.s3.amazonaws.com";

/// The key the report's own manifest is stored at under the job's prefix: the
/// one fixed key a reader of a completion report starts from.
const REPORT_MANIFEST_NAME: &str = "manifest.json";

/// One job the rule is asserted over.
struct Row {
    case: &'static str,
    /// The role name, unique within the row's own IAM store.
    role_name: &'static str,
    /// Where the job's manifest object is written in the job's bucket.
    manifest_key: &'static str,
    /// The keys the manifest lists, in manifest order; each is written into the
    /// job's bucket untagged first.
    listed: &'static [&'static str],
    /// The tag set the job's `S3PutObjectTagging` action carries.
    tag_set: &'static [(&'static str, &'static str)],
    /// Whether `JobReport$Bucket` names the job's OWN bucket rather than a
    /// second one.
    report_in_job_bucket: bool,
    /// `JobReport$Prefix`, or `None` for a report asked for without one.
    prefix: Option<&'static str>,
}

/// The job table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "report into the job's own bucket under one prefix segment, manifest under one",
            role_name: "batch-report-same-bucket",
            manifest_key: "manifest/manifest.csv",
            listed: &["objects/target.txt"],
            tag_set: &[("batch", "true")],
            report_in_job_bucket: true,
            prefix: Some("reports"),
        },
        Row {
            case: "report into the job's own bucket with no prefix, manifest at the bucket root",
            role_name: "batch-report-same-bucket-root",
            manifest_key: "manifest.csv",
            listed: &["data/a.txt", "data/b.txt"],
            tag_set: &[("stage", "done"), ("owner", "ops")],
            report_in_job_bucket: true,
            prefix: None,
        },
        Row {
            case: "report into a second bucket under a nested prefix, manifest nested too",
            role_name: "batch-report-other-bucket",
            manifest_key: "meta/jobs/2026/manifest.csv",
            listed: &["logs/a.log", "logs/b.log", "logs/c.log"],
            tag_set: &[("retention", "30d")],
            report_in_job_bucket: false,
            prefix: Some("audit/2026/reports/"),
        },
    ]
}

/// The row's tag set as the `<TagSet>` body of an `S3PutObjectTagging` action.
fn tag_set_xml(tags: &[(&str, &str)]) -> String {
    let members: String = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "<member><Key>{}</Key><Value>{}</Value></member>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect();
    format!("<TagSet>{members}</TagSet>")
}

/// `(key, value)` pairs, sorted, so a comparison is about WHICH tags an object
/// carries rather than the order one answer happened to list them in.
fn sorted(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// Where everything one job writes about itself goes: `<Prefix>/job-<JobId>`,
/// with the prefix left out where the create named none.
fn job_prefix(prefix: Option<&str>, job_id: &str) -> String {
    match prefix.map(|prefix| prefix.trim_end_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/job-{job_id}"),
        _ => format!("job-{job_id}"),
    }
}

/// RULE: a job created without a confirmation settles `Complete` with a
/// `TerminationDate` and a `ProgressSummary` accounting for every row of its
/// object list, tags exactly the objects that list names, and leaves its
/// completion report under `<Prefix>/job-<JobId>/` in the bucket the report
/// names — whichever bucket that is, the job's own included.
#[tokio::test]
async fn a_settled_job_leaves_its_report_in_the_bucket_the_report_names() {
    for (row_at, row) in rows().iter().enumerate() {
        let case = row.case;
        let (store, iam) = ObjectStore::with_iam();
        let bucket = format!("batch-report-dest-{row_at}");
        store.create_bucket(case, REGION, &bucket).await;
        let reports = if row.report_in_job_bucket {
            bucket.clone()
        } else {
            let reports = format!("batch-report-dest-elsewhere-{row_at}");
            store.create_bucket(case, REGION, &reports).await;
            reports
        };

        for key in row.listed {
            store
                .put_object(case, REGION, &bucket, key, b"to-be-tagged")
                .await;
        }
        let manifest_body: String = row
            .listed
            .iter()
            .map(|key| format!("{bucket},{key}\n"))
            .collect();
        let e_tag = store
            .put_object(
                case,
                REGION,
                &bucket,
                row.manifest_key,
                manifest_body.as_bytes(),
            )
            .await;

        // The role exists in IAM, which is the authority on who may assume it,
        // and its trust document grants the batch-operations principal the
        // assume the job's run needs.
        let role_arn = iam
            .create_role()
            .role_name(row.role_name)
            .assume_role_policy_document(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": BATCH_OPERATIONS_PRINCIPAL},
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: IAM must create {}: {error:?}", row.role_name)
            })
            .role()
            .map(|role| role.arn().to_string())
            .unwrap_or_else(|| panic!("case {case:?}: IAM must report an ARN for the role"));

        let report_prefix = match row.prefix {
            Some(prefix) => format!("<Prefix>{}</Prefix>", xml_escape(prefix)),
            None => String::new(),
        };
        let body = format!(
            "<CreateJobRequest xmlns=\"{XMLNS}\">\
             <ClientRequestToken>report-dest-{row_at}</ClientRequestToken>\
             <ConfirmationRequired>false</ConfirmationRequired>\
             <Operation><S3PutObjectTagging>{}</S3PutObjectTagging></Operation>\
             <Report><Bucket>arn:aws:s3:::{reports}</Bucket>\
             <Format>Report_CSV_20180820</Format><Enabled>true</Enabled>{report_prefix}\
             <ReportScope>AllTasks</ReportScope></Report>{}\
             <Priority>1</Priority><RoleArn>{}</RoleArn></CreateJobRequest>",
            tag_set_xml(row.tag_set),
            csv_manifest(&bucket, row.manifest_key, &e_tag),
            xml_escape(&role_arn)
        );
        let created = store
            .s3control(REGION, "POST", "/v20180820/jobs", &body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
            created.status, created.body
        );
        let job_id = created.text("JobId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateJob answers a JobId, got body {}",
                created.body
            )
        });

        // Nothing but DescribeJob is called from here: the job settles on its
        // own, and a terminal status is what a client waits for.
        let settled = poll_terminal(&store, case, &job_id).await;
        let status = settled.text("Status").unwrap_or_default();
        assert_eq!(
            status, JOB_STATUS_COMPLETE,
            "case {case:?}: expected the job to settle {JOB_STATUS_COMPLETE}, got {status:?} with \
             body {}",
            settled.body
        );
        assert!(
            settled
                .text("TerminationDate")
                .is_some_and(|date| !date.is_empty()),
            "case {case:?}: expected a terminated job to report when it terminated, got body {}",
            settled.body
        );
        let listed = i64::try_from(row.listed.len()).expect("the row count fits");
        assert_eq!(
            progress(case, &settled),
            (listed, listed, 0),
            "case {case:?}: expected every manifest row accounted for as a task that succeeded, \
             got body {}",
            settled.body
        );
        let reasons = settled.texts("FailureReason");
        assert!(
            reasons.is_empty(),
            "case {case:?}: expected no FailureReasons on a job that ran, got {reasons:?}"
        );

        // Every listed object carries exactly the job's tag set.
        let expected = sorted(row.tag_set);
        for key in row.listed {
            let mut found = store.object_tags(REGION, &bucket, key).await;
            found.sort();
            assert_eq!(
                found, expected,
                "case {case:?}: expected {key} to carry {expected:?}, got {found:?}"
            );
        }

        // The manifest object is not an object of the list, so nothing was done
        // to it: it still answers with the bytes it was written with, and
        // carries no tags.
        let (status, manifest_bytes) = store.object(REGION, &bucket, row.manifest_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the manifest object at {} to survive the job, got {status}",
            row.manifest_key
        );
        assert_eq!(
            String::from_utf8_lossy(&manifest_bytes),
            manifest_body,
            "case {case:?}: expected the manifest object to be left byte for byte"
        );
        let manifest_tags = store.object_tags(REGION, &bucket, row.manifest_key).await;
        assert!(
            manifest_tags.is_empty(),
            "case {case:?}: expected the manifest object, which the list does not name, to carry \
             no tags, got {manifest_tags:?}"
        );

        // The report is under `<Prefix>/job-<JobId>/` in the bucket the report
        // named, whichever bucket that is.
        let prefix = job_prefix(row.prefix, &job_id);
        let written = store
            .keys_under(REGION, &reports, &format!("{prefix}/"))
            .await;
        let manifest_key = format!("{prefix}/{REPORT_MANIFEST_NAME}");
        assert!(
            written.contains(&manifest_key),
            "case {case:?}: expected the report's manifest at {manifest_key} in {reports}, got \
             {written:?}"
        );

        let (status, report_manifest) = store.object(REGION, &reports, &manifest_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the report manifest at {manifest_key} to be readable, got \
             {status}"
        );
        let report_manifest: serde_json::Value = serde_json::from_slice(&report_manifest)
            .unwrap_or_else(|error| panic!("case {case:?}: the report manifest is JSON: {error}"));
        let results_key = report_manifest["Results"][0]["Key"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the report manifest names its result file: {report_manifest}"
                )
            })
            .to_string();
        assert!(
            results_key.starts_with(&format!("{prefix}/")),
            "case {case:?}: expected the result file under {prefix}/, got {results_key}"
        );

        // The result file accounts for exactly the tasks that ran — one row per
        // object of the list, in task order, each naming its object.
        let (status, results) = store.object(REGION, &reports, &results_key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the result file at {results_key} to be readable, got {status}"
        );
        let results = String::from_utf8_lossy(&results).into_owned();
        let accounted: Vec<&str> = results.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(
            accounted.len(),
            row.listed.len(),
            "case {case:?}: expected {} row(s) in a report over AllTasks, got {accounted:?}",
            row.listed.len()
        );
        for (key, reported) in row.listed.iter().zip(accounted.iter()) {
            assert!(
                reported.contains(key),
                "case {case:?}: expected the report row to name {key}, got {reported:?}"
            );
        }

        // The report's own objects are not objects of the list either, so the
        // job's tag set is nowhere on them — which is the whole difference a
        // report written into the job's own bucket could have blurred.
        for key in &written {
            let tags = store.object_tags(REGION, &reports, key).await;
            assert!(
                tags.is_empty(),
                "case {case:?}: expected the report object {key} to carry no tags, got {tags:?}"
            );
        }

        // And nothing the bucket held before the job is gone.
        let held = store.keys_under(REGION, &bucket, "").await;
        for key in row.listed.iter().chain([&row.manifest_key]) {
            assert!(
                held.contains(&(*key).to_string()),
                "case {case:?}: expected {bucket} to still hold {key}, got {held:?}"
            );
        }
    }
}
