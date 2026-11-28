#![deny(unsafe_op_in_unsafe_fn)]

mod confinement;
pub use confinement::{Confinement, Mechanism, supports};

mod program;
pub use program::program;

#[cfg(target_os = "linux")]
mod landlock;

#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(unix)]
mod session;
#[cfg(unix)]
pub use session::isolate_session;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::restrict_process_group;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{CREATE_SUSPENDED, TerminalJob};

#[cfg(windows)]
pub mod service;
