// SPDX-License-Identifier: CC0-1.0

//! WHICH objects a submitted S3 Batch Operations job's tasks act on, driven end
//! to end over HTTP against Amazon S3 and S3 Control on one shared engine.
//!
//! The rule under test: a job "performs a specified operation on every object
//! listed in the manifest" (`CreateJob`), and each object is the one its own row
//! ADDRESSES — the row's `Bucket` and `Key` values, read at the positions
//! `JobManifestSpec$Fields` describes ("if the specified manifest object is in
//! the `S3BatchOperations_CSV_20180820` format, this element describes which
//! columns contain the required data") and at the `Bucket`, `Key` columns when
//! the spec describes none, in the CSV form that format defines. So the manifest
//! object itself is addressed only by its own `JobManifestLocation$ObjectArn`
//! and may live in any bucket, one manifest may list objects of SEVERAL buckets,
//! and a key the CSV format has to quote names the object it is rather than a
//! different one. The job then settles `Complete` — its tasks ran, however many
//! of them failed — with `ProgressSummary` accounting for every row, every
//! addressed object carrying exactly the job's tag set, and every object no row
//! addressed left as it was.
//!
//! The rows span what the rule generalises over: how many buckets the manifest's
//! rows name and whether the manifest object is in one of them, which columns the
//! spec describes and in which order (including an `Ignore` column, a `VersionId`
//! column left empty, and a spec describing no columns at all), the form of a key
//! (one the CSV format has to quote, one it does not), how many rows the manifest
//! carries, and whether a row addresses an object its bucket does not hold.
//!
//! The manifest is written with the `csv` crate, which is what the format's
//! quoting is, and every object read back is one the object store itself holds,
//! tagged by the job's own run.

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::JOB_STATUS_COMPLETE;

use crate::access_point_lifecycle::{xml_escape, REGION, XMLNS};
use crate::batch_job_execution::{poll_terminal, progress};
use crate::object_store::ObjectStore;

/// The role every job below runs as: a well-formed `IAMRoleArn` this store's IAM
/// holds nothing under, which is evidence about the store rather than about the
/// role, so such a job runs (`crate::batch_job_role_trust`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchOperations";

/// The tag set every job below puts on each object its manifest addresses.
const TAG_SET: [(&str, &str); 2] = [("sim2real", "addressed"), ("stage", "tagged")];

/// The key the manifest object of every row is written at.
const MANIFEST_KEY: &str = "manifest/manifest.csv";

/// The `JobManifestFieldName` values a CSV manifest's columns are described
/// with. Source: the model's own enum, whose values are `Ignore`, `Bucket`,
/// `Key` and `VersionId`
/// (`smithy_query(query="shape", shape="JobManifestFieldName")`).
const FIELD_IGNORE: &str = "Ignore";
/// See [`FIELD_IGNORE`].
const FIELD_BUCKET: &str = "Bucket";
/// See [`FIELD_IGNORE`].
const FIELD_KEY: &str = "Key";
/// See [`FIELD_IGNORE`].
const FIELD_VERSION_ID: &str = "VersionId";

/// The columns a CSV manifest carries when its `Spec` describes none: the two
/// a task needs to address an object, in the order the model lists them in
/// `JobManifestFieldName`. Written out here rather than read from the service,
/// so the row below pins the default rather than restating it.
const DEFAULT_COLUMNS: [&str; 2] = [FIELD_BUCKET, FIELD_KEY];

/// The `JobManifestFormat` a manifest written FOR a batch job carries. Source:
/// the model's own enum
/// (`smithy_query(query="shape", shape="JobManifestFormat")`).
const MANIFEST_FORMAT_CSV: &str = "S3BatchOperations_CSV_20180820";

/// What an `Ignore` column of a manifest row carries: anything, since no task
/// reads it.
const IGNORED_VALUE: &str = "2026-01-01T00:00:00Z";

/// One object a row writes into the store: which of the row's buckets holds it,
/// and under which key.
type Object = (usize, &'static str);

/// One manifest the rule is asserted over.
struct Row {
    case: &'static str,
    /// How many object buckets the row creates. The manifest object itself goes
    /// into a bucket of its own, so a row naming one bucket already proves the
    /// manifest is addressed apart from the objects it lists.
    buckets: usize,
    /// Every object written into those buckets, before any job runs.
    written: &'static [Object],
    /// The objects the manifest's rows address, in manifest order. An entry no
    /// object was written at is a row whose task fails.
    listed: &'static [Object],
    /// The columns the manifest's `Spec` describes, or empty for a spec that
    /// describes none.
    fields: &'static [&'static str],
}

