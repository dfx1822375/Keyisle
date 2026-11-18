//! Authenticated release manifest verification, WinHTTP background downloader, and in-place updater.
use std::{cmp::Ordering as CmpOrdering, fs, path::Path};
#[cfg(windows)]
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

pub const RELEASE_PAGE_URL: &str = "https://github.com/dfx1822375/Keyisle/releases/latest";
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/dfx1822375/Keyisle/releases/latest/download/update.json";
pub const PINNED_DOWNLOAD_PREFIX: &str =
    "https://github.com/dfx1822375/Keyisle/releases/download/v";

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// Uncompressed ECDSA P-256 `(X || Y)` public key matching `assets/release-public.pem`.
pub const RELEASE_PUBLIC_KEY_XY: [u8; 64] = [
    0x85, 0xd7, 0xd8, 0x4f, 0x90, 0x3b, 0xd8, 0x8f, 0x27, 0x4a, 0xa4, 0xea, 0x51, 0xe7, 0x42, 0x2b,
    0xb9, 0x8d, 0x84, 0xe5, 0xdf, 0xab, 0x7f, 0x8d, 0x4a, 0x1e, 0x4e, 0xed, 0x94, 0x25, 0x8c, 0x05,
    0x6b, 0x27, 0xea, 0xd8, 0xcb, 0x1f, 0xb2, 0x8a, 0x61, 0x7f, 0x31, 0xfc, 0xd9, 0x35, 0x5b, 0x64,
    0x94, 0x00, 0x43, 0x43, 0x37, 0xc0, 0xd2, 0x4a, 0x2c, 0xc6, 0x92, 0x1a, 0x28, 0x7d, 0x3b, 0x2f,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut parts = text.split('.');
        let parse_part = |part: Option<&str>| -> Result<u32, String> {
            let s = part.ok_or_else(|| "版本号格式无效。".to_string())?;
            if s.is_empty()
                || (s.len() > 1 && s.starts_with('0'))
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("版本号格式无效。".into());
            }
            s.parse::<u32>().map_err(|_| "版本号超出支持范围。".into())
        };
        let major = parse_part(parts.next())?;
        let minor = parse_part(parts.next())?;
        let patch = parse_part(parts.next())?;
        if parts.next().is_some() {
            return Err("版本号格式无效。".into());
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    pub published_at: String,
    pub notes: String,
    pub installer_name: String,
    pub installer_url: String,
    pub installer_sha256: [u8; 32],
    pub installer_size: u64,
    pub portable_name: String,
    pub portable_url: String,
    pub portable_sha256: [u8; 32],
    pub portable_size: u64,
    pub exe_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate,
    Available(UpdateManifest),
}

// ==================== PURE-RUST STREAMING SHA-256 (FIPS 180-4) ====================

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0u8; 64],
            buffer_len: 0,
            total_bytes: 0,
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            let j = i * 4;
            w[i] = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        if self.buffer_len > 0 {
            let need = 64 - self.buffer_len;
            if data.len() < need {
                self.buffer[self.buffer_len..self.buffer_len + data.len()].copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            }
            self.buffer[self.buffer_len..64].copy_from_slice(&data[..need]);
            Self::compress(&mut self.state, &self.buffer);
            self.buffer_len = 0;
            data = &data[need..];
        }
        while data.len() >= 64 {
            let block: &[u8; 64] = data[..64].try_into().unwrap();
            Self::compress(&mut self.state, block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..64].fill(0);
            Self::compress(&mut self.state, &self.buffer);
            self.buffer_len = 0;
        }
        self.buffer[self.buffer_len..56].fill(0);
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        Self::compress(&mut self.state, &self.buffer);
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode_lowercase<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != N * 2 {
        return Err("十六进制校验值长度无效。".into());
    }
    let nibble = |b: u8| -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("十六进制校验值包含无效字符。".into()),
        }
    };
    let raw = text.as_bytes();
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (nibble(raw[i * 2])? << 4) | nibble(raw[i * 2 + 1])?;
    }
    Ok(out)
}

// ==================== PURE-RUST SAFE ECDSA P-256 (FIPS 186-5) ====================

#[derive(Clone, Copy, PartialEq, Eq)]
struct U256([u64; 4]);

impl U256 {
    const ZERO: Self = Self([0, 0, 0, 0]);
    const ONE: Self = Self([1, 0, 0, 0]);

    const fn from_u64(v: u64) -> Self {
        Self([v, 0, 0, 0])
    }

    fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        Self([
            u64::from_be_bytes(bytes[24..32].try_into().unwrap()),
            u64::from_be_bytes(bytes[16..24].try_into().unwrap()),
            u64::from_be_bytes(bytes[8..16].try_into().unwrap()),
            u64::from_be_bytes(bytes[0..8].try_into().unwrap()),
        ])
    }

    fn is_zero(self) -> bool {
        self.0 == [0, 0, 0, 0]
    }

    fn bit(self, index: usize) -> bool {
        ((self.0[index / 64] >> (index % 64)) & 1) != 0
    }

    fn cmp(self, other: Self) -> CmpOrdering {
        for i in (0..4).rev() {
            if self.0[i] != other.0[i] {
                return self.0[i].cmp(&other.0[i]);
            }
        }
        CmpOrdering::Equal
    }

    fn ge(self, other: Self) -> bool {
        !matches!(self.cmp(other), CmpOrdering::Less)
    }

    fn add_raw(self, other: Self) -> (Self, bool) {
        let mut out = [0u64; 4];
        let mut carry = 0u128;
        for i in 0..4 {
            let sum = (self.0[i] as u128) + (other.0[i] as u128) + carry;
            out[i] = sum as u64;
            carry = sum >> 64;
        }
        (Self(out), carry != 0)
    }

    fn sub_raw(self, other: Self) -> (Self, bool) {
        let mut out = [0u64; 4];
        let mut borrow = 0u128;
        for i in 0..4 {
            let lhs = self.0[i] as u128;
            let rhs = (other.0[i] as u128) + borrow;
            if lhs >= rhs {
                out[i] = (lhs - rhs) as u64;
                borrow = 0;
            } else {
                out[i] = ((1u128 << 64) + lhs - rhs) as u64;
                borrow = 1;
            }
        }
        (Self(out), borrow != 0)
    }

    fn add_mod(self, other: Self, m: Self) -> Self {
        let (sum, overflow) = self.add_raw(other);
        if overflow || sum.ge(m) {
            sum.sub_raw(m).0
        } else {
            sum
        }
    }

    fn sub_mod(self, other: Self, m: Self) -> Self {
        let (diff, borrow) = self.sub_raw(other);
        if borrow { diff.add_raw(m).0 } else { diff }
    }

    fn mul_mod(self, mut other: Self, m: Self) -> Self {
        if other.ge(m) {
            other = other.sub_raw(m).0;
        }
        let mut acc = Self::ZERO;
        for bit in (0..256).rev() {
            acc = acc.add_mod(acc, m);
            if self.bit(bit) {
                acc = acc.add_mod(other, m);
            }
        }
        acc
    }

    fn inv_mod(self, m: Self) -> Self {
        let exp = m.sub_raw(Self::from_u64(2)).0;
        let mut res = Self::ONE;
        let mut base = self;
        for bit in 0..256 {
            if exp.bit(bit) {
                res = res.mul_mod(base, m);
            }
            base = base.mul_mod(base, m);
        }
        res
    }
}

