// SPDX-License-Identifier: CC0-1.0

//! AWS's own catalog of managed policies — the `arn:aws:iam::aws:policy/` namespace
//! every account reads and attaches from, and the resolution of an ARN in it.
//!
//! A policy AWS owns is not account state (nothing creates or deletes one), which is
//! why the catalog is a constant here rather than a map in
//! [`ComAmazonawsIamAwsidentitymanagementv20100508State`].

use super::*;

/// One entry of AWS's own catalog of managed policies — a policy AWS owns and
/// every account can read and attach, addressed under the
/// `arn:aws:iam::aws:policy/` namespace ([`AWS_MANAGED_POLICY_ACCOUNT`]).
///
/// An AWS-managed policy is not account state: no call creates or deletes one, so
/// an entry carries no counter and no minted id. Everything a reader reports about
/// one is this entry's OWN data — its identity (the path and friendly name its ARN
/// spells, [`Self::arn`]), its id ([`Self::policy_id`]), the version number it is
/// currently published at ([`Self::default_version_id`]), the two instants of its
/// publication history ([`Self::published`], [`Self::revised`]), whether it may be
/// attached ([`Self::is_attachable`]) and its document — rather than a value a
/// template stamps on every entry alike. Two entries therefore differ in their id,
/// their version and their instants, as two published policies do.
///
/// The content carried for a policy is the ONE version a reader can serve: the
/// version [`Self::default_version_id`] names, which is what
/// `GetPolicy`'s `DefaultVersionId` reports and `GetPolicyVersion` serves. AWS's
/// friendly `Description` prose is not carried — the catalog holds a policy's
/// identity and document, so `Description` is reported only for a policy this
/// ACCOUNT created with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwsManagedPolicy {
    /// The policy's path, beginning and ending with `/`: `/` for a policy at the
    /// root, `/service-role/` for one AWS files under that path.
    pub path: &'static str,
    /// The policy's friendly name, as the AWS Managed Policy Reference spells it.
    pub policy_name: &'static str,
    /// This entry's `PolicyId`, carried as catalog data beside the path, name and
    /// document.
    ///
    /// Stable and unique, the two properties `Policy$PolicyId` documents ("the stable
    /// and unique string identifying the policy"): it is fixed per entry, so every
    /// read in every run reports the same id for the same policy, and the suite
    /// asserts the catalog's ids are distinct. AWS spells a policy's unique id as the
    /// `ANPA` prefix followed by 17 further characters, which is the form carried here
    /// and is of the modelled `idType` (len 16..128, `^[\w]+$`); it belongs to the
    /// policy rather than being read off its name, so renaming nothing about an entry
    /// but its id changes which policy a client correlating by id is looking at. An id
    /// the account's own `CreatePolicy` would mint onto a catalog id is passed over by
    /// the minter
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::next_policy_id`]), so one
    /// id never names two policies.
    pub policy_id: &'static str,
    /// The version currently set as this policy's default — `Policy$DefaultVersionId`,
    /// "the identifier for the version of the policy that is set as the default
    /// version".
    ///
    /// AWS bumps a managed policy's version as it revises the policy, so a long-lived
    /// entry is well past `v1` and two entries sit at different numbers. It is carried
    /// per entry, and it is the version a reader serves: the document below IS that
    /// version, so `GetPolicy`'s `DefaultVersionId` always names the version
    /// `GetPolicyVersion` serves.
    pub default_version_id: &'static str,
    /// When this policy was published — `Policy$CreateDate`, "the date and time, in
    /// ISO 8601 date-time format, when the policy was created" — written in that
    /// format and read through [`published_instant`].
    pub published: &'static str,
    /// When this policy was last revised — `Policy$UpdateDate`, "the date and time
    /// when the policy was last updated. When a policy has only one version, this
    /// field contains the date and time when the policy was created" — so an entry AWS
    /// has never revised carries the instant it was published, and a revised one
    /// carries the instant of the revision its [`Self::default_version_id`] names.
    pub revised: &'static str,
    /// Whether AWS publishes this policy as one that can be attached to an IAM user,
    /// group or role — `Policy$IsAttachable`, "specifies whether the policy can be
    /// attached to an IAM user, group, or role".
    ///
    /// Carried per entry, because it is a fact AWS publishes about the policy rather
    /// than something to read off its path. It is a REPORT, not the reservation: the
    /// policies AWS files under the path it reserves for service-linked roles are
    /// published as attachable like the rest, and an attempt to put one on an identity
    /// is what gets refused
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_is_attachable`]).
    pub is_attachable: bool,
    /// The document of the version [`Self::default_version_id`] names, an IAM JSON
    /// policy.
    pub document: &'static str,
}

impl AwsManagedPolicy {
    /// This entry's ARN: `arn:aws:iam::aws:policy<path><name>`.
    ///
    /// Built here rather than through `engine::emulator_arn` because that helper
    /// always writes the emulator's own account id into the account segment, and an
    /// AWS-managed policy is owned by AWS: its account segment is the literal
    /// [`AWS_MANAGED_POLICY_ACCOUNT`]. The rest of the ARN is the same grammar
    /// [`policy_arn`] builds and [`parse_policy_arn`] reads.
    pub fn arn(&self) -> String {
        format!(
            "arn:aws:iam::{AWS_MANAGED_POLICY_ACCOUNT}:policy{}{}",
            self.path, self.policy_name
        )
    }

    /// Whether AWS reserves this policy for the service-linked role it was written
    /// for, so no identity of an account may take it as a permissions policy or as a
    /// permissions boundary — "Amazon Web Services service role policies can only be
    /// attached to the service-linked role for that service"
    /// (`PolicyNotAttachableException`).
    ///
    /// The reservation is the PATH AWS files the policy under
    /// ([`SERVICE_LINKED_ROLE_PATH_PREFIX`]), which is why it is derived rather than
    /// carried: it holds for every policy under that path, including the ones this
    /// catalog does not carry. It is a different question from what
    /// [`Self::is_attachable`] reports, which is the flag AWS publishes for the policy.
    pub fn is_reserved_for_service_linked_role(&self) -> bool {
        is_service_linked_role_path(self.path)
    }

    /// This entry as a managed policy record: the `Policy` metadata every reader
    /// reports, plus the single version `GetPolicyVersion` serves.
    ///
    /// The two counts are the caller's, not the catalog's — they are what the
    /// account's own attachments and permissions boundaries make them, which is why
    /// they are passed in (see
    /// [`ComAmazonawsIamAwsidentitymanagementv20100508State::resolve_policy`]).
    pub(super) fn record(&self, attachment_count: i32, permissions_boundary_usage_count: i32) -> ManagedPolicy {
        let published = published_instant(self.published);
        let revised = published_instant(self.revised);
        let version = PolicyVersion {
            document: Some(self.document.to_string()),
            version_id: Some(self.default_version_id.to_string()),
            is_default_version: true,
            // The version in force came into being when the policy was last revised,
            // which is the instant that revision is published under.
            create_date: revised,
        };
        ManagedPolicy {
            policy: Policy {
                arn: Some(self.arn()),
                policy_id: Some(self.policy_id.to_string()),
                policy_name: Some(self.policy_name.to_string()),
                path: Some(self.path.to_string()),
                default_version_id: Some(self.default_version_id.to_string()),
                attachment_count: Some(attachment_count),
                permissions_boundary_usage_count: Some(permissions_boundary_usage_count),
                // "Specifies whether the policy can be attached to an IAM user,
                // group, or role" (`Policy$IsAttachable`): the flag AWS publishes for
                // this policy, carried per entry.
                is_attachable: self.is_attachable,
                // See the type's documentation: AWS's prose is not carried.
                description: None,
                create_date: published,
                update_date: revised,
                // An AWS-managed policy carries no tags: tagging is an account
                // operation on an account's own resource.
                tags: None,
            },
            versions: [(self.default_version_id.to_string(), version)]
                .into_iter()
                .collect(),
        }
    }
}

/// The instant a catalog entry's published timestamp names, or `None` when the text is
/// not an ISO 8601 date-time.
///
/// The calendar conversion comes from the one text-timestamp reader this crate holds
/// ([`parse_xs_date_time`]), so a catalog instant and a timestamp read off a request are
/// read the same way. The suite asserts every entry's two instants resolve, which is what
/// keeps a mistyped entry from reporting a policy with no dates at all.
fn published_instant(text: &str) -> Option<::aws_smithy_types::DateTime> {
    parse_xs_date_time(text)
}

