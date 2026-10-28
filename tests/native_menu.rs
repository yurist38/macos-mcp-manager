// AppKit menu checks must run on the process main thread, not a test worker.
#[cfg(target_os = "macos")]
#[path = "../src/native.rs"]
#[allow(dead_code)]
mod native;

fn main() {
    #[cfg(target_os = "macos")]
    native::verify_menu_icons();
}
