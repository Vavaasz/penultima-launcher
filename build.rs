fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon_with_id("assets/penultima-phoenix.ico", "1");
        res.set_manifest_file("build/launcher.manifest");
        res.set("CompanyName", "Penultima");
        res.set("InternalName", "penultima-launcher");
        res.set("FileDescription", "Penultima Launcher");
        res.set("ProductName", "Penultima Launcher");
        res.set("OriginalFilename", "penultima-launcher.exe");
        if let Err(e) = res.compile() {
            eprintln!("Error compiling resources: {}", e);
        }
    }
}
