// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the AWS Batch SHARING plane: the scheduling policy an
//! `AWS::Batch::SchedulingPolicy` declares and the quota share an `AWS::Batch::QuotaShare`
//! declares on a job queue, each created in AWS Batch itself. One file of [`super`]'s
//! per-type arms, dispatched to by its `Provisioner`, beside the compute environment, job
//! queue and service environment arms the same owning service has
//! ([`super::network_and_delivery`]) and the job-definition arms beside them
//! ([`super::batch_job_definitions`]).
//!
//! ## Why both are really created, and what the stack publishes for them
//!
//! A scheduling policy is what a job queue is SCHEDULED by: `CreateJobQueue`'s
//! `schedulingPolicyArn` is "the Amazon Resource Name (ARN) of the fair-share scheduling
//! policy", and that create resolves it against the policies AWS Batch holds — so a stack
//! that recorded a policy row without asking that service for one hands the queue beside it
//! an ARN nothing answers to, and the queue cannot be created at all. "Ref returns the
//! scheduling policy ARN", and `Fn::GetAtt Arn` reads that same ARN, which is the handle
//! `DescribeSchedulingPolicies`, `UpdateSchedulingPolicy` and a `batch:` policy statement
//! take.
//!
//! A quota share is created ON a queue — `CreateQuotaShare` takes the `jobQueue` the share
//! allocates capacity in, and the ARN that service mints for the share nests that queue
//! (`…:job-queue/<queue>/quota-share/<share>`), which is the only identifier
//! `DescribeQuotaShare`, `UpdateQuotaShare` and `DeleteQuotaShare` take. So the share's
//! identity is that ARN, which is what the type's `Ref` and its `QuotaShareArn` attribute
//! publish, and the identity in force is the one the create answered.
//!
//! ## The properties
//!
//! Each type's properties are its create's own members spelled with an upper-case initial,
//! all the way down, with two exceptions this module carries: the resource's own NAME
//! addresses the create rather than travelling beside the others — it is the name
//! CloudFormation decided, read back off the identifier that is the resource's physical id —
//! and `Tags` is rebuilt, so the resource carries the tags CloudFormation gives it beside
//! the declared ones, as the tag MAP each create takes.
//!
//! A declared property these arms do not carry is REFUSED rather than dropped
//! ([`refuse_unmapped_properties`]): a resource created with less than its declaration asked
//! for is the shallow success these arms exist to avoid.

use super::*;

/// The URIs the calls these two types need are bound to, from the AWS Batch model's own
/// `@http` traits (`smithy_query(query="operation", operation="CreateSchedulingPolicy,\
/// DeleteSchedulingPolicy,DescribeSchedulingPolicies,CreateQuotaShare,UpdateQuotaShare,\
/// DeleteQuotaShare,ListQuotaShares")`: each is a `POST`).
const BATCH_CREATE_SCHEDULING_POLICY_URI: &str = "/v1/createschedulingpolicy";
const BATCH_DELETE_SCHEDULING_POLICY_URI: &str = "/v1/deleteschedulingpolicy";
const BATCH_DESCRIBE_SCHEDULING_POLICIES_URI: &str = "/v1/describeschedulingpolicies";
const BATCH_CREATE_QUOTA_SHARE_URI: &str = "/v1/createquotashare";
const BATCH_UPDATE_QUOTA_SHARE_URI: &str = "/v1/updatequotashare";
const BATCH_DELETE_QUOTA_SHARE_URI: &str = "/v1/deletequotashare";
const BATCH_LIST_QUOTA_SHARES_URI: &str = "/v1/listquotashares";

/// The members `CreateSchedulingPolicy` and `DescribeSchedulingPolicies` name a policy's own
/// name and ARN by, and the ATTRIBUTE `AWS::Batch::SchedulingPolicy` publishes that ARN
/// through `Fn::GetAtt`.
const SCHEDULING_POLICY_NAME_MEMBER: &str = "name";
const SCHEDULING_POLICY_ARN_MEMBER: &str = "arn";
pub(crate) const SCHEDULING_POLICY_ARN_ATTRIBUTE: &str = "Arn";

/// The member `DescribeSchedulingPolicies` takes its ARNs in and the one it answers its
/// records under, which the model spells differently on the way in and out.
const SCHEDULING_POLICY_ARNS_MEMBER: &str = "arns";
const SCHEDULING_POLICIES_MEMBER: &str = "schedulingPolicies";

