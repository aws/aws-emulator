// SPDX-License-Identifier: CC0-1.0

//! What a deployed model ANSWERS for a row of its own training data.
//!
//! RULE: a training job over a labelled tabular channel fits the model by MINIMISING the
//! squared error of its predictions against the labels those records carry, so the fit it
//! writes answers a row of that same training data with a value in the labels' own scale —
//! near that row's label — whatever the records' width, the units their columns are written
//! in or how strongly those columns move together. A fit whose prediction for a row it was
//! trained on is orders of magnitude away from that row's label has not minimised anything:
//! it has diverged, which is what a step chosen without reference to the curvature of the
//! data it steps over does as soon as the columns are correlated enough.
//!
//! The rows span what the rule generalises over: how many feature columns the records carry
//! (one, a few, many), how strongly those columns move together (independent, perfectly
//! collinear, one constant column that carries no information), the units and magnitudes the
//! columns are written in (units, thousands, fractions), and the sign and size of the labels
//! (small, large, negative). Each case invokes the endpoint with the feature columns of
//! several of its OWN training rows, the first and the last among them.
//!
//! Only a cross-service suite can ask this: the flow spans IAM (the role the run reads and
//! writes under), Amazon S3 (the training data and the artifact), the SageMaker control plane
//! (train and deploy) and the SageMaker runtime (invoke), mounted on ONE engine the way
//! `core/router/src/lib.rs` mounts them. The harness is the one
//! [`crate::sagemaker_endpoint_serves_a_trained_model`] stands up, so this suite drives the
//! same router and the same deploy path as its sibling.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sagemaker_endpoint_serves_a_trained_model::{
    control_ok, deploy, emulator, given_bucket, given_object, given_role, image, invoke, settled,
    BUCKET,
};

/// One labelled record: its label and its feature columns.
struct Record {
    label: f64,
    features: Vec<f64>,
}

impl Record {
    /// The record as one `text/csv` row of a training channel: "the first column is the label"
    /// and the rest are the features.
    fn row(&self) -> String {
        let mut fields = vec![format!("{}", self.label)];
        fields.extend(self.features.iter().map(|value| format!("{value}")));
        fields.join(",")
    }

    /// The FEATURE columns alone, which is what an invocation sends.
    fn payload(&self) -> String {
        self.features
            .iter()
            .map(|value| format!("{value}"))
            .collect::<Vec<String>>()
            .join(",")
    }
}

/// One dataset: how its feature columns are built from the record's position, and the label
/// the record carries for them.
struct Dataset {
    case: &'static str,
    /// The feature columns of the record at `index`.
    columns: fn(usize) -> Vec<f64>,
    /// The label of a record carrying `features`.
    label: fn(&[f64]) -> f64,
    rows: usize,
}

impl Dataset {
    fn records(&self) -> Vec<Record> {
        (0..self.rows)
            .map(|index| {
                let features = (self.columns)(index);
                Record {
                    label: (self.label)(&features),
                    features,
                }
            })
            .collect()
    }
}

/// The datasets the rule is asserted over. Each label is a function of the record's own
/// columns, so every record is one a fit that minimised the squared error answers in the
/// label's scale.
fn datasets() -> Vec<Dataset> {
    vec![
        Dataset {
            case: "one feature column",
            columns: |index| vec![index as f64],
            label: |features| 2.0 * features[0] + 1.0,
            rows: 24,
        },
        Dataset {
            case: "three independent columns of different units",
            columns: |index| {
                let step = index as f64;
                vec![step, 1_000.0 - 7.0 * step, (step * 0.03) % 1.0]
            },
            label: |features| 3.0 + features[0] - 0.5 * features[1] + 40.0 * features[2],
            rows: 32,
        },
        Dataset {
            // The shape that makes a fixed step diverge: every column moves with every other,
            // so the curvature along the gradient grows with the column count.
            case: "twelve perfectly collinear columns",
            columns: |index| {
                (0..12)
                    .map(|column| (index + column + 1) as f64)
                    .collect()
            },
            label: |features| 3.0 + features.iter().sum::<f64>(),
            rows: 40,
        },
        Dataset {
            case: "a constant column beside a varying one",
            columns: |index| vec![5.0, (index % 7) as f64],
            label: |features| 10.0 * features[1] - 4.0,
            rows: 21,
        },
        Dataset {
            case: "columns in the thousands with a large label",
            columns: |index| {
                let step = index as f64;
                vec![2_000.0 + 13.0 * step, 7_500.0 - 11.0 * step]
            },
            label: |features| 0.25 * features[0] + 0.1 * features[1],
            rows: 30,
        },
        Dataset {
            case: "fractional columns with negative labels",
            columns: |index| {
                let step = index as f64;
                vec![0.01 * step, 0.5 - 0.02 * step, 0.25]
            },
            label: |features| -20.0 - 30.0 * features[0] + 8.0 * features[1],
            rows: 28,
        },
    ]
}

