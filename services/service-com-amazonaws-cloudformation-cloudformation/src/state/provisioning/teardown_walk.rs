// SPDX-License-Identifier: CC0-1.0

//! The TEARDOWN WALK: asking the services that own a stack's resources to delete them, newest
//! first, and recording what became of each.
//!
//! One concern, in its own file so [`super`] stays inside the length the build holds every file
//! to: the pass itself ([`delete_pass`]), and the re-asked pass a ROLLBACK drives
//! ([`unwind`]) while an owning service has not let a resource go yet. What the walk is judged
//! by ([`Teardown`]), what it reports back ([`Deleted`]), what a refusal does
//! ([`OnRefusal`]) and the entry points that drive it ([`super::delete`]) live beside it in
//! [`super`], which also states the relation a refusal holds back ([`super::needed_by`]).

use super::*;

/// One pass of [`delete`] over the set, recording what it transitioned.
///
/// `opened` is the logical ids whose deletion has already been OPENED, which this pass adds
/// to: each resource's `DELETE_IN_PROGRESS` is recorded the first time a pass reaches it,
/// and a RE-ASK of one an owning service has not carried out yet ([`unwind`]) is the same
/// deletion still in progress rather than a second one. A resource an earlier pass never
/// reached — one after the refusal that stopped it — is opened by the pass that does reach
/// it.
///
/// `policies` is whether this walk reads each resource's own `DeletionPolicy` ([`Policies`]).
pub(super) async fn delete_pass(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    walk: &Teardown<'_>,
    opened: &mut BTreeSet<String>,
) -> Deleted {
    let Teardown {
        stack_id,
        held,
        needs,
        on_refusal,
        policies,
        left_alone,
    } = *walk;
    let mut outcome = Deleted {
        resources: resources.to_vec(),
        failure: None,
        transitions: Vec::new(),
        removing: Vec::new(),
        refused: Vec::new(),
    };
    // The resources an owning service REFUSED to delete, and everything those resources need
    // ([`needed_by`]): the refusals are what the teardown reports as its failure, and what
    // they need is what it leaves alone, because a resource that is still there is one
    // whatever it depends on is still in use by. The walk starts from what an earlier pass of
    // the same teardown was refused ([`Teardown::left_alone`]), which it leaves alone for
    // exactly that reason.
    let mut refused: Vec<String> = Vec::new();
    let mut still_needed: BTreeSet<String> = left_alone.clone();
    // Newest first and, where the relation says so, a resource AFTER everything that depends
    // on it ([`teardown_order`]) — over the indices of the set being reported back, so each
    // resource's record is updated in place as its deletion succeeds.
    for index in teardown_order(&outcome.resources, needs) {
        let resource = &outcome.resources[index];
        if still_needed.contains(&resource.logical_id) {
            // A resource that one the owning service would not delete still needs: deleting
            // it is not something that service could carry out while that resource is there,
            // so nothing is asked about it and the stack keeps its record — a later deletion
            // is what deletes it.
            continue;
        }
        // Which service is asked to delete the resource. `None` is a type this emulator
        // provisions nowhere: there is no API to call for it.
        let owner = owner_for(&resource.resource_type);
        if !resource.materialised {
            // Nothing exists in any owning service for this resource, so nothing is asked
            // of anybody. The stack still HELD the record, though, and its teardown is
            // reported exactly as every other resource's is — whether an owning service
            // had one to delete is not something a stack's event stream says, and the
            // record this disposal leaves says `DELETE_COMPLETE` either way
            // ([`crate::state::stack::disposed_resources`]), so a resource whose create FAILED
            // goes through the delete pair as well. Two records have no teardown at all:
            // one the stack does not hold (a declaration the operation never reached) and
            // one an earlier pass of this same walk already took down.
            if !held.contains(&resource.logical_id) || opened.contains(&resource.logical_id) {
                continue;
            }
        }
        let policy = policy_of(resource, policies);
        if policy == crate::state::deletion_policy::RETAIN {
            // "CloudFormation keeps the resource without deleting the resource or its
            // contents when its stack is deleted": nothing at all is asked of the owning
            // service, so the resource stays exactly as it is and goes on serving its own
            // clients, and the one thing the stack records about it is that its deletion was
            // SKIPPED — once, however many passes of one walk reach the record. The record
            // stops carrying `materialised`: the stack has RELEASED the resource, so no
            // later deletion of this stack's own owes a call about it.
            let first = opened.insert(resource.logical_id.clone());
            outcome.resources[index].materialised = false;
            if first {
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    crate::state::resource_status::DELETE_SKIPPED,
                    None,
                ));
            }
            continue;
        }
        // The deletion of this record is OPENED only where there is a resource to delete: a
        // record whose create never produced one in the service that owns the type has no
        // deletion in progress to report, so its disposal is reported by its completion
        // alone — which is what the rollback of a FAILED create records for the resource it
        // failed on. A type this emulator provisions nowhere is the stack's own record of a
        // resource, and goes through the pair exactly as a resource that exists does.
        if opened.insert(resource.logical_id.clone())
            && (resource.materialised || owner.is_none())
        {
            outcome.transitions.push(ResourceTransition::starting(
                resource,
                crate::state::resource_status::DELETE_IN_PROGRESS,
            ));
        }
        // "For resources that support snapshots, CloudFormation creates a snapshot for the
        // resource before deleting it": for a type whose owning API takes that snapshot as a
        // member of the delete it rides on that call
        // ([`crate::state::provisioner::Provisioner::delete`]), and for one whose snapshot is a CALL
        // OF ITS OWN it is made here, BEFORE the delete — so the snapshot outlives the
        // resource either way. A type whose owning service takes no snapshot on the way out
        // is deleted as it is for the default policy: there is no snapshot of it to take.
        let final_snapshot = (policy == crate::state::deletion_policy::SNAPSHOT && resource.materialised)
            .then(|| final_snapshot_identifier(resource));
        if let Some(identifier) = final_snapshot.as_deref() {
            let resource = &outcome.resources[index];
            // BOXED, for the reason this walk's other provider calls are: the call's future is
            // the owning service's whole handler, and holding one of those inline in this
            // walk's state machine is what takes a teardown of a wide stack past the thread's
            // stack.
            let snapshotted = Box::pin(crate::state::service_role::on_resource(
                crate::state::service_role::target_of(resource),
                take_final_snapshot(ctx, region, resource, identifier),
            ))
            .await;
            if let Err(refusal) = snapshotted {
                // The snapshot the policy asked for could not be taken, so the resource is
                // NOT deleted: deleting it would destroy the data the policy exists to
                // preserve. The refusal is reported exactly as a refused delete is.
                let reason = handler_failure_reason(
                    owner,
                    &refusal,
                    &outcome.resources[index].physical_resource_id,
                );
                if on_refusal == OnRefusal::Abandon {
                    // ABANDONED: the stack has RELEASED the resource, exactly as a `Retain`
                    // policy releases one, so the record stops carrying `materialised` —
                    // this teardown owes no further call about it and the stack's deletion
                    // completes with it left exactly as it is.
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        crate::state::resource_status::DELETE_SKIPPED,
                        Some(reason),
                    ));
                    outcome.resources[index].materialised = false;
                    continue;
                }
                refused.push(resource.logical_id.clone());
                still_needed.extend(needed_by(needs, &resource.logical_id));
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    crate::state::resource_status::DELETE_FAILED,
                    Some(reason),
                ));
                continue;
            }
        }
        let resource = &outcome.resources[index];
        // A CUSTOM RESOURCE is removed by its own PROVIDER: the `Delete` request carries the
        // physical resource the provider minted and the properties it was last declared
        // with, and the removal is not finished until the provider answers
        // ([`custom_resource`]). `None` for every other resource, whose removal is the
        // owning service's own delete.
        let provider_delete = (resource.materialised
            && custom_resource::is_custom_resource(&resource.resource_type))
            .then(custom_resource::new_request_id);
        // ...and the owning service is asked ONLY about a resource that really exists: a
        // record nothing created has no resource anywhere to delete, and the identifier it
        // carries may be another stack's (the name its create was refused for), which is
        // that stack's to keep.
        let asked = owner.filter(|_| resource.materialised);
        let deleted = match (&provider_delete, asked) {
            (Some(request_id), _) => {
                let call = custom_resource::ProviderCall {
                    request_type: custom_resource::request_type::DELETE,
                    request_id,
                    stack_id,
                    logical_resource_id: &resource.logical_id,
                    resource_type: &resource.resource_type,
                    properties: &resource.properties,
                    physical_resource_id: Some(&resource.physical_resource_id),
                    // A `Delete` names no previous declaration: the properties it carries
                    // ARE the ones the resource was last declared with.
                    old_properties: None,
                };
                // BOXED for the same reason the arm below is: the send dispatches the provider
                // service's whole handler, so its future is of that same size class.
                Box::pin(custom_resource::send(ctx, region, &call))
                    .await
                    .map(|response_url| Some(call.remembered(&response_url)))
            }
            (None, Some(provisioner)) => {
                // BOXED, for the reason the deployment walk's own calls are: the call's future
                // is the owning service's whole handler, and holding one of those inline in
                // this walk's state machine is what takes a teardown of a wide stack past the
                // thread's stack.
                Box::pin(crate::state::service_role::on_resource(
                    crate::state::service_role::target_of(resource),
                    provisioner.delete(ctx, region, resource, final_snapshot.as_deref()),
                ))
                .await
                .map(|()| None)
            }
            (None, None) => Ok(None),
        };
        match deleted {
            Ok(provider_request) => {
                // The delete was ACCEPTED; whether the resource is gone is the owning
                // service's to say. A type whose delete that service documents as
                // asynchronous leaves the resource in a state it is still being removed from
                // and answers which one ([`crate::state::provisioner::Provisioner::removal`]), and
                // the operation is then held open until that service no longer holds it
                // ([`crate::state::stabilising`]) — CloudFormation does not report an operation
                // complete while a resource it deleted is still there. An unanswered read is
                // no evidence that the resource is still there, so it opens no wait: the
                // delete itself went through.
                let removal = match (&provider_request, asked) {
                    // The provider has been SENT the `Delete`; the resource is gone when it
                    // answers, which is a wait like any other.
                    (Some(_), _) => Ok(Removal::Pending(
                        crate::state::resource_status::DELETE_IN_PROGRESS.to_string(),
                    )),
                    (None, Some(provisioner)) => {
                        // Boxed for the reason the delete's own call above is.
                        Box::pin(provisioner.removal(
                            ctx,
                            region,
                            &outcome.resources[index].physical_resource_id,
                        ))
                        .await
                    }
                    (None, None) => Ok(Removal::Gone),
                };
                if let Ok(Removal::Pending(state)) = removal {
                    let resource = &outcome.resources[index];
                    outcome.removing.push(Stabilising {
                        awaited: crate::state::stabilising::Awaited::Gone,
                        logical_id: resource.logical_id.clone(),
                        resource_type: resource.resource_type.clone(),
                        physical_resource_id: resource.physical_resource_id.clone(),
                        reported_state: state,
                        provider_request,
                    });
                    // ...and the resource is STILL THERE until that answer comes, so
                    // everything it depends on is left alone exactly as a refusal leaves it:
                    // "CloudFormation deletes the resources in reverse order", and a resource
                    // whose removal has only been ACCEPTED has not been removed — deleting
                    // what it depends on now would take away something still in use. The
                    // teardown asks about them once this wait releases
                    // ([`settle_outstanding_teardowns`]).
                    still_needed.extend(needed_by(needs, &resource.logical_id));
                }
                outcome.resources[index].materialised = false;
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    crate::state::resource_status::DELETE_COMPLETE,
                    None,
                ));
            }
            Err(refusal) => {
                let reason = handler_failure_reason(
                    owner_for(&outcome.resources[index].resource_type),
                    &refusal,
                    &outcome.resources[index].physical_resource_id,
                );
                if on_refusal == OnRefusal::Abandon {
                    // ABANDONED: nothing more is asked of the owning service, so the
                    // resource stays exactly as it is and goes on serving its own clients.
                    // The record reports that its deletion was SKIPPED, carrying why, and
                    // the walk carries on with the resources declared before it — the
                    // teardown has no failure of its own, so the stack's deletion completes.
                    // The stack has RELEASED the resource, exactly as a `Retain` policy
                    // releases one, so the record stops carrying `materialised`: this
                    // teardown owes no further call about it.
                    let resource = &outcome.resources[index];
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        crate::state::resource_status::DELETE_SKIPPED,
                        Some(reason),
                    ));
                    outcome.resources[index].materialised = false;
                    continue;
                }
                if on_refusal == OnRefusal::Orphan {
                    // ORPHANED: the refusal is reported on the resource and is not this
                    // teardown's failure — the change this cleanup follows is already in
                    // force, so there is nothing for the refusal to undo. The stack stops
                    // recording the resource, which is what leaves the old physical resource
                    // behind rather than held under a logical id the stack now records a
                    // different resource under.
                    outcome.resources[index].materialised = false;
                    let resource = &outcome.resources[index];
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        crate::state::resource_status::DELETE_FAILED,
                        Some(reason),
                    ));
                    continue;
                }
                // The refusal is this teardown's FAILURE, and the resource is reported as the
                // one it failed on. The walk goes on with the resources that do not depend on
                // it: CloudFormation deletes what it can and reports what it could not, so a
                // sibling the refused resource has nothing to do with is deleted rather than
                // left behind by the refusal of another.
                let resource = &outcome.resources[index];
                refused.push(resource.logical_id.clone());
                still_needed.extend(needed_by(needs, &resource.logical_id));
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    crate::state::resource_status::DELETE_FAILED,
                    Some(reason),
                ));
            }
        }
    }
    if !refused.is_empty() {
        outcome.failure = Some(crate::state::resource_failure_summary(
            crate::state::failed_action::DELETE,
            refused.iter().map(String::as_str),
        ));
    }
    outcome.refused = refused;
    outcome
}

