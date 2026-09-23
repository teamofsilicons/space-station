"""Run with python3 scripts/test-package-cli-release.py; no compiler or network needed."""
import hashlib
import importlib.util
from pathlib import Path
import tarfile
import tempfile

spec = importlib.util.spec_from_file_location('packager', Path(__file__).with_name('package-cli-release.py'))
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)

with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    (root / 'crates/cli').mkdir(parents=True)
    (root / 'crates/cli/Cargo.toml').write_text('[package]\nversion = "1.2.3"\n')
    (root / 'scripts').mkdir()
    (root / 'scripts/install.sh').write_text('#!/bin/sh\n')
    try:
        packager.package(root, root / 'target')
        raise AssertionError('incomplete releases must fail')
    except FileNotFoundError:
        assert not (root / 'dist').exists()
    for _, target, triple in packager.TARGETS:
        directory = root / 'target' / triple / 'release'
        directory.mkdir(parents=True)
        suffix = '.exe' if target.startswith('windows-') else ''
        (directory / f'spacestation{suffix}').write_bytes(b'standalone')
        (directory / f'spacestation-honeycomb{suffix}').write_bytes(target.encode())
    result = packager.package(root, root / 'target')
    with tarfile.open(result) as archive:
        assert len(archive.getmembers()) == 7
        manifest = archive.extractfile('honeycomb.yaml').read().decode()
        assert 'app_id: spacestation\nversion: 1.2.3\n' in manifest
        assert 'org_id:' not in manifest  # Ownership lives in application configuration.
        for _, target, _ in packager.TARGETS:
            suffix = '.exe' if target.startswith('windows-') else ''
            name = f'targets/{target}/spacestation{suffix}'
            assert f'root: targets/{target}\n' in manifest
            assert archive.extractfile(name).read() == target.encode()
            assert archive.getmember(name).mode == 0o755
    before = result.read_bytes()
    assert packager.package(root, root / 'target').read_bytes() == before
    sums = (root / 'apps/web/public/downloads/SHA256SUMS').read_text()
    assert f'{hashlib.sha256(before).hexdigest()}  {result.name}\n' in sums
    assert len(sums.splitlines()) == 7
    with tarfile.open(root / 'apps/web/public/downloads/spacestation-windows-x64.tar.gz') as archive:
        assert archive.extractfile('spacestation.exe').read() == b'standalone'
print('six-target packaging, managed payloads, deterministic bytes and missing-target checks passed')
