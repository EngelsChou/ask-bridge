#[path = "../common.rs"]
mod common;

use std::env;
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_VERSION: &str = env!("ASK_BRIDGE_APP_VERSION");
const PAYLOAD_FINGERPRINT: &str = env!("ASK_BRIDGE_PAYLOAD_FINGERPRINT");
const ASK_BRIDGE: &[u8] = include_bytes!(env!("ASK_BRIDGE_BINARY_PATH"));
const ASK_BRIDGE_UPDATE: &[u8] = include_bytes!(env!("ASK_BRIDGE_UPDATE_BINARY_PATH"));
const UNINSTALLER: &[u8] = include_bytes!(env!("ASK_BRIDGE_UNINSTALL_BINARY_PATH"));

#[repr(C)]
struct VsFixedFileInfo {
    signature: u32,
    _structure_version: u32,
    file_version_ms: u32,
    file_version_ls: u32,
    _product_version_ms: u32,
    _product_version_ls: u32,
    _file_flags_mask: u32,
    _file_flags: u32,
    _file_os: u32,
    _file_type: u32,
    _file_subtype: u32,
    _file_date_ms: u32,
    _file_date_ls: u32,
}

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(file_name: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(
        file_name: *const u16,
        handle: u32,
        length: u32,
        data: *mut c_void,
    ) -> i32;
    fn VerQueryValueW(
        block: *const c_void,
        sub_block: *const u16,
        buffer: *mut *mut c_void,
        length: *mut u32,
    ) -> i32;
}

struct Options {
    install_dir: PathBuf,
    update_path: bool,
    quiet: bool,
    allow_downgrade: bool,
}

fn main() {
    let options = match parse_options() {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(error) => {
            eprintln!("安裝參數錯誤：{error}");
            common::pause_if_launched_directly(false);
            std::process::exit(2);
        }
    };

    if let Err(error) = install(&options) {
        eprintln!("安裝失敗：{error}");
        eprintln!("請先關閉正在執行的 ask-bridge，再重新執行 install.exe。");
        common::pause_if_launched_directly(options.quiet);
        std::process::exit(1);
    }
    common::pause_if_launched_directly(options.quiet);
}

fn parse_options() -> Result<Option<Options>, String> {
    let mut install_dir = common::default_install_dir()?;
    let mut update_path = true;
    let mut quiet = false;
    let mut allow_downgrade = false;
    let mut args = env::args_os().skip(1);

    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--install-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--install-dir 後方必須提供目錄。".to_string())?;
                install_dir = PathBuf::from(value);
            }
            "--no-path" => update_path = false,
            "--quiet" => quiet = true,
            "--allow-downgrade" => allow_downgrade = true,
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            value => return Err(format!("不支援的參數：{value}")),
        }
    }

    Ok(Some(Options {
        install_dir,
        update_path,
        quiet,
        allow_downgrade,
    }))
}

fn print_help() {
    println!("Ask Bridge 離線安裝程式 {APP_VERSION}");
    println!("用法：install.exe [--install-dir <目錄>] [--no-path] [--quiet] [--allow-downgrade]");
    println!("  --install-dir <目錄>  指定安裝位置，預設為 %USERPROFILE%\\.local\\bin");
    println!("  --no-path             不修改使用者 PATH");
    println!("  --quiet               結束時不等待按鍵");
    println!("  --allow-downgrade     明確允許以較舊版本覆寫目前安裝");
}

