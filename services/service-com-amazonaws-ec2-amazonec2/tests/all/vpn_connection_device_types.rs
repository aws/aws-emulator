// SPDX-License-Identifier: CC0-1.0

//! `GetVpnConnectionDeviceTypes` end-to-end: which customer gateway devices the service
//! reports a sample configuration file for, and how a caller pages and dry-runs them.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test:
//!
//! 1. The operation reports EVERY device a sample configuration file is published for —
//!    facts of the SERVICE, not of a Region or of the account: one device per published
//!    (vendor, platform, software release) entry of the guide's list, so a vendor with two
//!    platforms and a platform with two releases each contribute a device of their own.
//!    Every device carries all four modelled members (`VpnConnectionDeviceTypeId`,
//!    `Vendor`, `Platform`, `Software`), ids are distinct, and the answer is the same in
//!    every Region and on every repeat, with nothing created in between.
//! 2. A device's `VpnConnectionDeviceTypeId` is a function of the device: it is stable
//!    across calls and across a freshly built emulator, so an id read once can be
//!    handed back later.
//! 3. `MaxResults` bounds one page and `NextToken` resumes it: a page holds at most
//!    `MaxResults` devices, `NextToken` is absent once no device follows the page, and a
//!    request naming no `MaxResults` returns all results. `MaxResults` outside the
//!    modelled 200..1000 range is refused at the boundary, and a `NextToken` this
//!    operation did not mint — including one this service minted for a different
//!    paginating operation — is refused with `InvalidPaginationToken`.
//! 4. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//!
//! Device MEMBERSHIP is therefore compared over a sorted set, so a row says nothing about
//! where in the response a device sits. The one ordering claim asserted is the catalog's
//! own cursor order — vendor, then platform, then software — because that is the order a
//! `NextToken` walk resumes in: it is what makes the devices of one vendor contiguous and
//! keeps a paged walk from reporting a device twice or skipping one.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::ops::get_vpn_connection_device_types::{
    decode_device_page_token, device_page_start, device_page_token,
};

use super::ec2_query::{element, error_code, unescaped};

/// One `GetVpnConnectionDeviceTypes` call carrying `params`, as its HTTP status and XML
/// body. The Region only reaches the service through the SigV4 credential scope, which
/// `super::ec2_query::call` builds.
async fn get_device_types(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "GetVpnConnectionDeviceTypes", params).await
}

/// One device of a `GetVpnConnectionDeviceTypes` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Device {
    id: String,
    vendor: String,
    platform: String,
    software: String,
}

/// Every device in an ec2Query `GetVpnConnectionDeviceTypes` body, in the order the
/// response carried them. Each device is one `<item>` of the
/// `<vpnConnectionDeviceTypeSet>` list, read with `roxmltree` through the shared
/// document helpers in `super::wire`, so an element carrying an `xmlns` attribute needs
/// no special case.
fn devices(case: &str, body: &str) -> Vec<Device> {
    let document = super::wire::document(case, body);
    let Some(list) = document.descendants().find(|node| {
        node.is_element() && node.tag_name().name() == "vpnConnectionDeviceTypeSet"
    }) else {
        return Vec::new();
    };
    list.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let member = |name: &str| {
                super::wire::child_text(item, name).unwrap_or_else(|| {
                    panic!("case {case:?}: no <{name}> in device {item:?}")
                })
            };
            Device {
                id: member("vpnConnectionDeviceTypeId"),
                vendor: member("vendor"),
                platform: member("platform"),
                software: member("software"),
            }
        })
        .collect()
}

/// The devices one unpaged call reports, asserted to be a successful answer.
async fn all_devices(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<Device> {
    let (status, body) = get_device_types(router, region, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}"
    );
    devices(case, &body)
}

// ---------------------------------------------------------------------------
// Rule 1: the devices a sample configuration file is published for
// ---------------------------------------------------------------------------