const P256_P: U256 = U256([
    0xffffffffffffffff,
    0x00000000ffffffff,
    0x0000000000000000,
    0xffffffff00000001,
]);
const P256_N: U256 = U256([
    0xf3b9cac2fc632551,
    0xbce6faada7179e84,
    0xffffffffffffffff,
    0xffffffff00000000,
]);
const P256_B: U256 = U256([
    0x3bce3c3e27d2604b,
    0x651d06b0cc53b0f6,
    0xb3ebbd55769886bc,
    0x5ac635d8aa3a93e7,
]);
const P256_GX: U256 = U256([
    0xf4a13945d898c296,
    0x77037d812deb33a0,
    0xf8bce6e563a440f2,
    0x6b17d1f2e12c4247,
]);
const P256_GY: U256 = U256([
    0xcbb6406837bf51f5,
    0x2bce33576b315ece,
    0x8ee7eb4a7c0f9e16,
    0x4fe342e2fe1a7f9b,
]);

#[derive(Clone, Copy)]
struct AffinePoint {
    x: U256,
    y: U256,
    infinity: bool,
}

impl AffinePoint {
    const INFINITY: Self = Self {
        x: U256::ZERO,
        y: U256::ZERO,
        infinity: true,
    };

    fn is_on_curve(x: U256, y: U256) -> bool {
        if x.ge(P256_P) || y.ge(P256_P) {
            return false;
        }
        let lhs = y.mul_mod(y, P256_P);
        let x2 = x.mul_mod(x, P256_P);
        let x3 = x2.mul_mod(x, P256_P);
        let three_x = x.add_mod(x, P256_P).add_mod(x, P256_P);
        let rhs = x3.sub_mod(three_x, P256_P).add_mod(P256_B, P256_P);
        lhs == rhs
    }

    fn double(self) -> Self {
        if self.infinity || self.y.is_zero() {
            return Self::INFINITY;
        }
        let x2 = self.x.mul_mod(self.x, P256_P);
        let three_x2 = x2.add_mod(x2, P256_P).add_mod(x2, P256_P);
        let num = three_x2.sub_mod(U256::from_u64(3), P256_P);
        let den = self.y.add_mod(self.y, P256_P).inv_mod(P256_P);
        let lambda = num.mul_mod(den, P256_P);
        let x3 = lambda
            .mul_mod(lambda, P256_P)
            .sub_mod(self.x.add_mod(self.x, P256_P), P256_P);
        let y3 = lambda
            .mul_mod(self.x.sub_mod(x3, P256_P), P256_P)
            .sub_mod(self.y, P256_P);
        Self {
            x: x3,
            y: y3,
            infinity: false,
        }
    }

    fn add(self, other: Self) -> Self {
        if self.infinity {
            return other;
        }
        if other.infinity {
            return self;
        }
        if self.x == other.x {
            return if self.y == other.y {
                self.double()
            } else {
                Self::INFINITY
            };
        }
        let num = other.y.sub_mod(self.y, P256_P);
        let den = other.x.sub_mod(self.x, P256_P).inv_mod(P256_P);
        let lambda = num.mul_mod(den, P256_P);
        let x3 = lambda
            .mul_mod(lambda, P256_P)
            .sub_mod(self.x, P256_P)
            .sub_mod(other.x, P256_P);
        let y3 = lambda
            .mul_mod(self.x.sub_mod(x3, P256_P), P256_P)
            .sub_mod(self.y, P256_P);
        Self {
            x: x3,
            y: y3,
            infinity: false,
        }
    }

    fn scalar_mul(self, k: U256) -> Self {
        let mut res = Self::INFINITY;
        let mut cur = self;
        for bit in 0..256 {
            if k.bit(bit) {
                res = res.add(cur);
            }
            cur = cur.double();
        }
        res
    }
}

pub fn verify_p256_sha256(pubkey_xy: &[u8; 64], message: &[u8], sig_rs: &[u8; 64]) -> bool {
    let qx = U256::from_be_bytes(pubkey_xy[0..32].try_into().unwrap());
    let qy = U256::from_be_bytes(pubkey_xy[32..64].try_into().unwrap());
    if !AffinePoint::is_on_curve(qx, qy) {
        return false;
    }
    let r = U256::from_be_bytes(sig_rs[0..32].try_into().unwrap());
    let s = U256::from_be_bytes(sig_rs[32..64].try_into().unwrap());
    if r.is_zero() || r.ge(P256_N) || s.is_zero() || s.ge(P256_N) {
        return false;
    }
    let digest = sha256(message);
    let mut z = U256::from_be_bytes(&digest);
    if z.ge(P256_N) {
        z = z.sub_raw(P256_N).0;
    }
    let w = s.inv_mod(P256_N);
    let u1 = z.mul_mod(w, P256_N);
    let u2 = r.mul_mod(w, P256_N);
    let g = AffinePoint {
        x: P256_GX,
        y: P256_GY,
        infinity: false,
    };
    let q = AffinePoint {
        x: qx,
        y: qy,
        infinity: false,
    };
    let point = g.scalar_mul(u1).add(q.scalar_mul(u2));
    if point.infinity {
        return false;
    }
    let rx_mod_n = if point.x.ge(P256_N) {
        point.x.sub_raw(P256_N).0
    } else {
        point.x
    };
    rx_mod_n == r
}

