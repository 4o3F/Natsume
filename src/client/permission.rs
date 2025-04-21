
pub fn check_suid() -> bool {
    let euid = unsafe { libc::geteuid() };
    euid == 0
}
