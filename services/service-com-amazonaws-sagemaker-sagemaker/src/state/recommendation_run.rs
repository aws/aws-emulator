// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE RUN of an accepted Inference Recommender job: the endpoint it benchmarks,
//! the traffic it sends to the model behind it, and what it MEASURED — the recommendation
//! `GetScalingConfigurationRecommendation` then computes a scaling configuration from.
//!
//! # What a recommendation job DOES
//!
//! "Starts a recommendation job. You can create either an instance recommendation or load
//! test job" (`CreateInferenceRecommendationsJob`), and `RecommendationMetrics` is what it
//! answers with: `MaxInvocations`, "the expected maximum number of requests per minute for
//! the instance", and `ModelLatency`, "the expected model latency at maximum invocation per
//! minute for the instance". So the job's work is: get the model under test serving, send it
//! traffic, and report what that traffic measured.
//!
//! This module performs it IN PROCESS, against the SAME serving path an `InvokeEndpoint`
//! takes ([`super::model_serving`]): the model behind the endpoint under test is loaded out of
//! this emulator's own Amazon S3 and really applied to the records the benchmark sends, and
//! the numbers reported are measured off those invocations — how long they took and how many
//! of them one instance performed in that time. Nothing is synthesised: a test whose
//! invocations measured nothing reports NO metrics rather than numbers no invocation produced,
//! and a job that had nothing to put under test at all — it names no endpoint and no model
//! this account holds to deploy one for — ends in the modelled `FAILED` status carrying the
//! condition that stopped it rather than in `COMPLETED` with numbers no invocation produced.
//!
//! # A model under test whose container brings its own
//!
//! `ModelDataUrl` and `ModelDataSource` are optional on a container definition, so a container
//! that loads its own model declares neither and the model it serves is the container IMAGE's
//! own. That is a valid model and a valid registered package, and AWS benchmarks it like any
//! other, so the job is NOT failed for want of artifacts. What this emulator cannot do is run
//! the image: nothing here loads that model, so such a test measures nothing, and the
//! recommendation it reports carries the endpoint it was planned on and no metrics
//! ([`unmeasured_recommendation`]).
//!
//! # Which endpoints are benchmarked
//!
//! `RecommendationJobInputConfig$Endpoints` is "existing customer endpoints on which to run an
//! Inference Recommender job", so a job that names one benchmarks THAT endpoint, through its
//! own production variant. A job that names none has SageMaker deploy an endpoint of its own
//! for each `EndpointConfigurations` entry under test — "specifies the endpoint configuration
//! to use for a job" — and take it down when the job ends, so the name such a recommendation
//! reports is the name of an endpoint that is no longer there; the model under test is then the
//! one the job's input configuration names, either by `ModelName` ("the name of the created
//! model") or by `ModelPackageVersionArn` ("the Amazon Resource Name (ARN) of a versioned model
//! package"), whose artifact is the one the named inference specification of that package
//! carries ([`model_under_test`]). How many configurations are tested is
//! `RecommendationJobResourceLimit$MaxNumberOfTests`, "defines the maximum number of load
//! tests", bounded by [`MAX_BENCHMARKED_CONFIGURATIONS`].
//!
//! # Which traffic is sent
//!
//! A load test generates its own traffic. Where the job names a sample payload —
//! `RecommendationJobPayloadConfig$SamplePayloadUrl`, "the Amazon Simple Storage Service (S3)
//! path where the sample payload is stored", a gzip-compressed tar archive — the records of
//! that payload are the traffic, read out of Amazon S3 under the job's own role. Where it
//! names none, the generator sends a record of the width the model under test takes. Either
//! way the model computes a real prediction per invocation, which is what the measurement is
//! taken over.
//!
//! # Nothing here leaves this process
//!
//! Every read of an object goes through this service's ONE Amazon S3 client
//! ([`super::s3_access::s3_client`]), which is built over the emulator's own in-process
//! callback into its own Amazon S3: no socket is opened, no host is resolved and no address
//! from the request is dialled. The "invocation" is a function call in this process — the same
//! [`serve_inference`] an `InvokeEndpoint` performs — so the endpoint a job names selects which
//! STORED model is applied to the traffic, never a destination to connect to. Every ceiling the
//! run works under is this emulator's and is stated here: how many endpoints one job
//! benchmarks ([`MAX_BENCHMARKED_CONFIGURATIONS`]), how many invocations one test performs
//! ([`BENCHMARK_INVOCATIONS`]), how many records a sample payload contributes
//! ([`MAX_SAMPLE_PAYLOADS`]), and the bytes a download and its inflated archive may carry
//! ([`MAX_OBJECT_BYTES`], [`MAX_INFLATED_ARCHIVE_BYTES`]). The measuring itself is synchronous,
//! CPU-bound work and runs on the blocking pool, so the worker serving the request is released
//! while it runs.

