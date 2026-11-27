// SPDX-License-Identifier: CC0-1.0

//! Amazon Elastic Container Registry's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon ECR publishes its API call counts under.
const ECR_USAGE_SERVICE: &str = "ECR";

/// One Amazon ECR per-API REQUEST-RATE quota: `Rate of <Api> requests`, with the window,
/// the call-count metric and the sentence ECR publishes the whole class with.
///
/// SOURCE: the recorded default-quota listing of `ecr`, whose rate rows read
/// `The maximum number of <Api> requests that you can make per second in the current
/// Region.` — the account is NOT named in it, which is what distinguishes this service's
/// wording from [`super::request_rate`]'s. Every other published member of the class is the
/// one the shared rate constructor fills: the one-second window and the `AWS/Usage`
/// `CallCount` of the throttled operation under this service's display name.
fn ecr_request_rate(code: &str, api: &str, per_second: f64) -> QuotaRow {
    request_rate(code, ECR_USAGE_SERVICE, api, per_second).describe(&ecr_rate_sentence(api))
}

/// The sentence Amazon ECR publishes for one of its per-API request rates.
fn ecr_rate_sentence(api: &str) -> String {
    format!(
        "The maximum number of {api} requests that you can make per second in the current \
         Region."
    )
}

/// One Amazon ECR per-API request-rate quota of an operation a client calls ON THE PUSH
/// PATH: the class sentence, followed by the clause that states when the operation is
/// invoked during a push.
///
/// SOURCE: the recorded default-quota listing of `ecr`, whose layer-upload row reads the
/// class sentence followed by `When an image is pushed, the InitiateLayerUpload API is
/// called once per image layer that has not already been uploaded. Whether or not an image
/// layer has been uploaded is determined by the BatchCheckLayerAvailability API action.`,
/// and the "Amazon ECR service quotas" page of the Amazon ECR User Guide for the same
/// clause of each other push-path operation. The clause names WHEN the operation is called
/// during a push and is published per operation rather than per class, so it is passed in
/// while every other member of the row still comes from the one rate constructor.
fn push_path_request_rate(code: &str, api: &str, per_second: f64, when_called: &str) -> QuotaRow {
    ecr_request_rate(code, api, per_second)
        .describe(&format!("{} {when_called}", ecr_rate_sentence(api)))
}

