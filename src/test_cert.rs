// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later
//! A throw-away self-signed certificate for the tests of the HTTPS send (tests only, never part of the binary).
//!
//! It is built by hand (a minimal X.509 v3 in DER, signed with ECDSA P-256 through `ring`, which the TLS provider already uses) so that no
//! certificate-generating crate, and no date-and-time crate behind it, is needed even for the tests.

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

/// What a TLS test server and a client need: the certificate as PEM and as DER, and the private key as PKCS#8 DER.
pub struct TestCert {
    pub pem: String,
    pub der: Vec<u8>,
    pub key_pkcs8: Vec<u8>,
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 128 {
        out.push(n as u8);
    } else if n < 256 {
        out.extend_from_slice(&[0x81, n as u8]);
    } else {
        out.extend_from_slice(&[0x82, (n >> 8) as u8, n as u8]);
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

/// A self-signed certificate valid for the DNS name `name`, from the year 2025 to 2999.
pub fn self_signed(name: &str) -> TestCert {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap();

    // ecdsa-with-SHA256
    let sig_alg = seq(&[&tlv(
        0x06,
        &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02],
    )]);
    let mut serial = [0u8; 8];
    rng.fill(&mut serial).unwrap();
    serial[0] = 0x01;
    let issuer = seq(&[&tlv(
        0x31,
        &seq(&[
            &tlv(0x06, &[0x55, 0x04, 0x03]),
            &tlv(0x0C, b"twin test certificate"),
        ]),
    )]);
    let validity = seq(&[&tlv(0x17, b"250101000000Z"), &tlv(0x18, b"29991231235959Z")]);
    let mut point = vec![0u8];
    point.extend_from_slice(pair.public_key().as_ref());
    let spki = seq(&[
        &seq(&[
            &tlv(0x06, &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01]),
            &tlv(0x06, &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07]),
        ]),
        &tlv(0x03, &point),
    ]);
    // subjectAltName: one dNSName
    let san = seq(&[
        &tlv(0x06, &[0x55, 0x1D, 0x11]),
        &tlv(0x04, &seq(&[&tlv(0x82, name.as_bytes())])),
    ]);
    let extensions = tlv(0xA3, &seq(&[&san]));
    let tbs = seq(&[
        &tlv(0xA0, &tlv(0x02, &[0x02])),
        &tlv(0x02, &serial),
        &sig_alg,
        &issuer,
        &validity,
        &issuer,
        &spki,
        &extensions,
    ]);
    let signature = pair.sign(&rng, &tbs).unwrap();
    let mut bits = vec![0u8];
    bits.extend_from_slice(signature.as_ref());
    let der = seq(&[&tbs, &sig_alg, &tlv(0x03, &bits)]);

    let body = crate::ws::base64(&der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in body.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    TestCert {
        pem,
        der,
        key_pkcs8: pkcs8.as_ref().to_vec(),
    }
}