// ==================== STRICT ZERO-DEPENDENCY JSON & MANIFEST VERIFIER ====================

#[derive(Clone, Debug)]
enum JsonVal {
    Str(String),
    Int(u64),
    Other,
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err("更新清单 JSON 语法无效。".into())
        }
    }

    fn read_hex4(&mut self) -> Result<u16, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err("更新清单 Unicode 转义无效。".into());
        }
        let mut val = 0u16;
        for _ in 0..4 {
            let b = self.bytes[self.pos];
            self.pos += 1;
            let digit = match b {
                b'0'..=b'9' => (b - b'0') as u16,
                b'a'..=b'f' => (b - b'a' + 10) as u16,
                b'A'..=b'F' => (b - b'A' + 10) as u16,
                _ => return Err("更新清单 Unicode 转义无效。".into()),
            };
            val = (val << 4) | digit;
        }
        Ok(val)
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let start_limit = self.pos;
        let mut raw_utf8 = Vec::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            if b == b'"' {
                return String::from_utf8(raw_utf8)
                    .map_err(|_| "更新清单编码不是有效 UTF-8。".into());
            }
            if b < 0x20 {
                return Err("更新清单包含未转义的控制字符。".into());
            }
            if b != b'\\' {
                raw_utf8.push(b);
            } else {
                if self.pos >= self.bytes.len() {
                    return Err("更新清单转义序列未闭合。".into());
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' | b'\\' | b'/' => raw_utf8.push(esc),
                    b'b' => raw_utf8.push(0x08),
                    b'f' => raw_utf8.push(0x0c),
                    b'n' => raw_utf8.push(b'\n'),
                    b'r' => raw_utf8.push(b'\r'),
                    b't' => raw_utf8.push(b'\t'),
                    b'u' => {
                        let first = self.read_hex4()?;
                        let ch = if (0xd800..=0xdbff).contains(&first) {
                            if self.pos + 2 > self.bytes.len()
                                || self.bytes[self.pos] != b'\\'
                                || self.bytes[self.pos + 1] != b'u'
                            {
                                return Err("更新清单代理对不完整。".into());
                            }
                            self.pos += 2;
                            let second = self.read_hex4()?;
                            if !(0xdc00..=0xdfff).contains(&second) {
                                return Err("更新清单代理对无效。".into());
                            }
                            let code = 0x10000
                                + (((first - 0xd800) as u32) << 10)
                                + ((second - 0xdc00) as u32);
                            char::from_u32(code).ok_or("更新清单字符无效。")?
                        } else {
                            char::from_u32(first as u32).ok_or("更新清单字符无效。")?
                        };
                        let mut buf = [0u8; 4];
                        raw_utf8.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err("更新清单包含不支持的转义序列。".into()),
                }
            }
            if self.pos - start_limit > MAX_MANIFEST_BYTES {
                return Err("更新清单字段过长。".into());
            }
        }
        Err("更新清单字符串未闭合。".into())
    }

    fn parse_uint(&mut self) -> Result<u64, String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return Err("更新清单数字字段无效。".into());
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        if s.len() > 1 && s.starts_with('0') {
            return Err("更新清单数字格式无效。".into());
        }
        s.parse::<u64>()
            .map_err(|_| "更新清单数值超出范围。".into())
    }

    fn skip_nested(&mut self, open: u8, close: u8, depth: usize) -> Result<(), String> {
        if depth > 8 {
            return Err("更新清单嵌套过深。".into());
        }
        self.expect(open)?;
        if self.peek() == Some(close) {
            self.pos += 1;
            return Ok(());
        }
        loop {
            if open == b'{' {
                let _ = self.parse_string()?;
                self.expect(b':')?;
            }
            let _ = self.parse_value(depth + 1)?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(c) if c == close => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err("更新清单复合结构无效。".into()),
            }
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonVal, String> {
        match self.peek() {
            Some(b'"') => self.parse_string().map(JsonVal::Str),
            Some(b'0'..=b'9') => self.parse_uint().map(JsonVal::Int),
            Some(b'{') => self.skip_nested(b'{', b'}', depth).map(|_| JsonVal::Other),
            Some(b'[') => self.skip_nested(b'[', b']', depth).map(|_| JsonVal::Other),
            Some(b't') if self.bytes[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(JsonVal::Other)
            }
            Some(b'f') if self.bytes[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(JsonVal::Other)
            }
            Some(b'n') if self.bytes[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(JsonVal::Other)
            }
            _ => Err("更新清单字段类型不支持。".into()),
        }
    }

    fn parse_top_object(&mut self) -> Result<std::collections::BTreeMap<String, JsonVal>, String> {
        self.expect(b'{')?;
        let mut map = std::collections::BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(map);
        }
        loop {
            let key = self.parse_string()?;
            self.expect(b':')?;
            let val = self.parse_value(1)?;
            if map.insert(key, val).is_some() {
                return Err("更新清单包含重复字段。".into());
            }
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("更新清单 JSON 对象未闭合。".into()),
            }
        }
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err("更新清单包含多余尾部数据。".into());
        }
        Ok(map)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
    pub origin: String,
}

pub fn parse_update_url(url: &str, allow_loopback_http: bool) -> Result<ParsedUrl, String> {
    if url.is_empty()
        || url.len() > 2048
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url.contains('#')
    {
        return Err("更新地址格式无效。".into());
    }
    let (secure, rest, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest, 443u16)
    } else if allow_loopback_http {
        if let Some(rest) = url.strip_prefix("http://") {
            (false, rest, 80u16)
        } else {
            return Err("更新地址必须使用 HTTPS。".into());
        }
    } else {
        return Err("更新地址必须使用 HTTPS。".into());
    };
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => return Err("更新地址缺少资源路径。".into()),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err("更新地址主机名无效。".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            let port = p
                .parse::<u16>()
                .ok()
                .filter(|&v| v > 0)
                .ok_or("更新地址端口无效。")?;
            (h, port)
        }
        Some(_) => return Err("更新地址主机或端口格式无效。".into()),
        None => (authority, default_port),
    };
    if !secure && host != "127.0.0.1" {
        return Err("非加密 HTTP 仅允许本机隔离测试地址 127.0.0.1。".into());
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    {
        return Err("更新地址主机名包含无效字符。".into());
    }
    let scheme = if secure { "https" } else { "http" };
    let origin = if port == default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    };
    Ok(ParsedUrl {
        secure,
        host: host.to_ascii_lowercase(),
        port,
        path_and_query: path_and_query.to_owned(),
        origin,
    })
}

