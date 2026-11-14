// SPDX-License-Identifier: CC0-1.0

//! The response elements EC2 renders that this checkout's Smithy model does not declare.
//!
//! # The gap this closes
//!
//! The model a service crate is generated from is a SNAPSHOT of the API, and the live EC2
//! wire has moved past it in places: a reported shape carries an element the shape's
//! modelled member list does not mention. A recorded `DescribeVpcs` answers every VPC with
//! `<dhcpOptionsSource>`, every operation that reports a Verified Access instance
//! answers it with `<verifiedAccessInstanceTransitGatewayAttachmentSet>` — present and
//! empty for an instance with no transit-gateway attachment — and a recorded
//! `DescribeInstanceTypes` answers every instance type with `<cpuBurstModel>` and every
//! `networkInfo` with `<trafficMirroringSupport>`. The generated serializer walks
//! the model's members and only those, so those elements are missing from this emulator's
//! responses: a client that reads `dhcpOptionsSource` to decide whether a VPC's DHCP
//! options are its own or the account's reads nothing, and a strict deserializer that
//! iterates the attachment set is handed a null collection instead of an empty one.
//!
//! An element the model does not declare cannot be a field of a generated shape, so it
//! cannot be written from a handler. It is written HERE instead, at the one seam where this
//! service renders a success body ([`super::Ec2QueryEnvelope::serialize_output`]), the same
//! way the envelope's own `<return>` flag is: a member schema built by hand, written after
//! the shape's modelled members. The rule is the PROTOCOL's — "an element the live wire
//! renders on this shape is written whichever operation reports the shape" — so it reaches
//! every operation that answers with a VPC or a Verified Access instance (a create, a
//! modify, a describe, a delete) rather than being restated per handler.
//!
//! Nothing here invents a value: each element is derived from the record being reported —
//! the VPC's own DHCP option set association ([`WrittenMembers::dhcp_options_id`]), the
//! transit-gateway attachments a Verified Access instance of this store holds, of which
//! there are none, no operation of this service attaching one, the Availability Zone a
//! reported volume's attachment sits in, which is the zone of the VOLUME the attachment was
//! written under ([`EnclosingRecord`]) — a volume and the instance it is attached to are always
//! in one zone, which is what "the volume must be in the same Availability Zone as the
//! instance" (`AttachVolume`) means — and, for an instance type, the burstable performance
//! and the network adapter the type itself reports.
//!
//! # What it does NOT reach
//!
//! An element whose value is NOT a function of the reported record or of the record it was
//! reported under — a subnet's inherited DHCP option set id, which has to be resolved from
//! the subnet's VPC — needs the store, and a protocol has none. Those are left to the
//! projection that owns them.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The namespace every shape of this service's model lives in, so an element is only ever
/// written onto a shape of EC2's own.
const EC2_NAMESPACE: &str = "com.amazonaws.ec2";

