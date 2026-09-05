slint::include_modules!();

#[cfg(test)]
#[path = "../../src/emoji.rs"]
mod emoji;

#[cfg(test)]
#[path = "../../src/highlight.rs"]
mod highlight;

#[cfg(test)]
mod tests {
    use super::*;
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
                (80..106).contains(&x)
                    && (4..31).contains(&y)
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
                (80..106).contains(&x)
                    && (4..31).contains(&y)
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
            icon: emoji::EmojiIcons::load_system().get("🧩"),
            icon_label: "🧩".into(),
            depth: 0,
            is_directory: true,
            is_expanded: false,
            git_mark: "".into(),
            project_kind: "local".into(),
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
            "the editor surface used only part of the initial pane"
        );

        // Orca-style tabs belong to their pane, below the window title bar.
        // The file tree must remain a separate fixed sidebar.
        dispatch_click(&ui, 150.0, 51.0);
        assert_eq!(
            *activated_tab.borrow(),
            None,
            "a file tab overlaps the FILES sidebar"
        );

        assert_eq!(
            ui.get_tree_entries().row_data(0).unwrap().icon_label.as_str(),
            "🧩",
            "context-aware project emoji was not delivered to the tree UI"
        );
        let colored_emoji_pixels = populated_ui
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
            colored_emoji_pixels > 5,
            "tree emoji reached the UI but was not rendered in color"
        );

        let before_close_hover = render(&window);
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(340.0, 51.0),
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
                (320..360).contains(&x) && (34..70).contains(&y) && before != after
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
        ui.set_primary_active_tab_id(0);
        render(&window);
        assert!(
            ui.get_primary_tab_scroll_offset().abs() < 0.1,
            "activating the first tab did not scroll it back into view"
        );

        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(500.0, 51.0),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        assert!(
            ui.get_primary_tab_scroll_offset() < 0.0,
            "mouse wheel over the tab strip did not scroll hidden tabs into reach"
        );

        dispatch_click(&ui, 1146.0, 51.0);
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
                position: LogicalPosition::new(280.0, 51.0),
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
            dispatch_click(&ui, x as f32, 51.0);
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
        dispatch_click(&ui, 1146.0, 528.0);
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
                position: LogicalPosition::new(300.0, 51.0),
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
                position: LogicalPosition::new(300.0, 528.0),
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
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(724.0, 400.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(850.0, 400.0),
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(850.0, 400.0),
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
        dispatch_click(&ui, 600.0, 48.0);
        assert_eq!(
            ui.get_workspace_layout(),
            0,
            "clicking a pane tab bar without dragging changed the layout"
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(700.0, 508.0),
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
                position: LogicalPosition::new(300.0, 528.0),
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
                position: LogicalPosition::new(300.0, 51.0),
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
            position: LogicalPosition::new(320.0, 78.0),
            button: PointerEventButton::Left,
        });
        dispatch_pointer(&ui, WindowEvent::PointerMoved {
            position: LogicalPosition::new(450.0, 78.0),
        });
        dispatch_pointer(&ui, WindowEvent::PointerReleased {
            position: LogicalPosition::new(450.0, 78.0),
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
                (299..1000).contains(&x) && (62..130).contains(&y) && before != after
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
                position: LogicalPosition::new(1140.0, 80.0),
                button: PointerEventButton::Left,
            },
        );
        dispatch_pointer(
            &ui,
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(1140.0, 80.0),
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
        ui.set_terminal_grid_columns(80);
        ui.set_terminal_grid_rows(40);
        ui.set_terminal_cursor_row(0);
        ui.set_terminal_update_generation(ui.get_terminal_update_generation() + 1);
        let terminal_ui = render(&window);
        write_snapshot_if_requested("single-terminal.png", &terminal_ui);
        assert!(
            ui.get_primary_terminal_surface_width() > 900.0
                && ui.get_primary_terminal_surface_height() > 650.0,
            "the terminal surface remained a small intrinsic-size window instead of filling its pane"
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

        // After a long command such as `ls`, reveal the prompt on the last
        // screen row without requiring the user's first manual scroll.
        ui.set_terminal_cursor_row(39);
        ui.set_terminal_update_generation(ui.get_terminal_update_generation() + 1);
        render(&window);
        assert!(
            ui.get_terminal_scroll_offset() < 0.0,
            "long terminal output did not automatically reveal the prompt row"
        );

        // The terminal must focus an editable TextInput so the Windows backend
        // enables IME. A committed Hangul string is then forwarded once and
        // removed from the proxy buffer instead of being rendered twice.
        ui.invoke_focus_terminal();
        assert!(
            ui.get_terminal_ime_active(),
            "terminal focus did not activate its IME-capable TextInput"
        );
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: "한글".into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: "한글".into() });
        assert_eq!(
            terminal_text.borrow().last(),
            Some(&(1, "한글".to_string())),
            "primary terminal text was routed to the wrong tab"
        );
        assert_eq!(ui.get_terminal_ime_buffer().as_str(), "");
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Return.into() });
        assert_eq!(
            terminal_keys.borrow().last().map(|event| (event.0, event.1.as_str())),
            Some((1, "<ENTER>"))
        );
    }

    fn dispatch_pointer(ui: &AppWindow, event: WindowEvent) {
        ui.window().dispatch_event(event);
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
