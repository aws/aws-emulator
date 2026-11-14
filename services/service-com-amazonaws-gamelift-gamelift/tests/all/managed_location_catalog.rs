// SPDX-License-Identifier: CC0-1.0

//! Two response-fidelity contracts, over the wire.
//!
//! `ListLocations` under the `AWS` filter serves the service's published
//! managed-location catalog — the Amazon Web Services Regions and the Local Zones of
//! those Regions it hosts game servers in — each carrying the UDP ping beacon a player
//! device measures network latency against, and `Limit`/`NextToken` page over that whole
//! set.
//!
//! `DeleteMatchmakingRuleSet` refuses a rule set that resolves to nothing with the text
//! THAT operation words a missing rule set with, which is not the text
//! `DescribeMatchmakingRuleSets` words the same missing rule set with: the message a
//! refusal carries belongs to the operation and the error class, not to the resource.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `error_code`, `error_message`) is
//! the one `hosting_loop` already drives the production dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, error_code, error_message, gamelift_arn, GameLift, REGION,
};

/// The port every managed location's ping beacon answers on: `UDPEndpoint$Port`'s own doc
/// — "For Amazon GameLift Servers ping beacons, this is typically port 7770".
const PING_BEACON_PORT: u64 = 7770;

/// The ARN a rule set named `name` is assigned, the second identifier form
/// `MatchmakingRuleSetName` accepts (`^([a-zA-Z0-9-\.]*|arn:.*:matchmakingruleset\/
/// [a-zA-Z0-9-\.]*)$`).
fn rule_set_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingruleset/{name}"))
}

/// The locations `ListLocations` reports for `request`.
async fn list_locations(gamelift: &GameLift, request: Value) -> Vec<Value> {
    let listed = gamelift.ok("ListLocations", request).await;
    listed["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListLocations must report a list, got {listed}"))
}

