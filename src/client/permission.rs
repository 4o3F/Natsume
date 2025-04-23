use std::fs::OpenOptions;

pub fn check_permission(path: String) -> bool {
    // #[cfg(target_os = "windows")]
    // {
    //     true
    // }
    // #[cfg(not(target_os = "windows"))]
    // {
    //     let euid = unsafe { libc::geteuid() };
    //     if euid != 0 {

    //     }
    // }
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}
