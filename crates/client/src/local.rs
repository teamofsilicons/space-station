//! Shared filesystem privacy for the daemon and the CLI credential store.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Create a directory accessible only to its owner, including on Windows.
pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    private(path, true)
}

/// Open a private data file. Callers create its private parent before storing secrets.
pub fn open_private(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    private(path, false)?;
    Ok(file)
}

#[cfg(unix)]
pub(crate) fn private(path: &Path, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }))
}

#[cfg(windows)]
pub(crate) fn private(path: &Path, _directory: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };

    // Protected DACL: the owner alone has full access; children inherit it. OW is the
    // Owner Rights SID, so no username lookup, shell command, or inherited Users ACE remains.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path[..path.len() - 1].contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains a null character"));
    }
    let mut descriptor = null_mut();
    // SAFETY: both strings are terminated; Windows allocates the descriptor, which remains
    // valid through SetFileSecurityW and is freed exactly once with LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = (result == 0).then(io::Error::last_os_error);
        LocalFree(descriptor);
        error.map_or(Ok(()), Err)
    }
}
