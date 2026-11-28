// SPDX-License-Identifier: CC0-1.0

//! The append-only operation log every mutating call writes: the kind of call
//! each entry records, the status it carries, and what one entry reports.

use super::*;

/// One entry of the operation log: what a mutating call did, and when.
#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub id: String,
    pub resource_name: String,
    pub resource_type: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub is_terminal: bool,
    pub operation_details: Option<String>,
    /// The kind of call this entry records: a value of the modelled
    /// `OperationType` enum where it declares one, and otherwise the name of the
    /// call itself (see [`OPERATION_TYPE_COPY_SNAPSHOT`]) — never nothing, so an
    /// entry always says what ran.
    pub operation_type: String,
    pub status: String,
    pub status_changed_at_ms: i64,
    pub error_code: Option<String>,
    pub error_details: Option<String>,
    /// When the clock owes this operation its terminal `Succeeded` status, while
    /// one is outstanding.
    pub settles_at_ms: Option<i64>,
}

impl StoredOperation {
    /// This record as the modelled `Operation`.
    pub fn view(&self) -> Operation {
        Operation {
            id: Some(self.id.clone()),
            resource_name: Some(self.resource_name.clone()),
            resource_type: Some(self.resource_type.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            is_terminal: Some(self.is_terminal),
            operation_details: self.operation_details.clone(),
            operation_type: Some(self.operation_type.clone()),
            status: Some(self.status.clone()),
            status_changed_at: Some(timestamp(self.status_changed_at_ms)),
            error_code: self.error_code.clone(),
            error_details: self.error_details.clone(),
        }
    }
}

/// The `operationDetails` an entry whose call took the resource it names AWAY
/// reports: the empty string.
///
/// `Operation$operationDetails` is "details about the operation", a member
/// BESIDE the `resourceName` that already names what the call acted on — so it
/// never repeats that name. A removal has no further detail to give: the record
/// the entry would describe is the one the call just took away, so the detail is
/// reported as empty rather than naming a resource the account no longer holds.
pub const OPERATION_DETAILS_REMOVED: &str = "";

/// Whether an entry of `operation_type` records the removal of the resource the
/// entry NAMES — the entry's `resourceName` addresses no resource once the call
/// returns.
///
/// The members below are this log's own `operationType` values (see
/// [`OPERATION_TYPE_CREATE_INSTANCE`] and [`OPERATION_TYPE_COPY_SNAPSHOT`]) for
/// the calls whose whole work is to take that resource away. Lightsail's other
/// removals are NOT among them, because each names a resource that outlives the
/// call: deleting a recordset or an automatic snapshot, forgetting an instance's
/// recorded host keys, deleting a bucket's access key and deleting a container
/// image all name the zone, the source resource, the instance, the bucket or the
/// container service they belong to, and every one of those is still held
/// afterwards.
///
/// A call that joins two resources — an attach, a detach — has a counterpart to
/// detail, the other resource it acted with; the day one of those entries is
/// required to report it, that detail comes from the call that knows it rather
/// than from this predicate.
pub fn operation_removes_its_resource(operation_type: &str) -> bool {
    matches!(
        operation_type,
        OPERATION_TYPE_DELETE_INSTANCE
            | OPERATION_TYPE_DELETE_DISK
            | OPERATION_TYPE_DELETE_DISK_SNAPSHOT
            | OPERATION_TYPE_DELETE_INSTANCE_SNAPSHOT
            | OPERATION_TYPE_DELETE_KEY_PAIR
            | OPERATION_TYPE_DELETE_DOMAIN
            | OPERATION_TYPE_DELETE_BUCKET
            | OPERATION_TYPE_DELETE_CERTIFICATE
            | OPERATION_TYPE_DELETE_ALARM
            | OPERATION_TYPE_DELETE_CONTACT_METHOD
            | OPERATION_TYPE_DELETE_DISTRIBUTION
            | OPERATION_TYPE_DELETE_LOAD_BALANCER
            | OPERATION_TYPE_DELETE_LOAD_BALANCER_TLS_CERTIFICATE
            | OPERATION_TYPE_DELETE_RELATIONAL_DATABASE
            | OPERATION_TYPE_DELETE_RELATIONAL_DATABASE_SNAPSHOT
            | OPERATION_TYPE_DELETE_CONTAINER_SERVICE
            | OPERATION_TYPE_DELETE_SUBDOMAIN
            | OPERATION_TYPE_RELEASE_STATIC_IP
    )
}

/// `Operation$status` values this slice uses, from the modelled `OperationStatus`
/// enum: a mutating call is answered `Started` and settles into `Succeeded` when
/// the work it began is done.
pub const OPERATION_STATUS_STARTED: &str = "Started";
/// See [`OPERATION_STATUS_STARTED`].
pub const OPERATION_STATUS_SUCCEEDED: &str = "Succeeded";

/// `Operation$operationType` values this slice writes, from the modelled
/// `OperationType` enum.
pub const OPERATION_TYPE_CREATE_INSTANCE: &str = "CreateInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_START_INSTANCE: &str = "StartInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_STOP_INSTANCE: &str = "StopInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_REBOOT_INSTANCE: &str = "RebootInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_INSTANCE: &str = "DeleteInstance";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_KNOWN_HOST_KEYS: &str = "DeleteKnownHostKeys";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DOMAIN: &str = "CreateDomain";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET: &str = "CreateBucket";

/// The `Operation$operationType` a `DeleteBucket` call logs, from the modelled
/// `OperationType` enum.
pub const OPERATION_TYPE_DELETE_BUCKET: &str = "DeleteBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_BUCKET: &str = "UpdateBucket";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_BUCKET_BUNDLE: &str = "UpdateBucketBundle";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK: &str = "CreateDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_DISK: &str = "AttachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_DISK: &str = "DetachDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISK: &str = "DeleteDisk";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_STATIC_IP: &str = "AttachStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_STATIC_IP: &str = "DetachStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_INSTANCES_FROM_LOAD_BALANCER: &str =
    "DetachInstancesFromLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE: &str = "CreateRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE_SNAPSHOT: &str =
    "CreateRelationalDatabaseSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_RELATIONAL_DATABASE: &str = "DeleteRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_RELATIONAL_DATABASE_SNAPSHOT: &str =
    "DeleteRelationalDatabaseSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_RELATIONAL_DATABASE_FROM_SNAPSHOT: &str =
    "CreateRelationalDatabaseFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_RELATIONAL_DATABASE_PARAMETERS: &str =
    "UpdateRelationalDatabaseParameters";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_REBOOT_RELATIONAL_DATABASE: &str = "RebootRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_RELATIONAL_DATABASE: &str = "UpdateRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_START_RELATIONAL_DATABASE: &str = "StartRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_STOP_RELATIONAL_DATABASE: &str = "StopRelationalDatabase";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_LOAD_BALANCER: &str = "CreateLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_LOAD_BALANCER: &str = "DeleteLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_INSTANCES_TO_LOAD_BALANCER: &str =
    "AttachInstancesToLoadBalancer";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_LOAD_BALANCER_ATTRIBUTE: &str = "UpdateLoadBalancerAttribute";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "CreateLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "DeleteLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_LOAD_BALANCER_TLS_CERTIFICATE: &str =
    "AttachLoadBalancerTlsCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_SNAPSHOT: &str = "CreateDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISK_SNAPSHOT: &str = "DeleteDiskSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_INSTANCE_SNAPSHOT: &str = "CreateInstanceSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_INSTANCE_SNAPSHOT: &str = "DeleteInstanceSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISK_FROM_SNAPSHOT: &str = "CreateDiskFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_INSTANCES_FROM_SNAPSHOT: &str = "CreateInstancesFromSnapshot";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_BUCKET_ACCESS_KEY: &str = "CreateBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_BUCKET_ACCESS_KEY: &str = "DeleteBucketAccessKey";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_DISTRIBUTION: &str = "CreateDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_DISTRIBUTION: &str = "DeleteDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ALLOCATE_STATIC_IP: &str = "AllocateStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_RELEASE_STATIC_IP: &str = "ReleaseStaticIp";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_PUT_ALARM: &str = "PutAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_ALARM: &str = "DeleteAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_TEST_ALARM: &str = "TestAlarm";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_CERTIFICATE: &str = "CreateCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_CERTIFICATE: &str = "DeleteCertificate";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ATTACH_CERTIFICATE_TO_DISTRIBUTION: &str =
    "AttachCertificateToDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DETACH_CERTIFICATE_FROM_DISTRIBUTION: &str =
    "DetachCertificateFromDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_RESET_DISTRIBUTION_CACHE: &str = "ResetDistributionCache";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_DISTRIBUTION: &str = "UpdateDistribution";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_UPDATE_DISTRIBUTION_BUNDLE: &str = "UpdateDistributionBundle";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_CREATE_CONTACT_METHOD: &str = "CreateContactMethod";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DELETE_CONTACT_METHOD: &str = "DeleteContactMethod";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_SEND_CONTACT_METHOD_VERIFICATION: &str = "SendContactMethodVerification";
/// The `operationType` the log entry of a change to a zone's recordsets carries.
///
/// The modelled `OperationType` enum has `UpdateDomainEntry` and
/// `DeleteDomainEntry` but NO `CreateDomainEntry` member, and
/// `Operation$operationType` targets that enum — so adding an entry to a zone is
/// logged as the modelled recordset update, not as an unmodelled value.
pub const OPERATION_TYPE_UPDATE_DOMAIN_ENTRY: &str = "UpdateDomainEntry";
/// The `operationType` the log entry of an entry's removal from a zone carries,
/// the modelled `OperationType` enum's own `DeleteDomainEntry` member.
pub const OPERATION_TYPE_DELETE_DOMAIN_ENTRY: &str = "DeleteDomainEntry";
/// The `operationType` the log entry of a whole zone's removal carries, the
/// modelled `OperationType` enum's own `DeleteDomain` member.
pub const OPERATION_TYPE_DELETE_DOMAIN: &str = "DeleteDomain";
/// The `operationType` the log entry of a `CopySnapshot` call carries — and the
/// ONE statement of the rule every call the modelled `OperationType` enum
/// declares no member for follows.
///
/// `Operation$operationType` targets that enum, whose own members this module
/// reports wherever one of them states what the call did (see
/// [`OPERATION_TYPE_UPDATE_DOMAIN_ENTRY`]). The enum LAGS this API: it declares
/// no member for copying a snapshot, for a change to a resource's tags, for the
/// deletion of one automatic snapshot, for a key pair's deletion, for either VPC
/// peering call, for the Amazon EC2 export family or for the stack an export is
/// turned into — and no member it does carry states what one of those calls did,
/// so reporting a copy or an export as a snapshot CREATE would misreport it. The
/// member is a string on the wire and a Smithy enum is open there (a client has
/// to tolerate a value it does not know), so EVERY entry this log holds names the
/// call that wrote it, whether or not the enum declares that name: one rule for
/// the whole log `GetOperations` and `GetOperationsForResource` report, rather
/// than entries that say nothing about what ran.
pub const OPERATION_TYPE_COPY_SNAPSHOT: &str = "CopySnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`], the one rule for the `operationType` of
/// a call the modelled `OperationType` enum declares no member for.
pub const OPERATION_TYPE_TAG_RESOURCE: &str = "TagResource";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_UNTAG_RESOURCE: &str = "UntagResource";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_DELETE_AUTO_SNAPSHOT: &str = "DeleteAutoSnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_EXPORT_SNAPSHOT: &str = "ExportSnapshot";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_CREATE_KEY_PAIR: &str = "CreateKeyPair";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_DELETE_KEY_PAIR: &str = "DeleteKeyPair";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`]: the entry an import of a caller's
/// public key writes names that call, because the modelled `OperationType` enum
/// declares no member for it and no member it carries says a key pair was
/// imported.
pub const OPERATION_TYPE_IMPORT_KEY_PAIR: &str = "ImportKeyPair";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`]: the entry the call that CREATES the
/// account's default key pair writes names that call, because the modelled
/// `OperationType` enum declares no member for it and no member it carries says
/// a default key pair was created.
pub const OPERATION_TYPE_DOWNLOAD_DEFAULT_KEY_PAIR: &str = "DownloadDefaultKeyPair";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_PEER_VPC: &str = "PeerVpc";
/// See [`OPERATION_TYPE_COPY_SNAPSHOT`].
pub const OPERATION_TYPE_UNPEER_VPC: &str = "UnpeerVpc";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_DISABLE_ADD_ON: &str = "DisableAddOn";
/// See [`OPERATION_TYPE_CREATE_INSTANCE`].
pub const OPERATION_TYPE_ENABLE_ADD_ON: &str = "EnableAddOn";
/// The `operationType` the log entry of a `SetIpAddressType` call carries, the
/// modelled `OperationType` enum's own `SetIpAddressType` member.
pub const OPERATION_TYPE_SET_IP_ADDRESS_TYPE: &str = "SetIpAddressType";
/// The `operationType` the log entry of a `SetResourceAccessForBucket` call
/// carries, the modelled `OperationType` enum's own `SetResourceAccessForBucket`
/// member.
pub const OPERATION_TYPE_SET_RESOURCE_ACCESS_FOR_BUCKET: &str = "SetResourceAccessForBucket";
/// The `operationType` the log entry of an `UpdateInstanceMetadataOptions` call
/// carries, the modelled `OperationType` enum's own
/// `UpdateInstanceMetadataOptions` member.
pub const OPERATION_TYPE_UPDATE_INSTANCE_METADATA_OPTIONS: &str = "UpdateInstanceMetadataOptions";
/// The `operationType` the log entry of a `StartGUISession` call carries, the
/// modelled `OperationType` enum's own `StartGUISession` member.
pub const OPERATION_TYPE_START_GUI_SESSION: &str = "StartGUISession";
/// The `operationType` the log entry of a `StopGUISession` call carries, the
/// modelled `OperationType` enum's own `StopGUISession` member.
pub const OPERATION_TYPE_STOP_GUI_SESSION: &str = "StopGUISession";

/// The `operationType` the log entry of a `CreateSubdomain` call carries, the
/// modelled `OperationType` enum's own `CreateSubdomain` member.
pub const OPERATION_TYPE_CREATE_SUBDOMAIN: &str = "CreateSubdomain";
/// The `operationType` the log entry of an `UpdateSubdomainAssociation` call
/// carries, the modelled `OperationType` enum's own
/// `UpdateSubdomainAssociation` member.
pub const OPERATION_TYPE_UPDATE_SUBDOMAIN_ASSOCIATION: &str = "UpdateSubdomainAssociation";
/// The `operationType` the log entry of a `DeleteSubdomain` call carries, the
/// modelled `OperationType` enum's own `DeleteSubdomain` member.
pub const OPERATION_TYPE_DELETE_SUBDOMAIN: &str = "DeleteSubdomain";

impl ComAmazonawsLightsailLightsail20161128State {
    /// Append `operation` to the log, dropping the oldest entries past
    /// [`MAX_OPERATION_HISTORY`] so the log stays bounded however many calls a
    /// caller makes.
    pub fn record_operation(&mut self, operation: StoredOperation) -> StoredOperation {
        self.operations.push(operation.clone());
        if let Some(excess) = self.operations.len().checked_sub(MAX_OPERATION_HISTORY) {
            self.operations.drain(..excess);
        }
        operation
    }

    /// Build and record an operation of `operation_type` against
    /// `resource_name` — the ONE place an operation-log entry is built, so every
    /// entry this service writes carries the same members and names the call
    /// that wrote it whether or not the modelled `OperationType` enum declares a
    /// member for that call (see [`OPERATION_TYPE_COPY_SNAPSHOT`]).
    ///
    /// It is also the ONE place a request can grow the log, and the growth is
    /// bounded here: the entry is appended through [`Self::record_operation`],
    /// which keeps at most [`MAX_OPERATION_HISTORY`] entries and drops the
    /// oldest, so however many calls a caller makes the log holds no more than
    /// that.
    ///
    /// `resource_type` is the kind of resource the operation acted on, from the
    /// modelled `ResourceType` enum, so the log says what each entry is about.
    ///
    /// `settles_at_ms` is the instant the clock owes the operation its terminal
    /// status — `None` for work that was already finished when the call
    /// returned, in which case the operation is answered terminal.
    ///
    /// `operationDetails` is "details about the operation": a member BESIDE the
    /// `resourceName` the entry already carries, so it never repeats that name.
    /// Every one of this service's calls acts on the resource the entry names
    /// and has nothing further to detail, so an entry reports NO detail —
    /// except the entry of a call that took that resource away
    /// ([`operation_removes_its_resource`]), whose detail is empty
    /// ([`OPERATION_DETAILS_REMOVED`]) because the record it would describe is
    /// the one the call just removed.
    pub fn start_operation(
        &mut self,
        now_ms: i64,
        operation_type: &str,
        resource_type: &str,
        resource_name: &str,
        location: &StoredLocation,
        settles_at_ms: Option<i64>,
    ) -> StoredOperation {
        let id = self.mint_uuid();
        let terminal = settles_at_ms.is_none();
        let operation = StoredOperation {
            id,
            resource_name: resource_name.to_string(),
            resource_type: resource_type.to_string(),
            created_at_ms: now_ms,
            location: location.clone(),
            is_terminal: terminal,
            operation_details: operation_removes_its_resource(operation_type)
                .then(|| OPERATION_DETAILS_REMOVED.to_string()),
            operation_type: operation_type.to_string(),
            status: if terminal {
                OPERATION_STATUS_SUCCEEDED.to_string()
            } else {
                OPERATION_STATUS_STARTED.to_string()
            },
            status_changed_at_ms: now_ms,
            error_code: None,
            error_details: None,
            settles_at_ms,
        };
        self.record_operation(operation)
    }
}
