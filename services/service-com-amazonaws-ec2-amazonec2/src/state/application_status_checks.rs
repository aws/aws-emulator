// SPDX-License-Identifier: CC0-1.0

//! The application status CHECKS this service stores, and what reads one.
//!
//! The CONFIGURATION of each application status check the account holds — protocol, port,
//! path, thresholds, health check paths and tags — lives in
//! [`ComAmazonawsEc2Amazonec2State::application_status_checks`], keyed by
//! `ApplicationStatusCheckId`, as the modelled [`ApplicationStatusCheckResponseObject`]
//! `DescribeApplicationStatusChecks` reports, so every member a writer sets round-trips
//! without a field-by-field copy that can silently drop one. That is a different store from
//! [`ComAmazonawsEc2Amazonec2State::application_status`], which holds what each check
//! REPORTED about one instance: one configuration, many per-instance results.
//!
//! A check inside its deletion grace period is the stored configuration with its
//! `DeletionTime` set — "recently deleted application status checks that remain available
//! during the deletion grace period"
//! (`DescribeApplicationStatusChecksRequest$IncludeAll`) — so whether a check is active is
//! read off that member ([`application_status_check_is_active`]) rather than from a second
//! map of deleted checks. `DeleteApplicationStatusCheck` is what sets it
//! ([`ComAmazonawsEc2Amazonec2State::begin_application_status_check_deletion`]), which is also
//! what removes the check's associations and what bounds how many deletions the store keeps.
//!
//! A SUBMODULE of [`crate::state`], the way CloudFront's function slice is a submodule of
//! its state: this surface grows as the sibling modify and delete operations are implemented,
//! and it grows here rather than in the parts of `state.rs` the other resource kinds own.
//! `pub use` keeps every name reachable as `crate::state::<name>`, which is where each
//! handler names it.
//!
//! `CreateApplicationStatusCheck` is what MINTS a check, so the documented defaults it fills
//! an unspecified member with, the published bounds it enforces, the quota it asks about, the
//! bytes a stored check is charged and the comparison a replayed `ClientToken` is judged on
//! all live here, beside the store they are about.
//!
//! What a check is ASSOCIATED with is the end of this module: the TAG associations are the
//! check's own `TargetTagAssociations` member, so there is one copy of them and the describe
//! reports exactly what an associate stored, while an INSTANCE association is the per-check
//! result that instance holds in [`ComAmazonawsEc2Amazonec2State::application_status`] —
//! "health monitoring automatically begins" the moment one is made, which is a result of
//! `initializing` until something reports one.

use ::std::time::SystemTime;

use crate::ops::types::{
    ApplicationStatusCheckResponseObject, ApplicationStatusDetail, CustomTagKeyValueResponsePair,
    HealthCheckPathDestinationResponseObject, HealthCheckPathRequestObject,
    HealthCheckPathResponseObject, HealthCheckPathSourceResponseObject,
};
use crate::scaffolded::Scaffolded;

use super::{same_tags, tag_list_bytes, ComAmazonawsEc2Amazonec2State};

/// The id prefix an application status check carries.
///
/// The model publishes no `@pattern` for `ApplicationStatusCheckId` (it is a plain `String`
/// — `smithy_query(query="shape",shape="ApplicationStatusCheckId")`), so the id takes EC2's
/// own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with a prefix naming the resource — the same
/// arrangement `ROUTE_SERVER_ID_PREFIX` and the VPC endpoint connection id use.
///
/// `CreateApplicationStatusCheck` mints every stored check's id with this prefix, through
/// [`ComAmazonawsEc2Amazonec2State::next_id`].
pub const APPLICATION_STATUS_CHECK_ID_PREFIX: &str = "asc";

/// The `TagSpecification$ResourceType` a create may name when it tags the check it creates:
/// the `application-status-check` member of the modelled `ResourceType` catalog
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE: &str = "application-status-check";

/// How many application status checks one account holds before a create is refused: "You can
/// create a maximum of 50 application status checks for each account"
/// (`CreateApplicationStatusCheck`).
///
/// A DOCUMENTED quota rather than a ceiling this emulator chose, so it is the bound a create
/// asks about — and it counts the ACTIVE checks, a deleted one inside its grace period having
/// been given up by the account already.
pub const MAX_APPLICATION_STATUS_CHECKS: usize = 50;

/// The one interval a check may be configured with: "The interval, in seconds, between
/// health checks. Valid value: 60" (`CreateApplicationStatusCheckRequest$Interval`), which is
/// therefore also the interval a request naming none gets ("Interval is 60 seconds").
pub const APPLICATION_STATUS_CHECK_INTERVAL_SECONDS: i32 = 60;

