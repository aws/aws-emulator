// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PROVENANCE of the two MicroVM-image read rows a client audits
//! a built image by — the base-image release the version LISTING publishes for a version,
//! and the request identifier the single-build READ publishes for a build — driven over the
//! same router the emulator boots.
//!
//! RULE 1: `MicrovmImageVersionSummary$baseImageVersion` is the release RESOLVED from the
//! base image the version was built against, which for an AWS-managed base image is the
//! release the newest generation of its catalogue entry ships — a different namespace from
//! the `major.minor` version sequence this service mints for an account's own image, so a
//! listing row never reports that initial version for a resolution, whichever form of
//! `MicrovmImageIdentifier` the listing was addressed by.
//!
//! RULE 2: a build request id is DRAWN per build request rather than counted, so it carries
//! nothing of how many builds the store that minted it had minted before: two stores given
//! the very same create publish no identifier in common. That an identifier's drawn positions
//! vary is `build_request_id_form`'s own rule, asserted there over every build of one store.
//!
//! Each test states its rule and exercises it over a SET of inputs — the base images the
//! catalogue publishes crossed with both identifier forms, and the builds of two independent
//! stores. The values any one row happens to carry are examples: every expected release is
//! read off the catalogue and every expected identifier off the mint. Every helper is the
//! one its own suite owns — the wire harness `microvm_lifecycle`'s, the Amazon S3 router and
//! staging `image_build_outcome`'s, the pedigree accessors `image_version_pedigree`'s and
//! the build-identifier accessors `build_request_id_form`'s — so neither the harness nor the
//! form check is written twice.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    latest_managed_base_image_version_label, managed_base_image_arn, FIRST_IMAGE_VERSION,
    MANAGED_BASE_IMAGE_IDS,
};

use crate::build_request_id_form::{listed_build_ids, read_each_build_back};
use crate::image_build_outcome::{emulator as emulator_with_object_store, stage_object};
use crate::image_build_read_path::create_image;
use crate::image_version_pedigree::{create, text, version_row};
use crate::microvm_lifecycle::{emulator, REGION};

/// The bucket and key the staged code artifact lives at: the address the harness's default
/// `codeArtifact` names, so the builds of the images these cases create resolve it.
const ARTIFACT_BUCKET: &str = "artifacts";
const ARTIFACT_KEY: &str = "app.zip";

/// RULE: the version LISTING projects the stored pedigree, so the row of a build that pinned
/// no `baseImageVersion` on an AWS-managed base image publishes the release that base image
/// publishes — never this service's own initial version for an account's image
/// (`state::FIRST_IMAGE_VERSION`), which is what a listing answering a per-service constant
/// could not be told apart from — and the row names the base image the pedigree came from.
#[tokio::test]
async fn an_unpinned_listing_row_publishes_the_managed_release_not_the_initial_version() {
    let router = emulator_with_object_store();
    stage_object(&router, ARTIFACT_BUCKET, ARTIFACT_KEY).await;

    // Rows: every base image the AWS-managed catalogue publishes
    // (`state::MANAGED_BASE_IMAGE_IDS`, read off the catalogue rather than named here),
    // crossed with the two identifiers this service resolves one of its own images by — the
    // ARN it minted for the image, and the image's name (`MicrovmImageIdentifier` is "The
    // ARN or ID of the MicroVmImage"; which other value may also address a record is the
    // resolver's own rule, and nothing here pins it). The listing is one projection of one
    // record, so neither the base image named nor the address the listing was reached by may
    // change what a row carries.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let release = latest_managed_base_image_version_label(image_id)
            .unwrap_or_else(|| panic!("the catalogue publishes a generation of {image_id}"))
            .to_string();
        assert_ne!(
            release, FIRST_IMAGE_VERSION,
            "the release the catalogue publishes for {image_id} is that base image's own, \
             not this service's initial version for an account's image"
        );

        let name = format!("listed-resolved-{image_id}");
        let base_image_arn = managed_base_image_arn(REGION, image_id);
        let created = create(&router, &name, json!({"baseImageArn": base_image_arn.clone()})).await;
        let image_arn = text(&created, "imageArn");
        let image_version = text(&created, "imageVersion");

        for identifier in [image_arn.as_str(), name.as_str()] {
            let case = format!("the unpinned build on {image_id}, listed by {identifier}");
            let row = version_row(&router, identifier, &image_version, &case).await;
            let listed = text(&row, "baseImageVersion");
            assert_eq!(
                listed, release,
                "case {case:?}: expected the row to publish the release the catalogue's \
                 newest generation ships ({release:?}), got {listed:?}"
            );
            assert_ne!(
                listed, image_version,
                "case {case:?}: the base image's published release and this service's own \
                 version for the image are unrelated namespaces, got one value for both"
            );
            let named = text(&row, "baseImageArn");
            assert_eq!(
                named, base_image_arn,
                "case {case:?}: expected the row to name the base image the pedigree was \
                 resolved from ({base_image_arn:?}), got {named:?}"
            );
        }
    }
}

/// RULE: a build request id is drawn per build request, so it carries nothing of how many
/// builds the store that minted it had minted before: two independent stores, each given the
/// very same create, publish no identifier in common, and each one's single-build read
/// answers the identifier of its own store. A counter-rendered id would hand the first build
/// of every store one value.
#[tokio::test]
async fn no_two_stores_mint_the_same_build_request_id() {
    // Two stores, given the same create: nothing but the draw tells their builds apart, so
    // an overlap is the counter's signature rather than a coincidence of the input.
    let stores = [("the first store", emulator()), ("the second store", emulator())];

    let mut published: BTreeSet<String> = BTreeSet::new();
    for (case, router) in &stores {
        let image_arn = create_image(router, "drawn-across-stores", json!({})).await;
        let ids = listed_build_ids(router, &image_arn, FIRST_IMAGE_VERSION, case).await;
        read_each_build_back(router, &image_arn, FIRST_IMAGE_VERSION, &ids, case).await;

        let shared: Vec<&String> = ids.iter().filter(|id| published.contains(*id)).collect();
        assert!(
            shared.is_empty(),
            "case {case:?}: expected no identifier another store already minted, got \
             {shared:?} among {published:?}"
        );
        published.extend(ids);
    }
}
