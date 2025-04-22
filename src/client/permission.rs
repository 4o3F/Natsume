pub fn check_permission() -> bool {
    #[cfg(target_os = "windows")]
    {
        true
    }
    #[cfg(not(target_os = "windows"))]
    {
        let euid = unsafe { libc::geteuid() };
        euid == 0
    }
}