/// The scope a VPC's DHCP option set comes from, as `dhcpOptionsSource` names it: the VPC
/// itself, which is where every DHCP option set this store associates is associated
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_vpc`] gives the VPC the Region's
/// default set as its OWN association). The other scope the element reports — an
/// account-level default set, which no operation of this service creates — is why the
/// element exists at all, and why it is written from the association the VPC holds rather
/// than as a fact of every VPC.
const DHCP_OPTIONS_SOURCE_VPC: &str = "vpc";

/// The Smithy member name a VPC's DHCP option set id is written under, which is what says
/// the VPC has an association for `dhcpOptionsSource` to name the scope of.
const DHCP_OPTIONS_ID_MEMBER: &str = "DhcpOptionsId";

/// The Smithy member name a Verified Access instance's own id is written under, which is
/// what says the structure being walked IS such an instance.
const VERIFIED_ACCESS_INSTANCE_ID_MEMBER: &str = "VerifiedAccessInstanceId";

/// `InstanceMetadataProtocolState`'s `enabled`, the state of the IPv4 metadata endpoint on
/// every instance: IMDS has always served IPv4 and no operation of this service turns it
/// off — `HttpProtocolIpv6` is the protocol a launch switches on — so the element the live
/// wire renders beside that one reports the endpoint as enabled.
const METADATA_PROTOCOL_IPV4_ENABLED: &str = "enabled";

/// The Smithy member name an instance's IMDSv2 requirement is written under, which is what
/// says the structure being walked IS a reported metadata option set.
const HTTP_TOKENS_MEMBER: &str = "HttpTokens";

/// The Smithy member name a volume's own id is written under, which is what says the
/// structure being walked carries one — the identity check the volume attachment's elements
/// below are gated on.
const VOLUME_ID_MEMBER: &str = "VolumeId";

/// The Smithy member name an Availability Zone's NAME is written under, on every shape that
/// reports one.
const AVAILABILITY_ZONE_MEMBER: &str = "AvailabilityZone";

/// The Smithy member name an Availability Zone's ID is written under.
const AVAILABILITY_ZONE_ID_MEMBER: &str = "AvailabilityZoneId";

/// The Smithy member name an instance type's own name is written under, on the reported
/// instance type and on every other shape that names one.
const INSTANCE_TYPE_MEMBER: &str = "InstanceType";

/// The Smithy member name an instance type's burstable performance is written under, which
/// is what says the structure being walked IS a reported instance type — and what the burst
/// model below is read off.
const BURSTABLE_PERFORMANCE_MEMBER: &str = "BurstablePerformanceSupported";

/// The Smithy member name an instance type's Elastic Network Adapter support is written
/// under, which is what says the structure being walked IS a reported `networkInfo`.
const ENA_SUPPORT_MEMBER: &str = "EnaSupport";

/// The Smithy member name an instance type's bare metal flag is written under, which is
/// what says a type runs the caller's software on the host with no hypervisor under it.
const BARE_METAL_MEMBER: &str = "BareMetal";

/// `cpuBurstModel` of a burstable performance type: its vCPUs run at a baseline rate and
/// spend the CPU credits they earn to run above it.
const CPU_BURST_MODEL_CREDIT: &str = "credit";

/// `cpuBurstModel` of a type whose vCPUs run at their full rate, with no burst above a
/// baseline and so no burst model at all.
const CPU_BURST_MODEL_NONE: &str = "none";

/// `trafficMirroringSupport` of a type that supports Traffic Mirroring, and of one that does
/// not: copying a session's packets is done under the guest rather than by it, which a bare
/// metal instance — running on the host with no hypervisor under it — has nothing to do.
const TRAFFIC_MIRRORING_SUPPORTED: &str = "supported";

/// `trafficMirroringSupport` of a type whose packets nothing under the guest can copy.
const TRAFFIC_MIRRORING_UNSUPPORTED: &str = "unsupported";

/// `<cpuBurstModel>` on a reported instance type — a member schema for the same reason as
/// [`VPC_DHCP_OPTIONS_SOURCE`]: `com.amazonaws.ec2#InstanceTypeInfo` declares no such
/// member.
static INSTANCE_TYPE_CPU_BURST_MODEL: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#InstanceTypeInfo$cpuBurstModel",
        EC2_NAMESPACE,
        "cpuBurstModel",
    ),
    ShapeType::String,
    "cpuBurstModel",
    0,
);

/// `<trafficMirroringSupport>` on a reported instance type's network settings — a member
/// schema for the same reason as [`VPC_DHCP_OPTIONS_SOURCE`]:
/// `com.amazonaws.ec2#NetworkInfo` declares no such member.
static NETWORK_INFO_TRAFFIC_MIRRORING_SUPPORT: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#NetworkInfo$trafficMirroringSupport",
        EC2_NAMESPACE,
        "trafficMirroringSupport",
    ),
    ShapeType::String,
    "trafficMirroringSupport",
    0,
);

/// `<dhcpOptionsSource>` on a reported VPC.
///
/// A member schema rather than a modelled one: `com.amazonaws.ec2#Vpc` declares no such
/// member, so the element has no shape of its own to borrow. The XML codec names an element
/// from the member name, which is what makes this write `<dhcpOptionsSource>`.
static VPC_DHCP_OPTIONS_SOURCE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#Vpc$dhcpOptionsSource",
        EC2_NAMESPACE,
        "dhcpOptionsSource",
    ),
    ShapeType::String,
    "dhcpOptionsSource",
    0,
);