/// The AWS-managed policies this emulator carries, ordered by ARN.
///
/// AWS-managed policies "are created and managed by AWS" and exist in every
/// account, so a read of one is NOT a read of account state — which is why they
/// live in this constant rather than in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`], the account's
/// own policy store.
///
/// SOURCE: the AWS Managed Policy Reference ("AWS managed policies", IAM User
/// Guide), which publishes each policy's path, friendly name and default-version
/// document. AWS's catalog runs to well over a thousand policies and this is a
/// SUBSET of it — the policies the emulator's own suites and fixtures attach, plus
/// the account-wide and job-function ones a client is most likely to read, the
/// per-service access families (a service's full-access policy and its read-only
/// counterpart, which AWS publishes together) a client resolves before it attaches
/// one, the
/// service-linked-role policies AWS reserves for the role they were written for
/// ([`AwsManagedPolicy::is_reserved_for_service_linked_role`]), the service-role
/// policies a client provisions a service's own role from, and the root-task
/// policies a privileged root session is taken for, so every FAMILY a client can meet
/// is discoverable here.
///
/// MEMBERSHIP is what an enumeration of a path prefix projects, so it is carried per
/// POLICY and not per family: a policy AWS publishes under a prefix that this constant
/// does not hold cannot be discovered by listing that prefix at all, which is a
/// different kind of gap from a member whose data is stale. Every entry added here is
/// therefore the ARN AWS publishes — its path and its friendly name — beside that
/// policy's own publication data, and the right way to grow the catalog towards AWS's
/// is to add the policies a prefix is missing rather than to widen what an existing
/// entry reports.
///
/// Every entry carries its OWN publication data beside its identity and document: its
/// `PolicyId`, the version number it is currently published at, the instants it was
/// published and last revised, and whether AWS publishes it as attachable. None of
/// those is a value shared by the catalog or derived from the policy's name, so two
/// entries differ in them exactly as two published policies do, and a client keying,
/// pinning or sorting the catalog by them sees one policy per value.
///
/// WHICH of those fields is a published VALUE differs per entry, and an entry does not
/// say so itself, so it is said once here. An entry's path and friendly name — the ARN
/// a client addresses it by, and so the catalog's membership — are always AWS's, taken
/// from the reference above. An entry's id, version and instants are AWS's where this
/// project has read them (the pages
/// `tests/all/published_catalog_constants.rs` pins literally are those), and for the
/// rest they stand for the FORM AWS publishes them in: an `ANPA` id out of the id
/// space, a version number consistent with whether the entry has been revised, and
/// instants no later than the revision they name. A field standing for the form is the
/// first thing to replace when the published value becomes available, and is never
/// what a client should pin — the two durable facts here are that AWS publishes the
/// policy, and that it publishes it at that path.
///
/// It also carries every AWS-managed policy ARN another service of this emulator
/// hands a caller as a recommendation or as the policy of a role it provisioned: a
/// control plane that names a policy ARN in a response must be able to describe that
/// ARN, so an identifier this emulator publishes is never one its own API reports as
/// missing. Two places do so today — the EKS add-on catalog's
/// `podIdentityConfiguration[].recommendedManagedPolicies`, whose five policies are
/// entries here, and the permissions policy Application Auto Scaling attaches to the
/// service-linked role it creates for a namespace, one per supported namespace, which
/// are the `AWSApplicationAutoscaling…Policy` entries of the service-linked-role
/// family below.
///
/// An `arn:aws:iam::aws:policy/` ARN this catalog does not carry resolves to no
/// policy, and a read addressed by such an ARN is refused as the missing entity
/// `NoSuchEntityException` names — the operations' modelled refusal — rather than
/// with a code none of them declares.
///
/// Where the catalog carries a policy, it is authoritative about WHERE that policy
/// lives: an entry's path is the one AWS publishes it at, and a friendly name
/// identifies one policy ([`aws_managed_policy_named`]), so the same name under any
/// other path names nothing — for an attachment and a permissions boundary as well as
/// for a read ([`is_published_aws_managed_policy_arn`]). Adding an entry therefore adds
/// both facts: that AWS publishes the policy, and that it publishes it there.
pub const AWS_MANAGED_POLICIES: &[AwsManagedPolicy] = &[
    // The policy AWS publishes to take every permission away: one statement, `Deny` on
    // every action and every resource. It is the only grant a principal can carry whose
    // whole point is that the principal may do NOTHING, so the document has to be held
    // here — an attachment whose document this emulator cannot read is a grant it cannot
    // judge, and reading this one as unknown would serve exactly the calls it forbids.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSDenyAll",
        policy_id: "ANPAIX3YKGWMV2PMTPNHL",
        default_version_id: "v1",
        published: "2019-05-01T19:18:31Z",
        revised: "2019-05-01T19:18:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"*","Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLakeFormationDataAdmin",
        policy_id: "ANPAQT4ZJRMWH6PVKLSND",
        default_version_id: "v4",
        published: "2019-08-08T19:00:40Z",
        revised: "2023-10-10T17:28:23Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lakeformation:*","cloudtrail:DescribeTrails","cloudtrail:LookupEvents","glue:GetDatabase","glue:GetDatabases","glue:CreateDatabase","glue:UpdateDatabase","glue:DeleteDatabase","glue:GetConnections","glue:SearchTables","glue:GetTable","glue:CreateTable","glue:UpdateTable","glue:DeleteTable","glue:GetTableVersions","glue:GetPartitions","glue:GetTables","glue:ListWorkflows","glue:BatchGetWorkflows","glue:DeleteWorkflow","glue:GetWorkflows","glue:GetTags","glue:TagResource","glue:UnTagResource","iam:ListUsers","iam:ListRoles","iam:GetRole","iam:GetRolePolicy","s3:ListAllMyBuckets","s3:GetBucketLocation"],"Resource":"*"},{"Effect":"Deny","Action":["lakeformation:PutDataLakeSettings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLambdaExecute",
        policy_id: "ANPABX7MQWRTZ3KJHVNDC",
        default_version_id: "v4",
        published: "2015-02-06T18:40:46Z",
        revised: "2019-12-13T16:51:20Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"arn:aws:logs:*:*:*"},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"arn:aws:s3:::*"}]}"#,
    },
    // The write half of the AWS X-Ray grants, which an application that only emits
    // trace segments is given: named by a CloudFormation `AWS::Serverless::Function`
    // policy template by its bare friendly name, which resolves at the account root.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSXrayWriteOnlyAccess",
        policy_id: "ANPAVR2TYPLKJ5WQZMNHB",
        default_version_id: "v5",
        published: "2016-12-01T18:30:55Z",
        revised: "2021-10-05T20:21:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":["*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AdministratorAccess",
        policy_id: "ANPAKD6NWQJZT4YPMLRVC",
        default_version_id: "v1",
        published: "2015-02-06T18:39:46Z",
        revised: "2015-02-06T18:39:46Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
    },
    // The two halves of the access policy AWS publishes for Amazon DynamoDB, the pair
    // every service-access family of the reference comes in: the full-access policy a
    // principal that administers tables carries, and the read-only one a principal that
    // only reads them carries. Both grant the neighbouring services DynamoDB's own
    // console and auto-scaling reach through, which is why neither document is a bare
    // `dynamodb:` grant.
    //
    // As the rule above has it: the path and the friendly name of each entry are AWS's,
    // taken from the reference, and so is the membership they carry; this project has not
    // read these two policies' published id, version number or instants, so those fields
    // stand for the FORM AWS publishes them in and are the first thing to replace when
    // the published values become available. The same holds for the read-only halves of
    // the Amazon SNS and Amazon SQS families below.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonDynamoDBFullAccess",
        policy_id: "ANPAJQ7WTVM4ZKP3NHLRD",
        default_version_id: "v19",
        published: "2015-02-06T18:40:11Z",
        revised: "2023-05-15T18:28:57Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:*","dax:*","application-autoscaling:DeleteScalingPolicy","application-autoscaling:DeregisterScalableTarget","application-autoscaling:DescribeScalableTargets","application-autoscaling:DescribeScalingActivities","application-autoscaling:DescribeScalingPolicies","application-autoscaling:PutScalingPolicy","application-autoscaling:RegisterScalableTarget","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarmHistory","cloudwatch:DescribeAlarms","cloudwatch:DescribeAlarmsForMetric","cloudwatch:GetMetricStatistics","cloudwatch:ListMetrics","cloudwatch:PutMetricAlarm","cloudwatch:GetMetricData","datapipeline:ActivatePipeline","datapipeline:CreatePipeline","datapipeline:DeletePipeline","datapipeline:DescribeObjects","datapipeline:DescribePipelines","datapipeline:GetPipelineDefinition","datapipeline:ListPipelines","datapipeline:PutPipelineDefinition","datapipeline:QueryObjects","ec2:DescribeVpcs","ec2:DescribeSubnets","ec2:DescribeSecurityGroups","iam:GetRole","iam:ListRoles","kms:DescribeKey","kms:ListAliases","sns:CreateTopic","sns:DeleteTopic","sns:ListSubscriptions","sns:ListSubscriptionsByTopic","sns:ListTopics","sns:Subscribe","sns:Unsubscribe","sns:SetTopicAttributes","lambda:CreateFunction","lambda:ListFunctions","lambda:ListEventSourceMappings","lambda:CreateEventSourceMapping","lambda:DeleteEventSourceMapping","lambda:GetFunctionConfiguration","lambda:DeleteFunction","resource-groups:ListGroups","resource-groups:ListGroupResources","resource-groups:GetGroup","resource-groups:GetGroupQuery","resource-groups:DeleteGroup","resource-groups:CreateGroup","tag:GetResources","kinesis:ListStreams","kinesis:DescribeStream","kinesis:DescribeStreamSummary"],"Resource":"*"},{"Effect":"Allow","Action":"cloudwatch:GetInsightRuleReport","Resource":"arn:aws:cloudwatch:*:*:insight-rule/DynamoDBContributorInsights*"},{"Effect":"Allow","Action":["iam:CreateServiceLinkedRole"],"Resource":"*","Condition":{"StringEquals":{"iam:AWSServiceName":["replication.dynamodb.amazonaws.com","dax.amazonaws.com","dynamodb.application-autoscaling.amazonaws.com","contributorinsights.dynamodb.amazonaws.com","kinesisreplication.dynamodb.amazonaws.com"]}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonDynamoDBReadOnlyAccess",
        policy_id: "ANPAW4JTZQKM7PVNHLRB2",
        default_version_id: "v16",
        published: "2015-02-06T18:40:14Z",
        revised: "2023-05-15T18:29:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["application-autoscaling:DescribeScalableTargets","application-autoscaling:DescribeScalingActivities","application-autoscaling:DescribeScalingPolicies","cloudwatch:DescribeAlarmHistory","cloudwatch:DescribeAlarms","cloudwatch:DescribeAlarmsForMetric","cloudwatch:GetMetricStatistics","cloudwatch:ListMetrics","cloudwatch:GetMetricData","datapipeline:DescribeObjects","datapipeline:DescribePipelines","datapipeline:GetPipelineDefinition","datapipeline:ListPipelines","datapipeline:QueryObjects","dynamodb:BatchGetItem","dynamodb:Describe*","dynamodb:List*","dynamodb:GetItem","dynamodb:GetResourcePolicy","dynamodb:Query","dynamodb:Scan","dynamodb:PartiQLSelect","dax:Describe*","dax:List*","dax:GetItem","dax:BatchGetItem","dax:Query","dax:Scan","ec2:DescribeVpcs","ec2:DescribeSubnets","ec2:DescribeSecurityGroups","iam:GetRole","iam:ListRoles","kms:DescribeKey","kms:ListAliases","sns:ListSubscriptionsByTopic","sns:ListTopics","lambda:ListFunctions","lambda:ListEventSourceMappings","lambda:GetFunctionConfiguration","resource-groups:ListGroups","resource-groups:ListGroupResources","resource-groups:GetGroup","resource-groups:GetGroupQuery","tag:GetResources"],"Resource":"*"},{"Effect":"Allow","Action":"cloudwatch:GetInsightRuleReport","Resource":"arn:aws:cloudwatch:*:*:insight-rule/DynamoDBContributorInsights*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-ebs-csi-driver` add-on's
    // `ebs-csi-controller-sa` service account: the policy Amazon Web Services
    // publishes for the driver NOW, at the account root, which supersedes the
    // `/service-role/` entry of the same family below. The conditions are the
    // published policy's: the driver may create and delete only the volumes and
    // snapshots tagged for it, and it may restore a snapshot's fast-restore state.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEBSCSIDriverPolicyV2",
        policy_id: "ANPAZQT2WKMJ6PVNHLR4B",
        default_version_id: "v2",
        published: "2024-11-14T18:12:04Z",
        revised: "2025-02-20T16:41:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:AttachVolume","ec2:DetachVolume","ec2:ModifyVolume","ec2:DescribeAvailabilityZones","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeTags","ec2:DescribeVolumes","ec2:DescribeVolumesModifications","ec2:EnableFastSnapshotRestores"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"],"Condition":{"StringEquals":{"ec2:CreateAction":["CreateVolume","CreateSnapshot"]}}},{"Effect":"Allow","Action":["ec2:DeleteTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"]},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeSnapshotName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    // The policy AWS documents for the pull half of an Amazon ECR registry ("Amazon
    // ECR managed policies", Amazon ECR User Guide): the grants a container runtime
    // needs to log in to the registry and fetch a manifest and its layers, which is
    // why the node role of an Amazon EKS node group carries it and why a node whose
    // role does not cannot pull a private image at all.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEC2ContainerRegistryReadOnly",
        policy_id: "ANPAJ4ZMKQWT7VPNHLRDB",
        default_version_id: "v3",
        published: "2015-12-21T17:06:46Z",
        revised: "2019-12-10T20:51:32Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:GetAuthorizationToken","ecr:BatchCheckLayerAvailability","ecr:GetDownloadUrlForLayer","ecr:GetRepositoryPolicy","ecr:DescribeRepositories","ecr:ListImages","ecr:DescribeImages","ecr:BatchGetImage","ecr:GetLifecyclePolicy","ecr:GetLifecyclePolicyPreview","ecr:ListTagsForResource","ecr:DescribeImageScanFindings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEC2ReadOnlyAccess",
        policy_id: "ANPAZM3PKVTWQ7JHNDRLB",
        default_version_id: "v1",
        published: "2015-02-06T18:40:16Z",
        revised: "2015-02-06T18:40:16Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"},{"Effect":"Allow","Action":"elasticloadbalancing:Describe*","Resource":"*"},{"Effect":"Allow","Action":["cloudwatch:ListMetrics","cloudwatch:GetMetricStatistics","cloudwatch:Describe*"],"Resource":"*"},{"Effect":"Allow","Action":"autoscaling:Describe*","Resource":"*"}]}"#,
    },
    // The policy an EKS cluster's own service role carries, published at the account
    // root: the grant the control plane manages a cluster's EC2 network interfaces,
    // load balancers and volumes through.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKSClusterPolicy",
        policy_id: "ANPAHQ5WJZMPT2KVNLYRD",
        default_version_id: "v5",
        published: "2018-05-27T21:06:14Z",
        revised: "2023-02-16T20:20:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:DescribeAutoScalingGroups","autoscaling:UpdateAutoScalingGroup","ec2:AttachVolume","ec2:AuthorizeSecurityGroupIngress","ec2:CreateRoute","ec2:CreateSecurityGroup","ec2:CreateTags","ec2:CreateVolume","ec2:DeleteRoute","ec2:DeleteSecurityGroup","ec2:DeleteVolume","ec2:Describe*","ec2:DetachVolume","ec2:ModifyInstanceAttribute","ec2:ModifyVolume","ec2:RevokeSecurityGroupIngress","elasticloadbalancing:Add*","elasticloadbalancing:Apply*","elasticloadbalancing:Attach*","elasticloadbalancing:Configure*","elasticloadbalancing:Create*","elasticloadbalancing:Delete*","elasticloadbalancing:Deregister*","elasticloadbalancing:Describe*","elasticloadbalancing:Detach*","elasticloadbalancing:Modify*","elasticloadbalancing:Register*","elasticloadbalancing:Set*","kms:DescribeKey"],"Resource":"*"},{"Effect":"Allow","Action":"iam:CreateServiceLinkedRole","Resource":"*","Condition":{"StringEquals":{"iam:AWSServiceName":"elasticloadbalancing.amazonaws.com"}}}]}"#,
    },
    // The policy AWS documents for the POD EXECUTION ROLE of an Amazon EKS Fargate
    // profile ("Amazon EKS Pod execution IAM role", Amazon EKS User Guide): the Amazon
    // ECR grants the Fargate infrastructure pulls a selected Pod's images with, and
    // NOTHING else — a Fargate Pod's log router writes to CloudWatch Logs as this same
    // role, so a role carrying only this policy may not create a log group or a log
    // stream and may not put log events, and the grants for that are the separate policy
    // an operator adds beside this one.
    //
    // As the rule above has it: the path, the friendly name and the document are AWS's,
    // taken from the reference; this project has not read this policy's published id,
    // version number or instants, so those fields stand for the FORM AWS publishes them
    // in and are the first thing to replace when the published values become available.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKSFargatePodExecutionRolePolicy",
        policy_id: "ANPAQF7VXTLMKZJ3NHRDB",
        default_version_id: "v1",
        published: "2019-11-25T22:03:19Z",
        revised: "2019-11-25T22:03:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:GetAuthorizationToken","ecr:BatchCheckLayerAvailability","ecr:GetDownloadUrlForLayer","ecr:BatchGetImage"],"Resource":"*"}]}"#,
    },
    // The policy AWS documents for the NODE role of an Amazon EKS node group ("Amazon
    // EKS node IAM role", Amazon EKS User Guide): the Amazon EC2 describes the
    // in-cluster AWS cloud provider makes, the cluster read a joining node makes, and
    // the pod identity exchange a pod of that node makes. It carries no Amazon ECR
    // grant — "your nodes need permissions to pull images from Amazon ECR", which is
    // the separate policy a node role is given beside this one.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKSWorkerNodePolicy",
        policy_id: "ANPAVKZ4TJWQM7PNHLRBD",
        default_version_id: "v3",
        published: "2018-05-27T21:09:01Z",
        revised: "2023-11-27T21:40:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeInstanceTypes","ec2:DescribeRouteTables","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVolumes","ec2:DescribeVolumesModifications","ec2:DescribeVpcs","eks:DescribeCluster","eks-auth:AssumeRoleForPodIdentity"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `vpc-cni` add-on's `aws-node`
    // service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKS_CNI_Policy",
        policy_id: "ANPATN4KQZWVP6MJHLRCB",
        default_version_id: "v5",
        published: "2018-05-27T21:07:42Z",
        revised: "2023-03-15T18:58:53Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignPrivateIpAddresses","ec2:AttachNetworkInterface","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeTags","ec2:DescribeNetworkInterfaces","ec2:DescribeInstanceTypes","ec2:DescribeSubnets","ec2:DetachNetworkInterface","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignPrivateIpAddresses"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:network-interface/*"]}]}"#,
    },
    // The placement-group grant an EMR cluster's role carries, published at the
    // account root — NOT under `/service-role/`, where the legacy EMR service-role
    // policy below lives.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonElasticMapReducePlacementGroupPolicy",
        policy_id: "ANPAWY2JTMKQZ5PVNHDRL",
        default_version_id: "v1",
        published: "2020-09-24T22:12:11Z",
        revised: "2020-09-24T22:12:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreatePlacementGroup","ec2:DeletePlacementGroup","ec2:DescribePlacementGroups"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3FullAccess",
        policy_id: "ANPAPL7VQZKMW3TJNHBDY",
        default_version_id: "v2",
        published: "2015-02-06T18:40:58Z",
        revised: "2021-08-16T18:14:00Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*","s3-object-lambda:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3ReadOnlyAccess",
        policy_id: "ANPACR6MZQWJT4KVPLNHY",
        default_version_id: "v3",
        published: "2015-02-06T18:40:59Z",
        revised: "2021-08-16T18:19:38Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*","s3:Describe*","s3-object-lambda:Get*","s3-object-lambda:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSNSFullAccess",
        policy_id: "ANPADK3TQJMZW7VPNLHYB",
        default_version_id: "v1",
        published: "2015-02-06T18:41:05Z",
        revised: "2015-02-06T18:41:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:*"],"Resource":"*"}]}"#,
    },
    // The read-only half of the Amazon SNS family, published beside the full-access one
    // above.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSNSReadOnlyAccess",
        policy_id: "ANPAQL5ZJWKMT2PVNHRD7",
        default_version_id: "v1",
        published: "2015-02-06T18:41:06Z",
        revised: "2015-02-06T18:41:06Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:GetTopicAttributes","sns:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSQSFullAccess",
        policy_id: "ANPAFM5QZTKWJ2PVNHRLD",
        default_version_id: "v1",
        published: "2015-02-06T18:41:07Z",
        revised: "2015-02-06T18:41:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:*"],"Resource":"*"}]}"#,
    },
    // The read-only half of the Amazon SQS family, published beside the full-access one
    // above.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSQSReadOnlyAccess",
        policy_id: "ANPAVN7JQZKMW4PTHLRB5",
        default_version_id: "v2",
        published: "2015-02-06T18:41:08Z",
        revised: "2021-06-29T19:36:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:GetQueueAttributes","sqs:GetQueueUrl","sqs:ListQueues","sqs:ListDeadLetterSourceQueues","sqs:ListQueueTags"],"Resource":"*"}]}"#,
    },
    // The policy AWS documents for the instance profile of a node that is to be managed
    // by Systems Manager ("Configure instance permissions required for Systems Manager",
    // Systems Manager User Guide): its grants are what let the SSM Agent on an instance
    // register the node and keep reporting it, which is why an instance carrying it
    // becomes a managed node and one that does not never registers.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSSMManagedInstanceCore",
        policy_id: "ANPAWT3JQZMKV5PNHLRBD",
        default_version_id: "v1",
        published: "2017-08-14T19:32:43Z",
        revised: "2017-08-14T19:32:43Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:DescribeAssociation","ssm:GetDeployablePatchSnapshotForInstance","ssm:GetDocument","ssm:DescribeDocument","ssm:GetManifest","ssm:ListAssociations","ssm:ListInstanceAssociations","ssm:PutInventory","ssm:PutComplianceItems","ssm:PutConfigurePackageResult","ssm:UpdateAssociationStatus","ssm:UpdateInstanceAssociationStatus","ssm:UpdateInstanceInformation"],"Resource":"*"},{"Effect":"Allow","Action":["ssmmessages:CreateControlChannel","ssmmessages:CreateDataChannel","ssmmessages:OpenControlChannel","ssmmessages:OpenDataChannel"],"Resource":"*"},{"Effect":"Allow","Action":["ec2messages:AcknowledgeMessage","ec2messages:DeleteMessage","ec2messages:FailMessage","ec2messages:GetEndpoint","ec2messages:GetMessages","ec2messages:SendReply"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the
    // `amazon-cloudwatch-observability` add-on's `cloudwatch-agent` service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchAgentServerPolicy",
        policy_id: "ANPAGN2WJQZMT6KVPLHYR",
        default_version_id: "v4",
        published: "2017-11-28T19:45:48Z",
        revised: "2023-07-19T22:23:37Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:DescribeVolumes","ec2:DescribeTags","logs:PutLogEvents","logs:PutRetentionPolicy","logs:DescribeLogStreams","logs:DescribeLogGroups","logs:CreateLogStream","logs:CreateLogGroup","xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":"*"},{"Effect":"Allow","Action":["ssm:GetParameter"],"Resource":"arn:aws:ssm:*:*:parameter/AmazonCloudWatch-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchLogsFullAccess",
        policy_id: "ANPAHP4ZQJMWT3KVNLYRB",
        default_version_id: "v2",
        published: "2015-02-06T18:40:40Z",
        revised: "2023-11-10T21:17:20Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMFullAccess",
        policy_id: "ANPAJQ6TMZWKP5VJNHLRD",
        default_version_id: "v3",
        published: "2015-02-06T18:40:38Z",
        revised: "2021-02-23T17:48:56Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:*","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:DescribePolicy","organizations:ListChildren","organizations:ListParents","organizations:ListPoliciesForTarget","organizations:ListRoots","organizations:ListPolicies","organizations:ListTargetsForPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMReadOnlyAccess",
        policy_id: "ANPAKR3WQZJMT7PVNHLYB",
        default_version_id: "v4",
        published: "2015-02-06T18:40:39Z",
        revised: "2018-01-25T19:11:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GenerateCredentialReport","iam:GenerateServiceLastAccessedDetails","iam:Get*","iam:List*","iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "PowerUserAccess",
        policy_id: "ANPALS5MQZWKT2JVPNHRD",
        default_version_id: "v5",
        published: "2015-02-06T18:39:47Z",
        revised: "2023-07-06T19:34:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":["iam:*","organizations:*","account:*"],"Resource":"*"},{"Effect":"Allow","Action":["iam:CreateServiceLinkedRole","iam:DeleteServiceLinkedRole","iam:ListRoles","organizations:DescribeOrganization","account:ListRegions","account:GetAccountInformation"],"Resource":"*"}]}"#,
    },
    // The policy a client most often resolves by name: "ReadOnlyAccess" grants the read
    // verbs of EVERY service. AWS's own default version enumerates them service by
    // service and runs far past the character quota a managed policy's document is held
    // to here ([`MAX_MANAGED_POLICY_CHARACTERS`]), so the document carried for it states
    // the same grant in the form the IAM action grammar already has for "an action of
    // every service": the read verb families AWS's document is built out of, each over
    // the whole namespace position, beside the two policy-simulation actions AWS's names
    // outside those families. A reader of this document therefore sees the services a
    // read-only principal can reach as AWS's document puts them in reach — every one —
    // rather than the handful a hand-written namespace list named. The policy's identity
    // — its path, name, ARN and attachability — is AWS's.
    AwsManagedPolicy {
        path: "/",
        policy_name: "ReadOnlyAccess",
        policy_id: "ANPAMT7QZJWKP4VNHLYRB",
        default_version_id: "v116",
        published: "2015-02-06T18:39:48Z",
        revised: "2025-11-03T18:12:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["*:BatchGet*","*:Describe*","*:Get*","*:List*","*:Lookup*","*:Query*","*:Scan*","*:Search*","*:View*"],"Resource":"*"},{"Effect":"Allow","Action":["iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy"],"Resource":"*"}]}"#,
    },
    // The policies AWS files under the path it reserves for service-linked roles.
    // Each belongs to the service-linked role it was written for, so AWS reserves it
    // ([`AwsManagedPolicy::is_reserved_for_service_linked_role`]) and no identity of an
    // account can attach it or be bounded by it — while publishing it, like every other
    // policy of its catalog, as one that can be attached (`IsAttachable` true). A client
    // provisioning a service-linked role resolves the policy of the service it is
    // provisioning FOR by listing this family, so the family is carried rather than the
    // one or two entries a single fixture happens to name.
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "APIGatewayServiceRolePolicy",
        policy_id: "ANPAB4TQZJMWT5PVNHLRD",
        default_version_id: "v10",
        published: "2017-10-20T17:23:10Z",
        revised: "2023-05-04T18:21:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","elasticloadbalancing:DescribeLoadBalancers","firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch","servicediscovery:DiscoverInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAppConfigServiceRolePolicy",
        policy_id: "ANPAC7KWKJZQMT7PVNHLB",
        default_version_id: "v3",
        published: "2022-04-19T16:05:59Z",
        revised: "2023-11-27T21:34:12Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAppFabricServiceRolePolicy",
        policy_id: "ANPAB3KTQZJMWVXTPNHLR",
        default_version_id: "v1",
        published: "2023-06-27T18:12:04Z",
        revised: "2023-06-27T18:12:04Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogDelivery","logs:DeleteLogDelivery","logs:DescribeLogGroups","logs:GetLogDelivery","logs:ListLogDeliveries","logs:UpdateLogDelivery"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAppMeshPreviewServiceRolePolicy",
        policy_id: "ANPAC7XMQTZJWV4PNHLRD",
        default_version_id: "v1",
        published: "2019-04-11T17:25:40Z",
        revised: "2019-04-11T17:25:40Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeSubnets","ec2:DescribeVpcs","servicediscovery:DiscoverInstances","servicediscovery:GetNamespace","servicediscovery:GetService"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAppMeshServiceRolePolicy",
        policy_id: "ANPAD2MZQTMWKJ4PVNHLY",
        default_version_id: "v6",
        published: "2019-03-28T21:30:35Z",
        revised: "2022-02-15T19:08:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeSubnets","ec2:DescribeVpcs","servicediscovery:DiscoverInstances","servicediscovery:GetNamespace","servicediscovery:GetService"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAppSyncServiceRolePolicy",
        policy_id: "ANPAF6NMJWQZKT6PVNHLC",
        default_version_id: "v4",
        published: "2019-06-07T18:27:22Z",
        revised: "2021-09-30T17:14:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // AWS spells this one entry of the Application Auto Scaling family with a capital `S`
    // in "AutoScaling", unlike the fourteen beside it, so the published name is carried as
    // AWS publishes it rather than composed like its siblings'.
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoScalingCustomResourcePolicy",
        policy_id: "ANPAQ4MZQTMWKJ4PVNHLY",
        default_version_id: "v2",
        published: "2018-07-11T18:29:17Z",
        revised: "2019-05-20T19:33:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","execute-api:Invoke"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingAppStreamFleetPolicy",
        policy_id: "ANPAG3PQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2017-10-26T21:20:38Z",
        revised: "2018-07-19T18:38:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["appstream:DescribeFleets","appstream:UpdateFleet","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingCassandraTablePolicy",
        policy_id: "ANPAH8QWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2020-11-10T18:03:12Z",
        revised: "2020-11-10T18:03:12Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cassandra:Alter","cassandra:Select","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingComprehendEndpointPolicy",
        policy_id: "ANPAJ5RZQTMWKJ4PVNHLY",
        default_version_id: "v2",
        published: "2019-11-26T18:18:49Z",
        revised: "2021-03-22T20:11:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","comprehend:DescribeEndpoint","comprehend:UpdateEndpoint"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingDynamoDBTablePolicy",
        policy_id: "ANPAK9SMJWQZKT6PVNHLC",
        default_version_id: "v3",
        published: "2017-10-20T21:15:04Z",
        revised: "2022-08-18T17:42:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","dynamodb:DescribeTable","dynamodb:UpdateTable"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingEC2SpotFleetRequestPolicy",
        policy_id: "ANPAL2TQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2017-10-25T23:50:04Z",
        revised: "2019-09-11T20:26:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:DescribeSpotFleetRequests","ec2:ModifySpotFleetRequest"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingECSServicePolicy",
        policy_id: "ANPAM7VWKJZQMT7PVNHLB",
        default_version_id: "v3",
        published: "2017-10-25T23:53:08Z",
        revised: "2023-02-09T19:04:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ecs:DescribeServices","ecs:UpdateService"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingEMRInstanceGroupPolicy",
        policy_id: "ANPAN4WZQTMWKJ4PVNHLY",
        default_version_id: "v2",
        published: "2017-10-26T21:28:57Z",
        revised: "2020-02-18T19:33:25Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","elasticmapreduce:ListInstanceGroups","elasticmapreduce:ModifyInstanceGroups"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingElastiCacheRGPolicy",
        policy_id: "ANPAP6XMJWQZKT6PVNHLC",
        default_version_id: "v2",
        published: "2021-08-19T17:33:46Z",
        revised: "2023-06-21T16:58:02Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","elasticache:DecreaseReplicaCount","elasticache:DescribeReplicationGroups","elasticache:IncreaseReplicaCount","elasticache:ModifyReplicationGroupShardConfiguration"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingKafkaClusterPolicy",
        policy_id: "ANPAQ3YQZJMWT5PVNHLRD",
        default_version_id: "v1",
        published: "2020-09-30T21:04:17Z",
        revised: "2020-09-30T21:04:17Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","kafka:DescribeCluster","kafka:DescribeClusterOperation","kafka:UpdateBrokerStorage"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingLambdaConcurrencyPolicy",
        policy_id: "ANPAR8ZWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2019-12-03T17:48:29Z",
        revised: "2019-12-03T17:48:29Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","lambda:DeleteProvisionedConcurrencyConfig","lambda:GetProvisionedConcurrencyConfig","lambda:PutProvisionedConcurrencyConfig"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingNeptuneClusterPolicy",
        policy_id: "ANPAS5BZQTMWKJ4PVNHLY",
        default_version_id: "v1",
        published: "2021-01-26T18:12:40Z",
        revised: "2021-01-26T18:12:40Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","rds:AddTagsToResource","rds:CreateDBInstance","rds:DeleteDBInstance","rds:DescribeDBClusters","rds:DescribeDBInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingRDSClusterPolicy",
        policy_id: "ANPAT9CMJWQZKT6PVNHLC",
        default_version_id: "v4",
        published: "2017-10-17T17:46:56Z",
        revised: "2022-04-28T17:05:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","rds:AddTagsToResource","rds:CreateDBInstance","rds:DeleteDBInstance","rds:DescribeDBClusters","rds:DescribeDBInstances","rds:ModifyDBCluster"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingSageMakerEndpointPolicy",
        policy_id: "ANPAV2DQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2018-02-26T21:02:12Z",
        revised: "2022-11-29T18:47:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","sagemaker:DescribeEndpoint","sagemaker:DescribeEndpointConfig","sagemaker:UpdateEndpointWeightsAndCapacities"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationAutoscalingWorkSpacesPoolPolicy",
        policy_id: "ANPAW7FWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2024-06-25T17:19:58Z",
        revised: "2024-06-25T17:19:58Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","workspaces:DescribeWorkspacesPools","workspaces:UpdateWorkspacesPool"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSApplicationMigrationServiceRolePolicy",
        policy_id: "ANPAX4GZQTMWKJ4PVNHLY",
        default_version_id: "v9",
        published: "2021-04-07T17:57:41Z",
        revised: "2024-01-23T19:26:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeInstanceTypes","ec2:DescribeSnapshots","ec2:DescribeVolumes","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSArtifactServiceRolePolicy",
        policy_id: "ANPAF2YSQWZJTV8PNHLRD",
        default_version_id: "v1",
        published: "2018-10-04T19:08:17Z",
        revised: "2018-10-04T19:08:17Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts","organizations:ListAWSServiceAccessForOrganization"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAuditManagerServiceRolePolicy",
        policy_id: "ANPAY6HMJWQZKT6PVNHLC",
        default_version_id: "v7",
        published: "2020-12-08T19:33:52Z",
        revised: "2023-04-11T21:16:45Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:LookupEvents","config:DescribeConfigRules","config:DescribeDeliveryChannels","iam:GetAccountSummary","iam:ListUsers","organizations:DescribeOrganization","organizations:ListAccounts","securityhub:GetFindings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSAutoScalingPlansEC2AutoScalingPolicy",
        policy_id: "ANPAZ3JQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2018-08-23T22:46:46Z",
        revised: "2019-10-28T18:33:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:DescribeAutoScalingGroups","autoscaling:DescribePolicies","autoscaling:DescribeScheduledActions","autoscaling:PutScalingPolicy","autoscaling:PutScheduledUpdateGroupAction","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSBackupServiceLinkedRolePolicyForBackup",
        policy_id: "ANPANV2ZQJMWT6KPLHYRD",
        default_version_id: "v12",
        published: "2019-01-10T21:01:08Z",
        revised: "2024-02-14T18:33:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["backup:DescribeBackupVault","backup:ListBackupVaults","dynamodb:DescribeTable","ec2:CopySnapshot","ec2:CreateSnapshot","ec2:DeleteSnapshot","ec2:DescribeSnapshots","ec2:DescribeVolumes","elasticfilesystem:DescribeFilesystems","rds:DescribeDBInstances","rds:DescribeDBSnapshots","storagegateway:DescribeCachediSCSIVolumes","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSBillingServiceRolePolicy",
        policy_id: "ANPAG5ZUQWMJTV6PNHLRD",
        default_version_id: "v1",
        published: "2023-09-08T16:32:11Z",
        revised: "2023-09-08T16:32:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts","organizations:ListParents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSChatbotServiceLinkedRolePolicy",
        policy_id: "ANPAH9WNQZMJTV1PKHLRD",
        default_version_id: "v2",
        published: "2020-07-14T19:05:26Z",
        revised: "2022-03-02T18:21:47Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","logs:Describe*","logs:Get*","sns:ListSubscriptionsByTopic","sns:ListTopics","sns:Unsubscribe"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSCloud9ServiceRolePolicy",
        policy_id: "ANPAK3TQZJMW5VPNHLRDB",
        default_version_id: "v3",
        published: "2018-11-27T18:00:34Z",
        revised: "2020-02-26T17:48:02Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeInstances","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:RunInstances","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSCloudFrontLogger",
        policy_id: "ANPAL7XMQTZJ4WVPNHLRD",
        default_version_id: "v2",
        published: "2018-06-12T20:35:12Z",
        revised: "2021-10-14T18:03:55Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"arn:aws:logs:*:*:log-group:/aws/cloudfront/*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSCodeStarNotificationsServiceRolePolicy",
        policy_id: "ANPAM2YSQWZJ8TVPNHLRD",
        default_version_id: "v4",
        published: "2019-11-05T20:15:28Z",
        revised: "2023-02-16T17:52:40Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["chatbot:DescribeSlackChannelConfigurations","codecommit:GetCommentsForComparedCommit","codecommit:GetCommentsForPullRequest","codecommit:GetDifferences","sns:Publish"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSConfigMultiAccountSetupPolicy",
        policy_id: "ANPAN5ZUQWMJ6TVPNHLRD",
        default_version_id: "v6",
        published: "2020-04-21T19:33:17Z",
        revised: "2024-05-09T18:12:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["config:DescribeConfigurationRecorders","config:DescribeConfigurationRecorderStatus","config:PutConfigurationAggregator","config:PutConfigRule","organizations:DescribeAccount","organizations:ListAccounts","organizations:ListDelegatedAdministrators"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSConfigRemediationServiceRolePolicy",
        policy_id: "ANPAP9WNQZMJ1TVPKHLRD",
        default_version_id: "v2",
        published: "2019-07-30T17:44:21Z",
        revised: "2021-01-21T19:26:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:DescribeAutomationExecutions","ssm:DescribeAutomationStepExecutions","ssm:GetAutomationExecution","ssm:StartAutomationExecution","ssm:StopAutomationExecution"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSConfigServiceRolePolicy",
        policy_id: "ANPAPW4QZTMKJ3VNHLYRB",
        default_version_id: "v41",
        published: "2017-08-14T19:04:46Z",
        revised: "2025-04-02T15:27:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","cloudwatch:DescribeAlarms","config:Put*","dynamodb:DescribeTable","ec2:Describe*","elasticloadbalancing:DescribeLoadBalancers","iam:GetRole","iam:ListRoles","lambda:GetFunction","lambda:ListFunctions","organizations:DescribeOrganization","rds:DescribeDBInstances","s3:GetBucketAcl","s3:GetBucketLocation","s3:ListAllMyBuckets","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSDataSyncServiceRolePolicy",
        policy_id: "ANPAQ3KTZJMW5VPNHLRDB",
        default_version_id: "v5",
        published: "2018-12-10T18:05:39Z",
        revised: "2023-11-16T19:40:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSDeviceFarmServiceRolePolicy",
        policy_id: "ANPAR7XMTZJW4VPNHLRDC",
        default_version_id: "v1",
        published: "2019-02-21T18:37:14Z",
        revised: "2019-02-21T18:37:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSDirectConnectServiceRolePolicy",
        policy_id: "ANPAS2YSWZJT8VPNHLRDF",
        default_version_id: "v1",
        published: "2019-10-04T20:22:51Z",
        revised: "2019-10-04T20:22:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["secretsmanager:DescribeSecret","secretsmanager:GetSecretValue","secretsmanager:ListSecretVersionIds"],"Resource":"arn:aws:secretsmanager:*:*:secret:*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSDirectoryServiceServiceRolePolicy",
        policy_id: "ANPAT5ZUWMJT6VPNHLRDG",
        default_version_id: "v1",
        published: "2020-01-13T17:19:46Z",
        revised: "2020-01-13T17:19:46Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSEC2FleetServiceRolePolicy",
        policy_id: "ANPAU9WNZMJT1VPKHLRDH",
        default_version_id: "v1",
        published: "2018-04-18T20:48:32Z",
        revised: "2018-04-18T20:48:32Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeInstanceStatus","ec2:DescribeInstances","ec2:RunInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSEC2SpotFleetServiceRolePolicy",
        policy_id: "ANPAV3KTQJMW5VPNHLRDB",
        default_version_id: "v1",
        published: "2017-09-18T19:06:11Z",
        revised: "2017-09-18T19:06:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeImages","ec2:DescribeInstanceStatus","ec2:DescribeInstances","ec2:DescribeSubnets","ec2:RequestSpotInstances","ec2:RunInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSEC2SpotServiceRolePolicy",
        policy_id: "ANPAQX6MZJWKT5PVNHLRD",
        default_version_id: "v4",
        published: "2017-09-18T18:51:04Z",
        revised: "2021-06-17T19:12:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:RunInstances","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSECRPullThroughCache_ServiceRolePolicy",
        policy_id: "ANPAW7XMQZJW4VPNHLRDC",
        default_version_id: "v2",
        published: "2021-11-26T19:14:08Z",
        revised: "2023-08-09T17:31:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:BatchGetImage","ecr:CompleteLayerUpload","ecr:GetAuthorizationToken","ecr:InitiateLayerUpload","ecr:PutImage","ecr:UploadLayerPart"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticBeanstalkMaintenance",
        policy_id: "ANPAX2YSQZJT8VPNHLRDF",
        default_version_id: "v2",
        published: "2020-02-27T18:11:23Z",
        revised: "2022-04-07T16:58:40Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","cloudformation:UpdateStack","elasticbeanstalk:DescribeEnvironments","elasticbeanstalk:DescribePlatformVersion"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticBeanstalkManagedUpdatesServiceRolePolicy",
        policy_id: "ANPAY5ZUQMJT6VPNHLRDG",
        default_version_id: "v8",
        published: "2017-11-21T19:27:06Z",
        revised: "2024-07-24T18:44:35Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:DescribeAutoScalingGroups","cloudformation:DescribeStackResource","cloudformation:DescribeStacks","ec2:DescribeInstances","elasticbeanstalk:DescribeEnvironmentHealth","elasticbeanstalk:UpdateEnvironment","elasticloadbalancing:DescribeInstanceHealth"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticBeanstalkServiceRolePolicy",
        policy_id: "ANPARY3ZQTMWJ7KVPNHLB",
        default_version_id: "v8",
        published: "2017-09-13T23:46:37Z",
        revised: "2024-10-29T16:44:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudformation:Describe*","cloudformation:Get*","cloudformation:List*","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:Describe*","elasticloadbalancing:Describe*","logs:DescribeLogGroups","logs:DescribeLogStreams","s3:GetObject","s3:ListBucket","sns:ListSubscriptionsByTopic","sqs:GetQueueAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticLoadBalancingClassicServiceRolePolicy",
        policy_id: "ANPAZ9WNQMJT1VPKHLRDH",
        default_version_id: "v1",
        published: "2017-09-18T19:12:58Z",
        revised: "2017-09-18T19:12:58Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSecurityGroup","ec2:DescribeInstances","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticLoadBalancingServiceRolePolicy",
        policy_id: "ANPA3BKTQZJM5WVPNHLRD",
        default_version_id: "v1",
        published: "2017-09-18T19:13:36Z",
        revised: "2017-09-18T19:13:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSGlobalAcceleratorSLRPolicy",
        policy_id: "ANPA4C7XMQTZ9WVPNHLRD",
        default_version_id: "v2",
        published: "2018-11-26T20:09:17Z",
        revised: "2021-07-12T18:28:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","elasticloadbalancing:DescribeLoadBalancers"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSIPAMServiceRolePolicy",
        policy_id: "ANPA5F2YSQWZ8TVPNHLRD",
        default_version_id: "v2",
        published: "2021-12-02T18:33:49Z",
        revised: "2023-09-26T17:14:21Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeAccountAttributes","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","organizations:DescribeAccount","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSIncidentManagerServiceRolePolicy",
        policy_id: "ANPA6G5ZUQWM3TVPNHLRD",
        default_version_id: "v3",
        published: "2021-05-10T19:41:02Z",
        revised: "2023-03-15T18:07:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm-contacts:DescribeEngagement","ssm-contacts:ListContacts","ssm-contacts:StartEngagement","ssm-incidents:GetIncidentRecord","ssm-incidents:ListIncidentRecords"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSLambdaReplicator",
        policy_id: "ANPASZ5QJZMWT2KVPNHLR",
        default_version_id: "v4",
        published: "2017-07-06T22:25:19Z",
        revised: "2020-05-07T21:01:12Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lambda:CreateFunction","lambda:DeleteFunction","lambda:DisableReplication","lambda:EnableReplication","lambda:GetFunction","lambda:UpdateFunctionCode","lambda:UpdateFunctionConfiguration"],"Resource":"arn:aws:lambda:*:*:function:*"},{"Effect":"Allow","Action":["iam:PassRole"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSLambdaServiceRolePolicy",
        policy_id: "ANPA7H9WNQZM1TVPKHLRD",
        default_version_id: "v3",
        published: "2017-10-23T18:52:14Z",
        revised: "2021-04-20T17:35:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","xray:PutTelemetryRecords","xray:PutTraceSegments"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSLicenseManagerServiceRolePolicy",
        policy_id: "ANPA8BKTQZJM4WVPNHLRD",
        default_version_id: "v4",
        published: "2018-11-26T18:05:44Z",
        revised: "2022-02-10T19:18:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeImages","ec2:DescribeInstances","license-manager:ListUsageForLicenseConfiguration","organizations:DescribeOrganization","organizations:ListAccounts","ssm:ListInventoryEntries"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSNetworkFirewallServiceRolePolicy",
        policy_id: "ANPA9C7XMQTZ5WVPNHLRD",
        default_version_id: "v2",
        published: "2020-11-17T19:23:11Z",
        revised: "2022-09-28T18:40:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateVpcEndpoint","ec2:DeleteVpcEndpoints","ec2:DescribeSubnets","ec2:DescribeVpcEndpoints","ec2:DescribeVpcs"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSNetworkManagerServiceRolePolicy",
        policy_id: "ANPABF2YSQWZ6TVPNHLRD",
        default_version_id: "v3",
        published: "2019-12-03T17:28:50Z",
        revised: "2023-05-18T19:02:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["directconnect:DescribeConnections","directconnect:DescribeVirtualInterfaces","ec2:DescribeCustomerGateways","ec2:DescribeTransitGatewayAttachments","ec2:DescribeTransitGateways","ec2:DescribeVpnConnections"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSOrganizationsServiceTrustPolicy",
        policy_id: "ANPATB7MQZWKJ4PVNHLYR",
        default_version_id: "v2",
        published: "2017-02-27T00:20:51Z",
        revised: "2018-09-05T20:01:46Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:ListAccounts","organizations:ListAccountsForParent","organizations:ListChildren","organizations:ListParents","organizations:ListRoots"],"Resource":"*"},{"Effect":"Allow","Action":["iam:DeleteRole"],"Resource":"arn:aws:iam::*:role/aws-service-role/organizations.amazonaws.com/*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSOutpostsServiceRolePolicy",
        policy_id: "ANPACG5ZUQWM1TVPNHLRD",
        default_version_id: "v2",
        published: "2019-12-03T18:14:27Z",
        revised: "2022-11-08T17:46:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeSubnets","ec2:DescribeVpcs","organizations:DescribeAccount","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSResourceAccessManagerServiceRolePolicy",
        policy_id: "ANPADH9WNQZM3TVPKHLRD",
        default_version_id: "v3",
        published: "2018-11-20T19:35:12Z",
        revised: "2023-06-21T18:29:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:ListAccounts","organizations:ListAccountsForParent","organizations:ListRoots"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSSOServiceRolePolicy",
        policy_id: "ANPAUC2ZQJMWT6KVPNHLY",
        default_version_id: "v17",
        published: "2017-12-05T18:36:08Z",
        revised: "2024-06-20T17:09:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:AttachRolePolicy","iam:CreateRole","iam:CreateSAMLProvider","iam:DeleteRole","iam:DeleteRolePolicy","iam:DeleteSAMLProvider","iam:DetachRolePolicy","iam:GetRole","iam:GetSAMLProvider","iam:ListAttachedRolePolicies","iam:ListRolePolicies","iam:PutRolePolicy","iam:UpdateSAMLProvider","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSecurityHubServiceRolePolicy",
        policy_id: "ANPAEBKTQZJM5WVPNHLRD",
        default_version_id: "v4",
        published: "2018-11-27T20:12:38Z",
        revised: "2024-02-14T18:51:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","config:DescribeConfigurationRecorders","config:DescribeConfigRules","guardduty:ListDetectors","iam:GetAccountSummary","organizations:DescribeAccount","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSServiceRoleForEC2ScheduledInstances",
        policy_id: "ANPAFC7XMQTZ6WVPNHLRD",
        default_version_id: "v1",
        published: "2017-10-12T18:24:07Z",
        revised: "2017-10-12T18:24:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeInstances","ec2:RunInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSServiceRoleForImageBuilder",
        policy_id: "ANPAGF2YSQWZ1TVPNHLRD",
        default_version_id: "v6",
        published: "2019-12-02T18:03:21Z",
        revised: "2023-10-12T17:22:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateImage","ec2:CreateTags","ec2:DescribeImages","ec2:DescribeInstances","ec2:RunInstances","ec2:TerminateInstances","ssm:SendCommand"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSServiceRoleForLogDeliveryPolicy",
        policy_id: "ANPAHG5ZUQWM3TVPNHLRD",
        default_version_id: "v2",
        published: "2020-08-13T19:17:55Z",
        revised: "2021-12-07T18:35:12Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch","logs:PutLogEvents","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSShieldServiceRolePolicy",
        policy_id: "ANPAJH9WNQZM5TVPKHLRD",
        default_version_id: "v2",
        published: "2019-05-30T17:49:38Z",
        revised: "2021-03-24T19:11:26Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudfront:GetDistribution","cloudfront:ListDistributions","elasticloadbalancing:DescribeLoadBalancers","route53:GetHealthCheck","route53:ListHealthChecks"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSStorageGatewayServiceRolePolicy",
        policy_id: "ANPAKBKTQZJM6WVPNHLRD",
        default_version_id: "v1",
        published: "2020-06-04T18:28:44Z",
        revised: "2020-06-04T18:28:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["fsx:DescribeFileSystems","fsx:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSupportServiceRolePolicy",
        policy_id: "ANPAVD4QZTMWJ3KPVNHLR",
        default_version_id: "v29",
        published: "2018-04-19T18:04:44Z",
        revised: "2025-02-11T21:18:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["support:*","trustedadvisor:Describe*","trustedadvisor:Get*","trustedadvisor:List*","cloudformation:DescribeStacks","cloudwatch:DescribeAlarms","ec2:DescribeInstances","iam:GetRole","iam:ListRoles","s3:ListAllMyBuckets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSTransferServiceRolePolicy",
        policy_id: "ANPALC7XMQTZ1WVPNHLRD",
        default_version_id: "v2",
        published: "2021-02-11T19:04:16Z",
        revised: "2022-08-30T17:52:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSTrustedAdvisorServiceRolePolicy",
        policy_id: "ANPAWE6MZQJKT5PVNHLYR",
        default_version_id: "v11",
        published: "2018-02-22T21:24:25Z",
        revised: "2024-08-13T19:38:16Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudformation:DescribeStacks","cloudtrail:DescribeTrails","cloudwatch:GetMetricStatistics","ec2:Describe*","elasticloadbalancing:Describe*","iam:GenerateCredentialReport","iam:GetAccountSummary","iam:ListAccessKeys","kms:DescribeKey","rds:DescribeDBInstances","route53:ListHostedZones","s3:GetBucketAcl","s3:GetBucketPolicy","s3:ListAllMyBuckets","trustedadvisor:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSVPCFlowLogsServiceRolePolicy",
        policy_id: "ANPAMF2YSQWZ3TVPNHLRD",
        default_version_id: "v1",
        published: "2023-03-30T18:19:02Z",
        revised: "2023-03-30T18:19:02Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogDelivery","logs:DeleteLogDelivery","logs:GetLogDelivery","logs:ListLogDeliveries"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSVPCTransitGatewayServiceRolePolicy",
        policy_id: "ANPAXF3ZQTMWJ7KVPNHLD",
        default_version_id: "v2",
        published: "2018-11-26T16:19:13Z",
        revised: "2021-03-10T17:45:29Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSVpcLatticeServiceRolePolicy",
        policy_id: "ANPANG5ZUQWM5TVPNHLRD",
        default_version_id: "v2",
        published: "2023-03-30T19:08:27Z",
        revised: "2024-06-13T18:41:50Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","logs:CreateLogDelivery","logs:GetLogDelivery"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AccessAnalyzerServiceRolePolicy",
        policy_id: "ANPAPH9WNQZM6TVPKHLRD",
        default_version_id: "v6",
        published: "2019-12-02T17:14:33Z",
        revised: "2024-09-19T18:22:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GetRole","iam:ListRoles","kms:DescribeKey","kms:GetKeyPolicy","lambda:GetPolicy","s3:GetBucketPolicy","sqs:GetQueueAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonAthenaServiceRolePolicy",
        policy_id: "ANPAQBKTQZJM1WVPNHLRD",
        default_version_id: "v2",
        published: "2021-11-29T18:36:09Z",
        revised: "2023-07-19T17:25:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonCloudWatchRUMServiceRolePolicy",
        policy_id: "ANPARC7XMQTZ3WVPNHLRD",
        default_version_id: "v1",
        published: "2021-11-29T19:12:52Z",
        revised: "2021-11-29T19:12:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["xray:PutTelemetryRecords","xray:PutTraceSegments"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonCognitoIdpServiceRolePolicy",
        policy_id: "ANPASF2YSQWZ5TVPNHLRD",
        default_version_id: "v2",
        published: "2018-12-11T18:05:27Z",
        revised: "2020-10-15T17:48:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","cognito-idp:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonConnectServiceLinkedRolePolicy",
        policy_id: "ANPATG5ZUQWM6TVPNHLRD",
        default_version_id: "v9",
        published: "2018-10-16T19:22:14Z",
        revised: "2024-04-25T18:09:37Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["connect:DescribeInstance","kinesis:DescribeStream","kinesis:PutRecord","lex:PostContent","polly:SynthesizeSpeech","s3:GetObject","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonDetectiveServiceLinkedRolePolicy",
        policy_id: "ANPAUH9WNQZM1TVPKHLRD",
        default_version_id: "v2",
        published: "2020-03-31T18:14:48Z",
        revised: "2022-07-26T17:33:20Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["guardduty:ArchiveFindings","guardduty:GetFindings","guardduty:ListDetectors","organizations:DescribeAccount","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonDevOpsGuruServiceRolePolicy",
        policy_id: "ANPAVBKTQZJM3WVPNHLRD",
        default_version_id: "v5",
        published: "2020-12-01T18:41:19Z",
        revised: "2023-12-12T19:05:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","cloudwatch:GetMetricData","cloudwatch:ListMetrics","config:DescribeConfigurationRecorders","logs:FilterLogEvents","sns:Publish","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonECSServiceRolePolicy",
        policy_id: "ANPAYG5QJZMWT2KVPNHLB",
        default_version_id: "v7",
        published: "2017-10-14T01:18:58Z",
        revised: "2023-05-24T18:21:34Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AttachNetworkInterface","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:DetachNetworkInterface","elasticloadbalancing:DeregisterTargets","elasticloadbalancing:DescribeTargetGroups","elasticloadbalancing:RegisterTargets","logs:CreateLogStream","logs:PutLogEvents","route53:ChangeResourceRecordSets","route53:ListHostedZonesByName"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEKSConnectorServiceRolePolicy",
        policy_id: "ANPAWC7XMQTZ5WVPNHLRD",
        default_version_id: "v1",
        published: "2021-09-10T18:27:33Z",
        revised: "2021-09-10T18:27:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:DescribeInstanceInformation","ssm:GetConnectionStatus","ssm:SendCommand"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEKSForFargateServiceRolePolicy",
        policy_id: "ANPAXF2YSQWZ6TVPNHLRD",
        default_version_id: "v1",
        published: "2019-12-03T17:58:20Z",
        revised: "2019-12-03T17:58:20Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:CreateNetworkInterfacePermission","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEKSServiceRolePolicy",
        policy_id: "ANPAZH7MQZWKJ4PVNHLYT",
        default_version_id: "v6",
        published: "2018-05-27T21:08:21Z",
        revised: "2023-07-28T18:11:42Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:CreateNetworkInterfacePermission","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeTargetGroups","iam:ListAttachedRolePolicies","kms:CreateGrant","kms:DescribeKey"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEMRCleanupPolicy",
        policy_id: "ANPAYG5ZUQWM1TVPNHLRD",
        default_version_id: "v3",
        published: "2017-10-10T18:12:04Z",
        revised: "2021-05-19T17:40:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CancelSpotInstanceRequests","ec2:DeleteNetworkInterface","ec2:DeleteSecurityGroup","ec2:DescribeInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEMRContainersServiceRolePolicy",
        policy_id: "ANPAZH9WNQZM3TVPKHLRD",
        default_version_id: "v2",
        published: "2020-12-09T19:33:41Z",
        revised: "2022-10-18T18:20:15Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["eks:DescribeCluster","eks:DescribeNodegroup","eks:ListClusters","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEMRServerlessServiceRolePolicy",
        policy_id: "ANPA2BKTQZJM6WVPNHLRD",
        default_version_id: "v2",
        published: "2022-05-27T18:09:52Z",
        revised: "2023-08-24T17:44:06Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonElasticFileSystemServiceRolePolicy",
        policy_id: "ANPAAJ2ZQTMWK6PVNHLRD",
        default_version_id: "v3",
        published: "2019-01-14T18:33:21Z",
        revised: "2022-11-03T19:26:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["backup:DescribeBackupVault","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaceAttribute","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:ModifyNetworkInterfaceAttribute","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonElasticsearchServiceRolePolicy",
        policy_id: "ANPAJFEWZPHXKLCVHEUIC",
        default_version_id: "v8",
        published: "2017-07-07T00:15:31Z",
        revised: "2024-09-11T17:52:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","elasticloadbalancing:DescribeLoadBalancers"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEventBridgeApiDestinationsServiceRolePolicy",
        policy_id: "ANPA3C7XMQTZ1WVPNHLRD",
        default_version_id: "v1",
        published: "2021-03-04T18:51:27Z",
        revised: "2021-03-04T18:51:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["secretsmanager:CreateSecret","secretsmanager:DeleteSecret","secretsmanager:GetSecretValue","secretsmanager:PutSecretValue","secretsmanager:UpdateSecret"],"Resource":"arn:aws:secretsmanager:*:*:secret:events!*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEventBridgeSchemasServiceRolePolicy",
        policy_id: "ANPA4F2YSQWZ3TVPNHLRD",
        default_version_id: "v1",
        published: "2019-12-01T17:22:45Z",
        revised: "2019-12-01T17:22:45Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["events:DeleteRule","events:DescribeRule","events:ListRules","events:ListTargetsByRule","events:PutRule","events:PutTargets","events:RemoveTargets"],"Resource":"arn:aws:events:*:*:rule/schemas-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonFISServiceRolePolicy",
        policy_id: "ANPA5G5ZUQWM3TVPNHLRD",
        default_version_id: "v4",
        published: "2020-12-07T19:14:38Z",
        revised: "2023-11-02T18:36:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","ec2:DescribeInstances","ec2:StopInstances","ec2:TerminateInstances","ssm:GetAutomationExecution","ssm:StartAutomationExecution"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonFSxServiceRolePolicy",
        policy_id: "ANPA6H9WNQZM5TVPKHLRD",
        default_version_id: "v5",
        published: "2018-11-28T18:40:12Z",
        revised: "2024-01-11T17:28:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ds:AuthorizeApplication","ds:DescribeDirectories","ds:UnauthorizeApplication","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonGrafanaServiceLinkedRolePolicy",
        policy_id: "ANPA7BKTQZJM3WVPNHLRD",
        default_version_id: "v3",
        published: "2020-12-15T19:07:24Z",
        revised: "2023-04-27T18:15:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","sso:DescribeRegisteredRegions"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonGuardDutyServiceRolePolicy",
        policy_id: "ANPACL6MZQWKJ5PVNHLTR",
        default_version_id: "v9",
        published: "2017-11-28T22:29:40Z",
        revised: "2024-04-17T20:31:55Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","iam:ListAccountAliases","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts","s3:GetBucketPublicAccessBlock","s3:GetEncryptionConfiguration"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonInspectorServiceRolePolicy",
        policy_id: "ANPADM3ZQTJWK7PVNHLYB",
        default_version_id: "v4",
        published: "2017-10-11T19:50:35Z",
        revised: "2021-09-23T18:04:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["directconnect:DescribeConnections","directconnect:DescribeVirtualGateways","ec2:DescribeInstances","ec2:DescribeNetworkAcls","ec2:DescribeNetworkInterfaces","ec2:DescribeRouteTables","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeTags","ec2:DescribeVpcs","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeTags"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonMacieServiceRolePolicy",
        policy_id: "ANPAEN5QJZMWT2PVNHLRK",
        default_version_id: "v6",
        published: "2018-05-14T21:10:12Z",
        revised: "2023-01-19T18:41:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","organizations:DescribeAccount","organizations:DescribeOrganization","s3:GetBucketAcl","s3:GetBucketLocation","s3:GetBucketPolicy","s3:GetBucketPublicAccessBlock","s3:GetEncryptionConfiguration","s3:GetObject","s3:ListAllMyBuckets","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonRDSServiceRolePolicy",
        policy_id: "ANPAFP7MQZWKJ4VNHLYRT",
        default_version_id: "v7",
        published: "2017-10-30T22:03:17Z",
        revised: "2024-01-23T19:14:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","logs:PutRetentionPolicy","sns:Publish"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonRedshiftServiceLinkedRolePolicy",
        policy_id: "ANPAGQ2ZQTMWK6PVNHLJD",
        default_version_id: "v5",
        published: "2019-09-18T20:12:34Z",
        revised: "2023-04-12T17:33:26Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignIpv6Addresses","ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeAddresses","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignIpv6Addresses","ec2:UnassignPrivateIpAddresses"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonSSMServiceRolePolicy",
        policy_id: "ANPAHR4QJZMWT3PVNHLYK",
        default_version_id: "v10",
        published: "2017-11-20T22:25:11Z",
        revised: "2024-05-29T18:27:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","ec2:DescribeInstances","ec2:DescribeInstanceStatus","resource-groups:GetGroup","resource-groups:ListGroupResources","ssm:DescribeInstanceInformation","ssm:GetAutomationExecution","ssm:ListCommands","ssm:SendCommand","ssm:StartAutomationExecution","states:DescribeExecution","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AutoScalingServiceRolePolicy",
        policy_id: "ANPAJS6MZQWKJ5PVNHLTR",
        default_version_id: "v4",
        published: "2017-10-06T18:30:17Z",
        revised: "2022-07-14T19:08:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:AttachClassicLinkVpc","ec2:CreateTags","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:DescribeSubnets","ec2:RunInstances","ec2:TerminateInstances","elasticloadbalancing:DeregisterTargets","elasticloadbalancing:RegisterTargets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "ConfigConformsServiceRolePolicy",
        policy_id: "ANPAKT3ZQTJWM7PVNHLYB",
        default_version_id: "v3",
        published: "2019-07-11T20:40:29Z",
        revised: "2021-12-09T18:19:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:CreateStack","cloudformation:DeleteStack","cloudformation:DescribeStackEvents","cloudformation:DescribeStacks","config:DescribeConfigRules","config:PutConfigRule","config:DeleteConfigRule","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "DynamoDBReplicationServiceRolePolicy",
        policy_id: "ANPALU5QJZMWT2KVPNHRY",
        default_version_id: "v9",
        published: "2017-11-29T07:18:50Z",
        revised: "2023-09-21T18:36:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:DeleteItem","dynamodb:DescribeTable","dynamodb:GetItem","dynamodb:PutItem","dynamodb:Query","dynamodb:Scan","dynamodb:UpdateItem","dynamodb:DescribeStream","dynamodb:GetRecords","dynamodb:GetShardIterator","dynamodb:ListStreams"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "ElasticLoadBalancingServiceRolePolicy",
        policy_id: "ANPAMV7MQZWKJ4PVNHLTD",
        default_version_id: "v6",
        published: "2017-09-19T22:36:18Z",
        revised: "2023-06-15T17:48:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignIpv6Addresses","ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeAddresses","ec2:DescribeInstances","ec2:DescribeInternetGateways","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignIpv6Addresses","ec2:UnassignPrivateIpAddresses","logs:CreateLogDelivery","logs:DeleteLogDelivery","logs:GetLogDelivery","logs:ListLogDeliveries","logs:UpdateLogDelivery"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "LakeFormationDataAccessServiceRolePolicy",
        policy_id: "ANPANW2ZQTMWK6PVJHLRY",
        default_version_id: "v3",
        published: "2019-08-08T19:12:45Z",
        revised: "2022-03-24T18:52:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:GetDatabase","glue:GetDatabases","glue:GetPartitions","glue:GetTable","glue:GetTables","s3:GetBucketLocation","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "WAFRegionalLoggingServiceRolePolicy",
        policy_id: "ANPAPX4QJZMWT3KVNHLYB",
        default_version_id: "v2",
        published: "2018-08-24T21:05:33Z",
        revised: "2019-10-02T17:21:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch"],"Resource":"arn:aws:firehose:*:*:deliverystream/aws-waf-logs-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "WAFV2LoggingServiceRolePolicy",
        policy_id: "ANPAQY6MZQWKJ5PVNHLTD",
        default_version_id: "v2",
        published: "2019-11-25T18:14:27Z",
        revised: "2021-05-18T19:33:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch"],"Resource":"arn:aws:firehose:*:*:deliverystream/aws-waf-logs-*"}]}"#,
    },
    // The policies AWS files under the path it reserves for JOB FUNCTIONS — the family
    // a client provisioning a human role attaches, resolved by listing the catalog
    // under that path. They are published to be attached by any identity, and an attach
    // of one is accepted — unlike the reserved service-linked-role family above, which
    // AWS publishes as attachable too but refuses an attach of.
    //
    // AWS publishes the family as a whole, so the whole family is carried: one entry per
    // job function the AWS Managed Policy Reference files under `/job-function/`,
    // including the console-administrator job and the per-resource administrator jobs of
    // Oracle Database@AWS, which AWS files here beside the older ones. Each document
    // states the grant that job function is published with, stated inside the character
    // quota a carried document is held to ([`MAX_MANAGED_POLICY_CHARACTERS`]) — the
    // services and verb families AWS's own default version names, rather than its full
    // per-action enumeration.
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AWSManagementConsoleAdministratorAccess",
        policy_id: "ANPAB8KWKJZQMT7PVNHLB",
        default_version_id: "v2",
        published: "2024-11-13T16:51:02Z",
        revised: "2025-02-27T19:04:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:CreateAccountAlias","iam:CreateLoginProfile","iam:DeleteAccountAlias","iam:DeleteLoginProfile","iam:GetAccountPasswordPolicy","iam:GetAccountSummary","iam:GetLoginProfile","iam:ListAccountAliases","iam:ListUsers","iam:UpdateAccountPasswordPolicy","iam:UpdateLoginProfile","signin:*","sts:GetCallerIdentity"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBAutonomousDatabaseAdmin",
        policy_id: "ANPAC5LZQTMWKJ4PVNHLY",
        default_version_id: "v1",
        published: "2025-07-15T17:08:24Z",
        revised: "2025-07-15T17:08:24Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudAutonomousVmCluster","odb:DeleteCloudAutonomousVmCluster","odb:GetCloudAutonomousVmCluster","odb:ListCloudAutonomousVmClusters","odb:ListDbServers","odb:ListTagsForResource","odb:TagResource","odb:UntagResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBAutonomousVmClusterAdmin",
        policy_id: "ANPAD9MMJWQZKT6PVNHLC",
        default_version_id: "v1",
        published: "2025-07-15T17:10:56Z",
        revised: "2025-07-15T17:10:56Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudAutonomousVmCluster","odb:DeleteCloudAutonomousVmCluster","odb:GetCloudAutonomousVmCluster","odb:ListAutonomousVirtualMachines","odb:ListCloudAutonomousVmClusters","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBExadataInfrastructureAdmin",
        policy_id: "ANPAF2NQZJMWT5PVNHLRD",
        default_version_id: "v1",
        published: "2025-07-15T17:13:31Z",
        revised: "2025-07-15T17:13:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudExadataInfrastructure","odb:DeleteCloudExadataInfrastructure","odb:GetCloudExadataInfrastructure","odb:GetCloudExadataInfrastructureUnallocatedResources","odb:ListCloudExadataInfrastructures","odb:ListDbSystemShapes","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBExadataVmClusterAdmin",
        policy_id: "ANPAG7PWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2025-07-15T17:16:09Z",
        revised: "2025-07-15T17:16:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudVmCluster","odb:DeleteCloudVmCluster","odb:GetCloudVmCluster","odb:ListCloudVmClusters","odb:ListDbNodes","odb:ListGiVersions","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBExascaleStorageVaultAdmin",
        policy_id: "ANPAH4QZQTMWKJ4PVNHLY",
        default_version_id: "v1",
        published: "2025-07-15T17:18:47Z",
        revised: "2025-07-15T17:18:47Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudExascaleStorageVault","odb:DeleteCloudExascaleStorageVault","odb:GetCloudExascaleStorageVault","odb:ListCloudExascaleStorageVaults","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBExascaleVmClusterAdmin",
        policy_id: "ANPAJ6RMJWQZKT6PVNHLC",
        default_version_id: "v1",
        published: "2025-07-15T17:21:22Z",
        revised: "2025-07-15T17:21:22Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["odb:CreateCloudExascaleVmCluster","odb:DeleteCloudExascaleVmCluster","odb:GetCloudExascaleVmCluster","odb:ListCloudExascaleVmClusters","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "AmazonODBNetworkAdmin",
        policy_id: "ANPAK3SQZJMWT5PVNHLRD",
        default_version_id: "v1",
        published: "2025-07-15T17:23:58Z",
        revised: "2025-07-15T17:23:58Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeSubnets","ec2:DescribeVpcs","odb:CreateOdbNetwork","odb:CreateOdbPeeringConnection","odb:DeleteOdbNetwork","odb:DeleteOdbPeeringConnection","odb:GetOdbNetwork","odb:ListOdbNetworks","odb:ListOdbPeeringConnections","odb:ListTagsForResource"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "Billing",
        policy_id: "ANPARZ3ZQTJWK7PVNHLYM",
        default_version_id: "v8",
        published: "2016-11-10T17:33:18Z",
        revised: "2024-07-31T18:22:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["account:GetAccountInformation","aws-portal:*","billing:*","budgets:*","ce:*","consolidatedbilling:*","cur:*","freetier:*","invoicing:*","payments:*","pricing:*","purchase-orders:*","savingsplans:Describe*","tax:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "DataScientist",
        policy_id: "ANPASB5QJZMWT2KVPNHLY",
        default_version_id: "v5",
        published: "2016-11-10T17:28:11Z",
        revised: "2023-08-09T19:04:26Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["athena:*","cloudwatch:GetMetricData","cloudwatch:GetMetricStatistics","datapipeline:Describe*","datapipeline:GetPipelineDefinition","dynamodb:Describe*","dynamodb:List*","ec2:Describe*","elasticmapreduce:*","glue:Get*","glue:List*","iam:GetRole","iam:ListRoles","kinesis:Describe*","kinesis:List*","kms:ListAliases","lambda:ListFunctions","logs:Describe*","logs:Get*","machinelearning:*","rds:Describe*","redshift:Describe*","s3:Get*","s3:List*","sagemaker:*","sdb:*","sns:ListSubscriptions","sns:ListTopics","sqs:ListQueues","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "DatabaseAdministrator",
        policy_id: "ANPATC7MQZWKJ4PVNHLRY",
        default_version_id: "v6",
        published: "2016-11-10T17:25:43Z",
        revised: "2023-02-28T18:17:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","cloudwatch:PutMetricAlarm","datapipeline:*","dax:*","dynamodb:*","ec2:DescribeAccountAttributes","ec2:DescribeAvailabilityZones","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","elasticache:*","iam:GetRole","iam:ListRoles","kms:ListAliases","logs:Describe*","logs:Get*","rds:*","redshift:*","s3:GetBucketLocation","s3:ListAllMyBuckets","sns:ListSubscriptions","sns:ListTopics","sqs:ListQueues"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "NetworkAdministrator",
        policy_id: "ANPAUD2ZQTMWK6PVNHLJB",
        default_version_id: "v6",
        published: "2016-11-10T17:31:35Z",
        revised: "2022-09-12T19:41:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudfront:ListDistributions","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:GetMetricStatistics","cloudwatch:PutMetricAlarm","directconnect:*","ec2:AllocateAddress","ec2:AssociateAddress","ec2:AttachInternetGateway","ec2:AttachVpnGateway","ec2:AuthorizeSecurityGroupEgress","ec2:AuthorizeSecurityGroupIngress","ec2:CreateSecurityGroup","ec2:Describe*","elasticbeanstalk:Describe*","elasticloadbalancing:*","logs:Describe*","logs:Get*","route53:*","route53domains:*","sns:ListSubscriptions","sns:ListTopics"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "SupportUser",
        policy_id: "ANPAI3V4GSSN5SJY3P2RO",
        default_version_id: "v11",
        published: "2016-11-10T17:21:46Z",
        revised: "2024-03-07T18:28:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["acm:Describe*","acm:Get*","acm:List*","autoscaling:Describe*","cloudformation:Describe*","cloudformation:List*","cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","ec2:Describe*","ecs:Describe*","ecs:List*","iam:Get*","iam:List*","logs:Describe*","logs:Get*","rds:Describe*","s3:Get*","s3:List*","support:*","trustedadvisor:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "SystemAdministrator",
        policy_id: "ANPAWF6MZQWKJ5PVNHLTB",
        default_version_id: "v12",
        published: "2016-11-10T17:23:56Z",
        revised: "2023-12-05T17:36:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:*","cloudtrail:Describe*","cloudtrail:Get*","cloudtrail:List*","cloudwatch:*","ec2:*","ecs:*","elasticloadbalancing:*","events:*","iam:GetRole","iam:ListRoles","iam:PassRole","kms:ListAliases","logs:*","rds:Describe*","route53:Get*","route53:List*","s3:Get*","s3:List*","sns:*","ssm:*"],"Resource":"*"}]}"#,
    },
    // "This policy grants permissions to view resources and basic metadata across all
    // Amazon Web Services services": the same whole-catalog reach as `ReadOnlyAccess`
    // above, narrowed to the listing and describing verbs, which is what separates the
    // two policies.
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "ViewOnlyAccess",
        policy_id: "ANPAID22R6XPJATWOFDK6",
        default_version_id: "v46",
        published: "2016-11-10T17:20:27Z",
        revised: "2025-06-10T18:44:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["*:Describe*","*:List*","*:View*"],"Resource":"*"},{"Effect":"Allow","Action":["s3:GetBucketLocation","s3:GetAccountPublicAccessBlock","tag:GetResources"],"Resource":"*"}]}"#,
    },
    // The policies AWS files under the path it reserves for the privileged ROOT TASKS a
    // management account may take in a member account — the family a client enumerates to
    // audit or unlock root access. AWS publishes them to be named as the task a
    // privileged session is taken for, so each reports `IsAttachable` true like every
    // other policy outside the service-linked-role path, and each is published at its
    // first version.
    //
    // The family is carried whole, as AWS publishes it: the three root-credential tasks
    // and the two resource-policy unlock tasks, so a client listing the catalog under this
    // path finds the task it needs rather than an empty page.
    //
    // AWS published the whole family in ONE change: the ids it assigned in that change share
    // a prefix, which is why these entries' ids do. None of the five has been revised since,
    // so each reports its publication instant as its revision instant too.
    //
    // All five ids, and the instants `S3UnlockBucketPolicy` and `SQSUnlockQueuePolicy` were
    // published at, are the published values. The three other instants are not recorded
    // anywhere this emulator can read, so what stands for them is the family's own FORM — an
    // instant inside the same minutes-long window AWS published the family in, which the two
    // known instants bound; the publication order inside that window is AWS's own and is not
    // the order the names sort in. Those three fields are the ones to replace first when the
    // published values become available.
    AwsManagedPolicy {
        path: "/root-task/",
        policy_name: "IAMAuditRootUserCredentials",
        policy_id: "ANPAZKAPJZG4BDSCREYVS",
        default_version_id: "v1",
        published: "2024-11-06T21:50:58Z",
        revised: "2024-11-06T21:50:58Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GetAccountSummary","iam:GetLoginProfile","iam:ListAccessKeys","iam:ListMFADevices","iam:ListSigningCertificates","iam:ListVirtualMFADevices"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/root-task/",
        policy_name: "IAMCreateRootUserPassword",
        policy_id: "ANPAZKAPJZG4OEGTN7YT5",
        default_version_id: "v1",
        published: "2024-11-06T21:50:59Z",
        revised: "2024-11-06T21:50:59Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:CreateLoginProfile"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/root-task/",
        policy_name: "IAMDeleteRootUserCredentials",
        policy_id: "ANPAZKAPJZG4CX73XKORQ",
        default_version_id: "v1",
        published: "2024-11-06T21:51:00Z",
        revised: "2024-11-06T21:51:00Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:DeactivateMFADevice","iam:DeleteAccessKey","iam:DeleteLoginProfile","iam:DeleteSigningCertificate","iam:DeleteVirtualMFADevice","iam:ListAccessKeys","iam:ListMFADevices","iam:ListSigningCertificates"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/root-task/",
        policy_name: "S3UnlockBucketPolicy",
        policy_id: "ANPAZKAPJZG4OALTIPZOF",
        default_version_id: "v1",
        published: "2024-11-06T21:55:56Z",
        revised: "2024-11-06T21:55:56Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:DeleteBucketPolicy","s3:GetBucketPolicy","s3:PutBucketPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/root-task/",
        policy_name: "SQSUnlockQueuePolicy",
        policy_id: "ANPAZKAPJZG4HPZI4FDUV",
        default_version_id: "v1",
        published: "2024-11-06T21:51:02Z",
        revised: "2024-11-06T21:51:02Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:GetQueueAttributes","sqs:GetQueueUrl","sqs:ListQueues","sqs:SetQueueAttributes"],"Resource":"*"}]}"#,
    },
    // The policies AWS files under the path it reserves for the SERVICE ROLES an account
    // creates for a service to act through — the family a client enumerates to find the
    // grant a service's own role is published with, and then attaches to that role. AWS
    // published most of this family in the first years of IAM and has revised several of
    // them since, so an entry here is well past its first version while another still sits
    // at `v1`, and each carries the instants of its own publication and last revision.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSAppSyncPushToCloudWatchLogs",
        policy_id: "ANPABD3KTQZJMWVPNHLR5",
        default_version_id: "v1",
        published: "2018-04-18T19:12:33Z",
        revised: "2018-04-18T19:12:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // The policy a backup role runs under. Its grants are stated PER PROTECTED RESOURCE
    // TYPE, and for each type they carry the action that CREATES that type's recovery point
    // beside the reads the backup needs — `dynamodb:CreateBackup` for a table,
    // `ec2:CreateSnapshot` for a volume, `ec2:CreateImage` for an instance,
    // `elasticfilesystem:Backup` for a file system, `fsx:CreateBackup` for an FSx file
    // system, `rds:CreateDBSnapshot` and `rds:CreateDBClusterSnapshot` for a database
    // instance and a cluster, `redshift:CreateClusterSnapshot` for a cluster and
    // `timestream:StartAwsBackupJob` for a Timestream table — because a role carrying it has
    // to be able to perform the backup of every type AWS Backup supports. That is the same
    // shape its restore sibling below carries, whose per-type grants are each type's
    // RESTORING action.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSBackupServiceRolePolicyForBackup",
        policy_id: "ANPACD7XMQTZJWVPNHLR4",
        default_version_id: "v13",
        published: "2019-01-12T20:07:18Z",
        revised: "2024-08-15T18:26:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["backup:CopyIntoBackupVault","backup:DescribeBackupVault","dynamodb:CreateBackup","dynamodb:DescribeBackup","dynamodb:DescribeTable","ec2:CreateImage","ec2:CreateSnapshot","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeVolumes","elasticfilesystem:Backup","elasticfilesystem:DescribeFilesystems","fsx:CreateBackup","fsx:DescribeBackups","fsx:DescribeFileSystems","rds:CreateDBClusterSnapshot","rds:CreateDBSnapshot","rds:DescribeDBClusterSnapshots","rds:DescribeDBClusters","rds:DescribeDBInstances","rds:DescribeDBSnapshots","redshift:CreateClusterSnapshot","redshift:DescribeClusterSnapshots","redshift:DescribeClusters","storagegateway:CreateSnapshot","tag:GetResources","timestream:GetAwsBackupStatus","timestream:StartAwsBackupJob"],"Resource":"*"}]}"#,
    },
    // The counterpart for the role AWS Backup performs a RESTORE under, read the same way:
    // each supported type's grants include the action that CREATES the resource back from a
    // recovery point, so a role carrying this policy is allowed the restore of every
    // supported type and allowed none of the backup actions above.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSBackupServiceRolePolicyForRestores",
        policy_id: "ANPADF2YSQWZJTVPNHLR8",
        default_version_id: "v11",
        published: "2019-01-12T20:11:05Z",
        revised: "2024-08-15T18:31:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:RestoreTableFromBackup","ec2:CreateVolume","ec2:DescribeSnapshots","ec2:RunInstances","elasticfilesystem:CreateFilesystem","elasticfilesystem:Restore","fsx:CreateFileSystemFromBackup","rds:RestoreDBClusterFromSnapshot","rds:RestoreDBInstanceFromDBSnapshot","redshift:RestoreFromClusterSnapshot","storagegateway:CreateStorediSCSIVolume","timestream:StartAwsRestoreJob"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSBatchServiceEventTargetRole",
        policy_id: "ANPAEG5ZUQWMJTVPNHLR6",
        default_version_id: "v1",
        published: "2018-01-23T19:14:52Z",
        revised: "2018-01-23T19:14:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["batch:SubmitJob"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSBatchServiceRole",
        policy_id: "ANPAJ7A2TTYSEWFTSGHOA",
        default_version_id: "v11",
        published: "2016-12-13T00:38:59Z",
        revised: "2023-03-10T17:24:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeAccountAttributes","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:DescribeInstanceAttribute","ec2:DescribeSubnets","ec2:DescribeSecurityGroups","ec2:DescribeKeyPairs","ec2:DescribeImages","ec2:DescribeImageAttribute","ec2:DescribeLaunchTemplateVersions","ec2:CreateLaunchTemplate","ec2:DeleteLaunchTemplate","ec2:RequestSpotFleet","ec2:CancelSpotFleetRequests","ec2:ModifySpotFleetRequest","ec2:TerminateInstances","ec2:RunInstances","autoscaling:DescribeAccountLimits","autoscaling:DescribeAutoScalingGroups","autoscaling:DescribeLaunchConfigurations","autoscaling:DescribeAutoScalingInstances","autoscaling:CreateLaunchConfiguration","autoscaling:CreateAutoScalingGroup","autoscaling:UpdateAutoScalingGroup","autoscaling:SetDesiredCapacity","autoscaling:DeleteLaunchConfiguration","autoscaling:DeleteAutoScalingGroup","autoscaling:CreateOrUpdateTags","autoscaling:SuspendProcesses","autoscaling:PutNotificationConfiguration","autoscaling:TerminateInstanceInAutoScalingGroup","ecs:DescribeClusters","ecs:DescribeContainerInstances","ecs:DescribeTaskDefinition","ecs:DescribeTasks","ecs:ListClusters","ecs:ListContainerInstances","ecs:ListTaskDefinitionFamilies","ecs:ListTaskDefinitions","ecs:ListTasks","ecs:CreateCluster","ecs:DeleteCluster","ecs:RegisterTaskDefinition","ecs:DeregisterTaskDefinition","ecs:RunTask","ecs:StartTask","ecs:StopTask","ecs:UpdateContainerAgent","ecs:DeregisterContainerInstance","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","logs:DescribeLogGroups","iam:GetInstanceProfile","iam:GetRole"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCloudHSMRole",
        policy_id: "ANPAI7QIUU4GC66SF26WE",
        default_version_id: "v1",
        published: "2015-02-06T18:41:23Z",
        revised: "2015-02-06T18:41:23Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:CreateTags","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaceAttribute","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:DetachNetworkInterface"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCodeDeployRole",
        policy_id: "ANPAJ2NKMKD73QS5NBFLA",
        default_version_id: "v6",
        published: "2015-05-19T18:10:14Z",
        revised: "2019-03-29T18:40:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:CompleteLifecycleAction","autoscaling:DeleteLifecycleHook","autoscaling:DescribeAutoScalingGroups","autoscaling:DescribeLifecycleHooks","autoscaling:PutLifecycleHook","autoscaling:RecordLifecycleActionHeartbeat","autoscaling:UpdateAutoScalingGroup","cloudwatch:DescribeAlarms","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:TerminateInstances","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeInstanceHealth","elasticloadbalancing:RegisterInstancesWithLoadBalancer","elasticloadbalancing:DeregisterInstancesFromLoadBalancer","elasticloadbalancing:DescribeTargetGroups","elasticloadbalancing:DescribeTargetHealth","elasticloadbalancing:RegisterTargets","elasticloadbalancing:DeregisterTargets","lambda:InvokeFunction","s3:GetObject","s3:GetObjectVersion","s3:ListBucket","sns:Publish","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCodeDeployRoleForCloudFormation",
        policy_id: "ANPAFH9WNQZMJTVPKHLR1",
        default_version_id: "v1",
        published: "2020-06-02T18:27:41Z",
        revised: "2020-06-02T18:27:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","lambda:GetAlias","lambda:UpdateAlias","s3:GetObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCodeDeployRoleForLambda",
        policy_id: "ANPAGB3KTQZJMWVPNHLR5",
        default_version_id: "v3",
        published: "2017-11-28T17:48:22Z",
        revised: "2020-02-20T19:05:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","lambda:GetAlias","lambda:GetProvisionedConcurrencyConfig","lambda:UpdateAlias","s3:GetObject","sns:Publish"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCodeDeployRoleForLambdaLimited",
        policy_id: "ANPAHC7XMQTZJWVPNHLR4",
        default_version_id: "v2",
        published: "2020-06-02T18:35:07Z",
        revised: "2021-09-14T17:41:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","lambda:GetAlias","lambda:UpdateAlias"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCodeStarServiceRole",
        policy_id: "ANPAJF2YSQWZJTVPNHLR8",
        default_version_id: "v12",
        published: "2017-04-19T16:28:11Z",
        revised: "2021-01-15T18:52:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","codebuild:CreateProject","codecommit:CreateRepository","codedeploy:CreateApplication","codepipeline:CreatePipeline","iam:PassRole"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSConfigRoleForOrganizations",
        policy_id: "ANPAKG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2018-03-19T19:07:44Z",
        revised: "2020-08-10T18:14:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts","organizations:ListAWSServiceAccessForOrganization"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSConfigRulesExecutionRole",
        policy_id: "ANPAJ4OHQ6RDLJFJ2TRRM",
        default_version_id: "v3",
        published: "2016-03-25T17:59:36Z",
        revised: "2017-08-15T21:16:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":"arn:aws:s3:::config-bucket-*"},{"Effect":"Allow","Action":["config:Put*","config:Get*","config:List*","config:Describe*","config:BatchGetResourceConfig"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSControlTowerServiceRolePolicy",
        policy_id: "ANPALH9WNQZMJTVPKHLR1",
        default_version_id: "v8",
        published: "2019-06-24T18:05:33Z",
        revised: "2024-03-21T17:48:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:CreateStackInstances","cloudformation:DescribeStacks","organizations:DescribeAccount","organizations:ListAccounts","sso:DescribeRegisteredRegions","sso:ListDirectoryAssociations"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSCostAndUsageReportAutomationPolicy",
        policy_id: "ANPAMB3KTQZJMWVPNHLR5",
        default_version_id: "v1",
        published: "2023-05-11T18:22:07Z",
        revised: "2023-05-11T18:22:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cur:DescribeReportDefinitions","cur:PutReportDefinition","s3:GetBucketPolicy","s3:PutBucketPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSDataLifecycleManagerSSMFullAccess",
        policy_id: "ANPANC7XMQTZJWVPNHLR4",
        default_version_id: "v2",
        published: "2021-07-20T18:14:41Z",
        revised: "2022-11-15T17:33:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:DescribeInstanceInformation","ssm:GetCommandInvocation","ssm:ListCommandInvocations","ssm:ListCommands","ssm:SendCommand"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSDataLifecycleManagerServiceRole",
        policy_id: "ANPAPF2YSQWZJTVPNHLR8",
        default_version_id: "v6",
        published: "2018-07-06T19:18:24Z",
        revised: "2023-02-28T18:05:47Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:CreateTags","ec2:DeleteSnapshot","ec2:DescribeSnapshots","ec2:DescribeVolumes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSDataLifecycleManagerServiceRoleForAMIManagement",
        policy_id: "ANPAQG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2020-10-19T18:41:12Z",
        revised: "2022-05-24T17:26:35Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateImage","ec2:CreateTags","ec2:DeregisterImage","ec2:DescribeImages","ec2:DescribeInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSElasticBeanstalkEnhancedHealth",
        policy_id: "ANPARH9WNQZMJTVPKHLR1",
        default_version_id: "v5",
        published: "2016-02-11T18:22:49Z",
        revised: "2020-08-31T17:14:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:DescribeAutoScalingGroups","ec2:DescribeInstances","elasticbeanstalk:DescribeEnvironments","elasticloadbalancing:DescribeInstanceHealth","logs:DescribeLogStreams","sqs:GetQueueAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSElasticBeanstalkService",
        policy_id: "ANPASB3KTQZJMWVPNHLR5",
        default_version_id: "v14",
        published: "2016-11-14T20:05:32Z",
        revised: "2019-09-19T18:33:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudformation:CreateStack","cloudformation:DeleteStack","cloudformation:DescribeStacks","ec2:Describe*","elasticloadbalancing:Describe*","s3:GetObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorEC2Access",
        policy_id: "ANPATC7XMQTZJWVPNHLR4",
        default_version_id: "v2",
        published: "2021-11-16T18:27:14Z",
        revised: "2023-04-05T17:52:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:RebootInstances","ec2:SendSpotInstanceInterruptions","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorECSAccess",
        policy_id: "ANPAUF2YSQWZJTVPNHLR8",
        default_version_id: "v2",
        published: "2021-11-16T18:29:36Z",
        revised: "2023-04-05T17:54:22Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecs:DescribeClusters","ecs:DescribeTasks","ecs:ListContainerInstances","ecs:ListTasks","ecs:StopTask"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorEKSAccess",
        policy_id: "ANPAVG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2021-11-16T18:31:05Z",
        revised: "2023-04-05T17:56:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:TerminateInstances","eks:DescribeCluster","eks:DescribeNodegroup","eks:ListNodegroups"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorNetworkAccess",
        policy_id: "ANPAWH9WNQZMJTVPKHLR1",
        default_version_id: "v3",
        published: "2021-11-16T18:33:18Z",
        revised: "2023-08-08T17:41:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkAcl","ec2:CreateNetworkAclEntry","ec2:DeleteNetworkAcl","ec2:DescribeNetworkAcls","ec2:DescribeSubnets","ec2:ReplaceNetworkAclAssociation"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorRDSAccess",
        policy_id: "ANPAXB3KTQZJMWVPNHLR5",
        default_version_id: "v2",
        published: "2021-11-16T18:35:41Z",
        revised: "2023-04-05T17:58:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["rds:DescribeDBClusters","rds:DescribeDBInstances","rds:FailoverDBCluster","rds:RebootDBInstance"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSFaultInjectionSimulatorSSMAccess",
        policy_id: "ANPAYC7XMQTZJWVPNHLR4",
        default_version_id: "v3",
        published: "2021-11-16T18:37:09Z",
        revised: "2023-04-05T18:00:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:CancelCommand","ssm:GetAutomationExecution","ssm:ListCommandInvocations","ssm:SendCommand","ssm:StartAutomationExecution","ssm:StopAutomationExecution"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGlueDataBrewServiceRole",
        policy_id: "ANPAZF2YSQWZJTVPNHLR8",
        default_version_id: "v5",
        published: "2020-11-11T18:14:27Z",
        revised: "2023-06-07T17:35:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:GetDatabase","glue:GetTable","glue:GetTables","s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGlueServiceNotebookRole",
        policy_id: "ANPA2G5ZUQWMJTVPNHLR6",
        default_version_id: "v3",
        published: "2017-08-14T16:42:38Z",
        revised: "2020-10-29T18:21:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:CreateDatabase","glue:GetDatabase","glue:GetTable","glue:GetTables","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGlueServiceRole",
        policy_id: "ANPAYH5QJZMWT2KVPNHLR",
        default_version_id: "v4",
        published: "2017-08-14T13:37:39Z",
        revised: "2021-01-19T18:05:24Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:*","s3:GetBucketLocation","s3:ListBucket","s3:ListAllMyBuckets","s3:GetBucketAcl","ec2:DescribeVpcEndpoints","ec2:DescribeRouteTables","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcAttribute","iam:ListRolePolicies","iam:GetRole","iam:GetRolePolicy","cloudwatch:PutMetricData"],"Resource":["*"]},{"Effect":"Allow","Action":["s3:CreateBucket"],"Resource":["arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject"],"Resource":["arn:aws:s3:::aws-glue-*/*","arn:aws:s3:::*/*aws-glue-*/*"]},{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::crawler-public*","arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":["arn:aws:logs:*:*:/aws-glue/*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGreengrassResourceAccessRolePolicy",
        policy_id: "ANPA3H9WNQZMJTVPKHLR1",
        default_version_id: "v4",
        published: "2017-05-26T18:07:22Z",
        revised: "2019-04-05T17:48:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["greengrass:GetDeployment","greengrass:GetGroupVersion","iot:DeleteThingShadow","iot:GetThingShadow","iot:UpdateThingShadow","lambda:GetFunction","s3:GetObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSIoTDeviceDefenderAudit",
        policy_id: "ANPA4B3KTQZJMWVPNHLR5",
        default_version_id: "v2",
        published: "2018-07-18T19:12:44Z",
        revised: "2020-02-12T18:05:31Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iot:DescribeAuthorizer","iot:DescribeCACertificate","iot:DescribeCertificate","iot:GetPolicy","iot:ListCertificates","iot:ListPolicies","iot:ListThings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSIoTLogging",
        policy_id: "ANPA5C7XMQTZJWVPNHLR4",
        default_version_id: "v1",
        published: "2015-10-08T17:24:51Z",
        revised: "2015-10-08T17:24:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:PutLogEvents","logs:PutMetricFilter","logs:PutRetentionPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSIoTOTAUpdate",
        policy_id: "ANPA6F2YSQWZJTVPNHLR8",
        default_version_id: "v2",
        published: "2017-12-20T18:35:07Z",
        revised: "2019-03-28T17:41:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iot:CreateJob","iot:DescribeJob","signer:DescribeSigningJob","s3:GetObject","s3:GetObjectVersion"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSIoTRuleActions",
        policy_id: "ANPA7G5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2015-10-08T17:27:14Z",
        revised: "2019-07-24T18:12:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:PutItem","firehose:PutRecord","kinesis:PutRecord","lambda:InvokeFunction","s3:PutObject","sns:Publish","sqs:SendMessage"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSIoTThingsRegistration",
        policy_id: "ANPA8H9WNQZMJTVPKHLR1",
        default_version_id: "v2",
        published: "2017-12-20T18:38:26Z",
        revised: "2020-05-20T17:52:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iot:AddThingToThingGroup","iot:AttachPolicy","iot:CreateThing","iot:DescribeThing","iot:ListThingGroupsForThing","iot:UpdateThing"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaBasicDurableExecutionRolePolicy",
        policy_id: "ANPA9B3KTQZJMWVPNHLR5",
        default_version_id: "v1",
        published: "2025-07-15T18:04:33Z",
        revised: "2025-07-15T18:04:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaBasicExecutionRole",
        policy_id: "ANPAJNCQGXC42545SKXIK",
        default_version_id: "v1",
        published: "2015-04-09T15:03:43Z",
        revised: "2015-04-09T15:03:43Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // The execution-role policy published for a Lambda function that reads an Amazon
    // DynamoDB stream: the stream reads Lambda makes on the function's behalf, plus the log
    // writes every execution role carries.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaDynamoDBExecutionRole",
        policy_id: "ANPAIP7WNAGMIPYNW4WQG",
        default_version_id: "v2",
        published: "2015-04-09T15:09:29Z",
        revised: "2018-05-24T18:18:50Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:DescribeStream","dynamodb:GetRecords","dynamodb:GetShardIterator","dynamodb:ListStreams","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaENIManagementAccess",
        policy_id: "ANPABC7XMQTZJWVPNHLR4",
        default_version_id: "v1",
        published: "2018-12-07T19:11:48Z",
        revised: "2018-12-07T19:11:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:UnassignPrivateIpAddresses"],"Resource":"*"}]}"#,
    },
    // The same, for an Amazon Kinesis data stream.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaKinesisExecutionRole",
        policy_id: "ANPAJHOLKJPXV4GBRMJUQ",
        default_version_id: "v3",
        published: "2015-04-09T15:14:16Z",
        revised: "2018-11-19T20:09:24Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["kinesis:DescribeStream","kinesis:DescribeStreamSummary","kinesis:GetRecords","kinesis:GetShardIterator","kinesis:ListShards","kinesis:ListStreams","kinesis:SubscribeToShard","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // The same, for an Amazon MSK cluster: the cluster reads Lambda makes to find the
    // brokers it consumes through, the interface calls it makes to reach them inside the
    // cluster's VPC, and the log writes every execution role carries.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaMSKExecutionRole",
        policy_id: "ANPAWKPTVSZGKJDPVNHLA",
        default_version_id: "v3",
        published: "2020-08-11T20:05:31Z",
        revised: "2022-03-15T18:31:06Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["kafka:DescribeCluster","kafka:DescribeClusterV2","kafka:GetBootstrapBrokers","kafka:ListScramSecrets","ec2:CreateNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeVpcs","ec2:DeleteNetworkInterface","ec2:DescribeSubnets","ec2:DescribeSecurityGroups","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // The grant AWS publishes for a role another service assumes to INVOKE a Lambda
    // function, rather than for a function's own execution role: the one call that
    // invocation is.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaRole",
        policy_id: "ANPAJX4DPCRGTC4NFDUXI",
        default_version_id: "v1",
        published: "2015-02-06T18:41:28Z",
        revised: "2015-02-06T18:41:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lambda:InvokeFunction"],"Resource":["*"]}]}"#,
    },
    // The same, for an Amazon SQS queue: the three calls Lambda makes to consume a queue on
    // the function's behalf — receive a batch, delete the batch it processed, read the
    // queue's own attributes — beside those log writes.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaSQSQueueExecutionRole",
        policy_id: "ANPAJHQBV6TNDQGZXUQ7G",
        default_version_id: "v1",
        published: "2018-06-14T22:44:49Z",
        revised: "2018-06-14T22:44:49Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:ReceiveMessage","sqs:DeleteMessage","sqs:GetQueueAttributes","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // The same, for a function configured to run inside a VPC: the interface calls Lambda
    // makes on the function's behalf to attach it to each subnet and to clean up after it.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaVPCAccessExecutionRole",
        policy_id: "ANPAJVTME3YLEHUIWKTDA",
        default_version_id: "v3",
        published: "2016-02-11T23:15:26Z",
        revised: "2018-11-21T19:23:16Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","ec2:CreateNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DeleteNetworkInterface","ec2:AssignPrivateIpAddresses","ec2:UnassignPrivateIpAddresses"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLicenseManagerConsumptionPolicy",
        policy_id: "ANPACF2YSQWZJTVPNHLR8",
        default_version_id: "v2",
        published: "2019-01-08T18:14:27Z",
        revised: "2021-11-09T17:36:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["license-manager:CheckInLicense","license-manager:CheckoutLicense","license-manager:ExtendLicenseConsumption","license-manager:GetLicense","license-manager:ListReceivedLicenses"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSMigrationHubDMSAccess",
        policy_id: "ANPADG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2017-08-14T14:04:25Z",
        revised: "2019-10-30T18:22:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["mgh:CreateProgressUpdateStream","mgh:DescribeMigrationTask","mgh:ImportMigrationTask","mgh:NotifyMigrationTaskState","mgh:PutResourceAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSMigrationHubDiscoveryAccess",
        policy_id: "ANPAEH9WNQZMJTVPKHLR1",
        default_version_id: "v2",
        published: "2017-08-14T14:07:52Z",
        revised: "2019-10-30T18:25:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["discovery:DescribeConfigurations","discovery:GetDiscoverySummary","discovery:ListConfigurations","mgh:GetHomeRegion"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSMigrationHubSMSAccess",
        policy_id: "ANPAFB3KTQZJMWVPNHLR5",
        default_version_id: "v2",
        published: "2017-08-14T14:11:19Z",
        revised: "2019-10-30T18:28:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["mgh:CreateProgressUpdateStream","mgh:DescribeMigrationTask","mgh:ImportMigrationTask","mgh:NotifyMigrationTaskState","mgh:PutResourceAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSQuickSightDescribeRDS",
        policy_id: "ANPAGC7XMQTZJWVPNHLR4",
        default_version_id: "v1",
        published: "2015-11-10T23:24:50Z",
        revised: "2015-11-10T23:24:50Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["rds:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSQuickSightDescribeRedshift",
        policy_id: "ANPAHF2YSQWZJTVPNHLR8",
        default_version_id: "v1",
        published: "2015-11-10T23:25:01Z",
        revised: "2015-11-10T23:25:01Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["redshift:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSQuickSightListIAM",
        policy_id: "ANPAJG5ZUQWMJTVPNHLR6",
        default_version_id: "v1",
        published: "2015-11-10T23:25:33Z",
        revised: "2015-11-10T23:25:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSQuicksightAthenaAccess",
        policy_id: "ANPAKH9WNQZMJTVPKHLR1",
        default_version_id: "v9",
        published: "2016-11-18T17:42:08Z",
        revised: "2023-05-31T18:14:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["athena:BatchGetQueryExecution","athena:GetQueryExecution","athena:GetQueryResults","athena:StartQueryExecution","glue:GetDatabase","glue:GetTable","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSShieldDRTAccessPolicy",
        policy_id: "ANPALB3KTQZJMWVPNHLR5",
        default_version_id: "v3",
        published: "2018-06-05T18:27:44Z",
        revised: "2021-05-10T17:52:19Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudfront:ListDistributions","elasticloadbalancing:DescribeLoadBalancers","shield:DescribeAttack","shield:ListProtections","waf:GetWebACL","waf:UpdateWebACL"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSTransferLoggingAccess",
        policy_id: "ANPAL8TWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2020-02-10T22:39:29Z",
        revised: "2020-02-10T22:39:29Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogStreams","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWS_ConfigRole",
        policy_id: "ANPAM5VZQTMWKJ4PVNHLY",
        default_version_id: "v38",
        published: "2015-10-20T19:29:00Z",
        revised: "2021-09-27T22:13:54Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","cloudwatch:DescribeAlarms","config:Put*","ec2:Describe*","iam:GetRole","iam:ListRoles","s3:GetBucketAcl","s3:GetBucketLocation","s3:ListAllMyBuckets","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonAPIGatewayPushToCloudWatchLogs",
        policy_id: "ANPAN9WMJWQZKT6PVNHLC",
        default_version_id: "v1",
        published: "2015-11-13T19:04:00Z",
        revised: "2015-11-13T19:04:00Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:FilterLogEvents","logs:GetLogEvents","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonAppStreamServiceAccess",
        policy_id: "ANPAP2XQZJMWT5PVNHLRD",
        default_version_id: "v5",
        published: "2016-11-19T04:17:37Z",
        revised: "2021-07-23T18:02:16Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","s3:CreateBucket","s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonDMSCloudWatchLogsRole",
        policy_id: "ANPAQ7YWKJZQMT7PVNHLB",
        default_version_id: "v3",
        published: "2016-01-19T18:40:00Z",
        revised: "2021-01-14T17:52:38Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:PutLogEvents"],"Resource":"arn:aws:logs:*:*:log-group:dms-tasks-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonDMSRedshiftS3Role",
        policy_id: "ANPAR4ZZQTMWKJ4PVNHLY",
        default_version_id: "v3",
        published: "2016-04-20T17:05:56Z",
        revised: "2019-08-28T18:34:02Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:CreateBucket","s3:DeleteObject","s3:GetBucketLocation","s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":"arn:aws:s3:::dms-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonDMSVPCManagementRole",
        policy_id: "ANPAJHKIGMBQI4AEFFSYO",
        default_version_id: "v3",
        published: "2015-11-18T16:33:19Z",
        revised: "2016-05-23T16:29:57Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DescribeAvailabilityZones","ec2:DescribeInternetGateways","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:DeleteNetworkInterface","ec2:ModifyNetworkInterfaceAttribute"],"Resource":"*"}]}"#,
    },
    // The driver's policy of the IRSA era, which AWS still publishes under the path it
    // filed service-role policies at — superseded for the add-on catalog's
    // recommendation by the root-path `AmazonEBSCSIDriverPolicyV2` entry above. The
    // conditions are the published policy's: the driver may create and delete only the
    // volumes and snapshots it tagged for itself.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEBSCSIDriverPolicy",
        policy_id: "ANPAAK2ZQTMWK6PVNHLJR",
        default_version_id: "v3",
        published: "2022-05-26T19:23:18Z",
        revised: "2024-10-08T17:51:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:AttachVolume","ec2:DetachVolume","ec2:ModifyVolume","ec2:DescribeAvailabilityZones","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeTags","ec2:DescribeVolumes","ec2:DescribeVolumesModifications"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"],"Condition":{"StringEquals":{"ec2:CreateAction":["CreateVolume","CreateSnapshot"]}}},{"Effect":"Allow","Action":["ec2:DeleteTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"]},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeSnapshotName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2ContainerServiceAutoscaleRole",
        policy_id: "ANPAS6BMJWQZKT6PVNHLC",
        default_version_id: "v1",
        published: "2016-05-12T23:50:57Z",
        revised: "2016-05-12T23:50:57Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ecs:DescribeServices","ecs:UpdateService"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2ContainerServiceEventsRole",
        policy_id: "ANPAT3CQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2017-05-30T16:51:35Z",
        revised: "2020-10-02T18:18:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecs:RunTask"],"Resource":"*"},{"Effect":"Allow","Action":["iam:PassRole"],"Resource":"*","Condition":{"StringLike":{"iam:PassedToService":"ecs-tasks.amazonaws.com"}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2ContainerServiceRole",
        policy_id: "ANPAJO53W2XHNACG7V77Q",
        default_version_id: "v2",
        published: "2015-04-09T16:14:19Z",
        revised: "2016-08-11T13:08:54Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AuthorizeSecurityGroupIngress","ec2:Describe*","elasticloadbalancing:DeregisterInstancesFromLoadBalancer","elasticloadbalancing:DeregisterTargets","elasticloadbalancing:Describe*","elasticloadbalancing:RegisterInstancesWithLoadBalancer","elasticloadbalancing:RegisterTargets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2ContainerServiceforEC2Role",
        policy_id: "ANPAV8DWKJZQMT7PVNHLB",
        default_version_id: "v6",
        published: "2015-03-19T18:45:18Z",
        revised: "2024-08-06T18:14:32Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeTags","ecr:BatchCheckLayerAvailability","ecr:BatchGetImage","ecr:GetAuthorizationToken","ecr:GetDownloadUrlForLayer","ecs:DeregisterContainerInstance","ecs:DiscoverPollEndpoint","ecs:Poll","ecs:RegisterContainerInstance","ecs:StartTelemetrySession","ecs:Submit*","ecs:UpdateContainerInstancesState","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2RoleforAWSCodeDeploy",
        policy_id: "ANPAW5FZQTMWKJ4PVNHLY",
        default_version_id: "v2",
        published: "2015-05-19T18:10:14Z",
        revised: "2017-03-20T17:14:10Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2RoleforAWSCodeDeployLimited",
        policy_id: "ANPAMC7XMQTZJWVPNHLR4",
        default_version_id: "v2",
        published: "2020-03-20T18:14:52Z",
        revised: "2021-07-08T17:26:41Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","s3:GetObjectVersion","s3:ListBucket"],"Resource":"arn:aws:s3:::aws-codedeploy-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2RoleforDataPipelineRole",
        policy_id: "ANPAJ3Z5I2WAJE5DN2J36",
        default_version_id: "v3",
        published: "2015-02-06T18:41:25Z",
        revised: "2016-02-22T17:24:05Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","datapipeline:*","dynamodb:*","ec2:Describe*","rds:Describe*","s3:*","sdb:*","sns:*","sqs:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2RoleforSSM",
        policy_id: "ANPAI6TL3SMY22S4KMMX6",
        default_version_id: "v9",
        published: "2015-05-29T17:48:35Z",
        revised: "2019-01-18T20:40:11Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ssm:DescribeAssociation","ssm:GetDeployablePatchSnapshotForInstance","ssm:GetDocument","ssm:DescribeDocument","ssm:GetManifest","ssm:GetParameter","ssm:GetParameters","ssm:ListAssociations","ssm:ListInstanceAssociations","ssm:PutInventory","ssm:PutComplianceItems","ssm:PutConfigurePackageResult","ssm:UpdateAssociationStatus","ssm:UpdateInstanceAssociationStatus","ssm:UpdateInstanceInformation"],"Resource":"*"},{"Effect":"Allow","Action":["ssmmessages:CreateControlChannel","ssmmessages:CreateDataChannel","ssmmessages:OpenControlChannel","ssmmessages:OpenDataChannel"],"Resource":"*"},{"Effect":"Allow","Action":["ec2messages:AcknowledgeMessage","ec2messages:DeleteMessage","ec2messages:FailMessage","ec2messages:GetEndpoint","ec2messages:GetMessages","ec2messages:SendReply"],"Resource":"*"},{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:DescribeInstanceStatus","ds:CreateComputer","ds:DescribeDirectories","logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogGroups","logs:DescribeLogStreams","logs:PutLogEvents"],"Resource":"*"},{"Effect":"Allow","Action":["s3:GetBucketLocation","s3:PutObject","s3:GetObject","s3:GetEncryptionConfiguration","s3:AbortMultipartUpload","s3:ListMultipartUploadParts","s3:ListBucket","s3:ListBucketMultipartUploads"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2SpotFleetAutoscaleRole",
        policy_id: "ANPANF2YSQWZJTVPNHLR8",
        default_version_id: "v3",
        published: "2016-08-19T18:27:14Z",
        revised: "2019-06-11T17:41:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:DescribeSpotFleetRequests","ec2:ModifySpotFleetRequest"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEC2SpotFleetTaggingRole",
        policy_id: "ANPAY2HQZJMWT5PVNHLRD",
        default_version_id: "v4",
        published: "2017-07-20T20:49:42Z",
        revised: "2021-10-07T16:38:21Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeImages","ec2:DescribeInstanceStatus","ec2:DescribeInstances","ec2:DescribeSubnets","ec2:RequestSpotInstances","ec2:RunInstances","ec2:TerminateInstances","elasticloadbalancing:RegisterInstancesWithLoadBalancer","elasticloadbalancing:RegisterTargets"],"Resource":"*"},{"Effect":"Allow","Action":["iam:PassRole"],"Resource":"*","Condition":{"StringEquals":{"iam:PassedToService":["ec2.amazonaws.com"]}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonECSTaskExecutionRolePolicy",
        policy_id: "ANPABL4QJZMWT3KVPNHLY",
        default_version_id: "v1",
        published: "2017-11-16T18:48:22Z",
        revised: "2017-11-16T18:48:22Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:GetAuthorizationToken","ecr:BatchCheckLayerAvailability","ecr:GetDownloadUrlForLayer","ecr:BatchGetImage","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-efs-csi-driver` add-on's
    // `efs-csi-controller-sa` and `efs-csi-node-sa` service accounts. The conditions
    // are the published
    // policy's: the driver may create an access point only while tagging it for its
    // cluster, and may tag or delete only an access point already carrying that tag.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEFSCSIDriverPolicy",
        policy_id: "ANPACM6MZQWKJ5PVNHLTY",
        // AWS publishes this policy at its FIRST version — it has not been revised since
        // it was published — so the version a reader serves is `v1` and the two instants
        // are one and the same.
        default_version_id: "v1",
        published: "2022-11-15T20:17:44Z",
        revised: "2022-11-15T20:17:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["elasticfilesystem:DescribeAccessPoints","elasticfilesystem:DescribeFileSystems","elasticfilesystem:DescribeMountTargets","ec2:DescribeAvailabilityZones"],"Resource":"*"},{"Effect":"Allow","Action":["elasticfilesystem:CreateAccessPoint"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:TagResource","Resource":"*","Condition":{"StringLike":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:DeleteAccessPoint","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    // The EMR service role's own policy, which AWS files under `/service-role/` —
    // the counterpart of the placement-group policy above, published at the root. The
    // two are the pair a client provisioning EMR attaches, and they sit under
    // DIFFERENT paths, so the catalog carries each under the one AWS publishes it at.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEMRServicePolicy_v2",
        policy_id: "ANPAZ7JWKJZQMT7PVNHLB",
        default_version_id: "v6",
        published: "2021-07-13T19:50:41Z",
        revised: "2024-05-14T17:26:58Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:RunInstances","ec2:TerminateInstances","elasticmapreduce:ListInstances"],"Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/for-use-with-amazon-emr-managed-policies":"true"}}},{"Effect":"Allow","Action":["ec2:DescribeInstanceTypes","ec2:DescribeImages","s3:GetBucketLocation","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticMapReduceEditorsRole",
        policy_id: "ANPAB6LZQTMWKJ4PVNHLY",
        default_version_id: "v2",
        published: "2018-11-16T21:55:14Z",
        revised: "2020-11-20T19:07:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","elasticmapreduce:ListClusters","elasticmapreduce:ListInstances","s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticMapReduceRole",
        policy_id: "ANPAIDI2BQT2LKXZG36TW",
        default_version_id: "v10",
        published: "2015-02-06T18:41:20Z",
        revised: "2020-06-24T22:24:20Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AuthorizeSecurityGroupEgress","ec2:AuthorizeSecurityGroupIngress","ec2:CancelSpotInstanceRequests","ec2:CreateNetworkInterface","ec2:CreateSecurityGroup","ec2:CreateTags","ec2:DeleteNetworkInterface","ec2:DeleteSecurityGroup","ec2:DeleteTags","ec2:Describe*","ec2:DetachNetworkInterface","ec2:ModifyImageAttribute","ec2:ModifyInstanceAttribute","ec2:RequestSpotInstances","ec2:RevokeSecurityGroupEgress","ec2:RunInstances","ec2:TerminateInstances","ec2:CreateVolume","ec2:DeleteVolume","ec2:AttachVolume","ec2:DetachVolume","iam:GetRole","iam:GetRolePolicy","iam:ListInstanceProfiles","iam:ListRolePolicies","iam:PassRole","s3:CreateBucket","s3:Get*","s3:List*","sdb:BatchPutAttributes","sdb:Select","sqs:CreateQueue","sqs:Delete*","sqs:GetQueue*","sqs:PurgeQueue","sqs:ReceiveMessage","cloudwatch:PutMetricAlarm","cloudwatch:DescribeAlarms","cloudwatch:DeleteAlarms","application-autoscaling:RegisterScalableTarget","application-autoscaling:DeregisterScalableTarget","application-autoscaling:PutScalingPolicy","application-autoscaling:DeleteScalingPolicy","application-autoscaling:Describe*"],"Resource":"*"}]}"#,
    },
    // The policy an EMR cluster's EC2 instances carry through their instance profile — the
    // counterpart of the EMR service role's own policy above, which AWS files under the
    // same path.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticMapReduceforAutoScalingRole",
        policy_id: "ANPAC3NMJWQZKT6PVNHLC",
        default_version_id: "v1",
        published: "2016-11-18T01:09:10Z",
        revised: "2016-11-18T01:09:10Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","elasticmapreduce:ListInstanceGroups","elasticmapreduce:ModifyInstanceGroups"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticMapReduceforEC2Role",
        policy_id: "ANPAIGALS5RCDLZLB3PGS",
        default_version_id: "v3",
        published: "2015-02-06T18:41:21Z",
        revised: "2017-08-11T23:57:30Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Resource":"*","Action":["cloudwatch:*","dynamodb:*","ec2:Describe*","elasticmapreduce:Describe*","elasticmapreduce:ListBootstrapActions","elasticmapreduce:ListClusters","elasticmapreduce:ListInstanceGroups","elasticmapreduce:ListInstances","elasticmapreduce:ListSteps","kinesis:CreateStream","kinesis:DeleteStream","kinesis:DescribeStream","kinesis:GetRecords","kinesis:GetShardIterator","kinesis:MergeShards","kinesis:PutRecord","kinesis:SplitShard","rds:Describe*","s3:*","sdb:*","sns:*","sqs:*","glue:CreateDatabase","glue:UpdateDatabase","glue:DeleteDatabase","glue:GetDatabase","glue:GetDatabases","glue:CreateTable","glue:UpdateTable","glue:DeleteTable","glue:GetTable","glue:GetTables","glue:GetTableVersions","glue:CreatePartition","glue:BatchCreatePartition","glue:UpdatePartition","glue:DeletePartition","glue:BatchDeletePartition","glue:GetPartition","glue:GetPartitions","glue:BatchGetPartition","glue:CreateUserDefinedFunction","glue:UpdateUserDefinedFunction","glue:DeleteUserDefinedFunction","glue:GetUserDefinedFunction","glue:GetUserDefinedFunctions"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticTranscoderRole",
        policy_id: "ANPAJNW3WMKVXFJ2KPIQ2",
        default_version_id: "v2",
        published: "2015-02-06T18:41:24Z",
        revised: "2019-06-13T22:48:22Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:ListBucket","s3:Put*","s3:*MultipartUpload"],"Resource":"*"},{"Effect":"Allow","Action":"sns:Publish","Resource":"*"},{"Effect":"Deny","Action":["s3:*Delete*","s3:*Policy*","sns:*Delete*","sns:*Permission*","sns:*Remove*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonFreeRTOSOTAUpdate",
        policy_id: "ANPAPG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2018-11-27T18:05:33Z",
        revised: "2020-04-15T17:22:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iot:CreateJob","iot:DescribeJob","s3:GetObject","s3:GetObjectVersion","signer:DescribeSigningJob"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonGrafanaAthenaAccess",
        policy_id: "ANPAQH9WNQZMJTVPKHLR1",
        default_version_id: "v1",
        published: "2022-04-26T18:12:07Z",
        revised: "2022-04-26T18:12:07Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["athena:GetQueryExecution","athena:GetQueryResults","athena:ListWorkGroups","athena:StartQueryExecution","glue:GetDatabase","glue:GetTable","s3:GetObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonGrafanaCloudWatchAccess",
        policy_id: "ANPARB3KTQZJMWVPNHLR5",
        default_version_id: "v2",
        published: "2021-12-02T18:15:41Z",
        revised: "2023-02-07T17:48:12Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DescribeAlarms","cloudwatch:GetMetricData","cloudwatch:ListMetrics","logs:DescribeLogGroups","logs:GetQueryResults","logs:StartQuery","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonGrafanaRedshiftAccess",
        policy_id: "ANPASC7XMQTZJWVPNHLR4",
        default_version_id: "v1",
        published: "2022-04-26T18:18:26Z",
        revised: "2022-04-26T18:18:26Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["redshift-data:DescribeStatement","redshift-data:ExecuteStatement","redshift-data:GetStatementResult","redshift:DescribeClusters","secretsmanager:GetSecretValue"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonMacieHandshakeRole",
        policy_id: "ANPATF2YSQWZJTVPNHLR8",
        default_version_id: "v1",
        published: "2017-08-14T14:56:08Z",
        revised: "2017-08-14T14:56:08Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListAccountAliases","organizations:DescribeAccount","organizations:DescribeOrganization"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonMacieServiceRole",
        policy_id: "ANPAI3AKJVQIFWNUMKFTS",
        default_version_id: "v1",
        published: "2017-08-14T14:53:30Z",
        revised: "2017-08-14T14:53:30Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonPersonalizeFullAccess",
        policy_id: "ANPAUG5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2018-11-28T18:24:11Z",
        revised: "2019-12-03T17:35:46Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["personalize:*"],"Resource":"*"},{"Effect":"Allow","Action":["s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonRDSDirectoryServiceAccess",
        policy_id: "ANPAD5PQZJMWT5PVNHLRD",
        default_version_id: "v1",
        published: "2016-02-26T02:35:59Z",
        revised: "2016-02-26T02:35:59Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ds:AuthorizeApplication","ds:DescribeDirectories","ds:GetAuthorizedApplicationDetails","ds:UnauthorizeApplication"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonRDSEnhancedMonitoringRole",
        policy_id: "ANPAJ36T4OKJZUHBTRIAK",
        default_version_id: "v1",
        published: "2015-11-11T19:58:29Z",
        revised: "2015-11-11T19:58:29Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"EnableCreationAndManagementOfRDSCloudwatchLogGroups","Effect":"Allow","Action":["logs:CreateLogGroup","logs:PutRetentionPolicy"],"Resource":["arn:aws:logs:*:*:log-group:RDS*"]},{"Sid":"EnableCreationAndManagementOfRDSCloudwatchLogStreams","Effect":"Allow","Action":["logs:CreateLogStream","logs:PutLogEvents","logs:DescribeLogStreams","logs:GetLogEvents"],"Resource":["arn:aws:logs:*:*:log-group:RDS*:log-stream:*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonRekognitionServiceRole",
        policy_id: "ANPAF9QWKJZQMT7PVNHLB",
        default_version_id: "v3",
        published: "2017-11-29T18:42:07Z",
        revised: "2020-11-03T19:21:44Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:Publish"],"Resource":"arn:aws:sns:*:*:AmazonRekognition*"},{"Effect":"Allow","Action":["kinesis:PutRecord","kinesis:PutRecords"],"Resource":"arn:aws:kinesis:*:*:stream/AmazonRekognition*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonS3ObjectLambdaExecutionRolePolicy",
        policy_id: "ANPAVH9WNQZMJTVPKHLR1",
        default_version_id: "v1",
        published: "2021-03-18T18:07:52Z",
        revised: "2021-03-18T18:07:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","s3-object-lambda:WriteGetObjectResponse"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonSNSRole",
        policy_id: "ANPAG2RZQTMWKJ4PVNHLY",
        default_version_id: "v1",
        published: "2015-02-06T18:41:28Z",
        revised: "2015-02-06T18:41:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:DescribeLogStreams","logs:PutLogEvents","logs:PutMetricFilter","logs:PutRetentionPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonSSMAutomationRole",
        policy_id: "ANPAH6SMJWQZKT6PVNHLC",
        default_version_id: "v7",
        published: "2015-11-13T19:18:24Z",
        revised: "2018-10-04T17:44:51Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:CreateStack","cloudformation:DeleteStack","cloudformation:DescribeStacks","ec2:CreateImage","ec2:DescribeImages","ec2:DescribeInstanceStatus","ec2:DescribeInstances","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances","ssm:DescribeInstanceInformation","ssm:GetAutomationExecution","ssm:ListCommandInvocations","ssm:ListCommands","ssm:SendCommand","ssm:StartAutomationExecution"],"Resource":"*"},{"Effect":"Allow","Action":["lambda:InvokeFunction"],"Resource":"arn:aws:lambda:*:*:function:Automation*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonSSMMaintenanceWindowRole",
        policy_id: "ANPAJ3TQZJMWT5PVNHLRD",
        default_version_id: "v3",
        published: "2017-08-09T20:49:14Z",
        revised: "2019-01-17T20:08:36Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["resource-groups:ListGroupResources","ssm:CancelCommand","ssm:GetAutomationExecution","ssm:GetCommandInvocation","ssm:ListCommandInvocations","ssm:ListCommands","ssm:SendCommand","ssm:StartAutomationExecution","states:DescribeExecution","states:StartExecution","tag:GetResources"],"Resource":"*"},{"Effect":"Allow","Action":["lambda:InvokeFunction"],"Resource":"arn:aws:lambda:*:*:function:SSM*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonSecurityLakeMetastoreManager",
        policy_id: "ANPAWB3KTQZJMWVPNHLR5",
        default_version_id: "v3",
        published: "2023-11-16T18:41:27Z",
        revised: "2024-10-02T17:52:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:CreateDatabase","glue:CreateTable","glue:GetDatabase","glue:GetTable","lakeformation:GetDataAccess","s3:GetObject","s3:PutObject"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonTextractServiceRole",
        policy_id: "ANPAK7VWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2019-05-29T21:01:17Z",
        revised: "2019-05-29T21:01:17Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:Publish"],"Resource":"arn:aws:sns:*:*:AmazonTextract*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmplifyBackendDeployFullAccess",
        policy_id: "ANPAXC7XMQTZJWVPNHLR4",
        default_version_id: "v8",
        published: "2021-09-23T18:14:36Z",
        revised: "2024-05-30T17:28:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["amplify:CreateApp","amplify:GetApp","cloudformation:CreateStack","cloudformation:DescribeStacks","cognito-idp:CreateUserPool","lambda:CreateFunction","s3:CreateBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "ApplicationAutoScalingForAmazonAppStreamAccess",
        policy_id: "ANPAYF2YSQWZJTVPNHLR8",
        default_version_id: "v2",
        published: "2017-02-06T21:14:08Z",
        revised: "2018-07-19T18:41:33Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["appstream:DescribeFleets","appstream:UpdateFleet","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AutoScalingNotificationAccessRole",
        policy_id: "ANPAIO2VMUPGDC5PZVXVA",
        default_version_id: "v1",
        published: "2015-02-06T18:41:22Z",
        revised: "2015-02-06T18:41:22Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:SendMessage","sqs:GetQueueUrl","sns:Publish"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AwsGlueSessionUserRestrictedNotebookServiceRole",
        policy_id: "ANPAZG5ZUQWMJTVPNHLR6",
        default_version_id: "v1",
        published: "2022-08-10T18:05:14Z",
        revised: "2022-08-10T18:05:14Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:CreateSession","glue:DeleteSession","glue:GetSession","glue:ListSessions","glue:RunStatement"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AwsGlueSessionUserRestrictedServiceRole",
        policy_id: "ANPA2H9WNQZMJTVPKHLR1",
        default_version_id: "v2",
        published: "2022-08-10T18:08:41Z",
        revised: "2023-01-25T17:33:06Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:CreateSession","glue:DeleteSession","glue:GetSession","glue:ListSessions","glue:RunStatement","glue:StopSession"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "CloudWatchEventsBuiltInTargetExecutionAccess",
        policy_id: "ANPAL4WZQTMWKJ4PVNHLY",
        default_version_id: "v1",
        published: "2016-01-14T18:35:03Z",
        revised: "2016-01-14T18:35:03Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:Describe*","ec2:RebootInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "CloudWatchEventsInvocationAccess",
        policy_id: "ANPAJPZ67RUEZDTMCUDVC",
        default_version_id: "v1",
        published: "2016-01-14T18:35:28Z",
        revised: "2016-01-14T18:35:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["kinesis:PutRecord"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "ComprehendDataAccessRolePolicy",
        policy_id: "ANPAM9XMJWQZKT6PVNHLC",
        default_version_id: "v2",
        published: "2018-11-28T17:14:09Z",
        revised: "2020-09-02T19:36:28Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","s3:ListBucket"],"Resource":["arn:aws:s3:::*comprehend*"]},{"Effect":"Allow","Action":["s3:PutObject"],"Resource":["arn:aws:s3:::*comprehend*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "EC2ImageBuilderLifecycleExecutionPolicy",
        policy_id: "ANPA3B3KTQZJMWVPNHLR5",
        default_version_id: "v2",
        published: "2023-11-17T18:22:41Z",
        revised: "2024-04-18T17:14:09Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DeleteSnapshot","ec2:DeregisterImage","ec2:DescribeImages","ec2:DescribeSnapshots","ec2:ModifyImageAttribute"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "GreengrassOTAUpdateArtifactAccess",
        policy_id: "ANPA4C7XMQTZJWVPNHLR4",
        default_version_id: "v2",
        published: "2017-11-29T18:14:52Z",
        revised: "2019-01-16T17:41:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":"arn:aws:s3:::*-greengrass-updates/*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "QuickSightAccessForS3StorageManagementAnalyticsReadOnly",
        policy_id: "ANPA5F2YSQWZJTVPNHLR8",
        default_version_id: "v4",
        published: "2017-07-21T18:05:33Z",
        revised: "2021-01-14T17:26:48Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetBucketLocation","s3:GetObject","s3:ListAllMyBuckets","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "RDSCloudHsmAuthorizationRole",
        policy_id: "ANPAN5YQZJMWT5PVNHLRD",
        default_version_id: "v2",
        published: "2015-02-06T18:41:29Z",
        revised: "2019-09-26T22:14:29Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudhsm:CreateLunaClient","cloudhsm:DeleteLunaClient","cloudhsm:DescribeHapg","cloudhsm:DescribeHsm","cloudhsm:DescribeLunaClient","cloudhsm:GetConfig","cloudhsm:ListAvailableZones","cloudhsm:ListHapgs","cloudhsm:ListHsms","cloudhsm:ListLunaClients","cloudhsm:ModifyHapg","cloudhsm:ModifyLunaClient"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "ServerMigrationServiceLaunchRole",
        policy_id: "ANPA6G5ZUQWMJTVPNHLR6",
        default_version_id: "v2",
        published: "2018-11-20T18:33:14Z",
        revised: "2020-02-25T17:48:52Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:CreateStack","cloudformation:DescribeStacks","ec2:ModifyInstanceAttribute","ec2:RunInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "ServerMigrationServiceRoleForInstanceValidation",
        policy_id: "ANPA7H9WNQZMJTVPKHLR1",
        default_version_id: "v1",
        published: "2019-11-14T18:12:27Z",
        revised: "2019-11-14T18:12:27Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:DescribeInstances","ec2:DescribeInstanceStatus","ssm:GetCommandInvocation","ssm:SendCommand"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "VMImportExportRoleForAWSConnector",
        policy_id: "ANPAP8ZWKJZQMT7PVNHLB",
        default_version_id: "v1",
        published: "2015-09-03T20:48:59Z",
        revised: "2015-09-03T20:48:59Z",
        is_attachable: true,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetBucketAcl","s3:GetBucketLocation","s3:GetObject","s3:ListBucket","s3:PutObject"],"Resource":["arn:aws:s3:::import-to-ec2-*"]},{"Effect":"Allow","Action":["ec2:CancelConversionTask","ec2:CreateTags","ec2:DescribeConversionTasks","ec2:DescribeImages","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeTags","ec2:ImportInstance","ec2:ImportVolume"],"Resource":"*"}]}"#,
    },
];

