#!/usr/bin/env python3
"""ECDSA P-256 (prime256v1) signing and verification for Keyisle update manifests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

P256_ORDER = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
SPKI_P256_PREFIX = bytes.fromhex('3059301306072a8648ce3d020106082a8648ce3d03010703420004')
HEX64 = re.compile(r'^[0-9a-f]{64}\Z')
HEX128 = re.compile(r'^[0-9a-f]{128}\Z')
SEMVER = re.compile(r'^\d+\.\d+\.\d+\Z')


def canonical_payload(manifest: dict) -> bytes:
    if manifest.get('schema') != 1:
        raise ValueError('Unsupported update manifest schema')
    version = str(manifest.get('version', ''))
    if not SEMVER.fullmatch(version):
        raise ValueError(f'Invalid semantic version: {version!r}')
    single_line_keys = [
        'installer_name',
        'installer_url',
        'installer_sha256',
        'portable_name',
        'portable_url',
        'portable_sha256',
        'exe_sha256',
    ]
    for key in single_line_keys:
        val = manifest.get(key)
        if not isinstance(val, str) or not val or '\n' in val or '\r' in val:
            raise ValueError(f'Invalid manifest field: {key}')
    for digest_key in ('installer_sha256', 'portable_sha256', 'exe_sha256'):
        if not HEX64.fullmatch(manifest[digest_key]):
            raise ValueError(f'Invalid SHA-256 hex in {digest_key}')
    for size_key in ('installer_size', 'portable_size'):
        size_val = manifest.get(size_key)
        if not isinstance(size_val, int) or isinstance(size_val, bool) or size_val <= 0:
            raise ValueError(f'Invalid byte size in {size_key}')
    notes = manifest.get('notes', '')
    if not isinstance(notes, str) or len(notes.encode('utf-8')) > 16384:
        raise ValueError('Invalid release notes')
    notes_sha256 = hashlib.sha256(notes.encode('utf-8')).hexdigest()
    lines = [
        'keyisle-update-v1',
        f'version={version}',
        f'installer_name={manifest["installer_name"]}',
        f'installer_url={manifest["installer_url"]}',
        f'installer_sha256={manifest["installer_sha256"]}',
        f'installer_size={manifest["installer_size"]}',
        f'portable_name={manifest["portable_name"]}',
        f'portable_url={manifest["portable_url"]}',
        f'portable_sha256={manifest["portable_sha256"]}',
        f'portable_size={manifest["portable_size"]}',
        f'exe_sha256={manifest["exe_sha256"]}',
        f'notes_sha256={notes_sha256}',
    ]
    return ('\n'.join(lines) + '\n').encode('utf-8')


def _read_der_int(buf: bytes, offset: int) -> tuple[int, int]:
    if offset + 2 > len(buf) or buf[offset] != 0x02:
        raise ValueError('Expected ASN.1 INTEGER')
    length = buf[offset + 1]
    end = offset + 2 + length
    if length == 0 or length > 33 or end > len(buf):
        raise ValueError('Invalid ECDSA integer length')
    raw = buf[offset + 2:end]
    if raw[0] & 0x80:
        raise ValueError('Negative ECDSA integer')
    if len(raw) > 1 and raw[0] == 0x00 and not (raw[1] & 0x80):
        raise ValueError('Non-minimal ECDSA integer padding')
    value = int.from_bytes(raw, 'big')
    if not (1 <= value < P256_ORDER):
        raise ValueError('ECDSA scalar out of curve order range')
    return value, end


def der_to_raw_rs(der: bytes) -> bytes:
    if len(der) < 8 or der[0] != 0x30 or der[1] + 2 != len(der):
        raise ValueError('Invalid ASN.1 SEQUENCE in ECDSA signature')
    r, pos = _read_der_int(der, 2)
    s, pos = _read_der_int(der, pos)
    if pos != len(der):
        raise ValueError('Trailing bytes in ECDSA DER signature')
    return r.to_bytes(32, 'big') + s.to_bytes(32, 'big')


def _encode_der_int(value: int) -> bytes:
    if not (1 <= value < P256_ORDER):
        raise ValueError('ECDSA scalar out of curve order range')
    raw = value.to_bytes(32, 'big').lstrip(b'\x00')
    if raw[0] & 0x80:
        raw = b'\x00' + raw
    return bytes([0x02, len(raw)]) + raw


def raw_rs_to_der(raw: bytes) -> bytes:
    if len(raw) != 64:
        raise ValueError('Expected 64-byte IEEE P1363 signature')
    r = int.from_bytes(raw[:32], 'big')
    s = int.from_bytes(raw[32:], 'big')
    body = _encode_der_int(r) + _encode_der_int(s)
    return bytes([0x30, len(body)]) + body


def extract_public_xy(public_pem_path: Path) -> bytes:
    der = subprocess.check_output(
        ['openssl', 'pkey', '-pubin', '-in', str(public_pem_path), '-outform', 'DER'],
        stderr=subprocess.DEVNULL,
    )
    if not der.startswith(SPKI_P256_PREFIX) or len(der) != len(SPKI_P256_PREFIX) + 64:
        raise RuntimeError(f'Not an uncompressed P-256 public key: {public_pem_path}')
    return der[len(SPKI_P256_PREFIX):]


def derive_public_pem(private_key_path: Path) -> bytes:
    return subprocess.check_output(
        ['openssl', 'pkey', '-in', str(private_key_path), '-pubout'],
        stderr=subprocess.DEVNULL,
    )


def generate_keypair(private_key_path: Path, public_key_path: Path) -> None:
    for path in (private_key_path, public_key_path):
        if os.path.lexists(path):
            raise FileExistsError(f'Refusing to overwrite signing key: {path}')
    private_key_path.parent.mkdir(parents=True, exist_ok=True)
    public_key_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(private_key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, 'wb') as private_file:
            subprocess.run(
                ['openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout'],
                check=True,
                stdout=private_file,
                stderr=subprocess.DEVNULL,
            )
        public_key = derive_public_pem(private_key_path)
        with public_key_path.open('xb') as public_file:
            try:
                public_file.write(public_key)
                public_file.flush()
            except OSError:
                public_key_path.unlink()
                raise
    except (OSError, subprocess.SubprocessError):
        private_key_path.unlink()
        raise


def sign_manifest(manifest: dict, private_key_path: Path, public_key_path: Path | None = None) -> dict:
    if not private_key_path.is_file():
        raise RuntimeError(f'Missing release signing private key: {private_key_path}')
    if public_key_path is not None:
        if not public_key_path.is_file():
            raise RuntimeError(f'Missing release public key: {public_key_path}')
        expected_xy = extract_public_xy(public_key_path)
        with tempfile.NamedTemporaryFile(suffix='.pem') as tmp_pub:
            Path(tmp_pub.name).write_bytes(derive_public_pem(private_key_path))
            actual_xy = extract_public_xy(Path(tmp_pub.name))
        if actual_xy != expected_xy:
            raise RuntimeError('Release signing key does not match assets/release-public.pem')
    payload = canonical_payload(manifest)
    proc = subprocess.run(
        ['openssl', 'dgst', '-sha256', '-sign', str(private_key_path)],
        input=payload,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    raw_sig = der_to_raw_rs(proc.stdout)
    signed = dict(manifest)
    signed['signature'] = raw_sig.hex()
    if public_key_path is not None:
        verify_manifest(signed, public_key_path)
    return signed


def verify_manifest(manifest: dict, public_key_path: Path) -> None:
    sig_hex = manifest.get('signature', '')
    if not isinstance(sig_hex, str) or not HEX128.fullmatch(sig_hex):
        raise RuntimeError('Missing or malformed 64-byte hex signature')
    der_sig = raw_rs_to_der(bytes.fromhex(sig_hex))
    payload = canonical_payload(manifest)
    with tempfile.NamedTemporaryFile(suffix='.sig') as sig_file:
        Path(sig_file.name).write_bytes(der_sig)
        proc = subprocess.run(
            ['openssl', 'dgst', '-sha256', '-verify', str(public_key_path), '-signature', sig_file.name],
            input=payload,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if proc.returncode != 0:
            raise RuntimeError('Update manifest ECDSA signature verification failed')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)

    kg = sub.add_parser('keygen', help='Generate a P-256 release signing keypair')
    kg.add_argument('--private-key', type=Path, required=True)
    kg.add_argument('--public-key', type=Path, required=True)

    xy = sub.add_parser('pubkey-xy', help='Print 64-byte hex uncompressed (X||Y) public key')
    xy.add_argument('--public-key', type=Path, required=True)

    sg = sub.add_parser('sign', help='Sign an update.json file in-place')
    sg.add_argument('--manifest', type=Path, required=True)
    sg.add_argument('--private-key', type=Path, required=True)
    sg.add_argument('--public-key', type=Path)

    vf = sub.add_parser('verify', help='Verify a signed update.json file')
    vf.add_argument('--manifest', type=Path, required=True)
    vf.add_argument('--public-key', type=Path, required=True)

    args = parser.parse_args()
    if args.command == 'keygen':
        generate_keypair(args.private_key, args.public_key)
    elif args.command == 'pubkey-xy':
        print(extract_public_xy(args.public_key).hex())
    elif args.command == 'sign':
        data = json.loads(args.manifest.read_text(encoding='utf-8'))
        signed = sign_manifest(data, args.private_key, args.public_key)
        args.manifest.write_text(json.dumps(signed, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    elif args.command == 'verify':
        data = json.loads(args.manifest.read_text(encoding='utf-8'))
        verify_manifest(data, args.public_key)
        print('VERIFIED OK')


if __name__ == '__main__':
    main()