/// The manifest table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one bucket, one row, the columns the spec describes",
            buckets: 1,
            written: &[(0, "data/target.txt"), (0, "data/other.txt")],
            listed: &[(0, "data/target.txt")],
            fields: &[FIELD_BUCKET, FIELD_KEY],
        },
        Row {
            case: "three rows over two buckets, the manifest in neither",
            buckets: 2,
            written: &[
                (0, "data/a.txt"),
                (0, "data/kept.txt"),
                (1, "data/b.txt"),
                (1, "nested/deep/c.txt"),
            ],
            listed: &[(0, "data/a.txt"), (1, "data/b.txt"), (1, "nested/deep/c.txt")],
            fields: &[FIELD_BUCKET, FIELD_KEY],
        },
        Row {
            case: "a spec that describes no columns at all",
            buckets: 1,
            written: &[(0, "top.txt"), (0, "untouched.txt")],
            listed: &[(0, "top.txt")],
            fields: &[],
        },
        Row {
            case: "the columns in another order, behind an Ignore column",
            buckets: 1,
            written: &[(0, "reordered/one.txt"), (0, "reordered/two.txt")],
            listed: &[(0, "reordered/one.txt"), (0, "reordered/two.txt")],
            fields: &[FIELD_IGNORE, FIELD_KEY, FIELD_BUCKET],
        },
        Row {
            case: "a VersionId column left empty, which addresses the current version",
            buckets: 1,
            written: &[(0, "versioned/current.txt")],
            listed: &[(0, "versioned/current.txt")],
            fields: &[FIELD_BUCKET, FIELD_KEY, FIELD_VERSION_ID],
        },
        Row {
            case: "keys the CSV format has to quote",
            buckets: 1,
            written: &[
                (0, "data/with,comma.txt"),
                (0, "data/with,comma,twice.txt"),
                (0, "data/plain.txt"),
            ],
            listed: &[(0, "data/with,comma.txt"), (0, "data/with,comma,twice.txt")],
            fields: &[FIELD_BUCKET, FIELD_KEY],
        },
        Row {
            case: "a row addressing an object its bucket does not hold",
            buckets: 2,
            written: &[(0, "present.csv"), (1, "also-present.csv")],
            listed: &[
                (0, "present.csv"),
                (1, "absent.csv"),
                (1, "also-present.csv"),
            ],
            fields: &[FIELD_BUCKET, FIELD_KEY],
        },
    ]
}

impl Row {
    /// The columns the row's manifest is written with: the ones its spec
    /// describes, or the two a spec describing none leaves a reader with.
    fn columns(&self) -> Vec<&'static str> {
        if self.fields.is_empty() {
            DEFAULT_COLUMNS.to_vec()
        } else {
            self.fields.to_vec()
        }
    }

    /// The `<Fields>` element of the row's `Spec`, left out entirely by a row
    /// whose spec describes no columns.
    fn fields_xml(&self) -> String {
        if self.fields.is_empty() {
            return String::new();
        }
        let members: String = self
            .fields
            .iter()
            .map(|field| format!("<member>{field}</member>"))
            .collect();
        format!("<Fields>{members}</Fields>")
    }

    /// Whether `object` is one the row wrote, which is what decides whether the
    /// task addressing it can be carried out.
    fn holds(&self, object: &Object) -> bool {
        self.written.contains(object)
    }

    /// The (total, succeeded, failed) task counts the settled job must account
    /// for: one task per manifest row, carried out exactly where the row
    /// addresses an object the store holds.
    fn progress(&self) -> (i64, i64, i64) {
        let total = self.listed.len() as i64;
        let succeeded = self
            .listed
            .iter()
            .filter(|object| self.holds(object))
            .count() as i64;
        (total, succeeded, total - succeeded)
    }
}

/// The row's manifest as CSV: one record per listed object, each carrying the
/// row's columns in the order its spec describes them. Written with the `csv`
/// crate, so a key the format has to quote is quoted the way the format says.
fn manifest_csv(row: &Row, buckets: &[String]) -> Vec<u8> {
    let mut writer = ::csv::WriterBuilder::new().from_writer(Vec::new());
    for (bucket_at, key) in row.listed {
        let record: Vec<String> = row
            .columns()
            .iter()
            .map(|column| match *column {
                FIELD_BUCKET => buckets[*bucket_at].clone(),
                FIELD_KEY => (*key).to_string(),
                // The version a row leaves empty names no version, which leaves
                // the task acting on the object's current version.
                FIELD_VERSION_ID => String::new(),
                _ => IGNORED_VALUE.to_string(),
            })
            .collect();
        writer
            .write_record(&record)
            .expect("writing a manifest record to a Vec cannot fail");
    }
    writer
        .into_inner()
        .expect("the manifest writer hands back its buffer")
}

