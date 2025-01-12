use super::{KeyboardLayoutClient, KeyboardLayoutUpdate, Visibility, Workspace, WorkspaceUpdate};
use crate::{arc_mut, lock, send, spawn_blocking};
use color_eyre::Result;
use hyprland::ctl::switch_xkb_layout;
use hyprland::data::{Devices, Workspace as HWorkspace, Workspaces};
use hyprland::dispatch::{Dispatch, DispatchType, WorkspaceIdentifierWithSpecial};
use hyprland::event_listener::EventListener;
use hyprland::prelude::*;
use hyprland::shared::{HyprDataVec, WorkspaceType};
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tracing::{debug, error, info};

#[derive(Debug)]
pub struct Client {
    workspace_tx: Sender<WorkspaceUpdate>,
    _workspace_rx: Receiver<WorkspaceUpdate>,

    keyboard_layout_tx: Sender<KeyboardLayoutUpdate>,
    _keyboard_layout_rx: Receiver<KeyboardLayoutUpdate>,
}

impl Client {
    pub(crate) fn new() -> Self {
        let (workspace_tx, workspace_rx) = channel(16);
        let (keyboard_layout_tx, keyboard_layout_rx) = channel(16);

        let instance = Self {
            workspace_tx,
            _workspace_rx: workspace_rx,
            keyboard_layout_tx,
            _keyboard_layout_rx: keyboard_layout_rx,
        };

        instance.listen_workspace_events();
        instance
    }

