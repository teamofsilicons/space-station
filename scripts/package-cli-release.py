"""Package six native CLI builds for the website and Silicon Apps (Python >= 3.11)."""
import gzip
import hashlib
import io
import os
from pathlib import Path
import shutil
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
# Website asset name, Silicon Apps target, Rust target triple.
TARGETS = (
    ('darwin-arm64', 'macos-aarch64', 'aarch64-apple-darwin'),
    ('darwin-x64', 'macos-x86_64', 'x86_64-apple-darwin'),
    ('linux-arm64', 'linux-aarch64', 'aarch64-unknown-linux-musl'),
    ('linux-x64', 'linux-x86_64', 'x86_64-unknown-linux-musl'),
    ('windows-arm64', 'windows-aarch64', 'aarch64-pc-windows-msvc'),
    ('windows-x64', 'windows-x86_64', 'x86_64-pc-windows-msvc'),
)


def archive(path, entries):
    # Fixed gzip/tar metadata keeps the same release bytes reproducible.
    with path.open('wb') as output, gzip.GzipFile(filename='', fileobj=output, mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as tar:
            for name, data, mode in entries:
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                entry.mode = mode
                tar.addfile(entry, io.BytesIO(data))


def package(root=ROOT, target_dir=None):
    target_dir = target_dir or Path(os.environ.get('CARGO_TARGET_DIR', root / 'target'))
    version = tomllib.loads((root / 'crates/cli/Cargo.toml').read_text())['package']['version']
    manifest = f'schema_version: 1\napp_id: spacestation\nversion: {version}\ncommand: spacestation\ntargets:\n'
    payloads = []
    # Read every build before producing artifacts, so a missing platform fails the release.
    for asset, target, triple in TARGETS:
        suffix = '.exe' if target.startswith('windows-') else ''
        executable = f'spacestation{suffix}'
        directory = target_dir / triple / 'release'
        binary = (directory / executable).read_bytes()
        if not binary:
            raise ValueError(f'{triple}: executable is empty')
        payloads.append((asset, target, executable, binary))
        manifest += f'  {target}:\n    binary: targets/{target}/{executable}\n'

    out = root / 'apps/web/public/downloads'
    out.mkdir(parents=True, exist_ok=True)
    dist = root / 'dist'
    dist.mkdir(exist_ok=True)
    apps = dist / f'spacestation-apps-{version}.tar.gz'
    archive(apps, [('apps.yaml', manifest.encode(), 0o644)] + [
        (f'targets/{target}/{exe}', binary, 0o755) for _, target, exe, binary in payloads
    ])
    website_apps = out / apps.name
    shutil.copyfile(apps, website_apps)
    assets = [website_apps]
    for name, _, executable, binary in payloads:
        asset = out / f'spacestation-{name}.tar.gz'
        archive(asset, [(executable, binary, 0o755)])
        assets.append(asset)
    assets.extend(sorted(out.glob('*.tgz')))
    (out / 'SHA256SUMS').write_text(''.join(
        f'{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n' for asset in assets
    ))
    (root / 'apps/web/public/install.sh').write_bytes((root / 'scripts/install.sh').read_bytes())
    return apps


if __name__ == '__main__':
    print(package())
