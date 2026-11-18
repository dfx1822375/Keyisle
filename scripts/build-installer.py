#!/usr/bin/env python3
"""Build a per-user NSIS installer; the test variant isolates every registry write."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import shutil

PROJECT = Path(__file__).resolve().parents[1]


def build(build_root, test=False):
    version = re.search(r'^version = "([0-9.]+)"', (PROJECT / 'Cargo.toml').read_text(), re.M)[1]
    local = build_root / 'tools/nsis/usr'
    compiler = local / 'bin/makensis'
    bundled = compiler.is_file()
    if not bundled:
        compiler = Path(shutil.which('makensis') or '/nonexistent/makensis')
    if not compiler.is_file():
        raise SystemExit('NSIS is missing; install makensis or extract nsis/nsis-common into tools/nsis.')
    destination = build_root / ('diagnostics/installer-test' if test else 'installer')
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / f'Keyisle-{version}-Setup-x64{ "-test" if test else "" }.exe'
    environment = dict(os.environ)
    if bundled:
        environment['NSISDIR'] = str(local / 'share/nsis')
    arguments = [str(compiler), '-V3', '-WX', '-INPUTCHARSET', 'UTF8', f'-DVERSION={version}', f'-DOUTPUT={output}',
                 f'-DAPP_EXE={PROJECT / "dist/windows-password-manager.exe"}',
                 f'-DICON={PROJECT / "assets/keyisle.ico"}',
                 f'-DBANNER={PROJECT / "assets/keyisle-installer.bmp"}',
                 f'-DREADME={PROJECT / "README.md"}',
                 f'-DREADME_ZH={PROJECT / "README.zh-CN.md"}',
                 f'-DLICENSE={PROJECT / "LICENSE"}',
                 f'-DNOTICES={PROJECT / "THIRD-PARTY-NOTICES.txt"}']
    if test:
        arguments.append('-DTEST_MODE')
    arguments.append(str(PROJECT / 'installer/keyisle.nsi'))
    inputs = ['dist/windows-password-manager.exe', 'README.md', 'README.zh-CN.md', 'LICENSE',
              'THIRD-PARTY-NOTICES.txt', 'installer/keyisle.nsi', 'assets/keyisle.ico', 'assets/keyisle-installer.bmp']
    before = {name: hashlib.sha256((PROJECT / name).read_bytes()).hexdigest() for name in inputs}
    subprocess.run(arguments, env=environment, check=True)
    after = {name: hashlib.sha256((PROJECT / name).read_bytes()).hexdigest() for name in inputs}
    if before != after:
        output.unlink(missing_ok=True)
        output.with_suffix('.build.json').unlink(missing_ok=True)
        raise RuntimeError('Installer input changed during compilation; retry after the build finishes.')
    receipt = {'version': version, 'test_mode': test,
               'installer_sha256': hashlib.sha256(output.read_bytes()).hexdigest(),
               'inputs_sha256': before}
    output.with_suffix('.build.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(output)
    return output


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-root', type=Path, default=Path(os.environ.get('PASSWORD_MANAGER_BUILD_ROOT', '/mnt/e/CodexBuild/windows-password-manager')))
    parser.add_argument('--test', action='store_true')
    options = parser.parse_args()
    build(options.build_root, options.test)