/// The `CreateJob` body: one `S3PutObjectTagging` action over the row's
/// manifest, asking for no completion report.
fn create_job_body(row: &Row, token: &str, manifest_bucket: &str, e_tag: &str) -> String {
    let tags: String = TAG_SET
        .iter()
        .map(|(key, value)| {
            format!(
                "<member><Key>{}</Key><Value>{}</Value></member>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect();
    format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\">\
         <ClientRequestToken>{}</ClientRequestToken>\
         <ConfirmationRequired>false</ConfirmationRequired>\
         <Operation><S3PutObjectTagging><TagSet>{tags}</TagSet></S3PutObjectTagging></Operation>\
         <Report><Enabled>false</Enabled></Report>\
         <Manifest><Spec><Format>{MANIFEST_FORMAT_CSV}</Format>{}</Spec>\
         <Location><ObjectArn>arn:aws:s3:::{manifest_bucket}/{MANIFEST_KEY}</ObjectArn>\
         <ETag>{}</ETag></Location></Manifest>\
         <Priority>1</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>",
        xml_escape(token),
        row.fields_xml(),
        xml_escape(e_tag)
    )
}

/// The job's tag set as (key, value) pairs, sorted, so a comparison is about
/// WHICH tags an object carries rather than the order one answer listed them in.
fn job_tag_set() -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = TAG_SET
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// RULE: a job's tasks act on the objects its manifest's rows address — each
/// row's own `Bucket` and `Key`, at the positions the spec describes — so the
/// manifest may live anywhere and list objects of several buckets, and the
/// settled job accounts for every row while leaving every unaddressed object as
/// it was.
#[tokio::test]
async fn a_job_acts_on_exactly_the_objects_its_manifest_rows_address() {
    for (row_at, row) in rows().iter().enumerate() {
        let case = row.case;
        let store = ObjectStore::start();

        // The manifest object lives in a bucket of its own: it is addressed by
        // its ObjectArn, not by being beside the objects the job acts on.
        let manifest_bucket = format!("batch-addressing-{row_at}-manifest");
        store.create_bucket(case, REGION, &manifest_bucket).await;
        let buckets: Vec<String> = (0..row.buckets)
            .map(|at| format!("batch-addressing-{row_at}-objects-{at}"))
            .collect();
        for bucket in &buckets {
            store.create_bucket(case, REGION, bucket).await;
        }
        for (bucket_at, key) in row.written {
            store
                .put_object(case, REGION, &buckets[*bucket_at], key, b"to-be-tagged")
                .await;
        }

        let manifest = manifest_csv(row, &buckets);
        let e_tag = store
            .put_object(case, REGION, &manifest_bucket, MANIFEST_KEY, &manifest)
            .await;

        let created = store
            .s3control(
                REGION,
                "POST",
                "/v20180820/jobs",
                &create_job_body(
                    row,
                    &format!("addressing-{row_at}"),
                    &manifest_bucket,
                    &e_tag,
                ),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateJob to answer 200, got {} with body {}",
            created.status, created.body
        );
        let job_id = created.text("JobId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CreateJob to answer a JobId, got body {}",
                created.body
            )
        });

        // Nothing but DescribeJob from here: the job settles on its own.
        let settled = poll_terminal(&store, case, &job_id).await;
        let status = settled.text("Status").unwrap_or_default();
        assert_eq!(
            status, JOB_STATUS_COMPLETE,
            "case {case:?}: expected a job whose tasks ran to settle {JOB_STATUS_COMPLETE}, got \
             {status:?} with body {}",
            settled.body
        );
        assert_eq!(
            progress(case, &settled),
            row.progress(),
            "case {case:?}: expected the settled job to account for {:?} tasks, got body {}",
            row.progress(),
            settled.body
        );

        // Every object a row addressed carries exactly the job's tag set, in
        // whichever bucket that row named; every object no row addressed is
        // left as it was written.
        for (bucket_at, key) in row.written {
            let bucket = &buckets[*bucket_at];
            let addressed = row.listed.contains(&(*bucket_at, *key));
            let mut found = store.object_tags(REGION, bucket, key).await;
            found.sort();
            let expected = if addressed {
                job_tag_set()
            } else {
                Vec::new()
            };
            assert_eq!(
                found, expected,
                "case {case:?}: expected {bucket}/{key} to carry {expected:?}, got {found:?}"
            );
        }

        // The manifest object itself is read, never acted on: no row addresses
        // it, so the job leaves it untagged.
        let manifest_tags = store
            .object_tags(REGION, &manifest_bucket, MANIFEST_KEY)
            .await;
        assert!(
            manifest_tags.is_empty(),
            "case {case:?}: expected the manifest object to be left untagged, got \
             {manifest_tags:?}"
        );
    }
}