/// `<availabilityZone>` on a reported volume attachment — a member schema for the same
/// reason as [`VPC_DHCP_OPTIONS_SOURCE`]: `com.amazonaws.ec2#VolumeAttachment` declares no
/// Availability Zone member.
static VOLUME_ATTACHMENT_AVAILABILITY_ZONE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#VolumeAttachment$availabilityZone",
        EC2_NAMESPACE,
        "availabilityZone",
    ),
    ShapeType::String,
    "availabilityZone",
    0,
);

/// `<availabilityZoneId>` on a reported volume attachment — the zone's id beside the name
/// above.
static VOLUME_ATTACHMENT_AVAILABILITY_ZONE_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#VolumeAttachment$availabilityZoneId",
        EC2_NAMESPACE,
        "availabilityZoneId",
    ),
    ShapeType::String,
    "availabilityZoneId",
    0,
);

/// `<httpProtocolIpv4>` on a reported instance metadata option set — a member schema for
/// the same reason as [`VPC_DHCP_OPTIONS_SOURCE`].
static METADATA_OPTIONS_HTTP_PROTOCOL_IPV4: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#InstanceMetadataOptionsResponse$httpProtocolIpv4",
        EC2_NAMESPACE,
        "httpProtocolIpv4",
    ),
    ShapeType::String,
    "httpProtocolIpv4",
    0,
);

/// `<verifiedAccessInstanceTransitGatewayAttachmentSet>` on a reported Verified Access
/// instance — a member schema for the same reason as [`VPC_DHCP_OPTIONS_SOURCE`].
static VERIFIED_ACCESS_INSTANCE_TRANSIT_GATEWAY_ATTACHMENT_SET: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#VerifiedAccessInstance$verifiedAccessInstanceTransitGatewayAttachmentSet",
        EC2_NAMESPACE,
        "verifiedAccessInstanceTransitGatewayAttachmentSet",
    ),
    ShapeType::List,
    "verifiedAccessInstanceTransitGatewayAttachmentSet",
    0,
);

/// One BORROWED structure serialized with the recorded elements of every shape under it.
///
/// The entry point: a service that renders its own protocol envelope reaches its output as a
/// `&dyn SerializableStruct` and wraps it here. Every structure NESTED under that output is
/// re-entered, because the codec walks a nested structure with its own serializer, so the
/// rule reaches the reported shape wherever it sits — a list item, a member of its own.
pub struct RecordedMembersRef<'a>(pub &'a dyn SerializableStruct);

impl SerializableStruct for RecordedMembersRef<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut RecordedMemberSerializer {
            inner: s,
            written: None,
            zone: EnclosingRecord::default(),
        })
    }
}

/// One nested structure of no recorded elements of its own, serialized with the zone of the
/// record it was written under carried on into it — so a shape that DOES render the zone
/// still gets it however deep under that record it sits.
struct RecordedMembersUnder<'a>(&'a dyn SerializableStruct, EnclosingRecord);

impl SerializableStruct for RecordedMembersUnder<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut RecordedMemberSerializer {
            inner: s,
            written: None,
            zone: self.1.clone(),
        })
    }
}

/// The CONTEXT the structure a nested one is written UNDER reported, as that nested
/// structure's own recorded elements need it.
///
/// What this walk carries DOWN: a volume attachment renders the zone it
/// is in and the `VolumeAttachment` structure has no member for it, so the zone is taken
/// from the volume the attachment was reported under, whose own `AvailabilityZone` and
/// `AvailabilityZoneId` members are written before its `attachmentSet`; an instance type's
/// network settings render whether the type supports Traffic Mirroring and the `NetworkInfo`
/// structure has no member saying which type it belongs to or whether that type is bare
/// metal, so those too are taken from the instance type the settings were reported under.
/// Empty for a
/// structure written under one that reported neither, in which case nothing is rendered.
#[derive(Debug, Default, Clone)]
struct EnclosingRecord {
    /// `Volume$AvailabilityZone` as the enclosing structure wrote it.
    availability_zone: Option<String>,
    /// `Volume$AvailabilityZoneId` as the enclosing structure wrote it.
    availability_zone_id: Option<String>,
    /// `InstanceTypeInfo$BareMetal` as the enclosing structure wrote it.
    bare_metal: Option<bool>,
    /// `InstanceTypeInfo$InstanceType` as the enclosing structure wrote it, which is what
    /// says WHICH type's settings a nested `NetworkInfo` belongs to.
    instance_type: Option<String>,
}

