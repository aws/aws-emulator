// SPDX-License-Identifier: CC0-1.0

//! EMPTY on purpose: there is no suite here, and nothing in this file compiles into one.
//!
//! What was drafted here — the documented ECS-on-EC2 estate, read back through this service's own
//! listing — now sits beside the router, where the Amazon EC2, AWS IAM and AWS Systems Manager
//! calls it makes are served by the services that own them:
//! `core/router/tests/all/ecs_ec2_launched_host_registers_on_a_started_emulator.rs`. The file
//! itself stays because the tool this emulator's fixers run with can create and edit a file but
//! cannot delete or move one, and `tests/all/main.rs` declares every file of this directory.
