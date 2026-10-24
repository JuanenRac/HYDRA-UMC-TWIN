// =============================================================================
// HYDRA-UMC-TWIN - src/certificate.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! Interlock certificates: the twin signs its own risk report.
//!
//! HYDRA-UMC-HIL-BRIDGE refuses to send a command to real hardware on the word of whoever asks. It needs the twin's report of the
//! risk of THAT command, and it needs to know the twin made it. A certificate is the report plus a signature with the twin's Ed25519
//! key; the bridge holds only the public half, so nobody who can reach the bridge can mint one.
//!
//! What is signed is a short text, the same bytes on both sides (the bridge builds it again from the fields it received):
//!
//! ```text
//! hydra-umc-interlock-certificate-v1
//! <key_id>
//! <joint>
//! <position>             f64 in Rust's shortest round-trip form (`{:?}`)
//! <collision_imminent>   true | false
//! <distance_m>           f64 in the same form (inf / NaN do not occur: they are refused before signing)
//! <issued_at_ms>         integer, unix milliseconds
//! <nonce>                32 hex characters
//! ```
//!
//! The certificate is bound to the command it is for (joint and exact position), to the moment it was issued and to a nonce the
//! bridge remembers, so it cannot be reused for another command, kept for later, or replayed.

#[cfg(test)]
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};

pub const DOMAIN: &str = "hydra-umc-interlock-certificate-v1";
pub const MAX_JOINT_CHARS: usize = 64;

/// What the twin has to say about one command, before it signs it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RiskInput {
    pub joint: String,
    pub position: f64,
    pub collision_imminent: bool,
    pub distance_m: f64,
}

/// The signed report. `signature` is 64 bytes in hexadecimal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Certificate {
    pub joint: String,
    pub position: f64,
    pub collision_imminent: bool,
    pub distance_m: f64,
    pub issued_at_ms: u64,
    pub nonce: String,
    pub key_id: String,
    pub signature: String,
}

/// The bytes that are signed for these fields.
pub fn canonical(
    key_id: &str,
    joint: &str,
    position: f64,
    collision_imminent: bool,
    distance_m: f64,
    issued_at_ms: u64,
    nonce: &str,
) -> String {
    format!("{DOMAIN}\n{key_id}\n{joint}\n{position:?}\n{collision_imminent}\n{distance_m:?}\n{issued_at_ms}\n{nonce}")
}

impl Certificate {
    #[cfg(test)]
    pub fn signed_text(&self) -> String {
        canonical(
            &self.key_id,
            &self.joint,
            self.position,
            self.collision_imminent,
            self.distance_m,
            self.issued_at_ms,
            &self.nonce,
        )
    }

    /// True when the signature is valid for these fields under `key` (the freshness, the binding to a command and the replay check are
    /// the verifier's business, see the bridge). The twin only signs; the bridge verifies, and the tests check both against each other.
    #[cfg(test)]
    pub fn verifies_under(&self, key: &VerifyingKey) -> bool {
        let Ok(bytes) = decode_hex(&self.signature) else {
            return false;
        };
        let Ok(raw): Result<[u8; 64], _> = bytes.try_into() else {
            return false;
        };
        key.verify(self.signed_text().as_bytes(), &Signature::from_bytes(&raw))
            .is_ok()
    }
}

#[derive(Debug, PartialEq)]
pub enum CertifyError {
    /// The input cannot be certified: the reason is for the caller.
    Invalid(String),
}

impl std::fmt::Display for CertifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertifyError::Invalid(reason) => write!(f, "{reason}"),
        }
    }
}

/// Holds the signing key. The seed is never printed or logged.
pub struct Signer {
    key: SigningKey,
    key_id: String,
}

