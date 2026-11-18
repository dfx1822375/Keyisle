#!/usr/bin/env python3
"""Publish a checked build, then retain three releases and matching diagnostics."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_signing

PROJECT = Path(__file__).resolve().parents[1]
DEFAULT_ROOT = Path('/mnt/e/CodexBuild/windows-password-manager')
GITHUB_RELEASE_BASE = 'https://github.com/dfx1822375/Keyisle/releases/download'
PAYLOAD = {
    'Keyisle.exe': 'dist/windows-password-manager.exe',
    'README.md': 'README.md',
    'README.zh-CN.md': 'README.zh-CN.md',
    'docs/storage.md': 'docs/storage.md',
    'docs/preview.png': 'docs/preview.png',
    'docs/screenshots/quick-fill.png': 'docs/screenshots/quick-fill.png',
    'docs/screenshots/account-editor.png': 'docs/screenshots/account-editor.png',
    'docs/screenshots/verification.png': 'docs/screenshots/verification.png',
    'docs/screenshots/settings.png': 'docs/screenshots/settings.png',
    'docs/screenshots/backup-manager.png': 'docs/screenshots/backup-manager.png',
    'docs/installation.md': 'docs/installation.md',
    'docs/release.md': 'docs/release.md',
    'LICENSE': 'LICENSE',
    'THIRD-PARTY-NOTICES.txt': 'THIRD-PARTY-NOTICES.txt',
}
RELEASE = re.compile(r'release-v(\d+)\.(\d+)(?:\.(\d+))?\Z')


def version_key(name):
    match = RELEASE.fullmatch(name)
    return tuple(int(value or 0) for value in match.groups()) if match else None


def safe_tree(path):
    if path.is_symlink() or not path.is_dir():
        raise RuntimeError(f'Refusing non-directory/link: {path}')
    for directory, folders, files in os.walk(path, followlinks=False):
        if any((Path(directory) / name).is_symlink() for name in folders + files):
            raise RuntimeError(f'Refusing tree containing links: {path}')


def prune(root, keep=3, dry_run=False):
    safe_tree_shallow(root)
    releases = sorted((p for p in root.iterdir() if version_key(p.name) is not None),
                      key=lambda p: version_key(p.name), reverse=True)
    retained = {version_key(p.name) for p in releases[:keep]}
    for path in releases[keep:]:
        safe_tree(path)
        # Release folders are distribution artifacts; a vault here is never a build artifact.
        if any((p.suffix == '.db' or p.name == 'last-vault.txt')
               and p.relative_to(path).parts[0] != 'chrome-test-profile' for p in path.rglob('*')):
            raise RuntimeError(f'Refusing release folder containing vault data: {path}')
        print(f'{"WOULD REMOVE" if dry_run else "REMOVE"} {path.name}')
        if not dry_run:
            shutil.rmtree(path)
    diagnostics = root / 'diagnostics'
    if diagnostics.exists():
        safe_tree_shallow(diagnostics)
        for path in sorted(diagnostics.iterdir()):
            key = version_key('release-' + path.name)
            if (key is not None and key not in retained) or path.name == 'legacy':
                safe_tree(path)
                print(f'{"WOULD REMOVE" if dry_run else "REMOVE"} diagnostics/{path.name}')
                if not dry_run:
                    shutil.rmtree(path)
    return [p.name for p in releases[:keep]]


def safe_tree_shallow(path):
    if path.is_symlink() or not path.is_dir():
        raise RuntimeError(f'Refusing invalid build directory: {path}')


def organize_diagnostics(root):
    """Only known development artifacts move; build caches and unknown files stay put."""
    for release in root.iterdir():
        if version_key(release.name) is not None or release.name == 'release':
            safe_tree_shallow(release)
            for artifact in ['chrome-test-profile', 'windows-password-manager.exe.stderr.log', 'windows-password-manager.exe.stdout.log']:
                path = release / artifact
                if path.exists():
                    version = release.name.removeprefix('release-') if release.name != 'release' else 'legacy'
                    folder = root / 'diagnostics' / version
                    folder.mkdir(parents=True, exist_ok=True)
                    path.rename(folder / (release.name + '-' + artifact))
    for path in list(root.iterdir()):
        match = re.match(r'(?:validation|test|build|storage|preview|lock|hello-status|seed-build|baseline|fetch)-v(\d+(?:\.\d+){0,2})(?:[-.]|$)', path.name)
        if match:
            raw = match[1]
            if '.' in raw:
                parts = raw.split('.')
            elif raw in {'03', '04', '05', '051', '06'}:
                parts = list(raw)
            else:
                continue
            version = '.'.join(parts + ['0'] * (3 - len(parts)))
            folder = root / 'diagnostics' / ('v' + version)
            folder.mkdir(parents=True, exist_ok=True)
            safe_tree_shallow(folder)
            destination = folder / path.name
            if destination.exists():
                raise RuntimeError(f'Diagnostic destination exists: {destination}')
            path.rename(destination)
    legacy_files = {
        'build.log', 'fetch-sqlcipher.log', 'fetch-v0.3.log', 'storage-linux.log',
        'storage-validation.log', 'storage-gui-build.log', 'storage-windows-build.log',
        'storage-verification', 'research', 'tests',
        'cancel-system-test.ps1', 'check-input-desktop.ps1', 'check-provider-v05.ps1',
        'check-session-v05.ps1', 'cleanup-candidate-v051.ps1', 'cleanup-test-v05.ps1',
        'debug-system-dialog.json', 'debug-system-dialog.png', 'inspect-system-dialog.ps1',
        'inspect-v03.ps1', 'probe-start.ps1', 'stop-old.ps1',
        'run-windows-auth-tests.ps1', 'windows-auth-test-build.jsonl',
        'windows-auth-test-path.txt', 'windows-auth-test.stderr.log', 'windows-auth-test.stdout.log',
        'run-windows-core-tests.ps1', 'windows-core-test-build.jsonl',
        'windows-core-test-path.txt', 'windows-core-test.stderr.log', 'windows-core-test.stdout.log',
    }
    for name in legacy_files:
        path = root / name
        if path.exists():
            folder = root / 'diagnostics' / 'legacy'
            folder.mkdir(parents=True, exist_ok=True)
            if not (folder / name).exists():
                path.rename(folder / name)
    stable = root / 'release'
    for name in ['preview.png', 'SHA256SUMS.txt', 'smoke-windows.ps1', 'windows-password-manager-prototype-win-x64.zip']:
        path = stable / name
        if path.is_file():
            folder = root / 'diagnostics' / 'legacy'
            folder.mkdir(parents=True, exist_ok=True)
            path.rename(folder / ('stable-' + name))


def publish(root, project=PROJECT, notes=None):
    safe_tree_shallow(root)
    package = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', (project / 'Cargo.toml').read_text())
    version = re.search(r'(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"\s*$', package[1])[1]
    if version_key('release-v' + version) is None:
        raise RuntimeError('Unsupported release version')
    destination = root / ('release-v' + version)
    if destination.exists():
        raise RuntimeError(f'Archive already exists; do not overwrite a released version: {destination}')
    signing_key = Path(os.environ['KEYISLE_SIGNING_KEY']) if os.environ.get('KEYISLE_SIGNING_KEY') else (root / 'keys/release-signing.pem')
    public_key = project / 'assets/release-public.pem'
    if not signing_key.is_file() or not public_key.is_file():
        raise RuntimeError('Missing release signing keypair; generate keys/release-signing.pem and assets/release-public.pem before publishing.')
    installer_name = f'Keyisle-{version}-Setup-x64.exe'
    installer = root / 'installer' / installer_name
    receipt_path = installer.with_suffix('.build.json')
    if not installer.is_file() or not receipt_path.is_file():
        raise RuntimeError('Build the production installer before publishing.')
    receipt = json.loads(receipt_path.read_text())
    if receipt.get('version') != version or receipt.get('test_mode') is not False:
        raise RuntimeError('Wrong version or isolated test installer; not publishable.')
    if hashlib.sha256(installer.read_bytes()).hexdigest() != receipt.get('installer_sha256'):
        raise RuntimeError('Installer changed after building.')
    required_inputs = {
        'dist/windows-password-manager.exe', 'README.md', 'README.zh-CN.md', 'LICENSE',
        'THIRD-PARTY-NOTICES.txt', 'installer/keyisle.nsi', 'assets/keyisle.ico', 'assets/keyisle-installer.bmp'
    }
    for name, digest in receipt['inputs_sha256'].items():
        if name not in required_inputs:
            raise RuntimeError('Unexpected installer input path.')
        if hashlib.sha256((project / name).read_bytes()).hexdigest() != digest:
            raise RuntimeError(f'Installer input changed; rebuild it: {name}')
    if set(receipt['inputs_sha256']) != required_inputs:
        raise RuntimeError('Incomplete installer build receipt.')
    release_notes = notes if notes is not None else os.environ.get('KEYISLE_RELEASE_NOTES', f'Keyisle v{version} 正式版更新')
    with tempfile.TemporaryDirectory(prefix='.package-', dir=root) as temporary:
        stage = Path(temporary)
        for name, source in PAYLOAD.items():
            output = stage / name
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(project / source, output)
        sums = ''.join(f'{hashlib.sha256((stage / name).read_bytes()).hexdigest()}  {name}\n' for name in sorted(PAYLOAD))
        (stage / 'PAYLOAD-SHA256SUMS').write_text(sums)
        archive_name = f'Keyisle-{version}-Windows-x64-portable.zip'
        archive = stage / archive_name
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
            for name in [*PAYLOAD, 'PAYLOAD-SHA256SUMS']:
                bundle.write(stage / name, name)
        with zipfile.ZipFile(archive) as bundle:
            assert bundle.testzip() is None
            assert set(bundle.namelist()) == {*PAYLOAD, 'PAYLOAD-SHA256SUMS'}
            assert bundle.read('Keyisle.exe') == (stage / 'Keyisle.exe').read_bytes()
        shutil.copy2(installer, stage / installer_name)
        shutil.copy2(receipt_path, stage / 'BUILD-INFO.json')
        installer_bytes = (stage / installer_name).read_bytes()
        portable_bytes = archive.read_bytes()
        exe_bytes = (stage / 'Keyisle.exe').read_bytes()
        manifest = release_signing.sign_manifest(
            {
                'schema': 1,
                'version': version,
                'published_at': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'),
                'notes': release_notes,
                'installer_name': installer_name,
                'installer_url': f'{GITHUB_RELEASE_BASE}/v{version}/{installer_name}',
                'installer_sha256': hashlib.sha256(installer_bytes).hexdigest(),
                'installer_size': len(installer_bytes),
                'portable_name': archive_name,
                'portable_url': f'{GITHUB_RELEASE_BASE}/v{version}/{archive_name}',
                'portable_sha256': hashlib.sha256(portable_bytes).hexdigest(),
                'portable_size': len(portable_bytes),
                'exe_sha256': hashlib.sha256(exe_bytes).hexdigest(),
            },
            signing_key,
            public_key,
        )
        (stage / 'update.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        downloads = ''.join(
            f'{hashlib.sha256((stage / name).read_bytes()).hexdigest()}  {name}\n'
            for name in [installer_name, archive_name, 'update.json']
        )
        (stage / 'SHA256SUMS').write_text(downloads)
        release = root / 'release'
        release.mkdir(exist_ok=True)
        safe_tree_shallow(release)
        # Retain the old entry points for existing local users; public binaries use Keyisle.exe.
        for target in [root / 'windows-password-manager.exe', release / 'windows-password-manager.exe']:
            candidate = target.with_suffix('.exe.new')
            shutil.copy2(stage / 'Keyisle.exe', candidate)
            os.replace(candidate, target)
        for name in [*PAYLOAD, 'SHA256SUMS', 'PAYLOAD-SHA256SUMS', 'update.json', installer_name]:
            target = release / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(stage / name, target)
        for old in release.glob('Keyisle-*-Setup-x64.exe'):
            if old.name != installer_name and re.fullmatch(r'Keyisle-\d+\.\d+\.\d+-Setup-x64\.exe', old.name):
                old.unlink()
        stage.rename(destination)
    expected = hashlib.sha256((destination / 'Keyisle.exe').read_bytes()).hexdigest()
    for target in [root / 'windows-password-manager.exe', root / 'release/windows-password-manager.exe', root / 'release/Keyisle.exe']:
        assert hashlib.sha256(target.read_bytes()).hexdigest() == expected
    print(f'PUBLISHED v{version}: {expected}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-root', type=Path, default=Path(os.environ.get('PASSWORD_MANAGER_BUILD_ROOT', DEFAULT_ROOT)))
    parser.add_argument('--keep', type=int, choices=range(1, 11), default=3)
    parser.add_argument('--notes', type=str, default=None, help='Release notes embedded in signed update.json')
    parser.add_argument('--prune-only', action='store_true')
    parser.add_argument('--dry-run', action='store_true', help='Only list release pruning, never publish or move files')
    args = parser.parse_args()
    if not args.prune_only and not args.dry_run:
        publish(args.build_root, notes=args.notes)
    if not args.dry_run:
        organize_diagnostics(args.build_root)
    print('KEEP:', ', '.join(prune(args.build_root, args.keep, args.dry_run)))


if __name__ == '__main__':
    main()