/// The members a quota-share call names the share and its queue by, and the ATTRIBUTE
/// `AWS::Batch::QuotaShare` publishes the share's ARN through `Fn::GetAtt`.
const QUOTA_SHARE_NAME_MEMBER: &str = "quotaShareName";
const QUOTA_SHARE_ARN_MEMBER: &str = "quotaShareArn";
const QUOTA_SHARES_MEMBER: &str = "quotaShares";
const JOB_QUEUE_MEMBER: &str = "jobQueue";
pub(crate) const QUOTA_SHARE_ARN_ATTRIBUTE: &str = "QuotaShareArn";

/// The property each type takes its own NAME from, which addresses the create rather than
/// travelling beside the other members, and the one carrying the resource's tags, which is
/// rebuilt rather than carried.
const SCHEDULING_POLICY_NAME_PROPERTY: &str = "Name";
const QUOTA_SHARE_NAME_PROPERTY: &str = "QuotaShareName";
const TAGS_PROPERTY: &str = "Tags";

/// Every property `AWS::Batch::SchedulingPolicy` publishes.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `CreateSchedulingPolicy`'s own members (`smithy_query(query="operation",
/// operation="CreateSchedulingPolicy")`), which are these with a lower-case initial.
const SCHEDULING_POLICY_PROPERTIES: &[&str] = &[
    "FairsharePolicy",
    SCHEDULING_POLICY_NAME_PROPERTY,
    "QuotaSharePolicy",
    TAGS_PROPERTY,
];

/// The two policy properties whose value is a modelled STRUCTURE, carried under the member
/// it is with every member name inside it re-cased the same way ([`camel_cased`]) — which is
/// the whole difference between the type's spelling and the API's.
const SCHEDULING_POLICY_STRUCTURES: &[(&str, &str)] = &[
    ("FairsharePolicy", "fairsharePolicy"),
    ("QuotaSharePolicy", "quotaSharePolicy"),
];

/// Every property `AWS::Batch::QuotaShare` publishes.
///
/// Source: the same pairing of the type's resource reference with `CreateQuotaShare`'s own
/// members (`smithy_query(query="operation", operation="CreateQuotaShare")`:
/// `quotaShareName`, `jobQueue`, `capacityLimits`, `resourceSharingConfiguration`,
/// `preemptionConfiguration`, `state` and `tags`).
const QUOTA_SHARE_PROPERTIES: &[&str] = &[
    "CapacityLimits",
    "JobQueue",
    "PreemptionConfiguration",
    QUOTA_SHARE_NAME_PROPERTY,
    "ResourceSharingConfiguration",
    "State",
    TAGS_PROPERTY,
];

/// The share properties whose value goes through unchanged under its lower-camel member
/// name: the queue the share is created on — which the declaration states as a reference to
/// that queue, so what reaches AWS Batch is the ARN the queue really carries — and the state
/// the share is created in.
const QUOTA_SHARE_MEMBERS: &[(&str, &str)] = &[("JobQueue", JOB_QUEUE_MEMBER), ("State", "state")];

/// The share properties whose value is a modelled STRUCTURE (or a LIST of them), carried the
/// same way the policy's structures are.
const QUOTA_SHARE_STRUCTURES: &[(&str, &str)] = &[
    ("CapacityLimits", "capacityLimits"),
    ("PreemptionConfiguration", "preemptionConfiguration"),
    (
        "ResourceSharingConfiguration",
        "resourceSharingConfiguration",
    ),
];

/// The segment a quota-share ARN names the share after, inside the ARN of the job queue it
/// is created on (`arn:<partition>:batch:<region>:<account>:job-queue/<queue>/quota-share/<share>`).
const QUOTA_SHARE_ARN_SEGMENT: &str = "/quota-share/";

/// How many pages of a queue's quota shares to walk while looking for one share.
///
/// `ListQuotaShares` is paginated ("returns up to 100 results and a `nextToken`"), and the
/// answer to "does this share still exist" is only final once the pages are exhausted — so
/// the walk follows the token rather than reading the first page, bounded by this constant
/// because the pages come from another service and a caller's question may not depend on
/// that service answering a terminating token.
const BATCH_QUOTA_SHARE_PAGES: usize = 20;

