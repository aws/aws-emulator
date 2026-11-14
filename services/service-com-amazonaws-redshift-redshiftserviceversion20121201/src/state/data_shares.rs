// SPDX-License-Identifier: CC0-1.0

//! The datashares of this account, and the namespaces a datashare operation
//! names one of.
//!
//! A datashare is created on the PRODUCER's database, not through this control
//! plane: Amazon Redshift publishes no API that creates one — `AuthorizeDataShare`
//! only "authorizes the sharing of a datashare with one or more consumer
//! accounts" and `DescribeDataShares` "shows the status of all datashares of the
//! account" (`smithy_query(query="doc", operation="AuthorizeDataShare")`,
//! `smithy_query(query="doc", operation="DescribeDataShares")`). So this
//! collection is like
//! [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::recommendations`]:
//! real state, keyed and ordered like every other collection of this store, which
//! no operation of this service adds to — an account that has created no
//! datashare has none, and every operation that acts on one answers the modelled
//! `InvalidDataShareFault` for an ARN that resolves to nothing.
//!
//! The NAMESPACE arm is resolved rather than held: a producer or consumer ARN
//! names a namespace, and a namespace belongs either to this control plane (a
//! provisioned cluster's `Cluster$ClusterNamespaceArn`) or to Amazon Redshift
//! Serverless — a different service with its own store — so the second is asked
//! of that service over the in-process callback, exactly as a namespace
//! registration and a zero-ETL target are ([`super::serverless`]).

use super::*;

