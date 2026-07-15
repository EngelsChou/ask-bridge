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

    println!("cargo:rerun-if-env-changed=ASK_BRIDGE_APP_VERSION");
    let version = std::env::var("ASK_BRIDGE_APP_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let numeric_version = numeric_version(&version);
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("CompanyName", "Engels Chou")
        .set("ProductName", "Ask Bridge")
        .set("FileDescription", "Ask Bridge offline Windows installer")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set("LegalCopyright", "Copyright (c) Engels Chou")
        .set_version_info(VersionInfo::FILEVERSION, numeric_version)
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version);
    resource
        .compile()
        .expect("failed to compile Ask Bridge installer Windows version resources");
}
