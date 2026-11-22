use super::{Error, Identity};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::{Com::CoTaskMemFree, Threading::*},
    UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
};
use zeroize::{Zeroize, Zeroizing};

const ENTROPY: &[u8] = b"RemoteHub.Identity.CurrentUser.v1";
const FILE_MAGIC: &[u8; 8] = b"RHDPA001";
const MAX_FILE: usize = 16 * 1024;
const KEY_FILE: &str = "identity.dpapi";
const COMMIT_MAGIC: &[u8; 8] = b"RHCOM001";

struct LocalAllocation(*mut core::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}
struct ProtectedBlob(CRYPT_INTEGER_BLOB);
impl ProtectedBlob {
    fn bytes(&self) -> &[u8] {
        if self.0.cbData == 0 || self.0.pbData.is_null() {
            return &[];
        }
        // Returned length/allocation are supplied by the Windows cryptographic API.
        unsafe { std::slice::from_raw_parts(self.0.pbData, self.0.cbData as usize) }
    }
}
impl Drop for ProtectedBlob {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                LocalFree(self.0.pbData.cast());
            }
        }
    }
}
fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    }
}
fn protect(bytes: &[u8], entropy: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = ProtectedBlob(unsafe { std::mem::zeroed() });
    let ok = unsafe {
        CryptProtectData(
            &blob(bytes),
            null(),
            &blob(entropy),
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.0,
        )
    };
    if ok == 0 || output.bytes().is_empty() || output.bytes().len() + 12 > MAX_FILE {
        return Err(Error::Protect);
    }
    Ok(output.bytes().to_vec())
}
fn unprotect(bytes: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    if bytes.is_empty() || bytes.len() > MAX_FILE {
        return Err(Error::InvalidStore);
    }
    let mut output = ProtectedBlob(unsafe { std::mem::zeroed() });
    let ok = unsafe {
        CryptUnprotectData(
            &blob(bytes),
            null_mut(),
            &blob(entropy),
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.0,
        )
    };
    if ok == 0 {
        return Err(Error::Unprotect);
    }
    if output.bytes().len() != 72 {
        return Err(Error::InvalidStore);
    }
    Ok(Zeroizing::new(output.bytes().to_vec()))
}
fn random(bytes: &mut [u8]) -> Result<(), Error> {
    if unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(Error::Random);
    }
    Ok(())
}
fn wide(path: &Path) -> Result<Vec<u16>, Error> {
    let mut result: Vec<u16> = path.as_os_str().encode_wide().collect();
    if result.contains(&0) {
        return Err(Error::UnsafeStorage);
    }
    result.push(0);
    Ok(result)
}

