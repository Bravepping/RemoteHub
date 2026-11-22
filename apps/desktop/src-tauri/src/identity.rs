use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum IdentityView {
    Ready {
        device_id: String,
        fingerprint: String,
    },
    Unavailable {
        error_code: &'static str,
    },
}
pub(crate) struct DesktopIdentity {
    view: IdentityView,
    #[cfg(all(feature = "v2-peer-pairing", not(feature = "p006-relay")))]
    public: Option<hub_identity::PublicIdentity>,
}

impl DesktopIdentity {
    pub(crate) fn initialize() -> Self {
        Self::from_result(hub_identity::open_default())
    }
    fn from_result(result: Result<hub_identity::Identity, hub_identity::Error>) -> Self {
        match result {
            Ok(identity) => {
                let public = identity.public();
                // Only the public half is retained; the signing key is dropped.
                Self {
                    view: IdentityView::Ready {
                        device_id: public.device_id.clone(),
                        fingerprint: public.fingerprint.clone(),
                    },
                    #[cfg(all(feature = "v2-peer-pairing", not(feature = "p006-relay")))]
                    public: Some(public),
                }
            }
            Err(error) => Self {
                view: IdentityView::Unavailable {
                    error_code: error.code(),
                },
                #[cfg(all(feature = "v2-peer-pairing", not(feature = "p006-relay")))]
                public: None,
            },
        }
    }
    pub(crate) fn view(&self) -> IdentityView {
        self.view.clone()
    }
    #[cfg(all(feature = "v2-peer-pairing", not(feature = "p006-relay")))]
    pub(crate) fn public(&self) -> Option<&hub_identity::PublicIdentity> {
        self.public.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_wire_shape_has_no_key_path_or_native_error() {
        // Public synthetic fields only; private identities never implement Serialize.
        let view = IdentityView::Ready {
            device_id: "RH-0123-4567-89AB".into(),
            fingerprint: "0123456789ab".repeat(5) + "0123",
        };
        let value = serde_json::to_value(view).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert_eq!(value["status"], "ready");
        assert!(value.get("private_key").is_none());
        assert!(value.get("public_key").is_none());
        assert!(value.get("path").is_none());
    }
    #[test]
    fn failure_is_fixed_code_and_does_not_prevent_runtime_information() {
        let state = DesktopIdentity::from_result(Err(hub_identity::Error::Unprotect));
        let value = serde_json::to_value(state.view()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value["error_code"], "IDENTITY_UNPROTECT_FAILED");
    }
}
