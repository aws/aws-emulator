Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateGateway stores malformed EgressCidrBlocks and network CidrBlock values instead of refusing them

_Filed: 2026-10-24T05:14:37Z_
_Operation: CreateGateway | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateGatewayRequest$EgressCidrBlocks` is "the range of IP addresses that are allowed to contribute content or initiate output requests for flows communicating with this gateway. These IP addresses should be in the form of a Classless Inter-Domain Routing (CIDR) block", and `GatewayNetwork$CidrBlock` is "a unique IP address range to use for this network ... in the form of a Classless Inter-Domain Routing (CIDR) block; for example, 10.0.0.0/16". Both are IP ranges the service must parse to configure the gateway, and MediaConnect refuses a value that is not a CIDR block with 400 BadRequestException.

Expected: `POST /v1/gateways` with `egressCidrBlocks: ["banana"]`, or with a network whose `cidrBlock` is `"10.0.0.0"` / `"10.0.0.0/64"` / `""`, is refused with 400 BadRequestException and no gateway created.

Actual: the handler validates the gateway name, the network count (1..4), network-name uniqueness and the egress-block COUNT only; every CIDR string is stored verbatim. The create answers 201 and DescribeGateway/ListGateways then report a gateway configured with ranges that are not IP ranges. Nothing else in the crate parses them, so the bad value is permanent for the life of the record.

## Evidence

Spec: `smithy_query(query="shape",shape="CreateGatewayRequest")` (member `EgressCidrBlocks`, required) and `smithy_query(query="shape",shape="GatewayNetwork")` (member `CidrBlock`, required) — both documented as CIDR blocks with the example `10.0.0.0/16`; the model puts no `@pattern` on either, so the boundary walk cannot refuse them and the check belongs to the handler, exactly as the crate already does for tag lengths and maintenance-window formats (`src/state.rs:390-409`, `src/ops/update_flow.rs:349-372`).

Handler: `src/ops/create_gateway.rs:184-235` — `is_valid_gateway_name`, the 1..`MAX_GATEWAY_NETWORKS` count check, the duplicate-network-name check, and `egress_cidr_blocks.is_empty()` / `> MAX_GATEWAY_EGRESS_CIDR_BLOCKS`. Lines 267-286 store `egress_cidr_blocks` and `networks` unchanged. `grep -n 'cidr' src/state.rs` shows no CIDR parsing anywhere in the crate.

Test coverage: none — `tests/all/gateway_lifecycle.rs` exercises name, network-count, duplicate-network-name and empty-egress-list refusals, but no case sends a malformed CIDR value.

## Steps to Reproduce

1. POST /v1/gateways with {"name":"gw-bad-egress","egressCidrBlocks":["banana"],"networks":[{"name":"net-1","cidrBlock":"10.0.0.0/16"}]} — observe 201 where AWS answers 400 BadRequestException
2. POST /v1/gateways with {"name":"gw-bad-network","egressCidrBlocks":["10.0.0.0/16"],"networks":[{"name":"net-1","cidrBlock":"10.0.0.0/64"}]} — observe 201
3. GET /v1/gateways/{GatewayArn} and observe the gateway reporting the non-CIDR values as its egress range / network range
