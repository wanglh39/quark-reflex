fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !std::path::Path::new("assets/icon.ico").exists() {
        println!(
            "cargo:warning=assets/icon.ico 不存在，跳过图标嵌入（先运行 cargo run --example gen-icon 生成）"
        );
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "夸克助手桌面小窗");
    res.set("ProductName", "Quark Reflex");
    res.set("LegalCopyright", "Copyright (c) 2026 MIT License");
    res.set("OriginalFilename", "quark-reflex.exe");
    res.compile().expect("嵌入 Windows 资源失败");
}