/// The order one pass of [`delete_pass`] asks the owning services in: REVERSE DEPENDENCY
/// order, as indices into `resources`.
///
/// "CloudFormation deletes the resources in reverse order": a resource that depends on another
/// — through `DependsOn` or through an intrinsic reference to it — is deleted BEFORE the
/// resource it depends on, because the service that owns a resource refuses to delete it while
/// something of its own still references it (AWS AppSync refuses `DeleteDataSource` while a
/// resolver of the API still resolves against that data source). So the order is not the one
/// the records happen to be held in: it is the one the relation ([`Dependencies`]) states.
///
/// Newest first among resources with no dependency between them, which is the records' own
/// order reversed — so a set the relation says nothing about, and one whose records are already
/// held in creation order, is asked about exactly as before. A resource still needed by one
/// this pass has not reached yet is held back and asked about once that one has been; a
/// relation with a cycle in it (which no deployable template has) leaves the resources it holds
/// in that same newest-first order rather than dropping them.
fn teardown_order(resources: &[StoredResource], needs: &Dependencies) -> Vec<usize> {
    let newest_first = || (0..resources.len()).rev();
    if needs.is_empty() {
        return newest_first().collect();
    }
    // Who depends on each resource: the relation inverted, over the records being walked, so
    // the question "has everything that needs this one been deleted yet" costs one lookup.
    let mut dependents: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, resource) in resources.iter().enumerate() {
        for needed in needs.get(&resource.logical_id).into_iter().flatten() {
            dependents.entry(needed.as_str()).or_default().push(index);
        }
    }
    let mut pending = vec![true; resources.len()];
    let mut ordered: Vec<usize> = Vec::with_capacity(resources.len());
    // One pass per resource at most: each pass takes every resource nothing pending depends
    // on, newest first.
    for _ in 0..resources.len() {
        let taken = ordered.len();
        for index in newest_first() {
            if !pending[index] {
                continue;
            }
            let still_needed = dependents
                .get(resources[index].logical_id.as_str())
                .is_some_and(|dependents| {
                    dependents
                        .iter()
                        .any(|&dependent| dependent != index && pending[dependent])
                });
            if !still_needed {
                pending[index] = false;
                ordered.push(index);
            }
        }
        if ordered.len() == taken {
            break;
        }
    }
    // Anything a cycle held back, newest first.
    ordered.extend(newest_first().filter(|&index| pending[index]));
    ordered
}