use super::*;

use crate::ops::types::RecommendationJobInputConfig;

/// How many invocations the benchmark sends to one endpoint under test.
///
/// An EMULATOR ceiling, not an AWS number: every invocation is a real inference computed in
/// this process, so how much work one create performs is bounded here. It is more than one, so
/// the latency reported is a mean over several invocations rather than one sample.
pub const BENCHMARK_INVOCATIONS: usize = 16;

/// How many endpoints one job benchmarks, whatever `RecommendationJobResourceLimit$
/// MaxNumberOfTests` asks for and however many entries the request's `Endpoints` and
/// `EndpointConfigurations` lists carry. An EMULATOR ceiling, under
/// [`BENCHMARK_INVOCATIONS`]' rule: it is what bounds the WORK one job performs, since the
/// invocations per test are bounded by a constant.
pub const MAX_BENCHMARKED_CONFIGURATIONS: usize = 4;

/// How many payload records the benchmark reads out of a sample payload archive. An EMULATOR
/// ceiling, under [`BENCHMARK_INVOCATIONS`]' rule.
pub const MAX_SAMPLE_PAYLOADS: usize = 8;

/// `RecommendationJobStatus`'s member for a job whose benchmark finished.
pub const RECOMMENDATION_JOB_STATUS_COMPLETED: &str = "COMPLETED";

/// How many seconds one minute holds, which is what a measured invocation rate is reported
/// per (`RecommendationMetrics$MaxInvocations` is "requests per minute").
const SECONDS_PER_MINUTE: f64 = 60.0;

/// One endpoint the job benchmarks: where the traffic goes, and what serves it.
#[derive(Debug, Clone)]
pub struct BenchmarkTarget {
    /// `EndpointOutputConfiguration$EndpointName`: the endpoint the traffic was sent to.
    pub endpoint_name: String,
    /// `EndpointOutputConfiguration$VariantName`: the production variant behind it.
    pub variant_name: String,
    /// `EndpointOutputConfiguration$InstanceType`, when the test ran on a named instance
    /// type.
    pub instance_type: Option<String>,
    /// What serves an invocation of that variant — the model and its artifact.
    pub variant: ServingVariant,
}

/// What a recommendation job's run produced, recorded when the run reported and revealed by
/// the clock ([`super::InferenceRecommendationsJob::settle_status`]).
#[derive(Debug, Clone)]
pub enum RecommendationRunOutcome {
    /// The benchmark finished: one measured recommendation per endpoint it tested.
    Completed(Vec<InferenceRecommendation>),
    /// The benchmark did not measure anything, and this is the condition that stopped it —
    /// the job's `FailureReason`.
    Failed { reason: String },
}

/// How this service states that a recommendation job had nothing to benchmark: the job names
/// no endpoint to send traffic to and no model to deploy one for, so no load test ran.
#[must_use]
pub fn nothing_to_benchmark(job_name: &str) -> String {
    format!(
        "ValidationError: the inference recommendations job {job_name} names no endpoint to \
         benchmark and no model to deploy one for, so no load test was performed."
    )
}

/// The name of the endpoint SageMaker deploys for one of a job's own tests, and of the
/// production variant on it: the job's own name and the test's position, so the two names a
/// recommendation reports are the ones the job's own deployment carried.
#[must_use]
pub fn benchmark_endpoint_name(job_name: &str, test: usize) -> String {
    format!("sm-ep-{job_name}-{:03}", test + 1)
}

/// See [`benchmark_endpoint_name`].
#[must_use]
pub fn benchmark_variant_name(job_name: &str, test: usize) -> String {
    format!("sm-variant-{job_name}-{:03}", test + 1)
}

