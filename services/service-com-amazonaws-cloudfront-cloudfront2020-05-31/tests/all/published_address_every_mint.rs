// SPDX-License-Identifier: CC0-1.0

//! The address a distribution publishes as its `DomainName`, over every operation that
//! MINTS a distribution and every projection that reports one.
//!
//! RULE: `Distribution$DomainName` is the address a VIEWER is handed — "the domain name that
//! corresponds to the distribution", which a client fetches `https://<DomainName>/<object>`
//! from — so the value published for it is an address a client on the machine this emulator
//! runs on can DIAL and that this emulator answers on for THAT distribution: the `host:port`
//! of the listener its edge is served on. The address belongs to the distribution RECORD and
//! not to the call that happened to mint it, so whichever operation created the distribution
//! and whichever projection reports it, the one address comes back, it is that
//! distribution's own, and a reconfiguration neither moves nor closes it.
//!
//! The rows span what the rule generalises over: every operation that mints a standard
//! distribution — `CreateDistribution`, `CreateDistributionWithTags`, and `CopyDistribution`,
//! which mints one from another distribution's stored configuration — crossed with every
//! projection that reports a distribution's domain name: the minting call's own payload,
//! `GetDistribution`, the `ListDistributions` summary and the `UpdateDistribution` answer.
//! Each published address is then DIALLED — a socket is opened to it and an object fetched
//! through it, which is what a viewer handed that name does — once the distribution has
//! reached the edge locations, and again after the configuration has been resubmitted.
//!
//! The harness is [`crate::distribution_lifecycle`]'s — its router, its configuration
//! builder, its dialable-address predicate and its deployment poll loop — the minting calls
//! are the copy and tagging suites' helpers, the configuration read is
//! [`crate::connection_mode_immutable`]'s and the viewer dial is
//! [`crate::published_edge_address`]'s — each reused rather than copied.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_mode_immutable::get_distribution_config;
use crate::distribution_copy::copy_distribution;
use crate::distribution_lifecycle::{
    awaited_deployment, config, create_distribution, emulator, get_distribution,
    is_reachable_authority, list_distributions, update_distribution, Config, HOST,
};
use crate::distribution_tags::create_distribution_with_tags;
use crate::published_edge_address::dial;

/// The path a viewer fetches through each published address. The origin below is this
/// service's own control-plane endpoint, so this is an object the origin really serves and
/// what comes back is the origin's own answer.
const VIEWER_PATH: &str = "/2020-05-31/distribution";

/// The element the origin's answer carries, which is how the test tells an origin answer
/// from an answer the edge produced by itself.
const ORIGIN_DOCUMENT: &str = "DistributionList";

// ---------------------------------------------------------------------------
// The operations that mint a distribution
// ---------------------------------------------------------------------------

/// One operation that mints a standard distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mint {
    /// `POST /2020-05-31/distribution`.
    Create,
    /// `POST /2020-05-31/distribution?WithTags`.
    CreateWithTags,
    /// `POST /2020-05-31/distribution/<primary>/copy`, which mints a distribution from
    /// another one's STORED configuration rather than from a submitted document.
    Copy,
}

impl Mint {
    /// The operation's own name, which every assertion names its row by.
    fn case(self) -> &'static str {
        match self {
            Mint::Create => "CreateDistribution",
            Mint::CreateWithTags => "CreateDistributionWithTags",
            Mint::Copy => "CopyDistribution",
        }
    }
}

/// Every operation that mints a standard distribution.
const MINTS: [Mint; 3] = [Mint::Create, Mint::CreateWithTags, Mint::Copy];

/// The tag set the tagged create submits — one tag, because what this suite is about is the
/// address that create publishes and not the tagging itself.
fn one_tag() -> Vec<(String, Option<String>)> {
    vec![("owner".to_string(), Some("published-address".to_string()))]
}

/// A distribution this suite minted: the row that minted it, its id, the address it
/// published and its current version token.
#[derive(Debug, Clone)]
struct Minted {
    case: &'static str,
    id: String,
    domain: String,
    etag: String,
}

/// The configuration every row submits: one origin, which is this service's own
/// control-plane endpoint — an origin this emulator really serves, so a viewer request
/// through the published address is answered with the origin's own document.
fn served_origin_config(reference: &str) -> Config {
    Config {
        origins: vec![("primary".to_string(), HOST.to_string())],
        ..config("published-address", reference)
    }
}