/// The `QuotaShareStatus` of a share AWS Batch has finished creating, and the one it reports
/// for a configuration it could not accept (`smithy_query(query="shape",
/// shape="QuotaShareStatus")`: `CREATING | VALID | INVALID | UPDATING | DELETING`).
const QUOTA_SHARE_STATUS_VALID: &str = "VALID";
const QUOTA_SHARE_STATUS_INVALID: &str = "INVALID";

/// Create the scheduling policy an `AWS::Batch::SchedulingPolicy` declares, in AWS Batch
/// itself — so the ARN the stack publishes is one `batch describe-scheduling-policies`
/// resolves, and one the `SchedulingPolicyArn` of a job queue in the same stack can be
/// created against.
///
/// `name` is the name CloudFormation decided — the declared `Name`, or the generated one —
/// read back off the ARN that is this resource's physical id, exactly as the sibling Batch
/// arms read theirs.
///
/// `Err` is that service's own refusal of the policy — two policy kinds at once, a
/// `computeReservation` outside its documented range — which is what the resource fails
/// with, because a create that did not answer 2xx created nothing.
pub(super) async fn create_batch_scheduling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, SCHEDULING_POLICY_PROPERTIES)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        SCHEDULING_POLICY_NAME_MEMBER.to_string(),
        Value::String(batch_resource_name(&resource.physical_resource_id).to_string()),
    );
    carry_structures(&properties, SCHEDULING_POLICY_STRUCTURES, &mut request)?;
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_CREATE_SCHEDULING_POLICY_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered_identifier(&answered, SCHEDULING_POLICY_ARN_MEMBER, resource)?;
    let mut attributes = BTreeMap::new();
    attributes.insert(SCHEDULING_POLICY_ARN_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the scheduling policy a stack created, addressed by the ARN AWS Batch knows it by
/// — this resource's physical id, which is what `Ref` returns for the type.
///
/// A policy that service no longer holds is already in the state this asks for, and its own
/// delete answers that as the success it is, so there is nothing to check first. A policy a
/// job queue still names is that service's refusal to give, and the stack reports it: the
/// stack takes its resources down newest first, so the queue that named this policy has
/// already gone by the time this runs.
pub(super) async fn delete_batch_scheduling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DELETE_SCHEDULING_POLICY_URI,
        Some(&serde_json::json!({ SCHEDULING_POLICY_ARN_MEMBER: physical_resource_id })),
    )
    .await
    .map(|_answered| ())
}

/// The policy as AWS Batch reports it, read by the ARN that is this resource's physical id:
/// the one record `DescribeSchedulingPolicies` answers, or `None` where that service holds
/// none.
///
/// That reader takes a LIST of ARNs and answers the records it found, so a policy that
/// service no longer holds comes back as an empty list rather than as a refusal — the same
/// reading every other AWS Batch type's drift read is (`batch_described`), which cannot be
/// used here only because this one call spells its request member (`arns`) and its answer
/// member (`schedulingPolicies`) differently.
pub(super) async fn describe_batch_scheduling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DESCRIBE_SCHEDULING_POLICIES_URI,
        Some(&serde_json::json!({
            SCHEDULING_POLICY_ARNS_MEMBER: [physical_resource_id],
        })),
    )
    .await?;
    Ok(answered
        .get(SCHEDULING_POLICIES_MEMBER)
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .cloned())
}