/// How near the prediction for a training row has to be to that row's label: within half the
/// label's own magnitude, with a floor so a label near zero still has room. The point of the
/// bound is the ORDER of the answer — a prediction in the label's scale rather than one
/// anywhere else — not a particular number.
fn tolerance(label: f64) -> f64 {
    (label.abs() / 2.0).max(5.0)
}

/// Train a job named `name` on `records` and answer the artifact it wrote.
async fn trained(router: &MultiServiceRouter, case: &str, name: &str, records: &[Record]) -> String {
    let role_arn = given_role(router).await;
    let key = format!("{name}/train.csv");
    let csv: String = records
        .iter()
        .map(|record| format!("{}\n", record.row()))
        .collect();
    given_object(router, &key, &csv).await;
    control_ok(
        router,
        "CreateTrainingJob",
        &json!({
            "TrainingJobName": name,
            "AlgorithmSpecification": {
                "TrainingImage": image(),
                "TrainingInputMode": "File",
            },
            // The learning task of the recorded flow: a squared-error regression, which
            // answers the fitted score itself.
            "HyperParameters": { "objective": "reg:linear", "num_round": "5" },
            "RoleArn": role_arn,
            "InputDataConfig": [{
                "ChannelName": "train",
                "DataSource": {
                    "S3DataSource": {
                        "S3DataType": "S3Prefix",
                        "S3Uri": format!("s3://{BUCKET}/{key}"),
                    },
                },
                "ContentType": "text/csv",
            }],
            "OutputDataConfig": { "S3OutputPath": format!("s3://{BUCKET}/{name}/out/") },
            "ResourceConfig": {
                "InstanceType": "ml.m5.large",
                "InstanceCount": 1,
                "VolumeSizeInGB": 10,
            },
            "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
        }),
    )
    .await;
    let completed = settled(
        router,
        case,
        "DescribeTrainingJob",
        &json!({ "TrainingJobName": name }),
        "TrainingJobStatus",
        "Completed",
    )
    .await;
    completed
        .pointer("/ModelArtifacts/S3ModelArtifacts")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the completed job names no artifact: {completed}"))
        .to_string()
}

/// THE RULE: the endpoint's answer for a training row is near that row's label.
#[tokio::test]
async fn a_deployed_fit_answers_a_training_row_near_its_label() {
    for (index, dataset) in datasets().into_iter().enumerate() {
        let case = dataset.case;
        let records = dataset.records();
        // One emulator per case, so no case reads another's bucket, role or endpoint.
        let router = emulator();
        given_bucket(&router).await;
        let name = format!("fitted-{index}");
        let endpoint = format!("{name}-endpoint");
        let artifact = trained(&router, case, &name, &records).await;
        deploy(
            &router,
            case,
            &name,
            &endpoint,
            &format!("{name}-variant"),
            Some(&artifact),
        )
        .await;

        // The first record, the last and one from the middle: a fit that minimised the error
        // over the whole channel answers each of them in its own label's scale.
        let probed = [0, records.len() / 2, records.len() - 1];
        for position in probed {
            let record = &records[position];
            let answer = invoke(
                &router,
                &endpoint,
                &[("content-type", "text/csv")],
                record.payload().as_bytes(),
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?} record {position}: the endpoint must serve the row, got {} {}",
                answer.status,
                answer.text()
            );
            let body = answer.text();
            let predicted: f64 = body.trim().parse().unwrap_or_else(|_| {
                panic!("case {case:?} record {position}: expected a number, got {body:?}")
            });
            let allowed = tolerance(record.label);
            assert!(
                (predicted - record.label).abs() <= allowed,
                "case {case:?} record {position}: expected a prediction within {allowed} of \
                 the label {}, got {predicted} (body {body:?})",
                record.label
            );
        }
    }
}