fn safe_asset_name(name: &str, expected: &str) -> Result<(), String> {
    if name != expected
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(char::is_control)
    {
        return Err("更新清单中的安装包文件名不符合安全规范。".into());
    }
    Ok(())
}

pub fn verify_and_evaluate_manifest(
    json_bytes: &[u8],
    current_version: &str,
    pubkey_xy: &[u8; 64],
    custom_origin: Option<&str>,
    allow_loopback_http: bool,
) -> Result<CheckOutcome, String> {
    if json_bytes.is_empty() || json_bytes.len() > MAX_MANIFEST_BYTES {
        return Err("更新清单大小超出安全限制。".into());
    }
    let current = SemVer::parse(current_version)?;
    let map = JsonParser::new(json_bytes).parse_top_object()?;

    let get_str = |key: &str| -> Result<String, String> {
        match map.get(key) {
            Some(JsonVal::Str(s)) => Ok(s.clone()),
            _ => Err(format!("更新清单缺少有效文本字段：{key}。")),
        }
    };
    let get_uint = |key: &str| -> Result<u64, String> {
        match map.get(key) {
            Some(JsonVal::Int(n)) => Ok(*n),
            _ => Err(format!("更新清单缺少有效数值字段：{key}。")),
        }
    };

    if get_uint("schema")? != 1 {
        return Err("当前版本不支持该更新清单格式，请前往发布页手动下载。".into());
    }
    let version = get_str("version")?;
    let remote = SemVer::parse(&version)?;
    let published_at = match map.get("published_at") {
        Some(JsonVal::Str(s)) if s.len() <= 64 && !s.contains('\n') && !s.contains('\r') => {
            s.clone()
        }
        _ => String::new(),
    };
    let notes = match map.get("notes") {
        Some(JsonVal::Str(s)) if s.len() <= 16384 => s.clone(),
        None => String::new(),
        _ => return Err("更新说明内容过长或格式无效。".into()),
    };

    let installer_name = get_str("installer_name")?;
    let installer_url = get_str("installer_url")?;
    let installer_sha256_hex = get_str("installer_sha256")?;
    let installer_size = get_uint("installer_size")?;

    let portable_name = get_str("portable_name")?;
    let portable_url = get_str("portable_url")?;
    let portable_sha256_hex = get_str("portable_sha256")?;
    let portable_size = get_uint("portable_size")?;

    let exe_sha256_hex = get_str("exe_sha256")?;
    let signature_hex = get_str("signature")?;

    for single in [
        &installer_name,
        &installer_url,
        &portable_name,
        &portable_url,
    ] {
        if single.is_empty() || single.contains('\n') || single.contains('\r') {
            return Err("更新清单字段包含非法换行或空值。".into());
        }
    }

    safe_asset_name(&installer_name, &format!("Keyisle-{version}-Setup-x64.exe"))?;
    safe_asset_name(
        &portable_name,
        &format!("Keyisle-{version}-Windows-x64-portable.zip"),
    )?;

    if !(1..=MAX_ASSET_BYTES).contains(&installer_size)
        || !(1..=MAX_ASSET_BYTES).contains(&portable_size)
    {
        return Err("更新清单安装包大小超出安全限制。".into());
    }

    let installer_sha256 = hex_decode_lowercase::<32>(&installer_sha256_hex)?;
    let portable_sha256 = hex_decode_lowercase::<32>(&portable_sha256_hex)?;
    let exe_sha256 = hex_decode_lowercase::<32>(&exe_sha256_hex)?;
    let signature = hex_decode_lowercase::<64>(&signature_hex)?;

    let parsed_inst = parse_update_url(&installer_url, allow_loopback_http)?;
    let parsed_port = parse_update_url(&portable_url, allow_loopback_http)?;
    if let Some(origin) = custom_origin {
        if parsed_inst.origin != origin || parsed_port.origin != origin {
            return Err("更新包下载地址与更新源域名不一致，已拒绝加载。".into());
        }
    } else {
        let expected_inst = format!("{PINNED_DOWNLOAD_PREFIX}{version}/{installer_name}");
        let expected_port = format!("{PINNED_DOWNLOAD_PREFIX}{version}/{portable_name}");
        if installer_url != expected_inst || portable_url != expected_port {
            return Err("更新包下载地址不属于官方固定仓库，已拒绝加载。".into());
        }
    }

    let notes_sha256_hex = hex_encode(&sha256(notes.as_bytes()));
    let canonical = format!(
        "keyisle-update-v1\n\
         version={version}\n\
         installer_name={installer_name}\n\
         installer_url={installer_url}\n\
         installer_sha256={installer_sha256_hex}\n\
         installer_size={installer_size}\n\
         portable_name={portable_name}\n\
         portable_url={portable_url}\n\
         portable_sha256={portable_sha256_hex}\n\
         portable_size={portable_size}\n\
         exe_sha256={exe_sha256_hex}\n\
         notes_sha256={notes_sha256_hex}\n"
    );

    if !verify_p256_sha256(pubkey_xy, canonical.as_bytes(), &signature) {
        return Err("更新清单发布者签名校验未通过，已拒绝此更新。".into());
    }

    match remote.cmp(&current) {
        CmpOrdering::Less => Err("更新清单版本低于当前已安装版本，已拒绝降级。".into()),
        CmpOrdering::Equal => Ok(CheckOutcome::UpToDate),
        CmpOrdering::Greater => Ok(CheckOutcome::Available(UpdateManifest {
            version,
            published_at,
            notes,
            installer_name,
            installer_url,
            installer_sha256,
            installer_size,
            portable_name,
            portable_url,
            portable_sha256,
            portable_size,
            exe_sha256,
        })),
    }
}