impl EnclosingRecord {
    /// What a structure written under the one being walked is handed: whatever that structure
    /// reported for ITSELF, and otherwise what it was handed in turn — so a record passes on
    /// the context it was given rather than hiding it.
    fn inherited_from(&self, outer: &Self) -> Self {
        Self {
            availability_zone: self
                .availability_zone
                .clone()
                .or_else(|| outer.availability_zone.clone()),
            availability_zone_id: self
                .availability_zone_id
                .clone()
                .or_else(|| outer.availability_zone_id.clone()),
            bare_metal: self.bare_metal.or(outer.bare_metal),
            instance_type: self
                .instance_type
                .clone()
                .or_else(|| outer.instance_type.clone()),
        }
    }
}

/// What the structure being walked wrote of the members a recorded element is derived from.
///
/// Collected while the shape's MODELLED members are written, so the element below is decided
/// by the record the response carries rather than by the shape it carries it in. It is also
/// what IDENTIFIES the shape: see [`recorded_writer_for`] for why a name alone does not.
#[derive(Debug, Default, Clone)]
struct WrittenMembers {
    /// Whether the structure wrote a DHCP option set id of its own — the set associated
    /// WITH IT, which is the scope `dhcpOptionsSource` reports.
    dhcp_options_id: bool,
    /// Whether the structure wrote a Verified Access instance id of its own, which is the
    /// id the attachment set below belongs to.
    verified_access_instance_id: bool,
    /// Whether the structure wrote a volume id of its own, which is what makes a structure
    /// named after a volume attachment one.
    volume_id: bool,
    /// Whether the structure wrote an IMDSv2 requirement of its own, which is what makes a
    /// structure named after a reported metadata option set one.
    http_tokens: bool,
    /// Whether the structure wrote an instance type name of its own.
    instance_type: bool,
    /// The burstable performance the structure reported for itself, which is what says it IS
    /// a reported instance type and what its burst model follows from.
    burstable_performance_supported: Option<bool>,
    /// The Elastic Network Adapter support the structure reported for itself, which is what
    /// says it IS a reported `networkInfo`.
    ena_support: Option<String>,
    /// The context the structure reported for ITSELF — the zone it is in, and whether it is
    /// a bare metal instance type — which is what the structures nested under it are handed.
    zone: EnclosingRecord,
}

/// How one shape's recorded elements are written, once its modelled members have been:
/// from what the structure itself wrote and the zone of the record it was written under.
type RecordedWriter =
    fn(&mut dyn ShapeSerializer, &WrittenMembers, &EnclosingRecord) -> Result<(), SerdeError>;

/// How `schema`'s own shape renders the elements the model does not declare for it, or
/// `None` for a shape the live wire renders exactly as the model declares it.
///
/// `schema` is what the codec was handed for the structure being written, which is the
/// structure's own schema when it is a list item and the MEMBER's schema when it is a member
/// of another structure. Both carry the same shape NAME in EC2's model — every member that
/// targets one of these shapes is named after it (`CreateVpcResult$Vpc`,
/// `ModifyVerifiedAccessInstanceResult$VerifiedAccessInstance`) — so one name answers for a
/// shape in either position.
///
/// A name is NOT on its own enough to identify the shape: a member may be named after one
/// shape and target another (`Explanation$Vpc` and `PathComponent$Vpc` are members named
/// `Vpc` that target `AnalysisComponent`). So the name only selects the writer, and each
/// writer then asks [`WrittenMembers`] for the member that only its own shape carries — the
/// identity of the structure and the derivation of its element being the same question.
fn recorded_writer_for(schema: &Schema) -> Option<RecordedWriter> {
    if schema.shape_id().namespace() != EC2_NAMESPACE {
        return None;
    }
    match schema.shape_id().shape_name() {
        "Vpc" => Some(write_vpc_recorded_members),
        "VerifiedAccessInstance" => Some(write_verified_access_instance_recorded_members),
        "VolumeAttachment" => Some(write_volume_attachment_recorded_members),
        "InstanceTypeInfo" => Some(write_instance_type_recorded_members),
        "NetworkInfo" => Some(write_network_info_recorded_members),
        // An INSTANCE's metadata option set (`InstanceMetadataOptionsResponse`), which this
        // model reports under exactly two members: `Instance$MetadataOptions` and
        // `ModifyInstanceMetadataOptionsResult$InstanceMetadataOptions`. A LAUNCH TEMPLATE's
        // option set is a DIFFERENT shape under a member of the same name
        // (`ResponseLaunchTemplateData$MetadataOptions`, targeting
        // `LaunchTemplateInstanceMetadataOptions`) and carries the same member names, so the
        // name alone cannot tell them apart and the DECLARING shape decides.
        "InstanceMetadataOptions" => Some(write_metadata_options_recorded_members),
        "MetadataOptions" if declaring_shape(schema).as_deref() == Some("Instance") => {
            Some(write_metadata_options_recorded_members)
        }
        // A reported volume renders every element the model declares for it, and is walked
        // for its own shape all the same: the zone it reports is the zone the attachments
        // nested under it are in ([`EnclosingRecord`]).
        "Volume" => Some(write_no_recorded_members),
        _ => None,
    }
}