struct Security {
    token_user: Vec<usize>,
    descriptor: LocalAllocation,
}
impl Security {
    fn new() -> Result<Self, Error> {
        let mut raw = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(Error::UnsafeStorage);
        }
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length);
        }
        if length == 0 || length > 4096 {
            return Err(Error::UnsafeStorage);
        }
        // usize storage keeps TOKEN_USER correctly aligned; its SID points into this allocation.
        let mut token_user = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                token_user.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(Error::UnsafeStorage);
        }
        let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut sid_string = null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_string) } == 0 {
            return Err(Error::UnsafeStorage);
        }
        let _sid_allocation = LocalAllocation(sid_string.cast());
        let mut n = 0;
        while n < 256 && unsafe { *sid_string.add(n) } != 0 {
            n += 1;
        }
        if n == 256 {
            return Err(Error::UnsafeStorage);
        }
        let sid_text = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_string, n) })
            .map_err(|_| Error::UnsafeStorage)?;
        let sddl: Vec<u16> = format!("O:{sid_text}D:P(A;OICI;FA;;;{sid_text})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(Error::UnsafeStorage);
        }
        Ok(Self {
            token_user,
            descriptor: LocalAllocation(descriptor),
        })
    }
    fn sid(&self) -> PSID {
        unsafe { (*(self.token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }
    fn create_directory(&self, path: &Path) -> Result<(), Error> {
        if unsafe { CreateDirectoryW(wide(path)?.as_ptr(), &self.attributes()) } == 0
            && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
        {
            return Err(Error::Storage);
        }
        Ok(())
    }
    fn open(
        &self,
        path: &Path,
        access: u32,
        share: u32,
        creation: u32,
        flags: u32,
    ) -> Result<File, Error> {
        let handle = unsafe {
            CreateFileW(
                wide(path)?.as_ptr(),
                access,
                share,
                &self.attributes(),
                creation,
                flags | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(if unsafe { GetLastError() } == ERROR_SHARING_VIOLATION {
                Error::Busy
            } else {
                Error::Storage
            });
        }
        Ok(File::from(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }
    fn verify(&self, file: &File, directory: bool, private: bool) -> Result<(), Error> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks != 1)
        {
            return Err(Error::UnsafeStorage);
        }
        if !private {
            return Ok(());
        }
        let mut owner = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor = null_mut();
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        let _descriptor = LocalAllocation(descriptor);
        if status != ERROR_SUCCESS
            || dacl.is_null()
            || owner.is_null()
            || unsafe { EqualSid(owner, self.sid()) } == 0
        {
            return Err(Error::UnsafeStorage);
        }
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
            || unsafe { (*dacl).AceCount } != 1
        {
            return Err(Error::UnsafeStorage);
        }
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, 0, &mut ace) } == 0 || ace.is_null() {
            return Err(Error::UnsafeStorage);
        }
        // Validate the variable-size ACE before treating it as ACCESS_ALLOWED_ACE/SID.
        let header = unsafe { &*(ace.cast::<ACE_HEADER>()) };
        let size = header.AceSize as usize;
        if header.AceType != 0
            || size < 16
            || size
                > (unsafe { (*dacl).AclSize } as usize).saturating_sub(std::mem::size_of::<ACL>())
        {
            return Err(Error::UnsafeStorage);
        }
        let ace = unsafe { &*(ace.cast::<ACCESS_ALLOWED_ACE>()) };
        let sid_bytes = (&ace.SidStart as *const u32).cast::<u8>();
        if 8 + 4 * unsafe { *sid_bytes.add(1) } as usize > size - 8
            || ace.Mask != FILE_ALL_ACCESS
            || unsafe { EqualSid((&ace.SidStart as *const u32).cast_mut().cast(), self.sid()) } == 0
        {
            return Err(Error::UnsafeStorage);
        }
        Ok(())
    }
    fn directory(&self, path: &Path, private: bool) -> Result<File, Error> {
        self.create_directory(path)?;
        let dir = self.open(
            path,
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
        )?;
        self.verify(&dir, true, private)?;
        Ok(dir) // No FILE_SHARE_DELETE: hold directory identity for the entire operation.
    }
}

pub(super) fn open_default() -> Result<Identity, Error> {
    let mut raw = null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut raw) } < 0
        || raw.is_null()
    {
        return Err(Error::Storage);
    }
    let mut n = 0;
    while n < 32768 && unsafe { *raw.add(n) } != 0 {
        n += 1;
    }
    let path = if n < 32768 {
        String::from_utf16(unsafe { std::slice::from_raw_parts(raw, n) })
            .ok()
            .map(PathBuf::from)
    } else {
        None
    };
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    let root = path.ok_or(Error::Storage)?.join("RemoteHub");
    let security = Security::new()?;
    let _parent = security.directory(&root, false)?;
    open_or_create(&root.join("identity-v1"))
}