/// Every endpoint the job `job_name` benchmarks, read off the store the job's references
/// resolve against.
///
/// The existing endpoints the job names come first, as the module docs state; a job that names
/// none is benchmarked on the endpoints it deploys for its own `EndpointConfigurations`, over
/// the model its `ModelName` or its `ModelPackageVersionArn` names ([`model_under_test`]).
#[must_use]
pub fn benchmark_targets(
    store: &ComAmazonawsSagemakerSagemakerState,
    job_name: &str,
    config: &RecommendationJobInputConfig,
) -> Vec<BenchmarkTarget> {
    let mut targets: Vec<BenchmarkTarget> = Vec::new();
    let mut named: BTreeSet<String> = BTreeSet::new();
    for endpoint_name in config
        .endpoints
        .iter()
        .flatten()
        .filter_map(|endpoint| endpoint.endpoint_name.clone())
        // However many entries the request's list carries, no more than this emulator's
        // ceiling are looked at at all: the selection is bounded before any per-entry work.
        .take(MAX_BENCHMARKED_CONFIGURATIONS)
    {
        // One test per endpoint: a list naming the same endpoint twice asks for the same
        // benchmark twice, which is one test of it.
        if !named.insert(endpoint_name.clone()) {
            continue;
        }
        // The ML compute the endpoint is serving on, which is the instance type the test ran
        // on: the endpoint's own configuration carries it per production variant.
        let configured = store
            .endpoints
            .get(&endpoint_name)
            .and_then(|endpoint| store.endpoint_configs.get(&endpoint.config_name));
        for variant in endpoint_serving_variants(store, &endpoint_name) {
            let instance_type = configured.and_then(|config| {
                config
                    .production_variants
                    .iter()
                    .find(|declared| declared.variant_name == variant.variant_name)
                    .and_then(|declared| declared.instance_type.clone())
            });
            targets.push(BenchmarkTarget {
                endpoint_name: endpoint_name.clone(),
                variant_name: variant.variant_name.clone(),
                instance_type,
                variant,
            });
        }
    }
    if !targets.is_empty() {
        // Whatever the request's lists carry, the work one job performs is bounded by this
        // emulator's own ceiling.
        targets.truncate(MAX_BENCHMARKED_CONFIGURATIONS);
        return targets;
    }
    // The endpoint configurations under test: one target per `EndpointConfigurations` entry the
    // job declares, and a single test on no named instance type and the model package's own
    // inference specification for a job that declares none. The request's list is bounded
    // before any per-entry work, as the endpoints above are.
    let tested: Vec<EndpointUnderTest> = match config.endpoint_configurations.as_deref() {
        Some(configurations) if !configurations.is_empty() => configurations
            .iter()
            .take(MAX_BENCHMARKED_CONFIGURATIONS)
            .map(|configuration| EndpointUnderTest {
                instance_type: configuration.instance_type.clone(),
                specification: configuration.inference_specification_name.clone(),
            })
            .collect(),
        _ => vec![EndpointUnderTest::default()],
    };
    let limit = config
        .resource_limit
        .as_ref()
        .and_then(|limit| limit.max_number_of_tests)
        .and_then(|asked| usize::try_from(asked).ok())
        .unwrap_or(MAX_BENCHMARKED_CONFIGURATIONS)
        .clamp(1, MAX_BENCHMARKED_CONFIGURATIONS);
    for (test, under_test) in tested.into_iter().take(limit).enumerate() {
        // The model this test deploys its own endpoint for, which is the model the job's input
        // configuration names. A job naming one this account does not hold has nothing to
        // deploy for the test, and so nothing to benchmark on it. The test's position in the
        // request is what names the endpoint it deploys, so a request whose entries select a
        // mix of resolvable and unresolvable inference specifications reports the positions of
        // the tests that ran rather than renumbering them.
        let Some(variant) = model_under_test(store, config, under_test.specification.as_deref())
        else {
            continue;
        };
        targets.push(BenchmarkTarget {
            endpoint_name: benchmark_endpoint_name(job_name, test),
            variant_name: benchmark_variant_name(job_name, test),
            instance_type: under_test.instance_type,
            variant: ServingVariant {
                variant_name: benchmark_variant_name(job_name, test),
                ..variant
            },
        });
    }
    targets.truncate(MAX_BENCHMARKED_CONFIGURATIONS);
    targets
}

/// One endpoint configuration a job has SageMaker deploy for a test of its own, as
/// `EndpointInputConfiguration` declares it: the ML compute the test runs on
/// (`$InstanceType`, "the instance types to use for the load test") and which of the model
/// package's inference specifications it deploys (`$InferenceSpecificationName`, "the inference
/// specification name in the model package version").
#[derive(Debug, Clone, Default)]
struct EndpointUnderTest {
    instance_type: Option<String>,
    specification: Option<String>,
}

