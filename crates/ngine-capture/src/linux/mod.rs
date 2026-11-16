mod clock;
#[cfg(target_os = "linux")]
mod descriptor;
mod lifecycle;
mod nodes;
mod pixels;
mod portal_plan;
mod samples;
mod session_kind;

#[cfg(target_os = "linux")]
mod monotonic;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod pw;
#[cfg(target_os = "linux")]
mod surface;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use self::surface::{DmaBufPlane, DmaBufSurface};

#[cfg(target_os = "linux")]
static RESTART_DELAYS: [std::time::Duration; 4] = [
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(3),
];

#[cfg(target_os = "linux")]
pub(crate) fn system_backend()
-> Result<Box<dyn crate::backend::Backend>, crate::error::CaptureError> {
    use self::session_kind::{SessionEnvironment, SessionKind};

    let variable = |name: &str| std::env::var(name).ok();
    let session_type = variable("XDG_SESSION_TYPE");
    let wayland_display = variable("WAYLAND_DISPLAY");
    let x11_display = variable("DISPLAY");
    let environment = SessionEnvironment {
        session_type: session_type.as_deref(),
        wayland_display: wayland_display.as_deref(),
        x11_display: x11_display.as_deref(),
    };
    match session_kind::detect(&environment) {
        Some(SessionKind::Wayland) => Ok(Box::new(wayland::WaylandBackend)),
        Some(SessionKind::X11) => Ok(Box::new(x11::X11Backend)),
        None => Err(crate::error::CaptureError::Unavailable(
            "no Wayland or X11 session: WAYLAND_DISPLAY and DISPLAY are both unset".to_owned(),
        )),
    }
}