/// The name of the shape that DECLARES `schema`, when `schema` is a member schema, or
/// `None` for anything else — a member's shape id is `<namespace>#<Shape>$<member>`, so the
/// shape it belongs to is the part between the two separators.
///
/// What tells two members of the same NAME apart when they target different shapes, which
/// is the one question [`recorded_writer_for`]'s name match cannot answer on its own.
fn declaring_shape(schema: &Schema) -> Option<String> {
    schema.member_name()?;
    let id = schema.shape_id().to_string();
    let (_, qualified) = id.split_once('#')?;
    let (shape, _) = qualified.split_once('$')?;
    Some(shape.to_string())
}

/// A shape that renders no element of its own, walked only so the structures NESTED under
/// it are handed what it reported ([`EnclosingRecord`]).
fn write_no_recorded_members(
    _s: &mut dyn ShapeSerializer,
    _written: &WrittenMembers,
    _zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    Ok(())
}

/// A reported metadata option set's recorded elements: the state of the IPv4 metadata
/// endpoint ([`METADATA_PROTOCOL_IPV4_ENABLED`]), which the live wire renders beside the
/// IPv6 one the model declares.
///
/// Written only for a structure that reported an IMDSv2 requirement of its own, which is
/// what makes it a reported option set rather than a shape whose member shares the name.
fn write_metadata_options_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    _zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    if !written.http_tokens {
        return Ok(());
    }
    s.write_string(
        &METADATA_OPTIONS_HTTP_PROTOCOL_IPV4,
        METADATA_PROTOCOL_IPV4_ENABLED,
    )
}

/// A reported volume attachment's recorded elements: the Availability Zone it is in, by
/// name and by id.
///
/// "The volume must be in the same Availability Zone as the instance" (`AttachVolume`), so
/// the zone of an attachment IS the zone of the volume it belongs to — taken from the volume
/// this attachment was reported under rather than looked up, which is the only reading a
/// protocol has and the one that cannot disagree with the volume's own report.
///
/// Written only for a structure that reported a volume id of its own, and only under a
/// record that reported a zone: an attachment reported outside a volume has no zone to name,
/// so the elements are left out rather than claiming one.
fn write_volume_attachment_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    if !written.volume_id {
        return Ok(());
    }
    if let Some(availability_zone) = zone.availability_zone.as_deref() {
        s.write_string(&VOLUME_ATTACHMENT_AVAILABILITY_ZONE, availability_zone)?;
    }
    if let Some(availability_zone_id) = zone.availability_zone_id.as_deref() {
        s.write_string(&VOLUME_ATTACHMENT_AVAILABILITY_ZONE_ID, availability_zone_id)?;
    }
    Ok(())
}

