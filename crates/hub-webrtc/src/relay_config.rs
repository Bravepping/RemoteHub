//! Private WSS credential loading and launch policy; no network activity.
use hub_probe_wire::{now, PathMode, Role};
use serde::Deserialize;
use std::{net::Ipv4Addr, sync::Arc};
use zeroize::Zeroize;

#[cfg(all(test, feature = "p006-session-probe"))]
#[path = "relay_config_tests.rs"]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateConfig {
    #[cfg(feature = "p006-session-probe")]
    #[serde(skip)]
    synthetic_local: bool,
    #[cfg(feature = "p006-session-probe")]
    #[serde(skip)]
    pub(crate) synthetic_source: bool,
    #[cfg(feature = "p006-session-probe")]
    #[serde(skip)]
    pub(crate) diagnostic_drop_direct: bool,
    pub server_ipv4: Ipv4Addr,
    pub signaling_port: u16,
    pub ca_pem: String,
    sender_invitation: Option<String>,
    receiver_invitation: Option<String>,
    pub invitation_expires_at: u64,
}
impl Drop for PrivateConfig {
    fn drop(&mut self) {
        if let Some(token) = &mut self.sender_invitation {
            token.zeroize();
        }
        if let Some(token) = &mut self.receiver_invitation {
            token.zeroize();
        }
    }
}
impl PrivateConfig {
    pub fn load(path: &std::path::Path) -> Result<Arc<Self>, String> {
        Self::load_inner(path, false, None)
    }
    #[cfg(feature = "p006-session-probe")]
    pub fn load_local_synthetic(path: &std::path::Path) -> Result<Arc<Self>, String> {
        Self::load_inner(path, true, None)
    }
    #[cfg(feature = "p006-session-probe")]
    pub fn load_operator_synthetic(
        path: &std::path::Path,
        drop_direct: bool,
    ) -> Result<Arc<Self>, String> {
        let mut config = Self::load(path)?;
        let c = Arc::get_mut(&mut config).ok_or("LOCAL_CONFIG_ALREADY_SHARED")?;
        c.synthetic_source = true;
        c.diagnostic_drop_direct = drop_direct;
        Ok(config)
    }
    /// The two-PC synthetic build never receives the other PC's invitation.
    #[cfg(feature = "p006-session-probe")]
    pub fn load_operator_role_synthetic(
        path: &std::path::Path,
        role: Role,
    ) -> Result<Arc<Self>, String> {
        Self::load_inner(path, false, Some(role))
    }
    pub(crate) fn invitation(&self, role: Role) -> Result<&str, String> {
        match role {
            Role::Sender => self.sender_invitation.as_deref(),
            Role::Receiver => self.receiver_invitation.as_deref(),
        }
        .ok_or_else(|| "P006_INVITATION_ROLE_MISMATCH".into())
    }
    pub(crate) fn authorize_start(&self, role: Role, mode: PathMode) -> Result<(), String> {
        self.invitation(role)?;
        #[cfg(feature = "p006-session-probe")]
        if self.synthetic_source && mode != PathMode::DirectPreferred {
            return Err("P006_SYNTHETIC_REQUIRES_DIRECT_MODE".into());
        }
        let _ = mode;
        Ok(())
    }
    pub(crate) fn is_local_synthetic(&self) -> bool {
        #[cfg(feature = "p006-session-probe")]
        {
            self.synthetic_local
        }
        #[cfg(not(feature = "p006-session-probe"))]
        {
            false
        }
    }
    fn load_inner(
        path: &std::path::Path,
        local_synthetic: bool,
        only_role: Option<Role>,
    ) -> Result<Arc<Self>, String> {
        let info = std::fs::symlink_metadata(path).map_err(|_| "P006_PRIVATE_CONFIG_REQUIRED")?;
        if !info.is_file() || info.file_type().is_symlink() || info.len() > 16384 {
            return Err("P006_CONFIG_FILE_POLICY".into());
        }
        let data =
            zeroize::Zeroizing::new(std::fs::read_to_string(path).map_err(|_| "P006_CONFIG_READ")?);
        let c: Self = serde_json::from_str(&data).map_err(|_| "P006_CONFIG_SCHEMA")?;
        #[cfg(feature = "p006-session-probe")]
        let c = {
            let mut c = c;
            c.synthetic_local = local_synthetic;
            c.synthetic_source = local_synthetic || only_role.is_some();
            c
        };
        let valid = |token: &str| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit());
        let invitations_valid = match (
            only_role,
            c.sender_invitation.as_deref(),
            c.receiver_invitation.as_deref(),
        ) {
            (None, Some(sender), Some(receiver)) => {
                valid(sender) && valid(receiver) && sender != receiver
            }
            (Some(Role::Sender), Some(token), None) | (Some(Role::Receiver), None, Some(token)) => {
                valid(token)
            }
            _ => false,
        };
        if (local_synthetic
            && (!cfg!(feature = "p006-session-probe") || c.server_ipv4 != Ipv4Addr::LOCALHOST))
            || (!local_synthetic && !crate::ice_policy::public_v4(c.server_ipv4))
            || c.signaling_port < 1024
            || !invitations_valid
            || !(60..=7200).contains(&c.invitation_expires_at.saturating_sub(now()))
        {
            return Err("P006_CONFIG_EXPIRED_OR_INVALID".into());
        }
        Ok(Arc::new(c))
    }
}