/// A datashare as this service stores it.
///
/// The modelled `DataShare` shape (generated under the name
/// [`crate::ops::types::AssociateDataShareConsumerOutput`], because the
/// operations that act on a datashare return it unwrapped) IS the whole record: a
/// datashare carries no status the clock moves and nothing the shape has no
/// member for, so — like
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`] —
/// this collection needs no `Stored*` wrapper. The alias names what the records
/// are, so a handler does not read the association operation's output type as the
/// stored record by accident.
pub type StoredDataShare = crate::ops::types::AssociateDataShareConsumerOutput;

/// Why a request naming a `DataShareArn` this account holds no datashare under is
/// refused, naming the ARN the request carried.
///
/// `smithy_query(query="shape", shape="InvalidDataShareFault")` — "There is an
/// error with the datashare" — which says nothing a caller can act on, so the
/// sentence names what was wrong with the value it carried: the datashare the ARN
/// names is not one of this account's.
pub fn invalid_data_share_message(data_share_arn: &str) -> String {
    format!("Datashare {data_share_arn} not found.")
}

/// Why a request naming a namespace that resolves to nothing is refused, naming
/// the ARN the request carried.
///
/// `smithy_query(query="shape", shape="InvalidNamespaceFault")` — "The namespace
/// isn't valid because the namespace doesn't exist. Provide a valid namespace." —
/// whose own first clause is the fixed text and whose variable part is the ARN the
/// caller has to recognise.
pub fn invalid_namespace_message(namespace_arn: &str) -> String {
    format!("The namespace {namespace_arn} isn't valid because the namespace doesn't exist. Provide a valid namespace.")
}

/// What resolving one namespace ARN found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceResolution {
    /// A namespace of this account: a provisioned cluster of this store, or a
    /// serverless namespace the control plane that owns it reported.
    Held,
    /// The ARN was resolved against the control plane that would own it, and
    /// there is no such namespace — so a request naming it names no namespace.
    Absent,
    /// The ARN names a serverless namespace and that control plane could not be
    /// asked, so whether it exists is UNKNOWN. This emulator failing to resolve a
    /// fact about itself is not a caller naming something that does not exist, so
    /// the two are never answered the same way (see
    /// [`super::serverless::ServerlessTarget::Unresolvable`]).
    Undecided,
}

/// The service field a namespace ARN of this control plane carries, and the
/// resource prefixes such an ARN is written with.
///
/// Source: the `TargetArn` @pattern quoted in [`super::serverless`] —
/// `^arn:aws[a-z\-]*:redshift(-serverless)?:[a-z0-9\-]+:[0-9]{12}:(namespace\/|namespace:)[a-z0-9\-]+$`
/// — which is the one place this service's model states how a namespace ARN of
/// either control plane is spelled.
const PROVISIONED_ARN_SERVICE: &str = "redshift";
const NAMESPACE_RESOURCE_PREFIXES: [&str; 2] = ["namespace/", "namespace:"];

/// Whether `value` is a namespace ARN of THIS control plane: a `redshift` ARN
/// whose resource is a namespace.
pub fn is_provisioned_namespace_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    if fields.next() != Some("arn") {
        return false;
    }
    let _partition = fields.next();
    if fields.next() != Some(PROVISIONED_ARN_SERVICE) {
        return false;
    }
    let _region = fields.next();
    let _account = fields.next();
    fields.next().is_some_and(|resource| {
        NAMESPACE_RESOURCE_PREFIXES
            .iter()
            .any(|prefix| resource.strip_prefix(prefix).is_some_and(|token| !token.is_empty()))
    })
}

/// Resolve the namespace `namespace_arn` against the control plane that owns it.
///
/// ONE rule for every datashare operation that names a namespace, so a read and a
/// write cannot resolve the same ARN differently:
///
/// * a namespace ARN of this control plane is a namespace of this account exactly
///   when one of its clusters reports it (`Cluster$ClusterNamespaceArn`);
/// * a namespace ARN of Amazon Redshift Serverless is resolved by ASKING that
///   service ([`super::serverless::resolve_zero_etl_target`]), which is where the
///   namespace lives;
/// * a value that is neither names no namespace at all.
///
/// Call it with NO store guard held: the serverless arm awaits. The provisioned
/// arm is answered from `held`, which the caller reads out of this store under its
/// own guard before calling.
pub async fn resolve_namespace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    held: bool,
    namespace_arn: &str,
) -> NamespaceResolution {
    if is_provisioned_namespace_arn(namespace_arn) {
        return if held {
            NamespaceResolution::Held
        } else {
            NamespaceResolution::Absent
        };
    }
    match super::serverless::resolve_zero_etl_target(ctx, namespace_arn).await {
        // A serverless namespace that exists is a namespace a datashare can
        // belong to whether or not compute is attached to it right now: what the
        // datashare reads are about is the namespace, not its workgroups.
        Some(serverless::WarehouseTarget::Active | serverless::WarehouseTarget::NotActive) => {
            NamespaceResolution::Held
        }
        Some(serverless::WarehouseTarget::Absent) => NamespaceResolution::Absent,
        Some(serverless::WarehouseTarget::Unresolvable(reason)) => {
            // Visible at default verbosity, for the same reason the registration
            // path logs it: a read served because the owning control plane could
            // not be asked reads exactly like one served because the namespace is
            // really there.
            ::tracing::warn!(
                reason = %reason,
                "the serverless namespace a datashare read names could not be resolved"
            );
            NamespaceResolution::Undecided
        }
        // Not a namespace ARN of either control plane.
        None => NamespaceResolution::Absent,
    }
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// The stored datashare with this `DataShareArn`, if any.
    pub fn data_share(&self, data_share_arn: &str) -> Option<&StoredDataShare> {
        self.data_shares
            .iter()
            .find(|held| held.data_share_arn.as_deref() == Some(data_share_arn))
    }

    /// Apply one change to the datashare `data_share_arn` holds, and answer the
    /// datashare as it stands afterwards — or `None` when this account holds no
    /// datashare under that ARN, which is what every datashare operation answers
    /// `InvalidDataShareFault` for.
    ///
    /// ONE rule for what each of the five datashare writes DOES, held beside the
    /// state it changes: they all move the same thing — the association between
    /// the datashare and one consumer — so no two of them can report the same
    /// datashare differently.
    pub fn change_data_share(
        &mut self,
        data_share_arn: &str,
        consumer: &str,
        consumer_region: Option<&str>,
        change: DataShareChange,
        now: ::aws_smithy_types::DateTime,
    ) -> Option<StoredDataShare> {
        let held = self
            .data_shares
            .iter_mut()
            .find(|held| held.data_share_arn.as_deref() == Some(data_share_arn))?;
        let associations = held.data_share_associations.get_or_insert_default();
        let position = associations
            .iter()
            .position(|association| association.consumer_identifier.as_deref() == Some(consumer));
        match change.status() {
            // A disassociation removes the association rather than moving its
            // status: "removes association of a datashare from the consumer".
            None => {
                if let Some(position) = position {
                    associations.remove(position);
                }
            }
            Some(status) => {
                let association = match position {
                    Some(position) => &mut associations[position],
                    None => {
                        associations.push(crate::ops::types::DataShareAssociation {
                            consumer_identifier: Some(consumer.to_string()),
                            created_date: Some(now),
                            ..Default::default()
                        });
                        // Pushed just above, so the last entry is this one.
                        associations.last_mut()?
                    }
                };
                association.status = Some(status.to_string());
                association.status_change_date = Some(now);
                association.consumer_region = consumer_region
                    .map(str::to_string)
                    .or_else(|| association.consumer_region.clone());
                match change {
                    // "Specifies whether write operations were allowed during
                    // data share authorization": the producer's decision.
                    DataShareChange::Authorize { allow_writes } => {
                        association.producer_allowed_writes = allow_writes;
                    }
                    // "Specifies whether write operations were allowed during
                    // data share association": the consumer's.
                    DataShareChange::Associate { allow_writes } => {
                        association.consumer_accepted_writes = allow_writes;
                    }
                    DataShareChange::Deauthorize
                    | DataShareChange::Reject
                    | DataShareChange::Disassociate => {}
                }
            }
        }
        Some(held.clone())
    }

    /// The datashares whose `ProducerArn` is `producer_arn`, in the order the
    /// store holds them.
    pub fn data_shares_of_producer(&self, producer_arn: &str) -> Vec<&StoredDataShare> {
        self.data_shares
            .iter()
            .filter(|held| held.producer_arn.as_deref() == Some(producer_arn))
            .collect()
    }

    /// The datashares one of whose associations names `consumer_arn`, in the
    /// order the store holds them.
    ///
    /// A datashare reaches a consumer through its associations
    /// (`DataShare$DataShareAssociations`), each of which names the consumer it is
    /// an association with — so the consumer's own view is read off those rather
    /// than held beside them.
    pub fn data_shares_of_consumer(&self, consumer_arn: &str) -> Vec<&StoredDataShare> {
        self.data_shares
            .iter()
            .filter(|held| {
                held.data_share_associations
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|association| {
                        association.consumer_identifier.as_deref() == Some(consumer_arn)
                    })
            })
            .collect()
    }
}

/// What one datashare write does to the association it names.
///
/// The five writes of this family, as the one rule that applies them
/// ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::change_data_share`])
/// takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataShareChange {
    /// `AuthorizeDataShare`: the producer authorizes the consumer, with the
    /// writes it allows.
    Authorize {
        /// `AuthorizeDataShareMessage$AllowWrites`.
        allow_writes: Option<bool>,
    },
    /// `DeauthorizeDataShare`: the producer takes that authorization back.
    Deauthorize,
    /// `AssociateDataShareConsumer`: the consumer takes the datashare up, with
    /// the writes it accepted.
    Associate {
        /// `AssociateDataShareConsumerMessage$AllowWrites`.
        allow_writes: Option<bool>,
    },
    /// `DisassociateDataShareConsumer`: the consumer gives it up again.
    Disassociate,
    /// `RejectDataShare`: the consumer refuses it.
    Reject,
}

