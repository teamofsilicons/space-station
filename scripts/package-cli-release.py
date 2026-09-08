"""Package the four native CLI binaries for the website installer."""
import hashlib
import io
from pathlib import Path
import tarfile

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'apps/web/public/downloads'
TARGETS = {
    'darwin-arm64': 'aarch64-apple-darwin',
    'darwin-x64': 'x86_64-apple-darwin',
    'linux-arm64': 'aarch64-unknown-linux-musl',
    'linux-x64': 'x86_64-unknown-linux-musl',
}
OUT.mkdir(parents=True, exist_ok=True)
checksums = []
for name, target in TARGETS.items():
    binary = (ROOT / 'target' / target / 'release/spacestation').read_bytes()
    archive = OUT / f'spacestation-{name}.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        entry = tarfile.TarInfo('spacestation')
        entry.size = len(binary)
        entry.mode = 0o755
        tar.addfile(entry, io.BytesIO(binary))
    checksums.append(f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n')
(OUT / 'SHA256SUMS').write_text(''.join(checksums))
(ROOT / 'apps/web/public/install.sh').write_bytes((ROOT / 'scripts/install.sh').read_bytes())
