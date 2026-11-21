use slint::winit_030::{WinitWindowAccessor, winit};

pub fn graphics_description(api: &slint::GraphicsAPI<'_>) -> String {
    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
        let address = get_proc_address(c"glGetString");
        if address.is_null() || address as usize <= 3 || address as usize == usize::MAX {
            return "OpenGL (driver details unavailable)".into();
        }
        type GetString = unsafe extern "system" fn(u32) -> *const std::ffi::c_char;
        let get_string: GetString = unsafe { std::mem::transmute(address) };
        let string = |parameter| {
            let value = unsafe { get_string(parameter) };
            if value.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(value) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        return format!(
            "OpenGL {}; vendor={}; renderer={}",
            string(0x1F02),
            string(0x1F00),
            string(0x1F01)
        );
    }
    format!("{api:?}")
}

pub fn show_startup_error(message: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let text: Vec<u16> = message
            .chars()
            .take(1800)
            .collect::<String>()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "Reword — помилка запуску"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("Reword: {message}");
}

// Called at process entry, before Slint or the hotkey library starts threads.
pub fn prefer_integrated_gpu() {
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        unsafe {
            std::env::set_var("WGPU_POWER_PREF", "low");
        }
    }
}

pub fn keep_non_interactive(window: &slint::Window) {
    window.with_winit_window(|native| {
        let _ = native.set_cursor_hittest(false);
        #[cfg(windows)]
        no_activate(native);
    });
}

pub fn resident_description(window: &slint::Window) -> String {
    window
        .with_winit_window(|native| {
            let description = format!("{:?}", native.id());
            #[cfg(windows)] {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TRANSPARENT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_APPWINDOW};
                if let Ok(handle) = native.window_handle() {
                    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                        let style = unsafe { GetWindowLongPtrW(handle.hwnd.get() as *mut std::ffi::c_void, GWL_EXSTYLE) };
                        return format!("{description}; click-through={}; no-activate={}; excluded-from-AltTab={}", style & WS_EX_TRANSPARENT as isize != 0, style & WS_EX_NOACTIVATE as isize != 0, style & WS_EX_TOOLWINDOW as isize != 0 && style & WS_EX_APPWINDOW as isize == 0);
                    }
                }
            }
            description
        })
        .unwrap_or_else(|| "window not created".into())
}

pub fn window_attributes(
    attributes: winit::window::WindowAttributes,
    overlay: bool,
) -> winit::window::WindowAttributes {
    if !overlay {
        return attributes;
    }
    let attributes = attributes
        .with_active(false)
        .with_transparent(true)
        .with_decorations(false)
        .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        return attributes.with_skip_taskbar(true);
    }
    #[cfg(not(windows))]
    attributes
}

pub fn setup_overlay(window: &slint::Window) {
    window.on_winit_window_event(|window, event| {
        #[cfg(windows)]
        window.with_winit_window(no_activate);
        if matches!(
            event,
            winit::event::WindowEvent::Resized(_)
                | winit::event::WindowEvent::ScaleFactorChanged { .. }
        ) {
            keep_non_interactive(window);
            position_overlay(window);
        }
        slint::winit_030::EventResult::Propagate
    });
}

pub fn position_overlay(window: &slint::Window) {
    window.with_winit_window(position_native);
}

fn position_native(native: &winit::window::Window) {
    #[cfg(windows)]
    no_activate(native);
    if let Some(monitor) = native
        .current_monitor()
        .or_else(|| native.primary_monitor())
    {
        let origin = monitor.position();
        let screen = monitor.size();
        let size = native.outer_size();
        let scale = monitor.scale_factor();
        let x = origin.x + (screen.width as i32 - size.width as i32) / 2;
        let y = origin.y + screen.height as i32 - size.height as i32 - (66.0 * scale) as i32;
        native.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }
}

#[cfg(windows)]
fn no_activate(window: &winit::window::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };
    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
            let hwnd = handle.hwnd.get() as *mut std::ffi::c_void;
            // The window remains visible over the user's editor without taking
            // its keyboard focus. This is application behavior, not input injection.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let desired = (style | WS_EX_NOACTIVATE as isize | WS_EX_TOOLWINDOW as isize)
                    & !(WS_EX_APPWINDOW as isize);
                if style != desired {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired);
                }
            }
        }
    }
}

#[cfg(windows)]
pub fn hotkey_held() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
    unsafe {
        GetAsyncKeyState(VK_CONTROL as i32) < 0
            && GetAsyncKeyState(VK_MENU as i32) < 0
            && GetAsyncKeyState(0x48) < 0
    }
}
