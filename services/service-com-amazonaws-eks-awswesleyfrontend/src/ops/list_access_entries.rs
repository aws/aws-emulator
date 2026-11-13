// SPDX-License-Identifier: CC0-1.0

//! @op ListAccessEntries
//! IMPLEMENTED. Walks the access entries the store actually holds for one
//! cluster — [`crate::state::ClusterRecord::access_entries`], the same map
//! `CreateAccessEntry` writes and `DescribeAccessEntry` reads — in its sorted
//! principal-ARN order, and pages them with an opaque `nextToken` derived from
//! the last principal returned (so a token stays meaningful across creates and
//! deletes rather than indexing a transient vector).
//!
//! The listing is REPORTING only: it writes nothing, and a cluster this store
//! does not hold in the region the request is signed for is answered
//! `ResourceNotFoundException` rather than an empty page — the entries of a
//! cluster that does not exist are not "none", they are unknowable.
//!
//! `associatedPolicyArn` narrows the listing to the entries that policy is
//! associated to, read from each entry's own
//! [`crate::state::AccessEntryRecord::associated_policies`] — the map
//! `AssociateAccessPolicy` writes — so a cluster whose entries carry no
//! association for that policy lists none of them.
//!
//! One thing a real cluster's listing carries that this one does not: the
//! cluster-creator admin entry `CreateCluster` makes when
//! `accessConfig.bootstrapClusterCreatorAdminPermissions` is true. Minting it is
//! `CreateCluster`'s write and it needs the caller's IAM principal — the
//! identity this emulator's STS resolves from the request's credential, which
//! this service does not reach — so it is left to that operation's own issue
//! rather than invented here from a listing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAccessEntries`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the access entries for your cluster.
//!
//! ## Input
//!
//! * `associatedPolicyArn` (String) — The ARN of an AccessPolicy. When you specify an access
//!   policy ARN, only the access entries associated to that access policy are returned. For a
//!   list of available policy ARNs, use ListAccessPolicies.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `maxResults` (ListAccessEntriesRequestMaxResults, range 1..100) — The maximum number of
//!   results, returned in paginated output. You receive maxResults in a single page, along
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
//!
//! ## Output
//!
//! * `accessEntries` (StringList) — The list of access entries that exist for the cluster.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, ACCESS_ENTRIES_LISTING, DEFAULT_LIST_PAGE_SIZE};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAccessEntriesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAccessEntriesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListAccessEntries",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum ListAccessEntriesErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_access_entries::ListAccessEntriesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAccessEntriesError = crate::scaffolded::Scaffolded<ListAccessEntriesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAccessEntriesErrorModeled> for ListAccessEntriesError {
    fn from(modeled: ListAccessEntriesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAccessEntries` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAccessEntries<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAccessEntries<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAccessEntries<Eng>
{
    type Input = ListAccessEntriesInput;
    type Output = ListAccessEntriesOutput;
    type Error = ListAccessEntriesError;

    const NAME: &'static str = "ListAccessEntries";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_ENTRIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_ENTRIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            ListAccessEntriesError::from(ListAccessEntriesErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // A `nextToken` names the principal to resume AFTER. One this service did not
        // issue for THIS listing is a bad parameter, not an empty page — refused in the
        // wording the ACCESS-ENTRY plane this listing belongs to states such a member in
        // ([`state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE`]).
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(ACCESS_ENTRIES_LISTING, token).ok_or_else(|| {
                    ListAccessEntriesError::from(
                        ListAccessEntriesErrorModeled::InvalidParameterException(
                            InvalidParameterException {
                                cluster_name: Some(input.cluster_name.clone()),
                                message: Some(
                                    state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                                ),
                                ..Default::default()
                            },
                        ),
                    )
                })?,
            ),
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a
        // value, and it is a whole page.
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize);

        let guard = store.lock();
        // A cluster name identifies a cluster only within one region, so the entries
        // this lists are those of the cluster in the region the request is signed for
        // — and a cluster this store does not hold has no listing at all.
        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            // This listing resolves the CLUSTER only — the entries it enumerates hang
            // off it — so the cluster is the resource it reports missing, named in the
            // message and in `clusterName` ([`state::cluster_not_found`]).
            return Err(ListAccessEntriesErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };

        // Access entries are part of the Amazon EKS-managed plane, which a cluster
        // `RegisterCluster` connected has none of: EKS grants no Kubernetes access on
        // it, so this operation does not apply to it — which is a refusal, not an empty
        // listing, because the listing itself is not something EKS keeps for that
        // cluster.
        if cluster.is_connected() {
            return Err(ListAccessEntriesErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to
        // enumerate ([`state::serves_access_entries`]) — which is a refusal rather
        // than an empty listing, for the same reason the gate above is: the listing
        // itself is not something EKS keeps for that cluster.
        if !cluster.serves_access_entries() {
            return Err(ListAccessEntriesErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }

        let associated_policy_arn = input.associated_policy_arn.as_deref();
        // The entries hang off the cluster record, keyed by principal ARN, so this
        // walks them in sorted principal order with no second map to consult.
        let mut remaining = cluster
            .access_entries
            .values()
            // "When you specify an access policy ARN, only the access entries
            // associated to that access policy are returned."
            .filter(|record| {
                associated_policy_arn
                    .is_none_or(|arn| record.associated_policies.contains_key(arn))
            })
            .map(|record| record.principal_arn.as_str())
            .filter(|principal| {
                resume_after
                    .as_deref()
                    .is_none_or(|after| *principal > after)
            });
        let access_entries: Vec<String> = remaining
            .by_ref()
            .take(page_size)
            .map(str::to_owned)
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), access_entries.last()) {
            (Some(_), Some(last)) => {
                Some(state::encode_scoped_page_token(ACCESS_ENTRIES_LISTING, last))
            }
            _ => None,
        };

        Ok(ListAccessEntriesOutput {
            access_entries: Some(access_entries),
            next_token,
        })
    }
}
