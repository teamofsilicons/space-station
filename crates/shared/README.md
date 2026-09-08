# space-station-shared

The pieces every [Space Station](https://github.com/teamofsilicons/space-station) crate agrees
on: the limits, the ingest wire format, the shapes of secrets, and the sanitizer that keeps a
record inside those limits. Nothing here does I/O.

You normally want [`space-station`](https://crates.io/crates/space-station) (the client library
and daemon) or [`space-station-cli`](https://crates.io/crates/space-station-cli) instead; this
crate exists so the client, the CLI and the server cannot drift.

```rust
use space_station_shared::{limits, sanitize, secrets, wire};

let key = secrets::table_key("orders");           // table-orders-<32 hex>, shown once
assert_eq!(secrets::parse_table_key(&key), Some("orders"));

let mut record = serde_json::json!({ "note": "x".repeat(50_000) });
sanitize::record(&mut record).unwrap();            // cut in the middle to 32 KB
assert!(record["note"].as_str().unwrap().contains("...[50000]..."));

assert!(secrets::find_secret("// leftover: apikey-0123456789abcdef0123456789abcdef").is_some());
```

- `limits` — every number from the specification: 32 KB per value, 256 KB per record, 8 MB per
  batch, 64 KB of SiliconJSON, the 1 s / 16 MB flush, the 5-minute dedup window.
- `wire` — `Batch`, `Entry`, `Metadata`, `Ack`, `Rejection`, `Code`: what a daemon sends and what
  the server answers.
- `secrets` — minting, hashing and parsing table keys, access tokens, API keys and webhook
  secrets, plus `find_secret`, the scanner that refuses window code carrying any of them or an IAM
  credential (`stk-`, `ask_`, and every bearer and refresh token).
- `sanitize` — the middle cut, the `[FILETYPE:SIZE]` replacement, and the record size check.

MIT.