    fn listen_workspace_events(&self) {
        info!("Starting Hyprland event listener");

        let tx = self.workspace_tx.clone();
        let keyboard_layout_tx = self.keyboard_layout_tx.clone();

        spawn_blocking(move || {
            let mut event_listener = EventListener::new();

            // we need a lock to ensure events don't run at the same time
            let lock = arc_mut!(());

            // cache the active workspace since Hyprland doesn't give us the prev active
            let active = Self::get_active_workspace().expect("Failed to get active workspace");
            let active = arc_mut!(Some(active));

            {
                let tx = tx.clone();
                let lock = lock.clone();
                let active = active.clone();

                event_listener.add_workspace_added_handler(move |event_data| {
                    let _lock = lock!(lock);
                    let workspace_type = event_data.name;
                    debug!("Added workspace: {workspace_type:?}");

                    let workspace_name = get_workspace_name(workspace_type);
                    let prev_workspace = lock!(active);

                    let workspace = Self::get_workspace(&workspace_name, prev_workspace.as_ref());

                    if let Some(workspace) = workspace {
                        send!(tx, WorkspaceUpdate::Add(workspace));
                    }
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();
                let active = active.clone();

                event_listener.add_workspace_changed_handler(move |event_data| {
                    let _lock = lock!(lock);

                    let mut prev_workspace = lock!(active);

                    let workspace_type = event_data.name;
                    debug!(
                        "Received workspace change: {:?} -> {workspace_type:?}",
                        prev_workspace.as_ref().map(|w| &w.id)
                    );

                    let workspace_name = get_workspace_name(workspace_type);
                    let workspace = Self::get_workspace(&workspace_name, prev_workspace.as_ref());

                    workspace.map_or_else(
                        || {
                            error!("Unable to locate workspace");
                        },
                        |workspace| {
                            // there may be another type of update so dispatch that regardless of focus change
                            if !workspace.visibility.is_focused() {
                                Self::send_focus_change(&mut prev_workspace, workspace, &tx);
                            }
                        },
                    );
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();
                let active = active.clone();

                event_listener.add_active_monitor_changed_handler(move |event_data| {
                    let _lock = lock!(lock);

                    let workspace_type = if let Some(name) = event_data.workspace_name {
                        name
                    } else {
                        error!(
                            "unable to locate workspace on monitor: {}",
                            event_data.monitor_name
                        );
                        return;
                    };

                    let mut prev_workspace = lock!(active);

                    debug!(
                        "Received active monitor change: {:?} -> {workspace_type:?}",
                        prev_workspace.as_ref().map(|w| &w.name)
                    );

                    let workspace_name = get_workspace_name(workspace_type);
                    let workspace = Self::get_workspace(&workspace_name, prev_workspace.as_ref());

                    if let Some((false, workspace)) =
                        workspace.map(|w| (w.visibility.is_focused(), w))
                    {
                        Self::send_focus_change(&mut prev_workspace, workspace, &tx);
                    } else {
                        error!("unable to locate workspace: {workspace_name}");
                    }
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();

                event_listener.add_workspace_moved_handler(move |event_data| {
                    let _lock = lock!(lock);
                    let workspace_type = event_data.name;
                    debug!("Received workspace move: {workspace_type:?}");

                    let mut prev_workspace = lock!(active);

                    let workspace_name = get_workspace_name(workspace_type);
                    let workspace = Self::get_workspace(&workspace_name, prev_workspace.as_ref());

                    if let Some(workspace) = workspace {
                        send!(tx, WorkspaceUpdate::Move(workspace.clone()));

                        if !workspace.visibility.is_focused() {
                            Self::send_focus_change(&mut prev_workspace, workspace, &tx);
                        }
                    }
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();

                event_listener.add_workspace_renamed_handler(move |event_data| {
                    let _lock = lock!(lock);
                    debug!("Received workspace rename: {event_data:?}");

                    send!(
                        tx,
                        WorkspaceUpdate::Rename {
                            id: event_data.id as i64,
                            name: event_data.name
                        }
                    );
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();

                event_listener.add_workspace_deleted_handler(move |event_data| {
                    let _lock = lock!(lock);
                    debug!("Received workspace destroy: {event_data:?}");
                    send!(tx, WorkspaceUpdate::Remove(event_data.id as i64));
                });
            }

            {
                let tx = tx.clone();
                let lock = lock.clone();

                event_listener.add_urgent_state_changed_handler(move |address| {
                    let _lock = lock!(lock);
                    debug!("Received urgent state: {address:?}");

                    let clients = match hyprland::data::Clients::get() {
                        Ok(clients) => clients,
                        Err(err) => {
                            error!("Failed to get clients: {err}");
                            return;
                        }
                    };
                    clients.iter().find(|c| c.address == address).map_or_else(
                        || {
                            error!("Unable to locate client");
                        },
                        |c| {
                            send!(
                                tx,
                                WorkspaceUpdate::Urgent {
                                    id: c.workspace.id as i64,
                                    urgent: true,
                                }
                            );
                        },
                    );
                });
            }

            {
                let tx = keyboard_layout_tx.clone();
                let lock = lock.clone();

                event_listener.add_layout_changed_handler(move |layout_event| {
                    let _lock = lock!(lock);

                    let layout = layout_event.layout_name;

                    debug!("Received layout: {layout:?}");

                    send!(tx, KeyboardLayoutUpdate(layout));
                });
            }

            event_listener
                .start_listener()
                .expect("Failed to start listener");
        });
    }

    /// Sends a `WorkspaceUpdate::Focus` event
    /// and updates the active workspace cache.
    fn send_focus_change(
        prev_workspace: &mut Option<Workspace>,
        workspace: Workspace,
        tx: &Sender<WorkspaceUpdate>,
    ) {
        send!(
            tx,
            WorkspaceUpdate::Focus {
                old: prev_workspace.take(),
                new: workspace.clone(),
            }
        );

        send!(
            tx,
            WorkspaceUpdate::Urgent {
                id: workspace.id,
                urgent: false,
            }
        );

        prev_workspace.replace(workspace);
    }

    /// Gets a workspace by name from the server, given the active workspace if known.
    fn get_workspace(name: &str, active: Option<&Workspace>) -> Option<Workspace> {
        Workspaces::get()
            .expect("Failed to get workspaces")
            .into_iter()
            .find_map(|w| {
                if w.name == name {
                    let vis = Visibility::from((&w, active.map(|w| w.name.as_ref()), &|w| {
                        create_is_visible()(w)
                    }));

                    Some(Workspace::from((vis, w)))
                } else {
                    None
                }
            })
    }

    /// Gets the active workspace from the server.
    fn get_active_workspace() -> Result<Workspace> {
        let w = HWorkspace::get_active().map(|w| Workspace::from((Visibility::focused(), w)))?;
        Ok(w)
    }
}

#[cfg(feature = "workspaces")]
impl super::WorkspaceClient for Client {
    fn focus(&self, id: i64) {
        let identifier = WorkspaceIdentifierWithSpecial::Id(id as i32);

        if let Err(e) = Dispatch::call(DispatchType::Workspace(identifier)) {
            error!("Couldn't focus workspace '{id}': {e:#}");
        }
    }

    fn subscribe(&self) -> Receiver<WorkspaceUpdate> {
        let rx = self.workspace_tx.subscribe();

        let active_id = HWorkspace::get_active().ok().map(|active| active.name);
        let is_visible = create_is_visible();

        let workspaces = Workspaces::get()
            .expect("Failed to get workspaces")
            .into_iter()
            .map(|w| {
                let vis = Visibility::from((&w, active_id.as_deref(), &is_visible));

                Workspace::from((vis, w))
            })
            .collect();

        send!(self.workspace_tx, WorkspaceUpdate::Init(workspaces));

        rx
    }
}

impl KeyboardLayoutClient for Client {
    fn set_next_active(&self) {
        let device = Devices::get()
            .expect("Failed to get devices")
            .keyboards
            .iter()
            .find(|k| k.main)
            .map(|k| k.name.clone());

        if let Some(device) = device {
            if let Err(e) =
                switch_xkb_layout::call(device, switch_xkb_layout::SwitchXKBLayoutCmdTypes::Next)
            {
                error!("Failed to switch keyboard layout due to Hyprland error: {e}");
            }
        } else {
            error!("Failed to get keyboard device from hyprland");
        }
    }

    fn subscribe(&self) -> Receiver<KeyboardLayoutUpdate> {
        let rx = self.keyboard_layout_tx.subscribe();

        let layout = Devices::get()
            .expect("Failed to get devices")
            .keyboards
            .iter()
            .find(|k| k.main)
            .map(|k| k.active_keymap.clone());

        if let Some(layout) = layout {
            send!(self.keyboard_layout_tx, KeyboardLayoutUpdate(layout));
        } else {
            error!("Failed to get current keyboard layout hyprland");
        }

        rx
    }
}

fn get_workspace_name(name: WorkspaceType) -> String {
    match name {
        WorkspaceType::Regular(name) => name,
        WorkspaceType::Special(name) => name.unwrap_or_default(),
    }
}

/// Creates a function which determines if a workspace is visible.
///
/// This function makes a Hyprland call that allocates so it should be cached when possible,
/// but it is only valid so long as workspaces do not change so it should not be stored long term
fn create_is_visible() -> impl Fn(&HWorkspace) -> bool {
    let monitors = hyprland::data::Monitors::get().map_or(Vec::new(), HyprDataVec::to_vec);

    move |w| monitors.iter().any(|m| m.active_workspace.id == w.id)
}

impl From<(Visibility, HWorkspace)> for Workspace {
    fn from((visibility, workspace): (Visibility, HWorkspace)) -> Self {
        Self {
            id: workspace.id as i64,
            name: workspace.name,
            monitor: workspace.monitor,
            visibility,
        }
    }
}

impl<'a, 'f, F> From<(&'a HWorkspace, Option<&str>, F)> for Visibility
where
    F: FnOnce(&'f HWorkspace) -> bool,
    'a: 'f,
{
    fn from((workspace, active_name, is_visible): (&'a HWorkspace, Option<&str>, F)) -> Self {
        if Some(workspace.name.as_str()) == active_name {
            Self::focused()
        } else if is_visible(workspace) {
            Self::visible()
        } else {
            Self::Hidden
        }
    }
}
