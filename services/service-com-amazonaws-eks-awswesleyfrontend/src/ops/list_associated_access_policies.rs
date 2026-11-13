// SPDX-License-Identifier: CC0-1.0

//! @op ListAssociatedAccessPolicies
//! IMPLEMENTED. A pure read of the
//! [`crate::state::AssociatedPolicyRecord`]s `AssociateAccessPolicy` wrote onto the
//! access entry, in sorted policy-ARN order — which is the order the store holds
//! them in, so a `nextToken` can name the policy ARN to resume after and stays
//! valid across later associations.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAssociatedAccessPolicies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the access policies associated with an access entry.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `maxResults` (ListAssociatedAccessPoliciesRequestMaxResults, range 1..100) — The maximum
//!   number of results, returned in paginated output. You receive maxResults in a single
//!   page, along with a nextToken response element. You can see the remaining results of the
//!   initial request by sending another request with the returned nextToken value. This value
//!   can be between 1 and 100. If you don't use this parameter, 100 results and a nextToken
//!   value, if applicable, are returned.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `principalArn` (String) **required** — The ARN of the IAM principal for the AccessEntry.
//!
//! ## Output
//!
//! * `associatedAccessPolicies` (AssociatedAccessPoliciesList) — The list of access policies
//!   associated with the access entry.
//! * `clusterName` (String) — The name of your cluster.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `principalArn` (String) — The ARN of the IAM principal for the AccessEntry.
//!
//! ## Errors
//!
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, ASSOCIATED_ACCESS_POLICIES_LISTING, DEFAULT_LIST_PAGE_SIZE};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAssociatedAccessPoliciesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAssociatedAccessPoliciesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
//
// `InvalidParameterException` is widened in the same way: this operation's own list
// carries only the state-dependent `InvalidRequestException`, but a `nextToken` this
// listing did not mint IS a bad parameter, and the SERVICE models that shape ("The
// specified parameter is invalid. Review the available parameters for the API
// request.") — so every listing of this service answers a foreign page token with the
// one code (`ListClusters`, `ListAccessEntries` and `ListAccessPolicies` already do).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListAssociatedAccessPolicies",
    extra_errors = ["ClientException", "InvalidParameterException"]
)]
#[derive(Debug)]
pub enum ListAssociatedAccessPoliciesErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_associated_access_policies::ListAssociatedAccessPoliciesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAssociatedAccessPoliciesError = crate::scaffolded::Scaffolded<ListAssociatedAccessPoliciesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAssociatedAccessPoliciesErrorModeled> for ListAssociatedAccessPoliciesError {
    fn from(modeled: ListAssociatedAccessPoliciesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAssociatedAccessPolicies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAssociatedAccessPolicies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAssociatedAccessPolicies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAssociatedAccessPolicies<Eng>
{
    type Input = ListAssociatedAccessPoliciesInput;
    type Output = ListAssociatedAccessPoliciesOutput;
    type Error = ListAssociatedAccessPoliciesError;

    const NAME: &'static str = "ListAssociatedAccessPolicies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ASSOCIATED_ACCESS_POLICIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ASSOCIATED_ACCESS_POLICIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListAssociatedAccessPoliciesError::from(
                ListAssociatedAccessPoliciesErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        // This listing hangs off ONE resource — the `(cluster, access entry)` pair —
        // so a miss of either half is the same generic, memberless payload:
        // [`state::resource_not_found`].
        let not_found = || {
            ListAssociatedAccessPoliciesError::from(
                ListAssociatedAccessPoliciesErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };

        // A `nextToken` names the policy ARN to resume AFTER, in THIS listing's own
        // token scope, so a well-formed token another listing issued is refused here
        // too. One this service did not issue is a MALFORMED PARAMETER — the value the
        // caller handed in cannot be decoded, whatever the cluster or the access entry
        // is doing — so it is refused as `InvalidParameterException`, the code every
        // other listing of this service answers a foreign token with, rather than as
        // the state-dependent `InvalidRequestException`. The SENTENCE is the one the
        // ACCESS-ENTRY plane this listing belongs to states such a member in
        // ([`state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE`]), which is not the
        // pagination sentence the cluster-management listings use.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(ASSOCIATED_ACCESS_POLICIES_LISTING, token)
                    .ok_or_else(|| {
                        ListAssociatedAccessPoliciesError::from(
                            ListAssociatedAccessPoliciesErrorModeled::InvalidParameterException(
                                InvalidParameterException {
                                    cluster_name: Some(input.cluster_name.clone()),
                                    message: Some(
                                        state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE
                                            .to_string(),
                                    ),
                                    ..Default::default()
                                },
                            ),
                        )
                    })?,
            ),
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED
        // by schema validation before this handler runs. Only the absent case gets a
        // value: "If you don't use this parameter, 100 results and a nextToken value,
        // if applicable, are returned." The floor of 1 keeps the handler
        // self-contained: a page size of zero would answer an empty page with no
        // token, which reads as "no associations" rather than as a refusal.
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize)
            .max(1);

        let region = state::request_region(ctx.parts());
        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        let Some(cluster) = guard.clusters.get(&(region, input.cluster_name.clone())) else {
            return Err(not_found());
        };
        // Access entries, and the policies associated to them, are part of the Amazon
        // EKS-managed plane, which a cluster `RegisterCluster` connected has none of: EKS
        // grants no Kubernetes access on it, so this operation does not apply to it. The
        // gate is a fact about the CLUSTER, so it precedes resolving the entry.
        if cluster.is_connected() {
            return Err(
                ListAssociatedAccessPoliciesErrorModeled::ClientException(
                    crate::state::connected_cluster_refusal(&input.cluster_name),
                )
                .into(),
            );
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to
        // enumerate the policies of ([`crate::state::serves_access_entries`]). The
        // gate is a fact about the CLUSTER too, so it precedes resolving the entry.
        if !cluster.serves_access_entries() {
            return Err(
                ListAssociatedAccessPoliciesErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(
                            crate::state::access_entry_authentication_mode_refusal(),
                        ),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }
        let Some(entry) = cluster.access_entries.get(&input.principal_arn) else {
            return Err(not_found());
        };

        let mut remaining = entry
            .associated_policies
            .iter()
            .filter(|(policy_arn, _)| {
                resume_after
                    .as_deref()
                    .is_none_or(|after| policy_arn.as_str() > after)
            });
        let page: Vec<(&String, AssociatedAccessPolicy)> = remaining
            .by_ref()
            .take(page_size)
            .map(|(policy_arn, record)| (policy_arn, record.to_associated_access_policy()))
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some((last, _))) => Some(state::encode_scoped_page_token(
                ASSOCIATED_ACCESS_POLICIES_LISTING,
                last,
            )),
            _ => None,
        };

        Ok(ListAssociatedAccessPoliciesOutput {
            cluster_name: Some(input.cluster_name),
            principal_arn: Some(input.principal_arn),
            next_token,
            associated_access_policies: Some(
                page.into_iter().map(|(_, policy)| policy).collect(),
            ),
        })
    }
}
