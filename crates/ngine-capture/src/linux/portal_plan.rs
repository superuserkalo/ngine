use crate::target::TargetKind;

pub(super) const CURSOR_HIDDEN: u32 = 1;
pub(super) const CURSOR_EMBEDDED: u32 = 2;
pub(super) const SOURCE_MONITOR: u32 = 1;
pub(super) const SOURCE_WINDOW: u32 = 2;

const FIRST_VERSION_WITH_CURSOR_MODES: u32 = 2;
const FIRST_VERSION_WITH_RESTORE_TOKENS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PortalAbilities {
    pub(super) version: u32,
    pub(super) cursor_modes: u32,
    pub(super) source_types: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub(super) cursor_mode: Option<u32>,
    pub(super) source_types: u32,
    pub(super) restores: bool,
}

pub(super) fn plan(abilities: PortalAbilities, show_cursor: bool) -> Selection {
    let wanted_cursor = if show_cursor {
        CURSOR_EMBEDDED
    } else {
        CURSOR_HIDDEN
    };
    let has_cursor_modes = abilities.version >= FIRST_VERSION_WITH_CURSOR_MODES;
    let cursor_mode =
        (has_cursor_modes && abilities.cursor_modes & wanted_cursor != 0).then_some(wanted_cursor);
    let wanted_sources = SOURCE_MONITOR | SOURCE_WINDOW;
    let source_types = match abilities.source_types & wanted_sources {
        0 if abilities.source_types == 0 => wanted_sources,
        0 => SOURCE_MONITOR,
        offered => offered,
    };
    Selection {
        cursor_mode,
        source_types,
        restores: abilities.version >= FIRST_VERSION_WITH_RESTORE_TOKENS,
    }
}

pub(super) fn kind_of(source_type: Option<u32>) -> TargetKind {
    match source_type {
        Some(SOURCE_MONITOR) => TargetKind::Screen,
        Some(SOURCE_WINDOW) => TargetKind::Window,
        _ => TargetKind::Picked,
    }
}

pub(super) fn label_of(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Screen => "Picked screen",
        TargetKind::Window => "Picked window",
        _ => "Picked source",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal(version: u32, cursor_modes: u32, source_types: u32) -> PortalAbilities {
        PortalAbilities {
            version,
            cursor_modes,
            source_types,
        }
    }

    #[test]
    fn a_current_portal_gets_the_asked_cursor_both_sources_and_a_restore_token() {
        let current = portal(5, 7, 7);

        assert_eq!(
            plan(current, true),
            Selection {
                cursor_mode: Some(CURSOR_EMBEDDED),
                source_types: SOURCE_MONITOR | SOURCE_WINDOW,
                restores: true,
            }
        );
        assert_eq!(plan(current, false).cursor_mode, Some(CURSOR_HIDDEN));
    }

    #[test]
    fn the_first_portal_version_has_no_cursor_choice_and_no_restore_token() {
        let first = plan(portal(1, 0, 1), false);

        assert_eq!(first.cursor_mode, None);
        assert!(!first.restores);
        assert_eq!(first.source_types, SOURCE_MONITOR);
    }

    #[test]
    fn a_cursor_mode_the_compositor_lacks_is_left_to_the_compositor() {
        let only_hidden_and_metadata = portal(4, 5, 3);

        assert_eq!(plan(only_hidden_and_metadata, true).cursor_mode, None);
        assert_eq!(
            plan(only_hidden_and_metadata, false).cursor_mode,
            Some(CURSOR_HIDDEN)
        );
    }

    #[test]
    fn restore_tokens_start_with_version_four() {
        assert!(!plan(portal(3, 7, 3), true).restores);
        assert!(plan(portal(4, 7, 3), true).restores);
    }

    #[test]
    fn sources_are_narrowed_to_what_the_compositor_offers() {
        assert_eq!(plan(portal(4, 7, 1), true).source_types, SOURCE_MONITOR);
        assert_eq!(plan(portal(4, 7, 2), true).source_types, SOURCE_WINDOW);
        assert_eq!(plan(portal(4, 7, 4), true).source_types, SOURCE_MONITOR);
        assert_eq!(
            plan(portal(4, 7, 0), true).source_types,
            SOURCE_MONITOR | SOURCE_WINDOW
        );
    }

    #[test]
    fn a_picked_stream_says_what_it_is_when_the_portal_tells() {
        assert_eq!(kind_of(Some(SOURCE_MONITOR)), TargetKind::Screen);
        assert_eq!(kind_of(Some(SOURCE_WINDOW)), TargetKind::Window);
        assert_eq!(kind_of(Some(4)), TargetKind::Picked);
        assert_eq!(kind_of(None), TargetKind::Picked);
        assert_eq!(label_of(TargetKind::Screen), "Picked screen");
        assert_eq!(label_of(TargetKind::Window), "Picked window");
        assert_eq!(label_of(TargetKind::Picked), "Picked source");
    }
}
