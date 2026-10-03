# Official IAM SDK

Vendored from silicon-iam-client 5.0.0, commit `f1e9c4768029aacabe337ca41be52e05023d1631`, `crates/client` in teamofsilicons/silicon-iam. Local compatibility patch: `webhook.rs::validate_event` accepts nonempty string aggregate IDs, as required by the published webhook contract. This preserves the previous Space Station patch for canonical membership IDs; signatures are still checked against the original bytes. Other normalized package sources are unmodified. See `.cargo_vcs_info.json` and `UPSTREAM.json`.