/// The timeout bounds a check may be configured with: "Valid values: 1 to 30"
/// (`CreateApplicationStatusCheckRequest$Timeout`). The member carries no modelled `@range`,
/// so this is the only place those published bounds are enforced.
pub const APPLICATION_STATUS_CHECK_TIMEOUT_RANGE: (i32, i32) = (1, 30);

/// The path a request naming none gets: "You must start the Path with a forward slash (/).
/// Default: /" (`CreateApplicationStatusCheck`).
///
/// There is no constant here for the aggregation default — "it defaults to `included`, which
/// means the check contributes to the instance-level application status" — because that word
/// is the `included` member of `AggregationStatusEnum`, which this module already names once
/// as [`super::AGGREGATION_INCLUDED`].
pub const DEFAULT_APPLICATION_STATUS_CHECK_PATH: &str = "/";

/// The timeout a request naming none gets: "Timeout is 6 seconds"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_TIMEOUT: i32 = 6;

/// The failure threshold a request naming none gets: "FailureThreshold is 2"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_FAILURE_THRESHOLD: i32 = 2;

/// The success threshold a request naming none gets: "SuccessThreshold is 2"
/// (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_SUCCESS_THRESHOLD: i32 = 2;

/// The status code matcher a request naming none gets: "StatusCodeMatcher is 200"
/// (`CreateApplicationStatusCheck`), also stated on the member itself ("Default: 200").
pub const DEFAULT_APPLICATION_STATUS_CHECK_STATUS_CODE_MATCHER: &str = "200";

/// The initialization grace period a request naming none gets:
/// "InitializationGracePeriodSeconds is 300 seconds" (`CreateApplicationStatusCheck`).
pub const DEFAULT_APPLICATION_STATUS_CHECK_GRACE_PERIOD_SECONDS: i32 = 300;

/// The network device index a request naming none gets: "The index of the network device to
/// use for the health check. The value must be greater than or equal to 0"
/// (`CreateApplicationStatusCheckRequest$DeviceIndex`).
///
/// The least value that member admits is also the device every instance has — the primary
/// network interface sits at device index 0 — so a check that names no device is configured
/// against that interface, which is a value the created check REPORTS
/// (`ApplicationStatusCheckResponseObject$DeviceIndex`) rather than a member left absent.
pub const DEFAULT_APPLICATION_STATUS_CHECK_DEVICE_INDEX: i32 = 0;

/// The IP version a request naming none gets: "The IP version to use for the health check.
/// Valid values: `ipv4` and `ipv6`" (`CreateApplicationStatusCheckRequest$IpVersion`), of
/// which `ipv4` is the one every instance's primary interface has — an IPv6 address being an
/// option of a dual-stack subnet.
pub const DEFAULT_APPLICATION_STATUS_CHECK_IP_VERSION: &str = "ipv4";

/// The IP scope a request naming none gets: "The IP scope to use for the health check. Valid
/// value: `private`" (`CreateApplicationStatusCheckRequest$IpScope`) — the member's ONE
/// documented value, which is also the only member of `IpScopeEnum`
/// ([`crate::ops::types::IP_SCOPE_ENUM_VALUES`]).
pub const DEFAULT_APPLICATION_STATUS_CHECK_IP_SCOPE: &str = "private";

/// The longest status code matcher a check may carry: "Maximum length: 64 characters"
/// (`CreateApplicationStatusCheckRequest$StatusCodeMatcher`). The member is a plain `String`
/// with no modelled `@length`, so this is the only place that published bound is enforced.
pub const MAX_STATUS_CODE_MATCHER_CHARS: usize = 64;

/// The longest health check path a check may carry.
///
/// CHOSEN: the member is "The URL path to use for the health check HTTP request"
/// (`CreateApplicationStatusCheckRequest$Path`), a plain `String` the model bounds nowhere and
/// AWS publishes no length for, and the store keeps whatever it is given for the process's
/// lifetime — so the caller-chosen bytes one check can hold have to be finite. It is well
/// above any URL path a health check uses, so no request a real account would make is refused
/// for a reason this emulator cannot have.
pub const MAX_HEALTH_CHECK_PATH_CHARS: usize = 1_024;

/// How many health check paths one check may carry, and how many destinations each may name.
///
/// CHOSEN for the same reason as [`MAX_HEALTH_CHECK_PATH_CHARS`]: `HealthCheckPathRequestList`
/// and `HealthCheckPathDestinationRequestSet` carry no modelled `@length`, so the number of
/// subnet ids one create can retain has to be bounded somewhere, and the product of the two is
/// what bounds the whole `HealthCheckPaths` member.
pub const MAX_HEALTH_CHECK_PATHS: usize = 10;