/// What serves one test's traffic: the model the job's `InputConfig` names, however it names
/// it.
///
/// `RecommendationJobInputConfig` names the model under test in exactly one of two ways —
/// `$ModelName`, "the name of the created model", or `$ModelPackageVersionArn`, "the Amazon
/// Resource Name (ARN) of a versioned model package" — so both resolve to the model whose
/// artifact the load test invokes, read off the store rather than off the request. A package
/// this account holds is served from the container its selected inference specification names
/// ([`model_package_serving_variant`]), `specification` selecting which specification this test
/// deploys; a package or model the store does not hold resolves to nothing, and the job has
/// nothing to benchmark.
///
/// The `variant_name` it answers is the model's own: the caller replaces it with the name of
/// the production variant the job's own deployment carried
/// ([`benchmark_variant_name`]), which is the name the recommendation reports.
fn model_under_test(
    store: &ComAmazonawsSagemakerSagemakerState,
    config: &RecommendationJobInputConfig,
    specification: Option<&str>,
) -> Option<ServingVariant> {
    if let Some(model_name) = config.model_name.as_deref() {
        return model_serving_variant(store, model_name);
    }
    let identifier = config.model_package_version_arn.as_deref()?;
    model_package_serving_variant(store, identifier, specification)
}

/// What serves an invocation of the model `model_name` when it is deployed on its own: the
/// model and the artifact its container names, read the one way this service reads them
/// ([`super::model_serving`]).
fn model_serving_variant(
    store: &ComAmazonawsSagemakerSagemakerState,
    model_name: &str,
) -> Option<ServingVariant> {
    let model = store.models.get(model_name)?;
    let model_data = served_model_data(store, model);
    Some(ServingVariant {
        variant_name: model.name.clone(),
        model_name: model.name.clone(),
        container_supplied_model: model_data.is_none() && model_declares_containers(model),
        model_data,
        inference_program: served_inference_program(store, model),
    })
}

/// Run one accepted recommendation job: load the model behind each endpoint under test, send
/// it the benchmark's traffic and report what the invocations measured.
///
/// Answers a [`RecommendationRunOutcome`] rather than an error channel: the benchmark is the
/// job's own work, so what it could not do is the job's modelled `FAILED` status.
pub async fn run_inference_recommendations_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_name: &str,
    config: &RecommendationJobInputConfig,
    targets: &[BenchmarkTarget],
) -> RecommendationRunOutcome {
    if targets.is_empty() {
        return RecommendationRunOutcome::Failed {
            reason: nothing_to_benchmark(job_name),
        };
    }
    let payloads = sample_payloads(ctx, region, config).await;
    let mut measured: Vec<InferenceRecommendation> = Vec::new();
    for target in targets {
        let model = match load_served_artifact(ctx, region, &target.variant).await {
            Some(Ok(model)) => model,
            // The container of the model under test declares NO model data, so the model is
            // the container image's own. That is a model this emulator does not run, so this
            // test MEASURED nothing — but it is not a failure of the job: the package is a
            // valid one (`ModelDataUrl` and `ModelDataSource` are both optional), the test
            // was planned and its endpoint was put under test, so the job reports the
            // recommendation for it with no metrics on it rather than failing for want of
            // artifacts. Nothing is claimed that nothing measured.
            Some(Err(ServingRefusal::NoModelArtifact { .. }))
                if target.variant.container_supplied_model =>
            {
                measured.push(unmeasured_recommendation(target));
                continue;
            }
            Some(Err(refusal)) => {
                return RecommendationRunOutcome::Failed {
                    reason: refusal.message(),
                }
            }
            // The artifact could not be read at all — a fault of this process rather than of
            // the request, which the job reports as such.
            None => {
                return RecommendationRunOutcome::Failed {
                    reason: crate::state::INTERNAL_FAILURE_REASON.to_string(),
                }
            }
        };
        // The traffic this test sends: the sample payload's own records when the job named
        // one, and otherwise a record of the width this model takes.
        let traffic: Vec<Vec<u8>> = if payloads.is_empty() {
            vec![generated_payload(&model)]
        } else {
            payloads.clone()
        };
        let benchmark = ::tokio::task::spawn_blocking(move || benchmark_model(&model, &traffic))
            .await;
        let Ok(benchmark) = benchmark else {
            return RecommendationRunOutcome::Failed {
                reason: crate::state::INTERNAL_FAILURE_REASON.to_string(),
            };
        };
        match benchmark {
            Ok(measurement) => measured.push(InferenceRecommendation {
                recommendation_id: mint_opaque_resource_id(),
                endpoint_name: target.endpoint_name.clone(),
                variant_name: target.variant_name.clone(),
                instance_type: target.instance_type.clone().unwrap_or_default(),
                measured: Some(measurement),
            }),
            Err(refusal) => {
                return RecommendationRunOutcome::Failed {
                    reason: refusal.message(),
                }
            }
        }
    }
    RecommendationRunOutcome::Completed(measured)
}

