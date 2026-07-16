use winresource::VersionInfo;

fn numeric_version(version: &str) -> u64 {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let major = u64::from(parts.next().unwrap_or(0));
    let minor = u64::from(parts.next().unwrap_or(0));
    let patch = u64::from(parts.next().unwrap_or(0));
    let build = u64::from(parts.next().unwrap_or(0));
    (major << 48) | (minor << 32) | (patch << 16) | build
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo package version is required");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("CompanyName", "Engels Chou")
        .set("ProductName", "Ask Bridge")
        .set(
            "FileDescription",
            "AI browser bridge for ChatGPT, Gemini, Claude and Microsoft 365 Copilot",
        )
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set_version_info(VersionInfo::FILEVERSION, numeric_version(&version))
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version(&version))
        .set("LegalCopyright", "Copyright (c) Engels Chou");
    resource
        .compile()
        .expect("failed to compile Ask Bridge Windows version resources");
}
