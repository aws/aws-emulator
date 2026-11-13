// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAddonVersions
//! IMPLEMENTED. Reports the add-on catalog this service's state holds
//! (`crate::state::AddonCatalogRecord`, seeded from the add-ons Amazon EKS
//! publishes and the ones listed on AWS Marketplace, each add-on's builds newest
//! first), narrowed by the request's filters.
//!
//! This is a REPORTING operation: it writes nothing. The catalog is a
//! PUBLICATION, not per-account data — "Describes the versions for an add-on" —
//! so it is seeded once and every member of a reported `AddonInfo` comes from
//! that seed. Which builds are OFFERED is derived per request from the clock:
//! a build is published alongside the Kubernetes version it supports, so a build
//! for a Kubernetes version the calendar has not released yet is not reported,
//! exactly as `DescribeClusterVersions` does not report that version.
//!
//! What each input member does here:
//!
//! * `addonName` — keeps only the add-on of that name ("The name must match one
//!   of the names returned by `ListAddons`"); a name the catalog does not
//!   publish simply matches nothing, which is an EMPTY listing rather than an
//!   error, because this operation reports a catalog rather than resolving a
//!   resource.
//! * `kubernetesVersion` — "The Kubernetes versions that you can use the add-on
//!   with": keeps only the builds that support that version, and reports each
//!   kept build's compatibility for that version alone, so an add-on with no
//!   build for it drops out of the listing entirely.
//! * `types` / `publishers` / `owners` — keep the add-ons whose own `type`,
//!   `publisher` or `owner` is one of the values given; several values in one
//!   member widen it, and the members combine.
//! * `maxResults` / `nextToken` — the catalog is served WHOLE: the recorded
//!   corpus shows the real service answering with every add-on that matches and
//!   no `nextToken`, whatever those two members say — it SERVES a `maxResults`
//!   past the 1..100 bound its own documentation states, and a `nextToken` as
//!   well, in both cases answering with the whole matching catalog. So neither
//!   member narrows anything here, nothing is truncated and no token is minted;
//!   there is no page after the first to resume from. The bound is not declared
//!   on the shape this operation's input binds either
//!   (`DescribeAddonVersionsRequestMaxResults` in `smithy-model/`, which no other
//!   operation binds): declaring it would have the centralized walk refuse,
//!   before this handler runs, a call the real service answers. It is still what
//!   the documentation above advises a caller to stay inside.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAddonVersions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes the versions for an add-on. Information such as the Kubernetes versions that
//! you can use the add-on with, the owner, publisher, and the type of the add-on are
//! returned.
//!
//! ## Input
//!
//! * `addonName` (String) — The name of the add-on. The name must match one of the names
//!   returned by ListAddons .
//! * `kubernetesVersion` (String) — The Kubernetes versions that you can use the add-on with.
//! * `maxResults` (DescribeAddonVersionsRequestMaxResults) — The maximum number
//!   of results, returned in paginated output. You receive maxResults in a single page, along
//!   with a nextToken response element. You can see the remaining results of the initial
//!   request by sending another request with the returned nextToken value. This value can be
//!   between 1 and 100. If you don't use this parameter, 100 results and a nextToken value,
//!   if applicable, are returned.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `owners` (StringList) — The owner of the add-on. For valid owners, don't specify a value
//!   for this property.
//! * `publishers` (StringList) — The publisher of the add-on. For valid publishers, don't
//!   specify a value for this property.
//! * `types` (StringList) — The type of the add-on. For valid types, don't specify a value
//!   for this property.
//!
//! ## Output
//!
//! * `addons` (Addons) — The list of available versions with Kubernetes version compatibility
//!   and other properties.
//! * `nextToken` (String) — The nextToken value to include in a future DescribeAddonVersions
//!   request. When the results of a DescribeAddonVersions request exceed maxResults, you can
//!   use this value to retrieve the next page of results. This value is null when there are
//!   no more results to return. This token should be treated as an opaque identifier that is
//!   used only to retrieve the next items in a list and not for other programmatic purposes.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::AddonCatalogRecord;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAddonVersionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeAddonVersionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeAddonVersions")]
#[derive(Debug)]
pub enum DescribeAddonVersionsErrorModeled {
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_addon_versions::DescribeAddonVersionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeAddonVersionsError = crate::scaffolded::Scaffolded<DescribeAddonVersionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeAddonVersionsErrorModeled> for DescribeAddonVersionsError {
    fn from(modeled: DescribeAddonVersionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAddonVersions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAddonVersions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAddonVersions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAddonVersions<Eng>
{
    type Input = DescribeAddonVersionsInput;
    type Output = DescribeAddonVersionsOutput;
    type Error = DescribeAddonVersionsError;

    const NAME: &'static str = "DescribeAddonVersions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_VERSIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_VERSIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeAddonVersionsError::from(DescribeAddonVersionsErrorModeled::ServerException(
                ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // The catalog is GLOBAL: an add-on version is published for the service,
        // not per Region or per account, so nothing here reads a region or an
        // account from the request.
        let now = SystemTime::now();

        // The catalog is served WHOLE: the recorded corpus shows the real service
        // answering with every add-on that matches and no `nextToken`, whatever
        // `maxResults` and `nextToken` say — so neither narrows the answer, nothing
        // is truncated, and no token is minted. There is no page after the first to
        // resume from, which is why an incoming `nextToken` names no position and
        // changes nothing.
        let _ = (&input.max_results, &input.next_token);

        let requested_name = input.addon_name.as_deref();
        let kubernetes_version = input.kubernetes_version.as_deref();
        let types = input.types.unwrap_or_default();
        let publishers = input.publishers.unwrap_or_default();
        let owners = input.owners.unwrap_or_default();
        // A filter member given no values narrows nothing, so an absent member and an
        // empty list are the same request.
        let keeps = |values: &[String], published: &str| {
            values.is_empty() || values.iter().any(|value| value == published)
        };

        let guard = store.lock();
        let addons: Vec<AddonInfo> = guard
            .published_addons(now)
            .filter(|addon| requested_name.is_none_or(|name| name == addon.addon_name))
            .filter(|addon| keeps(&types, &addon.addon_type))
            .filter(|addon| keeps(&publishers, &addon.publisher))
            .filter(|addon| keeps(&owners, &addon.owner))
            .filter_map(|addon| describe(addon, kubernetes_version, now))
            .collect();

        Ok(DescribeAddonVersionsOutput {
            addons: Some(addons),
            next_token: None,
        })
    }
}

/// One catalog entry as the `AddonInfo` this listing reports — or `None` when the
/// entry has no build to report.
///
/// An add-on whose every published build is filtered out has no versions to
/// describe, so it is not part of the answer: "The list of available versions
/// with Kubernetes version compatibility and other properties" is a list of
/// add-ons that HAVE such a version.
fn describe(
    addon: &AddonCatalogRecord,
    kubernetes_version: Option<&str>,
    now: SystemTime,
) -> Option<AddonInfo> {
    let versions = addon.to_addon_version_infos(now, kubernetes_version);
    (!versions.is_empty()).then(|| addon.to_addon_info(versions))
}
