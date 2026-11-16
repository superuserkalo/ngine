use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use ashpd::zbus::block_on;
use ashpd::{PortalError, WindowIdentifier};

use super::portal_plan::{self, PortalAbilities};
use crate::error::{CaptureError, Permission};
use crate::target::ParentWindow;

pub(super) type PortalSession = Session<Screencast>;

pub(super) struct Live {
    pub(super) session: PortalSession,
    pub(super) remote: OwnedFd,
    pub(super) node_id: u32,
    pub(super) source_type: Option<u32>,
    pub(super) shows_cursor: bool,
}

pub(super) struct Opened {
    pub(super) live: Live,
    pub(super) restore_token: Option<String>,
}

fn identifier_of(parent: &ParentWindow) -> Option<WindowIdentifier> {
    match parent {
        ParentWindow::Wayland(handle) => {
            Some(WindowIdentifier::from_xdg_foreign_exported(handle.clone()))
        }
        ParentWindow::X11(window) => Some(WindowIdentifier::from_xid((*window).into())),
        ParentWindow::Win32(_) => None,
    }
}

async fn negotiate(
    show_cursor: bool,
    restore_token: Option<&str>,
    parent: Option<&ParentWindow>,
) -> Result<Opened, ashpd::Error> {
    let portal = Screencast::new().await?;
    let abilities = PortalAbilities {
        version: portal.version(),
        cursor_modes: portal
            .available_cursor_modes()
            .await
            .map_or(0, |modes| modes.bits()),
        source_types: portal
            .available_source_types()
            .await
            .map_or(0, |types| types.bits()),
    };
    let selection = portal_plan::plan(abilities, show_cursor);
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let mut sources = SelectSourcesOptions::default()
        .set_sources(SourceType::from_bits_truncate(selection.source_types))
        .set_multiple(false)
        .set_cursor_mode(selection.cursor_mode.map(CursorMode::from_bits_truncate));
    if selection.restores {
        sources = sources
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(restore_token);
    }
    portal.select_sources(&session, sources).await?.response()?;
    let identifier = parent.and_then(identifier_of);
    let started = portal
        .start(&session, identifier.as_ref(), StartCastOptions::default())
        .await?
        .response()?;
    let stream = started.streams().first().ok_or(ashpd::Error::NoResponse)?;
    let node_id = stream.pipe_wire_node_id();
    let source_type = stream.source_type().map(|kind| kind.bits());
    let restore_token = started.restore_token().map(str::to_owned);
    let remote = portal
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await?;
    Ok(Opened {
        live: Live {
            session,
            remote,
            node_id,
            source_type,
            shows_cursor: show_cursor,
        },
        restore_token,
    })
}

fn unreachable_portal(error: &dyn std::fmt::Display) -> CaptureError {
    CaptureError::Unavailable(format!(
        "the desktop portal could not be reached over D-Bus: {error}"
    ))
}

fn refusal(error: ashpd::Error) -> Result<Option<Opened>, CaptureError> {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled)
        | ashpd::Error::Portal(PortalError::Cancelled(_)) => Ok(None),
        ashpd::Error::Portal(PortalError::NotAllowed(_)) => {
            Err(CaptureError::PermissionDenied(Permission::ScreenRecording))
        }
        ashpd::Error::PortalNotFound(_) => Err(CaptureError::Unavailable(
            "this desktop has no screen-capture portal: install xdg-desktop-portal and the backend for the compositor"
                .to_owned(),
        )),
        ashpd::Error::Zbus(error) | ashpd::Error::Portal(PortalError::ZBus(error)) => {
            Err(unreachable_portal(&error))
        }
        ashpd::Error::RequiresVersion(needed, found) => Err(CaptureError::Unsupported(format!(
            "the screen-capture portal is version {found} and version {needed} is needed"
        ))),
        other => Err(CaptureError::Os(other.to_string())),
    }
}

pub(super) fn open_unless_cancelled(
    show_cursor: bool,
    restore_token: Option<&str>,
    parent: Option<&ParentWindow>,
) -> Result<Option<Opened>, CaptureError> {
    block_on(negotiate(show_cursor, restore_token, parent))
        .map_or_else(refusal, |opened| Ok(Some(opened)))
}

pub(super) fn close(session: &PortalSession) {
    let _already_closed = block_on(session.close());
}
