use super::*;

fn identity(seed: u8) -> Identity {
    Identity {
        key: SigningKey::from_bytes(&[seed; 32]),
    }
}

#[test]
fn public_id_is_stable_and_full_fingerprint_is_not_short_id() {
    let first = identity(41).public();
    let again = identity(41).public();
    assert!(first == again);
    assert_ne!(first.device_id, identity(42).public().device_id);
    assert_eq!(first.device_id.len(), 17);
    assert_eq!(first.fingerprint.len(), 64);
    assert_eq!(
        first.device_id.replace('-', "")[2..],
        first.fingerprint[..12].to_uppercase()
    );
}

#[test]
fn strict_local_proof_rejects_tampering_wrong_nonce_wrong_and_weak_key() {
    let id = identity(12);
    let nonce = [37; 32];
    let mut sig = id.local_proof(&nonce);
    assert!(verify_local_proof(&id.public().public_key, &nonce, &sig));
    assert!(!verify_local_proof(
        &id.public().public_key,
        &[38; 32],
        &sig
    ));
    assert!(!verify_local_proof(
        &identity(13).public().public_key,
        &nonce,
        &sig
    ));
    sig[63] |= 0xf0;
    assert!(!verify_local_proof(&id.public().public_key, &nonce, &sig));
    let mut identity_point = [0u8; 32];
    identity_point[0] = 1;
    assert!(!verify_local_proof(&identity_point, &nonce, &[0; 64]));
}

#[test]
fn payload_rejects_wrong_length_version_or_public_key() {
    let mut value = Zeroizing::new(vec![0u8; 72]);
    value[..8].copy_from_slice(PAYLOAD_MAGIC);
    value[8..40].copy_from_slice(&[9; 32]);
    value[40..].copy_from_slice(identity(9).key.verifying_key().as_bytes());
    assert!(Identity::from_payload(&value).is_ok());
    assert!(Identity::from_payload(&value[..71]).is_err());
    value[0] ^= 1;
    assert!(Identity::from_payload(&value).is_err());
    value[0] ^= 1;
    value[71] ^= 1;
    assert!(Identity::from_payload(&value).is_err());
}

#[test]
fn rfc8032_public_test_vector_matches_ed25519() {
    // Published RFC8032 test material, never a production secret.
    fn hex<const N: usize>(s: &str) -> [u8; N] {
        std::array::from_fn(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
    }
    let seed = Zeroizing::new(hex::<32>(
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    ));
    let key = SigningKey::from_bytes(&seed);
    assert_eq!(
        key.verifying_key().to_bytes(),
        hex::<32>("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    );
    let sig = key.sign(&[]);
    assert_eq!(sig.to_bytes(), hex::<64>("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"));
    assert!(key.verifying_key().verify_strict(&[], &sig).is_ok());
}
