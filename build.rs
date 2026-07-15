fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set("CompanyName", "Engels Chou")
        .set("ProductName", "Ask Bridge")
        .set(
            "FileDescription",
            "AI browser bridge for ChatGPT, Gemini, Claude and Microsoft 365 Copilot",
        )
        .set("LegalCopyright", "Copyright (c) Engels Chou");
    resource
        .compile()
        .expect("failed to compile Ask Bridge Windows version resources");
}
