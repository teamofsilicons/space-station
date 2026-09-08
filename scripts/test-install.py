"""Offline installer checks: platform selection, safe paths, repeat installs, failed checksums."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent

class InstallerTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.home = self.base / "user's home"
        self.home.mkdir()
        self.tools = self.base / 'tools'
        self.tools.mkdir()
        self.assets = self.base / 'assets'
        self.assets.mkdir()
        checksums = []
        for platform in ('darwin', 'linux'):
            for arch in ('x64', 'arm64'):
                name = f'spacestation-{platform}-{arch}.tar.gz'
                data = f'#!/bin/sh\nprintf "%s\\n" "spacestation test-{platform}-{arch}"\n'.encode()
                with tarfile.open(self.assets / name, 'w:gz') as tar:
                    entry = tarfile.TarInfo('spacestation')
                    entry.size, entry.mode = len(data), 0o755
                    tar.addfile(entry, io.BytesIO(data))
                checksums.append(f'{hashlib.sha256((self.assets / name).read_bytes()).hexdigest()}  {name}\n')
        (self.assets / 'SHA256SUMS').write_text(''.join(checksums))
        self.script('curl', '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args=sys.argv[1:]
url=next(a for a in args if a.startswith('https://'))
shutil.copyfile(pathlib.Path(os.environ['TEST_ASSETS'])/url.rsplit('/',1)[1], args[args.index('-o')+1])
''')
        self.script('uname', '#!/bin/sh\ncase "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; esac\n')
        self.env = dict(os.environ, HOME=str(self.home), PATH=f'{self.tools}:{os.environ["PATH"]}',
                        TEST_ASSETS=str(self.assets), TEST_OS='Darwin', TEST_ARCH='arm64',
                        SPACE_STATION_SKIP_NODE='1', SHELL='/bin/zsh', ZDOTDIR=str(self.home))

    def script(self, name, text):
        path = self.tools / name
        path.write_text(text)
        path.chmod(0o755)

    def install(self):
        return subprocess.run(['sh', str(ROOT / 'scripts/install.sh')], env=self.env,
                              text=True, capture_output=True)

    def test_all_platforms_and_reinstall(self):
        for platform, arch, label in [('Darwin','arm64','darwin-arm64'), ('Darwin','x86_64','darwin-x64'),
                                      ('Linux','aarch64','linux-arm64'), ('Linux','x86_64','linux-x64')]:
            with self.subTest(platform=platform, arch=arch):
                self.env.update(TEST_OS=platform, TEST_ARCH=arch)
                result = self.install()
                self.assertEqual(result.returncode, 0, result.stderr)
                cli = subprocess.check_output([str(self.home / '.local/bin/spacestation'), '--version'], text=True)
                self.assertIn(label, cli)
        for name in ('.profile', '.bashrc', '.zshrc'):
            self.assertEqual((self.home / name).read_text().count('# spacestation PATH'), 1)
        self.assertFalse((self.home / '.bash_profile').exists())

    def test_checksum_failure_keeps_existing_install(self):
        self.assertEqual(self.install().returncode, 0)
        binary = self.home / '.local/share/spacestation/bin/spacestation'
        original = binary.read_bytes()
        (self.assets / 'spacestation-darwin-arm64.tar.gz').write_bytes(b'corrupted')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertEqual(binary.read_bytes(), original)

    def test_archive_with_extra_member_is_rejected(self):
        """Archive extraction accepts only the single expected executable."""
        archive = self.assets / 'spacestation-darwin-arm64.tar.gz'
        payload = b'#!/bin/sh\nexit 0\n'
        with tarfile.open(archive, 'w:gz') as tar:
            evil = tarfile.TarInfo('../installer-should-not-write')
            evil_data = b'owned'
            evil.size = len(evil_data)
            tar.addfile(evil, io.BytesIO(evil_data))
            entry = tarfile.TarInfo('spacestation')
            entry.size = len(payload)
            tar.addfile(entry, io.BytesIO(payload))
        sums = (self.assets / 'SHA256SUMS').read_text().splitlines()
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        sums = [f'{digest}  {archive.name}' if line.endswith(archive.name) else line for line in sums]
        (self.assets / 'SHA256SUMS').write_text('\n'.join(sums) + '\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unexpected archive contents', result.stderr)
        self.assertFalse((self.base / 'installer-should-not-write').exists())
        self.assertFalse((self.home / '.local').exists())

    def test_unsupported_arch(self):
        self.env['TEST_ARCH'] = 'riscv64'
        self.assertNotEqual(self.install().returncode, 0)
        self.assertFalse((self.home / '.local').exists())

if __name__ == '__main__':
    unittest.main()