impl DataShareChange {
    /// The `DataShareAssociation$Status` this change leaves the association at,
    /// or `None` for the one change that REMOVES the association instead.
    ///
    /// Source: `smithy_query(query="shape", shape="DataShareStatus")` — the
    /// enum's values are `ACTIVE`, `AUTHORIZED`, `AVAILABLE`, `DEAUTHORIZED`,
    /// `PENDING_AUTHORIZATION` and `REJECTED`
    /// ([`crate::ops::types::DATA_SHARE_STATUS_VALUES`]).
    fn status(self) -> Option<&'static str> {
        match self {
            Self::Authorize { .. } => Some("AUTHORIZED"),
            Self::Deauthorize => Some("DEAUTHORIZED"),
            Self::Associate { .. } => Some("ACTIVE"),
            Self::Reject => Some("REJECTED"),
            Self::Disassociate => None,
        }
    }
}

/// What a consumer the `AssociateEntireAccount` arm names is recorded as.
///
/// `AssociateDataShareConsumerMessage$AssociateEntireAccount` is "a value that
/// specifies whether the datashare is associated with the entire account", so the
/// consumer is the ACCOUNT rather than one namespace of it — recorded under the
/// account's own id, which is the identifier a consumer account is named by
/// (`DataShareAssociation$ConsumerIdentifier` is "the name of the consumer
/// accounts that have an association with a producer datashare").
pub fn entire_account_consumer() -> String {
    ::engine::emulator_account_id().to_string()
}

