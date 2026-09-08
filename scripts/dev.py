#!/usr/bin/env python3
"""Run the local v1 stack without changing .env or the user's CLI identity."""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / ".local" / "run"
STATE = RUN / "processes.json"
BUILD_DIR = RUN / "build-directory"
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or
              (BUILD_DIR.read_text().strip() if BUILD_DIR.exists() else ROOT / "target" / "main")).resolve()
PORTS = {"iam": 8099, "backend": 8080, "web": 3000}


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def listening(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=2):
            return True
    except OSError:
        return False


def saved():
    return json.loads(STATE.read_text()) if STATE.exists() else {}


def started_at(pid):
    return subprocess.check_output(["ps", "-p", str(pid), "-o", "lstart="], text=True).strip()


def process_id(entry):
    return entry["pid"] if isinstance(entry, dict) else entry


def wait_http(url, process, seconds=90, expected=200):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Service exited; inspect logs in {RUN}")
        try:
            with urllib.request.urlopen(url, timeout=3) as response:
                if response.status == expected:
                    return
        except urllib.error.HTTPError as error:
            if error.code == expected:
                return
        except (OSError, TimeoutError):
            pass
        time.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {url}; inspect logs in {RUN}")


def stop():
    processes = saved()
    for name, entry in reversed(list(processes.items())):
        pid = process_id(entry)
        if alive(pid):
            # Verify the process group still belongs to the recorded detached leader.
            if os.getpgid(pid) != pid or (isinstance(entry, dict) and started_at(pid) != entry["started"]):
                raise RuntimeError(f"PID {pid} was reused; refusing to stop {name}")
            os.killpg(pid, signal.SIGTERM)
            print(f"Stopped {name}")
    deadline = time.monotonic() + 10
    while any(listening(PORTS[name]) for name in processes):
        if time.monotonic() >= deadline:
            raise RuntimeError("Services are still shutting down; wait and check status before restarting.")
        time.sleep(0.2)
    STATE.unlink(missing_ok=True)


def start(skip_build):
    if any(alive(process_id(entry)) for entry in saved().values()):
        raise RuntimeError("A local stack is already running. Use status or stop first.")
    busy = [str(port) for port in PORTS.values() if listening(port)]
    if busy:
        raise RuntimeError(f"Ports already in use: {', '.join(busy)}. Existing services were left alone.")
    os.umask(0o077)
    RUN.mkdir(parents=True, exist_ok=True)
    BUILD_DIR.write_text(str(TARGET))
    if not all(listening(port) for port in (8123, 5433, 6379)):
        subprocess.run(["docker", "compose", "-f", "infra/local/docker-compose.yml", "up", "-d", "--wait"],
                       cwd=ROOT, check=True, timeout=180)
    # A separate Postgres database and Redis database keep the app out of integration tests.
    admin = "postgres://dev:dev@localhost:5433/postgres"
    exists = subprocess.check_output(["psql", admin, "-Atc",
                                      "SELECT 1 FROM pg_database WHERE datname = 'space_station_v1'"], text=True)
    if not exists.strip():
        subprocess.run(["psql", admin, "-c", "CREATE DATABASE space_station_v1"], check=True)
    env = os.environ.copy()
    for line in (ROOT / ".env.example").read_text().splitlines():
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            env[key] = value.strip().strip("'\"")
    # These are a local fixture's credentials, never the real IAM environment.
    env.pop("SILICON_IAM_TEST_KEY", None)
    env.pop("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", None)
    keyfile = RUN / "encryption-key"
    if not keyfile.exists():
        keyfile.write_text(secrets.token_hex(32))
    env.update(SS_KEY=keyfile.read_text().strip(), DATABASE_URL=admin.replace("/postgres", "/space_station_v1"),
               REDIS_URL="redis://localhost:6379/6", CARGO_TARGET_DIR=str(TARGET),
               SS_BACKEND_URL="http://localhost:8080", IAM_STUB_ADDR="127.0.0.1:8099")
    env.pop("IAM_STUB_SEED", None)
    env.pop("VITE_WS_URL", None)
    if not skip_build:
        subprocess.run(["cargo", "build", "-p", "space-station-backend", "-p", "space-station-cli",
                        "--bins", "--examples"], cwd=ROOT, env=env, check=True)
    for folder in (ROOT / "apps/web", ROOT / "packages/space-station"):
        if not (folder / "node_modules").exists():
            # Both workspaces commit lockfiles; `ci` is reproducible and avoids
            # silently resolving a newer dependency during a local start.
            subprocess.run(["npm", "ci"], cwd=folder, check=True)
    processes = {}

    def spawn(name, command, cwd=ROOT):
        with (RUN / f"{name}.log").open("a") as log:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes[name] = {"pid": process.pid, "started": started_at(process.pid)}
        STATE.write_text(json.dumps(processes))
        return process

    try:
        iam = spawn("iam", [str(TARGET / "debug/examples/iam-stub")])
        wait_http("http://127.0.0.1:8099/api/version", iam, expected=422)
        backend = spawn("backend", [str(TARGET / "debug/space-station-backend")])
        wait_http("http://localhost:8080/api/health", backend)
        web = spawn("web", ["npm", "run", "dev", "--", "--host", "localhost", "--port", "3000"], ROOT / "apps/web")
        wait_http("http://localhost:3000", web)
    except BaseException:
        stop()
        raise
    print("\nSpace Station: http://localhost:3000\nBackend:       http://localhost:8080/api/health")
    print("Sign in to org tos as Alice using the local IAM page. All records persist across restarts.")
    print(f"CLI: SPACE_STATION_URL=http://localhost:8080 {TARGET / 'debug/spacestation'} login --org tos")
    print(f"Logs: {RUN}\nStop: python3 scripts/dev.py stop")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("start", "stop", "status"), nargs="?", default="start")
    parser.add_argument("--skip-build", action="store_true", help="reuse already compiled Rust binaries")
    args = parser.parse_args()
    if args.action == "start":
        start(args.skip_build)
    elif args.action == "stop":
        stop()
    else:
        for name, entry in saved().items():
            pid = process_id(entry)
            print(f"{name}: {'running' if alive(pid) else 'stopped'} (pid {pid}, port {PORTS[name]})")
        if not saved():
            print("Local stack is stopped.")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, OSError) as error:
        sys.exit(str(error))
