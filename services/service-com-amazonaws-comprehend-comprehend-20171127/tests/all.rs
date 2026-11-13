// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/analysis_job_listing.rs"]
mod analysis_job_listing;
#[path = "all/analysis_job_runs.rs"]
mod analysis_job_runs;
#[path = "all/batch_detect_entities.rs"]
mod batch_detect_entities;
#[path = "all/batch_inference.rs"]
mod batch_inference;
#[path = "all/batch_inference_calibration.rs"]
mod batch_inference_calibration;
#[path = "all/batch_sentiment.rs"]
mod batch_sentiment;
#[path = "all/batch_syntax.rs"]
mod batch_syntax;
#[path = "all/classifier_training.rs"]
mod classifier_training;
#[path = "all/client_refusals.rs"]
mod client_refusals;
#[path = "all/comprehend_client.rs"]
mod comprehend_client;
#[path = "all/detect_pii_entities.rs"]
mod detect_pii_entities;
#[path = "all/document_classifiers.rs"]
mod document_classifiers;
#[path = "all/dominant_language_jobs.rs"]
mod dominant_language_jobs;
#[path = "all/endpoints.rs"]
mod endpoints;
#[path = "all/entity_confidence.rs"]
mod entity_confidence;
#[path = "all/entity_detection.rs"]
mod entity_detection;
#[path = "all/entity_mention_spans.rs"]
mod entity_mention_spans;
// Declared beside the suite it backs rather than under `s`: `scratch_dump` holds the
// whole-document baselines that `entity_detection` asserts the rules of.
#[path = "all/scratch_dump.rs"]
mod scratch_dump;
#[path = "all/key_phrases.rs"]
mod key_phrases;
#[path = "all/key_phrases_jobs.rs"]
mod key_phrases_jobs;
#[path = "all/language_value_sets.rs"]
mod language_value_sets;
#[path = "all/lifecycle_phase_observability.rs"]
mod lifecycle_phase_observability;
#[path = "all/not_found_refusals.rs"]
mod not_found_refusals;
#[path = "all/noun_phrase_key_phrases.rs"]
mod noun_phrase_key_phrases;
#[path = "all/person_name_typing.rs"]
mod person_name_typing;
#[path = "all/phone_number_forms.rs"]
mod phone_number_forms;
#[path = "all/pii_detection.rs"]
mod pii_detection;
#[path = "all/pii_entities.rs"]
mod pii_entities;
#[path = "all/pii_entities_jobs.rs"]
mod pii_entities_jobs;
#[path = "all/pii_labelled_values.rs"]
mod pii_labelled_values;
#[path = "all/realtime_text_quotas.rs"]
mod realtime_text_quotas;
#[path = "all/request_validation.rs"]
mod request_validation;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/sentiment_jobs.rs"]
mod sentiment_jobs;
#[path = "all/single_document_conventions.rs"]
mod single_document_conventions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sync_inference.rs"]
mod sync_inference;
#[path = "all/sync_inference_refusals.rs"]
mod sync_inference_refusals;
#[path = "all/syntax.rs"]
mod syntax;
#[path = "all/targeted_sentiment.rs"]
mod targeted_sentiment;
#[path = "all/toxic_content.rs"]
mod toxic_content;
#[path = "all/toxic_content_conduct.rs"]
mod toxic_content_conduct;
// Declared beside the suite it backs rather than under `s`: `zz_probe` holds the
// whole-document baseline that `single_document_conventions` asserts the rules of,
// for the single-document inference family.
#[path = "all/zz_probe.rs"]
mod zz_probe;
// Declared beside the suites it backs rather than under `b`: `zz_scratch` holds the
// documents written from scratch for the four `BatchDetect*` analysers and the
// whole-document baseline they must keep answering, which
// `batch_inference_calibration` and `noun_phrase_key_phrases` assert the rules of.
#[path = "all/zz_scratch.rs"]
mod zz_scratch;