impl Signer {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let key = SigningKey::from_bytes(&seed);
        let public = key.verifying_key().to_bytes();
        Signer {
            key_id: encode_hex(&public[..8]),
            key,
        }
    }

    /// The seed as 64 hexadecimal characters (one line, surrounding white space allowed).
    pub fn from_seed_hex(text: &str) -> Result<Self, String> {
        let bytes = decode_hex(text.trim())
            .map_err(|_| "the signing key must be 64 hexadecimal characters".to_string())?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "the signing key must be 64 hexadecimal characters".to_string())?;
        Ok(Signer::from_seed(seed))
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key_hex(&self) -> String {
        encode_hex(&self.key.verifying_key().to_bytes())
    }

    /// Sign a report. `nonce` must be 32 hexadecimal characters, never used before.
    pub fn certify(
        &self,
        input: &RiskInput,
        issued_at_ms: u64,
        nonce: &str,
    ) -> Result<Certificate, CertifyError> {
        if input.joint.is_empty()
            || input.joint.chars().count() > MAX_JOINT_CHARS
            || input.joint.chars().any(|c| c.is_control())
        {
            return Err(CertifyError::Invalid(format!(
                "joint must be 1 to {MAX_JOINT_CHARS} characters with no control character"
            )));
        }
        if !input.position.is_finite() {
            return Err(CertifyError::Invalid(
                "position must be a finite number".into(),
            ));
        }
        // a distance of infinity ("nothing in range") is a real answer of a twin, but it has no portable text form: it is signed as the largest finite value
        let distance_m = if input.distance_m == f64::INFINITY {
            f64::MAX
        } else {
            input.distance_m
        };
        if distance_m.is_nan() {
            return Err(CertifyError::Invalid("distance_m must be a number".into()));
        }
        if nonce.len() != 32 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CertifyError::Invalid(
                "the nonce must be 32 hexadecimal characters".into(),
            ));
        }
        let text = canonical(
            &self.key_id,
            &input.joint,
            input.position,
            input.collision_imminent,
            distance_m,
            issued_at_ms,
            nonce,
        );
        let signature = self.key.sign(text.as_bytes());
        Ok(Certificate {
            joint: input.joint.clone(),
            position: input.position,
            collision_imminent: input.collision_imminent,
            distance_m,
            issued_at_ms,
            nonce: nonce.to_string(),
            key_id: self.key_id.clone(),
            signature: encode_hex(&signature.to_bytes()),
        })
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn decode_hex(text: &str) -> Result<Vec<u8>, ()> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(());
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| ()))
        .collect()
}

/// 16 random bytes from the operating system, as 32 hexadecimal characters.
pub fn random_nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("no source of randomness: {e}"))?;
    Ok(encode_hex(&bytes))
}

