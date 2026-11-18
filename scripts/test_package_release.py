import importlib.util
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_signing

spec = importlib.util.spec_from_file_location('package_release', Path(__file__).with_name('package-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class RetentionTests(unittest.TestCase):
    def test_test_profiles_and_versioned_logs_move_out_of_releases_and_prune_with_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for version in ['0.4.0', '0.5.0', '0.5.1', '0.6.0']:
                profile = root / ('release-v' + version) / 'chrome-test-profile'
                profile.mkdir(parents=True)
                (profile / 'synthetic-browser.db').write_bytes(b'synthetic browser fixture')
            (root / 'test-v04-core.log').write_text('synthetic old log')
            (root / 'test-v06-core.log').write_text('synthetic recent log')
            (root / 'user-vault.db').write_bytes(b'not a build artifact')
            release.organize_diagnostics(root)
            self.assertFalse((root / 'release-v0.6.0/chrome-test-profile').exists())
            release.prune(root)
            self.assertFalse((root / 'release-v0.4.0').exists())
            self.assertFalse((root / 'diagnostics/v0.4.0').exists())
            self.assertTrue((root / 'diagnostics/v0.6.0/test-v06-core.log').exists())
            self.assertTrue((root / 'user-vault.db').exists())

    def test_retention_sorts_versions_and_preserves_unknown_files_and_build_tools(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ['release-v0.2', 'release-v0.5.1', 'release-v0.6.0', 'release-v0.10.0', 'release', 'target', 'cargo', 'xwin', 'tools', 'personal']:
                (root / name).mkdir()
            (root / 'unknown.db').write_bytes(b'untouched')
            self.assertEqual(len(release.prune(root, 3, dry_run=True)), 3)
            self.assertTrue((root / 'release-v0.2').exists())
            retained = release.prune(root)
            self.assertEqual(retained, ['release-v0.10.0', 'release-v0.6.0', 'release-v0.5.1'])
            self.assertFalse((root / 'release-v0.2').exists())
            for name in ['release', 'target', 'cargo', 'xwin', 'tools', 'personal', 'unknown.db']:
                self.assertTrue((root / name).exists())

    def test_cleanup_refuses_linked_archives_and_vaults_inside_archives(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for version in ['0.1.0', '0.2.0', '0.3.0', '0.4.0']:
                (root / ('release-v' + version)).mkdir()
            old = root / 'release-v0.1.0'
            (old / 'vault.db').write_bytes(b'untouched')
            with self.assertRaises(RuntimeError):
                release.prune(root)
            (old / 'vault.db').unlink()
            (old / 'link').symlink_to(root / 'release-v0.4.0', target_is_directory=True)
            with self.assertRaises(RuntimeError):
                release.prune(root)
            self.assertTrue((root / 'release-v0.4.0').exists())

    def test_only_whitelisted_distribution_files_enter_package_and_outer_copies_match(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / 'source'
            build = root / 'build'
            project.mkdir(); build.mkdir()
            (project / 'Cargo.toml').write_text('[package]\nversion="0.6.0"\n')
            for path in release.PAYLOAD.values():
                file = project / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(b'synthetic release payload')
            inputs=['dist/windows-password-manager.exe','README.md','README.zh-CN.md','LICENSE','THIRD-PARTY-NOTICES.txt','installer/keyisle.nsi','assets/keyisle.ico','assets/keyisle-installer.bmp']
            for name in inputs:
                file=project/name
                if not file.exists():
                    file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(b'synthetic input')
            installer=build/'installer/Keyisle-0.6.0-Setup-x64.exe'
            installer.parent.mkdir();installer.write_bytes(b'synthetic installer')
            receipt={'version':'0.6.0','test_mode':False,'installer_sha256':hashlib.sha256(installer.read_bytes()).hexdigest(),'inputs_sha256':{name:hashlib.sha256((project/name).read_bytes()).hexdigest() for name in inputs}}
            installer.with_suffix('.build.json').write_text(json.dumps(receipt))
            with self.assertRaises(RuntimeError):
                release.publish(build, project)
            private_key = build / 'keys/release-signing.pem'
            public_key = project / 'assets/release-public.pem'
            release_signing.generate_keypair(private_key, public_key)
            original = private_key.read_bytes(), public_key.read_bytes()
            with self.assertRaises(FileExistsError):
                release_signing.generate_keypair(private_key, public_key)
            self.assertEqual((private_key.read_bytes(), public_key.read_bytes()), original)
            (project / 'private.db').write_bytes(b'never package this')
            (project/'README.md').write_bytes(b'changed after installer build')
            with self.assertRaises(RuntimeError):
                release.publish(build, project)
            self.assertFalse((build/'windows-password-manager.exe').exists())
            (project/'README.md').write_bytes(b'synthetic release payload')
            release.publish(build, project, notes='支持在线签名更新与覆盖升级')
            self.assertEqual((build / 'windows-password-manager.exe').read_bytes(),
                             (build / 'release/windows-password-manager.exe').read_bytes())
            self.assertFalse((build / 'release/private.db').exists())
            manifest = json.loads((build / 'release-v0.6.0/update.json').read_text(encoding='utf-8'))
            self.assertEqual(manifest['version'], '0.6.0')
            self.assertEqual(manifest['notes'], '支持在线签名更新与覆盖升级')
            release_signing.verify_manifest(manifest, project / 'assets/release-public.pem')
            tampered = dict(manifest, installer_size=manifest['installer_size'] + 1)
            with self.assertRaises(RuntimeError):
                release_signing.verify_manifest(tampered, project / 'assets/release-public.pem')
            with self.assertRaises(RuntimeError):
                release.publish(build, project)


if __name__ == '__main__':
    unittest.main()