/// The `LocationName`s `ListLocations` reports for `request`, in order.
async fn listed_names(gamelift: &GameLift, request: Value) -> Vec<String> {
    list_locations(gamelift, request)
        .await
        .iter()
        .map(|location| {
            location["LocationName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The entry of `locations` named `name`, or `None`.
fn entry<'a>(locations: &'a [Value], name: &str) -> Option<&'a Value> {
    locations
        .iter()
        .find(|location| location["LocationName"] == json!(name))
}

/// Rows of the published catalog, each with the ping-beacon domain its name implies.
///
/// Drawn from the Amazon GameLift Servers service-location reference the `ListLocations`
/// documentation links to ("Learn more Service locations"), which the emulator's own
/// catalog (`crate::state::MANAGED_LOCATIONS`) carries: Regions, whose beacon is
/// `gamelift-ping.<region>.api.aws`, and Local Zones of a Region, whose beacon is
/// `gamelift-ping-<zone code>.<parent region>.api.aws`. The rows deliberately include
/// locations in both forms, in Regions on three continents, and are not only the ones the
/// recorded response happened to list first.
const CATALOG_ROWS: &[(&str, &str)] = &[
    // A Region in each of three continents, none of them the Region these requests are
    // signed for.
    ("af-south-1", "gamelift-ping.af-south-1.api.aws"),
    ("eu-west-2", "gamelift-ping.eu-west-2.api.aws"),
    ("ap-northeast-1", "gamelift-ping.ap-northeast-1.api.aws"),
    // The Region the requests ARE signed for, which the catalog carries like any other.
    ("us-west-2", "gamelift-ping.us-west-2.api.aws"),
    // Local Zones of two different parent Regions.
    ("us-east-1-atl-1", "gamelift-ping-atl-1.us-east-1.api.aws"),
    ("us-west-2-lax-1", "gamelift-ping-lax-1.us-west-2.api.aws"),
    // And two the recorded response did list, one Region and one Local Zone.
    ("ap-south-1", "gamelift-ping.ap-south-1.api.aws"),
    ("us-east-1-mia-2", "gamelift-ping-mia-2.us-east-1.api.aws"),
];

/// The `AWS` filter serves the published catalog of managed locations, every one of them
/// carrying its ping beacon and none of them an ARN.
#[tokio::test]
async fn the_aws_filter_serves_the_published_catalog_with_a_ping_beacon_per_location() {
    let gamelift = GameLift::new();
    let listed = list_locations(&gamelift, json!({"Filters": ["AWS"], "Limit": 200})).await;
    let names: Vec<&str> = listed
        .iter()
        .map(|location| location["LocationName"].as_str().unwrap_or_default())
        .collect();

    // The catalog is a SET of locations, not the one Region the request was served in.
    assert!(
        listed.len() > 1,
        "the managed-location catalog must hold more than the Region a request is served \
         in, got {names:?}"
    );

    for (location, domain) in CATALOG_ROWS {
        let Some(reported) = entry(&listed, location) else {
            panic!("case {location:?}: the catalog must carry this location, got {names:?}");
        };
        assert_eq!(
            reported["PingBeacon"],
            json!({"UDPEndpoint": {"Domain": domain, "Port": PING_BEACON_PORT}}),
            "case {location:?}: expected the location's ping beacon, got {reported}"
        );
        assert!(
            reported["LocationArn"].is_null(),
            "case {location:?}: an ARN is assigned to a custom location only, got {reported}"
        );
    }

    // And that holds for EVERY managed location, not only the rows above: each carries a
    // beacon in the service's published form on the documented port.
    for reported in &listed {
        let name = reported["LocationName"].as_str().unwrap_or_default();
        let domain = reported["PingBeacon"]["UDPEndpoint"]["Domain"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            domain.starts_with("gamelift-ping") && domain.ends_with(".api.aws"),
            "case {name:?}: expected a published ping-beacon domain, got {reported}"
        );
        assert_eq!(
            reported["PingBeacon"]["UDPEndpoint"]["Port"].as_u64(),
            Some(PING_BEACON_PORT),
            "case {name:?}: expected the documented beacon port, got {reported}"
        );
        assert!(
            reported["LocationArn"].is_null(),
            "case {name:?}: a managed location carries no ARN, got {reported}"
        );
    }

    // Reading the catalog changed nothing a later caller sees.
    let again = list_locations(&gamelift, json!({"Filters": ["AWS"], "Limit": 200})).await;
    assert_eq!(
        again, listed,
        "listing the locations must not change what a later list reports, got {again:?}"
    );

    // A location this store knows about only because a fleet deploys to it is reported
    // too — it is a location the account can host in — but it carries NO beacon: the
    // service publishes one only for the locations it operates.
    let unpublished = "ap-southeast-4";
    create_fleet(
        &gamelift,
        anywhere_fleet("beacons-remote-fleet", &[unpublished]),
    )
    .await;
    let with_fleet = list_locations(&gamelift, json!({"Filters": ["AWS"], "Limit": 200})).await;
    let Some(reported) = entry(&with_fleet, unpublished) else {
        panic!(
            "case {unpublished:?}: a location a fleet deploys to must be reported, got \
             {with_fleet:?}"
        );
    };
    assert!(
        reported["PingBeacon"].is_null(),
        "case {unpublished:?}: a location the service does not publish carries no beacon, got \
         {reported}"
    );
    // And every published entry still carries its own.
    for (location, domain) in CATALOG_ROWS {
        let Some(published) = entry(&with_fleet, location) else {
            panic!("case {location:?}: the catalog must still carry this location");
        };
        assert_eq!(
            published["PingBeacon"],
            json!({"UDPEndpoint": {"Domain": domain, "Port": PING_BEACON_PORT}}),
            "case {location:?}: expected the location's ping beacon, got {published}"
        );
    }
}

/// A custom location is reported beside the catalog, with the ARN the create minted and no
/// ping beacon: the service operates no beacon for a caller's own hosting hardware.
#[tokio::test]
async fn a_custom_location_carries_its_arn_and_no_ping_beacon() {
    let gamelift = GameLift::new();
    for name in ["custom-rack-1", "custom-rack-2"] {
        gamelift
            .ok("CreateLocation", json!({"LocationName": name}))
            .await;
    }

    let custom = list_locations(&gamelift, json!({"Filters": ["CUSTOM"], "Limit": 200})).await;
    for name in ["custom-rack-1", "custom-rack-2"] {
        let Some(reported) = entry(&custom, name) else {
            panic!("case {name:?}: the created location must be reported, got {custom:?}");
        };
        assert_eq!(
            reported["LocationArn"],
            json!(gamelift_arn(&format!("location/{name}"))),
            "case {name:?}: expected the ARN the create minted, got {reported}"
        );
        assert!(
            reported["PingBeacon"].is_null(),
            "case {name:?}: a custom location carries no ping beacon, got {reported}"
        );
    }

    // The unfiltered list holds both kinds: the catalog and the custom locations.
    let both = listed_names(&gamelift, json!({"Limit": 200})).await;
    for name in ["custom-rack-1", "us-west-2", "ap-south-1", "us-east-1-atl-1"] {
        assert!(
            both.contains(&name.to_string()),
            "case {name:?}: an unfiltered list holds both kinds of location, got {both:?}"
        );
    }
}

/// `Limit` and `NextToken` page over the whole catalog, at every page size, and the
/// modelled bounds of `Limit` are the ones the operation honours.
#[tokio::test]
async fn the_catalog_is_paged_by_limit_and_next_token() {
    let gamelift = GameLift::new();
    gamelift
        .ok("CreateLocation", json!({"LocationName": "custom-paged"}))
        .await;

    let whole = listed_names(&gamelift, json!({})).await;
    assert!(
        whole.len() > 2,
        "the catalog must be long enough for a page to fill, got {whole:?}"
    );

    // `Limit` is modelled `ListLocationsLimit` with range 1..200, so these page sizes span
    // the smallest page, several in between, and the largest the model allows.
    for page_size in [1_usize, 2, 7, 25, 200] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"Limit": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = gamelift.ok("ListLocations", request).await;
            let page = response["Locations"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most Limit locations, got {}",
                page.len()
            );
            walked.extend(page.iter().map(|location| {
                location["LocationName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case page size {page_size}: the paged walk must report every location once, in \
             the order the unpaged list reports, got {walked:?}"
        );
    }

    // A `Limit` one past either modelled bound is refused, and nothing is served for it.
    for limit in [0, 201] {
        let (status, code) = gamelift.err("ListLocations", json!({"Limit": limit})).await;
        assert_eq!(
            status, 400,
            "case Limit {limit}: a Limit outside the modelled range must be refused with 400, \
             got {status} {code}"
        );
    }
}

/// One row of the not-found table: the rule-set identifier a refused call names.
struct MissingRuleSet {
    label: &'static str,
    /// The `Name` the call carries.
    name: String,
    /// Whether a rule set of that name is created and deleted first, so the name resolved
    /// to a rule set once and resolves to nothing now.
    deleted_first: bool,
}

/// A rule set body the service accepts, so the setup below is not refused for its body.
fn conformant_body() -> String {
    json!({
        "name": "aliens-vs-cowboys",
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "players", "maxPlayers": 2, "minPlayers": 1}],
    })
    .to_string()
}

/// `DeleteMatchmakingRuleSet` words a rule set that resolves to nothing with its own
/// `NotFoundException` text, over a set of identifiers; `DescribeMatchmakingRuleSets`
/// words the same missing rule set with the text IT answers a failed lookup in.
///
/// The rows differ along what the rule generalises over: the LENGTH of the identifier at
/// each bound `MatchmakingRuleSetName` declares (len 1..256), the characters its pattern
/// allows (`^([a-zA-Z0-9-\.]*|arn:.*:matchmakingruleset\/[a-zA-Z0-9-\.]*)$`: digits,
/// hyphens, dots, upper case), which of the two identifier FORMS the call names the rule
/// set by, and whether the name never existed or was deleted.
#[tokio::test]
async fn a_rule_set_that_resolves_to_nothing_is_refused_in_each_operations_own_wording() {
    let gamelift = GameLift::new();

    let rows = vec![
        MissingRuleSet {
            label: "a name no rule set ever carried",
            name: "no-such-rule-set".to_string(),
            deleted_first: false,
        },
        MissingRuleSet {
            label: "the shortest name the model allows (1 character)",
            name: "a".to_string(),
            deleted_first: false,
        },
        MissingRuleSet {
            label: "the longest name the model allows (256 characters)",
            name: "r".repeat(256),
            deleted_first: false,
        },
        MissingRuleSet {
            label: "dots, digits and upper case, which the pattern allows",
            name: "Probe.Rules.7-v2".to_string(),
            deleted_first: false,
        },
        MissingRuleSet {
            label: "an ARN no rule set carries",
            name: rule_set_arn("never-created.rules"),
            deleted_first: false,
        },
        MissingRuleSet {
            label: "a name a rule set carried until it was deleted",
            name: "gone.rules".to_string(),
            deleted_first: true,
        },
        MissingRuleSet {
            label: "the ARN of a rule set that was deleted",
            name: rule_set_arn("gone-by-arn.rules"),
            deleted_first: true,
        },
    ];

    for case in &rows {
        let label = case.label;
        if case.deleted_first {
            // The ARN form names the rule set whose name is its last segment.
            let created = case
                .name
                .rsplit('/')
                .next()
                .expect("an identifier has a last segment")
                .to_string();
            gamelift
                .ok(
                    "CreateMatchmakingRuleSet",
                    json!({"Name": &created, "RuleSetBody": conformant_body()}),
                )
                .await;
            gamelift
                .ok("DeleteMatchmakingRuleSet", json!({"Name": &created}))
                .await;
        }

        let (status, parsed) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &case.name}))
            .await;
        let code = error_code(&parsed);
        let message = error_message(&parsed);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "NotFoundException",
                format!("Matchmaking rule set {} not found", case.name).as_str()
            ),
            "case {label:?}: expected the delete's own not-found refusal, got {status} {code} \
             {message:?}"
        );

        // The same missing rule set, named to the describe, is the refusal THAT operation
        // words — a different error class and a different sentence.
        let (status, parsed) = gamelift
            .call(
                "DescribeMatchmakingRuleSets",
                &json!({"Names": [&case.name]}),
            )
            .await;
        let code = error_code(&parsed);
        let message = error_message(&parsed);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "InvalidRequestException",
                format!("Failed to find rule set: {}", case.name).as_str()
            ),
            "case {label:?}: expected the describe's own refusal, got {status} {code} \
             {message:?}"
        );
    }

    // The control: a rule set that DOES resolve is deleted rather than refused, and the
    // same delete then answers the not-found refusal for it.
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "live.rules", "RuleSetBody": conformant_body()}),
        )
        .await;
    for selector in ["live.rules".to_string(), rule_set_arn("live.rules")] {
        let served = selector == "live.rules";
        let (status, parsed) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {selector:?}: a stored rule set must be deleted, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_message(&parsed).as_str()),
                (
                    400,
                    format!("Matchmaking rule set {selector} not found").as_str()
                ),
                "case {selector:?}: the deleted rule set must be not-found by either \
                 identifier form, got {status} {parsed}"
            );
        }
    }

    // And an identifier of another Region names no rule set this store holds — a further
    // form of "resolves to nothing", refused with the same template carrying the
    // identifier the request supplied verbatim.
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "elsewhere.rules", "RuleSetBody": conformant_body()}),
        )
        .await;
    let foreign = format!(
        "arn:aws:gamelift:eu-central-1:{}:matchmakingruleset/elsewhere.rules",
        engine::emulator_account_id()
    );
    let (status, parsed) = gamelift
        .call("DeleteMatchmakingRuleSet", &json!({"Name": &foreign}))
        .await;
    assert_eq!(
        (status, error_message(&parsed).as_str()),
        (
            400,
            format!("Matchmaking rule set {foreign} not found").as_str()
        ),
        "an ARN of another Region must be refused the same way (the rule set was created \
         in {REGION}), got {status} {parsed}"
    );
}
