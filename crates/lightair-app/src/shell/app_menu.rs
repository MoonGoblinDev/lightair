//! The menu bar (macOS's; on Windows the same actions are in the command palette and on
//! their shortcuts). Items use the window's actions, so each shows its shortcut.


use super::*;

actions!(
    lightair_menu,
    [
        About,
        Quit,
        HideApp,
        HideOthers,
        ShowAll,
        NewWorkspace,
        OpenWorkspace,
        AddExistingRecordings,
        ImportObsScenes,
        CloseWindow,
        Minimize,
        ZoomWindow,
        AddSceneMenu,
        AddImageObject,
        AddAvatarObject,
        AddVideoObject,
        AddTextObject,
        AddColourObject,
        AddTimerObject,
        AddScreenScenes,
        AddVerticalCanvas,
        ToggleRecord,
        PauseRecord,
        SplitRecord,
        StudioTakeMenu,
        GoLiveMenu,
        SaveClipMenu,
        KeyboardShortcuts
    ]
);

fn act(name: &str, action: impl Action) -> MenuItem {
    MenuItem::action(name.to_string(), action)
}

/// The menus, in order.
pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Lightair").items([
            act("About Lightair", About),
            MenuItem::separator(),
            act("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            act("Hide Lightair", HideApp),
            act("Hide Others", HideOthers),
            act("Show All", ShowAll),
            MenuItem::separator(),
            act("Quit Lightair", Quit),
        ]),
        Menu::new("File").items([
            act("New Workspace…", NewWorkspace),
            act("Open Workspace…", OpenWorkspace),
            MenuItem::separator(),
            act("Add Existing Recordings…", AddExistingRecordings),
            act("Import OBS Scenes…", ImportObsScenes),
            MenuItem::separator(),
            act("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", CutSelection, OsAction::Cut),
            MenuItem::os_action("Copy", CopySelection, OsAction::Copy),
            MenuItem::os_action("Paste", PasteObjects, OsAction::Paste),
            act("Duplicate", DuplicateSelection),
            act("Delete", DeleteSelection),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            act("Rename", RenameSelection),
            act("Group", GroupSelection),
            act("Ungroup", UngroupSelection),
        ]),
        Menu::new("View").items([
            act("Library", ShowLibrary),
            act("Live", ShowLive),
            act("Editor", ShowEditor),
            MenuItem::separator(),
            act("Zoom In", ZoomIn),
            act("Zoom Out", ZoomOut),
            act("Zoom to Fit", ZoomToFit),
            act("Actual Size", ZoomToActual),
            MenuItem::separator(),
            act("Command Palette…", OpenPalette),
        ]),
        Menu::new("Object").items([
            act("New Scene", AddSceneMenu),
            act("Screen Recording Scenes", AddScreenScenes),
            MenuItem::separator(),
            act("Add Image…", AddImageObject),
            act("Add Video…", AddVideoObject),
            act("Add Avatar…", AddAvatarObject),
            act("Add Text", AddTextObject),
            act("Add Colour", AddColourObject),
            act("Add Timer", AddTimerObject),
            act("New Empty", CreateEmpty),
            MenuItem::separator(),
            act("Bring to Front", BringToFront),
            act("Bring Forward", BringForward),
            act("Send Backward", SendBackward),
            act("Send to Back", SendToBack),
            MenuItem::separator(),
            act("Show or Hide", ToggleObjectVisible),
            act("Lock or Unlock", ToggleObjectLocked),
            act("Fill Canvas", FillCanvas),
            MenuItem::separator(),
            act("Add Vertical Canvas", AddVerticalCanvas),
        ]),
        Menu::new("Record").items([
            act("Record or Stop", ToggleRecord),
            act("Pause or Resume", PauseRecord),
            act("Add Marker", AddMarker),
            act("Continue in a New File", SplitRecord),
            MenuItem::separator(),
            act("Studio Take", StudioTakeMenu),
            MenuItem::separator(),
            act("Go Live…", GoLiveMenu),
            act("Save Clip", SaveClipMenu),
            MenuItem::separator(),
            act("Mute or Unmute Microphones", ToggleMics),
        ]),
        Menu::new("Window").items([act("Minimize", Minimize), act("Zoom", ZoomWindow)]),
        Menu::new("Help").items([act("Keyboard Shortcuts", KeyboardShortcuts)]),
    ]
}

/// What the app does whatever window is in front.
pub fn bind(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new(crate::ui::key("cmd-q"), Quit, None),
        KeyBinding::new(crate::ui::key("cmd-h"), HideApp, None),
        KeyBinding::new(crate::ui::key("cmd-alt-h"), HideOthers, None),
        KeyBinding::new(crate::ui::key("cmd-n"), NewWorkspace, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-o"), OpenWorkspace, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-w"), CloseWindow, Some("Studio")),
        KeyBinding::new(crate::ui::key("cmd-r"), ToggleRecord, Some("Studio")),
    ]);
    cx.set_menus(menus());
}

impl StudioWindow {
    /// The window's handlers for the menu's actions.
    pub(super) fn menu_actions(&self, el: Div, cx: &mut Context<Self>) -> Div {
        el.on_action(cx.listener(|this, _: &About, _, cx| this.open_settings(settings::Page::About, cx)))
            .on_action(cx.listener(|this, _: &KeyboardShortcuts, _, cx| this.open_settings(settings::Page::Shortcuts, cx)))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                this.show_place(Place::Library, cx);
                this.library.update(cx, |l, cx| l.start_new_workspace(window, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenWorkspace, _, cx| this.show_place(Place::Library, cx)))
            .on_action(cx.listener(|this, _: &AddExistingRecordings, window, cx| {
                this.show_place(Place::Library, cx);
                this.library.update(cx, |l, cx| l.add_existing(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ImportObsScenes, window, cx| this.import_obs(window, cx)))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &ZoomWindow, window, _| window.zoom_window()))
            .on_action(cx.listener(|this, _: &AddSceneMenu, _, cx| this.add_scene(cx)))
            .on_action(cx.listener(|this, _: &AddImageObject, window, cx| this.add_image(window, cx)))
            .on_action(cx.listener(|this, _: &AddAvatarObject, window, cx| this.add_avatar(window, cx)))
            .on_action(cx.listener(|this, _: &AddVideoObject, window, cx| this.add_video(window, cx)))
            .on_action(cx.listener(|this, _: &AddTextObject, _, cx| this.add_text(cx)))
            .on_action(cx.listener(|this, _: &AddColourObject, _, cx| this.add_colour(cx)))
            .on_action(cx.listener(|this, _: &AddTimerObject, _, cx| this.add_timer(cx)))
            .on_action(cx.listener(|this, _: &AddScreenScenes, _, cx| this.add_screen_layouts(cx)))
            .on_action(cx.listener(|this, _: &AddVerticalCanvas, _, cx| this.add_vertical(cx)))
            .on_action(cx.listener(|this, _: &ToggleRecord, _, cx| this.record_click(cx)))
            .on_action(cx.listener(|this, _: &PauseRecord, _, cx| this.studio.update(cx, |s, cx| s.pause_recording(cx))))
            .on_action(cx.listener(|this, _: &SplitRecord, _, cx| this.studio.update(cx, |s, cx| s.split_recording(cx))))
            .on_action(cx.listener(|this, _: &StudioTakeMenu, _, cx| this.studio_take(cx)))
            .on_action(cx.listener(|this, _: &GoLiveMenu, _, cx| this.open_go_live(cx)))
            .on_action(cx.listener(|this, _: &SaveClipMenu, _, cx| this.studio.update(cx, |s, cx| s.save_clip(cx))))
    }
}
