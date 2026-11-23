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
    assert!(local
        .authorize_start(Role::Sender, PathMode::RelayOnly)
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
    assert!(operator
        .authorize_start(Role::Sender, PathMode::RelayOnly)
        .is_err());
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

#[test]
fn synthetic_dual_pc_configs_never_need_the_other_roles_invitation() {
    let path = std::env::temp_dir().join(format!(
        "remotehub-role-invitation-{}.json",
        uuid::Uuid::new_v4()
    ));
    let mut value = serde_json::json!({
        "server_ipv4": "8.8.8.8", "signaling_port": 8443, "ca_pem": "fixture-only",
        "invitation_expires_at": now() + 180,
    });
    for (role, field, other, token) in [
        (
            Role::Sender,
            "sender_invitation",
            "receiver_invitation",
            "a".repeat(64),
        ),
        (
            Role::Receiver,
            "receiver_invitation",
            "sender_invitation",
            "b".repeat(64),
        ),
    ] {
        value[field] = serde_json::json!(&token);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let cfg = PrivateConfig::load_operator_role_synthetic(&path, role).unwrap();
        assert!(cfg.synthetic_source && !cfg.synthetic_local && !cfg.diagnostic_drop_direct);
        assert_eq!(cfg.ca_pem, "fixture-only");
        assert_eq!(cfg.invitation(role).ok(), Some(token.as_str()));
        assert!(cfg.invitation(role.opposite()).is_err());
        assert!(PrivateConfig::load_operator_role_synthetic(&path, role.opposite()).is_err());
        assert!(PrivateConfig::load(&path).is_err());
        assert!(cfg.authorize_start(role, PathMode::DirectPreferred).is_ok());
        assert_eq!(
            cfg.authorize_start(role.opposite(), PathMode::DirectPreferred)
                .err()
                .as_deref(),
            Some("P006_INVITATION_ROLE_MISMATCH")
        );
        assert!(cfg.authorize_start(role, PathMode::RelayOnly).is_ok());
        let fault = PrivateConfig::load_operator_role_synthetic_with_fault(
            &path,
            role,
            PathMode::DirectPreferred,
            true,
        );
        if role == Role::Sender {
            let fault = fault.unwrap();
            assert!(fault.diagnostic_drop_direct);
            assert!(fault
                .authorize_start(role, PathMode::DirectPreferred)
                .is_ok());
            assert!(fault.authorize_start(role, PathMode::RelayOnly).is_err());
        } else {
            assert!(fault.is_err());
        }
        assert!(PrivateConfig::load_operator_role_synthetic_with_fault(
            &path,
            role,
            PathMode::RelayOnly,
            true,
        )
        .is_err());
        assert!(cfg
            .authorize_start(role.opposite(), PathMode::RelayOnly)
            .is_err());

        value[other] = serde_json::json!("c".repeat(64));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(PrivateConfig::load_operator_role_synthetic(&path, role).is_err());
        assert!(PrivateConfig::load(&path).is_ok());
        value.as_object_mut().unwrap().remove(other);
        value[field] = serde_json::json!("not-a-token");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(PrivateConfig::load_operator_role_synthetic(&path, role).is_err());
        value.as_object_mut().unwrap().remove(field);
    }
    std::fs::remove_file(path).unwrap();
}
