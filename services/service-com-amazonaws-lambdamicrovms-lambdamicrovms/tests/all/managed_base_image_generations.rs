// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GENERATIONS the AWS-managed base-image catalogue publishes:
//! the form of a managed version id, how many of them one base image publishes, and the
//! architectures each one supports.
//!
//! RULE: an AWS-published base image is versioned by a plain integer GENERATION counter
//! — not the `major.minor` form this service mints for an account's own images — and the
//! catalogue publishes every generation it has published for that image, newest first,
//! each reporting the CPU architectures a MicroVM built on it can target
//! (`supportedCpuConfigurations`, one `{architecture}` entry per architecture).
//!
//! `supportedCpuConfigurations` is a MODELLED member of `ManagedMicrovmImageVersion`,
//! beside `imageArn`, `imageVersion`, `status`, `createdAt` and `updatedAt`
//! (`smithy_query(query="shape",
//! shape="ManagedMicrovmImageVersion",
//! model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`),
//! so a listing row reports it as it reports every other member of that shape.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The paging, the ARN, the
//! status and the identifier forms of this listing are the subject of
//! `managed_base_image_versions`; the wire harness (`emulator`, `call`, `image_body`, …)
//! is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::ops::types::ARCHITECTURE_VALUES;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    latest_managed_base_image_version, latest_managed_base_image_version_label,
    managed_base_image_arn, managed_base_image_supported_architectures,
    published_managed_base_image_versions, version_order, MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{as_uri_value, call, emulator, image_body, REGION};

/// `GET /2025-09-09/managed-microvm-images/{imageIdentifier}/versions`, with whatever
/// `query` adds to the query string.
async fn list_versions(
    router: &MultiServiceRouter,
    identifier: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/2025-09-09/managed-microvm-images/{}/versions",
        as_uri_value(identifier)
    );
    let uri = match query.is_empty() {
        true => path,
        false => format!("{path}?{query}"),
    };
    call(router, "GET", &uri, None).await
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// RULE: a managed base image publishes SEVERAL generations, each id a plain integer
/// counter, served newest first — so a client can pin an older generation for a
/// reproducible build.
#[tokio::test]
async fn a_managed_base_image_publishes_its_whole_generation_set() {
    let router = emulator();

    // Rows: one per image the catalogue publishes, read through both identifier forms,
    // so what is published does not depend on how the image is addressed.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let published = published_managed_base_image_versions(image_id);
        assert!(
            published.len() > 1,
            "case {image_id:?}: an AWS-published base image publishes more than one \
             generation, got {published:?}"
        );

        for identifier in [(*image_id).to_string(), managed_base_image_arn(REGION, image_id)] {
            let case = format!("{identifier:?}");
            let (status, body) = list_versions(&router, &identifier, "").await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

            let served: Vec<String> = page_items(&body, &case)
                .iter()
                .map(|item| {
                    item["imageVersion"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case}: every entry carries its version, got {item}")
                        })
                        .to_string()
                })
                .collect();
            assert_eq!(
                served,
                published.iter().map(|v| (*v).to_string()).collect::<Vec<_>>(),
                "case {case}: expected the published generation set, got {body}"
            );

            // The id of a MANAGED generation is a plain integer counter: the
            // `major.minor` form is the one this service mints for an ACCOUNT's own
            // image versions, and the two are not interchangeable.
            for version in &served {
                assert!(
                    version.parse::<u64>().is_ok(),
                    "case {case}: a managed generation id is an integer counter, got \
                     {version:?}"
                );
            }

            // Newest first: each entry's order is strictly above the next one's, and
            // the first row is the newest generation the catalogue publishes.
            for pair in served.windows(2) {
                assert!(
                    version_order(&pair[0]) > version_order(&pair[1]),
                    "case {case}: expected the generations newest first, got {served:?}"
                );
            }
            assert_eq!(
                served.first().map(String::as_str),
                latest_managed_base_image_version(image_id),
                "case {case}: the newest published generation is the row served first, got \
                 {served:?}"
            );
        }
    }
}

