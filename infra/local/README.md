# local services

```
docker compose -f infra/local/docker-compose.yml up -d --wait   # start
docker compose -f infra/local/docker-compose.yml down -v        # wipe
```

| service    | url                                              |
|------------|--------------------------------------------------|
| clickhouse | http://dev:dev@localhost:8123/space_station (native: 9000) |
| postgres   | postgres://dev:dev@localhost:5432/space_station  |
| redis      | redis://localhost:6379 (AOF, appendfsync everysec) |

Copy `.env.example` to `.env`.