/// The catalog entry `arn` names, or `None` when `arn` names no AWS-managed policy
/// this emulator carries.
///
/// The ARN is read through the one grammar this crate holds
/// ([`parse_policy_arn`]), so a value that is not a managed-policy ARN at all names
/// nothing here either; the account segment is what says the ARN is in AWS's
/// namespace rather than an account's, and the entry answers only when the ARN asked
/// for names IT ([`same_policy_arn`]: the path as published, the name whatever case it
/// is spelled in) — the same identity test
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_by_arn`] applies to
/// the account's policies.
pub fn aws_managed_policy(arn: &str) -> Option<&'static AwsManagedPolicy> {
    let parsed = parse_policy_arn(arn)?;
    if parsed.account != AWS_MANAGED_POLICY_ACCOUNT {
        return None;
    }
    AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| {
            entry.path == parsed.path
                && entry.policy_name.eq_ignore_ascii_case(&parsed.policy_name)
        })
        .filter(|entry| same_policy_arn(&entry.arn(), arn))
}

/// Whether `arn` is an ARN in AWS's own managed-policy namespace — well formed and
/// carrying [`AWS_MANAGED_POLICY_ACCOUNT`] as its account — whether or not
/// [`AWS_MANAGED_POLICIES`] carries that policy.
///
/// This is what separates "AWS owns this policy" from "this account owns it": the
/// write operations refuse an AWS-owned ARN as an out-of-range value for their
/// parameter rather than as a missing entity, because no account can create, change
/// or delete a policy AWS owns.
pub fn is_aws_managed_policy_arn(arn: &str) -> bool {
    parse_policy_arn(arn).is_some_and(|parsed| parsed.account == AWS_MANAGED_POLICY_ACCOUNT)
}

/// What IAM answers a TAG call addressed to a policy outside the account's own policy
/// domain with — an ARN in AWS's own managed-policy namespace
/// ([`is_aws_managed_policy_arn`]).
///
/// Entirely fixed text: the sentence states that the DOMAIN the addressed policy lives in
/// does not support tags at all, so it names neither the policy nor the tags the request
/// carried, and it is the same sentence whether the request was adding tags or removing
/// them — what it reports is a fact about the domain, not about the call.
pub const POLICY_TAGS_UNSUPPORTED_DOMAIN: &str = "Tags not supported for policies in this domain";

/// The catalog entry published under the friendly name `policy_name`, whatever path
/// AWS files it under, or `None` when the catalog carries no policy of that name.
///
/// A managed policy's friendly name identifies ONE policy in the namespace that owns
/// it — IAM holds a single policy per name whichever path it is filed under, which is
/// why the account's own policies are keyed by name alone
/// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`]) — so the name is
/// enough to say WHICH policy an ARN means to name, and the entry it answers with is
/// the one whose path that ARN has to carry to name it.
///
/// Matched without regard to ASCII case, the folding every IAM name is keyed under
/// ([`name_key`]).
pub fn aws_managed_policy_named(policy_name: &str) -> Option<&'static AwsManagedPolicy> {
    AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| entry.policy_name.eq_ignore_ascii_case(policy_name))
}

/// Whether `arn`, an ARN in AWS's own managed-policy namespace, names a policy AWS
/// publishes.
///
/// An AWS-managed policy is not account state, so what the account holds says nothing
/// about it; the catalog does. A policy's identity is its path AND its friendly name
/// (the two its ARN spells), and a name names one policy
/// ([`aws_managed_policy_named`]), so an ARN that spells a carried policy's name under
/// any other path names NO policy at all — the path is not a hint AWS ignores, it is
/// part of the published identifier.
///
/// [`AWS_MANAGED_POLICIES`] is a SUBSET of AWS's catalog, which runs to well over a
/// thousand policies, so a name the catalog does not carry cannot be judged: such an
/// ARN is taken as published rather than refusing an attachment AWS accepts. What the
/// catalog knows, it is authoritative about — including where the policy it knows
/// lives.
pub fn is_published_aws_managed_policy_arn(arn: &str) -> bool {
    let Some(parsed) = parse_policy_arn(arn) else {
        return false;
    };
    if parsed.account != AWS_MANAGED_POLICY_ACCOUNT {
        return false;
    }
    match aws_managed_policy_named(&parsed.policy_name) {
        Some(entry) => same_policy_arn(&entry.arn(), arn),
        None => true,
    }
}