/// Every reported device carries all four modelled members, filled in, and no two
/// devices share an id.
#[tokio::test]
async fn every_device_carries_its_four_members_and_a_distinct_id() {
    let (router, _engine) = super::ec2_query::emulator();
    let reported = all_devices(&router, "us-east-1", "the whole catalog").await;

    assert!(
        !reported.is_empty(),
        "case \"the whole catalog\": expected the published devices, got none"
    );
    for device in &reported {
        for (member, value) in [
            ("VpnConnectionDeviceTypeId", &device.id),
            ("Vendor", &device.vendor),
            ("Platform", &device.platform),
            ("Software", &device.software),
        ] {
            assert!(
                !value.trim().is_empty(),
                "case {device:?}: {member} must be reported, got {value:?}"
            );
        }
        // AWS reports the identifier as eight LOWERCASE hex characters, and the model
        // documents it only as an opaque "Customer gateway device identifier".
        assert!(
            device.id.len() == 8 && device.id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "case {device:?}: expected eight lowercase hex characters, got {:?}",
            device.id
        );
    }

    let ids: BTreeSet<&String> = reported.iter().map(|device| &device.id).collect();
    assert_eq!(
        ids.len(),
        reported.len(),
        "case \"the whole catalog\": every device needs its own id, got {reported:?}"
    );
}

/// The catalog holds the devices AWS publishes an example configuration file for, named
/// by vendor and platform.
///
/// The rows are entries of the list in "Your customer gateway device"
/// (<https://docs.aws.amazon.com/vpn/latest/s2svpn/your-cgw.html>) — the page the
/// modelled `VpnConnectionDeviceType` shape's own doc links to — and they differ in
/// vendor, in how the platform is named, and in how the software release is written: a
/// single release (`6.44.3`), a floor (`JunOS 11.0+`), and a vendor with more than one
/// platform entry in the list.
#[tokio::test]
async fn the_published_devices_are_reported() {
    let (router, _engine) = super::ec2_query::emulator();
    let reported = all_devices(&router, "eu-west-1", "the published devices").await;

    // (case, vendor, platform) — the device, without pinning its id, which is derived.
    let cases: [(&str, &str, &str); 5] = [
        ("a single software release", "Mikrotik", "RouterOS"),
        ("a software floor", "Juniper", "SRX"),
        ("a second platform of one vendor", "Juniper", "J-Series"),
        ("a multi-word vendor", "Palo Alto Networks", "PAN-OS"),
        ("a firewall appliance", "Check Point", "Security Gateway"),
    ];
    for (case, vendor, platform) in cases {
        let found = reported
            .iter()
            .find(|device| device.vendor == vendor && device.platform == platform);
        assert!(
            found.is_some(),
            "case {case:?}: expected {vendor} {platform} among the devices, got {reported:?}"
        );
    }

    // A device AWS publishes no example configuration for is not reported.
    for (case, vendor) in [("no such vendor", "Acme Networks"), ("empty", "")] {
        assert!(
            !reported.iter().any(|device| device.vendor == vendor),
            "case {case:?}: {vendor:?} is published for no device, got {reported:?}"
        );
    }
}

/// The devices are facts of the service: the same answer in every Region, on every
/// repeat, and on a freshly built emulator with its own store — so the ids a caller read
/// once can be handed back later (rules 1 and 2).
#[tokio::test]
async fn the_devices_are_the_same_in_every_region_and_on_every_call() {
    let (router, _engine) = super::ec2_query::emulator();
    let baseline: BTreeSet<Device> = all_devices(&router, "us-east-1", "baseline")
        .await
        .into_iter()
        .collect();

    // The rows are Regions of different partitions and shapes, plus a repeat of the
    // Region the baseline was read in.
    for region in ["us-east-1", "eu-central-1", "ap-northeast-2", "us-gov-west-1"] {
        let again: BTreeSet<Device> = all_devices(&router, region, region).await.into_iter().collect();
        assert_eq!(
            again, baseline,
            "case {region:?}: expected the same devices as us-east-1, got {again:?}"
        );
    }

    // A second emulator, with its own engine and store, derives the same ids.
    let (fresh_router, _fresh_engine) = super::ec2_query::emulator();
    let fresh: BTreeSet<Device> = all_devices(&fresh_router, "us-east-1", "a fresh emulator")
        .await
        .into_iter()
        .collect();
    assert_eq!(
        fresh, baseline,
        "case \"a fresh emulator\": a device id is a function of the device, got {fresh:?}"
    );
}