/// RULE: every published generation reports the architectures a MicroVM built on it can
/// target, as one `{architecture}` entry per architecture — so a caller can select a
/// base image by architecture. The catalogue's own table is the source
/// (`state::managed_base_image_supported_architectures`), and every value it names is
/// one the API models (`Architecture`, whose values `state::ARCHITECTURE_VALUES`
/// carries).
#[tokio::test]
async fn every_published_generation_reports_the_architectures_it_supports() {
    let router = emulator();

    // Rows: every generation of every published image, at every page size that cuts the
    // set differently, so the member is reported on a first page, a middle page and a
    // last one alike.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let published = published_managed_base_image_versions(image_id);
        for size in [None, Some(1), Some(published.len())] {
            let case = format!("{image_id:?} maxResults={size:?}");
            let mut token: Option<String> = None;
            let mut seen = 0usize;

            for _ in 0..=published.len() {
                let mut query: Vec<String> = Vec::new();
                if let Some(size) = size {
                    query.push(format!("maxResults={size}"));
                }
                if let Some(token) = &token {
                    query.push(format!("nextToken={token}"));
                }
                let (status, body) = list_versions(&router, image_id, &query.join("&")).await;
                assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

                for item in page_items(&body, &case) {
                    let version = item["imageVersion"].as_str().unwrap_or_default();
                    let expected: Value = managed_base_image_supported_architectures(
                        image_id, version,
                    )
                    .iter()
                    .map(|configuration| json!({"architecture": configuration.architecture}))
                    .collect();
                    assert!(
                        !expected.as_array().is_some_and(Vec::is_empty),
                        "case {case}: the catalogue publishes an architecture for generation \
                         {version:?}, got {expected}"
                    );
                    assert_eq!(
                        item["supportedCpuConfigurations"], expected,
                        "case {case}: generation {version:?} reports the architectures the \
                         catalogue publishes, got {item}"
                    );
                    for entry in item["supportedCpuConfigurations"]
                        .as_array()
                        .unwrap_or(&Vec::new())
                    {
                        let architecture = entry["architecture"].as_str().unwrap_or_default();
                        assert!(
                            ARCHITECTURE_VALUES.contains(&architecture),
                            "case {case}: {architecture:?} is not a modelled Architecture \
                             ({ARCHITECTURE_VALUES:?})"
                        );
                    }
                    seen += 1;
                }

                token = body["nextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen,
                published.len(),
                "case {case}: the walk served every published generation"
            );
        }
    }
}

/// RULE: the catalogue is the ONE table both sides of the service read — a create that
/// pins a generation the catalogue publishes is accepted and records that generation, one
/// that pins none records the base-image release the newest generation of the base image
/// it named ships, and neither changes what the catalogue publishes.
#[tokio::test]
async fn a_build_pins_any_published_generation() {
    let router = emulator();
    let image_id = MANAGED_BASE_IMAGE_IDS[0];
    let base_image_arn = managed_base_image_arn(REGION, image_id);
    let published = published_managed_base_image_versions(image_id);

    let (status, before) = list_versions(&router, image_id, "").await;
    assert_eq!(status, 200, "the catalogue read got {status} {before}");

    // Rows: each published generation pinned explicitly, plus the request that pins
    // none, whose resolved version is the one stated beside that row.
    let mut rows: Vec<(String, Value, String)> = published
        .iter()
        .enumerate()
        .map(|(index, version)| {
            (
                format!("pinned-{index}"),
                json!({"baseImageArn": base_image_arn, "baseImageVersion": version}),
                (*version).to_string(),
            )
        })
        .collect();
    rows.push((
        "unpinned".to_string(),
        json!({"baseImageArn": base_image_arn}),
        // A build that pins no version is RESOLVED against the catalogue: it records the
        // `major.minor` release the newest generation published for the base image it
        // named ships (`state::resolve_base_image_version`), never the generation counter
        // that generation is addressed by.
        latest_managed_base_image_version_label(image_id)
            .unwrap_or_else(|| panic!("the catalogue publishes a generation of {image_id}"))
            .to_string(),
    ));

    for (name, extra, expected) in rows {
        let (status, created) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, extra)),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: create got {status} {created}");
        assert_eq!(
            created["baseImageVersion"].as_str(),
            Some(expected.as_str()),
            "case {name:?}: expected the base-image version the build resolves to, got {:?}",
            created["baseImageVersion"]
        );
    }

    let (status, after) = list_versions(&router, image_id, "").await;
    assert_eq!(status, 200, "the re-read got {status} {after}");
    assert_eq!(
        after, before,
        "the catalogue is AWS's own: building on it changes nothing, got {after}"
    );
}
