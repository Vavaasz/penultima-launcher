fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon_with_id("assets/ultima-logo.ico", "1");
        res.set_manifest_file("build/launcher.manifest");
        if let Err(e) = res.compile() {
            eprintln!("Error compiling resources: {}", e);
        }
    }
}
