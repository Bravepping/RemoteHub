use crate::public_video_profile::PublicVideoProfile;
use hub_webrtc::public_video_wss::Role;
use serde::Serialize;
use std::{collections::HashMap, ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
use uuid::Uuid;
use windows::{
    core::{w, PWSTR},
    Win32::{
        Foundation::HWND,
        UI::Controls::Dialogs::{
            CommDlgExtendedError, GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_FILEMUSTEXIST,
            OFN_NOCHANGEDIR, OFN_NODEREFERENCELINKS, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        },
    },
};

pub(crate) fn choose(owner: usize) -> Result<Option<PathBuf>, &'static str> {
    let mut path = vec![0u16; 32768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpstrTitle: w!("选择本机的 RemoteHub 单轮邀请"),
        lpstrFilter: w!("RemoteHub 邀请 (*.json)\0*.json\0"),
        lpstrFile: PWSTR(path.as_mut_ptr()),
        nMaxFile: path.len() as u32,
        Flags: OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_NODEREFERENCELINKS
            | OFN_DONTADDTORECENT,
        ..Default::default()
    };
    // The dialog is modal. Its owner and backing buffers outlive this call.
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return if unsafe { CommDlgExtendedError() }.0 == 0 {
            Ok(None)
        } else {
            Err("PUBLIC_VIDEO_PROFILE_FILE")
        };
    }
    let end = path
        .iter()
        .position(|&ch| ch == 0)
        .ok_or("PUBLIC_VIDEO_PROFILE_FILE")?;
    Ok(Some(PathBuf::from(OsString::from_wide(&path[..end]))))
}

#[derive(Serialize)]
pub(crate) struct Preview {
    pub(crate) id: String,
    role: &'static str,
    expires_at: u64,
}

#[derive(Default)]
pub(crate) struct Invitations {
    selected: Option<(String, PublicVideoProfile)>,
    consumed: HashMap<Uuid, u64>,
}

impl Invitations {
    pub(crate) fn select(
        &mut self,
        profile: PublicVideoProfile,
        now: u64,
    ) -> Result<Preview, &'static str> {
        profile.verify_invitation_time(now)?;
        let approval = profile.approval();
        self.consumed.retain(|_, expires| *expires > now);
        if self.consumed.contains_key(&approval.room) {
            return Err("PUBLIC_VIDEO_PROFILE_CONSUMED");
        }
        let id = Uuid::new_v4().simple().to_string();
        self.selected = Some((id.clone(), profile));
        Ok(Preview {
            id,
            role: match approval.role {
                Role::Host => "host",
                Role::Viewer => "viewer",
            },
            expires_at: approval.scope.expires_at,
        })
    }

    pub(crate) fn take(
        &mut self,
        id: &str,
        role: Role,
        now: u64,
    ) -> Result<PublicVideoProfile, &'static str> {
        let selected = self
            .selected
            .as_ref()
            .filter(|(current, profile)| current == id && profile.approval().role == role)
            .ok_or("PUBLIC_VIDEO_PROFILE_UNAVAILABLE")?;
        selected.1.verify_invitation_time(now)?;
        self.selected
            .take()
            .map(|(_, profile)| profile)
            .ok_or("PUBLIC_VIDEO_PROFILE_UNAVAILABLE")
    }

    pub(crate) fn consume(&mut self, room: Uuid, expires_at: u64) -> Result<(), &'static str> {
        if self.consumed.insert(room, expires_at).is_some() {
            return Err("PUBLIC_VIDEO_PROFILE_CONSUMED");
        }
        Ok(())
    }
}