/// How many destinations one health check path may name — see [`MAX_HEALTH_CHECK_PATHS`].
pub const MAX_HEALTH_CHECK_PATH_DESTINATIONS: usize = 10;

/// `InvalidApplicationStatusCheckId.NotFound` (HTTP 400): the request named an application
/// status check id this account holds no check for. EC2 spells a per-resource lookup failure
/// `Invalid<Resource>Id.NotFound` (`InvalidRouteServerId.NotFound`,
/// `InvalidLaunchTemplateId.NotFound`).
pub const INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND: &str =
    "InvalidApplicationStatusCheckId.NotFound";

/// Refuse a request that named an application status check id no check of the account has:
/// HTTP 400 [`INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND`], `The application-status-check ID
/// '<id>' does not exist`.
///
/// The resource is named the way this family's refusals name it — the hyphenated
/// `application-status-check` of [`APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE`], which is also
/// what [`invalid_application_status_check_id_malformed`] carries, so the two halves of the
/// pair name one resource one way.
pub fn invalid_application_status_check_id_not_found<E>(
    application_status_check_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND,
        400,
        format!(
            "The {APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE} ID '{}' does not exist",
            super::echoed_resource_id(application_status_check_id)
        ),
    )
}

/// `InvalidApplicationStatusCheckId.Malformed` (HTTP 400): the request named something that
/// is not an application status check id AT ALL, so there is nothing to look up — the
/// `.Malformed` half EC2 answers BEFORE it resolves an id, as
/// [`INVALID_APPLICATION_STATUS_CHECK_ID_NOT_FOUND`] is the half it answers after.
pub const INVALID_APPLICATION_STATUS_CHECK_ID_MALFORMED: &str =
    "InvalidApplicationStatusCheckId.Malformed";

/// Whether `id` is a well-formed application status check id — one this service could have
/// issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`super::prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`APPLICATION_STATUS_CHECK_ID_PREFIX`]), which is also what
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints. A value under any other prefix is not an
/// id of this kind, whatever its suffix looks like.
pub fn application_status_check_id_is_well_formed(id: &str) -> bool {
    super::prefixed_id_is_well_formed(APPLICATION_STATUS_CHECK_ID_PREFIX, id)
}

/// Refuse something that is not an application status check id at all: HTTP 400
/// [`INVALID_APPLICATION_STATUS_CHECK_ID_MALFORMED`], `The application-status-check ID '<id>'
/// is malformed`, built from the value the REQUEST sent (bounded by
/// [`super::echoed_resource_id`]).
///
/// The sentence template this crate words every malformed resource id with — `The <resource
/// type> ID '<id>' is malformed` — with this resource's own type name, the
/// `application-status-check` member of the modelled `ResourceType` catalog that
/// [`APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE`] already names once.
pub fn invalid_application_status_check_id_malformed<E>(
    application_status_check_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_APPLICATION_STATUS_CHECK_ID_MALFORMED,
        400,
        format!(
            "The {APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE} ID '{}' is malformed",
            super::echoed_resource_id(application_status_check_id)
        ),
    )
}

/// Refuse an `ApplicationStatusCheckId` that is not an id of this kind, BEFORE anything is
/// looked up or any other member is read — the ordering EC2 keeps, so a caller can tell "not
/// an id of this kind" from every other reason a request is refused.
pub fn require_well_formed_application_status_check_id<E>(
    application_status_check_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !application_status_check_id_is_well_formed(application_status_check_id) {
        return Err(invalid_application_status_check_id_malformed(
            application_status_check_id,
        ));
    }
    Ok(())
}

/// Refuse an application status check id the request named that this service cannot resolve: a
/// MALFORMED one before an unknown one, the way this service orders the two for every resource
/// kind whose id has both halves ([`require_well_formed_application_status_check_id`] and
/// [`invalid_application_status_check_id_not_found`]).
pub fn require_known_application_status_check<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    application_status_check_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_application_status_check_id(application_status_check_id)?;
    if !store
        .application_status_checks
        .contains_key(application_status_check_id)
    {
        return Err(invalid_application_status_check_id_not_found(
            application_status_check_id,
        ));
    }
    Ok(())
}

/// Whether `check` is ACTIVE — outside the deletion grace period a deleted check spends
/// still visible.
///
/// "If you omit this parameter or set it to false, the response includes only active checks"
/// (`DescribeApplicationStatusChecksRequest$IncludeAll`), and a deleted check is the one that
/// carries "the date and time when the application status check was deleted"
/// (`ApplicationStatusCheckResponseObject$DeletionTime`) — so that member is what tells the
/// two apart, in one place, for every operation that has to.
pub fn application_status_check_is_active(check: &ApplicationStatusCheckResponseObject) -> bool {
    check.deletion_time.is_none()
}

