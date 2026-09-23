# IAM SDK 4.0.0 snapshot

Source: published [silicon-iam-client 4.0.0](https://crates.io/crates/silicon-iam-client/4.0.0), upstream commit `5743fb2452ced1be82a19846c7527b5ccc3afe1b`.
Registry archive SHA-256: `a1668b04888a69dc8923b4f179a6641416226a01c9d095d66dead59c68476c79`.
Normalized package metadata and Apache-2.0 license are retained. A local rustfmt.toml keeps the published source formatting independent of the consumer workspace. No sibling checkout or unpublished dependency is required.

Local change: `src/webhook.rs::validate_event` accepts a nonempty string for
`aggregate.id` instead of requiring a UUID. The [live IAM OpenAPI](https://docs.iam.teamofsilicons.com/openapi.yaml)
defines it as a string; membership events now carry IDs such as `c:alice[tos]`.
Event IDs remain UUIDs. Signature, timestamp, event ID, envelope, and testing-key checks are unchanged;
no signed bytes are rewritten.

Regression: `cargo test -p space-station-backend --lib iam::webhook` and the SDK/stub
canonical webhook tests. Return to the registry dependency when a published SDK contains this fix.
