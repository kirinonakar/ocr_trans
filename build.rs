fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("app.ico");
        res.compile().unwrap();
    }
    slint_build::compile("ui/main.slint").unwrap();
}
