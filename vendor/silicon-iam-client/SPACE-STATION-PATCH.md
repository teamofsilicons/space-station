# Space Station webhook compatibility

This is the published `silicon-iam-client` 5.2.1 source, with one change in
`src/webhook.rs`: webhook `aggregate.id` accepts a nonempty string instead of
requiring a UUID. IAM also uses canonical IDs such as `c:alice[tos]`, `si:bot[tos]`
and application IDs. The exact-byte HMAC signature, timestamp, event ID, version,
event type and environment verification remain upstream code.

`iam::webhook::tests::canonical_aggregate_ids_verify_without_changing_signed_bytes`
covers canonical IDs and rejects modified signed bytes. Remove this patch when
the published verifier accepts these IDs.