/// The recommendation one test of the job reports when its own invocations measured NOTHING:
/// the endpoint and the ML compute the test was planned on, and no metrics at all.
///
/// `RecommendationMetrics` is an optional member of a recommendation, and the metrics it
/// carries are documented as values that may be unavailable, so a test that measured nothing
/// reports a recommendation without them. The alternative would be a number no invocation
/// produced, which is not a measurement.
fn unmeasured_recommendation(target: &BenchmarkTarget) -> InferenceRecommendation {
    InferenceRecommendation {
        recommendation_id: mint_opaque_resource_id(),
        endpoint_name: target.endpoint_name.clone(),
        variant_name: target.variant_name.clone(),
        instance_type: target.instance_type.clone().unwrap_or_default(),
        measured: None,
    }
}

/// What one endpoint's benchmark measured.
#[derive(Debug, Clone, Copy)]
pub struct Measurement {
    /// `RecommendationMetrics$MaxInvocations`: how many of the invocations performed fit in a
    /// minute at the rate they were served at, which is what one instance sustained.
    pub invocations_per_minute: i32,
    /// `RecommendationMetrics$ModelLatency`: the mean time one invocation took, in
    /// milliseconds, rounded up to the whole milliseconds the member reports and never zero —
    /// an invocation that was served took time.
    pub latency_millis: i32,
}