fn install(options: &Options) -> Result<(), String> {
    let install_dir = absolute_path(&options.install_dir)?;
    // Hold the same per-directory lock used by install.ps1 from the version
    // check until every payload has been atomically replaced.
    let _install_lock = common::acquire_install_lock(&install_dir)?;
    println!("Ask Bridge {APP_VERSION} 離線安裝程式");
    println!("安裝目錄：{}", install_dir.display());

    guard_against_downgrade(
        &install_dir.join("ask-bridge.exe"),
        APP_VERSION,
        options.allow_downgrade,
    )?;

    let files = [
        ("ask-bridge-update.exe", ASK_BRIDGE_UPDATE),
        ("uninstall.exe", UNINSTALLER),
        ("ask.exe", ASK_BRIDGE),
        // The authoritative executable is the commit point. If any supporting
        // payload cannot be replaced, the previous main binary remains intact.
        ("ask-bridge.exe", ASK_BRIDGE),
    ];
    for (name, contents) in files {
        let destination = install_dir.join(name);
        common::write_embedded_file(&destination, contents)
            .map_err(|error| format!("無法寫入 {}：{error}", destination.display()))?;
        println!("已安裝：{}", destination.display());
    }

    if options.update_path {
        match common::update_user_path(&install_dir, true) {
            Ok(true) => println!("已將安裝目錄加入使用者 PATH。"),
            Ok(false) => println!("使用者 PATH 已包含安裝目錄。"),
            Err(error) => return Err(format!("無法更新使用者 PATH：{error}")),
        }
    } else {
        println!("依 --no-path 略過使用者 PATH 更新。")
    }

    print_runtime_status();
    println!();
    println!("安裝完成。請重新開啟終端機後執行 ask-bridge --version。");
    println!(
        "解除安裝可執行：{}",
        install_dir.join("uninstall.exe").display()
    );
    let _ = PAYLOAD_FINGERPRINT;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("無法解析安裝目錄：{error}"))
}

fn command_version(command: &str, argument: &str) -> Option<OsString> {
    let output = Command::new(command).arg(argument).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(OsString::from(text))
    }
}

fn parse_stable_version(text: &str) -> Option<[u64; 3]> {
    text.split_whitespace().find_map(|candidate| {
        let candidate = candidate.trim_start_matches('v');
        if candidate.contains('-') || candidate.contains('+') {
            return None;
        }
        let mut parts = candidate.split('.');
        let version = [
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ];
        parts.next().is_none().then_some(version)
    })
}

fn guard_against_downgrade(
    installed_binary: &Path,
    target_version: &str,
    allow_downgrade: bool,
) -> Result<(), String> {
    if allow_downgrade || !installed_binary.is_file() {
        return Ok(());
    }

    let installed = read_file_version(installed_binary).map_err(|error| {
        format!(
            "無法安全讀取既有程式 {} 的版本資源：{error}。若確定要覆寫，請加上 --allow-downgrade。",
            installed_binary.display()
        )
    })?;
    let target = parse_stable_version(target_version)
        .ok_or_else(|| format!("安裝程式版本 `{target_version}` 不是有效的穩定版本。"))?;

    ensure_version_transition(installed, target, allow_downgrade).map_err(|_| {
        format!(
            "已安裝版本 {}.{}.{} 比此安裝程式的 {target_version} 更新；已拒絕降版。若確定要降版，請加上 --allow-downgrade。",
            installed[0], installed[1], installed[2]
        )
    })
}

fn read_file_version(path: &Path) -> Result<[u64; 3], String> {
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut unused_handle = 0_u32;
    let info_size = unsafe { GetFileVersionInfoSizeW(wide_path.as_ptr(), &mut unused_handle) };
    if info_size == 0 {
        return Err("檔案沒有可讀取的 Windows version resource".to_string());
    }

    let mut data = vec![0_u8; info_size as usize];
    if unsafe { GetFileVersionInfoW(wide_path.as_ptr(), 0, info_size, data.as_mut_ptr().cast()) }
        == 0
    {
        return Err("GetFileVersionInfoW 失敗".to_string());
    }

    let root: Vec<u16> = OsStr::new("\\").encode_wide().chain(Some(0)).collect();
    let mut value = std::ptr::null_mut::<c_void>();
    let mut value_size = 0_u32;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            root.as_ptr(),
            &mut value,
            &mut value_size,
        )
    } == 0
        || value.is_null()
        || value_size < std::mem::size_of::<VsFixedFileInfo>() as u32
    {
        return Err("VerQueryValueW 無法取得固定版本資訊".to_string());
    }

    // `VerQueryValueW` returns a pointer into the byte buffer above.  Rust does
    // not guarantee that a `Vec<u8>` is aligned for `VsFixedFileInfo`, so copy
    // the value out with an unaligned read instead of creating a reference.
    let fixed = unsafe { std::ptr::read_unaligned(value.cast::<VsFixedFileInfo>()) };
    if fixed.signature != 0xFEEF_04BD {
        return Err("Windows version resource signature 無效".to_string());
    }

    Ok([
        (fixed.file_version_ms >> 16) as u64,
        (fixed.file_version_ms & 0xffff) as u64,
        (fixed.file_version_ls >> 16) as u64,
    ])
}