fn ensure_regular_path(path: &Path, expect_dir: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "无法检查更新缓存目录状态。".to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("更新目录包含重解析点或符号链接，已停止操作。".into());
        }
    }
    if meta.file_type().is_symlink()
        || meta.is_dir() != expect_dir
        || (!expect_dir && !meta.is_file())
    {
        return Err("更新目录包含异常文件类型，已停止操作。".into());
    }
    Ok(())
}

fn clean_regular_dir_contents(dir: &Path, depth: usize) -> Result<(), String> {
    if depth > 4 {
        return Err("更新缓存目录层级异常。".into());
    }
    ensure_regular_path(dir, true)?;
    for entry in fs::read_dir(dir).map_err(|_| "无法读取更新缓存目录。".to_string())? {
        let entry = entry.map_err(|_| "无法读取更新缓存项。".to_string())?;
        let path = entry.path();
        let meta =
            fs::symlink_metadata(&path).map_err(|_| "无法读取更新缓存元数据。".to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("更新缓存包含链接，拒绝清理。".into());
            }
        }
        if meta.file_type().is_symlink() {
            return Err("更新缓存包含链接，拒绝清理。".into());
        }
        if meta.is_dir() {
            let _ = clean_regular_dir_contents(&path, depth + 1);
            let _ = fs::remove_dir(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

/// Best-effort cleanup of `<data_dir>/updates` staging directory on startup or reset.
pub fn cleanup_staging(root: &Path) {
    let staging = root.join("updates");
    if fs::symlink_metadata(&staging).is_ok() && clean_regular_dir_contents(&staging, 0).is_ok() {
        let _ = fs::remove_dir(&staging);
    }
}

// ==================== WINDOWS WINHTTP & IN-PLACE UPDATER ====================

#[cfg(windows)]
#[derive(Clone, Debug)]
pub enum PreparedUpdate {
    Installer {
        setup_exe: PathBuf,
        install_dir: PathBuf,
    },
    Portable {
        updater_exe: PathBuf,
        target_exe: PathBuf,
    },
}

#[cfg(windows)]
pub fn is_installed_mode() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("Uninstall.exe")))
        .is_some_and(|uninst| ensure_regular_path(&uninst, false).is_ok())
}