/// How many ACTIVE application status checks the account holds right now — what a create
/// compares against [`MAX_APPLICATION_STATUS_CHECKS`].
///
/// Counted off [`application_status_check_is_active`], so the quota and the describe agree
/// about which checks the account still holds: one inside its deletion grace period is
/// reported only when a caller asks for it and does not stand between the account and its
/// next create.
pub fn active_application_status_check_count(store: &ComAmazonawsEc2Amazonec2State) -> usize {
    store
        .application_status_checks
        .values()
        .filter(|check| application_status_check_is_active(check))
        .count()
}

/// Refuse a create that would take the account past the documented quota: HTTP 400
/// [`super::RESOURCE_LIMIT_EXCEEDED`], the code this service answers for a published ceiling
/// with no code of its own.
pub fn application_status_check_limit_exceeded<E>(held: usize) -> Scaffolded<E> {
    super::resource_limit_exceeded(format!(
        "You have reached the limit of {MAX_APPLICATION_STATUS_CHECKS} application status \
         checks for this account, which already holds {held}"
    ))
}

/// How many deleted application status checks this store keeps inside their deletion grace
/// period, the oldest-deleted dropped to make room for a newer one.
///
/// The grace period itself is a span AWS publishes no length for — "recently deleted
/// application status checks that remain available during the deletion grace period"
/// (`DescribeApplicationStatusChecksRequest$IncludeAll`) — so what ends it here is this
/// bounded history, the arrangement this store already keeps a resource's deletions under
/// ([`super::MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`]). It is the account's own quota
/// of checks ([`MAX_APPLICATION_STATUS_CHECKS`]), so a caller that deletes everything it may
/// hold can still read every one of those deletions back.
pub const MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS: usize = MAX_APPLICATION_STATUS_CHECKS;

impl ComAmazonawsEc2Amazonec2State {
    /// Delete the application status check `application_status_check_id` as of `at` and answer
    /// it as this call leaves it, or `None` when the store holds no check under that id.
    ///
    /// The one way a check is deleted, so the delete's answer and what a later
    /// `DescribeApplicationStatusChecks` reports cannot disagree. The record STAYS in the
    /// store carrying "the date and time when the application status check was deleted"
    /// (`ApplicationStatusCheckResponseObject$DeletionTime`), which is what makes it one
    /// inside its deletion grace period ([`application_status_check_is_active`]): reported
    /// only to a caller that asks for deleted checks, and no longer counted against the
    /// account's quota.
    ///
    /// "Deleting a check automatically removes all of its associations"
    /// (`DeleteApplicationStatusCheck`), so both halves go: the TAG associations are the
    /// check's own `TargetTagAssociations` member, and the bytes they held go back to the
    /// store's budget on the terms [`tag_association_bytes`] took them on, while an INSTANCE
    /// association is the per-check result that instance holds, which stops being one of the
    /// results `DescribeApplicationStatus` aggregates for it.
    ///
    /// The deletion history is bounded by
    /// [`MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS`]: once it is full the
    /// oldest-deleted check leaves the store for good, giving back the bytes it held and taking
    /// the create token that minted it with it — a token must never answer with a check that is
    /// no longer stored.
    /// A check already inside its deletion grace period has been deleted once: this reports the
    /// record THAT delete left — its instant, and the associations it already took — and writes
    /// nothing again, so a retried delete answers what the first one answered and the deletion
    /// instant a reader sees never moves.
    pub fn begin_application_status_check_deletion(
        &mut self,
        application_status_check_id: &str,
        at: SystemTime,
    ) -> Option<ApplicationStatusCheckResponseObject> {
        let held = self
            .application_status_checks
            .get(application_status_check_id)?;
        if !application_status_check_is_active(held) {
            return Some(held.clone());
        }

        // The instance half of "all of its associations": the instance holds this check's
        // result, so removing the result is what ends the monitoring. An instance this check
        // was the last thing monitoring, and which carries no suppression either, holds nothing
        // at all afterwards, so its row goes too — a read of it answers `not-applicable` from
        // the absence, exactly as it did before any association was made. A row this check was
        // never associated with is left exactly as it is.
        self.application_status.retain(|_, record| {
            let monitored = record.details.remove(application_status_check_id).is_some();
            !monitored || !record.details.is_empty() || record.suppression.is_some()
        });

        let check = self
            .application_status_checks
            .get_mut(application_status_check_id)?;
        // The tag half: the pairs leave the check, and their bytes leave the budget.
        let released =
            tag_association_bytes(check.target_tag_associations.as_deref().unwrap_or_default());
        check.target_tag_associations = Some(Vec::new());
        check.deletion_time = Some(super::system_time_to_date_time(at));
        let deleted = check.clone();
        self.release_stored_bytes(released);

        self.forget_oldest_application_status_check_deletions(application_status_check_id);
        Some(deleted)
    }