/// Why a request that names neither the consumer namespace nor the whole account
/// is refused.
///
/// The two members are the two ways of naming the consumer an association is
/// with, so a request carrying neither names none.
pub const DATA_SHARE_CONSUMER_REQUIRED: &str =
    "Either ConsumerArn or AssociateEntireAccount must be specified to name the consumer the \
     datashare is associated with.";

/// Why a disassociation that names neither is refused. See
/// [`DATA_SHARE_CONSUMER_REQUIRED`].
pub const DATA_SHARE_DISASSOCIATE_CONSUMER_REQUIRED: &str =
    "Either ConsumerArn or DisassociateEntireAccount must be specified to name the consumer the \
     datashare is disassociated from.";

/// One stored datashare as the modelled `DataShare` the AUTHORIZE path answers
/// with.
///
/// The five writes and the three reads of this family all answer the SAME modelled
/// shape, which the generated types project once per operation — so each
/// projection is filled from the one stored record here, and no two operations can
/// report a datashare differently.
pub fn authorize_view(held: &StoredDataShare) -> crate::ops::types::AuthorizeDataShareOutput {
    crate::ops::types::AuthorizeDataShareOutput {
        data_share_arn: held.data_share_arn.clone(),
        producer_arn: held.producer_arn.clone(),
        allow_publicly_accessible_consumers: held.allow_publicly_accessible_consumers,
        data_share_associations: held.data_share_associations.clone(),
        managed_by: held.managed_by.clone(),
        data_share_type: held.data_share_type.clone(),
    }
}

/// One stored datashare as the DEAUTHORIZE path answers it. See
/// [`authorize_view`].
pub fn deauthorize_view(held: &StoredDataShare) -> crate::ops::types::DeauthorizeDataShareOutput {
    crate::ops::types::DeauthorizeDataShareOutput {
        data_share_arn: held.data_share_arn.clone(),
        producer_arn: held.producer_arn.clone(),
        allow_publicly_accessible_consumers: held.allow_publicly_accessible_consumers,
        data_share_associations: held.data_share_associations.clone(),
        managed_by: held.managed_by.clone(),
        data_share_type: held.data_share_type.clone(),
    }
}

/// One stored datashare as the DISASSOCIATE path answers it. See
/// [`authorize_view`].
pub fn disassociate_view(
    held: &StoredDataShare,
) -> crate::ops::types::DisassociateDataShareConsumerOutput {
    crate::ops::types::DisassociateDataShareConsumerOutput {
        data_share_arn: held.data_share_arn.clone(),
        producer_arn: held.producer_arn.clone(),
        allow_publicly_accessible_consumers: held.allow_publicly_accessible_consumers,
        data_share_associations: held.data_share_associations.clone(),
        managed_by: held.managed_by.clone(),
        data_share_type: held.data_share_type.clone(),
    }
}

/// One stored datashare as the REJECT path answers it. See [`authorize_view`].
pub fn reject_view(held: &StoredDataShare) -> crate::ops::types::RejectDataShareOutput {
    crate::ops::types::RejectDataShareOutput {
        data_share_arn: held.data_share_arn.clone(),
        producer_arn: held.producer_arn.clone(),
        allow_publicly_accessible_consumers: held.allow_publicly_accessible_consumers,
        data_share_associations: held.data_share_associations.clone(),
        managed_by: held.managed_by.clone(),
        data_share_type: held.data_share_type.clone(),
    }
}
