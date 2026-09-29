# Elysium Behavior → Genesis decision v1

Canonical wire shape: [decision.schema.json](v1/decision.schema.json). [Example](v1/examples/decision-apply.json). The Behavior Engine producer model is in `FastAPI-module/services/behavior-engine/app/schemas/genesis_decision.py`; Genesis parses the same fields in `core/src/behavior_decision.rs`. These models validate a **decision**, not a device action. ELYSIUM-343 built the timed lifecycle and ELYSIUM-344 the reconciliation of the physical state, both in `core/src/grant.rs`; the transport is still missing, so no endpoint accepts a decision yet.

## Authority and scope

Behavior Engine remains the source of truth for goal/evidence decisions. The producer must derive `subject_id`, `central_unit_id`, `household_id`, target and time window from server-owned records; a mobile client cannot assert them. Genesis must authenticate the Behavior service separately, verify that the central unit belongs to the household and the target device/capability is in that household, check `valid_from <= now < expires_at`, and enforce permissions before creating a ledger command. JSON validation alone never authorizes execution. `decision_id` and `idempotency_key` identify the same logical intent on retries. `operation=apply` or `revert` records why the requested boolean value was chosen. Genesis treats `apply` as opening a timed grant and `revert` as withdrawing every grant that may still hold that device and capability open, so a revert must carry the value that closes the access, never the one the grant opened.

The window timestamps must end in UTC `Z`, with `valid_from < expires_at`. `required_confirmation=provider` accepts only provider acknowledgment as the requested minimum; `device` requires a matching device observation. Neither `accepted` nor `sent` proves the requested result.

## Results and failures

The execution response follows the existing Genesis [command v1](../v1/message.schema.json) status model and carries the originating `decision_id` in the future Behavior transport mapping. A successful service call may yield `provider_confirmed` while the device remains unverified. Missing or mismatched observation yields `unknown` after timeout; the caller must query the ledger and reconcile before retrying. An explicit rejection yields `failed` with a reason code. Expired, unauthorized, cross-household or unsupported capability decisions are rejected before dispatch, with no physical action. The future transport must define an authenticated error envelope and persistent decision-to-command mapping; this contract alone does not claim these are implemented.

## Compatibility

Breaking changes require v2. Extra fields and unknown enum values are rejected in v1. Both repositories have independent contract validation tests; update schema, Rust model, Pydantic model and examples together.