    /// Drop the checks whose deletion grace period has ended: the oldest-deleted ones past
    /// [`MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS`], `keep` — the check the delete
    /// running right now is about — never among them.
    ///
    /// Ordered by the instant each was deleted at, the id breaking a tie, so the history drops
    /// deletions in the order they happened however many land in one millisecond.
    fn forget_oldest_application_status_check_deletions(&mut self, keep: &str) {
        let mut deleted: Vec<((i64, u32), String)> = self
            .application_status_checks
            .iter()
            .filter(|(application_status_check_id, check)| {
                application_status_check_id.as_str() != keep
                    && !application_status_check_is_active(check)
            })
            .map(|(application_status_check_id, check)| {
                (
                    check
                        .deletion_time
                        .map(super::date_time_key)
                        .unwrap_or_default(),
                    application_status_check_id.clone(),
                )
            })
            .collect();
        deleted.sort();
        // `keep` is one of the deletions this history holds, so it counts towards the bound
        // even though it is never the one dropped.
        let held = deleted.len().saturating_add(1);
        let over = held.saturating_sub(MAX_REMEMBERED_APPLICATION_STATUS_CHECK_DELETIONS);
        for (_, application_status_check_id) in deleted.into_iter().take(over) {
            self.forget_application_status_check(&application_status_check_id);
        }
    }

    /// Remove one application status check from the store for good, giving its caller-chosen
    /// bytes back to the store's budget on the terms
    /// [`application_status_check_payload_bytes`] took them on and dropping the
    /// `CreateApplicationStatusCheck` token that minted it.
    fn forget_application_status_check(&mut self, application_status_check_id: &str) {
        let Some(check) = self
            .application_status_checks
            .remove(application_status_check_id)
        else {
            return;
        };
        let minting_token = self
            .application_status_check_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == application_status_check_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &minting_token {
            self.application_status_check_client_tokens.remove(token);
        }
        self.release_stored_bytes(application_status_check_payload_bytes(
            &check,
            minting_token.as_deref(),
        ));
    }
}

/// Whether `matcher` is the form `StatusCodeMatcher` documents: "a comma-separated list of
/// individual status codes or ranges, for example, 200,202,300-399. For a range, the first
/// value must be less than the second value"
/// (`CreateApplicationStatusCheckRequest$StatusCodeMatcher`).
///
/// An HTTP status code is three digits, so each entry is either one three-digit code or two
/// separated by a hyphen, the first below the second. The LENGTH bound the same member
/// publishes is [`MAX_STATUS_CODE_MATCHER_CHARS`], asked separately so a matcher that is too
/// long is refused for being too long rather than for its form.
pub fn status_code_matcher_is_well_formed(matcher: &str) -> bool {
    !matcher.is_empty()
        && matcher.split(',').all(|entry| match entry.split_once('-') {
            Some((low, high)) => match (http_status_code(low), http_status_code(high)) {
                (Some(low), Some(high)) => low < high,
                _ => false,
            },
            None => http_status_code(entry).is_some(),
        })
}

