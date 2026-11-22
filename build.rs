// Встраивает иконку в exe (только при сборке на Windows). Если не вышло — сборка не падает, просто без иконки.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=иконка не встроена: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
