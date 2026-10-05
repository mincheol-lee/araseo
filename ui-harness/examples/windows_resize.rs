#![cfg_attr(windows, windows_subsystem = "windows")]
// Native Windows integration fixture using the production Slint UI and renderer.
#[cfg(windows)]
#[path = "../../src/window_resize.rs"]
mod window_resize;

#[cfg(windows)]
fn main() -> Result<(), slint::PlatformError> {
    use slint::ComponentHandle;
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
    let ui = araseo_ui_harness::AppWindow::new()?;
    ui.set_status_text("Native Windows resize verification".into());
    let weak = ui.as_weak();
    let refresh = window_resize::ResizeRefresh::default();
    ui.window().on_winit_window_event(move |_, event| {
        match event {
            winit::event::WindowEvent::Resized(size) if size.width == 0 || size.height == 0 => {
                refresh.cancel()
            }
            winit::event::WindowEvent::Resized(_)
            | winit::event::WindowEvent::ScaleFactorChanged { .. } => {
                let weak = weak.clone();
                refresh.request(move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.window().request_redraw();
                    }
                });
            }
            _ => {}
        }
        EventResult::Propagate
    });
    ui.run()
}
#[cfg(not(windows))]
fn main() {}
