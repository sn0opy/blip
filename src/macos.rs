//! Opening documents from Finder. macOS doesn't pass them on the command line but sends them
//! to the app delegate's `application:openURLs:`, which winit's delegate doesn't implement.
//! Without it AppKit reports that the app "cannot open files of this type".

use eframe::egui;
use objc2::ffi::class_addMethod;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSApplication, NSApplicationWillFinishLaunchingNotification};
use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSURL};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Adds `application:openURLs:` to winit's app delegate. Must run before the event loop starts:
/// documents a launch was asked to open are delivered right after the delegate is told the app
/// will finish launching, and winit only creates the delegate class when the event loop is built.
pub fn install_open_handler() {
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
            return;
        };
        let delegate: &AnyObject = (*delegate).as_ref();
        let class: *const AnyClass = delegate.class();
        let imp: OpenUrls = open_urls;
        // SAFETY: `open_urls` matches the selector's signature, described by "v@:@@" (void
        // return, self, _cmd, two objects). An existing implementation is left untouched.
        unsafe {
            class_addMethod(
                class.cast_mut(),
                sel!(application:openURLs:),
                std::mem::transmute::<OpenUrls, Imp>(imp),
                c"v@:@@".as_ptr(),
            );
        }
    });
    // SAFETY: the block runs on the main thread, where AppKit posts the notification.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
            None,
            &block,
        )
    };
    // The observer stays registered for the life of the process.
    std::mem::forget(observer);
}

/// Lets documents opened while blip is running wake up the UI.
pub fn set_context(ctx: egui::Context) {
    let _ = CTX.set(ctx);
}

/// The most recent document Finder asked to open, if any arrived since the last call.
pub fn take_opened() -> Option<PathBuf> {
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    let last = opened.pop();
    opened.clear();
    last
}

type OpenUrls = unsafe extern "C-unwind" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>);

unsafe extern "C-unwind" fn open_urls(
    _this: &AnyObject,
    _cmd: Sel,
    _app: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    // With several files selected in Finder the first one is shown, like a drop.
    let Some(first) = urls.iter().find_map(|u| u.to_file_path()) else {
        return;
    };
    OPENED.lock().unwrap_or_else(|e| e.into_inner()).push(first);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}
