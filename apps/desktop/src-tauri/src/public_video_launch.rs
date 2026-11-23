//! Process launch policy; no network, filesystem reads, or window state.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const PROFILE_ARG: &str = "--public-video-profile";

pub(crate) enum LaunchMode {
    Normal,
    PublicVideo(Option<PathBuf>),
}

impl LaunchMode {
    pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut args = args.into_iter();
        let mut result = Self::Normal;
        while let Some(arg) = args.next() {
            if arg == PROFILE_ARG {
                if result.is_public_video() {
                    return Self::PublicVideo(None);
                }
                let path = args
                    .next()
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute());
                result = Self::PublicVideo(path);
            } else if arg
                .as_encoded_bytes()
                .starts_with(b"--public-video-profile=")
            {
                // Unsupported shorthand still counts as an explicit request,
                // so a typo cannot leave the process in ordinary LAN mode.
                return Self::PublicVideo(None);
            }
        }
        result
    }

    pub(crate) fn is_public_video(&self) -> bool {
        matches!(self, Self::PublicVideo(_))
    }

    pub(crate) fn profile(&self) -> Option<&Path> {
        match self {
            Self::PublicVideo(path) => path.as_deref(),
            Self::Normal => None,
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