/// How many times the UNWIND of a failed operation asks the owning services to delete
/// what that operation had already made.
///
/// A service that answers a create while the resource is still coming up refuses to
/// delete it until it is up (Amazon Data Firehose answers `DeleteDeliveryStream` for a
/// stream it is still creating with "cannot be deleted in CREATING state"), and
/// CloudFormation's rollback is what waits for that: `ROLLBACK_IN_PROGRESS` is a status a
/// stack sits in until the resources are gone. The wait is BOUNDED — these attempts, one
/// [`UNWIND_RETRY_DELAY`] apart — so a service that refuses the delete for any other
/// reason is reported rather than waited on forever.
///
/// The count is what makes the bound cover a resource that is merely coming up: together
/// with [`UNWIND_RETRY_DELAY`] it has to outlast the longest span a called service gives
/// one of those (a few seconds — Amazon Data Firehose gives a stream three), or a rollback
/// would report a resource it could have removed as one it could not.
pub(super) const UNWIND_ATTEMPTS: usize = 20;

/// How long the unwind waits before asking again. Chosen: shorter than the status spans
/// the services this emulator calls give a resource that is coming up, so the first
/// attempt after one of those elapses carries the delete through.
pub(super) const UNWIND_RETRY_DELAY: ::std::time::Duration = ::std::time::Duration::from_millis(200);

