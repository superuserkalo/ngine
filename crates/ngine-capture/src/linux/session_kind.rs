#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionKind {
    Wayland,
    X11,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SessionEnvironment<'a> {
    pub(super) session_type: Option<&'a str>,
    pub(super) wayland_display: Option<&'a str>,
    pub(super) x11_display: Option<&'a str>,
}

fn is_set(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

pub(super) fn detect(environment: &SessionEnvironment<'_>) -> Option<SessionKind> {
    let has_wayland = is_set(environment.wayland_display);
    let has_x11 = is_set(environment.x11_display);
    let declared = match environment.session_type {
        Some("wayland") if has_wayland => Some(SessionKind::Wayland),
        Some("x11") if has_x11 => Some(SessionKind::X11),
        _ => None,
    };
    let wayland = has_wayland.then_some(SessionKind::Wayland);
    let x11 = has_x11.then_some(SessionKind::X11);
    declared.or(wayland).or(x11)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment<'a>(
        session_type: Option<&'a str>,
        wayland_display: Option<&'a str>,
        x11_display: Option<&'a str>,
    ) -> SessionEnvironment<'a> {
        SessionEnvironment {
            session_type,
            wayland_display,
            x11_display,
        }
    }

    #[test]
    fn a_wayland_session_wins_over_the_x11_display_that_xwayland_sets() {
        let both = environment(Some("wayland"), Some("wayland-1"), Some(":0"));
        let undeclared = environment(None, Some("wayland-1"), Some(":0"));

        assert_eq!(detect(&both), Some(SessionKind::Wayland));
        assert_eq!(detect(&undeclared), Some(SessionKind::Wayland));
    }

    #[test]
    fn an_x11_session_is_x11_even_when_a_wayland_socket_is_named() {
        let declared = environment(Some("x11"), Some("wayland-1"), Some(":0"));
        let only_display = environment(None, None, Some(":1"));
        let over_ssh = environment(Some("tty"), None, Some("localhost:10.0"));

        assert_eq!(detect(&declared), Some(SessionKind::X11));
        assert_eq!(detect(&only_display), Some(SessionKind::X11));
        assert_eq!(detect(&over_ssh), Some(SessionKind::X11));
    }

    #[test]
    fn a_declared_session_without_its_display_falls_back_to_the_display_that_exists() {
        let wayland_without_socket = environment(Some("wayland"), None, Some(":0"));
        let x11_without_display = environment(Some("x11"), Some("wayland-0"), Some(""));

        assert_eq!(detect(&wayland_without_socket), Some(SessionKind::X11));
        assert_eq!(detect(&x11_without_display), Some(SessionKind::Wayland));
    }

    #[test]
    fn no_display_at_all_is_no_session() {
        assert_eq!(detect(&SessionEnvironment::default()), None);
        assert_eq!(detect(&environment(Some("wayland"), Some(""), None)), None);
    }
}