pub(super) fn open_or_create(root: &Path) -> Result<Identity, Error> {
    use std::path::{Component, Prefix};
    if !root.is_absolute()
        || root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || !matches!(root.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        return Err(Error::UnsafeStorage);
    }
    let security = Security::new()?;
    let _directory = security.directory(root, true)?;
    let mut lock = security.open(
        &root.join("identity.lock"),
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        0,
        OPEN_ALWAYS,
        0,
    )?;
    security.verify(&lock, false, true)?;
    let mut marker = Vec::new();
    Read::by_ref(&mut lock)
        .take(41)
        .read_to_end(&mut marker)
        .map_err(|_| Error::Storage)?;
    let committed: Option<[u8; 32]> = if marker.is_empty() {
        None
    } else if marker.len() == 40 && &marker[..8] == COMMIT_MAGIC {
        Some(marker[8..].try_into().map_err(|_| Error::InvalidStore)?)
    } else {
        return Err(Error::InvalidStore);
    };
    if std::fs::read_dir(root)
        .map_err(|_| Error::Storage)?
        .any(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().starts_with("pending-"))
                .unwrap_or(true)
        })
    {
        return Err(Error::IncompleteStore);
    }
    let path = root.join(KEY_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let identity = load(&security, &path)?;
            verify_or_commit(&mut lock, committed, &identity)?;
            return Ok(identity);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Error::Storage),
    }
    if committed.is_some() {
        return Err(Error::IncompleteStore);
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    random(&mut seed[..])?;
    let identity = Identity {
        key: SigningKey::from_bytes(&seed),
    };
    let sealed = protect(&identity.payload(), ENTROPY)?;
    let mut nonce = [0u8; 8];
    random(&mut nonce)?;
    let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let pending = root.join(format!("pending-{suffix}"));
    let mut created = false;
    let write = (|| {
        let mut file = security.open(
            &pending,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            CREATE_NEW,
            0,
        )?;
        created = true;
        security.verify(&file, false, true)?;
        file.write_all(FILE_MAGIC)
            .and_then(|_| file.write_all(&(sealed.len() as u32).to_le_bytes()))
            .and_then(|_| file.write_all(&sealed))
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::Storage)?;
        drop(file);
        // No REPLACE_EXISTING flag. Never overwrite a concurrently supplied identity.
        if unsafe {
            MoveFileExW(
                wide(&pending)?.as_ptr(),
                wide(&path)?.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(Error::Storage);
        }
        Ok(())
    })();
    if write.is_err() && created {
        let _ = std::fs::remove_file(&pending);
    }
    write?;
    // Do not return a new public identity until its persisted form is decryptable and verified.
    let restored = load(&security, &path)?;
    if restored.public() != identity.public() {
        return Err(Error::InvalidStore);
    }
    verify_or_commit(&mut lock, committed, &restored)?;
    Ok(restored)
}
fn verify_or_commit(
    lock: &mut File,
    committed: Option<[u8; 32]>,
    identity: &Identity,
) -> Result<(), Error> {
    let digest: [u8; 32] = Sha256::digest(identity.public().public_key).into();
    match committed {
        Some(expected) if expected != digest => Err(Error::InvalidStore),
        Some(_) => Ok(()),
        None => lock
            .write_all(COMMIT_MAGIC)
            .and_then(|_| lock.write_all(&digest))
            .and_then(|_| lock.sync_all())
            .map_err(|_| Error::Storage),
    }
}
fn load(security: &Security, path: &Path) -> Result<Identity, Error> {
    let mut file = security.open(path, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING, 0)?;
    security.verify(&file, false, true)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() < 13 || bytes.len() > MAX_FILE || &bytes[..8] != FILE_MAGIC {
        return Err(Error::InvalidStore);
    }
    let length =
        u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| Error::InvalidStore)?) as usize;
    if length != bytes.len() - 12 {
        return Err(Error::InvalidStore);
    }
    Identity::from_payload(&unprotect(&bytes[12..], ENTROPY)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn root() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "remotehub-identity-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&base).unwrap();
        base.join("identity")
    }
    fn clean(root: &Path) {
        // Each root is created above; no user/production identity path is accepted here.
        let target = root.parent().unwrap().canonicalize().unwrap();
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(target.parent(), Some(temp.as_path()));
        assert!(target
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("remotehub-identity-test-{}-", std::process::id())));
        std::fs::remove_dir_all(target).unwrap();
    }
    #[test]
    fn real_dpapi_roundtrip_entropy_and_tamper_rejection() {
        let identity = Identity {
            key: SigningKey::from_bytes(&[17; 32]),
        };
        let sealed = protect(&identity.payload(), ENTROPY).unwrap();
        assert!(
            Identity::from_payload(&unprotect(&sealed, ENTROPY).unwrap())
                .unwrap()
                .public()
                == identity.public()
        );
        assert!(unprotect(&sealed, b"another-purpose").is_err());
        let mut bad = sealed;
        let n = bad.len() - 1;
        bad[n] ^= 1;
        assert!(unprotect(&bad, ENTROPY).is_err());
    }
    #[test]
    fn store_restores_same_identity_and_contains_no_seed_or_public_key() {
        let root = root();
        let first = open_or_create(&root).unwrap();
        let bytes = std::fs::read(root.join(KEY_FILE)).unwrap();
        assert!(!bytes
            .windows(32)
            .any(|w| w == first.key.as_bytes() || w == first.public().public_key));
        let again = open_or_create(&root).unwrap();
        assert!(first.public() == again.public());
        assert!(super::super::verify_local_proof(
            &again.public().public_key,
            &[2; 32],
            &first.local_proof(&[2; 32])
        ));
        // A crash before recording the first commit can leave an empty lock, not a new identity.
        std::fs::write(root.join("identity.lock"), []).unwrap();
        let recovered = open_or_create(&root).unwrap();
        assert!(recovered.public() == first.public());
        assert!(std::fs::read(root.join(KEY_FILE)).unwrap() == bytes);
        assert_eq!(
            std::fs::metadata(root.join("identity.lock")).unwrap().len(),
            40
        );
        drop(first);
        drop(again);
        clean(&root);
    }
    #[test]
    fn corrupted_store_is_not_overwritten_or_regenerated() {
        let root = root();
        open_or_create(&root).unwrap();
        let file = root.join(KEY_FILE);
        let original = std::fs::read(&file).unwrap();
        for bad in [
            vec![],
            vec![0; 32],
            vec![0; MAX_FILE + 1],
            {
                let mut b = original.clone();
                b[8] ^= 1;
                b
            },
            {
                let mut b = original.clone();
                let n = b.len() - 1;
                b[n] ^= 1;
                b
            },
        ] {
            std::fs::write(&file, &bad).unwrap();
            assert!(open_or_create(&root).is_err());
            // A failed assertion must not dump a protected identity blob into test logs.
            assert!(std::fs::read(&file).unwrap() == bad);
        }
        std::fs::write(&file, &original).unwrap();
        for bad_marker in [vec![0; 1], vec![0; 40], vec![0; 41]] {
            std::fs::write(root.join("identity.lock"), &bad_marker).unwrap();
            assert!(matches!(open_or_create(&root), Err(Error::InvalidStore)));
            assert!(std::fs::read(&file).unwrap() == original);
        }
        clean(&root);
    }
    #[test]
    fn lock_conflict_and_pending_file_fail_closed() {
        let root = root();
        open_or_create(&root).unwrap();
        let security = Security::new().unwrap();
        let lock = security
            .open(
                &root.join("identity.lock"),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                OPEN_EXISTING,
                0,
            )
            .unwrap();
        assert!(matches!(open_or_create(&root), Err(Error::Busy)));
        drop(lock);
        std::fs::write(root.join("pending-incomplete"), b"sealed-test-fixture").unwrap();
        assert!(matches!(open_or_create(&root), Err(Error::IncompleteStore)));
        clean(&root);
    }
    #[test]
    fn hardlinked_identity_is_rejected() {
        let root = root();
        open_or_create(&root).unwrap();
        std::fs::hard_link(root.join(KEY_FILE), root.join("alias")).unwrap();
        assert!(matches!(open_or_create(&root), Err(Error::UnsafeStorage)));
        clean(&root);
    }
    #[test]
    fn unprotected_existing_directory_is_rejected() {
        let root = root();
        std::fs::create_dir(&root).unwrap();
        assert!(matches!(open_or_create(&root), Err(Error::UnsafeStorage)));
        assert!(!root.join(KEY_FILE).exists());
        clean(&root);
    }
    #[test]
    fn concurrent_creators_converge_on_one_persisted_identity() {
        let root = root();
        let workers: Vec<_> = (0..6)
            .map(|_| {
                let path = root.clone();
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        match open_or_create(&path) {
                            Ok(id) => return id.public().fingerprint,
                            Err(Error::Busy) => {
                                std::thread::sleep(std::time::Duration::from_millis(10))
                            }
                            Err(error) => panic!("unexpected fixed identity error: {error}"),
                        }
                    }
                    panic!("identity lock did not become available");
                })
            })
            .collect();
        let fingerprints: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert!(fingerprints.iter().all(|v| v == &fingerprints[0]));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        clean(&root);
    }
    #[test]
    fn missing_confirmed_key_is_not_silently_regenerated() {
        let root = root();
        open_or_create(&root).unwrap();
        std::fs::remove_file(root.join(KEY_FILE)).unwrap();
        let rejected = matches!(open_or_create(&root), Err(Error::IncompleteStore));
        clean(&root);
        assert!(
            rejected,
            "a missing committed key must not silently change identity"
        );
    }
    #[test]
    fn substituted_valid_dpapi_identity_is_rejected() {
        let first = root();
        let second = root();
        open_or_create(&first).unwrap();
        open_or_create(&second).unwrap();
        std::fs::copy(second.join(KEY_FILE), first.join(KEY_FILE)).unwrap();
        let rejected = matches!(open_or_create(&first), Err(Error::InvalidStore));
        clean(&first);
        clean(&second);
        assert!(
            rejected,
            "another valid protected identity must not replace a committed key"
        );
    }
}
