use super::*;
#[test]
fn synthetic_loopback_entry_is_explicit_and_cannot_be_enabled_by_private_json() {
    let path = std::env::temp_dir().join(format!(
        "remotehub-wss-policy-{}.json",
        uuid::Uuid::new_v4()
    ));
    let value = serde_json::json!({"server_ipv4":"127.0.0.1","signaling_port":8443,"ca_pem":"test-only","sender_invitation":"a".repeat(64),"receiver_invitation":"b".repeat(64),"invitation_expires_at":now()+180});
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(PrivateConfig::load(&path).is_err());
    let local = PrivateConfig::load_local_synthetic(&path).unwrap();
    assert!(local.is_local_synthetic() && local.synthetic_source);
    assert!(RelayClient::start(
        local,
        Role::Sender,
        "192.168.1.2".parse().unwrap(),
        10,
        None
    )
    .is_err());
    let mut public = value.clone();
    public["server_ipv4"] = serde_json::json!("8.8.8.8");
    std::fs::write(&path, serde_json::to_vec(&public).unwrap()).unwrap();
    assert!(PrivateConfig::load_local_synthetic(&path).is_err());
    let normal = PrivateConfig::load(&path).unwrap();
    assert!(
        !normal.is_local_synthetic() && !normal.synthetic_source && !normal.diagnostic_drop_direct
    );
    let operator = PrivateConfig::load_operator_synthetic(&path, true).unwrap();
    assert!(
        !operator.is_local_synthetic()
            && operator.synthetic_source
            && operator.diagnostic_drop_direct
    );
    let diagnostic = PrivateConfig::load_operator_synthetic(&path, false).unwrap();
    assert!(
        !diagnostic.is_local_synthetic()
            && diagnostic.synthetic_source
            && !diagnostic.diagnostic_drop_direct
    );
    public["synthetic_source"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_vec(&public).unwrap()).unwrap();
    assert!(PrivateConfig::load(&path).is_err());
    std::fs::remove_file(path).unwrap();
}