/// Create the quota share an `AWS::Batch::QuotaShare` declares, on the job queue its
/// declaration names — so the share the stack publishes is one `batch describe-quota-share`
/// resolves and one a job submitted into that queue can be placed in.
///
/// What this answers as the resource's identity is the ARN AWS Batch reported: that service
/// composes it UNDER the queue, so the ARN CloudFormation's naming rules computed is the
/// stand-in a declaration is resolved against until the share really exists.
///
/// A fresh share is `CREATING` — a status in which that service has not finished making it —
/// so the resource is not created until it is `VALID`, which is what a client that reads the
/// stack's output and then submits into the queue depends on.
pub(super) async fn create_batch_quota_share(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, QUOTA_SHARE_PROPERTIES)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        QUOTA_SHARE_NAME_MEMBER.to_string(),
        Value::String(batch_resource_name(&resource.physical_resource_id).to_string()),
    );
    copy_members(&properties, QUOTA_SHARE_MEMBERS, &mut request);
    carry_structures(&properties, QUOTA_SHARE_STRUCTURES, &mut request)?;
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_CREATE_QUOTA_SHARE_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered_identifier(&answered, QUOTA_SHARE_ARN_MEMBER, resource)?;
    let mut attributes = BTreeMap::new();
    attributes.insert(QUOTA_SHARE_ARN_ATTRIBUTE.to_string(), arn.clone());
    quota_share_await_valid(ctx, region, &arn).await?;
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the quota share a stack created, addressed by the ARN AWS Batch knows it by —
/// this resource's physical id.
///
/// AWS Batch takes a share out of service in two steps, and a client has to make both:
/// "you must first disable submissions for the share by updating the state to DISABLED",
/// then `DeleteQuotaShare`. A share that service no longer holds is already in the state
/// this asks for, so it is read first: the update, unlike the delete, reports a share that
/// is not there as nonexistent, and a stack deleting what it already deleted is not a
/// failure.
pub(super) async fn delete_batch_quota_share(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    if quota_share_listed(ctx, region, physical_resource_id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_QUOTA_SHARE_URI,
        Some(&serde_json::json!({
            QUOTA_SHARE_ARN_MEMBER: physical_resource_id,
            "state": BATCH_STATE_DISABLED,
        })),
    )
    .await?;
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DELETE_QUOTA_SHARE_URI,
        Some(&serde_json::json!({ QUOTA_SHARE_ARN_MEMBER: physical_resource_id })),
    )
    .await
    .map(|_answered| ())
}

/// The share as AWS Batch reports it, read by the ARN that is this resource's physical id —
/// the record `ListQuotaShares` carries for it, or `None` where the queue it was created on
/// holds none ([`quota_share_listed`]).
pub(super) async fn describe_batch_quota_share(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    quota_share_listed(ctx, region, physical_resource_id).await
}