/// A new signing seed from the operating system, as 64 hexadecimal characters.
pub fn random_seed_hex() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("no source of randomness: {e}"))?;
    Ok(encode_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> Signer {
        Signer::from_seed([7u8; 32])
    }

    fn input() -> RiskInput {
        RiskInput {
            joint: "shoulder".into(),
            position: 0.5,
            collision_imminent: false,
            distance_m: 0.4,
        }
    }

    const NONCE: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn a_certificate_verifies_under_the_public_key_of_its_signer() {
        let s = signer();
        let cert = s.certify(&input(), 1_000, NONCE).unwrap();
        let public =
            VerifyingKey::from_bytes(&decode_hex(&s.public_key_hex()).unwrap().try_into().unwrap())
                .unwrap();
        assert!(cert.verifies_under(&public));
        assert_eq!(cert.key_id, s.key_id());
    }

    #[test]
    fn changing_any_field_breaks_the_signature() {
        let s = signer();
        let cert = s.certify(&input(), 1_000, NONCE).unwrap();
        let public =
            VerifyingKey::from_bytes(&decode_hex(&s.public_key_hex()).unwrap().try_into().unwrap())
                .unwrap();
        let mut variants = Vec::new();
        let mut c = cert.clone();
        c.joint = "elbow".into();
        variants.push(c);
        let mut c = cert.clone();
        c.position = 0.6;
        variants.push(c);
        let mut c = cert.clone();
        c.collision_imminent = true;
        variants.push(c);
        let mut c = cert.clone();
        c.distance_m = 9.0;
        variants.push(c);
        let mut c = cert.clone();
        c.issued_at_ms = 2_000;
        variants.push(c);
        let mut c = cert.clone();
        c.nonce = "ffffffffffffffffffffffffffffffff".into();
        variants.push(c);
        let mut c = cert.clone();
        c.key_id = "0000000000000000".into();
        variants.push(c);
        for variant in variants {
            assert!(!variant.verifies_under(&public), "{variant:?}");
        }
    }

    #[test]
    fn another_key_does_not_verify() {
        let cert = signer().certify(&input(), 1_000, NONCE).unwrap();
        let other = Signer::from_seed([9u8; 32]);
        let public = VerifyingKey::from_bytes(
            &decode_hex(&other.public_key_hex())
                .unwrap()
                .try_into()
                .unwrap(),
        )
        .unwrap();
        assert!(!cert.verifies_under(&public));
    }

    #[test]
    fn a_signature_that_is_not_64_bytes_of_hex_is_not_valid() {
        let s = signer();
        let public =
            VerifyingKey::from_bytes(&decode_hex(&s.public_key_hex()).unwrap().try_into().unwrap())
                .unwrap();
        for bad in ["", "zz", "abc", &"00".repeat(63), &"00".repeat(65)] {
            let mut cert = s.certify(&input(), 1_000, NONCE).unwrap();
            cert.signature = bad.to_string();
            assert!(!cert.verifies_under(&public), "{bad}");
        }
    }

    #[test]
    fn what_cannot_be_signed_is_refused_with_the_reason() {
        let s = signer();
        let mut bad = input();
        bad.joint = String::new();
        assert!(s.certify(&bad, 1, NONCE).is_err());
        bad.joint = "a\nb".into();
        assert!(s.certify(&bad, 1, NONCE).is_err());
        bad.joint = "j".repeat(65);
        assert!(s.certify(&bad, 1, NONCE).is_err());
        for position in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut p = input();
            p.position = position;
            assert!(s.certify(&p, 1, NONCE).is_err());
        }
        let mut d = input();
        d.distance_m = f64::NAN;
        assert!(s.certify(&d, 1, NONCE).is_err());
        for nonce in [
            "",
            "short",
            "zz112233445566778899aabbccddeeff",
            &"0".repeat(33),
        ] {
            assert!(s.certify(&input(), 1, nonce).is_err(), "{nonce}");
        }
    }

    #[test]
    fn an_unbounded_distance_is_signed_as_the_largest_finite_value() {
        let mut open = input();
        open.distance_m = f64::INFINITY;
        let cert = signer().certify(&open, 1, NONCE).unwrap();
        assert_eq!(cert.distance_m, f64::MAX);
        assert!(serde_json::to_string(&cert).is_ok());
    }

    #[test]
    fn the_signed_text_is_exactly_what_the_format_says() {
        let s = signer();
        let cert = s.certify(&input(), 1_000, NONCE).unwrap();
        assert_eq!(
            cert.signed_text(),
            format!(
                "hydra-umc-interlock-certificate-v1\n{}\nshoulder\n0.5\nfalse\n0.4\n1000\n{NONCE}",
                s.key_id()
            )
        );
    }

    #[test]
    fn the_seed_is_read_from_64_hex_characters_only() {
        let hex = "07".repeat(32);
        assert_eq!(
            Signer::from_seed_hex(&format!("  {hex}\n"))
                .unwrap()
                .public_key_hex(),
            signer().public_key_hex()
        );
        for bad in [
            "",
            "07",
            &"07".repeat(31),
            &"07".repeat(33),
            &"zz".repeat(32),
        ] {
            assert!(Signer::from_seed_hex(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_nonce_and_a_seed_come_from_the_system() {
        let a = random_nonce().unwrap();
        let b = random_nonce().unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(random_seed_hex().unwrap().len(), 64);
    }

    #[test]
    fn a_published_vector_stays_valid() {
        // the same fixture the bridge verifies: if the format ever changes, both repositories fail on it
        let s = Signer::from_seed([7u8; 32]);
        let cert = s.certify(&input(), 1_700_000_000_000, NONCE).unwrap();
        assert_eq!(cert.key_id, s.key_id());
        // checked against an independent RFC 8032 implementation; the bridge verifies the same vector
        assert_eq!(
            s.public_key_hex(),
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
        );
        assert_eq!(cert.key_id, "ea4a6c63e29c520a");
        assert_eq!(cert.signature, "325a7b5fda1f54fc002b73616ea3e786eb9d7418d572460c11e21d7edde313f76d4ddc0d028c5a510885e7c3b3a923e055283d188ec6774654a41962e664850a");
        let json = serde_json::to_value(&cert).unwrap();
        assert_eq!(json["signature"].as_str().unwrap().len(), 128);
        assert_eq!(json["joint"], "shoulder");
    }
}
