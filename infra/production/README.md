# Native production deployment

Frontend: https://spacestation.teamofsilicons.com (SolidJS + Vite, Vercel).
API: https://backend.spacestation.teamofsilicons.com (Rust executable under systemd).

The `space-station-production` CloudFormation stack is in AWS `us-east-1`.
The API host (`t4g.large`, 64 GB encrypted gp3) also runs PostgreSQL 16 and Redis 7.
A separate `t4g.large` with 128 GB encrypted gp3 runs ClickHouse 26.8 LTS.
There are no containers or virtualized application runtimes. Both EC2 instances are private,
managed through SSM, and require IMDSv2. Only the dedicated HTTPS load balancer can reach the
API, and only the API security group can reach ClickHouse. PostgreSQL and Redis bind locally.

The application intentionally accepts processor JavaScript and renderer HTML in authenticated
version uploads. Its dedicated WAF counts common body-content rules that would reject this
code, while retaining IP reputation, request rate and other common checks. SQL is independently
restricted by the backend, and processors and renderers run in separate sandboxes.

## Deploy

`template.py` emits `stack.json`. The parameters select the VPC, private subnet, Ubuntu ARM64
AMI and ACM certificate; the public gateway spans two public subnets in that VPC.
Apply stack changes with CloudFormation, preserving existing parameters and IAM capabilities.
Do not delete retained instances, volumes, secrets or the artifact bucket during updates.

```sh
python3 infra/production/template.py > infra/production/stack.json
# After the infrastructure is CREATE_COMPLETE or UPDATE_COMPLETE:
python3 infra/production/deploy.py
```

`deploy.py` uploads a source archive to the private artifact bucket and dispatches native setup
through SSM. The initial deployment alone uses `--initialize`; it refuses existing secrets.
Runtime credentials are fetched using each instance role from its own Secrets Manager secret,
and written only to root-readable `/etc/space-station/runtime.env`. No credentials are in
CloudFormation or SSM command strings. Rust 1.98 builds the API with two build jobs.
The API executable is installed atomically and the service is restarted. Schema migrations run
at startup. Future schema changes must preserve rollback compatibility before replacing a binary.

Command IDs are written to `.local/production-commands.json`. Use AWS SSM command status/output
to inspect completion. `ssm.py api <script>` or `ssm.py clickhouse <script>` runs an operator script.
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

The application is `tos>spacestation`. Its approved, active webhook is
`https://spacestation.teamofsilicons.com/webhooks/api/`. The receiver verifies IAM signatures and
records event IDs transactionally, so retries are idempotent. The live deployment has received
real IAM events. `iam app dead-letters 'tos>spacestation'` inspects exhausted deliveries;
`iam app replay 'tos>spacestation' --delivery <id>` retries a repaired delivery.
