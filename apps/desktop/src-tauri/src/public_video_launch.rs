//! Process launch policy; no network, filesystem reads, or window state.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const PROFILE_ARG: &str = "--public-video-profile";
const CONTROL_ARG: &str = "--public-control-profile";

pub(crate) enum LaunchMode {
    Normal,
    Invitations,
    PublicVideo(Option<PathBuf>),
    PublicControl(Option<PathBuf>),
}

impl LaunchMode {
    pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut args = args.into_iter().peekable();
        if cfg!(feature = "v2-control-invitations") && args.peek().is_none() {
            return Self::Invitations;
        }
        let mut result = Self::Normal;
        while let Some(arg) = args.next() {
            let control = arg == CONTROL_ARG
                || arg
                    .as_encoded_bytes()
                    .starts_with(b"--public-control-profile=");
            let video = arg == PROFILE_ARG
                || arg
                    .as_encoded_bytes()
                    .starts_with(b"--public-video-profile=");
            if !control && !video {
                continue;
            }
            let mode = if control {
                Self::PublicControl
            } else {
                Self::PublicVideo
            };
            if result.is_public_video() || arg.as_encoded_bytes().contains(&b'=') {
                return mode(None);
            }
            let path = args
                .next()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute());
            result = mode(path);
        }
        result
    }

    pub(crate) fn is_public_video(&self) -> bool {
        matches!(
            self,
            Self::PublicVideo(_) | Self::PublicControl(_) | Self::Invitations
        )
    }

    pub(crate) fn is_control(&self) -> bool {
        matches!(self, Self::PublicControl(_) | Self::Invitations)
    }

    pub(crate) fn profile(&self) -> Option<&Path> {
        match self {
            Self::PublicVideo(path) | Self::PublicControl(path) => path.as_deref(),
            Self::Normal | Self::Invitations => None,
        }
    }

    pub(crate) fn require_lan(&self) -> Result<(), &'static str> {
        if self.is_public_video() {
            Err("LAN_DISABLED_IN_PUBLIC_VIDEO_MODE")
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> LaunchMode {
        LaunchMode::parse(args.iter().copied().map(OsString::from))
    }

    #[test]
    fn only_the_invitation_build_opens_the_picker_by_default() {
        let launch = parse(&[]);
        assert_eq!(
            launch.is_control(),
            cfg!(feature = "v2-control-invitations")
        );
        assert_eq!(
            launch.require_lan().is_err(),
            cfg!(feature = "v2-control-invitations")
        );
        assert!(launch.profile().is_none());
    }

    #[test]
    fn normal_launch_keeps_lan_available() {
        let mode = parse(&["--other-option", "profile.json"]);
        assert!(!mode.is_public_video());
        assert_eq!(mode.profile(), None);
        assert_eq!(mode.require_lan(), Ok(()));
    }

    #[test]
    fn explicit_video_launch_blocks_lan_even_before_reading_a_profile() {
        let path = std::env::temp_dir().join("remotehub-fixture-do-not-open.json");
        let mode = LaunchMode::parse([PROFILE_ARG.into(), path.clone().into_os_string()]);
        assert!(mode.is_public_video());
        assert_eq!(mode.profile(), Some(path.as_path()));
        assert_eq!(mode.require_lan(), Err("LAN_DISABLED_IN_PUBLIC_VIDEO_MODE"));

        let duplicate = LaunchMode::parse([
            PROFILE_ARG.into(),
            path.clone().into_os_string(),
            PROFILE_ARG.into(),
            path.into_os_string(),
        ]);
        assert!(duplicate.is_public_video());
        assert_eq!(duplicate.profile(), None);
        assert_eq!(
            duplicate.require_lan(),
            Err("LAN_DISABLED_IN_PUBLIC_VIDEO_MODE")
        );
    }

    #[test]
    fn explicit_control_never_falls_back_to_video_or_lan() {
        let path = std::env::temp_dir().join("remotehub-fixture-do-not-open.json");
        let mode = LaunchMode::parse([CONTROL_ARG.into(), path.clone().into_os_string()]);
        assert!(mode.is_control());
        assert_eq!(mode.profile(), Some(path.as_path()));
        assert!(mode.require_lan().is_err());

        let mixed = LaunchMode::parse([
            PROFILE_ARG.into(),
            path.clone().into_os_string(),
            CONTROL_ARG.into(),
            path.into_os_string(),
        ]);
        assert!(mixed.profile().is_none());
        assert!(mixed.require_lan().is_err());
        let bad = parse(&["--public-control-profile=relative.json"]);
        assert!(bad.is_control());
        assert!(bad.profile().is_none());
    }

    #[test]
    fn malformed_video_request_never_falls_back_to_lan() {
        for args in [
            &[PROFILE_ARG][..],
            &[PROFILE_ARG, "relative.json"],
            &["--public-video-profile=profile.json"],
        ] {
            let mode = parse(args);
            assert!(mode.is_public_video());
            assert_eq!(mode.profile(), None);
            assert_eq!(mode.require_lan(), Err("LAN_DISABLED_IN_PUBLIC_VIDEO_MODE"));
        }
    }
}