fn ensure_version_transition(
    installed: [u64; 3],
    target: [u64; 3],
    allow_downgrade: bool,
) -> Result<(), ()> {
    if installed > target && !allow_downgrade {
        Err(())
    } else {
        Ok(())
    }
}

fn print_runtime_status() {
    println!();
    println!("執行環境檢查（安裝程式不會執行 npm install）：");
    match command_version("node", "--version") {
        Some(version) => println!("  Node.js：{}", version.to_string_lossy()),
        None => println!("  警告：PATH 中找不到 Node.js。"),
    }
    match command_version("npx.cmd", "--version") {
        Some(version) => println!("  npx：{}", version.to_string_lossy()),
        None => println!("  警告：PATH 中找不到 npx.cmd。"),
    }
    if chrome_is_installed() {
        println!("  Google Chrome：已在標準位置找到。")
    } else {
        println!("  警告：Google Chrome 不在標準安裝位置。")
    }
    println!("  chrome-devtools-mcp：沿用公司電腦既有的 npx 安裝／快取，不另行下載。")
}

fn chrome_is_installed() -> bool {
    let candidates = [
        env::var_os("ProgramFiles").map(|root| {
            PathBuf::from(root)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
        env::var_os("ProgramFiles(x86)").map(|root| {
            PathBuf::from(root)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
        env::var_os("LOCALAPPDATA").map(|root| {
            PathBuf::from(root)
                .join("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe")
        }),
    ];
    candidates.into_iter().flatten().any(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_version_transition, guard_against_downgrade, parse_stable_version, read_file_version,
    };

    #[test]
    fn parses_cli_and_plain_stable_versions() {
        assert_eq!(parse_stable_version("ask-bridge 0.3.1\n"), Some([0, 3, 1]));
        assert_eq!(parse_stable_version("v12.34.56"), Some([12, 34, 56]));
    }

    #[test]
    fn rejects_incomplete_or_prerelease_versions() {
        assert_eq!(parse_stable_version("0.3"), None);
        assert_eq!(parse_stable_version("ask-bridge 0.3.1-beta.1"), None);
        assert_eq!(parse_stable_version("not-a-version"), None);
    }

    #[test]
    fn rejects_downgrade_unless_explicitly_allowed() {
        assert!(ensure_version_transition([0, 3, 2], [0, 3, 1], false).is_err());
        assert!(ensure_version_transition([0, 3, 2], [0, 3, 1], true).is_ok());
        assert!(ensure_version_transition([0, 3, 1], [0, 3, 1], false).is_ok());
        assert!(ensure_version_transition([0, 3, 0], [0, 3, 1], false).is_ok());
    }

    #[test]
    fn reads_version_resource_without_executing_the_target() {
        let current_exe = std::env::current_exe().expect("test executable path");
        let expected = parse_stable_version(super::APP_VERSION).expect("stable installer version");
        assert_eq!(read_file_version(&current_exe).unwrap(), expected);
        assert!(guard_against_downgrade(&current_exe, "0.3.0", false).is_err());
        assert!(guard_against_downgrade(&current_exe, "0.3.0", true).is_ok());
    }
}