/// Amazon Elastic Container Registry's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ecr` service code (the "Amazon
/// ECR service quotas" page of the Amazon ECR User Guide) for the registry, repository
/// and image planes it publishes, and the recorded default-quota listing of that code
/// for which `QuotaCode` names which quota and for the per-API request rates it
/// publishes beside them. A size plane carries the unit AWS publishes it in rather
/// than being a count, and a request-rate row carries the one-second window and the
/// `AWS/Usage` `CallCount` of the operation it throttles ([`ecr_request_rate`]).
///
/// The recorded listing publishes the `CompleteLayerUpload` rate under `L-44194860`, the
/// `PutImage` rate under `L-AD52DFB2` and the `InitiateLayerUpload` rate under
/// `L-95B28F8D`, so those are the codes they are published under here; the registry
/// replication plane, the pull-path `GetDownloadUrlForLayer` rate and the `BatchGetImage`
/// rate this catalog had published under them take the codes those bindings vacated,
/// because the listing reports every one of those codes in this service's catalog.
///
/// A row of a push-path operation closes on the clause stating when the operation is
/// invoked during a push ([`push_path_request_rate`]) rather than on the class sentence
/// alone.
///
/// Adjustability is published PER CODE rather than per class: the recorded listing reports
/// part of this service's rate planes and part of its counted planes as not adjustable, so
/// each row carries the adjustability the listing reports for its own code rather than the
/// one its constructor defaults to.
pub(in crate::state) fn ecr_quotas() -> Vec<QuotaRow> {
    let rate = ecr_request_rate;
    vec![
        quota("Registered repositories", 10_000.0)
            .code("L-03A36CE1")
            .describe(
                "The maximum number of repositories that can be registered in this account in \
                 the current Region.",
            ),
        quota("Images per repository", 10_000.0)
            .code("L-0A556EFC")
            .fixed()
            .describe("The maximum number of images in one repository."),
        quota("Lifecycle policy rules per repository", 50.0)
            .code("L-1165B4AE")
            .fixed()
            .describe("The maximum number of rules in one repository's lifecycle policy."),
        quota("Filters per lifecycle policy rule", 100.0)
            .code("L-16E70933")
            .describe("The maximum number of filters in one lifecycle policy rule."),
        quota("Repository policy length", 10_240.0)
            .code("L-241DEEBA")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one repository's policy document."),
        quota("Layer parts per image layer", 10_000.0)
            .code("L-24725E9A")
            .fixed()
            .describe("The maximum number of parts that one image layer can be uploaded in."),
        quota("Maximum image layer size", 52.0)
            .code("L-343F3D94")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size, in GiB, of one image layer."),
        push_path_request_rate(
            "L-44194860",
            "CompleteLayerUpload",
            100.0,
            "When an image is pushed, the CompleteLayerUpload API is called once per each \
             new image layer to verify that the upload has completed.",
        ),
        rate("L-55A41110", "GetAuthorizationToken", 500.0),
        // The pull-through cache plane of a registry, which the service describes without
        // naming the registry: the limit is on the rules themselves.
        quota("Pull through cache rules per registry", 50.0)
            .code("L-6EF7FC96")
            .fixed()
            .describe("The maximum number of pull-through cache rules."),
        rate("L-8BE1781E", "BatchDeleteImage", 100.0).fixed(),
        // The layer-upload rate of the push path, published under the code the recorded
        // listing states for it, with the adjustability that code is published with; the
        // `BatchGetImage` rate this catalog had served here takes the code this binding
        // vacated.
        push_path_request_rate(
            "L-95B28F8D",
            "InitiateLayerUpload",
            100.0,
            "When an image is pushed, the InitiateLayerUpload API is called once per image \
             layer that has not already been uploaded. Whether or not an image layer has \
             been uploaded is determined by the BatchCheckLayerAvailability API action.",
        ),
        quota("Replication destinations per registry", 25.0)
            .code("L-9B60BFFB")
            .fixed()
            .describe(
                "The maximum number of destinations in one registry's replication \
                 configuration.",
            ),
        rate("L-9C9D5A08", "CreateRepository", 10.0).fixed(),
        rate("L-A1670B10", "DeleteRepository", 10.0),
        rate("L-A60A366D", "DescribeImages", 100.0),
        rate("L-A71EAF5A", "DescribeRepositories", 100.0).fixed(),
        // The push-path image rate, published under the code the recorded listing states
        // for it; the pull-path layer-download rate this catalog had served here takes the
        // code that binding vacated.
        push_path_request_rate(
            "L-AD52DFB2",
            "PutImage",
            10.0,
            "When an image is pushed and all new image layers have been uploaded, the \
             PutImage API is called once to create or update the image manifest and the \
             tags associated with the image.",
        ),
        push_path_request_rate(
            "L-B9173138",
            "BatchCheckLayerAvailability",
            1000.0,
            "When an image is pushed, the BatchCheckLayerAvailability API is called once \
             per image layer to verify if that image layer has already been uploaded.",
        ),
        // The image-read rate the layer-upload binding above displaced: it takes the code
        // that binding vacated, with that code's published adjustability.
        rate("L-C0B33BA1", "BatchGetImage", 10_000.0).fixed(),
        rate("L-CB63E564", "ListImages", 100.0).fixed(),
        rate("L-CFEB8E8D", "GetDownloadUrlForLayer", 3000.0),
        rate("L-EF8F6F4E", "UploadLayerPart", 1000.0).fixed(),
    ]
}
