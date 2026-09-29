# Genesis contracts

This directory defines the first versioned wire contract shared by Genesis core, the Home Assistant adapter, the Flutter panel, and later Elysium integration. It is a **contract**, not an API endpoint or a deployed service.

## Version 1.0

- Schema: [v1/message.schema.json](v1/message.schema.json) (JSON Schema Draft 2020-12).
- Valid examples: [v1/examples/](v1/examples/).
- Validation: `python3 -m pip install -r contracts/requirements-dev.txt` then `python3 -m unittest discover -s contracts/tests -v` from the repository root.

The root schema accepts five message kinds: `household`, `device`, `capability`, `observation`, and `command`. Every message has `schema_version: "1.0"` and a `kind`. Unknown fields are rejected. Identifiers are opaque strings; clients must not parse provider or ownership information out of them.

## Identity and ownership

- `household_id` scopes each device, capability, observation, and command. Authorization must verify that the actor can access this household; schema validation alone cannot do that.
- `device_id` and `capability_id` are Genesis identities. Provider references stay separate and must not become the primary identity.
- A capability states its read/write support and primitive value type. The first pilot uses a boolean `switch` capability for a light. The runtime must check a command value against the referenced capability; JSON Schema cannot validate a cross-message relationship.
- `actor` is required on commands. The runtime resolves the actor against authentication and household membership. A caller-supplied actor field is not proof of identity.

## Observations and commands

- `observation` stores a reported value, source, observed/received timestamps, quality, and correlation ID. `unknown` quality carries `null` value. Timestamps use UTC RFC 3339 form ending in `Z`; the validator checks the date-time format.
- `command` carries a stable `command_id`, `idempotency_key`, `correlation_id`, actor, requested value, current status, confirmation level, and timestamps. Repeated requests with the same household + idempotency key must return the same logical command result rather than perform a second physical action. The service owns this guarantee.
- `provider_confirmed` requires provider acknowledgment evidence. It means the Home Assistant or other provider accepted the command, **not** that the physical device changed state.
- `device_confirmed` requires a correlated device observation. The service must verify that the observation is fresh, belongs to the same device/capability, and matches the requested value before emitting this status.
- `unknown` is an explicit outcome when the actual effect cannot be established. It cannot claim device confirmation. A timeout, failure, or rejection requires a reason.
- The command object is a snapshot. The eventual execution ledger must retain each status change as an append-only event with its own timestamp, actor, and correlation ID; overwriting this object is not an audit log.

## Evolution

Version `1.0` is intentionally narrow. A breaking field or semantic change requires a new versioned schema directory and migration plan. Additive changes within v1 require a reviewed schema change, examples, and tests; existing clients must not silently accept unknown fields. Persisted events must retain their original schema version.

The Elysium Behavior authorization and time-bound entitlement payloads are separate work in ELYSIUM-342. This v1 contract does not grant access by itself.