/// One entry of a [`status_code_matcher_is_well_formed`] matcher as the status code it names,
/// or `None` when it is not one: an HTTP status code is exactly three decimal digits.
fn http_status_code(text: &str) -> Option<u16> {
    (text.len() == 3 && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The caller-chosen bytes one stored application status check holds: its tags, its path, its
/// status code matcher, the subnet and security group ids of its health check paths, and the
/// `ClientToken` the token map keeps for it. Everything else on the record — the id, the
/// ports, the thresholds, the instants — is minted by this module or bounded by the model, so
/// it is fixed-size.
///
/// The ONE definition of that charge, the way [`super::route_server_payload_bytes`] is for a
/// route server: a create asks it before the write and charges exactly it, and a removal path
/// must give back the same number.
pub fn application_status_check_payload_bytes(
    check: &ApplicationStatusCheckResponseObject,
    client_token: Option<&str>,
) -> usize {
    let paths: usize = check
        .health_check_paths
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(health_check_path_bytes)
        .sum();
    tag_list_bytes(check.tags.as_deref().unwrap_or_default())
        .saturating_add(check.path.as_deref().map_or(0, str::len))
        .saturating_add(check.status_code_matcher.as_deref().map_or(0, str::len))
        .saturating_add(paths)
        .saturating_add(tag_association_bytes(
            check.target_tag_associations.as_deref().unwrap_or_default(),
        ))
        .saturating_add(client_token.map_or(0, str::len))
}

/// The health check paths one request named, as the modelled response objects a stored check
/// holds — the ONE reading of that member, so the create that mints a check and the modify
/// that replaces its paths cannot disagree about what a path is or about what is refused.
///
/// "Health check paths define the network path from a source subnet to one or more
/// destination subnets" (`$HealthCheckPaths`), so a path names a source subnet and at least
/// one destination; neither list carries a modelled `@length`, so both are bounded here
/// ([`MAX_HEALTH_CHECK_PATHS`], [`MAX_HEALTH_CHECK_PATH_DESTINATIONS`]) before any work
/// proportional to them is done. The subnets and security groups the paths name are resolved
/// by the CALLER, which holds the store.
pub fn health_check_paths_from_request<E>(
    asked_paths: &[HealthCheckPathRequestObject],
) -> Result<Vec<HealthCheckPathResponseObject>, Scaffolded<E>> {
    if asked_paths.len() > MAX_HEALTH_CHECK_PATHS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "The request carries {} health check paths, above the maximum of \
             {MAX_HEALTH_CHECK_PATHS}",
            asked_paths.len()
        )));
    }
    let mut health_check_paths: Vec<HealthCheckPathResponseObject> =
        Vec::with_capacity(asked_paths.len());
    for asked in asked_paths {
        let source = asked.source.as_ref().ok_or_else(|| {
            Scaffolded::missing_parameter("Each health check path must name a source")
        })?;
        let source_subnet_id = source.subnet_id.clone().ok_or_else(|| {
            Scaffolded::missing_parameter("Each health check path source must name a subnet")
        })?;
        let destinations = asked.destinations.as_deref().unwrap_or_default();
        if destinations.is_empty() {
            return Err(Scaffolded::missing_parameter(format!(
                "The health check path from '{source_subnet_id}' must name at least one \
                 destination"
            )));
        }
        if destinations.len() > MAX_HEALTH_CHECK_PATH_DESTINATIONS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The health check path from '{source_subnet_id}' carries {} destinations, \
                 above the maximum of {MAX_HEALTH_CHECK_PATH_DESTINATIONS}",
                destinations.len()
            )));
        }
        let mut reported: Vec<HealthCheckPathDestinationResponseObject> =
            Vec::with_capacity(destinations.len());
        for destination in destinations {
            let subnet_id = destination.subnet_id.clone().ok_or_else(|| {
                Scaffolded::missing_parameter(format!(
                    "Each destination of the health check path from '{source_subnet_id}' \
                     must name a subnet"
                ))
            })?;
            reported.push(HealthCheckPathDestinationResponseObject {
                subnet_id: Some(subnet_id),
                security_group_id: destination.security_group_id.clone(),
            });
        }
        health_check_paths.push(HealthCheckPathResponseObject {
            source: Some(HealthCheckPathSourceResponseObject {
                subnet_id: Some(source_subnet_id),
                security_group_id: source.security_group_id.clone(),
            }),
            destinations: Some(reported),
        });
    }
    Ok(health_check_paths)
}

/// The caller-chosen bytes a check's TAG ASSOCIATIONS hold: the key and the value of each
/// pair the caller asked the check to monitor.
///
/// Part of [`application_status_check_payload_bytes`] rather than a second charge of its own,
/// so an association path charges exactly what a removal path gives back — and reachable on
/// its own because `AssociateApplicationStatusCheck` has to ask what the pairs it is about to
/// store would cost BEFORE it stores them.
pub fn tag_association_bytes(pairs: &[CustomTagKeyValueResponsePair]) -> usize {
    pairs.iter().fold(0, |bytes, pair| {
        bytes
            .saturating_add(pair.key.as_deref().map_or(0, str::len))
            .saturating_add(pair.value.as_deref().map_or(0, str::len))
    })
}

/// The caller-chosen bytes one health check path holds: the ids its source and each of its
/// destinations name.
fn health_check_path_bytes(path: &HealthCheckPathResponseObject) -> usize {
    let source = path.source.as_ref().map_or(0, |source| {
        source
            .subnet_id
            .as_deref()
            .map_or(0, str::len)
            .saturating_add(source.security_group_id.as_deref().map_or(0, str::len))
    });
    path.destinations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .fold(source, |bytes, destination| {
            bytes
                .saturating_add(destination.subnet_id.as_deref().map_or(0, str::len))
                .saturating_add(
                    destination
                        .security_group_id
                        .as_deref()
                        .map_or(0, str::len),
                )
        })
}

