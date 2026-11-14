// SPDX-License-Identifier: CC0-1.0

//! WorkSpaces Instances is a REGIONAL service: a resource belongs to the Region the
//! create call that made it was signed for, and is listed, read, tagged and deleted
//! only through that Region — a call signed for any other Region sees an account that
//! does not hold it.
//!
//! The rule is asserted over a TABLE of ordered Region pairs, so no case is the one
//! Region pair the implementation happens to special-case: every pair of the Regions
//! below plays both the creating and the foreign side, for instances AND for volumes,
//! for the idempotency token a create carries, and for the per-Region quota counters.
//!
//! The Regions are rows of the service's own availability set
//! (`wsinst::state::SUPPORTED_REGIONS`, what `ListRegions` reports), and the last suite
//! here covers the other side of that set: a Region the service is not available in
//! holds no resources and admits none.
//!
//! Everything goes through the router, over the wire this service speaks (awsJson1_0,
//! `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`), with the Region
//! travelling where it really travels — the SigV4 credential scope.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call_in_region, error_code, harness, minimal_create};

/// The Regions the isolation rule is asserted over, as ordered pairs: four rows of the
/// service's availability set spanning four continents, so a pair never shares a prefix
/// (`us-east-1` / `us-east-2` would) and the Region string cannot be matched by shape.
/// Membership is checked against `wsinst::state::SUPPORTED_REGIONS` below, so a row that
/// drifts out of the set fails rather than silently testing an unsupported Region.
const REGION_ROWS: &[&str] = &["us-east-1", "eu-west-3", "ap-northeast-2", "sa-east-1"];

/// Regions this service is NOT available in, each with why: the two partitions its
/// availability set deliberately excludes, and commercial Regions it has not launched
/// in. Each row is asserted absent from `wsinst::state::SUPPORTED_REGIONS`.
const UNSUPPORTED_REGION_ROWS: &[(&str, &str)] = &[
    ("the GovCloud partition", "us-gov-west-1"),
    ("the GovCloud partition", "us-gov-east-1"),
    ("the China partition", "cn-north-1"),
    ("the China partition", "cn-northwest-1"),
    ("a commercial Region the service has not launched in", "us-west-1"),
    ("a commercial Region the service has not launched in", "eu-central-2"),
];

/// Every ordered pair of distinct [`REGION_ROWS`]: `(created_in, foreign)`.
fn region_pairs() -> Vec<(&'static str, &'static str)> {
    for region in REGION_ROWS {
        assert!(
            wsinst::state::SUPPORTED_REGIONS.contains(region),
            "case {region:?}: this suite's Regions must be ones the service is available in, \
             which are {:?}",
            wsinst::state::SUPPORTED_REGIONS
        );
    }
    REGION_ROWS
        .iter()
        .flat_map(|created_in| {
            REGION_ROWS
                .iter()
                .filter(move |foreign| foreign != &created_in)
                .map(move |foreign| (*created_in, *foreign))
        })
        .collect()
}

/// One call signed for `region`, asserting a 200 and returning the body.
async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, payload) = call_in_region(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} in {region} to answer HTTP 200, got {status} \
         with {payload}"
    );
    payload
}

/// `CreateWorkspaceInstance` signed for `region`, returning the minted id.
async fn create_instance_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: Value,
) -> String {
    let payload = ok_in(router, region, case, "CreateWorkspaceInstance", body).await;
    payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
        .to_string()
}