/// Send `traffic` to `model` and measure what the invocations took.
///
/// Every invocation is the inference the serving path computes ([`serve_artifact`]), so the
/// time measured is the time real predictions took. An invocation the model could not answer
/// stops the benchmark with that condition: a load test over a payload the model cannot be
/// applied to measures nothing.
///
/// The traffic is comma-separated records — the generated one is
/// ([`generated_payload`]), and a sample payload holds the records of a dataset — so that is the
/// media type the invocations carry, which is what a model served by its own inference code is
/// handed beside the payload.
pub fn benchmark_model(
    model: &ServedArtifact,
    traffic: &[Vec<u8>],
) -> Result<Measurement, ServingRefusal> {
    let started = ::std::time::Instant::now();
    let mut served = 0_usize;
    for index in 0..BENCHMARK_INVOCATIONS {
        let payload = &traffic[index % traffic.len()];
        serve_artifact(model, payload, Some(CSV_MEDIA_TYPE), None)?;
        served += 1;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let per_invocation = elapsed / served.max(1) as f64;
    let millis = (per_invocation * 1_000.0).ceil().max(1.0);
    let per_minute = (SECONDS_PER_MINUTE / per_invocation.max(f64::MIN_POSITIVE)).floor();
    Ok(Measurement {
        invocations_per_minute: clamped(per_minute),
        latency_millis: clamped(millis),
    })
}

/// A measured number as the modelled `Integer` member carries it: never past what that member
/// can hold, and never below one — the benchmark performed at least one invocation.
fn clamped(measured: f64) -> i32 {
    if measured >= f64::from(i32::MAX) {
        return i32::MAX;
    }
    (measured as i32).max(1)
}

/// The record the generator sends to a model the job named no sample payload for: one row of
/// the width that model takes, as the `text/csv` a SageMaker inference container reads.
///
/// A model that was fitted on no particular width ([`ServedArtifact::fitted_width`]) is applied to
/// a record of any width, so the generator sends a single-value one.
#[must_use]
pub fn generated_payload(model: &ServedArtifact) -> Vec<u8> {
    let row: Vec<String> = (0..model.fitted_width().unwrap_or(1).max(1))
        .map(|_| 1.to_string())
        .collect();
    row.join(CSV_FIELD_SEPARATOR).into_bytes()
}

/// The records of the job's sample payload, read out of Amazon S3 — empty for a job that names
/// none, or one whose archive could not be read.
///
/// "This path must point to a single gzip compressed tar archive (.tar.gz suffix)"
/// (`RecommendationJobPayloadConfig$SamplePayloadUrl`), so the archive is read with the one
/// reading this crate gives a `tar.gz`, and each entry it holds is one invocation's payload.
async fn sample_payloads(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &RecommendationJobInputConfig,
) -> Vec<Vec<u8>> {
    let Some(url) = config
        .container_config
        .as_ref()
        .and_then(|container| container.payload_config.as_ref())
        .and_then(|payload| payload.sample_payload_url.clone())
    else {
        return Vec::new();
    };
    let Some(location) = S3Location::parse(&url) else {
        return Vec::new();
    };
    let client = s3_client(ctx, region);
    let Ok(bytes) = read_object(&client, &location, MAX_OBJECT_BYTES).await else {
        return Vec::new();
    };
    archive_entries(&bytes)
}

/// The entries one sample payload archive holds, each as the bytes of one invocation's
/// payload, bounded by [`MAX_SAMPLE_PAYLOADS`] entries and by the inflated-archive budget this
/// crate reads every `tar.gz` under.
#[must_use]
pub fn archive_entries(bytes: &[u8]) -> Vec<Vec<u8>> {
    use ::std::io::Read;

    let decoder = ::flate2::read::GzDecoder::new(bytes).take(MAX_INFLATED_ARCHIVE_BYTES);
    let mut archive = ::tar::Archive::new(decoder);
    let Ok(entries) = archive.entries() else {
        return Vec::new();
    };
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for entry in entries.take(MAX_SAMPLE_PAYLOADS) {
        let Ok(entry) = entry else {
            break;
        };
        let mut body: Vec<u8> = Vec::new();
        if entry
            .take(MAX_INFLATED_ARCHIVE_BYTES)
            .read_to_end(&mut body)
            .is_err()
        {
            break;
        }
        if !body.is_empty() {
            payloads.push(body);
        }
    }
    payloads
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fitted model of `features` feature columns.
    fn model(features: usize) -> ServedArtifact {
        ServedArtifact::Model(ServedModel::Fitted(LinearModel {
            intercept: 1.0,
            weights: (0..features).map(|index| index as f64).collect(),
            epochs: 1,
            link: ObjectiveLink::Identity,
        }))
    }

    /// RULE: a benchmark reports what its invocations measured — a latency of at least one
    /// millisecond and the invocation rate that latency sustains — and a payload the model
    /// cannot be applied to measures nothing.
    ///
    /// The rows differ along the WIDTH of the model and the payload sent to it.
    #[test]
    fn the_benchmark_measures_the_invocations_it_performed() {
        // (case, model features, payload, whether the model can answer it)
        let rows: [(&str, usize, &str, bool); 4] = [
            ("one feature", 1, "2", true),
            ("three features", 3, "1,2,3", true),
            ("a payload of the wrong width", 3, "1,2", false),
            ("a payload holding no record", 2, "not-a-record", false),
        ];
        for (case, features, payload, answerable) in rows {
            let fitted = model(features);
            let measured = benchmark_model(&fitted, &[payload.as_bytes().to_vec()]);
            assert_eq!(
                measured.is_ok(),
                answerable,
                "case {case:?}: expected the benchmark to {}, got {measured:?}",
                if answerable { "measure" } else { "measure nothing" }
            );
            if let Ok(measured) = measured {
                assert!(
                    measured.latency_millis >= 1 && measured.invocations_per_minute >= 1,
                    "case {case:?}: expected measured numbers, got {measured:?}"
                );
            }
        }
    }

    /// RULE: a model the job named no sample payload for is sent a record of the width it
    /// takes, so the generated traffic is traffic that model can answer.
    #[test]
    fn the_generator_sends_a_record_of_the_model_width() {
        for features in [1_usize, 2, 7] {
            let fitted = model(features);
            let payload = generated_payload(&fitted);
            let text = String::from_utf8(payload.clone()).expect("the record is text");
            assert_eq!(
                text.split(CSV_FIELD_SEPARATOR).count(),
                features,
                "case {features} features: expected a record of that width, got {text:?}"
            );
            assert!(
                serve_artifact(&fitted, &payload, None, None).is_ok(),
                "case {features} features: the model must answer the generated record"
            );
        }
    }
}
