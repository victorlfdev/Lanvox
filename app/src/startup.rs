//! Per-user Windows startup registration and desktop instance ownership.

#[cfg(windows)]
mod windows {
    use std::{io, ptr};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, HANDLE,
        },
        System::{Registry::*, Threading::CreateMutexW},
    };

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn check(code: u32) -> io::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code as i32))
        }
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    pub fn enabled() -> io::Result<bool> {
        let mut key = ptr::null_mut();
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(code)?;
        let key = Key(key);
        let mut size = 0;
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                wide("Lanvox").as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(code)?;
        Ok(size > 2)
    }

    pub fn set_enabled(enabled: bool) -> io::Result<()> {
        let mut key = ptr::null_mut();
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_SET_VALUE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        })?;
        let key = Key(key);
        if enabled {
            use std::os::windows::ffi::OsStrExt;
            let executable = std::env::current_exe()?;
            let command: Vec<u16> = std::iter::once(b'"' as u16)
                .chain(executable.as_os_str().encode_wide())
                .chain("\" --autostart".encode_utf16())
                .chain(Some(0))
                .collect();
            check(unsafe {
                RegSetValueExW(
                    key.0,
                    wide("Lanvox").as_ptr(),
                    0,
                    REG_SZ,
                    command.as_ptr().cast(),
                    (command.len() * 2) as u32,
                )
            })
        } else {
            let code = unsafe { RegDeleteValueW(key.0, wide("Lanvox").as_ptr()) };
            if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                check(code)
            }
        }
    }

    pub struct DesktopInstance(HANDLE);
    impl Drop for DesktopInstance {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    impl DesktopInstance {
        pub fn acquire() -> io::Result<Option<Self>> {
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            dirs::config_dir().hash(&mut hash);
            let name = wide(&format!("Local\\LanvoxDesktop-{:x}", hash.finish()));
            Self::acquire_named(&name)
        }

        fn acquire_named(name: &[u16]) -> io::Result<Option<Self>> {
            let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            let guard = Self(handle);
            Ok((!exists).then_some(guard))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop_instance_excludes_duplicates_and_releases_on_exit() {
            let name = wide(&format!(
                "Local\\LanvoxTest-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let first = DesktopInstance::acquire_named(&name).unwrap().unwrap();
            assert!(DesktopInstance::acquire_named(&name).unwrap().is_none());
            drop(first);
            assert!(DesktopInstance::acquire_named(&name).unwrap().is_some());
        }
    }
}

#[cfg(windows)]
pub use windows::*;
