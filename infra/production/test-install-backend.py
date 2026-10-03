"""Check that rollback cannot restore pre-IAM-5 session handling. No services are touched."""
import os
from pathlib import Path
import subprocess
import tempfile


with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    binary = root / "bin"
    binary.mkdir()
    script = Path(__file__).with_name("install-backend.sh").read_text()
    installer = root / "install.sh"
    installer.write_text(script.replace("/opt/space-station", str(root)))
    current = binary / "space-station-backend"
    previous = binary / "space-station-backend.previous"
    current.write_text("current")
    previous.write_text("previous")
    previous.chmod(0o755)
    for command in ("systemctl", "curl", "sha256sum"):
        stub = binary / command
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o755)
    env = {**os.environ, "PATH": str(binary) + os.pathsep + os.environ["PATH"]}
    for contract in (None, "4", "5"):
        marker = root / "deployed-auth-contract.previous"
        if contract is not None:
            marker.write_text(contract + "\n")
        result = subprocess.run(["bash", str(installer), "--rollback"], env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == (contract == "5"), result.stderr
        assert current.read_text() == ("previous" if contract == "5" else "current")
    assert (root / "deployed-auth-contract").read_text().strip() == "5"
print("IAM 5 rollback boundary passed")