/// A reported instance type's recorded elements: the burst model of its vCPUs.
///
/// "Burstable performance instances ... can burst above the baseline" (EC2 User Guide): a
/// type whose vCPUs run at a baseline spends CPU credits to run above it, and a type that
/// runs at its full rate has no burst to model. So the element is read off the burstable
/// performance the type itself reported, which is the fact the model DOES declare for it.
///
/// Written only for a structure that reported an instance type and a burstable performance
/// of its own, which is what makes it a reported instance type rather than a shape whose
/// member shares the name.
fn write_instance_type_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    _zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    let Some(burstable) = written.burstable_performance_supported else {
        return Ok(());
    };
    if !written.instance_type {
        return Ok(());
    }
    let burst_model = if burstable {
        CPU_BURST_MODEL_CREDIT
    } else {
        CPU_BURST_MODEL_NONE
    };
    s.write_string(&INSTANCE_TYPE_CPU_BURST_MODEL, burst_model)
}

/// A reported instance type's network settings' recorded elements: whether the type supports
/// Traffic Mirroring.
///
/// Whether a type's packets can be copied is a fact the catalog publishes per type
/// ([`crate::state::published_instance_type_facts`]), read off the instance type these
/// settings were reported under ([`EnclosingRecord`]), the `NetworkInfo` structure having no
/// member of its own for it. For a type the catalog holds no row for, the answer follows from
/// the hypervisor: copying the packets of a session is done UNDER the guest rather than by it,
/// so a bare metal type — which runs the caller's software on the host with no hypervisor
/// under it, and reports no hypervisor at all — does not support it while every other type
/// does.
///
/// Written only for a structure that reported an adapter support of its own, which is what
/// makes it a reported `networkInfo`.
fn write_network_info_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    if written.ena_support.is_none() {
        return Ok(());
    }
    let published = zone
        .instance_type
        .as_deref()
        .and_then(crate::state::published_instance_type_facts)
        .and_then(|facts| facts.traffic_mirroring_supported);
    let supported = published.unwrap_or(zone.bare_metal != Some(true));
    let mirroring = if supported {
        TRAFFIC_MIRRORING_SUPPORTED
    } else {
        TRAFFIC_MIRRORING_UNSUPPORTED
    };
    s.write_string(&NETWORK_INFO_TRAFFIC_MIRRORING_SUPPORT, mirroring)
}

/// A reported VPC's recorded elements: the scope its DHCP option set comes from.
///
/// Written only for a VPC that reported an option set association of its own: a VPC with no
/// association has no scope to name, so the element is left out rather than claiming one.
fn write_vpc_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    _zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    if !written.dhcp_options_id {
        return Ok(());
    }
    s.write_string(&VPC_DHCP_OPTIONS_SOURCE, DHCP_OPTIONS_SOURCE_VPC)
}

/// A reported Verified Access instance's recorded elements: the transit-gateway attachments
/// it holds.
///
/// Always EMPTY, and present: no operation of this service attaches a transit gateway to a
/// Verified Access instance, so an instance of this store holds none — and an empty
/// collection is still a collection, which is the whole difference this closes for a client
/// that iterates it.
///
/// Written only for a structure that reported an instance id of its own, which is what makes
/// it one of these instances rather than a shape whose member merely shares the name.
fn write_verified_access_instance_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: &WrittenMembers,
    _zone: &EnclosingRecord,
) -> Result<(), SerdeError> {
    if !written.verified_access_instance_id {
        return Ok(());
    }
    s.write_list(
        &VERIFIED_ACCESS_INSTANCE_TRANSIT_GATEWAY_ATTACHMENT_SET,
        &|_items| Ok(()),
    )
}

/// One structure, serialized as its modelled members followed by the recorded elements its
/// shape renders on the live wire.
struct WithRecordedMembers<'a> {
    /// The structure as the generated serializer writes it.
    inner: &'a dyn SerializableStruct,
    /// How this shape's recorded elements are written.
    writer: RecordedWriter,
    /// The zone of the record THIS structure was written under, for a shape whose elements
    /// are read off it.
    zone: EnclosingRecord,
}

impl SerializableStruct for WithRecordedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        let mut written = WrittenMembers::default();
        {
            // The modelled members first, so the recorded elements sit after them and the
            // order the model gave this shape is untouched. The walk notes what it wrote,
            // which is what the recorded elements are derived from.
            let mut walk = RecordedMemberSerializer {
                inner: s,
                written: Some(&mut written),
                zone: self.zone.clone(),
            };
            self.inner.serialize_members(&mut walk)?;
        }
        (self.writer)(s, &written, &self.zone)
    }
}

