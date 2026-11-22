use super::*;
#[test]
fn legacy_hello_is_unchanged_and_path_policy_is_explicit_bounded_and_single_use() {
    let legacy = r#"{"version":1,"role":"sender","room":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","seconds":30}"#;
    let hello: Hello = serde_json::from_str(legacy).unwrap();
    assert_eq!(hello.mode, PathMode::RelayOnly);
    assert!(!serde_json::to_string(&hello).unwrap().contains("mode"));
    let good = PathPolicy::new(vec!["192.168.1.2".parse().unwrap()]).unwrap();
    let payload = serde_json::to_string(&good).unwrap();
    let mut exchange = PathExchange::new(vec![]).unwrap();
    assert!(!exchange.ready());
    exchange.accept(&payload).unwrap();
    assert!(exchange.may_publish_private_local("192.168.1.2".parse().unwrap()));
    assert!(!exchange.may_publish_private_local("192.168.1.3".parse().unwrap()));
    assert!(exchange.accept(&payload).is_err());
    assert!(!exchange.ready());
    for bad in [
        payload.replace("direct_preferred", "relay_only"),
        payload.replace("192.168.1.2", "127.0.0.1"),
        payload.replace("192.168.1.2", "169.254.169.254"),
        payload.replace("192.168.1.2", "8.8.8.8"),
        payload.replace("\"v\":1", "\"v\":2"),
        payload.replace("[\"192.168.1.2\"]", "[\"192.168.1.2\",\"192.168.1.2\"]"),
        "x".repeat(513),
    ] {
        let mut ex = PathExchange::new(vec![]).unwrap();
        assert!(ex.accept(&bad).is_err());
        assert!(ex.accept(&payload).is_err());
    }
}
#[test]
fn real_codec_authenticates_policy_and_rejects_tamper_replay_and_oversize() {
    let key = Arc::new(PairKey::generate().unwrap());
    let ready = Ready {
        session_id: uuid::Uuid::new_v4().to_string(),
        nonce: uuid::Uuid::new_v4().to_string(),
        expires_at: 1300,
        ice_servers: vec![],
    };
    let payload = PathExchange::new(vec!["192.168.1.2".parse().unwrap()])
        .unwrap()
        .payload()
        .unwrap();
    let mut tx = Codec::new(key.clone(), &ready, Role::Sender, 1000).unwrap();
    let mut rx = Codec::new(key.clone(), &ready, Role::Receiver, 1000).unwrap();
    let encoded = tx.encode("path", payload.clone(), 1000).unwrap();
    let (kind, body) = rx.accept(&encoded, 1000).unwrap();
    assert_eq!(kind, "path");
    assert_eq!(body, payload);
    assert!(rx.accept(&encoded, 1000).is_err());
    let mut rx = Codec::new(key, &ready, Role::Receiver, 1000).unwrap();
    assert!(rx
        .accept(&encoded.replace("192.168.1.2", "192.168.1.3"), 1000)
        .is_err());
    assert!(tx.encode("path", "x".repeat(513), 1000).is_err());
}
