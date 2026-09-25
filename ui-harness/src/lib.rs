slint::include_modules!();

#[cfg(test)]
#[path = "../../src/emoji.rs"]
mod emoji;

#[cfg(test)]
#[path = "../../src/highlight.rs"]
mod highlight;

#[cfg(test)]
#[path = "../../src/background.rs"]
mod background;

#[cfg(test)]
#[path = "../../src/appearance.rs"]
#[allow(dead_code)]
mod appearance;

#[cfg(test)]
#[path = "../../src/editor_view.rs"]
mod editor_view;

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_core::input::{InternalKeyEvent, KeyEvent as InternalKeyEventData, KeyEventType};
    use i_slint_core::window::WindowInner;
    use slint::platform::software_renderer::{
        MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
    };
    use slint::platform::{Clipboard, Key, Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
    use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, PhysicalSize, VecModel};
    use std::cell::RefCell;
    use std::fmt::Write as _;
    use std::path::Path;
    use std::rc::Rc;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct TestPixel {
        red: u8,
        green: u8,
        blue: u8,
    }

    impl TargetPixel for TestPixel {
        fn blend(&mut self, color: PremultipliedRgbaColor) {
            let inverse_alpha = 255u32 - color.alpha as u32;
            self.red = (color.red as u32 + self.red as u32 * inverse_alpha / 255) as u8;
            self.green = (color.green as u32 + self.green as u32 * inverse_alpha / 255) as u8;
            self.blue = (color.blue as u32 + self.blue as u32 * inverse_alpha / 255) as u8;
        }

        fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
            Self { red, green, blue }
        }
    }

    struct TestPlatform {
        window: Rc<MinimalSoftwareWindow>,
        clipboard: Rc<RefCell<String>>,
    }

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            Ok(self.window.clone())
        }

        fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
            if clipboard == Clipboard::DefaultClipboard {
                *self.clipboard.borrow_mut() = text.to_string();
            }
        }

        fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
            (clipboard == Clipboard::DefaultClipboard).then(|| self.clipboard.borrow().clone())
        }
    }

    #[test]
    fn mouse_selection_is_visible_to_the_editor_and_control_c_copies_it() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(1200, 800));
        let clipboard = Rc::new(RefCell::new(String::new()));
        slint::platform::set_platform(Box::new(TestPlatform {
            window: window.clone(),
            clipboard: clipboard.clone(),
        }))
        .expect("test platform must be installed once");

        let ui = AppWindow::new().unwrap();
        let branded_window = render(&window);
        let title_logo_pixels = branded_window
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                x < 250
                    && y < 34
                    && pixel.red > 180
                    && pixel.green > 180
                    && pixel.blue > 180
            })
            .count();
        let title_accent_pixels = branded_window
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                x < 250
                    && y < 34
                    && pixel.blue > pixel.red.saturating_add(25)
                    && pixel.blue > pixel.green.saturating_add(15)
                    && pixel.blue > 100
            })
            .count();
        assert!(
            title_logo_pixels > 20 && title_accent_pixels > 2,
            "new Araseo icon is not visible in the title bar (light={title_logo_pixels}, accent={title_accent_pixels})"
        );
        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 0,
            title: "sample.js".into(),
            detail: "/workspace/sample.js".into(),
            kind: "file".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_tree_entries(ModelRc::new(VecModel::from(vec![TreeEntry {
            name: "local-project".into(),
            path: "/workspace/local-project".into(),
            icon: emoji::EmojiIcons::load_system().get("🧩"),
            icon_label: "🧩".into(),
            depth: 0,
            is_directory: true,
            is_expanded: false,
            git_mark: "".into(),
            project_kind: "local".into(),
            is_added_root: false,
        }])));
        ui.set_git_change_count(1);
        ui.set_git_entries(ModelRc::new(VecModel::from(vec![GitEntry {
            kind: "repository".into(),
            name: "local-project".into(),
            detail: "/workspace/local-project".into(),
            path: "/workspace/local-project".into(),
            status: "".into(),
            count: 1,
            icon: slint::Image::default(),
            expanded: false,
        }])));
        ui.set_primary_active_tab_id(0);
        ui.set_primary_active_kind("file".into());
        ui.set_primary_active_title("sample.js".into());
        ui.set_primary_active_detail("/workspace/sample.js".into());
        ui.set_editor_text("const selectedText = copyThisValue;\n".into());
        ui.set_syntax_highlight_enabled(true);
        ui.set_highlighted_text(
            slint::StyledText::from_markdown(
                "<font color=\"#61afef\">const</font> selectedText = copyThisValue;\n",
            )
            .unwrap(),
        );

        let selection = Rc::new(RefCell::new((0, 0)));
        let observed_selection = selection.clone();
        ui.on_editor_selection_changed(move |anchor, cursor| {
            *observed_selection.borrow_mut() = (anchor, cursor);
        });
        let activated_tab = Rc::new(RefCell::new(None));
        let observed_tab = activated_tab.clone();
        ui.on_tab_activated(move |index| {
            *observed_tab.borrow_mut() = Some(index);
        });
        let closed_tabs = Rc::new(RefCell::new(Vec::new()));
        let observed_closed_tabs = closed_tabs.clone();
        ui.on_tab_close(move |tab_id| {
            observed_closed_tabs.borrow_mut().push(tab_id);
        });
        let terminal_groups = Rc::new(RefCell::new(Vec::new()));
        let observed_terminal_groups = terminal_groups.clone();
        ui.on_new_terminal_requested(move |group| {
            observed_terminal_groups.borrow_mut().push(group);
        });
        let terminal_text = Rc::new(RefCell::new(Vec::new()));
        let observed_terminal_text = terminal_text.clone();
        ui.on_terminal_text(move |tab_id, text| {
            observed_terminal_text
                .borrow_mut()
                .push((tab_id, text.to_string()));
        });
        let terminal_keys = Rc::new(RefCell::new(Vec::new()));
        let observed_terminal_keys = terminal_keys.clone();
        ui.on_terminal_key(move |tab_id, text, control, alt, shift| {
            observed_terminal_keys.borrow_mut().push((
                tab_id,
                text.to_string(),
                control,
                alt,
                shift,
            ));
        });
        let terminal_scrolls = Rc::new(RefCell::new(Vec::new()));
        let observed_terminal_scrolls = terminal_scrolls.clone();
        ui.on_terminal_scrollback(move |tab_id, rows| {
            observed_terminal_scrolls.borrow_mut().push((tab_id, rows));
        });
        let terminal_copy_requests = Rc::new(RefCell::new(Vec::new()));
        let observed_terminal_copy_requests = terminal_copy_requests.clone();
        ui.on_terminal_copy_requested(
            move |tab_id, anchor_row, anchor_column, cursor_row, cursor_column| {
                observed_terminal_copy_requests.borrow_mut().push((
                    tab_id,
                    anchor_row,
                    anchor_column,
                    cursor_row,
                    cursor_column,
                ));
                "Codex response".into()
            },
        );
        let tree_creates = Rc::new(RefCell::new(Vec::new()));
        let observed_tree_creates = tree_creates.clone();
        ui.on_tree_create_requested(move |target, name, is_directory| {
            observed_tree_creates.borrow_mut().push((
                target.to_string(),
                name.to_string(),
                is_directory,
            ));
        });
        let added_folder = Rc::new(RefCell::new(String::new()));
        let observed_added_folder = added_folder.clone();
        ui.on_add_folder_requested(move |path| {
            *observed_added_folder.borrow_mut() = path.to_string();
        });
        let browse_requests = Rc::new(RefCell::new(0usize));
        let observed_browse_requests = browse_requests.clone();
        ui.on_browse_folder_requested(move || {
            *observed_browse_requests.borrow_mut() += 1;
        });
        let terminal_folder = Rc::new(RefCell::new(String::new()));
        let observed_terminal_folder = terminal_folder.clone();
        ui.on_tree_terminal_requested(move |path| {
            *observed_terminal_folder.borrow_mut() = path.to_string();
        });
        let tree_renames = Rc::new(RefCell::new(Vec::new()));
        let observed_tree_renames = tree_renames.clone();
        ui.on_tree_rename_requested(move |path, name| {
            observed_tree_renames
                .borrow_mut()
                .push((path.to_string(), name.to_string()));
        });
        let tree_deletes = Rc::new(RefCell::new(Vec::new()));
        let observed_tree_deletes = tree_deletes.clone();
        ui.on_tree_delete_requested(move |path| {
            observed_tree_deletes.borrow_mut().push(path.to_string());
        });
        let copied_tree_paths = Rc::new(RefCell::new(Vec::new()));
        let observed_tree_path_copies = copied_tree_paths.clone();
        ui.on_tree_path_copied(move |path| {
            observed_tree_path_copies.borrow_mut().push(path.to_string());
        });
        let opened_git_changes = Rc::new(RefCell::new(Vec::new()));
        let observed_git_changes = opened_git_changes.clone();
        ui.on_git_change_activated(move |path| {
            observed_git_changes.borrow_mut().push(path.to_string());
        });
        let toggled_git_repositories = Rc::new(RefCell::new(Vec::new()));
        let observed_repository_toggles = toggled_git_repositories.clone();
        ui.on_git_repository_toggled(move |path| {
            observed_repository_toggles.borrow_mut().push(path.to_string());
        });
        let discarded_git_changes = Rc::new(RefCell::new(Vec::new()));
        let observed_discards = discarded_git_changes.clone();
        ui.on_git_discard_requested(move |path| {
            observed_discards.borrow_mut().push(path.to_string());
        });
        let docked_tabs = Rc::new(RefCell::new(Vec::new()));
        let observed_docked_tabs = docked_tabs.clone();
        ui.on_tab_dock_requested(move |tab_id, zone| {
            observed_docked_tabs.borrow_mut().push((tab_id, zone));
        });
        let cycled_tabs = Rc::new(RefCell::new(Vec::new()));
        let observed_cycles = cycled_tabs.clone();
        ui.on_tab_cycle(move |delta| {
            observed_cycles.borrow_mut().push(delta);
        });
        ui.show().unwrap();
        let populated_ui = render(&window);
        assert_eq!(
            populated_ui[300 * 1200 + 200],
            TestPixel::from_rgb(0x17, 0x1e, 0x28),
            "the explorer lost its distinct dark surface"
        );
        assert_eq!(
            populated_ui[300 * 1200 + 600],
            TestPixel::from_rgb(0x10, 0x15, 0x1d),
            "the editor lost its low-glare canvas"
        );
        write_snapshot_if_requested("single-editor.png", &populated_ui);

        assert!(
            ui.get_primary_group_visible() && !ui.get_secondary_group_visible(),
            "a single tab unexpectedly reserved space for an empty second pane"
        );
        assert!(
            ui.get_primary_group_width() > 900.0 && ui.get_primary_group_height() > 700.0,
            "the initial pane did not fill the workspace"
        );
        assert!(
            ui.get_primary_editor_surface_width() > 850.0
                && ui.get_primary_editor_surface_height() > 650.0,
            "the editor surface used only part of the initial pane: {} x {}",
            ui.get_primary_editor_surface_width(),
            ui.get_primary_editor_surface_height()
        );

        let window_drags = Rc::new(RefCell::new(0));
        let observed_drags = window_drags.clone();
        ui.on_window_drag_requested(move || *observed_drags.borrow_mut() += 1);
        dispatch_pointer(&ui, WindowEvent::PointerMoved {
            position: LogicalPosition::new(800.0, 17.0),
        });
        assert_eq!(*window_drags.borrow(), 0, "hover must not drag the window");
        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(800.0, 17.0),
            button: PointerEventButton::Left,
        });
        dispatch_pointer(&ui, WindowEvent::PointerMoved {
            position: LogicalPosition::new(820.0, 17.0),
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(820.0, 17.0),
            button: PointerEventButton::Left,
        });
        assert!(*window_drags.borrow() > 0, "blank tab space must drag the window");

        // Tabs share the top row with the fixed window controls.
        // The file tree must remain a separate sidebar and its divider must
        // resize the tree without overlapping the editor.
        dispatch_click(&ui, 150.0, 51.0);
        assert_eq!(ui.get_primary_group_y(), 0.0);
        assert!(ui.get_primary_editor_surface_height() > 700.0);
        assert_eq!(
            *activated_tab.borrow(),
            None,
            "a file tab overlaps the FILES sidebar"
        );

        let initial_sidebar_width = ui.get_sidebar_width();
        let initial_workspace_width = ui.get_primary_group_width();
        assert_eq!(ui.get_file_tree_x(), 0.0);
        assert_eq!(ui.get_file_tree_y(), 66.0);
        assert_eq!(ui.get_file_tree_width(), initial_sidebar_width);
        assert_eq!(ui.get_file_tree_height(), 710.0);
        assert!(ui.get_file_tree_row_height() >= 25.0);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(initial_sidebar_width + 2.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(370.0, 400.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(370.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        render(&window);
        assert!(
            ui.get_sidebar_width() > initial_sidebar_width + 100.0,
            "dragging the file-tree divider did not widen the sidebar"
        );
        assert!(
            ui.get_primary_group_width() < initial_workspace_width - 100.0,
            "resizing the file tree did not give the remaining width to the workspace"
        );

        let widened_sidebar_width = ui.get_sidebar_width();
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(widened_sidebar_width + 2.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(40.0, 400.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(40.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            ui.get_sidebar_width(),
            170.0,
            "the file-tree divider ignored its minimum width"
        );
        ui.set_sidebar_width(250.0);
        render(&window);
        ui.set_sidebar_view(0);

        assert_eq!(
            ui.get_tree_entries().row_data(0).unwrap().icon_label.as_str(),
            "🧩",
            "context-aware project emoji was not delivered to the tree UI"
        );
        let colored_icon_pixels = populated_ui
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                (8..32).contains(&x)
                    && (64..90).contains(&y)
                    && !(pixel.red == pixel.green && pixel.green == pixel.blue)
                    && (pixel.red > 60 || pixel.green > 60 || pixel.blue > 60)
            })
            .count();
        assert!(
            colored_icon_pixels > 5,
            "the explorer's icon badge was not rendered with its accent color"
        );

        dispatch_click(&ui, 60.0, 51.0);
        assert!(
            ui.get_tree_create_visible() && !ui.get_tree_create_is_directory(),
            "the FILES toolbar did not open the new-file dialog"
        );
        let create_dialog = render(&window);
        let input_surface = create_dialog[399 * 1200 + 600];
        assert!(
            input_surface.red < 90 && input_surface.green < 90 && input_surface.blue < 110,
            "the create dialog input returned to the bright system style"
        );
        for (label, left, right) in [("Cancel", 636, 710), ("Create", 718, 792)] {
            let caption_pixels = (445..462)
                .flat_map(|y| (left + 12..right - 12).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let pixel = create_dialog[y * 1200 + x];
                    pixel.red > 150 && pixel.green > 150 && pixel.blue > 150
                })
                .count();
            let clipped_leading_pixels = (445..462)
                .flat_map(|y| (left + 1..left + 10).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let pixel = create_dialog[y * 1200 + x];
                    pixel.red > 150 && pixel.green > 150 && pixel.blue > 150
                })
                .count();
            assert!(
                caption_pixels > 10 && clipped_leading_pixels == 0,
                "the {label} caption is missing or clipped against its button's leading edge"
            );
        }
        write_snapshot_if_requested("new-file-dialog.png", &create_dialog);
        ui.set_tree_create_name("new.rs".into());
        render(&window);
        dispatch_click(&ui, 760.0, 451.0);
        assert_eq!(
            tree_creates.borrow().last(),
            Some(&("".to_string(), "new.rs".to_string(), false)),
            "the root file creation request was not forwarded"
        );

        ui.set_folder_browser_available(true);
        render(&window);
        dispatch_click(&ui, 108.0, 51.0);
        assert!(ui.get_add_folder_visible(), "the FILES toolbar did not open Add Folder");
        write_snapshot_if_requested("add-folder-dialog.png", &render(&window));
        dispatch_click(&ui, 778.0, 416.0);
        assert_eq!(*browse_requests.borrow(), 1, "Browse did not request the native picker");
        ui.set_add_folder_path("/home/user/other-project".into());
        render(&window);
        dispatch_click(&ui, 794.0, 454.0);
        assert_eq!(added_folder.borrow().as_str(), "/home/user/other-project");

        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        assert!(ui.get_tree_context_visible(), "right-click did not open the file-tree menu");
        write_snapshot_if_requested("file-tree-context-menu.png", &render(&window));
        dispatch_click(&ui, 100.0, 211.0);
        assert_eq!(clipboard.borrow().as_str(), "/workspace/local-project");
        assert_eq!(
            copied_tree_paths.borrow().last().map(String::as_str),
            Some("/workspace/local-project"),
            "Copy Path did not copy the selected Linux path"
        );

        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_click(&ui, 100.0, 151.0);
        assert!(
            ui.get_tree_create_visible() && ui.get_tree_create_is_directory(),
            "New Folder did not open a folder creation dialog"
        );
        assert_eq!(ui.get_tree_create_target().as_str(), "/workspace/local-project");
        ui.set_tree_create_visible(false);

        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_click(&ui, 100.0, 181.0);
        assert!(ui.get_tree_rename_visible(), "Rename did not open the dialog");
        assert!(ui.get_tree_rename_is_directory());
        assert_eq!(ui.get_tree_rename_name().as_str(), "local-project");
        ui.set_tree_rename_name("renamed-project".into());
        render(&window);
        dispatch_click(&ui, 760.0, 451.0);
        assert_eq!(
            tree_renames.borrow().last(),
            Some(&("/workspace/local-project".to_string(), "renamed-project".to_string())),
            "folder rename request was not forwarded"
        );

        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_click(&ui, 100.0, 241.0);
        assert!(ui.get_tree_delete_visible(), "Delete did not require confirmation");
        dispatch_click(&ui, 748.0, 451.0);
        assert_eq!(
            tree_deletes.borrow().last().map(String::as_str),
            Some("/workspace/local-project"),
            "confirmed file-tree deletion was not forwarded"
        );

        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 76.0),
            button: PointerEventButton::Right,
        });
        dispatch_click(&ui, 100.0, 271.0);
        assert_eq!(terminal_folder.borrow().as_str(), "/workspace/local-project");

        ui.set_tree_entries(ModelRc::new(VecModel::from(vec![
            ui.get_tree_entries().row_data(0).unwrap(),
            TreeEntry {
                name: "sample.js".into(),
                path: "/workspace/sample.js".into(),
                icon: slint::Image::default(),
                icon_label: "".into(),
                depth: 0,
                is_directory: false,
                is_expanded: false,
                git_mark: "".into(),
                project_kind: "".into(),
                is_added_root: false,
            },
        ])));
        render(&window);
        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(60.0, 101.0),
            button: PointerEventButton::Right,
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(60.0, 101.0),
            button: PointerEventButton::Right,
        });
        dispatch_click(&ui, 100.0, 206.0);
        assert!(ui.get_tree_rename_visible() && !ui.get_tree_rename_is_directory());
        assert_eq!(ui.get_tree_rename_name().as_str(), "sample.js");
        ui.set_tree_rename_name("renamed.js".into());
        render(&window);
        dispatch_click(&ui, 760.0, 451.0);
        assert_eq!(
            tree_renames.borrow().last(),
            Some(&("/workspace/sample.js".to_string(), "renamed.js".to_string())),
            "file rename request was not forwarded"
        );

        let before_close_hover = render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(370.0, 17.0),
            },
        );
        let after_close_hover = render(&window);
        let changed_close_pixels = before_close_hover
            .iter()
            .zip(&after_close_hover)
            .enumerate()
            .filter(|(index, (before, after))| {
                let x = index % 1200;
                let y = index / 1200;
                (350..390).contains(&x) && (0..36).contains(&y) && before != after
            })
            .count();
        assert!(
            changed_close_pixels > 20,
            "tab close button has no visible hover highlight"
        );

        let many_tabs = (0..16)
            .map(|index| TabEntry {
                id: index,
                title: format!("long-open-file-{index}.rs").into(),
                detail: format!("/workspace/long-open-file-{index}.rs").into(),
                kind: "file".into(),
                group: 0,
                active: index == 15,
                dirty: index == 4,
            })
            .collect::<Vec<_>>();
        ui.set_primary_tabs(ModelRc::new(VecModel::from(many_tabs)));
        ui.set_primary_active_tab_id(15);
        let before_window_control_hover = render(&window);
        assert!(
            ui.get_primary_tab_scroll_offset() < 0.0,
            "the newest active tab was not scrolled into view"
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(1132.0, 17.0),
            },
        );
        let after_window_control_hover = render(&window);
        let changed_maximize_pixels = before_window_control_hover
            .iter()
            .zip(&after_window_control_hover)
            .enumerate()
            .filter(|(index, (before, after))| {
                let x = index % 1200;
                let y = index / 1200;
                (1108..1155).contains(&x) && y < 34 && before != after
            })
            .count();
        assert!(
            changed_maximize_pixels > 100,
            "file tabs pushed the fixed maximize control out of the visible window"
        );
        dispatch_click(&ui, 1132.0, 17.0);
        let maximized_controls = render(&window);
        assert!(after_window_control_hover.iter().zip(&maximized_controls)
            .enumerate().any(|(index, (before, after))|
                (1108..1155).contains(&(index % 1200)) && index / 1200 < 34 && before != after),
            "overflowing tabs intercepted the maximize control");
        dispatch_click(&ui, 1132.0, 17.0);
        ui.set_primary_active_tab_id(0);
        render(&window);
        assert!(
            ui.get_primary_tab_scroll_offset().abs() < 0.1,
            "activating the first tab did not scroll it back into view"
        );

        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(500.0, 17.0),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        assert!(
            ui.get_primary_tab_scroll_offset() < 0.0,
            "mouse wheel over the tab strip did not scroll hidden tabs into reach"
        );

        dispatch_click(&ui, 1042.0, 17.0);
        assert_eq!(
            terminal_groups.borrow().as_slice(),
            &[0],
            "the primary pane terminal button did not request a terminal in its group"
        );

        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 50,
            title: "main.rs".into(),
            detail: "/workspace/src/main.rs".into(),
            kind: "file".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_secondary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 51,
            title: "Terminal 1 · agent_ide".into(),
            detail: "/home/minch/agent_ide".into(),
            kind: "terminal".into(),
            group: 1,
            active: true,
            dirty: false,
        }])));
        ui.set_primary_active_tab_id(50);
        ui.set_primary_active_kind("file".into());
        ui.set_primary_active_title("main.rs".into());
        ui.set_secondary_active_tab_id(51);
        ui.set_secondary_active_kind("terminal".into());
        ui.set_secondary_active_title("Terminal 1 · agent_ide".into());
        ui.set_secondary_active_detail("/home/minch/agent_ide".into());
        render(&window);

        dispatch_click(&ui, 300.0, 520.0);
        assert_eq!(
            *activated_tab.borrow(),
            Some(51),
            "a terminal tab did not activate through the same tab strip as a file"
        );

        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(280.0, 17.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(1180.0, 300.0),
            },
        );
        render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(1180.0, 300.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            docked_tabs.borrow().last(),
            Some(&(50, 1)),
            "dragging a file tab did not request a right-side group"
        );
        assert_eq!(ui.get_workspace_layout(), 3);

        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 51,
            title: "Terminal 1 · agent_ide".into(),
            detail: "/home/minch/agent_ide".into(),
            kind: "terminal".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_primary_active_tab_id(-1);
        render(&window);
        ui.set_primary_active_tab_id(51);
        ui.set_secondary_tabs(ModelRc::new(VecModel::from(Vec::<TabEntry>::new())));
        ui.set_secondary_active_tab_id(-1);
        render(&window);
        assert!(ui.get_primary_tab_scroll_offset().abs() < 0.1);
        for x in (380..470).step_by(4) {
            dispatch_click(&ui, x as f32, 17.0);
            if !closed_tabs.borrow().is_empty() {
                break;
            }
        }
        assert_eq!(
            closed_tabs.borrow().last(),
            Some(&51),
            "the close button did not apply to a terminal tab"
        );

        // The file tree is fixed while either tab group can occupy any edge,
        // share either axis, or take over the entire workspace.
        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 50,
            title: "main.rs".into(),
            detail: "/workspace/src/main.rs".into(),
            kind: "file".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_secondary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 51,
            title: "Terminal 1 · agent_ide".into(),
            detail: "/home/minch/agent_ide".into(),
            kind: "terminal".into(),
            group: 1,
            active: true,
            dirty: false,
        }])));
        ui.set_primary_active_tab_id(50);
        ui.set_secondary_active_tab_id(51);
        ui.set_secondary_active_kind("terminal".into());
        ui.set_secondary_active_title("agent_ide".into());
        ui.set_secondary_active_detail("/home/minch/agent_ide".into());
        ui.set_workspace_layout(0);
        ui.set_panel_split_ratio(0.64);
        render(&window);
        ui.set_focused_group(1);
        ui.invoke_focus_terminal();
        assert!(
            ui.get_secondary_terminal_ime_active(),
            "the terminal in the secondary group did not receive independent IME focus"
        );
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: "둘".into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "둘".into() });
        assert_eq!(
            terminal_text.borrow().last(),
            Some(&(51, "둘".to_string())),
            "terminal text was routed to the wrong tab"
        );
        dispatch_click(&ui, 1182.0, 516.0);
        assert_eq!(
            terminal_groups.borrow().as_slice(),
            &[0, 1],
            "the terminal button ignored the focused secondary group"
        );
        ui.set_focused_group(0);
        assert!(ui.get_primary_group_visible() && ui.get_secondary_group_visible());
        assert!(
            ui.get_primary_group_y() < ui.get_secondary_group_y(),
            "default layout did not place the primary group above the secondary group"
        );

        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(300.0, 17.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(700.0, 550.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(700.0, 550.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            docked_tabs.borrow().last(),
            Some(&(50, 5)),
            "dropping a tab onto the secondary pane did not target that pane"
        );

        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(300.0, 516.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(700.0, 300.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(700.0, 300.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            docked_tabs.borrow().last(),
            Some(&(51, 4)),
            "dropping a tab onto the primary pane did not target that pane"
        );

        ui.set_workspace_layout(1);
        render(&window);
        assert_eq!(ui.get_workspace_layout(), 1);
        assert!(
            ui.get_secondary_group_y() < ui.get_primary_group_y(),
            "docking the secondary group to the top did not swap the vertical groups"
        );

        ui.set_workspace_layout(2);
        render(&window);
        assert_eq!(ui.get_workspace_layout(), 2);
        assert!(
            ui.get_secondary_group_x() < ui.get_primary_group_x(),
            "docking the secondary group left did not create a left/right split"
        );

        ui.set_workspace_layout(3);
        ui.set_panel_split_ratio(0.5);
        let split_ui = render(&window);
        write_snapshot_if_requested("split-file-terminal.png", &split_ui);
        assert_eq!(ui.get_workspace_layout(), 3);
        assert!(
            ui.get_primary_group_x() < ui.get_secondary_group_x(),
            "docking the secondary group right did not create a primary-left split"
        );
        assert!(
            (ui.get_primary_group_width() - ui.get_secondary_group_width()).abs() < 1.0,
            "a 50/50 horizontal split did not give both groups equal width"
        );

        ui.set_primary_active_kind("file".into());
        ui.set_secondary_active_kind("file".into());
        ui.set_editor_text("alpha".into());
        ui.set_secondary_editor_text("beta".into());
        render(&window);
        dispatch_click(&ui, 800.0, 82.0);
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "X".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "X".into() });
        assert_eq!(ui.get_editor_text().as_str(), "alpha");
        assert!(
            ui.get_secondary_editor_text().contains('X'),
            "editing a file in the secondary group did not update that view"
        );
        ui.set_secondary_active_kind("terminal".into());

        ui.invoke_toggle_panel_maximize(0);
        assert_eq!(ui.get_workspace_layout(), 4);
        ui.invoke_toggle_panel_maximize(0);
        render(&window);
        assert_eq!(
            ui.get_workspace_layout(),
            3,
            "restoring a maximized panel did not recover the previous arrangement"
        );

        let split_ratio_before_drag = ui.get_panel_split_ratio();
        let split_divider_x = ui.get_sidebar_width() + 6.0 + ui.get_primary_group_width();
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(split_divider_x + 2.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(split_divider_x + 126.0, 400.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(split_divider_x + 126.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(
            ui.get_panel_split_ratio() > split_ratio_before_drag + 0.1,
            "dragging the divider did not resize the horizontal split"
        );

        ui.set_workspace_layout(0);
        ui.set_panel_split_ratio(0.64);
        render(&window);
        dispatch_click(&ui, 600.0, 17.0);
        assert_eq!(
            ui.get_workspace_layout(),
            0,
            "clicking a pane tab bar without dragging changed the layout"
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(700.0, 496.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(700.0, 400.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(700.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(
            ui.get_panel_split_ratio() < 0.55,
            "dragging the divider did not resize the vertical split"
        );

        ui.set_workspace_layout(4);
        render(&window);
        assert_eq!(ui.get_workspace_layout(), 4);
        assert!(ui.get_primary_group_visible() && !ui.get_secondary_group_visible());
        assert!(
            ui.get_primary_group_width() > 900.0 && ui.get_primary_group_height() > 700.0,
            "primary-group maximize did not fill the workspace"
        );

        let activated_tree_entry = Rc::new(RefCell::new(None));
        let observed_tree_entry = activated_tree_entry.clone();
        ui.on_tree_activated(move |index| {
            *observed_tree_entry.borrow_mut() = Some(index);
        });
        dispatch_click(&ui, 20.0, 76.0);
        assert_eq!(
            *activated_tree_entry.borrow(),
            Some(0),
            "maximizing a panel covered or displaced the fixed file tree"
        );

        ui.set_workspace_layout(5);
        render(&window);
        assert_eq!(ui.get_workspace_layout(), 5);
        assert!(!ui.get_primary_group_visible() && ui.get_secondary_group_visible());
        assert!(
            ui.get_secondary_group_width() > 900.0 && ui.get_secondary_group_height() > 700.0,
            "secondary-group maximize did not fill the workspace"
        );

        // Exercise both production pane-local tab drag handlers. First drag
        // the terminal tab from the secondary pane to the top edge.
        ui.set_workspace_layout(0);
        ui.set_panel_split_ratio(0.64);
        render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(300.0, 516.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(600.0, 45.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(600.0, 45.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            ui.get_workspace_layout(),
            1,
            "dragging the secondary pane's terminal tab to the top did not dock it there"
        );

        // Then drag the editor to the right and render the live preview before
        // release so the preview path is covered by the regression Harness.
        ui.set_workspace_layout(0);
        render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(300.0, 17.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(1180.0, 300.0),
            },
        );
        render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(1180.0, 300.0),
                button: PointerEventButton::Left,
            },
        );
        assert_eq!(
            ui.get_workspace_layout(),
            3,
            "dragging the active file tab to the right did not create a right-side group"
        );

        ui.set_workspace_layout(0);
        ui.set_panel_split_ratio(0.64);
        ui.set_primary_active_kind("file".into());
        ui.set_editor_text("const selectedText = copyThisValue;\n".into());
        ui.set_highlighted_text(
            slint::StyledText::from_markdown(
                "<font color=\"#61afef\">const</font> selectedText = copyThisValue;\n",
            )
            .unwrap(),
        );
        ui.set_syntax_highlight_enabled(true);
        let before_editor_selection = render(&window);

        // The editor starts after the 250px tree and 48px line-number gutter.
        // Its pane-local tab bar occupies the first 36px of the workspace.
        // Drag across a portion of the first source line below that tab bar.
        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(320.0, 44.0),
            button: PointerEventButton::Left,
        });
        dispatch_pointer(&ui, WindowEvent::PointerMoved {
            position: LogicalPosition::new(450.0, 44.0),
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(450.0, 44.0),
            button: PointerEventButton::Left,
        });

        let (anchor, cursor) = *selection.borrow();
        assert_ne!((anchor, cursor), (0, 0), "editor did not report mouse selection");
        assert_ne!(anchor, cursor, "mouse drag did not create a selection");
        assert_eq!(
            ui.get_editor_selection_length(),
            (cursor - anchor).abs(),
            "visible selection state does not match TextInput"
        );

        let after_selection = render(&window);
        let changed_editor_pixels = before_editor_selection
            .iter()
            .zip(&after_selection)
            .enumerate()
            .filter(|(index, (before, after))| {
                let x = index % 1200;
                let y = index / 1200;
                (299..1000).contains(&x) && (36..96).contains(&y) && before != after
            })
            .count();
        assert!(
            changed_editor_pixels > 100,
            "mouse selection exists internally but is not visibly highlighted"
        );

        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: Key::Tab.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: Key::Tab.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        assert_eq!(
            cycled_tabs.borrow().last(),
            Some(&1),
            "Control+Tab did not request the next editor tab"
        );
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: Key::Shift.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: Key::Tab.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: Key::Tab.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: Key::Shift.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        assert_eq!(
            cycled_tabs.borrow().last(),
            Some(&-1),
            "Control+Shift+Tab did not request the previous editor tab"
        );

        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });

        let copied = clipboard.borrow().clone();
        assert!(!copied.is_empty(), "Control+C did not write to the clipboard");
        assert!("const selectedText = copyThisValue;\n".contains(&copied));

        clipboard.borrow_mut().clear();
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(1140.0, 46.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(1140.0, 46.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(
            !clipboard.borrow().is_empty(),
            "the visible Copy button did not copy the selected editor text"
        );
        assert!(
            ui.get_copy_feedback_active(),
            "Copy button did not switch to its copied state"
        );

        // A package-lock can be thousands of lines while still fitting under
        // the editor's 2 MiB limit. It must remain editable, but rendering the
        // generated file as tens of thousands of StyledText spans can exhaust
        // the software renderer. Exercise the production highlight policy and
        // verify that the same large plain-text document still renders.
        let mut package_lock = String::with_capacity(300_000);
        package_lock.push_str("{\n  \"packages\": {\n");
        for index in 0..7_250 {
            writeln!(
                package_lock,
                "    \"node_modules/package-{index}\": {{ \"version\": \"1.0.0\" }},"
            )
            .unwrap();
        }
        package_lock.push_str("  }\n}\n");
        assert!(package_lock.len() > 250_000);
        assert!(
            highlight::highlighted(Path::new("package-lock.json"), &package_lock).is_none(),
            "generated lock file unexpectedly entered the expensive StyledText path"
        );
        ui.set_editor_text(package_lock.into());
        ui.set_syntax_highlight_enabled(false);
        ui.set_highlighted_text(slint::StyledText::default());
        assert_eq!(render(&window).len(), 1200 * 800);

        // The first shell prompt starts on row zero. Do not jump to the bottom
        // of a taller VT screen and hide it before the user enters a command.
        ui.set_secondary_tabs(ModelRc::new(VecModel::from(Vec::<TabEntry>::new())));
        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 1,
            title: "Terminal 1 · agent_ide".into(),
            detail: "/home/minch/agent_ide".into(),
            kind: "terminal".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_secondary_active_tab_id(-1);
        ui.set_primary_active_tab_id(1);
        ui.set_primary_active_kind("terminal".into());
        ui.set_primary_active_title("agent_ide".into());
        ui.set_primary_active_detail("/home/minch/agent_ide".into());
        ui.set_focused_group(0);
        render(&window);

        // Dragging a tree entry only pastes when it is released over the
        // content of an active terminal pane. The same path is also copied to
        // the platform clipboard so a later manual paste stays consistent.
        *clipboard.borrow_mut() = "unchanged".into();
        *activated_tree_entry.borrow_mut() = None;
        let terminal_text_before_missed_drop = terminal_text.borrow().len();
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(60.0, 76.0),
            },
        );
        assert!(
            !ui.get_tree_dragging(),
            "hovering a file-tree entry unexpectedly entered drag mode"
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(60.0, 76.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(120.0, 180.0),
            },
        );
        assert!(ui.get_tree_dragging());
        assert_eq!(ui.get_tree_drop_terminal_id(), -1);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(120.0, 180.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(!ui.get_tree_dragging());
        assert_eq!(terminal_text.borrow().len(), terminal_text_before_missed_drop);
        assert_eq!(clipboard.borrow().as_str(), "unchanged");
        assert_eq!(*activated_tree_entry.borrow(), None);

        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(60.0, 76.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(600.0, 300.0),
            },
        );
        assert!(ui.get_tree_dragging());
        assert_eq!(ui.get_tree_drop_terminal_id(), 1);
        write_snapshot_if_requested("tree-terminal-drop.png", &render(&window));
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(600.0, 300.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(!ui.get_tree_dragging());
        assert_eq!(ui.get_tree_drop_terminal_id(), -1);
        assert_eq!(clipboard.borrow().as_str(), "/workspace/local-project");
        assert_eq!(
            *activated_tree_entry.borrow(),
            None,
            "dragging a tree entry also activated it as a click"
        );
        assert_eq!(
            terminal_text.borrow().last(),
            Some(&(1, "/workspace/local-project".to_string())),
            "dropping a tree path did not paste it into the target terminal"
        );
        assert_eq!(
            copied_tree_paths.borrow().last().map(String::as_str),
            Some("/workspace/local-project"),
            "terminal drop did not report the copied path"
        );

        ui.set_terminal_grid_columns(80);
        ui.set_terminal_grid_rows(40);
        ui.set_terminal_cursor_row(0);
        ui.set_terminal_cells(ModelRc::new(VecModel::from(vec![
            TerminalCell {
                row: 0,
                column: 0,
                glyph: " ".into(),
                foreground: slint::Color::from_rgb_u8(255, 255, 255),
                background: slint::Color::from_rgb_u8(180, 20, 20),
                bold: false,
                cursor: false,
                column_span: 1,
            },
            TerminalCell {
                row: 35,
                column: 70,
                glyph: " ".into(),
                foreground: slint::Color::from_rgb_u8(255, 255, 255),
                background: slint::Color::from_rgb_u8(20, 180, 20),
                bold: false,
                cursor: false,
                column_span: 1,
            },
        ])));
        ui.set_terminal_update_generation(ui.get_terminal_update_generation() + 1);
        let terminal_ui = render(&window);
        write_snapshot_if_requested("single-terminal.png", &terminal_ui);
        // A terminal that fits horizontally must paint its background all the
        // way to the status bar, without a scrollbar track along its bottom.
        // Apply the dimensions exactly as the runtime resize loop does.
        ui.set_terminal_grid_columns(ui.get_terminal_columns().round() as i32);
        ui.set_terminal_grid_rows(ui.get_terminal_rows().round() as i32);
        assert!(ui.get_terminal_columns() * 8.0 <= ui.get_primary_terminal_surface_width());
        assert!(ui.get_terminal_rows() * 16.0 <= ui.get_primary_terminal_surface_height());
        let fitted_terminal = render(&window);
        let workspace_left = (ui.get_sidebar_width() + 6.0).round() as usize;
        for y in 762..776 {
            for x in workspace_left..1200 {
                assert_eq!(fitted_terminal[y * 1200 + x],
                    TestPixel::from_rgb(0x28, 0x2c, 0x34),
                    "unexpected terminal bottom stripe at ({x}, {y})");
            }
        }

        assert!(
            ui.get_primary_terminal_surface_width() > 900.0
                && ui.get_primary_terminal_surface_height() > 650.0,
            "the terminal surface remained a small intrinsic-size window instead of filling its pane"
        );
        assert!(
            ui.get_primary_terminal_canvas_width()
                >= ui.get_primary_terminal_surface_width()
                && ui.get_primary_terminal_canvas_height()
                    >= ui.get_primary_terminal_surface_height(),
            "the terminal canvas remained centered at its intrinsic grid size"
        );
        assert!(
            ui.get_terminal_columns() > 100.0 && ui.get_terminal_rows() > 30.0,
            "the PTY size was not derived from the full terminal pane"
        );
        assert_eq!(
            ui.get_editor_selection_length(),
            0,
            "file-selection status leaked into the active terminal tab"
        );
        assert!(
            ui.get_terminal_scroll_offset().abs() < 0.1,
            "the initial terminal prompt was scrolled out of view"
        );
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(600.0, 300.0),
            delta_x: 0.0,
            delta_y: 120.0,
        });
        assert_eq!(
            terminal_scrolls.borrow().last(),
            Some(&(1, 3)),
            "mouse wheel over the terminal did not request retained output"
        );
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(600.0, 300.0),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        assert_eq!(
            terminal_scrolls.borrow().last(),
            Some(&(1, -3)),
            "downward mouse wheel over the terminal was not delivered"
        );
        let top_left_cell_pixels = terminal_ui
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                (workspace_left..workspace_left + 8).contains(&x)
                    && (36..52).contains(&y)
                    && pixel.red > 120
                    && pixel.red > pixel.green.saturating_add(60)
            })
            .count();
        let positioned_cell_pixels = terminal_ui
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                (workspace_left + 70 * 8..workspace_left + 71 * 8).contains(&x)
                    && (596..612).contains(&y)
                    && pixel.green > 120
                    && pixel.green > pixel.red.saturating_add(60)
            })
            .count();
        assert!(
            top_left_cell_pixels > 80 && positioned_cell_pixels > 80,
            "sparse terminal cells were not rendered at their explicit grid positions"
        );

        // Terminal output is made from VT cells rather than a TextInput. A
        // mouse drag must create a visible selection, and Control+C should
        // ask the production terminal callback for its text instead of
        // sending SIGINT to Codex.
        let terminal_before_selection = render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(workspace_left as f32 + 2.0, 44.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(workspace_left as f32 + 26.0, 44.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(workspace_left as f32 + 26.0, 44.0),
                button: PointerEventButton::Left,
            },
        );
        assert!(ui.get_terminal_selection_active(), "terminal drag did not create a selection");
        let terminal_after_selection = render(&window);
        assert_ne!(
            terminal_before_selection, terminal_after_selection,
            "terminal selection was not visibly highlighted"
        );
        let terminal_key_count = terminal_keys.borrow().len();
        clipboard.borrow_mut().clear();
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        assert_eq!(clipboard.borrow().as_str(), "Codex response");
        assert_eq!(terminal_keys.borrow().len(), terminal_key_count, "copy sent Control+C to Codex");
        assert_eq!(
            terminal_copy_requests.borrow().last(),
            Some(&(1, 0, 0, 0, 3)),
            "terminal copy used the wrong selected cell range"
        );

        // A stale, wider grid can remain until the asynchronous PTY resize
        // completes. It must not flash a horizontal scrollbar either.
        ui.set_terminal_grid_columns(160);
        let resizing_terminal = render(&window);
        for y in 762..776 {
            for x in workspace_left..1200 {
                assert_eq!(resizing_terminal[y * 1200 + x],
                    TestPixel::from_rgb(0x28, 0x2c, 0x34),
                    "horizontal scrollbar appeared during terminal resize at ({x}, {y})");
            }
        }
        ui.set_terminal_grid_columns(ui.get_terminal_columns().round() as i32);

        // After a long command such as `ls`, reveal the prompt on the last
        // screen row without requiring the user's first manual scroll.
        ui.set_terminal_grid_rows(60);
        ui.set_terminal_cursor_row(59);
        ui.set_terminal_update_generation(ui.get_terminal_update_generation() + 1);
        render(&window);
        assert!(
            ui.get_terminal_scroll_offset() < 0.0,
            "long terminal output did not automatically reveal the prompt row"
        );

        // Return to a compact prompt for deterministic IME pixel assertions.
        // Scroll-following itself is covered immediately above.
        ui.set_terminal_grid_rows(ui.get_terminal_rows().round() as i32);
        ui.set_terminal_cursor_row(0);
        ui.set_terminal_cursor_column(0);
        ui.set_terminal_cells(ModelRc::new(VecModel::from(vec![TerminalCell {
            row: 0,
            column: 0,
            glyph: " ".into(),
            foreground: slint::Color::from_rgb_u8(255, 255, 255),
            background: slint::Color::from_rgb_u8(0x28, 0x2c, 0x34),
            bold: false,
            cursor: true,
            column_span: 1,
        }])));
        ui.set_terminal_update_generation(ui.get_terminal_update_generation() + 1);
        render(&window);
        assert!(ui.get_terminal_cells().row_data(0).unwrap().cursor);
        assert_eq!(ui.get_terminal_scroll_offset(), 0.0);

        // The terminal must focus an editable TextInput so the Windows backend
        // enables IME. A committed Hangul string is then forwarded once and
        // removed from the proxy buffer instead of being rendered twice.
        let terminal_before_focus = render(&window);
        ui.invoke_focus_terminal();
        assert!(
            ui.get_terminal_ime_active(),
            "terminal focus did not activate its IME-capable TextInput"
        );
        let focused_terminal = render(&window);
        assert_eq!(
            terminal_before_focus, focused_terminal,
            "focusing the invisible IME proxy painted over the Codex input surface"
        );
        // The minimal Linux renderer does not have a Hangul fallback font,
        // so use Latin preedit text to verify the undecorated preview pixels.
        dispatch_preedit(&ui, "h");
        assert_eq!(ui.get_terminal_ime_preedit().as_str(), "h");
        let initial_preedit = render(&window);
        let changed_preedit_pixels = focused_terminal
            .iter()
            .zip(&initial_preedit)
            .enumerate()
            .filter_map(|(index, (before, after))| (before != after).then_some(index))
            .collect::<Vec<_>>();
        assert!(
            changed_preedit_pixels.len() > 5,
            "the first composition character was not painted"
        );
        assert!(
            ui.get_terminal_ime_width() >= 8.0 && ui.get_terminal_ime_height() >= 16.0,
            "the IME proxy was clipped too tightly to show Hangul composition"
        );
        assert!(
            ui.get_terminal_ime_width() <= 32.0,
            "the IME proxy can paint a line beyond the composed glyphs"
        );
        assert!(
            changed_preedit_pixels
                .iter()
                .all(|index| index % 1200 < workspace_left + 32),
            "the IME preview painted outside its bounded composition area"
        );
        dispatch_preedit(&ui, "ha");
        assert_ne!(
            initial_preedit,
            render(&window),
            "the composition preview did not update"
        );
        dispatch_preedit(&ui, "ㅎ");
        assert_eq!(ui.get_terminal_ime_preedit().as_str(), "ㅎ");
        dispatch_preedit(&ui, "하");
        assert_eq!(ui.get_terminal_ime_preedit().as_str(), "하");
        dispatch_commit(&ui, "한글");
        assert_eq!(
            terminal_text.borrow().last(),
            Some(&(1, "한글".to_string())),
            "primary terminal text was routed to the wrong tab"
        );
        assert_eq!(ui.get_terminal_ime_buffer().as_str(), "");
        assert_eq!(ui.get_terminal_ime_preedit().as_str(), "");
        *clipboard.borrow_mut() = "/workspace/local-project".into();
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "v".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "v".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        assert_eq!(
            terminal_text.borrow().last(),
            Some(&(1, "/workspace/local-project".to_string())),
            "Control+V did not paste a copied file-tree path into the terminal"
        );
        assert_eq!(
            focused_terminal,
            render(&window),
            "committed terminal input left an extra line over the Codex input surface"
        );
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Return.into() });
        assert_eq!(
            terminal_keys.borrow().last().map(|event| (event.0, event.1.as_str())),
            Some((1, "<ENTER>"))
        );
        ui.set_status_text("Ready".into());
        render(&window);
        dispatch_click(&ui, 1128.0, 788.0);
        assert!(ui.get_font_menu_visible(), "Aa did not open the font appearance menu");
        assert_eq!(
            (
                ui.get_terminal_font_brightness(),
                ui.get_editor_font_brightness(),
                ui.get_tree_font_brightness(),
            ),
            (120, 120, 120),
            "default brightness did not match the brighter Orca preset",
        );
        write_snapshot_if_requested("font-menu.png", &render(&window));
        dispatch_click(&ui, 1012.0, 590.0);
        assert_eq!(ui.get_terminal_font_size(), 15, "terminal size control did not apply");
        dispatch_click(&ui, 1012.0, 638.0);
        assert_eq!(ui.get_editor_font_size(), 15, "editor size control did not apply");
        dispatch_click(&ui, 1012.0, 687.0);
        assert_eq!(ui.get_tree_font_size(), 13, "file list size control did not apply");
        dispatch_click(&ui, 1130.0, 590.0);
        dispatch_click(&ui, 1130.0, 638.0);
        dispatch_click(&ui, 1130.0, 687.0);
        assert_eq!(
            (
                ui.get_terminal_font_brightness(),
                ui.get_editor_font_brightness(),
                ui.get_tree_font_brightness(),
            ),
            (121, 121, 121),
            "brightness controls did not apply independently",
        );
        dispatch_click(&ui, 892.0, 737.0);
        assert_eq!(
            (
                ui.get_terminal_font_size(),
                ui.get_editor_font_size(),
                ui.get_tree_font_size(),
                ui.get_terminal_font_brightness(),
                ui.get_editor_font_brightness(),
                ui.get_tree_font_brightness(),
            ),
            (14, 14, 12, 120, 120, 120),
            "Reset did not restore Orca font defaults",
        );
        render(&window);
        let original_columns = ui.get_terminal_columns();
        let original_rows = ui.get_terminal_rows();
        let original_editor_size = ui.get_editor_font_size();
        let original_tree_size = ui.get_tree_font_size();
        ui.set_terminal_font_size(20);
        render(&window);
        assert!(ui.get_terminal_columns() < original_columns);
        assert!(ui.get_terminal_rows() < original_rows);
        assert_eq!(ui.get_editor_font_size(), original_editor_size);
        assert_eq!(ui.get_tree_font_size(), original_tree_size);
        ui.set_editor_font_size(18);
        ui.set_tree_font_size(16);
        render(&window);
        assert_eq!(ui.get_terminal_font_size(), 20);
        ui.set_primary_active_kind("file".into());
        ui.set_editor_text("Brightness sample text".into());
        ui.set_syntax_highlight_enabled(false);
        ui.set_editor_font_brightness(50);
        let dim_editor = render(&window);
        ui.set_editor_font_brightness(150);
        let bright_editor = render(&window);
        let editor_luminance = |pixels: &[TestPixel]| -> u64 {
            pixels
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    let x = index % 1200;
                    let y = index / 1200;
                    (300..800).contains(&x) && (40..80).contains(&y)
                })
                .map(|(_, pixel)| pixel.red as u64 + pixel.green as u64 + pixel.blue as u64)
                .sum()
        };
        assert!(
            editor_luminance(&bright_editor) > editor_luminance(&dim_editor),
            "file viewer brightness did not affect rendered text",
        );
        dispatch_click(&ui, 600.0, 400.0);
        assert!(!ui.get_font_menu_visible(), "clicking outside did not dismiss font appearance");

        dispatch_click(&ui, 188.0, 50.0);
        assert_eq!(
            ui.get_sidebar_view(),
            1,
            "GIT did not replace the FILES view"
        );
        render(&window);
        dispatch_click(&ui, 90.0, 125.0);
        assert_eq!(
            toggled_git_repositories.borrow().last().map(String::as_str),
            Some("/workspace/local-project"),
            "clicking a collapsed project did not request expansion",
        );
        assert!(
            opened_git_changes.borrow().is_empty(),
            "a collapsed project row was confused with a changed file",
        );
        ui.set_git_entries(ModelRc::new(VecModel::from(vec![
            GitEntry {
                kind: "repository".into(),
                name: "local-project".into(),
                detail: "/workspace/local-project".into(),
                path: "/workspace/local-project".into(),
                status: "".into(),
                count: 1,
                icon: slint::Image::default(),
                expanded: true,
            },
            GitEntry {
                kind: "change".into(),
                name: "sample.js".into(),
                detail: "src".into(),
                path: "/workspace/local-project/src/sample.js".into(),
                status: "M".into(),
                count: 0,
                icon: emoji::EmojiIcons::load_system().get("⚡"),
                expanded: false,
            },
        ])));
        render(&window);
        dispatch_click(&ui, 90.0, 165.0);
        assert_eq!(
            opened_git_changes.borrow().last().map(String::as_str),
            Some("/workspace/local-project/src/sample.js"),
            "clicking a Git change did not request its diff",
        );
        dispatch_click(&ui, 210.0, 165.0);
        assert!(
            ui.get_discard_confirm_visible(),
            "discard did not require confirmation"
        );
        dispatch_click(&ui, 748.0, 451.0);
        assert_eq!(
            discarded_git_changes.borrow().last().map(String::as_str),
            Some("/workspace/local-project/src/sample.js"),
            "confirmed discard was not forwarded",
        );

        ui.set_workspace_layout(0);
        ui.set_secondary_tabs(ModelRc::new(VecModel::from(Vec::<TabEntry>::new())));
        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 77,
            title: "sample.js (Working Tree)".into(),
            detail: "/workspace/local-project/src/sample.js".into(),
            kind: "diff".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_primary_active_tab_id(77);
        ui.set_primary_active_kind("diff".into());
        ui.set_diff_old_title("src/sample.js (HEAD)".into());
        ui.set_diff_new_title("src/sample.js (Working Tree)".into());
        let long_old_diff_line = "  return <button className=\"small-button\" type=\"button\" onClick={copy} disabled={!value}>{labels.copy}</button>";
        let long_new_diff_line = "  return <Tooltip label={labels.copy}><Button className=\"copy-button\" variant=\"ghost\" size=\"icon\" onClick={copy} disabled={!value}>{labels.copy}</Button></Tooltip>";
        ui.set_diff_rows(ModelRc::new(VecModel::from(vec![
            DiffRow {
                old_line: "1".into(),
                new_line: "1".into(),
                old_text: "const value = 1;".into(),
                new_text: "const value = 1;".into(),
                old_highlighted: highlight::highlighted(Path::new("sample.js"), "const value = 1;").unwrap(),
                new_highlighted: highlight::highlighted(Path::new("sample.js"), "const value = 1;").unwrap(),
                old_highlighted_enabled: true,
                new_highlighted_enabled: true,
                old_kind: "context".into(),
                new_kind: "context".into(),
            },
            DiffRow {
                old_line: "2".into(),
                new_line: "2".into(),
                old_text: "const oldName = value;".into(),
                new_text: "const newName = value;".into(),
                old_highlighted: highlight::highlighted(Path::new("sample.js"), "const oldName = value;").unwrap(),
                new_highlighted: highlight::highlighted(Path::new("sample.js"), "const newName = value;").unwrap(),
                old_highlighted_enabled: true,
                new_highlighted_enabled: true,
                old_kind: "removed".into(),
                new_kind: "added".into(),
            },
            DiffRow {
                old_line: "".into(),
                new_line: "3".into(),
                old_text: "".into(),
                new_text: "console.log(newName);".into(),
                old_highlighted: slint::StyledText::default(),
                new_highlighted: highlight::highlighted(Path::new("sample.js"), "console.log(newName);").unwrap(),
                old_highlighted_enabled: false,
                new_highlighted_enabled: true,
                old_kind: "empty".into(),
                new_kind: "added".into(),
            },
            DiffRow {
                old_line: "4".into(),
                new_line: "4".into(),
                old_text: long_old_diff_line.into(),
                new_text: long_new_diff_line.into(),
                old_highlighted: highlight::highlighted(
                    Path::new("sample.jsx"),
                    long_old_diff_line,
                )
                .unwrap(),
                new_highlighted: highlight::highlighted(
                    Path::new("sample.jsx"),
                    long_new_diff_line,
                )
                .unwrap(),
                old_highlighted_enabled: true,
                new_highlighted_enabled: true,
                old_kind: "removed".into(),
                new_kind: "added".into(),
            },
            DiffRow {
                old_line: "5".into(),
                new_line: "5".into(),
                old_text: "afterLongLine();".into(),
                new_text: "afterLongLine();".into(),
                old_highlighted: highlight::highlighted(
                    Path::new("sample.jsx"),
                    "afterLongLine();",
                )
                .unwrap(),
                new_highlighted: highlight::highlighted(
                    Path::new("sample.jsx"),
                    "afterLongLine();",
                )
                .unwrap(),
                old_highlighted_enabled: true,
                new_highlighted_enabled: true,
                old_kind: "context".into(),
                new_kind: "context".into(),
            },
        ])));
        let diff_view = render(&window);
        write_snapshot_if_requested("git-diff-view.png", &diff_view);
        let diff_row_height = (ui.get_editor_font_size() as f32 * 1.5)
            .max(21.0)
            .round() as usize;
        let long_line_top = 70 + 3 * diff_row_height;
        let long_line_start_pixels = diff_view
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                (workspace_left + 57..workspace_left + 140).contains(&x)
                    && (long_line_top..long_line_top + diff_row_height).contains(&y)
                    && pixel.blue > pixel.red.saturating_add(40)
                    && pixel.blue > pixel.green.saturating_add(15)
            })
            .count();
        assert!(
            long_line_start_pixels > 5,
            "a long highlighted diff line wrapped and displayed its middle instead of its start"
        );
        let (removed_pixels, added_pixels) = diff_view
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                let x = index % 1200;
                let y = index / 1200;
                (250..1200).contains(&x) && (88..138).contains(&y)
            })
            .fold((0usize, 0usize), |(removed, added), (_, pixel)| {
                (
                    removed + usize::from(pixel.red > pixel.green.saturating_add(20)),
                    added + usize::from(pixel.green > pixel.red.saturating_add(12)),
                )
            });
        assert!(
            removed_pixels > 100,
            "diff view did not render VS Code-style removed rows"
        );
        assert!(
            added_pixels > 100,
            "diff view did not render VS Code-style added rows"
        );
        let syntax_pixels = diff_view
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 1200;
                let y = index / 1200;
                (297..1180).contains(&x)
                    && (70..125).contains(&y)
                    && pixel.blue > pixel.red.saturating_add(40)
                    && pixel.blue > pixel.green.saturating_add(20)
            })
            .count();
        assert!(
            syntax_pixels > 10,
            "diff view did not render file-viewer syntax colors"
        );

        // The production window can render a nested third pane and route its
        // input through the same WorkspaceGroup implementation.
        ui.set_dynamic_panes(true);
        ui.set_primary_layout_x(0.0);
        ui.set_primary_layout_y(0.0);
        ui.set_primary_layout_width(0.5);
        ui.set_primary_layout_height(1.0);
        ui.set_primary_layout_visible(true);
        ui.set_primary_tabs(ModelRc::new(VecModel::from(vec![TabEntry {
            id: 50,
            title: "main.rs".into(),
            detail: "/workspace/main.rs".into(),
            kind: "file".into(),
            group: 0,
            active: true,
            dirty: false,
        }])));
        ui.set_primary_active_tab_id(50);
        ui.set_secondary_layout_x(0.5);
        ui.set_secondary_layout_y(0.0);
        ui.set_secondary_layout_width(0.5);
        ui.set_secondary_layout_height(0.5);
        ui.set_secondary_layout_visible(true);
        ui.set_primary_active_kind("file".into());
        ui.set_secondary_active_kind("file".into());
        ui.set_extra_panes(ModelRc::new(VecModel::from(vec![PaneEntry {
            group: 2,
            x: 0.5,
            y: 0.5,
            width: 0.5,
            height: 0.5,
            visible: true,
            tabs: ModelRc::new(VecModel::from(vec![TabEntry {
                id: 52,
                title: "Terminal 2".into(),
                detail: "/workspace".into(),
                kind: "terminal".into(),
                group: 2,
                active: true,
                dirty: false,
            }])),
            active_tab_id: 52,
            active_kind: "terminal".into(),
            active_title: "Terminal 2".into(),
            ..PaneEntry::default()
        }])));
        ui.set_pane_dividers(ModelRc::new(VecModel::from(vec![
            DividerEntry { id: 0, horizontal: true, x: 0.5, y: 0.0, width: 0.0, height: 1.0 },
            DividerEntry { id: 1, horizontal: false, x: 0.5, y: 0.5, width: 0.5, height: 0.0 },
        ])));
        let nested_view = render(&window);
        write_snapshot_if_requested("nested-panes.png", &nested_view);
        assert!(ui.get_secondary_group_x() > ui.get_primary_group_x());
        assert!((ui.get_primary_group_width() - ui.get_workspace_area_width() * 0.5).abs() < 1.0);

        let nested_dock = Rc::new(RefCell::new(None));
        let observed_dock = nested_dock.clone();
        ui.on_pane_dock_requested(move |tab, group, zone| {
            *observed_dock.borrow_mut() = Some((tab, group, zone));
        });
        ui.on_dock_target_requested(|_, _, _| DockTarget {
            group: 2, zone: 3, x: 0.5, y: 0.75, width: 0.5, height: 0.25,
        });
        dispatch_pointer(&ui, WindowEvent::PointerPressed {
            position: LogicalPosition::new(300.0, 17.0),
            button: PointerEventButton::Left,
        });
        dispatch_pointer(&ui, WindowEvent::PointerMoved {
            position: LogicalPosition::new(850.0, 600.0),
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(850.0, 600.0),
            button: PointerEventButton::Left,
        });
        assert_eq!(*nested_dock.borrow(), Some((50, 2, 3)));

        ui.set_focused_group(2);
        ui.invoke_focus_terminal();
        render(&window);
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "확장".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "확장".into() });
        assert_eq!(terminal_text.borrow().last(), Some(&(52, "확장".to_string())));

        let panes = ui.get_extra_panes();
        let model = panes.as_any().downcast_ref::<VecModel<PaneEntry>>().unwrap();
        let mut third = model.row_data(0).unwrap();
        third.active_tab_id = 53;
        third.active_kind = "file".into();
        third.editor_text = "gamma".into();
        third.tabs = ModelRc::new(VecModel::from(vec![TabEntry {
            id: 53,
            title: "third.rs".into(),
            detail: "/workspace/third.rs".into(),
            kind: "file".into(),
            group: 2,
            active: true,
            dirty: false,
        }]));
        model.set_row_data(0, third);
        render(&window);
        dispatch_click(&ui, 800.0, 450.0);
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "X".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "X".into() });
        assert!(model.row_data(0).unwrap().editor_text.contains('X'));
        assert_eq!(ui.get_editor_text().as_str(), "Brightness sample text");

        ui.set_workspace_layout(0);
        ui.set_primary_active_kind("image".into());
        ui.set_primary_preview_natural_width(16.0);
        ui.set_primary_preview_natural_height(16.0);
        let mut red = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(16, 16);
        for pixel in red.make_mut_slice() {
            *pixel = slint::Rgba8Pixel::new(255, 0, 0, 255);
        }
        ui.set_primary_preview_image(slint::Image::from_rgba8(red));
        let image_pixels = render(&window);
        assert!(image_pixels.iter().filter(|pixel| pixel.red > 180 && pixel.green < 80).count() > 100);
        assert_eq!(ui.get_primary_preview_displayed_width(), 16.0);
        ui.set_primary_preview_natural_width(1198.0);
        ui.set_primary_preview_natural_height(807.0);
        assert!(ui.get_primary_preview_displayed_width() < 1198.0);
        ui.set_primary_preview_zoom(100);
        assert_eq!(ui.get_primary_preview_displayed_width(), 1198.0);
        ui.set_primary_preview_zoom(200);
        assert_eq!(ui.get_primary_preview_displayed_width(), 2396.0);

        ui.set_primary_active_kind("pdf".into());
        ui.set_primary_preview_zoom(100);
        let pdf_events = Rc::new(RefCell::new(Vec::new()));
        let recorded_events = pdf_events.clone();
        ui.on_pdf_selection_event(move |tab, x, y, phase| {
            recorded_events.borrow_mut().push((tab, x, y, phase));
        });
        ui.on_pdf_copy_requested(|_| "Hello PDF".into());
        let pdf_pixels = render(&window);
        let pdf_hit = pdf_pixels.iter().enumerate().find_map(|(index, pixel)| {
            (pixel.red > 180 && pixel.green < 80 && pixel.blue < 80)
                .then_some(LogicalPosition::new((index % 1200) as f32, (index / 1200) as f32))
        }).expect("PDF image should be visible");
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: pdf_hit,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(pdf_hit.x + 30.0, pdf_hit.y + 10.0),
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(pdf_hit.x + 30.0, pdf_hit.y + 10.0),
            button: PointerEventButton::Left,
        });
        assert!(pdf_events.borrow().iter().any(|event| event.3 == 0), "PDF pointer missed at {pdf_hit:?}");
        assert!(pdf_events.borrow().iter().any(|event| event.3 == 2));
        let (_, page_x, page_y, _) = pdf_events.borrow()[0];
        ui.set_primary_pdf_selection(ModelRc::new(VecModel::from(vec![PdfSelectionRect {
            x: page_x, y: page_y, width: 40.0, height: 20.0,
        }])));
        let selected_pixels = render(&window);
        let highlight_pixel = (pdf_hit.y as usize + 5) * 1200 + pdf_hit.x as usize + 5;
        assert_ne!(pdf_pixels[highlight_pixel], selected_pixels[highlight_pixel],
            "PDF selection band should visibly cover the line");
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "c".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
        assert_eq!(clipboard.borrow().as_str(), "Hello PDF");

        ui.set_primary_active_kind("markdown".into());
        ui.set_primary_preview_blocks(ModelRc::new(VecModel::from(vec![
            PreviewBlock {
                kind: "h1".into(),
                text: slint::StyledText::from_plain_text("Preview heading"),
            },
            PreviewBlock {
                kind: "paragraph".into(),
                text: slint::StyledText::from_markdown("**Bold content**").unwrap(),
            },
        ])));
        let markdown_pixels = render(&window);
        assert_ne!(image_pixels, markdown_pixels, "Markdown preview did not replace the image viewer");
    }

    fn dispatch_pointer(ui: &AppWindow, event: WindowEvent) {
        ui.window().dispatch_event(event);
    }

    fn dispatch_preedit(ui: &AppWindow, text: &str) {
        WindowInner::from_pub(ui.window()).process_key_input(InternalKeyEvent {
            event_type: KeyEventType::UpdateComposition,
            preedit_text: text.into(),
            preedit_selection: Some(text.len() as i32..text.len() as i32),
            ..Default::default()
        });
    }

    fn dispatch_commit(ui: &AppWindow, text: &str) {
        let mut key_event = InternalKeyEventData::default();
        key_event.text = text.into();
        WindowInner::from_pub(ui.window()).process_key_input(InternalKeyEvent {
            event_type: KeyEventType::CommitComposition,
            key_event,
            ..Default::default()
        });
    }

    fn write_snapshot_if_requested(name: &str, pixels: &[TestPixel]) {
        let Ok(directory) = std::env::var("ARASEO_UI_SNAPSHOT_DIR") else {
            return;
        };
        let directory = Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        let width = 1200u32;
        let height = 800u32;
        let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
        for row in pixels.chunks_exact(width as usize) {
            raw.push(0);
            for pixel in row {
                raw.extend_from_slice(&[pixel.red, pixel.green, pixel.blue]);
            }
        }

        let mut zlib = vec![0x78, 0x01];
        let mut remaining = raw.as_slice();
        while !remaining.is_empty() {
            let block_length = remaining.len().min(u16::MAX as usize);
            let final_block = block_length == remaining.len();
            zlib.push(u8::from(final_block));
            let length = block_length as u16;
            zlib.extend_from_slice(&length.to_le_bytes());
            zlib.extend_from_slice(&(!length).to_le_bytes());
            zlib.extend_from_slice(&remaining[..block_length]);
            remaining = &remaining[block_length..];
        }
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        append_png_chunk(&mut png, b"IHDR", &header);
        append_png_chunk(&mut png, b"IDAT", &zlib);
        append_png_chunk(&mut png, b"IEND", &[]);
        std::fs::write(directory.join(name), png).unwrap();
    }

    fn adler32(bytes: &[u8]) -> u32 {
        let mut a = 1u32;
        let mut b = 0u32;
        for byte in bytes {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    fn append_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn dispatch_click(ui: &AppWindow, x: f32, y: f32) {
        dispatch_pointer(
            ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            },
        );
    }

    fn render(window: &Rc<MinimalSoftwareWindow>) -> Vec<TestPixel> {
        let mut pixels = vec![TestPixel::default(); 1200 * 800];
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 1200);
        });
        pixels
    }
}