/// The whole published list is reported, one device per published entry, in cursor order.
///
/// The rows are entries of the list in "Your customer gateway device"
/// (<https://docs.aws.amazon.com/vpn/latest/s2svpn/your-cgw.html>), the page the modelled
/// `VpnConnectionDeviceType` shape's own doc links to. They span what the published list
/// varies along: the vendor that opens it and the one that closes it, a platform the guide
/// publishes TWO software releases for, a vendor the guide publishes two and three
/// platforms for, two vendors whose names share a prefix, an operating system named as the
/// platform, and software releases written as a single version, as a floor and with a
/// wildcarded minor.
#[tokio::test]
async fn the_whole_published_device_list_is_reported_in_cursor_order() {
    let (router, _engine) = super::ec2_query::emulator();
    let reported = all_devices(&router, "us-west-2", "the published list").await;

    // (case, vendor, platform, software)
    let cases: [(&str, &str, &str, &str); 13] = [
        ("the vendor that opens the list", "AXGate", "NF", "aos 3.2+"),
        (
            "a second software release of one platform",
            "Check Point",
            "Security Gateway",
            "R80.10+",
        ),
        ("a vendor sharing a prefix with another", "Cisco Meraki", "MX", "15.12+"),
        (
            "the qualified vendor name of that prefix",
            "Cisco Systems, Inc.",
            "ASA",
            "ASA 9.7+",
        ),
        (
            "a second platform of that vendor",
            "Cisco Systems, Inc.",
            "CSR",
            "IOS-XE 16.9+",
        ),
        (
            "a newer release of one platform",
            "Fortinet",
            "Fortigate 40+ Series",
            "FortiOS 6.4.4+",
        ),
        ("a single software release", "H3C", "MSR", "Comware 7"),
        ("a third platform of one vendor", "Juniper", "ScreenOS", "6.3"),
        (
            "an operating system as the platform",
            "Microsoft",
            "Windows Server",
            "2012 R2",
        ),
        ("a second platform of one vendor", "Sophos", "UTM", "9.4+"),
        ("a router appliance", "Ubiquiti", "EdgeRouter", "EdgeOS 1.10+"),
        ("a wildcarded minor release", "WatchGuard", "Fireware", "XTM 12.x"),
        ("the vendor that closes the list", "Zyxel", "ZyWALL USG", "4.2+"),
    ];

    for (case, vendor, platform, software) in cases {
        let found = reported.iter().find(|device| {
            device.vendor == vendor && device.platform == platform && device.software == software
        });
        assert!(
            found.is_some(),
            "case {case:?}: expected the published device {vendor} / {platform} / {software} \
             among the {} reported, got {reported:?}",
            reported.len()
        );
    }

    // Two releases of one platform are two devices, each with its own id: the id is a
    // function of all three published members, not of the vendor and platform alone.
    for (case, vendor, platform) in [
        ("two releases of one firewall", "Check Point", "Security Gateway"),
        ("two releases of one appliance", "Fortinet", "Fortigate 40+ Series"),
        ("two releases of one platform", "WatchGuard", "Fireware"),
    ] {
        let releases: BTreeSet<&String> = reported
            .iter()
            .filter(|device| device.vendor == vendor && device.platform == platform)
            .map(|device| &device.software)
            .collect();
        let ids: BTreeSet<&String> = reported
            .iter()
            .filter(|device| device.vendor == vendor && device.platform == platform)
            .map(|device| &device.id)
            .collect();
        assert!(
            releases.len() > 1,
            "case {case:?}: expected more than one published release of {vendor} {platform}, \
             got {releases:?}"
        );
        assert_eq!(
            ids.len(),
            releases.len(),
            "case {case:?}: each release of {vendor} {platform} needs its own id, got {ids:?}"
        );
    }

    // The response is in the catalog's cursor order, so the devices of one vendor are
    // contiguous and no device is reported twice.
    let order: Vec<(&String, &String, &String)> = reported
        .iter()
        .map(|device| (&device.vendor, &device.platform, &device.software))
        .collect();
    for pair in order.windows(2) {
        assert!(
            pair[0] < pair[1],
            "case \"cursor order\": expected {:?} to sort before {:?}",
            pair[0],
            pair[1]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: MaxResults / NextToken
// ---------------------------------------------------------------------------

/// `MaxResults` bounds one page, and a request that names none "returns all results".
///
/// The rows span the modelled `GVCDMaxResults` range 200..1000
/// (`smithy_query(query="shape",shape="GVCDMaxResults")`): its lower bound, a size
/// inside it, and its upper bound. Every one of them is at least the published device
/// count, so each page carries every device and no `NextToken` — "This value is null
/// when there are no more results to return".
#[tokio::test]
async fn max_results_bounds_a_page_and_the_last_page_carries_no_token() {
    let (router, _engine) = super::ec2_query::emulator();
    let unpaged: BTreeSet<Device> = all_devices(&router, "us-east-1", "unpaged")
        .await
        .into_iter()
        .collect();

    for max_results in ["200", "500", "1000"] {
        let case = format!("MaxResults={max_results}");
        let params = vec![("MaxResults".to_string(), max_results.to_string())];
        let (status, body) = get_device_types(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = devices(&case, &body);
        let limit: usize = max_results.parse().expect("a numeric MaxResults");
        assert!(
            page.len() <= limit,
            "case {case:?}: a page holds at most {limit} devices, got {}",
            page.len()
        );
        let paged: BTreeSet<Device> = page.into_iter().collect();
        assert_eq!(
            paged, unpaged,
            "case {case:?}: a page this large holds every device, got {paged:?}"
        );
        assert!(
            element(&body, "nextToken").is_none(),
            "case {case:?}: a last page carries no NextToken, got {body}"
        );
    }

    assert!(
        !unpaged.is_empty(),
        "case \"unpaged\": a request naming no MaxResults returns all results, got none"
    );
}

/// `MaxResults` outside the modelled 200..1000 range is refused, each published bound is
/// accepted, and the value one past each bound is not.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, MaxResults, is it inside the modelled range)
    let cases: [(&str, &str, bool); 6] = [
        ("one below the minimum", "199", false),
        ("the minimum", "200", true),
        ("inside", "640", true),
        ("the maximum", "1000", true),
        ("one above the maximum", "1001", false),
        ("zero", "0", false),
    ];

    for (case, value, inside) in cases {
        let params = vec![("MaxResults".to_string(), value.to_string())];
        let (status, body) = get_device_types(&router, "us-east-1", &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={value} is inside 200..1000, got {status} {body}"
            );
            assert!(
                !devices(case, &body).is_empty(),
                "case {case:?}: expected the published devices, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={value} is outside 200..1000, got {status} {body}"
            );
            assert!(
                element(&body, "vpnConnectionDeviceTypeSet").is_none(),
                "case {case:?}: a refused request reports no devices, got {body}"
            );
        }
    }
}

/// A `NextToken` this operation did not mint is refused with 400 `InvalidPaginationToken`,
/// rather than read as a cursor or ignored.
///
/// The rows differ in how the token is wrong: not hex at all, hex that decodes to
/// something else, a token of the right shape with a broken checksum, a token this
/// service really minted but for another paginating operation's cursor, a cursor sent
/// unencoded, and an empty one.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    // A token of the right SHAPE — this service's marker, a checksum field and a cursor,
    // hex-encoded — whose checksum is not the one the cursor hashes to, and a token this
    // service really did mint, for a different paginating operation's cursor.
    let forged_checksum = hex::encode("ec2-1:0000000000000000:Juniper".as_bytes());
    let another_operations_token = ec2::state::encode_page_token("vpc-0123456789abcdef0");
    let cases: [(&str, &str); 6] = [
        ("not hex", "not-a-token"),
        ("hex of other text", &hex::encode("Juniper".as_bytes())),
        ("a marked token with a broken checksum", &forged_checksum),
        ("another operation's token", &another_operations_token),
        ("an unencoded cursor", "Juniper SRX JunOS 11.0+"),
        ("empty", ""),
    ];

    for (case, token) in cases {
        let params = vec![("NextToken".to_string(), token.to_string())];
        let (status, body) = get_device_types(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for NextToken {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
        assert!(
            element(&body, "vpnConnectionDeviceTypeSet").is_none(),
            "case {case:?}: a refused request reports no devices, got {body}"
        );
        // EC2's message for this refusal is one fixed sentence, naming no token.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: expected EC2's own pagination-token sentence, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: DryRun
// ---------------------------------------------------------------------------
/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, while the
/// same call without it answers the published devices.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let (router, _engine) = super::ec2_query::emulator();

    // The rows differ in what the dry run is sent beside: nothing, a page size, and a
    // token that would otherwise be refused — a dry run performs nothing, so it reads
    // no other parameter either. That ORDER is the service's own choice, not a modelled
    // rule (neither the model nor `DryRun`'s doc says which of a dry run and a malformed
    // parameter is reported); what the row pins is that this crate answers it the same
    // way for every `DryRun` read.
    let cases: [(&str, Vec<(String, String)>); 3] = [
        ("DryRun alone", Vec::new()),
        (
            "DryRun beside MaxResults",
            vec![("MaxResults".to_string(), "200".to_string())],
        ),
        (
            "DryRun beside a foreign NextToken",
            vec![("NextToken".to_string(), "not-a-token".to_string())],
        ),
    ];
    for (case, extra) in cases {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        params.extend(extra);
        let (status, body) = get_device_types(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "vpnConnectionDeviceTypeSet").is_none(),
            "case {case:?}: a dry run reports no devices, got {body}"
        );
    }

    // `DryRun=false` is not a dry run: the request is performed.
    let params = vec![("DryRun".to_string(), "false".to_string())];
    let (status, body) = get_device_types(&router, "us-east-1", &params).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected 200, got {status} {body}"
    );
    assert!(
        !devices("DryRun=false", &body).is_empty(),
        "case \"DryRun=false\": expected the published devices, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3, the resume arithmetic the wire cannot reach
// ---------------------------------------------------------------------------

/// A cursor resumes the catalog AT the device it names, and only a device cursor does.
///
/// The wire path cannot reach this: the modelled `GVCDMaxResults` floor of 200 is above
/// the number of devices a sample configuration file is published for, so the operation
/// never mints a token and no caller can present one. The paging the handler performs is
/// still the real cursor arithmetic, so it is exercised here directly through the two
/// state primitives the handler uses — one row per device, plus a cursor that names no
/// device and one minted for another operation.
#[test]
fn a_cursor_resumes_the_catalog_at_the_device_it_names() {
    let catalog = ec2::ops::get_vpn_connection_device_types::vpn_connection_device_types();
    let cursors: Vec<String> = catalog
        .iter()
        .map(
            ec2::ops::get_vpn_connection_device_types::VpnConnectionDeviceTypeRecord::page_cursor,
        )
        .collect();
    assert!(
        cursors.windows(2).all(|pair| pair[0] < pair[1]),
        "case \"cursor order\": the catalog must be in strictly increasing cursor order, \
         got {cursors:?}"
    );

    for (index, cursor) in cursors.iter().enumerate() {
        let token = ec2::state::encode_page_token(cursor);
        let decoded = decode_device_page_token(&token).unwrap_or_else(|| {
            panic!("case {cursor:?}: a device cursor's own token must decode, got none")
        });
        let from = device_page_start(&catalog, Some(decoded.as_str()));
        assert_eq!(
            from, index,
            "case {cursor:?}: expected the page to resume at device {index}, got {from}"
        );
    }

    // `None` is the first page, and a cursor sorting past the last device resumes past
    // the end — an empty page, not a repeat of the first one.
    assert_eq!(
        device_page_start(&catalog, None),
        0,
        "case \"no cursor\": expected the first page"
    );
    assert_eq!(
        device_page_start(&catalog, Some("vpn-connection-device-type\u{1}~")),
        catalog.len(),
        "case \"a cursor past the last device\": expected an empty page"
    );

    // A token this service minted for another paginating operation's cursor is not a
    // device cursor, however well-formed it is.
    for (case, cursor) in [
        ("a VPC id", "vpc-0123456789abcdef0"),
        ("an offering cursor", "us-east-1 m5.large"),
    ] {
        let token = ec2::state::encode_page_token(cursor);
        assert!(
            decode_device_page_token(&token).is_none(),
            "case {case:?}: {cursor:?} is no device cursor, got a resume position"
        );
    }
}

/// A page truncated at `MaxResults` mints a token for the device that FOLLOWS it, and
/// that token resumes exactly there; a page that ends at the last device mints none.
///
/// This is the handler's own mint-and-resume round trip, with the page sizes the wire
/// cannot ask for: the modelled `GVCDMaxResults` floor of 200 is above the number of
/// published devices, so the mint branch is unreachable over ec2Query. The rows are the
/// page sizes that truncate the catalog — the first device alone, a size inside it, and
/// one device short of the whole — plus the two sizes that do not truncate it.
#[test]
fn a_truncated_page_mints_a_token_that_resumes_at_the_next_device() {
    let catalog = ec2::ops::get_vpn_connection_device_types::vpn_connection_device_types();
    assert!(
        catalog.len() > 2,
        "case \"the catalog\": the rows below need more than two devices, got {}",
        catalog.len()
    );

    // The handler's own mint step, so this test covers shipped code rather than a copy
    // of it.
    let minted = |limit: usize| device_page_token(&catalog, Some(limit));

    for limit in [1usize, catalog.len() / 2, catalog.len() - 1] {
        let case = format!("MaxResults={limit}");
        let token = minted(limit)
            .unwrap_or_else(|| panic!("case {case:?}: a truncated page must mint a token"));
        let decoded = decode_device_page_token(&token).unwrap_or_else(|| {
            panic!("case {case:?}: the token this operation minted must decode, got none")
        });
        let from = device_page_start(&catalog, Some(decoded.as_str()));
        assert_eq!(
            from, limit,
            "case {case:?}: the next page must resume at device {limit}, got {from}"
        );
        // The walk sees every device exactly once: the first page plus the rest.
        assert_eq!(
            limit + catalog[from..].len(),
            catalog.len(),
            "case {case:?}: the two pages must cover the catalog once, got {} + {}",
            limit,
            catalog[from..].len()
        );
    }

    for limit in [catalog.len(), catalog.len() + 1] {
        let case = format!("MaxResults={limit}");
        assert!(
            minted(limit).is_none(),
            "case {case:?}: a page holding every device mints no token"
        );
    }

    // A request that named no `MaxResults` "returns all results", so it is never followed
    // by another page.
    assert!(
        device_page_token(&catalog, None).is_none(),
        "case \"no MaxResults\": an unpaged answer mints no token"
    );
}
