//! Skips repaints while the window is dragged, which otherwise make dragging choppy.
//! See https://github.com/rust-windowing/winit/issues/4708

use eframe::egui;
use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, WM_EXITSIZEMOVE, WM_MOVING, WM_NCDESTROY, WM_PAINT,
};

struct State {
    ctx: egui::Context,
    moving: Cell<bool>,
}

pub fn install_move_handler(cc: &eframe::CreationContext) {
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;
    let state = Box::into_raw(Box::new(State {
        ctx: cc.egui_ctx.clone(),
        moving: Cell::new(false),
    }));
    // SAFETY: runs on the window's thread; `state` is freed in `subclass` on WM_NCDESTROY.
    unsafe {
        if SetWindowSubclass(hwnd, Some(subclass), 0, state as usize) == 0 {
            drop(Box::from_raw(state));
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let state = data as *mut State;
    // SAFETY: `data` is the `State` installed above, alive until WM_NCDESTROY.
    unsafe {
        match msg {
            WM_MOVING => (*state).moving.set(true),
            // Validate the paint without letting winit render a frame.
            WM_PAINT if (*state).moving.get() => return DefWindowProcW(hwnd, msg, wparam, lparam),
            WM_EXITSIZEMOVE if (*state).moving.replace(false) => (*state).ctx.request_repaint(),
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(subclass), id);
                drop(Box::from_raw(state));
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}