/// `CreateVolume` signed for `region`, in that Region's zone `letter`.
async fn create_volume_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    letter: char,
    extra: Value,
) -> String {
    let mut body = json!({
        "AvailabilityZone": format!("{region}{letter}"),
        "SizeInGB": 4,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    let payload = ok_in(router, region, case, "CreateVolume", body).await;
    payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no VolumeId in {payload}"))
        .to_string()
}

/// Every `WorkspaceInstanceId` `ListWorkspaceInstances` reports to a caller in
/// `region`, paging to the end.
async fn listed_ids_in(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<String> {
    let mut request = json!({});
    let mut ids = Vec::new();
    loop {
        let page = ok_in(router, region, case, "ListWorkspaceInstances", request.clone()).await;
        let instances = page["WorkspaceInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstances in {page}"))
            .clone();
        for instance in instances {
            ids.push(
                instance["WorkspaceInstanceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => request["NextToken"] = json!(token),
            None => return ids,
        }
    }
}

/// The not-found refusal this service answers for an id the Region does not hold: its
/// own status, the modelled code, the id that was looked up and the resource kind.
fn assert_not_found(case: &str, status: u16, payload: &Value, id: &str, resource_type: &str) {
    let expected_status = wsinst::state::RESOURCE_NOT_FOUND_STATUS;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} for {id:?}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException for {id:?}, got {payload}"
    );
    assert_eq!(
        payload["ResourceId"].as_str(),
        Some(id),
        "case {case:?}: expected the refusal to name {id:?}, got {payload}"
    );
    assert_eq!(
        payload["ResourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected the resource kind {resource_type:?}, got {payload}"
    );
}

/// The tags `ListTagsForResource` reports to a caller in `region`.
async fn tags_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    id: &str,
) -> std::collections::BTreeMap<String, String> {
    let payload = ok_in(
        router,
        region,
        case,
        "ListTagsForResource",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    payload["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Tags in {payload}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_instance_is_listed_only_in_the_region_it_was_created_in() {
    for (created_in, foreign) in region_pairs() {
        let case = format!("created in {created_in}, listed from {foreign}");
        let (router, _engine) = harness();
        let id = create_instance_in(&router, created_in, &case, minimal_create()).await;

        let own = listed_ids_in(&router, created_in, &case).await;
        assert_eq!(
            own,
            vec![id.clone()],
            "case {case:?}: the creating Region must report exactly its own instance, got {own:?}"
        );

        let elsewhere = listed_ids_in(&router, foreign, &case).await;
        assert!(
            elsewhere.is_empty(),
            "case {case:?}: a Region that holds no instance must report an empty list, got \
             {elsewhere:?}"
        );
    }
}

#[tokio::test]
async fn every_region_lists_its_own_instances_and_no_others() {
    // One account, one instance created through each Region's endpoint: each Region
    // reports exactly the instance created through it, whatever the others hold.
    let (router, _engine) = harness();
    let mut created: Vec<(&str, String)> = Vec::new();
    for region in REGION_ROWS {
        let case = format!("one instance per Region, creating in {region}");
        let id = create_instance_in(&router, region, &case, minimal_create()).await;
        created.push((region, id));
    }

    for (region, id) in &created {
        let case = format!("one instance per Region, listing in {region}");
        let listed = listed_ids_in(&router, region, &case).await;
        assert_eq!(
            listed,
            vec![id.clone()],
            "case {case:?}: expected only {id:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_instance_is_readable_taggable_and_deletable_only_in_its_own_region() {
    for (created_in, foreign) in region_pairs() {
        let case = format!("created in {created_in}, reached from {foreign}");
        let (router, _engine) = harness();
        let mut body = minimal_create();
        body["Tags"] = json!([{ "Key": "owner", "Value": "home-region" }]);
        let id = create_instance_in(&router, created_in, &case, body).await;

        // Every single-instance operation the foreign Region can name answers the
        // refusal this service gives an id it never minted.
        for (operation, request) in [
            (
                "GetWorkspaceInstance",
                json!({ "WorkspaceInstanceId": &id }),
            ),
            ("ListTagsForResource", json!({ "WorkspaceInstanceId": &id })),
            (
                "TagResource",
                json!({
                    "WorkspaceInstanceId": &id,
                    "Tags": [{ "Key": "owner", "Value": "foreign-region" }],
                }),
            ),
            (
                "DeleteWorkspaceInstance",
                json!({ "WorkspaceInstanceId": &id }),
            ),
        ] {
            let (status, payload) = call_in_region(&router, foreign, operation, request).await;
            assert_not_found(
                &format!("{case}: {operation}"),
                status,
                &payload,
                &id,
                wsinst::state::WORKSPACE_INSTANCE_RESOURCE_TYPE,
            );
        }

        // None of those refusals touched the instance: its own Region still reports it,
        // with the tag its create set and no teardown started.
        let read = ok_in(
            &router,
            created_in,
            &case,
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &id }),
        )
        .await;
        assert_eq!(
            read["WorkspaceInstanceId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the creating Region must still report the instance, got {read}"
        );
        let state = read["ProvisionState"].as_str().unwrap_or_default();
        assert_ne!(
            state, "DEALLOCATING",
            "case {case:?}: a delete from another Region must not start the teardown, got \
             {read}"
        );
        let tags = tags_in(&router, created_in, &case, &id).await;
        assert_eq!(
            tags.get("owner").map(String::as_str),
            Some("home-region"),
            "case {case:?}: a TagResource from another Region must not change the tags, got \
             {tags:?}"
        );

        // And the instance's own Region can still do all of it.
        ok_in(
            &router,
            created_in,
            &case,
            "TagResource",
            json!({
                "WorkspaceInstanceId": &id,
                "Tags": [{ "Key": "owner", "Value": "retagged" }],
            }),
        )
        .await;
        let tags = tags_in(&router, created_in, &case, &id).await;
        assert_eq!(
            tags.get("owner").map(String::as_str),
            Some("retagged"),
            "case {case:?}: the instance's own Region must be able to tag it, got {tags:?}"
        );
        ok_in(
            &router,
            created_in,
            &case,
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": &id }),
        )
        .await;
    }
}

#[tokio::test]
async fn a_volume_is_deletable_only_in_the_region_it_was_created_in() {
    // The zone letter varies with the row so the rule is asserted across the zones a
    // Region has — each Region's OWN letters
    // (`wsinst::state::availability_zone_letters`), which differ per Region — not just
    // the first one.
    for (row, (created_in, foreign)) in region_pairs().into_iter().enumerate() {
        let letters: Vec<char> = wsinst::state::availability_zone_letters(created_in)
            .chars()
            .collect();
        let letter = letters[row % letters.len()];
        let case = format!("volume in {created_in}{letter}, deleted from {foreign}");
        let (router, _engine) = harness();
        let id = create_volume_in(&router, created_in, &case, letter, json!({})).await;

        let (status, payload) =
            call_in_region(&router, foreign, "DeleteVolume", json!({ "VolumeId": &id })).await;
        assert_not_found(
            &case,
            status,
            &payload,
            &id,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );

        // The volume survived the foreign delete, so its own Region can still delete it
        // — and only once.
        ok_in(
            &router,
            created_in,
            &case,
            "DeleteVolume",
            json!({ "VolumeId": &id }),
        )
        .await;
        let (status, payload) = call_in_region(
            &router,
            created_in,
            "DeleteVolume",
            json!({ "VolumeId": &id }),
        )
        .await;
        assert_not_found(
            &format!("{case}: deleted twice in its own Region"),
            status,
            &payload,
            &id,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );
    }
}

#[tokio::test]
async fn a_client_token_is_scoped_to_the_region_the_create_was_signed_for() {
    for (created_in, foreign) in region_pairs() {
        let case = format!("token spent in {created_in}, presented in {foreign}");
        let (router, _engine) = harness();
        let token = format!("tok-{created_in}-{foreign}");

        // An instance create: the SAME request in another Region asks a Region that
        // holds nothing for an instance, so it creates its own rather than replaying.
        let mut body = minimal_create();
        body["ClientToken"] = json!(token);
        let home = create_instance_in(&router, created_in, &case, body.clone()).await;
        let away = create_instance_in(&router, foreign, &case, body.clone()).await;
        assert_ne!(
            home, away,
            "case {case:?}: a token spent in one Region must not hand its instance to another"
        );
        let listed_home = listed_ids_in(&router, created_in, &case).await;
        let listed_away = listed_ids_in(&router, foreign, &case).await;
        assert_eq!(
            (listed_home.as_slice(), listed_away.as_slice()),
            (
                std::slice::from_ref(&home),
                std::slice::from_ref(&away)
            ),
            "case {case:?}: each Region must hold only the instance created through it"
        );

        // The same request again, in the Region the token was spent in: the token is
        // spent there, so it is refused and creates nothing.
        let (status, payload) =
            call_in_region(&router, created_in, "CreateWorkspaceInstance", body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 for the spent token in its own Region, got \
             {status} with {payload}"
        );
        assert_eq!(
            payload["ResourceId"].as_str(),
            Some(home.as_str()),
            "case {case:?}: expected the conflict in the token's own Region to name {home:?}, \
             got {payload}"
        );
        let listed_home = listed_ids_in(&router, created_in, &case).await;
        assert_eq!(
            listed_home,
            vec![home.clone()],
            "case {case:?}: the refusal must create no second instance, got {listed_home:?}"
        );

        // A volume create carries its Region in `AvailabilityZone` as well, so the same
        // token in another Region is a create with different parameters: scoped to its
        // Region it is a new volume, and both volumes stay reachable only from their own
        // Region.
        let home_volume = create_volume_in(
            &router,
            created_in,
            &case,
            'a',
            json!({ "ClientToken": token }),
        )
        .await;
        let away_volume = create_volume_in(
            &router,
            foreign,
            &case,
            'a',
            json!({ "ClientToken": token }),
        )
        .await;
        assert_ne!(
            home_volume, away_volume,
            "case {case:?}: a volume token spent in one Region must mint a separate volume \
             in another"
        );
        let (status, payload) = call_in_region(
            &router,
            foreign,
            "DeleteVolume",
            json!({ "VolumeId": &home_volume }),
        )
        .await;
        assert_not_found(
            &format!("{case}: the foreign Region deleting the home volume"),
            status,
            &payload,
            &home_volume,
            wsinst::state::VOLUME_RESOURCE_TYPE,
        );
    }
}

#[tokio::test]
async fn the_quota_counters_are_per_region() {
    // A Service Quotas limit applies to one account in one Region, so what one Region
    // holds leaves another Region's capacity untouched. The counters are what the
    // creates are checked against, so they are read out of the store the router shares.
    let (router, engine) = harness();
    // Rows differ in how many instances and volumes the Region holds, and one of them
    // has an instance tearing down: a deleted instance frees its slot while its record
    // is still held.
    let rows: [(&str, usize, usize, usize); 4] = [
        ("us-east-1", 3, 1, 2),
        ("eu-west-3", 1, 0, 1),
        ("ap-northeast-2", 2, 1, 0),
        ("sa-east-1", 0, 0, 3),
    ];

    for (region, instances, deleted, volumes) in rows {
        let case = format!("{region} holds {instances} instances and {volumes} volumes");
        let mut ids = Vec::new();
        for _ in 0..instances {
            ids.push(create_instance_in(&router, region, &case, minimal_create()).await);
        }
        for id in ids.iter().take(deleted) {
            ok_in(
                &router,
                region,
                &case,
                "DeleteWorkspaceInstance",
                json!({ "WorkspaceInstanceId": id }),
            )
            .await;
        }
        let letters: Vec<char> = wsinst::state::availability_zone_letters(region)
            .chars()
            .collect();
        for index in 0..volumes {
            let letter = letters[index % letters.len()];
            create_volume_in(&router, region, &case, letter, json!({})).await;
        }
    }

    let store = wsinst::state::service_state(&*engine).expect("the service's own store");
    for (region, instances, deleted, volumes) in rows {
        let case = format!("{region} holds {instances} instances and {volumes} volumes");
        let (running, held, stored_volumes) = {
            let state = store.lock();
            (
                state.running_instance_count(region),
                state.held_instance_count(region),
                state.volume_count(region),
            )
        };
        assert_eq!(
            (running, held, stored_volumes),
            (instances - deleted, instances, volumes),
            "case {case:?}: expected ({}, {instances}, {volumes}) for (running, held, volumes) \
             in this Region, got ({running}, {held}, {stored_volumes})",
            instances - deleted
        );
        let has_capacity = store.lock().has_instance_capacity(region);
        assert!(
            has_capacity,
            "case {case:?}: a Region holding {instances} instances is nowhere near the quota \
             of {}",
            wsinst::state::MAX_INSTANCES
        );
    }
}

#[tokio::test]
async fn a_create_signed_for_a_region_the_service_is_not_available_in_is_refused() {
    for (why, region) in UNSUPPORTED_REGION_ROWS {
        let case = format!("{region} ({why})");
        assert!(
            !wsinst::state::SUPPORTED_REGIONS.contains(region),
            "case {case:?}: this row must name a Region outside the service's availability \
             set, which is {:?}",
            wsinst::state::SUPPORTED_REGIONS
        );
        let (router, engine) = harness();

        for (operation, request) in [
            ("CreateWorkspaceInstance", minimal_create()),
            (
                "CreateVolume",
                json!({ "AvailabilityZone": format!("{region}a"), "SizeInGB": 4 }),
            ),
        ] {
            let (status, payload) = call_in_region(&router, region, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to answer HTTP 400, got {status} with \
                 {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: expected {operation} to answer ValidationException, got \
                 {payload}"
            );
            assert_eq!(
                payload["Reason"].as_str(),
                Some(wsinst::state::VALIDATION_REASON_UNSUPPORTED_OPERATION),
                "case {case:?}: expected the reason for a call this service cannot serve, got \
                 {payload}"
            );
            // No member of the request is at fault — the Region travels in the
            // credential scope — so no field is named, and nothing the caller sent is
            // echoed back.
            assert!(
                payload.get("FieldList").is_none_or(Value::is_null),
                "case {case:?}: expected no FieldList for {operation}, got {payload}"
            );
            assert!(
                !payload.to_string().contains(region),
                "case {case:?}: the refusal must echo none of the request, got {payload}"
            );
        }

        let (instances, volumes) = {
            let store = wsinst::state::service_state(&*engine).expect("the service's own store");
            let state = store.lock();
            (state.instances.len(), state.volumes.len())
        };
        assert_eq!(
            (instances, volumes),
            (0, 0),
            "case {case:?}: a refused create must store nothing, got {instances} instances \
             and {volumes} volumes"
        );
    }
}

#[tokio::test]
async fn every_region_the_service_reports_can_hold_a_resource() {
    // The other side of the availability set: each Region `ListRegions` reports is one
    // a create really works in, and what it creates is visible there and nowhere else.
    let (router, _engine) = harness();
    for region in wsinst::state::SUPPORTED_REGIONS {
        let case = format!("create in the supported Region {region}");
        let id = create_instance_in(&router, region, &case, minimal_create()).await;
        let listed = listed_ids_in(&router, region, &case).await;
        assert_eq!(
            listed,
            vec![id.clone()],
            "case {case:?}: expected the Region to report only {id:?}, got {listed:?}"
        );
        // The LAST zone letter this Region publishes, which differs from Region to
        // Region (`wsinst::state::availability_zone_letters`): a Region's zone set is
        // its own, not a run of the same three letters everywhere.
        let letter = wsinst::state::availability_zone_letters(region)
            .chars()
            .next_back()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a Region the service reports must publish zones")
            });
        let volume = create_volume_in(&router, region, &case, letter, json!({})).await;
        ok_in(
            &router,
            region,
            &case,
            "DeleteVolume",
            json!({ "VolumeId": &volume }),
        )
        .await;
    }
}