/// Whether two application status checks are configured the SAME way — every member a create
/// takes from the request, and nothing the service decides for itself (the id and the
/// instants).
///
/// What an idempotency replay is judged on: "If you retry a request with the same token, the
/// service ignores the request but does not return an error"
/// (`CreateApplicationStatusCheckRequest$ClientToken`) holds for a retry of the SAME request,
/// so a token reused for a different configuration is the mismatch EC2 refuses instead. The
/// modelled shapes carry no `PartialEq`, which is why the comparison is spelled out here once
/// rather than in each operation that needs it.
pub fn same_application_status_check_configuration(
    stored: &ApplicationStatusCheckResponseObject,
    asked: &ApplicationStatusCheckResponseObject,
) -> bool {
    stored.aggregation == asked.aggregation
        && stored.protocol == asked.protocol
        && stored.port == asked.port
        && stored.path == asked.path
        && stored.device_index == asked.device_index
        && stored.ip_version == asked.ip_version
        && stored.ip_scope == asked.ip_scope
        && stored.interval == asked.interval
        && stored.timeout == asked.timeout
        && stored.failure_threshold == asked.failure_threshold
        && stored.success_threshold == asked.success_threshold
        && stored.status_code_matcher == asked.status_code_matcher
        && stored.initialization_grace_period_seconds == asked.initialization_grace_period_seconds
        && same_health_check_paths(
            stored.health_check_paths.as_deref().unwrap_or_default(),
            asked.health_check_paths.as_deref().unwrap_or_default(),
        )
        && same_tags(
            stored.tags.as_deref().unwrap_or_default(),
            asked.tags.as_deref().unwrap_or_default(),
        )
}

/// Whether two health check path lists name the same paths in the same order.
///
/// ORDER matters, unlike a tag list's: a path list is a sequence the caller gave, not a set
/// keyed by anything, so nothing makes two orderings the same request.
fn same_health_check_paths(
    stored: &[HealthCheckPathResponseObject],
    asked: &[HealthCheckPathResponseObject],
) -> bool {
    stored.len() == asked.len()
        && stored.iter().zip(asked).all(|(stored, asked)| {
            let same_source = match (stored.source.as_ref(), asked.source.as_ref()) {
                (None, None) => true,
                (Some(stored), Some(asked)) => {
                    stored.subnet_id == asked.subnet_id
                        && stored.security_group_id == asked.security_group_id
                }
                _ => false,
            };
            let stored_destinations = stored.destinations.as_deref().unwrap_or_default();
            let asked_destinations = asked.destinations.as_deref().unwrap_or_default();
            same_source
                && stored_destinations.len() == asked_destinations.len()
                && stored_destinations
                    .iter()
                    .zip(asked_destinations)
                    .all(|(stored, asked)| {
                        stored.subnet_id == asked.subnet_id
                            && stored.security_group_id == asked.security_group_id
                    })
        })
}

// =============================================================================
// What a check is ASSOCIATED with
// =============================================================================

/// How many TAG associations one application status check may hold: "You can create a maximum
/// of 50 tag associations for each application status check"
/// (`AssociateApplicationStatusCheck`).
///
/// A DOCUMENTED ceiling rather than one this emulator chose, so it is the bound an association
/// asks about, counted over the pairs the check would hold afterwards.
pub const MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS: usize = 50;

/// The `AssociationType` of a TAG association, as the association response objects spell it:
/// "The type of association. Valid values: EC2TAG and INSTANCE_ID"
/// (`SuccessfulAssociationResponseObject$AssociationType`).
///
/// A different spelling from the `AssociationTypeEnum` the association DESCRIBE reports
/// (`tag`, `instance-id`): one association kind, two spellings, each a fact of the shape that
/// carries it.
pub const ASSOCIATION_TYPE_EC2_TAG: &str = "EC2TAG";

/// The `AssociationType` of an INSTANCE association — see [`ASSOCIATION_TYPE_EC2_TAG`].
pub const ASSOCIATION_TYPE_INSTANCE_ID: &str = "INSTANCE_ID";

/// What separates the two halves of a tag association's reported value: "the value is
/// formatted as `key=value`" (`SuccessfulAssociationResponseObject$AssociationValue`).
pub const TAG_ASSOCIATION_VALUE_SEPARATOR: char = '=';

/// The `AssociationValue` of one tag association: "For EC2TAG, the value is formatted as
/// `key=value`" (`SuccessfulAssociationResponseObject$AssociationValue`) — the ONE place that
/// value is built, so every reader of it reads one form.
pub fn tag_association_value(key: &str, value: &str) -> String {
    format!("{key}{TAG_ASSOCIATION_VALUE_SEPARATOR}{value}")
}

