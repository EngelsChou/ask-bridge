#[cfg(not(target_os = "windows"))]
compile_error!("ask-bridge Windows installer can only be built for Windows");

use std::env;
use std::ffi::{OsStr, OsString, c_void};
use std::fs;
use std::io::{self, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

type Hkey = *mut c_void;

const HKEY_CURRENT_USER: Hkey = 0x80000001usize as Hkey;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const HWND_BROADCAST: isize = 0xffff;
const WM_SETTINGCHANGE: u32 = 0x001a;
const SMTO_ABORTIFHUNG: u32 = 0x0002;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: Hkey,
        sub_key: *const u16,
        options: u32,
        desired_access: u32,
        result: *mut Hkey,
    ) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        value_name: *const u16,
        reserved: *mut u32,
        value_type: *mut u32,
        data: *mut u8,
        data_size: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Hkey,
        value_name: *const u16,
        reserved: u32,
        value_type: u32,
        data: *const u8,
        data_size: u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageTimeoutW(
        window: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> usize;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetConsoleProcessList(process_list: *mut u32, process_count: u32) -> u32;
}

struct RegistryKey(Hkey);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn registry_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn open_user_environment() -> io::Result<RegistryKey> {
    let sub_key = wide_null(OsStr::new("Environment"));
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error(status));
    }
    Ok(RegistryKey(key))
}

fn read_registry_path(key: &RegistryKey) -> io::Result<(OsString, u32)> {
    let value_name = wide_null(OsStr::new("Path"));
    let mut value_type = REG_EXPAND_SZ;
    let mut data_size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            value_name.as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            ptr::null_mut(),
            &mut data_size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok((OsString::new(), REG_EXPAND_SZ));
    }
    if status != ERROR_SUCCESS {
        return Err(registry_error(status));
    }

    let mut data = vec![0u16; (data_size as usize).div_ceil(2)];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            value_name.as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            data.as_mut_ptr().cast(),
            &mut data_size,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error(status));
    }
    while data.last() == Some(&0) {
        data.pop();
    }
    Ok((OsString::from_wide(&data), value_type))
}

fn write_registry_path(key: &RegistryKey, value: &OsStr, value_type: u32) -> io::Result<()> {
    let value_name = wide_null(OsStr::new("Path"));
    let data = wide_null(value);
    let registry_type = if matches!(value_type, REG_SZ | REG_EXPAND_SZ) {
        value_type
    } else {
        REG_EXPAND_SZ
    };
    let status = unsafe {
        RegSetValueExW(
            key.0,
            value_name.as_ptr(),
            0,
            registry_type,
            data.as_ptr().cast(),
            (data.len() * size_of::<u16>()) as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error(status));
    }
    Ok(())
}

fn normalized_path_entry(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

pub fn update_user_path(install_dir: &Path, add: bool) -> io::Result<bool> {
    let key = open_user_environment()?;
    let (current, value_type) = read_registry_path(&key)?;
    let current_text = current.to_string_lossy();
    let install_text = install_dir.to_string_lossy();
    let normalized_install = normalized_path_entry(&install_text);
    let mut entries: Vec<String> = current_text
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let already_present = entries
        .iter()
        .any(|entry| normalized_path_entry(entry) == normalized_install);

    if add {
        if already_present {
            return Ok(false);
        }
        entries.push(install_text.into_owned());
    } else {
        if !already_present {
            return Ok(false);
        }
        entries.retain(|entry| normalized_path_entry(entry) != normalized_install);
    }

    let new_value = OsString::from(entries.join(";"));
    write_registry_path(&key, &new_value, value_type)?;
    broadcast_environment_change();
    Ok(true)
}

fn broadcast_environment_change() {
    let environment = wide_null(OsStr::new("Environment"));
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            &mut result,
        );
    }
}

pub fn default_install_dir() -> Result<PathBuf, String> {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .ok_or_else(|| "找不到 USERPROFILE，無法決定使用者安裝目錄。".to_string())?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

#[allow(dead_code)]
pub fn default_config_dir() -> Result<PathBuf, String> {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .ok_or_else(|| "找不到 USERPROFILE，無法決定使用者設定目錄。".to_string())?;
    Ok(PathBuf::from(home).join(".config").join("ask-bridge"))
}

#[allow(dead_code)]
pub fn write_embedded_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(|value| format!("{value}.tmp-{}", std::process::id()))
        .unwrap_or_else(|| format!("tmp-{}", std::process::id()));
    let temporary = path.with_extension(extension);
    let _ = fs::remove_file(&temporary);
    fs::write(&temporary, contents)?;

    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[allow(dead_code)]
pub fn remove_file_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn pause_if_launched_directly(quiet: bool) {
    if quiet {
        return;
    }
    let mut process_ids = [0u32; 4];
    let process_count =
        unsafe { GetConsoleProcessList(process_ids.as_mut_ptr(), process_ids.len() as u32) };
    if process_count <= 1 {
        print!("按 Enter 鍵結束...");
        let _ = io::stdout().flush();
        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
    }
}
