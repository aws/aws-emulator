// SPDX-License-Identifier: CC0-1.0

//! The DB PARAMETER GROUP an `AWS::RDS::DBInstance` declaration puts a DB instance on, APPLIED
//! so the engine is really running under it by the time the stack reports the resource
//! deployed. One file of [`super`]'s per-type arms, reached by the readiness arm of that type
//! ([`super::application_services::databases_and_networks::db_instance_readiness`]).
//!
//! Amazon RDS does not apply a parameter-group ASSOCIATION on its own:
//! `ModifyDBInstance$DBParameterGroupName` states that "the parameter group name itself is
//! changed immediately, but the actual parameter changes are not applied until you reboot the
//! instance without failover. In this case, the DB instance isn't rebooted automatically, and
//! the parameter changes aren't applied during the next maintenance window", and the
//! membership's `DBParameterGroupStatus$ParameterApplyStatus` reports exactly that —
//! `pending-reboot`, "the parameter group change will be applied after the DB instance
//! reboots", against `in-sync`, "the parameter group change is synchronized with the
//! database".
//!
//! So the reboot is the deployment's to make: `AWS::RDS::DBInstance`'s own
//! `DBParameterGroupName` documentation says that if the referenced parameter group's data
//! members change during an update the DB instance might need to be restarted, which causes
//! some interruption, and that a group carrying static parameters — changed or not — makes an
//! update trigger a reboot. CloudFormation therefore restarts the instance it deployed while a
//! declared association is still waiting for the reboot that applies it, and holds the
//! resource's work open until Amazon RDS reports the membership synchronized. A stack that
//! reported the resource deployed with the association still unapplied would say the database
//! runs under the parameters its template declares while the engine runs under the previous
//! group's.
//!
//! The memberships in force are read back off that service on every pass, so the arm
//! converges: a readiness answer is asked again on each pass of the stabilisation walk, and an
//! instance whose group is already synchronized is left alone rather than restarted a second
//! time.

use super::*;

/// The property the memberships are reported under, and the element one of them is reported
/// as: `DBInstance$DBParameterGroups` is a list of `DBParameterGroupStatus`, whose entries the
/// awsQuery answer writes as `<DBParameterGroup>` (`smithy_query(query="shape",
/// shape="DBParameterGroupStatusList")`).
const MEMBERSHIP_ELEMENT: &str = "DBParameterGroup";

/// `DBParameterGroupStatus$ParameterApplyStatus` of a membership whose values the instance's
/// engine is RUNNING: "the parameter group change is synchronized with the database".
const IN_SYNC: &str = "in-sync";

/// The status of a membership that is attached and waiting for the reboot that applies it:
/// "the parameter group change will be applied after the DB instance reboots". This is the one
/// outstanding state a caller has to act on — every other one Amazon RDS reaches on its own.
const PENDING_REBOOT: &str = "pending-reboot";

/// The status of a membership whose change Amazon RDS could NOT apply, which is where the
/// association rests rather than a state it leaves on its own: nothing a deployment does brings
/// such a group into force, so the resource fails on it rather than waiting its window out.
const FAILED_TO_APPLY: &str = "failed-to-apply";

/// Put the DB instance's declared parameter group IN FORCE, restarting the instance where a
/// membership is waiting for the reboot that applies it.
///
/// `Ok(None)` is every membership synchronized, which is the state the resource's work is
/// finished in — including an instance whose group was in force all along, for which no call is
/// made at all. `Ok(Some(readiness))` is a membership that is NOT in force: the state the
/// instance is in once this pass has done what it can — the status the reboot answered where one
/// was asked for, and the membership's own outstanding status where Amazon RDS is still applying
/// it, which is a change that service completes on its own — or the resource's FAILURE where
/// that service reports a change it could not apply, which no restart of this arm's would bring
/// into force.
///
/// `Err` is the refusal of the read this arm takes first, or of the reboot it asks for, which
/// says nothing about the resource: the caller decides what an unanswered call means, as it does
/// for every other readiness answer.
pub(crate) async fn apply_declared_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Readiness>, Refusal> {
    let Some((group, outstanding)) =
        outstanding_membership(ctx, region, physical_resource_id).await?
    else {
        return Ok(None);
    };
    if outstanding == FAILED_TO_APPLY {
        // "The parameter group change failed to be applied": the association is where Amazon
        // RDS left it, so the resource's work is finished — as a failure, in that service's own
        // account of the membership.
        return Ok(Some(Readiness::Failed(format!(
            "Amazon RDS reports the DB parameter group {group} of the DB instance \
             {physical_resource_id} in status {outstanding}"
        ))));
    }
    if outstanding != PENDING_REBOOT {
        // Amazon RDS is still attaching the group (`applying`), or the change waits on work of
        // that service's own: nothing is asked of it, and the next pass reads the membership
        // again.
        return Ok(Some(Readiness::Pending(outstanding)));
    }
    // "Rebooting a DB instance restarts the database engine service", which is what puts the
    // newly associated group's values in force; the instance goes out of service for the
    // momentary outage it causes, and the status that call answers is the state this pass
    // leaves the resource in.
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "RebootDBInstance",
        &[(
            "DBInstanceIdentifier".to_string(),
            physical_resource_id.to_string(),
        )],
    )
    .await?;
    Ok(Some(Readiness::Pending(
        xml_element(&answered, "DBInstanceStatus")
            .filter(|status| !status.is_empty())
            .unwrap_or(outstanding),
    )))
}

/// The `(DBParameterGroupName, ParameterApplyStatus)` of the first membership this instance
/// reports that is NOT synchronized, or `None` when every one of them is.
///
/// An instance Amazon RDS no longer holds reports no membership: the resource is answered for
/// by the readiness arm's own read of that service, so a delete that landed in between leaves
/// nothing outstanding here rather than a reboot of an instance that is gone.
async fn outstanding_membership(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<(String, String)>, Refusal> {
    let answered = match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBInstances",
        &[(
            "DBInstanceIdentifier".to_string(),
            physical_resource_id.to_string(),
        )],
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == DB_INSTANCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    Ok(xml_scalar_children_of_each(&answered, MEMBERSHIP_ELEMENT)
        .into_iter()
        .filter_map(|membership| {
            let status = membership
                .get("ParameterApplyStatus")
                .and_then(scalar_text)
                .filter(|status| !status.is_empty())?;
            let group = membership
                .get("DBParameterGroupName")
                .and_then(scalar_text)
                .unwrap_or_default();
            Some((group, status))
        })
        .find(|(_, status)| status != IN_SYNC))
}