/// Delete what a FAILED create had already made, for the stack operation that is rolling
/// back — [`delete`], re-asked while an owning service has not let a resource go yet.
///
/// The resources still carrying `materialised` are the ones left to delete, which is what
/// each re-ask narrows to, so nothing is asked for twice. [`Deleted::failure`] is what the
/// LAST attempt answered: a refusal that outlives the bound above is the rollback's own
/// failure, reported on the stack rather than passed off as a rollback that removed what
/// it did not.
///
/// `held` is the set [`delete`] documents: the records the stack holds, which is what this
/// takes down and reports. A rollback is the stack giving up the resources the failed
/// operation had made, so what becomes of each PHYSICAL resource is its own
/// `DeletionPolicy` — unless the operation was asked for `RetainExceptOnCreate`, the one
/// member documented to override that policy for a resource the operation itself created
/// ([`crate::state::StoredStack::rolled_back_resource_policies`], which is what `policies` carries
/// here).
pub async fn unwind(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    walk: &Teardown<'_>,
) -> Deleted {
    // The deletions already opened, carried across the attempts: a re-ask of one still in
    // flight does not open it again, and one a stopped pass never reached is opened by the
    // pass that does reach it.
    let mut opened = BTreeSet::new();
    // An unwind is not a `DeleteStack`, so no `DeletionMode` chose for it: a refusal that
    // outlives the retries is the rollback's own failure.
    let mut outcome = delete_pass(ctx, region, resources, walk, &mut opened).await;
    for _ in 1..UNWIND_ATTEMPTS {
        if outcome.failure.is_none() {
            break;
        }
        ::tokio::time::sleep(UNWIND_RETRY_DELAY).await;
        let remaining = ::std::mem::take(&mut outcome.resources);
        // What the earlier attempts transitioned happened BEFORE what this one does. The
        // refusal this attempt is about to retry was not that resource's OUTCOME — its
        // deletion is still in progress, which is the whole point of the re-ask — so the
        // failed terminal the last pass recorded is dropped, and only the terminal of the
        // attempt nothing follows stands.
        let mut transitioned: Vec<ResourceTransition> =
            ::std::mem::take(&mut outcome.transitions)
                .into_iter()
                .filter(|transition| {
                    transition.status != crate::state::resource_status::DELETE_FAILED
                })
                .collect();
        // The waits the earlier attempts opened are the deletions they carried through, and
        // they stand: this attempt is about the resources those passes could NOT delete.
        let mut removing = ::std::mem::take(&mut outcome.removing);
        outcome = delete_pass(ctx, region, &remaining, walk, &mut opened).await;
        transitioned.append(&mut outcome.transitions);
        outcome.transitions = transitioned;
        removing.append(&mut outcome.removing);
        outcome.removing = removing;
    }
    // The last attempt's refusal is the rollback's own failure, which concludes the
    // operation: the removals these passes accepted are carried out before it does.
    settle_accepted_removals(ctx, region, &mut outcome).await;
    outcome
}
