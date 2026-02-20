#[cfg(target_os = "illumos")]
mod smf;
#[cfg(target_os = "illumos")]
pub use smf::{install, log, status, uninstall};

#[cfg(not(target_os = "illumos"))]
mod systemd;
#[cfg(not(target_os = "illumos"))]
pub use systemd::{install, log, status, uninstall};
