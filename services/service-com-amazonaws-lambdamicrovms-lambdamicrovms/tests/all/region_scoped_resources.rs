// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION a MicroVM image and a MicroVM belong to, driven over
//! the same router the emulator boots.
//!
//! RULE: every resource this service mints is regional. The region a request is signed
//! for is the endpoint that holds the record — so an identifier minted through one
//! region's endpoint is served there and, at every other region's endpoint, answered
//! exactly as an identifier nobody ever created there: the modelled 404
//! `ResourceNotFoundException` built from the identifier the request carried, and absent
//! from that region's listings. Authorization is decided one scope wider, on the ACCOUNT
//! the identifier names, which is why the foreign region reports an ABSENCE rather than
//! refusing access to an ARN of the caller's own account.
//!
//! Each test states that rule and exercises it over a SET of inputs: the rows differ in
//! which foreign region asks, which operation asks and which form the identifier takes,
//! because none of those changes the answer. The wire harness (`call_in_region`,
//! `image_body`, …) is the one `microvm_lifecycle` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_endpoint::{
    serve_request, AUTH_TOKEN_HEADER, NO_GUEST_RESPONSE_STATUS,
};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_network::connector_state;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::ComAmazonawsLambdamicrovmsLambdamicrovmsState as MicrovmState;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, microvm_not_found_message, too_many_regions_message,
    FIRST_IMAGE_VERSION, IMAGE_STATE_CREATED, MAX_REGIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call_in_region, emulator, emulator_with_engine, image_body, ACCOUNT,
    PAST_THE_SPAN, REGION,
};

/// The regions that must answer nothing for a record minted in [`REGION`]: another
/// region of the same geography, one of another continent, and one of another hemisphere
/// — the dimension the rule generalises over is "not the region that minted it", so the
/// rows only have to differ in that.
const FOREIGN_REGIONS: &[&str] = &["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The ARN this service mints for the image `name` in `region`.
fn image_arn_in(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:microvm-image:{name}")
}

/// `POST /2025-09-09/microvm-images`, signed for `region`.
async fn create_image(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    extra: Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await
}

/// `GET /2025-09-09/microvm-images/{imageIdentifier}`, signed for `region`.
async fn get_image(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    let uri = format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier));
    call_in_region(router, region, "GET", &uri, None).await
}

