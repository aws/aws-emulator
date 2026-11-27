# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: DescribeHealthServiceStatusForOrganization — unsupported shape construct: operation 'DescribeHealthServiceStatusForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeTagKeys — unsupported shape construct: operation 'DescribeTagKeys' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeTagKeysForOrganization — unsupported shape construct: operation 'DescribeTagKeysForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeTagsOnboardingStatus — unsupported shape construct: operation 'DescribeTagsOnboardingStatus' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DescribeTagsOnboardingStatusForOrganization — unsupported shape construct: operation 'DescribeTagsOnboardingStatusForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DisableHealthServiceAccessForOrganization — unsupported shape construct: operation 'DisableHealthServiceAccessForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DisableTags — unsupported shape construct: operation 'DisableTags' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DisableTagsForOrganization — unsupported shape construct: operation 'DisableTagsForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: EnableHealthServiceAccessForOrganization — unsupported shape construct: operation 'EnableHealthServiceAccessForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: EnableTags — unsupported shape construct: operation 'EnableTags' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: EnableTagsForOrganization — unsupported shape construct: operation 'EnableTagsForOrganization' has a Unit/absent input, which the generator does not emit yet
