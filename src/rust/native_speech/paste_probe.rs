//! Pasteboard promise used to learn whether an external paste was consumed.
//!
//! A synthetic Cmd+V can be posted successfully and still be ignored by the
//! target (P-2026-2291).  Instead of copying the transcript eagerly, the owner
//! publishes a lazily provided string.  AppKit asks this process for the data
//! only when some reader actually pastes, so the callback is the first real
//! "the target took the text" signal on the external path.
//!
//! The item is also marked `org.nspasteboard.TransientType`; clipboard
//! managers that follow the nspasteboard.org convention (Relearn included)
//! skip transient items, so their eager reads cannot fake a consumption.
//!
//! AppKit delivers provider callbacks on the main thread run loop.  Callers
//! must wait from a worker thread; the Tauri main thread keeps that run loop
//! alive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";
pub const PLAIN_TEXT_TYPE: &str = "public.utf8-plain-text";

struct PromiseState {
    text: String,
    consumed: AtomicBool,
}

fn active_promise() -> &'static Mutex<Option<Arc<PromiseState>>> {
    static ACTIVE: OnceLock<Mutex<Option<Arc<PromiseState>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Handle for one published promise.  Dropping it stops answering callbacks
/// for this text, so a later paste can never be served a stale transcript.
pub struct PastePromise {
    state: Arc<PromiseState>,
}

impl PastePromise {
    pub fn is_consumed(&self) -> bool {
        self.state.consumed.load(Ordering::SeqCst)
    }

    /// Wait until a reader requested the text or `timeout` elapses.
    pub fn wait_consumed(&self, timeout: Duration) -> Option<Duration> {
        let started = Instant::now();
        loop {
            if self.is_consumed() {
                return Some(started.elapsed());
            }
            if started.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

impl Drop for PastePromise {
    fn drop(&mut self) {
        if let Ok(mut active) = active_promise().lock() {
            if active
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.state))
            {
                *active = None;
            }
        }
    }
}

fn take_text_for_provider() -> Option<String> {
    let active = active_promise().lock().ok()?;
    let state = active.as_ref()?;
    state.consumed.store(true, Ordering::SeqCst);
    Some(state.text.clone())
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSArray, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Protocol, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    extern "C" fn provide_data(
        _this: &Object,
        _cmd: Sel,
        _pasteboard: id,
        item: id,
        data_type: id,
    ) {
        let Some(text) = take_text_for_provider() else {
            return;
        };
        unsafe {
            let value = NSString::alloc(nil).init_str(&text);
            let _: bool = msg_send![item, setString: value forType: data_type];
            let _: () = msg_send![value, release];
        }
    }

    extern "C" fn finished_with_provider(_this: &Object, _cmd: Sel, _pasteboard: id) {}

    fn provider_class() -> &'static Class {
        static CLASS: OnceLock<usize> = OnceLock::new();
        let ptr = *CLASS.get_or_init(|| {
            let superclass = class!(NSObject);
            let mut decl = ClassDecl::new("IterateSpeechPastePromiseProvider", superclass)
                .expect("paste promise provider class is declared once");
            if let Some(protocol) = Protocol::get("NSPasteboardItemDataProvider") {
                decl.add_protocol(protocol);
            }
            unsafe {
                decl.add_method(
                    sel!(pasteboard:item:provideDataForType:),
                    provide_data as extern "C" fn(&Object, Sel, id, id, id),
                );
                decl.add_method(
                    sel!(pasteboardFinishedWithDataProvider:),
                    finished_with_provider as extern "C" fn(&Object, Sel, id),
                );
            }
            decl.register() as *const Class as usize
        });
        unsafe { &*(ptr as *const Class) }
    }

    /// One provider instance for the life of the process; the pasteboard
    /// item only holds a weak-ish reference, so it must never be freed.
    fn provider_instance() -> id {
        static INSTANCE: OnceLock<usize> = OnceLock::new();
        *INSTANCE.get_or_init(|| unsafe {
            let instance: id = msg_send![provider_class(), new];
            instance as usize
        }) as id
    }

    pub fn publish(text: &str) -> Result<PastePromise, String> {
        let state = Arc::new(PromiseState {
            text: text.to_string(),
            consumed: AtomicBool::new(false),
        });
        *active_promise()
            .lock()
            .map_err(|_| "paste promise state is poisoned".to_string())? = Some(state.clone());
        let promise = PastePromise { state };

        unsafe {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard == nil {
                return Err("获取 NSPasteboard 失败".to_string());
            }
            let plain = NSString::alloc(nil).init_str(PLAIN_TEXT_TYPE);
            let transient = NSString::alloc(nil).init_str(TRANSIENT_TYPE);
            let empty = NSString::alloc(nil).init_str("");

            let item: id = msg_send![class!(NSPasteboardItem), new];
            let promised_types = NSArray::arrayWithObject(nil, plain);
            let provider_set: bool =
                msg_send![item, setDataProvider: provider_instance() forTypes: promised_types];
            let marker_set: bool = msg_send![item, setString: empty forType: transient];

            let _: i64 = msg_send![pasteboard, clearContents];
            let objects = NSArray::arrayWithObject(nil, item);
            let written: bool = msg_send![pasteboard, writeObjects: objects];

            let _: () = msg_send![item, release];
            let _: () = msg_send![plain, release];
            let _: () = msg_send![transient, release];
            let _: () = msg_send![empty, release];

            if !(provider_set && marker_set && written) {
                return Err(format!(
                    "paste promise write failed: provider={provider_set} marker={marker_set} written={written}"
                ));
            }
        }
        Ok(promise)
    }
}

#[cfg(target_os = "macos")]
pub use imp::publish;

#[cfg(not(target_os = "macos"))]
pub fn publish(_text: &str) -> Result<PastePromise, String> {
    Err("paste promise is only implemented on macOS".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(text: &str) -> PastePromise {
        let state = Arc::new(PromiseState {
            text: text.to_string(),
            consumed: AtomicBool::new(false),
        });
        *active_promise().lock().unwrap() = Some(state.clone());
        PastePromise { state }
    }

    // One test: the active promise is process-global state.
    #[test]
    fn provider_serves_only_the_active_promise_and_wait_reports_reads() {
        let first = install("first");
        let second = install("second");
        assert_eq!(take_text_for_provider().as_deref(), Some("second"));
        assert!(second.is_consumed());
        assert!(!first.is_consumed());
        drop(first);
        assert_eq!(take_text_for_provider().as_deref(), Some("second"));
        drop(second);
        assert_eq!(take_text_for_provider(), None);

        let promise = install("unread");
        assert_eq!(promise.wait_consumed(Duration::from_millis(40)), None);
        take_text_for_provider();
        assert!(promise.wait_consumed(Duration::from_millis(40)).is_some());
    }
}