/// Whether `check` already monitors the instances carrying the tag `(key, value)`.
///
/// The PAIR is the association's identity, not the key alone: "Each tag is a key-value pair"
/// (`AssociateApplicationStatusCheckRequest$TargetTagAssociations`), so one check can monitor
/// two values of one key and neither displaces the other.
pub fn holds_tag_association(
    check: &ApplicationStatusCheckResponseObject,
    key: &str,
    value: &str,
) -> bool {
    check
        .target_tag_associations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|pair| {
            pair.key.as_deref().unwrap_or_default() == key
                && pair.value.as_deref().unwrap_or_default() == value
        })
}

/// Whether the check `application_status_check_id` names already monitors `instance_id`.
///
/// An instance association IS the per-check result the instance holds for that check
/// ([`ComAmazonawsEc2Amazonec2State::application_status`] keeps "the result of every
/// application status check associated with this instance"), so the association is read off
/// that one store rather than from a second map of associated instances that could disagree
/// with it.
pub fn holds_instance_association(
    store: &ComAmazonawsEc2Amazonec2State,
    application_status_check_id: &str,
    instance_id: &str,
) -> bool {
    store
        .application_status
        .get(instance_id)
        .is_some_and(|record| record.details.contains_key(application_status_check_id))
}

/// The per-check result an instance gets the moment a check is associated with it: the check
/// applies and has reported nothing yet.
///
/// "Once you create an association, health monitoring automatically begins for the specified
/// instances" (`AssociateApplicationStatusCheck`), and a check that is monitoring but has
/// reported no result is `initializing` — "The check is initializing or has not reached a
/// success or failure threshold" (`ApplicationStatusDetail$Status`). The detail carries the
/// check's own aggregation setting, which is what decides whether it contributes to the
/// instance-level status ([`super::aggregated_application_status`]).
pub fn initializing_application_status_detail(
    check: &ApplicationStatusCheckResponseObject,
    at: SystemTime,
) -> ApplicationStatusDetail {
    let at = super::system_time_to_date_time(at);
    ApplicationStatusDetail {
        application_status_check_id: check.application_status_check_id.clone(),
        check_update_time: Some(at),
        aggregation: check.aggregation.clone(),
        status: Some(super::APPLICATION_STATUS_CHECK_INITIALIZING.to_string()),
        status_time_stamp: Some(at),
        status_since: Some(at),
        // "The reason for the current status" belongs to a check that reported one; nothing
        // has reported yet.
        reason: None,
    }
}

/// Why `instance_id` cannot be monitored by an application status check, or `None` when the
/// account holds that instance.
///
/// The SENTENCE this service refuses that instance id with
/// ([`super::require_instance_record`], a malformed id before an unknown one), carried as one
/// item's `Reason` instead of as a refusal of the whole request: the association operations
/// report per-item outcomes ("The associations that were successfully created" beside "The
/// associations that failed to be created"), so one instance the account does not hold fails
/// on its own row and the rest of the request is still carried out.
pub fn instance_association_failure_reason(
    store: &ComAmazonawsEc2Amazonec2State,
    instance_id: &str,
) -> Option<String> {
    let refusal: Scaffolded<()> = match super::require_instance_record(store, instance_id) {
        Ok(_) => return None,
        Err(refusal) => refusal,
    };
    // `None` is the caller's "this instance can be monitored", so every refusal has to come
    // back as a reason: the id reader answers one of this service's own error codes, and the
    // modelled channel carries no message at all (EC2's model declares no error shape), so
    // that one variant is worded from the id itself rather than reported as no failure.
    Some(match refusal {
        Scaffolded::ServiceError(refusal) => refusal.message,
        Scaffolded::ValidationException(refusal) => refusal.message,
        Scaffolded::NotImplemented(refusal) => refusal.message,
        Scaffolded::Modeled(()) => super::invalid_id_message(instance_id),
    })
}

/// Refuse an association that would take one check past the documented ceiling of tag
/// associations: HTTP 400 [`super::RESOURCE_LIMIT_EXCEEDED`], the code this service answers
/// for a published limit with no code of its own.
pub fn tag_association_limit_exceeded<E>(held: usize, asked: usize) -> Scaffolded<E> {
    super::resource_limit_exceeded(format!(
        "An application status check holds at most \
         {MAX_APPLICATION_STATUS_CHECK_TAG_ASSOCIATIONS} tag associations, and this one \
         already holds {held}, so it cannot take {asked} more"
    ))
}

