//! Tests mount the production GPUI panels, input handlers and dock containers.
#[cfg(test)]
mod tests {
    use araseo::{
        document::Document,
        gpui_panel::{Content, Opened, WorkspacePanel},
        session::{SavedKind, SavedTab},
    };
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        self as gpui, AppContext, Bounds, Entity, EntityInputHandler, Focusable, TestAppContext,
        WindowBounds, WindowOptions, px, size,
    };
    use std::path::PathBuf;

    #[gpui_kit::test]
    fn agent_logos_follow_status_in_the_production_dock(cx: &mut TestAppContext) {
        use araseo::terminal::{AgentKind, AgentStatus};
        use gpui_kit::component::dock::{DockLayout, DockSkin, panel_handle};
        cx.update(gpui_kit::init);
        let (handle, panel) = cx.update(|cx| {
            let mut panel = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let terminal = cx.new(|cx| {
                    let mut panel = WorkspacePanel::new(
                        SavedTab {
                            id: 70,
                            path: "/work".into(),
                            kind: SavedKind::Terminal { number: 1 },
                        },
                        14.,
                        cx,
                    );
                    panel.set_agent(
                        Some(AgentStatus {
                            kind: AgentKind::Claude,
                            running: false,
                        }),
                        cx,
                    );
                    panel
                });
                let (area, _) = DockSkin::dock_area("agent-dock", None, window, cx);
                area.update(cx, |area, cx| {
                    area.set_center(
                        DockLayout::tabs().panel_view(panel_handle(terminal.clone()), cx),
                        window,
                        cx,
                    )
                });
                panel = Some(terminal);
                area
            })
            .unwrap();
            (handle, panel.unwrap())
        });
        for (kind, running, label) in [
            (AgentKind::Claude, false, "Claude waiting"),
            (AgentKind::Claude, true, "Claude running"),
            (AgentKind::Codex, true, "Codex running"),
            (AgentKind::Codex, false, "Codex waiting"),
        ] {
            cx.update_window(handle, |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.set_agent(Some(AgentStatus { kind, running }), cx)
                });
                window.render_frame(cx);
                let icon = window.find("agent-tab-icon");
                assert_eq!(icon.label(), Some(label));
                assert_eq!(icon.bounds().size, size(px(16.), px(16.)));
            })
            .unwrap();
        }
        cx.update_window(handle, |_, window, cx| {
            panel.update(cx, |panel, cx| panel.set_agent(None, cx));
            window.render_frame(cx);
            assert!(window.try_find("agent-tab-icon").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn agent_rotation_is_bounded_and_does_not_render_cached_content(cx: &mut TestAppContext) {
        use araseo::{
            gpui_agent_icon::AgentTabIcon,
            terminal::{AgentKind, AgentStatus},
        };
        use gpui_kit::{prelude::*, *};
        use std::{cell::Cell, rc::Rc, time::Duration};
        struct ContentProbe(Rc<Cell<usize>>);
        impl Render for ContentProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.set(self.0.get() + 1);
                div().size_full().child("Terminal content")
            }
        }
        struct Host {
            icon: Entity<AgentTabIcon>,
            content: Entity<ContentProbe>,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.icon.clone()).child(
                    self.content
                        .clone()
                        .cached(StyleRefinement::default().size(px(100.))),
                )
            }
        }
        cx.update(gpui_kit::init);
        let renders = Rc::new(Cell::new(0));
        let notifications = Rc::new(Cell::new(0));
        let (handle, icon, _subscription) = cx.update(|cx| {
            let icon = cx.new(AgentTabIcon::new);
            icon.update(cx, |icon, cx| {
                icon.set_status(
                    Some(AgentStatus {
                        kind: AgentKind::Claude,
                        running: true,
                    }),
                    cx,
                )
            });
            let subscription = cx.observe(&icon, {
                let notifications = notifications.clone();
                move |_, _| notifications.set(notifications.get() + 1)
            });
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                window.activate_window();
                cx.new(|cx| Host {
                    icon: icon.clone(),
                    content: cx.new(|_| ContentProbe(renders.clone())),
                })
            })
            .unwrap();
            (handle, icon, subscription)
        });
        cx.run_until_parked();
        let content_renders = renders.get();
        let initial = notifications.get();
        for _ in 0..3 {
            cx.executor().advance_clock(Duration::from_millis(105));
            cx.run_until_parked();
        }
        assert_eq!(
            notifications.get() - initial,
            3,
            "rotation should tick at about 10 Hz"
        );
        assert_eq!(
            renders.get(),
            content_renders,
            "rotation must reuse cached terminal content"
        );
        cx.update(|cx| {
            icon.update(cx, |icon, cx| {
                icon.set_status(
                    Some(AgentStatus {
                        kind: AgentKind::Claude,
                        running: false,
                    }),
                    cx,
                )
            })
        });
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        let stopped = notifications.get();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            stopped,
            "waiting must not schedule more frames"
        );
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            icon.update(cx, |icon, cx| {
                icon.set_status(
                    Some(AgentStatus {
                        kind: AgentKind::Codex,
                        running: true,
                    }),
                    cx,
                )
            });
        });
        cx.run_until_parked();
        let reduced = notifications.get();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            reduced,
            "reduced motion must stay static"
        );
        cx.update(|cx| cx.set_reduce_motion(false));
        cx.run_until_parked();
        gpui_kit::VisualTestContext::from_window(handle, cx).deactivate_window();
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        let inactive = notifications.get();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            inactive,
            "inactive windows must stop rotation"
        );
        cx.update_window(handle, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(105));
        cx.run_until_parked();
        assert!(
            notifications.get() > inactive,
            "activation must resume working rotation"
        );
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("agent-tab-icon").label(), Some("Codex running"))
        })
        .unwrap();
    }

    fn file() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "araseo-gpui-editor-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "original\n").unwrap();
        path
    }
    fn panel(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Entity<WorkspacePanel>, PathBuf) {
        let path = file();
        let document = Document::open(path.clone(), path.clone()).unwrap();
        cx.update(gpui_kit::init);
        let (handle, panel) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut panel = WorkspacePanel::new(
                            SavedTab {
                                id: 1,
                                path: path.clone(),
                                kind: SavedKind::File {
                                    unsaved_text: None,
                                    revision: None,
                                },
                            },
                            14.,
                            cx,
                        );
                        panel.finish(Opened::File(document), window, cx);
                        panel
                    })
                },
            )
            .unwrap()
        });
        (handle, panel, path)
    }
    #[gpui_kit::test]
    fn production_editor_handles_unicode_undo_and_redo(cx: &mut TestAppContext) {
        let (handle, panel, path) = panel(cx);
        cx.update_window(handle, |_, window, cx| {
            panel.focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
            window.input("한글😀", cx);
            let Content::File { editor, .. } = &panel.read(cx).content else {
                panic!()
            };
            let editor = editor.clone();
            assert!(editor.read(cx).value().contains("한글😀"));
            window.press("ctrl-z", cx);
            assert!(!editor.read(cx).value().contains("한글😀"));
            window.press("ctrl-y", cx);
            assert!(editor.read(cx).value().contains("한글😀"));
        })
        .unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[gpui_kit::test]
    fn production_editor_composition_stays_uncommitted_until_commit(cx: &mut TestAppContext) {
        let (handle, panel, path) = panel(cx);
        cx.update_window(handle, |_, window, cx| {
            let Content::File { editor, .. } = &panel.read(cx).content else {
                panic!()
            };
            let editor = editor.clone();
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(Some(0..0), "ㅎ", None, window, cx);
                editor.replace_and_mark_text_in_range(None, "한", None, window, cx);
                assert_eq!(editor.marked_text_range(window, cx), Some(0..1));
                editor.replace_text_in_range(None, "한", window, cx);
                assert_eq!(editor.marked_text_range(window, cx), None);
                assert_eq!(editor.value(), "한original\n");
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.update(|cx| {
            let Content::File { document, .. } = &panel.read(cx).content else {
                panic!()
            };
            assert_eq!(document.text, "한original\n");
            assert!(document.dirty);
        });
        std::fs::remove_file(path).unwrap();
    }
    #[gpui_kit::test]
    fn docking_a_production_editor_keeps_its_entity_buffer_and_input(cx: &mut TestAppContext) {
        use gpui_kit::component::dock::{
            DockLayout, DockPlacement, DockSkin, InsertTarget, PaneRef, PanelId, panel_handle,
        };
        let path = file();
        cx.update(gpui_kit::init);
        let (handle, (area, first, second)) = cx.update(|cx| {
            let mut entities = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let first = cx.new(|cx| {
                    let mut panel = WorkspacePanel::new(
                        SavedTab {
                            id: 1,
                            path: path.clone(),
                            kind: SavedKind::File {
                                unsaved_text: None,
                                revision: None,
                            },
                        },
                        14.,
                        cx,
                    );
                    panel.finish(
                        Opened::File(Document::open(path.clone(), path.clone()).unwrap()),
                        window,
                        cx,
                    );
                    panel
                });
                let second = cx.new(|cx| {
                    WorkspacePanel::new(
                        SavedTab {
                            id: 2,
                            path: path.clone(),
                            kind: SavedKind::File {
                                unsaved_text: None,
                                revision: None,
                            },
                        },
                        14.,
                        cx,
                    )
                });
                let (area, _) = DockSkin::dock_area("test-dock", None, window, cx);
                area.update(cx, |area, cx| {
                    area.set_center(
                        DockLayout::tabs()
                            .panel_view(panel_handle(first.clone()), cx)
                            .panel_view(panel_handle(second.clone()), cx),
                        window,
                        cx,
                    )
                });
                entities = Some((area.clone(), first, second));
                area
            })
            .unwrap();
            (handle, entities.unwrap())
        });
        cx.update_window(handle, |_, window, cx| {
            first.focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
            window.input("before", cx);
            let before = if let Content::File { editor, .. } = &first.read(cx).content {
                editor.clone()
            } else {
                panic!()
            };
            area.update(cx, |area, cx| {
                let node = area
                    .layout(DockPlacement::Center)
                    .unwrap()
                    .find_panel_node(PanelId::from(second.entity_id()))
                    .unwrap();
                area.move_panel(
                    PanelId::from(first.entity_id()),
                    InsertTarget::Split {
                        node,
                        placement: gpui::base::Placement::Bottom,
                        size: None,
                    },
                    window,
                    cx,
                );
                assert!(matches!(
                    area.layout(DockPlacement::Center).unwrap().root().kind(),
                    PaneRef::Split { .. }
                ));
            });
            window.render_frame(cx);
            window.input("after한", cx);
            let Content::File { editor, .. } = &first.read(cx).content else {
                panic!()
            };
            assert_eq!(editor.entity_id(), before.entity_id());
            assert!(editor.read(cx).value().contains("beforeafter한"));
        })
        .unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[gpui_kit::test]
    fn terminal_native_input_and_ime_send_only_committed_text(cx: &mut TestAppContext) {
        use araseo::{gpui_terminal::TerminalView, terminal::TerminalSession};
        use std::time::Duration;
        let mut session = TerminalSession::spawn("Ubuntu", &std::env::temp_dir()).unwrap();
        let input = session.capture_input().unwrap();
        cx.update(gpui_kit::init);
        let (handle, terminal) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| TerminalView::new(session, 14., 120, cx))
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            terminal.focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
            window.input("a", cx);
            window.input("b", cx);
            assert_eq!(input.recv_timeout(Duration::from_secs(2)).unwrap(), b"a");
            assert_eq!(input.recv_timeout(Duration::from_secs(2)).unwrap(), b"b");
            terminal.update(cx, |terminal, cx| {
                terminal.replace_and_mark_text_in_range(None, "ㅎ", None, window, cx);
                terminal.replace_and_mark_text_in_range(None, "한", None, window, cx);
                assert!(input.try_recv().is_err());
                assert_eq!(terminal.marked_text_range(window, cx), Some(0..1));
                terminal.replace_text_in_range(None, "한", window, cx);
                assert_eq!(terminal.marked_text_range(window, cx), None);
            });
            assert_eq!(
                input.recv_timeout(Duration::from_secs(2)).unwrap(),
                "한".as_bytes()
            );
            assert!(input.try_recv().is_err());
            window.render_frame(cx);
            window.input("c", cx);
            assert_eq!(input.recv_timeout(Duration::from_secs(2)).unwrap(), b"c");
        })
        .unwrap();
    }
    #[gpui_kit::test]
    async fn save_button_keeps_the_latest_native_edit(cx: &mut TestAppContext) {
        use gpui_kit::test::TestAppContextExt;
        let (handle, panel, path) = panel(cx);
        cx.update_window(handle, |_, window, cx| {
            panel.focus_handle(cx).focus(window, cx);
            window.render_frame(cx);
            window.input("saved 한글 ", cx);
            window.click("save", cx);
        })
        .unwrap();
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, cx| {
            !panel.read(cx).dirty
                && std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("saved 한글 ")
        })
        .await;
        std::fs::remove_file(path).unwrap();
    }
    #[gpui_kit::test]
    fn image_fit_keeps_full_resolution_pixels(cx: &mut TestAppContext) {
        use araseo::{
            gpui_panel::PreparedPreview,
            preview::{ImagePixels, LoadedPreview, PreviewKind},
        };
        cx.update(gpui_kit::init);
        let loaded = LoadedPreview {
            linux_path: "/image.png".into(),
            pixels: Some(ImagePixels {
                width: 1200,
                height: 600,
                rgba: vec![255; 1200 * 600 * 4],
                premultiplied: false,
            }),
            kind: PreviewKind::Image,
            pdf_bytes: None,
            page_count: 0,
            first_page: None,
        };
        let prepared = PreparedPreview::new(loaded);
        let (handle, panel) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(640.), px(480.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut panel = WorkspacePanel::new(
                            SavedTab {
                                id: 3,
                                path: "/image.png".into(),
                                kind: SavedKind::Preview,
                            },
                            14.,
                            cx,
                        );
                        panel.finish(Opened::Preview(prepared), window, cx);
                        panel
                    })
                },
            )
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let bounds = panel.read(cx).measured_preview_bounds();
            assert!(bounds.size.width > px(0.) && bounds.size.width <= px(640.));
            assert!(bounds.size.height > px(0.) && bounds.size.height <= px(480.));
            let Content::Preview { image, .. } = &panel.read(cx).content else {
                panic!()
            };
            let pixels = image.size(0);
            assert_eq!(pixels.width.0, 1200);
            assert_eq!(pixels.height.0, 600);
        })
        .unwrap();
    }
    #[gpui_kit::test]
    async fn session_restore_returns_focus_after_other_visible_files_finish(
        cx: &mut TestAppContext,
    ) {
        use araseo::{
            gpui_app::Workbench,
            session::Session,
            tabs::{Dock, TabGroups},
            workspace::Workspace,
        };
        use gpui_kit::test::TestAppContextExt;
        let original_first = file();
        let original_second = file();
        let root = original_first.with_extension("workspace");
        std::fs::create_dir(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::rename(original_first, &first).unwrap();
        std::fs::rename(original_second, &second).unwrap();
        let mut groups = TabGroups::default();
        groups.add(10, 0);
        groups.add(20, 0);
        groups.dock_into(20, 0, Dock::Right);
        groups.activate(10);
        let saved = Session::new(
            "Ubuntu".into(),
            root.clone(),
            vec![
                SavedTab {
                    id: 10,
                    path: first.clone(),
                    kind: SavedKind::File {
                        unsaved_text: None,
                        revision: None,
                    },
                },
                SavedTab {
                    id: 20,
                    path: second.clone(),
                    kind: SavedKind::File {
                        unsaved_text: None,
                        revision: None,
                    },
                },
            ],
            groups,
        );
        cx.update(gpui_kit::init);
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        Workspace::prepare("Ubuntu", root.clone()).unwrap(),
                        Some(saved),
                        None,
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update(|cx| {
            workbench.update(cx, |workbench, _| {
                workbench.disable_session_persistence_for_test()
            })
        });
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, cx| {
            workbench.read(cx).session_restore_complete()
                && workbench.read(cx).focused_tab() == Some(10)
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("caption-row").bounds().top(), px(0.));
            assert_eq!(
                window.find("window-close").bounds().right(),
                window.viewport_size().width
            );
        })
        .unwrap();
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[gpui_kit::test]
    fn legacy_zoomed_pane_restores(cx: &mut TestAppContext) {
        use araseo::{
            gpui_app::Workbench, session::Session, tabs::TabGroups, workspace::Workspace,
        };
        let path = file();
        let original = path.clone();
        let root = path.with_extension("zoom-workspace");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("file.txt");
        std::fs::rename(original, &path).unwrap();
        let mut groups = TabGroups::default();
        groups.add(4, 0);
        let mut saved = Session::new(
            "Ubuntu".into(),
            root.clone(),
            vec![SavedTab {
                id: 4,
                path: path.clone(),
                kind: SavedKind::File {
                    unsaved_text: None,
                    revision: None,
                },
            }],
            groups,
        );
        saved.maximized_group = Some(0);
        cx.update(gpui_kit::init);
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        Workspace::prepare("Ubuntu", root.clone()).unwrap(),
                        Some(saved),
                        None,
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            workbench.update(cx, |workbench, _| {
                workbench.disable_session_persistence_for_test()
            });
            assert!(workbench.read(cx).pane_is_zoomed(cx));
            assert_eq!(window.find("caption-row").bounds().top(), px(0.));
            assert_eq!(
                window.find("window-close").bounds().right(),
                window.viewport_size().width
            );
        })
        .unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[gpui_kit::test]
    async fn compact_header_icons_create_entries_and_keep_caption_above_content(
        cx: &mut TestAppContext,
    ) {
        use araseo::{
            gpui_app::Workbench, session::Session, tabs::TabGroups, workspace::Workspace,
        };
        use gpui_kit::test::TestAppContextExt;
        let original = file();
        let root = original.with_extension("header-workspace");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("original.txt");
        std::fs::rename(original, &path).unwrap();
        let mut groups = TabGroups::default();
        groups.add(10, 0);
        let mut saved = Session::new(
            "Ubuntu".into(),
            root.clone(),
            vec![SavedTab {
                id: 10,
                path: path.clone(),
                kind: SavedKind::File {
                    unsaved_text: None,
                    revision: None,
                },
            }],
            groups,
        );
        saved.sidebar_width = 150.;
        cx.update(gpui_kit::init);
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut workbench = Workbench::new(
                            Workspace::prepare("Ubuntu", root.clone()).unwrap(),
                            Some(saved),
                            None,
                            window,
                            cx,
                        );
                        workbench.disable_session_persistence_for_test();
                        workbench
                    })
                },
            )
            .unwrap()
        });
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, cx| {
            workbench.read(cx).session_restore_complete()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let caption = window.find("caption-row").bounds();
            assert_eq!(caption.top(), px(0.));
            assert_eq!(caption.size.height, px(34.));
            let close = window.find("window-close").bounds();
            assert_eq!(close.right(), px(900.));
            for id in [
                "new-terminal",
                "window-minimize",
                "window-maximize",
                "window-close",
            ] {
                assert!(caption.contains(&window.find(id).bounds().center()), "{id}");
            }
            for id in [
                "files-view",
                "new-file",
                "new-folder",
                "add-project",
                "git-view",
            ] {
                let bounds = window.find(id).bounds();
                assert!(
                    bounds.left() >= px(0.) && bounds.right() <= px(150.),
                    "{id}: {bounds:?}"
                );
            }
            window.click("new-file", cx);
            window.input("created.txt", cx);
            window.click("confirm-dialog", cx);
        })
        .unwrap();
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, cx| {
            root.join("created.txt").is_file()
                && workbench.read(cx).focused_tab().is_some_and(|id| id != 10)
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.click("new-folder", cx);
            window.input("created-folder", cx);
            window.click("confirm-dialog", cx);
        })
        .unwrap();
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, _| {
            root.join("created-folder").is_dir()
        })
        .await;
        let previous_tab = cx
            .update_window(handle, |_, window, cx| {
                window.click("add-project", cx);
                assert!(window.find("browse-folder").visible());
                window.click("cancel-dialog", cx);
                let previous = workbench.read(cx).focused_tab();
                window.click("new-terminal", cx);
                previous
            })
            .unwrap();
        cx.wait_for(handle, std::time::Duration::from_secs(3), |_, cx| {
            workbench.read(cx).focused_tab().is_some()
                && workbench.read(cx).focused_tab() != previous_tab
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("caption-row").bounds().top(), px(0.));
            window.click("window-close", cx);
        })
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[gpui_kit::test]
    fn production_diff_fits_both_columns_and_wraps_after_resize(cx: &mut TestAppContext) {
        use araseo::{git::FileDiff, tree::GitStatus};
        use gpui_kit::component::diff::{DiffLineRange, DiffSide};
        let old = "let message = \"한국어 repeated text\"; ".repeat(8);
        let new = "let message = \"한글 modified text\"; ".repeat(10);
        let patch = format!(
            "diff --git a/src/long.rs b/src/long.rs\n--- a/src/long.rs\n+++ b/src/long.rs\n@@ -1,2 +1,2 @@\n-{old}\n+{new}\n tail\n"
        );
        cx.update(gpui_kit::init);
        let (handle, panel) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1000.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut panel = WorkspacePanel::new(
                            SavedTab {
                                id: 8,
                                path: "/work/src/long.rs".into(),
                                kind: SavedKind::Diff,
                            },
                            14.,
                            cx,
                        );
                        panel.finish(
                            Opened::Diff(FileDiff {
                                repository: "/work".into(),
                                path: "/work/src/long.rs".into(),
                                status: GitStatus::Modified,
                                lines: vec![],
                                patch,
                            }),
                            window,
                            cx,
                        );
                        panel
                    })
                },
            )
            .unwrap()
        });
        let mut heights = Vec::new();
        for width in [1000., 420., 760.] {
            cx.simulate_window_resize(handle, size(px(width), px(600.)));
            // Flush native paint notifications between frames. Do not force
            // refresh: the panel must observe viewport/presentation changes.
            for _ in 0..3 {
                cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
                    .unwrap();
            }
            cx.update_window(handle, |_, window, cx| {
                let left = window.find("diff-original-heading").bounds();
                let right = window.find("diff-modified-heading").bounds();
                assert!((f32::from(left.size.width) - width / 2.).abs() < 1.);
                assert!((f32::from(right.left()) - width / 2.).abs() < 1.);
                let old_source = window
                    .within(("diff-file", 0usize))
                    .within(("old", 0usize))
                    .find("source")
                    .bounds();
                let new_source = window
                    .within(("diff-file", 0usize))
                    .within(("new", 0usize))
                    .find("source")
                    .bounds();
                assert!(
                    old_source.left() >= left.left() && old_source.right() <= left.right() + px(1.)
                );
                assert!(
                    new_source.left() >= right.left() - px(1.)
                        && new_source.right() <= right.right() + px(1.)
                );
                heights.push(old_source.size.height);
                let Content::Diff(state) = &panel.read(cx).content else {
                    panic!("missing production diff")
                };
                state.clone().update(cx, |state, cx| {
                    state.set_selected_lines(
                        Some(DiffLineRange::new("src/long.rs", DiffSide::Modified, 1, 1)),
                        cx,
                    );
                    assert_eq!(state.selected_text(cx), format!("{new}\n"));
                });
            })
            .unwrap();
        }
        assert!(
            heights[1] > heights[0],
            "narrow view should wrap further: {heights:?}"
        );
        assert!(
            heights[2] < heights[1],
            "growing should reflow again: {heights:?}"
        );
    }
    #[gpui_kit::test]
    async fn production_git_sidebar_groups_repositories_and_opens_the_selected_diff(
        cx: &mut TestAppContext,
    ) {
        use araseo::{
            gpui_app::Workbench, session::Session, tabs::TabGroups, workspace::Workspace,
        };
        use gpui_kit::test::TestAppContextExt;
        let original = file();
        let root = original.with_extension("git-workspace");
        std::fs::remove_file(original).unwrap();
        for name in ["alpha", "beta"] {
            let repo = root.join(name);
            std::fs::create_dir_all(repo.join("src")).unwrap();
            assert!(
                std::process::Command::new("git")
                    .arg("init")
                    .arg("--quiet")
                    .arg(&repo)
                    .status()
                    .unwrap()
                    .success()
            );
            std::fs::write(
                repo.join("src/main.rs"),
                format!("{}\n", "let message = \"한글 source text\"; ".repeat(10)),
            )
            .unwrap();
        }
        std::fs::write(root.join("alpha/root.txt"), "root file\n").unwrap();
        let path = root.join("alpha/src/main.rs");
        let mut groups = TabGroups::default();
        groups.add(10, 0);
        let saved = Session::new(
            "Ubuntu".into(),
            root.clone(),
            vec![SavedTab {
                id: 10,
                path,
                kind: SavedKind::File {
                    unsaved_text: None,
                    revision: None,
                },
            }],
            groups,
        );
        cx.update(gpui_kit::init);
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workbench = Workbench::new(
                        Workspace::prepare("Ubuntu", root.clone()).unwrap(),
                        Some(saved),
                        None,
                        window,
                        cx,
                    );
                    workbench.disable_session_persistence_for_test();
                    workbench
                })
            })
            .unwrap()
        });
        cx.wait_for(handle, std::time::Duration::from_secs(5), |_, cx| {
            workbench.read(cx).git_group_summary()
                == vec![(root.join("alpha"), 2), (root.join("beta"), 1)]
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("git-view", cx);
            assert!(window.find(("git-repository", 0usize)).visible());
            assert!(window.find(("git-repository", 3usize)).visible());
            window.click(("git-repository", 0usize), cx);
            assert!(window.find(("git-repository", 1usize)).visible());
            assert!(window.try_find(("git-change", 1usize)).is_none());
            window.click(("git-repository", 0usize), cx);
            window.click(("git-change", 2usize), cx);
        })
        .unwrap();
        cx.wait_for(handle, std::time::Duration::from_secs(3), |window, cx| {
            workbench.read(cx).focused_tab().is_some_and(|id| id != 10)
                && window.try_find("diff-modified-heading").is_some()
        })
        .await;
        // Exercise the dock's cached panel, not just a diff used as a root view.
        let mut heights = Vec::new();
        for width in [1250., 850., 1100.] {
            cx.simulate_window_resize(handle, size(px(width), px(650.)));
            for _ in 0..4 {
                cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
                    .unwrap();
            }
            cx.update_window(handle, |_, window, _| {
                let left = window.find("diff-original-heading").bounds();
                let right = window.find("diff-modified-heading").bounds();
                assert!((left.size.width - right.size.width).abs() < px(1.));
                let source = window
                    .within(("diff-file", 0usize))
                    .within(("new", 0usize))
                    .find("source")
                    .bounds();
                assert!(source.left() >= right.left() - px(1.));
                assert!(source.right() <= right.right() + px(1.));
                heights.push(source.size.height);
            })
            .unwrap();
        }
        assert!(
            heights[1] > heights[0],
            "docked diff must wrap: {heights:?}"
        );
        assert!(
            heights[2] < heights[1],
            "docked diff must reflow: {heights:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