/// Wait, bounded, until AWS Batch reports the share `arn` as `VALID`.
///
/// `CreateQuotaShare` answers the share in the transient `CREATING` status, and
/// CloudFormation reports a resource CREATE_COMPLETE only once its service has really
/// created it. An `INVALID` status is that service's own verdict on the configuration the
/// template declared, and is reported as the failure it is rather than waited out — the same
/// reading the sibling Batch arms' wait makes ([`batch_await_valid`]).
async fn quota_share_await_valid(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Result<(), Refusal> {
    for _ in 0..BATCH_DELETION_CHECKS {
        let status = quota_share_listed(ctx, region, arn)
            .await?
            .as_ref()
            .and_then(|record| record.get("status"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match status.as_deref() {
            Some(QUOTA_SHARE_STATUS_VALID) => return Ok(()),
            Some(QUOTA_SHARE_STATUS_INVALID) => {
                return Err(Refusal {
                    code: "ClientException".to_string(),
                    message: format!("{arn} is {QUOTA_SHARE_STATUS_INVALID}"),
                })
            }
            _ => ::tokio::time::sleep(BATCH_DELETION_CHECK_INTERVAL).await,
        }
    }
    Err(Refusal {
        code: "ClientException".to_string(),
        message: format!("{arn} did not become {QUOTA_SHARE_STATUS_VALID}"),
    })
}

/// The `QuotaShareDetail` AWS Batch holds for the share `arn` names, or `None` where the
/// queue that ARN nests holds no such share.
///
/// `DescribeQuotaShare` is not the reader here: it REFUSES an ARN it holds no share for, so
/// a caller could tell "no longer there" from any other refusal only by reading a message.
/// `ListQuotaShares` answers a LIST for the queue instead — "a reference that resolves to no
/// job queue" included — so a share that is gone is simply absent from it, which is the same
/// reading every other AWS Batch type's read makes ([`batch_described`]).
///
/// `None` is therefore "that service holds no such share", which is only known once the pages
/// are exhausted: a walk that reaches [`BATCH_QUOTA_SHARE_PAGES`] with a page still to come
/// has not learnt that, and answers the refusal it is rather than an absence its callers would
/// read as a share already gone.
async fn quota_share_listed(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Result<Option<Value>, Refusal> {
    let mut token: Option<String> = None;
    for _ in 0..BATCH_QUOTA_SHARE_PAGES {
        let mut request = Map::new();
        request.insert(
            JOB_QUEUE_MEMBER.to_string(),
            Value::String(job_queue_of(arn).to_string()),
        );
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), Value::String(token.clone()));
        }
        let answered = call_rest_json(
            ctx,
            BATCH_SIGNING_NAME,
            region,
            "POST",
            BATCH_LIST_QUOTA_SHARES_URI,
            Some(&Value::Object(request)),
        )
        .await?;
        let found = answered
            .get(QUOTA_SHARES_MEMBER)
            .and_then(Value::as_array)
            .and_then(|shares| {
                shares
                    .iter()
                    .find(|share| {
                        share.get(QUOTA_SHARE_ARN_MEMBER).and_then(Value::as_str) == Some(arn)
                    })
                    .cloned()
            });
        if found.is_some() {
            return Ok(found);
        }
        token = answered
            .get("nextToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(ToString::to_string);
        if token.is_none() {
            return Ok(None);
        }
    }
    Err(Refusal {
        code: "ClientException".to_string(),
        message: format!(
            "AWS Batch reported more than {BATCH_QUOTA_SHARE_PAGES} pages of quota shares for \
             {}, so whether it still holds {arn} is not known",
            job_queue_of(arn),
        ),
    })
}

/// The ARN of the job queue a quota-share ARN nests the share in: everything before the
/// share's own segment ([`QUOTA_SHARE_ARN_SEGMENT`]), which is that queue's own ARN — and
/// the whole value where it carries no such segment, which AWS Batch then judges.
fn job_queue_of(arn: &str) -> &str {
    match arn.split_once(QUOTA_SHARE_ARN_SEGMENT) {
        Some((queue, _share)) => queue,
        None => arn,
    }
}

/// The declared properties of `structures` as their create takes them: each under the member
/// it is, with every member name INSIDE it spelled as the API member it is
/// ([`camel_cased`]). A property the template leaves out is not sent.
///
/// The ONE mapping of a Batch structure-valued property, shared by the create of a scheduling
/// policy and a quota share here with the change applied to a policy that already exists
/// ([`super::batch_in_place`]).
pub(super) fn carry_structures(
    properties: &Map<String, Value>,
    structures: &[(&str, &str)],
    request: &mut Map<String, Value>,
) -> Result<(), Refusal> {
    for (property, member) in structures {
        let Some(declared) = properties.get(*property).filter(|value| !value.is_null()) else {
            continue;
        };
        request.insert((*member).to_string(), camel_cased(declared, 0)?);
    }
    Ok(())
}

/// The identifier AWS Batch answered a create with, under the member that create reports it
/// as.
///
/// A create that identified nothing is no created resource: the resource FAILS rather than
/// the stack publishing an identifier no call of that service takes — the same reading the
/// job-definition arm makes of its own answer.
fn answered_identifier(
    answered: &Value,
    member: &str,
    resource: &StoredResource,
) -> Result<String, Refusal> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|identifier| !identifier.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: format!(
                "AWS Batch reported no {member} for the {} it created",
                resource.resource_type
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One declared resource of `resource_type`, for the tables below.
    fn declared(resource_type: &str, physical: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Subject".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: physical.to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// The declared properties of either type reach the create under that call's own member
    /// names, all the way down — and the name the create is addressed by is the one the
    /// resource's identifier carries, whether the template declared it or CloudFormation
    /// generated it.
    #[test]
    fn the_declared_properties_are_carried_under_the_api_member_names() {
        for (case, properties, structures, expected) in [
            (
                "a fair-share policy",
                serde_json::json!({
                    "FairsharePolicy": {
                        "ShareDecaySeconds": 600,
                        "ComputeReservation": 25,
                        "ShareDistribution": [
                            { "ShareIdentifier": "teamA*", "WeightFactor": 0.5 }
                        ]
                    }
                }),
                SCHEDULING_POLICY_STRUCTURES,
                serde_json::json!({
                    "fairsharePolicy": {
                        "shareDecaySeconds": 600,
                        "computeReservation": 25,
                        "shareDistribution": [
                            { "shareIdentifier": "teamA*", "weightFactor": 0.5 }
                        ]
                    }
                }),
            ),
            (
                "a quota-share policy",
                serde_json::json!({
                    "QuotaSharePolicy": { "IdleResourceAssignmentStrategy": "FIFO" }
                }),
                SCHEDULING_POLICY_STRUCTURES,
                serde_json::json!({
                    "quotaSharePolicy": { "idleResourceAssignmentStrategy": "FIFO" }
                }),
            ),
            (
                "a share's capacity, sharing and preemption",
                serde_json::json!({
                    "CapacityLimits": [
                        { "MaxCapacity": 1, "CapacityUnit": "ml.m6i.large" }
                    ],
                    "ResourceSharingConfiguration": {
                        "Strategy": "LEND_AND_BORROW",
                        "BorrowLimit": 200
                    },
                    "PreemptionConfiguration": { "InSharePreemption": "DISABLED" }
                }),
                QUOTA_SHARE_STRUCTURES,
                serde_json::json!({
                    "capacityLimits": [
                        { "maxCapacity": 1, "capacityUnit": "ml.m6i.large" }
                    ],
                    "resourceSharingConfiguration": {
                        "strategy": "LEND_AND_BORROW",
                        "borrowLimit": 200
                    },
                    "preemptionConfiguration": { "inSharePreemption": "DISABLED" }
                }),
            ),
        ] {
            let mut request = Map::new();
            let declared = properties.as_object().cloned().unwrap_or_else(Map::new);
            carry_structures(&declared, structures, &mut request)
                .unwrap_or_else(|refusal| panic!("case {case:?}: the walk converts it: {refusal:?}"));
            let carried = Value::Object(request);
            assert_eq!(
                carried, expected,
                "case {case:?}: expected {expected:?}, got {carried:?}"
            );
        }
    }

    /// The name each create is addressed by is the last segment of the identifier the
    /// resource carries, which is the name CloudFormation decided — and the queue a share's
    /// read is addressed by is the queue ARN its own ARN nests.
    #[test]
    fn a_resource_is_addressed_by_the_identifier_it_carries() {
        let policy = declared(
            "AWS::Batch::SchedulingPolicy",
            "arn:aws:batch:us-east-1:000000000000:scheduling-policy/fifo-sp",
            serde_json::json!({}),
        );
        let name = batch_resource_name(&policy.physical_resource_id);
        assert_eq!(
            name, "fifo-sp",
            "case \"a policy ARN\": expected the policy name, got {name:?}"
        );
        for (case, arn, queue) in [
            (
                "a share ARN",
                "arn:aws:batch:us-east-1:000000000000:job-queue/sm-jq/quota-share/sm-qs",
                "arn:aws:batch:us-east-1:000000000000:job-queue/sm-jq",
            ),
            (
                "an identifier that is no share ARN",
                "sm-qs",
                "sm-qs",
            ),
        ] {
            let resolved = job_queue_of(arn);
            assert_eq!(
                resolved, queue,
                "case {case:?}: expected the queue {queue:?}, got {resolved:?}"
            );
            if case == "a share ARN" {
                let share = batch_resource_name(arn);
                assert_eq!(
                    share, "sm-qs",
                    "case {case:?}: expected the share name, got {share:?}"
                );
            }
        }
    }

    /// A create whose answer carries no identifier is no created resource: the arm refuses
    /// rather than publishing an empty handle.
    #[test]
    fn a_create_that_identified_nothing_is_refused() {
        let resource = declared(
            "AWS::Batch::QuotaShare",
            "arn:aws:batch:us-east-1:000000000000:job-queue/sm-jq/quota-share/sm-qs",
            serde_json::json!({}),
        );
        for (case, answered) in [
            ("no member at all", serde_json::json!({})),
            (
                "an empty identifier",
                serde_json::json!({ QUOTA_SHARE_ARN_MEMBER: "" }),
            ),
        ] {
            let answer = answered_identifier(&answered, QUOTA_SHARE_ARN_MEMBER, &resource);
            assert!(
                answer.is_err(),
                "case {case:?}: expected a refusal, got {answer:?}"
            );
        }
        let answered = serde_json::json!({ QUOTA_SHARE_ARN_MEMBER: "arn:aws:batch:x" });
        let answer = answered_identifier(&answered, QUOTA_SHARE_ARN_MEMBER, &resource);
        assert_eq!(
            answer.as_deref().ok(),
            Some("arn:aws:batch:x"),
            "case \"an answered identifier\": expected it carried through, got {answer:?}"
        );
    }
}
