# Local services

`docker compose -f infra/local/docker-compose.yml up -d --wait` starts PostgreSQL on port 5433, Redis on 6379, and ClickHouse on 8123. These are local development dependencies; production uses native system services.

Use `python3 scripts/dev.py start` from the repository root to also start the backend and browser. Configure Silicon Accounts using `.env.example` and the [development guide](../../docs/DEVELOPING.md). No identity fixture, organization, or testing environment is used.

Cookies are scoped to hosts. Run separate browser stacks on separate loopback hostnames if they need independent sessions.