/// Mint one distribution through `how` and read the address it published.
async fn mint_distribution(router: &MultiServiceRouter, how: Mint, reference: &str) -> Minted {
    let case = how.case();
    let config = served_origin_config(reference);
    let (answer, minted_from) = match how {
        Mint::Create => (create_distribution(router, &config.xml()).await, None),
        Mint::CreateWithTags => (
            create_distribution_with_tags(router, &config, Some(&one_tag())).await,
            None,
        ),
        Mint::Copy => {
            // A copy runs under a PRIMARY's stored configuration, so the row creates one
            // first; the primary is a distribution of its own, with an address of its own.
            let primary = create_distribution(
                router,
                &served_origin_config(&format!("{reference}-primary")).xml(),
            )
            .await;
            assert_eq!(
                primary.status, 201,
                "case {case}: expected HTTP 201 creating the primary, got {} with {}",
                primary.status, primary.raw
            );
            let primary_id = primary.root().text_at("Id").unwrap_or_default().to_string();
            let primary_domain = primary
                .root()
                .text_at("DomainName")
                .unwrap_or_default()
                .to_string();
            (
                copy_distribution(router, &primary_id, reference, None, None, None).await,
                Some(primary_domain),
            )
        }
    };
    assert_eq!(
        answer.status, 201,
        "case {case}: expected HTTP 201 from the minting call, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case {case}: the minting call carried no Id: {}",
        answer.raw
    );
    let domain = answer
        .root()
        .text_at("DomainName")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_reachable_authority(&domain),
        "case {case}: expected the address of the listener this distribution's edge is \
         served on, got {domain:?}"
    );
    if let Some(primary_domain) = minted_from {
        assert_ne!(
            domain, primary_domain,
            "case {case}: expected the copy to publish an address of its own, got the \
             primary's {primary_domain:?}"
        );
    }
    Minted {
        case,
        id,
        domain,
        etag: answer.header("etag").to_string(),
    }
}

/// Fetch the viewer path through `domain` and assert the origin's own answer came back.
async fn assert_serves(case: &str, domain: &str, context: &str) {
    let served = dial(domain, "GET", VIEWER_PATH).await;
    assert!(
        served.starts_with("HTTP/1.1 200"),
        "case {case} ({context}): expected the published address {domain} to serve the \
         origin's answer, got {served:?}"
    );
    assert!(
        served.contains(ORIGIN_DOCUMENT),
        "case {case} ({context}): expected the origin's own document from {domain}, got \
         {served:?}"
    );
    assert!(
        served
            .to_ascii_lowercase()
            .contains("x-cache: miss from cloudfront"),
        "case {case} ({context}): expected the edge to report a cache miss on {domain}, got \
         {served:?}"
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Whichever operation minted it, the address a distribution publishes is its own and it
/// SERVES: a viewer that dials it is answered by that distribution's edge with the origin's
/// document, and no two distributions publish the same address.
#[tokio::test]
async fn every_operation_that_mints_a_distribution_publishes_an_address_that_serves() {
    let router = emulator();
    let mut minted: Vec<Minted> = Vec::new();
    for how in MINTS {
        minted.push(mint_distribution(&router, how, &format!("ref-mint-{}", how.case())).await);
    }

    let mut addresses: BTreeSet<String> = BTreeSet::new();
    for one in &minted {
        assert!(
            addresses.insert(one.domain.clone()),
            "case {}: the address {} was already published by another distribution",
            one.case,
            one.domain
        );
    }

    for one in &minted {
        awaited_deployment(&router, one.case, &one.id).await;
        assert_serves(one.case, &one.domain, "the address the minting call published").await;
    }
}

/// Every projection of one distribution reports the address its minting call published — the
/// read, the listing and the update answer alike — and the address still serves once the
/// configuration has been resubmitted.
#[tokio::test]
async fn every_projection_reports_the_address_the_minting_call_published() {
    let router = emulator();
    for how in MINTS {
        let one = mint_distribution(&router, how, &format!("ref-projection-{}", how.case())).await;
        let case = one.case;

        let read = get_distribution(&router, &one.id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
            read.status, read.raw
        );
        let read_domain = read.root().text_at("DomainName").unwrap_or_default();
        assert_eq!(
            read_domain, one.domain,
            "case {case} (GetDistribution): expected the published address {:?}, got \
             {read_domain:?}",
            one.domain
        );

        let listed = list_distributions(&router, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected HTTP 200 from ListDistributions, got {} with {}",
            listed.status, listed.raw
        );
        let summary_domain = listed
            .root()
            .child("Items")
            .map(|items| items.children_named("DistributionSummary"))
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary.text_at("Id").unwrap_or_default() == one.id)
            .and_then(|summary| summary.text_at("DomainName"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            summary_domain, one.domain,
            "case {case} (ListDistributions): expected the summary of {} to report the \
             published address {:?}, got {summary_domain:?}",
            one.id, one.domain
        );

        // The stored configuration, resubmitted whole: a reconfiguration is the call that
        // could move the address, and it does not — the record keeps the address its
        // listener answers on.
        let stored = get_distribution_config(&router, &one.id).await;
        assert_eq!(
            stored.status, 200,
            "case {case}: expected HTTP 200 from GetDistributionConfig, got {} with {}",
            stored.status, stored.raw
        );
        let updated = update_distribution(&router, &one.id, &stored.raw, Some(&one.etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            updated.status, updated.raw
        );
        let updated_domain = updated.root().text_at("DomainName").unwrap_or_default();
        assert_eq!(
            updated_domain, one.domain,
            "case {case} (UpdateDistribution): expected the published address {:?}, got \
             {updated_domain:?}",
            one.domain
        );

        awaited_deployment(&router, case, &one.id).await;
        assert_serves(case, &one.domain, "the address after a reconfiguration").await;
    }
}