#[cfg(windows)]
mod win_net {
    use super::*;
    use std::{
        io::{self, Write},
        mem::size_of,
        os::windows::{ffi::OsStrExt, process::CommandExt},
        process::Command,
        ptr::{null, null_mut},
        thread,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError},
        Networking::WinHttp::*,
        Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, SYNCHRONIZE,
        },
        System::Threading::{OpenProcess, WaitForSingleObject},
        UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    struct WinHttpHandle(*mut core::ffi::c_void);
    impl Drop for WinHttpHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn check_canceled(cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) {
            Err("已取消更新操作。".into())
        } else {
            Ok(())
        }
    }

    fn configured_manifest_url() -> Result<(ParsedUrl, Option<String>, bool), String> {
        let allow_loopback = std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_some();
        if allow_loopback && let Ok(raw) = std::env::var("KEYISLE_UPDATE_URL") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let parsed = parse_update_url(trimmed, allow_loopback)?;
                if parsed.host != "127.0.0.1" {
                    return Err("隔离更新测试仅支持本机地址 127.0.0.1。".into());
                }
                let origin = parsed.origin.clone();
                return Ok((parsed, Some(origin), allow_loopback));
            }
        }
        let parsed = parse_update_url(DEFAULT_MANIFEST_URL, false)?;
        Ok((parsed, None, false))
    }

    fn open_request(
        url: &ParsedUrl,
        cancel: &AtomicBool,
    ) -> Result<(WinHttpHandle, WinHttpHandle, WinHttpHandle, u32), String> {
        check_canceled(cancel)?;
        let agent = wide(&format!("Keyisle/{}", env!("CARGO_PKG_VERSION")));
        let session_ptr = unsafe {
            if url.host == "127.0.0.1" {
                WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null(),
                    null(),
                    0,
                )
            } else {
                let auto = WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    0,
                );
                if !auto.is_null() {
                    auto
                } else {
                    WinHttpOpen(
                        agent.as_ptr(),
                        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                        null(),
                        null(),
                        0,
                    )
                }
            }
        };
        if session_ptr.is_null() {
            return Err(format!(
                "无法初始化系统网络组件（错误 {}）。",
                unsafe { GetLastError() }
            ));
        }
        let session = WinHttpHandle(session_ptr);
        unsafe {
            let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 15_000);
            if url.secure {
                let protocols: u32 =
                    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
                let _ = WinHttpSetOption(
                    session.0,
                    WINHTTP_OPTION_SECURE_PROTOCOLS,
                    (&protocols as *const u32).cast(),
                    size_of::<u32>() as u32,
                );
            }
        }
        let host_w = wide(&url.host);
        let conn_ptr = unsafe { WinHttpConnect(session.0, host_w.as_ptr(), url.port, 0) };
        if conn_ptr.is_null() {
            return Err("无法连接到更新服务器，请检查网络连接。".into());
        }
        let connection = WinHttpHandle(conn_ptr);

        let verb_w = wide("GET");
        let path_w = wide(&url.path_and_query);
        let flags = if url.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let req_ptr = unsafe {
            WinHttpOpenRequest(
                connection.0,
                verb_w.as_ptr(),
                path_w.as_ptr(),
                null(),
                null(),
                null(),
                flags,
            )
        };
        if req_ptr.is_null() {
            return Err("无法创建更新检查请求。".into());
        }
        let request = WinHttpHandle(req_ptr);
        unsafe {
            let disable: u32 = WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION;
            if WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_DISABLE_FEATURE,
                (&disable as *const u32).cast(),
                size_of::<u32>() as u32,
            ) == 0
            {
                return Err("无法禁用更新请求的 Cookie 和身份验证，已停止请求。".into());
            }
            if !url.secure {
                let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
                if WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_REDIRECT_POLICY,
                    (&policy as *const u32).cast(),
                    size_of::<u32>() as u32,
                ) == 0
                {
                    return Err("无法隔离测试更新请求，已停止请求。".into());
                }
            }
        }
        check_canceled(cancel)?;
        let sent = unsafe { WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) };
        if sent == 0 {
            return Err("连接更新服务器超时或网络不可用，请检查网络后重试。".into());
        }
        check_canceled(cancel)?;
        let received = unsafe { WinHttpReceiveResponse(request.0, null_mut()) };
        if received == 0 {
            return Err("等待更新服务器响应超时，请稍后重试。".into());
        }
        let mut status: u32 = 0;
        let mut status_len = size_of::<u32>() as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut status_len,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err("无法读取更新服务器响应状态。".into());
        }
        Ok((session, connection, request, status))
    }

    pub fn check_for_updates(cancel: &AtomicBool) -> Result<CheckOutcome, String> {
        let (url, custom_origin, allow_loopback) = configured_manifest_url()?;
        let (_session, _conn, request, status) = open_request(&url, cancel)?;
        if status == 404 {
            return Err(
                "发布页暂未附带在线更新清单（HTTP 404），可点击“打开发布页”查看最新版本。".into(),
            );
        }
        if status != 200 {
            return Err(format!(
                "更新服务器返回异常状态（HTTP {status}），请稍后重试。"
            ));
        }
        let mut body = Vec::with_capacity(2048);
        let mut chunk = [0u8; 8192];
        loop {
            check_canceled(cancel)?;
            let mut read: u32 = 0;
            let ok = unsafe {
                WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    &mut read,
                )
            };
            if ok == 0 {
                return Err("读取版本清单中断，请检查网络后重试。".into());
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > MAX_MANIFEST_BYTES {
                return Err("在线更新清单体积超出安全限制，已中止。".into());
            }
        }
        verify_and_evaluate_manifest(
            &body,
            env!("CARGO_PKG_VERSION"),
            &RELEASE_PUBLIC_KEY_XY,
            custom_origin.as_deref(),
            allow_loopback,
        )
    }

    fn download_verified_file(
        url_str: &str,
        allow_loopback: bool,
        destination: &Path,
        expected_size: u64,
        expected_sha256: &[u8; 32],
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<(), String> {
        let parsed = parse_update_url(url_str, allow_loopback)?;
        let (_session, _conn, request, status) = open_request(&parsed, cancel)?;
        if status != 200 {
            return Err(format!("下载更新包失败（HTTP {status}），请稍后重试。"));
        }
        let part_path = destination.with_extension("part");
        if fs::symlink_metadata(&part_path).is_ok() {
            ensure_regular_path(&part_path, false)?;
            let _ = fs::remove_file(&part_path);
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|_| "无法在本地数据目录创建更新临时文件。".to_string())?;

        let mut hasher = Sha256::new();
        let mut received: u64 = 0;
        let mut chunk = vec![0u8; 32 * 1024];
        on_progress(0, expected_size);

        let transfer = (|| -> Result<(), String> {
            loop {
                check_canceled(cancel)?;
                let mut read: u32 = 0;
                let ok = unsafe {
                    WinHttpReadData(
                        request.0,
                        chunk.as_mut_ptr().cast(),
                        chunk.len() as u32,
                        &mut read,
                    )
                };
                if ok == 0 {
                    return Err("下载更新包时连接中断，请重试。".into());
                }
                if read == 0 {
                    break;
                }
                let slice = &chunk[..read as usize];
                received = received.saturating_add(slice.len() as u64);
                if received > expected_size || received > MAX_ASSET_BYTES {
                    return Err("下载的更新包大小与签名清单不符，已中止。".into());
                }
                hasher.update(slice);
                file.write_all(slice)
                    .map_err(|_| "写入更新包失败，请检查磁盘剩余空间。".to_string())?;
                on_progress(received, expected_size);
            }
            if received != expected_size {
                return Err("更新包下载不完整，未执行安装。".into());
            }
            let actual_hash = hasher.finalize();
            if &actual_hash != expected_sha256 {
                return Err("更新包 SHA-256 完整性校验未通过，已拒绝执行安装。".into());
            }
            file.sync_all()
                .map_err(|_| "无法同步更新包到磁盘。".to_string())?;
            Ok(())
        })();

        drop(file);
        if let Err(err) = transfer {
            let _ = fs::remove_file(&part_path);
            return Err(err);
        }
        if fs::symlink_metadata(destination).is_ok() {
            ensure_regular_path(destination, false)?;
            let _ = fs::remove_file(destination);
        }
        fs::rename(&part_path, destination).map_err(|_| {
            let _ = fs::remove_file(&part_path);
            "无法保存已完成校验的更新包。".to_string()
        })
    }

    pub fn prepare_update_install(
        root: &Path,
        manifest: &UpdateManifest,
        cancel: &AtomicBool,
        on_progress: impl FnMut(u64, u64),
    ) -> Result<PreparedUpdate, String> {
        ensure_regular_path(root, true)?;
        let updates_dir = root.join("updates");
        if !updates_dir
            .try_exists()
            .map_err(|_| "无法访问更新目录。".to_string())?
        {
            fs::create_dir(&updates_dir).map_err(|_| "无法创建更新缓存目录。".to_string())?;
        }
        ensure_regular_path(&updates_dir, true)?;

        let current_exe =
            std::env::current_exe().map_err(|_| "无法定位当前运行的程序路径。".to_string())?;
        let exe_dir = current_exe
            .parent()
            .ok_or("无法解析当前程序所在目录。")?
            .to_path_buf();
        let allow_loopback = std::env::var_os("PASSWORD_MANAGER_DATA_DIR").is_some();

        if is_installed_mode() {
            let setup_exe = updates_dir.join(&manifest.installer_name);
            download_verified_file(
                &manifest.installer_url,
                allow_loopback,
                &setup_exe,
                manifest.installer_size,
                &manifest.installer_sha256,
                cancel,
                on_progress,
            )?;
            check_canceled(cancel)?;
            Ok(PreparedUpdate::Installer {
                setup_exe,
                install_dir: exe_dir,
            })
        } else {
            let zip_path = updates_dir.join(&manifest.portable_name);
            download_verified_file(
                &manifest.portable_url,
                allow_loopback,
                &zip_path,
                manifest.portable_size,
                &manifest.portable_sha256,
                cancel,
                on_progress,
            )?;
            check_canceled(cancel)?;

            let stage_dir = updates_dir.join(format!("stage-{}", manifest.version));
            if fs::symlink_metadata(&stage_dir).is_ok() {
                clean_regular_dir_contents(&stage_dir, 0)?;
                let _ = fs::remove_dir(&stage_dir);
            }
            fs::create_dir(&stage_dir).map_err(|_| "无法创建便携版解压目录。".to_string())?;

            let system_root =
                std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let tar_exe = PathBuf::from(system_root).join("System32\\tar.exe");
            let status = Command::new(&tar_exe)
                .arg("-xf")
                .arg(&zip_path)
                .arg("-C")
                .arg(&stage_dir)
                .arg("Keyisle.exe")
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map_err(|_| "无法调用系统解压组件（tar.exe）准备便携版更新。".to_string())?;
            if !status.success() {
                let _ = clean_regular_dir_contents(&stage_dir, 0);
                let _ = fs::remove_dir(&stage_dir);
                return Err("解压便携版更新包失败，未替换现有程序。".into());
            }

            let staged_exe = stage_dir.join("Keyisle.exe");
            ensure_regular_path(&staged_exe, false)?;
            let exe_bytes =
                fs::read(&staged_exe).map_err(|_| "无法读取解压后的新版本程序。".to_string())?;
            if sha256(&exe_bytes) != manifest.exe_sha256 {
                let _ = clean_regular_dir_contents(&stage_dir, 0);
                let _ = fs::remove_dir(&stage_dir);
                return Err("解压后的 Keyisle.exe 哈希校验不符，已拒绝覆盖。".into());
            }
            check_canceled(cancel)?;

            let updater_exe = updates_dir.join(format!("updater-{}.exe", manifest.version));
            if fs::symlink_metadata(&updater_exe).is_ok() {
                ensure_regular_path(&updater_exe, false)?;
                let _ = fs::remove_file(&updater_exe);
            }
            fs::copy(&staged_exe, &updater_exe)
                .map_err(|_| "无法准备便携版覆盖升级助手。".to_string())?;
            let _ = fs::remove_file(staged_exe);
            let _ = fs::remove_dir(stage_dir);

            Ok(PreparedUpdate::Portable {
                updater_exe,
                target_exe: current_exe,
            })
        }
    }

    pub fn launch_prepared_update(prepared: &PreparedUpdate) -> Result<(), String> {
        match prepared {
            PreparedUpdate::Installer {
                setup_exe,
                install_dir,
            } => {
                let dir_str = install_dir
                    .to_str()
                    .filter(|s| {
                        !s.is_empty() && !s.contains('"') && !s.chars().any(char::is_control)
                    })
                    .ok_or("安装目录路径包含不支持的字符，请从下载页手动运行安装包。")?;
                // NSIS requires `/D=` to be the final unquoted parameter on the command line.
                Command::new(setup_exe)
                    .raw_arg(format!("/S /AUTORELAUNCH /D={dir_str}"))
                    .spawn()
                    .map_err(|_| {
                        "无法启动安装器执行覆盖升级，请检查安全软件拦截设置。".to_string()
                    })?;
                Ok(())
            }
            PreparedUpdate::Portable {
                updater_exe,
                target_exe,
            } => {
                Command::new(updater_exe)
                    .arg("--apply-portable-update")
                    .arg(target_exe)
                    .arg(std::process::id().to_string())
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .map_err(|_| "无法启动便携版覆盖升级助手。".to_string())?;
                Ok(())
            }
        }
    }

    fn move_replace(from: &Path, to: &Path) -> bool {
        let from_w: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to_w: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(
                from_w.as_ptr(),
                to_w.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            ) != 0
        }
    }

    pub fn apply_portable_update_helper() -> i32 {
        let mut args = std::env::args_os().skip(2);
        let (Some(target_os), Some(pid_os)) = (args.next(), args.next()) else {
            return 2;
        };
        let target_exe = PathBuf::from(target_os);
        if let Some(pid) = pid_os.to_str().and_then(|s| s.parse::<u32>().ok()) {
            unsafe {
                let handle = OpenProcess(SYNCHRONIZE, 0, pid);
                if !handle.is_null() {
                    let _ = WaitForSingleObject(handle, 15_000);
                    let _ = CloseHandle(handle);
                }
            }
        }
        let Ok(updater_exe) = std::env::current_exe() else {
            return 3;
        };
        let temp_new = target_exe.with_extension(format!("exe.update-{}", std::process::id()));
        let copied = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_new)
            .and_then(|mut target| {
                let result = fs::File::open(&updater_exe)
                    .and_then(|mut source| io::copy(&mut source, &mut target))
                    .and_then(|_| target.sync_all());
                drop(target);
                if result.is_err() {
                    let _ = fs::remove_file(&temp_new);
                }
                result
            });
        if copied.is_err() {
            let msg = wide(
                "Keyisle 便携版覆盖更新失败：无法写入程序所在目录（请检查目录写入权限）。原版本未受影响。",
            );
            let cap = wide("Keyisle 更新");
            unsafe {
                MessageBoxW(null_mut(), msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            }
            return 4;
        }
        let mut replaced = false;
        for _ in 0..60 {
            if move_replace(&temp_new, &target_exe) {
                replaced = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !replaced {
            let _ = fs::remove_file(&temp_new);
            let msg = wide(
                "Keyisle 便携版程序文件仍被占用，未能完成覆盖替换。请关闭占用后重试；密码库未受影响。",
            );
            let cap = wide("Keyisle 更新");
            unsafe {
                MessageBoxW(null_mut(), msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            }
            return 5;
        }
        if Command::new(&target_exe).spawn().is_err() {
            let msg = wide("Keyisle 已更新，请手动启动程序。密码库未受影响。");
            let cap = wide("Keyisle 更新");
            unsafe {
                MessageBoxW(null_mut(), msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            }
            return 6;
        }
        0
    }
}

#[cfg(windows)]
pub use win_net::{
    apply_portable_update_helper, check_for_updates, launch_prepared_update, prepare_update_install,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn base64_decode(input: &str) -> Vec<u8> {
        let clean: Vec<u8> = input
            .bytes()
            .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
            .collect();
        let val = |b: u8| -> u8 {
            match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("invalid base64 byte"),
            }
        };
        let mut out = Vec::new();
        for chunk in clean.chunks(4) {
            let b0 = val(chunk[0]);
            let b1 = val(chunk[1]);
            out.push((b0 << 2) | (b1 >> 4));
            if chunk.len() > 2 {
                let b2 = val(chunk[2]);
                out.push((b1 << 4) | (b2 >> 2));
                if chunk.len() > 3 {
                    let b3 = val(chunk[3]);
                    out.push((b2 << 6) | b3);
                }
            }
        }
        out
    }

    const GOLDEN_SIGNED_V1_1_0: &str = r#"{"schema": 1, "version": "1.1.0", "published_at": "2026-12-01T00:00:00Z", "notes": "支持自动检测更新与免手动退出覆盖升级。", "installer_name": "Keyisle-1.1.0-Setup-x64.exe", "installer_url": "https://github.com/dfx1822375/Keyisle/releases/download/v1.1.0/Keyisle-1.1.0-Setup-x64.exe", "installer_sha256": "b337156a4a0059a7bf5551abca18ab2a725b01ae2c5a1205d3a5d3eed659b351", "installer_size": 5120000, "portable_name": "Keyisle-1.1.0-Windows-x64-portable.zip", "portable_url": "https://github.com/dfx1822375/Keyisle/releases/download/v1.1.0/Keyisle-1.1.0-Windows-x64-portable.zip", "portable_sha256": "37f0976b7e893a5989ba816229f2e96239c5c12280e001d9f96cca3c4df08642", "portable_size": 4980000, "exe_sha256": "278edc342d141616e165504a54039219717e49ee30b415b9349571dc35f02e01", "signature": "f1865ffc2f1399daf761e0b32f379d18da543127a4c3a9e1f4ceac0b43e6a443d4e9b6192f11083234f3f724f8e14d89fbac9061972b613e92e3e098504161b3"}"#;

    #[test]
    fn sha256_matches_nist_fips_180_4_vectors() {
        assert_eq!(
            hex_encode(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let mut streaming = Sha256::new();
        for _ in 0..1000 {
            streaming.update(&[b'a'; 1000]);
        }
        assert_eq!(
            hex_encode(&streaming.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn public_key_constant_matches_pem_asset() {
        let pem = include_str!("../assets/release-public.pem");
        let body: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let der = base64_decode(&body);
        assert_eq!(der.len(), 27 + 64);
        assert_eq!(der[26], 0x04);
        assert_eq!(&der[27..], &RELEASE_PUBLIC_KEY_XY[..]);
    }

    #[test]
    fn verifies_signed_manifest_enforces_version_monotonicity_and_rejects_tampering() {
        let outcome = verify_and_evaluate_manifest(
            GOLDEN_SIGNED_V1_1_0.as_bytes(),
            "1.0.0",
            &RELEASE_PUBLIC_KEY_XY,
            None,
            false,
        )
        .unwrap();
        let CheckOutcome::Available(manifest) = outcome else {
            panic!("expected available update");
        };
        assert_eq!(manifest.version, "1.1.0");
        assert_eq!(manifest.notes, "支持自动检测更新与免手动退出覆盖升级。");

        // Same version -> UpToDate
        assert_eq!(
            verify_and_evaluate_manifest(
                GOLDEN_SIGNED_V1_1_0.as_bytes(),
                "1.1.0",
                &RELEASE_PUBLIC_KEY_XY,
                None,
                false,
            )
            .unwrap(),
            CheckOutcome::UpToDate
        );

        // Older manifest than installed -> downgrade rejected!
        assert!(
            verify_and_evaluate_manifest(
                GOLDEN_SIGNED_V1_1_0.as_bytes(),
                "1.2.0",
                &RELEASE_PUBLIC_KEY_XY,
                None,
                false,
            )
            .is_err()
        );

        // Tampered hash, notes, size, or URL -> signature / pinning rejected!
        for (needle, replacement) in [
            ("5120000", "5120001"),
            ("支持自动检测更新", "篡改后的更新说明"),
            (
                "b337156a4a0059a7bf5551abca18ab2a725b01ae2c5a1205d3a5d3eed659b351",
                "0337156a4a0059a7bf5551abca18ab2a725b01ae2c5a1205d3a5d3eed659b351",
            ),
            (
                "https://github.com/dfx1822375/Keyisle/releases/download/v1.1.0/Keyisle-1.1.0-Setup-x64.exe",
                "https://evil.example.com/dfx1822375/Keyisle/releases/download/v1.1.0/Keyisle-1.1.0-Setup-x64.exe",
            ),
        ] {
            let mutated = GOLDEN_SIGNED_V1_1_0.replace(needle, replacement);
            assert!(
                verify_and_evaluate_manifest(
                    mutated.as_bytes(),
                    "1.0.0",
                    &RELEASE_PUBLIC_KEY_XY,
                    None,
                    false,
                )
                .is_err(),
                "mutation should be rejected: {needle}"
            );
        }

        // Wrong public key (flipping coordinates) -> rejected!
        let mut wrong_key = RELEASE_PUBLIC_KEY_XY;
        wrong_key[0] ^= 0x01;
        assert!(
            verify_and_evaluate_manifest(
                GOLDEN_SIGNED_V1_1_0.as_bytes(),
                "1.0.0",
                &wrong_key,
                None,
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn url_parser_enforces_https_and_loopback_rules() {
        let parsed = parse_update_url(DEFAULT_MANIFEST_URL, false).unwrap();
        assert!(parsed.secure);
        assert_eq!(parsed.host, "github.com");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.origin, "https://github.com");

        assert!(parse_update_url("http://github.com/update.json", false).is_err());
        assert!(parse_update_url("http://github.com/update.json", true).is_err());
        assert!(parse_update_url("http://127.0.0.1:8080/update.json", false).is_err());
        let local = parse_update_url("http://127.0.0.1:8080/update.json", true).unwrap();
        assert!(!local.secure);
        assert_eq!(local.port, 8080);
        assert_eq!(local.origin, "http://127.0.0.1:8080");
    }
}