/// `GET /2025-09-09/microvm-images`, signed for `region`.
async fn list_images(router: &MultiServiceRouter, region: &str) -> Vec<Value> {
    let (status, body) =
        call_in_region(router, region, "GET", "/2025-09-09/microvm-images", None).await;
    assert_eq!(status, 200, "list images in {region}: got {status} {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// `GET /2025-09-09/microvms`, signed for `region`.
async fn list_microvms(router: &MultiServiceRouter, region: &str) -> Vec<Value> {
    let (status, body) = call_in_region(router, region, "GET", "/2025-09-09/microvms", None).await;
    assert_eq!(status, 200, "list MicroVMs in {region}: got {status} {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// Assert the modelled not-found envelope for `identifier`, with `expected` as its whole
/// message.
fn assert_not_found(status: u16, body: &Value, expected: &str, case: &str) {
    assert_eq!(status, 404, "case {case}: expected 404, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected),
        "case {case}: expected the message built from the request's own identifier, got {:?}",
        body["message"]
    );
}

/// RULE: an image minted through one region's endpoint is readable there and reported
/// ABSENT at every other region's endpoint — the item reads addressed at a
/// `MicrovmImageIdentifier` report the modelled not-found, and the paginated version
/// enumeration (which refuses nothing past the authorization verdict) an empty page — and
/// it appears in no other region's listing.
#[tokio::test]
async fn an_image_is_addressable_only_through_the_region_that_minted_it() {
    let router = emulator();

    let name = "regional-image";
    let (status, created) = create_image(&router, REGION, name, json!({})).await;
    assert_eq!(status, 201, "create in {REGION}: got {status} {created}");
    let arn = image_arn_in(REGION, name);
    assert_eq!(
        created["imageArn"].as_str(),
        Some(arn.as_str()),
        "the create must mint the ARN of the region it was signed for, got {created}"
    );
    tokio::time::sleep(PAST_THE_SPAN).await;

    for region in FOREIGN_REGIONS {
        // Rows differ in WHICH call asks about the identifier: the image's own read, the
        // version-addressed read and the tag listing all resolve the same
        // `imageIdentifier`, so each must report the same absence.
        let rows: Vec<(String, String)> = vec![
            (
                "GetMicrovmImage".to_string(),
                format!("/2025-09-09/microvm-images/{}", as_uri_value(&arn)),
            ),
            (
                "GetMicrovmImageVersion".to_string(),
                format!(
                    "/2025-09-09/microvm-images/{}/versions/{}",
                    as_uri_value(&arn),
                    as_uri_value(FIRST_IMAGE_VERSION)
                ),
            ),
            (
                "ListTags".to_string(),
                format!("/2017-03-31/tags/{}", as_uri_value(&arn)),
            ),
        ];
        for (operation, uri) in rows {
            let (status, body) = call_in_region(&router, region, "GET", &uri, None).await;
            assert_not_found(
                status,
                &body,
                &image_not_found_message(&arn),
                &format!("{operation} for {arn} from {region}"),
            );
        }

        // The version ENUMERATION reports what the region holds for the identifier, which
        // is nothing: an empty page (`image_refusal_paths` owns that rule).
        let (status, body) = call_in_region(
            &router,
            region,
            "GET",
            &format!(
                "/2025-09-09/microvm-images/{}/versions",
                as_uri_value(&arn)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: ListMicrovmImageVersions for {arn} expected 200, got {status} {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {region}: another region's image holds no versions here, got {body}"
        );

        let listed = list_images(&router, region).await;
        assert!(
            !listed
                .iter()
                .any(|item| item["imageArn"].as_str() == Some(arn.as_str())),
            "case {region}: the image of another region must not be listed, got {listed:?}"
        );
    }

    // The home region is unaffected by every foreign call: the record is still there, and
    // it is the one its own listing carries.
    let (status, home) = get_image(&router, REGION, &arn).await;
    assert_eq!(
        status, 200,
        "case {REGION}: the minting region still serves the image, got {status} {home}"
    );
    assert_eq!(
        home["state"].as_str(),
        Some(IMAGE_STATE_CREATED),
        "case {REGION}: the record is unchanged by the foreign reads, got {home}"
    );
    let listed = list_images(&router, REGION).await;
    assert!(
        listed
            .iter()
            .any(|item| item["imageArn"].as_str() == Some(arn.as_str())),
        "case {REGION}: the minting region lists the image, got {listed:?}"
    );
}

/// RULE: `ImageName` is unique within the account AND the region, so the same name is
/// free in every other region, and the two records are independent: each carries the ARN
/// of the region that minted it, each has its own identity, and neither is reachable
/// through the other's endpoint.
#[tokio::test]
async fn one_name_names_a_different_image_in_each_region() {
    let router = emulator();

    let name = "shared-name";
    let (status, home) = create_image(&router, REGION, name, json!({"description": "home"})).await;
    assert_eq!(status, 201, "create in {REGION}: got {status} {home}");

    // A second create of the SAME name in the minting region is still refused: the name
    // is taken there, which is what makes the rows below a REGIONAL freedom rather than a
    // missing uniqueness check.
    let (status, refused) = create_image(&router, REGION, name, json!({})).await;
    assert_eq!(
        status, 400,
        "case {REGION}: the name is taken in its own region, got {status} {refused}"
    );

    for region in FOREIGN_REGIONS {
        let (status, created) =
            create_image(&router, region, name, json!({"description": "foreign"})).await;
        assert_eq!(
            status, 201,
            "case {region}: the name is free in another region, got {status} {created}"
        );
        assert_eq!(
            created["imageArn"].as_str(),
            Some(image_arn_in(region, name).as_str()),
            "case {region}: the create mints the ARN of its own region, got {created}"
        );
        assert_ne!(
            created["id"].as_str(),
            home["id"].as_str(),
            "case {region}: the two records are distinct resources, got {created}"
        );

        // Each endpoint serves its OWN record for the shared name, and the other
        // region's ARN is absent from it.
        let (status, read) = get_image(&router, region, name).await;
        assert_eq!(
            status, 200,
            "case {region}: its own record reads back by name, got {status} {read}"
        );
        assert_eq!(
            read["id"].as_str(),
            created["id"].as_str(),
            "case {region}: the read reports that region's own record, got {read}"
        );
        assert_eq!(
            read["imageArn"].as_str(),
            Some(image_arn_in(region, name).as_str()),
            "case {region}: the read reports that region's own ARN, got {read}"
        );
        let (status, foreign) = get_image(&router, region, &image_arn_in(REGION, name)).await;
        assert_not_found(
            status,
            &foreign,
            &image_not_found_message(&image_arn_in(REGION, name)),
            &format!("the minting region's ARN read from {region}"),
        );
    }

    let (status, read) = get_image(&router, REGION, name).await;
    assert_eq!(
        status, 200,
        "case {REGION}: the home record survives every other region's create, got {read}"
    );
    assert_eq!(
        read["id"].as_str(),
        home["id"].as_str(),
        "case {REGION}: the home record keeps its own identity, got {read}"
    );
}

/// RULE: the store's one caller-driven dimension — the region a request is signed for —
/// is bounded: records are held for at most `MAX_REGIONS` regions, and a create that
/// would open one more answers the modelled HTTP 402 `ServiceQuotaExceededException`
/// rather than growing the store.
#[tokio::test]
async fn a_create_past_the_stores_region_ceiling_is_refused() {
    let router = emulator();

    // Each row is one region the store opens a partition for; the names are generated so
    // the table spans exactly the published bound and one past it.
    for index in 0..MAX_REGIONS {
        let region = format!("us-test-{index}");
        let (status, created) = create_image(&router, &region, "ceiling", json!({})).await;
        assert_eq!(
            status, 201,
            "case region {index} of {MAX_REGIONS}: a create in a region the store has room for \
             is served, got {status} {created}"
        );
    }

    let over = format!("us-test-{MAX_REGIONS}");
    let (status, refused) = create_image(&router, &over, "ceiling", json!({})).await;
    assert_eq!(
        status, 402,
        "case region {MAX_REGIONS} (one past the bound): expected 402, got {status} {refused}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case {over}: expected ServiceQuotaExceededException, got {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(too_many_regions_message().as_str()),
        "case {over}: expected the region-ceiling message, got {:?}",
        refused["message"]
    );

    // A region the store already holds is unaffected by the refusal: the ceiling bounds
    // the partitions, not the calls.
    let (status, served) = get_image(&router, "us-test-0", "ceiling").await;
    assert_eq!(
        status, 200,
        "case us-test-0: a region the store holds is still served, got {status} {served}"
    );
}

/// RULE: an authentication token is scoped to the partition that authorised it — the
/// MicroVM's `microvmId` AND the region the mint resolved it in — so a token minted at
/// one region's endpoint authenticates nothing at another region's partition, even for a
/// MicroVM carrying the same id.
#[tokio::test]
async fn an_authentication_token_grants_access_only_in_the_region_that_minted_it() {
    let (router, engine) = emulator_with_engine();

    let name = "regional-token";
    let (status, created) = create_image(&router, REGION, name, json!({})).await;
    assert_eq!(status, 201, "create in {REGION}: got {status} {created}");
    // The build is clock-owed, so this waits for the CONDITION the launch below needs —
    // an active version to launch from — rather than for a fixed time.
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let (status, read) = call_in_region(
                &router,
                REGION,
                "GET",
                &format!("/2025-09-09/microvm-images/{name}"),
                None,
            )
            .await;
            assert_eq!(status, 200, "read the image {name}: got {status} {read}");
            if read["latestActiveImageVersion"].is_string() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the image {name} never settled with an active version: last read {read}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    let (status, run) = call_in_region(
        &router,
        REGION,
        "POST",
        "/2025-09-09/microvms",
        Some(json!({"imageIdentifier": name})),
    )
    .await;
    assert_eq!(status, 200, "run in {REGION}: got {status} {run}");
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
    let endpoint = run["endpoint"].as_str().unwrap_or_default().to_string();

    // A second region holding a MicroVM under the SAME id. The ids this service mints are
    // unique across the store, so the collision is written into the store directly — the
    // point of the rule is that the token must not be accepted for it even if one ever
    // arose. The region sorts before the minting one, so it is the partition the
    // endpoint's address (which carries the id and no region) resolves the id in.
    let other_region = "eu-central-1";
    {
        let state = engine.state::<MicrovmState>();
        let mut store = state.lock();
        let mut twin = store
            .microvm(REGION, &microvm_id)
            .cloned()
            .unwrap_or_else(|| panic!("the MicroVM {microvm_id} was run in {REGION}"));
        twin.microvm_arn = format!("arn:aws:lambda:{other_region}:{ACCOUNT}:microvm:{microvm_id}");
        store.insert_microvm(other_region, twin);
    }

    // The token minted at the MINTING region's endpoint grants nothing in the partition
    // the address resolves in.
    let home_token = mint_auth_token(&router, REGION, &microvm_id).await;
    let (status, _, body) = through_endpoint(&router, &engine, &endpoint, Some(&home_token)).await;
    assert_eq!(
        status, 403,
        "case a token minted in {REGION} presented for the {other_region} partition: expected \
         403, got {status} {body}"
    );

    // The control: a token minted at THAT region's endpoint is accepted there, so the
    // refusal above is the region check and not a dead path. (This image's code artifact
    // was never served, so the MicroVM holds no guest program to run and the proxy
    // answers for a MicroVM it reached nothing inside of — see `microvm_endpoint`.)
    let foreign_token = mint_auth_token(&router, other_region, &microvm_id).await;
    let (status, _, body) =
        through_endpoint(&router, &engine, &endpoint, Some(&foreign_token)).await;
    assert_eq!(
        status,
        NO_GUEST_RESPONSE_STATUS,
        "case a token minted in {other_region}: expected the authenticated answer, got {status} \
         {body}"
    );
}

/// Mint an authentication token for `microvm_id` at `region`'s endpoint, scoped to every
/// port, and return its value.
async fn mint_auth_token(router: &MultiServiceRouter, region: &str, microvm_id: &str) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "POST",
        &format!("/2025-09-09/microvms/{microvm_id}/auth-token"),
        Some(json!({"expirationInMinutes": 15, "allowedPorts": [{"allPorts": {}}]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "mint a token for {microvm_id} in {region}: got {status} {response}"
    );
    response["authToken"][AUTH_TOKEN_HEADER]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One request to the address a MicroVM is published on, answered by the one decision the
/// core's listener hands every request there (`state::microvm_endpoint::serve_request`).
async fn through_endpoint(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    endpoint: &str,
    token: Option<&str>,
) -> (u16, String, String) {
    let mut builder = http::Request::builder().method("GET").uri(endpoint);
    if let Some(token) = token {
        builder = builder.header(AUTH_TOKEN_HEADER, token);
    }
    let request = builder
        .body(Vec::new())
        .expect("a well-formed request to a MicroVM endpoint");
    let state = engine.state::<MicrovmState>();
    let connectors = connector_state(&**engine);
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let response = serve_request(&state, connectors.as_ref(), &callback, &request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, content_type, body)
}

/// RULE: a MicroVM is regional for the same reason its image is — `RunMicrovm` mints its
/// ARN in the region the request was signed for — so it is readable only through that
/// region's endpoint and listed by no other.
#[tokio::test]
async fn a_microvm_is_addressable_only_through_the_region_that_ran_it() {
    let router = emulator();

    let name = "regional-runtime";
    let (status, created) = create_image(&router, REGION, name, json!({})).await;
    assert_eq!(status, 201, "create in {REGION}: got {status} {created}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, run) = call_in_region(
        &router,
        REGION,
        "POST",
        "/2025-09-09/microvms",
        Some(json!({"imageIdentifier": name})),
    )
    .await;
    assert_eq!(status, 200, "run in {REGION}: got {status} {run}");
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
    let microvm_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm:{microvm_id}");

    for region in FOREIGN_REGIONS {
        // Rows differ in which form `MicrovmIdentifier` the request carries ("The ARN or
        // ID of the MicroVm"), because neither resolves outside its own region.
        for identifier in [microvm_id.as_str(), microvm_arn.as_str()] {
            let uri = format!("/2025-09-09/microvms/{}", as_uri_value(identifier));
            let (status, body) = call_in_region(&router, region, "GET", &uri, None).await;
            assert_not_found(
                status,
                &body,
                &microvm_not_found_message(identifier),
                &format!("GetMicrovm for {identifier} from {region}"),
            );
        }

        let listed = list_microvms(&router, region).await;
        assert!(
            listed.is_empty(),
            "case {region}: the MicroVM of another region must not be listed, got {listed:?}"
        );
    }

    let uri = format!("/2025-09-09/microvms/{}", as_uri_value(&microvm_id));
    let (status, home) = call_in_region(&router, REGION, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {REGION}: the running region still serves the MicroVM, got {status} {home}"
    );
    let listed = list_microvms(&router, REGION).await;
    assert_eq!(
        listed.len(),
        1,
        "case {REGION}: the running region lists its own MicroVM, got {listed:?}"
    );
}
