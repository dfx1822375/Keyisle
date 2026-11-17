#!/usr/bin/env python3
"""Collect upstream licence text from the locked local Windows dependency graph."""
import json
import os
from pathlib import Path
import subprocess

PROJECT = Path(__file__).resolve().parents[1]
BUILD = Path(os.environ.get('PASSWORD_MANAGER_BUILD_ROOT', '/mnt/e/CodexBuild/windows-password-manager'))
environment = dict(os.environ, CARGO_HOME=str(BUILD / 'cargo'), CARGO_TARGET_DIR=str(BUILD / 'target'))
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--offline', '--locked', '--format-version=1',
    '--filter-platform=x86_64-pc-windows-msvc'], cwd=PROJECT, env=environment))
sections = [
    'KEYISLE — THIRD-PARTY NOTICES\n\nKeyisle source is licensed under MIT.\n\n' + (PROJECT / 'LICENSE').read_text(),
    'Slint is used under the Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0. '
    'The application includes AboutSlint in its About page. Alternative upstream licence expressions '
    'below do not change the licence of Keyisle. This inventory also includes build-time dependencies.']
missing = []
licence_texts = {}
nodes = {item['id']: item for item in metadata['resolve']['nodes']}
included = set()
pending = list(metadata['workspace_default_members'])
while pending:
    identifier = pending.pop()
    if identifier not in included:
        included.add(identifier)
        pending.extend(item['pkg'] for item in nodes[identifier]['deps'])
for package in sorted(metadata['packages'], key=lambda item: (item['name'], item['version'])):
    if package['source'] is None or package['id'] not in included:
        continue
    root = Path(package['manifest_path']).parent
    candidates = [path for path in root.iterdir() if path.is_file() and
                  any(word in path.name.lower() for word in ['license', 'licence', 'copying', 'copyright', 'notice'])]
    if package.get('license_file'):
        path = Path(package['license_file'])
        candidates.append(path if path.is_absolute() else root / path)
    if (root / 'LICENSES').is_dir():
        candidates.extend((root / 'LICENSES').iterdir())
    if package['name'] == 'sqlcipher-src':
        candidates.extend(path for path in root.rglob('*') if path.is_file() and
                          path.name.lower().startswith(('license', 'copying', 'notice')))
    if 'LicenseRef-Slint-Royalty-free' in (package.get('license') or ''):
        candidates = [path for path in candidates if 'Royalty-free-2.0' in path.name]
    cache = PROJECT / 'licenses/upstream' / f'{package["name"]}-{package["version"]}'
    if cache.is_dir():
        candidates.extend(cache.iterdir())
    texts = []
    for path in sorted(set(candidates)):
        if path.is_file():
            body = path.read_text(encoding='utf-8', errors='replace').strip()
            reference = licence_texts.setdefault(body, len(licence_texts) + 1)
            texts.append(f'{path.name}: licence text {reference}')
    if not texts:
        vcs = root / '.cargo_vcs_info.json'
        missing.append({'name': package['name'], 'version': package['version'], 'repository': package.get('repository'),
                        'vcs': json.loads(vcs.read_text()) if vcs.is_file() else {}})
        continue
    sections.append(f'{package["name"]} {package["version"]}\nUpstream: {package.get("license") or "see included text"}\n\n' + '\n\n'.join(texts))
nsis = BUILD / 'tools/nsis/usr/share/doc/nsis/copyright'
if nsis.is_file():
    sections.append('NSIS installer compiler and runtime\n\n' + nsis.read_text())
if missing:
    Path('/tmp/keyisle-notices-missing.json').write_text(json.dumps(missing))
    raise SystemExit('Missing upstream licence text: ' + ', '.join(f'{item["name"]} {item["version"]}' for item in missing))
sections.extend(f'LICENCE TEXT {reference}\n\n{body}' for body, reference in licence_texts.items())
output = PROJECT / 'THIRD-PARTY-NOTICES.txt'
output.write_text('\n\n' + ('\n\n' + '=' * 72 + '\n\n').join(sections) + '\n')
print(f'Collected notices for {len(metadata["packages"])} locked packages.')