/// The serializer a wrapped output is walked with: every call is forwarded to the codec's own
/// serializer unchanged, except a STRUCTURE, which is re-entered so the rule reaches every
/// depth and so a shape with recorded elements gets them.
struct RecordedMemberSerializer<'a> {
    /// The codec's own serializer, or the next wrapper out.
    inner: &'a mut dyn ShapeSerializer,
    /// The members of the structure being walked, when this walk is collecting them for that
    /// structure's recorded elements. `None` while walking anything else, so a nested
    /// structure's members are never read as the enclosing one's.
    written: Option<&'a mut WrittenMembers>,
    /// The zone handed DOWN to the structures written from here: the one the structure being
    /// walked has reported so far, or the one it was itself written under.
    zone: EnclosingRecord,
}

impl RecordedMemberSerializer<'_> {
    /// The context the structures written from here sit in: what the structure being walked
    /// reported for ITSELF, and otherwise what it was written under — so a volume hands
    /// its own zone to its attachments, an instance type hands down whether it is bare
    /// metal, and a structure that reports neither passes on what it was given rather than
    /// hiding it.
    fn nested_zone(&self) -> EnclosingRecord {
        match self.written.as_deref() {
            Some(written) => written.zone.inherited_from(&self.zone),
            None => self.zone.clone(),
        }
    }
}

impl ShapeSerializer for RecordedMemberSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        let zone = self.nested_zone();
        match recorded_writer_for(schema) {
            Some(writer) => self.inner.write_struct(
                schema,
                &WithRecordedMembers {
                    inner: value,
                    writer,
                    zone,
                },
            ),
            None => self
                .inner
                .write_struct(schema, &RecordedMembersUnder(value, zone)),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let zone = self.nested_zone();
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut RecordedMemberSerializer {
                inner,
                written: None,
                zone: zone.clone(),
            })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let zone = self.nested_zone();
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut RecordedMemberSerializer {
                inner,
                written: None,
                zone: zone.clone(),
            })
        })
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        if let Some(written) = self.written.as_deref_mut() {
            if schema.shape_id().namespace() == EC2_NAMESPACE {
                match schema.shape_id().shape_name() {
                    BURSTABLE_PERFORMANCE_MEMBER => {
                        written.burstable_performance_supported = Some(value);
                    }
                    BARE_METAL_MEMBER => written.zone.bare_metal = Some(value),
                    _ => {}
                }
            }
        }
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    /// The leaves this notes as it forwards: the DHCP option set id a VPC reports, which is
    /// what [`write_vpc_recorded_members`] names the scope of, the id a Verified Access
    /// instance reports, which is what identifies it as one, the volume id that identifies a
    /// volume attachment, the zone a record reports for itself, which is the zone the
    /// records nested under it are in
    /// ([`write_volume_attachment_recorded_members`]), and the instance type name and
    /// adapter support a reported instance type and its network settings carry.
    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if let Some(written) = self.written.as_deref_mut() {
            if schema.shape_id().namespace() == EC2_NAMESPACE {
                match schema.shape_id().shape_name() {
                    DHCP_OPTIONS_ID_MEMBER => written.dhcp_options_id = true,
                    VERIFIED_ACCESS_INSTANCE_ID_MEMBER => {
                        written.verified_access_instance_id = true;
                    }
                    VOLUME_ID_MEMBER => written.volume_id = true,
                    HTTP_TOKENS_MEMBER => written.http_tokens = true,
                    AVAILABILITY_ZONE_MEMBER => {
                        written.zone.availability_zone = Some(value.to_string());
                    }
                    AVAILABILITY_ZONE_ID_MEMBER => {
                        written.zone.availability_zone_id = Some(value.to_string());
                    }
                    INSTANCE_TYPE_MEMBER => {
                        written.instance_type = true;
                        written.zone.instance_type = Some(value.to_string());
                    }
                    ENA_SUPPORT_MEMBER => written.ena_support = Some(value.to_string()),
                    _ => {}
                }
            }
        }
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}
