use super::SetupResult;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

const EXISTS: &str = "V2_SETUP_OUTPUT_EXISTS";
const UNSAFE: &str = "V2_SETUP_OUTPUT_UNSAFE";
const WRITE_FAILED: &str = "V2_SETUP_WRITE_FAILED";

fn finish_file(mut file: File, bytes: &[u8]) -> SetupResult<()> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| WRITE_FAILED)
}

fn io_error(error: std::io::Error) -> &'static str {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        EXISTS
    } else {
        WRITE_FAILED
    }
}

fn check_new(root: &Path) -> SetupResult<()> {
    match std::fs::symlink_metadata(root) {
        Ok(_) => Err(EXISTS),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(UNSAFE),
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    pub(super) fn local_path(_path: &Path) -> bool {
        true
    }

    pub(super) struct PrivateOutput(PathBuf);
    impl PrivateOutput {
        pub(super) fn new(root: &Path) -> SetupResult<Self> {
            let parent = root.parent().ok_or(UNSAFE)?;
            for part in parent.ancestors() {
                let metadata = std::fs::symlink_metadata(part).map_err(|_| UNSAFE)?;
                let mode = metadata.permissions().mode();
                // Avoid redirected paths or writable parents where another
                // account can move an entry that it does not own.
                if !metadata.is_dir() || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
                    return Err(UNSAFE);
                }
            }
            check_new(root)?;
            Ok(Self(root.to_owned()))
        }

        pub(super) fn root(&self) -> &Path {
            &self.0
        }

        pub(super) fn create_directory(&self, path: &Path) -> SetupResult<File> {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(io_error)?;
            File::open(path).map_err(io_error)
        }

        pub(super) fn write_file(&self, path: &Path, bytes: &[u8]) -> SetupResult<()> {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(io_error)?;
            finish_file(file, bytes)
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::OsString,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        path::{Component, Prefix},
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{
            Com::CoTaskMemFree, SystemServices::FILE_PERSISTENT_ACLS, Threading::*,
            WindowsProgramming::DRIVE_FIXED,
        },
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };

    pub(super) fn local_path(path: &Path) -> bool {
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        ) && path.components().all(|part| match part {
            Component::Normal(name) => name.to_str().is_some_and(|text| {
                !text.ends_with([' ', '.']) && !text.contains([':', '\r', '\n', '\0'])
            }),
            _ => true,
        })
    }

    fn wide(path: &Path) -> SetupResult<Vec<u16>> {
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(UNSAFE);
        }
        wide.push(0);
        Ok(wide)
    }

    struct Allocation(*mut core::ffi::c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { LocalFree(self.0) };
            }
        }
    }

    fn owner_acl() -> SetupResult<Allocation> {
        unsafe {
            let mut raw = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
                return Err(UNSAFE);
            }
            let token = OwnedHandle::from_raw_handle(raw);
            let mut length = 0;
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length);
            if length < std::mem::size_of::<TOKEN_USER>() as u32 || length > 4096 {
                return Err(UNSAFE);
            }
            // Align the TOKEN_USER storage and keep its embedded SID alive.
            let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            ) == 0
            {
                return Err(UNSAFE);
            }
            let sid = (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            let mut raw_sid_text = null_mut();
            if ConvertSidToStringSidW(sid, &mut raw_sid_text) == 0 || raw_sid_text.is_null() {
                return Err(UNSAFE);
            }
            let _sid_text_allocation = Allocation(raw_sid_text.cast());
            let end = (0..256)
                .find(|offset| *raw_sid_text.add(*offset) == 0)
                .ok_or(UNSAFE)?;
            let sid_text = String::from_utf16(std::slice::from_raw_parts(raw_sid_text, end))
                .map_err(|_| UNSAFE)?;
            // Explicit owner and protected DACL. Never inherit other principals.
            let sddl: Vec<u16> = format!("O:{sid_text}D:P(A;OICI;FA;;;{sid_text})")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err(UNSAFE);
            }
            Ok(Allocation(descriptor))
        }
    }

    struct KnownFolderAllocation(*mut u16);
    impl Drop for KnownFolderAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CoTaskMemFree(self.0.cast()) };
            }
        }
    }

    pub(super) fn known_local_app_data() -> SetupResult<PathBuf> {
        let mut path = KnownFolderAllocation(null_mut());
        // No KF_FLAG_CREATE: this program never creates the KnownFolder itself.
        if unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut path.0) } < 0
            || path.0.is_null()
        {
            return Err(UNSAFE);
        }
        let end = (0..32768)
            .find(|offset| unsafe { *path.0.add(*offset) == 0 })
            .ok_or(UNSAFE)?;
        let path = PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(path.0, end)
        }));
        if !path.is_absolute() || !local_path(&path) || path.to_str().is_none() {
            return Err(UNSAFE);
        }
        Ok(path)
    }

    fn hold_dir(path: &Path, read_acl: bool) -> SetupResult<File> {
        let path = wide(path)?;
        // The last component is not followed. All its ancestors are opened
        // from the root down and held without FILE_SHARE_DELETE.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                // LIST_DIRECTORY requests data access so share denial applies;
                // FILE_READ_ATTRIBUTES alone would not provide this protection.
                FILE_LIST_DIRECTORY
                    | FILE_READ_ATTRIBUTES
                    | if read_acl { READ_CONTROL } else { 0 },
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(UNSAFE);
        }
        let file = File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
        let info = file_info(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(UNSAFE);
        }
        Ok(file)
    }

    fn file_info(file: &File) -> SetupResult<BY_HANDLE_FILE_INFORMATION> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(UNSAFE);
        }
        Ok(info)
    }

    fn same_dir(first: &File, second: &File) -> SetupResult<bool> {
        let identity = |info: BY_HANDLE_FILE_INFORMATION| {
            (
                info.dwVolumeSerialNumber,
                info.nFileIndexHigh,
                info.nFileIndexLow,
            )
        };
        Ok(identity(file_info(first)?) == identity(file_info(second)?))
    }

    fn pin_ancestors(parent: &Path) -> SetupResult<Vec<File>> {
        let ancestors: Vec<_> = parent.ancestors().collect();
        check_volume(ancestors.last().ok_or(UNSAFE)?)?;
        ancestors
            .into_iter()
            .rev()
            .map(|dir| hold_dir(dir, false))
            .collect()
    }

    fn check_volume(root: &Path) -> SetupResult<()> {
        let root = wide(root)?;
        let mut flags = 0;
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED
            || unsafe {
                GetVolumeInformationW(
                    root.as_ptr(),
                    null_mut(),
                    0,
                    null_mut(),
                    null_mut(),
                    &mut flags,
                    null_mut(),
                    0,
                )
            } == 0
            || flags & FILE_PERSISTENT_ACLS == 0
        {
            return Err(UNSAFE);
        }
        Ok(())
    }

    pub(super) struct PrivateOutput {
        root: PathBuf,
        descriptor: Allocation,
        _ancestors: Vec<File>,
    }

    impl PrivateOutput {
        pub(super) fn new(root: &Path) -> SetupResult<Self> {
            let requested = pin_ancestors(root.parent().ok_or(UNSAFE)?)?;
            let local = known_local_app_data()?;
            let ancestors = pin_ancestors(&local)?;
            if !same_dir(
                requested.last().ok_or(UNSAFE)?,
                ancestors.last().ok_or(UNSAFE)?,
            )? {
                return Err("V2_SETUP_LOCALAPPDATA_CHILD_REQUIRED");
            }
            // Always use the OS-supplied path, not a drive alias supplied by the caller.
            let root = local.join(root.file_name().ok_or(UNSAFE)?);
            check_new(&root)?;
            Ok(Self {
                root,
                descriptor: owner_acl()?,
                _ancestors: ancestors,
            })
        }

        pub(super) fn root(&self) -> &Path {
            &self.root
        }

        fn verify_owner_only(&self, file: &File) -> SetupResult<()> {
            let mut expected_sid = null_mut();
            let mut actual_sid = null_mut();
            let mut owner_defaulted = 0;
            let mut dacl: *mut ACL = null_mut();
            let mut raw_descriptor = null_mut();
            unsafe {
                if GetSecurityDescriptorOwner(
                    self.descriptor.0,
                    &mut expected_sid,
                    &mut owner_defaulted,
                ) == 0
                    || expected_sid.is_null()
                {
                    return Err(UNSAFE);
                }
                let status = GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut actual_sid,
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut raw_descriptor,
                );
                let descriptor = Allocation(raw_descriptor);
                if status != ERROR_SUCCESS
                    || actual_sid.is_null()
                    || dacl.is_null()
                    || descriptor.0.is_null()
                    || EqualSid(actual_sid, expected_sid) == 0
                {
                    return Err(UNSAFE);
                }
                let mut control = 0;
                let mut revision = 0;
                if GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) == 0
                    || control & SE_DACL_PROTECTED == 0
                    || (*dacl).AceCount != 1
                {
                    return Err(UNSAFE);
                }
                let mut raw_ace = null_mut();
                if GetAce(dacl, 0, &mut raw_ace) == 0 || raw_ace.is_null() {
                    return Err(UNSAFE);
                }
                let header = &*raw_ace.cast::<ACE_HEADER>();
                let size = header.AceSize as usize;
                if header.AceType != 0
                    || header.AceFlags & (INHERIT_ONLY_ACE as u8 | INHERITED_ACE as u8) != 0
                    || size < 16
                    || size > ((*dacl).AclSize as usize).saturating_sub(std::mem::size_of::<ACL>())
                {
                    return Err(UNSAFE);
                }
                let ace = &*raw_ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid = (&ace.SidStart as *const u32).cast::<u8>();
                if 8 + 4 * (*sid.add(1)) as usize > size - 8
                    || ace.Mask != FILE_ALL_ACCESS
                    || EqualSid(sid.cast_mut().cast(), expected_sid) == 0
                {
                    return Err(UNSAFE);
                }
            }
            Ok(())
        }

        fn attrs(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor.0,
                bInheritHandle: 0,
            }
        }

        pub(super) fn create_directory(&self, path: &Path) -> SetupResult<File> {
            let wide = wide(path)?;
            if unsafe { CreateDirectoryW(wide.as_ptr(), &self.attrs()) } == 0 {
                return Err(io_error(std::io::Error::last_os_error()));
            }
            let directory = hold_dir(path, true)?;
            self.verify_owner_only(&directory)?;
            Ok(directory)
        }

        pub(super) fn write_file(&self, path: &Path, bytes: &[u8]) -> SetupResult<()> {
            let path = wide(path)?;
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_WRITE | READ_CONTROL,
                    0,
                    &self.attrs(),
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io_error(std::io::Error::last_os_error()));
            }
            let file = File::from(unsafe { OwnedHandle::from_raw_handle(handle) });
            self.verify_owner_only(&file)?;
            finish_file(file, bytes)
        }
    }
}

pub(super) fn local_path(path: &Path) -> bool {
    platform::local_path(path)
}

#[cfg(all(test, windows))]
pub(super) fn known_local_app_data() -> SetupResult<PathBuf> {
    platform::known_local_app_data()
}

pub(super) struct PrivateOutput(platform::PrivateOutput);

impl PrivateOutput {
    pub(super) fn new(root: &Path) -> SetupResult<Self> {
        platform::PrivateOutput::new(root).map(Self)
    }

    pub(super) fn root(&self) -> &Path {
        self.0.root()
    }

    pub(super) fn create_directory(&self, path: &Path) -> SetupResult<File> {
        self.0.create_directory(path)
    }

    pub(super) fn write_file(&self, path: &Path, bytes: &[u8]) -> SetupResult<()> {
        self.0.write_file(path, bytes)
    }
}
