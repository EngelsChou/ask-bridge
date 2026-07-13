#[path = "../common.rs"]
mod common;

use std::env;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

struct Options {
    install_dir: PathBuf,
    update_path: bool,
    purge: bool,
    quiet: bool,
}

fn main() {
    let options = match parse_options() {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(error) => {
            eprintln!("解除安裝參數錯誤：{error}");
            common::pause_if_launched_directly(false);
            std::process::exit(2);
        }
    };

    if let Err(error) = uninstall(&options) {
        eprintln!("解除安裝失敗：{error}");
        common::pause_if_launched_directly(options.quiet);
        std::process::exit(1);
    }
    common::pause_if_launched_directly(options.quiet);
}

fn parse_options() -> Result<Option<Options>, String> {
    let mut explicit_install_dir = None;
    let mut update_path = true;
    let mut purge = false;
    let mut quiet = false;
    let mut args = env::args_os().skip(1);

    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--install-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--install-dir 後方必須提供目錄。".to_string())?;
                explicit_install_dir = Some(PathBuf::from(value));
            }
            "--no-path" => update_path = false,
            "--purge" => purge = true,
            "--quiet" => quiet = true,
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            value => return Err(format!("不支援的參數：{value}")),
        }
    }

    let install_dir = match explicit_install_dir {
        Some(path) => absolute_path(&path)?,
        None => infer_install_dir().unwrap_or(common::default_install_dir()?),
    };
    Ok(Some(Options {
        install_dir,
        update_path,
        purge,
        quiet,
    }))
}

fn print_help() {
    println!("Ask Bridge 離線解除安裝程式");
    println!("用法：uninstall.exe [--install-dir <目錄>] [--no-path] [--purge] [--quiet]");
    println!("  --install-dir <目錄>  指定原安裝位置");
    println!("  --no-path             不修改使用者 PATH");
    println!("  --purge               一併移除登入資料、Chrome profile、記錄與設定");
    println!("  --quiet               結束時不等待按鍵");
}

fn infer_install_dir() -> Option<PathBuf> {
    let current = env::current_exe().ok()?;
    let parent = current.parent()?;
    if parent.join("ask-bridge.exe").is_file() {
        Some(parent.to_path_buf())
    } else {
        None
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("無法解析安裝目錄：{error}"))
}

fn uninstall(options: &Options) -> Result<(), String> {
    println!("Ask Bridge 解除安裝程式");
    println!("安裝目錄：{}", options.install_dir.display());

    for name in ["ask.exe", "ask-bridge.exe", "ask-bridge-update.exe"] {
        let path = options.install_dir.join(name);
        match common::remove_file_if_present(&path) {
            Ok(true) => println!("已移除：{}", path.display()),
            Ok(false) => {}
            Err(error) => return Err(format!("無法移除 {}：{error}", path.display())),
        }
    }

    let installed_uninstaller = options.install_dir.join("uninstall.exe");
    let running_installed_copy = env::current_exe()
        .ok()
        .is_some_and(|current| paths_equal(&current, &installed_uninstaller));
    if running_installed_copy {
        schedule_self_delete(&installed_uninstaller)?;
        println!("已排程移除：{}", installed_uninstaller.display());
    } else {
        match common::remove_file_if_present(&installed_uninstaller) {
            Ok(true) => println!("已移除：{}", installed_uninstaller.display()),
            Ok(false) => {}
            Err(error) => {
                return Err(format!(
                    "無法移除 {}：{error}",
                    installed_uninstaller.display()
                ));
            }
        }
    }

    if options.update_path {
        match common::update_user_path(&options.install_dir, false) {
            Ok(true) => println!("已從使用者 PATH 移除安裝目錄。"),
            Ok(false) => println!("使用者 PATH 未包含安裝目錄。"),
            Err(error) => return Err(format!("無法更新使用者 PATH：{error}")),
        }
    } else {
        println!("依 --no-path 略過使用者 PATH 更新。")
    }

    if options.purge {
        let config_dir = common::default_config_dir()?;
        match fs::remove_dir_all(&config_dir) {
            Ok(()) => println!("已清除設定與 Chrome profile：{}", config_dir.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("無法清除 {}：{error}", config_dir.display())),
        }
    } else {
        println!("保留使用者設定與 Chrome profile；如需清除，請搭配 --purge。")
    }

    if !running_installed_copy {
        let _ = fs::remove_dir(&options.install_dir);
    }
    println!("解除安裝完成。");
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
}

fn schedule_self_delete(path: &Path) -> Result<(), String> {
    let command = format!(
        "ping 127.0.0.1 -n 2 >NUL & del /F /Q \"{}\" & rmdir \"{}\" 2>NUL",
        path.display(),
        path.parent().unwrap_or(Path::new(".")).display()
    );
    Command::new("cmd.exe")
        .args(["/D", "/S", "/C", &command])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("無法排程刪除解除安裝程式：{error}"))
}
