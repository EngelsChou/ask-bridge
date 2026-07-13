#[path = "../common.rs"]
mod common;

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_VERSION: &str = env!("ASK_BRIDGE_APP_VERSION");
const PAYLOAD_FINGERPRINT: &str = env!("ASK_BRIDGE_PAYLOAD_FINGERPRINT");
const ASK_BRIDGE: &[u8] = include_bytes!(env!("ASK_BRIDGE_BINARY_PATH"));
const ASK_BRIDGE_UPDATE: &[u8] = include_bytes!(env!("ASK_BRIDGE_UPDATE_BINARY_PATH"));
const UNINSTALLER: &[u8] = include_bytes!(env!("ASK_BRIDGE_UNINSTALL_BINARY_PATH"));

struct Options {
    install_dir: PathBuf,
    update_path: bool,
    quiet: bool,
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
    }))
}

fn print_help() {
    println!("Ask Bridge 離線安裝程式 {APP_VERSION}");
    println!("用法：install.exe [--install-dir <目錄>] [--no-path] [--quiet]");
    println!("  --install-dir <目錄>  指定安裝位置，預設為 %USERPROFILE%\\.local\\bin");
    println!("  --no-path             不修改使用者 PATH");
    println!("  --quiet               結束時不等待按鍵");
}

fn install(options: &Options) -> Result<(), String> {
    let install_dir = absolute_path(&options.install_dir)?;
    println!("Ask Bridge {APP_VERSION} 離線安裝程式");
    println!("安裝目錄：{}", install_dir.display());

    let files = [
        ("ask-bridge.exe", ASK_BRIDGE),
        ("ask.exe", ASK_BRIDGE),
        ("ask-bridge-update.exe", ASK_BRIDGE_UPDATE),
        ("uninstall.exe", UNINSTALLER),
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
