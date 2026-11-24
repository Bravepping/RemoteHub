mod archive;
pub(super) use archive::load;

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
        lpstrTitle: w!("打开本机的角色 ZIP 或单轮 JSON"),
        lpstrFilter: w!("RemoteHub 邀请 (*.zip;*.json)\0*.zip;*.json\0"),
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
    round: Option<u8>,
    role: &'static str,
    expires_at: u64,
}

#[derive(Default)]
pub(crate) struct Invitations {
    selected: HashMap<String, PublicVideoProfile>,
    consumed: HashMap<Uuid, u64>,
}

impl Invitations {
    pub(crate) fn select(
        &mut self,
        rounds: archive::Rounds,
        now: u64,
    ) -> Result<Vec<Preview>, &'static str> {
        self.consumed.retain(|_, expires| *expires > now);
        let mut candidates = HashMap::new();
        let mut previews = Vec::new();
        for (round, profile) in rounds {
            let approval = profile.approval();
            if self.consumed.contains_key(&approval.room) {
                continue;
            }
            profile.verify_invitation_time(now)?;
            let id = Uuid::new_v4().simple().to_string();
            candidates.insert(id.clone(), profile);
            previews.push(Preview {
                id,
                round,
                role: match approval.role {
                    Role::Host => "host",
                    Role::Viewer => "viewer",
                },
                expires_at: approval.scope.expires_at,
            });
        }
        if previews.is_empty() {
            return Err("PUBLIC_VIDEO_PROFILE_CONSUMED");
        }
        self.selected = candidates;
        Ok(previews)
    }

    pub(crate) fn take(
        &mut self,
        id: &str,
        role: Role,
        now: u64,
    ) -> Result<PublicVideoProfile, &'static str> {
        let profile = self
            .selected
            .get(id)
            .filter(|p| p.approval().role == role)
            .ok_or("PUBLIC_VIDEO_PROFILE_UNAVAILABLE")?;
        profile.verify_invitation_time(now)?;
        self.selected
            .remove(id)
            .ok_or("PUBLIC_VIDEO_PROFILE_UNAVAILABLE")
    }

    pub(crate) fn consume(&mut self, room: Uuid, expires_at: u64) -> Result<(), &'static str> {
        if self.consumed.insert(room, expires_at).is_some() {
            return Err("PUBLIC_VIDEO_PROFILE_CONSUMED");
        }
        Ok(())
    }
}
