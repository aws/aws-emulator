// SPDX-License-Identifier: CC0-1.0

//! Where an asynchronous SageMaker inference reports: the Amazon S3 URI the response object
//! is written to, and the one the failure object is written to beside it.
//!
//! # The rule
//!
//! A submission is accepted against a configured OUTPUT PATH — the endpoint's
//! `AsyncInferenceConfig$OutputConfig$S3OutputPath` for a submission to an endpoint, the
//! path the ROUTING TARGET specifies for one to an inference router — and the response
//! object's URI is that path, plus the request's own `S3OutputPathExtension` as a further
//! path segment ("the path extension that is appended to the Amazon S3 output path"), plus
//! the object's name: the request's `Filename` where it sent one, and otherwise a name
//! derived from the inference id, which is what the caller is answered and can poll for. The
//! failure object is named under the configured FAILURE path by the same inference id, with
//! the `-error` suffix AWS writes it with.
//!
//! The derivation is a pure function of those inputs, so resubmitting one inference id
//! yields the SAME strings by construction rather than by answering from a stored record.
//!
//! Here, once, because two SageMaker planes accept such a submission and answer such a
//! location: a second copy of the composition drifts into reporting a different URI for the
//! same submission, and the URI is what a caller fetches the result at.
//!
//! # What is NOT done here
//!
//! The segments of `S3OutputPathExtension` are spliced in as the member wrote them — `.` and
//! `..` included, since that member's pattern (`^(?!s3:|https:)[a-zA-Z0-9!_.*'()/-]+$`)
//! admits them, an Amazon S3 key is the string it was given, and nothing here or in a caller
//! ever RESOLVES the reported key. The one normalisation is deliberate and structural: a
//! leading or trailing `/` and an empty segment are dropped, so the extension joins the
//! configured path with exactly one separator and the reported key carries no `//`.
//!
//! Whether a `Filename` is a value its member's own modelled pattern admits is the CALLER's
//! refusal to make, before anything is composed: that pattern carries a negative look-ahead,
//! which is `crate::unapplied_patterns`' subject, and a refusal's envelope belongs to the
//! service making it.

/// The suffix the failure object's name carries, before its extension: the object the
/// failure details of the submission `<inference id>` are written to is
/// `<inference id>-error.out`.
const FAILURE_SUFFIX: &str = "-error";

/// The extension every object this derivation names carries, as AWS writes an asynchronous
/// inference result: `<name>.out`.
const OBJECT_EXTENSION: &str = ".out";

/// Where the RESPONSE of one asynchronous submission is written: `s3_output_path`, the
/// request's `extension`, and the object name — `filename` when the request sent one, else
/// the inference id's own object. See the module header for the rule.
#[must_use]
pub fn output_location(
    s3_output_path: &str,
    extension: Option<&str>,
    filename: Option<&str>,
    inference_id: &str,
) -> String {
    let mut location = under(s3_output_path, extension);
    location.push('/');
    match filename {
        Some(filename) => location.push_str(filename),
        None => {
            location.push_str(inference_id);
            location.push_str(OBJECT_EXTENSION);
        }
    }
    location
}

/// Where the FAILURE details of one asynchronous submission are written: the configured
/// `s3_failure_path` and the inference id's own error object.
#[must_use]
pub fn failure_location(s3_failure_path: &str, inference_id: &str) -> String {
    format!(
        "{}/{inference_id}{FAILURE_SUFFIX}{OBJECT_EXTENSION}",
        s3_failure_path.trim_end_matches('/')
    )
}

/// `base` with `extension`'s own segments appended, joined by exactly one separator each and
/// carrying no empty segment; see the module header.
fn under(base: &str, extension: Option<&str>) -> String {
    let mut location = base.trim_end_matches('/').to_string();
    let Some(extension) = extension else {
        return location;
    };
    let segments: Vec<&str> = extension
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if !segments.is_empty() {
        location.push('/');
        location.push_str(&segments.join("/"));
    }
    location
}

#[cfg(test)]
mod tests {
    use super::{failure_location, output_location};

    /// One row of the response-location table: the configured output path, the request's
    /// `S3OutputPathExtension` and `Filename` where it sent them, the URI they compose, and
    /// the case's label.
    type ResponseCase = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        &'static str,
        &'static str,
    );

    /// THE RULE: the reported URI is the configured path, the request's extension and the
    /// object name — and the same inputs always compose the same URI.
    ///
    /// The rows differ along what the rule generalises over: whether the configured path ends
    /// in a separator, whether the request sent an extension (and how that extension is
    /// written), and whether it named the object itself.
    #[test]
    fn the_response_object_is_the_configured_path_the_extension_and_the_object_name() {
        let cases: &[ResponseCase] = &[
            (
                "s3://bucket/out",
                None,
                None,
                "s3://bucket/out/req-1.out",
                "no extension and no file name",
            ),
            (
                "s3://bucket/out/",
                None,
                None,
                "s3://bucket/out/req-1.out",
                "a configured path ending in a separator",
            ),
            (
                "s3://bucket/out",
                Some("tenant/7"),
                None,
                "s3://bucket/out/tenant/7/req-1.out",
                "an extension of two segments",
            ),
            (
                "s3://bucket/out",
                Some("/tenant//7/"),
                None,
                "s3://bucket/out/tenant/7/req-1.out",
                "an extension written with leading, trailing and empty segments",
            ),
            (
                "s3://bucket/out",
                Some("tenant"),
                Some("answer.json"),
                "s3://bucket/out/tenant/answer.json",
                "a named object under an extension",
            ),
            (
                "s3://bucket/out",
                None,
                Some("answer.json"),
                "s3://bucket/out/answer.json",
                "a named object with no extension",
            ),
        ];
        for (path, extension, filename, expected, case) in cases {
            let actual = output_location(path, *extension, *filename, "req-1");
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    /// THE RULE's failure half: the failure object is the configured failure path and the
    /// inference id's own error object, whatever the request named for the response.
    #[test]
    fn the_failure_object_is_the_failure_path_and_the_inference_id() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                "s3://bucket/failed",
                "req-1",
                "s3://bucket/failed/req-1-error.out",
                "a failure path with no separator",
            ),
            (
                "s3://bucket/failed/",
                "req-1",
                "s3://bucket/failed/req-1-error.out",
                "a failure path ending in a separator",
            ),
            (
                "s3://bucket/failed",
                "4c2f1d8e-0000-4000-8000-000000000001",
                "s3://bucket/failed/4c2f1d8e-0000-4000-8000-000000000001-error.out",
                "a generated inference id",
            ),
        ];
        for (path, inference_id, expected, case) in cases {
            let actual = failure_location(path, inference_id);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }
}
