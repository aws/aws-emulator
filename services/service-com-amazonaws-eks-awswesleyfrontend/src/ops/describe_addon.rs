// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAddon
//!
//! IMPLEMENTED. `DescribeAddon` REPORTS the [`crate::state::AddonRecord`] that
//! `CreateAddon` wrote; it is the read-back for that slice and writes nothing of its
//! own beyond materialising the status move the clock already owed
//! ([`crate::state::AddonRecord::materialize_due_transitions`]), which was due whether
//! or not anybody called.
//!
//! The layers are the same as `CreateAddon`'s, outside in: the cluster named by the
//! `@httpLabel` must exist in the request's region, and then the add-on must be
//! installed on it — each missing resource is the modelled `ResourceNotFoundException`
//! (404), naming which one was not found.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAddon`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes an Amazon EKS add-on.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by ListAddons .
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `addon` (Addon) — An Amazon EKS add-on. For more information, see Amazon EKS add-ons in
//!   the Amazon EKS User Guide.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
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
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAddonError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeAddonError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeAddon")]
#[derive(Debug)]
pub enum DescribeAddonErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_addon::DescribeAddonError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeAddonError = crate::scaffolded::Scaffolded<DescribeAddonErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeAddonErrorModeled> for DescribeAddonError {
    fn from(modeled: DescribeAddonErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAddon` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAddon<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAddon<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAddon<Eng>
{
    type Input = DescribeAddonInput;
    type Output = DescribeAddonOutput;
    type Error = DescribeAddonError;

    const NAME: &'static str = "DescribeAddon";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeAddonError::from(DescribeAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // THE REQUEST'S OWN ADDRESSING FIRST, before the store is even locked:
        // `clusterName` arrives in the path and every cluster name of this API obeys
        // one grammar and one length maximum (`ClusterName` is `@length(1..100)` +
        // `@pattern`, [`crate::state::is_cluster_name`]), so a label outside it names
        // no cluster in ANY Region — it is the invalid parameter it is, in the sentence
        // Amazon EKS states for that parameter
        // ([`crate::state::cluster_name_refusal`]), rather than a not-found that would
        // invite the caller to create a cluster under a name Amazon EKS will never
        // accept. The framework's validating walk does not judge a value bound to an
        // `@httpLabel`, so this is where it is judged.
        if !crate::state::is_cluster_name(&input.cluster_name) {
            return Err(DescribeAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(crate::state::cluster_name_refusal(
                        "clusterName",
                        &input.cluster_name,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // What the CloudWatch agent of a cluster of this Region still OWES CloudWatch
        // Logs — the Container Insights performance its collection interval has made due
        // — is delivered before the add-on is read. The agent runs because the add-on is
        // `ACTIVE`, and it collects on its own interval whether or not anybody called, so
        // this read materialises what the clock already owed (see
        // [`crate::state::settle_container_insights`]). A cluster running no agent owes
        // nothing and nothing is asked of CloudWatch Logs.
        crate::state::settle_container_insights(ctx.callback(), &*self.engine, &region).await;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // The cluster first: an add-on exists only inside one, and a cluster name
        // identifies a cluster only within the region the request is signed for. This
        // read addresses ONE resource — the `(cluster, add-on)` pair — so neither half
        // is named and no identity member is serialized
        // ([`crate::state::resource_not_found`]).
        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(DescribeAddonErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
        };

        // An add-on is a piece of software Amazon EKS manages inside the cluster, and a
        // cluster `RegisterCluster` connected has no EKS-managed plane for one to live
        // in: the operation does not apply to it, which the connected-cluster gate
        // reports. It is a fact about the CLUSTER, so it precedes resolving the add-on.
        if cluster.is_connected() {
            return Err(DescribeAddonErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        let key = (region, input.cluster_name.clone(), input.addon_name.clone());
        match guard.addons.get(&key) {
            // "An array of EKS Pod Identity associations owned by the add-on": the
            // associations live in the store's own map, so what this add-on still
            // owns is read from there rather than from the list it was created
            // with — a caller may have deleted one directly.
            Some(record) => {
                let live = guard.association_arns_owned_by(&record.arn);
                Ok(DescribeAddonOutput {
                    addon: Some(record.to_addon(&live)),
                })
            }
            None => Err(DescribeAddonErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into()),
        }
    }
}
