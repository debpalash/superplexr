use gpui::{Menu, MenuItem, OsAction};

use crate::{
    AboutTermi9ne, ActivateLastWorkspace, ActivateWorkspace1, ActivateWorkspace2,
    ActivateWorkspace3, ActivateWorkspace4, ActivateWorkspace5, ActivateWorkspace6,
    ActivateWorkspace7, ActivateWorkspace8, CloseActiveWorkspace, CopyTerminal, DecreaseAppZoom,
    HideApplication, HideOtherApplications, IncreaseAppZoom, MinimizeWindow, NewSession,
    NewTerminal, NewWorkspace, NextSession, NextWorkspace, OpenGraphInspector, OpenPluginManager,
    OpenProviderSettings, PasteTerminal, PreviousSession, PreviousWorkspace, QuitApplication,
    ReloadTheme, ResetAppZoom, RestoreClosedWorkspace, ShowAllApplications, ToggleCommandDeck,
    ToggleFocusMode, ToggleFullScreen, ToggleSidebar, ToggleWorkspacePin, UseGraphiteTheme,
    UsePaperTheme, ZoomWindow, theme::ThemeSelection,
};

pub(crate) fn native_menus(theme: &ThemeSelection) -> Vec<Menu> {
    vec![
        Menu::new("termi9ne").items([
            MenuItem::action("About termi9ne", AboutTermi9ne),
            MenuItem::separator(),
            #[cfg(target_os = "macos")]
            MenuItem::os_submenu("Services", gpui::SystemMenuType::Services),
            #[cfg(target_os = "macos")]
            MenuItem::separator(),
            #[cfg(target_os = "macos")]
            MenuItem::action("Hide termi9ne", HideApplication),
            #[cfg(target_os = "macos")]
            MenuItem::action("Hide Others", HideOtherApplications),
            #[cfg(target_os = "macos")]
            MenuItem::action("Show All", ShowAllApplications),
            #[cfg(target_os = "macos")]
            MenuItem::separator(),
            MenuItem::action("Quit termi9ne", QuitApplication),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Workspace", NewWorkspace),
            MenuItem::separator(),
            MenuItem::action("Close Workspace", CloseActiveWorkspace),
            MenuItem::action("Restore Closed Workspace", RestoreClosedWorkspace),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Copy", CopyTerminal, OsAction::Copy),
            MenuItem::os_action("Paste", PasteTerminal, OsAction::Paste),
        ]),
        Menu::new("View").items([
            MenuItem::action("Zoom In", IncreaseAppZoom),
            MenuItem::action("Zoom Out", DecreaseAppZoom),
            MenuItem::action("Actual Size", ResetAppZoom),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Focus Mode", ToggleFocusMode),
            MenuItem::action("Command Deck", ToggleCommandDeck),
            MenuItem::action("Graph Inspector", OpenGraphInspector),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new("Theme").items([
                    MenuItem::action("Graphite", UseGraphiteTheme)
                        .checked(theme.is_builtin("graphite")),
                    MenuItem::action("Paper", UsePaperTheme).checked(theme.is_builtin("paper")),
                    MenuItem::separator(),
                    MenuItem::action("Reload Theme File", ReloadTheme).disabled(!theme.is_file()),
                ]),
            ),
            MenuItem::separator(),
            MenuItem::action("Enter Full Screen", ToggleFullScreen),
        ]),
        Menu::new("Workspace").items([
            MenuItem::action("Next Workspace", NextWorkspace),
            MenuItem::action("Previous Workspace", PreviousWorkspace),
            MenuItem::action("Pin or Unpin Workspace", ToggleWorkspacePin),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Select Workspace").items([
                MenuItem::action("Workspace 1", ActivateWorkspace1),
                MenuItem::action("Workspace 2", ActivateWorkspace2),
                MenuItem::action("Workspace 3", ActivateWorkspace3),
                MenuItem::action("Workspace 4", ActivateWorkspace4),
                MenuItem::action("Workspace 5", ActivateWorkspace5),
                MenuItem::action("Workspace 6", ActivateWorkspace6),
                MenuItem::action("Workspace 7", ActivateWorkspace7),
                MenuItem::action("Workspace 8", ActivateWorkspace8),
                MenuItem::action("Last Workspace", ActivateLastWorkspace),
            ])),
        ]),
        Menu::new("Terminal").items([
            MenuItem::action("New Session", NewSession),
            MenuItem::action("New Terminal", NewTerminal),
            MenuItem::separator(),
            MenuItem::action("Next Session", NextSession),
            MenuItem::action("Previous Session", PreviousSession),
        ]),
        Menu::new("Tools").items([
            MenuItem::action("Plugins", OpenPluginManager),
            MenuItem::action("Agent Providers…", OpenProviderSettings),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", MinimizeWindow),
            MenuItem::action("Zoom", ZoomWindow),
        ]),
        Menu::new("Help").items([MenuItem::action("About termi9ne", AboutTermi9ne)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{OwnedMenuItem, SystemMenuType};

    #[test]
    fn native_menu_exposes_the_complete_application_hierarchy() {
        let menus = native_menus(&ThemeSelection::default())
            .into_iter()
            .map(Menu::owned)
            .collect::<Vec<_>>();
        let names = menus
            .iter()
            .map(|menu| menu.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            [
                "termi9ne",
                "File",
                "Edit",
                "View",
                "Workspace",
                "Terminal",
                "Tools",
                "Window",
                "Help"
            ]
        );

        let app_items = &menus[0].items;
        assert!(app_items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action { name, .. } if name == "Quit termi9ne"
        )));
        #[cfg(target_os = "macos")]
        assert!(app_items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::SystemMenu(menu) if menu.menu_type == SystemMenuType::Services
        )));

        let edit_items = &menus[2].items;
        assert!(edit_items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action {
                name,
                os_action: Some(OsAction::Copy),
                ..
            } if name == "Copy"
        )));
        assert!(edit_items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action {
                name,
                os_action: Some(OsAction::Paste),
                ..
            } if name == "Paste"
        )));

        let theme_menu = menus[3]
            .items
            .iter()
            .find_map(|item| match item {
                OwnedMenuItem::Submenu(menu) if menu.name == "Theme" => Some(menu),
                _ => None,
            })
            .expect("View should expose theme controls");
        for zoom_item in ["Zoom In", "Zoom Out", "Actual Size"] {
            assert!(menus[3].items.iter().any(|item| matches!(
                item,
                OwnedMenuItem::Action { name, .. } if name == zoom_item
            )));
        }
        assert!(theme_menu.items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action { name, checked: true, .. } if name == "Graphite"
        )));
        assert!(theme_menu.items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action { name, disabled: true, .. } if name == "Reload Theme File"
        )));
        assert!(menus[6].items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action { name, .. } if name == "Plugins"
        )));
        assert!(menus[6].items.iter().any(|item| matches!(
            item,
            OwnedMenuItem::Action { name, .. } if name == "Agent Providers…"
        )));
    }
}
