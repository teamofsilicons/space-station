# Native production deployment

Frontend: https://spacestation.teamofsilicons.com (SolidJS + Vite, Vercel).
API: https://backend.spacestation.teamofsilicons.com (Rust executable under systemd).

The `space-station-production` CloudFormation stack is in AWS `us-east-1`.
The API host (`t4g.large`, 64 GB encrypted gp3) also runs PostgreSQL 16 and Redis 7.
A separate `t4g.large` with 128 GB encrypted gp3 runs ClickHouse 26.8 LTS.
There are no containers or virtualized application runtimes. Both EC2 instances are private,
managed through SSM, and require IMDSv2. Only the dedicated HTTPS load balancer can reach the
API, and only the API security group can reach ClickHouse. PostgreSQL and Redis bind locally.
Docker Compose in `infra/local/` supplies local development and integration-test dependencies
only. Production runs one prebuilt backend executable: it is compiled off the servers, and no
server has a Rust toolchain in the deploy path.

Verified live on 2026-09-16: both native application services and backup timers are active,
neither host has a Docker or containerd service, the ALB target is healthy, and the public
API health endpoint returns `{"status":"ok"}`.

The application intentionally accepts processor JavaScript and renderer HTML in authenticated
version uploads. Its dedicated WAF counts common body-content rules that would reject this
code, while retaining IP reputation, request rate and other common checks. SQL is independently
restricted by the backend, and processors and renderers run in separate sandboxes.

## Deploy

The identifier schema change requires the coordinated [migration procedure](../../docs/PUBLIC-ID-MIGRATION.md)
before starting this backend against an existing database. Update the deployed app ID to
`spacestation` while retaining its existing app secret and encryption key.

`template.py` emits `stack.json`. The parameters select the VPC, private subnet, Ubuntu ARM64
AMI and ACM certificate; the public gateway spans two public subnets in that VPC.
Apply stack changes with CloudFormation, preserving existing parameters and IAM capabilities.
Do not delete retained instances, volumes, secrets or the artifact bucket during updates.

```sh
python3 infra/production/template.py > infra/production/stack.json
# After the infrastructure is CREATE_COMPLETE or UPDATE_COMPLETE:
python3 infra/production/deploy.py              # build, ship and install the backend executable
python3 infra/production/deploy.py --setup      # also converge both hosts' native services
python3 infra/production/deploy.py --rollback   # put the previous executable back
```

**The backend ships as one executable.** `deploy.py` builds it on the machine you run it from —
`cargo zigbuild -p space-station-backend --release --locked --target aarch64-unknown-linux-gnu.2.31`,
which needs Rust 1.98, Zig and `cargo-zigbuild`, the same tools as `scripts/build-cli-release.sh` —
or ships the one `--binary PATH` names, after checking it is an aarch64 Linux ELF. The result
links only glibc (2.31 or newer, so any Ubuntu from 20.04), with rustls instead of OpenSSL.
It goes into one release archive with this directory's host scripts, uploaded to the artifact
bucket under `releases/<sha256>.tar.gz`. Each host downloads it, checks that digest, and unpacks it
to `/opt/space-station/release`; nothing is compiled there.

On the API host `install-backend.sh` keeps the running executable as
`/opt/space-station/bin/space-station-backend.previous`, installs the new one atomically, restarts
`space-station.service` and waits up to a minute for `/api/health`. If the API is not healthy, it
puts the previous executable back and the deploy fails. `--rollback` (or `install-backend.sh
--rollback` on the host) swaps the previous one back by hand. Schema migrations run at startup,
so a schema change must stay compatible with the previous executable before it ships.

`--setup` first runs `setup-native.sh` on each host, ClickHouse first: apt packages (PostgreSQL
and Redis on the API host, ClickHouse from its LTS repository), `configure-native.py`
(configuration, the Postgres role and database, the systemd unit) and `operations.py` (logging,
CloudWatch, daily backups). The initial deployment alone uses `--initialize`, which implies
`--setup` and refuses existing secrets. Runtime credentials are fetched using each instance role
from its own Secrets Manager secret, and written only to root-readable
`/etc/space-station/runtime.env`. No credentials are in CloudFormation or SSM command strings.

`deploy.py` waits for each SSM command and prints its output; command IDs are also written to
`.local/production-commands.json`. `ssm.py api <script>` or `ssm.py clickhouse <script>` runs an
operator script.

By hand, the same deploy is: build the executable as above, then on the API host (as root) put it
at `/opt/space-station/release/space-station-backend` next to this directory's scripts and run
`bash /opt/space-station/release/infra/production/install-backend.sh`.

Hosts deployed before 2026-09-26 compiled the backend in place. After the first executable deploy
passes, their build leftovers can go: `/opt/space-station/source` (with its Cargo target cache),
`/root/.cargo` and `/root/.rustup`. The instances' first-boot user data still installs
`build-essential`, `pkg-config` and `libssl-dev`; they are unused, but changing EC2 user data
stops and restarts the instance on the next stack update, so remove them only with a planned
restart.
Do not put secrets in those scripts or print the runtime environment.

For the frontend, run `npm --prefix apps/web run check-all`. Vercel uses `apps/web` as the project
root. Upload only frontend source, public assets and project configuration: do not upload the
repository's `.local`, environment files, Cargo targets or credentials. `vercel.json` proxies
`/api/*` and both exact IAM webhook paths, and serves the SPA for application routes.
The browser connects directly to the API's WSS endpoint. Session and login cookies are Secure,
HttpOnly and scoped to `spacestation.teamofsilicons.com`, so both application hosts share them.

## Operations and recovery

Native CloudWatch agents collect API/ClickHouse error logs (30 days), memory and root disk usage.
CloudWatch alarms cover unhealthy API targets, memory above 90%, disk above 80%, failed/missing
daily backups, and EC2 system checks. System-check alarms automatically request instance recovery.
Capacity and backup alarms are visible in CloudWatch; no external notification subscription is
configured. Add an operator-approved SNS destination when an alert recipient is chosen.

At 03:15 UTC daily, systemd timers create PostgreSQL custom dumps and ClickHouse native backups,
upload them to encrypted, versioned S3, and publish a success metric. Logical backups expire after
30 days; previous object versions after 7 days. EBS snapshots run daily at 03:30 UTC and retain
7 snapshots. Secrets and encrypted data volumes are retained when a stack is removed.

`verify-backup.sh <api|clickhouse>` creates a fresh backup, downloads the latest S3 object,
restores it to an isolated probe database, checks its contents and removes that probe. Run it
through SSM after copying it from the release source. It does not overwrite the live database.
Both PostgreSQL and ClickHouse restore probes passed on 2026-09-06.

For disaster recovery, provision native replacement hosts with the same software versions,
attach retained volumes or download the selected logical backups, restore PostgreSQL with
`pg_restore` and ClickHouse with `RESTORE DATABASE`, then update runtime connection addresses and
target registration. Keep the old disks until the restored services pass readiness and queries.
The current deployment has one API/database host and one ClickHouse host; backups support recovery,
but there is no database replica or automatic application failover. A native restart briefly
interrupts service. Redis uses AOF; loss of both its durable storage and client spools can lose
records that have not yet reached ClickHouse.

## IAM

The application is `spacestation`. Its approved, active webhook is
`https://spacestation.teamofsilicons.com/webhooks/api/`. The receiver verifies IAM signatures and
records event IDs transactionally, so retries are idempotent. The live deployment has received
real IAM events. `iam app dead-letters 'spacestation'` inspects exhausted deliveries;
`iam app replay 'spacestation' --delivery <id>` retries a repaired delivery.
